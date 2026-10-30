//! The startup registry: ONE place where every service of the machine says how far its start has come, so the screen (and
//! `iyakki dev status`, `dev diag`, `dev state`) can show what is still starting, what waits for what, and what failed.
//!
//! * A service is in one [`StartupState`]: pending, starting (with a short step text and since when), ready, failed (with a
//!   reason), stopped (by choice) or disabled (not on this machine). Services that know their own progress push it
//!   ([`Registry::starting`], [`Registry::ready`], ...); the ones that cannot are read by one cheap poll ([`crate::startup_probe`]).
//! * The catalogue ([`CATALOGUE`]) holds the names, the order and the dependencies ("k3s waits for the clock and the network").
//!   A service that is pending or starting without a step of its own while something it needs is not ready shows "waiting for
//!   the clock" ([`Registry::snapshot`]).
//! * What the person reads is made here too, with the same text for the Home strip, the pop-up, the Settings page and the
//!   developer commands: [`Summary`]. An administrator sees the step texts and the reasons; everybody else sees names, steps
//!   and "did not start" without the reason (a reason can hold paths and addresses).
//! * "All services ready" fades after a few seconds ([`Calm`]); a failure stays until it is fixed.
//! * Failures are queued once per transition ([`Registry::take_failures`]) so the caller writes one audit line each.
//!
//! Nothing here starts, stops or waits for anything.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Where a service stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupState {
    /// Not started yet (nothing known about it).
    Pending,
    /// Starting: what it is doing now.
    Starting { step: String },
    Ready,
    /// Did not start or stopped by itself.
    Failed(String),
    /// Off because the person (or a setting) chose so: quiet.
    Stopped,
    /// Not on this machine (not installed, no hardware): quiet.
    Disabled,
}

impl StartupState {
    /// Still on its way up.
    pub fn busy(&self) -> bool {
        matches!(self, StartupState::Pending | StartupState::Starting { .. })
    }

    pub fn failed(&self) -> bool {
        matches!(self, StartupState::Failed(_))
    }

    /// The word `dev status` prints.
    pub fn word(&self) -> &'static str {
        match self {
            StartupState::Pending => "pending",
            StartupState::Starting { .. } => "starting",
            StartupState::Ready => "ready",
            StartupState::Failed(_) => "failed",
            StartupState::Stopped => "stopped",
            StartupState::Disabled => "disabled",
        }
    }
}

/// Who needs to know: system-wide services (they run before anybody signs in) or the signed-in person's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    System,
    Session,
}

/// The Settings page a Open button leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goes {
    Nothing,
    Cluster,
    Network,
    Firewall,
    Sound,
    Time,
    Models,
    Shares,
}

impl Goes {
    /// The item the shell's `jump_to("settings", ...)` takes; `None` for nowhere.
    pub fn item(self) -> Option<&'static str> {
        match self {
            Goes::Nothing => None,
            Goes::Cluster => Some("tab:devcluster"),
            Goes::Network => Some("tab:network"),
            Goes::Firewall => Some("tab:firewall"),
            Goes::Sound => Some("tab:sound"),
            Goes::Time => Some("tab:time"),
            Goes::Models => Some("tab:models"),
            Goes::Shares => Some("tab:shares"),
        }
    }
}

/// What the catalogue knows about one service.
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    /// How it is called in "waiting for ...".
    pub short: &'static str,
    pub scope: Scope,
    /// Services that must be ready first.
    pub after: &'static [&'static str],
    pub goes: Goes,
    /// A Retry button can start it again ([`crate::startup_probe::retry`]).
    pub can_retry: bool,
    /// Listed (and waited for) from the start; services that only push begin disabled.
    pub tracked: bool,
}

/// The services, in the order they are listed. Wi-Fi is part of the network: one line for both.
pub const CATALOGUE: [Def; 12] = [
    Def { id: "clock", name: "Clock", short: "the clock", scope: Scope::System, after: &[], goes: Goes::Time, can_retry: false, tracked: true },
    Def { id: "network", name: "Network and Wi-Fi", short: "the network", scope: Scope::System, after: &[], goes: Goes::Network, can_retry: false, tracked: true },
    Def { id: "firewall", name: "Firewall", short: "the firewall", scope: Scope::System, after: &[], goes: Goes::Firewall, can_retry: false, tracked: true },
    Def { id: "sound", name: "Sound", short: "sound", scope: Scope::System, after: &[], goes: Goes::Sound, can_retry: false, tracked: true },
    Def { id: "k3s", name: "Dev cluster (k3s)", short: "the cluster", scope: Scope::System, after: &["network", "clock"], goes: Goes::Cluster, can_retry: true, tracked: true },
    Def { id: "models", name: "Local models", short: "the models", scope: Scope::Session, after: &[], goes: Goes::Models, can_retry: true, tracked: false },
    Def { id: "web", name: "Web engine", short: "the web engine", scope: Scope::Session, after: &[], goes: Goes::Nothing, can_retry: false, tracked: true },
    Def { id: "flows", name: "Flows scheduler", short: "the flows scheduler", scope: Scope::Session, after: &[], goes: Goes::Nothing, can_retry: false, tracked: false },
    Def { id: "shares", name: "Shared folders", short: "shared folders", scope: Scope::Session, after: &["network"], goes: Goes::Shares, can_retry: false, tracked: true },
    Def { id: "service", name: "Web service", short: "the web service", scope: Scope::System, after: &["network"], goes: Goes::Nothing, can_retry: false, tracked: false },
    Def { id: "devname", name: "Device name (mDNS)", short: "the device name", scope: Scope::System, after: &["network"], goes: Goes::Network, can_retry: false, tracked: false },
    Def { id: "agent", name: "Assistant", short: "the assistant", scope: Scope::Session, after: &[], goes: Goes::Nothing, can_retry: false, tracked: true },
];

struct Slot {
    def: &'static Def,
    state: StartupState,
    /// Registry seconds when the state last changed (a new step text of a service that keeps starting does not move it).
    since: u64,
}

/// A failure, queued once per transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub id: &'static str,
    pub name: &'static str,
    pub reason: String,
}

struct Inner {
    slots: Vec<Slot>,
    fails: Vec<Failure>,
}

/// The registry. One per process ([`global`]); tests make their own with a clock they move.
pub struct Registry {
    inner: Mutex<Inner>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

/// One service as it is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    pub scope: Scope,
    pub state: StartupState,
    /// The step text to show: the service's own, "waiting for the clock", or "" when there is nothing to say.
    pub step: String,
    /// Names of what it waits for (only for pending or starting services).
    pub waits_for: Vec<&'static str>,
    /// Seconds since the state changed.
    pub age: u64,
    pub goes: Goes,
    pub can_retry: bool,
}

/// What a Retry or Open button of a line does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actions {
    pub retry: bool,
    pub open: Goes,
}

/// One line of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub id: &'static str,
    pub text: String,
    pub failed: bool,
    /// Pending services wait for others; starting ones spin.
    pub starting: bool,
    /// Buttons (administrators only; empty for everybody else).
    pub actions: Actions,
}

/// Everything the screens need: the services that are not ready yet, in the order they are shown.
#[derive(Clone, Debug)]
pub struct Summary {
    /// Pending, starting and failed services: failures first, then the ones that started first, then pending ones.
    pub rows: Vec<Row>,
    pub ready: usize,
    pub tracked: usize,
}

impl Summary {
    pub fn all_ready(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn failed(&self) -> usize {
        self.rows.iter().filter(|r| r.state.failed()).count()
    }

    pub fn busy(&self) -> bool {
        self.rows.iter().any(|r| r.state.busy())
    }

    /// Only the system-wide services (the sign-in screen).
    pub fn system_only(&self) -> Summary {
        Summary { rows: self.rows.iter().filter(|r| r.scope == Scope::System).cloned().collect(), ready: self.ready, tracked: self.tracked }
    }

    /// One line per service that is not ready.
    pub fn lines(&self, admin: bool) -> Vec<Line> {
        self.rows
            .iter()
            .map(|r| {
                let failed = r.state.failed();
                let text = match &r.state {
                    StartupState::Failed(reason) if admin => format!("{}: {}", r.name, reason),
                    StartupState::Failed(_) => format!("{}: did not start", r.name),
                    _ if r.step.is_empty() => format!("{}: starting", r.name),
                    _ => format!("{}: {}", r.name, r.step),
                };
                let actions = if admin { Actions { retry: failed && r.can_retry, open: if failed { r.goes } else { Goes::Nothing } } } else { Actions { retry: false, open: Goes::Nothing } };
                Line { id: r.id, text, failed, starting: matches!(r.state, StartupState::Starting { .. }), actions }
            })
            .collect()
    }

    /// The text of the strip: the most important line and how many more there are. Empty when everything is ready.
    pub fn strip(&self, admin: bool) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        if !admin {
            let failed = self.failed();
            return if failed > 0 {
                "A service did not start. Ask an administrator.".to_string()
            } else {
                format!("Starting services, {} left", self.rows.len())
            };
        }
        let lines = self.lines(true);
        let first = &lines[0];
        let head = if first.failed { first.text.clone() } else { format!("Starting {}", first.text) };
        match lines.len() - 1 {
            0 => head,
            n => format!("{head}  +{n} more"),
        }
    }
}

impl Registry {
    pub fn new(now: impl Fn() -> u64 + Send + Sync + 'static) -> Registry {
        let slots = CATALOGUE.iter().map(|d| Slot { def: d, state: if d.tracked { StartupState::Pending } else { StartupState::Disabled }, since: 0 }).collect();
        Registry { inner: Mutex::new(Inner { slots, fails: Vec::new() }), now: Box::new(now) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set a service's state. Returns true when something changed. A starting service that only changes its step keeps its
    /// "since". A failure is queued once, when it begins or its reason changes. Unknown ids are ignored.
    pub fn report(&self, id: &str, state: StartupState) -> bool {
        let now = (self.now)();
        let mut g = self.lock();
        let Some(i) = g.slots.iter().position(|s| s.def.id == id) else { return false };
        let slot = &mut g.slots[i];
        if slot.state == state {
            return false;
        }
        let both_starting = matches!((&slot.state, &state), (StartupState::Starting { .. }, StartupState::Starting { .. }));
        let new_failure = match (&slot.state, &state) {
            (old, StartupState::Failed(r)) if !matches!(old, StartupState::Failed(x) if x == r) => Some(r.clone()),
            _ => None,
        };
        if !both_starting {
            slot.since = now;
        }
        slot.state = state;
        let (def_id, def_name) = (slot.def.id, slot.def.name);
        if let Some(reason) = new_failure {
            if g.fails.len() >= 32 {
                g.fails.remove(0);
            }
            g.fails.push(Failure { id: def_id, name: def_name, reason });
        }
        true
    }

    pub fn starting(&self, id: &str, step: &str) -> bool {
        self.report(id, StartupState::Starting { step: step.to_string() })
    }

    pub fn ready(&self, id: &str) -> bool {
        self.report(id, StartupState::Ready)
    }

    pub fn failed(&self, id: &str, reason: &str) -> bool {
        self.report(id, StartupState::Failed(reason.to_string()))
    }

    pub fn stopped(&self, id: &str) -> bool {
        self.report(id, StartupState::Stopped)
    }

    pub fn disabled(&self, id: &str) -> bool {
        self.report(id, StartupState::Disabled)
    }

    pub fn state(&self, id: &str) -> Option<StartupState> {
        self.lock().slots.iter().find(|s| s.def.id == id).map(|s| s.state.clone())
    }

    /// Seconds a service has been in its state (0 for an unknown one).
    pub fn age(&self, id: &str) -> u64 {
        let now = (self.now)();
        self.lock().slots.iter().find(|s| s.def.id == id).map_or(0, |s| now.saturating_sub(s.since))
    }

    /// The failures since the last call (one per transition).
    pub fn take_failures(&self) -> Vec<Failure> {
        std::mem::take(&mut self.lock().fails)
    }

    /// Every service in catalogue order, with the dependency text worked out.
    pub fn snapshot(&self) -> Vec<Row> {
        let now = (self.now)();
        let g = self.lock();
        g.slots
            .iter()
            .map(|s| {
                let waits_for: Vec<&'static str> = if s.state.busy() {
                    s.def.after.iter().filter_map(|a| g.slots.iter().find(|x| x.def.id == *a)).filter(|x| matches!(x.state, StartupState::Pending | StartupState::Starting { .. } | StartupState::Failed(_))).map(|x| x.def.short).collect()
                } else {
                    Vec::new()
                };
                let own = match &s.state {
                    StartupState::Starting { step } => step.clone(),
                    _ => String::new(),
                };
                let step = if !own.is_empty() {
                    own
                } else if !waits_for.is_empty() {
                    format!("waiting for {}", join_and(&waits_for))
                } else if s.state == StartupState::Pending {
                    "waiting to start".to_string()
                } else {
                    String::new()
                };
                Row { id: s.def.id, name: s.def.name, scope: s.def.scope, state: s.state.clone(), step, waits_for, age: now.saturating_sub(s.since), goes: s.def.goes, can_retry: s.def.can_retry }
            })
            .collect()
    }

    /// The services that are not ready yet (see [`Summary`]).
    pub fn summary(&self) -> Summary {
        let all = self.snapshot();
        let tracked = all.iter().filter(|r| !matches!(r.state, StartupState::Disabled)).count();
        let ready = all.iter().filter(|r| r.state == StartupState::Ready).count();
        let mut rows: Vec<Row> = all.into_iter().filter(|r| r.state.busy() || r.state.failed()).collect();
        // failures first, then starting (the longest first), then the ones still waiting; catalogue order inside each
        let rank = |r: &Row| match r.state {
            StartupState::Failed(_) => 0,
            StartupState::Starting { .. } => 1,
            _ => 2,
        };
        rows.sort_by(|a, b| rank(a).cmp(&rank(b)).then(if rank(a) == 1 { b.age.cmp(&a.age) } else { std::cmp::Ordering::Equal }));
        Summary { rows, ready, tracked }
    }

    /// `dev status` and `dev diag`: one row per service, `name  state  step or reason`.
    pub fn report_lines(&self) -> Vec<String> {
        self.snapshot()
            .iter()
            .map(|r| {
                let detail = match &r.state {
                    StartupState::Failed(reason) => reason.clone(),
                    _ => r.step.clone(),
                };
                format!("{:<20} {:<9} {}", r.name, r.state.word(), detail).trim_end().to_string()
            })
            .collect()
    }
}

fn join_and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [a] => (*a).to_string(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The process-wide registry. Its clock is seconds since the first use.
pub fn global() -> &'static Arc<Registry> {
    static G: OnceLock<Arc<Registry>> = OnceLock::new();
    G.get_or_init(|| {
        let t0 = Instant::now();
        Arc::new(Registry::new(move || t0.elapsed().as_secs()))
    })
}

/// What the Home strip is showing, as a quiet state machine: while something is starting it is busy; when the last one is
/// ready it says "All services ready" for [`Calm::HOLD_MS`], fades for [`Calm::FADE_MS`], and is gone until something starts again.
#[derive(Clone, Debug, Default)]
pub struct Calm {
    seen_busy: bool,
    calm_since: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Services are starting or failed: show the lines.
    Busy,
    /// Everything is ready: "All services ready" at this opacity (255 = full).
    Done(u8),
    /// Nothing to show.
    Gone,
}

impl Calm {
    pub const HOLD_MS: u64 = 3000;
    pub const FADE_MS: u64 = 1000;

    /// `busy`: something is pending, starting or failed. `now_ms`: any steadily rising clock.
    pub fn update(&mut self, busy: bool, now_ms: u64) -> Phase {
        if busy {
            self.seen_busy = true;
            self.calm_since = None;
            return Phase::Busy;
        }
        if !self.seen_busy {
            return Phase::Gone;
        }
        let since = *self.calm_since.get_or_insert(now_ms);
        let t = now_ms.saturating_sub(since);
        if t < Self::HOLD_MS {
            Phase::Done(255)
        } else if t < Self::HOLD_MS + Self::FADE_MS {
            Phase::Done((255 - (t - Self::HOLD_MS) * 255 / Self::FADE_MS).clamp(1, 255) as u8)
        } else {
            self.seen_busy = false;
            self.calm_since = None;
            Phase::Gone
        }
    }
}
