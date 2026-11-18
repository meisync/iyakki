//! Types moved out of `iyakki_agent::profilepolicy` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.


use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// At most this many sites in either list.
pub const MAX_SITES: usize = 200;

/// What one beat of the timer decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Beat {
    /// Not protected, not open, switched off, not yet due, or one is running.
    Nothing,
    /// Open and due, but nothing changed since the last write-back.
    Unchanged,
    /// A checkpoint was written.
    Written,
    Failed(String),
}

/// Minutes between checkpoints when the person has not chosen (design D6).
pub const DEFAULT_CHECKPOINT_MINUTES: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// 0 switches the timed checkpoint off.
    pub checkpoint_minutes: u32,
    /// Seal and remove the decrypted copy when the screen locks; open it again at unlock.
    pub lock_seal: bool,
    /// Leave every cookie out of the sealed file at sign-out.
    pub clear_cookies_on_exit: bool,
    /// Leave the site data (local storage, IndexedDB) of these sites out of the sealed file at sign-out.
    pub clear_sites_on_exit: Vec<String>,
    /// These sites' site data is never written into the sealed file, not even at a checkpoint.
    pub never_persist: Vec<String>,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { checkpoint_minutes: DEFAULT_CHECKPOINT_MINUTES, lock_seal: false, clear_cookies_on_exit: false, clear_sites_on_exit: vec![], never_persist: vec![] }
    }
}

/// A host name as typed by a person: letters, digits, dots and dashes, no scheme, no path, lower case.
pub fn clean_site(s: &str) -> Result<String, String> {
    let t = s.trim().to_ascii_lowercase();
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(&t).trim_end_matches('/').to_string();
    let ok = !t.is_empty() && t.len() <= 253 && t.contains('.') && !t.starts_with(['.', '-']) && !t.ends_with(['.', '-']) && !t.contains("..") && t.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    if ok { Ok(t) } else { Err(format!("\"{}\" is not a site name like example.com", s.trim().chars().take(60).collect::<String>())) }
}

impl Policy {
    pub fn path(data: &Path, user: &str) -> PathBuf {
        data.join("web-profile").join(format!("{}.policy.json", crate::webengine::safe_name(user)))
    }

    /// The saved choices, or the defaults (also for a file that cannot be read: nothing is lost by that, the defaults clear nothing).
    pub fn load(data: &Path, user: &str) -> Policy {
        std::fs::read_to_string(Policy::path(data, user)).ok().and_then(|t| serde_json::from_str::<Policy>(&t).ok()).map(Policy::sanitized).unwrap_or_default()
    }

    fn sanitized(mut self) -> Policy {
        self.checkpoint_minutes = self.checkpoint_minutes.min(24 * 60);
        for l in [&mut self.clear_sites_on_exit, &mut self.never_persist] {
            let mut v: Vec<String> = l.iter().filter_map(|s| clean_site(s).ok()).collect();
            v.sort();
            v.dedup();
            v.truncate(MAX_SITES);
            *l = v;
        }
        self
    }

    pub fn save(&self, data: &Path, user: &str) -> Result<(), String> {
        let p = self.clone().sanitized();
        let t = serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&Policy::path(data, user), t.as_bytes(), 0o600)
    }

    /// Text for the Settings page and the chat tool: one line per choice.
    pub fn summary(&self) -> String {
        let cp = if self.checkpoint_minutes == 0 { "off".to_string() } else { format!("every {} min while the data changes", self.checkpoint_minutes) };
        let list = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
        format!("Timed checkpoint: {cp}\nSeal when the screen locks: {}\nCookies left out at sign-out: {}\nSite data left out at sign-out: {}\nNever kept: {}", if self.lock_seal { "yes" } else { "no" }, if self.clear_cookies_on_exit { "yes" } else { "no" }, list(&self.clear_sites_on_exit), list(&self.never_persist))
    }
}
