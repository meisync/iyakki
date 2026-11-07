//! The sharing policy an administrator sets: who may START sharing, per kind (wired and USB, Wi-Fi hotspot, Bluetooth), and limits.
//! Stored in `<data>/share/policy.json` (changed only by an administrator, audited with before and after). It is enforced at every
//! entry point: `iyakki share`, the Settings page, and the service itself, which stops a running session within seconds when the
//! policy no longer allows it. Default: only administrators may start; everybody may see the status.

use super::{LinkKind, ShareError};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    Admins,
    Users,
    Nobody,
}

impl Who {
    pub fn word(self) -> &'static str {
        match self {
            Who::Admins => "admins",
            Who::Users => "users",
            Who::Nobody => "nobody",
        }
    }
    pub fn parse(s: &str) -> Option<Who> {
        match s {
            "admins" => Some(Who::Admins),
            "users" => Some(Who::Users),
            "nobody" => Some(Who::Nobody),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyKind {
    Wired,
    Wifi,
    Bluetooth,
}

pub fn policy_kind(k: LinkKind) -> PolicyKind {
    match k {
        LinkKind::Wifi => PolicyKind::Wifi,
        LinkKind::BluetoothNap | LinkKind::BluetoothPanu => PolicyKind::Bluetooth,
        _ => PolicyKind::Wired,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub wired: Who,
    pub wifi: Who,
    pub bluetooth: Who,
    /// Most devices at once (0: the built-in limit).
    pub max_clients: u8,
    /// Hard maximum for one session in hours (0: none); the idle switch-off still applies.
    pub max_hours: u8,
    /// Interface names that may be shared through (empty: any).
    pub allowed_downstreams: Vec<String>,
    /// Interface names that may be the internet source (empty: any).
    pub allowed_uplinks: Vec<String>,
    pub changed_by: String,
    pub changed_at: u64,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { wired: Who::Admins, wifi: Who::Admins, bluetooth: Who::Admins, max_clients: 0, max_hours: 0, allowed_downstreams: Vec::new(), allowed_uplinks: Vec::new(), changed_by: String::new(), changed_at: 0 }
    }
}

pub const BLOCKED_LINE: &str = "Internet sharing is turned off by your administrator.";

impl Policy {
    pub fn who(&self, k: PolicyKind) -> Who {
        match k {
            PolicyKind::Wired => self.wired,
            PolicyKind::Wifi => self.wifi,
            PolicyKind::Bluetooth => self.bluetooth,
        }
    }

    /// May a person with this role start sharing through this kind?
    pub fn may_start(&self, k: LinkKind, is_admin: bool) -> Result<(), ShareError> {
        match self.who(policy_kind(k)) {
            Who::Nobody => Err(ShareError::Blocked(BLOCKED_LINE.into())),
            Who::Admins if !is_admin => Err(ShareError::Blocked("Internet sharing can only be started by an administrator.".into())),
            _ => Ok(()),
        }
    }

    /// Is there any kind this person may start? (the Start button is shown only then)
    pub fn may_start_any(&self, is_admin: bool) -> bool {
        [LinkKind::Ethernet, LinkKind::Wifi, LinkKind::BluetoothNap].iter().any(|k| self.may_start(*k, is_admin).is_ok())
    }

    pub fn check_names(&self, down: &str, up: &str) -> Result<(), ShareError> {
        if !self.allowed_downstreams.is_empty() && !self.allowed_downstreams.iter().any(|n| n == down) {
            return Err(ShareError::Blocked(format!("Sharing through {down} is not allowed by your administrator.")));
        }
        if !self.allowed_uplinks.is_empty() && !self.allowed_uplinks.iter().any(|n| n == up) {
            return Err(ShareError::Blocked(format!("Sharing the internet of {up} is not allowed by your administrator.")));
        }
        Ok(())
    }

    /// `wired admins, Wi-Fi nobody, ...` for the audit line and the page.
    pub fn summary(&self) -> String {
        format!("wired {}, wifi {}, bluetooth {}, max clients {}, max hours {}, downstreams [{}], uplinks [{}]", self.wired.word(), self.wifi.word(), self.bluetooth.word(), self.max_clients, self.max_hours, self.allowed_downstreams.join(","), self.allowed_uplinks.join(","))
    }

    /// The line the page shows: who changed the policy and when.
    pub fn changed_line(&self) -> String {
        if self.changed_by.is_empty() {
            "Policy: the default (only administrators may start sharing).".into()
        } else {
            format!("Policy last changed by {} (at {}).", self.changed_by, self.changed_at)
        }
    }
}

pub fn read(data: &Path) -> Policy {
    std::fs::read_to_string(super::control::dir(data).join("policy.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Store a new policy. Only an administrator may call this (`is_admin`); the returned line is the audit entry with before and after.
pub fn write(data: &Path, new: &Policy, by: &str, is_admin: bool, now: u64) -> Result<String, String> {
    if !is_admin {
        return Err("Only administrators can change the sharing policy.".into());
    }
    let before = read(data);
    let mut p = new.clone();
    p.changed_by = by.into();
    p.changed_at = now;
    let d = super::control::dir(data);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let tmp = d.join("policy.tmp");
    std::fs::write(&tmp, serde_json::to_string(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, d.join("policy.json")).map_err(|e| e.to_string())?;
    let line = format!("share policy changed by {by}: before [{}] after [{}]", before.summary(), p.summary());
    {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(d.join("audit.log")) {
            let _ = writeln!(f, "{now} {line}");
        }
    }
    Ok(line)
}
