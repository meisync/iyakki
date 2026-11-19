use crate::doctor::Facts;
use crate::doctor::NetProbe;
use std::sync::Arc;

/// How many audit entries are looked at for the exposure line.
pub const AUDIT_LOOKBACK: usize = 400;

/// The sentence that goes under a command-line report: what it could not look at.
pub fn not_checked_note() -> &'static str {
    "Not looked at from the command line (only the running system knows them): the dev cluster, the vault, the VPN and services that failed to start. Open the DOCTOR tile to check those."
}

/// The newest audit entries first, as display strings (`<time> <user> <event> <detail>`), asked for when the probe runs.
pub type AuditRecent = Arc<dyn Fn(usize) -> Vec<String> + Send + Sync>;

/// What the slow reads need (owned: it moves to a thread).
pub struct SlowIn {
    /// (provider id, `host:port`) of every enabled model server on this computer or the local network.
    pub servers: Vec<(String, String)>,
    pub audit_recent: AuditRecent,
    /// Developer mode's port when it is on (asked on this computer only).
    pub dev_port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlowOut {
    pub models_unreachable: Vec<String>,
    pub exposure: Option<String>,
    pub net: Option<NetProbe>,
    pub dev_port_answers: Option<bool>,
}

impl SlowOut {
    /// Put the slow answers into the facts they belong to.
    pub fn apply(self, f: &mut Facts) {
        f.models_unreachable = self.models_unreachable;
        f.exposure_unexpected = self.exposure;
        f.net = self.net;
        f.dev_port_answers = self.dev_port_answers;
    }
}
