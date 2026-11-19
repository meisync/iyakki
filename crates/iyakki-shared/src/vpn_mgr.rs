//! Types moved out of `iyakki_agent::vpn_mgr` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Stopped,
    /// The interface is up, no handshake yet (or the last one is stale).
    Connecting,
    Connected { since: u64 },
}

use iyakki_vpn::profile::{Mode, Profile};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Status {
    pub state: State,
    pub error: Option<String>,
    pub attempts: u32,
    pub retry_at: Option<u64>,
    pub needs_you: bool,
    pub warnings: Vec<String>,
    pub rx: u64,
    pub tx: u64,
    pub last_handshake: Option<i64>,
    /// The guard table is in place right now.
    pub guard: bool,
}

impl Status {
    pub fn line(&self, enabled: bool, now: u64) -> String {
        match self.state {
            State::Connected { since } => format!("connected, {} ago", ago(now.saturating_sub(since))),
            State::Connecting => "connecting…".into(),
            State::Stopped if !enabled => "disabled".into(),
            State::Stopped => match (&self.error, self.retry_at) {
                (Some(_), Some(at)) => format!("retrying in {} s", at.saturating_sub(now)),
                (Some(_), None) => "stopped: needs you".into(),
                (None, _) => "starting…".into(),
            },
        }
    }
}

/// How the top bar's lock and the Devices line show the VPN. `None` of it when no profile is enabled (nothing to show).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Light {
    /// Connected and the handshake is fresh.
    Secure,
    /// Coming up or reconnecting.
    Working,
    /// Enabled but not connected, and the guard is blocking what it should (nothing leaks).
    Blocked,
    /// Enabled but not connected and no guard: traffic goes direct.
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub light: Light,
    /// One short line, no key and no address of the server: "VPN connected: Office (FULL)".
    pub line: String,
}

/// [`Manager::guard_line`] over rows (pure).
pub fn guard_line_of(rows: &[Row]) -> Option<String> {
    let names: Vec<(&str, bool)> = rows
        .iter()
        .filter(|r| r.enabled && r.status.guard)
        .filter_map(|r| match r.profile.settings.mode {
            Mode::Full if r.profile.settings.kill_switch => Some((r.profile.name.as_str(), true)),
            Mode::Hybrid if r.profile.settings.fail_closed => Some((r.profile.name.as_str(), false)),
            _ => None,
        })
        .collect();
    let (first, full) = *names.first()?;
    let more = if names.len() > 1 { format!(" and {} more", names.len() - 1) } else { String::new() };
    Some(if full {
        format!("VPN kill switch is active ({first}{more}): traffic outside the VPN is blocked while it is down. Your local network keeps working. Change it in Settings, Network, VPN.")
    } else {
        format!("VPN guard is active ({first}{more}): traffic that must go through the VPN is blocked while it is down. Change it in Settings, Network, VPN.")
    })
}

pub fn ago(s: u64) -> String {
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub profile: Profile,
    pub enabled: bool,
    pub status: Status,
}

/// What an import found, for the person.
#[derive(Clone, Debug)]
pub struct Imported {
    pub profile: Profile,
    pub warnings: Vec<String>,
}

/// The words that describe a profile in lists and cards: kind, server, what it routes, mode and options. No keys.
pub fn describe(p: &Profile) -> String {
    let routes = if p.full_tunnel() { "all traffic" } else { "only the networks the profile lists" };
    let mode = match p.settings.mode {
        Mode::Full => format!("FULL (everything through the VPN{})", if p.settings.kill_switch { ", kill switch on" } else { "" }),
        Mode::Hybrid => format!("HYBRID ({} filter rule{}{})", p.settings.filters.len(), if p.settings.filters.len() == 1 { "" } else { "s" }, if p.settings.fail_closed { ", fail closed" } else { "" }),
    };
    format!("{} · server {} · profile routes {routes} · mode {mode}", p.kind().as_str(), p.endpoint_label().unwrap_or_else(|| "(none)".into()))
}
