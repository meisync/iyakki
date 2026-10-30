//! Sharing a dashboard with other accounts. The default cluster dashboard belongs to the administrator who owns it and is private to them;
//! they may share it with chosen accounts, each as a VIEWER (look only) or an OPERATOR (look, and change the panels, layout, time range and
//! refresh, saved into the owner's dashboard), and change or stop it at any time. Neither role can share, unshare, rename, delete or export
//! the dashboard or see the owner's source settings; only the owner can.
//!
//! * One file, `<data>/dash/shares.json`: for each (owner, dashboard) the account names it is shared with. It is looked at when a
//!   dashboard is listed, opened or queried, never cached: unsharing, a role change, a deleted dashboard or a deleted owner takes effect at the next
//!   read. A name that is not an account any more is left out when shown.
//! * A recipient sees the dashboard as `share:<owner>/<id>` and its sources as `share:<owner>/<id>/<source>`. Such an id is not a valid
//!   dashboard id, so [`super::store::Dashboards::save`] refuses it. A viewer's changes are never saved; an operator's go through
//!   [`Env::save_dashboard`], which checks them against the owner's current dashboard. Export refuses a shared id in the screen too.
//! * Only dashboards whose sources are all the local cluster (`kube`) can be shared. That source holds no address and no secret, and a
//!   recipient reaches it through a wrapper that answers only the views the shared dashboard itself shows ([`Scoped`]): the same read-only
//!   cluster views, never Secrets, logs or exec, and no query of their own.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::connector::*;
use super::frame::Frame;
use super::model::Dashboard;
use super::store::{Dashboards, Entry, Sources};
use super::tools::Env;
use iyakki_sec::policy::{valid_username, App};

pub const SHARE_PREFIX: &str = "share:";
/// Viewers per dashboard and shares in all.
pub const MAX_VIEWERS: usize = 20;
pub const MAX_SHARES: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Operator,
}

impl Role {
    pub fn word(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" | "view" => Some(Role::Viewer),
            "operator" | "operate" => Some(Role::Operator),
            _ => None,
        }
    }
}

/// An account and what it may do with a shared dashboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub name: String,
    pub role: Role,
}

impl Grant {
    pub fn viewer(name: &str) -> Grant {
        Grant { name: name.into(), role: Role::Viewer }
    }
    pub fn operator(name: &str) -> Grant {
        Grant { name: name.into(), role: Role::Operator }
    }
}

/// `bob (operator), carol (viewer)`
pub fn grants_text(g: &[Grant]) -> String {
    g.iter().map(|x| format!("{} ({})", x.name, x.role.word())).collect::<Vec<_>>().join(", ")
}

/// `bob:operator, carol` as the Share dialog shows it (viewer is the default and is not written).
pub fn grants_input(g: &[Grant]) -> String {
    g.iter().map(|x| if x.role == Role::Operator { format!("{}:operator", x.name) } else { x.name.clone() }).collect::<Vec<_>>().join(", ")
}

/// What a person typed in the Share dialog: `bob:operator, carol, dave:viewer` (commas or semicolons between accounts, viewer by default).
pub fn parse_grants(text: &str) -> Result<Vec<Grant>, String> {
    let mut out: Vec<Grant> = vec![];
    for item in text.split([',', ';']).map(str::trim).filter(|i| !i.is_empty()) {
        let toks: Vec<&str> = item.split(|c: char| c == ':' || c.is_whitespace()).filter(|x| !x.is_empty()).collect();
        let g = match toks.as_slice() {
            [n] => Grant::viewer(n),
            [n, r] => Grant { name: (*n).into(), role: Role::parse(r).ok_or_else(|| format!("`{}` is not a role: write viewer or operator", super::frame::clip(r, 20)))? },
            _ => return Err(format!("`{}`: write an account name, or name:operator", super::frame::clip(item, 40))),
        };
        if out.iter().any(|o| o.name == g.name) {
            return Err(format!("{} is listed twice", g.name));
        }
        out.push(g);
    }
    Ok(out)
}

/// `with` is everyone it is shared with; `operators` (a subset) are the ones who may also change it. A file from before roles has only
/// `with`: everybody in it is a viewer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub owner: String,
    pub dashboard: String,
    #[serde(default, deserialize_with = "names")]
    pub with: Vec<String>,
    #[serde(default, deserialize_with = "names")]
    pub operators: Vec<String>,
}

/// A list of names; anything else in its place (a damaged file) reads as no names instead of failing the whole file.
fn names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default())
}

impl Share {
    fn grants(&self) -> Vec<Grant> {
        self.with.iter().map(|n| Grant { name: n.clone(), role: if self.operators.contains(n) { Role::Operator } else { Role::Viewer } }).collect()
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SharesFile {
    schema: u32,
    shares: Vec<Share>,
}

/// `share:amy/k3s-local`
pub fn share_id(owner: &str, dashboard: &str) -> String {
    format!("{SHARE_PREFIX}{owner}/{dashboard}")
}

/// `share:amy/k3s-local/kube-local`
pub fn share_source_id(owner: &str, dashboard: &str, source: &str) -> String {
    format!("{SHARE_PREFIX}{owner}/{dashboard}/{source}")
}

fn parts(id: &str) -> Option<Vec<&str>> {
    let p: Vec<&str> = id.strip_prefix(SHARE_PREFIX)?.split('/').collect();
    (p.len() >= 2 && valid_username(p[0]) && p[1..].iter().all(|x| valid_id(x))).then_some(p)
}

/// (owner, dashboard) of `share:<owner>/<id>`.
pub fn parse_share_id(id: &str) -> Option<(String, String)> {
    parts(id).filter(|p| p.len() == 2).map(|p| (p[0].to_string(), p[1].to_string()))
}

/// (owner, dashboard, source) of `share:<owner>/<id>/<source>`.
pub fn parse_share_source_id(id: &str) -> Option<(String, String, String)> {
    parts(id).filter(|p| p.len() == 3).map(|p| (p[0].to_string(), p[1].to_string(), p[2].to_string()))
}

pub struct Shares {
    file: PathBuf,
}

impl Shares {
    pub fn open(data_dir: &Path) -> Shares {
        Shares { file: data_dir.join("dash").join("shares.json") }
    }

    /// The file, cleaned: odd entries, bad names and duplicates are dropped, so a hand-edited or damaged file cannot widen anything.
    fn read(&self) -> Vec<Share> {
        let f: SharesFile = std::fs::read_to_string(&self.file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut out: Vec<Share> = vec![];
        for s in f.shares.into_iter().take(MAX_SHARES * 2) {
            if !valid_username(&s.owner) || !valid_id(&s.dashboard) || out.iter().any(|o| o.owner == s.owner && o.dashboard == s.dashboard) {
                continue;
            }
            let mut with: Vec<String> = vec![];
            for w in s.with {
                if valid_username(&w) && w != s.owner && !with.contains(&w) && with.len() < MAX_VIEWERS {
                    with.push(w);
                }
            }
            let operators: Vec<String> = s.operators.into_iter().filter(|o| with.contains(o)).collect();
            if !with.is_empty() {
                out.push(Share { owner: s.owner, dashboard: s.dashboard, with, operators });
            }
        }
        out.truncate(MAX_SHARES);
        out
    }

    fn write(&self, shares: &[Share]) -> Result<(), String> {
        if let Some(d) = self.file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let body = serde_json::to_string_pretty(&SharesFile { schema: 1, shares: shares.to_vec() }).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&self.file, body.as_bytes(), 0o600)
    }

    pub fn with_of(&self, owner: &str, dashboard: &str) -> Vec<Grant> {
        self.read().into_iter().find(|s| s.owner == owner && s.dashboard == dashboard).map(|s| s.grants()).unwrap_or_default()
    }

    /// What `who` may do with the dashboard right now (`None`: it is not shared with them).
    pub fn role_of(&self, owner: &str, dashboard: &str, who: &str) -> Option<Role> {
        self.with_of(owner, dashboard).into_iter().find(|g| g.name == who).map(|g| g.role)
    }

    pub fn is_shared_with(&self, owner: &str, dashboard: &str, who: &str) -> bool {
        self.role_of(owner, dashboard, who).is_some()
    }

    /// (owner, dashboard) of everything shared with `who`.
    pub fn for_viewer(&self, who: &str) -> Vec<(String, String)> {
        self.read().into_iter().filter(|s| s.with.iter().any(|w| w == who)).map(|s| (s.owner, s.dashboard)).collect()
    }

    /// Replace the list for one dashboard (empty stops sharing). The names are checked here for form only; whether the accounts exist
    /// is the caller's question.
    pub fn set(&self, owner: &str, dashboard: &str, grants: &[Grant]) -> Result<ShareChange, String> {
        if !valid_username(owner) || !valid_id(dashboard) {
            return Err("that is not a dashboard of an account".into());
        }
        let mut now: Vec<Grant> = vec![];
        for g in grants.iter().map(|g| Grant { name: g.name.trim().to_string(), role: g.role }).filter(|g| !g.name.is_empty()) {
            if !valid_username(&g.name) {
                return Err(format!("`{}` is not an account name (lowercase letters, digits, - and _)", super::frame::clip(&g.name, 40)));
            }
            if g.name == owner {
                return Err("it is already yours: name the other accounts".into());
            }
            match now.iter().find(|n| n.name == g.name) {
                Some(n) if n.role != g.role => return Err(format!("{} is listed twice with different roles", g.name)),
                Some(_) => {}
                None => now.push(g),
            }
        }
        if now.len() > MAX_VIEWERS {
            return Err(format!("at most {MAX_VIEWERS} accounts per dashboard"));
        }
        let mut all = self.read();
        let before: Vec<Grant> = all.iter().find(|s| s.owner == owner && s.dashboard == dashboard).map(|s| s.grants()).unwrap_or_default();
        all.retain(|s| !(s.owner == owner && s.dashboard == dashboard));
        if !now.is_empty() {
            if all.len() >= MAX_SHARES {
                return Err(format!("at most {MAX_SHARES} shared dashboards"));
            }
            all.push(Share { owner: owner.into(), dashboard: dashboard.into(), with: now.iter().map(|g| g.name.clone()).collect(), operators: now.iter().filter(|g| g.role == Role::Operator).map(|g| g.name.clone()).collect() });
        }
        self.write(&all)?;
        Ok(ShareChange {
            added: now.iter().filter(|n| !before.iter().any(|b| b.name == n.name)).cloned().collect(),
            changed: now.iter().filter(|n| before.iter().any(|b| b.name == n.name && b.role != n.role)).cloned().collect(),
            removed: before.iter().filter(|b| !now.iter().any(|n| n.name == b.name)).map(|b| b.name.clone()).collect(),
        })
    }
}

/// Every source id a dashboard asks, the dashboard's default for panels that name none.
pub fn used_sources(d: &Dashboard) -> BTreeSet<String> {
    let mut v: BTreeSet<String> = d.panels.iter().flat_map(|p| p.sources(&d.default_source)).collect();
    for var in &d.variables {
        if let super::model::VarKind::Query { source, .. } = &var.kind {
            if !source.is_empty() {
                v.insert(source.clone());
            }
        }
    }
    v
}

const SEES: &str = "see the live state of this machine's k3s cluster: node, pod, namespace and deployment names, restarts and reasons, event messages, services and their open ports, CPU and memory use, volume claims";
const OPERATOR_CAN: &str = "Operators can also change this dashboard (add, remove, move and edit its panels, the time range and the refresh); the changes are saved in your dashboard and your previous version is kept as a backup. Only with the views it already shows. ";

/// The text a person is asked to confirm before sharing, in plain words, for each role.
pub fn share_warning(title: &str, now: &[Grant], before: &[Grant]) -> String {
    let added: Vec<&Grant> = now.iter().filter(|n| !before.iter().any(|b| b.name == n.name)).collect();
    let changed: Vec<(&Grant, Role)> = now.iter().filter_map(|n| before.iter().find(|b| b.name == n.name && b.role != n.role).map(|b| (n, b.role))).collect();
    let removed: Vec<&Grant> = before.iter().filter(|b| !now.iter().any(|n| n.name == b.name)).collect();
    let list = |v: &[&Grant]| v.iter().map(|g| format!("{} ({})", g.name, g.role.word())).collect::<Vec<_>>().join(", ");
    let mut t = String::new();
    if !added.is_empty() {
        t.push_str(&format!("Share “{title}” with {}? Through this dashboard they will be able to {SEES}. ", list(&added)));
    }
    for (g, was) in &changed {
        t.push_str(&match g.role {
            Role::Operator => format!("Make {} an operator instead of a {}? ", g.name, was.word()),
            Role::Viewer => format!("Make {} a viewer instead of an {}? They keep looking, and can no longer change the dashboard. ", g.name, was.word()),
        });
    }
    let operators = now.iter().any(|n| n.role == Role::Operator && (added.iter().any(|a| a.name == n.name) || changed.iter().any(|(c, _)| c.name == n.name)));
    if operators {
        t.push_str(OPERATOR_CAN);
    }
    if !added.is_empty() || !changed.is_empty() {
        t.push_str("Nobody you share with can change anything else, read Secrets, logs or a shell, or share, unshare, rename, delete or export your dashboard, or see your source settings. ");
    }
    if !removed.is_empty() {
        t.push_str(&format!("Stop sharing with {}? They lose access at once. ", removed.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if t.is_empty() {
        t.push_str("Nothing changes.");
    }
    t.push_str("You can change this at any time.");
    t
}

/// A shared source as the viewer gets it: the owner's `kube` source behind a wrapper that answers only the views the shared dashboard
/// shows. It cannot be made to ask anything else.
struct Scoped {
    id: String,
    inner: Box<dyn DataSource>,
    views: BTreeSet<String>,
}

impl DataSource for Scoped {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        self.inner.kind()
    }
    fn language(&self) -> Lang {
        self.inner.language()
    }
    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        self.inner.test(cancel)
    }
    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        Ok(self.inner.browse(path, cancel)?.into_iter().filter(|n| self.views.contains(&n.name)).collect())
    }
    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let name = super::kube::view_name(&req.text)?;
        if !self.views.contains(name) {
            return Err(DashError::read_only("this dashboard was shared with you: only the views it shows can be read").fix("Ask its owner for another dashboard."));
        }
        self.inner.query(req, cancel)
    }
    fn starter(&self, target: &str) -> String {
        self.inner.starter(target)
    }
}

/// What changed when a dashboard's viewers were set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShareChange {
    pub added: Vec<Grant>,
    /// Accounts that stay but now have another role.
    pub changed: Vec<Grant>,
    pub removed: Vec<String>,
}

/// What each operator last read of the owner's file (a hash of it), to notice that the owner changed it since. Key: data folder, account,
/// shared id.
fn bases() -> &'static Mutex<HashMap<String, u64>> {
    static B: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Set while an unattended run (Autopilot) works: an owner's save then goes through without asking about an operator's change
/// (the previous version is still kept as `<id>.json.bak`).
static UNATTENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_unattended(on: bool) {
    UNATTENDED.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn file_hash(dir: &Path, id: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::fs::read(dir.join(format!("{id}.json"))).unwrap_or_default().hash(&mut h);
    h.finish()
}

impl Env {
    /// May this account see the cluster on its own (and own a dashboard on it)? The administrators.
    pub fn is_admin(&self) -> bool {
        self.policy.manage_users
    }

    pub fn shares(&self) -> Shares {
        Shares::open(&self.data_dir)
    }

    /// The accounts one of my dashboards is shared with and as what; a name that is not an account any more is left out.
    pub fn shared_with(&self, id: &str) -> Vec<Grant> {
        self.shares().with_of(&self.user, id).into_iter().filter(|g| self.people.exists(&g.name)).collect()
    }

    /// What I may do with `share:<owner>/<id>` (`None`: it is not shared with me, or not a shared id).
    pub fn my_role(&self, id: &str) -> Option<Role> {
        let (owner, dash) = parse_share_id(id)?;
        self.people.exists(&owner).then(|| self.shares().role_of(&owner, &dash, &self.user)).flatten()
    }

    /// My dashboards, then the ones shared with me (each as `share:<owner>/<id>`, titled "<title> (shared by <owner>)").
    pub fn list_dashboards(&self) -> Vec<Entry> {
        let mut v = self.dashboards().list();
        if !self.policy.apps.contains(&App::Dash) {
            return v;
        }
        for (owner, id) in self.shares().for_viewer(&self.user) {
            if let Ok((d, _)) = self.read_shared(&owner, &id) {
                v.push(Entry { id: share_id(&owner, &id), title: format!("{} (shared by {owner})", d.title), panels: d.panels.len() });
            }
        }
        v
    }

    /// A dashboard of mine, or one shared with me (`share:...`). Opening a shared one remembers what the owner's file looked like, so a
    /// later save by an operator can tell whether the owner changed it in between.
    pub fn load_dashboard(&self, id: &str) -> Result<Dashboard, String> {
        if id.starts_with(SHARE_PREFIX) {
            let (owner, dash) = parse_share_id(id).ok_or_else(|| format!("`{}` is not a dashboard", super::frame::clip(id, 40)))?;
            let (d, hash) = self.read_shared(&owner, &dash)?;
            bases().lock().unwrap_or_else(|e| e.into_inner()).insert(format!("{}|{}|{id}", self.data_dir.display(), self.user), hash);
            return Ok(self.as_viewer_copy(&owner, &dash, d));
        }
        let d = self.dashboards().load(id)?;
        self.note_saved(id);
        Ok(d)
    }

    /// Remember the file of one of my dashboards as it is now, when it is shared: an operator's save after this is what
    /// [`Env::overwrite_warning`] looks for.
    pub fn note_saved(&self, id: &str) {
        if id.is_empty() || id.starts_with(SHARE_PREFIX) || self.shares().with_of(&self.user, id).is_empty() {
            return;
        }
        let key = format!("{}|{}|{id}", self.data_dir.display(), self.user);
        let hash = file_hash(self.dashboards().dir(), id);
        bases().lock().unwrap_or_else(|e| e.into_inner()).insert(key, hash);
    }

    /// Would saving this dashboard of mine overwrite what an operator saved after I opened it? A sentence to ask the person with, or
    /// `None` (nothing shared, nothing changed, or an unattended run is working). The save itself always keeps the previous version.
    pub fn overwrite_warning(&self, d: &Dashboard) -> Option<String> {
        if d.id.is_empty() || d.id.starts_with(SHARE_PREFIX) || UNATTENDED.load(std::sync::atomic::Ordering::Relaxed) || self.shares().with_of(&self.user, &d.id).is_empty() {
            return None;
        }
        let key = format!("{}|{}|{}", self.data_dir.display(), self.user, d.id);
        let base = bases().lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied()?;
        (base != file_hash(self.dashboards().dir(), &d.id)).then(|| format!("Someone you shared “{}” with changed it after you opened it. Saving now replaces their change (they can not get it back; you keep one backup of the version being replaced).", super::frame::clip(&d.title, 40)))
    }

    /// The owner's dashboard as it is in their file, with a hash of the file: only when it is shared with this account right now, the
    /// owner still exists, and it uses nothing but the local cluster source.
    fn read_shared(&self, owner: &str, id: &str) -> Result<(Dashboard, u64), String> {
        let gone = || "that dashboard is not shared with you (any more)".to_string();
        if owner == self.user || !self.shares().is_shared_with(owner, id, &self.user) || !self.people.exists(owner) {
            return Err(gone());
        }
        let boards = Dashboards::open(&self.people.home(owner, &self.data_dir));
        let hash = file_hash(boards.dir(), id);
        let d = boards.load(id).map_err(|_| gone())?;
        let owners = Sources::open(&self.data_dir, owner);
        for s in used_sources(&d) {
            if owners.get(&s).is_none_or(|c| c.kind != "kube") {
                return Err(gone());
            }
        }
        Ok((d, hash))
    }

    /// The dashboard as its viewer works on it: its source ids rewritten to the shared form, its id `share:<owner>/<id>`.
    fn as_viewer_copy(&self, owner: &str, id: &str, mut d: Dashboard) -> Dashboard {
        let map = |s: &mut String| {
            if !s.is_empty() {
                *s = share_source_id(owner, id, s);
            }
        };
        map(&mut d.default_source);
        for p in &mut d.panels {
            map(&mut p.source);
            p.queries.iter_mut().for_each(|q| map(&mut q.source));
        }
        for v in &mut d.variables {
            if let super::model::VarKind::Query { source, .. } = &mut v.kind {
                map(source);
            }
        }
        d.id = share_id(owner, id);
        d
    }

    /// Save a dashboard: one of mine as it is. One shared with me only when I am an OPERATOR of it, and then only what an operator may
    /// change (panels, layout, time range, refresh) goes into the owner's file: the title, description, variables and default source stay
    /// the owner's; every panel must use the dashboard's own cluster source and a view the owner's saved dashboard already shows. If the
    /// owner changed the file after it was opened here, nothing is written and the message says so. The owner's previous version is kept
    /// as `<id>.json.bak` (see `Dashboards::save`).
    pub fn save_dashboard(&self, d: &Dashboard) -> Result<(), String> {
        if !d.id.starts_with(SHARE_PREFIX) {
            return self.dashboards().save(d);
        }
        let (owner, dash) = parse_share_id(&d.id).ok_or("that is not a dashboard")?;
        match self.shares().role_of(&owner, &dash, &self.user) {
            Some(Role::Operator) => {}
            Some(Role::Viewer) => return Err("You are a viewer of this dashboard: it is view only. Ask its owner to make you an operator.".into()),
            None => return Err("that dashboard is not shared with you (any more)".into()),
        }
        let (cur, hash_now) = self.read_shared(&owner, &dash)?;
        let key = format!("{}|{}|{}", self.data_dir.display(), self.user, d.id);
        let base = bases().lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied();
        if base != Some(hash_now) {
            return Err(format!("{owner} changed this dashboard after you opened it, so nothing was saved (their version is untouched). Open it again to see their changes, then redo yours."));
        }
        if d.title.trim() != cur.title {
            return Err("Operators cannot rename the dashboard: only its owner can.".into());
        }
        let used = used_sources(&cur);
        let unmap = |s: &str| -> Result<String, String> {
            if s.is_empty() {
                return Ok(String::new());
            }
            match parse_share_source_id(s) {
                Some((o, dd, src)) if o == owner && dd == dash && used.contains(&src) => Ok(src),
                _ => Err("Operators can only use the cluster source this dashboard already has.".to_string()),
            }
        };
        let mut shown: BTreeSet<String> = BTreeSet::new();
        for p in &cur.panels {
            for (_, q) in p.query_defs(&cur.default_source) {
                if let Ok(v) = super::kube::view_name(&q.query.text) {
                    shown.insert(v.to_string());
                }
            }
        }
        let mut panels = d.panels.clone();
        for p in &mut panels {
            p.source = unmap(&p.source)?;
            for q in &mut p.queries {
                q.source = unmap(&q.source)?;
            }
            let texts = std::iter::once(&p.query.text).chain(p.queries.iter().map(|q| &q.query.text));
            if p.visual.needs_query() {
                for text in texts {
                    match super::kube::view_name(text) {
                        Ok(v) if shown.contains(v) => {}
                        _ => return Err(format!("Operators can only use the views the dashboard already shows; “{}” is not one of them. The owner can add it.", super::frame::clip(text.trim(), 40))),
                    }
                }
            }
        }
        let mut new = cur;
        new.panels = panels;
        new.time = d.time.clone();
        new.refresh_s = d.refresh_s;
        new.sanitize();
        let boards = Dashboards::open(&self.people.home(&owner, &self.data_dir));
        boards.save(&new)?;
        bases().lock().unwrap_or_else(|e| e.into_inner()).insert(key, file_hash(boards.dir(), &dash));
        (self.audit)("dash.edited_shared", &format!("{dash}: {} (operator)", self.user));
        Ok(())
    }

    /// The connector for `share:<owner>/<id>/<source>`: only for an account the dashboard is shared with, only the owner's `kube`
    /// source, only the views that dashboard shows.
    pub(crate) fn shared_source(&self, sid: &str) -> Result<(SourceConfig, Box<dyn DataSource>), DashError> {
        let denied = || DashError::config("that dashboard is not shared with you (any more)");
        let (owner, dash, src) = parse_share_source_id(sid).ok_or_else(|| DashError::config("that is not a shared source"))?;
        if owner == self.user || !self.shares().is_shared_with(&owner, &dash, &self.user) || !self.people.exists(&owner) {
            return Err(denied());
        }
        let d = Dashboards::open(&self.people.home(&owner, &self.data_dir)).load(&dash).map_err(|_| denied())?;
        if !used_sources(&d).contains(&src) {
            return Err(denied());
        }
        let cfg = Sources::open(&self.data_dir, &owner).get(&src).filter(|c| c.kind == "kube").ok_or_else(denied)?;
        let mut views = BTreeSet::new();
        for p in &d.panels {
            for (_, q) in p.query_defs(&d.default_source) {
                if q.source == src {
                    if let Ok(v) = super::kube::view_name(&q.query.text) {
                        views.insert(v.to_string());
                    }
                }
            }
        }
        let inner = super::registry::build(&cfg, &self.deps)?;
        let shown = SourceConfig { id: sid.into(), name: format!("{} (shared by {owner})", cfg.name), ..cfg };
        Ok((shown, Box::new(Scoped { id: sid.into(), inner, views })))
    }

    /// Share one of my dashboards with exactly these accounts, each as viewer or operator (empty: stop sharing). Audited with account
    /// names and roles only.
    pub fn set_shares(&self, id: &str, with: &[Grant]) -> Result<ShareChange, String> {
        if id.starts_with(SHARE_PREFIX) {
            return Err("a dashboard shared with you can only be shared by its owner".into());
        }
        if !self.policy.apps.contains(&App::Dash) {
            return Err("your account may not use the dashboards".into());
        }
        let d = self.dashboards().load(id)?;
        for g in with.iter().filter(|g| !g.name.trim().is_empty()) {
            if valid_username(g.name.trim()) && g.name.trim() != self.user && !self.people.exists(g.name.trim()) {
                return Err(format!("there is no account `{}`", g.name.trim()));
            }
        }
        if with.iter().any(|g| !g.name.trim().is_empty()) {
            if !self.is_admin() {
                return Err("only an administrator can share a dashboard of the cluster".into());
            }
            let mine = self.sources();
            for s in used_sources(&d) {
                match mine.get(&s) {
                    Some(c) if c.kind == "kube" => {}
                    _ => return Err("only dashboards that use nothing but the local cluster can be shared; this one uses another source".into()),
                }
            }
        }
        let ch = self.shares().set(&self.user, id, with)?;
        if !ch.added.is_empty() {
            (self.audit)("dash.shared", &format!("{id}: {}", grants_text(&ch.added)));
        }
        if !ch.changed.is_empty() {
            (self.audit)("dash.share_role", &format!("{id}: {}", grants_text(&ch.changed)));
        }
        if !ch.removed.is_empty() {
            (self.audit)("dash.unshared", &format!("{id}: {}", ch.removed.join(", ")));
        }
        Ok(ch)
    }

    /// Delete one of my dashboards and stop sharing it (a new dashboard with the same id would otherwise be shared again). A dashboard
    /// shared with me cannot be deleted, whatever my role.
    pub fn delete_dashboard(&self, id: &str) -> Result<bool, String> {
        if id.starts_with(SHARE_PREFIX) {
            return Err("a dashboard shared with you can only be deleted by its owner".into());
        }
        let had = self.shares().with_of(&self.user, id);
        let r = self.dashboards().delete(id)?;
        if !had.is_empty() {
            self.shares().set(&self.user, id, &[])?;
            (self.audit)("dash.unshared", &format!("{id}: {}", had.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        Ok(r)
    }
}
