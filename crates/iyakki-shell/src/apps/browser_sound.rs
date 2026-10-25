//! The browser's side of the sound doctor (`iyakki_agent::sounddoctor`): remembers whether the page plays audio, looks at the
//! kernel's playback streams about twice a second while it does, and turns the doctor's actions into things the view does.
//! Also keeps the first audio error line of the engine's output, so a silent session leaves a line in the log.

use std::path::Path;
use std::time::{Duration, Instant};

use iyakki_agent::sounddoctor::{self, Action, Doctor};

/// How often the kernel is looked at while the page plays.
const LOOK_EVERY: Duration = Duration::from_millis(500);

pub struct SoundWatch {
    doctor: Doctor,
    playing: bool,
    start: Instant,
    last: Option<Duration>,
    /// The first line of the engine's output that was about a failing audio path.
    pub first_error: Option<String>,
}

impl SoundWatch {
    pub fn new() -> SoundWatch {
        SoundWatch { doctor: Doctor::new(), playing: false, start: Instant::now(), last: None, first_error: None }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// The helper's `AudioPlaying`. The web engine's flag also tells the agent not to open the speaker itself (softvol's prime).
    pub fn set_playing(&mut self, on: bool) {
        if on != self.playing {
            log::info!("sound doctor: the page {} audio", if on { "plays" } else { "stopped playing" });
        }
        self.playing = on;
        iyakki_agent::softvol::note_web_media(on);
    }

    /// A line of the helper's output: the first audio problem is logged once (and kept).
    pub fn on_log(&mut self, line: &str) {
        if self.first_error.is_none() && sounddoctor::audio_error_line(line) {
            let l: String = line.chars().take(300).collect();
            log::warn!("sound doctor: first audio message from the web engine: {l}");
            self.first_error = Some(l);
        }
    }

    /// Look and decide, at most every [`LOOK_EVERY`]; the actions to carry out. `helper` is the engine helper's pid.
    pub fn tick(&mut self, helper: u32, asound: &Path, proc_root: &Path, own_pid: u32) -> Vec<Action> {
        if !self.playing && !self.doctor.wants_look() {
            return Vec::new();
        }
        let now = self.start.elapsed();
        if self.last.is_some_and(|t| now.saturating_sub(t) < LOOK_EVERY) {
            return Vec::new();
        }
        self.last = Some(now);
        self.step(now, helper, asound, proc_root, own_pid)
    }

    fn step(&mut self, now: Duration, helper: u32, asound: &Path, proc_root: &Path, own_pid: u32) -> Vec<Action> {
        let obs = sounddoctor::scan(asound, proc_root, helper, own_pid, self.playing);
        let actions = self.doctor.step(now, &obs);
        for a in &actions {
            match a {
                Action::Note(n) => log::info!("sound doctor: {n} (engine {:?}, other holder {:?}, first engine message {:?})", obs.engine, obs.holder, self.first_error),
                Action::CloseHolder(p) => log::info!("sound doctor: ending the idle program {p} that holds the speaker"),
                Action::Reconnect => log::info!("sound doctor: pausing and playing the page's media again to reconnect its sound"),
                Action::Clear => log::info!("sound doctor: the sound is connected"),
            }
        }
        actions
    }
}

impl Drop for SoundWatch {
    fn drop(&mut self) {
        if self.playing {
            iyakki_agent::softvol::note_web_media(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let r = std::env::temp_dir().join(format!("browsersound-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("asound/card0/pcm0p/sub0")).unwrap();
        std::fs::create_dir_all(r.join("proc/100")).unwrap();
        std::fs::write(r.join("asound/card0/pcm0p/sub0/status"), "closed\n").unwrap();
        std::fs::write(r.join("proc/100/stat"), "100 (helper) S 1 1 1 0\n").unwrap();
        r
    }

    #[test]
    fn a_silent_page_is_reconnected_twice_then_told_once_and_a_quiet_page_is_never_looked_at() {
        let r = tree("flow");
        let (a, p) = (r.join("asound"), r.join("proc"));
        let mut w = SoundWatch::new();
        assert!(w.step(Duration::from_secs(0), 100, &a, &p, 1).is_empty());
        w.set_playing(true);
        let mut got = Vec::new();
        for t in 0..12u64 {
            got.extend(w.step(Duration::from_secs(t), 100, &a, &p, 1));
        }
        assert_eq!(got.iter().filter(|x| **x == Action::Reconnect).count(), 2);
        assert_eq!(got.iter().filter(|x| **x == Action::Note(sounddoctor::NOTE_GIVE_UP.into())).count(), 1);
        // the sink runs: cleared
        std::fs::write(a.join("card0/pcm0p/sub0/status"), "state: RUNNING\nowner_pid : 100\n").unwrap();
        assert_eq!(w.step(Duration::from_secs(13), 100, &a, &p, 1), [Action::Clear]);
        w.set_playing(false);
        assert!(w.tick(100, &a, &p, 1).is_empty());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn the_first_audio_error_line_is_kept_and_later_ones_are_not() {
        let mut w = SoundWatch::new();
        w.on_log("INFO nothing");
        assert!(w.first_error.is_none());
        w.on_log("WARN alsasink: Could not open audio device for playback. Device is being used by another application.");
        w.on_log("ERROR alsasink: another");
        assert!(w.first_error.as_deref().unwrap().contains("being used"));
    }
}
