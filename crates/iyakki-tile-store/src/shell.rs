//! What the STORE tile asks of the shell's coding-agent sign-in (alpha.10 AP3, D-AP3-01): the state of a sign-in and the requests to start
//! or end one. The real answers come from `crate::agent_login` through `StorePort` in `crates/iyakki-apps/src/apps/store_port.rs`.

/// The sign-in state of one coding agent on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginState {
    NotInstalled,
    /// The status has not been looked up yet (a lookup was started).
    Checking,
    SignedOut,
    SignedIn(String),
    /// Installed, and the program has no way to say (or an API key from the vault is used).
    Unknown(String),
}

/// The host facts the Setup tab reads: memory, the product name, the sysfs root and the uptime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub mem_total_kb: u64,
    pub product_name: String,
    pub sys_root: std::path::PathBuf,
    pub uptime_secs: u64,
}

/// The work the Setup tab hands to its threads (they must be `Send`, so they take this handle rather than the screen's port).
pub trait ShellJobs: Send + Sync {
    /// The lines of the browser package card, read from the pinned store (a network read: run on a thread).
    fn browser_package_card(&self, data: &std::path::Path) -> Result<Vec<String>, String>;
    /// Remove the voice pack; `delete` removes one model (the Setup tab's seam or the Lemonade call).
    fn voicepack_remove(&self, delete: &dyn Fn(&str) -> Result<(), String>) -> Result<String, String>;
}
