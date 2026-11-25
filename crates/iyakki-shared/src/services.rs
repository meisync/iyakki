//! Types moved out of `iyakki_agent::services` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VaultState {
    Unlocked,
    Created,
    /// Exists but could not be unlocked (e.g. the password was reset by an admin).
    Locked(String),
    NotApplicable,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


pub use iyakki_provider_kind::PullStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionDeleted {
    pub runs: usize,
    pub files: usize,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoginReport {
    pub user_vault: VaultState,
    pub system_vault: VaultState,
    /// What happened to the person's protected browser data at sign-in (`profilestore.rs`; `Off` when not protected).
    pub browser_data: crate::profilestore::SignIn,
}
