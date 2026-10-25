//! System sounds: small cues for signing in, signing out, a new notification and a finished job.
//!
//! Nothing is shipped: every cue is made here, as 16-bit mono PCM at 44.1 kHz, from a few sine notes with a little FM and an
//! envelope that starts and ends at zero (no clicks). They are short (under 600 ms) and quiet: the cue level is 12 dB below
//! IYAKKI's own volume unless the person changes it (Settings -> System sounds), and they play through the same output chain as
//! the radio and the test chime: the output and the software volume of Settings -> Sound, never around them.
//!
//! Playing is asynchronous. [`start`] returns at once with a [`CueHandle`]; the sign-out path starts the logout cue first, does its
//! own work, and then waits for the handle for at most [`LOGOUT_WAIT`], so the sound is over before the screen changes and a slow
//! or stuck output can never hold sign-out back.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audioout::SoundSettings;
use crate::voice;

/// Sample rate of the cues.
pub const RATE: u32 = 44_100;

/// The loudest sample of a cue before the offset, as a fraction of full scale (about -4.4 dBFS).
pub const PEAK: f32 = 0.6;

/// The longest sign-out waits for its cue to end.
pub const LOGOUT_WAIT: Duration = Duration::from_millis(700);

/// The offset range of Settings -> System sounds: never louder than IYAKKI's volume, at most 30 dB quieter.
pub const MIN_OFFSET_DB: i32 = -30;
pub const MAX_OFFSET_DB: i32 = 0;
pub const DEFAULT_OFFSET_DB: i32 = -12;

/// The moments that have a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    Login,
    Logout,
    Notify,
    JobDone,
}

impl Cue {
    pub const ALL: [Cue; 4] = [Cue::Login, Cue::Logout, Cue::Notify, Cue::JobDone];

    pub fn id(self) -> &'static str {
        match self {
            Cue::Login => "login",
            Cue::Logout => "logout",
            Cue::Notify => "notify",
            Cue::JobDone => "job-done",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cue::Login => "Signing in",
            Cue::Logout => "Signing out",
            Cue::Notify => "New notification",
            Cue::JobDone => "A job finished",
        }
    }

    /// How the sound feels, for the settings page.
    pub fn feel(self) -> &'static str {
        match self {
            Cue::Login => "two soft notes, rising",
            Cue::Logout => "two soft notes, falling",
            Cue::Notify => "a short two-note chime",
            Cue::JobDone => "three quick notes, rising",
        }
    }
}

/// Settings -> System sounds (kept in the settings file; old files without it load with these defaults).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemSounds {
    /// The master switch.
    pub enabled: bool,
    /// How much quieter than IYAKKI's volume the cues are, in dB (-30 to 0).
    pub offset_db: i32,
    pub login: bool,
    pub logout: bool,
    pub notify: bool,
    pub job_done: bool,
}

impl Default for SystemSounds {
    fn default() -> Self {
        SystemSounds { enabled: true, offset_db: DEFAULT_OFFSET_DB, login: true, logout: true, notify: true, job_done: true }
    }
}

impl SystemSounds {
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_OFFSET_DB..=MAX_OFFSET_DB).contains(&self.offset_db) {
            return Err(format!("the system sounds level must be between {MIN_OFFSET_DB} and {MAX_OFFSET_DB} dB"));
        }
        Ok(())
    }

    /// Whether this cue is switched on (the master switch and its own).
    pub fn allows(&self, c: Cue) -> bool {
        self.enabled && self.get(c)
    }

    pub fn get(&self, c: Cue) -> bool {
        match c {
            Cue::Login => self.login,
            Cue::Logout => self.logout,
            Cue::Notify => self.notify,
            Cue::JobDone => self.job_done,
        }
    }

    pub fn set(&mut self, c: Cue, on: bool) {
        match c {
            Cue::Login => self.login = on,
            Cue::Logout => self.logout = on,
            Cue::Notify => self.notify = on,
            Cue::JobDone => self.job_done = on,
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ the sounds

/// One note: its pitch (Hz), where it starts and how long it lasts (ms), and its share of the loudness.
struct Note {
    hz: f32,
    at_ms: u32,
    len_ms: u32,
    level: f32,
}

fn notes(c: Cue) -> Vec<Note> {
    // C5 523.25, E5 659.26, G5 783.99, B5 987.77, E6 1318.51
    match c {
        Cue::Login => vec![Note { hz: 523.25, at_ms: 0, len_ms: 260, level: 0.8 }, Note { hz: 783.99, at_ms: 150, len_ms: 330, level: 1.0 }],
        Cue::Logout => vec![Note { hz: 783.99, at_ms: 0, len_ms: 260, level: 1.0 }, Note { hz: 523.25, at_ms: 150, len_ms: 380, level: 0.8 }],
        Cue::Notify => vec![Note { hz: 1318.51, at_ms: 0, len_ms: 130, level: 0.8 }, Note { hz: 987.77, at_ms: 110, len_ms: 230, level: 1.0 }],
        Cue::JobDone => vec![Note { hz: 523.25, at_ms: 0, len_ms: 160, level: 0.7 }, Note { hz: 659.26, at_ms: 100, len_ms: 160, level: 0.85 }, Note { hz: 783.99, at_ms: 200, len_ms: 280, level: 1.0 }],
    }
}

/// How long the cue plays, in milliseconds.
pub fn duration_ms(c: Cue) -> u32 {
    notes(c).iter().map(|n| n.at_ms + n.len_ms).max().unwrap_or(0)
}

/// The cue as samples: 44.1 kHz mono, quieter than IYAKKI's volume by `offset_db` (0 or below). Starts and ends at zero.
pub fn render(c: Cue, offset_db: i32) -> Vec<i16> {
    let total = (RATE as u64 * duration_ms(c) as u64 / 1000) as usize + 1;
    let mut mix = vec![0f32; total];
    for n in notes(c) {
        let (start, len) = ((RATE as u64 * n.at_ms as u64 / 1000) as usize, (RATE as u64 * n.len_ms as u64 / 1000) as usize);
        let attack = (RATE as f32 * 0.008) as usize;
        let release = (RATE as f32 * 0.020) as usize;
        let tau = n.len_ms as f32 / 1000.0 * 0.32;
        for i in 0..len {
            let Some(slot) = mix.get_mut(start + i) else { break };
            let t = i as f32 / RATE as f32;
            let ph = std::f32::consts::TAU * n.hz * t;
            // a little FM that dies away quickly gives the note a soft bell-like start; the second harmonic adds warmth
            let fm = 0.9 * (-t * 14.0).exp();
            let tone = (ph + fm * (ph * 2.0).sin()).sin() + 0.22 * (2.0 * ph).sin() * (-t * 6.0).exp();
            let mut env = (-t / tau).exp();
            if i < attack {
                env *= i as f32 / attack as f32;
            }
            if i + release > len {
                env *= ((len - i) as f32 / release as f32).clamp(0.0, 1.0);
            }
            *slot += tone * env * n.level;
        }
    }
    let peak = mix.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let db = offset_db.clamp(MIN_OFFSET_DB, MAX_OFFSET_DB) as f32;
    let gain = PEAK / peak * 10f32.powf(db / 20.0);
    let mut out: Vec<i16> = mix.iter().map(|v| (v * gain * 32767.0).round().clamp(-32767.0, 32767.0) as i16).collect();
    if let Some(l) = out.last_mut() {
        *l = 0;
    }
    if let Some(f) = out.first_mut() {
        *f = 0;
    }
    out
}

/// The cue as a WAV file.
pub fn wav(c: Cue, offset_db: i32) -> Vec<u8> {
    voice::encode_wav(&render(c, offset_db), RATE)
}

// ------------------------------------------------------------------------------------------------------------ playing

/// What became of a cue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Played,
    /// Not played on purpose: the switch is off, or sound is muted in Settings -> Sound.
    Off(String),
    /// There is no sound output (Settings -> Sound says why).
    NoOutput(String),
    Failed(String),
}

impl Outcome {
    /// One plain sentence for a message line.
    pub fn say(&self, cue: Cue) -> Result<String, String> {
        match self {
            Outcome::Played => Ok(format!("Played the {} sound.", cue.label().to_lowercase())),
            Outcome::Off(why) => Err(why.clone()),
            Outcome::NoOutput(why) => Err(format!("No sound output: {why}")),
            Outcome::Failed(e) => Err(format!("The sound did not play: {e}")),
        }
    }
}

/// Where cues go. The real one is [`SystemOut`]; tests give a fake.
pub trait CueOut: Send + Sync {
    /// Whether a sound output exists now (`Err(why)`: none, so the cue is skipped quietly).
    fn ready(&self) -> Result<(), String>;
    /// Play the WAV and return when it has been played (or failed).
    fn play(&self, wav: &[u8]) -> Result<(), String>;
}

/// The output chain of Settings -> Sound with the play command of Settings -> Voice: the same path the radio and the test chime
/// use (`audioout::route_args`, the software volume, `voice::run_capture`).
pub struct SystemOut {
    pub play_cmd: String,
}

impl CueOut for SystemOut {
    fn ready(&self) -> Result<(), String> {
        let sel = crate::audioout::global();
        if !sel.is_active() {
            return Ok(()); // not installed (another OS, a sound server): the default device
        }
        let st = sel.status();
        if st.choice.output.is_none() && !st.sound_server {
            return Err(st.choice.reason);
        }
        Ok(())
    }

    fn play(&self, wav: &[u8]) -> Result<(), String> {
        // the entry point decides: mixed over what plays (and ducking it), or the play command of Settings -> Voice
        crate::audiosink::play_wav_blocking(crate::mixer::SourceKind::Notification, "system sound", wav, &self.play_cmd, Duration::from_secs(5))
    }
}

/// A cue that is playing (or was skipped).
pub struct CueHandle {
    rx: Receiver<Outcome>,
    done: std::cell::RefCell<Option<Outcome>>,
}

impl CueHandle {
    /// A handle that already has its answer.
    pub fn finished(o: Outcome) -> CueHandle {
        let (tx, rx) = channel();
        let _ = tx.send(o);
        CueHandle { rx, done: Default::default() }
    }

    /// Wait up to `max` for the end of the cue. None: it is still playing (the caller goes on; the sound finishes by itself).
    pub fn wait(&self, max: Duration) -> Option<Outcome> {
        if let Some(o) = self.done.borrow().clone() {
            return Some(o);
        }
        let o = self.rx.recv_timeout(max).ok()?;
        *self.done.borrow_mut() = Some(o.clone());
        Some(o)
    }
}

fn run(cue: Cue, offset_db: i32, muted: bool, out: Arc<dyn CueOut>) -> CueHandle {
    if muted {
        return CueHandle::finished(Outcome::Off("Sound is muted in Settings -> Sound.".into()));
    }
    if let Err(why) = out.ready() {
        return CueHandle::finished(Outcome::NoOutput(why));
    }
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("iyakki-cue".into()).spawn(move || {
        let r = out.play(&wav(cue, offset_db));
        let _ = tx.send(match r {
            Ok(()) => Outcome::Played,
            Err(e) => Outcome::Failed(e),
        });
    });
    match spawned {
        Ok(_) => CueHandle { rx, done: Default::default() },
        Err(e) => CueHandle::finished(Outcome::Failed(e.to_string())),
    }
}

/// Start `cue` if the settings allow it: nothing is made or played when the master switch or the cue's own switch is off, or
/// when sound is muted. Returns at once.
pub fn start(sounds: &SystemSounds, sound: &SoundSettings, cue: Cue, out: Arc<dyn CueOut>) -> CueHandle {
    if !sounds.allows(cue) {
        return CueHandle::finished(Outcome::Off("System sounds are off.".into()));
    }
    run(cue, sounds.offset_db, sound.muted, out)
}

/// The same for the Preview button: the switches do not matter (the person asked to hear it), mute and a missing output do.
pub fn preview(sounds: &SystemSounds, sound: &SoundSettings, cue: Cue, out: Arc<dyn CueOut>) -> CueHandle {
    run(cue, sounds.offset_db, sound.muted, out)
}

impl crate::services::Services {
    /// The real output for cues (the play command of Settings -> Voice through the chain of Settings -> Sound).
    pub fn cue_out(&self) -> Arc<dyn CueOut> {
        Arc::new(SystemOut { play_cmd: self.voice_settings().play_cmd })
    }

    /// System sounds as set in Settings -> System sounds.
    pub fn system_sounds(&self) -> SystemSounds {
        self.settings.lock().unwrap().get().sounds.clone()
    }

    /// Change Settings -> System sounds (callers check that the person may).
    pub fn update_system_sounds(&self, f: impl FnOnce(&mut SystemSounds)) -> Result<SystemSounds, String> {
        self.settings.lock().unwrap().update(|s| f(&mut s.sounds)).map_err(|e| e.to_string())?;
        Ok(self.system_sounds())
    }

    /// Play a cue now (asynchronously) on `out`.
    pub fn play_cue(&self, cue: Cue, out: Arc<dyn CueOut>) -> CueHandle {
        start(&self.system_sounds(), &self.sound_settings(), cue, out)
    }

    /// Preview a cue (the switches are ignored) on `out`; waits for the end (up to 5 s) and says what happened.
    pub fn preview_cue(&self, cue: Cue, out: Arc<dyn CueOut>) -> Result<String, String> {
        let h = preview(&self.system_sounds(), &self.sound_settings(), cue, out);
        match h.wait(Duration::from_secs(5)) {
            Some(o) => o.say(cue),
            None => Err("The sound did not finish in time.".into()),
        }
    }
}

#[cfg(test)]
#[path = "syssounds_tests.rs"]
mod tests;
