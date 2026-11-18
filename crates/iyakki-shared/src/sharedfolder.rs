//! Types moved out of `iyakki_agent::sharedfolder` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

pub const EXPIRIES: [(&str, u64); 4] = [("1 hour", 3600), ("1 day", 86400), ("7 days", 604800), ("never", 0)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A browsable list of files (downloads, optional uploads).
    #[default]
    Files,
    /// A static site: `index.html` is served, no list, no uploads.
    Website,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// A secret address `/s/<128-bit token>/` that can be changed to revoke it.
    #[default]
    Token,
    /// One shared password (HTTP Basic; weak on plain HTTP).
    Password,
    /// Anyone on the local network.
    Open,
    /// This machine only (127.0.0.1).
    Local,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::Token => "secret link",
            Access::Password => "password",
            Access::Open => "anyone on the local network",
            Access::Local => "this machine only",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareCfg {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub mode: Mode,
    pub access: Access,
    pub upload: bool,
    pub mkdir: bool,
    pub overwrite: bool,
    pub show_hidden: bool,
    pub max_upload_mb: u64,
    /// `all` or one local address.
    pub bind: String,
    pub port: u16,
    /// The secret of the link (token mode). Kept in the share list, which only its owner can read.
    pub token: String,
    /// Argon2id hash of the password (password mode); the password itself is never stored.
    pub pass_hash: String,
    /// How long a start lasts (0 = never ends).
    pub expiry_secs: u64,
    /// When the running share ends (0 = never); set at each start.
    pub expires_at: i64,
    /// Start again when the owner signs in.
    pub keep_running: bool,
    pub owner: String,
    /// A "Receive a package" share: its folder is `<data>/local/packages`, it takes only the add-on package and its checksum (see [`package_name_ok`]).
    pub package_drop: bool,
}

/// What the form sends.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub name: String,
    pub folder: String,
    pub mode: Mode,
    pub access: Access,
    pub upload: bool,
    pub mkdir: bool,
    pub overwrite: bool,
    pub show_hidden: bool,
    pub max_upload_mb: u64,
    pub bind: String,
    /// 0 = choose one.
    pub port: u16,
    /// Password mode: the new password (empty keeps the old one when editing).
    pub password: String,
    pub expiry_secs: u64,
    pub keep_running: bool,
    /// The person ticked the card that says everyone on the network can open it.
    pub confirm_open: bool,
}

#[derive(Clone, Debug)]
pub struct ShareInfo {
    pub cfg: ShareCfg,
    pub running: bool,
    pub urls: Vec<String>,
    pub clients: usize,
    pub requests: u64,
    pub error: Option<String>,
    /// Seconds left while running with an expiry.
    pub left: Option<i64>,
}

/// The form values to start from for an existing share (Edit).
pub fn draft_of(c: &ShareCfg) -> Draft {
    Draft { name: c.name.clone(), folder: c.folder.clone(), mode: c.mode, access: c.access, upload: c.upload, mkdir: c.mkdir, overwrite: c.overwrite, show_hidden: c.show_hidden, max_upload_mb: c.max_upload_mb, bind: c.bind.clone(), port: c.port, password: String::new(), expiry_secs: c.expiry_secs, keep_running: c.keep_running, confirm_open: c.access == Access::Open }
}
