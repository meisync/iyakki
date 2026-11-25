//! Types moved out of `iyakki_agent::voicedesk` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::time::Duration;


/// The most characters the text box takes.
pub const MAX_TEXT_CHARS: usize = 20_000;

/// The longest recording (seconds).
pub const MAX_RECORD_SECS: usize = 300;

/// A sound input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mic {
    /// The ALSA device (`plughw:1,0`).
    pub id: String,
    pub label: String,
    pub usb: bool,
}

/// The T2 sound card's microphones: not there, there but hidden on purpose, or allowed by an administrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T2Mic {
    Absent,
    Off,
    On,
}

/// How long a pause after speech ends a dictation, in milliseconds.
pub const PAUSE_END_MS: u64 = 1500;

/// Ends a dictation by itself when the person stops talking: it watches the microphone level (0.0-1.0, from `Recorder::level`), learns how
/// loud the room is (the quietest recent level), and calls speech only what is clearly above that **and lasts**: a click, a cough, a key
/// press or a bump (a spike of a few tenths of a second) is not talking. Talking needs about a quarter of a second of loud frames (a leaky
/// count, so the little dips between words do not end it); a pause is [`PAUSE_END_MS`] (or the person's own) since the last frame of
/// confirmed talking. Nothing is decided before talking: waiting for the person to start never ends it. Time is given in, so a test needs no
/// clock.
#[derive(Clone, Debug, Default)]
pub struct PauseDetector {
    pub floor: Option<f32>,
    pub heard: bool,
    pub last_talk_ms: u64,
    pub last_tick_ms: u64,
    /// The leaky count of loud time, in ms (up while loud, down while quiet, 0 to [`TALK_CAP_MS`]).
    pub loud_ms: u64,
    pub pause_ms: u64,
}

/// How much loud time makes it talking, and the most the count can hold (so it drains again within about 0.15 s).
pub const TALK_MIN_MS: u64 = 250;

pub const TALK_CAP_MS: u64 = 400;

/// The quietest level that can be speech at all, whatever the room.
pub const SPEECH_MIN_LEVEL: f32 = 0.15;

impl PauseDetector {
    /// With the default pause ([`PAUSE_END_MS`]).
    pub fn new() -> PauseDetector {
        PauseDetector::with_pause(PAUSE_END_MS)
    }

    /// With the person's own pause, in milliseconds.
    pub fn with_pause(ms: u64) -> PauseDetector {
        PauseDetector { pause_ms: ms.clamp(500, 4000), ..PauseDetector::default() }
    }

    /// The detector for the person's settings, or `None` when automatic ending is off.
    pub fn from_auto(auto_stop: bool, pause_ms: u32) -> Option<PauseDetector> {
        auto_stop.then(|| PauseDetector::with_pause(u64::from(pause_ms)))
    }

    /// Talking has been confirmed at least once (a pause can now end it).
    pub fn has_heard(&self) -> bool {
        self.heard
    }

    /// One look at the level at `now_ms`; true when the dictation should end now.
    pub fn tick(&mut self, level: f32, now_ms: u64) -> bool {
        let dt = if self.last_tick_ms == 0 { 100 } else { now_ms.saturating_sub(self.last_tick_ms).clamp(1, 500) };
        self.last_tick_ms = now_ms.max(1);
        // the room: follows a lower level at once and a higher one slowly, and only from moments that are not loud (so a long sentence
        // never becomes the floor and ends itself); a room that stays loud for many seconds raises it a little even so
        let floor = self.floor.unwrap_or(level);
        let loud = level > (floor * 2.5 + 0.05).max(SPEECH_MIN_LEVEL);
        self.floor = Some(if !loud {
            if level < floor { level } else { floor + (level - floor) * 0.05 }
        } else {
            floor + (level - floor) * 0.002
        });
        self.loud_ms = if loud { (self.loud_ms + dt).min(TALK_CAP_MS) } else { self.loud_ms.saturating_sub(dt) };
        if loud && self.loud_ms >= TALK_MIN_MS {
            self.heard = true;
            self.last_talk_ms = now_ms;
        }
        self.heard && now_ms.saturating_sub(self.last_talk_ms) >= self.pause_ms
    }
}

/// The microphone to use: the person's choice (`VoiceSettings::mic`) when that device is still there, else the first one found (the list is
/// ordered by card and device, the built-in microphone before a USB one; loopbacks and digital outputs are never in it).
pub fn pick_mic<'a>(mics: &'a [Mic], chosen: &str) -> Option<&'a Mic> {
    if !chosen.is_empty() {
        if let Some(m) = mics.iter().find(|m| m.id == chosen) {
            return Some(m);
        }
    }
    mics.first()
}

/// A recording in progress.
pub trait Capture: Send {
    /// Loudness of the last moment, 0 to 1 (the level meter).
    fn level(&self) -> f32;
    fn elapsed(&self) -> Duration;
    /// The longest recording was reached.
    fn is_full(&self) -> bool;
    /// The recording program is still running (it ends early when the device is missing or busy).
    fn is_alive(&mut self) -> bool;
    /// Stop and give the samples (16 kHz mono). Nothing is written to disk.
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String>;
    /// The samples captured since the last call, forgotten at once (for the wake listener, which reads small frames and keeps
    /// nothing). A capture that cannot stream returns nothing.
    fn drain(&mut self) -> Vec<i16> {
        Vec::new()
    }
}

/// Finds microphones and starts recordings (a trait so tests record nothing for real).
pub trait Recorders: Send + Sync {
    fn mics(&self) -> Vec<Mic>;
    fn product(&self) -> String;
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String>;
}

/// A sound that is playing: ask whether it still is, wait for it, stop it.
pub trait Playback: Send {
    fn is_playing(&mut self) -> bool;
    /// Wait for the end; `false` when `timeout` ran out first.
    fn wait(&mut self, timeout: Duration) -> bool;
    fn stop(&mut self);
    /// How much of the sound was heard, ms (`None` when the player reports nothing).
    fn played_ms(&self) -> Option<u64> {
        None
    }
}

/// Starts a sound on the speaker (a trait so tests play nothing for real).
pub trait Players: Send + Sync {
    /// Play `wav` with the speaker command of Settings -> Voice (empty: the mixer).
    fn play(&self, cmd: &str, wav: &[u8]) -> Result<Box<dyn Playback>, String>;
}

/// Test doubles for the recording and playing traits: scripted microphones and audio, every call recorded (F-SPLIT-03).
pub mod fakes {
    use super::{Capture, Duration, Mic, Playback, Players, Recorders};
    use std::sync::{Arc, Mutex};

    /// A capture that hands out the samples it was given.
    pub struct FakeCapture {
        pub samples: Vec<i16>,
        pub level: f32,
    }

    impl Capture for FakeCapture {
        fn level(&self) -> f32 {
            self.level
        }
        fn elapsed(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn is_full(&self) -> bool {
            false
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
            Ok(self.samples)
        }
    }

    /// Microphones and recordings a test scripts; `calls` says what was asked.
    #[derive(Default)]
    pub struct FakeRecorders {
        pub mics: Vec<Mic>,
        pub audio: Vec<i16>,
        pub calls: Mutex<Vec<String>>,
    }

    impl Recorders for FakeRecorders {
        fn mics(&self) -> Vec<Mic> {
            self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push("mics".into());
            self.mics.clone()
        }
        fn product(&self) -> String {
            "Fake machine".into()
        }
        fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String> {
            self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(format!("start({cmd},{max_secs})"));
            Ok(Box::new(FakeCapture { samples: self.audio.clone(), level: 0.5 }))
        }
    }

    /// A speaker that plays nothing and says how long it was told to play.
    #[derive(Default)]
    pub struct FakePlayers {
        pub played: Arc<Mutex<Vec<(String, usize)>>>,
    }

    struct Done;

    impl Playback for Done {
        fn is_playing(&mut self) -> bool {
            false
        }
        fn wait(&mut self, _t: Duration) -> bool {
            true
        }
        fn stop(&mut self) {}
    }

    impl Players for FakePlayers {
        fn play(&self, cmd: &str, wav: &[u8]) -> Result<Box<dyn Playback>, String> {
            self.played.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((cmd.to_string(), wav.len()));
            Ok(Box::new(Done))
        }
    }
}

#[cfg(test)]
mod recorder_fake_tests {
    use super::fakes::*;
    use super::*;

    #[test]
    fn the_fake_recorders_hand_out_scripted_audio_and_record_what_was_asked() {
        let r = FakeRecorders { mics: vec![Mic { id: "plughw:1,0".into(), label: "USB".into(), usb: true }], audio: vec![1, 2, 3], ..Default::default() };
        assert_eq!(pick_mic(&r.mics(), "").map(|m| m.id.as_str()), Some("plughw:1,0"));
        assert_eq!(pick_mic(&r.mics(), "gone").map(|m| m.id.as_str()), Some("plughw:1,0"), "a device that is gone falls back to the first");
        let c = r.start("arecord -q", 5).unwrap();
        assert_eq!(c.stop().unwrap(), vec![1, 2, 3]);
        assert_eq!(r.calls.lock().unwrap().as_slice(), ["mics", "mics", "start(arecord -q,5)"]);
    }

    #[test]
    fn the_fake_speaker_plays_nothing_and_remembers_the_command_and_size() {
        let p = FakePlayers::default();
        let mut h = p.play("aplay", &[0u8; 44]).unwrap();
        assert!(!h.is_playing() && h.wait(Duration::from_secs(1)));
        assert_eq!(p.played.lock().unwrap().as_slice(), [("aplay".to_string(), 44)]);
    }

    #[test]
    fn a_pause_ends_a_dictation_only_after_talking() {
        let mut d = PauseDetector::with_pause(600);
        let mut t = 1;
        for _ in 0..30 {
            assert!(!d.tick(0.01, t), "silence before any talking ends nothing");
            t += 100;
        }
        assert!(!d.has_heard());
    }
}

// ---- moved from iyakki_agent::voicedesk (F-SPLIT-03) ----

use std::path::Path;
use iyakki_speechtime::Locality;
use crate::voice::SpeechEngine;

pub const SPEED_MIN: f32 = 0.7;
pub const SPEED_MAX: f32 = 1.5;

// ------------------------------------------------------------------------------------------------------------ voices

/// A voice to read with.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReadVoice {
    /// Stable id (the choice is remembered by it).
    pub id: String,
    pub label: String,
    /// Where it comes from, for the list: "Kokoro (lemonade-local)", "this computer", ...
    pub source: String,
    pub engine: SpeechEngine,
    /// The voice name sent to the server, when the engine takes one.
    pub voice: Option<String>,
    /// The engine reads English only (Kokoro's voices).
    pub english_only: bool,
}

impl ReadVoice {
    pub fn takes_speed(&self) -> bool {
        crate::voice::engine_takes_speed(&self.engine)
    }
}


/// The address a speech engine sends to (None: it runs on this computer and sends nowhere).
pub fn engine_url(e: &SpeechEngine) -> Option<String> {
    match e {
        SpeechEngine::Http { url, .. } => Some(url.clone()),
        _ => None,
    }
}

/// The reading voices that can be used now: the one set in Settings -> Voice, the voices of every Lemonade text-to-speech model
/// (Kokoro's usual voices: Lemonade has no call that lists them), other providers' text-to-speech models, and the speech engines

// ------------------------------------------------------------------------------------------------------------ where it goes

/// Where an engine's server is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Dest {
    /// What the person calls it: the provider's id when Settings knows the address, else the host.
    pub name: String,
    /// `host` or `host:port`.
    pub address: String,
    /// `scheme://host:port`, what a yes is stored for.
    pub server: String,
    pub place: Locality,
}

impl Dest {
    /// "this computer", "your network" or "the internet".
    pub fn place_words(&self) -> &'static str {
        match self.place {
            Locality::ThisComputer => "this computer",
            Locality::Network => "your network",
            Locality::Internet => "the internet",
        }
    }
}

/// Where `engine` sends (None: nothing leaves this computer).

pub fn voice_key(v: &ReadVoice, speed: f32) -> String {
    format!("{}:{:.2}", v.id, speed)
}


// ------------------------------------------------------------------------------------------------------------ language hint

/// What language the text looks like, from its letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub name: &'static str,
    /// BCP 47 tag.
    pub tag: &'static str,
}

/// The scripts (and so languages) the Voice tile knows: first code point, last code point, name, BCP 47 tag.
const SCRIPTS: [(u32, u32, &str, &str); 13] = [
    (0x0B80, 0x0BFF, "Tamil", "ta-IN"),
    (0x0C00, 0x0C7F, "Telugu", "te-IN"),
    (0x0C80, 0x0CFF, "Kannada", "kn-IN"),
    (0x0D00, 0x0D7F, "Malayalam", "ml-IN"),
    (0x0900, 0x097F, "Hindi", "hi-IN"),
    (0x0980, 0x09FF, "Bengali", "bn-IN"),
    (0x0A80, 0x0AFF, "Gujarati", "gu-IN"),
    (0x0A00, 0x0A7F, "Punjabi", "pa-IN"),
    (0x0600, 0x06FF, "Arabic", "ar-SA"),
    (0x3040, 0x30FF, "Japanese", "ja-JP"),
    (0x4E00, 0x9FFF, "Chinese", "zh-CN"),
    (0x0400, 0x04FF, "Russian", "ru-RU"),
    (0x0000, 0x0000, "English", "en-US"),
];

/// The names of the languages the Voice tile knows (Translate in the transcript's assistant offers them).
pub fn language_names() -> Vec<&'static str> {
    SCRIPTS.iter().map(|x| x.2).collect()
}

/// The language of `text` by script (None when there are too few letters). Latin letters count as English (a hint, not a test).
pub fn detect_language(text: &str) -> Option<Hint> {
    let mut counts = [0usize; 13];
    let mut letters = 0usize;
    for c in text.chars().take(4000).filter(|c| c.is_alphabetic()) {
        letters += 1;
        let cp = c as u32;
        match SCRIPTS.iter().position(|(a, b, _, _)| *b != 0 && cp >= *a && cp <= *b) {
            Some(i) => counts[i] += 1,
            None => counts[12] += 1,
        }
    }
    if letters < 8 {
        return None;
    }
    let (i, n) = counts.iter().enumerate().max_by_key(|(_, n)| **n)?;
    (*n * 10 >= letters * 3).then(|| Hint { name: SCRIPTS[i].2, tag: SCRIPTS[i].3 })
}

/// A warning when the chosen voice cannot read the language the text looks like (None: fine, or unknown).
pub fn language_warning(text: &str, v: Option<&ReadVoice>) -> Option<String> {
    let h = detect_language(text)?;
    let v = v?;
    (v.english_only && h.tag != "en-US").then(|| format!("This text looks like {}. {} reads English only: pick another voice for {}.", h.name, v.label.split(" · ").next().unwrap_or("This voice"), h.name))
}

// ------------------------------------------------------------------------------------------------------------ microphones


/// The inputs listed by `/proc/asound/cards` and `/proc/asound/pcm` (every device with a capture stream).
pub fn parse_mics(cards: &str, pcm: &str) -> Vec<Mic> {
    parse_mics_with(cards, pcm, false)
}

/// The sound card of a T2 Mac (apple-bce's `aaudio`, driver name `AppleT2x4`...): opening its capture streams (the microphones, the
/// "Bridge Loopback") while something played hung and reset a MacBookPro16,2, so its inputs are hidden unless the kernel module
/// parameter `capture` is on (`allow_t2`, see [`t2_capture_allowed`]).
pub fn t2_card_numbers(cards: &str) -> Vec<u32> {
    cards.lines().filter_map(|l| {
        let (n, rest) = l.trim_start().split_once(' ')?;
        let n = n.parse::<u32>().ok()?;
        rest.contains("]: AppleT2").then_some(n)
    }).collect()
}

/// Whether an administrator turned the T2 capture streams on (`echo 1 > /sys/module/apple_bce/parameters/capture`).
pub fn t2_capture_allowed(sys_root: &Path) -> bool {
    std::fs::read_to_string(sys_root.join("module/apple_bce/parameters/capture")).map(|v| matches!(v.trim(), "Y" | "1")).unwrap_or(false)
}

pub fn parse_mics_with(cards: &str, pcm: &str, allow_t2: bool) -> Vec<Mic> {
    let t2 = if allow_t2 { Vec::new() } else { t2_card_numbers(cards) };
    // " 1 [Device        ]: USB-Audio - USB PnP Sound Device"
    let mut usb = std::collections::HashSet::new();
    for l in cards.lines() {
        let t = l.trim_start();
        if let Some((n, rest)) = t.split_once(' ') {
            if let Ok(n) = n.parse::<u32>() {
                if rest.contains("USB-Audio") {
                    usb.insert(n);
                }
            }
        }
    }
    let mut out = Vec::new();
    for l in pcm.lines() {
        // "01-00: USB Audio : USB Audio : playback 1 : capture 1"
        let Some((ids, rest)) = l.split_once(':') else { continue };
        let Some((c, d)) = ids.trim().split_once('-') else { continue };
        let (Ok(c), Ok(d)) = (c.parse::<u32>(), d.parse::<u32>()) else { continue };
        if t2.contains(&c) {
            continue;
        }
        let parts: Vec<&str> = rest.split(':').map(str::trim).collect();
        if !parts.iter().any(|p| p.starts_with("capture")) {
            continue;
        }
        let name = parts.first().copied().filter(|n| !n.is_empty()).unwrap_or("Microphone");
        // not microphones: loopbacks (the T2 "Bridge Loopback", snd-aloop) and digital outputs read back (IEC958, S/PDIF)
        let low = rest.to_ascii_lowercase();
        if ["loopback", "bridge", "iec958", "spdif", "s/pdif", "hdmi", "displayport"].iter().any(|w| low.contains(w)) {
            continue;
        }
        let is_usb = usb.contains(&c);
        out.push(Mic { id: format!("plughw:{c},{d}"), label: if is_usb { format!("{name} (USB)") } else { name.to_string() }, usb: is_usb });
    }
    out
}

/// The microphones of this computer.
pub fn list_mics(proc_root: &Path) -> Vec<Mic> {
    let rd = |f: &str| std::fs::read_to_string(proc_root.join("asound").join(f)).unwrap_or_default();
    parse_mics_with(&rd("cards"), &rd("pcm"), t2_capture_allowed(&proc_root.parent().unwrap_or(Path::new("/")).join("sys")))
}


pub fn t2_mic(proc_root: &Path) -> T2Mic {
    let cards = std::fs::read_to_string(proc_root.join("asound/cards")).unwrap_or_default();
    if t2_card_numbers(&cards).is_empty() {
        T2Mic::Absent
    } else if t2_capture_allowed(&proc_root.parent().unwrap_or(Path::new("/")).join("sys")) {
        T2Mic::On
    } else {
        T2Mic::Off
    }
}

/// Allow (or forbid again) recording from the T2 sound card (`apple_bce` parameter `capture`). Lasts until the next restart. `Err` is one
/// plain line, for a computer without that driver parameter.
pub fn set_t2_capture(proc_root: &Path, on: bool) -> Result<(), String> {
    let p = proc_root.parent().unwrap_or(Path::new("/")).join("sys/module/apple_bce/parameters/capture");
    if !p.exists() {
        return Err("this system's sound driver has no switch for the built-in microphone".into());
    }
    std::fs::write(&p, if on { "1\n" } else { "0\n" }).map_err(|e| format!("could not change it: {e}"))
}

/// Whether this computer has a T2 sound card whose microphones are hidden (so the Voice tile can say why none was found).
pub fn t2_mic_hidden(proc_root: &Path) -> bool {
    let cards = std::fs::read_to_string(proc_root.join("asound/cards")).unwrap_or_default();
    !t2_card_numbers(&cards).is_empty() && !t2_capture_allowed(&proc_root.parent().unwrap_or(Path::new("/")).join("sys"))
}

/// The record command aimed at one microphone: `-D plughw:1,0` after `arecord`, unless the command names a device itself or is another program.
/// `pcm_for` says the device string of a card the capture formats were learned for (`iyakkimic0d0`), or `None` (the id is used).
pub fn record_cmd_for_with(base: &str, mic: Option<&Mic>, pcm_for: &dyn Fn(&str) -> Option<String>) -> String {
    let Some(m) = mic else { return base.to_string() };
    let Ok(args) = crate::voice::split_command(base) else { return base.to_string() };
    // only ALSA's `arecord` takes -D; a custom recorder (a script, parecord, ffmpeg) is left exactly as the person wrote it
    if args.is_empty() || args[0].rsplit('/').next() != Some("arecord") || crate::pcm::names_device(&args) {
        return base.to_string();
    }
    record_cmd_for_device(&args, &pcm_for(&m.id).unwrap_or_else(|| m.id.clone()))
}

/// `arecord` aimed at `device` (`plughw:0,0`, or the capture PCM with the format named, `iyakkimic0d0`, where the card needs it).
pub fn record_cmd_for_device(args: &[String], device: &str) -> String {
    let mut a = args.to_vec();
    a.insert(1, device.to_string());
    a.insert(1, "-D".to_string());
    crate::pcm::join_command(&a)
}


/// File names shared with the web engine's microphone plugin (`dist/webview/wpe-helper/micscan.h`): the shell writes the choice, the plugin
/// writes why a microphone could not be opened.
pub const BROWSER_MIC_CHOICE: &str = "iyakki-browser-mic";
pub const BROWSER_MIC_STATUS: &str = "iyakki-browser-mic-status";


/// Hand the person's choice to the web engine (`dir` is the shared memory folder; an empty choice removes the file: automatic).
pub fn share_mic_choice(dir: &Path, id: &str) {
    let f = dir.join(BROWSER_MIC_CHOICE);
    if id.is_empty() {
        let _ = std::fs::remove_file(&f);
    } else {
        let _ = std::fs::write(&f, format!("{id}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644));
        }
    }
}

/// What the plugin last reported: (device name, reason), when a microphone failed and has not delivered sound since.
pub fn browser_mic_failure(dir: &Path) -> Option<(String, String)> {
    let t = std::fs::read_to_string(dir.join(BROWSER_MIC_STATUS)).ok()?;
    let (name, why) = t.trim().split_once('\t')?;
    (!name.is_empty()).then(|| (name.to_string(), why.trim().to_string()))
}

/// The reason in plain words, with the device name.
pub fn mic_failure_text(name: &str, why: &str) -> String {
    let w = why.to_ascii_lowercase();
    let cause = if w.contains("busy") {
        "another program is using it".to_string()
    } else if w.contains("no such") || w.contains("not found") || w.contains("enodev") || w.contains("no device") {
        "the computer does not offer it (the sound driver may have it switched off)".to_string()
    } else if w.contains("permission") || w.contains("denied") {
        "this account may not use it".to_string()
    } else if w.contains("rate") || w.contains("negotiat") {
        "it does not take the sound format the page asked for".to_string()
    } else {
        format!("the sound system said: {}", why.trim().trim_end_matches('.'))
    };
    format!("The microphone {name} could not be used: {cause}. Choose another microphone in the question or in the Voice tile.")
}

/// What to tell the person when no microphone was found, knowing that a T2 sound card's microphones are switched off and whether the
/// machine's model is one whose built-in microphone has no driver (`cs8409`).
pub fn no_mic_text_with(product: &str, t2_hidden: bool, cs8409: bool) -> String {
    if t2_hidden {
        return format!("The microphone of this {} is not supported yet: the T2 chip's sound driver can hang the computer when it records, so it is switched off. A USB microphone or a headset with a USB adapter works. (An administrator can try the built-in one: echo 1 > /sys/module/apple_bce/parameters/capture, at their own risk.)", product.trim());
    }
    let mut t = "No microphone found. USB microphones work: plug one in and it appears here.".to_string();
    if cs8409 {
        t.push_str(&format!(" The built-in microphone of this {} is not supported yet (its sound chip has no driver in IYAKKI), so use a USB microphone or a headset with a USB adapter.", product.trim()));
    } else {
        t.push_str(" The built-in microphones of some laptops are not supported yet (the MacBookPro14,1 microphone has no driver).");
    }
    t
}

/// The machine's model (`/sys/class/dmi/id/product_name`).
pub fn product_name(sys_root: &Path) -> String {
    std::fs::read_to_string(sys_root.join("class/dmi/id/product_name")).unwrap_or_default().trim().to_string()
}



pub const NO_SPEECH: &str = "No speech was recognised.";
/// What the recorder side says when the level never rose above silence.
pub const ONLY_SILENCE: &str = "I only heard silence. Check the microphone and its volume.";

/// A result that is "no words" (noise, silence), not a failure of the microphone or the engine.
pub fn is_no_words(e: &str) -> bool {
    e == NO_SPEECH || e == ONLY_SILENCE
}



/// Where the kernel's process and sound tables live (a function so tiles ask core instead of naming a kernel path themselves).
pub fn proc_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/proc")
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn the_record_command_is_aimed_only_for_arecord_and_asks_the_given_device_lookup() {
        let mic = Mic { id: "plughw:1,0".into(), label: "USB".into(), usb: true };
        assert_eq!(record_cmd_for_with("arecord -f S16_LE", Some(&mic), &|_| None), "arecord -D plughw:1,0 -f S16_LE");
        assert_eq!(record_cmd_for_with("arecord -f S16_LE", Some(&mic), &|id| Some(format!("named-{id}"))), "arecord -D named-plughw:1,0 -f S16_LE");
        assert_eq!(record_cmd_for_with("parecord x", Some(&mic), &|_| None), "parecord x", "another program is left as written");
        assert_eq!(record_cmd_for_with("arecord", None, &|_| None), "arecord");
    }

    #[test]
    fn the_no_microphone_words_depend_on_the_t2_card_and_the_known_chip() {
        assert!(no_mic_text_with("MacBookPro14,1", true, false).contains("T2 chip"));
        assert!(no_mic_text_with("MacBookPro14,1", false, true).contains("no driver in IYAKKI"));
        let plain = no_mic_text_with("Lenovo", false, false);
        assert!(plain.starts_with("No microphone found.") && plain.contains("some laptops"));
    }
}
