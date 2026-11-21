//! Types moved out of `iyakki_agent::sshtunnel_mgr` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum State {
    #[default]
    Stopped,
    Connecting,
    /// Since this Unix time.
    Connected { since: u64 },
}

use crate::ssh::HostKey;
use crate::sshtunnel::{self as tun, Fault, Tunnel};

#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Status {
    pub state: State,
    /// The last problem in plain words; kept until the next success.
    pub error: Option<String>,
    pub fault: Option<Fault>,
    pub attempts: u32,
    /// The Unix time of the next automatic try.
    pub retry_at: Option<u64>,
    /// Waits for the person (a hard fault).
    pub needs_you: bool,
}

impl Status {
    /// One line for the list.
    pub fn line(&self, enabled: bool, now: u64) -> String {
        match self.state {
            State::Connected { since } => format!("connected, {} ago", tun::ago(now.saturating_sub(since))),
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub tunnel: Tunnel,
    pub status: Status,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TestReport {
    pub ok: bool,
    pub lines: Vec<String>,
    /// Hosts to trust before it can work, with the keys they present.
    pub untrusted: Vec<(String, u16, Vec<HostKey>)>,
}

