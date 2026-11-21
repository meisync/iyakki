//! App bundles: one package, several kinds of content (docs/development/CORE-ACCESS-MODEL.md section 6d).
//!
//! The `contents` list of a tile package's `app.json` names members (skill, flow, tool, mcp, agent, template, theme, extension, tile), each with
//! a file inside the app, its own capabilities and its own `requires`. This module plans the members for the install card, installs, updates
//! and removes them all or none, and keeps one approval per member (`<bundle>/<member>` in the approval book, so a member never holds more than
//! its own declared capabilities and the app cannot lend its capabilities to it).
//!
//! What a member does is behind [`MemberBackend`]; [`LiveMembers`] is the real one. Wired today: `skill` (the skills store), `flow` (a DRAFT in
//! the person's own Flows folder through `FlowStore::create`; it never runs until the person publishes it) and the data kinds `agent`,
//! `template`, `theme`, `extension` (kept verified in the store's bundle folder; the tiles that read them are not hooked to bundle members yet).
//! `mcp` is wired for ADMINISTRATORS installing for everyone (the MCP list is machine wide and a server is a command that runs): the member file is
//! `{"name", "command", "args"}`, the command must be a plain program name or an absolute path, and the card names the member; it is removed again only
//! when it is still exactly what the bundle installed. `tool` and a nested `tile` are known kinds but NOT wired (they need their own signed package and administrator rules): they show
//! on the card as "not supported by this IYAKKI" like a kind nobody knows, and block the install unless the member is marked optional.

pub use iyakki_shared::store_bundle::{MemberCard};
use std::path::PathBuf;
use std::sync::Arc;

use iyakki_core_api::appcaps::{self, ApprovalBook, Grant};
use iyakki_core_api::bundle::{check_requires, Member, MemberKind, CORE_INTERFACE};

use super::install::Actor;
use super::tile::Bundle;
use super::Store;
use crate::services::Services;
use crate::skills::Scope;

pub use iyakki_shared::store_bundle::MemberState;

/// What the store needs from the places members live in. `install` creates (an update uninstalls the old member first), `uninstall` receives the
/// bytes that were installed.
pub trait MemberBackend {
    fn supports(&self, kind: &MemberKind) -> bool;
    /// Is this member's file good for its kind? (Checked while the card is made, before anything is written.)
    fn check(&self, m: &Member, bytes: &[u8]) -> Result<(), String>;
    fn install(&self, m: &Member, bytes: &[u8]) -> Result<(), String>;
    fn uninstall(&self, m: &Member, bytes: &[u8]) -> Result<(), String>;
    /// What to tell the person about this member on the card, besides its capabilities. Nothing by default.
    fn describe(&self, _m: &Member, _bytes: &[u8]) -> String {
        String::new()
    }
}



#[derive(Default)]
pub struct MemberPlan {
    pub cards: Vec<MemberCard>,
    /// The members that will be installed, with their file bytes.
    pub install: Vec<(Member, Vec<u8>)>,
    /// Why the bundle cannot be installed (a member that is needed but not supported, a file that is not good).
    pub refusal: Option<String>,
}

pub fn member_key(bundle_key: &str, member: &str) -> String {
    format!("{bundle_key}/{member}")
}

/// The broker's session of one bundle member (F-CORE-26): everything a member does to a device or a protected thing goes through here, so the
/// broker holds it to the approval the person gave for that member (and only that member: the bundle itself and its other members hold
/// nothing they did not ask for). The caller is a store app whose installation key is `<bundle>/<member>`, so the access log names it
/// `bundle/member`, and a revoke applies at the next call. A member that is not installed has no approval and gets `not_granted`.
pub struct MemberSession {
    broker: std::sync::Arc<iyakki_core_api::broker::Broker>,
    caller: iyakki_core_api::broker::Caller,
}

impl MemberSession {
    pub fn new(broker: std::sync::Arc<iyakki_core_api::broker::Broker>, bundle_key: &str, bundle_name: &str, member: &str, person: &str, device: &str) -> MemberSession {
        let caller = iyakki_core_api::broker::Caller { app: format!("{bundle_name}/{member}"), kind: iyakki_core_api::broker::CallerKind::Store { installation: member_key(bundle_key, member) }, person: person.into(), device: device.into() };
        MemberSession { broker, caller }
    }

    /// Ask to use a capability on a unit (a camera id and the like). The decision is written to the access log by the broker.
    pub fn decide(&self, capability: &str, unit: Option<&str>, label: &str) -> Result<(), iyakki_core_api::broker::Denial> {
        self.broker.decide(&iyakki_core_api::broker::Call { caller: &self.caller, capability, unit, label })
    }

    pub fn caller(&self) -> &iyakki_core_api::broker::Caller {
        &self.caller
    }
}

/// Decide what happens to each member. `held` answers with the grants the installed version's member approval still holds.
pub fn plan_members(bundle: &Bundle, backend: &dyn MemberBackend, held: &dyn Fn(&str) -> Vec<Grant>) -> MemberPlan {
    let mut plan = MemberPlan::default();
    for m in &bundle.manifest.ext.contents {
        let kind = m.kind();
        let mut skip: Option<(String, String)> = None; // (card text, reason in the refusal)
        if !backend.supports(&kind) {
            let why = if kind.is_known() { "not wired in this IYAKKI yet" } else { "a kind this IYAKKI does not know" };
            skip = Some(("not supported by this IYAKKI".into(), why.into()));
        } else if let Some(r) = &m.requires {
            if let Err(e) = check_requires(r, CORE_INTERFACE) {
                skip = Some((if e.update == iyakki_core_api::bundle::UpdateSide::Core { "needs a newer IYAKKI".into() } else { "needs a newer version of the app".into() }, e.message));
            }
        }
        let bytes = bundle.files.get(&m.file).map(|d| d.bytes().to_vec()).unwrap_or_default();
        if skip.is_none() {
            if let Err(e) = backend.check(m, &bytes) {
                plan.refusal.get_or_insert(format!("The bundle's member {:?} ({}) is not valid: {e}", m.id, m.kind));
            }
        }
        let note = if skip.is_none() { backend.describe(m, &bytes) } else { String::new() };
        let (state, lines) = match &skip {
            Some((text, reason)) => {
                if !m.optional {
                    plan.refusal.get_or_insert(format!("The bundle's member {:?} ({}) is {text} and is not marked optional, so the bundle cannot be installed here: {reason}.", m.id, m.kind));
                }
                (MemberState::Skip(text.clone()), vec![])
            }
            None => {
                plan.install.push((m.clone(), bytes));
                (MemberState::Install, appcaps::card_lines(&m.capabilities, &held(&m.id)))
            }
        };
        plan.cards.push(MemberCard { id: m.id.clone(), title: m.title.clone(), kind: m.kind.clone(), optional: m.optional, state, lines, note });
    }
    plan
}

// ------------------------------------------------------------------------------------------------ atomic install, update, remove

enum Undo {
    /// A member was installed (and maybe replaced an older one).
    Put { new: (Member, Vec<u8>), prev: Option<(Member, Vec<u8>)> },
    /// A member that the new version dropped was removed.
    Dropped { old: (Member, Vec<u8>) },
}

/// A finished member install that can still be taken back if a later step of the same install fails.
pub struct Tx {
    undo: Vec<Undo>,
    dir: PathBuf,
    canon_before: Vec<(String, Option<Vec<u8>>)>,
}

fn dir_of(store: &Store, key: &str) -> PathBuf {
    store.dir().join("bundles").join(key)
}

fn read_index(dir: &std::path::Path) -> Vec<Member> {
    std::fs::read(dir.join("index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn canon(dir: &std::path::Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.member"))
}

fn run_undo(backend: &dyn MemberBackend, undo: Vec<Undo>) -> Vec<String> {
    let mut problems = Vec::new();
    for u in undo.into_iter().rev() {
        let r = match u {
            Undo::Put { new, prev } => backend.uninstall(&new.0, &new.1).and_then(|()| prev.map_or(Ok(()), |p| backend.install(&p.0, &p.1))),
            Undo::Dropped { old } => backend.install(&old.0, &old.1),
        };
        if let Err(e) = r {
            problems.push(e);
        }
    }
    problems
}

impl Tx {
    /// Take back everything this transaction did (members and the kept copies). Used when a later step of the install fails.
    pub fn rollback(self, backend: &dyn MemberBackend) -> Vec<String> {
        let problems = run_undo(backend, self.undo);
        for (id, before) in self.canon_before {
            let p = canon(&self.dir, &id);
            match before {
                Some(b) => {
                    let _ = super::install::write_private(&p, &b);
                }
                None => {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        problems
    }
}

/// Who approved, for the approval records of the members.
pub struct Approver<'a> {
    pub who: &'a str,
    pub bundle: &'a str,
    pub version: &'a str,
    pub sha256: &'a str,
    pub at: i64,
}

/// Install or update all members, or none. Members the new version no longer lists are removed. On the first failure everything done so far is undone
/// and the error names the member. Approvals are written last, one per member that declares capabilities (`<key>/<member>`), and dropped for the rest.
pub fn apply(store: &Store, key: &str, backend: &dyn MemberBackend, desired: &[(Member, Vec<u8>)], by: &Approver) -> Result<Tx, String> {
    let dir = dir_of(store, key);
    let old = read_index(&dir);
    let old_with_bytes: Vec<(Member, Vec<u8>)> = old.iter().filter_map(|m| std::fs::read(canon(&dir, &m.id)).ok().map(|b| (m.clone(), b))).collect();
    let mut tx = Tx { undo: vec![], dir: dir.clone(), canon_before: vec![] };
    let fail = |tx: Tx, member: &str, e: String| -> String {
        let problems = tx.rollback(backend);
        let tail = if problems.is_empty() { "Nothing was changed.".to_string() } else { format!("Some earlier members could not be put back: {}.", problems.join("; ")) };
        format!("The bundle's member {member:?} failed: {e}. {tail}")
    };
    for (m, bytes) in desired {
        let prev = old_with_bytes.iter().find(|(o, _)| o.id == m.id).cloned();
        if let Some(p) = &prev {
            if let Err(e) = backend.uninstall(&p.0, &p.1) {
                return Err(fail(tx, &m.id, e));
            }
        }
        match backend.install(m, bytes) {
            Ok(()) => tx.undo.push(Undo::Put { new: (m.clone(), bytes.clone()), prev }),
            Err(e) => {
                // the older member was taken out to make room: put it back before undoing the rest
                if let Some(p) = &prev {
                    let _ = backend.install(&p.0, &p.1);
                }
                return Err(fail(tx, &m.id, e));
            }
        }
    }
    for (o, b) in &old_with_bytes {
        if desired.iter().any(|(m, _)| m.id == o.id) {
            continue;
        }
        if let Err(e) = backend.uninstall(o, b) {
            return Err(fail(tx, &o.id, e));
        }
        tx.undo.push(Undo::Dropped { old: (o.clone(), b.clone()) });
    }
    // the kept copies and the index, then the approvals; a failure here undoes the members too
    let mut write = || -> Result<(), String> {
        for id in desired.iter().map(|(m, _)| m.id.clone()).chain(old.iter().map(|m| m.id.clone())) {
            if !tx.canon_before.iter().any(|(i, _)| *i == id) {
                tx.canon_before.push((id.clone(), std::fs::read(canon(&dir, &id)).ok()));
            }
        }
        for (m, b) in desired {
            super::install::write_private(&canon(&dir, &m.id), b)?;
        }
        for o in &old {
            if !desired.iter().any(|(m, _)| m.id == o.id) {
                let _ = std::fs::remove_file(canon(&dir, &o.id));
            }
        }
        let index: Vec<&Member> = desired.iter().map(|(m, _)| m).collect();
        super::install::write_private(&dir.join("index.json"), &serde_json::to_vec_pretty(&index).unwrap_or_default())
    };
    if let Err(e) = write() {
        return Err(fail(tx, "(kept copies)", e));
    }
    let mut book = super::approvals::FileBook(store);
    for o in &old {
        if !desired.iter().any(|(m, _)| m.id == o.id) {
            book.remove(&member_key(key, &o.id));
        }
    }
    for (m, _) in desired {
        let k = member_key(key, &m.id);
        if m.capabilities.is_empty() {
            book.remove(&k);
            continue;
        }
        let held = book.get(&k);
        let mut a = appcaps::record_approval(by.who, &member_key(by.bundle, &m.id), by.version, by.sha256, by.at, &m.capabilities);
        if let Some(h) = held.filter(|h| matches!(appcaps::plan_ask(Some(h), &m.capabilities), appcaps::AskPlan::Nothing)) {
            a.revoked = h.revoked;
        }
        book.put(&k, a);
    }
    Ok(tx)
}

/// Remove every member of the bundle, or none (if one cannot be removed the ones already removed are put back). Their approvals go with the
/// installation record (`FileBook::remove_prefix`), not here, so turning an app off and on keeps them.
pub fn remove_all(store: &Store, key: &str, backend: &dyn MemberBackend) -> Result<(), String> {
    let dir = dir_of(store, key);
    let old = read_index(&dir);
    if old.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(());
    }
    let mut undo = Vec::new();
    for m in old.iter().rev() {
        let bytes = std::fs::read(canon(&dir, &m.id)).map_err(|e| format!("the kept copy of {:?} is gone: {e}", m.id));
        let r = bytes.and_then(|b| backend.uninstall(m, &b).map(|()| b));
        match r {
            Ok(b) => undo.push(Undo::Dropped { old: (m.clone(), b) }),
            Err(e) => {
                let problems = run_undo(backend, undo);
                let tail = if problems.is_empty() { "Nothing was removed.".to_string() } else { format!("Some members could not be put back: {}.", problems.join("; ")) };
                return Err(format!("The bundle's member {:?} could not be removed: {e}. {tail}", m.id));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The ids of the members currently installed for this bundle.
pub fn installed_members(store: &Store, key: &str) -> Vec<Member> {
    read_index(&dir_of(store, key))
}

/// One kept data member of an installed bundle, as a tile that reads it sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptMember {
    /// The bundle's key in the store (`tile/<scope>/<name>`).
    pub bundle: String,
    pub id: String,
    pub title: String,
    /// The member's file, a JSON object.
    pub value: serde_json::Value,
}

/// The kept members of `kind` (`agent`, `template`, `theme` or `extension`) of every installed bundle, for the tile that reads that kind
/// (an agent picker, the template list, the theme chooser, the extension list). A member
/// whose kept copy is missing or is not a JSON object, and a kind that is not a data kind are left out: a reader never gets what the
/// store did not verify. Oldest bundle name first, members in the bundle's own order.
pub fn kept_members(store: &Store, kind: &str) -> Vec<KeptMember> {
    if !matches!(kind, "agent" | "template" | "theme" | "extension") {
        return Vec::new();
    }
    let root = store.dir().join("bundles");
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![(String::new(), root)];
    while let Some((key, d)) = stack.pop() {
        if d.join("index.json").is_file() {
            dirs.push((key.clone(), d.clone()));
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                let name = e.file_name().to_string_lossy().into_owned();
                stack.push((if key.is_empty() { name } else { format!("{key}/{name}") }, e.path()));
            }
        }
    }
    dirs.sort();
    let mut out = Vec::new();
    for (key, d) in dirs {
        for m in read_index(&d).into_iter().filter(|m| m.kind == kind) {
            let Ok(bytes) = std::fs::read(canon(&d, &m.id)) else { continue };
            let Ok(v @ serde_json::Value::Object(_)) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            out.push(KeptMember { bundle: key.clone(), id: m.id.clone(), title: m.title.clone(), value: v });
        }
    }
    out
}

/// A kept member that passed its kind's schema, with the bundle it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept<T> {
    pub bundle: String,
    pub id: String,
    pub value: T,
}

fn typed<T>(store: &Store, kind: &str, read: impl Fn(&serde_json::Value) -> Result<T, String>) -> Vec<Kept<T>> {
    kept_members(store, kind).into_iter().filter_map(|m| read(&m.value).ok().map(|value| Kept { bundle: m.bundle, id: m.id, value })).collect()
}

/// The desktop-view templates of installed bundles, for the template list (Settings, Desktop views). A member that fails the schema is left out.
pub fn view_templates(store: &Store) -> Vec<Kept<iyakki_shared::views::UserTemplate>> {
    typed(store, "template", iyakki_shared::bundle_members::view_template)
}

/// The agent presets of installed bundles, for the agent picker.
pub fn agent_presets(store: &Store) -> Vec<Kept<iyakki_shared::bundle_members::AgentPreset>> {
    typed(store, "agent", iyakki_shared::bundle_members::agent_preset)
}

/// The extension members of installed bundles, for the extension list (the Store's Installed page).
pub fn extension_lines(store: &Store) -> Vec<Kept<iyakki_shared::bundle_members::ExtensionLine>> {
    typed(store, "extension", iyakki_shared::bundle_members::extension_line)
}

/// What `store_bundle_members` says: every installed bundle with its members (all kinds), and the kept, verified content of the data kinds.
/// Read-only. `kind` narrows the list to one kind.
pub fn members_text(store: &Store, kind: Option<&str>) -> String {
    let root = store.dir().join("bundles");
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![(String::new(), root)];
    while let Some((key, d)) = stack.pop() {
        if d.join("index.json").is_file() {
            dirs.push((key.clone(), d.clone()));
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                let name = e.file_name().to_string_lossy().into_owned();
                stack.push((if key.is_empty() { name } else { format!("{key}/{name}") }, e.path()));
            }
        }
    }
    dirs.sort();
    let mut out = String::new();
    for (key, d) in dirs {
        let members: Vec<Member> = read_index(&d).into_iter().filter(|m| kind.is_none_or(|k| m.kind == k)).collect();
        if members.is_empty() {
            continue;
        }
        out.push_str(&format!("{key}:\n"));
        for m in members {
            let kept = if matches!(m.kind.as_str(), "agent" | "template" | "theme" | "extension") {
                match std::fs::read(canon(&d, &m.id)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
                    Some(v @ serde_json::Value::Object(_)) => {
                        let schema = match m.kind.as_str() {
                            "template" => iyakki_shared::bundle_members::view_template(&v).err(),
                            "agent" => iyakki_shared::bundle_members::agent_preset(&v).err(),
                            "extension" => iyakki_shared::bundle_members::extension_line(&v).err(),
                            _ => None,
                        };
                        let note = schema.map(|e| format!(" · not offered, {e}")).unwrap_or_default();
                        format!(" · kept: {}{note}", v.to_string().chars().take(300).collect::<String>())
                    }
                    _ => " · kept copy missing or not a JSON object".to_string(),
                }
            } else {
                String::new()
            };
            out.push_str(&format!("  {} ({}){}{}\n", m.id, m.kind, if m.title.is_empty() { String::new() } else { format!(" {}", m.title) }, kept));
        }
    }
    if out.is_empty() { "No bundle members are installed.".into() } else { out }
}

// ------------------------------------------------------------------------------------------------ the real backend

pub struct LiveMembers<'a> {
    pub store: &'a Store,
    pub svc: &'a Services,
    pub actor: &'a Actor,
    pub scope: &'a Scope,
    pub now: i64,
    pub data_dir: PathBuf,
    pub home: Option<PathBuf>,
}

impl<'a> LiveMembers<'a> {
    pub fn new(store: &'a Store, svc: &'a Services, actor: &'a Actor, scope: &'a Scope) -> Self {
        LiveMembers { store, svc, actor, scope, now: store.now(), data_dir: store.data_dir().to_path_buf(), home: store.flows_home.clone() }
    }

    fn flows(&self) -> Result<iyakki_flow::store::FlowStore, String> {
        let Scope::User(u) = self.scope else { return Err("flows belong to a person: install this bundle for yourself".into()) };
        let t = self.now.saturating_mul(1000);
        let home = self.home.clone().unwrap_or_else(|| crate::scope::home_root(u, &self.data_dir));
        Ok(iyakki_flow::store::FlowStore::new(&home.join("Flows"), Arc::new(move || t)))
    }

    /// Whose theme folder a theme member goes to: the person's own, or everybody's for a shared install.
    fn theme_user(&self) -> Option<&str> {
        match self.scope {
            Scope::User(u) => Some(u.as_str()),
            _ => None,
        }
    }

    /// Start a nested `tile` member (KI-891): the signed package of its own goes through the same review and install as any app (its signer
    /// must be trusted here, the level and the revocations are checked), under the actor and scope of the bundle, and it becomes an app of its
    /// own with its own approvals. Three more rules: it may not be a bundle itself (one level only), it may ask the core for nothing the member
    /// did not declare in the bundle (the person approved exactly those on the card), and a name that is already installed is refused.
    fn start_nested_tile(&self, m: &Member, text: &str) -> Result<(), String> {
        let p = nested_tile(text)?;
        let key = super::install::record_key("tile", self.scope, &p.manifest.name);
        if self.store.records().contains_key(&key) {
            return Err(format!("an app named {:?} is already installed; remove it first", p.manifest.name));
        }
        let review = self.store.review(self.svc, self.actor, text.as_bytes().to_vec(), None, "bundle member", false, self.scope.clone(), false, &[]);
        if let Some(why) = &review.refusal {
            return Err(format!("the nested app {:?} cannot be installed: {why}", p.manifest.name));
        }
        use iyakki_shared::store_trust::Level;
        if !matches!(review.verdict.level, Level::Official | Level::Verified | Level::Trusted) {
            return Err(format!("the nested app {:?} cannot be installed: its signer is not trusted here (a nested app needs an official, verified or trusted signer, and this one is {})", p.manifest.name, review.verdict.level.id()));
        }
        if !review.members.is_empty() {
            return Err(format!("the nested app {:?} carries members of its own; a nested app is one level only", p.manifest.name));
        }
        if !review.permissions.is_empty() {
            return Err(format!("the nested app {:?} asks for permissions of its own; a nested app may only use the capabilities declared for the member", p.manifest.name));
        }
        let declared: Vec<&str> = m.capabilities.iter().map(|c| c.name.as_str()).collect();
        if let Some(extra) = review.core_requests.iter().find(|r| !declared.contains(&r.name.as_str())) {
            return Err(format!("the nested app {:?} asks for {:?}, which the bundle did not declare for this member", p.manifest.name, extra.name));
        }
        self.store.install(self.svc, self.actor, &review, iyakki_shared::store_install::Confirm::all()).map(|_| ()).map_err(|e| format!("the nested app {:?} was not installed: {e}", p.manifest.name))
    }

    fn text<'b>(&self, bytes: &'b [u8]) -> Result<&'b str, String> {
        std::str::from_utf8(bytes).map_err(|_| "the file is not text".to_string())
    }
}

/// The server an `mcp` member describes. A command is a plain program name or an absolute path with no shell character, so nothing is read as a shell line.
pub fn mcp_member(text: &str) -> Result<crate::mcp::StoredServer, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("the mcp member is not JSON: {e}"))?;
    let name = v["name"].as_str().ok_or("the mcp member needs a name")?.to_string();
    if !crate::mcp::valid_server_name(&name) {
        return Err("the server name is letters, digits, - and _ (up to 32)".into());
    }
    let command = v["command"].as_str().ok_or("the mcp member needs a command")?.to_string();
    let plain = |s: &str| !s.is_empty() && s.len() <= 500 && !s.chars().any(|c| c.is_control() || c.is_whitespace() || "|&;$`<>\"'\\*?(){}".contains(c));
    let bare = !command.contains('/');
    if !plain(&command) || !(bare || command.starts_with('/')) || command.split('/').any(|p| p == "..") {
        return Err("the command is a program name or an absolute path, without spaces or shell characters".into());
    }
    let args: Vec<String> = match v.get("args") {
        None | Some(serde_json::Value::Null) => vec![],
        Some(serde_json::Value::Array(a)) if a.len() <= 32 => a.iter().map(|x| x.as_str().filter(|s| s.len() <= 500 && !s.contains('\0')).map(str::to_string).ok_or("every argument is text")).collect::<Result<_, _>>()?,
        _ => return Err("args is a list of at most 32 texts".into()),
    };
    Ok(crate::mcp::StoredServer { name, command, args, sample: false })
}

/// A `tool` member: a description of a tool the bundle keeps for an administrator to switch on. It never carries a command (an `mcp` member does that,
/// and shows it on the card): `{"name", "description"}` and nothing that is run.
pub fn tool_member(text: &str) -> Result<(String, String), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("the tool member is not JSON: {e}"))?;
    let o = v.as_object().ok_or("the tool member is a JSON object")?;
    let name = o.get("name").and_then(|x| x.as_str()).ok_or("the tool member needs a name")?;
    if !crate::mcp::valid_server_name(name) {
        return Err("the tool name is letters, digits, - and _ (up to 32)".into());
    }
    for k in ["command", "exec", "shell", "script", "args", "url"] {
        if o.contains_key(k) {
            return Err(format!("a tool member carries no `{k}`: a program to run is an mcp member, shown on the card"));
        }
    }
    let desc = o.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if desc.chars().count() > 300 || desc.chars().any(|c| c.is_control()) {
        return Err("the description is at most 300 plain characters".into());
    }
    Ok((name.to_string(), desc))
}

/// A nested `tile` member is another tile app as a SIGNED package of its own: parsed with the package reader, of kind tile, with a signer.
pub fn nested_tile(text: &str) -> Result<crate::pkg::Package, String> {
    let p = crate::pkg::parse(text).map_err(|e| format!("the nested app is not a package: {e}"))?;
    if p.manifest.kind != "tile" {
        return Err(format!("the nested app is a {}, not a tile", p.manifest.kind));
    }
    if p.manifest.signer.as_deref().is_none_or(str::is_empty) {
        return Err("a nested app must be a signed package of its own".into());
    }
    Ok(p)
}

impl MemberBackend for LiveMembers<'_> {
    fn describe(&self, m: &Member, bytes: &[u8]) -> String {
        let Ok(text) = std::str::from_utf8(bytes) else { return String::new() };
        match m.kind() {
            MemberKind::Mcp => mcp_member(text).map(|s| format!("runs this program on the computer when the assistant uses it: {}{}", s.command, if s.args.is_empty() { String::new() } else { format!(" {}", s.args.join(" ")) })).unwrap_or_default(),
            MemberKind::Tool => tool_member(text).map(|(n, d)| format!("kept as a description of the tool {n}{}; nothing runs and an administrator switches it on later", if d.is_empty() { String::new() } else { format!(" ({d})") })).unwrap_or_default(),
            MemberKind::Tile => nested_tile(text).map(|p| format!("installs the signed app {} {} as an app of its own (its signer must be trusted here, it asks for nothing this member did not declare, and it goes away with the bundle)", p.manifest.name, p.manifest.version)).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn supports(&self, kind: &MemberKind) -> bool {
        match kind {
            MemberKind::Skill | MemberKind::Agent | MemberKind::Template | MemberKind::Theme | MemberKind::Extension => true,
            MemberKind::Flow => matches!(self.scope, Scope::User(_)),
            // a server is a command the machine runs for everybody: only an administrator installing for everyone may add one
            MemberKind::Mcp | MemberKind::Tool | MemberKind::Tile => matches!(self.scope, Scope::Shared) && self.actor.admin && self.actor.can_shared,
            _ => false,
        }
    }

    fn check(&self, m: &Member, bytes: &[u8]) -> Result<(), String> {
        let text = self.text(bytes)?;
        match m.kind() {
            MemberKind::Skill => crate::skills::parse(text, self.scope.clone()).map(|_| ()).map_err(|e| e.to_string()),
            MemberKind::Flow => iyakki_flow::model::Flow::from_json(text).map(|_| ()),
            MemberKind::Mcp => mcp_member(text).map(|_| ()),
            MemberKind::Tool => tool_member(text).map(|_| ()),
            MemberKind::Tile => nested_tile(text).map(|_| ()),
            MemberKind::Theme => match serde_json::from_str::<serde_json::Value>(text) {
                Ok(v) => iyakki_shared::bundle_members::theme_text(&v).map(|_| ()),
                Err(_) => Err("the file is a JSON object".into()),
            },
            MemberKind::Template => serde_json::from_str::<serde_json::Value>(text).map_err(|_| "the file is a JSON object".to_string()).and_then(|v| iyakki_shared::bundle_members::view_template(&v).map(|_| ())),
            MemberKind::Agent => serde_json::from_str::<serde_json::Value>(text).map_err(|_| "the file is a JSON object".to_string()).and_then(|v| iyakki_shared::bundle_members::agent_preset(&v).map(|_| ())),
            _ => match serde_json::from_str::<serde_json::Value>(text) {
                Ok(v) if v.is_object() => Ok(()),
                _ => Err("the file is a JSON object".into()),
            },
        }
    }

    fn install(&self, m: &Member, bytes: &[u8]) -> Result<(), String> {
        let text = self.text(bytes)?;
        match m.kind() {
            MemberKind::Skill => {
                let sk = crate::skills::parse(text, self.scope.clone()).map_err(|e| e.to_string())?;
                if self.svc.skills.get(self.scope, &sk.name).is_some() {
                    return Err(format!("a skill named {:?} is already installed; remove it first", sk.name));
                }
                self.svc.skills.add(self.scope.clone(), text, self.actor.can_shared, self.actor.can_own).map(|_| ()).map_err(|e| e.to_string())
            }
            MemberKind::Flow => {
                // a flow from outside arrives as a draft: never production, triggers off, no self-fixing agent (the same sanitiser as every import)
                let mut flow = iyakki_flow::model::Flow::from_json(text)?;
                iyakki_flow::templates::sanitize_import(&mut flow);
                self.flows()?.create(flow, &self.actor.user).map(|_| ())
            }
            MemberKind::Mcp => self.svc.mcp.add(mcp_member(text)?),
            MemberKind::Theme => {
                let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                iyakki_shared::bundle_members::put_theme(&self.data_dir, self.theme_user(), &m.id, &v)
            }
            MemberKind::Template | MemberKind::Agent => {
                let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                iyakki_shared::bundle_members::put_data_member(&self.data_dir, &m.kind, self.theme_user(), &m.id, &v)
            }
            MemberKind::Tile => self.start_nested_tile(m, text),
            _ => Ok(()),
        }
    }

    fn uninstall(&self, m: &Member, bytes: &[u8]) -> Result<(), String> {
        let text = self.text(bytes)?;
        match m.kind() {
            MemberKind::Skill => {
                let sk = crate::skills::parse(text, self.scope.clone()).map_err(|e| e.to_string())?;
                match self.svc.skills.remove(self.scope, &sk.name, self.actor.can_shared, self.actor.can_own) {
                    Ok(()) | Err(crate::skills::SkillError::NotFound) => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            }
            MemberKind::Flow => {
                let flow = iyakki_flow::model::Flow::from_json(text)?;
                let store = self.flows()?;
                if store.exists(&flow.id) { store.delete(&flow.id) } else { Ok(()) }
            }
            MemberKind::Mcp => {
                let s = mcp_member(text)?;
                // only what the bundle put there: a server the administrator changed by hand since is left alone
                match self.svc.mcp.list().into_iter().find(|x| x.name == s.name) {
                    Some(now) if now == s => self.svc.mcp.remove(&s.name),
                    _ => Ok(()),
                }
            }
            MemberKind::Theme => {
                let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                iyakki_shared::bundle_members::drop_theme(&self.data_dir, self.theme_user(), &m.id, &v)
            }
            MemberKind::Template | MemberKind::Agent => {
                let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
                iyakki_shared::bundle_members::drop_data_member(&self.data_dir, &m.kind, self.theme_user(), &m.id, &v)
            }
            MemberKind::Tile => {
                // only the app this bundle put there: one the administrator installed or replaced since is left alone
                let p = nested_tile(text)?;
                let key = super::install::record_key("tile", self.scope, &p.manifest.name);
                match self.store.records().get(&key) {
                    Some(r) if r.sha256 == p.manifest.sha256 => self.store.remove(self.svc, self.actor, "tile", &p.manifest.name, self.scope),
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }
}
