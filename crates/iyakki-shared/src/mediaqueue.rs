//! Types moved out of `iyakki_agent::mediaqueue` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Downloading,
    Succeeded,
    Failed,
    Cancelled,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Downloading => "Downloading",
            Status::Succeeded => "Succeeded",
            Status::Failed => "Failed",
            Status::Cancelled => "Cancelled",
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::Cancelled)
    }
}

use crate::mediaindex::ItemKind;

/// What the list shows for one entry.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub id: u64,
    pub url: String,
    pub status: Status,
    pub kind: Option<ItemKind>,
    /// The folder as far as it is known: the chosen one, the automatic choice, or the one the file went to.
    pub folder: String,
    /// The folder was chosen by the program (by the kind of file), not by hand.
    pub auto: bool,
    pub file: String,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub total: u64,
    /// Bytes per second since it started (0 when not known).
    pub speed: u64,
    pub eta_secs: Option<u64>,
    pub error: String,
    pub note: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// What a restart makes of a download that was not finished.
pub const INTERRUPTED: &str = "Interrupted: IYAKKI was restarted before it finished. Retry it.";

/// What a person asks for: an address and the free-text words that go with it.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub url: String,
    /// Free-text categories (any words); known ones become types, the others tags.
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub rating: u8,
    /// A folder chosen by hand; `None` = automatic, by the kind of file.
    pub folder: Option<String>,
}

/// The folder name to show for a path (`/home/me/Videos` → `Videos`).
pub fn folder_label(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

/// Who is asking for a download and where an automatic one may go (what the queue's worker needs from the person's session).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Who {
    pub policy: iyakki_sec::policy::Policy,
    pub user: String,
    /// The folders an automatic download may go to (the person's library folders and Pictures; only writable ones are used).
    pub folders: Vec<String>,
}
