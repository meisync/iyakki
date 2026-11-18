//! What developer mode does on the device, through the STORE's own pipeline (there is no second installer): review the bytes as the
//! STORE would with the source `developer:<computer>`, install the very bytes that were reviewed, remove only what developer mode
//! installed, list it, and pack a project folder on the computer's side. The listener (`iyakki_remote::devserver`) and the IDE's
//! "This device" target both call these.

use std::path::Path;

use ed25519_dalek::SigningKey;
use serde::Serialize;

use super::{DevMode, SOURCE_PREFIX};
use crate::pkg;
use crate::services::Services;
use crate::skills::Scope;
use crate::store::install::{scope_key, Actor, Confirm, Record};
use crate::store::Store;

/// The largest package developer mode takes (an `extension` may be 12 MiB; anything else is smaller).
pub const MAX_PACKAGE: usize = pkg::MAX_EXTENSION_PACKAGE_BYTES + 64 * 1024;

/// Who is acting: the paired computer's name and the account of this device it acts as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub computer: String,
    pub account: String,
}

impl Who {
    pub fn source(&self) -> String {
        format!("{SOURCE_PREFIX}{}", self.computer)
    }
}

/// The result of an install, for the computer.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Installed {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub level: String,
    pub scope: String,
    pub sha256: String,
    pub warnings: Vec<String>,
    pub permissions: Vec<String>,
    /// It was the same version again (a rebuild).
    pub reinstalled: bool,
}

/// Where an install goes when the computer does not say: the account's own apps, except tool servers (for everyone, as in the STORE).
pub fn default_scope(actor: &Actor, kind: &str) -> Scope {
    if kind == "mcp" { Scope::Shared } else { actor.default_scope() }
}

/// Install (or upgrade, or reinstall the same version of) a package sent by a paired computer. Refused: anything the STORE's review
/// refuses for this account, driver packs and kernel modules, command-line tools and `app` packages.
pub fn install(svc: &Services, store: &Store, dm: &DevMode, actor: &Actor, who: &Who, bytes: Vec<u8>, shared: bool) -> Result<Installed, String> {
    if bytes.len() > MAX_PACKAGE {
        return Err(format!("the package is larger than {} MiB", MAX_PACKAGE >> 20));
    }
    let text = String::from_utf8(bytes.clone()).map_err(|_| "this is not an IYAKKI package (not text)".to_string())?;
    let parsed = pkg::parse(&text).map_err(|e| format!("this is not a usable package: {e}"))?;
    let kind = parsed.manifest.kind.clone();
    if kind == "extension" && parsed.manifest.meta.as_deref().and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()).is_some_and(|m| m["ext_kind"] == "driver") {
        dm.log(&who.computer, &who.account, "dev.install", &format!("refused driver pack {}", parsed.manifest.name), false);
        return Err("driver packs and kernel modules cannot be installed from a developer computer: use the STORE's Drivers page on the device".into());
    }
    let scope = if shared { Scope::Shared } else { default_scope(actor, &kind) };
    let review = store.review(svc, actor, bytes, None, &who.source(), false, scope.clone(), false, &[]);
    if let Some(why) = &review.refusal {
        dm.log(&who.computer, &who.account, "dev.install", &format!("refused {kind}:{} {}: {why}", review.name, review.version), false);
        return Err(why.clone());
    }
    let reinstalled = review.installed.as_deref() == Some(review.version.as_str());
    let rec = store.install(svc, actor, &review, Confirm::all()).map_err(|e| {
        dm.log(&who.computer, &who.account, "dev.install", &format!("failed {kind}:{} {}: {e}", review.name, review.version), false);
        e.to_string()
    })?;
    let what = format!("{}:{} {} ({}, {})", rec.kind, rec.name, rec.version, rec.scope, &rec.sha256[..12.min(rec.sha256.len())]);
    dm.log(&who.computer, &who.account, "dev.install", &what, true);
    dm.app_log(&rec.name, &format!("{} {what} from {}", if reinstalled { "reinstalled" } else { "installed" }, who.computer));
    super::bump_reload(dm.data_dir(), &rec.name);
    Ok(Installed {
        kind: rec.kind,
        name: rec.name,
        version: rec.version,
        level: rec.level,
        scope: rec.scope,
        sha256: rec.sha256,
        warnings: review.warnings.iter().filter(|w| !w.starts_with("Developer build") && !w.starts_with("Version ")).cloned().collect(),
        permissions: review.permissions.iter().map(|p| p.words.clone()).collect(),
        reinstalled,
    })
}

/// `kind:name` or a bare name.
fn split_id(id: &str) -> (Option<&str>, &str) {
    match id.split_once(':') {
        Some((k, n)) => (Some(k), n),
        None => (None, id),
    }
}

/// What developer mode installed that this account can see (its own and the shared ones). With `all`, every store record it can see.
pub fn list(store: &Store, actor: &Actor, all: bool) -> Vec<Record> {
    let mine = scope_key(&Scope::User(actor.user.clone()));
    store.records().into_values().filter(|r| r.scope == "shared" || r.scope == mine).filter(|r| all || r.source.starts_with(SOURCE_PREFIX)).collect()
}

/// Remove something developer mode installed (and only that: a store install is removed in the STORE). Its saved data is kept.
pub fn uninstall(svc: &Services, store: &Store, dm: &DevMode, actor: &Actor, who: &Who, id: &str) -> Result<Record, String> {
    let (kind, name) = split_id(id.trim());
    let found: Vec<Record> = list(store, actor, true).into_iter().filter(|r| r.name == name && kind.is_none_or(|k| r.kind == k)).collect();
    let Some(rec) = found.iter().find(|r| r.source.starts_with(SOURCE_PREFIX)).cloned() else {
        let why = if found.is_empty() { format!("{id} is not installed for {}", actor.user) } else { format!("{id} was not installed from a developer computer: remove it in the STORE on the device") };
        dm.log(&who.computer, &who.account, "dev.uninstall", &format!("refused {id}: {why}"), false);
        return Err(why);
    };
    if found.iter().filter(|r| r.source.starts_with(SOURCE_PREFIX)).count() > 1 && kind.is_none() {
        return Err(format!("{name} is installed as more than one kind: say which, for example {}:{name}", rec.kind));
    }
    store.remove(svc, actor, &rec.kind, &rec.name, &rec.scope()).map_err(|e| {
        dm.log(&who.computer, &who.account, "dev.uninstall", &format!("failed {}:{}: {e}", rec.kind, rec.name), false);
        e
    })?;
    dm.log(&who.computer, &who.account, "dev.uninstall", &format!("{}:{} {} ({})", rec.kind, rec.name, rec.version, rec.scope), true);
    dm.app_log(&rec.name, &format!("removed {} by {}", rec.version, who.computer));
    super::bump_reload(dm.data_dir(), &rec.name);
    Ok(rec)
}

// -------------------------------------------------------------------------------------------- the computer's side

/// Pack a project folder into a package, signed with the developer key: a project with `iyakki-app.json` through the same packer as
/// `iyakki pkg pack` (any kind it supports); a plain tile app folder (`app.json` + `index.html`) as a `tile` package.
pub fn pack_folder(dir: &Path, key: &SigningKey) -> Result<String, String> {
    if dir.join(crate::store::vet::PROJECT_FILE).is_file() {
        return crate::store::author::pack_dir(dir, Some(key)).map(|(t, _)| t);
    }
    if dir.join(crate::store::tile::MANIFEST).is_file() && dir.join("index.html").is_file() {
        let b = crate::store::tile::Bundle::from_dir(dir)?;
        let (problems, _) = crate::store::tile::lint(&b);
        if let Some(p) = problems.first() {
            return Err(format!("the app fails its checks: {p}"));
        }
        let m = &b.manifest;
        let text = pkg::build_generic("tile", &m.id, &m.version, "UNLICENSED", &b.to_payload(), None, None, Some(key)).map_err(|e| e.to_string())?;
        return Ok(text);
    }
    Err(format!("{} is neither a project (iyakki-app.json) nor a tile app (app.json and index.html)", dir.display()))
}

/// A package file or a project folder, as the bytes to send.
pub fn package_bytes(path: &Path, key: &SigningKey) -> Result<Vec<u8>, String> {
    if path.is_dir() {
        return pack_folder(path, key).map(String::into_bytes);
    }
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() > MAX_PACKAGE {
        return Err(format!("{} is larger than {} MiB", path.display(), MAX_PACKAGE >> 20));
    }
    Ok(b)
}

/// A number that changes when a file under `dir` changes (for `push --watch`).
pub fn folder_stamp(dir: &Path) -> u64 {
    crate::store::tilehub::dev_stamp(dir)
}
