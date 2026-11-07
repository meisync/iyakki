//! The logic behind the VOICE tile: which reading voices exist, where the text goes, how it is cut into pieces, the
//! microphones, and the recorder and speech-to-text calls. The screen (`iyakki-shell` `apps/voice*.rs`) only draws and clicks.
//!
//! Reading uses the same pieces as TALES and the chat (`story::Reader`, `voice::synthesize_with`); listening uses the
//! speech-to-text engine of Settings -> Voice, else a Lemonade whisper model. Text and sound go to another machine only after
//! the person said yes to that machine (`Services::allow_speech_server`, the decision Settings -> Models stores).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::lemonade::{self, ModelType};
use crate::services::Services;
use crate::speechtime::{self, Locality};
use crate::story::{self, Chunk};
use crate::voice::{self, SpeakOpts, SpeechEngine};

/// The most characters the text box takes.
pub const MAX_TEXT_CHARS: usize = 20_000;

/// The longest recording (seconds).
pub const MAX_RECORD_SECS: usize = 300;

/// The speed range of the slider.
pub const SPEED_MIN: f32 = 0.7;
pub const SPEED_MAX: f32 = 1.5;

// ------------------------------------------------------------------------------------------------------------ voices

/// A voice to read with.
#[derive(Clone, Debug, PartialEq)]
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
        story::engine_takes_speed(&self.engine)
    }
}

fn providers(svc: &Services) -> Vec<crate::providers::ProviderCfg> {
    svc.settings.lock().map(|s| s.get().providers.clone()).unwrap_or_default()
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
/// found on this computer.
pub fn read_voices(svc: &Services) -> Vec<ReadVoice> {
    let mut out: Vec<ReadVoice> = Vec::new();
    let push = |v: ReadVoice, out: &mut Vec<ReadVoice>| {
        if !out.iter().any(|o| o.id == v.id || (o.engine == v.engine && o.voice == v.voice)) {
            out.push(v);
        }
    };
    let set = svc.voice_settings().tts;
    if let Some(e) = set.clone().filter(|e| !matches!(e, SpeechEngine::Browser)) {
        let voice = match &e {
            SpeechEngine::Http { voice, .. } | SpeechEngine::Host { voice, .. } => voice.clone().filter(|v| !v.is_empty()),
            _ => None,
        };
        let english = matches!(&e, SpeechEngine::Http { model, .. } if model.to_ascii_lowercase().contains("kokoro"));
        let label = format!("{} (set in Settings -> Voice)", e.label());
        push(ReadVoice { id: "settings".into(), label, source: "Settings -> Voice".into(), engine: e, voice, english_only: english }, &mut out);
    }
    let all = providers(svc);
    for (prov, model) in svc.lemonade_models(ModelType::TextToSpeech) {
        let Some(p) = all.iter().find(|p| p.id == prov) else { continue };
        let Ok(base) = p.base() else { continue };
        let kokoro = model.to_ascii_lowercase().contains("kokoro");
        for v in lemonade::voices_for(&model) {
            let engine = lemonade::tts_engine(&base, &model, Some(v), p.key_ref.clone());
            let name = if kokoro { "Kokoro".to_string() } else { model.clone() };
            push(ReadVoice { id: format!("lemonade:{prov}:{model}:{v}"), label: format!("{name} · {v}"), source: format!("{name} on {prov}"), engine, voice: Some(v.to_string()), english_only: kokoro }, &mut out);
        }
    }
    for p in all.iter().filter(|p| p.enabled && p.kind != crate::providers::Kind::Lemonade) {
        let Ok(base) = p.base() else { continue };
        for (m, _) in p.model_kinds.iter().filter(|(_, k)| k.as_str() == "tts") {
            let probe = SpeechEngine::Http { url: format!("{base}/audio/speech"), model: m.clone(), key_ref: p.key_ref.clone(), voice: None, language: None };
            for v in story::voice_choices(&probe) {
                let engine = SpeechEngine::Http { url: format!("{base}/audio/speech"), model: m.clone(), key_ref: p.key_ref.clone(), voice: Some(v.clone()), language: None };
                push(ReadVoice { id: format!("provider:{}:{m}:{v}", p.id), label: format!("{m} · {v}"), source: format!("{m} on {}", p.id), engine, voice: Some(v), english_only: false }, &mut out);
            }
        }
    }
    svc.host_tts.ensure();
    if let Some(r) = svc.host_tts.report() {
        for e in r.engines.iter().filter(|e| e.usable()) {
            let engine = e.setting();
            let label = format!("{} (on this computer)", e.kind.label());
            push(ReadVoice { id: format!("host:{}", e.kind.id()), label, source: "this computer".into(), engine, voice: None, english_only: false }, &mut out);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------------------ where it goes

/// Where an engine's server is.
#[derive(Clone, Debug, PartialEq, Eq)]
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
pub fn destination(svc: &Services, engine: &SpeechEngine) -> Option<Dest> {
    let url = engine_url(engine)?;
    let server = speechtime::server_of(&url);
    let address = server.split_once("://").map_or(server.clone(), |(_, r)| r.to_string());
    let name = providers(svc).iter().find(|p| p.base().ok().is_some_and(|b| speechtime::server_of(&b) == server)).map(|p| p.id.clone()).unwrap_or_else(|| iyakki_sec::url_host(&url).unwrap_or_default());
    Some(Dest { name, address, server, place: speechtime::locality(&url) })
}

/// The line that says where the text goes: "Sent to lemonade-local at localhost:13305 (this computer)".
pub fn where_text(svc: &Services, engine: &SpeechEngine, what: &str) -> String {
    match destination(svc, engine) {
        Some(d) => format!("{what} sent to {} at {} ({})", d.name, d.address, d.place_words()),
        None => format!("{what} stays on this computer (read with {})", engine.label()),
    }
}

/// The card shown before text or sound goes to a machine the person has not agreed to yet (None: nothing to ask).
pub fn consent_text(svc: &Services, engine: &SpeechEngine, listening: bool) -> Option<(Dest, String)> {
    let d = destination(svc, engine)?;
    if svc.speech_server_allowed(&engine_url(engine)?) {
        return None;
    }
    let place = match d.place {
        Locality::ThisComputer => "on this computer",
        Locality::Network => "on the network",
        Locality::Internet => "on the internet",
    };
    let host = iyakki_sec::url_host(&d.server).unwrap_or_else(|| d.address.clone());
    let text = if listening {
        format!("{} ({host}) is {place}. Speech-to-text with it sends what the microphone hears to that machine. Send sound to {host} from now on?", d.name)
    } else {
        format!("{} ({host}) is {place}. Reading aloud with it sends the text being read to that machine, and its sound comes back from there. Send text to {host} from now on?", d.name)
    };
    Some((d, text))
}

// ------------------------------------------------------------------------------------------------------------ reading

/// The pieces to read: one per sentence (long ones cut at a comma or a space), without the ones that have nothing to say
/// (a bare address, a line of marks). Positions are characters of `text`, so the lit sentence can be drawn in the box.
pub fn pieces(text: &str) -> Vec<Chunk> {
    story::sentence_chunks(text, story::MAX_CHUNK_CHARS).into_iter().filter(|c| !voice::speakable(&c.text).is_empty()).collect()
}

/// Makes the sound of one piece with the chosen voice and speed (the engine is asked directly: the choice on the page wins over
/// Settings -> Voice for this reading only).
pub struct ChoiceSynth {
    pub svc: Arc<Services>,
    pub engine: SpeechEngine,
    pub opts: SpeakOpts,
}

impl story::Synth for ChoiceSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        let key = speechtime::cache_key(&self.engine, &self.opts, text);
        if let Some((w, _)) = speechtime::cached(&key) {
            return Ok((*w).clone());
        }
        let up = self.svc.slow_upstream();
        let wav = voice::synthesize_with(&self.engine, text, &self.opts, &self.svc.secrets, up.as_ref()).map_err(|e| explain_tts(&self.engine, &e))?;
        speechtime::remember(&key, Arc::new(wav.clone()), None);
        Ok(wav)
    }
}

/// An error of the speech server in plain words.
pub fn explain_tts(engine: &SpeechEngine, e: &str) -> String {
    if speechtime::unreachable(e) {
        let at = engine_url(engine).map_or("the speech server".to_string(), |u| speechtime::server_of(&u));
        return format!("The reading voice did not answer at {at}. Is the server running?");
    }
    match engine {
        SpeechEngine::Http { model, .. } => lemonade::explain_error(model, e),
        _ => e.to_string(),
    }
}

/// The key under which sound made for one voice is kept by the reader (a different voice or speed makes new sound).
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

/// The language of `text` by script (None when there are too few letters). Latin letters count as English (a hint, not a test).
pub fn detect_language(text: &str) -> Option<Hint> {
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

/// A sound input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mic {
    /// The ALSA device (`plughw:1,0`).
    pub id: String,
    pub label: String,
    pub usb: bool,
}

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

/// The T2 sound card's microphones: not there, there but hidden on purpose, or allowed by an administrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T2Mic {
    Absent,
    Off,
    On,
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
pub fn record_cmd_for(base: &str, mic: Option<&Mic>) -> String {
    let Some(m) = mic else { return base.to_string() };
    let Ok(args) = voice::split_command(base) else { return base.to_string() };
    // only ALSA's `arecord` takes -D; a custom recorder (a script, parecord, ffmpeg) is left exactly as the person wrote it
    if args.is_empty() || args[0].rsplit('/').next() != Some("arecord") || crate::audioout::names_device(&args) {
        return base.to_string();
    }
    let mut a = args;
    a.insert(1, m.id.clone());
    a.insert(1, "-D".to_string());
    crate::audioout::join_command(&a)
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

/// File names shared with the web engine's microphone plugin (`dist/webview/wpe-helper/micscan.h`): the shell writes the choice, the plugin
/// writes why a microphone could not be opened.
pub const BROWSER_MIC_CHOICE: &str = "iyakki-browser-mic";
pub const BROWSER_MIC_STATUS: &str = "iyakki-browser-mic-status";

/// The folder both sides use: `IYAKKI_MIC_DIR` when set (tests, the plugin reads the same variable), else the shared memory folder.
pub fn mic_share_dir() -> PathBuf {
    match std::env::var("IYAKKI_MIC_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => crate::sysinfo::shared_memory_dir(),
    }
}

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

/// What to tell the person when no microphone was found.
pub fn no_mic_text(product: &str) -> String {
    no_mic_text_t2(product, false)
}

/// [`no_mic_text`], knowing that a T2 sound card's microphones are switched off.
pub fn no_mic_text_t2(product: &str, t2_hidden: bool) -> String {
    if t2_hidden {
        return format!("The microphone of this {} is not supported yet: the T2 chip's sound driver can hang the computer when it records, so it is switched off. A USB microphone or a headset with a USB adapter works. (An administrator can try the built-in one: echo 1 > /sys/module/apple_bce/parameters/capture, at their own risk.)", product.trim());
    }
    let mut t = "No microphone found. USB microphones work: plug one in and it appears here.".to_string();
    if iyakki_drivers::cs8409::model_applies(product) {
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
}

impl Capture for voice::Recorder {
    fn level(&self) -> f32 {
        voice::Recorder::level(self)
    }
    fn elapsed(&self) -> Duration {
        voice::Recorder::elapsed(self)
    }
    fn is_full(&self) -> bool {
        voice::Recorder::is_full(self)
    }
    fn is_alive(&mut self) -> bool {
        voice::Recorder::is_alive(self)
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        voice::Recorder::stop(*self)
    }
}

/// Finds microphones and starts recordings (a trait so tests record nothing for real).
pub trait Recorders: Send + Sync {
    fn mics(&self) -> Vec<Mic>;
    fn product(&self) -> String;
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String>;
}

/// The real thing: `/proc/asound` and the record command of Settings -> Voice (`arecord`).
pub struct SystemRecorders;

impl Recorders for SystemRecorders {
    fn mics(&self) -> Vec<Mic> {
        list_mics(Path::new("/proc"))
    }
    fn product(&self) -> String {
        product_name(Path::new("/sys"))
    }
    fn start(&self, cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String> {
        Ok(Box::new(voice::Recorder::start_max(cmd, max_secs)?))
    }
}

// ------------------------------------------------------------------------------------------------------------ listening

/// The speech-to-text engine to use: the one of Settings -> Voice, else a downloaded Lemonade whisper model (the smallest one
/// first is not assumed: the first listed).
pub fn stt_engine(svc: &Services) -> Option<SpeechEngine> {
    if let Some(e) = svc.voice_settings().stt {
        return Some(e);
    }
    let (prov, model) = svc.lemonade_models(ModelType::SpeechToText).into_iter().next()?;
    let p = providers(svc).into_iter().find(|p| p.id == prov)?;
    Some(lemonade::stt_engine(&p.base().ok()?, &model, p.key_ref.clone()))
}

/// Recorded samples to text with `engine`. A silent recording is told apart from a failure.
pub fn transcribe(svc: &Services, engine: &SpeechEngine, pcm: &[i16]) -> Result<String, String> {
    if pcm.is_empty() {
        return Err("Nothing was recorded.".into());
    }
    if voice::peak(pcm) < 150 {
        return Err("I only heard silence. Check the microphone and its volume.".into());
    }
    let up = svc.slow_upstream();
    let text = voice::transcribe(engine, &voice::encode_wav(pcm, voice::SAMPLE_RATE), &svc.secrets, up.as_ref()).map_err(|e| match engine {
        SpeechEngine::Http { model, .. } if speechtime::unreachable(&e) => format!("The speech-to-text server did not answer ({model}). Is it running?"),
        SpeechEngine::Http { model, .. } => lemonade::explain_error(model, &e),
        _ => e,
    })?;
    if text.is_empty() { Err("No speech was recognised.".into()) } else { Ok(text) }
}

#[cfg(test)]
#[path = "voicedesk_tests.rs"]
mod tests;

/// Where the kernel's process and sound tables live (a function so tiles ask core instead of naming a kernel path themselves).
pub fn proc_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/proc")
}

/// A storage error in words a person can act on (a function so tiles ask core instead of using the storage crate).
pub fn reword_storage_error(msg: &str) -> String {
    iyakki_storage::loss::reword(msg)
}
