//! Types moved out of `iyakki_agent::profilestore` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The vault secret that holds the profile key.
pub const VAULT_SECRET: &str = "browser-profile";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not protected: plain files (the default).
    Off,
    /// Protected, closed (nobody signed in, or the browser data was sealed).
    Sealed,
    /// Protected, decrypted copy in RAM while the person is signed in.
    Open,
    /// A migration stopped half way: the sealed file and plain files both exist.
    NeedsFinish,
}

impl Status {
    /// The short text shown in Settings and by `iyakki dev state` (never a key, path or name).
    pub fn text(self) -> &'static str {
        match self {
            Status::Off => "off: browser data is plain files",
            Status::Sealed => "protected: sealed",
            Status::Open => "protected: open while signed in",
            Status::NeedsFinish => "protection was interrupted; press Protect again",
        }
    }
}

/// What happened at sign-in (for one audit line and one toast).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SignIn {
    /// Protection is off for this person: nothing was done.
    Off,
    Opened { from_backup: bool },
    /// Protected but not opened (the key is not available, or the file is damaged): the reason, without paths or keys.
    Locked(String),
}
