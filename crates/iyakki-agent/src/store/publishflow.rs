//! The Publish flow of the IDE (and the assistant's tools): from a workspace folder to a package in a private store, in steps the person
//! sees and approves. Everything that is not drawing lives here so it is tested without a window:
//!
//! 1. [`preflight`]: the checks, what can be fixed, a version bump that must beat what the store already has, a changelog line;
//! 2. [`stores`]: the stores added on this device, private ones first, with their fingerprints and the default publishing target;
//! 3. signing keys (`keys.rs`): the private key lives in the person's data folder, never in the workspace;
//! 4. the publish token: the vault entry `store-token-<catalog id>`, asked for in a masked dialog, never shown to the model;
//! 5. [`card`]: the review card (server, http or https, name, version, kind, permissions, trust it will get, size, files), then [`publish`];
//! 6. the publish log (`.iyakki/publish.json`: store, version, time, hash; no secret) and [`header_hint`].

use std::path::Path;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::author;
use super::catalog::{Fetch, SourceCfg, SourceKind};
use super::install::{Actor, Confirm, Review};
use super::manifest::Permission;
use super::{authtools, tile, vet, Store};
use crate::pkg::{self, newer};
use crate::services::Services;
use crate::skills::Scope;

/// What a withdrawal can and cannot do (shown on the result and in the docs).
pub const YANK_NOTE: &str = "Withdrawing: only the store's administrator can withdraw a package (DELETE /v1/packages/<name>). That removes it from the catalog and stops new installs on devices that refresh, but it does NOT remove copies already installed; to correct a mistake, publish a newer version.";

// ------------------------------------------------------------------------------------------------ 1. preflight

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn label(self) -> &'static str {
        match self {
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        }
    }

    pub fn from_label(s: &str) -> Option<Bump> {
        [Bump::Patch, Bump::Minor, Bump::Major].into_iter().find(|b| b.label() == s.trim().to_lowercase())
    }
}

/// `1.2.3` bumped: a patch raises the last number, a minor the second and clears the third, a major the first.
pub fn bumped(v: &str, b: Bump) -> Result<String, String> {
    if !pkg::valid_version(v) {
        return Err(format!("{v:?} is not a version (digits separated by dots)"));
    }
    let mut p: Vec<u64> = v.split('.').map(|x| x.parse().unwrap_or(0)).collect();
    while p.len() < 3 {
        p.push(0);
    }
    let i = match b {
        Bump::Major => 0,
        Bump::Minor => 1,
        Bump::Patch => 2,
    };
    p[i] += 1;
    for x in p.iter_mut().skip(i + 1) {
        *x = 0;
    }
    Ok(p.iter().map(u64::to_string).collect::<Vec<_>>().join("."))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    pub kind: String,
    pub name: String,
    pub version: String,
    /// Things to fix before anything can be published, each in plain words.
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
    /// For a tile app: the headless tests ran and passed (None: not run, e.g. not a tile app).
    pub tests: Option<bool>,
    /// `CHANGELOG.md` already has a line for this version.
    pub changelog_has_version: bool,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Run the checks of a project folder: `store_validate`/`tile_validate`, a title and summary, an icon for a tile app, the tests of a tile
/// app, and the change log.
pub fn preflight(dir: &Path) -> Preflight {
    let mut pf = Preflight::default();
    let rep = vet::vet_dir(dir);
    pf.problems.extend(rep.problems.clone());
    pf.warnings.extend(rep.warnings.clone());
    let Some(v) = vet::read_project(dir) else { return pf };
    pf.kind = str_of(&v, "kind");
    pf.name = str_of(&v, "name");
    pf.version = str_of(&v, "version");
    if str_of(&v, "title").is_empty() {
        pf.problems.push("add a title in iyakki-app.json (what people see in the store)".into());
    }
    if str_of(&v, "summary").is_empty() {
        pf.problems.push("add a one-sentence summary in iyakki-app.json".into());
    }
    pf.warnings.retain(|w| w != "fill in title and summary");
    if pf.kind == "tile" {
        match tile::run_tests(dir) {
            Ok(r) => {
                pf.tests = Some(r.ok());
                if !r.ok() {
                    pf.problems.push(format!("the headless tests do not pass:\n{}", r.text()));
                }
            }
            Err(e) => {
                pf.tests = Some(false);
                pf.problems.push(format!("the headless tests could not run: {e}"));
            }
        }
    }
    let log = std::fs::read_to_string(dir.join("CHANGELOG.md")).unwrap_or_default();
    pf.changelog_has_version = !pf.version.is_empty() && log.contains(&pf.version);
    if !pf.changelog_has_version {
        pf.warnings.push(format!("CHANGELOG.md has no line for {}: add one (the Publish dialog can write it)", pf.version));
    }
    pf
}

fn replace_json_string(text: &str, key: &str, value: &str) -> Option<String> {
    let k = format!("\"{key}\"");
    let i = text.find(&k)?;
    let after = &text[i + k.len()..];
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    let q1 = rest.find('"')?;
    let q2 = rest[q1 + 1..].find('"')? + q1 + 1;
    let start = i + k.len() + colon + 1 + q1 + 1;
    let end = i + k.len() + colon + 1 + q2;
    Some(format!("{}{}{}", &text[..start], value, &text[end..]))
}

/// Raise the version in `iyakki-app.json` (and `app.json` of a tile app) and put a line for it at the top of `CHANGELOG.md`. Returns the new version.
pub fn apply_bump(dir: &Path, b: Bump, line: &str, today: &str) -> Result<String, String> {
    let v = vet::read_project(dir).ok_or("there is no iyakki-app.json here")?;
    let new = bumped(&str_of(&v, "version"), b)?;
    set_version(dir, &new)?;
    add_changelog(dir, &new, line, today)?;
    Ok(new)
}

pub fn set_version(dir: &Path, new: &str) -> Result<(), String> {
    for f in ["iyakki-app.json", tile::MANIFEST] {
        let p = dir.join(f);
        if let Ok(t) = std::fs::read_to_string(&p) {
            if let Some(n) = replace_json_string(&t, "version", new) {
                std::fs::write(&p, n).map_err(|e| crate::scope::explain_io(&e, &p, "write"))?;
            }
        }
    }
    Ok(())
}

pub fn add_changelog(dir: &Path, version: &str, line: &str, today: &str) -> Result<(), String> {
    let line: String = line.chars().filter(|c| !c.is_control()).take(200).collect();
    if line.trim().is_empty() {
        return Err("write one line saying what changed".into());
    }
    let p = dir.join("CHANGELOG.md");
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let entry = format!("## {version} - {today}\n- {}\n\n", line.trim());
    let new = match old.strip_prefix("# Changelog") {
        Some(rest) => format!("# Changelog\n\n{entry}{}", rest.trim_start_matches('\n')),
        None if old.trim().is_empty() => format!("# Changelog\n\n{entry}"),
        None => format!("{entry}{old}"),
    };
    std::fs::write(&p, new).map_err(|e| crate::scope::explain_io(&e, &p, "write"))
}

/// The newest version of `name` that a store lists, asked of its `catalog.json` (None: unreachable or not listed). Advisory: the store
/// checks again when it receives the package.
pub fn remote_version(fetch: &dyn Fetch, server: &str, name: &str) -> Option<String> {
    let url = format!("{}/catalog.json", server.trim_end_matches('/').trim_end_matches("/catalog.json"));
    let body = fetch.get(&url, super::catalog::MAX_CATALOG_BYTES).ok()?;
    let v: Value = serde_json::from_slice(&body).ok()?;
    v["entries"].as_array()?.iter().filter(|e| e["name"] == name).filter_map(|e| e["version"].as_str().map(str::to_string)).reduce(|a, b| if newer(&b, &a) { b } else { a })
}

/// The version must beat the store's.
pub fn check_version(local: &str, remote: Option<&str>, store: &str) -> Result<(), String> {
    match remote {
        Some(r) if !newer(local, r) => Err(format!("version {local} is not newer than {r}, which {store} already has: versions only go up. Raise it (the Publish dialog offers patch, minor and major; {} would do).", bumped(r, Bump::Patch).unwrap_or_default())),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ 2. stores

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreChoice {
    pub id: String,
    pub name: String,
    pub address: String,
    pub fingerprint: String,
    pub is_default: bool,
    /// A private store with a pinned key (the only kind that takes a publish).
    pub private: bool,
}

/// The stores added on this device: private ones that take publishes first, the default target first among them.
pub fn stores(store: &Store) -> Vec<StoreChoice> {
    let cfg = store.config();
    let mut v: Vec<StoreChoice> = cfg
        .sources
        .iter()
        .map(|s| StoreChoice { id: s.id.clone(), name: s.name.clone(), address: authtools::server_of(s), fingerprint: if s.pin.is_empty() { String::new() } else { super::server::fingerprint(&s.pin) }, is_default: cfg.publish_target == s.id, private: !s.pin.is_empty() && s.kind == SourceKind::Url })
        .collect();
    v.sort_by(|a, b| b.private.cmp(&a.private).then(b.is_default.cmp(&a.is_default)).then(a.name.cmp(&b.name)));
    v
}

// ------------------------------------------------------------------------------------------------ 4. token

pub fn token_present(svc: &Services, user: &str, source_id: &str) -> bool {
    match Store::for_services(svc).config().sources.iter().find(|s| s.id == source_id) {
        Some(src) => authtools::publish_token(svc, user, src).is_some(),
        None => svc.user_get(user, &authtools::token_entry(source_id)).is_ok(),
    }
}

/// Keep the publish token in the person's vault (the dialog's masked field comes straight here; the model never sees it).
pub fn save_token(svc: &Services, user: &str, source_id: &str, token: &str) -> Result<(), String> {
    let t = token.trim();
    if t.len() < 8 || t.len() > 200 || t.chars().any(|c| c.is_control() || c == ' ') {
        return Err("that does not look like a publish token".into());
    }
    svc.user_set(user, &authtools::token_entry(source_id), t).map_err(|e| format!("the vault could not keep it: {e}"))
}

// ------------------------------------------------------------------------------------------------ 5. the card and the publish

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub store: String,
    pub server: String,
    pub https: bool,
    pub http_warning: Option<String>,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub permissions: Vec<String>,
    /// The level the package will probably get, in words.
    pub trust: String,
    pub new_key: bool,
    pub key_warning: Option<String>,
    pub key_fingerprint: String,
    pub size: usize,
    pub files: Vec<String>,
    pub store_has: Option<String>,
    pub token_present: bool,
    pub yank_note: &'static str,
}

/// Everything the person sees before approving. Packs and signs the project (nothing is sent).
pub fn card(svc: &Services, user: &str, dir: &Path, src: &SourceCfg, key: &(SigningKey, String), fetch: &dyn Fetch) -> Result<(Card, String), String> {
    let pf = preflight(dir);
    if !pf.ok() {
        return Err(format!("fix these first:\n- {}", pf.problems.join("\n- ")));
    }
    let (text, _) = author::pack_dir(dir, Some(&key.0))?;
    let p = pkg::parse(&text).map_err(|e| e.to_string())?;
    let server = authtools::server_of(src);
    // the store must answer before anything is shown for approval: a deployed store whose pod is not ready is said plainly here
    if let Err(e) = fetch.get(&format!("{}/catalog.json", server.trim_end_matches('/')), super::catalog::MAX_CATALOG_BYTES) {
        return Err(super::privatepub::unreachable_for(svc, user, &src.location, &e));
    }
    let store_has = remote_version(fetch, &server, &p.manifest.name);
    check_version(&p.manifest.version, store_has.as_deref(), &src.name)?;
    let signer = p.manifest.signer.clone().unwrap_or_default();
    let known = known_signers(fetch, &server);
    let new_key = known.as_ref().is_none_or(|k| !k.contains(&signer));
    let permissions = super::manifest::Meta::parse(p.manifest.meta.as_deref().unwrap_or("{}")).map(|m| m.permissions.iter().filter_map(|x| Permission::parse(x).ok()).map(|x| x.words(user)).collect()).unwrap_or_default();
    let files = file_list(&p);
    let https = server.starts_with("https://");
    Ok((
        Card {
            store: src.name.clone(),
            server: server.clone(),
            https,
            http_warning: (!https).then(|| "This address is plain http: the publish token would travel unencrypted. Only a loopback or private-network address is accepted; prefer https.".to_string()),
            name: p.manifest.name.clone(),
            version: p.manifest.version.clone(),
            kind: p.manifest.kind.clone(),
            permissions,
            trust: if new_key { "Community (this store has not seen your key; until its administrator trusts it the package is listed as Community)".into() } else { "the level your earlier packages in this store have".into() },
            new_key,
            key_warning: new_key.then(|| format!("Your key {} is new to {}. The package will be listed as Community until an administrator adds the key to the store's trusted-keys.txt. Use “Copy key for the store admin”.", key.1, src.name)),
            key_fingerprint: key.1.clone(),
            size: text.len(),
            files,
            store_has,
            token_present: token_present(svc, user, &src.id),
            yank_note: YANK_NOTE,
        },
        text,
    ))
}

fn file_list(p: &pkg::Package) -> Vec<String> {
    match p.manifest.kind.as_str() {
        "tile" => tile::Bundle::from_payload(&p.payload).map(|b| b.files.iter().map(|(n, d)| format!("{n} ({} B)", d.len())).collect()).unwrap_or_default(),
        k => vec![format!("{k} payload ({} B)", p.payload.len())],
    }
}

/// The keys that signed the packages a store lists (a key it has never seen is new to it).
fn known_signers(fetch: &dyn Fetch, server: &str) -> Option<Vec<String>> {
    let url = format!("{}/catalog.json", server.trim_end_matches('/'));
    let body = fetch.get(&url, super::catalog::MAX_CATALOG_BYTES).ok()?;
    let v: Value = serde_json::from_slice(&body).ok()?;
    Some(v["entries"].as_array()?.iter().filter_map(|e| e["signer"].as_str().map(str::to_string)).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Ask for the token in the masked dialog, then try again.
    NeedsToken,
    /// There is no signing key: create one.
    NeedsKey,
    Refused(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::NeedsToken => write!(f, "no publish token for this store yet: enter it in the dialog (it goes into your vault)"),
            Stop::NeedsKey => write!(f, "you have no publisher signing key yet: create one first"),
            Stop::Refused(m) => write!(f, "{m}"),
        }
    }
}

/// The approved publish: version check against the store, pack and sign, POST, log. Call only after the person approved the card.
pub fn publish(svc: &Services, user: &str, dir: &Path, src: &SourceCfg, key: Option<&(SigningKey, String)>, fetch: &dyn Fetch, now: i64) -> Result<author::Published, Stop> {
    let key = key.ok_or(Stop::NeedsKey)?;
    let token = authtools::publish_token(svc, user, src).ok_or(Stop::NeedsToken)?;
    let pf = preflight(dir);
    if !pf.ok() {
        return Err(Stop::Refused(format!("fix these first:\n- {}", pf.problems.join("\n- "))));
    }
    let server = authtools::server_of(src);
    check_version(&pf.version, remote_version(fetch, &server, &pf.name).as_deref(), &src.name).map_err(Stop::Refused)?;
    let (text, _) = author::pack_dir(dir, Some(&key.0)).map_err(Stop::Refused)?;
    let r = author::publish_to_server(&*svc.quick_upstream(), &server, &token, &text).map_err(Stop::Refused)?;
    record(dir, &src.name, &src.id, &pf.kind, &r, now);
    Ok(r)
}

/// Install what was just published, from the store, on this device: the normal review comes back for the person to confirm
/// (then [`Store::install`]). For a tile app the caller opens it afterwards.
pub fn review_published(svc: &Services, store: &Store, actor: &Actor, fetch: &dyn Fetch, name: &str, scope: Scope) -> Result<Review, String> {
    let r = store.refresh(svc, fetch);
    let items = store.items(svc, actor, &r.loaded);
    let item = items.iter().find(|i| i.entry.name == name).ok_or_else(|| format!("{name} is not in the catalog yet: check for updates in the STORE and try again"))?;
    store.review_item(svc, actor, item, &r.loaded, fetch, scope, &[])
}

pub fn install_reviewed(svc: &Services, store: &Store, actor: &Actor, review: &Review) -> Result<String, String> {
    store.install(svc, actor, review, Confirm::all()).map(|r| format!("Installed {} {} from {}", r.name, r.version, r.source)).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------------------------------------ 6. the log

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub store: String,
    pub store_id: String,
    pub name: String,
    pub kind: String,
    pub version: String,
    pub time: i64,
    pub sha256: String,
    pub trust: String,
    pub serial: u64,
}

fn log_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".iyakki").join("publish.json")
}

pub fn read_log(dir: &Path) -> Vec<LogEntry> {
    std::fs::read_to_string(log_path(dir)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| serde_json::from_value(v["entries"].clone()).ok()).unwrap_or_default()
}

fn record(dir: &Path, store: &str, store_id: &str, kind: &str, r: &author::Published, now: i64) {
    let mut e = read_log(dir);
    e.push(LogEntry { store: store.into(), store_id: store_id.into(), name: r.name.clone(), kind: kind.into(), version: r.version.clone(), time: now, sha256: r.sha256.clone(), trust: r.trust.clone(), serial: r.serial });
    let n = e.len();
    if n > 100 {
        e.drain(..n - 100);
    }
    let _ = std::fs::create_dir_all(dir.join(".iyakki"));
    let _ = std::fs::write(log_path(dir), serde_json::to_vec_pretty(&serde_json::json!({"entries": e})).unwrap_or_default());
}

/// The header line of a workspace: "Published: 1.0.2 → Office store", and when the workspace is ahead, the hint that it is not published yet.
pub fn header_hint(dir: &Path) -> Option<String> {
    let log = read_log(dir);
    let last = log.iter().reduce(|a, b| if newer(&b.version, &a.version) || (b.version == a.version && b.time >= a.time) { b } else { a })?;
    let line = format!("Published: {} \u{2192} {}", last.version, last.store);
    let local = vet::read_project(dir).map(|v| str_of(&v, "version"));
    Some(match local {
        Some(l) if newer(&l, &last.version) => format!("{line} (workspace is {l}: update available to publish)"),
        Some(l) if newer(&last.version, &l) => format!("{line} (workspace is {l}: outdated, the store has a newer one)"),
        _ => line,
    })
}
