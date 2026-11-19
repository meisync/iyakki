//! The lifecycle of what is installed: check, update to the latest, go back, pin, turn off, remove. One layer for every kind
//! (skills, WASM tools, MCP servers, command-line tools, themes, templates and the other content kinds, driver packs) that the
//! STORE tile, the terminal's `store` command and the assistant's `store_*` tools all call; none of them has rules of its own.
//!
//! What it guarantees:
//!
//! * the bytes that were reviewed are the bytes that are installed (the install checks again), and an update that asks for MORE
//!   permissions than the installed version is not applied without an explicit confirmation;
//! * versions only go up, except an explicit rollback, which is recorded; the previous version's verified package is kept for
//!   ONE generation (bounded disk) and nothing else is kept;
//! * an update is written as a transaction: a journal entry first, the new version, a check that it works (the skill parses, the
//!   tool's manifest loads, the MCP server starts and answers `tools/list` within a time limit, the driver pack is listed),
//!   and if the check fails the old version is put back; a cut-off in the middle is mended by [`Store::recover`];
//! * every change is written to the lifecycle log (`<store>/lifecycle.log`) and handed to the caller's audit.
//!
//! Platform services (Gitea, the private store) are listed here with their pinned upgrade path, but they are upgraded and removed
//! only through the platform engine, whose plan names the cluster and which always asks.

pub use iyakki_shared::store_lifecycle::{Ctx, Life, LifeError, RemovePlan, Report, UpdatePlan};
pub use iyakki_shared::store_lifecycle::{Done, Hist, Pin};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use super::catalog::{Entry as CatEntry, Fetch, Loaded, Origin};
use super::install::{scope_from_key, scope_key, Actor, Confirm, InstallError, Perm, Record};
use super::installed::{Entry as IEntry, Status, Type};
use super::manifest::{canonical_set, Meta, Permission};
use super::{builtin, Store};
use crate::pkg::{self, newer};
use crate::services::Services;
use crate::skills::Scope;

pub use iyakki_shared::store_lifecycle::Check;

pub use iyakki_shared::store_lifecycle::RemoveOpts;

/// How long a freshly updated MCP server has to start and answer `tools/list`.
pub const MCP_CHECK_SECS: u64 = 10;
/// History lines kept per item.
const HISTORY_KEEP: usize = 12;
/// The lifecycle log is cut to its last half when it passes this size.
const LOG_CAP: u64 = 256 * 1024;

// ------------------------------------------------------------------------------------------------ what is kept


#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Prev {
    pub version: String,
    pub sha256: String,
    pub at: i64,
}


#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    pub error: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Journal {
    pub key: String,
    pub from_sha: String,
    pub to_sha: String,
    pub action: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct LifeState {
    pub pins: BTreeMap<String, Pin>,
    pub prev: BTreeMap<String, Prev>,
    pub history: BTreeMap<String, Vec<Hist>>,
    /// The vendor's newest version of each command-line tool, as last asked.
    pub cli_latest: BTreeMap<String, Latest>,
    pub journal: Option<Journal>,
}

// ------------------------------------------------------------------------------------------------ the view




#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceLife {
    pub id: String,
    pub title: String,
    pub version: String,
    pub latest: Option<String>,
    pub connected: bool,
    pub note: String,
}

// ------------------------------------------------------------------------------------------------ errors and plans











// ------------------------------------------------------------------------------------------------ helpers

/// Compare two version texts the way vendors write them (`v1.31.0`, `1.9.5`, `2.60.0-rc1`): numbers by position.
pub fn version_newer(new: &str, old: &str) -> bool {
    let key = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().unwrap_or(0)).collect() };
    let (mut a, mut b) = (key(new), key(old));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(160).collect()
}

fn actor_for(scope: &Scope) -> Actor {
    match scope {
        Scope::Shared => Actor::system(),
        Scope::User(u) => Actor { user: u.clone(), allowed: true, can_own: true, can_shared: false, admin: false },
    }
}

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if m.is_dir() {
        std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| dir_size(&e.path())).sum()
    } else {
        m.len()
    }
}

fn type_of_kind(kind: &str) -> &'static str {
    match kind {
        "skill" => "skill",
        "tool" => "WASM tool",
        "mcp" => "MCP server",
        "cli" => "command-line tool",
        "theme" => "theme",
        "template" => "template",
        "agent" => "agent preset",
        "tile" => "custom app",
        _ => "extension",
    }
}

// ------------------------------------------------------------------------------------------------ the store's methods

impl Store {
    fn life_path(&self) -> std::path::PathBuf {
        self.dir().join("lifecycle.json")
    }

    pub fn life_state(&self) -> LifeState {
        std::fs::read(self.life_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub(super) fn save_life(&self, s: &LifeState) -> Result<(), String> {
        super::install::write_private(&self.life_path(), &serde_json::to_vec_pretty(s).unwrap_or_default())
    }

    pub(crate) fn with_life(&self, f: impl FnOnce(&mut LifeState)) {
        let mut s = self.life_state();
        f(&mut s);
        let _ = self.save_life(&s);
    }

    /// Append to the lifecycle log (one line per change) and hand the same line to the caller's audit.
    pub fn life_log(&self, audit: &dyn Fn(&str, &str), who: &str, action: &str, what: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        audit(&format!("store.{action}"), what);
        let path = self.dir().join("lifecycle.log");
        if let Ok(m) = std::fs::metadata(&path) {
            if m.len() > LOG_CAP {
                if let Ok(t) = std::fs::read_to_string(&path) {
                    let _ = std::fs::write(&path, &t[t.len() / 2..]);
                }
            }
        }
        let _ = std::fs::create_dir_all(self.dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&path) {
            let _ = writeln!(f, "{} {who} {action} {}", self.now(), what.replace('\n', " "));
        }
    }

    /// Whether a kept package copy is still needed by an installed record or as someone's previous version.
    fn copy_needed(&self, sha: &str, state: &LifeState) -> bool {
        self.records().values().any(|r| r.sha256 == sha) || state.prev.values().any(|p| p.sha256 == sha)
    }

    /// Called by the install after a record was written: keep the replaced version for one generation, drop older ones.
    pub(super) fn after_install(&self, old: &Option<Record>, new: &Record, rollback: bool) {
        let mut st = self.life_state();
        let key = new.key();
        let h = st.history.entry(key.clone()).or_default();
        h.push(Hist { version: new.version.clone(), action: if rollback { "rolled back".into() } else if old.is_some() { "updated".into() } else { "installed".into() }, at: self.now() });
        if h.len() > HISTORY_KEEP {
            h.remove(0);
        }
        if let Some(o) = old.as_ref().filter(|o| o.sha256 != new.sha256) {
            if let Some(displaced) = st.prev.insert(key.clone(), Prev { version: o.version.clone(), sha256: o.sha256.clone(), at: self.now() }) {
                if displaced.sha256 != o.sha256 && !self.copy_needed(&displaced.sha256, &st) {
                    let _ = std::fs::remove_file(self.pkg_copy(&displaced.sha256));
                }
            }
        }
        if rollback {
            st.pins.insert(key, Pin { version: new.version.clone(), at: self.now(), why: "rolled back".into() });
        } else {
            st.pins.remove(&key);
        }
        let _ = self.save_life(&st);
    }

    /// Called by `remove`: its pin, history and kept previous copy go too.
    pub(super) fn after_remove(&self, rec: &Record) {
        let mut st = self.life_state();
        let key = rec.key();
        st.pins.remove(&key);
        st.history.remove(&key);
        super::approvals::FileBook(self).remove_with_members(&key);
        let prev = st.prev.remove(&key);
        for sha in [Some(rec.sha256.clone()), prev.map(|p| p.sha256)].into_iter().flatten() {
            if !self.copy_needed(&sha, &st) {
                let _ = std::fs::remove_file(self.pkg_copy(&sha));
            }
        }
        let _ = self.save_life(&st);
    }

    // ---------------------------------------------------------------------------------------- pin

    /// Do not update this item. `version` defaults to the installed one.
    pub fn pin(&self, key: &str, version: &str) -> Result<(), String> {
        let mut st = self.life_state();
        st.pins.insert(key.to_string(), Pin { version: version.to_string(), at: self.now(), why: "pinned".into() });
        self.save_life(&st)
    }

    pub fn unpin(&self, key: &str) -> Result<(), String> {
        let mut st = self.life_state();
        if st.pins.remove(key).is_none() {
            return Err("It is not pinned.".into());
        }
        self.save_life(&st)
    }

    // ---------------------------------------------------------------------------------------- the list

    /// The best newer-or-equal candidate for an installed item: from the store that installed it, or from another store whose
    /// package is signed by the same key.
    fn candidate<'a>(&self, loaded: &'a [Loaded], kind: &str, name: &str, source: &str, signer: &Option<String>) -> Option<(&'a CatEntry, &'a Loaded)> {
        let mut best: Option<(&CatEntry, &Loaded)> = None;
        for l in loaded {
            for e in l.catalog.entries.iter().filter(|e| e.kind == kind && e.name == name && !e.demo && e.action.is_none()) {
                let same_source = l.source == source;
                let same_key = signer.as_ref().is_some_and(|k| !e.signer.is_empty() && e.signer == *k);
                if !(same_source || same_key) {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((b, bl)) => newer(&e.version, &b.version) || (e.version == b.version && l.source == source && bl.source != source),
                };
                if better {
                    best = Some((e, l));
                }
            }
        }
        best
    }

    /// Everything installed with its update state (reads the catalogs already on the device; no network).
    pub fn lifecycle(&self, svc: &Services, policy: &Policy, user: &str, loaded: &[Loaded], ctx: &Ctx) -> Vec<Life> {
        let actor = Actor::from_policy(user, policy);
        let items = self.items(svc, &actor, loaded);
        let inst = self.installed(svc, policy, user, &items);
        let st = self.life_state();
        let recs = self.records();
        let cfg = self.config();
        let mut out = Vec::new();
        for e in inst {
            let rec = recs.get(&e.key);
            let mut life = Life {
                id: if e.ty == Type::App { format!("app:{}", e.name) } else { format!("{}:{}", e.kind, e.name) },
                source: rec.map(|r| r.source.clone()).unwrap_or_default(),
                source_name: e.source.clone(),
                latest: None,
                check: Check::UpToDate,
                changelog: String::new(),
                added_permissions: vec![],
                pinned: st.pins.get(&e.key).cloned(),
                previous: st.prev.get(&e.key).filter(|p| self.pkg_copy(&p.sha256).exists()).map(|p| p.version.clone()),
                history: st.history.get(&e.key).cloned().unwrap_or_default(),
                builtin: false,
                from_other: None,
                cat_source: String::new(),
                entry: e,
            };
            let e = life.entry.clone();
            if e.ty == Type::App {
                life.builtin = true;
                life.check = Check::BuiltIn;
            } else if e.ty == Type::Cli {
                match st.cli_latest.get(&e.name) {
                    Some(l) if l.error.is_empty() => {
                        life.latest = Some(l.version.clone());
                        life.check = if version_newer(&l.version, &e.version) { Check::Update } else { Check::UpToDate };
                    }
                    Some(l) => life.check = if crate::clis::def(&e.name).is_some_and(|d| matches!(d.version, crate::clis::VersionFrom::Latest)) { Check::NoWay(l.error.clone()) } else { Check::Offline(l.error.clone()) },
                    None => {
                        life.check = if crate::clis::def(&e.name).is_some_and(|d| matches!(d.version, crate::clis::VersionFrom::Latest)) {
                            Check::NoWay("The vendor publishes only \"latest\", with no version number to compare. Update installs the newest.".into())
                        } else {
                            Check::NotChecked
                        }
                    }
                }
                life.source = "vendor".into();
                life.source_name = format!("the vendor ({})", e.publisher);
            } else if e.status == Status::Revoked {
                life.check = Check::Revoked;
            } else {
                let shipped = rec.is_none() && (e.source == "came with IYAKKI" || e.source == "built into the system image");
                let signer = rec.and_then(|r| r.signer.clone()).or_else(|| match e.kind.as_str() {
                    "skill" => svc.packages.list(&e.scope).into_iter().find(|(n, _)| *n == e.name).and_then(|(_, i)| i.signer),
                    "tool" => svc.packages.list_tools(&e.scope).into_iter().find(|(n, _)| *n == e.name).and_then(|(_, i)| i.signer),
                    _ => None,
                });
                let by_hand = rec.is_none() && signer.is_none() && !shipped;
                if shipped {
                    life.builtin = true;
                    life.check = Check::BuiltIn;
                } else if by_hand {
                    life.check = Check::NoWay(if e.kind == "mcp" { "Added by hand in the MCP tile: there is no store to ask.".into() } else { "Written or installed by hand: there is no store to ask.".into() });
                } else if rec.is_some_and(|r| r.source.starts_with(crate::devmode::SOURCE_PREFIX)) {
                    life.check = Check::NoWay("Installed from a paired developer computer (developer mode): it is never updated from a store. Push a new build, or remove it and install the store's version.".into());
                } else if rec.is_some_and(|r| r.source == "file") {
                    life.check = Check::NoWay("Installed from a file: there is no store to ask. Install a newer file to update it.".into());
                } else if let Some(store) = rec.and_then(|r| self.withdrawn_by_source(&r.source, &r.name, &r.version, &r.sha256)) {
                    life.check = Check::Withdrawn(format!("{store} withdrew this version. It still runs, but it is no longer offered: remove it (`store remove {}`) or install the store's newer version.", life.id));
                } else if !pkg::valid_version(&e.version) {
                    life.check = Check::NoWay("Its version is not a number the store can compare.".into());
                } else {
                    let source = rec.map(|r| r.source.clone()).unwrap_or_default();
                    match self.candidate(loaded, &e.kind, &e.name, &source, &signer) {
                        Some((c, l)) => {
                            life.latest = Some(c.version.clone());
                            life.cat_source = l.source.clone();
                            if l.source != source && !source.is_empty() {
                                life.from_other = Some(l.name.clone());
                            }
                            if newer(&c.version, &e.version) {
                                life.check = Check::Update;
                                life.changelog = first_line(&c.meta.changelog);
                                let have: Vec<String> = e.permissions.clone();
                                life.added_permissions = c.meta.permissions.iter().filter_map(|p| Permission::parse(p).ok()).map(|p| p.words(user)).filter(|w| !have.contains(w)).collect();
                            }
                            if let (Origin::Remote { cached_at: Some(t), .. }, true) = (&l.origin, ctx.refresh_errors.iter().any(|(n, _)| *n == l.name)) {
                                let days = (self.now() - t).max(0) / 86_400;
                                life.check = Check::Offline(format!("{} could not be reached; the copy of its list is {days} day(s) old{}.", l.name, if life.check == Check::Update { " (it shows an update)" } else { "" }));
                            } else if l.check.expired {
                                life.check = Check::Offline(format!("{}'s list has expired. Check for updates to load a new one.", l.name));
                            }
                        }
                        None if source.is_empty() => life.check = Check::NoWay("Installed outside the store (from a file or the SKILLS tile): there is no store to ask. Install a newer file to update it.".into()),
                        None => {
                            let src = cfg.sources.iter().find(|s| s.id == source);
                            life.check = match src {
                                Some(s) if s.enabled => match ctx.refresh_errors.iter().find(|(n, _)| *n == s.name) {
                                    Some((_, why)) => Check::Offline(format!("{} could not be reached: {why}", s.name)),
                                    None => Check::Offline(format!("{} has not been downloaded yet. Check for updates.", s.name)),
                                },
                                Some(s) => Check::SourceGone(format!("{} is switched off.", s.name)),
                                None if source == "builtin" => Check::BuiltIn,
                                None => Check::SourceGone(format!("The store it came from ({source}) is no longer in your list.")),
                            };
                        }
                    }
                }
            }
            life.entry.status = match (&life.check, &life.entry.status) {
                (_, Status::Revoked) => Status::Revoked,
                (_, Status::Off) => Status::Off,
                (Check::Update, _) => Status::Update(life.latest.clone().unwrap_or_default()),
                _ => Status::On,
            };
            out.push(life);
        }
        out
    }

    /// Find one item by `kind:name`, a bare name, or the tile's key. A user's own item wins over a shared one of the same name.
    pub fn find_life<'a>(lives: &'a [Life], id: &str) -> Result<&'a Life, String> {
        let id = id.trim();
        let hits: Vec<&Life> = lives.iter().filter(|l| l.entry.key == id || l.id == id || l.entry.name == id).collect();
        match hits.as_slice() {
            [] => Err(format!("Nothing installed is called \"{id}\". `store list` shows what is.")),
            [one] => Ok(one),
            many => {
                let mine = many.iter().filter(|l| matches!(l.entry.scope, Scope::User(_))).collect::<Vec<_>>();
                if mine.len() == 1 {
                    return Ok(mine[0]);
                }
                Err(format!("\"{id}\" is ambiguous: {}. Use kind:name.", many.iter().map(|l| l.id.clone()).collect::<Vec<_>>().join(", ")))
            }
        }
    }

    pub fn outdated(lives: &[Life]) -> Vec<&Life> {
        lives.iter().filter(|l| l.has_update()).collect()
    }

    // ---------------------------------------------------------------------------------------- services (read-only here)

    /// The platform services the person deployed, with the newest version their pinned upgrade path knows. Upgrading and
    /// removing them is the platform engine's (it names the cluster, takes a backup and always asks).
    pub fn services(&self, svc: &Services, user: &str) -> Vec<ServiceLife> {
        crate::platform::installed::load(svc.dir(), user)
            .into_iter()
            .map(|i| {
                let latest = crate::platform::spec(&i.service).and_then(|d| d.versions().last().cloned()).filter(|v| *v != i.version && version_newer(v, &i.version));
                ServiceLife {
                    note: if i.connected { "Connected, not deployed by IYAKKI: upgrade it where it runs.".into() } else if latest.is_some() { "Upgrade with platform_upgrade or the SERVICES tile: the plan names the cluster, takes a backup first and always asks.".into() } else { "On the newest version IYAKKI knows.".into() },
                    id: i.id,
                    title: i.title,
                    version: i.version,
                    latest: latest.filter(|_| !i.connected),
                    connected: i.connected,
                }
            })
            .collect()
    }

    // ---------------------------------------------------------------------------------------- command-line tool news

    /// Ask the vendors for the newest version of each installed command-line tool (network: call it off the screen's thread).
    /// Tools whose vendor publishes only "latest" are noted as such. The answers are kept for the lists.
    pub fn check_cli_latest(&self, svc: &Services) -> Vec<(String, Result<String, String>)> {
        let mut out = Vec::new();
        for c in svc.clis.installed() {
            let r = svc.clis.check_latest(&c.id);
            out.push((c.id.clone(), r));
        }
        let now = self.now();
        self.with_life(|s| {
            for (id, r) in &out {
                s.cli_latest.insert(id.clone(), match r {
                    Ok(v) => Latest { version: v.clone(), error: String::new(), at: now },
                    Err(e) => Latest { version: String::new(), error: e.clone(), at: now },
                });
            }
        });
        out
    }

    // ---------------------------------------------------------------------------------------- update

    pub(super) fn scope_ok(actor: &Actor, e: &IEntry) -> Result<(), String> {
        if !actor.allowed {
            return Err("Your account may not use the STORE.".into());
        }
        match &e.scope {
            Scope::Shared if !actor.can_shared && e.ty != Type::Cli => Err("Only operators and administrators can change what is installed for everyone.".into()),
            Scope::User(u) if *u != actor.user => Err("That belongs to someone else.".into()),
            Scope::User(_) if !actor.can_own => Err("Your role does not allow this.".into()),
            _ => Ok(()),
        }
    }

    /// Download and review the newer version and say what would change. Slow (network): call it off the screen's thread.
    pub fn plan_update(&self, svc: &Services, actor: &Actor, life: &Life, loaded: &[Loaded], fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError> {
        let e = &life.entry;
        Self::scope_ok(actor, e).map_err(LifeError::Refused)?;
        if life.builtin {
            return Err(LifeError::Refused(format!("{} is part of IYAKKI and changes when the system is updated.", e.title)));
        }
        if let Some(p) = &life.pinned {
            return Err(LifeError::Refused(format!("{} is pinned at {}. Unpin it first to update.", e.title, p.version)));
        }
        if e.ty == Type::Cli {
            if !actor.admin {
                return Err(LifeError::Refused("Only administrators can update command-line tools.".into()));
            }
            let d = crate::clis::def(&e.name).ok_or_else(|| LifeError::Refused("That tool is not in the CLI list.".into()))?;
            let to = life.latest.clone().unwrap_or_else(|| "the newest".into());
            if life.check != Check::Update && !matches!(d.version, crate::clis::VersionFrom::Latest) {
                return Err(LifeError::Refused(format!("{} is up to date ({}).", e.title, e.version)));
            }
            let mut warnings = vec![];
            if let Some(p) = svc.clis.install_plan(&e.name) {
                warnings.push(p);
            }
            return Ok(UpdatePlan { key: e.key.clone(), title: e.title.clone(), ty: e.ty, from: e.version.clone(), to, review: None, added: vec![], removed: vec![], warnings, changelog: String::new(), from_other: None, cli: true });
        }
        if !life.has_update() {
            return Err(LifeError::Refused(match &life.check {
                Check::UpToDate => format!("{} is up to date ({}).", e.title, e.version),
                c => format!("{}: {} {}", e.title, c.label(), c.note()),
            }));
        }
        let source = self.records().get(&e.key).map(|r| r.source.clone()).unwrap_or_default();
        let signer = self.records().get(&e.key).and_then(|r| r.signer.clone());
        let (c, l) = self.candidate(loaded, &e.kind, &e.name, &source, &signer).ok_or_else(|| LifeError::Refused("No store offers a newer version now.".into()))?;
        let (bytes, builtin_src) = match &l.origin {
            Origin::Builtin => (builtin::package(&c.builtin).ok_or_else(|| LifeError::Failed("That built-in item has no package.".into()))?.as_bytes().to_vec(), true),
            o => (o.fetch_package(c, fetch).map_err(LifeError::Failed)?, false),
        };
        let review = self.review(svc, actor, bytes, Some(c), &l.source, builtin_src, e.scope.clone(), l.check.expired, &[]);
        if let Some(why) = &review.refusal {
            return Err(LifeError::Refused(why.clone()));
        }
        // what the installed version asks for, from its kept verified copy
        let old = self.records().get(&e.key).cloned();
        let old_perms: Vec<String> = match old.as_ref().and_then(|r| std::fs::read(self.pkg_copy(&r.sha256)).ok()) {
            Some(b) => self.review(svc, actor, b, None, &source, source == "builtin", e.scope.clone(), false, &[]).permissions.into_iter().map(|p| p.canonical).collect(),
            None => canonical_set(&e.permissions.iter().filter_map(|w| Permission::parse(w).ok().map(|p| p.canonical())).collect::<Vec<_>>()),
        };
        let mut added: Vec<Perm> = review.permissions.iter().filter(|p| !old_perms.contains(&p.canonical)).cloned().collect();
        // core capabilities the installed version was not approved for count like new permissions
        added.extend(review.core_lines.iter().filter(|l| l.is_new).map(|l| Perm { canonical: format!("core:{}", l.name), words: l.words.clone(), sensitive: l.hard_ask }));
        // the same for each bundle member's own capabilities
        for mc in &review.members {
            added.extend(mc.lines.iter().filter(|l| l.is_new).map(|l| Perm { canonical: format!("core:{}/{}", mc.id, l.name), words: format!("{}: {}", mc.name(), l.words), sensitive: l.hard_ask }));
        }
        let removed: Vec<String> = old_perms.iter().filter(|o| !review.permissions.iter().any(|p| &p.canonical == *o)).filter_map(|o| Permission::parse(o).ok().map(|p| p.words(&actor.user))).collect();
        let changelog = first_line(&review.meta.changelog);
        Ok(UpdatePlan { key: e.key.clone(), title: e.title.clone(), ty: e.ty, from: e.version.clone(), to: review.version.clone(), warnings: review.warnings.clone(), added, removed, changelog, from_other: life.from_other.clone(), cli: false, review: Some(review) })
    }

    /// Check that what was just installed works.
    pub fn post_check(&self, svc: &Services, kind: &str, name: &str, scope: &Scope) -> Result<String, String> {
        match kind {
            "skill" => svc.skills.get(scope, name).map(|_| "the skill loads".to_string()).ok_or_else(|| "the skill is not there or does not parse".to_string()),
            "tool" => svc.packages.get_tool(scope, name).map(|_| "the tool's manifest loads".to_string()).ok_or_else(|| "the tool does not load (hash, signature or manifest)".to_string()),
            "mcp" => {
                let s = svc.mcp.list().into_iter().find(|s| s.name == name).ok_or("the server is not registered")?;
                let mut c = crate::mcp::McpClient::connect(&s.spec(), Duration::from_millis(self.check_timeout_ms)).map_err(|e| format!("the server did not start: {e}"))?;
                let t = c.tools().map_err(|e| format!("the server did not answer tools/list: {e}"))?;
                Ok(format!("the server started and offers {} tool(s)", t.len()))
            }
            "extension" if super::drivers::Drivers::for_data(self.data_dir()).installed().iter().any(|r| r.id == name) => Ok("the driver pack is listed".into()),
            "extension" | "theme" | "template" | "agent" | "tile" => {
                let text = std::fs::read_to_string(self.content_file(scope, kind, name)).map_err(|_| "the installed file is missing")?;
                let p = pkg::parse(&text).map_err(|e| format!("the installed file does not verify: {e}"))?;
                (p.manifest.name == name).then(|| "the content verifies".to_string()).ok_or_else(|| "the installed file is not the one expected".to_string())
            }
            _ => Ok(String::new()),
        }
    }

    /// Apply one transaction: journal, run, check, and put the old version back if the check fails.
    fn transact(&self, svc: &Services, actor: &Actor, key: &str, to_sha: &str, action: &str, run: &dyn Fn() -> Result<Record, InstallError>) -> Result<Record, LifeError> {
        let old = self.records().get(key).cloned();
        let old_bytes = old.as_ref().and_then(|r| std::fs::read(self.pkg_copy(&r.sha256)).ok());
        let snapshot = self.life_state();
        // the older kept version is displaced by a successful update; if this one fails it must come back
        let prev_bytes = snapshot.prev.get(key).and_then(|p| std::fs::read(self.pkg_copy(&p.sha256)).ok());
        let mut st = snapshot.clone();
        st.journal = Some(Journal { key: key.into(), from_sha: old.as_ref().map(|r| r.sha256.clone()).unwrap_or_default(), to_sha: to_sha.into(), action: action.into(), at: self.now() });
        self.save_life(&st).map_err(LifeError::Failed)?;
        let result = run();
        let outcome = match result {
            Ok(new) => match self.post_check(svc, &new.kind, &new.name, &new.scope()) {
                Ok(_) => Ok(new),
                Err(why) => {
                    let back = self.restore(svc, actor, &old, &old_bytes, &snapshot, Some(&new.sha256), &prev_bytes);
                    Err(LifeError::Failed(format!("{} {} failed its check ({why}). {}", new.title, new.version, back)))
                }
            },
            Err(InstallError::Failed(m)) => {
                let back = self.restore(svc, actor, &old, &old_bytes, &snapshot, Some(to_sha), &prev_bytes);
                Err(LifeError::Failed(format!("{m}. {back}")))
            }
            Err(e) => Err(e.into()),
        };
        // done either way (a failed one was put back): the journal must not outlive the transaction
        self.with_life(|s| s.journal = None);
        outcome
    }

    /// Put the old version back exactly as it was (record, kept copy, pin, previous). Returns a sentence about it.
    #[allow(clippy::too_many_arguments)]
    fn restore(&self, svc: &Services, actor: &Actor, old: &Option<Record>, old_bytes: &Option<Vec<u8>>, snapshot: &LifeState, bad_sha: Option<&str>, prev_bytes: &Option<Vec<u8>>) -> String {
        let Some(old) = old else { return "Nothing was installed before, so there is nothing to put back.".into() };
        let Some(bytes) = old_bytes.clone() else { return format!("The kept copy of {} {} is missing, so it could not be put back automatically; install it again.", old.title, old.version) };
        let a = if matches!(old.scope(), Scope::Shared) { actor.clone() } else { actor_for(&old.scope()) };
        let r = self.review_rollback(svc, &Actor { admin: true, ..a.clone() }, bytes, &old.source, old.scope());
        let res = if r.refusal.is_some() { Err(InstallError::Refused(r.refusal.clone().unwrap_or_default())) } else { self.install(svc, &Actor { admin: true, ..a }, &r, Confirm::all()) };
        match res {
            Ok(_) => {
                let mut all = self.records();
                all.insert(old.key(), old.clone());
                let _ = self.save_records(&all);
                let mut s = snapshot.clone();
                s.journal = None;
                s.history.entry(old.key()).or_default().push(Hist { version: old.version.clone(), action: "kept after a failed update".into(), at: self.now() });
                let _ = self.save_life(&s);
                // the failed version's copy is not needed any more, and the older kept version comes back
                if let Some(bad) = bad_sha.filter(|b| *b != old.sha256 && !self.copy_needed(b, &s)) {
                    let _ = std::fs::remove_file(self.pkg_copy(bad));
                }
                if let (Some(p), Some(b)) = (s.prev.get(&old.key()), prev_bytes) {
                    if !self.pkg_copy(&p.sha256).exists() {
                        let _ = super::install::write_private(&self.pkg_copy(&p.sha256), b);
                    }
                }
                format!("{} {} was put back and is working.", old.title, old.version)
            }
            Err(e) => format!("Putting {} {} back failed too ({e}); install it again from the store.", old.title, old.version),
        }
    }

    /// Mend a cut-off update: if the journal says one was running, put the system in the state the records say, and re-check.
    pub fn recover(&self, svc: &Services) -> Vec<String> {
        let mut said = Vec::new();
        for m in svc.clis.repair_all() {
            said.push(format!("Repaired the command link for {m}."));
        }
        let st = self.life_state();
        let Some(j) = st.journal.clone() else { return said };
        let rec = self.records().get(&j.key).cloned();
        let old_bytes = std::fs::read(self.pkg_copy(&j.from_sha)).ok();
        let old_rec_matches = rec.as_ref().is_some_and(|r| r.sha256 == j.to_sha);
        let mut snapshot = st.clone();
        snapshot.journal = None;
        if old_rec_matches {
            // the record was written: it only needs its check
            if let Some(r) = &rec {
                if let Err(why) = self.post_check(svc, &r.kind, &r.name, &r.scope()) {
                    let old = rec.clone().map(|r| Record { sha256: j.from_sha.clone(), ..r });
                    // the old record's other fields are in the kept copy; rebuild it from the package header
                    let old = old.and_then(|o| old_bytes.as_ref().and_then(|b| pkg::parse(&String::from_utf8_lossy(b)).ok()).map(|p| Record { version: p.manifest.version, signer: p.manifest.signer, cert: p.manifest.cert, ..o }));
                    said.push(format!("An update of {} was cut off and the new version does not work ({why}). {}", r.title, self.restore(svc, &actor_for(&r.scope()), &old, &old_bytes, &snapshot, Some(&j.to_sha), &None)));
                } else {
                    said.push(format!("An update of {} was cut off after it was installed; it checks out.", r.title));
                }
            }
        } else if let (Some(r), Some(b)) = (rec.clone(), old_bytes.clone()) {
            // the record still says the old version: make the files agree with it
            said.push(format!("An update of {} was cut off before it finished. {}", r.title, self.restore(svc, &actor_for(&r.scope()), &rec, &Some(b), &snapshot, Some(&j.to_sha), &None)));
        }
        self.with_life(|s| s.journal = None);
        said
    }

    /// Apply a planned update. Permissions added by the new version need `confirm.permissions`.
    pub fn apply_update(&self, svc: &Services, actor: &Actor, plan: &UpdatePlan, confirm: Confirm, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        if plan.cli {
            let id = plan.key.trim_start_matches("cli//").to_string();
            svc.clis.install(&id, &actor.user).map_err(LifeError::Failed)?;
            let j = wait_cli(svc, &id, 1800);
            return match j {
                Ok(()) => {
                    let now = svc.clis.is_installed(&id).map(|i| i.version).unwrap_or_default();
                    self.with_life(|s| {
                        s.pins.remove(&plan.key);
                        let h = s.history.entry(plan.key.clone()).or_default();
                        h.push(Hist { version: now.clone(), action: "updated".into(), at: 0 });
                    });
                    self.life_log(audit, &actor.user, "update", &format!("{} {} -> {now}", plan.title, plan.from));
                    crate::appevent_sources::app_changed(&*svc.app_events, &actor.user, &id, "updated");
                    Ok(format!("{} is now {now} (was {}). The old version is kept for one rollback.", plan.title, plan.from))
                }
                Err(e) => {
                    self.life_log(audit, &actor.user, "update-failed", &format!("{}: {e}", plan.title));
                    Err(LifeError::Failed(format!("{e}. {} {} is still installed and working.", plan.title, plan.from)))
                }
            };
        }
        let review = plan.review.as_ref().ok_or_else(|| LifeError::Failed("There is nothing to apply.".into()))?;
        if !plan.added.is_empty() && !confirm.permissions {
            return Err(LifeError::NeedsConfirm(format!("{} {} asks for permissions the installed version does not have: {}. Confirm them to update.", plan.title, plan.to, plan.added.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; "))));
        }
        if (review.needs_warning_ack() || plan.from_other.is_some()) && !confirm.warnings {
            return Err(LifeError::NeedsConfirm(format!("Read the warnings first: {}", review.warnings.iter().cloned().chain(plan.from_other.iter().map(|o| format!("this version comes from {o}, not the store that installed it"))).collect::<Vec<_>>().join(" / "))));
        }
        let c = Confirm { permissions: !review.permissions.is_empty() || review.asks_core(), warnings: true };
        // the app's own data is copied first, so an update that goes wrong for the data can be put back (F-CORE-25)
        let snap = self.snapshot_before_update(&plan.key, &plan.from, &actor.user, audit);
        let new = self.transact(svc, actor, &plan.key, &review.sha256, "update", &|| self.install(svc, actor, review, c))?;
        if let Some(a) = self.approval(&new.key()).filter(|a| a.app_version == new.version && !a.grants.is_empty()) {
            audit("store.approve", &a.audit_detail());
        }
        self.life_log(audit, &actor.user, "update", &format!("{} {} -> {}", plan.title, plan.from, new.version));
        crate::appevent_sources::app_changed(&*svc.app_events, &actor.user, &plan.key, "updated");
        crate::appevent_sources::update_changed(&*svc.app_events, &plan.key, "applied");
        let moved = self.take_last_auto_migration(&plan.key);
        Ok(format!("{} is now {} (was {}). The old version is kept for one rollback.{snap}{moved}", plan.title, new.version, plan.from))
    }

    /// The most a snapshot taken before an update may hold; a bigger data folder is not copied and the update says so.
    pub const SNAPSHOT_MAX_BYTES: u64 = 256 << 20;
    /// How many snapshots an app keeps (older ones are moved aside, not deleted).
    pub const SNAPSHOTS_KEPT: usize = 3;

    /// Where the snapshots of one installed thing are kept.
    pub fn snapshot_root(&self, key: &str) -> PathBuf {
        self.dir().join("snapshots").join(key.replace('/', "_"))
    }

    /// What the automatic data move of the last update said, kept for the update's answer (one line per item, read once).
    pub(super) fn set_last_auto_migration(&self, key: &str, said: &str) {
        let _ = std::fs::write(self.dir().join("auto-migration").with_extension(key.replace('/', "_")), said);
    }

    pub(super) fn take_last_auto_migration(&self, key: &str) -> String {
        let p = self.dir().join("auto-migration").with_extension(key.replace('/', "_"));
        let t = std::fs::read_to_string(&p).unwrap_or_default();
        let _ = std::fs::remove_file(&p);
        t
    }

    /// The data folder of an installed thing, when it has one.
    pub fn app_data_dir(&self, key: &str) -> Option<PathBuf> {
        let r = self.records().get(key).cloned()?;
        let d = self.dir().join("appdata").join(&r.kind).join(r.scope.replace('/', "_")).join(&r.name);
        d.is_dir().then_some(d)
    }

    /// The snapshots of an installed thing's data, oldest first.
    pub fn data_snapshots(&self, key: &str) -> Vec<crate::bundlesnap::Snapshot> {
        crate::bundlesnap::list(&self.snapshot_root(key))
    }

    /// Put the data of `key` back as it was in snapshot `id`. What is there now is kept next to it. Returns where that went.
    pub fn restore_data(&self, key: &str, id: &str) -> Result<PathBuf, String> {
        let dst = self.app_data_dir(key).ok_or_else(|| "this item keeps no data folder".to_string())?;
        crate::bundlesnap::restore(&self.snapshot_root(key), id, &dst)
    }

    /// Copy the data of `key` before it is updated. Returns the words to add to the update's answer (empty when the thing keeps no data).
    /// A data folder over the size limit is not copied: the update goes on and says so, because refusing it would strand the person on an old version.
    fn snapshot_before_update(&self, key: &str, from: &str, user: &str, audit: &dyn Fn(&str, &str)) -> String {
        let Some(data) = self.app_data_dir(key) else { return String::new() };
        let root = self.snapshot_root(key);
        let clean: String = from.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').take(24).collect();
        let base = format!("pre-update-{clean}-{}", self.now());
        let mut id = base.clone();
        let mut n = 1;
        while root.join(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        match crate::bundlesnap::snapshot(&data, &root, &id, self.now(), Self::SNAPSHOT_MAX_BYTES) {
            Ok(s) => {
                let moved = crate::bundlesnap::prune(&root, Self::SNAPSHOTS_KEPT);
                self.life_log(audit, user, "snapshot", &format!("{key} {id}: {} files, {} bytes{}", s.files.len(), s.bytes, if moved.is_empty() { String::new() } else { format!("; {} older moved aside", moved.len()) }));
                format!(" Its data was copied first (snapshot {id}, {} files).", s.files.len())
            }
            Err(e) => {
                self.life_log(audit, user, "snapshot-skipped", &format!("{key}: {e}"));
                format!(" Its data was NOT copied first: {e}.")
            }
        }
    }

    // ---------------------------------------------------------------------------------------- rollback

    /// Go back to the kept previous version. Allowed on purpose (the version rule for going up does not apply) and recorded; the item is
    /// pinned at the old version so the update does not come straight back.
    pub fn rollback(&self, svc: &Services, actor: &Actor, life: &Life, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        let e = &life.entry;
        Self::scope_ok(actor, e).map_err(LifeError::Refused)?;
        if e.ty == Type::Cli {
            if !actor.admin {
                return Err(LifeError::Refused("Only administrators can change command-line tools.".into()));
            }
            let m = svc.clis.rollback(&e.name).map_err(LifeError::Failed)?;
            self.with_life(|s| {
                let v = svc.clis.is_installed(&e.name).map(|i| i.version).unwrap_or_default();
                s.pins.insert(e.key.clone(), Pin { version: v.clone(), at: 0, why: "rolled back".into() });
                s.history.entry(e.key.clone()).or_default().push(Hist { version: v, action: "rolled back".into(), at: 0 });
            });
            self.life_log(audit, &actor.user, "rollback", &m);
            return Ok(m);
        }
        let st = self.life_state();
        let prev = st.prev.get(&e.key).ok_or_else(|| LifeError::Refused(format!("{} has no earlier version kept.", e.title)))?;
        let bytes = std::fs::read(self.pkg_copy(&prev.sha256)).map_err(|_| LifeError::Refused("The kept copy of the earlier version is gone.".into()))?;
        let rec = self.records().get(&e.key).cloned().ok_or_else(|| LifeError::Refused("Only things installed through the store can be rolled back.".into()))?;
        let r = self.review_rollback(svc, actor, bytes, &rec.source, e.scope.clone());
        if let Some(why) = &r.refusal {
            return Err(LifeError::Refused(format!("Cannot go back: {why}")));
        }
        let new = self.transact(svc, actor, &e.key, &r.sha256, "rollback", &|| self.install(svc, actor, &r, Confirm::all()))?;
        self.life_log(audit, &actor.user, "rollback", &format!("{} {} -> {}", e.title, rec.version, new.version));
        Ok(format!("{} is back at {} (it was {}). It is pinned there; unpin it to receive updates again.", e.title, new.version, rec.version))
    }

    // ---------------------------------------------------------------------------------------- remove

    fn vault_names_for(&self, svc: &Services, user: &str, kind: &str, name: &str) -> (Vec<String>, String) {
        match svc.user_names(user) {
            Ok(names) => (vault_matches(&names, kind, name), String::new()),
            Err(_) => (vec![], "Your vault is locked, so entries named after it could not be listed.".into()),
        }
    }

    /// Everything that going away would take, and what it would leave.
    pub fn plan_remove(&self, svc: &Services, user: &str, lives: &[Life], life: &Life) -> RemovePlan {
        let e = &life.entry;
        let mut removes = Vec::new();
        match e.ty {
            Type::Skill => removes.push(format!("the skill file ({}) and its entry in the skills index", e.files.first().cloned().unwrap_or_default())),
            Type::Mcp => removes.push("its entry in the MCP server list (the assistant stops offering its tools)".into()),
            Type::Cli => removes.push(format!("the downloaded program, its command links ({}) and the kept older version", e.files.join(", "))),
            Type::Driver => removes.push("the driver pack's files and settings that IYAKKI wrote for it (see the Drivers page)".into()),
            Type::App => {}
            Type::Extension if e.kind == "tile" => removes.push("the app's tile in your launcher and the verified copy of the app".into()),
            Type::Extension if e.kind == "tool" => removes.push("the WASM tool package and its entry in the tool registry".into()),
            Type::Extension => removes.push(format!("the installed {} for {}", type_of_kind(&e.kind), e.target.clone().unwrap_or_else(|| "other tiles".into()))),
        }
        let appdata = self.dir().join("appdata").join(&e.kind).join(scope_key(&e.scope).replace('/', "_")).join(&e.name);
        let data: Vec<(String, u64)> = if appdata.is_dir() { vec![(appdata.display().to_string(), dir_size(&appdata))] } else { vec![] };
        let (vault, vault_note) = self.vault_names_for(svc, user, &e.kind, &e.name);
        let cached = {
            let st = self.life_state();
            let recs = self.records();
            recs.get(&e.key).map(|r| std::fs::metadata(self.pkg_copy(&r.sha256)).map_or(0, |m| m.len())).unwrap_or(0) + st.prev.get(&e.key).map(|p| std::fs::metadata(self.pkg_copy(&p.sha256)).map_or(0, |m| m.len())).unwrap_or(0)
        };
        RemovePlan { key: e.key.clone(), title: e.title.clone(), ty: e.ty, removes, data, vault, vault_note, dependents: self.dependents(svc, lives, life), cached }
    }

    /// Installed things that use this one: declared dependencies of store packages, MCP servers that run a command-line tool, and
    /// tools and servers a skill's package names.
    pub fn dependents(&self, svc: &Services, lives: &[Life], life: &Life) -> Vec<String> {
        let e = &life.entry;
        let mut out: Vec<String> = Vec::new();
        for other in lives.iter().filter(|o| o.entry.key != e.key) {
            // declared in the package's store information
            let declared = self.records().get(&other.entry.key).and_then(|r| std::fs::read_to_string(self.pkg_copy(&r.sha256)).ok()).and_then(|t| pkg::parse(&t).ok()).and_then(|p| p.manifest.meta.as_deref().and_then(|j| Meta::parse(j).ok())).is_some_and(|m| m.dependencies.iter().any(|d| d.name == e.name));
            if declared {
                out.push(other.entry.title.clone());
            }
        }
        if e.ty == Type::Cli {
            let bins: Vec<&str> = crate::clis::def(&e.name).map(|d| d.bins.iter().map(|b| b.0).collect()).unwrap_or_default();
            for s in svc.mcp.list() {
                let prog = s.command.rsplit('/').next().unwrap_or("");
                if bins.contains(&prog) && !out.contains(&s.name) {
                    out.push(format!("MCP server {}", s.name));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Remove for real. Data and vault entries are kept unless `opts.delete_data` and the person typed the item's name.
    pub fn remove_item(&self, svc: &Services, policy: &Policy, user: &str, lives: &[Life], life: &Life, opts: &RemoveOpts, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        let e = &life.entry;
        let plan = self.plan_remove(svc, user, lives, life);
        if !plan.dependents.is_empty() && !opts.ack_dependents {
            return Err(LifeError::NeedsConfirm(format!("{} is used by: {}. Confirm that you know they may stop working.", e.title, plan.dependents.join(", "))));
        }
        if opts.delete_data {
            if !plan.has_data() {
                // nothing to delete is not an error: the choice is simply moot
            } else if opts.typed.trim() != e.name {
                return Err(LifeError::NeedsConfirm(format!("Deleting its data cannot be undone. Type its name ({}) to confirm.", e.name)));
            }
        }
        let held_before: Vec<String> = self.approval(&e.key).map(|a| a.grants.iter().filter(|g| a.holds(&g.name, g.param.as_deref())).map(|g| g.name.clone()).collect()).unwrap_or_default();
        let msg = self.remove_installed(svc, policy, user, e).map_err(LifeError::Refused)?;
        crate::appevent_sources::permissions_told(&*svc.app_events, user, &e.name, &held_before, &[], "person", true);
        let mut said = vec![msg];
        if opts.delete_data {
            for (p, _) in &plan.data {
                let _ = std::fs::remove_dir_all(p);
                said.push(format!("Deleted its data folder {p}."));
            }
            for v in &plan.vault {
                if svc.user_delete(user, v).is_ok() {
                    said.push(format!("Deleted the vault entry {v}."));
                }
            }
        } else if plan.has_data() {
            said.push(format!("Kept its data: {}.", plan.data.iter().map(|d| d.0.clone()).chain(plan.vault.iter().map(|v| format!("vault entry {v}"))).collect::<Vec<_>>().join(", ")));
        }
        self.with_life(|s| {
            s.pins.remove(&e.key);
            s.cli_latest.remove(&e.name);
        });
        self.life_log(audit, user, "remove", &format!("{} delete_data={}", e.title, opts.delete_data));
        crate::appevent_sources::app_changed(&*svc.app_events, user, &e.key, "removed");
        Ok(said.join(" "))
    }

    // ---------------------------------------------------------------------------------------- update all

    /// What Update all would do, one line per item, before anything is downloaded: updates that can go ahead, and those that cannot
    /// (pinned, source gone, revoked, offline) with the reason. Permission changes show only after the download (`plan_update`).
    pub fn update_all_preview(lives: &[Life]) -> Vec<(String, String, bool)> {
        lives
            .iter()
            .filter(|l| l.has_update())
            .map(|l| {
                let ok = l.can_update();
                let mut s = format!("{} -> {}", l.entry.version, l.latest.clone().unwrap_or_default());
                if !l.added_permissions.is_empty() {
                    s.push_str(&format!("; asks for more: {} (needs your confirmation, so Update all skips it)", l.added_permissions.join(", ")));
                }
                if let Some(p) = &l.pinned {
                    s.push_str(&format!("; pinned at {}", p.version));
                }
                if let Some(o) = &l.from_other {
                    s.push_str(&format!("; from {o}, not the store that installed it"));
                }
                (l.entry.title.clone(), s, ok && l.added_permissions.is_empty() && l.from_other.is_none())
            })
            .collect()
    }

    /// Update everything that has an update, one at a time. Items that need a confirmation (more permissions, warnings, another
    /// store, a driver pack) are skipped and say so. `progress` is told before each item. Slow: call it off the screen's thread.
    #[allow(clippy::too_many_arguments)]
    pub fn update_all(&self, svc: &Services, policy: &Policy, user: &str, loaded: &[Loaded], fetch: &dyn Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> Report {
        let actor = Actor::from_policy(user, policy);
        let lives = self.lifecycle(svc, policy, user, loaded, &Ctx::default());
        let mut rep = Report::default();
        for l in lives.iter().filter(|l| l.has_update()) {
            let title = l.entry.title.clone();
            progress(&format!("Updating {title}…"));
            if l.entry.ty == Type::Driver {
                rep.items.push((title, Done::Skipped("driver packs change the whole machine: update them from the Drivers page".into())));
                continue;
            }
            if let Some(p) = &l.pinned {
                rep.items.push((title, Done::Skipped(format!("pinned at {}", p.version))));
                continue;
            }
            let plan = match self.plan_update(svc, &actor, l, loaded, fetch) {
                Ok(p) => p,
                Err(e) => {
                    rep.items.push((title, Done::Skipped(e.to_string())));
                    continue;
                }
            };
            if plan.needs_confirm() {
                let why = if !plan.added.is_empty() { format!("asks for new permissions ({}): update it by hand to confirm", plan.added.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; ")) } else { "needs your confirmation: update it by hand".to_string() };
                rep.items.push((title, Done::Skipped(why)));
                continue;
            }
            rep.items.push((title, match self.apply_update(svc, &actor, &plan, Confirm::default(), audit) {
                Ok(m) => Done::Updated(m),
                Err(LifeError::NeedsConfirm(m)) | Err(LifeError::Refused(m)) => Done::Skipped(m),
                Err(LifeError::Failed(m)) => Done::Failed(m),
            }));
        }
        rep
    }

    /// Every record's scope parsed from its key (for tools that list by key).
    pub fn scope_of_key(key: &str) -> Scope {
        key.split('/').nth(1).map_or(Scope::Shared, |s| if s == "user" { Scope::User(key.split('/').nth(2).unwrap_or("").to_string()) } else { scope_from_key(s) })
    }
}

/// Vault entries that belong to an item by name: `svc.<name>...`, `mcp.<name>...`, `<kind>.<name>...`, `<name>.<...>` or `<name>`.
pub fn vault_matches(names: &[String], kind: &str, name: &str) -> Vec<String> {
    let prefixes = [format!("svc.{name}"), format!("mcp.{name}"), format!("{kind}.{name}"), name.to_string()];
    names.iter().filter(|n| prefixes.iter().any(|p| *n == p || n.starts_with(&format!("{p}.")) || n.starts_with(&format!("{p}_")))).cloned().collect()
}

/// Wait for a command-line tool's install thread (it was started a moment ago).
fn wait_cli(svc: &Services, id: &str, max_secs: u64) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        if let Some(j) = svc.clis.job(id) {
            if let Some(e) = j.error {
                return Err(e);
            }
            if j.finished {
                return Ok(());
            }
        }
        if start.elapsed().as_secs() > max_secs {
            return Err("the update did not finish in time".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}
