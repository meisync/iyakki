//! Types moved out of `iyakki_agent::tzdetect` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What Settings says about it (the label is explicit about what is sent).
pub const EXPLAIN: &str = "Detect automatically asks ipapi.co (then worldtimeapi.org) which time zone this network's public IP address is in. Your IP address is visible to that service; nothing else is sent. Off unless you switch it on.";

/// How often an automatic zone is looked up again.
pub const RECHECK_SECS: i64 = 6 * 3600;

/// After a failure (no network yet) it is tried again sooner.
pub const RETRY_SECS: i64 = 10 * 60;

/// Is a lookup due (automatic on, and long enough since the last one; sooner after a failure)?
pub fn due(t: &crate::settings::TimeSettings, now: i64) -> bool {
    if !t.zone_auto {
        return false;
    }
    let failed = t.zone_note.starts_with("Could not");
    now - t.zone_checked_at >= if failed { RETRY_SECS } else { RECHECK_SECS }
}

/// Put a lookup's result into the settings: the zone when it worked (and still automatic), the note either way. Returns the new
/// zone when it changed.
pub fn apply(t: &mut crate::settings::TimeSettings, result: &Result<(String, &'static str), String>, now: i64) -> Option<String> {
    t.zone_checked_at = now;
    match result {
        Ok((z, src)) => {
            t.zone_note = format!("{src}: {z}");
            if t.zone_auto && t.zone != *z {
                t.zone = z.clone();
                return Some(z.clone());
            }
            None
        }
        Err(e) => {
            t.zone_note = e.clone();
            None
        }
    }
}
