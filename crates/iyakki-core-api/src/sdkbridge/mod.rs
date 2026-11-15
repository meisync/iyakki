//! The SDK bridge manifest: the one place that lists the namespaces, calls and capabilities of `window.iyakki` with the version each appeared in
//! (docs/development/SDK-BRIDGE-V2.md, docs/development/SDK-API.md section 14). No IO.
//!
//! The rules it carries (the compatibility charter, CORE-COMPATIBILITY-DESIGN.md, applied to the app bridge):
//!
//! * **Additive only.** A release adds a namespace, a call, an optional argument or an event. It never renames, removes or changes the meaning of
//!   one. The version is `MAJOR.MINOR`; a minor only adds.
//! * **Found by name, not by number.** An app asks [`has`] / the capability listing; the version only answers "can we talk at all".
//! * **Unknown is data.** A call this release does not know answers a structured `Unsupported` error ([`unsupported`]); unknown arguments
//!   of the new namespaces are ignored and reported back; the host never panics on a name it does not know.
//! * **Every call has an approval class.** Read-only calls are free; calls that change state, spend or listen are never `None`
//!   (checked by a test of this module).

use std::collections::BTreeMap;

use serde_json::{json, Value};

pub mod markdown;
pub mod money;

/// The SDK version of this release. 1.0 was the original bridge, 1.1 added discovery, `llm`, `models` and `speech.say`, 1.2 the namespaces of
/// F-SDK-05 to F-SDK-13. Only additions within a major.
pub const SDK_VERSION: &str = "1.2";
pub const SDK_MAJOR: u16 = 1;
pub const SDK_MINOR: u16 = 2;

/// `MAJOR.MINOR`, ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    pub const CURRENT: Version = Version { major: SDK_MAJOR, minor: SDK_MINOR };

    pub fn parse(s: &str) -> Option<Version> {
        let (a, b) = s.trim().split_once('.')?;
        let num = |t: &str| (!t.is_empty() && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit())).then(|| t.parse::<u16>().ok()).flatten();
        Some(Version { major: num(a)?, minor: num(b)? })
    }

    pub fn text(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

/// What the person is asked before a call runs. The names are stable (they appear in the listing and the typings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// Nothing: the call only reads, or only touches the app's own things.
    None,
    /// Once per app (kept, revocable in the Info panel).
    FirstUse,
    /// A card every time, with the details of that call.
    EachCall,
    /// A card every time and the person types a name (destructive).
    EachCallTyped,
    /// Money: checked against the limit the person set (default nothing); the person's card decides raising it.
    Limit,
}

impl Approval {
    pub fn name(&self) -> &'static str {
        match self {
            Approval::None => "none",
            Approval::FirstUse => "first-use",
            Approval::EachCall => "each-call",
            Approval::EachCallTyped => "each-call+typed",
            Approval::Limit => "limit",
        }
    }
}

/// What a call does to the machine or the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Reads something the app may see.
    Read,
    /// Touches only what the app itself made (its own embed, its own sound).
    Own,
    /// Changes something outside the app (a cluster, a dashboard, a flow run, the speaker).
    Change,
    /// Spends money.
    Spend,
    /// Opens the microphone.
    Listen,
}

impl Effect {
    pub fn name(&self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Own => "own",
            Effect::Change => "change",
            Effect::Spend => "spend",
            Effect::Listen => "listen",
        }
    }
}

/// How a capability is written in `app.json` and what its value means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    /// `true` or `false`.
    Flag,
    /// A list of names (at most `max`), like the flows an app may run.
    Names { max: usize },
    /// A list of exact host names (no wildcards, no addresses), like `network`.
    Hosts { max: usize },
    /// One word out of a fixed list, like `"read"` or `"manage"`.
    Level(&'static [&'static str]),
    /// A currency code (three capital letters).
    Currency,
}

/// A capability key of `app.json`.
#[derive(Clone, Copy, Debug)]
pub struct Capability {
    pub key: &'static str,
    pub since: &'static str,
    pub param: Param,
    /// The install card's words (plain, short).
    pub words: &'static str,
    /// Asks the person again whatever the autonomy setting (money, the cluster, the microphone, the network).
    pub sensitive: bool,
    /// Parsed by the original code, not by [`normalize_grant`] (listed here so discovery is complete).
    pub legacy: bool,
}

const fn cap(key: &'static str, since: &'static str, param: Param, words: &'static str, sensitive: bool) -> Capability {
    Capability { key, since, param, words, sensitive, legacy: false }
}

const fn old(key: &'static str, since: &'static str, param: Param, words: &'static str, sensitive: bool) -> Capability {
    Capability { key, since, param, words, sensitive, legacy: true }
}

/// Every capability key, in the order the install card lists them. New keys are appended with their `since`.
pub const CAPABILITIES: &[Capability] = &[
    old("storage", "1.0", Param::Flag, "keep its own saved data on this device (at most 256 KB, only this app can read it)", false),
    old("clipboard", "1.0", Param::Flag, "read and write the clipboard", false),
    old("notifications", "1.0", Param::Flag, "leave notes in your inbox", false),
    old("files", "1.0", Param::Flag, "open and save files you pick in the file dialog (it never sees other files)", false),
    old("agent", "1.0", Param::Flag, "ask the assistant questions, each one only after you allow it", false),
    old("network", "1.0", Param::Hosts { max: 8 }, "reach the listed internet hosts", true),
    old("llm", "1.1", Param::Flag, "use your AI model for its own questions, with no tools and no access to your files (you are asked once; it has a daily limit; the model is the one you chose in Settings, which may send the text to another computer)", true),
    old("speech", "1.1", Param::Flag, "read text aloud with your voice settings (you are asked once; a voice that runs on another computer gets the text)", true),
    cap("markdown", "1.2", Param::Flag, "show formatted text (Markdown) in a viewer; links open only after the usual link warning", false),
    cap("embed", "1.2", Param::Names { max: 4 }, "show the listed kinds of viewer (markdown, browser, media, dash) inside its own page; a web page inside an app asks you once and may only visit the sites listed in embed_hosts", true),
    cap("embed_hosts", "1.2", Param::Hosts { max: 8 }, "let an embedded web page visit the listed sites (your own host policy still applies)", true),
    cap("dash", "1.2", Param::Names { max: 8 }, "read from the listed dashboard data sources (read only; you are asked once per source; adding a panel asks every time)", true),
    cap("flows", "1.2", Param::Names { max: 8 }, "start the listed published flows, each run only after you allow it", true),
    cap("k3s", "1.2", Param::Level(&["read", "manage"]), "look at the objects, logs and status of its own part of the cluster (read), or also change it (manage: every change asks you first, with the plan)", true),
    cap("media", "1.2", Param::Flag, "list your music and videos and play them in the player (you are asked once to list, and again for a file outside your media folders)", false),
    cap("audio", "1.2", Param::Flag, "play sound on this computer (you are asked once; your volume and mute settings always apply)", false),
    cap("microphone", "1.2", Param::Flag, "listen through the microphone and turn speech into text, only when you press Allow each time; the sound is never given to the app", true),
    cap("money", "1.2", Param::Currency, "spend money up to a daily limit that you set (it starts at nothing, and you are asked before every payment)", true),
];

pub fn capability(key: &str) -> Option<&'static Capability> {
    CAPABILITIES.iter().find(|c| c.key == key)
}

/// One call of a namespace.
#[derive(Clone, Copy, Debug)]
pub struct Op {
    pub name: &'static str,
    pub since: &'static str,
    /// The capability this call needs when it is not the namespace's own (`speech.listen` needs `microphone`).
    pub capability: Option<&'static str>,
    pub approval: Approval,
    pub effect: Effect,
}

const fn op(name: &'static str, since: &'static str, effect: Effect, approval: Approval) -> Op {
    Op { name, since, capability: None, approval, effect }
}

const fn op_cap(name: &'static str, since: &'static str, capability: &'static str, effect: Effect, approval: Approval) -> Op {
    Op { name, since, capability: Some(capability), approval, effect }
}

#[derive(Clone, Copy, Debug)]
pub struct Namespace {
    pub name: &'static str,
    pub since: &'static str,
    /// The capability key most of its calls need; `None`: no capability (the original `app`, `ui` and the discovery calls).
    pub capability: Option<&'static str>,
    /// Calls of this namespace are answered by the original bridge code, not by the v2 handler.
    pub legacy: bool,
    pub ops: &'static [Op],
}

use Approval as A;
use Effect as E;

/// Every namespace and call. Append only: a row is never edited except to add calls.
pub const NAMESPACES: &[Namespace] = &[
    Namespace { name: "app", since: "1.0", capability: None, legacy: true, ops: &[op("info", "1.0", E::Read, A::None)] },
    Namespace { name: "storage", since: "1.0", capability: Some("storage"), legacy: true, ops: &[op("get", "1.0", E::Own, A::None), op("set", "1.0", E::Own, A::None), op("remove", "1.0", E::Own, A::None), op("list", "1.0", E::Own, A::None)] },
    Namespace { name: "ui", since: "1.0", capability: None, legacy: true, ops: &[op("toast", "1.0", E::Own, A::None), op("title", "1.0", E::Own, A::None)] },
    Namespace { name: "host", since: "1.0", capability: Some("files"), legacy: true, ops: &[op("openFile", "1.0", E::Read, A::EachCall), op("saveFile", "1.0", E::Change, A::EachCall)] },
    Namespace { name: "clipboard", since: "1.0", capability: Some("clipboard"), legacy: true, ops: &[op("read", "1.0", E::Read, A::EachCall), op("write", "1.0", E::Change, A::EachCall)] },
    Namespace { name: "notify", since: "1.0", capability: Some("notifications"), legacy: true, ops: &[op("show", "1.0", E::Change, A::FirstUse)] },
    Namespace { name: "agent", since: "1.0", capability: Some("agent"), legacy: true, ops: &[op("ask", "1.0", E::Change, A::EachCall)] },
    Namespace { name: "llm", since: "1.1", capability: Some("llm"), legacy: true, ops: &[op("chat", "1.1", E::Change, A::FirstUse), op("stream", "1.1", E::Change, A::FirstUse), op("cancel", "1.1", E::Own, A::None), op("usage", "1.1", E::Read, A::None)] },
    Namespace { name: "models", since: "1.1", capability: Some("llm"), legacy: true, ops: &[op("list", "1.1", E::Read, A::None)] },
    Namespace {
        name: "speech",
        since: "1.1",
        capability: Some("speech"),
        legacy: false,
        ops: &[op("say", "1.1", E::Change, A::FirstUse), op("stop", "1.1", E::Own, A::None), op_cap("listen", "1.2", "microphone", E::Listen, A::EachCall), op_cap("cancel", "1.2", "microphone", E::Own, A::None)],
    },
    Namespace { name: "sdk", since: "1.1", capability: None, legacy: false, ops: &[op("capabilities", "1.1", E::Read, A::None), op("has", "1.1", E::Read, A::None)] },
    Namespace { name: "markdown", since: "1.2", capability: Some("markdown"), legacy: false, ops: &[op("render", "1.2", E::Read, A::None), op("view", "1.2", E::Own, A::None), op("openFile", "1.2", E::Read, A::EachCall)] },
    Namespace {
        name: "embed",
        since: "1.2",
        capability: Some("embed"),
        legacy: false,
        ops: &[op("mount", "1.2", E::Change, A::FirstUse), op("update", "1.2", E::Own, A::None), op("call", "1.2", E::Own, A::None), op("unmount", "1.2", E::Own, A::None), op("list", "1.2", E::Read, A::None)],
    },
    Namespace { name: "dash", since: "1.2", capability: Some("dash"), legacy: false, ops: &[op("sources", "1.2", E::Read, A::None), op("query", "1.2", E::Read, A::FirstUse), op("panel.add", "1.2", E::Change, A::EachCall)] },
    Namespace {
        name: "flows",
        since: "1.2",
        capability: Some("flows"),
        legacy: false,
        ops: &[op("list", "1.2", E::Read, A::None), op("run", "1.2", E::Change, A::EachCall), op("status", "1.2", E::Read, A::None), op("cancel", "1.2", E::Change, A::EachCall)],
    },
    Namespace {
        name: "k3s",
        since: "1.2",
        capability: Some("k3s"),
        legacy: false,
        ops: &[op("status", "1.2", E::Read, A::None), op("get", "1.2", E::Read, A::None), op("logs", "1.2", E::Read, A::None), op("apply", "1.2", E::Change, A::EachCall), op("restart", "1.2", E::Change, A::EachCall)],
    },
    Namespace {
        name: "media",
        since: "1.2",
        capability: Some("media"),
        legacy: false,
        ops: &[
            op("library", "1.2", E::Read, A::FirstUse),
            op("play", "1.2", E::Change, A::EachCall),
            op("pause", "1.2", E::Own, A::None),
            op("resume", "1.2", E::Own, A::None),
            op("stop", "1.2", E::Own, A::None),
            op("state", "1.2", E::Read, A::None),
            op("queue.add", "1.2", E::Change, A::EachCall),
            op("queue.list", "1.2", E::Read, A::None),
        ],
    },
    Namespace {
        name: "audio",
        since: "1.2",
        capability: Some("audio"),
        legacy: false,
        ops: &[op("play", "1.2", E::Change, A::FirstUse), op("pause", "1.2", E::Own, A::None), op("resume", "1.2", E::Own, A::None), op("stop", "1.2", E::Own, A::None), op("setVolume", "1.2", E::Own, A::None), op("outputs", "1.2", E::Read, A::None)],
    },
    Namespace { name: "money", since: "1.2", capability: Some("money"), legacy: false, ops: &[op("limits", "1.2", E::Read, A::None), op("spend", "1.2", E::Spend, A::Limit), op("requestLimit", "1.2", E::Change, A::EachCall)] },
];

pub fn namespace(name: &str) -> Option<&'static Namespace> {
    NAMESPACES.iter().find(|n| n.name == name)
}

/// `markdown.render` or `dash.panel.add` -> the namespace and the call. The namespace is the part before the first dot.
pub fn find_op(full: &str) -> Option<(&'static Namespace, &'static Op)> {
    let (ns, rest) = full.split_once('.')?;
    let n = namespace(ns)?;
    n.ops.iter().find(|o| o.name == rest).map(|o| (n, o))
}

/// The capability a call needs: its own override, else its namespace's.
pub fn capability_of(n: &Namespace, o: &Op) -> Option<&'static str> {
    o.capability.or(n.capability)
}

fn since_of(text: &str) -> Version {
    Version::parse(text).unwrap_or(Version { major: 0, minor: 0 })
}

/// The call is known to a host that speaks `host` (an older host does not know the newer calls).
pub fn known_at(host: Version, o: &Op) -> bool {
    since_of(o.since) <= host
}

/// The error for a name this release does not know: a plain sentence and the facts an app can test. Never a crash, never a guess.
pub fn unsupported(full: &str, host: Version) -> Value {
    let name: String = full.chars().filter(|c| !c.is_control()).take(60).collect();
    let (ns, _) = name.split_once('.').unwrap_or((name.as_str(), ""));
    json!({
        "code": "Unsupported",
        "message": format!("iyakki.{name} is not available in this version of IYAKKI (SDK {}): use iyakki.has('{ns}') to check first", host.text()),
        "op": name,
        "namespace": ns,
        "sdk": host.text(),
    })
}

// ------------------------------------------------------------------------------------------------ grants

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 64 && s.chars().all(|c| c.is_alphanumeric() || " ._-".contains(c)) && s.trim() == s
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 120 && h != "*" && !h.starts_with('.') && !h.ends_with('.') && (h.contains('.') || h == "localhost") && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && !h.ends_with(".invalid")
}

/// Check the value an app wrote for a capability key. `Ok(None)`: switched off (`false`); `Ok(Some(v))`: the normalised grant. The errors are in words an app author needs.
pub fn normalize_grant(key: &str, v: &Value) -> Result<Option<Value>, String> {
    let c = capability(key).ok_or_else(|| format!("unknown capability {key:?}"))?;
    match c.param {
        Param::Flag => match v.as_bool() {
            Some(true) => Ok(Some(Value::Bool(true))),
            Some(false) => Ok(None),
            None => Err(format!("capability {key:?} is true or false")),
        },
        Param::Names { max } | Param::Hosts { max } => {
            let hosts = matches!(c.param, Param::Hosts { .. });
            let a = v.as_array().ok_or_else(|| format!("capability {key:?} is a list"))?;
            if a.is_empty() {
                return Ok(None);
            }
            if a.len() > max {
                return Err(format!("capability {key:?}: at most {max} entries"));
            }
            let mut out: Vec<String> = Vec::new();
            for x in a {
                let s = x.as_str().ok_or_else(|| format!("capability {key:?}: entries are text"))?;
                let s = if hosts { s.trim().to_ascii_lowercase() } else { s.to_string() };
                if hosts && !host_ok(&s) {
                    return Err(format!("capability {key:?}: {s:?} is not a plain host name (no wildcards, no addresses with paths)"));
                }
                if !hosts && !name_ok(&s) {
                    return Err(format!("capability {key:?}: {s:?} is 1 to 64 letters, digits, spaces and . _ -"));
                }
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            if key == "embed" {
                if let Some(bad) = out.iter().find(|k| !EMBED_KINDS.contains(&k.as_str())) {
                    return Err(format!("capability \"embed\": {bad:?} is not a kind (markdown, browser, media, dash)"));
                }
            }
            Ok(Some(json!(out)))
        }
        Param::Level(levels) => {
            let s = v.as_str().ok_or_else(|| format!("capability {key:?} is one of: {}", levels.join(", ")))?;
            if levels.contains(&s) {
                Ok(Some(json!(s)))
            } else {
                Err(format!("capability {key:?} is one of: {}", levels.join(", ")))
            }
        }
        Param::Currency => {
            let s = v.as_str().ok_or_else(|| format!("capability {key:?} is a currency code like \"USD\""))?;
            if money::valid_currency(s) {
                Ok(Some(json!(s)))
            } else {
                Err(format!("capability {key:?} is a currency code of three capital letters, like \"USD\""))
            }
        }
    }
}

/// The kinds `embed.mount` knows.
pub const EMBED_KINDS: [&str; 4] = ["markdown", "browser", "media", "dash"];

/// The store's permission string of a grant: `tile:flows=a,b`, `tile:money=USD`, `tile:markdown`.
pub fn permission_string(key: &str, v: &Value) -> String {
    match v {
        Value::Array(a) => format!("tile:{key}={}", a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")),
        Value::String(s) => format!("tile:{key}={s}"),
        _ => format!("tile:{key}"),
    }
}

/// The other way: `flows=a,b` (without `tile:`) -> the key and the grant. `None` for a key this release does not know.
pub fn parse_permission(p: &str) -> Option<(String, Value)> {
    let (key, rest) = match p.split_once('=') {
        Some((k, r)) => (k, Some(r)),
        None => (p, None),
    };
    let c = capability(key).filter(|c| !c.legacy)?;
    let v = match (c.param, rest) {
        (Param::Flag, None) => Value::Bool(true),
        (Param::Names { .. } | Param::Hosts { .. }, Some(r)) => json!(r.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>()),
        (Param::Level(_) | Param::Currency, Some(r)) => json!(r),
        _ => return None,
    };
    normalize_grant(key, &v).ok().flatten().map(|v| (key.to_string(), v))
}

/// The install card's words for a grant, with its parameters (`start the flows: backup, report`).
pub fn grant_words(key: &str, v: &Value) -> Option<String> {
    let c = capability(key)?;
    let list = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    Some(match (key, v) {
        ("embed", v) => format!("{} [{}]", c.words, list(v)),
        ("embed_hosts", v) | ("dash", v) | ("flows", v) => format!("{} [{}]", c.words, list(v)),
        ("k3s", Value::String(l)) => format!("{} [{l}]", c.words),
        ("money", Value::String(cur)) => format!("{} [{cur}]", c.words),
        _ => c.words.to_string(),
    })
}

// ------------------------------------------------------------------------------------------------ discovery

/// The listing `iyakki.capabilities()` returns. `granted` tells whether the app holds a capability key. `host` is the version the host speaks
/// (this release: [`Version::CURRENT`]); calls newer than that are left out, which is how a test plays an older host.
pub fn listing(host: Version, granted: &dyn Fn(&str) -> bool) -> Value {
    let mut namespaces = Vec::new();
    for n in NAMESPACES.iter().filter(|n| since_of(n.since) <= host) {
        let ops: Vec<Value> = n
            .ops
            .iter()
            .filter(|o| known_at(host, o))
            .map(|o| {
                let ready = capability_of(n, o).is_none_or(granted);
                json!({"name": o.name, "since": o.since, "approval": o.approval.name(), "effect": o.effect.name(), "ready": ready})
            })
            .collect();
        let ready = n.capability.is_none_or(granted);
        namespaces.push(json!({"name": n.name, "since": n.since, "capability": n.capability, "ready": ready, "ops": ops}));
    }
    let capabilities: Vec<Value> = CAPABILITIES.iter().filter(|c| since_of(c.since) <= host).map(|c| json!({"key": c.key, "since": c.since, "granted": granted(c.key)})).collect();
    json!({"sdk": host.text(), "major": host.major, "minor": host.minor, "namespaces": namespaces, "capabilities": capabilities})
}

/// `iyakki.has('markdown')` or `iyakki.has('markdown.render')`: the host knows it at this version and the app holds what it needs.
pub fn has(host: Version, name: &str, granted: &dyn Fn(&str) -> bool) -> bool {
    if let Some((n, o)) = find_op(name) {
        return known_at(host, o) && capability_of(n, o).is_none_or(granted);
    }
    match namespace(name) {
        Some(n) => since_of(n.since) <= host && n.capability.is_none_or(granted),
        None => false,
    }
}

/// `app.json` may state `"sdk": "1.2"`: the least SDK it needs. `Ok` when this host speaks at least that; else the sentence for the install review.
pub fn needs_ok(host: Version, wanted: &str) -> Result<(), String> {
    let w = Version::parse(wanted).ok_or_else(|| format!("sdk: {wanted:?} is not a version like 1.2"))?;
    if w.major != host.major {
        return Err(format!("this app is written for SDK {}, this IYAKKI speaks SDK {}", w.text(), host.text()));
    }
    if w > host {
        return Err(format!("this app needs SDK {} or newer, this IYAKKI speaks SDK {}: update IYAKKI", w.text(), host.text()));
    }
    Ok(())
}

/// Grants an app holds, as the map the host keeps (`key` -> value).
pub type Grants = BTreeMap<String, Value>;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fuzz_tests;
