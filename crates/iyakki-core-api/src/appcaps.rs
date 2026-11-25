//! The app capability catalogue (version 0), the `capabilities` list of an app manifest, and the approval record
//! (docs/development/CORE-ACCESS-MODEL.md sections 4 and 5).
//!
//! Pure data and logic, no IO and no dependency beyond serde, so the future published interface, the broker and the WASM client can use
//! the same types. Not to be confused with [`crate::identity::Capabilities`], which is what a signed-in person's role allows.
//! The catalogue is unstable until the beta tag.

use serde::{Deserialize, Serialize};

/// Version of the catalogue; changes when a name or its meaning changes.
pub const CATALOGUE_VERSION: u32 = 0;

/// What a capability takes as its parameter (the `param` of a request).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// No parameter: the capability is all or nothing.
    None,
    /// A device of that kind, by the name the system lists it with. Empty means any.
    Device,
    /// `vvvv:pppp`, a USB vendor and product number in hex.
    UsbId,
    /// An absolute folder, without `..`.
    Folder,
    /// `host:port` or `host`, no wildcards.
    HostPort,
    /// A port number, 1 to 65535.
    Port,
    /// A notification filter in words (source, kind, tag, minimum importance, text).
    Filter,
    /// A schedule in words (once, interval, calendar rule).
    Schedule,
}

/// What a built-in app holds without an install card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinDefault {
    /// Held by every built-in app.
    Yes,
    /// A built-in app may request it; the person still answers each time.
    MayAsk,
    /// Only the apps named in the built-in table.
    TableOnly,
    /// The person's role policy decides.
    RolePolicy,
    /// No built-in app holds it by default.
    No,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityDef {
    pub name: &'static str,
    /// What it gives, in plain words (shown on the install card).
    pub gives: &'static str,
    pub param: ParamKind,
    pub builtin: BuiltinDefault,
    /// Every use shows a card with Allow, whatever was approved at install.
    pub hard_ask: bool,
}

const fn def(name: &'static str, gives: &'static str, param: ParamKind, builtin: BuiltinDefault, hard_ask: bool) -> CapabilityDef {
    CapabilityDef { name, gives, param, builtin, hard_ask }
}

use BuiltinDefault as B;
use ParamKind as P;

/// The catalogue, version 0 (section 4 of the design).
pub const CATALOGUE: &[CapabilityDef] = &[
    def("camera.view", "see pictures from a camera", P::Device, B::Yes, false),
    def("camera.record", "record from a camera", P::Device, B::MayAsk, true),
    def("microphone.listen", "listen through a microphone", P::Device, B::MayAsk, true),
    def("speaker.play", "play sound", P::None, B::Yes, false),
    def("bluetooth.use", "scan for, pair and connect Bluetooth devices", P::None, B::TableOnly, false),
    def("usb.use", "list and open a USB device", P::UsbId, B::No, false),
    def("files.read", "read files in a folder", P::Folder, B::RolePolicy, false),
    def("files.write", "change files in a folder", P::Folder, B::RolePolicy, false),
    def("net.connect", "connect to a network address", P::HostPort, B::RolePolicy, false),
    def("net.listen", "accept connections from the network", P::Port, B::No, true),
    def("screen.capture", "take a picture of the screen", P::None, B::No, true),
    def("input.inject", "press keys and move the pointer", P::None, B::No, true),
    def("notify", "leave notes in the inbox", P::None, B::Yes, false),
    def("notify.receive", "receive the notes that match its filter", P::Filter, B::Yes, false),
    def("schedule.use", "be started or told at a set time", P::Schedule, B::Yes, false),
    // browser extensions (docs/development/BROWSER-EXTENSIONS-PLAN.md); web.content.script and web.user.agent are the browser plug-ins (F-BROW-90); the other web.* hook capabilities are proposed names only
    def("web.request.headers", "add request headers to the sites its rules name", P::Filter, B::TableOnly, false),
    def("web.content.script", "run the person's own scripts and style sheets in the pages of the sites each one names (the person approved each plug-in when adding and switching it on)", P::Filter, B::TableOnly, false),
    def("web.user.agent", "change the User-Agent the browser sends", P::None, B::TableOnly, false),
    // the client's own devices and clipboard (docs/development/CLIENT-SPLIT-DESIGN.md 7b); meant for client devices (an app may not meaningfully ask for them: they act on a client, not on this machine)
    def("clipboard.client", "copy text between this machine and the clipboard of a connected client device", P::None, B::No, true),
    def("files.client", "send files between a person's home folder and a connected client device (every use asks)", P::None, B::No, true),
    def("client.camera.offer", "offer the camera of a connected client device to this machine as a source of its own", P::None, B::No, true),
    def("client.microphone.offer", "offer the microphone of a connected client device to this machine as a source of its own", P::None, B::No, true),
    // the machine itself (docs/development/CORE-ACCESS-MODEL.md 6c): the guarded Os (guarded.rs) asks for these before it reaches a device
    def("system.read", "read the machine's figures and states (memory, load, disks, links, radios, screen, sound cards)", P::None, B::Yes, false),
    def("system.radio.set", "switch a radio (Wi-Fi, Bluetooth) on or off", P::None, B::TableOnly, false),
    def("system.brightness.set", "change the screen brightness", P::None, B::TableOnly, false),
    def("system.volume.set", "change the sound volume", P::None, B::TableOnly, false),
    def("system.link.set", "switch a network connection (Ethernet, Wi-Fi) on or off and ask for its address", P::None, B::TableOnly, false),
    def("system.startup.set", "change what the firmware starts: make IYAKKI's own boot entry, put it first, start it at the next restart only, or put the old order back (administrators only; the card always comes first)", P::None, B::TableOnly, false),
    def("system.memory.set", "change compressed memory and swap (every use asks)", P::None, B::TableOnly, true),
    def("system.crashlog.write", "mark the last crash note as seen and copy a kept kernel log into a folder of the person's", P::None, B::TableOnly, false),
    def("net.scan", "scan the local network for machines and the services they run (every use asks)", P::None, B::TableOnly, true),
    def("system.disk.install", "erase a disk and install IYAKKI on it (every use asks)", P::None, B::TableOnly, true),
    def("system.disk.partition", "make a new partition table, add, delete, format, rename or resize the partitions of a drive (administrators only, a typed drive name and a second confirmation always come first)", P::None, B::TableOnly, false),
    def("system.disk.unlock", "unlock, lock and reopen for writing an encrypted drive (LUKS, BitLocker) with the key the person types", P::None, B::TableOnly, false),
    def("system.disk.mount", "open, close and eject the drives (USB sticks, SD cards, disks) of the signed-in person, within what their role allows", P::None, B::TableOnly, false),
    def("system.hostname.set", "change the computer's name (every use asks)", P::None, B::TableOnly, true),
    def("router.use", "start a model router run: a bigger model plans and lighter models do the steps (every use asks)", P::None, B::TableOnly, true),
    def("router.strategy.set", "choose or add the strategy that decides which model takes each step of a router run (every use asks)", P::None, B::TableOnly, true),
    def("k8s.portforward", "start a port-forward from this computer to a pod, Service or Deployment of a cluster, on 127.0.0.1 only (every use asks)", P::None, B::TableOnly, true),
    def("fleet.read", "see the pair groups (fleets) this computer is in, their members and the numbers the members share (disk, load, memory)", P::None, B::TableOnly, false),
    def("fleet.join", "join a pair group of IYAKKI computers with a one-time code, or accept an offer to join (every use asks)", P::None, B::TableOnly, true),
    def("fleet.manage", "create a pair group, offer a join code, approve or remove a member, make a member an administrator, leave a group (every use asks)", P::None, B::TableOnly, true),
    def("fleet.assign", "send work to another computer in a pair group (every use asks)", P::None, B::TableOnly, true),
    def("fleet.receive", "accept work another computer in a pair group sent, or choose whether to accept work (every use asks)", P::None, B::TableOnly, true),
    // events delivered to apps (docs/sdk/events.md): the scope each event of appevents::CATALOGUE needs; the person sees them on the install card and in Settings > Apps
    def("flows.start", "start a flow of the person by its name (a flow that changes something asks inside as always)", P::None, B::MayAsk, false),
    def("tools.request", "ask the person to run a chat tool: nothing runs until they answer a card that names the app and the tool (every use asks)", P::None, B::MayAsk, true),
    def("events.network", "hear about the network: online, offline, metered, a sign-in page, a VPN, paired computers", P::None, B::MayAsk, false),
    def("events.power", "hear about power: battery or mains, low battery, sleep and wake, heat, memory pressure, quiet hours", P::None, B::MayAsk, false),
    def("events.session", "hear that a person signed in or out, locked or unlocked the screen, and that the vault opened or closed", P::None, B::MayAsk, false),
    def("events.security", "hear about security alerts: failed sign-ins, a new paired device, an unlock from a new place (never who or where)", P::None, B::MayAsk, false),
    def("events.display", "hear about the screen: size, scale, theme, skin and font changes", P::None, B::MayAsk, false),
    def("events.locale", "hear about the language and the accessibility settings", P::None, B::MayAsk, false),
    def("events.devices", "hear about sound devices, cameras, printers, scanners, Bluetooth devices and disks being added, removed or unwell", P::None, B::MayAsk, false),
    def("events.storage", "hear about volumes being mounted, unmounted or nearly full, and about backups", P::None, B::MayAsk, false),
    def("events.clock", "hear about clock changes and, if it asks, a tick every minute or hour", P::None, B::MayAsk, false),
    def("events.settings", "hear that a setting changed (its name, never its value)", P::None, B::MayAsk, false),
    def("events.apps", "hear about apps coming to the front or going away, installed, updated or removed", P::None, B::MayAsk, false),
    def("events.inbox", "hear that notifications and approvals are waiting (the number, never the text)", P::None, B::MayAsk, false),
    def("events.updates", "hear that an update is available, downloaded or applied, and that a download or install finished", P::None, B::MayAsk, false),
    def("events.flows", "hear that a flow or job finished or failed", P::None, B::MayAsk, false),
    def("events.files", "hear that a file in a folder it may read changed (the name, never the content)", P::None, B::MayAsk, false),
    def("events.models", "hear that the model host is ready, failed or changed model", P::None, B::MayAsk, false),
    def("events.voice", "hear the wake phrase (experimental, only while the person switched it on)", P::None, B::MayAsk, false),
    def("events.reminders", "hear that a reminder or a calendar entry is due", P::None, B::MayAsk, false),
    def("events.custom", "hear the events other apps publish", P::None, B::MayAsk, false),
    def("events.publish", "publish events of its own for other apps", P::None, B::MayAsk, false),
];

pub fn lookup(name: &str) -> Option<&'static CapabilityDef> {
    CATALOGUE.iter().find(|d| d.name == name)
}

/// Whether every use of this capability shows a card with Allow. Unknown names count as hard asks (fail closed).
pub fn is_hard_ask(name: &str) -> bool {
    lookup(name).is_none_or(|d| d.hard_ask)
}

// ------------------------------------------------------------------------------------------------ manifest

pub const MAX_REQUESTS: usize = 24;
pub const MAX_WHY: usize = 200;
pub const MAX_PARAM: usize = 200;

/// One line of the `capabilities` list in `app.json`: `{ "name": "camera.view", "why": "show the live view", "param": "front" }`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub name: String,
    /// Shown to the person on the install card.
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// Fields a newer release's manifest has: kept (charter rule 2, finding F-1) and reported with [`unknown_request_fields`].
    /// Unknown capability NAMES are still refused by [`validate_requests`].
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One note per entry that carries fields this release does not know ("camera.view: optional"), for the author and the audit log.
pub fn unknown_request_fields(reqs: &[CapabilityRequest]) -> Vec<String> {
    reqs.iter().filter(|r| !r.extra.is_empty()).map(|r| format!("{}: {}", r.name, r.extra.keys().cloned().collect::<Vec<_>>().join(", "))).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapError {
    Unknown(String),
    NoReason(String),
    BadParam { name: String, why: String },
    Duplicate(String),
    TooMany,
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapError::Unknown(n) => write!(f, "unknown capability {n:?}: the names are {}", CATALOGUE.iter().map(|d| d.name).collect::<Vec<_>>().join(", ")),
            CapError::NoReason(n) => write!(f, "capability {n:?} needs a \"why\" (1 to {MAX_WHY} characters): the person reads it when installing"),
            CapError::BadParam { name, why } => write!(f, "capability {name:?}: {why}"),
            CapError::Duplicate(n) => write!(f, "capability {n:?} is listed twice with the same parameter"),
            CapError::TooMany => write!(f, "at most {MAX_REQUESTS} capabilities"),
        }
    }
}

impl std::error::Error for CapError {}

fn plain(s: &str) -> bool {
    !s.chars().any(|c| c.is_control())
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 120 && h != "*" && !h.starts_with('.') && !h.ends_with('.') && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

fn check_param(d: &CapabilityDef, param: Option<&str>) -> Result<(), String> {
    let Some(p) = param else { return Ok(()) };
    if p.len() > MAX_PARAM || !plain(p) {
        return Err(format!("the parameter is at most {MAX_PARAM} characters of plain text"));
    }
    match d.param {
        P::None => Err("this capability takes no parameter".into()),
        P::Device | P::Filter | P::Schedule => Ok(()),
        P::UsbId => {
            let ok = p.split_once(':').is_some_and(|(v, q)| [v, q].iter().all(|h| h.len() == 4 && h.chars().all(|c| c.is_ascii_hexdigit())));
            if ok { Ok(()) } else { Err("a USB device is written vvvv:pppp in hex".into()) }
        }
        P::Folder => {
            if p.starts_with('/') && !p.split('/').any(|s| s == "..") { Ok(()) } else { Err("a folder is an absolute path without ..".into()) }
        }
        P::HostPort => {
            let (h, port) = p.rsplit_once(':').map_or((p, None), |(h, q)| (h, Some(q)));
            let port_ok = port.is_none_or(|q| q.parse::<u16>().is_ok_and(|n| n > 0));
            if host_ok(h) && port_ok { Ok(()) } else { Err("a network address is host or host:port, with no wildcards".into()) }
        }
        P::Port => {
            if p.parse::<u16>().is_ok_and(|n| n > 0) { Ok(()) } else { Err("a port is a number from 1 to 65535".into()) }
        }
    }
}

/// Check a manifest's capability list: known names, a reason for each, parameters that fit, no duplicates.
pub fn validate_requests(reqs: &[CapabilityRequest]) -> Result<(), CapError> {
    if reqs.len() > MAX_REQUESTS {
        return Err(CapError::TooMany);
    }
    let mut seen: Vec<(&str, Option<&str>)> = Vec::new();
    for r in reqs {
        let d = lookup(&r.name).ok_or_else(|| CapError::Unknown(r.name.clone()))?;
        let why = r.why.trim();
        if why.is_empty() || why.chars().count() > MAX_WHY || !plain(why) {
            return Err(CapError::NoReason(r.name.clone()));
        }
        check_param(d, r.param.as_deref()).map_err(|why| CapError::BadParam { name: r.name.clone(), why })?;
        let key = (r.name.as_str(), r.param.as_deref());
        if seen.contains(&key) {
            return Err(CapError::Duplicate(r.name.clone()));
        }
        seen.push(key);
    }
    Ok(())
}

/// How the install card shows one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardLine {
    pub name: String,
    /// "see pictures from a camera (front): show the live view"
    pub words: String,
    pub hard_ask: bool,
    /// Not held by the installed version (an update asks about these only).
    pub is_new: bool,
}

/// The card lines for a validated list. `held` is what the installed version already holds (empty on a first install).
pub fn card_lines(reqs: &[CapabilityRequest], held: &[Grant]) -> Vec<CardLine> {
    reqs.iter()
        .map(|r| {
            let gives = lookup(&r.name).map_or("(unknown)", |d| d.gives);
            let p = r.param.as_deref().filter(|p| !p.is_empty()).map(|p| format!(" ({p})")).unwrap_or_default();
            CardLine { name: r.name.clone(), words: format!("{gives}{p}: {}", r.why.trim()), hard_ask: is_hard_ask(&r.name), is_new: !covered(held, r) }
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ approval

/// One thing a person allowed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub name: String,
    /// `None` is "any" (every camera, every folder the role allows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
}

impl Grant {
    pub fn of(r: &CapabilityRequest) -> Grant {
        Grant { name: r.name.clone(), param: r.param.clone() }
    }
}

fn covered(held: &[Grant], r: &CapabilityRequest) -> bool {
    held.iter().any(|g| g.name == r.name && (g.param.is_none() || g.param == r.param))
}

/// Who approved what, when, for which app version: kept per installation, never shared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// The person who pressed Approve.
    pub who: String,
    pub app: String,
    pub app_version: String,
    pub app_sha256: String,
    /// Seconds since the epoch.
    pub at: i64,
    pub grants: Vec<Grant>,
    /// Taken away later (Settings, Apps); the broker checks this at every call.
    #[serde(default)]
    pub revoked: Vec<Grant>,
    /// Fields a newer release wrote: kept, and written back (charter rule 2).
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Approval {
    /// Whether the app holds this capability now (granted and not revoked).
    pub fn holds(&self, name: &str, param: Option<&str>) -> bool {
        let r = CapabilityRequest { name: name.into(), why: String::new(), param: param.map(str::to_string), extra: Default::default() };
        covered(&self.grants, &r) && !self.revoked.iter().any(|g| g.name == name && (g.param.is_none() || g.param.as_deref() == param))
    }

    /// Grants with an end: capability name to the Unix time it ends. Kept in the approval's own extra field `ends` (a map of name to seconds),
    /// so an older release keeps it and writes it back (charter rule 2). A value that is not a whole number is ignored.
    pub fn ends(&self) -> std::collections::BTreeMap<String, i64> {
        self.extra.get("ends").and_then(|v| v.as_object()).map(|o| o.iter().filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n))).collect()).unwrap_or_default()
    }

    /// Whether the app holds this capability at `now`: [`Approval::holds`] and the grant has not ended (F-SDK-217).
    pub fn holds_at(&self, name: &str, param: Option<&str>, now: i64) -> bool {
        self.holds(name, param) && self.ends().get(name).is_none_or(|end| *end > now)
    }

    /// Give a held capability an end (Unix seconds), or take the end away with `None`. Returns false when the app does not hold it now or an end
    /// in the past was asked for (`now`): a limit never gives anything back, it only shortens what was granted (F-SDK-217).
    pub fn set_end(&mut self, name: &str, end: Option<i64>, now: i64) -> bool {
        if !self.grants.iter().any(|g| g.name == name) || self.revoked.iter().any(|g| g.name == name && g.param.is_none()) || end.is_some_and(|e| e <= now) {
            return false;
        }
        let mut ends = self.extra.get("ends").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        match end {
            Some(e) => ends.insert(name.into(), e.into()),
            None => ends.remove(name),
        };
        if ends.is_empty() {
            self.extra.remove("ends");
        } else {
            self.extra.insert("ends".into(), serde_json::Value::Object(ends));
        }
        true
    }

    /// Take a capability away (all its parameters). The broker refuses it from the next call; an update that asks for it again asks the person again.
    /// Returns false when nothing changed (it was not granted, or already revoked).
    pub fn revoke(&mut self, name: &str) -> bool {
        if !self.grants.iter().any(|g| g.name == name) || self.revoked.iter().any(|g| g.name == name && g.param.is_none()) {
            return false;
        }
        self.revoked.push(Grant { name: name.into(), param: None });
        true
    }

    /// The audit line for the approval (never any content): who, what, when is the audit's own time, app and version and hash.
    pub fn audit_detail(&self) -> String {
        let names: Vec<String> = self.grants.iter().map(|g| g.param.as_ref().map_or_else(|| g.name.clone(), |p| format!("{}[{p}]", g.name))).collect();
        format!("{} {} sha256={} approved by {}: {}", self.app, self.app_version, self.app_sha256, self.who, if names.is_empty() { "nothing".to_string() } else { names.join(", ") })
    }
}

/// What the person must be asked at install or update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskPlan {
    /// The app asks for nothing, or for what it already holds (and everything held stays): no card.
    Nothing,
    /// A first install: all of it.
    All(Vec<CapabilityRequest>),
    /// An update that asks for more: only the new ones are marked.
    More(Vec<CapabilityRequest>),
}

/// Decide the card. `held` is the grants of the installed version (`None` for a first install). An update that asks for the same or less does not ask.
pub fn plan_ask(held: Option<&Approval>, reqs: &[CapabilityRequest]) -> AskPlan {
    if reqs.is_empty() {
        return AskPlan::Nothing;
    }
    match held {
        None => AskPlan::All(reqs.to_vec()),
        Some(a) => {
            let new: Vec<CapabilityRequest> = reqs.iter().filter(|r| !covered(&a.grants, r) || a.revoked.iter().any(|g| g.name == r.name)).cloned().collect();
            if new.is_empty() { AskPlan::Nothing } else { AskPlan::More(new) }
        }
    }
}

/// Make the record for an Approve. Grants are what the manifest asked for now (the old ones that are no longer asked for drop out).
pub fn record_approval(who: &str, app: &str, version: &str, sha256: &str, at: i64, reqs: &[CapabilityRequest]) -> Approval {
    Approval { who: who.into(), app: app.into(), app_version: version.into(), app_sha256: sha256.into(), at, grants: reqs.iter().map(Grant::of).collect(), revoked: vec![], extra: Default::default() }
}

/// Where approvals are kept, one per installation (an installation is an app in a scope, for example `tile/user/alice/notes`).
pub trait ApprovalBook {
    fn get(&self, installation: &str) -> Option<Approval>;
    fn put(&mut self, installation: &str, a: Approval);
    fn remove(&mut self, installation: &str);
}

/// The in-memory book: the fake for tests, and the shape a real one follows.
#[derive(Default, Debug)]
pub struct MemoryBook(pub std::collections::BTreeMap<String, Approval>);

impl ApprovalBook for MemoryBook {
    fn get(&self, installation: &str) -> Option<Approval> {
        self.0.get(installation).cloned()
    }
    fn put(&mut self, installation: &str, a: Approval) {
        self.0.insert(installation.into(), a);
    }
    fn remove(&mut self, installation: &str) {
        self.0.remove(installation);
    }
}

// ------------------------------------------------------------------------------------------------ built-in apps

/// What the built-in apps hold without an install card: a static table compiled into the image, so it is covered by the image's signature.
/// App ids are those of `iyakki_sec::policy::App::id()`. Capabilities with `BuiltinDefault::Yes` are held by every built-in app and are not listed.
pub const BUILTIN_TABLE: &[(&str, &[&str])] = &[("settings", &["bluetooth.use", "system.radio.set", "system.link.set", "system.startup.set", "system.brightness.set", "system.volume.set", "system.memory.set", "system.disk.install", "system.hostname.set", "system.crashlog.write", "fleet.read", "fleet.join", "fleet.manage", "fleet.assign", "fleet.receive"]), ("scan", &["net.scan"]), ("k8s", &["k8s.portforward"]), ("disks", &["system.disk.mount", "system.disk.partition", "system.disk.unlock"]), ("chat", &["router.use", "router.strategy.set", "microphone.listen", "fleet.read", "fleet.join", "fleet.manage", "fleet.assign", "fleet.receive"]), ("shell", &["system.brightness.set", "system.volume.set", "system.disk.mount"]), ("cameras", &["camera.view", "camera.record"]), ("camera", &["camera.view", "camera.record"]), ("checkup", &["camera.view", "microphone.listen"]), ("voice", &["microphone.listen"]), ("browser-headers", &["web.request.headers"]), ("browser-plugins", &["web.content.script", "web.user.agent"]), ("client.ui", &["events.network", "events.power", "events.session", "events.apps", "events.flows", "events.updates", "events.settings", "events.display", "events.locale", "events.clock", "events.storage", "events.devices"]), ("sample", &["flows.start", "tools.request", "events.network", "events.power", "events.session", "events.apps", "events.flows", "events.updates", "events.settings", "events.display", "events.locale", "events.clock", "events.files", "events.storage", "events.devices"])];

/// Whether a built-in app (the caller has already established that it is one) holds `cap` by default. Hard asks are never "held" in the sense of skipping the per-use card: this only says the app
/// may request them without an install card. Store and third-party apps are never here.
pub fn builtin_holds(app: &str, cap: &str) -> bool {
    let Some(d) = lookup(cap) else { return false };
    match d.builtin {
        B::Yes => true,
        B::TableOnly | B::MayAsk => BUILTIN_TABLE.iter().any(|(a, caps)| *a == app && caps.contains(&cap)),
        B::RolePolicy | B::No => false,
    }
}

// ------------------------------------------------------------------------------------------------ the older object form

/// What the older object form of `capabilities` (`{"storage": true, "network": [...]}`) means in the catalogue. Only a flag that is a device or system
/// access has a catalogue name; the rest (the app's own storage, the clipboard, the agent, models, the file picker) are not device access and are listed
/// in `unmapped` with the reason, so a card can say so instead of hiding them. Pure data; nothing is enforced from this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyMap {
    pub requests: Vec<CapabilityRequest>,
    /// `(flag, why it has no catalogue name)`.
    pub unmapped: Vec<(String, &'static str)>,
}

/// Map the flags that were set (`storage`, `clipboard`, `notifications`, `files`, `agent`, `llm`, `speech`) and the hosts of `network`.
pub fn map_legacy(flags: &[&str], hosts: &[String]) -> LegacyMap {
    let mut m = LegacyMap::default();
    for f in flags {
        match *f {
            "notifications" => m.requests.push(CapabilityRequest { name: "notify".into(), why: "leaves notes in the inbox (from the older permission list)".into(), param: None, extra: Default::default() }),
            "speech" => m.requests.push(CapabilityRequest { name: "speaker.play".into(), why: "reads text aloud (from the older permission list)".into(), param: None, extra: Default::default() }),
            "storage" => m.unmapped.push((f.to_string(), "its own private storage, not a device")),
            "clipboard" => m.unmapped.push((f.to_string(), "the shell clipboard, asked at each use by the shell")),
            "files" => m.unmapped.push((f.to_string(), "the file picker: the person chooses each file, so no folder is granted")),
            "agent" | "llm" => m.unmapped.push((f.to_string(), "the person's own assistant and model setup, not a device")),
            other => m.unmapped.push((other.to_string(), "no catalogue name")),
        }
    }
    for h in hosts {
        m.requests.push(CapabilityRequest { name: "net.connect".into(), why: "reaches this address (from the older permission list)".into(), param: Some(h.clone()), extra: Default::default() });
    }
    m
}

#[cfg(test)]
mod tests;
