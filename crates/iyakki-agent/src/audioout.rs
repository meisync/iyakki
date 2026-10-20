//! Sound output: which ALSA playback device IYAKKI's own players use, and the volume IYAKKI applies itself.
//!
//! Why: ALSA's `default` device is card 0 device 0. Many machines have no such device: a Mac mini 2018 (T2) has only the
//! HDMI/DP devices 3, 7 and 8 of its Intel codec on card 0 (the built-in speaker and the headphone jack are a second card,
//! `Apple T2 Audio`, that only the apple-bce driver of the Mac image provides), and PCs often have several cards or HDMI
//! only. Playing to `default` there is silent.
//!
//! What: [`scan`] lists the playback devices from `/proc/asound` (cards, `pcmNp/info`), classifies each (speakers or
//! headphones, HDMI/DP, USB, Bluetooth, digital), and reads whether something is connected: the jack controls and ELD
//! (monitor name, speakers) from `amixer -c N contents`, else the `eld#*` files of `/proc/asound/cardN`. [`choose`] picks
//! the person's chosen output when it is there; else connected speakers/headphones; else an HDMI/DP output with a monitor
//! (one that reports speakers first); else USB; else a built-in speaker that has no jack (the T2 Mac's `Speaker`); else
//! nothing, with the reason. The device is handed to `aplay` ([`route_args`]): `-D iyakki` when IYAKKI's ALSA
//! configuration describes the output (see below), else `-D plughw:C,D`, so format and rate conversion still work; a play
//! command that already names a device, and players other than `aplay`, are left as they are.
//!
//! A [`Selector`] keeps the picture and is re-read every few seconds in the background ([`start_watch`]), so a monitor
//! plugged in later, or moved to another port, is used by the next sound; the radio restarts its player when
//! [`Selector::generation`] changes.
//!
//! Volume: once installed, the global selector keeps `/etc/asound.conf` in step with the chosen output
//! ([`crate::softvol`]): an output whose card has no hardware volume (HDMI on Intel, the T2 Mac's speaker and headphones)
//! gets an ALSA softvol device, made `default`, whose `Master` control carries the volume and mute of Settings -> Sound, so
//! every program that plays sound follows it (ffplay included). The last volume of each output is kept in the settings
//! ([`SoundSettings::levels`]) and restored when that output is used again. Where there is no softvol (a card with a
//! hardware volume, a sound server, a file IYAKKI did not write) the volume is applied in software by IYAKKI's players
//! ([`scale_wav`], the radio's decoder), as before.
//!
//! The process-wide selector ([`global`]) stays inactive (no `-D`, full volume) until the real front end calls [`install`],
//! so tests and tools that open their own services never touch the machine's sound.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What is said when there is nothing to play on.
pub const NO_OUTPUT: &str = "no sound output: connect a monitor with speakers over HDMI/DP or a USB audio device; built-in speakers on T2 Macs need the apple-bce driver of the Mac image";

/// How often the background watcher looks at the sound hardware again.
pub const WATCH_EVERY: Duration = Duration::from_secs(3);

// ------------------------------------------------------------------------------------------------ settings

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
}

/// At most this many outputs keep their own volume.
pub const MAX_LEVELS: usize = 32;

impl Default for SoundSettings {
    fn default() -> Self {
        SoundSettings { output: String::new(), volume_pct: 100, muted: false, levels: Default::default() }
    }
}

fn output_key_ok(k: &str) -> bool {
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
        Ok(())
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

// ------------------------------------------------------------------------------------------------ what the machine has

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
}

impl Output {
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

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// `/proc/asound/cards`: ` 0 [PCH            ]: HDA-Intel - HDA Intel PCH` → (0, "PCH", "HDA-Intel", "HDA Intel PCH").
fn parse_cards(text: &str) -> Vec<(u32, String, String, String)> {
    text.lines()
        .filter_map(|l| {
            let (idx, rest) = l.trim_start().split_once(' ')?;
            let card: u32 = idx.parse().ok()?;
            let (id, after) = rest.trim_start().strip_prefix('[')?.split_once("]:")?;
            let (driver, name) = after.split_once(" - ").unwrap_or((after, after));
            Some((card, id.trim().to_string(), driver.trim().to_string(), name.trim().to_string()))
        })
        .collect()
}

/// `key: value` lines of a `pcmNp/info` or `eld#*` file.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.strip_prefix(key)?;
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        (rest.starts_with(char::is_whitespace) || rest.is_empty()).then(|| rest.trim().to_string())
    })
}

/// The mixer controls that tell what is connected (from `amixer -c N contents`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    /// `(name, on)` of every `… Jack` control.
    pub jacks: Vec<(String, bool)>,
    /// `(pcm device, bytes)` of every `ELD` control.
    pub eld: Vec<(u32, Vec<u8>)>,
    /// `(index, on)` of every `IEC958 Playback Switch`.
    pub iec958: Vec<(u32, bool)>,
    /// The names of the `… Playback Volume` controls (a hardware volume).
    pub volumes: Vec<String>,
}

/// Parse `amixer -c N contents`:
/// ```text
/// numid=17,iface=CARD,name='HDMI/DP,pcm=3 Jack'
///   ; type=BOOLEAN,access=r-------,values=1
///   : values=on
/// numid=22,iface=PCM,name='ELD',device=3
///   ; type=BYTES,access=r--v----,values=95
///   : values=0x10,0x00,0x08,...
/// ```
pub fn parse_controls(text: &str) -> Controls {
    let mut c = Controls::default();
    let mut cur: Option<(String, u32, u32)> = None; // name, device, index
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("numid=") {
            cur = l.split_once("name='").and_then(|(_, rest)| {
                let (name, tail) = rest.split_once('\'')?;
                let num = |k: &str| tail.split(',').find_map(|p| p.strip_prefix(k)).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
                Some((name.to_string(), num("device="), num("index=")))
            });
            if let Some((name, _, _)) = &cur {
                if name.ends_with(" Playback Volume") && !c.volumes.contains(name) {
                    c.volumes.push(name.clone());
                }
            }
            continue;
        }
        let (Some(v), Some((name, device, index))) = (l.strip_prefix(": values="), &cur) else { continue };
        if name.ends_with(" Jack") {
            c.jacks.push((name.clone(), v.trim() == "on"));
        } else if name == "ELD" {
            c.eld.push((*device, parse_bytes(v)));
        } else if name == "IEC958 Playback Switch" {
            c.iec958.push((*index, v.split(',').next().map(str::trim) == Some("on")));
        }
    }
    c
}

/// `0x10,0x00,...` (also accepts `10:00:...` and a plain hex string).
fn parse_bytes(v: &str) -> Vec<u8> {
    let v = v.trim();
    if v.is_empty() {
        return vec![];
    }
    if !v.contains([',', ':', ' ']) && !v.starts_with("0x") && v.len() % 2 == 0 {
        return (0..v.len()).step_by(2).filter_map(|i| u8::from_str_radix(&v[i..i + 2], 16).ok()).collect();
    }
    v.split([',', ':', ' ']).filter(|t| !t.is_empty()).filter_map(|t| u8::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok()).collect()
}

/// The monitor name and the number of audio formats from ELD bytes (CEA-861 ELD: the name length is the low five bits of
/// byte 4, the format count the high four bits of byte 5, the name starts at byte 20).
pub fn eld_monitor(b: &[u8]) -> Option<(String, usize)> {
    if b.len() < 20 {
        return None;
    }
    let mnl = (b[4] & 0x1f) as usize;
    let sads = (b[5] >> 4) as usize;
    let name = String::from_utf8_lossy(b.get(20..20 + mnl)?).trim_matches(char::from(0)).trim().to_string();
    Some((name, sads))
}

/// One `/proc/asound/cardN/eld#C.P` file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcEld {
    order: (u32, u32),
    present: bool,
    name: Option<String>,
    sads: usize,
}

fn proc_elds(card_dir: &Path) -> Vec<ProcEld> {
    let mut v: Vec<ProcEld> = std::fs::read_dir(card_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let (c, p) = n.strip_prefix("eld#")?.split_once('.')?;
            let t = read(e.path());
            let present = field(&t, "monitor_present").as_deref() == Some("1") && field(&t, "eld_valid").as_deref() != Some("0");
            let name = field(&t, "monitor_name").filter(|s| !s.is_empty());
            let sads = field(&t, "sad_count").and_then(|s| s.parse().ok()).unwrap_or(0);
            Some(ProcEld { order: (c.parse().unwrap_or(0), p.parse().unwrap_or(0)), present, name, sads })
        })
        .collect();
    v.sort_by_key(|e| e.order);
    v
}

fn is_hdmi(id: &str, name: &str) -> bool {
    let t = format!("{id} {name}").to_ascii_uppercase();
    t.contains("HDMI") || t.contains("DISPLAYPORT") || t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "DP")
}

/// The card of the T2 chip's audio (apple-bce's `aaudio`: `/proc/asound/cards` says `AppleT2x1 - Apple T2 Audio`).
fn is_t2_card(driver: &str, card_name: &str) -> bool {
    driver.starts_with("AppleT2") || card_name.contains("Apple T2")
}

/// How a shared (dmix) softvol opens this output: the T2 chip's devices take only 24-bit (the speaker one channel), everything
/// else (HDMI, USB, analog codecs) 16-bit stereo. If the device refuses, `softvol` falls back to the exclusive device.
pub fn mix_for(o: &Output) -> crate::softvol::Mix {
    use crate::softvol::Mix;
    if o.card_name.contains("Apple T2") {
        if o.builtin_speaker { Mix::T2_MONO } else { Mix::T2_STEREO }
    } else {
        Mix::STEREO
    }
}

/// The T2's headphone jack is the PCM `Codec Output` (the speaker is `Speaker`).
fn is_headphone_pcm(pcm_name: &str) -> bool {
    let n = pcm_name.to_ascii_lowercase();
    n.contains("headphone") || n == "codec output"
}

fn is_output_jack(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.contains("hdmi") && !n.contains("mic") && (n.contains("headphone") || n.contains("line out") || n.contains("speaker") || n.contains("headset"))
}

/// Every playback device of the machine. `mixer(card)` gives `amixer -c card contents` (None when there is no `amixer`).
pub fn scan(proc_root: &Path, mixer: &dyn Fn(u32) -> Option<String>) -> Vec<Output> {
    let asound = proc_root.join("asound");
    let mut out = Vec::new();
    for (card, card_id, driver, card_name) in parse_cards(&read(asound.join("cards"))) {
        let low = format!("{driver} {card_name}").to_ascii_lowercase();
        if low.contains("loopback") || low.contains("dummy") {
            continue; // virtual cards nobody hears
        }
        let dir = asound.join(format!("card{card}"));
        let usb = dir.join("usbid").exists() || low.contains("usb-audio") || low.contains("usb audio");
        let t2 = is_t2_card(&driver, &card_name);
        let bt = low.contains("bluetooth") || low.contains("bluealsa");
        let mut pcms: Vec<(u32, String, String)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let dev: u32 = n.strip_prefix("pcm")?.strip_suffix('p')?.parse().ok()?;
                let info = read(e.path().join("info"));
                let id = field(&info, "id").unwrap_or_default();
                let name = field(&info, "name").unwrap_or_else(|| id.clone());
                Some((dev, id, name))
            })
            .collect();
        pcms.sort_by_key(|p| p.0);
        let ctl = mixer(card).map(|t| parse_controls(&t)).unwrap_or_default();
        let analog_jacks: Vec<bool> = ctl.jacks.iter().filter(|(n, _)| is_output_jack(n)).map(|(_, on)| *on).collect();
        let mut outs: Vec<Output> = Vec::new();
        let mut hdmi_no = 0;
        for (device, id, pcm_name) in pcms {
            let kind = if usb {
                Kind::Usb
            } else if bt {
                Kind::Bluetooth
            } else if is_hdmi(&id, &pcm_name) {
                Kind::Hdmi
            } else if format!("{id} {pcm_name}").to_ascii_lowercase().contains("digital") || pcm_name.contains("IEC958") || pcm_name.contains("SPDIF") {
                Kind::Digital
            } else {
                Kind::Analog
            };
            // a jack control named after this very PCM (`Codec Output Jack` of the T2 card) says what is plugged into it
            let own_jack = ctl.jacks.iter().find(|(n, _)| n.strip_suffix(" Jack") == Some(pcm_name.as_str())).map(|(_, on)| *on);
            let builtin_speaker = t2 && kind == Kind::Analog && pcm_name.eq_ignore_ascii_case("speaker");
            // the card's Master/PCM/... drive its analog codec, never the HDMI or S/PDIF stream
            let hw_volume = if matches!(kind, Kind::Hdmi | Kind::Digital) { None } else { crate::softvol::hw_control(&ctl.volumes) };
            let mut o = Output { card, device, card_id: card_id.clone(), card_name: card_name.clone(), pcm_name, kind, connected: None, monitor: None, speakers: None, hdmi_no: 0, digital_off: false, builtin_speaker, hw_volume };
            match kind {
                Kind::Hdmi => {
                    hdmi_no += 1;
                    o.hdmi_no = hdmi_no;
                    o.connected = ctl.jacks.iter().find(|(n, _)| n.contains(&format!("pcm={device} ")) || n.ends_with(&format!("pcm={device}"))).map(|(_, on)| *on);
                    if let Some((name, sads)) = ctl.eld.iter().find(|(d, _)| *d == device).and_then(|(_, b)| eld_monitor(b)) {
                        o.monitor = Some(name).filter(|n| !n.is_empty());
                        o.speakers = Some(sads > 0);
                        o.connected.get_or_insert(true);
                    }
                    o.digital_off = ctl.iec958.iter().find(|(i, _)| *i as usize == hdmi_no - 1).is_some_and(|(_, on)| !on);
                }
                // the built-in speaker is always there; another analog output follows its own jack, else the card's jacks
                Kind::Analog if builtin_speaker => o.connected = Some(true),
                Kind::Analog => o.connected = own_jack.or(if analog_jacks.is_empty() { None } else { Some(analog_jacks.iter().any(|on| *on)) }),
                Kind::Usb | Kind::Bluetooth => o.connected = Some(true),
                Kind::Digital => {}
            }
            outs.push(o);
        }
        fill_from_proc_eld(&mut outs, &proc_elds(&dir));
        out.extend(outs);
    }
    out
}

/// Without the mixer's answer, or for the monitor's name: the kernel's `eld#*` files. Their numbers are codec pins, not PCM
/// devices. With more pins than HDMI devices (Intel since Haswell: devices are given to connected pins in turn) the n-th
/// connected monitor plays on the n-th HDMI device; with as many pins as devices, pin n is device n.
fn fill_from_proc_eld(outs: &mut [Output], elds: &[ProcEld]) {
    if elds.is_empty() {
        return;
    }
    let hdmi: Vec<usize> = outs.iter().enumerate().filter(|(_, o)| o.kind == Kind::Hdmi).map(|(i, _)| i).collect();
    if hdmi.is_empty() {
        return;
    }
    let dynamic = elds.len() != hdmi.len();
    let present: Vec<&ProcEld> = elds.iter().filter(|e| e.present).collect();
    let jacks_known = hdmi.iter().any(|&i| outs[i].connected.is_some());
    let mut next = 0;
    for (n, &i) in hdmi.iter().enumerate() {
        let o = &mut outs[i];
        let eld = if dynamic {
            // with the mixer's jacks: the connected devices in order; without: the first devices
            let take = if jacks_known { o.connected == Some(true) } else { next < present.len() };
            if take && next < present.len() {
                next += 1;
                Some(present[next - 1])
            } else {
                None
            }
        } else {
            elds.get(n).filter(|e| e.present)
        };
        if !jacks_known {
            o.connected = Some(eld.is_some());
        }
        if let (Some(e), Some(true)) = (eld, o.connected) {
            if o.monitor.is_none() {
                o.monitor = e.name.clone();
            }
            if o.speakers.is_none() {
                o.speakers = Some(e.sads > 0);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ choosing

/// The output IYAKKI plays on, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub output: Option<Output>,
    /// Chosen by the person (not automatic).
    pub chosen: bool,
    pub reason: String,
}

/// Pick the output: the chosen one (`chosen` = its [`Output::key`]) when it is there, else the best one connected.
pub fn choose(outs: &[Output], chosen: &str) -> Choice {
    if !chosen.is_empty() {
        if let Some(o) = outs.iter().find(|o| o.key() == chosen) {
            return Choice { output: Some(o.clone()), chosen: true, reason: "chosen in Settings → Sound".into() };
        }
    }
    let rank = |o: &Output| -> Option<(u8, String)> {
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
    };
    let best = outs.iter().filter_map(|o| rank(o).map(|(r, why)| (r, why, o))).min_by_key(|(r, _, _)| *r);
    let missing = if chosen.is_empty() { String::new() } else { format!("the chosen output ({chosen}) is not connected now; ") };
    match best {
        Some((_, why, o)) => Choice { output: Some(o.clone()), chosen: false, reason: format!("{missing}{why}") },
        None if outs.is_empty() => Choice { output: None, chosen: false, reason: format!("{missing}{NO_OUTPUT} (no sound card was found)") },
        None => Choice { output: None, chosen: false, reason: format!("{missing}{NO_OUTPUT}") },
    }
}

// ------------------------------------------------------------------------------------------------ play commands

/// Does this argument list already name an ALSA device (`-D x`, `-Dx`, `-qD x`, `--device=x`)?
pub fn names_device(args: &[String]) -> bool {
    args.iter().any(|a| a == "--device" || a.starts_with("--device=") || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('D')))
}

/// Give `aplay` the device: `-D <device>` right after the program. Other players, and commands that already name a device,
/// are returned unchanged. No shell is involved: this works on the split argument list.
pub fn route_args(mut args: Vec<String>, device: Option<&str>) -> Vec<String> {
    let Some(dev) = device.filter(|d| !d.is_empty()) else { return args };
    let Some(prog) = args.first() else { return args };
    if prog.rsplit('/').next() != Some("aplay") || names_device(&args[1..]) {
        return args;
    }
    args.insert(1, "-D".into());
    args.insert(2, dev.into());
    args
}

/// The ALSA device an argument list names (`-D x`, `-Dx`, `-qD x`, `--device=x`, `--device x`), if any.
pub fn named_device(args: &[String]) -> Option<String> {
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--device" || a == "-D" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--device=") {
            return Some(v.to_string());
        }
        if a.starts_with('-') && !a.starts_with("--") && a[1..].contains('D') {
            let (_, v) = a[1..].split_once('D').unwrap_or(("", ""));
            return if v.is_empty() { it.next().cloned() } else { Some(v.to_string()) };
        }
    }
    None
}

/// The volume a player started with these (routed) arguments applies to its samples: the device IYAKKI keeps (`iyakki`,
/// `default`, none) carries the software volume when there is one ([`Selector::gain`]); a device named by hand bypasses it
/// ([`Selector::prefs_gain`]).
pub fn gain_for(sel: &Selector, args: &[String]) -> u32 {
    match named_device(args).as_deref() {
        None | Some(crate::softvol::PCM) | Some("default") => sel.gain(),
        Some(_) => sel.prefs_gain(),
    }
}

/// The argument list without the device it names (`-D x`, `-Dx`, `-qD x`, `--device=x`, `--device x`).
fn strip_device(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if i > 0 && (a == "--device" || a == "-D") {
            i += 2;
            continue;
        }
        if i > 0 && a.starts_with("--device=") {
            i += 1;
            continue;
        }
        if i > 0 && a.starts_with('-') && !a.starts_with("--") && a[1..].contains('D') {
            let (flags, value) = a[1..].split_once('D').unwrap_or((&a[1..], ""));
            if !flags.is_empty() {
                out.push(format!("-{flags}"));
            }
            i += if value.is_empty() { 2 } else { 1 };
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out
}

/// Join an argument list back into a command line that `voice::split_command` reads the same way.
pub fn join_command(args: &[String]) -> String {
    args.iter()
        .map(|a| if !a.is_empty() && !a.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') { a.clone() } else if !a.contains('\'') { format!("'{a}'") } else { format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")) })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The play command with this device in it (Test sound on one particular output).
pub fn command_with_device(play_cmd: &str, device: &str) -> Result<String, String> {
    let args = crate::voice::split_command(play_cmd)?;
    if args.first().is_some_and(|p| p.rsplit('/').next() == Some("aplay")) {
        // the person's aplay, with the device it names (if any) replaced by this one
        return Ok(join_command(&route_args(strip_device(&args), Some(device))));
    }
    // another player (or one with its own device): the usual aplay is what can reach one ALSA device
    Ok(join_command(&["aplay".to_string(), "-q".to_string(), "-D".to_string(), device.to_string()]))
}

// ------------------------------------------------------------------------------------------------ software volume

/// Scale 16-bit samples by `gain` percent.
pub fn scale(pcm: &mut [i16], gain: u32) {
    if gain >= 100 {
        return;
    }
    for s in pcm {
        *s = (*s as i32 * gain as i32 / 100) as i16;
    }
}

/// A WAV with its 16-bit PCM samples scaled by `gain` percent. Anything else is returned unchanged.
pub fn scale_wav(wav: &[u8], gain: u32) -> Vec<u8> {
    let mut out = wav.to_vec();
    if gain >= 100 || wav.len() < 44 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return out;
    }
    let (mut pos, mut pcm16) = (12usize, false);
    while pos + 8 <= out.len() {
        let id = [out[pos], out[pos + 1], out[pos + 2], out[pos + 3]];
        let len = u32::from_le_bytes([out[pos + 4], out[pos + 5], out[pos + 6], out[pos + 7]]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(out.len());
        if &id == b"fmt " && end >= body + 16 {
            pcm16 = u16::from_le_bytes([out[body], out[body + 1]]) == 1 && u16::from_le_bytes([out[body + 14], out[body + 15]]) == 16;
        } else if &id == b"data" && pcm16 {
            // a stream written as it is made may say 0 or 0xFFFFFFFF here: then the data runs to the end
            let end = if len == 0 || len == u32::MAX as usize { out.len() } else { end };
            for c in out[body..end].chunks_exact_mut(2) {
                let v = (i16::from_le_bytes([c[0], c[1]]) as i32 * gain as i32 / 100) as i16;
                c.copy_from_slice(&v.to_le_bytes());
            }
            break;
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    out
}

/// A short, gentle two-note chime (Test sound).
pub fn test_tone_wav() -> Vec<u8> {
    let mut pcm = crate::voice::tone(523.25, 260);
    pcm.extend(crate::voice::tone(659.25, 380));
    for s in pcm.iter_mut() {
        *s = (*s as i32 * 3 / 4) as i16; // softer than the voice test tone
    }
    crate::voice::encode_wav(&pcm, crate::voice::SAMPLE_RATE)
}

// ------------------------------------------------------------------------------------------------ the selector

/// What the selector sees now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub outputs: Vec<Output>,
    pub choice: Choice,
    /// A sound server (PulseAudio, PipeWire) runs on this computer: in automatic mode it chooses the output.
    pub sound_server: bool,
    pub prefs: SoundSettings,
    /// What players get as `-D` (None: the system's default device is left alone).
    pub device: Option<String>,
    /// The card whose software volume (ALSA softvol, see [`crate::softvol`]) carries the volume of Settings -> Sound.
    pub softvol: Option<u32>,
}

impl Status {
    /// One line for the SYSTEM tile and the Devices popover.
    pub fn summary(&self) -> String {
        let vol = if self.prefs.muted { " · muted".to_string() } else if self.prefs.volume_pct < 100 { format!(" · volume {}%", self.prefs.volume_pct) } else { String::new() };
        match (&self.choice.output, self.sound_server && !self.choice.chosen) {
            (_, true) => format!("chosen by the sound server of this computer{vol}"),
            (Some(o), _) => format!("{} ({}){vol}", o.label(), self.choice.reason.trim_start_matches("automatic: ")),
            (None, _) => self.choice.reason.clone(),
        }
    }
}

struct State {
    prefs: SoundSettings,
    outputs: Vec<Output>,
    choice: Choice,
    server: bool,
    /// The hardware device of the chosen output (`plughw:C,D`).
    target: Option<String>,
    device: Option<String>,
    softvol: Option<u32>,
    /// The output whose volume `prefs.volume_pct` is.
    level_key: Option<String>,
    at: Option<Instant>,
}

type MixerFn = Box<dyn Fn(u32) -> Option<String> + Send + Sync>;
type ServerFn = Box<dyn Fn() -> bool + Send + Sync>;
type PrefsHook = Box<dyn Fn(&SoundSettings) + Send + Sync>;

/// Keeps which output to use, re-read on demand (and every few seconds by [`start_watch`] for the global one).
pub struct Selector {
    root: PathBuf,
    mixer: MixerFn,
    server: ServerFn,
    state: Mutex<State>,
    generation: AtomicU64,
    gain: AtomicU32,
    active: AtomicBool,
    /// Keeps the ALSA configuration in step with the output (the global selector, and tests that give one).
    softvol: Option<crate::softvol::Softvol>,
    /// The software volume holds the level: IYAKKI's players leave their samples alone.
    soft_on: AtomicBool,
    /// Told when the selector itself changed the settings (a remembered output volume restored), to save them.
    on_prefs: Mutex<Option<PrefsHook>>,
}

impl Selector {
    /// A selector over `proc_root` (normally `/`, so `/proc/asound` is read). `mixer(card)` is `amixer -c card contents`;
    /// `server()` says whether a sound server runs.
    pub fn new(proc_root: impl Into<PathBuf>, mixer: impl Fn(u32) -> Option<String> + Send + Sync + 'static, server: impl Fn() -> bool + Send + Sync + 'static) -> Selector {
        let prefs = SoundSettings::default();
        Selector {
            root: proc_root.into(),
            mixer: Box::new(mixer),
            server: Box::new(server),
            state: Mutex::new(State { prefs, outputs: vec![], choice: choose(&[], ""), server: false, target: None, device: None, softvol: None, level_key: None, at: None }),
            generation: AtomicU64::new(0),
            gain: AtomicU32::new(100),
            active: AtomicBool::new(true),
            softvol: None,
            soft_on: AtomicBool::new(false),
            on_prefs: Mutex::new(None),
        }
    }

    /// Also keep an ALSA configuration (normally `/etc/asound.conf`) in step with the chosen output: a software volume over
    /// an output that has no hardware volume, made `default` (see [`crate::softvol`]).
    pub fn with_softvol(mut self, softvol: crate::softvol::Softvol) -> Selector {
        self.softvol = Some(softvol);
        self
    }

    /// `f` is told the settings whenever the selector changes them itself (it restores the remembered volume of an output
    /// that is used again), so they can be saved.
    pub fn set_on_prefs(&self, f: impl Fn(&SoundSettings) + Send + Sync + 'static) {
        *self.on_prefs.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn status_of(st: &State) -> Status {
        Status { outputs: st.outputs.clone(), choice: st.choice.clone(), sound_server: st.server, prefs: st.prefs.clone(), device: st.device.clone(), softvol: st.softvol }
    }

    /// Decide again from what is known. Returns the settings when the remembered volume of a newly used output replaced
    /// the volume (the caller tells [`set_on_prefs`](Self::set_on_prefs) once the lock is released).
    fn recompute(&self, st: &mut State) -> Option<SoundSettings> {
        st.choice = choose(&st.outputs, &st.prefs.output);
        let server_decides = st.server && !st.choice.chosen;
        let target = if server_decides { None } else { st.choice.output.as_ref().map(Output::alsa) };
        // another output: its own volume, when one is remembered
        let mut restored = None;
        let key = target.as_ref().and(st.choice.output.as_ref()).map(Output::key);
        if key != st.level_key {
            if let Some(level) = key.as_ref().and_then(|k| st.prefs.levels.get(k)).copied() {
                if level != st.prefs.volume_pct {
                    st.prefs.volume_pct = level;
                    self.gain.store(st.prefs.gain(), Ordering::SeqCst);
                    restored = Some(st.prefs.clone());
                }
            }
            st.level_key = key;
        }
        // the ALSA configuration: only by the real front end, and never under a sound server (it owns `default`)
        let synced = match &self.softvol {
            Some(sv) if self.active.load(Ordering::SeqCst) => {
                let plan = if st.server {
                    None
                } else {
                    st.choice.output.as_ref().map(|o| if o.hw_volume.is_some() { crate::softvol::Plan::Hw { card: o.card, device: o.device } } else { crate::softvol::Plan::Mix { card: o.card, device: o.device, mix: mix_for(o) } })
                };
                sv.sync(plan.as_ref(), st.prefs.gain())
            }
            _ => crate::softvol::Synced::default(),
        };
        st.softvol = synced.soft_card;
        self.soft_on.store(synced.soft_card.is_some(), Ordering::SeqCst);
        let device = if synced.managed && target.is_some() { Some(crate::softvol::PCM.to_string()) } else { target.clone() };
        // a player restarts when the hardware behind its device changes, even if the device name stays `iyakki`
        if device != st.device || target != st.target {
            st.device = device;
            st.target = target;
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        restored
    }

    fn tell(&self, restored: Option<SoundSettings>) {
        if let Some(p) = restored {
            if let Some(f) = self.on_prefs.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                f(&p);
            }
        }
    }

    /// Settings -> Sound changed.
    pub fn set_prefs(&self, p: SoundSettings) {
        self.gain.store(p.gain(), Ordering::SeqCst);
        let restored = {
            let mut st = self.lock();
            st.prefs = p;
            self.recompute(&mut st)
        };
        self.tell(restored);
    }

    /// Look at the hardware now.
    pub fn refresh(&self) -> Status {
        let outputs = scan(&self.root, &*self.mixer);
        let server = (self.server)();
        let (status, restored) = {
            let mut st = self.lock();
            st.outputs = outputs;
            st.server = server;
            st.at = Some(Instant::now());
            let r = self.recompute(&mut st);
            (Self::status_of(&st), r)
        };
        self.tell(restored);
        status
    }

    /// The picture, re-read when it is older than [`WATCH_EVERY`].
    pub fn status(&self) -> Status {
        {
            let st = self.lock();
            if st.at.is_some_and(|t| t.elapsed() < WATCH_EVERY) {
                return Self::status_of(&st);
            }
        }
        self.refresh()
    }

    /// The ALSA device players should use now (None: leave the play command alone).
    pub fn device(&self) -> Option<String> {
        if !self.active.load(Ordering::SeqCst) {
            return None;
        }
        self.status().device
    }

    /// Changes whenever [`device`](Self::device) changes (a player that is running restarts on the new device).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The volume IYAKKI's players apply to their samples, in percent: 0 when muted; 100 when the software volume of the
    /// output already holds the level (or before [`install`]); else the volume of Settings -> Sound.
    pub fn gain(&self) -> u32 {
        if !self.active.load(Ordering::SeqCst) {
            return 100;
        }
        let g = self.gain.load(Ordering::SeqCst);
        if self.soft_on.load(Ordering::SeqCst) && g > 0 { 100 } else { g }
    }

    /// The volume of Settings -> Sound in percent (0 when muted), whatever applies it: for a sound sent to one particular
    /// output by hand (Test sound on a row), which bypasses the software volume.
    pub fn prefs_gain(&self) -> u32 {
        if self.active.load(Ordering::SeqCst) { self.gain.load(Ordering::SeqCst) } else { 100 }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Settings -> Sound as the selector has it (no look at the hardware).
    pub fn prefs(&self) -> SoundSettings {
        self.lock().prefs.clone()
    }

    /// The card whose software volume carries the volume of Settings -> Sound (None before [`install`] or when the
    /// output has a hardware volume, a sound server runs, or `/etc/asound.conf` is someone else's).
    pub fn softvol_card(&self) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        self.lock().softvol
    }

    /// The output in use ([`Output::key`]), whose volume Settings -> Sound shows (None before [`install`]).
    pub fn current_key(&self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        self.lock().level_key.clone()
    }
}

/// The outputs under `proc_root` for a page that shows them: the watched picture for the real `/proc` once [`install`]ed,
/// else a fresh look (with `amixer` only for the real `/proc`, so a test's made-up tree never meets the real mixer).
pub fn outputs_at(proc_root: &Path) -> Vec<Output> {
    let real = proc_root == Path::new("/proc");
    if real && global().is_active() {
        return global().status().outputs;
    }
    if real { scan(proc_root, &amixer_contents) } else { scan(proc_root, &|_| None) }
}

/// One line about the sound output for the SYSTEM tile: where sound plays and why, or why it cannot.
pub fn summary_at(proc_root: &Path) -> String {
    if proc_root == Path::new("/proc") && global().is_active() {
        return global().status().summary();
    }
    let outputs = outputs_at(proc_root);
    let prefs = global().prefs();
    let choice = choose(&outputs, &prefs.output);
    Status { outputs, choice, sound_server: false, prefs, device: None, softvol: None }.summary()
}

/// `amixer -c N contents` (alsa-utils), or None when it is missing or fails.
pub fn amixer_contents(card: u32) -> Option<String> {
    let out = std::process::Command::new("amixer").args(["-c", &card.to_string(), "contents"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Is a sound server (PulseAudio or PipeWire) running for this user? Then its `default` device already follows the person's
/// choice of output, and automatic mode leaves it alone.
pub fn sound_server_running() -> bool {
    if std::env::var_os("PULSE_SERVER").is_some() {
        return true;
    }
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).is_some_and(|d| d.join("pulse/native").exists() || d.join("pipewire-0").exists())
}

static GLOBAL: OnceLock<Selector> = OnceLock::new();
static WATCHING: AtomicBool = AtomicBool::new(false);

/// The selector IYAKKI's players ask. Inactive (no `-D`, full volume) until [`install`].
pub fn global() -> &'static Selector {
    GLOBAL.get_or_init(|| {
        let s = Selector::new("/proc", amixer_contents, sound_server_running).with_softvol(crate::softvol::Softvol::system());
        s.active.store(false, Ordering::SeqCst);
        s
    })
}

/// The real front end: use the selector, with Settings -> Sound, and watch for monitors and devices coming and going.
/// From now on `/etc/asound.conf` follows the chosen output (see [`crate::softvol`]); the first look at the hardware is
/// the watcher's (or the first sound's), so the remembered volume is set on the output as soon as it is known.
pub fn install(prefs: SoundSettings) {
    let g = global();
    g.gain.store(prefs.gain(), Ordering::SeqCst);
    g.lock().prefs = prefs;
    g.active.store(true, Ordering::SeqCst);
    start_watch();
}

/// Settings -> Sound changed (only reaches the selector once it is installed).
pub fn apply_prefs(prefs: SoundSettings) {
    let g = global();
    if g.is_active() {
        g.set_prefs(prefs);
    }
}

/// Re-read the sound hardware every [`WATCH_EVERY`] in the background (once per process).
pub fn start_watch() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let started = std::thread::Builder::new().name("iyakki-audioout".into()).spawn(|| loop {
        let before = global().generation();
        let st = global().refresh();
        if global().generation() != before {
            log::info!("sound output: {}", st.summary());
        }
        std::thread::sleep(WATCH_EVERY);
    });
    if started.is_err() {
        WATCHING.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Tree(PathBuf);
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(tag: &str) -> Tree {
        let d = std::env::temp_dir().join(format!("iyakki-audioout-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("asound")).unwrap();
        Tree(d)
    }

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn pcm(root: &Path, card: u32, dev: u32, id: &str) {
        put(root, &format!("asound/card{card}/pcm{dev}p/info"), &format!("card: {card}\ndevice: {dev}\nsubdevice: 0\nstream: PLAYBACK\nid: {id}\nname: {id}\nsubname: subdevice #0\nclass: 0\n"));
    }

    fn eld_file(present: bool, name: &str) -> String {
        if present {
            format!("monitor_present\t\t1\neld_valid\t\t1\nmonitor_name\t\t{name}\nconnection_type\t\tHDMI\neld_version\t\t[0x2] CEA-861D or below\nsad_count\t\t1\nsad0_coding_type\t[0x1] LPCM\n")
        } else {
            "monitor_present\t\t0\neld_valid\t\t0\n".to_string()
        }
    }

    /// ELD bytes with a monitor name and `sads` audio formats.
    fn eld_bytes(name: &str, sads: u8) -> String {
        let mut b = vec![0u8; 20];
        b[0] = 0x10;
        b[2] = 0x08;
        b[4] = name.len() as u8 & 0x1f;
        b[5] = sads << 4;
        b.extend(name.as_bytes());
        b.extend(std::iter::repeat_n(0u8, 3 * sads as usize));
        b.iter().map(|x| format!("0x{x:02x}")).collect::<Vec<_>>().join(",")
    }

    /// The Mac mini 2018 (Macmini8,1): card 0 HDA Intel PCH, only HDMI devices 3, 7 and 8, monitor PDM-15T on the first.
    fn mac_mini(t: &Tree, monitor: bool) {
        let r = &t.0;
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xa1210000 irq 136\n");
        for (d, id) in [(3, "HDMI 0"), (7, "HDMI 1"), (8, "HDMI 2")] {
            pcm(r, 0, d, id);
        }
        for p in 0..9 {
            put(r, &format!("asound/card0/eld#2.{p}"), &eld_file(monitor && p == 6, "PDM-15T"));
        }
    }

    fn mac_mixer(on3: bool, with_eld: bool) -> String {
        let mut s = String::new();
        for (i, d) in [3, 7, 8].iter().enumerate() {
            let on = *d == 3 && on3;
            s += &format!("numid={},iface=CARD,name='HDMI/DP,pcm={d} Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={}\n", 10 + i, if on { "on" } else { "off" });
            s += &format!("numid={},iface=MIXER,name='IEC958 Playback Switch',index={i}\n  ; type=BOOLEAN,access=rw------,values=1\n  : values=on\n", 20 + i);
            let bytes = if on && with_eld { eld_bytes("PDM-15T", 1) } else { String::new() };
            s += &format!("numid={},iface=PCM,name='ELD',device={d}\n  ; type=BYTES,access=r--v----,values={}\n  : values={bytes}\n", 30 + i, if bytes.is_empty() { 0 } else { 1 });
        }
        s
    }

    fn none(_: u32) -> Option<String> {
        None
    }

    #[test]
    fn mac_mini_2018_hdmi_only_with_the_monitor_on_the_first_port_plays_on_device_3() {
        let t = tree("macmini");
        mac_mini(&t, true);
        // with amixer: the jack and the ELD control
        let outs = scan(&t.0, &|_| Some(mac_mixer(true, true)));
        assert_eq!(outs.len(), 3, "{outs:?}");
        assert!(outs.iter().all(|o| o.kind == Kind::Hdmi), "no device 0, no analog output");
        assert_eq!(outs[0].label(), "HDMI: PDM-15T");
        assert_eq!(outs[0].speakers, Some(true));
        assert_eq!(outs[1].label(), "HDMI 2 (no monitor)");
        assert_eq!(outs[2].label(), "HDMI 3 (no monitor)");
        let c = choose(&outs, "");
        assert_eq!(c.output.as_ref().unwrap().alsa(), "plughw:0,3");
        assert!(c.reason.contains("PDM-15T") && c.reason.starts_with("automatic"), "{}", c.reason);
        // without amixer: the kernel's ELD files (9 pins for 3 devices: the first connected monitor gets the first device)
        let outs = scan(&t.0, &none);
        assert_eq!(outs[0].connected, Some(true));
        assert_eq!(outs[0].monitor.as_deref(), Some("PDM-15T"));
        assert_eq!(outs[1].connected, Some(false));
        assert_eq!(choose(&outs, "").output.unwrap().alsa(), "plughw:0,3");
        // jack on but the ELD control empty: the name still comes from the ELD file
        let outs = scan(&t.0, &|_| Some(mac_mixer(true, false)));
        assert_eq!(outs[0].label(), "HDMI: PDM-15T");
    }

    #[test]
    fn mac_mini_without_a_monitor_has_no_output_and_says_why() {
        let t = tree("macmini-none");
        mac_mini(&t, false);
        let outs = scan(&t.0, &|_| Some(mac_mixer(false, false)));
        assert_eq!(outs.len(), 3);
        let c = choose(&outs, "");
        assert!(c.output.is_none());
        assert_eq!(c.reason, NO_OUTPUT);
        assert!(c.reason.contains("T2 Macs"));
    }

    /// The Mac mini 2018 with the apple-bce driver loaded (as seen on the machine: `cat /proc/asound/cards`, `aplay -l`,
    /// `amixer -c1 contents`): card 1 `Apple T2 Audio` with `Speaker` (device 0), `Codec Output` (device 2, the headphone
    /// jack) and the capture device 3; its only control is `Codec Output Jack`.
    fn mac_mini_t2(t: &Tree, monitor: bool) {
        mac_mini(t, monitor);
        let r = &t.0;
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x81710000 irq 76\n 1 [Audio          ]: AppleT2x1 - Apple T2 Audio\n                      Apple T2 Audio\n");
        pcm(r, 1, 0, "Speaker");
        pcm(r, 1, 2, "Codec Output");
        put(r, "asound/card1/pcm3c/info", "card: 1\ndevice: 3\nstream: CAPTURE\nid: Codec Input\nname: Codec Input\n");
    }

    fn t2_mixer(card: u32, monitor: bool, headphones: bool) -> Option<String> {
        Some(if card == 0 {
            mac_mixer(monitor, monitor)
        } else {
            format!("numid=1,iface=CARD,name='Codec Output Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={}\n", if headphones { "on" } else { "off" })
        })
    }

    #[test]
    fn t2_mac_plays_on_the_built_in_speaker_when_no_monitor_or_headphones_are_connected() {
        let t = tree("t2-speaker");
        mac_mini_t2(&t, false);
        let outs = scan(&t.0, &|c| t2_mixer(c, false, false));
        assert_eq!(outs.len(), 5, "three HDMI devices and two T2 outputs, no capture device: {outs:?}");
        let speaker = outs.iter().find(|o| o.key() == "Audio:0").unwrap();
        assert!(speaker.builtin_speaker && speaker.kind == Kind::Analog && speaker.connected == Some(true));
        assert_eq!(speaker.label(), "Built-in speaker (Apple T2 Audio)");
        assert_eq!(speaker.state(), "built-in speaker");
        let jack = outs.iter().find(|o| o.key() == "Audio:2").unwrap();
        assert!(!jack.builtin_speaker && jack.connected == Some(false), "the headphone jack follows `Codec Output Jack`: {jack:?}");
        assert_eq!(jack.label(), "Headphones (Apple T2 Audio)");
        let c = choose(&outs, "");
        assert_eq!(c.output.as_ref().unwrap().alsa(), "plughw:1,0");
        assert!(c.reason.contains("built-in speaker"), "{}", c.reason);
        // without amixer the speaker is still the choice (the jack is then unknown and ranks below it)
        assert_eq!(choose(&scan(&t.0, &none), "").output.unwrap().alsa(), "plughw:1,0");
    }

    #[test]
    fn t2_mac_prefers_headphones_then_a_monitor_with_speakers_then_usb_over_the_built_in_speaker() {
        let t = tree("t2-order");
        mac_mini_t2(&t, true);
        // a monitor on HDMI: it wins over the always-present speaker
        let outs = scan(&t.0, &|c| t2_mixer(c, true, false));
        let c = choose(&outs, "");
        assert_eq!(c.output.as_ref().unwrap().alsa(), "plughw:0,3", "{}", c.reason);
        // headphones plugged into the jack: they win over the monitor, as on any PC
        let outs = scan(&t.0, &|c| t2_mixer(c, true, true));
        let c = choose(&outs, "");
        assert_eq!(c.output.as_ref().unwrap().alsa(), "plughw:1,2");
        assert_eq!(c.output.unwrap().label(), "Headphones (Apple T2 Audio)");
        // no monitor, a USB audio device plugged in: the device the person plugged in
        let r = &t.0;
        mac_mini_t2(&t, false);
        let cards = read(r.join("asound/cards"));
        put(r, "asound/cards", &format!("{cards} 2 [Device         ]: USB-Audio - USB Audio Device\n"));
        put(r, "asound/card2/usbid", "0d8c:0014\n");
        pcm(r, 2, 0, "USB Audio");
        let outs = scan(r, &|c| if c == 2 { None } else { t2_mixer(c, false, false) });
        assert_eq!(choose(&outs, "").output.unwrap().alsa(), "plughw:2,0");
        // the person can still choose the speaker in Settings
        assert_eq!(choose(&outs, "Audio:0").output.unwrap().alsa(), "plughw:1,0");
    }

    #[test]
    fn a_monitor_unplugged_from_a_t2_mac_moves_the_next_sound_to_the_built_in_speaker() {
        let t = tree("t2-hotplug");
        mac_mini_t2(&t, true);
        let monitor = Arc::new(Mutex::new(true));
        let m2 = monitor.clone();
        let sel = Selector::new(t.0.clone(), move |c| t2_mixer(c, *m2.lock().unwrap(), false), || false);
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:0,3"));
        let g = sel.generation();
        *monitor.lock().unwrap() = false;
        for p in 0..9 {
            put(&t.0, &format!("asound/card0/eld#2.{p}"), &eld_file(false, ""));
        }
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:1,0"));
        assert!(sel.generation() > g);
    }

    #[test]
    fn a_pc_with_analog_and_hdmi_prefers_connected_headphones_then_the_monitor() {
        let t = tree("pc");
        let r = &t.0;
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7f10000 irq 32\n 1 [NVidia         ]: HDA-Intel - HDA NVidia\n                      HDA NVidia at 0xf7080000 irq 17\n");
        pcm(r, 0, 0, "ALC892 Analog");
        pcm(r, 0, 1, "ALC892 Digital");
        pcm(r, 1, 3, "HDMI 0");
        pcm(r, 1, 7, "HDMI 1");
        let headphones = std::sync::Arc::new(Mutex::new(true));
        let h2 = headphones.clone();
        let mixer = move |card: u32| -> Option<String> {
            Some(if card == 0 {
                format!("numid=1,iface=CARD,name='Front Headphone Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={}\nnumid=2,iface=CARD,name='Line Out Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=off\nnumid=3,iface=CARD,name='Front Mic Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=on\n", if *h2.lock().unwrap() { "on" } else { "off" })
            } else {
                format!("numid=1,iface=CARD,name='HDMI/DP,pcm=3 Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=off\nnumid=2,iface=CARD,name='HDMI/DP,pcm=7 Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=on\nnumid=3,iface=PCM,name='ELD',device=7\n  ; type=BYTES,access=r--v----,values=1\n  : values={}\n", eld_bytes("DELL U2415", 2))
            })
        };
        let outs = scan(r, &mixer);
        assert_eq!(outs.iter().map(|o| o.kind).collect::<Vec<_>>(), vec![Kind::Analog, Kind::Digital, Kind::Hdmi, Kind::Hdmi]);
        assert_eq!(choose(&outs, "").output.unwrap().alsa(), "plughw:0,0", "headphones first");
        *headphones.lock().unwrap() = false;
        let outs = scan(r, &mixer);
        assert_eq!(outs[0].connected, Some(false));
        let analog_only: Vec<Output> = outs.iter().filter(|o| o.kind == Kind::Analog).cloned().collect();
        let c = choose(&analog_only, "");
        assert!(c.output.is_some() && c.reason.contains("nothing is reported plugged in"), "an analog output with every jack off is the last choice, not silence: {}", c.reason);
        let c = choose(&outs, "");
        assert_eq!(c.output.as_ref().unwrap().alsa(), "plughw:1,7", "then the monitor that is connected");
        assert_eq!(c.output.unwrap().label(), "HDMI: DELL U2415");
        // no amixer and no ELD files: the analog output is used (as plain `default` would have)
        let outs = scan(r, &none);
        assert_eq!(choose(&outs, "").output.unwrap().alsa(), "plughw:0,0");
    }

    #[test]
    fn a_usb_card_alone_is_used_and_a_machine_with_nothing_says_so() {
        let t = tree("usb");
        let r = &t.0;
        put(r, "asound/cards", " 1 [Device         ]: USB-Audio - USB Audio Device\n                      C-Media USB Audio Device at usb-0000:00:14.0-2, full speed\n");
        put(r, "asound/card1/usbid", "0d8c:0014\n");
        pcm(r, 1, 0, "USB Audio");
        let outs = scan(r, &none);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].kind, Kind::Usb);
        assert_eq!(outs[0].label(), "USB Audio: USB Audio Device");
        let c = choose(&outs, "");
        assert_eq!(c.output.unwrap().alsa(), "plughw:1,0");
        assert!(c.reason.contains("USB"));
        let empty = tree("nothing");
        let outs = scan(&empty.0, &none);
        assert!(outs.is_empty());
        let c = choose(&outs, "");
        assert!(c.output.is_none() && c.reason.starts_with(NO_OUTPUT) && c.reason.contains("no sound card"), "{}", c.reason);
    }

    #[test]
    fn a_monitor_plugged_in_later_or_moved_switches_the_next_sound() {
        let t = tree("hotplug");
        mac_mini(&t, false);
        let jack = Arc::new(Mutex::new(None::<u32>));
        let j2 = jack.clone();
        let sel = Selector::new(t.0.clone(), move |_| {
            let on = *j2.lock().unwrap();
            let mut s = String::new();
            for d in [3, 7, 8] {
                s += &format!("numid=1,iface=CARD,name='HDMI/DP,pcm={d} Jack'\n  : values={}\n", if on == Some(d) { "on" } else { "off" });
            }
            Some(s)
        }, || false);
        assert_eq!(sel.refresh().device, None);
        let g0 = sel.generation();
        *jack.lock().unwrap() = Some(3);
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:0,3"));
        let g1 = sel.generation();
        assert!(g1 > g0, "a change of device is a new generation");
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:0,3"));
        assert_eq!(sel.generation(), g1, "nothing changed, same generation");
        // the monitor moves to the second port
        *jack.lock().unwrap() = Some(7);
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:0,7"));
        assert!(sel.generation() > g1);
        // the next sound is routed there
        let args = route_args(vec!["aplay".into(), "-q".into()], sel.device().as_deref());
        assert_eq!(args, ["aplay", "-D", "plughw:0,7", "-q"]);
    }

    #[test]
    fn the_persons_choice_wins_and_falls_back_when_that_device_disappears() {
        let t = tree("choice");
        let r = &t.0;
        mac_mini(&t, true);
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n 1 [Device         ]: USB-Audio - USB Audio Device\n");
        put(r, "asound/card1/usbid", "0d8c:0014\n");
        pcm(r, 1, 0, "USB Audio");
        let sel = Selector::new(r.clone(), |c| (c == 0).then(|| mac_mixer(true, true)), || false);
        assert_eq!(sel.refresh().device.as_deref(), Some("plughw:0,3"), "automatic: the monitor before USB");
        sel.set_prefs(SoundSettings { output: "Device:0".into(), ..Default::default() });
        let st = sel.status();
        assert_eq!(st.device.as_deref(), Some("plughw:1,0"));
        assert!(st.choice.chosen && st.choice.reason.contains("chosen"));
        // the USB device is unplugged
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
        let st = sel.refresh();
        assert_eq!(st.device.as_deref(), Some("plughw:0,3"));
        assert!(!st.choice.chosen && st.choice.reason.contains("Device:0") && st.choice.reason.contains("not connected"), "{}", st.choice.reason);
        // a chosen output that has no monitor is still used: the person asked for it
        sel.set_prefs(SoundSettings { output: "PCH:8".into(), ..Default::default() });
        assert_eq!(sel.status().device.as_deref(), Some("plughw:0,8"));
    }

    #[test]
    fn a_sound_server_keeps_the_default_device_unless_an_output_is_chosen() {
        let t = tree("server");
        mac_mini(&t, true);
        let sel = Selector::new(t.0.clone(), |_| Some(mac_mixer(true, true)), || true);
        let st = sel.refresh();
        assert_eq!(st.device, None);
        assert!(st.summary().contains("sound server"));
        sel.set_prefs(SoundSettings { output: "PCH:3".into(), ..Default::default() });
        assert_eq!(sel.status().device.as_deref(), Some("plughw:0,3"));
    }

    #[test]
    fn the_device_goes_right_after_aplay_and_user_commands_with_a_device_are_untouched() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let d = Some("plughw:0,3");
        assert_eq!(route_args(s(&["aplay", "-q"]), d), s(&["aplay", "-D", "plughw:0,3", "-q"]));
        assert_eq!(route_args(s(&["/usr/bin/aplay", "-q", "-t", "wav"]), d), s(&["/usr/bin/aplay", "-D", "plughw:0,3", "-q", "-t", "wav"]));
        for kept in [&["aplay", "-q", "-D", "hw:1,0"][..], &["aplay", "-Dhw:1,0"], &["aplay", "-qD", "hw:1"], &["aplay", "--device=hw:1,0"], &["aplay", "--device", "hw:1,0"]] {
            assert_eq!(route_args(s(kept), d), s(kept), "{kept:?}");
        }
        for other in [&["pw-play", "-"][..], &["play", "-q", "-t", "wav", "-"], &["sh", "-c", "aplay -q"], &["cat"], &["tee", "/tmp/x"]] {
            assert_eq!(route_args(s(other), d), s(other), "{other:?}");
        }
        assert_eq!(route_args(s(&["aplay", "-q"]), None), s(&["aplay", "-q"]), "no device: unchanged");
        assert_eq!(route_args(vec![], d), Vec::<String>::new());
        // a command line survives the round trip through the splitter (no shell)
        let args = s(&["aplay", "-D", "plughw:0,3", "a b", "it's"]);
        assert_eq!(crate::voice::split_command(&join_command(&args)).unwrap(), args);
        assert_eq!(command_with_device("aplay -q", "plughw:0,7").unwrap(), "aplay -D plughw:0,7 -q");
        assert_eq!(command_with_device("aplay -q -D hw:2,0", "plughw:0,7").unwrap(), "aplay -D plughw:0,7 -q", "Test sound on one output uses that output");
        assert_eq!(command_with_device("aplay -qDhw:2,0 -t wav", "plughw:0,7").unwrap(), "aplay -D plughw:0,7 -q -t wav");
        assert_eq!(command_with_device("aplay --device=hw:2 -q", "plughw:0,7").unwrap(), "aplay -D plughw:0,7 -q");
        assert_eq!(command_with_device("pw-play -", "plughw:0,7").unwrap(), "aplay -q -D plughw:0,7");
    }

    #[test]
    fn software_volume_scales_samples_and_wav_data_and_mute_is_silence() {
        let mut pcm = vec![10000i16, -10000, 32767, -32768];
        scale(&mut pcm, 50);
        assert_eq!(pcm, vec![5000, -5000, 16383, -16384]);
        let mut same = vec![1234i16];
        scale(&mut same, 100);
        assert_eq!(same, vec![1234]);
        let wav = crate::voice::encode_wav(&[8000, -8000, 4000], 16_000);
        let half = crate::voice::parse_wav(&scale_wav(&wav, 50)).unwrap();
        assert_eq!(half.samples, vec![4000, -4000, 2000]);
        let muted = crate::voice::parse_wav(&scale_wav(&wav, 0)).unwrap();
        assert!(muted.samples.iter().all(|s| *s == 0));
        assert_eq!(scale_wav(&wav, 100), wav, "full volume leaves the bytes alone");
        assert_eq!(scale_wav(b"not a wav at all, just some bytes here......", 10), b"not a wav at all, just some bytes here......".to_vec());
        // a streamed WAV whose data size is unknown (0xFFFFFFFF)
        let mut stream = wav.clone();
        stream[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        let out = scale_wav(&stream, 50);
        assert_eq!(i16::from_le_bytes([out[44], out[45]]), 4000);
        assert_eq!(SoundSettings { volume_pct: 40, muted: true, ..Default::default() }.gain(), 0);
        assert_eq!(SoundSettings { volume_pct: 40, ..Default::default() }.gain(), 40);
        let sel = Selector::new("/nonexistent", none, || false);
        sel.set_prefs(SoundSettings { volume_pct: 30, ..Default::default() });
        assert_eq!(sel.gain(), 30);
    }

    #[test]
    fn controls_eld_bytes_and_settings_are_parsed_and_checked() {
        let c = parse_controls(&mac_mixer(true, true));
        assert_eq!(c.jacks.len(), 3);
        assert_eq!(c.jacks[0], ("HDMI/DP,pcm=3 Jack".to_string(), true));
        assert_eq!(c.iec958, vec![(0, true), (1, true), (2, true)]);
        let (dev, bytes) = &c.eld[0];
        assert_eq!(*dev, 3);
        assert_eq!(eld_monitor(bytes), Some(("PDM-15T".to_string(), 1)));
        assert!(c.eld[1].1.is_empty());
        assert_eq!(parse_bytes("10:00:08"), vec![0x10, 0, 8]);
        assert_eq!(parse_bytes("100008"), vec![0x10, 0, 8]);
        assert_eq!(eld_monitor(&[0u8; 10]), None);
        assert!(SoundSettings::default().validate().is_ok());
        assert!(SoundSettings { volume_pct: 101, ..Default::default() }.validate().is_err());
        assert!(SoundSettings { output: "PCH:3; rm".into(), ..Default::default() }.validate().is_err());
        // a switched-off digital output is said
        let t = tree("iec");
        mac_mini(&t, true);
        let outs = scan(&t.0, &|_| Some(mac_mixer(true, true).replacen("values=on\nnumid=30", "values=off\nnumid=30", 1)));
        assert!(outs[0].digital_off && outs[0].state().contains("switched off"), "{:?}", outs[0]);
        assert_eq!(parse_cards(" 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1 irq 3\n"), vec![(0, "PCH".into(), "HDA-Intel".into(), "HDA Intel PCH".into())]);
    }

    /// A T2 Mac mini whose monitor and headphones come and go, seen by a selector that keeps an asound.conf in `t`.
    fn t2_with_softvol(t: &Tree, monitor: Arc<Mutex<bool>>, headphones: Arc<Mutex<bool>>, fake: crate::softvol::fake::Fake) -> Selector {
        mac_mini_t2(t, *monitor.lock().unwrap());
        let (m, h) = (monitor.clone(), headphones.clone());
        Selector::new(t.0.clone(), move |c| t2_mixer(c, *m.lock().unwrap(), *h.lock().unwrap()), || false).with_softvol(crate::softvol::Softvol::new(t.0.join("asound.conf"), fake))
    }

    fn plug_monitor(t: &Tree, on: bool, monitor: &Arc<Mutex<bool>>) {
        *monitor.lock().unwrap() = on;
        for p in 0..9 {
            put(&t.0, &format!("asound/card0/eld#2.{p}"), &eld_file(on && p == 6, "PDM-15T"));
        }
    }

    #[test]
    fn a_t2_speaker_without_a_volume_control_gets_a_software_volume_that_carries_the_slider_and_mute() {
        let t = tree("softvol-t2");
        let fake = crate::softvol::fake::Fake::default();
        let sel = t2_with_softvol(&t, Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)), fake.clone());
        sel.set_prefs(SoundSettings { volume_pct: 30, ..Default::default() });
        let st = sel.refresh();
        assert!(st.outputs.iter().all(|o| o.hw_volume.is_none()), "neither card has a hardware volume");
        assert_eq!(st.device.as_deref(), Some("iyakki"), "players go through the softvol");
        assert_eq!(st.softvol, Some(1));
        let conf = std::fs::read_to_string(t.0.join("asound.conf")).unwrap();
        assert_eq!(conf, crate::softvol::config_text(&crate::softvol::Plan::Mix { card: 1, device: 0, mix: crate::softvol::Mix::T2_MONO }));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"]);
        assert_eq!(sel.gain(), 100, "the softvol scales: IYAKKI's players leave their samples alone");
        assert_eq!(sel.prefs_gain(), 30, "a sound sent to one output by hand still gets the volume");
        assert_eq!(route_args(vec!["aplay".into(), "-q".into()], sel.device().as_deref()), ["aplay", "-D", "iyakki", "-q"]);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(gain_for(&sel, &s(&["aplay", "-D", "iyakki", "-q"])), 100);
        assert_eq!(gain_for(&sel, &s(&["aplay", "-D", "plughw:0,3", "-q"])), 30);
        assert_eq!(gain_for(&sel, &s(&["aplay", "-qDplughw:0,3"])), 30);
        // mute: 0 % on the control (softvol has no switch), and IYAKKI's own players play silence too; unmute restores 30 %
        sel.set_prefs(SoundSettings { volume_pct: 30, muted: true, ..Default::default() });
        assert_eq!(sel.gain(), 0);
        sel.set_prefs(SoundSettings { volume_pct: 30, ..Default::default() });
        assert_eq!(sel.gain(), 100);
        assert_eq!(fake.calls.lock().unwrap()[2..], ["amixer -q -c 1 sset IYAKKI 0%", "amixer -q -c 1 sset IYAKKI 30%"]);
    }

    #[test]
    fn switching_outputs_rewrites_the_config_restarts_players_and_restores_each_outputs_volume() {
        let t = tree("softvol-switch");
        let fake = crate::softvol::fake::Fake::default();
        let (monitor, headphones) = (Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)));
        let sel = t2_with_softvol(&t, monitor.clone(), headphones.clone(), fake.clone());
        let saved = Arc::new(Mutex::new(Vec::<u32>::new()));
        let s2 = saved.clone();
        sel.set_on_prefs(move |p| s2.lock().unwrap().push(p.volume_pct));
        let mut levels = std::collections::BTreeMap::new();
        levels.insert("Audio:0".to_string(), 30);
        levels.insert("PCH:3".to_string(), 70);
        sel.set_prefs(SoundSettings { volume_pct: 30, levels, ..Default::default() });
        assert_eq!(sel.refresh().device.as_deref(), Some("iyakki"));
        assert_eq!(sel.current_key().as_deref(), Some("Audio:0"));
        let g = sel.generation();
        // a monitor with speakers is plugged in: the file follows it, a playing radio restarts (same device name)
        plug_monitor(&t, true, &monitor);
        let st = sel.refresh();
        assert_eq!(st.device.as_deref(), Some("iyakki"));
        assert!(sel.generation() > g, "the hardware behind `iyakki` changed");
        assert!(std::fs::read_to_string(t.0.join("asound.conf")).unwrap().contains("pcm \"hw:0,3\""));
        assert_eq!(st.prefs.volume_pct, 70, "the monitor's own volume is back");
        assert_eq!(*saved.lock().unwrap(), [70], "and saved to the settings");
        assert_eq!(st.softvol, Some(0));
        // headphones: no remembered volume, the current one stays
        *headphones.lock().unwrap() = true;
        let st = sel.refresh();
        assert!(std::fs::read_to_string(t.0.join("asound.conf")).unwrap().contains("pcm \"hw:1,2\""));
        assert_eq!(st.prefs.volume_pct, 70);
        // both unplugged: back on the speaker at 30 %
        *headphones.lock().unwrap() = false;
        plug_monitor(&t, false, &monitor);
        assert_eq!(sel.refresh().prefs.volume_pct, 30);
        assert_eq!(*saved.lock().unwrap(), [70, 30]);
        let c = fake.calls.lock().unwrap().clone();
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%", "prime iyakki", "amixer -q -c 0 sset IYAKKI 70%", "prime iyakki", "amixer -q -c 1 sset IYAKKI 70%", "prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"]);
        // remembering: the volume of the output in use goes into `levels`
        let mut p = sel.prefs();
        p.volume_pct = 45;
        p.remember_level(&sel.current_key().unwrap());
        assert_eq!(p.levels.get("Audio:0"), Some(&45));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn an_output_with_a_hardware_volume_keeps_it_and_a_sound_server_gets_no_file() {
        let t = tree("softvol-hw");
        let r = &t.0;
        put(r, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
        pcm(r, 0, 0, "ALC892 Analog");
        pcm(r, 0, 3, "HDMI 0");
        let mixer = |_| Some("numid=1,iface=MIXER,name='Master Playback Volume'\n  ; type=INTEGER,access=rw---R--,values=1,min=0,max=87,step=0\n  : values=60\nnumid=2,iface=CARD,name='Headphone Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values=on\n".to_string());
        let fake = crate::softvol::fake::Fake::default();
        let sel = Selector::new(r.clone(), mixer, || false).with_softvol(crate::softvol::Softvol::new(r.join("asound.conf"), fake.clone()));
        sel.set_prefs(SoundSettings { volume_pct: 40, ..Default::default() });
        let st = sel.refresh();
        assert_eq!(st.outputs[0].hw_volume.as_deref(), Some("Master"));
        assert_eq!(st.outputs[1].hw_volume, None, "HDMI on the same card is not driven by its Master");
        assert_eq!(st.device.as_deref(), Some("iyakki"));
        assert_eq!(st.softvol, None);
        let conf = std::fs::read_to_string(r.join("asound.conf")).unwrap();
        assert_eq!(conf, crate::softvol::config_text(&crate::softvol::Plan::Hw { card: 0, device: 0 }));
        assert!(fake.calls.lock().unwrap().is_empty(), "the hardware control is not touched");
        assert_eq!(sel.gain(), 40, "IYAKKI keeps scaling its own sound as before");
        // the HDMI output chosen on that card: a softvol after all
        sel.set_prefs(SoundSettings { volume_pct: 40, output: "PCH:3".into(), ..Default::default() });
        assert_eq!(sel.status().softvol, Some(0));
        assert!(std::fs::read_to_string(r.join("asound.conf")).unwrap().contains("pcm \"hw:0,3\""));
        // a sound server: IYAKKI's file goes and `-D plughw` routing is as before for a chosen output
        let t2 = tree("softvol-server");
        mac_mini(&t2, true);
        let fake = crate::softvol::fake::Fake::default();
        std::fs::write(t2.0.join("asound.conf"), crate::softvol::config_text(&crate::softvol::Plan::Soft { card: 0, device: 3 })).unwrap();
        let sel = Selector::new(t2.0.clone(), |_| Some(mac_mixer(true, true)), || true).with_softvol(crate::softvol::Softvol::new(t2.0.join("asound.conf"), fake.clone()));
        assert_eq!(sel.refresh().device, None);
        assert!(!t2.0.join("asound.conf").exists());
        sel.set_prefs(SoundSettings { output: "PCH:3".into(), ..Default::default() });
        assert_eq!(sel.status().device.as_deref(), Some("plughw:0,3"));
        assert!(!t2.0.join("asound.conf").exists() && fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_global_selector_does_nothing_until_installed() {
        // tests and tools never route or scale the machine's sound
        assert_eq!(global().device(), None);
        assert_eq!(global().gain(), 100);
        assert!(!global().is_active());
        assert_eq!(global().softvol_card(), None);
        assert_eq!(global().current_key(), None);
        let tone = crate::voice::parse_wav(&test_tone_wav()).unwrap();
        assert!(tone.samples.len() > 8000 && crate::voice::peak(&tone.samples) < 9000, "short and gentle");
    }
}
