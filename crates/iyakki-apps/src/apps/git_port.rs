//! The real `GitPort` of the GIT tile (alpha.8 N1): the account and its file and network rules, the audit log, the person's connections (their tokens
//! stay in the vault), the AI IDE's workspaces, the jobs of the Git engine (`GitKit`) and the picture tools. Built on every call from the shell's
//! context; the tile never sees `Services`, the keys or the registry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent_api::GitBatch;
use iyakki_git_model::gitbrowse::Egress;
use iyakki_git_model::gitconn::Conns;
use iyakki_git_model::gitjobs::{Batch, CloneSpec, HostPrompt, Item, ItemState, Op, Phase};
use iyakki_git_model::gitsite::RepoEntry;
use iyakki_shared::gitjobs::CtxFor;
use iyakki_shared::gitkeys::Who;
use iyakki_tile_git::{GitPort, JobSetup};
use iyakki_tile_git::port::GitLineErrors;

use super::AppCx;

/// The shell's context as the tile's port, for one call.
pub struct CxPort<'a>(pub &'a AppCx);

/// A batch of the Git engine as the tile's `Batch`: every call forwards.
pub struct ShellBatch(pub Box<dyn GitBatch>);

impl Batch for ShellBatch {
    fn items(&self) -> Vec<Item> {
        self.0.items()
    }
    fn phase(&self) -> Phase {
        self.0.phase()
    }
    fn prompts(&self) -> Vec<HostPrompt> {
        self.0.prompts()
    }
    fn done(&self) -> usize {
        self.0.done()
    }
    fn failed(&self) -> usize {
        self.0.failed()
    }
    fn pending(&self) -> bool {
        self.0.pending()
    }
    fn running_count(&self) -> usize {
        self.0.running_count()
    }
    fn tick(&mut self) {
        self.0.tick()
    }
    fn trust_host(&mut self, host: &str, port: u16) -> Result<(), String> {
        self.0.trust_host(host, port)
    }
    fn refuse_host(&mut self, host: &str, port: u16) {
        self.0.refuse_host(host, port)
    }
    fn retry(&mut self, item: usize) -> bool {
        self.0.retry(item)
    }
    fn take_changed(&mut self) -> bool {
        self.0.take_changed()
    }
    fn agent_dir(&self) -> Option<PathBuf> {
        self.0.agent_dir()
    }
    fn set_state(&mut self, item: usize, state: ItemState) {
        self.0.set_state(item, state)
    }
}

impl AppCx {
    fn job_ctx(&self, job: JobSetup<'_>) -> Result<iyakki_shared::gitjobs::Ctx, String> {
        let cx = self;
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        cx.git_kit().user_ctx(CtxFor {
            user: &cx.session.user,
            policy: &cx.session.policy,
            audit: Arc::new(move |a, d| audit.record(&user, a, d)),
            root: job.root,
            name: job.name,
            extra_write: job.extra_write,
            who: Who::Person,
            tools: job.tools,
            probe: job.probe,
        })
    }
}

/// The account's git keys (the vault, the host-key store and the session), for the connections and for the tests that look at what was recorded.
pub fn keys_of(cx: &AppCx) -> iyakki_shared::gitkeys::GitKeys {
    let audit = cx.audit.clone();
    let user = cx.session.user.clone();
    cx.services.git_keys(&cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)), iyakki_shared::gitkeys::shared_session())
}

fn boxed(b: Box<dyn GitBatch>) -> Box<dyn Batch> {
    Box::new(ShellBatch(b))
}

impl GitPort for AppCx {
    fn shell(&self) -> &dyn std::any::Any {
        self
    }
    fn user(&self) -> String {
        self.session.user.clone()
    }
    fn data_dir(&self) -> PathBuf {
        self.data_dir().to_path_buf()
    }
    fn default_repos_root(&self) -> (PathBuf, Option<PathBuf>) {
        iyakki_shared::gitworktools::default_repos_root(self.data_dir(), &self.session.user, &self.session.policy)
    }
    fn allows_write(&self, path: &str) -> bool {
        self.session.policy.allows_write(&self.session.user, path)
    }
    fn egress(&self) -> Egress {
        let policy = self.session.policy.clone();
        Arc::new(move |h| if policy.allows_host(h) { Ok(()) } else { Err(format!("Your account may not reach {h} (the network policy of this account).")) })
    }
    fn audit(&self, action: &str, detail: &str) {
        self.audit.record(&self.session.user, action, detail)
    }
    fn conns(&self) -> Conns {
        keys_of(self).conns()
    }
    fn workspace_repo_dirs(&self) -> Vec<PathBuf> {
        let reg = iyakki_shared::workspace::Registry::load(self.data_dir(), &self.session.user);
        let mut v = Vec::new();
        for w in &reg.items {
            let root = w.root_path();
            if let Ok(c) = iyakki_shared::workspace::ProjectConfig::load(&root) {
                for r in c.repos {
                    if r.path != "." && !r.path.contains("..") {
                        v.push(root.join(&r.path));
                    }
                }
            }
            if root.join(".git").exists() {
                v.push(root);
            }
        }
        v
    }
    fn ensure_repos_root(&self, root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string()).and_then(|_| iyakki_shared::workspace::ProjectConfig::ensure(root, "Repositories").map(|_| ()))
    }
    fn workspace_find(&self, canon: &Path) -> Option<String> {
        let reg = iyakki_shared::workspace::Registry::load(self.data_dir(), &self.session.user);
        reg.items.iter().find(|w| Path::new(&w.root) == canon).map(|w| w.id.clone())
    }
    fn workspace_add(&self, name: &str, canon: &Path) -> Result<(String, String), String> {
        let mut reg = iyakki_shared::workspace::Registry::load(self.data_dir(), &self.session.user);
        reg.add(name, canon).map(|w| (w.id, w.root))
    }
    fn plan_clone(&self, job: JobSetup<'_>, specs: &[CloneSpec]) -> Result<Box<dyn Batch>, GitLineErrors> {
        let ctx = self.job_ctx(job).map_err(|e| vec![(0, e)])?;
        self.git_kit().plan_clone(ctx, specs).map(boxed)
    }
    fn plan_update(&self, job: JobSetup<'_>, entries: &[RepoEntry], op: Op) -> Result<Box<dyn Batch>, GitLineErrors> {
        let ctx = self.job_ctx(job).map_err(|e| vec![(0, e)])?;
        self.git_kit().plan_update(ctx, entries, op).map(boxed)
    }
    fn plan_push(&self, job: JobSetup<'_>, remote: &str, folder: &str, branch: &str) -> Result<Box<dyn Batch>, GitLineErrors> {
        let ctx = self.job_ctx(job).map_err(|e| vec![(0, e)])?;
        self.git_kit().plan_push(ctx, remote, folder, branch).map(boxed)
    }
    fn picture_size(&self, bytes: &[u8]) -> Option<(u32, u32)> {
        iyakki_tile_lab::pic::header_size(bytes)
    }
    fn picture_thumb(&self, bytes: &[u8], max: u32) -> Result<Arc<tiny_skia::Pixmap>, String> {
        super::browser_images::to_pixmap(bytes).map(|pm| iyakki_tile_lab::pic::thumb(&pm, max))
    }
}

impl GitPort for CxPort<'_> {
    fn shell(&self) -> &dyn std::any::Any {
        GitPort::shell(self.0)
    }
    fn user(&self) -> String {
        GitPort::user(self.0)
    }
    fn data_dir(&self) -> PathBuf {
        GitPort::data_dir(self.0)
    }
    fn default_repos_root(&self) -> (PathBuf, Option<PathBuf>) {
        GitPort::default_repos_root(self.0)
    }
    fn allows_write(&self, path: &str) -> bool {
        GitPort::allows_write(self.0, path)
    }
    fn egress(&self) -> Egress {
        GitPort::egress(self.0)
    }
    fn audit(&self, action: &str, detail: &str) {
        GitPort::audit(self.0, action, detail)
    }
    fn conns(&self) -> Conns {
        GitPort::conns(self.0)
    }
    fn workspace_repo_dirs(&self) -> Vec<PathBuf> {
        GitPort::workspace_repo_dirs(self.0)
    }
    fn ensure_repos_root(&self, root: &Path) -> Result<(), String> {
        GitPort::ensure_repos_root(self.0, root)
    }
    fn workspace_find(&self, canon: &Path) -> Option<String> {
        GitPort::workspace_find(self.0, canon)
    }
    fn workspace_add(&self, name: &str, canon: &Path) -> Result<(String, String), String> {
        GitPort::workspace_add(self.0, name, canon)
    }
    fn plan_clone(&self, job: JobSetup<'_>, specs: &[CloneSpec]) -> Result<Box<dyn Batch>, GitLineErrors> {
        GitPort::plan_clone(self.0, job, specs)
    }
    fn plan_update(&self, job: JobSetup<'_>, entries: &[RepoEntry], op: Op) -> Result<Box<dyn Batch>, GitLineErrors> {
        GitPort::plan_update(self.0, job, entries, op)
    }
    fn plan_push(&self, job: JobSetup<'_>, remote: &str, folder: &str, branch: &str) -> Result<Box<dyn Batch>, GitLineErrors> {
        GitPort::plan_push(self.0, job, remote, folder, branch)
    }
    fn picture_size(&self, bytes: &[u8]) -> Option<(u32, u32)> {
        GitPort::picture_size(self.0, bytes)
    }
    fn picture_thumb(&self, bytes: &[u8], max: u32) -> Result<Arc<tiny_skia::Pixmap>, String> {
        GitPort::picture_thumb(self.0, bytes, max)
    }
}

/// What the tile uses that the shell's real run replaces: the host's API, the git program, the clock.
pub fn system_rig() -> iyakki_tile_git::Rig {
    let run: Arc<dyn iyakki_k8s_model::wsexec::Runner> = crate::agent().exec().system();
    iyakki_tile_git::Rig { http: crate::agent().helpers_b6().git_http(), run, timer: Arc::new(iyakki_k8s_model::k8sretry::SystemTimer::new()), scratch: std::env::temp_dir(), repos_root: None, tools: None, probe: None, run_jobs: true }
}
