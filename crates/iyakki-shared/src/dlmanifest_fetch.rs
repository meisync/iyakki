//! The download list check as the screen shows it (moved from the agent; the fetching stays there).

/// What the screen shows.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct CheckStatus {
    pub running: bool,
    /// Unix seconds of the last check.
    pub at: Option<u64>,
    /// The last result in plain words ("up to date", "no connection", "refused: signature did not match").
    pub result: String,
    /// After an applied update: "Download list updated: N items changed (names)".
    pub notice: Option<String>,
}

pub fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(now: u64, at: u64) -> String {
    let d = now.saturating_sub(at);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

/// The one line of the "Check for updates" control: "Checked 3 h ago: up to date".
pub fn status_line(s: &CheckStatus, now: u64) -> String {
    if s.running {
        return "Checking for a newer download list...".into();
    }
    match s.at {
        Some(at) => format!("Checked {}: {}", ago(now, at), s.result),
        None => "Not checked yet".into(),
    }
}
