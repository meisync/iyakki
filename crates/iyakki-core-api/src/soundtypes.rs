//! Sound output types the UI and the core share (F-SPLIT-03): the Settings -> Sound data, one playback device and the choice between devices.
//! Pure data and rules; the agent keeps the scanning of the hardware. Moved here from `iyakki-shared` so the core facet [`crate::soundctl`] can use them.


use serde::Deserialize;
use serde::Serialize;

/// The largest software boost, in dB (Settings -> Sound -> Advanced -> Boost). More than this only distorts.
pub const MAX_BOOST_DB: u32 = 12;

/// What is said when there is nothing to play on.
pub const NO_OUTPUT: &str = "no sound output: connect a monitor with speakers over HDMI/DP or a USB audio device; built-in speakers on T2 Macs need the apple-bce driver of the Mac image";

/// What Settings -> Sound says while the card starts.
pub const STARTING_LINE: &str = "waiting for the sound card (the built-in speakers of this Mac start up slowly)";

/// Which volume control moves the sound of a card that has a hardware one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeMode {
    /// The hardware control, except on codecs known to have a control that does nothing ([`hw_volume_is_dummy`]).
    #[default]
    Automatic,
    /// Always the card's hardware control.
    Hardware,
    /// Always IYAKKI's software volume (ALSA softvol); the card's own control is left alone.
    Software,
}

impl VolumeMode {
    pub fn label(self) -> &'static str {
        match self {
            VolumeMode::Automatic => "Automatic",
            VolumeMode::Hardware => "Hardware",
            VolumeMode::Software => "Software",
        }
    }

    /// The next mode in the order Automatic, Hardware, Software.
    pub fn next(self) -> VolumeMode {
        match self {
            VolumeMode::Automatic => VolumeMode::Hardware,
            VolumeMode::Hardware => VolumeMode::Software,
            VolumeMode::Software => VolumeMode::Automatic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Built-in speakers, headphones, line out.
    Analog,
    /// A monitor or TV over HDMI or DisplayPort.
    Hdmi,
    Usb,
    Bluetooth,
    /// S/PDIF and other digital outputs.
    Digital,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Analog => "speakers/headphones",
            Kind::Hdmi => "HDMI/DP",
            Kind::Usb => "USB",
            Kind::Bluetooth => "Bluetooth",
            Kind::Digital => "digital",
        }
    }
}

/// How often the background watcher takes a full look at the sound hardware (jacks and monitors are in that look).
/// The loudness a new system starts at (the IYAKKI volume), in percent: comfortable, with headroom for the person to go up.
pub const DEFAULT_VOLUME_PCT: u32 = 70;

/// The software boost every card starts with, in dB: a little up (20 % of [`MAX_BOOST_DB`], rounded). Settings -> Sound -> Advanced changes it.
pub const DEFAULT_BOOST_DB: u32 = 2;

/// Settings -> Sound (system-wide, changed by administrators, kept in the settings file).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    /// The chosen output ([`Output::key`], e.g. `PCH:3`); empty = automatic.
    pub output: String,
    /// IYAKKI's own volume, 0 to 100 (applied to the sound IYAKKI plays).
    pub volume_pct: u32,
    pub muted: bool,
    /// The last volume of each output ([`Output::key`] → percent), restored when that output is used again.
    pub levels: std::collections::BTreeMap<String, u32>,
    /// A software boost in dB for a card (card id → 0 to [`MAX_BOOST_DB`]), for codecs whose fixed amplifier
    /// gain is low (Settings → Sound → Advanced → Boost). It clips, so it may distort; 0 (absent) is the normal case.
    pub boosts: std::collections::BTreeMap<String, u32>,
    /// Which volume a card uses (card id → mode); absent = Automatic. Set from Settings -> Sound -> Advanced.
    pub volume_modes: std::collections::BTreeMap<String, VolumeMode>,
    /// Mix sounds with the IYAKKI mixer (`sound.mixer`, see `docs/development/AUDIO-MIXER.md`). On by default (owner decision 2026-10-28); the person can switch it off in Settings -> Sound.
    pub mixer: bool,
    /// (experimental) the web engine's sound goes through the mixer too (`sound.web_mix`, [`crate::webmix`]). Needs `mixer`. Off until tested.
    #[serde(default)]
    pub web_mix: bool,
    /// Moves the highlight of read-aloud words against the heard position of the sound, ms, -500 to 500 (positive: later). 0 is
    /// the mixer's own model; the person tunes what the card adds that nothing can measure (see [`crate::audiosink::highlight_offset_ms`]).
    #[serde(default)]
    pub highlight_offset_ms: i32,
    /// The person's level for each kind of sound (source name such as `radio` or `Browser` → percent 0 to 100), set in the speaker
    /// dropdown; it multiplies the master volume. Absent is 100.
    #[serde(default)]
    pub source_levels: std::collections::BTreeMap<String, u32>,
}

/// At most this many outputs keep their own volume.
pub const MAX_LEVELS: usize = 32;

impl Default for SoundSettings {
    fn default() -> Self {
        SoundSettings { output: String::new(), volume_pct: DEFAULT_VOLUME_PCT, muted: false, levels: Default::default(), boosts: Default::default(), volume_modes: Default::default(), mixer: true, web_mix: false, highlight_offset_ms: 0, source_levels: Default::default() }
    }
}

pub fn output_key_ok(k: &str) -> bool {
    k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.'))
}

impl SoundSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.volume_pct > 100 {
            return Err("the volume must be between 0 and 100 percent".into());
        }
        if !output_key_ok(&self.output) {
            return Err("the sound output must be a card id and device number such as PCH:3".into());
        }
        if self.levels.len() > MAX_LEVELS || self.levels.iter().any(|(k, v)| k.is_empty() || !output_key_ok(k) || *v > 100) {
            return Err("the remembered output volumes are not valid".into());
        }
        if self.boosts.len() > MAX_LEVELS || self.boosts.iter().any(|(k, v)| k.is_empty() || !output_key_ok(k) || *v > MAX_BOOST_DB) {
            return Err(format!("the sound boost must be between 0 and {} dB", MAX_BOOST_DB));
        }
        if !(-500..=500).contains(&self.highlight_offset_ms) {
            return Err("the highlight timing offset must be between -500 and 500 ms".into());
        }
        if self.source_levels.len() > MAX_LEVELS || self.source_levels.iter().any(|(k, v)| k.is_empty() || k.len() > 32 || k.chars().any(|c| c.is_control()) || *v > 100) {
            return Err("the volumes of the sounds are not valid".into());
        }
        if self.volume_modes.len() > MAX_LEVELS || self.volume_modes.keys().any(|k| k.is_empty() || !output_key_ok(k)) {
            return Err("the remembered volume controls are not valid".into());
        }
        Ok(())
    }

    /// Which volume control this card uses.
    pub fn volume_mode_for(&self, card_id: &str) -> VolumeMode {
        self.volume_modes.get(card_id).copied().unwrap_or_default()
    }

    /// Set the volume control of a card; Automatic removes the entry.
    pub fn set_volume_mode(&mut self, card_id: &str, m: VolumeMode) {
        if m == VolumeMode::Automatic {
            self.volume_modes.remove(card_id);
        } else if output_key_ok(card_id) && !card_id.is_empty() && (self.volume_modes.len() < MAX_LEVELS || self.volume_modes.contains_key(card_id)) {
            self.volume_modes.insert(card_id.to_string(), m);
        }
    }

    /// The boost of this card in dB: what the person set (0 is a choice too), else [`DEFAULT_BOOST_DB`].
    pub fn boost_for(&self, card_id: &str) -> u32 {
        self.boosts.get(card_id).copied().unwrap_or(DEFAULT_BOOST_DB).min(MAX_BOOST_DB)
    }

    /// Set the boost of a card; 0 is kept as the person's choice of no boost (without an entry the default applies).
    pub fn set_boost(&mut self, card_id: &str, db: u32) {
        if output_key_ok(card_id) && !card_id.is_empty() && (self.boosts.len() < MAX_LEVELS || self.boosts.contains_key(card_id)) {
            self.boosts.insert(card_id.to_string(), db.min(MAX_BOOST_DB));
        }
    }

    /// Remember the volume now as the one of `output` (a key such as `Audio:0`).
    pub fn remember_level(&mut self, output: &str) {
        if output.is_empty() || !output_key_ok(output) || (self.levels.len() >= MAX_LEVELS && !self.levels.contains_key(output)) {
            return;
        }
        self.levels.insert(output.to_string(), self.volume_pct.min(100));
    }

    /// The factor IYAKKI applies, in percent (0 when muted).
    pub fn gain(&self) -> u32 {
        if self.muted { 0 } else { self.volume_pct.min(100) }
    }
}

/// One playback device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub card: u32,
    pub device: u32,
    /// The card's id (`PCH`), stable across boots unlike its number.
    pub card_id: String,
    pub card_name: String,
    pub pcm_name: String,
    pub kind: Kind,
    /// Something is plugged in: `Some(true)` on, `Some(false)` off, `None` when the hardware does not say.
    pub connected: Option<bool>,
    /// The monitor's name (from its ELD), for HDMI/DP.
    pub monitor: Option<String>,
    /// HDMI/DP: the monitor says it can play sound (its ELD lists audio formats).
    pub speakers: Option<bool>,
    /// HDMI/DP: 1 for the card's first HDMI device, 2 for the second, ... (0 for the others).
    pub hdmi_no: usize,
    /// The digital output's `IEC958 Playback Switch` is off in the mixer.
    pub digital_off: bool,
    /// A speaker built into the computer that is always there and has no jack (the T2 Mac's `Speaker` device).
    pub builtin_speaker: bool,
    /// The card's hardware volume control (`Master`, `PCM`, ...), None when it has none (or `amixer` did not answer).
    pub hw_volume: Option<String>,
    /// The card's hardware volume control is known to do nothing (the CS8409 of the Apple path, see [`hw_volume_is_dummy`]).
    pub dummy_volume: bool,
}

/// The card id of the one output a computer without ALSA (the macOS window) has: whatever the system plays on.
pub const SYSTEM_DEFAULT_ID: &str = "system-default";

impl Output {
    /// "System default output": the sound goes where the operating system sends it (macOS), so there is no list to choose from.
    pub fn system_default() -> Output {
        Output { card: 0, device: 0, card_id: SYSTEM_DEFAULT_ID.into(), card_name: "the system's own output".into(), pcm_name: String::new(), kind: Kind::Analog, connected: Some(true), monitor: None, speakers: None, hdmi_no: 0, digital_off: false, builtin_speaker: true, hw_volume: None, dummy_volume: false }
    }

    /// Does the output's volume come from the card's hardware control? (Else: IYAKKI's software volume.)
    pub fn uses_hw_volume(&self, mode: VolumeMode) -> bool {
        self.hw_volume.is_some()
            && match mode {
                VolumeMode::Hardware => true,
                VolumeMode::Software => false,
                VolumeMode::Automatic => !self.dummy_volume,
            }
    }

    /// How Settings remembers it: `<card id>:<device>`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.card_id, self.device)
    }

    /// The ALSA device `aplay -D` gets. `plughw` converts format, rate and channels for the hardware.
    pub fn alsa(&self) -> String {
        format!("plughw:{},{}", self.card, self.device)
    }

    /// "HDMI: PDM-15T", "HDMI 2 (no monitor)", "USB Audio: …", "Speakers/headphones (HDA Intel PCH)".
    pub fn label(&self) -> String {
        if self.card_id == SYSTEM_DEFAULT_ID {
            return "System default output".to_string();
        }
        match self.kind {
            Kind::Hdmi => match (&self.monitor, self.connected) {
                (Some(m), _) => format!("HDMI: {m}"),
                (None, Some(true)) => format!("HDMI {} (monitor connected)", self.hdmi_no),
                (None, Some(false)) => format!("HDMI {} (no monitor)", self.hdmi_no),
                (None, None) => format!("HDMI {}", self.hdmi_no),
            },
            Kind::Usb => format!("USB Audio: {}", self.card_name),
            Kind::Bluetooth => format!("Bluetooth: {}", self.card_name),
            Kind::Analog if self.builtin_speaker => format!("Built-in speaker ({})", self.card_name),
            Kind::Analog if is_headphone_pcm(&self.pcm_name) => format!("Headphones ({})", self.card_name),
            Kind::Analog => format!("Speakers/headphones ({})", self.card_name),
            Kind::Digital => format!("Digital out ({} · {})", self.card_name, self.pcm_name),
        }
    }

    /// What the hardware says about it, in a few words.
    pub fn state(&self) -> String {
        let s = match (self.kind, self.connected) {
            (Kind::Hdmi, Some(true)) if self.speakers == Some(false) => "monitor connected, it reports no speakers".to_string(),
            (Kind::Hdmi, Some(true)) => "monitor connected".to_string(),
            (Kind::Hdmi, Some(false)) => "no monitor".to_string(),
            (Kind::Analog, Some(true)) if self.builtin_speaker => "built-in speaker".to_string(),
            (Kind::Analog, Some(true)) => "speaker or headphones present".to_string(),
            (Kind::Analog, Some(false)) => "nothing plugged in".to_string(),
            (Kind::Usb | Kind::Bluetooth, _) => "plugged in".to_string(),
            (_, _) => "the card does not report what is connected".to_string(),
        };
        if self.digital_off { format!("{s} · digital output switched off in the mixer") } else { s }
    }
}

/// The T2's headphone jack is the PCM `Codec Output` (the speaker is `Speaker`).
fn is_headphone_pcm(pcm_name: &str) -> bool {
    let n = pcm_name.to_ascii_lowercase();
    n.contains("headphone") || n == "codec output"
}

/// The output IYAKKI plays on, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub output: Option<Output>,
    /// Chosen by the person (not automatic).
    pub chosen: bool,
    pub reason: String,
}

/// An HDMI/DP output whose pin reports no monitor (ELD not valid, jack off).
fn pin_is_dead(o: &Output) -> bool {
    o.kind == Kind::Hdmi && o.connected == Some(false)
}

/// Where an output stands in the automatic order (lower is better) and the reason, None when it is not usable automatically.
fn automatic_rank(o: &Output) -> Option<(u8, String)> {
    match (o.kind, o.connected) {
        // a speaker that is always there says nothing about the person's wish: a monitor, USB or Bluetooth device comes first
        (Kind::Analog, Some(true)) if o.builtin_speaker => Some((4, "automatic: the built-in speaker (no headphones, monitor with speakers or USB audio device is connected)".into())),
        (Kind::Analog, Some(true)) => Some((0, "automatic: speakers or headphones are connected".into())),
        (Kind::Hdmi, Some(true)) if o.speakers != Some(false) => Some((1, format!("automatic: {} is connected over HDMI/DP", o.monitor.as_deref().map_or("a monitor".to_string(), |m| format!("monitor {m}"))))),
        (Kind::Usb, _) => Some((2, "automatic: a USB audio device is plugged in".into())),
        (Kind::Bluetooth, _) => Some((3, "automatic: a Bluetooth audio device".into())),
        (Kind::Hdmi, Some(true)) => Some((5, format!("automatic: {} is connected over HDMI/DP but reports no speakers", o.monitor.as_deref().map_or("a monitor".to_string(), |m| format!("monitor {m}"))))),
        (Kind::Analog, None) => Some((6, "automatic: the card's analog output (it does not report its jacks)".into())),
        // some codecs report no speaker jack at all: their analog output is still better than silence
        (Kind::Analog, Some(false)) => Some((7, "automatic: the card's analog output (nothing is reported plugged in)".into())),
        _ => None,
    }
}

/// Pick the output: the chosen one (`chosen` = its [`Output::key`]) when it is there, else the best one connected.
pub fn choose(outs: &[Output], chosen: &str) -> Choice {
    let mut dead_pin = false;
    if !chosen.is_empty() {
        if let Some(o) = outs.iter().find(|o| o.key() == chosen) {
            // an HDMI/DP pin whose monitor is gone plays into nothing: the next automatic choice takes over until it is back
            // (when nothing else could play, the chosen one stays: there is nothing better to hear)
            if pin_is_dead(o) && outs.iter().any(|x| x.key() != chosen && automatic_rank(x).is_some()) {
                dead_pin = true;
            } else {
                return Choice { output: Some(o.clone()), chosen: true, reason: "chosen in Settings → Sound".into() };
            }
        }
    }
    let best = outs.iter().filter_map(|o| automatic_rank(o).map(|(r, why)| (r, why, o))).min_by_key(|(r, _, _)| *r);
    let missing = if chosen.is_empty() {
        String::new()
    } else if dead_pin {
        format!("the chosen output ({chosen}) has no monitor connected now, it comes back when one is; ")
    } else {
        format!("the chosen output ({chosen}) is not connected now; ")
    };
    match best {
        Some((_, why, o)) => Choice { output: Some(o.clone()), chosen: false, reason: format!("{missing}{why}") },
        None if outs.is_empty() => Choice { output: None, chosen: false, reason: format!("{missing}{NO_OUTPUT} (no sound card was found)") },
        None => Choice { output: None, chosen: false, reason: format!("{missing}{NO_OUTPUT}") },
    }
}

/// Counts the changes of the master volume (see [`Master::generation`]).
pub static MASTER_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one master volume of IYAKKI (Settings -> Sound, the volume keys, the Devices popover, the radio and media sliders all
/// read and change this). It lives in the settings file; the softvol control `IYAKKI` (or, without one, `gain()` for the
/// players that scale their samples) applies it, once per stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Master {
    pub percent: u32,
    pub muted: bool,
    /// Changes whenever the percent or the mute changes, so views can notice within a frame.
    pub generation: u64,
}

impl SoundSettings {
    /// The master volume these settings hold.
    pub fn master(&self) -> Master {
        Master { percent: self.volume_pct.min(100), muted: self.muted, generation: MASTER_GEN.load(std::sync::atomic::Ordering::SeqCst) }
    }
}


// ------------------------------------------------------------------------------------------------ system sounds (cues)

use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// The longest sign-out waits for its cue to end.
pub const LOGOUT_WAIT: Duration = Duration::from_millis(700);

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

/// A cue that is playing (or was skipped).
pub struct CueHandle {
    rx: Receiver<Outcome>,
    done: std::cell::RefCell<Option<Outcome>>,
}

impl CueHandle {
    /// A handle whose answer will come on `rx`.
    pub fn pending(rx: Receiver<Outcome>) -> CueHandle {
        CueHandle { rx, done: Default::default() }
    }

    /// A handle that already has its answer.
    pub fn finished(o: Outcome) -> CueHandle {
        let (tx, rx) = channel();
        let _ = tx.send(o);
        CueHandle { rx, done: Default::default() }
    }

    /// Wait for the end of the cue for at most [`LOGOUT_WAIT`] (what sign-out and the wake chime allow).
    pub fn wait_short(&self) -> Option<Outcome> {
        self.wait(LOGOUT_WAIT)
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

