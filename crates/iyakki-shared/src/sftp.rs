//! Types moved out of `iyakki_agent::sftp` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const EXPIRIES: [(&str, u64); 4] = [("1 hour", 3600), ("1 day", 86400), ("7 days", 604800), ("never", 0)];

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde::Deserialize;
use serde::Serialize;

pub const DEFAULT_PORT: u16 = 2222;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    /// Accounts that may log in (empty: nobody).
    pub users: Vec<String>,
    /// Allow the account password as well as keys (off by default).
    pub password_login: bool,
    /// Seconds a start lasts (0: until stopped).
    pub expiry_secs: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config { port: DEFAULT_PORT, users: Vec::new(), password_login: false, expiry_secs: 86400 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct Status {
    pub running: bool,
    pub port: u16,
    pub until: i64,
    pub by: String,
    pub config: Config,
    pub host_fingerprint: String,
    /// The ECDSA P-256 host key some clients ask for (made on first use, like the Ed25519 one).
    pub host_fingerprint_ec: String,
    /// The RSA host key for very old clients (F-FILE-14); empty while it is still being made on first use.
    pub host_fingerprint_rsa: String,
    pub keys: Vec<(String, Vec<String>)>,
    pub addresses: Vec<String>,
}
