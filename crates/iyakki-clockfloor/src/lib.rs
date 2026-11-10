//! The clock floor ("fake hwclock"): a machine whose battery-backed clock is unusable (a MacBook Pro 2016/17 reads
//! 2024-01-01 at every boot) must not run its first minute in the past: certificates made later are "not yet valid",
//! and k3s then fails in odd ways until the network time arrives.
//!
//! * The time is saved to `<data>/clock-floor` (atomically: write a temp file, flush, rename) at shutdown, every few
//!   minutes while running and when the network time has been set. Nothing is written until the data volume is a real
//!   disk (a RAM-only session has nothing to keep).
//! * Early in boot the clock is never left earlier than the floor = the largest of: the compiled-in [`COMPILED_FLOOR`],
//!   the image's build time (the modification time of this program) and the saved time. The saved time is only believed
//!   when it is not absurd (more than [`MAX_AHEAD_SECS`] after the build time) and the file is well formed. The clock is
//!   only ever moved forward here; a clock already ahead of the floor is left alone.
//! * The floor is applied twice: right at the start of boot (build time and compiled-in floor; the data volume is not
//!   there yet) and again once the data volume is mounted (the saved time).
//!
//! The decisions are pure functions ([`floor`], [`behind`], [`apply`]) with tests; the real clock is set with
//! the setter the agent passes in (`ntp::set_clock`). The k3s dev cluster waits for [`pending`] to end (docs/K3S-DEV.md).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 2026-10-01 00:00:00 UTC: no IYAKKI image is older than this, so no clock should be either. Raised at each release.
pub const COMPILED_FLOOR: i64 = 1_790_812_800;
/// A saved time (or build time) further than this beyond the compiled-in floor is treated as damaged.
pub const MAX_AHEAD_SECS: i64 = 20 * 365 * 86_400;
/// How often the running system saves the time.
pub const SAVE_EVERY: Duration = Duration::from_secs(300);
/// Name of the file in the data directory.
pub const FILE_NAME: &str = "clock-floor";

/// The text kept in the file.
pub fn encode(t: i64) -> String {
    format!("{t}\n")
}

/// The time in the file's text; `None` for anything that is not exactly one whole number of seconds.
pub fn parse(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() || t.len() > 12 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok().filter(|v| *v > 0)
}

/// The floor from what the image says: the build time counts only when it is plausible (not 1970 from a tool that drops
/// time stamps, not years ahead).
pub fn build_floor(build_time: i64) -> i64 {
    if build_time > COMPILED_FLOOR && build_time <= COMPILED_FLOOR + MAX_AHEAD_SECS { build_time } else { COMPILED_FLOOR }
}

/// Is a saved time believable next to this floor? Not before it (it adds nothing) and not absurdly far ahead.
pub fn saved_is_sane(saved: i64, base_floor: i64) -> bool {
    saved >= base_floor && saved <= base_floor + MAX_AHEAD_SECS
}

/// The time the clock must not be earlier than.
pub fn floor(saved: Option<i64>, build_time: i64) -> i64 {
    let base = build_floor(build_time);
    match saved {
        Some(s) if saved_is_sane(s, base) => s.max(base),
        _ => base,
    }
}

/// The time to set when the clock `now` is behind `floor`; `None` when it is at or ahead of it (never go backwards).
pub fn behind(now: i64, floor: i64) -> Option<i64> {
    (now < floor).then_some(floor)
}

/// What [`apply`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub floor: i64,
    /// The time the clock was set to, when it was behind.
    pub set_to: Option<i64>,
    /// The saved time was missing, damaged or absurd and was ignored.
    pub saved_ignored: bool,
    /// Setting the clock failed (not root).
    pub error: Option<String>,
}

/// Move the clock up to the floor when it is behind. `set` changes the real clock (or a test's).
pub fn apply(saved_text: Option<&str>, build_time: i64, now: i64, set: &mut dyn FnMut(i64) -> Result<(), String>) -> Applied {
    let saved = saved_text.and_then(parse);
    let base = build_floor(build_time);
    let saved_ignored = saved_text.is_some() && !saved.is_some_and(|s| saved_is_sane(s, base));
    let fl = floor(saved, build_time);
    match behind(now, fl) {
        Some(t) => match set(t) {
            Ok(()) => Applied { floor: fl, set_to: Some(t), saved_ignored, error: None },
            Err(e) => Applied { floor: fl, set_to: None, saved_ignored, error: Some(e) },
        },
        None => Applied { floor: fl, set_to: None, saved_ignored, error: None },
    }
}

/// Write `t` to `path` so that a reader sees the old or the new content, never half: temp file in the same folder, flush to
/// disk, rename over.
pub fn write_atomic(path: &Path, t: i64) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or(FILE_NAME)));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(encode(t).as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Should the running clock be saved? Never a time before the build floor: that would be the clock before it was fixed.
pub fn worth_saving(now: i64, build_time: i64) -> bool {
    now >= build_floor(build_time)
}

// ------------------------------------------------------------------------------------------------ the real machine

static DATA_READY: AtomicBool = AtomicBool::new(false);
static MANAGED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
static SAVER: AtomicBool = AtomicBool::new(false);

fn data_dir() -> PathBuf {
    PathBuf::from("/var/lib/iyakki")
}

/// The image's build time: the newest modification time of this program and of `/etc/os-release` (an image build keeps its
/// file times; a copied or touched file only makes the floor later, which is safe up to the sanity limit).
pub fn image_build_time() -> i64 {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    let exe = std::env::current_exe().map(|p| mtime(&p)).unwrap_or(0);
    exe.max(mtime(Path::new("/etc/os-release")))
}

fn now_secs() -> i64 {
    iyakki_clock::now_unix()
}

/// Boot, first step (before the data volume exists): the build-time floor. Marks the floor as pending for the k3s gate.
pub fn apply_early(set: &mut dyn FnMut(i64) -> Result<(), String>) -> Applied {
    MANAGED.store(true, Ordering::SeqCst);
    PENDING.store(true, Ordering::SeqCst);
    let a = apply(None, image_build_time(), now_secs(), set);
    log::info!("clock floor (early): {}", describe(&a));
    a
}

/// Boot, second step: the data volume is mounted (`on_disk`: it is a real disk), so the saved time is read and applied. Ends
/// the pending state; starts the saver when the data lives on a disk.
pub fn apply_after_data(on_disk: bool, set: &mut dyn FnMut(i64) -> Result<(), String>) -> Applied {
    DATA_READY.store(on_disk, Ordering::SeqCst);
    let text = if on_disk { std::fs::read_to_string(data_dir().join(FILE_NAME)).ok() } else { None };
    let a = apply(text.as_deref(), image_build_time(), now_secs(), set);
    log::info!("clock floor (data volume): {}", describe(&a));
    PENDING.store(false, Ordering::SeqCst);
    if on_disk {
        save_now();
        spawn_saver();
    }
    a
}

fn describe(a: &Applied) -> String {
    let mut s = match (&a.set_to, &a.error) {
        (Some(t), _) => format!("the clock was behind and is now {t} (unix seconds)"),
        (None, Some(e)) => format!("the clock is behind {} but could not be set: {e}", a.floor),
        (None, None) => format!("the clock is not behind {}", a.floor),
    };
    if a.saved_ignored {
        s.push_str("; the saved time was missing, damaged or absurd and was ignored");
    }
    s
}

/// True from the start of boot until the saved time was applied. Always false on a machine where IYAKKI does not run as the
/// first process (nothing manages the clock there).
pub fn pending() -> bool {
    MANAGED.load(Ordering::SeqCst) && PENDING.load(Ordering::SeqCst)
}

/// Save the current time (atomically) when the data volume is a disk and the clock is past the build floor.
pub fn save_now() {
    if !DATA_READY.load(Ordering::SeqCst) {
        return;
    }
    let now = now_secs();
    if !worth_saving(now, image_build_time()) {
        return;
    }
    if let Err(e) = write_atomic(&data_dir().join(FILE_NAME), now) {
        log::warn!("clock floor: could not save the time: {e}");
    }
}

/// Save every few minutes for as long as the system runs.
pub fn spawn_saver() {
    if SAVER.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("iyakki-clock-save".into()).spawn(|| loop {
        std::thread::sleep(SAVE_EVERY);
        save_now();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: i64 = COMPILED_FLOOR + 20 * 86_400;
    const FAKE_BOOT: i64 = 1_704_067_320; // 2024-01-01 00:02

    fn run(saved: Option<&str>, build: i64, now: i64) -> (Applied, Vec<i64>) {
        let mut sets = Vec::new();
        let a = apply(saved, build, now, &mut |t| {
            sets.push(t);
            Ok(())
        });
        (a, sets)
    }

    #[test]
    fn a_clock_behind_the_floor_is_set_to_the_largest_of_saved_build_and_compiled() {
        let saved = (BUILD + 5 * 86_400).to_string();
        let (a, sets) = run(Some(&saved), BUILD, FAKE_BOOT);
        assert_eq!((sets, a.set_to), (vec![BUILD + 5 * 86_400], Some(BUILD + 5 * 86_400)));
        // first boot: no saved file, the image's build time
        assert_eq!(run(None, BUILD, FAKE_BOOT).1, vec![BUILD]);
        // a build time that says 1970 (a tool dropped it): the compiled-in floor
        assert_eq!(run(None, 0, FAKE_BOOT).1, vec![COMPILED_FLOOR]);
        // a saved time older than the build (an old data volume under a new image): the build time wins
        assert_eq!(run(Some(&(BUILD - 99_999).to_string()), BUILD, FAKE_BOOT).1, vec![BUILD]);
    }

    #[test]
    fn a_clock_ahead_of_the_floor_is_never_moved_back() {
        let now = BUILD + 400 * 86_400;
        let (a, sets) = run(Some(&(BUILD + 86_400).to_string()), BUILD, now);
        assert!(sets.is_empty() && a.set_to.is_none() && a.error.is_none());
        assert_eq!(behind(now, now), None, "equal is not behind");
        assert_eq!(behind(now - 1, now), Some(now));
    }

    #[test]
    fn a_missing_damaged_or_absurd_saved_time_is_ignored() {
        for bad in ["", "abc", "-5", "12.5", "99999999999999", "1790000000 junk", "0"] {
            let (a, sets) = run(Some(bad), BUILD, FAKE_BOOT);
            assert_eq!(sets, vec![BUILD], "{bad:?}");
            assert!(a.saved_ignored, "{bad:?}");
        }
        // years in the future (a clock that was once set wrong): beyond build time + 20 years
        let absurd = (BUILD + MAX_AHEAD_SECS + 10 * 86_400).to_string();
        let (a, sets) = run(Some(&absurd), BUILD, FAKE_BOOT);
        assert_eq!(sets, vec![BUILD]);
        assert!(a.saved_ignored);
        // just inside the limit is believed
        assert_eq!(run(Some(&(BUILD + MAX_AHEAD_SECS - 1).to_string()), BUILD, FAKE_BOOT).1, vec![BUILD + MAX_AHEAD_SECS - 1]);
        // no file at all is not "ignored", it is first boot
        assert!(!run(None, BUILD, FAKE_BOOT).0.saved_ignored);
        // an absurd build time (years ahead) falls back to the compiled-in floor
        assert_eq!(build_floor(COMPILED_FLOOR + MAX_AHEAD_SECS + 1), COMPILED_FLOOR);
        assert_eq!(parse(" 1790812800\n"), Some(1_790_812_800));
    }

    #[test]
    fn a_failure_to_set_the_clock_is_reported_not_fatal() {
        let a = apply(None, BUILD, FAKE_BOOT, &mut |_| Err("not root".into()));
        assert_eq!((a.set_to, a.error.as_deref()), (None, Some("not root")));
        assert!(describe(&a).contains("could not be set"));
    }

    #[test]
    fn the_time_is_written_atomically_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("iyakki-clockfloor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("state").join(FILE_NAME);
        write_atomic(&p, BUILD + 1).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&p).unwrap()), Some(BUILD + 1));
        write_atomic(&p, BUILD + 2).unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&p).unwrap()), Some(BUILD + 2));
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE_NAME.to_string()], "no temp file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_fixed_clock_is_saved() {
        assert!(!worth_saving(FAKE_BOOT, BUILD), "the fake 2024 time is never saved");
        assert!(worth_saving(BUILD, BUILD) && worth_saving(BUILD + 1, BUILD));
        assert!(!pending(), "a program that never applied the floor is not waiting for it");
    }
}
