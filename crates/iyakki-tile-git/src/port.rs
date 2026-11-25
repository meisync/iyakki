//! What the GIT tile asks of the shell (alpha.8 N1): the account (its name, its data folder, what it may write and reach, the audit log), the
//! person's connections, the workspaces of the AI IDE, the jobs of a clone, fetch, pull or push, and pictures out of a clone. The tile holds no
//! `Services`, no keys, no workspace registry and no agent: every call into the shell goes through [`GitPort`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_git_model::gitconn::Conns;
use iyakki_git_model::gitjobs::{Batch, CloneSpec, Op, Probe, Tools};
use iyakki_git_model::gitsite::RepoEntry;
use iyakki_uikit::tilekit::Pixmap;

/// The reasons a batch could not be planned: (line, sentence) pairs.
pub type GitLineErrors = Vec<(usize, String)>;

/// Where and with what a batch of jobs runs (the keys, the account and the write check are the port's own business).
pub struct JobSetup<'a> {
    /// The folder the jobs work in and its name.
    pub root: &'a Path,
    pub name: &'a str,
    /// A folder the write check must also allow (the account's own Repositories folder when it has no home to write in).
    pub extra_write: Option<&'a Path>,
    /// Tests: the programs the jobs use instead of the ones found on the system.
    pub tools: Option<Tools>,
    /// Tests: what answers "which host key does this server present".
    pub probe: Option<Probe>,
}

/// What a screen of the tile answers to an event: stay, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

/// The shell's side of the tile for one call.
pub trait GitPort {
    /// The shell's own context, for the shell's own code to read back (the tile never looks inside).
    fn shell(&self) -> &dyn std::any::Any;
    fn user(&self) -> String;
    /// The data folder of the program (where the tile keeps its small preference file).
    fn data_dir(&self) -> PathBuf;
    /// `/home/<user>/Repositories`, or the account's folder under the data folder when it has no home to write in; the second value is the
    /// folder the jobs' write check must also allow.
    fn default_repos_root(&self) -> (PathBuf, Option<PathBuf>);
    /// The file rules of the account allow writing here.
    fn allows_write(&self, path: &str) -> bool;
    /// The network rules of the account as the browser asks them (a host it may reach, or the sentence that says why not).
    fn egress(&self) -> iyakki_git_model::gitbrowse::Egress;
    /// A line in the audit log under the account (action and detail, never a value).
    fn audit(&self, action: &str, detail: &str);
    /// The person's connections to git hosts: their tokens come from the vault, never through the tile.
    fn conns(&self) -> Conns;
    /// The folders of the repositories the AI IDE's workspaces list (cloned there too), as clones on this computer.
    fn workspace_repo_dirs(&self) -> Vec<PathBuf>;
    /// Make the Repositories folder and its `project.json`.
    fn ensure_repos_root(&self, root: &Path) -> Result<(), String>;
    /// The id of the AI IDE workspace whose root is this folder, if there is one.
    fn workspace_find(&self, canon: &Path) -> Option<String>;
    /// Take the folder as a workspace of the AI IDE: its id and root.
    fn workspace_add(&self, name: &str, canon: &Path) -> Result<(String, String), String>;
    fn plan_clone(&self, job: JobSetup<'_>, specs: &[CloneSpec]) -> Result<Box<dyn Batch>, GitLineErrors>;
    fn plan_update(&self, job: JobSetup<'_>, entries: &[RepoEntry], op: Op) -> Result<Box<dyn Batch>, GitLineErrors>;
    fn plan_push(&self, job: JobSetup<'_>, remote: &str, folder: &str, branch: &str) -> Result<Box<dyn Batch>, GitLineErrors>;
    /// The pixel size a picture file says it has (before it is decoded).
    fn picture_size(&self, bytes: &[u8]) -> Option<(u32, u32)>;
    /// A picture file decoded and made at most `max` pixels on its longest side.
    fn picture_thumb(&self, bytes: &[u8], max: u32) -> Result<Arc<Pixmap>, String>;
}
