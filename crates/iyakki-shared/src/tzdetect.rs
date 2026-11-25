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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TimeSettings;

    fn auto(checked_at: i64, note: &str) -> TimeSettings {
        TimeSettings { zone_auto: true, zone_checked_at: checked_at, zone_note: note.into(), ..TimeSettings::default() }
    }

    #[test]
    fn a_lookup_is_never_due_while_automatic_zone_is_off() {
        assert!(!due(&TimeSettings::default(), 1_000_000));
    }

    #[test]
    fn a_never_checked_automatic_zone_is_due_at_once() {
        assert!(due(&auto(0, ""), RECHECK_SECS));
    }

    #[test]
    fn a_good_lookup_waits_the_full_recheck_interval() {
        let t = auto(1000, "ipapi.co: Europe/Paris");
        assert!(!due(&t, 1000 + RECHECK_SECS - 1));
        assert!(due(&t, 1000 + RECHECK_SECS));
    }

    #[test]
    fn a_failed_lookup_is_retried_after_the_shorter_retry_interval() {
        let t = auto(1000, "Could not reach ipapi.co");
        assert!(!due(&t, 1000 + RETRY_SECS - 1));
        assert!(due(&t, 1000 + RETRY_SECS));
        assert!(RETRY_SECS < RECHECK_SECS);
    }

    #[test]
    fn a_changed_zone_from_automatic_lookup_is_applied_and_returned() {
        let mut t = auto(0, "");
        let r: Result<(String, &'static str), String> = Ok(("Asia/Kolkata".into(), "ipapi.co"));
        assert_eq!(apply(&mut t, &r, 42), Some("Asia/Kolkata".to_string()));
        assert_eq!(t.zone, "Asia/Kolkata");
        assert_eq!(t.zone_note, "ipapi.co: Asia/Kolkata");
        assert_eq!(t.zone_checked_at, 42);
    }

    #[test]
    fn an_unchanged_zone_is_not_reported_as_a_change() {
        let mut t = auto(0, "");
        t.zone = "Europe/Paris".into();
        let r: Result<(String, &'static str), String> = Ok(("Europe/Paris".into(), "worldtimeapi.org"));
        assert_eq!(apply(&mut t, &r, 7), None);
        assert_eq!(t.zone_note, "worldtimeapi.org: Europe/Paris");
    }

    #[test]
    fn a_hand_picked_zone_is_kept_while_the_note_still_records_the_answer() {
        let mut t = TimeSettings { zone: "Europe/Paris".into(), ..TimeSettings::default() };
        let r: Result<(String, &'static str), String> = Ok(("Asia/Kolkata".into(), "ipapi.co"));
        assert_eq!(apply(&mut t, &r, 9), None);
        assert_eq!(t.zone, "Europe/Paris");
        assert_eq!(t.zone_note, "ipapi.co: Asia/Kolkata");
    }

    #[test]
    fn a_failed_lookup_keeps_the_zone_and_becomes_the_note_so_it_is_retried_sooner() {
        let mut t = auto(0, "");
        t.zone = "Europe/Paris".into();
        let r: Result<(String, &'static str), String> = Err("Could not reach ipapi.co".into());
        assert_eq!(apply(&mut t, &r, 500), None);
        assert_eq!(t.zone, "Europe/Paris");
        assert_eq!(t.zone_note, "Could not reach ipapi.co");
        assert_eq!(t.zone_checked_at, 500);
        assert!(due(&t, 500 + RETRY_SECS));
    }

    #[test]
    fn the_explanation_names_the_lookup_services_and_what_is_sent() {
        assert!(EXPLAIN.contains("ipapi.co") && EXPLAIN.contains("worldtimeapi.org"));
        assert!(EXPLAIN.contains("nothing else is sent"));
    }
}
