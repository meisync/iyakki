//! What a few engines publish so the home screen can say something about their apps (F-LAUNCH-09, LAUNCHER-PLAN slice 5): the engine writes
//! one number here when it knows it; the core reads it when it builds the statuses (`corehandle`). Numbers only, never content.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

static VOICE_LISTENING: AtomicBool = AtomicBool::new(false);
static DOCTOR_PROBLEMS: AtomicI64 = AtomicI64::new(-1);

/// The wake listener has the microphone open to the spotter right now (the red sign is on).
pub fn set_voice_listening(on: bool) {
    VOICE_LISTENING.store(on, Ordering::SeqCst);
}

pub fn voice_listening() -> bool {
    VOICE_LISTENING.load(Ordering::SeqCst)
}

/// How many findings that are more than a note the last DOCTOR report had (set whenever a report is made in this run of the system).
pub fn set_doctor_problems(n: usize) {
    DOCTOR_PROBLEMS.store(n as i64, Ordering::SeqCst);
}

/// The problems of the last report, `None` before any report was made.
pub fn doctor_problems() -> Option<usize> {
    usize::try_from(DOCTOR_PROBLEMS.load(Ordering::SeqCst)).ok()
}

/// Study sessions that are due: not done, dated today or earlier in the computer's own time zone (decision D-LAUNCH-09: the day ends at
/// the computer's local midnight, the zone Settings, Time shows).
pub fn learn_due(plans: &[iyakki_shared::study::Plan], today: &str) -> usize {
    plans.iter().flat_map(|p| p.sessions.iter()).filter(|s| !s.done && s.date.as_str() <= today).count()
}

/// Today's date as `YYYY-MM-DD` in the computer's time zone.
pub fn today_text(zone: &iyakki_clock::Zone, now: i64) -> String {
    let l = zone.local(now);
    format!("{:04}-{:02}-{:02}", l.year, l.month, l.day)
}
