//! The tile apps the launcher shows (data only): the entry of one app, the capabilities it declared, and the rule that says who may run them.
//! The listing, the dev-app file and the opening stay in `iyakki_agent::store::tilehub`.

use std::path::PathBuf;

use serde_json::Value;

use crate::skills::Scope;

/// What the app declared it needs. Everything is off unless declared.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub storage: bool,
    /// Hosts the app may reach (exact names, no wildcards); empty: no network.
    pub network: Vec<String>,
    pub clipboard: bool,
    pub notifications: bool,
    pub files: bool,
    pub agent: bool,
    /// `iyakki.llm` and `iyakki.models`: a bare model through the person's own provider setup.
    pub llm: bool,
    /// `iyakki.speech.say`: reads text aloud.
    pub speech: bool,
    /// The capabilities of SDK 1.1 and later that are not flags above (`markdown`, `embed` and its hosts, `dash`, `flows`, `k3s`, `media`, `audio`,
    /// `microphone`, `money`): key -> the normalised value (`iyakki_core_api::sdkbridge`). Unknown keys are never kept here.
    pub v2: std::collections::BTreeMap<String, Value>,
    /// Capabilities of a newer IYAKKI that the app listed as optional: not granted, not shown, written back untouched.
    pub kept: std::collections::BTreeMap<String, Value>,
}

impl Caps {
    /// The store vocabulary: `tile:storage` ... and `net:host`.
    pub fn permissions(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (on, n) in [(self.storage, "storage"), (self.clipboard, "clipboard"), (self.notifications, "notifications"), (self.files, "files"), (self.agent, "agent"), (self.llm, "llm"), (self.speech, "speech")] {
            if on {
                v.push(format!("tile:{n}"));
            }
        }
        v.extend(self.network.iter().map(|h| format!("net:{h}")));
        v.extend(self.v2.iter().map(|(k, val)| iyakki_core_api::sdkbridge::permission_string(k, val)));
        v
    }

    /// Does the app hold this capability key (the original flags, `network` when it lists a host, or an SDK 1.2 grant)?
    pub fn holds(&self, key: &str) -> bool {
        match key {
            "storage" => self.storage,
            "clipboard" => self.clipboard,
            "notifications" => self.notifications,
            "files" => self.files,
            "agent" => self.agent,
            "llm" => self.llm,
            "speech" => self.speech,
            "network" => !self.network.is_empty(),
            k => self.v2.contains_key(k),
        }
    }

    /// What these older flags mean in the core capability catalogue (`appcaps::map_legacy`); shown, never enforced from.
    pub fn catalogue_view(&self) -> iyakki_core_api::appcaps::LegacyMap {
        let flags: Vec<&str> = [(self.storage, "storage"), (self.clipboard, "clipboard"), (self.notifications, "notifications"), (self.files, "files"), (self.agent, "agent"), (self.llm, "llm"), (self.speech, "speech")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
        iyakki_core_api::appcaps::map_legacy(&flags, &self.network)
    }

    pub fn from_permissions(perms: &[String]) -> Caps {
        let has = |n: &str| perms.iter().any(|p| p == &format!("tile:{n}"));
        Caps { storage: has("storage"), clipboard: has("clipboard"), notifications: has("notifications"), files: has("files"), agent: has("agent"), llm: has("llm"), speech: has("speech"), network: perms.iter().filter_map(|p| p.strip_prefix("net:").map(str::to_string)).collect(), v2: perms.iter().filter_map(|p| p.strip_prefix("tile:").and_then(iyakki_core_api::sdkbridge::parse_permission)).collect(), kept: Default::default() }
    }

    pub fn all() -> Caps {
        Caps { storage: true, network: vec![], clipboard: true, notifications: true, files: true, agent: true, llm: true, speech: true, v2: Default::default(), kept: Default::default() }
    }

    /// Every SDK 1.2 capability, with the widest parameter each takes (tests).
    pub fn all_v2() -> Caps {
        use serde_json::json;
        let mut c = Caps::all();
        for (k, v) in [("markdown", json!(true)), ("embed", json!(["markdown", "browser", "media", "dash"])), ("embed_hosts", json!(["docs.example.com"])), ("dash", json!(["prom"])), ("flows", json!(["backup"])), ("k3s", json!("manage")), ("media", json!(true)), ("audio", json!(true)), ("microphone", json!(true)), ("money", json!("USD"))] {
            c.v2.insert(k.into(), v);
        }
        c
    }
}

/// How a dev app is labelled everywhere it shows.
pub const DEV_TRUST: &str = "unsigned dev app";
/// How an app installed from a paired developer computer (developer mode, `devmode`) is labelled on its tile and in its panels.
pub const DEVELOPER_TRUST: &str = "Developer (local)";

#[derive(Clone, Debug)]
pub struct TileEntry {
    /// `tile/<scope>/<name>` for an installed app, `dev/<name>` for a dev app.
    pub key: String,
    pub name: String,
    pub title: String,
    /// An emoji, or empty when the icon is a picture (`icon_png`).
    pub icon: String,
    pub icon_picture: Option<(String, Vec<u8>)>,
    pub group: String,
    pub version: String,
    pub publisher: String,
    pub trust: String,
    pub dev: bool,
    pub dir: Option<PathBuf>,
    pub scope: Scope,
    pub caps: Caps,
    /// A `wasm` tile: the app carries a WebAssembly module (`"wasm"` in `app.json`) that the sandboxed tile host runs, only while
    /// Settings -> Developer mode -> WASM tiles (experimental) is on.
    pub wasm: bool,
}

impl TileEntry {
    /// "Calculator · custom app · unsigned dev app".
    /// Installed from a paired developer computer in developer mode (labelled Developer (local), never promoted).
    pub fn is_developer(&self) -> bool {
        self.trust == DEVELOPER_TRUST
    }

    pub fn detail_line(&self) -> String {
        format!("custom app · {} · {}", if self.publisher.is_empty() { "unknown publisher" } else { &self.publisher }, self.trust)
    }
}

/// May this account run tile apps at all? They run in the browser's sandbox, so whoever may not use the browser may not use them.
pub fn allowed(p: &iyakki_sec::policy::Policy) -> bool {
    p.apps.contains(&iyakki_sec::policy::App::Browser)
}


/// A number that changes when a file of a dev app changes (the tile reloads when it does).
pub fn dev_stamp(dir: &std::path::Path) -> u64 {
    fn walk(d: &std::path::Path, depth: usize, acc: &mut u64) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.') || n == "tests" {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if depth < 4 {
                    walk(&e.path(), depth + 1, acc);
                }
            } else {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
                *acc = acc.wrapping_mul(31).wrapping_add(t ^ m.len());
            }
        }
    }
    let mut acc = 0;
    walk(dir, 0, &mut acc);
    acc
}
