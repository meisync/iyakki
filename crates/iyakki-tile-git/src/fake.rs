//! A fake `GitPort` for the tile's own tests: an account with a temp data folder, an in-memory vault for the connections, an audit list, a
//! registry of IDE workspaces in memory, and batches that do nothing but remember what they were asked. No host, no git program, no keys, no thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use iyakki_git_model::gitbrowse::Egress;
use iyakki_git_model::gitconn::Conns;
use iyakki_git_model::gitjobs::{Batch, CloneSpec, HostPrompt, Item, ItemState, Op, Phase};
use iyakki_git_model::gitsite::RepoEntry;
use iyakki_git_model::keys::SecretStore;
use iyakki_uikit::tilekit::Pixmap;

use crate::port::{GitLineErrors, GitPort, JobSetup};

/// The person's vault, in memory.
#[derive(Default)]
pub struct MemVault(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemVault {
    fn get(&self, name: &str) -> Result<iyakki_git_model::keys::Zeroizing<String>, String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned().map(iyakki_git_model::keys::Zeroizing::new).ok_or_else(|| "no such secret".to_string())
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        Ok(())
    }
    fn names(&self) -> Result<Vec<String>, String> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect())
    }
}

/// What a fake batch saw and answers.
#[derive(Default)]
pub struct BatchLog {
    pub ticks: usize,
    pub pending_for: usize,
    pub failed: usize,
    pub error: String,
    pub prompts: Vec<HostPrompt>,
}

pub struct FakeBatch {
    pub log: Arc<Mutex<BatchLog>>,
    pub items: Vec<Item>,
}

impl Batch for FakeBatch {
    fn items(&self) -> Vec<Item> {
        let l = self.log.lock().unwrap();
        let mut v = self.items.clone();
        if !l.error.is_empty() {
            if let Some(i) = v.first_mut() {
                i.state = ItemState::Failed(l.error.clone());
            }
        }
        v
    }
    fn phase(&self) -> Phase {
        if self.log.lock().unwrap().prompts.is_empty() { Phase::Running } else { Phase::Confirm }
    }
    fn prompts(&self) -> Vec<HostPrompt> {
        self.log.lock().unwrap().prompts.clone()
    }
    fn done(&self) -> usize {
        usize::from(!self.pending())
    }
    fn failed(&self) -> usize {
        self.log.lock().unwrap().failed
    }
    fn pending(&self) -> bool {
        self.log.lock().unwrap().pending_for > 0
    }
    fn running_count(&self) -> usize {
        usize::from(self.pending())
    }
    fn tick(&mut self) {
        let mut l = self.log.lock().unwrap();
        l.ticks += 1;
        l.pending_for = l.pending_for.saturating_sub(1);
    }
    fn trust_host(&mut self, _host: &str, _port: u16) -> Result<(), String> {
        Ok(())
    }
    fn refuse_host(&mut self, _host: &str, _port: u16) {}
    fn retry(&mut self, _item: usize) -> bool {
        false
    }
    fn take_changed(&mut self) -> bool {
        false
    }
    fn agent_dir(&self) -> Option<PathBuf> {
        None
    }
    fn set_state(&mut self, item: usize, state: ItemState) {
        if let Some(i) = self.items.get_mut(item) {
            i.state = state;
        }
    }
}

#[derive(Default)]
pub struct State {
    pub audit: Vec<(String, String)>,
    pub workspaces: Vec<(String, String, String)>,
    /// (what, root, name, folder or remote) of every plan asked for.
    pub planned: Vec<String>,
    pub ensured: Vec<PathBuf>,
    pub write_ok: bool,
    pub plan_error: Option<String>,
    /// How many ticks the batches of the next plans stay pending.
    pub pending_for: usize,
    pub batches: Vec<Arc<Mutex<BatchLog>>>,
}

pub struct FakePort {
    pub dir: PathBuf,
    pub root: PathBuf,
    pub vault: Arc<MemVault>,
    pub st: Mutex<State>,
}

static N: AtomicUsize = AtomicUsize::new(0);

impl FakePort {
    pub fn new() -> FakePort {
        let dir = std::env::temp_dir().join(format!("iyakki-tile-git-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        FakePort { root: dir.join("Repositories"), dir, vault: Arc::new(MemVault::default()), st: Mutex::new(State { write_ok: true, pending_for: 1, ..Default::default() }) }
    }
    pub fn st(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn plan(&self, what: &str, job: &JobSetup<'_>, detail: &str, items: usize) -> Result<Box<dyn Batch>, GitLineErrors> {
        let mut s = self.st();
        s.planned.push(format!("{what} {} {} {detail}", job.root.display(), job.name));
        if let Some(e) = &s.plan_error {
            return Err(vec![(0, e.clone())]);
        }
        let log = Arc::new(Mutex::new(BatchLog { pending_for: s.pending_for, ..Default::default() }));
        s.batches.push(log.clone());
        let item = |i: usize| Item {
            op: Op::Fetch,
            spec: CloneSpec::default(),
            url: iyakki_git_model::gitsite::parse(&format!("https://git.example.org/me/app{i}.git"), Default::default()).unwrap(),
            folder: format!("app{i}"),
            auth: iyakki_git_model::keys::Auth::None,
            state: ItemState::Queued,
            job: None,
            last: String::new(),
            progress: None,
            tries: 0,
        };
        Ok(Box::new(FakeBatch { log, items: (0..items).map(item).collect() }))
    }
}

impl GitPort for FakePort {
    fn shell(&self) -> &dyn std::any::Any {
        &()
    }
    fn user(&self) -> String {
        "root".into()
    }
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
    fn default_repos_root(&self) -> (PathBuf, Option<PathBuf>) {
        (self.root.clone(), None)
    }
    fn allows_write(&self, _path: &str) -> bool {
        self.st().write_ok
    }
    fn egress(&self) -> Egress {
        Arc::new(|_| Ok(()))
    }
    fn audit(&self, action: &str, detail: &str) {
        self.st().audit.push((action.into(), detail.into()));
    }
    fn conns(&self) -> Conns {
        Conns::new(&self.dir, self.vault.clone(), Arc::new(|_, _| {}))
    }
    fn workspace_repo_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn ensure_repos_root(&self, root: &Path) -> Result<(), String> {
        self.st().ensured.push(root.to_path_buf());
        Ok(())
    }
    fn workspace_find(&self, canon: &Path) -> Option<String> {
        self.st().workspaces.iter().find(|w| Path::new(&w.2) == canon).map(|w| w.0.clone())
    }
    fn workspace_add(&self, name: &str, canon: &Path) -> Result<(String, String), String> {
        let mut s = self.st();
        let id = format!("ws{}", s.workspaces.len() + 1);
        s.workspaces.push((id.clone(), name.into(), canon.display().to_string()));
        Ok((id, canon.display().to_string()))
    }
    fn plan_clone(&self, job: JobSetup<'_>, specs: &[CloneSpec]) -> Result<Box<dyn Batch>, GitLineErrors> {
        self.plan("clone", &job, &specs.iter().map(|s| s.url.clone()).collect::<Vec<_>>().join(","), specs.len())
    }
    fn plan_update(&self, job: JobSetup<'_>, entries: &[RepoEntry], op: Op) -> Result<Box<dyn Batch>, GitLineErrors> {
        self.plan(op.verb(), &job, &entries.iter().map(|e| e.url.clone()).collect::<Vec<_>>().join(","), entries.len())
    }
    fn plan_push(&self, job: JobSetup<'_>, remote: &str, folder: &str, branch: &str) -> Result<Box<dyn Batch>, GitLineErrors> {
        self.plan("push", &job, &format!("{remote} {folder} {branch}"), 1)
    }
    fn picture_size(&self, _bytes: &[u8]) -> Option<(u32, u32)> {
        None
    }
    fn picture_thumb(&self, _bytes: &[u8], _max: u32) -> Result<Arc<Pixmap>, String> {
        Err("pictures are not in the fake".into())
    }
}
