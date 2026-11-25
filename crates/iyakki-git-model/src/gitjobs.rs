//! Types moved out of `iyakki_agent::gitjobs` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::{PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GitCheck {
    Ready { path: PathBuf, version: String },
    Missing { message: String },
}

/// `@gitea-iyakki-gitea/team/app` (or `gitea:gitea-iyakki-gitea/team/app`): a repository of a git service of the platform services
/// (`docs/PLATFORM-SERVICES.md`) named by its instance, resolved to its clone address.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstanceRef {
    pub instance: String,
    /// The clone address (`https://host/team/app.git`; the service's own web address with the path added).
    pub url: String,
    pub host: String,
    /// The vault entry holding IYAKKI's token for that service, when it has one.
    pub token_vault: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloneSpec {
    pub url: String,
    pub branch: String,
    pub depth: u32,
    /// The folder name the person chose; empty: from the address.
    pub folder: String,
    /// Also clone the repository's submodules (`--recurse-submodules`; `--shallow-submodules` when `depth` is set). Off unless the
    /// person asked: a submodule is another address the clone reaches, so it is only fetched over the transports of
    /// [`submodule_protocols`].
    pub submodules: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Clone,
    Fetch,
    Pull,
    /// Send the branch of the repository to its `origin` (the workspace's own repository, after the review).
    Push,
}

impl Op {
    pub fn verb(self) -> &'static str {
        match self {
            Op::Clone => "clone",
            Op::Fetch => "fetch",
            Op::Pull => "pull",
            Op::Push => "push",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Queued,
    Running,
    Done,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Probing,
    Confirm,
    Running,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoStatus {
    pub branch: String,
    pub detached: bool,
    pub upstream: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

impl RepoStatus {
    pub fn dirty(&self) -> bool {
        self.changed + self.untracked + self.conflicted > 0
    }
    /// `main ↑1 ↓2 ✎3`.
    pub fn summary(&self) -> String {
        let mut s = if self.detached { "detached".to_string() } else if self.branch.is_empty() { "no commits".to_string() } else { self.branch.clone() };
        if self.ahead > 0 {
            s.push_str(&format!(" ↑{}", self.ahead));
        }
        if self.behind > 0 {
            s.push_str(&format!(" ↓{}", self.behind));
        }
        if self.changed + self.conflicted > 0 {
            s.push_str(&format!(" ✎{}", self.changed + self.conflicted));
        }
        if self.untracked > 0 {
            s.push_str(&format!(" +{}", self.untracked));
        }
        s
    }
}

pub fn missing_message() -> String {
    if std::path::Path::new("/etc/iyakki-release").exists() {
        "git is not installed on this IYAKKI image. The web and Mac images include it; the libre image leaves out GPL programs that are not needed to boot, and no in-system install route exists for git yet. Use an image with git, or work with the files without cloning.".to_string()
    } else {
        "git is not installed on this computer. Install it with your system's package manager (Debian/Ubuntu `sudo apt install git`, Fedora `sudo dnf install git`, Arch `sudo pacman -S git`, macOS `xcode-select --install`, Windows https://git-scm.com); IYAKKI never runs sudo for you.".to_string()
    }
}

/// Reads `git status --porcelain=v2 --branch`.
pub fn parse_status(out: &str) -> RepoStatus {
    let mut s = RepoStatus::default();
    for l in out.lines() {
        if let Some(r) = l.strip_prefix("# branch.head ") {
            if r == "(detached)" {
                s.detached = true;
            } else {
                s.branch = r.to_string();
            }
        } else if let Some(r) = l.strip_prefix("# branch.upstream ") {
            s.upstream = r.to_string();
        } else if let Some(r) = l.strip_prefix("# branch.ab ") {
            let mut it = r.split_whitespace();
            s.ahead = it.next().and_then(|x| x.trim_start_matches('+').parse().ok()).unwrap_or(0);
            s.behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if l.starts_with("1 ") || l.starts_with("2 ") {
            s.changed += 1;
        } else if l.starts_with("u ") {
            s.conflicted += 1;
        } else if l.starts_with("? ") {
            s.untracked += 1;
        }
    }
    s
}

// ---- the data of a batch of git jobs (moved from iyakki_agent::gitjobs; the engine stays there)

use std::sync::Arc;

use crate::gitsite::RepoUrl;
use crate::hostkey::HostKey;
use crate::keys::Auth;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Tools {
    pub git: PathBuf,
    /// Needed only for SSH addresses; the reason it is missing is kept for the message.
    pub ssh: Result<PathBuf, String>,
    /// Tests may clone from local paths.
    pub allow_local: bool,
}



pub type Probe = Arc<dyn Fn(&str, u16) -> Result<Vec<HostKey>, String> + Send + Sync>;


/// A host whose key the person has to look at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPrompt {
    pub host: String,
    pub port: u16,
    pub keys: Vec<HostKey>,
}



#[derive(Clone, Debug)]
pub struct Item {
    pub op: Op,
    pub spec: CloneSpec,
    pub url: RepoUrl,
    /// The folder inside the workspace.
    pub folder: String,
    pub auth: Auth,
    pub state: ItemState,
    pub job: Option<String>,
    /// The last line git printed (credentials removed).
    pub last: String,
    pub progress: Option<f32>,
    pub tries: u32,
}

impl Item {
    pub fn title(&self) -> String {
        format!("git {} {}", self.op.verb(), self.url.short())
    }
}

/// A batch of git jobs the tile started (a clone, a fetch, a pull or a push), as the tile drives it: the same methods as the agent API's
/// `GitBatch`, so the shell wraps one in the other. The tile looks at the items, ticks it, and answers its host-key questions.
pub trait Batch: Send {
    fn items(&self) -> Vec<Item>;
    fn phase(&self) -> Phase;
    /// Hosts whose key the person has to look at.
    fn prompts(&self) -> Vec<HostPrompt>;
    fn done(&self) -> usize;
    fn failed(&self) -> usize;
    fn pending(&self) -> bool;
    fn running_count(&self) -> usize;
    /// Does the work that is due (probe answers, queued items, finished jobs, project.json for cloned repositories).
    fn tick(&mut self);
    /// The person compared the fingerprint and trusts the host.
    fn trust_host(&mut self, host: &str, port: u16) -> Result<(), String>;
    fn refuse_host(&mut self, host: &str, port: u16);
    /// Makes a failed item run again.
    fn retry(&mut self, item: usize) -> bool;
    /// True once after entries were written into project.json.
    fn take_changed(&mut self) -> bool;
    /// The key service's folder (tests and the audit).
    fn agent_dir(&self) -> Option<std::path::PathBuf>;
    /// Tests: put an item into a state.
    fn set_state(&mut self, item: usize, state: ItemState);
}
