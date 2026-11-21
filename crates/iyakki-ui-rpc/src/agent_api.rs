//! The agent facade (`iyakki_agent_api::AgentApi`, the process-wide `iyakki_apps::agent()`) as the UI program sees it over the core's socket
//! (F-SPLIT-03, D-SPLIT-03-QUIET): the three traits the core serves as roots (`AgentDisplay`, `AgentFiles`, `AgentStore`) are remote objects; every other
//! trait of the facade answers with its own default, which is "this core does not serve this call" or a quiet value. The handle says it is real only
//! while the core answers a call on the files root (KI-1010: a handle that is not real is never taken for the machine).

use std::sync::Arc;

use iyakki_agent_api::{
    AgentChat, AgentCluster, AgentDisplay, AgentFacets, AgentFacetsB1, AgentFacetsB2, AgentFacetsB3, AgentFacetsB4, AgentFacetsB6, AgentFacetsB7, AgentFacetsB8, AgentFacetsS6B, AgentFiles, AgentNetwork, AgentOther, AgentSettings, AgentSound,
    AgentStore, AgentVault,
};
use serde_json::json;

use crate::{RemoteAny, Rpc};

/// `AgentApi` over one UI socket connection.
pub struct RemoteAgentApi {
    rpc: Rpc,
    display: Arc<dyn AgentDisplay>,
    files: Arc<dyn AgentFiles>,
    store: Arc<dyn AgentStore>,
}

impl RemoteAgentApi {
    pub fn new(rpc: Rpc) -> RemoteAgentApi {
        let display = match rpc.root("AgentDisplay") {
            Some(RemoteAny::AgentDisplay(r)) => r,
            _ => unreachable!("AgentDisplay is generated"),
        };
        let files = match rpc.root("AgentFiles") {
            Some(RemoteAny::AgentFiles(r)) => r,
            _ => unreachable!("AgentFiles is generated"),
        };
        let store = match rpc.root("AgentStore") {
            Some(RemoteAny::AgentStore(r)) => r,
            _ => unreachable!("AgentStore is generated"),
        };
        RemoteAgentApi { rpc, display, files, store }
    }

    /// True while the core answers a call on the files root (one cheap, pure call).
    pub fn answers(&self) -> bool {
        self.rpc.call::<bool>("AgentFiles", 0, "media_is_mp3", json!(["a.mp3"])).is_ok()
    }
}

impl AgentFacets for RemoteAgentApi {
    fn is_real(&self) -> bool {
        self.answers()
    }
}

// the traits of the facade the core does not serve: their own defaults answer
impl AgentChat for RemoteAgentApi {}
impl AgentCluster for RemoteAgentApi {}
impl AgentNetwork for RemoteAgentApi {}
impl AgentOther for RemoteAgentApi {}
impl AgentSettings for RemoteAgentApi {}
impl AgentSound for RemoteAgentApi {}
impl AgentVault for RemoteAgentApi {}
impl AgentFacetsB1 for RemoteAgentApi {}
impl AgentFacetsB2 for RemoteAgentApi {}
impl AgentFacetsB3 for RemoteAgentApi {}
impl AgentFacetsB4 for RemoteAgentApi {}
impl AgentFacetsB6 for RemoteAgentApi {}
impl AgentFacetsB7 for RemoteAgentApi {}
impl AgentFacetsB8 for RemoteAgentApi {}
impl AgentFacetsS6B for RemoteAgentApi {}

impl AgentDisplay for RemoteAgentApi {
    fn cameras_release_screen_at(&self, data: &std::path::Path, shown: &[String]) {
        self.display.cameras_release_screen_at(data, shown)
    }
    fn camrec_active_ids(&self, data: &std::path::Path) -> Vec<String> {
        self.display.camrec_active_ids(data)
    }
    fn camrec_active_names(&self, data: &std::path::Path) -> Vec<String> {
        self.display.camrec_active_names(data)
    }
    fn camrec_stop_all_for(&self, data: &std::path::Path, user: &str) -> usize {
        self.display.camrec_stop_all_for(data, user)
    }
}

impl AgentFiles for RemoteAgentApi {
    fn disks_account_owner(&self, user: &str, passwd: &str) -> Option<(u32, u32)> {
        self.files.disks_account_owner(user, passwd)
    }
    fn disks_media_dir(&self, home: &str) -> String {
        self.files.disks_media_dir(home)
    }
    fn disks_set_signout_pref(&self, home: &str, on: bool) -> Result<(), String> {
        self.files.disks_set_signout_pref(home, on)
    }
    fn disks_signout_pref(&self, home: &str) -> bool {
        self.files.disks_signout_pref(home)
    }
    fn gitconn_host_of(&self, url: &str) -> String {
        self.files.gitconn_host_of(url)
    }
    fn media_is_mp3(&self, path: &str) -> bool {
        self.files.media_is_mp3(path)
    }
    fn pkg_newer(&self, new: &str, old: &str) -> bool {
        self.files.pkg_newer(new, old)
    }
    fn sharedfolder_ask_share(&self, folder: &str) {
        self.files.sharedfolder_ask_share(folder)
    }
    fn sharedfolder_on_logout(&self, user: &str) {
        self.files.sharedfolder_on_logout(user)
    }
    fn sharedfolder_random_hex(&self, bytes: usize) -> String {
        self.files.sharedfolder_random_hex(bytes)
    }
    fn sharedfolder_running_total(&self) -> usize {
        self.files.sharedfolder_running_total()
    }
    fn sharedfolder_stop_everything(&self) -> usize {
        self.files.sharedfolder_stop_everything()
    }
    fn sharedfolder_take_share_ask(&self) -> Option<String> {
        self.files.sharedfolder_take_share_ask()
    }
    fn wsexec_git_available(&self) -> bool {
        self.files.wsexec_git_available()
    }
    fn wsexec_on_path(&self, name: &str) -> bool {
        self.files.wsexec_on_path(name)
    }
}

impl AgentStore for RemoteAgentApi {
    fn linuxapps_ide_context(&self) -> Option<(std::path::PathBuf, String)> {
        self.store.linuxapps_ide_context()
    }
    fn linuxapps_ide_wrap(&self, cwd: &std::path::Path, argv: Vec<String>) -> Result<Vec<String>, String> {
        self.store.linuxapps_ide_wrap(cwd, argv)
    }
    fn linuxapps_set_ide_context(&self, data: &std::path::Path, user: &str) {
        self.store.linuxapps_set_ide_context(data, user)
    }
    fn linuxapps_valid_name(&self, n: &str) -> bool {
        self.store.linuxapps_valid_name(n)
    }
    fn linuxapps_valid_package(&self, p: &str) -> bool {
        self.store.linuxapps_valid_package(p)
    }
    fn linuxapps_workspace_root_of(&self, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
        self.store.linuxapps_workspace_root_of(cwd)
    }
}
