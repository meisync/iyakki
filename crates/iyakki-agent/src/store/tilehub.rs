//! Which tile apps a person has, and how one is opened: the apps installed through the store (their own and the shared ones, each
//! verified again), and the unsigned dev apps an administrator added from a workspace folder (Developer mode). The launcher, the IDE
//! and the assistant's tools all read this one list.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_sec::policy::{App, Policy};
use serde::{Deserialize, Serialize};

use super::install::record_key;
use super::tile::{AppInfo, Bundle, Caps, FileData, TileRuntime, TileStorage};
use super::tilesdk::TileSdk;
use super::Store;
use crate::services::Services;
use crate::skills::Scope;

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
pub fn allowed(p: &Policy) -> bool {
    p.apps.contains(&App::Browser)
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
struct DevFile {
    apps: Vec<DevApp>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DevApp {
    pub user: String,
    pub name: String,
    pub dir: String,
    pub added: i64,
}

fn dev_path(svc: &Services) -> PathBuf {
    svc.dir().join("dev-tiles.json")
}

fn dev_apps(svc: &Services) -> Vec<DevApp> {
    std::fs::read_to_string(dev_path(svc)).ok().and_then(|t| serde_json::from_str::<DevFile>(&t).ok()).map(|f| f.apps).unwrap_or_default()
}

fn save_dev(svc: &Services, apps: Vec<DevApp>) -> Result<(), String> {
    std::fs::write(dev_path(svc), serde_json::to_vec_pretty(&DevFile { apps }).unwrap_or_default()).map_err(|e| e.to_string())
}

fn entry_of(b: &Bundle, key: String, version: &str, publisher: String, trust: String, dev: bool, dir: Option<PathBuf>, scope: Scope) -> TileEntry {
    let m = &b.manifest;
    let pic = b.icon_file().map(|(p, d)| (p.to_string(), d.bytes().to_vec()));
    TileEntry { key, name: m.id.clone(), title: m.name.clone(), icon: if pic.is_some() { String::new() } else { m.icon.clone() }, icon_picture: pic, group: m.group.clone(), version: version.into(), publisher, trust, dev, dir, scope, caps: m.caps.clone() }
}

/// The tile apps this person sees in the launcher: installed for them, installed for everyone, and (if they added them) dev apps.
pub fn installed(svc: &Services, policy: &Policy, user: &str) -> Vec<TileEntry> {
    if !allowed(policy) || !iyakki_sec::policy::valid_username(user) {
        return vec![];
    }
    let store = Store::for_services(svc);
    let recs = store.records();
    let mut out = Vec::new();
    for c in store.content("tile", user) {
        let Ok(b) = Bundle::from_payload(&c.payload) else { continue };
        let r = recs.get(&record_key("tile", &c.scope, &c.name));
        let (publisher, trust) = match r {
            Some(r) if r.level == super::trust::Level::Developer.id() => (r.publisher.clone(), DEVELOPER_TRUST.into()),
            Some(r) => (r.publisher.clone(), r.level.clone()),
            None => (String::new(), "Community".into()),
        };
        out.push(entry_of(&b, format!("tile/{}/{}", super::install::scope_key(&c.scope), c.name), &c.version, publisher, trust, false, None, c.scope.clone()));
    }
    for d in dev_apps(svc).into_iter().filter(|d| d.user == user) {
        let dir = PathBuf::from(&d.dir);
        if let Ok(b) = Bundle::from_dir(&dir) {
            out.push(entry_of(&b, format!("dev/{}", d.name), &b.manifest.version.clone(), "you".into(), DEV_TRUST.into(), true, Some(dir), Scope::User(user.into())));
        }
    }
    out.sort_by(|a, b| a.group.cmp(&b.group).then(a.title.to_lowercase().cmp(&b.title.to_lowercase())));
    out
}

/// Developer mode: add the app in a workspace folder to this administrator's launcher as an unsigned dev app. It is read from the folder
/// every time it opens, so edits show on Reload (the tile also reloads by itself when a file changes).
pub fn add_dev(svc: &Services, policy: &Policy, user: &str, dir: &Path) -> Result<TileEntry, String> {
    if !policy.manage_settings {
        return Err("Only an administrator can add an unsigned dev app. Pack, sign and install it from the STORE instead.".into());
    }
    if !allowed(policy) {
        return Err("your account may not run tile apps (they run in the browser's sandbox)".into());
    }
    let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let b = Bundle::from_dir(&dir)?;
    let (problems, _) = super::tile::lint(&b);
    if let Some(p) = problems.first() {
        return Err(format!("the app fails its checks, fix this first: {p}"));
    }
    let mut apps = dev_apps(svc);
    apps.retain(|d| !(d.user == user && d.name == b.manifest.id));
    apps.push(DevApp { user: user.into(), name: b.manifest.id.clone(), dir: dir.display().to_string(), added: crate::clock::now_unix() });
    save_dev(svc, apps)?;
    installed(svc, policy, user).into_iter().find(|e| e.dev && e.name == b.manifest.id).ok_or_else(|| "could not add it".into())
}

/// An app folder to look at right now without adding it to the launcher (the IDE's Run): marked unsigned dev app like the others.
pub fn preview_entry(user: &str, dir: &Path) -> Result<TileEntry, String> {
    let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let b = Bundle::from_dir(&dir)?;
    Ok(entry_of(&b, format!("preview/{}", b.manifest.id), &b.manifest.version.clone(), "you".into(), DEV_TRUST.into(), true, Some(dir), Scope::User(user.into())))
}

pub fn remove_dev(svc: &Services, user: &str, name: &str) -> Result<(), String> {
    let mut apps = dev_apps(svc);
    let n = apps.len();
    apps.retain(|d| !(d.user == user && d.name == name));
    if apps.len() == n {
        return Err(format!("{name} is not one of your dev apps"));
    }
    save_dev(svc, apps)
}

/// The bundle of an entry as it is now (a dev app is read from its folder again).
pub fn bundle_of(svc: &Services, user: &str, e: &TileEntry) -> Result<Bundle, String> {
    match &e.dir {
        Some(d) => Bundle::from_dir(d),
        None => {
            let store = Store::for_services(svc);
            let c = store.content("tile", user).into_iter().find(|c| c.name == e.name && c.scope == e.scope).ok_or("that app is no longer installed")?;
            Bundle::from_payload(&c.payload)
        }
    }
}

/// Open an app: its page, its bridge with this person's data, and the network it declared and the policy allows.
pub fn open(svc: &Arc<Services>, policy: &Policy, user: &str, e: &TileEntry, web: Arc<dyn iyakki_web::Fetch>) -> Result<TileRuntime, String> {
    if !allowed(policy) {
        return Err("your account may not run tile apps".into());
    }
    let bundle = bundle_of(svc, user, e)?;
    let storage_path = if e.dev {
        svc.dir().join("appdata").join("tile-dev").join(user).join(format!("{}.json", e.name))
    } else {
        super::tile::storage_file(&Store::for_services(svc), &e.scope, &e.name, user)
    };
    let info = AppInfo { id: e.name.clone(), name: e.title.clone(), version: e.version.clone(), publisher: e.publisher.clone(), trust: e.trust.clone(), dev: e.dev };
    let hosts = super::tile::reachable_hosts(&bundle.manifest, policy);
    let app = e.name.clone();
    // the model, the voice and the usage log of this system, for the apps that declared `llm` or `speech`
    TileRuntime::open_sdk(bundle, info, TileStorage::open(&storage_path), web, hosts, None, |out| Some(TileSdk::for_services(svc, user, &app, out)))
}

/// A number that changes when a file of a dev app changes (the tile reloads when it does).
pub fn dev_stamp(dir: &Path) -> u64 {
    fn walk(d: &Path, depth: usize, acc: &mut u64) {
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

/// Is this file text the shell may show as the app's source (used by the info panel)? Pictures are not.
pub fn is_text(d: &FileData) -> bool {
    matches!(d, FileData::Text(_))
}
