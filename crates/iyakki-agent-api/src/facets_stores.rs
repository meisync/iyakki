//! The person's stores as the UI reads them (F-SPLIT-03, Services read facets, piece 6): sessions, skills, runs, MCP servers, skill packages and background jobs.
//! The real ones stay in the agent; the UI holds `Arc<dyn ...Api>` through the `AppCx` accessors.

use std::path::Path;
use std::time::Duration;

use std::sync::Arc;
use iyakki_proxy::Streamer;
use iyakki_tile_api::radio::{CertProblem, RadioState, Station, BANDS};
use iyakki_shared::jobs::*;
use iyakki_shared::mcp::*;
use iyakki_shared::pkg::*;
use iyakki_shared::runs::*;
use iyakki_shared::sessions::*;
use iyakki_shared::skills::*;

pub trait SessionApi: Send + Sync {
    fn create(&self, user: &str) -> Result<Session, String>;
    fn list(&self, user: &str) -> Vec<SessionMeta>;
    fn load(&self, user: &str, id: &str) -> Option<Session>;
    fn save(&self, s: &mut Session) -> Result<(), String>;
    fn remember_turn(&self, s: &Session, question: &str, answer: &str);
}

pub trait SkillApi: Send + Sync {
    fn add(&self, scope: Scope, markdown: &str, can_shared: bool, can_own: bool) -> Result<Skill, SkillError>;
    fn get(&self, scope: &Scope, name: &str) -> Option<Skill>;
    fn list(&self, user: &str) -> Vec<Skill>;
    fn list_scope(&self, scope: &Scope) -> Vec<Skill>;
    fn remove(&self, scope: &Scope, name: &str, can_shared: bool, can_own: bool) -> Result<(), SkillError>;
}

pub trait RunApi: Send + Sync {
    fn delete(&self, user: &str, id: &str) -> Result<(), String>;
    fn list(&self, user: &str) -> Vec<RunMeta>;
    fn live(&self) -> Vec<LiveRun>;
    fn load(&self, user: &str, id: &str) -> Result<(RunLog, Integrity), String>;
    fn save(&self, log: &RunLog) -> Result<(), String>;
    fn stop_all_live(&self, user: Option<&str>) -> usize;
    fn stop_live(&self, id: &str) -> bool;
}

pub trait McpApi: Send + Sync {
    fn add(&self, s: StoredServer) -> Result<(), String>;
    fn list(&self) -> Vec<StoredServer>;
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// `McpApi` over the `mcp()` member of a services handle (the root object the core serves over the UI socket, F-SPLIT-03).
pub struct McpVia(pub Arc<dyn crate::ServicesHandle>);
impl McpApi for McpVia {
    fn add(&self, s: StoredServer) -> Result<(), String> {
        self.0.mcp().add(s)
    }
    fn list(&self) -> Vec<StoredServer> {
        self.0.mcp().list()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.mcp().remove(name)
    }
}

pub trait PackageApi: Send + Sync {
    fn install(&self, review: &Review, approved: bool, can_shared: bool, can_own: bool) -> Result<Skill, PkgError>;
    fn list(&self, scope: &Scope) -> Vec<(String, Installed)>;
    fn remove(&self, scope: &Scope, name: &str, can_shared: bool, can_own: bool) -> Result<(), PkgError>;
    fn review(&self, text: &str, scope: &Scope) -> Result<Review, PkgError>;
    fn trusted_keys(&self) -> Vec<(String, String)>;
    fn trust_key(&self, key_hex: &str, label: &str) -> Result<(), PkgError>;
}

pub trait JobApi: Send + Sync {
    fn dismiss(&self, owner: &str, id: &str) -> Result<(), String>;
    fn get(&self, owner: &str, id: &str) -> Result<Job, String>;
    fn list(&self, owner: &str) -> Vec<Job>;
    fn log_len(&self, owner: &str, id: &str) -> u64;
    fn tail(&self, owner: &str, id: &str, lines: usize) -> Result<String, String>;
    fn log_text(&self, owner: &str, id: &str, max: usize) -> Result<String, String>;
    fn running_count(&self, owner: &str) -> usize;
    fn start(&self, owner: &str, spec: &Spec, scope: &dyn Fn(&Path) -> Result<(), String>) -> Result<Job, String>;
    fn stop(&self, owner: &str, id: &str, grace: Duration) -> Result<Job, String>;
    fn take_finished(&self, owner: &str, workspace: &str, who: Channel) -> Vec<Job>;
    fn take_finished_all(&self, owner: &str, who: Channel) -> Vec<Job>;
    fn visible(&self, owner: &str, workspace: &str) -> Vec<Job>;
    fn visible_all(&self, owner: &str) -> Vec<Job>;
    fn wait(&self, owner: &str, id: &str, timeout: Duration) -> Result<Job, String>;
}


pub trait RadioApi: Send + Sync {
    fn cert_problem(&self) -> Option<CertProblem>;
    fn is_active(&self) -> bool;
    fn is_paused(&self) -> bool;
    fn levels(&self) -> [f32; BANDS];
    fn pause(&self) -> bool;
    fn play(&self, station: &Station, allow: Arc<dyn Fn(&str) -> bool + Send + Sync>, play_cmd: &str);
    /// [`RadioApi::play`] with the person's current list, for moving on to the next station when a live one drops (F-RADIO-17).
    fn play_in(&self, station: &Station, allow: Arc<dyn Fn(&str) -> bool + Send + Sync>, play_cmd: &str, around: iyakki_tile_api::radio::Around);
    /// What the status line adds while the radio recovers ("Reconnecting 1 of 2", "Switched to NAME").
    fn notice(&self) -> Option<String>;
    fn resume(&self) -> bool;
    fn set_retry_wait(&self, d: Duration);
    fn set_streamer(&self, s: Arc<dyn Streamer>);
    fn set_volume(&self, v: u8);
    fn state(&self) -> RadioState;
    fn stop(&self);
    fn volume(&self) -> u8;
}


/// What the platform panels need from the person's services (the cluster they may start, the jobs they run and watch, the data folder, and the opaque handle the
/// platform facet takes back): built by `AppCx::platform_handles`, so the panel names no `Services`.
#[derive(Clone)]
pub struct PlatformHandles {
    pub dir: std::path::PathBuf,
    pub k3s: Arc<dyn crate::DevCluster>,
    pub jobs: Arc<dyn JobApi>,
    /// The services as the platform facet's `private_*` calls take them.
    pub any: Arc<dyn std::any::Any + Send + Sync>,
    pub start_registry: Arc<dyn Fn(&str, &iyakki_shared::quickinstall::RegistryArgs, bool, &iyakki_shared::quickinstall::Launch) -> Result<Vec<Job>, String> + Send + Sync>,
}
