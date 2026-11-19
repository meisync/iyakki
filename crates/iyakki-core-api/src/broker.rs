//! The broker: the one place that decides whether an app may use a capability, and writes the access log
//! (docs/development/CORE-ACCESS-MODEL.md sections 3, 5, 6 and 6b).
//!
//! Pure logic with seams (approvals, the per-use card, the log, the clock are traits), so it builds without libc and is meant to compile for
//! wasm32. The local implementations (a file of approvals, a file log, the on-screen card) live in `iyakki-agent`.
//!
//! The rules, in order:
//! 1. A name outside the catalogue is refused (`unknown_capability`).
//! 2. A built-in app holds what `builtin_holds` says; a store app holds what its stored [`Approval`] holds. Otherwise `not_granted`, or
//!    `revoked` when the person took it away.
//! 3. A hard ask also needs a per-use card with Allow, whatever was granted: `card_declined` when the person pressed Deny, `no_card` when
//!    there is no screen to ask on. Never silently allowed.
//! 4. Every decision, allowed or refused, is written to the access log. The log holds no content, no paths and no pictures: only who, what, which unit, when.
//!
//! The error codes are stable strings ([`Denial::code`]); tests and remote clients compare them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::appcaps::{builtin_holds, is_hard_ask, lookup, Approval, ApprovalBook};

/// How the broker found out who is calling. For a store app this comes from the connection's credentials (slice 3), never from what the app says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallerKind {
    /// A default app of the OS, trusted by the signed table.
    Builtin,
    /// A store app: its installation key in the approval book (for example `tile/user/alice/notes`).
    Store { installation: String },
    /// A client device (a browser page, a window program, a phone) enrolled with a one-time link, speaking for a signed-in person. It is
    /// an untrusted app: it holds only what the approval book keeps under `client/<device id>` (see [`client_key`]). For this kind
    /// `Caller::device` is the name the person gave the client device, so the access log shows which device used what.
    Client { device_id: String },
}

/// The key of a client device in the approval book.
pub fn client_key(device_id: &str) -> String {
    format!("client/{device_id}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The app id (for built-in apps those of `iyakki_sec::policy::App::id()`).
    pub app: String,
    pub kind: CallerKind,
    /// The signed-in person on whose behalf it runs.
    pub person: String,
    /// The machine's name (the log stays on the machine, but a copied log says where it came from).
    pub device: String,
}

/// One use of a capability. `unit` names the device (a camera id) and is matched against a grant's parameter; `label` is what the log shows (a camera name).
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub caller: &'a Caller,
    pub capability: &'a str,
    pub unit: Option<&'a str>,
    pub label: &'a str,
}

/// Why a call was refused. The codes never change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denial {
    UnknownCapability,
    NotGranted,
    Revoked,
    CardDeclined,
    NoCard,
}

impl Denial {
    pub fn code(&self) -> &'static str {
        match self {
            Denial::UnknownCapability => "unknown_capability",
            Denial::NotGranted => "not_granted",
            Denial::Revoked => "revoked",
            Denial::CardDeclined => "card_declined",
            Denial::NoCard => "no_card",
        }
    }
}

impl std::fmt::Display for Denial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let words = match self {
            Denial::UnknownCapability => "that is not a capability this system knows",
            Denial::NotGranted => "this app was not given that access",
            Denial::Revoked => "that access was taken away (Settings, Apps access)",
            Denial::CardDeclined => "you said no",
            Denial::NoCard => "this needs your answer on the screen, and there is no screen to ask on",
        };
        write!(f, "denied ({}): {words}", self.code())
    }
}

impl std::error::Error for Denial {}

/// What the person answered on a per-use card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAnswer {
    Allow,
    Deny,
    /// Nobody can be asked (no screen, a remote session, a timeout).
    Unavailable,
}

/// The per-use card of a hard ask ("App X wants to listen. Allow?"). The real one is the consent mechanism of the shell; tests use a fake.
pub trait CardAsk: Send + Sync {
    fn ask(&self, caller: &Caller, capability: &str, label: &str) -> CardAnswer;
}

/// No card can be shown: every hard ask is refused with `no_card`.
pub struct NoCard;
impl CardAsk for NoCard {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        CardAnswer::Unavailable
    }
}

/// Where the broker reads approvals. Any [`ApprovalBook`] that can be shared is one.
pub trait ApprovalLookup: Send + Sync {
    fn approval(&self, installation: &str) -> Option<Approval>;
}

impl<T: ApprovalBook + Send + Sync> ApprovalLookup for T {
    fn approval(&self, installation: &str) -> Option<Approval> {
        self.get(installation)
    }
}

/// One line of the access log. No content, no paths, no pictures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessEvent {
    /// Seconds since the epoch.
    pub at: i64,
    pub app: String,
    pub person: String,
    pub device: String,
    pub capability: String,
    /// The unit used, by name (a camera's name).
    pub target: String,
    pub allowed: bool,
    /// Empty when allowed, else a [`Denial::code`].
    #[serde(default)]
    pub reason: String,
}

/// Where the broker writes. The real one is a bounded file on the machine; it never leaves it.
pub trait AccessSink: Send + Sync {
    fn record(&self, event: AccessEvent);
}

/// Throws everything away (a hub nobody wired a log to).
pub struct NullSink;
impl AccessSink for NullSink {
    fn record(&self, _: AccessEvent) {}
}

/// The in-memory log: the fake for tests, bounded like the real one.
pub struct MemoryLog {
    cap: usize,
    rows: Mutex<Vec<AccessEvent>>,
}

impl MemoryLog {
    pub fn new(cap: usize) -> MemoryLog {
        MemoryLog { cap: cap.max(1), rows: Mutex::new(Vec::new()) }
    }
    pub fn rows(&self) -> Vec<AccessEvent> {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl AccessSink for MemoryLog {
    fn record(&self, event: AccessEvent) {
        let mut r = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        r.push(event);
        let over = r.len().saturating_sub(self.cap);
        r.drain(..over);
    }
}

/// A repeated allowed use (a live view asks for a picture ten times a second) is one log line per this many seconds.
pub const COALESCE_ALLOWED_SECS: i64 = 60;
/// A repeated refusal is one line per this many seconds.
pub const COALESCE_DENIED_SECS: i64 = 10;
/// The most distinct (app, person, capability, unit, outcome) keys remembered for coalescing.
const MAX_KEYS: usize = 512;

pub struct Broker {
    lookup: Arc<dyn ApprovalLookup>,
    card: Arc<dyn CardAsk>,
    sink: Arc<dyn AccessSink>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    last: Mutex<HashMap<String, i64>>,
}

impl Broker {
    pub fn new(lookup: Arc<dyn ApprovalLookup>, card: Arc<dyn CardAsk>, sink: Arc<dyn AccessSink>, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Broker {
        Broker { lookup, card, sink, clock, last: Mutex::new(HashMap::new()) }
    }

    /// Whether the caller holds the capability at all (the grant, not the card).
    fn holds(&self, call: &Call<'_>) -> bool {
        match &call.caller.kind {
            CallerKind::Builtin => builtin_holds(&call.caller.app, call.capability),
            CallerKind::Client { device_id } => self.lookup.approval(&client_key(device_id)).is_some_and(|a| a.holds_at(call.capability, call.unit, (self.clock)())),
            CallerKind::Store { installation } => self.lookup.approval(installation).is_some_and(|a| a.holds_at(call.capability, call.unit, (self.clock)())),
        }
    }

    /// Write one extra line for a capability the caller holds and has already been let use (a step of a long piece of work, such as one
    /// decision of a router run). Nothing is asked and nothing is decided; a caller that does not hold the capability writes nothing, and
    /// the lines are never folded together. Answers whether a line was written.
    pub fn note(&self, call: &Call<'_>, text: &str) -> bool {
        if lookup(call.capability).is_none() || !self.holds(call) {
            return false;
        }
        let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
        self.sink.record(AccessEvent { at: (self.clock)(), app: clean(&call.caller.app, 80), person: clean(&call.caller.person, 80), device: clean(&call.caller.device, 80), capability: clean(call.capability, 40), target: clean(text, 120), allowed: true, reason: String::new() });
        true
    }

    /// The decision, without the log (the rules of the module header, steps 1 to 3).
    fn rule(&self, call: &Call<'_>, label: &dyn Fn() -> String) -> Result<(), Denial> {
        if lookup(call.capability).is_none() {
            return Err(Denial::UnknownCapability);
        }
        let held = match &call.caller.kind {
            CallerKind::Builtin => builtin_holds(&call.caller.app, call.capability),
            CallerKind::Client { device_id } => match self.lookup.approval(&client_key(device_id)) {
                Some(a) if a.holds_at(call.capability, call.unit, (self.clock)()) => true,
                Some(a) if a.revoked.iter().any(|g| g.name == call.capability) => return Err(Denial::Revoked),
                _ => false,
            },
            CallerKind::Store { installation } => match self.lookup.approval(installation) {
                Some(a) if a.holds_at(call.capability, call.unit, (self.clock)()) => true,
                Some(a) if a.revoked.iter().any(|g| g.name == call.capability) => return Err(Denial::Revoked),
                _ => false,
            },
        };
        if !held {
            return Err(Denial::NotGranted);
        }
        if is_hard_ask(call.capability) {
            return match self.card.ask(call.caller, call.capability, &label()) {
                CardAnswer::Allow => Ok(()),
                CardAnswer::Deny => Err(Denial::CardDeclined),
                CardAnswer::Unavailable => Err(Denial::NoCard),
            };
        }
        Ok(())
    }

    /// Decide one call and write the access log line. `Ok` means the caller may go on to the device.
    pub fn decide(&self, call: &Call<'_>) -> Result<(), Denial> {
        self.decide_with(call, &|| call.label.to_string())
    }

    /// Like [`Broker::decide`], but the unit's name is only worked out when a line is written or a card is shown (a live view asks ten times a
    /// second and nearly all of those are folded into one line).
    pub fn decide_with(&self, call: &Call<'_>, label: &dyn Fn() -> String) -> Result<(), Denial> {
        let r = self.rule(call, label);
        self.log(call, &r, label);
        r
    }

    fn log(&self, call: &Call<'_>, r: &Result<(), Denial>, label: &dyn Fn() -> String) {
        let at = (self.clock)();
        let reason = r.as_ref().err().map(|d| d.code()).unwrap_or("");
        let key = format!("{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}", call.caller.app, call.caller.person, call.capability, call.unit.unwrap_or(""), reason);
        let window = if r.is_ok() { COALESCE_ALLOWED_SECS } else { COALESCE_DENIED_SECS };
        {
            let mut m = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if m.get(&key).is_some_and(|t| at >= *t && at - *t < window) {
                return;
            }
            if m.len() >= MAX_KEYS {
                m.retain(|_, t| at - *t < COALESCE_ALLOWED_SECS);
                if m.len() >= MAX_KEYS {
                    m.clear();
                }
            }
            m.insert(key, at);
        }
        let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
        self.sink.record(AccessEvent {
            at,
            app: clean(&call.caller.app, 80),
            person: clean(&call.caller.person, 80),
            device: clean(&call.caller.device, 80),
            capability: clean(call.capability, 40),
            target: clean(&label(), 120),
            allowed: r.is_ok(),
            reason: reason.to_string(),
        });
    }
}

#[cfg(test)]
mod tests;
