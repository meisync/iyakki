//! The git jobs of the AI IDE and the GIT tile: the pure data (operations, phases, specs, the status parse) is the leaf crate `iyakki-git-model`'s
//! `gitjobs`, re-exported here under the old path (alpha.8 N1); this file keeps what needs the keys, the host-key store and the workspace.

pub use iyakki_git_model::gitjobs::*;
use std::path::PathBuf;

// ---- the data of a batch of git jobs (moved from iyakki_agent::gitjobs; the engine stays there)

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::gitkeys::{GitKeys, Who};

/// Everything a batch needs from the IDE.
#[derive(Clone)]
pub struct Ctx {
    pub keys: Arc<GitKeys>,
    pub tools: Tools,
    pub owner: String,
    pub workspace: String,
    pub root: PathBuf,
    /// This workspace's own key choices (host -> key id).
    pub ws_keys: BTreeMap<String, String>,
    pub who: Who,
    /// Where the per-batch agent folder is made (on the data volume, private).
    pub run_dir: PathBuf,
    pub probe: Probe,
    pub scope: Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>,
}

/// What [`user_ctx`] needs to know besides the services.
pub struct CtxFor<'a> {
    pub user: &'a str,
    pub policy: &'a iyakki_sec::policy::Policy,
    pub audit: crate::gitkeys::AuditFn,
    /// The folder the jobs work in and its name.
    pub root: &'a Path,
    pub name: &'a str,
    /// A folder the write check must also allow (the account's own Repositories folder when it has no home to write in).
    pub extra_write: Option<&'a Path>,
    pub who: Who,
    /// Tests give their own programs and key probe.
    pub tools: Option<Tools>,
    pub probe: Option<Probe>,
}



impl Ctx {
    /// Folder names already used by the repositories of the workspace's project.json (lower case).
    pub fn keys_taken(&self) -> HashSet<String> {
        crate::workspace::ProjectConfig::load(&self.root).map(|c| c.repos.iter().map(|r| r.path.to_ascii_lowercase()).collect()).unwrap_or_default()
    }
}

