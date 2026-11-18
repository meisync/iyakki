//! Types moved out of `iyakki_agent::services` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultState {
    Unlocked,
    Created,
    /// Exists but could not be unlocked (e.g. the password was reset by an admin).
    Locked(String),
    NotApplicable,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// A model download started from the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStatus {
    pub provider: String,
    pub model: String,
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

impl PullStatus {
    pub fn describe(&self) -> String {
        match (&self.error, self.done) {
            (Some(e), _) => format!("{}/{}: failed: {e}", self.provider, self.model),
            (None, true) => format!("{}/{}: downloaded", self.provider, self.model),
            _ if self.total > 0 => format!("{}/{}: {} ({}%, {} of {} MB)", self.provider, self.model, self.status, self.completed * 100 / self.total, self.completed >> 20, self.total >> 20),
            _ => format!("{}/{}: {}", self.provider, self.model, self.status),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionDeleted {
    pub runs: usize,
    pub files: usize,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginReport {
    pub user_vault: VaultState,
    pub system_vault: VaultState,
    /// What happened to the person's protected browser data at sign-in (`profilestore.rs`; `Off` when not protected).
    pub browser_data: crate::profilestore::SignIn,
}
