//! Which tile apps a person has, and how one is opened: the apps installed through the store (their own and the shared ones, each
//! verified again), and the unsigned dev apps an administrator added from a workspace folder (Developer mode). The launcher, the IDE
//! and the assistant's tools all read this one list.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use super::install::record_key;
use super::tile::{AppInfo, Bundle, FileData, TileHost, TileRuntime, TileStorage};
use super::tilesdk::TileSdk;
use super::Store;
use crate::services::Services;
use crate::skills::Scope;

pub use iyakki_shared::store_tilehub::{allowed, TileEntry, DEVELOPER_TRUST, DEV_TRUST};

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
    TileEntry { key, name: m.id.clone(), title: m.name.clone(), icon: if pic.is_some() { String::new() } else { m.icon.clone() }, icon_picture: pic, group: m.group.clone(), version: version.into(), publisher, trust, dev, dir, scope, caps: m.caps.clone(), wasm: b.wasm_module().is_some() }
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
    let sounds = super::sdkv2_playback::files_of_bundle(&app, &bundle);
    // the model, the voice and the usage log of this system, for the apps that declared `llm` or `speech`
    TileRuntime::open_sdk(bundle, info, TileStorage::open(&storage_path), web, hosts, None, |out| Some(TileSdk::for_services_with_files(svc, user, &app, out, policy, Some(sounds))))
}

pub use iyakki_shared::store_tilehub::dev_stamp;

/// Is this file text the shell may show as the app's source (used by the info panel)? Pictures are not.
pub fn is_text(d: &FileData) -> bool {
    matches!(d, FileData::Text(_))
}

/// Why a WASM tile does not start while the switch is off.
pub const WASM_OFF: &str = "WASM tiles are an experimental feature and are off. Turn them on in Settings -> Developer mode (key W), or ask the assistant to turn them on.";

/// Is the experimental WASM tile switch on (off by default)?
pub fn wasm_enabled(svc: &Services) -> bool {
    svc.settings.lock().map(|s| s.get().wasm_tiles).unwrap_or(false)
}

/// The pictures a `wasm` tile carries in its bundle (F-WASM-16), decoded into the tile's table once, with the table's caps. A tile with none
/// has an empty table; a refusal is a sentence for the person.
pub fn wasm_pictures(svc: &Arc<Services>, user: &str, e: &TileEntry) -> Result<iyakki_tile_host::ImageTable, String> {
    super::tile::picture_table(&bundle_of(svc, user, e)?.images())
}

/// Open a `wasm` tile: its module runs in the `iyakki-tile-host` sandbox (fuel, time and memory budgets, the SDK bridge only: the app
/// gets no tool access from this loader). Refused while the experimental switch is off, and for accounts that may not run tile apps.
pub fn open_wasm(svc: &Arc<Services>, policy: &Policy, user: &str, e: &TileEntry) -> Result<iyakki_tile_host::TileRunner, String> {
    if !allowed(policy) {
        return Err("your account may not run tile apps".into());
    }
    if !wasm_enabled(svc) {
        return Err(WASM_OFF.into());
    }
    let bundle = bundle_of(svc, user, e)?;
    let module = bundle.wasm_module().ok_or("this app has no WebAssembly module")?;
    let seed = e.name.bytes().fold(0x9e37_79b9u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    // the device backend: `call("sdk", op, json)` is the same bridge an HTML app's page has, with this person's limits, cards and audit
    let storage_path = super::tile::storage_file(&Store::for_services(svc), &e.scope, &e.name, user);
    let info = AppInfo { id: e.name.clone(), name: e.title.clone(), version: e.version.clone(), publisher: e.publisher.clone(), trust: e.trust.clone(), dev: e.dev };
    let out = Arc::new(std::sync::Mutex::new(super::tile::Outbox::default()));
    let sounds = super::sdkv2_playback::files_of_bundle(&e.name, &bundle);
    let sdk = Arc::new(TileSdk::for_services_with_files(svc, user, &e.name, out.clone(), policy, Some(sounds)));
    let host = TileHost::new(info, bundle.manifest.caps.clone(), TileStorage::open(&storage_path), out).with_sdk(sdk);
    let backend: Arc<dyn iyakki_tile_host::ServiceBackend> = super::wasm_services::WasmServices::new(host);
    iyakki_tile_host::TileRunner::load(module, iyakki_tile_host::TileManifest::new(&e.name, &[]), iyakki_tile_host::Budgets::default(), Some(backend), seed).map_err(|x| x.to_string())
}
