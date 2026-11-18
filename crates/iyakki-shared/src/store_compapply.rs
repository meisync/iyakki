//! The system component update, the data part: the review the person is shown, the line of the last check, the result of the last install and the
//! plain sentences for them. The plan, the download and the apply stay in `iyakki_agent::store::compapply`.

use serde::{Deserialize, Serialize};

/// What the person is shown before anything is downloaded or written, and what an approval is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub text: String,
    /// Identifies exactly this plan (store, channel, release, every part with its version and hashes).
    pub digest: String,
    /// Any part needs the machine to restart (kernel, boot, base).
    pub restart_machine: bool,
    /// Any part is stored for the next start of IYAKKI's own programs (core, kernel layer).
    pub restart_core: bool,
    pub parts: usize,
    pub download: u64,
}

/// One line per store for Settings: how many parts are newer than what is installed (from the last list fetched).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreLine {
    pub store: String,
    pub newer: usize,
    pub checked_at: i64,
    pub error: String,
}

/// The sentence Settings shows for one store line.
pub fn check_sentence(l: &StoreLine) -> String {
    if !l.error.is_empty() {
        format!("Components from {}: could not check ({}).", l.store, l.error)
    } else if l.newer == 0 {
        format!("Components from {}: up to date.", l.store)
    } else {
        format!("Components from {}: {} newer.", l.store, l.newer)
    }
}

/// How an apply ended, in the three words the person needs to tell a refusal from a success.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Everything was written and read back; a restart is needed to run it.
    Applied,
    /// Refused before anything was written (no approval, not enough room, a part must go first, an update is pending, ...).
    Refused,
    /// Something went wrong after work had started and the earlier version was put back, or nothing was changed.
    Failed,
    /// A check after writing failed and the previous version was put back (or could not be, which the text says).
    RolledBack,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Applied => "Installed",
            Outcome::Refused => "Refused, nothing was changed",
            Outcome::Failed => "Failed",
            Outcome::RolledBack => "Failed and rolled back",
        }
    }
}

/// The last component install, as the page and the inbox show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyResult {
    pub at: i64,
    pub outcome: Outcome,
    pub store: String,
    pub channel: String,
    /// The full plain-words text (what was installed, or the reason).
    pub text: String,
}

/// How long ago, in words.
pub fn ago_text(secs: i64) -> String {
    match secs.max(0) {
        0..=89 => "just now".to_string(),
        90..=5399 => format!("{} minutes ago", secs / 60),
        5400..=129_599 => format!("{} hours ago", (secs + 1800) / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// The sentence the page shows first: what happened and when.
pub fn result_headline(r: &ApplyResult, now: i64) -> String {
    format!("Last system component install ({}): {}.", ago_text(now - r.at), r.outcome.label())
}

pub const CHANNELS: [&str; 3] = ["stable", "beta", "nightly"];

/// The next channel in the order stable, beta, nightly (the Settings button).
pub fn next_channel(cur: &str) -> &'static str {
    CHANNELS[(CHANNELS.iter().position(|c| *c == cur).map_or(0, |i| i + 1)) % CHANNELS.len()]
}

pub fn channel_sentence(ch: &str) -> String {
    let what = match ch {
        "stable" => "tested builds only",
        "beta" => "builds that are being tried before they become stable",
        "nightly" => "the newest builds, not yet tried widely",
        _ => "a channel you chose",
    };
    format!("Component channel on this computer: {ch} ({what}).")
}


pub fn human(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}
