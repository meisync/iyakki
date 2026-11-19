//! The plain data of developer mode (pairings, keys, grants, state), moved out of `iyakki_agent::devmode` for F-SPLIT-03; the agent re-exports it.

use serde::{Deserialize, Serialize};
use crate::devmode::scopes::Scope;
pub use crate::devmode::stamp;

pub const DEFAULT_PORT: u16 = 7979;

/// What a freshly paired computer gets.
pub fn default_scopes() -> Vec<String> {
    vec![Scope::Install.id().to_string()]
}


#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    /// Who switched it on, and when.
    pub by: String,
    pub since: i64,
    /// When it switches itself off (0: never).
    pub until: i64,
    /// 0 means [`DEFAULT_PORT`].
    #[serde(default)]
    pub port: u16,
}

impl State {
    pub fn port(&self) -> u16 {
        if self.port == 0 { DEFAULT_PORT } else { self.port }
    }
}

/// A developer computer allowed to act on this device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DevKey {
    pub name: String,
    /// ed25519 public key, hex.
    pub key: String,
    /// The account of this device whose role it acts with.
    pub account: String,
    pub added: i64,
    pub added_by: String,
    #[serde(default)]
    pub last_used: i64,
    /// What this computer may do (see [`scopes`]); keys paired before scopes existed hold only `install`.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
}

impl DevKey {
    /// `ab12 cd34 …`: the first 16 hex characters, for lists.
    pub fn short(&self) -> String {
        self.key.chars().take(16).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
    }
}

/// What the firewall is asked for (see `iyakki_fw::devrules::Want`, which the caller fills from this).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FwWant {
    pub on: bool,
    pub until: i64,
    pub port: u16,
    pub peers: Vec<String>,
    pub pair_until: i64,
    pub network_wide: bool,
}

/// An open pairing: the code shown on the screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub expires: i64,
    /// The account the new key will act as (the administrator who opened the pairing).
    pub account: String,
    pub tries: u32,
    /// The administrator who opened it.
    #[serde(default)]
    pub by: Option<String>,
    /// The scopes the computer that pairs will get (shown on the pairing card; the administrator can change them while it is open).
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
}

/// One line of the activity log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub time: i64,
    /// The key's name, the administrator, or "?" for a refused stranger.
    pub who: String,
    pub account: String,
    pub action: String,
    pub detail: String,
    pub ok: bool,
}

impl Activity {
    pub fn line(&self) -> String {
        format!("{} {} {}{} {} {}", stamp(self.time), if self.ok { "ok " } else { "NO " }, self.who, if self.account.is_empty() { String::new() } else { format!(" ({})", self.account) }, self.action, self.detail)
    }
}

impl DevKey {
    /// Does this computer hold the scope (`shell` includes `power`)?
    pub fn has(&self, s: Scope) -> bool {
        self.scopes.iter().any(|x| x == s.id()) || (s == Scope::Power && self.scopes.iter().any(|x| x == Scope::Shell.id()))
    }

    pub fn scope_list(&self) -> Vec<Scope> {
        Scope::ALL.into_iter().filter(|s| self.scopes.iter().any(|x| x == s.id())).collect()
    }

    /// `install, debug, files`
    pub fn scope_text(&self) -> String {
        self.scope_list().iter().map(|s| s.id()).collect::<Vec<_>>().join(", ")
    }
}
pub mod grants {
    use super::*;

    fn shell_kind() -> String {
        "shell".into()
    }


    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
    pub struct ShellGrant {
        #[serde(default = "shell_kind")]
        pub kind: String,
        pub computer: String,
        pub account: String,
        pub as_root: bool,
        /// For one session: ends when the first session of this grant closes.
        pub once: bool,
        pub expires: i64,
        pub by: String,
        pub since: i64,
    }

    /// A computer whose shell requests the administrator chose to approve automatically for a while.
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
    pub struct AutoShell {
        pub computer: String,
        pub until: i64,
        pub by: String,
    }

}

pub mod trusted {
    use super::*;

    /// What the switch covers.
    #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
    #[serde(rename_all = "lowercase")]
    pub enum TrustLevel {
        /// Every approval card (shell, patch, restart/switch-off).
        #[default]
        Everything,
        /// The patch approval only ("Patches without asking").
        Patches,
    }

    impl TrustLevel {
        pub fn covers(self, kind: &str) -> bool {
            match self {
                TrustLevel::Everything => matches!(kind, "shell" | "patch" | "power"),
                TrustLevel::Patches => kind == "patch",
            }
        }
    }

    /// How long it lasts.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TrustFor {
        /// Until the administrator switches it off (and developer mode stays on).
        Kept,
        /// 1 to 24 hours from now (the page offers 4 and 12).
        Hours(u32),
    }

    /// A switch as shown.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TrustedView {
        pub computer: String,
        pub by: String,
        pub level: TrustLevel,
        /// 0: until switched off by hand.
        pub until: i64,
    }

    impl TrustedView {
        pub fn kept(&self) -> bool {
            self.until == 0
        }
    }

}

pub mod restartgrant {
    

    /// A grant as shown: with the moment it ends (0: when developer mode is switched off).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RestartView {
        pub computer: String,
        pub by: String,
        pub until: i64,
        pub awaiting_start: bool,
        /// Kept until switched off by hand (`until` is 0 then).
        pub persist: bool,
    }

}

pub mod sessions {
    

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct SessionInfo {
        pub id: u64,
        pub computer: String,
        pub account: String,
        /// The scope it runs under (`shell`, `forward`, `debug`...).
        pub scope: &'static str,
        /// `shell`, `forward 6443`, `dmesg --follow`...
        pub what: String,
        pub started: i64,
    }

}

pub mod pairings {
    

    /// "3 minutes ago", "5 days ago", "never".
    pub fn ago(now: i64, t: i64) -> String {
        if t <= 0 {
            return "never".into();
        }
        let s = (now - t).max(0);
        let (n, unit) = match s {
            0..=59 => return "just now".into(),
            60..=3599 => (s / 60, "minute"),
            3600..=86399 => (s / 3600, "hour"),
            _ => (s / 86400, "day"),
        };
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    }


    use super::sessions::SessionInfo;

    /// A pairing as the person sees it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Pairing {
        pub name: String,
        pub account: String,
        pub key_short: String,
        pub added: i64,
        pub last_used: i64,
        pub scopes: Vec<String>,
        pub api_scopes: Vec<String>,
        pub core_scopes: Vec<String>,
        /// Until when its shell requests are approved without asking (a person's grant), if they are.
        pub shell_auto_until: Option<i64>,
        pub restart_grant: bool,
        /// The sessions it has open now (shell windows, forwarded ports, followed logs).
        pub sessions: Vec<SessionInfo>,
    }

    impl Pairing {
        /// How many of its sessions are shell windows.
        pub fn shell_windows(&self) -> usize {
            self.sessions.iter().filter(|s| s.scope == "shell").count()
        }

        /// The list of what removing it takes away (scopes, grants, open windows), without a sentence around it.
        pub fn takes_away(&self) -> String {
            let mut parts = vec![format!("its access scopes ({})", if self.scopes.is_empty() { "none".to_string() } else { self.scopes.join(", ") })];
            if !self.api_scopes.is_empty() {
                parts.push(format!("its web service scopes ({})", self.api_scopes.join(", ")));
            }
            if !self.core_scopes.is_empty() {
                parts.push(format!("its core contract scopes ({})", self.core_scopes.join(", ")));
            }
            if self.shell_auto_until.is_some() {
                parts.push("the shell approval you gave it".to_string());
            }
            if self.restart_grant {
                parts.push("its permission to restart without asking".to_string());
            }
            let shells = self.shell_windows();
            let others = self.sessions.len() - shells;
            parts.push(match (shells, others) {
                (0, 0) => "no open session (none is open now)".to_string(),
                (s, 0) => format!("its {s} open shell window{} (closed at once)", if s == 1 { "" } else { "s" }),
                (0, o) => format!("its {o} open session{} (ended at once)", if o == 1 { "" } else { "s" }),
                (s, o) => format!("its {s} open shell window{} and {o} other session{} (closed at once)", if s == 1 { "" } else { "s" }, if o == 1 { "" } else { "s" }),
            });
            parts.join(", ")
        }

        /// The question asked before removing: what goes and what stays.
        pub fn removal_text(&self) -> String {
            format!("Removing {} takes away {}. Its next request is refused; to use it again it must pair again with a new code. Apps it installed stay.", self.name, self.takes_away())
        }
    }

}
