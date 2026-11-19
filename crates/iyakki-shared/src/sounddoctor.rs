//! Types moved out of `iyakki_agent::sounddoctor` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::time::Duration;

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
pub enum Phase {
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
    pub phase: Phase,
    /// A note is on the screen.
    pub noted: bool,
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

    pub fn attempt(&mut self, now: Duration, n: u8, obs: &Obs, out: &mut Vec<Action>) {
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

/// What the doctor sees when the engine's sound goes through the mixer ([`crate::webmix`]): the engine's streams are then never on the
/// card (the page writes into a pipe, the mixer's one `aplay` holds the card), so "no running engine stream" is the NORMAL state and
/// must not raise an alarm, and the holder of the card is the mixer's own writer, which the doctor must never end. `carrying` says that
/// the tap is handing the page's sound to the mixer: then the engine counts as running. Without sound arriving the engine stays as
/// scanned (the doctor may still reconnect the page), but the mixer's writer is never named or ended.
pub fn through_mixer(mut obs: Obs, carrying: bool) -> Obs {
    obs.holder = None;
    if carrying {
        obs.engine = Engine::Running;
    }
    obs
}

/// Is this line of the web engine's error output about the audio path (GStreamer's sinks, ALSA) and a problem? The first such
/// line of a session goes to the log, so a silent session leaves evidence.
pub fn audio_error_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let audio = ["alsa", "audiosink", "audio sink", "pulsesink", "gstaudio", "audio output"].iter().any(|k| l.contains(k));
    let bad = ["warn", "error", "could not", "busy", "fail", "cannot", "can't"].iter().any(|k| l.contains(k));
    audio && bad
}
