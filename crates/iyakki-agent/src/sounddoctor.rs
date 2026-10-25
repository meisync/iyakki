//! The sound doctor: a page that says it plays audio while none of the web engine's processes has a playback stream running.
//!
//! Why: on the T2 Mac the browser was sometimes silent on YouTube for a whole session, and closing and opening it again fixed it.
//! The owner read the kernel while it was silent: `/proc/asound/card0/pcm0p/sub0/status` said `closed`, nobody held the speaker, so
//! the engine's GStreamer sink never opened the card (or fell back to another sink, or the pipeline failed before opening) and
//! WebKit does not retry. The helper reports `AudioPlaying` (WebKit `is-playing-audio`); the shell feeds [`Doctor::step`] about
//! twice a second with what the kernel says ([`scan`]) and carries out the [`Action`]s it returns:
//! * playing and a stream of the engine is RUNNING: fine, nothing to do (and a running recovery stops, its note is cleared);
//! * playing and no engine stream RUNNING (closed, PREPARED, open) for [`GRACE`]: attempt 1: if another program holds the card and
//!   is not running (the agent's own prime, an idle leftover) it is ended; a running holder is only named; then the page's
//!   playing media elements are paused and played again ([`RECONNECT_JS`]), which makes GStreamer create its sink again (what
//!   closing and opening the browser achieved);
//! * still silent after [`RETRY_WAIT`]: attempt 2, the same;
//! * still silent after another [`RETRY_WAIT`]: one message, "reload the tab", and no more tries until the page stops playing.
//!
//! The state machine is pure (a clock value and an observation in, actions out); [`scan`] reads `/proc` through given roots, so
//! tests use a fake tree. The lookup of who owns a stream stays here, in the agent (it runs as root; the developer shell does not).

use std::path::Path;
use std::time::Duration;

use crate::quiet::{PcmState, playback_streams};

/// How long a page may play with no running engine stream before the first attempt.
pub const GRACE: Duration = Duration::from_secs(3);
/// How long each attempt gets to work.
pub const RETRY_WAIT: Duration = Duration::from_secs(3);
/// Shown at every attempt.
pub const NOTE_RECONNECTING: &str = "Sound was not connected; reconnecting...";
/// Shown once, when two attempts failed.
pub const NOTE_GIVE_UP: &str = "Sound is still not connected: reload the tab";

/// Run in the page: pause and play again every media element that is playing, so the engine's audio sink is created again.
/// Answers how many it touched.
pub const RECONNECT_JS: &str = "(function(){var n=0;document.querySelectorAll('video,audio').forEach(function(m){if(m.paused||m.ended)return;n++;var t=m.currentTime;m.pause();setTimeout(function(){try{m.currentTime=t}catch(e){}var p=m.play();if(p&&p.catch)p.catch(function(){})},250)});return n})()";

/// What the engine's own processes hold: the best state among their playback streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Running,
    /// A stream is open but not running (PREPARED, OPEN, PAUSED...).
    Idle,
    /// No stream at all.
    Closed,
}

/// A program that is not the engine and holds a playback stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    pub name: String,
    pub running: bool,
}

/// One look at the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obs {
    /// The page says it plays audio.
    pub playing: bool,
    pub engine: Engine,
    pub holder: Option<Holder>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Show this in the browser bar (and the log).
    Note(String),
    /// End this program (SIGTERM; it is idle, see [`close_holder`]).
    CloseHolder(u32),
    /// Run [`RECONNECT_JS`] in the page.
    Reconnect,
    /// The trouble is over: take the note away.
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The page plays nothing.
    Quiet,
    /// The page plays and the engine's stream runs.
    Healthy,
    /// Playing, not running since this time.
    Watching(Duration),
    /// An attempt was made at this time (attempt number 1 or 2).
    Trying(Duration, u8),
    /// Given up until the page stops playing.
    GaveUp,
}

pub struct Doctor {
    phase: Phase,
    /// A note is on the screen.
    noted: bool,
}

impl Default for Doctor {
    fn default() -> Doctor {
        Doctor { phase: Phase::Quiet, noted: false }
    }
}

impl Doctor {
    pub fn new() -> Doctor {
        Doctor::default()
    }

    /// Is a look worth taking (the page plays)?
    pub fn wants_look(&self) -> bool {
        self.phase != Phase::Quiet || self.noted
    }

    /// `now` is time since any fixed start.
    pub fn step(&mut self, now: Duration, obs: &Obs) -> Vec<Action> {
        let mut out = Vec::new();
        if !obs.playing {
            if self.noted {
                out.push(Action::Clear);
                self.noted = false;
            }
            self.phase = Phase::Quiet;
            return out;
        }
        if obs.engine == Engine::Running {
            if self.noted {
                out.push(Action::Clear);
                self.noted = false;
            }
            self.phase = Phase::Healthy;
            return out;
        }
        match self.phase {
            Phase::Quiet | Phase::Healthy => self.phase = Phase::Watching(now),
            Phase::Watching(since) if now.saturating_sub(since) >= GRACE => self.attempt(now, 1, obs, &mut out),
            Phase::Trying(at, n) if now.saturating_sub(at) >= RETRY_WAIT => {
                if n < 2 {
                    self.attempt(now, n + 1, obs, &mut out);
                } else {
                    self.phase = Phase::GaveUp;
                    self.noted = true;
                    out.push(Action::Note(NOTE_GIVE_UP.into()));
                }
            }
            _ => {}
        }
        out
    }

    fn attempt(&mut self, now: Duration, n: u8, obs: &Obs, out: &mut Vec<Action>) {
        self.phase = Phase::Trying(now, n);
        self.noted = true;
        out.push(Action::Note(NOTE_RECONNECTING.into()));
        if let Some(h) = &obs.holder {
            if h.running {
                out.push(Action::Note(format!("Sound was not connected: {} (process {}) is using the speaker", h.name, h.pid)));
            } else {
                out.push(Action::CloseHolder(h.pid));
            }
        }
        out.push(Action::Reconnect);
    }
}

/// The parent of a process (`/proc/<pid>/stat`, the field after the command name, which may hold spaces and brackets).
fn parent_of(proc_root: &Path, pid: u32) -> Option<u32> {
    let t = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    let rest = &t[t.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Is `pid` the engine's helper or one of its descendants? Bounded walk, no `/proc/<pid>/fd` needed.
pub fn is_engine(proc_root: &Path, helper: u32, pid: u32) -> bool {
    let mut p = pid;
    for _ in 0..32 {
        if p == helper {
            return true;
        }
        match parent_of(proc_root, p) {
            Some(q) if q > 1 && q != p => p = q,
            _ => return false,
        }
    }
    false
}

fn comm(proc_root: &Path, pid: u32) -> String {
    let c = std::fs::read_to_string(proc_root.join(pid.to_string()).join("comm")).unwrap_or_default();
    let c = c.trim();
    if c.is_empty() { format!("process {pid}") } else { c.to_string() }
}

/// Look: which playback streams the engine's processes hold, and who else holds one. `asound` is `/proc/asound`, `proc_root` is `/proc`.
pub fn scan(asound: &Path, proc_root: &Path, helper: u32, own_pid: u32, playing: bool) -> Obs {
    let mut engine = Engine::Closed;
    let mut holder: Option<Holder> = None;
    for s in playback_streams(asound) {
        if s.state == PcmState::Closed {
            continue;
        }
        let engine_owned = s.owner.is_some_and(|p| is_engine(proc_root, helper, p));
        if engine_owned {
            engine = match (engine, s.state) {
                (_, PcmState::Running) => Engine::Running,
                (Engine::Running, _) => Engine::Running,
                _ => Engine::Idle,
            };
        } else if holder.is_none() {
            let (pid, name) = match s.owner {
                Some(p) => (p, comm(proc_root, p)),
                None => (0, "another program".to_string()),
            };
            // the agent's own prime is a holder like any other, but it may always be ended
            let name = if pid == own_pid { "the sound agent".to_string() } else { name };
            holder = Some(Holder { pid, name, running: s.state == PcmState::Running });
        }
    }
    Obs { playing, engine, holder }
}

/// End an idle holder. Never this process, init, or pid 0 (unknown owner).
pub fn close_holder(pid: u32, own_pid: u32) -> bool {
    if pid <= 1 || pid == own_pid {
        return false;
    }
    // SAFETY: a plain signal to a process found a moment ago
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
}

/// Is this line of the web engine's error output about the audio path (GStreamer's sinks, ALSA) and a problem? The first such
/// line of a session goes to the log, so a silent session leaves evidence.
pub fn audio_error_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let audio = ["alsa", "audiosink", "audio sink", "pulsesink", "gstaudio", "audio output"].iter().any(|k| l.contains(k));
    let bad = ["warn", "error", "could not", "busy", "fail", "cannot", "can't"].iter().any(|k| l.contains(k));
    audio && bad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(playing: bool, engine: Engine, holder: Option<Holder>) -> Obs {
        Obs { playing, engine, holder }
    }
    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    fn idle_holder() -> Option<Holder> {
        Some(Holder { pid: 500, name: "aplay".into(), running: false })
    }

    #[test]
    fn playing_with_a_running_engine_stream_is_fine() {
        let mut d = Doctor::new();
        for t in 0..20 {
            assert!(d.step(s(t), &obs(true, Engine::Running, None)).is_empty());
        }
    }

    #[test]
    fn a_page_that_plays_nothing_is_never_looked_at() {
        let mut d = Doctor::new();
        assert!(!d.wants_look());
        for t in 0..20 {
            assert!(d.step(s(t), &obs(false, Engine::Closed, None)).is_empty());
        }
    }

    #[test]
    fn closed_or_prepared_for_three_seconds_reconnects_then_again_then_says_it_once_and_stops() {
        for state in [Engine::Closed, Engine::Idle] {
            let mut d = Doctor::new();
            assert!(d.step(s(0), &obs(true, state, None)).is_empty());
            assert!(d.step(s(2), &obs(true, state, None)).is_empty(), "inside the grace time");
            let a = d.step(s(3), &obs(true, state, None));
            assert_eq!(a, [Action::Note(NOTE_RECONNECTING.into()), Action::Reconnect], "{state:?}");
            assert!(d.step(s(5), &obs(true, state, None)).is_empty(), "the attempt gets its time");
            let a = d.step(s(6), &obs(true, state, None));
            assert_eq!(a, [Action::Note(NOTE_RECONNECTING.into()), Action::Reconnect], "the second attempt");
            assert!(d.step(s(8), &obs(true, state, None)).is_empty());
            let a = d.step(s(9), &obs(true, state, None));
            assert_eq!(a, [Action::Note(NOTE_GIVE_UP.into())], "one message");
            for t in 10..60 {
                assert!(d.step(s(t), &obs(true, state, None)).is_empty(), "never tries again, never loops");
            }
        }
    }

    #[test]
    fn an_idle_holder_is_ended_first_a_running_one_is_only_named() {
        let mut d = Doctor::new();
        d.step(s(0), &obs(true, Engine::Closed, idle_holder()));
        let a = d.step(s(3), &obs(true, Engine::Closed, idle_holder()));
        assert_eq!(a, [Action::Note(NOTE_RECONNECTING.into()), Action::CloseHolder(500), Action::Reconnect]);
        let busy = Some(Holder { pid: 77, name: "ffplay".into(), running: true });
        let mut d = Doctor::new();
        d.step(s(0), &obs(true, Engine::Closed, busy.clone()));
        let a = d.step(s(3), &obs(true, Engine::Closed, busy));
        assert!(a.contains(&Action::Reconnect) && !a.iter().any(|x| matches!(x, Action::CloseHolder(_))));
        assert!(a.iter().any(|x| matches!(x, Action::Note(n) if n.contains("ffplay") && n.contains("77"))));
    }

    #[test]
    fn recovering_midway_stops_and_clears_the_note() {
        let mut d = Doctor::new();
        d.step(s(0), &obs(true, Engine::Closed, None));
        d.step(s(3), &obs(true, Engine::Closed, None));
        assert_eq!(d.step(s(4), &obs(true, Engine::Running, None)), [Action::Clear]);
        assert!(d.step(s(7), &obs(true, Engine::Running, None)).is_empty());
        // a later silence starts over with the grace time
        assert!(d.step(s(10), &obs(true, Engine::Idle, None)).is_empty());
        assert!(d.step(s(12), &obs(true, Engine::Idle, None)).is_empty());
        assert!(!d.step(s(13), &obs(true, Engine::Idle, None)).is_empty());
    }

    #[test]
    fn the_page_stopping_cancels_and_a_later_play_starts_fresh() {
        let mut d = Doctor::new();
        d.step(s(0), &obs(true, Engine::Closed, None));
        d.step(s(3), &obs(true, Engine::Closed, None));
        assert_eq!(d.step(s(4), &obs(false, Engine::Closed, None)), [Action::Clear]);
        assert!(!d.wants_look());
        assert!(d.step(s(5), &obs(true, Engine::Closed, None)).is_empty());
        assert!(d.step(s(7), &obs(true, Engine::Closed, None)).is_empty());
        assert!(!d.step(s(8), &obs(true, Engine::Closed, None)).is_empty());
        // after giving up, stopping and playing again gets a new set of attempts and the note goes away
        let mut d = Doctor::new();
        for t in [0, 3, 6, 9] {
            d.step(s(t), &obs(true, Engine::Closed, None));
        }
        assert_eq!(d.step(s(10), &obs(false, Engine::Closed, None)), [Action::Clear]);
    }

    fn tree(tag: &str) -> std::path::PathBuf {
        let r = std::env::temp_dir().join(format!("sounddoctor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("asound")).unwrap();
        std::fs::create_dir_all(r.join("proc")).unwrap();
        r
    }
    fn stream(r: &Path, status: &str) {
        let d = r.join("asound/card0/pcm0p/sub0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("status"), status).unwrap();
    }
    fn process(r: &Path, pid: u32, ppid: u32, name: &str) {
        let d = r.join("proc").join(pid.to_string());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("stat"), format!("{pid} ({name} x) S {ppid} 1 1 0 -1\n")).unwrap();
        std::fs::write(d.join("comm"), format!("{name}\n")).unwrap();
    }

    #[test]
    fn the_scan_tells_engine_streams_from_other_programs_by_ancestry() {
        let r = tree("scan");
        process(&r, 100, 1, "iyakki-webview"); // the helper
        process(&r, 110, 100, "WPEWebProcess");
        process(&r, 500, 1, "aplay");
        let (a, p) = (r.join("asound"), r.join("proc"));
        stream(&r, "closed\n");
        assert_eq!(scan(&a, &p, 100, 1, true), obs(true, Engine::Closed, None), "closed: nobody");
        stream(&r, "state: PREPARED\nowner_pid : 110\n");
        assert_eq!(scan(&a, &p, 100, 1, true), obs(true, Engine::Idle, None));
        stream(&r, "state: RUNNING\nowner_pid : 110\n");
        assert_eq!(scan(&a, &p, 100, 1, true).engine, Engine::Running);
        stream(&r, "state: PREPARED\nowner_pid : 500\n");
        assert_eq!(scan(&a, &p, 100, 1, true), obs(true, Engine::Closed, Some(Holder { pid: 500, name: "aplay".into(), running: false })));
        stream(&r, "state: RUNNING\nowner_pid : 500\n");
        assert!(scan(&a, &p, 100, 1, true).holder.unwrap().running);
        stream(&r, "state: PREPARED\nowner_pid : 500\n");
        assert_eq!(scan(&a, &p, 100, 500, true).holder.unwrap().name, "the sound agent", "the agent's own open is named so");
        assert!(!is_engine(&p, 100, 500) && is_engine(&p, 100, 110) && is_engine(&p, 100, 100));
        assert_eq!(scan(&r.join("none"), &p, 100, 1, true), obs(true, Engine::Closed, None));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn pids_that_must_not_be_ended_are_refused() {
        assert!(!close_holder(0, 9) && !close_holder(1, 9) && !close_holder(9, 9));
    }

    #[test]
    fn audio_error_lines_are_recognised() {
        assert!(audio_error_line("0:00:01 WARN alsasink gstalsasink.c:880:gst_alsasink_open: Could not open audio device for playback."));
        assert!(audio_error_line("ERROR autoaudiosink: Failed to find a usable audio sink"));
        assert!(!audio_error_line("INFO alsasink: opened"));
        assert!(!audio_error_line("WARN webkit: something about fonts"));
    }

    #[test]
    fn the_reconnect_script_touches_only_playing_media_and_restores_the_position() {
        assert!(RECONNECT_JS.contains("querySelectorAll('video,audio')") && RECONNECT_JS.contains("m.paused||m.ended") && RECONNECT_JS.contains("currentTime=t") && RECONNECT_JS.contains("m.play()"));
    }
}
