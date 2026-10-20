//! Tile apps: a static web bundle (index.html, scripts, styles, pictures and an optional `app.json`) that runs inside a tile of the
//! launcher on IYAKKI's own browser engine. This module holds everything that does not draw: the manifest and its rules, the bundle and
//! its package payload, the lint, the app's private storage, the bridge (`window.iyakki`) with its quotas and rate limit, the
//! page that serves the app from the bundle on an origin of its own, and a headless runner for its tests.
//!
//! The rules (docs/APP-AUTHORING.md, "Tile apps"): the app's origin is `https://tile-<id>.iyakki.invalid/`, so it shares nothing with
//! any site or any other app; the only host modules it gets are the standard web ones and the bridge; the network is limited to the
//! hosts it declared AND the person's host policy allows (none by default); no `file:` address exists for it; code is never made from
//! text (`eval`, `new Function`); the vault is not offered to it at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_js::modules::{Bridge, BridgeModule};
use iyakki_js::{LivePage, Limits, SiteData};
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};
use serde_json::{json, Value};

pub const MANIFEST: &str = "app.json";
pub const DEFAULT_GROUP: &str = "My apps";
pub const MAX_FILES: usize = 64;
pub const MAX_FILE_BYTES: usize = 512 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 1536 * 1024;
pub const MAX_ICON_BYTES: usize = 12 * 1024;
/// What one app may keep per person.
pub const STORAGE_QUOTA: usize = 256 * 1024;
pub const STORAGE_KEYS: usize = 256;
pub const STORAGE_VALUE: usize = 65536;
/// Bridge calls per second before the app is told to slow down.
pub const RATE_PER_SEC: u32 = 200;
/// Requests to the person (file dialogs, questions) an app may have waiting at once.
pub const MAX_PENDING: usize = 4;

const TEXT_EXT: [&str; 9] = ["html", "js", "mjs", "css", "json", "svg", "txt", "md", "csv"];
const BIN_EXT: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "woff2", "ico"];

// ------------------------------------------------------------------------------------------------ manifest

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
}

impl Caps {
    /// The store vocabulary: `tile:storage` ... and `net:host`.
    pub fn permissions(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (on, n) in [(self.storage, "storage"), (self.clipboard, "clipboard"), (self.notifications, "notifications"), (self.files, "files"), (self.agent, "agent")] {
            if on {
                v.push(format!("tile:{n}"));
            }
        }
        v.extend(self.network.iter().map(|h| format!("net:{h}")));
        v
    }

    pub fn from_permissions(perms: &[String]) -> Caps {
        let has = |n: &str| perms.iter().any(|p| p == &format!("tile:{n}"));
        Caps { storage: has("storage"), clipboard: has("clipboard"), notifications: has("notifications"), files: has("files"), agent: has("agent"), network: perms.iter().filter_map(|p| p.strip_prefix("net:").map(str::to_string)).collect() }
    }

    pub fn all() -> Caps {
        Caps { storage: true, network: vec![], clipboard: true, notifications: true, files: true, agent: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// An emoji, or the path of a PNG or SVG inside the bundle.
    pub icon: String,
    pub group: String,
    pub caps: Caps,
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 120 && h != "*" && !h.starts_with('.') && !h.ends_with('.') && h.contains('.') | (h == "localhost") && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && !h.ends_with(".invalid")
}

fn emoji_like(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 8 && s.chars().all(|c| !c.is_ascii() && !c.is_control())
}

impl TileManifest {
    /// Read `app.json`. Every field is checked; the errors are in the words an app author needs.
    pub fn parse(text: &str) -> Result<TileManifest, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("{MANIFEST} is not valid JSON: {e}"))?;
        let obj = v.as_object().ok_or_else(|| format!("{MANIFEST} must be a JSON object"))?;
        let s = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        for k in obj.keys() {
            if !["id", "name", "version", "icon", "group", "capabilities", "description"].contains(&k.as_str()) {
                return Err(format!("{MANIFEST}: unknown field {k:?}"));
            }
        }
        let m = TileManifest { id: s("id"), name: s("name"), version: if s("version").is_empty() { "0.1.0".into() } else { s("version") }, icon: s("icon"), group: if s("group").is_empty() { DEFAULT_GROUP.into() } else { s("group") }, caps: Self::caps(obj.get("capabilities"))? };
        m.validate()?;
        Ok(m)
    }

    fn caps(v: Option<&Value>) -> Result<Caps, String> {
        let mut c = Caps::default();
        let Some(v) = v else { return Ok(c) };
        let o = v.as_object().ok_or("capabilities must be an object, like {\"storage\": true}")?;
        for (k, val) in o {
            let on = || val.as_bool().ok_or_else(|| format!("capability {k:?} is true or false"));
            match k.as_str() {
                "storage" => c.storage = on()?,
                "clipboard" => c.clipboard = on()?,
                "notifications" => c.notifications = on()?,
                "files" => c.files = on()?,
                "agent" => c.agent = on()?,
                "network" => {
                    let a = val.as_array().ok_or("capability \"network\" is a list of host names")?;
                    if a.len() > 8 {
                        return Err("capability \"network\": at most 8 hosts".into());
                    }
                    for h in a {
                        let h = h.as_str().ok_or("capability \"network\": host names are text")?.trim().to_ascii_lowercase();
                        if !host_ok(&h) {
                            return Err(format!("capability \"network\": {h:?} is not a plain host name (no wildcards, no addresses with paths)"));
                        }
                        c.network.push(h);
                    }
                }
                "vault" => return Err("tile apps may not use the vault: ask the person to type what they need, or keep it in iyakki.storage".into()),
                "js-eval" | "eval" => return Err("tile apps may not make code from text (eval, new Function): write the code in a script file".into()),
                other => return Err(format!("unknown capability {other:?}: storage, network, clipboard, notifications, files, agent")),
            }
        }
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !crate::skills::valid_skill_name(&self.id) {
            return Err("id: lowercase letters, digits and dashes, at most 48 characters (it names the app and its private origin)".into());
        }
        if self.name.is_empty() || self.name.chars().count() > super::manifest::MAX_TITLE || self.name.chars().any(|c| c.is_control()) {
            return Err(format!("name: 1 to {} characters", super::manifest::MAX_TITLE));
        }
        if !crate::pkg::valid_version(&self.version) {
            return Err("version: digits separated by dots, like 1.0.0".into());
        }
        if self.group.chars().count() > 24 || self.group.chars().any(|c| c.is_control()) {
            return Err("group: at most 24 characters".into());
        }
        if !self.icon.is_empty() && !emoji_like(&self.icon) && !((self.icon.ends_with(".png") || self.icon.ends_with(".svg")) && path_ok(&self.icon)) {
            return Err("icon: one emoji, or the path of a small .png or .svg in the app".into());
        }
        Ok(())
    }

    pub fn to_json(&self) -> Value {
        let mut caps = serde_json::Map::new();
        for (on, n) in [(self.caps.storage, "storage"), (self.caps.clipboard, "clipboard"), (self.caps.notifications, "notifications"), (self.caps.files, "files"), (self.caps.agent, "agent")] {
            if on {
                caps.insert(n.into(), json!(true));
            }
        }
        if !self.caps.network.is_empty() {
            caps.insert("network".into(), json!(self.caps.network));
        }
        json!({"id": self.id, "name": self.name, "version": self.version, "icon": self.icon, "group": self.group, "capabilities": caps})
    }

    /// The private origin of the app.
    pub fn origin_host(&self) -> String {
        format!("tile-{}.iyakki.invalid", self.id)
    }
}

fn path_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 100 && !p.starts_with('/') && !p.starts_with('.') && p.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.starts_with('.')) && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

// ------------------------------------------------------------------------------------------------ bundle

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileData {
    Text(String),
    Bytes(Vec<u8>),
}

impl FileData {
    pub fn len(&self) -> usize {
        match self {
            FileData::Text(t) => t.len(),
            FileData::Bytes(b) => b.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn bytes(&self) -> &[u8] {
        match self {
            FileData::Text(t) => t.as_bytes(),
            FileData::Bytes(b) => b,
        }
    }
}

fn ext(p: &str) -> String {
    p.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

pub fn content_type(p: &str) -> &'static str {
    match ext(p).as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        "csv" => "text/csv",
        _ => "text/plain; charset=utf-8",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: TileManifest,
    pub files: BTreeMap<String, FileData>,
}

impl Bundle {
    pub fn text(&self, path: &str) -> Option<&str> {
        match self.files.get(path) {
            Some(FileData::Text(t)) => Some(t),
            _ => None,
        }
    }

    pub fn total(&self) -> usize {
        self.files.values().map(FileData::len).sum()
    }

    /// Build a bundle from files already in memory (the built-in samples). `app.json` among them is the manifest.
    pub fn from_files(files: &[(&str, &str)], fallback_id: &str) -> Result<Bundle, String> {
        let mut map = BTreeMap::new();
        for (p, c) in files {
            map.insert(p.to_string(), FileData::Text(c.to_string()));
        }
        Bundle::assemble(map, fallback_id, "")
    }

    fn assemble(files: BTreeMap<String, FileData>, fallback_id: &str, fallback_title: &str) -> Result<Bundle, String> {
        let manifest = match files.get(MANIFEST) {
            Some(FileData::Text(t)) => TileManifest::parse(t)?,
            Some(_) => return Err(format!("{MANIFEST} must be text")),
            None => {
                let m = TileManifest { id: fallback_id.to_string(), name: if fallback_title.is_empty() { super::author::title_of(fallback_id) } else { fallback_title.to_string() }, version: "0.1.0".into(), icon: String::new(), group: DEFAULT_GROUP.into(), caps: Caps::default() };
                m.validate()?;
                m
            }
        };
        let b = Bundle { manifest, files };
        b.check_shape()?;
        Ok(b)
    }

    fn check_shape(&self) -> Result<(), String> {
        if self.files.len() > MAX_FILES {
            return Err(format!("too many files: {} (at most {MAX_FILES})", self.files.len()));
        }
        if !self.files.contains_key("index.html") {
            return Err("there is no index.html: the app starts there".into());
        }
        for (p, d) in &self.files {
            if !path_ok(p) {
                return Err(format!("{p:?}: file names use letters, digits and . _ - / only, no hidden files"));
            }
            if d.len() > MAX_FILE_BYTES {
                return Err(format!("{p} is larger than {} KB", MAX_FILE_BYTES / 1024));
            }
        }
        if self.total() > MAX_BUNDLE_BYTES {
            return Err(format!("the app is larger than {} KB in all", MAX_BUNDLE_BYTES / 1024));
        }
        Ok(())
    }

    /// Read a project folder. `tests/`, the README, the change log, the project file and anything hidden or private are left out.
    pub fn from_dir(dir: &Path) -> Result<Bundle, String> {
        let mut map = BTreeMap::new();
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, FileData>, depth: usize) -> Result<(), String> {
            for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(t) = e.file_type() else { continue };
                if t.is_symlink() || name.starts_with('.') || name.ends_with(".key") || name.ends_with(".iyp") {
                    continue;
                }
                let rel = e.path().strip_prefix(root).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
                if t.is_dir() {
                    if depth < 4 && !matches!(name.as_str(), "tests" | "node_modules" | "target" | "dist") {
                        walk(root, &e.path(), out, depth + 1)?;
                    }
                    continue;
                }
                if matches!(rel.as_str(), "iyakki-app.json" | "README.md" | "CHANGELOG.md" | "LICENSE") {
                    continue;
                }
                if out.len() >= MAX_FILES + 8 {
                    return Err(format!("too many files (at most {MAX_FILES})"));
                }
                let bytes = std::fs::read(e.path()).map_err(|e| format!("{rel}: {e}"))?;
                let x = ext(&rel);
                if TEXT_EXT.contains(&x.as_str()) {
                    out.insert(rel.clone(), FileData::Text(String::from_utf8(bytes).map_err(|_| format!("{rel} is not UTF-8 text"))?));
                } else if BIN_EXT.contains(&x.as_str()) {
                    out.insert(rel, FileData::Bytes(bytes));
                } else {
                    return Err(format!("{rel}: this kind of file is not allowed in a tile app (html, js, css, json, svg, txt, md, csv, png, jpg, gif, webp, woff2, ico)"));
                }
            }
            Ok(())
        }
        walk(dir, dir, &mut map, 0)?;
        let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let id = crate::workspace::slug(&id);
        Bundle::assemble(map, &id, "")
    }

    /// The package payload.
    pub fn to_payload(&self) -> String {
        let mut files = serde_json::Map::new();
        for (p, d) in &self.files {
            files.insert(p.clone(), match d {
                FileData::Text(t) => json!(t),
                FileData::Bytes(b) => json!({"b": crate::story::base64_encode(b)}),
            });
        }
        json!({"v": 1, "manifest": self.manifest.to_json(), "files": files}).to_string()
    }

    pub fn from_payload(payload: &str) -> Result<Bundle, String> {
        let v: Value = serde_json::from_str(payload).map_err(|e| format!("the app inside is not readable: {e}"))?;
        if v["v"] != 1 {
            return Err("the app inside is a newer format than this system understands".into());
        }
        let manifest = TileManifest::parse(&v["manifest"].to_string())?;
        let mut files = BTreeMap::new();
        for (p, d) in v["files"].as_object().ok_or("the app inside has no files")? {
            let data = match d {
                Value::String(t) => FileData::Text(t.clone()),
                Value::Object(o) => FileData::Bytes(o.get("b").and_then(Value::as_str).and_then(crate::models::b64_decode).ok_or_else(|| format!("{p}: not valid base64"))?),
                _ => return Err(format!("{p}: unreadable")),
            };
            files.insert(p.clone(), data);
        }
        let b = Bundle { manifest, files };
        b.check_shape()?;
        Ok(b)
    }

    /// The icon as a picture the launcher can draw (`None` for an emoji).
    pub fn icon_file(&self) -> Option<(&str, &FileData)> {
        let i = self.manifest.icon.as_str();
        self.files.get(i).map(|d| (i, d))
    }
}

// ------------------------------------------------------------------------------------------------ lint

/// Everything wrong or doubtful about a bundle, before it is packed or installed: (problems, warnings).
pub fn lint(b: &Bundle) -> (Vec<String>, Vec<String>) {
    let (mut problems, mut warnings) = (Vec::new(), Vec::new());
    let m = &b.manifest;
    if m.icon.is_empty() {
        problems.push("no icon: set \"icon\" in app.json to an emoji or a small .png or .svg".into());
    } else if !emoji_like(&m.icon) {
        match b.files.get(&m.icon) {
            None => problems.push(format!("the icon file {} is not in the app", m.icon)),
            Some(d) if d.len() > MAX_ICON_BYTES => problems.push(format!("the icon is larger than {} KB", MAX_ICON_BYTES / 1024)),
            _ => {}
        }
    }
    let declared: Vec<&str> = m.caps.network.iter().map(String::as_str).collect();
    for (p, d) in &b.files {
        let FileData::Text(t) = d else { continue };
        if matches!(ext(p).as_str(), "html" | "js" | "mjs" | "css" | "svg") {
            for (line, host) in remote_refs(t) {
                if host == m.origin_host() {
                    continue;
                }
                if !declared.contains(&host.as_str()) {
                    problems.push(format!("{p} line {line} loads or calls {host}, which is not in capabilities.network: declare it, or bring the file inside the app"));
                }
            }
            for (n, l) in t.lines().enumerate() {
                let lo = l.to_ascii_lowercase();
                if lo.contains("file://") {
                    problems.push(format!("{p} line {}: file: addresses do not exist for a tile app", n + 1));
                }
                if lo.contains("javascript:") {
                    problems.push(format!("{p} line {}: javascript: addresses are not allowed", n + 1));
                }
                if matches!(ext(p).as_str(), "js" | "mjs" | "html") && (contains_call(&lo, "eval") || lo.contains("new function(") || lo.contains("new function (")) {
                    problems.push(format!("{p} line {}: eval and new Function are blocked in tile apps (the page would throw)", n + 1));
                }
                if (lo.contains("localstorage") || lo.contains("sessionstorage")) && matches!(ext(p).as_str(), "js" | "mjs" | "html") {
                    warnings.push(format!("{p} line {}: localStorage lives only while the app is open; use iyakki.storage to keep data", n + 1));
                }
                if lo.contains("iyakki.vault") {
                    problems.push(format!("{p} line {}: there is no vault in the bridge", n + 1));
                }
            }
        }
        for (n, w) in super::vet::secrets_scan(t) {
            problems.push(format!("{p} line {n} looks like it holds {w}: remove it"));
        }
    }
    let uses = |needle: &str| b.files.iter().any(|(p, d)| matches!(d, FileData::Text(t) if matches!(ext(p).as_str(), "js" | "mjs" | "html") && t.contains(needle)));
    for (needle, on, cap) in [("iyakki.storage", m.caps.storage, "storage"), ("iyakki.clipboard", m.caps.clipboard, "clipboard"), ("iyakki.notify", m.caps.notifications, "notifications"), ("iyakki.host", m.caps.files, "files"), ("iyakki.agent", m.caps.agent, "agent")] {
        if uses(needle) && !on {
            problems.push(format!("the app calls {needle} but app.json does not declare capabilities.{cap}: the call would be refused"));
        }
    }
    if !b.files.keys().any(|p| p.ends_with(".js")) && !b.files.values().any(|d| matches!(d, FileData::Text(t) if t.contains("<script"))) {
        warnings.push("the app has no script: it will be a static page".into());
    }
    (problems, warnings)
}

fn contains_call(lo: &str, name: &str) -> bool {
    let mut rest = lo;
    while let Some(i) = rest.find(name) {
        let before = rest[..i].chars().last();
        let after = rest[i + name.len()..].trim_start().chars().next();
        if after == Some('(') && !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$') {
            return true;
        }
        rest = &rest[i + name.len()..];
    }
    false
}

/// `(line, host)` of every address with a host in this text: `http://`, `https://`, `//host/`, `wss://`.
fn remote_refs(t: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in t.lines().enumerate() {
        let lo = line.to_ascii_lowercase();
        for pat in ["https://", "http://", "wss://", "ws://", "=\"//", "='//", "url(//"] {
            let mut from = 0;
            while let Some(i) = lo[from..].find(pat) {
                let start = from + i + pat.len() - if pat.ends_with("//") && !pat.contains(':') { 0 } else { 0 };
                let host: String = lo[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').collect();
                // an XML namespace or a link to documentation inside a comment is not a request
                let is_ns = host == "www.w3.org" || host == "w3.org";
                if !host.is_empty() && !is_ns && host.contains('.') {
                    out.push((n + 1, host));
                }
                from = start.max(from + 1);
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ serving the bundle

/// Serves the app's own origin from the bundle and the declared hosts from the real network (still checked by the caller's policy).
pub struct TileFetch {
    pub bundle: Arc<Bundle>,
    pub inner: Arc<dyn Fetch>,
    /// Declared hosts the person's policy also allows.
    pub hosts: Vec<String>,
}

impl Fetch for TileFetch {
    fn get(&self, url: &Url) -> Result<Response, String> {
        if url.host == self.bundle.manifest.origin_host() {
            let p = url.path.trim_start_matches('/');
            let p = if p.is_empty() { "index.html" } else { p };
            return Ok(match self.bundle.files.get(p) {
                Some(d) => Response { status: 200, content_type: content_type(p).into(), body: d.bytes().to_vec(), ..Default::default() },
                None => Response { status: 404, content_type: "text/plain".into(), body: b"not in this app".to_vec(), ..Default::default() },
            });
        }
        if self.hosts.iter().any(|h| *h == url.host) {
            return self.inner.get(url);
        }
        Err(format!("this app may not reach {}", url.host))
    }
}

/// Which hosts a page of this app may reach: its own origin, and each declared host the person's policy allows.
pub fn reachable_hosts(m: &TileManifest, policy: &iyakki_sec::policy::Policy) -> Vec<String> {
    m.caps.network.iter().filter(|h| policy.allows_host(h)).cloned().collect()
}

// ------------------------------------------------------------------------------------------------ storage

/// An app's saved data for one person: a small key/value map with hard limits, in a private file (or in memory for tests).
pub struct TileStorage {
    path: Option<PathBuf>,
    map: BTreeMap<String, String>,
}

impl TileStorage {
    pub fn memory() -> TileStorage {
        TileStorage { path: None, map: BTreeMap::new() }
    }

    pub fn open(path: &Path) -> TileStorage {
        let map = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        TileStorage { path: Some(path.to_path_buf()), map }
    }

    pub fn used(&self) -> usize {
        self.map.iter().map(|(k, v)| k.len() + v.len()).sum()
    }

    pub fn get(&self, k: &str) -> Option<String> {
        self.map.get(k).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }

    pub fn set(&mut self, k: &str, v: &str) -> Result<(), String> {
        if v.len() > STORAGE_VALUE {
            return Err(format!("QuotaExceeded: a value is at most {} KB", STORAGE_VALUE / 1024));
        }
        let others: usize = self.map.iter().filter(|(x, _)| x.as_str() != k).map(|(x, y)| x.len() + y.len()).sum();
        if others + k.len() + v.len() > STORAGE_QUOTA {
            return Err(format!("QuotaExceeded: this app may keep {} KB", STORAGE_QUOTA / 1024));
        }
        if !self.map.contains_key(k) && self.map.len() >= STORAGE_KEYS {
            return Err(format!("QuotaExceeded: at most {STORAGE_KEYS} keys"));
        }
        self.map.insert(k.into(), v.into());
        self.save()
    }

    pub fn remove(&mut self, k: &str) -> Result<(), String> {
        self.map.remove(k);
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self.map).unwrap_or_default()).map_err(|e| e.to_string())?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, p).map_err(|e| e.to_string())
    }
}

/// Where an app's data for a person lives: the folder the lifecycle layer offers to keep or delete when the app is removed.
pub fn storage_file(store: &super::Store, scope: &crate::skills::Scope, name: &str, user: &str) -> PathBuf {
    store.dir().join("appdata").join("tile").join(super::install::scope_key(scope).replace('/', "_")).join(name).join(format!("{user}.json"))
}

// ------------------------------------------------------------------------------------------------ the bridge

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// The trust level label, or "unsigned dev app".
    pub trust: String,
    pub dev: bool,
}

/// A request the app made that needs the person: the shell shows it and answers with [`TileRuntime::resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    /// `host.openFile`, `host.saveFile`, `clipboard.read`, `clipboard.write`, `agent.ask`.
    pub kind: String,
    pub args: Value,
}

#[derive(Default)]
pub struct Outbox {
    pub toasts: Vec<String>,
    pub title: Option<String>,
    pub requests: Vec<Request>,
    pub notices: Vec<(String, String)>,
    /// Calls refused, for the info panel and the audit log.
    pub refused: Vec<String>,
    next: u32,
}

type Clock = Box<dyn Fn() -> u64 + Send>;

pub struct TileHost {
    info: AppInfo,
    caps: Caps,
    storage: TileStorage,
    out: Arc<Mutex<Outbox>>,
    clock: Clock,
    window: (u64, u32),
}

impl TileHost {
    pub fn new(info: AppInfo, caps: Caps, storage: TileStorage, out: Arc<Mutex<Outbox>>) -> TileHost {
        let t0 = std::time::Instant::now();
        TileHost { info, caps, storage, out, clock: Box::new(move || t0.elapsed().as_millis() as u64), window: (0, 0) }
    }

    /// A clock for tests.
    pub fn with_clock(mut self, c: impl Fn() -> u64 + Send + 'static) -> TileHost {
        self.clock = Box::new(c);
        self
    }

    fn deny(&self, what: &str) -> String {
        let m = format!("this app did not ask for {what}: add it to capabilities in app.json (the person sees it when installing)");
        if let Ok(mut o) = self.out.lock() {
            if o.refused.len() < 50 {
                o.refused.push(m.clone());
            }
        }
        m
    }

    fn need(&self, on: bool, what: &str) -> Result<(), String> {
        if on {
            Ok(())
        } else {
            Err(self.deny(what))
        }
    }

    fn pending(&self, kind: &str, args: Value) -> Result<Value, String> {
        let mut o = self.out.lock().map_err(|_| "busy".to_string())?;
        if o.requests.len() >= MAX_PENDING {
            return Err("too many requests are waiting for the person: wait for an answer".into());
        }
        o.next += 1;
        let id = o.next;
        o.requests.push(Request { id, kind: kind.into(), args });
        Ok(json!({"pending": id}))
    }
}

fn s<'a>(a: &'a Value, k: &str, max: usize) -> Result<&'a str, String> {
    let v = a.get(k).and_then(Value::as_str).ok_or_else(|| format!("{k} must be a string"))?;
    if v.chars().count() > max || v.contains('\0') {
        return Err(format!("{k} is too long"));
    }
    Ok(v)
}

fn key(a: &Value) -> Result<&str, String> {
    let k = s(a, "k", 64)?;
    if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c)) {
        return Err("a key is 1 to 64 letters, digits and . _ : -".into());
    }
    Ok(k)
}

impl Bridge for TileHost {
    fn call(&mut self, op: &str, a: &Value) -> Result<Value, String> {
        let now = (self.clock)();
        if now.saturating_sub(self.window.0) >= 1000 {
            self.window = (now, 0);
        }
        self.window.1 += 1;
        if self.window.1 > RATE_PER_SEC {
            return Err(format!("too many calls: at most {RATE_PER_SEC} a second"));
        }
        match op {
            "app.info" => Ok(json!({"id": self.info.id, "name": self.info.name, "version": self.info.version, "publisher": self.info.publisher, "trust": self.info.trust, "dev": self.info.dev, "capabilities": self.caps.permissions()})),
            "storage.get" => {
                self.need(self.caps.storage, "storage")?;
                Ok(self.storage.get(key(a)?).map_or(Value::Null, Value::String))
            }
            "storage.set" => {
                self.need(self.caps.storage, "storage")?;
                let k = key(a)?.to_string();
                self.storage.set(&k, s(a, "v", STORAGE_VALUE)?)?;
                Ok(Value::Null)
            }
            "storage.remove" => {
                self.need(self.caps.storage, "storage")?;
                let k = key(a)?.to_string();
                self.storage.remove(&k)?;
                Ok(Value::Null)
            }
            "storage.list" => {
                self.need(self.caps.storage, "storage")?;
                Ok(json!(self.storage.list()))
            }
            "ui.toast" => {
                let t = s(a, "text", 200)?.chars().filter(|c| !c.is_control()).collect::<String>();
                let mut o = self.out.lock().map_err(|_| "busy".to_string())?;
                if o.toasts.len() < 5 {
                    o.toasts.push(t);
                }
                Ok(Value::Null)
            }
            "ui.title" => {
                let t = s(a, "text", 60)?.chars().filter(|c| !c.is_control()).collect::<String>();
                self.out.lock().map_err(|_| "busy".to_string())?.title = Some(t);
                Ok(Value::Null)
            }
            "notify.show" => {
                self.need(self.caps.notifications, "notifications")?;
                let (t, b) = (s(a, "title", 80)?.to_string(), s(a, "body", 300).unwrap_or("").to_string());
                let mut o = self.out.lock().map_err(|_| "busy".to_string())?;
                if o.notices.len() < 5 {
                    o.notices.push((t, b));
                }
                Ok(Value::Null)
            }
            "host.openFile" => {
                self.need(self.caps.files, "files")?;
                self.pending(op, json!({}))
            }
            "host.saveFile" => {
                self.need(self.caps.files, "files")?;
                let name = s(a, "name", 80)?;
                if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
                    return Err("the file name is a plain name, without folders".into());
                }
                self.pending(op, json!({"name": name, "text": s(a, "text", 262144)?}))
            }
            "clipboard.read" => {
                self.need(self.caps.clipboard, "clipboard")?;
                self.pending(op, json!({}))
            }
            "clipboard.write" => {
                self.need(self.caps.clipboard, "clipboard")?;
                self.pending(op, json!({"text": s(a, "text", 65536)?}))
            }
            "agent.ask" => {
                self.need(self.caps.agent, "the assistant")?;
                self.pending(op, json!({"q": s(a, "q", 2000)?}))
            }
            other => Err(format!("no such bridge call: {}", other.chars().take(40).collect::<String>())),
        }
    }
}

// ------------------------------------------------------------------------------------------------ running an app

/// An app that is open: its page stays alive, and the shell draws it, clicks it and answers its requests.
pub struct TileRuntime {
    pub page: LivePage,
    pub out: Arc<Mutex<Outbox>>,
    pub bundle: Arc<Bundle>,
    origin: Url,
}

impl TileRuntime {
    /// Start the app. `web` is the network (only the declared hosts that `policy` allows are ever asked); `storage` is the person's data of this app.
    pub fn open(bundle: Bundle, info: AppInfo, storage: TileStorage, web: Arc<dyn Fetch>, policy: &iyakki_sec::policy::Policy) -> Result<TileRuntime, String> {
        let hosts = reachable_hosts(&bundle.manifest, policy);
        Self::open_with(bundle, info, storage, web, hosts, None)
    }

    pub fn open_with(bundle: Bundle, info: AppInfo, storage: TileStorage, web: Arc<dyn Fetch>, hosts: Vec<String>, clock: Option<Clock>) -> Result<TileRuntime, String> {
        let origin = Url::parse(&format!("https://{}/", bundle.manifest.origin_host()))?;
        let out = Arc::new(Mutex::new(Outbox::default()));
        let mut host = TileHost::new(info, bundle.manifest.caps.clone(), storage, out.clone());
        if let Some(c) = clock {
            host.clock = c;
        }
        let html = bundle.text("index.html").ok_or("no index.html")?.to_string();
        let bundle = Arc::new(bundle);
        let fetch: Arc<dyn Fetch> = Arc::new(TileFetch { bundle: bundle.clone(), inner: web, hosts: hosts.clone() });
        let own = bundle.manifest.origin_host();
        let allow = move |h: &str| h == own || hosts.iter().any(|x| x == h);
        let limits = Limits { memory_bytes: 96 << 20, wall: std::time::Duration::from_secs(5), ..Limits::default() };
        let page = LivePage::load(&html, &origin, fetch, allow, &SiteData::default(), limits, vec![Box::new(BridgeModule::new(Box::new(host)))]).ok_or("the page engine could not start")?;
        Ok(TileRuntime { page, out, bundle, origin })
    }

    /// The page as the reader engine lays it out.
    pub fn doc(&mut self) -> iyakki_web::Doc {
        let tree = self.page.tree();
        let bundle = self.bundle.clone();
        let own = bundle.manifest.origin_host();
        let sheet = move |u: &Url| -> Option<String> {
            (u.host == own).then(|| bundle.text(u.path.trim_start_matches('/')).map(str::to_string)).flatten()
        };
        let vp = iyakki_web::css::Viewport::default();
        iyakki_web::style::styled_doc(&tree, &self.origin, &vp, &sheet)
    }

    /// All the text on the page, as one line per block of text.
    pub fn page_text(&mut self) -> String {
        let t = self.page.tree();
        let body = t.find("body").unwrap_or(0);
        t.text_of(body).split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub fn click_control(&mut self, doc: &iyakki_web::Doc, idx: usize) -> Result<(), String> {
        let c = doc.controls.get(idx).ok_or("no such control")?;
        if c.disabled {
            return Ok(());
        }
        self.page.click(c.node).map_err(|e| e.to_string())
    }

    pub fn type_into(&mut self, doc: &iyakki_web::Doc, idx: usize, text: &str) -> Result<(), String> {
        let c = doc.controls.get(idx).ok_or("no such control")?;
        self.page.input(c.node, text).map_err(|e| e.to_string())
    }

    /// Click the first button whose label is `label` (what a person does; headless tests use it).
    pub fn press(&mut self, label: &str) -> Result<(), String> {
        let doc = self.doc();
        let i = doc.controls.iter().position(|c| matches!(c.kind, iyakki_web::doc::ControlKind::Button | iyakki_web::doc::ControlKind::Submit) && c.label == label).ok_or_else(|| format!("no button called {label:?}"))?;
        self.click_control(&doc, i)
    }

    /// The person answered a request (`ok` false: refused or failed, `value` says why).
    pub fn resolve(&mut self, id: u32, ok: bool, value: &str) {
        let v = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
        let _ = self.page.eval("resolve", &format!("__iyakkiResolve({id}, {ok}, {v})"));
        if let Ok(mut o) = self.out.lock() {
            o.requests.retain(|r| r.id != id);
        }
        self.page.advance(0);
    }

    pub fn take_requests(&self) -> Vec<Request> {
        self.out.lock().map(|o| o.requests.clone()).unwrap_or_default()
    }
}

// ------------------------------------------------------------------------------------------------ headless tests

const HARNESS: &str = r#"
globalThis.T = (function () {
  const results = [];
  const el = (sel) => { const e = document.querySelector(sel); if (!e) throw new Error('no element matches ' + sel); return e; };
  const fire = (e, t) => e.dispatchEvent(new Event(t, { bubbles: true, cancelable: true }));
  return {
    results,
    $: el,
    click(sel) { el(sel).click(); },
    type(sel, v) { const e = el(sel); e.value = String(v); fire(e, 'input'); fire(e, 'change'); },
    text(sel) { return el(sel).textContent.trim(); },
    exists(sel) { return !!document.querySelector(sel); },
    eq(a, b, msg) { if (a !== b) throw new Error((msg || 'expected equal') + ': ' + JSON.stringify(a) + ' !== ' + JSON.stringify(b)); },
    ok(c, msg) { if (!c) throw new Error(msg || 'expected true'); },
    test(name, fn) { try { fn(); results.push({ name, ok: true }); } catch (e) { results.push({ name, ok: false, error: String((e && e.message) || e) }); } },
  };
})();
"#;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestReport {
    pub passed: usize,
    pub failed: usize,
    pub lines: Vec<String>,
}

impl TestReport {
    pub fn ok(&self) -> bool {
        self.failed == 0 && self.passed > 0
    }

    pub fn text(&self) -> String {
        let mut o = self.lines.join("\n");
        o.push_str(&format!("\n{} passed, {} failed{}", self.passed, self.failed, if self.passed + self.failed == 0 { " (no tests ran: add tests/*.js that call T.test)" } else { "" }));
        o.trim_start().to_string()
    }
}

/// Run the app's `tests/*.js` on the real engine, headless. They share one page and use `T` (`T.click`, `T.type`, `T.text`, `T.eq`, `T.ok`, `T.test`) and `iyakki` as the app would.
pub fn run_tests(dir: &Path) -> Result<TestReport, String> {
    let bundle = Bundle::from_dir(dir)?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("tests")).map_err(|_| "there is no tests folder: scaffold one with tile_scaffold".to_string())?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
    files.sort();
    if files.is_empty() {
        return Err("tests/ has no .js files".into());
    }
    let info = AppInfo { id: bundle.manifest.id.clone(), name: bundle.manifest.name.clone(), version: bundle.manifest.version.clone(), publisher: "you".into(), trust: "test".into(), dev: true };
    let caps = bundle.manifest.caps.clone();
    let mut rt = TileRuntime::open_with(bundle, info, TileStorage::memory(), Arc::new(iyakki_web::fetch::NoNetwork), vec![], None)?;
    let mut rep = TestReport::default();
    for n in rt.page.report.notes.iter().chain(rt.page.report.console.iter().filter(|l| l.level == "error").map(|l| &l.text)) {
        rep.lines.push(format!("page: {n}"));
    }
    let _ = caps;
    rt.page.eval("harness", HARNESS).map_err(|e| format!("test harness: {e}"))?;
    for f in files {
        let code = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Err(e) = rt.page.eval(&name, &code) {
            rep.failed += 1;
            rep.lines.push(format!("FAIL {name}: {e}"));
        }
        rt.page.advance(50);
    }
    let r = rt.page.eval("results", "JSON.stringify(T.results)").map_err(|e| e.to_string())?;
    let list: Vec<Value> = serde_json::from_str(&r).unwrap_or_default();
    for t in list {
        if t["ok"] == true {
            rep.passed += 1;
            rep.lines.push(format!("ok   {}", t["name"].as_str().unwrap_or("")));
        } else {
            rep.failed += 1;
            rep.lines.push(format!("FAIL {}: {}", t["name"].as_str().unwrap_or(""), t["error"].as_str().unwrap_or("")));
        }
    }
    Ok(rep)
}
