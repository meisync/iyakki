//! Workspaces of the AI IDE: one folder per product, a registry of them per person, the project file kept inside each
//! (`.iyakki/project.json`), an advisory lock so two sessions do not edit the same one unaware, and the file operations
//! (tree, search, create, rename, delete) that never leave the workspace's root.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PROJECT_DIR: &str = ".iyakki";
pub const PROJECT_FILE: &str = "project.json";
pub const LOCK_FILE: &str = "lock";
/// A lock nobody refreshed for this long is stale.
pub const LOCK_TTL_SECS: u64 = 90;
pub const MAX_WORKSPACES: usize = 40;
pub const MAX_SEARCH_FILE: u64 = 512 * 1024;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ------------------------------------------------------------------------------------------------------ project.json

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct K8sConfig {
    /// A cluster profile / kubeconfig context name (empty: `$KUBECONFIG` or the default).
    pub profile: String,
    pub namespace: String,
    /// A label selector such as `app=web`.
    pub selector: String,
    /// Path of a kubeconfig file (inside the workspace or the person's scope); empty for the default.
    pub kubeconfig: String,
    /// `host:port` of the registry a remote cluster pulls from (empty: found in the cluster).
    pub registry: String,
    /// The address the NODES pull from when it differs from `registry` (the address this machine pushes to), e.g.
    /// `localhost:30500` for a registry that runs inside the cluster as a NodePort Service. Empty: the same.
    pub registry_pull: String,
    /// Which registry option was chosen: `none`, `in-cluster`, `system` or `remote` (empty: not chosen).
    pub registry_kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Commands {
    pub test: String,
    pub run: String,
    pub build: String,
    /// A command that prints (or writes) a coverage report; empty = detect the project's usual tool.
    pub coverage: String,
}

/// `preview` in project.json: the page the IDE's Browser tab showed last (a deployed app's address, a dev server).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

impl PreviewConfig {
    fn is_empty(&self) -> bool {
        self.url.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub name: String,
    pub k8s: K8sConfig,
    pub features_file: String,
    /// The language of the project when the files do not say it (or say it wrongly): `Go`, `TypeScript`, `Python`… (empty: detected).
    pub stack: String,
    /// What the workspace is for (`iyakki-tile`, `k3s-service`, `mcp-server`, `skill`, `wasm-tool`, `library/other`; see `target.rs`); empty: not chosen.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub commands: Commands,
    #[serde(skip_serializing_if = "PreviewConfig::is_empty")]
    pub preview: PreviewConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig { name: String::new(), k8s: K8sConfig::default(), features_file: "FEATURES.md".into(), stack: String::new(), target: String::new(), commands: Commands::default(), preview: PreviewConfig::default() }
    }
}

/// `My Product!` gives `my-product`: a name usable as a Kubernetes namespace or a folder.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let t = out.trim_end_matches('-');
    let t: String = t.chars().take(40).collect();
    if t.is_empty() { "workspace".into() } else { t }
}

impl ProjectConfig {
    pub fn path(root: &Path) -> PathBuf {
        root.join(PROJECT_DIR).join(PROJECT_FILE)
    }

    /// The namespace to look at: the configured one, else `<workspace>-dev`.
    pub fn namespace(&self, fallback_name: &str) -> String {
        if !self.k8s.namespace.trim().is_empty() {
            return self.k8s.namespace.trim().to_string();
        }
        let n = if self.name.trim().is_empty() { fallback_name } else { self.name.as_str() };
        format!("{}-dev", slug(n))
    }

    pub fn features_path(&self, root: &Path) -> PathBuf {
        let f = if self.features_file.trim().is_empty() { "FEATURES.md" } else { self.features_file.trim() };
        root.join(f)
    }

    /// Read the project file; a missing file gives the defaults, a broken one an error (so it is not overwritten blindly).
    pub fn load(root: &Path) -> Result<ProjectConfig, String> {
        match std::fs::read_to_string(Self::path(root)) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", Self::path(root).display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProjectConfig::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Create the file with defaults when the folder has none (the first use of a folder as a project).
    pub fn ensure(root: &Path, name: &str) -> Result<ProjectConfig, String> {
        let p = Self::path(root);
        if p.exists() {
            return Self::load(root);
        }
        let mut c = ProjectConfig::default();
        c.name = name.to_string();
        c.save(root)?;
        Ok(c)
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let p = Self::path(root);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let t = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::featureboard::write_with_backup(&p, &(t + "\n")).map(|_| {
            let _ = std::fs::remove_file(p.with_file_name("project.json.bak"));
        })
    }
}

// ----------------------------------------------------------------------------------------------------------- registry

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub root: String,
    #[serde(default)]
    pub last_opened: u64,
    /// Index into [`ACCENTS`].
    #[serde(default)]
    pub accent: u8,
    #[serde(default)]
    pub pinned: bool,
}

pub const ACCENTS: [[u8; 3]; 8] = [[70, 225, 255], [255, 80, 190], [110, 224, 160], [255, 196, 94], [167, 116, 255], [255, 130, 100], [88, 190, 235], [176, 226, 92]];

impl Workspace {
    pub fn color(&self) -> [u8; 4] {
        let a = ACCENTS[self.accent as usize % ACCENTS.len()];
        [a[0], a[1], a[2], 255]
    }
    pub fn root_path(&self) -> PathBuf {
        PathBuf::from(&self.root)
    }
    /// A short key naming this workspace's chats in the session store (valid as a user key there).
    pub fn chat_key(&self, user: &str) -> String {
        format!("ws_{:016x}", fnv(format!("{user}\u{1}{}", self.id).as_bytes()))
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    items: Vec<Workspace>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    parent: Option<String>,
}

pub struct Registry {
    path: PathBuf,
    pub items: Vec<Workspace>,
    pub last: Option<String>,
    /// The folder new workspaces are created in, as the person last chose it.
    pub parent: Option<String>,
}

fn valid_user_key(user: &str) -> bool {
    iyakki_sec::policy::valid_username(user)
}

impl Registry {
    /// The registry of `user`, kept as `ide-workspaces-<user>.json` in `dir` (the services folder).
    pub fn load(dir: &Path, user: &str) -> Registry {
        let path = dir.join(format!("ide-workspaces-{}.json", if valid_user_key(user) { user } else { "invalid" }));
        let f: RegistryFile = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Registry { path, items: f.items, last: f.last, parent: f.parent }
    }

    /// Read the list again from the file (another window may have changed it), so a change made here does not erase theirs.
    pub fn reload(&mut self) {
        if let Some(f) = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice::<RegistryFile>(&b).ok()) {
            self.items = f.items;
            self.last = f.last;
            self.parent = f.parent;
        }
    }

    pub fn save(&self) -> Result<(), String> {
        use std::io::Write;
        if self.path.to_string_lossy().ends_with("-invalid.json") {
            return Err("invalid user".into());
        }
        let t = serde_json::to_vec_pretty(&RegistryFile { items: self.items.clone(), last: self.last.clone(), parent: self.parent.clone() }).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("tmp");
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&t).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.items.iter().find(|w| w.id == id)
    }

    /// Register a folder. The root must exist; the same folder twice is refused (and so is a folder inside or around another
    /// workspace, which would let two products share files).
    pub fn add(&mut self, name: &str, root: &Path) -> Result<Workspace, String> {
        self.reload();
        let root = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a folder", root.display()));
        }
        if self.items.len() >= MAX_WORKSPACES {
            return Err(format!("at most {MAX_WORKSPACES} workspaces; remove one from the list first"));
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(|c| c.is_control()) {
            return Err("a workspace name is 1 to 40 characters".into());
        }
        for w in &self.items {
            let other = Path::new(&w.root);
            if other == root {
                return Err(format!("{} is already the workspace \"{}\"", root.display(), w.name));
            }
            if root.starts_with(other) || other.starts_with(&root) {
                return Err(format!("{} overlaps the workspace \"{}\" ({}): products need separate folders", root.display(), w.name, w.root));
            }
        }
        if self.items.iter().any(|w| w.name.eq_ignore_ascii_case(name)) {
            return Err(format!("there is already a workspace named \"{name}\""));
        }
        let mut r = [0u8; 4];
        iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
        let id = format!("w_{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let ws = Workspace { id, name: name.into(), root: root.to_string_lossy().into_owned(), last_opened: now_secs(), accent: (self.items.len() % ACCENTS.len()) as u8, pinned: false };
        self.items.push(ws.clone());
        self.last = Some(ws.id.clone());
        self.save()?;
        Ok(ws)
    }

    /// Create a new product folder `parent/<slug>` (with `.iyakki/project.json`, and `git init` when `git` is true) and register it.
    pub fn create(&mut self, name: &str, parent: &Path, git: bool) -> Result<Workspace, String> {
        let dir = parent.join(slug(name));
        if dir.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
            return Err(format!("{} already exists and is not empty: open it as an existing folder instead", dir.display()));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let ws = self.add(name, &dir)?;
        ProjectConfig::ensure(&ws.root_path(), name)?;
        if git {
            let _ = crate::wsexec::git_init(&ws.root_path());
        }
        Ok(ws)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        self.reload();
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err("a workspace name is 1 to 40 characters".into());
        }
        if self.items.iter().any(|w| w.id != id && w.name.eq_ignore_ascii_case(name)) {
            return Err(format!("there is already a workspace named \"{name}\""));
        }
        let w = self.items.iter_mut().find(|w| w.id == id).ok_or("no such workspace")?;
        w.name = name.into();
        self.save()
    }

    /// Take a workspace off the list. The folder and its files stay untouched.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.reload();
        let n = self.items.len();
        self.items.retain(|w| w.id != id);
        if self.items.len() == n {
            return Err("no such workspace".into());
        }
        if self.last.as_deref() == Some(id) {
            self.last = None;
        }
        self.save()
    }

    pub fn pin(&mut self, id: &str, on: bool) -> Result<(), String> {
        self.reload();
        self.items.iter_mut().find(|w| w.id == id).ok_or("no such workspace")?.pinned = on;
        self.save()
    }

    pub fn set_accent(&mut self, id: &str, accent: u8) -> Result<(), String> {
        self.reload();
        self.items.iter_mut().find(|w| w.id == id).ok_or("no such workspace")?.accent = accent % ACCENTS.len() as u8;
        self.save()
    }

    pub fn touch(&mut self, id: &str) {
        if let Some(w) = self.items.iter_mut().find(|w| w.id == id) {
            w.last_opened = now_secs();
            self.last = Some(id.to_string());
            let _ = self.save();
        }
    }

    /// Pinned first, then the most recently opened.
    pub fn ordered(&self) -> Vec<Workspace> {
        let mut v = self.items.clone();
        v.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.last_opened.cmp(&a.last_opened)).then(a.name.cmp(&b.name)));
        v
    }
}

/// Can a folder be created (or already is) at `p` and written to? A probe file is made and removed.
pub fn writable_folder(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    let probe = p.join(format!(".iyakki-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// A raw operating-system error turned into something a person can act on.
pub fn friendly_io(err: &str, folder: &Path, suggestion: &Path) -> String {
    if err.contains("Permission denied") || err.contains("os error 13") || err.contains("Read-only file system") || err.contains("os error 30") {
        format!("IYAKKI cannot create folders in {}. Choose another parent folder (for example {}) or press Ctrl+U to use {}.", folder.display(), suggestion.display(), suggestion.display())
    } else {
        err.to_string()
    }
}

/// Pairs of workspaces that look at the same Kubernetes context and namespace (they would step on each other's workloads).
pub fn namespace_collisions(list: &[(String, ProjectConfig)]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (i, (na, ca)) in list.iter().enumerate() {
        for (nb, cb) in &list[i + 1..] {
            let (a, b) = (ca.namespace(na), cb.namespace(nb));
            if a == b && ca.k8s.profile == cb.k8s.profile && ca.k8s.kubeconfig == cb.k8s.kubeconfig {
                out.push((na.clone(), nb.clone(), a));
            }
        }
    }
    out
}

// -------------------------------------------------------------------------------------------------------------- lock

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub user: String,
    pub session: String,
    pub pid: u32,
    pub host: String,
    /// When the last heartbeat was written.
    pub time: u64,
    /// When this session took the lock (kept across heartbeats); 0 in locks written by older versions.
    #[serde(default)]
    pub opened: u64,
}

impl LockInfo {
    /// Held by another window of the same account on this very computer (the common "I started it twice" case).
    pub fn same_account_here(&self, user: &str) -> bool {
        self.user == user && self.host == host_name()
    }

    /// The sentence the lock dialog shows.
    pub fn describe(&self, user: &str) -> String {
        let (h, m) = ((self.opened.max(1) / 3600) % 24, (self.opened.max(1) / 60) % 60);
        if self.same_account_here(user) {
            format!("This workspace is open in another IYAKKI window on this computer (process {}, opened {h:02}:{m:02} UTC). Take over, open read-only, or close that window.", self.pid)
        } else {
            format!("{} has it open in another IYAKKI session (process {} on {}, opened {h:02}:{m:02} UTC). Opening it here read-only is safe. Taking it over makes the other session read-only, and unsaved work there is not lost but can no longer be saved.", self.user, self.pid, self.host)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockState {
    /// This session holds it now.
    Mine,
    /// Somebody else (another IYAKKI session, maybe another person) holds it and is alive.
    Held(LockInfo),
}

fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "host".into())
}

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join(LOCK_FILE)
}

pub fn read_lock(root: &Path) -> Option<LockInfo> {
    serde_json::from_slice(&std::fs::read(lock_path(root)).ok()?).ok()
}

/// A lock is stale when nobody refreshed it lately, or when its process is gone (same machine).
/// A heartbeat dated far in the future (the writer's clock runs ahead) is not trusted by itself: see [`lock_live`].
pub fn lock_stale(l: &LockInfo, now: u64) -> bool {
    if now.saturating_sub(l.time) > LOCK_TTL_SECS {
        return true;
    }
    l.host == host_name() && l.pid != std::process::id() && !crate::jobs::process_exists(l.pid)
}

/// The lock of `root` if a live session holds it. Clock skew is handled with the lock file's own modification time (set by
/// the file system of the machine that wrote it): a stamp more than a TTL ahead of `now` counts only while the file is fresh.
pub fn lock_live(root: &Path, now: u64) -> Option<LockInfo> {
    let l = read_lock(root)?;
    if lock_stale(&l, now) {
        return None;
    }
    if l.time > now + LOCK_TTL_SECS {
        let age = std::fs::metadata(lock_path(root)).and_then(|m| m.modified()).ok().and_then(|t| std::time::SystemTime::now().duration_since(t).ok()).map_or(u64::MAX, |d| d.as_secs());
        if age > LOCK_TTL_SECS {
            return None;
        }
    }
    Some(l)
}

fn write_lock(root: &Path, l: &LockInfo) -> Result<(), String> {
    let p = lock_path(root);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(l).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

fn mine(user: &str, session: &str, now: u64, opened: u64) -> LockInfo {
    LockInfo { user: user.into(), session: session.into(), pid: std::process::id(), host: host_name(), time: now, opened }
}

/// Take the lock unless a live one belongs to another session. `session` names this IYAKKI session (one per window).
/// When the folder cannot be written the lock cannot be kept either: the session is then "Mine" (nobody can be told otherwise)
/// and the folder's own permissions keep the files safe.
pub fn acquire_lock(root: &Path, user: &str, session: &str, now: u64) -> LockState {
    if let Some(l) = lock_live(root, now) {
        if l.session != session {
            return LockState::Held(l);
        }
    }
    let _ = write_lock(root, &mine(user, session, now, now));
    LockState::Mine
}

/// Keep a held lock fresh (called every half minute while the workspace is open: one small file write). Returns false when
/// somebody else holds it now (they took it over).
pub fn refresh_lock(root: &Path, user: &str, session: &str, now: u64) -> bool {
    match lock_live(root, now) {
        Some(l) if l.session != session => false,
        other => {
            let opened = other.map_or(now, |l| if l.opened == 0 { now } else { l.opened });
            let _ = write_lock(root, &mine(user, session, now, opened));
            true
        }
    }
}

pub fn take_over_lock(root: &Path, user: &str, session: &str, now: u64) {
    let _ = write_lock(root, &mine(user, session, now, now));
}

pub fn release_lock(root: &Path, session: &str) {
    if read_lock(root).is_some_and(|l| l.session == session) {
        let _ = std::fs::remove_file(lock_path(root));
    }
}

// ------------------------------------------------------------------------------------------------ paths and the tree

/// Folders that are not shown in the tree or searched unless asked for: build output, dependencies, version control.
pub fn hidden_dir(name: &str) -> bool {
    matches!(name, "target" | "node_modules" | ".git" | ".hg" | ".svn" | "__pycache__" | ".venv" | "venv" | ".mypy_cache" | ".pytest_cache" | ".gradle" | ".idea" | ".next" | ".cache" | "build-output")
}

/// Names a root `.gitignore` lists as plain entries (`out`, `out/`, `/out`), so those folders are collapsed too.
pub fn gitignored_names(root: &Path) -> Vec<String> {
    let Ok(t) = std::fs::read_to_string(root.join(".gitignore")) else { return vec![] };
    t.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!') && !l.contains(['*', '?', '[']))
        .map(|l| l.trim_matches('/').to_string())
        .filter(|l| !l.is_empty() && !l.contains('/'))
        .collect()
}

/// Resolve `rel` (or an absolute path) inside `root`: no `..` out of it, no symlink pointing out of it. The result is the
/// real path. The last component need not exist (a file about to be created).
pub fn resolve_in(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root_real = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let p = Path::new(rel.trim());
    let joined = if p.is_absolute() { p.to_path_buf() } else { root_real.join(p) };
    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                if !norm.pop() {
                    return Err(format!("{rel}: outside the workspace"));
                }
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    // follow symlinks of the longest existing prefix
    let mut existing = norm.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => tail.push(n.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut real = std::fs::canonicalize(&existing).map_err(|e| format!("{rel}: {e}"))?;
    for t in tail.into_iter().rev() {
        real.push(t);
    }
    if real.starts_with(&root_real) {
        Ok(real)
    } else {
        Err(format!("{rel}: outside the workspace {} (the assistant and the IDE stay inside the workspace folder)", root_real.display()))
    }
}

/// `rel` as shown: the path below the root.
pub fn rel_of(root: &Path, p: &Path) -> String {
    let r = std::fs::canonicalize(root).unwrap_or_else(|_| root.into());
    p.strip_prefix(&r).or_else(|_| p.strip_prefix(root)).unwrap_or(p).to_string_lossy().into_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// A link that leaves the workspace (shown, never followed).
    pub escapes: bool,
    /// A folder that is collapsed by default: build output, dependencies, version control.
    pub heavy: bool,
}

/// The entries of a folder of the workspace: folders first, then files, each sorted by name.
pub fn list_dir(root: &Path, rel: &str, show_heavy: bool) -> Result<Vec<Entry>, String> {
    let dir = resolve_in(root, rel)?;
    let root_real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let ignored = gitignored_names(&root_real);
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{rel}: {e}"))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == PROJECT_DIR && !show_heavy {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let (mut is_dir, mut escapes) = (ft.is_dir(), false);
        if ft.is_symlink() {
            match std::fs::canonicalize(e.path()) {
                Ok(t) if t.starts_with(&root_real) => is_dir = t.is_dir(),
                _ => escapes = true,
            }
        }
        let heavy = is_dir && (hidden_dir(&name) || ignored.contains(&name));
        if heavy && !show_heavy {
            continue;
        }
        out.push(Entry { name, is_dir: is_dir && !escapes, escapes, heavy });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

fn walk_files(root: &Path, dir: &Path, depth: usize, ignored: &[String], cap: usize, out: &mut Vec<PathBuf>) {
    if depth > 12 || out.len() >= cap {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<_> = rd.flatten().collect();
    es.sort_by_key(|e| e.file_name());
    for e in es {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if !hidden_dir(&name) && !ignored.contains(&name) && name != PROJECT_DIR {
                walk_files(root, &e.path(), depth + 1, ignored, cap, out);
            }
        } else if ft.is_file() {
            out.push(e.path());
            if out.len() >= cap {
                return;
            }
        }
    }
}

/// Every file of the workspace (without the collapsed folders), relative paths, up to `cap`.
pub fn all_files(root: &Path, cap: usize) -> Vec<String> {
    let Ok(real) = std::fs::canonicalize(root) else { return vec![] };
    let ig = gitignored_names(&real);
    let mut v = Vec::new();
    walk_files(&real, &real, 0, &ig, cap, &mut v);
    v.into_iter().map(|p| p.strip_prefix(&real).unwrap_or(&p).to_string_lossy().into_owned()).collect()
}

/// Letters of `q` in order inside `n`: a score (more for runs and word starts), or `None`.
fn subsequence(q: &[char], n: &[char]) -> Option<i32> {
    let (mut qi, mut score, mut last) = (0, 0i32, None::<usize>);
    for (i, c) in n.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            score += 10;
            if last == Some(i.wrapping_sub(1)) {
                score += 8;
            }
            if i == 0 || matches!(n[i - 1], '/' | '_' | '-' | '.') {
                score += 6;
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score)
}

/// Score of a match of `q` in the path `name`, higher is better, `None` when it does not match. The file's own name counts
/// most (exact, then its start, then anywhere in it, then letters in order), the folders only count as a last resort.
pub fn fuzzy(q: &str, name: &str) -> Option<i32> {
    let (ql, nl) = (q.trim().to_lowercase(), name.to_lowercase());
    if ql.is_empty() {
        return Some(0);
    }
    let base = nl.rsplit('/').next().unwrap_or(&nl);
    let len = nl.chars().count() as i32;
    if base == ql {
        return Some(1000 - len);
    }
    if base.starts_with(&ql) {
        return Some(800 - len);
    }
    if base.contains(&ql) {
        return Some(600 - len);
    }
    let qc: Vec<char> = ql.chars().collect();
    let bc: Vec<char> = base.chars().collect();
    if let Some(sc) = subsequence(&qc, &bc) {
        return Some(300 + sc - len);
    }
    if nl.contains(&ql) {
        return Some(200 - len);
    }
    let nc: Vec<char> = nl.chars().collect();
    subsequence(&qc, &nc).map(|sc| 100 + sc - len.min(90))
}

/// Quick open: files whose path matches `q`, best first.
pub fn find_files(root: &Path, q: &str, max: usize) -> Vec<String> {
    let mut v: Vec<(i32, String)> = all_files(root, 20_000).into_iter().filter_map(|f| fuzzy(q, &f).map(|s| (s, f))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(&b.1)));
    v.into_iter().take(max).map(|x| x.1).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// Project search for text (case-insensitive), within caps: at most `max_hits` lines, 5 000 files, 512 KB per file.
pub fn grep(root: &Path, query: &str, max_hits: usize) -> Vec<Hit> {
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let Ok(real) = std::fs::canonicalize(root) else { return out };
    let ig = gitignored_names(&real);
    let mut files = Vec::new();
    walk_files(&real, &real, 0, &ig, 5_000, &mut files);
    for f in files {
        if out.len() >= max_hits {
            break;
        }
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        if meta.len() > MAX_SEARCH_FILE {
            continue;
        }
        let Ok(bytes) = std::fs::read(&f) else { continue };
        if bytes.iter().take(2048).any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        for (n, l) in text.lines().enumerate() {
            if l.to_lowercase().contains(&q) {
                out.push(Hit { file: f.strip_prefix(&real).unwrap_or(&f).to_string_lossy().into_owned(), line: n + 1, text: l.trim().chars().take(160).collect() });
                if out.len() >= max_hits {
                    break;
                }
            }
        }
    }
    out
}

// --------------------------------------------------------------------------------------------------- file operations

fn valid_name(n: &str) -> Result<(), String> {
    if n.is_empty() || n == "." || n == ".." || n.contains(['/', '\0']) || n.chars().count() > 200 {
        Err(format!("\"{n}\" is not a valid file name"))
    } else {
        Ok(())
    }
}

/// Create an empty file (with its folders) at `rel`; refuses to overwrite.
pub fn create_file(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = resolve_in(root, rel)?;
    if p.exists() {
        return Err(format!("{rel} already exists"));
    }
    valid_name(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    // the parent chain may have just been created: check again after creating it
    let p = resolve_in(root, rel)?;
    std::fs::write(&p, b"").map_err(|e| format!("{rel}: {e}"))?;
    Ok(p)
}

pub fn create_dir(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = resolve_in(root, rel)?;
    if p.exists() {
        return Err(format!("{rel} already exists"));
    }
    std::fs::create_dir_all(&p).map_err(|e| format!("{rel}: {e}"))?;
    resolve_in(root, rel)
}

/// Rename the last component of `rel` to `new_name` (no moving between folders).
pub fn rename(root: &Path, rel: &str, new_name: &str) -> Result<PathBuf, String> {
    valid_name(new_name)?;
    let from = resolve_in(root, rel)?;
    let root_real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if from == root_real {
        return Err("the workspace folder itself cannot be renamed here".into());
    }
    let to = from.with_file_name(new_name);
    if to.exists() {
        return Err(format!("{new_name} already exists"));
    }
    std::fs::rename(&from, &to).map_err(|e| format!("{rel}: {e}"))?;
    Ok(to)
}

/// Delete a file or a folder with everything in it (the UI asks first). The workspace root is never deleted.
pub fn delete(root: &Path, rel: &str) -> Result<(), String> {
    let rp = Path::new(rel.trim());
    // a symlink is removed itself, never what it points to (even when it points out of the workspace)
    if let Some(name) = rp.file_name() {
        let parent = rp.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let target = resolve_in(root, &parent)?.join(name);
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return std::fs::remove_file(&target).map_err(|e| format!("{rel}: {e}"));
        }
    }
    let p = resolve_in(root, rel)?;
    let root_real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    if p == root_real {
        return Err("the workspace folder itself is not deleted from here".into());
    }
    if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }.map_err(|e| format!("{rel}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-ws-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn slugs_and_default_namespaces() {
        assert_eq!(slug("My Product!"), "my-product");
        assert_eq!(slug("   "), "workspace");
        let c = ProjectConfig::default();
        assert_eq!(c.namespace("Shop Front"), "shop-front-dev");
        let mut c2 = c.clone();
        c2.k8s.namespace = "prod-shop".into();
        assert_eq!(c2.namespace("x"), "prod-shop");
    }

    #[test]
    fn registry_add_remove_rename_pin_and_persist() {
        let d = tmp("reg");
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut r = Registry::load(&d.join("svc"), "alice");
        let wa = r.add("Alpha", &a).unwrap();
        let wb = r.add("Beta", &b).unwrap();
        assert_ne!(wa.id, wb.id);
        assert_ne!(wa.chat_key("alice"), wb.chat_key("alice"), "each workspace has its own chat store");
        assert!(iyakki_sec::policy::valid_username(&wa.chat_key("alice")));
        assert!(r.add("Alpha", &d).is_err(), "name taken, and overlapping");
        assert!(r.add("Again", &a).unwrap_err().contains("already"));
        std::fs::create_dir_all(a.join("inner")).unwrap();
        assert!(r.add("Inner", &a.join("inner")).unwrap_err().contains("overlaps"));
        r.rename(&wa.id, "Alpha 2").unwrap();
        assert!(r.rename(&wa.id, "beta").is_err());
        r.pin(&wb.id, true).unwrap();
        assert_eq!(r.ordered()[0].name, "Beta");
        let again = Registry::load(&d.join("svc"), "alice");
        assert_eq!(again.items.len(), 2);
        assert_eq!(again.get(&wa.id).unwrap().name, "Alpha 2");
        assert!(Registry::load(&d.join("svc"), "bob").items.is_empty(), "per person");
        r.remove(&wa.id).unwrap();
        assert!(a.is_dir(), "removing from the list never deletes files");
        assert!(r.remove("nope").is_err());
        assert!(Registry::load(&d.join("svc"), "../evil").save().is_err());
    }

    #[test]
    fn creating_a_workspace_makes_the_folder_and_project_file() {
        let d = tmp("create");
        let mut r = Registry::load(&d.join("svc"), "alice");
        let w = r.create("Shop Front", &d.join("Workspaces"), false).unwrap();
        assert!(w.root.ends_with("Workspaces/shop-front"));
        let c = ProjectConfig::load(&w.root_path()).unwrap();
        assert_eq!((c.name.as_str(), c.features_file.as_str()), ("Shop Front", "FEATURES.md"));
        assert_eq!(c.namespace(&w.name), "shop-front-dev");
        std::fs::write(w.root_path().join("x"), "1").unwrap();
        assert!(r.create("Shop Front", &d.join("Workspaces"), false).is_err());
        // a broken project file is reported, not overwritten
        std::fs::write(ProjectConfig::path(&w.root_path()), "{not json").unwrap();
        assert!(ProjectConfig::ensure(&w.root_path(), "x").is_err());
    }

    #[test]
    fn namespaces_that_collide_are_reported() {
        let mut a = ProjectConfig::default();
        a.name = "One".into();
        let mut b = ProjectConfig::default();
        b.name = "Two".into();
        assert!(namespace_collisions(&[("One".into(), a.clone()), ("Two".into(), b.clone())]).is_empty());
        b.k8s.namespace = "one-dev".into();
        let c = namespace_collisions(&[("One".into(), a.clone()), ("Two".into(), b.clone())]);
        assert_eq!(c, vec![("One".into(), "Two".into(), "one-dev".into())]);
        b.k8s.profile = "other-cluster".into();
        assert!(namespace_collisions(&[("One".into(), a), ("Two".into(), b)]).is_empty(), "a different cluster is no collision");
    }

    #[test]
    fn locks_warn_the_second_session_and_stale_ones_expire() {
        let d = tmp("lock");
        let t = 1_000_000u64;
        assert_eq!(acquire_lock(&d, "alice", "s1", t), LockState::Mine);
        assert_eq!(acquire_lock(&d, "alice", "s1", t + 5), LockState::Mine, "the same session again");
        match acquire_lock(&d, "bob", "s2", t + 10) {
            LockState::Held(l) => assert_eq!((l.user.as_str(), l.session.as_str()), ("alice", "s1")),
            other => panic!("{other:?}"),
        }
        assert!(!refresh_lock(&d, "bob", "s2", t + 10), "cannot refresh somebody else's lock");
        assert!(refresh_lock(&d, "alice", "s1", t + 30));
        // nobody refreshed it
        assert_eq!(acquire_lock(&d, "bob", "s2", t + 30 + LOCK_TTL_SECS + 1), LockState::Mine, "stale lock expired");
        assert_eq!(read_lock(&d).unwrap().user, "bob");
        take_over_lock(&d, "alice", "s1", t + 500);
        assert_eq!(read_lock(&d).unwrap().session, "s1");
        release_lock(&d, "s2");
        assert!(read_lock(&d).is_some(), "only the holder releases");
        release_lock(&d, "s1");
        assert!(read_lock(&d).is_none());
        // a lock of a dead process on this machine is stale even when fresh
        let dead = LockInfo { user: "x".into(), session: "z".into(), pid: 4_000_000_000, host: host_name(), time: t, opened: t };
        assert!(lock_stale(&dead, t));
    }

    #[test]
    fn lock_lifecycle_heartbeat_takeover_skew_pid_reuse_and_odd_folders() {
        let d = tmp("lock2");
        let t = 2_000_000_000u64;
        // acquire, then heartbeats keep the opening time and move the stamp
        assert_eq!(acquire_lock(&d, "ann", "w1", t), LockState::Mine);
        assert!(refresh_lock(&d, "ann", "w1", t + 30));
        let l = read_lock(&d).unwrap();
        assert_eq!((l.time, l.opened), (t + 30, t), "the heartbeat moves the stamp, not the opening time");
        // a heartbeat every 30 s keeps it live far past one TTL
        for k in 1..10 {
            assert!(refresh_lock(&d, "ann", "w1", t + 30 * k));
        }
        assert!(lock_live(&d, t + 30 * 9 + LOCK_TTL_SECS).is_some());
        // takeover by a second window: the owner's next heartbeat sees a different session
        take_over_lock(&d, "ann", "w2", t + 300);
        assert!(!refresh_lock(&d, "ann", "w1", t + 310), "the old owner notices and must not fight back");
        assert_eq!(read_lock(&d).unwrap().session, "w2", "and did not overwrite it");
        // same pid reused by another program: the pid looks alive, so only the TTL frees the lock
        let reused = LockInfo { user: "ann".into(), session: "gone".into(), pid: std::process::id(), host: host_name(), time: t, opened: t };
        std::fs::write(lock_path(&d), serde_json::to_vec(&reused).unwrap()).unwrap();
        assert!(lock_live(&d, t + 10).is_some() || reused.pid == std::process::id());
        assert!(lock_live(&d, t + LOCK_TTL_SECS + 1).is_none(), "TTL frees a lock whose pid was reused");
        // a dead pid on this machine is free at once (kill -9)
        let dead = LockInfo { pid: 4_000_000_000, session: "dead".into(), ..reused.clone() };
        std::fs::write(lock_path(&d), serde_json::to_vec(&dead).unwrap()).unwrap();
        assert_eq!(acquire_lock(&d, "bob", "w3", t + 1), LockState::Mine, "dead process: free immediately");
        // a lock from another computer is believed until its TTL
        let far = LockInfo { host: "other-box".into(), pid: 4_000_000_001, session: "far".into(), time: t, ..reused.clone() };
        std::fs::write(lock_path(&d), serde_json::to_vec(&far).unwrap()).unwrap();
        assert!(matches!(acquire_lock(&d, "bob", "w3", t + 5), LockState::Held(_)));
        assert!(matches!(acquire_lock(&d, "bob", "w3", t + LOCK_TTL_SECS + 5), LockState::Mine));
        // clock skew: a stamp days in the future from a machine that is gone is freed once the file itself is old
        let ahead = LockInfo { time: t + 3 * 86_400, session: "ahead".into(), ..far.clone() };
        std::fs::write(lock_path(&d), serde_json::to_vec(&ahead).unwrap()).unwrap();
        assert!(lock_live(&d, t).is_some(), "a fresh file with a future stamp is believed");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let f = std::fs::File::options().write(true).open(lock_path(&d)).unwrap();
        f.set_modified(old).unwrap();
        assert!(lock_live(&d, t).is_none(), "an old file with a future stamp is stale");
        // a lock written by an older version (no opening time) still reads
        std::fs::write(lock_path(&d), r#"{"user":"u","session":"s","pid":1,"host":"h","time":5}"#).unwrap();
        assert_eq!(read_lock(&d).unwrap().opened, 0);
        // the wording: same account on this computer vs another person or host
        let here = LockInfo { user: "ann".into(), session: "x".into(), pid: 4242, host: host_name(), time: t, opened: 13 * 3600 + 5 * 60 };
        let msg = here.describe("ann");
        assert!(msg.contains("another IYAKKI window on this computer") && msg.contains("process 4242") && msg.contains("13:05"), "{msg}");
        let other = LockInfo { user: "bob".into(), host: "other-box".into(), ..here.clone() };
        assert!(other.describe("ann").starts_with("bob has it open in another IYAKKI session"));
        // an unwritable folder: no panic, the session carries on without a lock file
        let ro = tmp("lock3");
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perm.set_mode(0o555);
        std::fs::set_permissions(&ro, perm).unwrap();
        assert_eq!(acquire_lock(&ro, "ann", "w1", t), LockState::Mine);
        assert!(refresh_lock(&ro, "ann", "w1", t + 30));
        release_lock(&ro, "w1");
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&ro, perm).unwrap();
    }

    #[test]
    fn two_windows_changing_the_workspace_list_do_not_erase_each_other() {
        let d = tmp("reg2");
        let (a, b, c) = (d.join("a"), d.join("b"), d.join("c"));
        for p in [&a, &b, &c] {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut one = Registry::load(&d, "ann");
        let mut two = Registry::load(&d, "ann");
        one.add("Alpha", &a).unwrap();
        two.add("Beta", &b).unwrap();
        one.add("Gamma", &c).unwrap();
        let all = Registry::load(&d, "ann");
        assert_eq!(all.items.len(), 3, "{:?}", all.items.iter().map(|w| &w.name).collect::<Vec<_>>());
    }

    #[test]
    fn paths_stay_inside_the_root_dotdot_and_symlinks() {
        let d = tmp("paths");
        let root = d.join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        std::fs::write(d.join("other/secret"), "s").unwrap();
        std::os::unix::fs::symlink(d.join("other"), root.join("out")).unwrap();
        std::os::unix::fs::symlink(root.join("src"), root.join("inner")).unwrap();
        assert!(resolve_in(&root, "src/a.rs").is_ok());
        assert!(resolve_in(&root, "src/new/file.rs").is_ok());
        assert!(resolve_in(&root, "../other/secret").unwrap_err().contains("outside"));
        assert!(resolve_in(&root, "src/../../other").is_err());
        assert!(resolve_in(&root, "out/secret").is_err(), "symlink escape");
        assert!(resolve_in(&root, "out/new").is_err(), "and creating through it");
        assert!(resolve_in(&root, d.join("other/secret").to_str().unwrap()).is_err(), "absolute path outside");
        assert!(resolve_in(&root, "inner/a.rs").is_ok(), "a link inside stays fine");
        assert!(create_file(&root, "out/x").is_err());
        assert!(!d.join("other/x").exists());
        assert!(delete(&root, "out").is_ok(), "deleting a link removes the link");
        assert!(d.join("other/secret").exists(), "never the target");
    }

    #[test]
    fn tree_hides_heavy_folders_and_search_finds_files_and_text() {
        let d = tmp("tree");
        let root = d.join("p");
        for dir in ["src", "target/debug", "node_modules/x", ".git/objects", "out"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "out/\n*.log\n# c\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"Hello Needle\");\n}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("target/debug/big.rs"), "needle\n").unwrap();
        std::fs::write(root.join("README.md"), "readme\n").unwrap();
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2, b'n', b'e', b'e', b'd', b'l', b'e']).unwrap();
        let names: Vec<String> = list_dir(&root, "", false).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["src", ".gitignore", "bin.dat", "README.md"], "{names:?}");
        let all: Vec<String> = list_dir(&root, "", true).unwrap().into_iter().map(|e| e.name).collect();
        assert!(all.contains(&"target".to_string()) && all.contains(&".git".to_string()) && all.contains(&"out".to_string()));
        assert!(list_dir(&root, "../p2", false).is_err());
        assert_eq!(find_files(&root, "main", 5), vec!["src/main.rs"]);
        assert_eq!(find_files(&root, "slib", 5)[0], "src/lib.rs", "{:?}", find_files(&root, "slib", 5));
        let hits = grep(&root, "needle", 10);
        assert_eq!(hits, vec![Hit { file: "src/main.rs".into(), line: 2, text: "println!(\"Hello Needle\");".into() }], "target/ and binary files are skipped");
        assert!(grep(&root, "  ", 10).is_empty());
        assert_eq!(grep(&root, "fn", 1).len(), 1, "capped");
    }

    #[test]
    fn create_rename_delete_inside_the_root_only() {
        let d = tmp("ops");
        let root = d.join("p");
        std::fs::create_dir_all(&root).unwrap();
        let f = create_file(&root, "src/new/mod.rs").unwrap();
        assert!(f.is_file());
        assert!(create_file(&root, "src/new/mod.rs").is_err());
        assert!(create_file(&root, "../x").is_err());
        create_dir(&root, "docs").unwrap();
        let r = rename(&root, "src/new/mod.rs", "lib.rs").unwrap();
        assert!(r.ends_with("src/new/lib.rs") && r.is_file());
        assert!(rename(&root, "src/new/lib.rs", "../escape").is_err());
        assert!(rename(&root, "docs", "src").is_err(), "exists");
        delete(&root, "src").unwrap();
        assert!(!root.join("src").exists());
        assert!(delete(&root, ".").is_err() && delete(&root, "..").is_err(), "never the root");
        assert!(root.is_dir());
    }
}
