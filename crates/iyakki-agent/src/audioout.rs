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

use iyakki_core_api::AudioRoute;
use iyakki_core_local::LocalAudioRoute;

pub use iyakki_shared::audioout::{Master, MASTER_GEN, Kind, NO_OUTPUT, STARTING_LINE, VolumeMode, SoundSettings, Output, Choice, choose, output_key_ok, DEFAULT_VOLUME_PCT, DEFAULT_BOOST_DB, MAX_LEVELS, SYSTEM_DEFAULT_ID};


pub const WATCH_EVERY: Duration = Duration::from_secs(3);
/// During the first minute after boot the watcher checks every 2 s whether a sound card or PCM appeared (the built-in
/// speakers of a MacBookPro14,1 appear about 11 s after boot); a change is looked at at once.
pub const WATCH_EARLY: Duration = Duration::from_secs(2);
pub const EARLY_SECS: u64 = 60;
/// A sound card that is slow to start is waited for this long before it counts as missing.
pub const STARTING_SECS: u64 = 30;
/// What the CS8409 of the MacBookPro14,1 accepts (measured): 32-bit, 44.1 kHz, four channels. Used when the card cannot be asked.
pub const CS8409_MIX: crate::softvol::Mix = crate::softvol::Mix { format: "S32_LE", channels: 4, sized: true, rate: 44_100, probed: true };

// ------------------------------------------------------------------------------------------------ settings




/// How programs share the chosen output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sharing {
    /// One program at a time (`plughw`/`hw` under the software volume): the others get `Busy`.
    #[default]
    OneAtATime,
    /// ALSA `dmix` (devices where it is cheap).
    Dmix,
    /// IYAKKI's own mixer ([`crate::mixer`]) is the one writer to the device and mixes the programs that use it.
    IyakkiMixer,
}

impl Sharing {
    pub fn label(self) -> &'static str {
        match self {
            Sharing::OneAtATime => "one program at a time",
            Sharing::Dmix => "shared (dmix)",
            Sharing::IyakkiMixer => "shared (IYAKKI mixer)",
        }
    }
}

/// How the planned ALSA configuration is shared. `dmix` is kept wherever it is the plan; a plan that is exclusive (fixed-period
/// devices such as the T2's, or a card with a hardware volume) uses the IYAKKI mixer when `sound.mixer` is on.
pub fn sharing_for(plan: Option<&crate::softvol::Plan>, mixer_on: bool) -> Sharing {
    use crate::softvol::Plan;
    match plan {
        Some(Plan::Mix { mix, .. }) if mix.sized => Sharing::Dmix,
        Some(_) if mixer_on => Sharing::IyakkiMixer,
        _ => Sharing::OneAtATime,
    }
}





// ------------------------------------------------------------------------------------------------ what the machine has






#[cfg(test)]
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

fn proc_elds(route: &dyn AudioRoute, card: &str) -> Vec<ProcEld> {
    let mut v: Vec<ProcEld> = route
        .asound_dir(card)
        .into_iter()
        .filter_map(|n| {
            let (c, p) = n.strip_prefix("eld#")?.split_once('.')?;
            let t = route.asound_text(&format!("{card}/{n}")).unwrap_or_default();
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

/// The channel count in a T2 card's driver name (`/proc/asound/cards` says `AppleT2x4 - Apple T2 Audio` for four).
fn t2_channels(cards: &str, card: u32) -> Option<u32> {
    let (_, _, driver, _) = parse_cards(cards).into_iter().find(|c| c.0 == card)?;
    driver.strip_prefix("AppleT2x")?.parse().ok().filter(|n| (1..=8).contains(n))
}


fn is_output_jack(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.contains("hdmi") && !n.contains("mic") && (n.contains("headphone") || n.contains("line out") || n.contains("speaker") || n.contains("headset"))
}

/// The kernel's list of sound cards (`asound/cards` under `proc_root`), empty when it cannot be read: the one reader of that file.
pub fn cards_text(proc_root: &Path) -> String {
    LocalAudioRoute::for_proc(proc_root).cards_text()
}

/// Every playback device of the machine. `mixer(card)` gives `amixer -c card contents` (None when there is no `amixer`).
pub fn scan(proc_root: &Path, mixer: &dyn Fn(u32) -> Option<String>) -> Vec<Output> {
    scan_via(&LocalAudioRoute::for_proc(proc_root), mixer)
}

/// [`scan`] over the kernel facts of an [`AudioRoute`] (the machine's, or a made-up one).
pub fn scan_via(route: &dyn AudioRoute, mixer: &dyn Fn(u32) -> Option<String>) -> Vec<Output> {
    let mut out = Vec::new();
    for (card, card_id, driver, card_name) in parse_cards(&route.cards_text()) {
        let low = format!("{driver} {card_name}").to_ascii_lowercase();
        if low.contains("loopback") || low.contains("dummy") {
            continue; // virtual cards nobody hears
        }
        let dir = format!("card{card}");
        let usb = route.asound_has(&format!("{dir}/usbid")) || low.contains("usb-audio") || low.contains("usb audio");
        let t2 = is_t2_card(&driver, &card_name);
        let bt = low.contains("bluetooth") || low.contains("bluealsa");
        let mut pcms: Vec<(u32, String, String)> = route
            .asound_dir(&dir)
            .into_iter()
            .filter_map(|n| {
                let dev: u32 = n.strip_prefix("pcm")?.strip_suffix('p')?.parse().ok()?;
                let info = route.asound_text(&format!("{dir}/{n}/info")).unwrap_or_default();
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
            let dummy = hw_volume.is_some() && hw_volume_is_dummy_via(route, card);
            let mut o = Output { card, device, card_id: card_id.clone(), card_name: card_name.clone(), pcm_name, kind, connected: None, monitor: None, speakers: None, hdmi_no: 0, digital_off: false, builtin_speaker, hw_volume, dummy_volume: dummy };
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
        fill_from_proc_eld(&mut outs, &proc_elds(route, &dir));
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
    // the kernel says no pin has a monitor: a jack or ELD control that still says "on" is stale (a monitor that was just
    // unplugged), and sound written to that pin goes nowhere
    if present.is_empty() {
        for &i in &hdmi {
            let o = &mut outs[i];
            o.connected = Some(false);
            o.monitor = None;
            o.speakers = None;
        }
        return;
    }
    // a monitor is reported but no jack control says so: trust the pins (as when the mixer could not be asked)
    let jacks_known = hdmi.iter().any(|&i| outs[i].connected == Some(true));
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





// ------------------------------------------------------------------------------------------------ play commands


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

pub use iyakki_shared::audioout::{to_pcm16_wav, names_device, join_command};

/// The rate low-rate speech is brought to before it is played (what nearly every sound card runs at).
pub const SPEECH_OUT_RATE: u32 = 48_000;

/// A 16-bit PCM WAV below 44.1 kHz (Kokoro speaks at 24 kHz) as a 48 kHz WAV. `None` when it is not a 16-bit PCM WAV or its
/// rate is already 44.1 kHz or more. Why: ALSA's rate converter in front of the T2 MacBook's sound card (fixed 48 kHz, one-frame
/// periods) let 24 kHz audio through unconverted, so speech played at twice the speed ("a parrot"); doing the conversion here does
/// not depend on the ALSA chain. Cubic (Catmull-Rom) interpolation: plenty for speech, which has nothing above half the old rate.
pub fn resample_low_rate_wav(wav: &[u8]) -> Option<Vec<u8>> {
    let w = crate::voice::parse_wav(wav).ok()?;
    let ch = w.channels as usize;
    if ch == 0 || w.rate == 0 || w.rate >= 44_100 || w.samples.len() < ch * 2 {
        return None;
    }
    let frames = w.samples.len() / ch;
    let out_frames = ((frames as u64 * SPEECH_OUT_RATE as u64) / w.rate as u64) as usize;
    let at = |f: isize, c: usize| -> f32 { w.samples[(f.clamp(0, frames as isize - 1) as usize) * ch + c] as f32 };
    let mut out: Vec<i16> = Vec::with_capacity(out_frames * ch);
    for n in 0..out_frames {
        let pos = n as f64 * w.rate as f64 / SPEECH_OUT_RATE as f64;
        let i = pos.floor() as isize;
        let t = (pos - i as f64) as f32;
        for c in 0..ch {
            let (p0, p1, p2, p3) = (at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c));
            let v = 0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
            out.push(v.clamp(-32768.0, 32767.0) as i16);
        }
    }
    let data_len = (out.len() * 2) as u32;
    let mut o = Vec::with_capacity(44 + out.len() * 2);
    o.extend_from_slice(b"RIFF");
    o.extend((36 + data_len).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend(16u32.to_le_bytes());
    o.extend(1u16.to_le_bytes());
    o.extend((ch as u16).to_le_bytes());
    o.extend(SPEECH_OUT_RATE.to_le_bytes());
    o.extend((SPEECH_OUT_RATE * ch as u32 * 2).to_le_bytes());
    o.extend((ch as u16 * 2).to_le_bytes());
    o.extend(16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend(data_len.to_le_bytes());
    for s in &out {
        o.extend(s.to_le_bytes());
    }
    Some(o)
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

// ------------------------------------------------------------------------------------------------ slow cards

/// The sound card of a MacBookPro13/14 or 2017-2019 iMac whose CS8409 codec is still setting up the Apple amplifiers (about
/// 8 s): it shows its HDMI devices already but not yet the `CS8409/CS42L83 Analog` one. Its number, if that is the state.
/// The model comes from the DMI name under `sys`; the codec from `/proc/asound/cardN/codec#*`.
pub fn card_starting(proc_root: &Path, sys: &Path) -> Option<u32> {
    card_starting_via(&LocalAudioRoute::at(proc_root, sys))
}

/// [`card_starting`] over an [`AudioRoute`].
pub fn card_starting_via(route: &dyn AudioRoute) -> Option<u32> {
    let product = route.product_name().unwrap_or_default();
    if !iyakki_core_local::kmodpack::is_cs8409_model(&product) {
        return None;
    }
    let outs = scan_via(route, &|_| None);
    for (card, _, _, _) in parse_cards(&route.cards_text()) {
        let cs8409 = is_cs8409_card(route, card);
        if cs8409 && !outs.iter().any(|o| o.card == card && o.kind == Kind::Analog) {
            return Some(card);
        }
    }
    None
}

/// The programs that have this playback device open (`/proc/<pid>/fd` links to `/dev/snd/pcmCcDdp`), by name.
pub fn pcm_holders(proc_root: &Path, card: u32, device: u32) -> Vec<String> {
    pcm_holders_via(&LocalAudioRoute::for_proc(proc_root), card, device)
}

/// [`pcm_holders`] over an [`AudioRoute`].
pub fn pcm_holders_via(route: &dyn AudioRoute, card: u32, device: u32) -> Vec<String> {
    let mut names = Vec::new();
    for h in route.pcm_holders(card, device) {
        let name = if h.name.is_empty() { format!("process {}", h.pid) } else { format!("{} ({})", h.name, h.pid) };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.truncate(4);
    names
}

/// The web engine's processes that hold this playback device open without playing anything. WebKit opens the sound device
/// when a page makes a media element or an audio context and keeps it (state PREPARED, or SETUP) until the page goes; on a
/// device that cannot be shared (the T2 Mac's: `dmix` underruns on it) that blocks the radio, the voice, and the sound test.
/// A holder is idle when the device's state is not RUNNING or PAUSED (a page that is playing is never touched).
pub fn idle_web_holders(proc_root: &Path, card: u32, device: u32) -> Vec<u32> {
    idle_web_holders_via(&LocalAudioRoute::for_proc(proc_root), card, device)
}

/// [`idle_web_holders`] over an [`AudioRoute`].
pub fn idle_web_holders_via(route: &dyn AudioRoute, card: u32, device: u32) -> Vec<u32> {
    let status = route.asound_text(&format!("card{card}/pcm{device}p/sub0/status")).unwrap_or_default();
    let state = status.lines().next().unwrap_or("").trim();
    let state = state.strip_prefix("state:").unwrap_or(state).trim();
    if matches!(state, "RUNNING" | "PAUSED" | "DRAINING") {
        return Vec::new();
    }
    route.pcm_holders(card, device).into_iter().filter(|h| h.name.starts_with("WPEWebProcess")).map(|h| h.pid).collect()
}

/// Ask the idle web processes holding the device to let go (SIGTERM: the helper starts a new web process for the page, which
/// reloads; a page that plays is never touched). How many were asked. Called when something else needs the device.
pub fn release_idle_web(proc_root: &Path, card: u32, device: u32) -> usize {
    release_idle_web_via(&LocalAudioRoute::for_proc(proc_root), card, device)
}

/// [`release_idle_web`] over an [`AudioRoute`] (the signal itself stays here: it is an action, not a read).
pub fn release_idle_web_via(route: &dyn AudioRoute, card: u32, device: u32) -> usize {
    let pids = idle_web_holders_via(route, card, device);
    for p in &pids {
        // SAFETY: a plain signal to a process found just now
        unsafe { libc::kill(*p as i32, libc::SIGTERM) };
    }
    pids.len()
}

/// "sound card is in use by <process>: stop it, then press Look again".
fn busy_line(route: &dyn AudioRoute, o: &Output) -> String {
    let who = pcm_holders_via(route, o.card, o.device);
    let who = if who.is_empty() { "another program".to_string() } else { who.join(", ") };
    format!("sound card is in use by {who}: stop it, then press Look again")
}

/// Is the codec of this card a Cirrus Logic CS8409 (`/proc/asound/cardN/codec#*` says `Codec: Cirrus Logic CS8409`)?
fn is_cs8409_card(route: &dyn AudioRoute, card: u32) -> bool {
    let dir = format!("card{card}");
    route.asound_dir(&dir).into_iter().filter(|n| n.starts_with("codec#")).any(|n| {
        let t = route.asound_text(&format!("{dir}/{n}")).unwrap_or_default();
        t.lines().any(|l| l.starts_with("Codec:") && l.to_ascii_uppercase().contains("CS8409"))
    })
}

/// A hardware volume control that is known to do nothing. The CS8409 codec of the MacBookPro13/14 and iMac18/19 (the driver of
/// the Apple path) shows `PCM Playback Volume` with a decibel scale, but the owner played one tone at 255, 128, 51 and 13
/// straight to `hw:0,0` and all four sounded the same: the amplifiers have a fixed gain. IYAKKI then uses its software volume
/// there, unless Settings -> Sound -> Advanced says otherwise for that card ([`VolumeMode`]).
pub fn hw_volume_is_dummy(proc_root: &Path, card: u32) -> bool {
    hw_volume_is_dummy_via(&LocalAudioRoute::for_proc(proc_root), card)
}

/// [`hw_volume_is_dummy`] over an [`AudioRoute`].
pub fn hw_volume_is_dummy_via(route: &dyn AudioRoute, card: u32) -> bool {
    is_cs8409_card(route, card)
}

/// What changes when a card or a PCM appears or goes: `/proc/asound/cards` and `/proc/asound/pcm`, and per card which `eld#*`
/// pins report a monitor (the kernel changes them when an HDMI/DP monitor comes or goes).
pub fn sound_signature(proc_root: &Path) -> String {
    sound_signature_via(&LocalAudioRoute::for_proc(proc_root))
}

/// [`sound_signature`] over an [`AudioRoute`].
pub fn sound_signature_via(route: &dyn AudioRoute) -> String {
    let mut s = format!("{}\n--\n{}", route.cards_text(), route.asound_text("pcm").unwrap_or_default());
    // `asound_dir` is sorted
    let cards: Vec<String> = route.asound_dir("").into_iter().filter(|n| n.starts_with("card")).collect();
    for name in cards {
        for e in proc_elds(route, &name) {
            s += &format!("\n{name}:{}.{}={}:{}", e.order.0, e.order.1, e.present, e.name.as_deref().unwrap_or(""));
        }
    }
    s
}

/// Is a full look due: the cards or PCMs changed, or [`WATCH_EVERY`] has gone by.
pub fn look_due(signature_changed: bool, since_last_look: Duration) -> bool {
    signature_changed || since_last_look >= WATCH_EVERY
}

/// How long the watcher waits between checks: [`WATCH_EARLY`] while the computer has been up for less than a minute.
pub fn watch_wait(uptime: Duration) -> Duration {
    if uptime < Duration::from_secs(EARLY_SECS) { WATCH_EARLY } else { WATCH_EVERY }
}

fn uptime() -> Duration {
    let up = crate::sysinfo::uptime_f64(crate::sysinfo::proc_root()).unwrap_or(1e9);
    Duration::from_secs_f64(up.clamp(0.0, 1e9))
}

// ------------------------------------------------------------------------------------------------ test patterns

/// Rate of the test patterns.
pub const PATTERN_RATE: u32 = 48_000;

fn wav_pcm16(channels: u16, rate: u32, frames: &[i16]) -> Vec<u8> {
    let data = (frames.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + frames.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in frames {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// A 1 s tone (with short fades, no clicks) on one channel of `channels`, one after the other for `order` (the channel
/// numbers to play, 1 s each); `amp` is the peak in 0 to 1.
fn pattern(channels: u16, order: &[usize], freq: f32, amp: f32) -> Vec<u8> {
    let n = PATTERN_RATE as usize;
    let mut frames = Vec::with_capacity(n * order.len() * channels as usize);
    for &ch in order {
        for i in 0..n {
            let t = i as f32 / PATTERN_RATE as f32;
            let fade = (i.min(n - 1 - i) as f32 / 1200.0).min(1.0);
            let v = ((2.0 * std::f32::consts::PI * freq * t).sin() * amp * fade * i16::MAX as f32) as i16;
            for c in 0..channels as usize {
                frames.push(if c == ch { v } else { 0 });
            }
        }
    }
    wav_pcm16(channels, PATTERN_RATE, &frames)
}

/// Stereo test: 1 s on the left speaker, then 1 s on the right one (2-channel WAV, 48 kHz).
pub fn stereo_test_wav() -> Vec<u8> {
    pattern(2, &[0, 1], 523.25, 0.5)
}

/// Channel walk for a card with `channels` channels (more than two): each channel alone for 1 s (WAV, 48 kHz). It goes straight to the
/// card, past the software volume and its boost, so it needs a level of its own: the Stereo test is half scale and then +12 dB of boost
/// on a quiet card (a Mac's speakers), and a walk at 0.2 was inaudible there. The person's volume still scales it down.
pub fn channel_walk_wav(channels: u16) -> Vec<u8> {
    let order: Vec<usize> = (0..channels as usize).collect();
    pattern(channels, &order, 440.0, 0.7)
}

/// The self-check tone: 2 s of 440 Hz on both channels.
pub fn check_tone_wav() -> Vec<u8> {
    let n = 2 * PATTERN_RATE as usize;
    let mut frames = Vec::with_capacity(n * 2);
    for i in 0..n {
        let fade = (i.min(n - 1 - i) as f32 / 1200.0).min(1.0);
        let v = ((2.0 * std::f32::consts::PI * 440.0 * i as f32 / PATTERN_RATE as f32).sin() * 0.4 * fade * i16::MAX as f32) as i16;
        frames.push(v);
        frames.push(v);
    }
    wav_pcm16(2, PATTERN_RATE, &frames)
}

/// The first `[NN%]` of `amixer sget` output.
pub fn percent_of(sget: &str) -> Option<u32> {
    let i = sget.find('[')?;
    let rest = &sget[i + 1..];
    rest.split_once("%]").and_then(|(n, _)| n.trim().parse::<u32>().ok()).filter(|p| *p <= 100)
}

/// "Does the hardware volume work?" without a microphone: the same tone at 100 % and then at 12 % of the card's own control
/// (`control`, e.g. `PCM`), played straight on the card; the person says whether the second was quieter. The control is put
/// back to its old level afterwards, also when a tone failed. `amixer` runs `amixer` with these arguments, `play` plays a
/// WAV on an ALSA device.
pub fn hw_volume_check(amixer: &dyn Fn(&[String]) -> Result<String, String>, play: &dyn Fn(&str, &[u8]) -> Result<(), String>, card: u32, device: u32, control: &str) -> Result<String, String> {
    let a = |args: &[&str]| amixer(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let c = card.to_string();
    let old = a(&["-c", &c, "sget", control]).ok().and_then(|t| percent_of(&t));
    let dev = format!("plughw:{card},{device}");
    let wav = check_tone_wav();
    let result = (|| {
        a(&["-q", "-c", &c, "sset", control, "100%"])?;
        play(&dev, &wav)?;
        a(&["-q", "-c", &c, "sset", control, "12%"])?;
        play(&dev, &wav)
    })();
    let _ = a(&["-q", "-c", &c, "sset", control, &format!("{}%", old.unwrap_or(100))]);
    result.map(|_| "Played the same tone twice on the card itself: first with its hardware volume at 100 %, then at about 12 %.".to_string())
}

/// Play a WAV on one ALSA device with `aplay` (waits for the end, up to `secs`).
pub fn play_wav_on(device: &str, wav: &[u8], secs: u64) -> Result<(), String> {
    let args: Vec<String> = ["aplay", "-q", "-D", device].iter().map(|s| s.to_string()).collect();
    crate::voice::run_capture(crate::voice::command_for(&args, None), Some(wav), Duration::from_secs(secs)).map(|_| ()).map_err(|e| format!("aplay -D {device}: {e}"))
}

/// [`hw_volume_check`] with the real `amixer` and `aplay`.
pub fn hw_volume_check_system(card: u32, device: u32, control: &str) -> Result<String, String> {
    let amixer = |args: &[String]| -> Result<String, String> {
        iyakki_core_local::starters::amixer_text(args).map_err(|f| format!("amixer: {}", f.text))
    };
    hw_volume_check(&amixer, &|d, w| play_wav_on(d, w, 6), card, device, control)
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
    /// How the shared device behind the software volume is opened (None: no shared device, or not IYAKKI's file).
    pub mix: Option<crate::softvol::Mix>,
    /// A sound card is still starting up (see [`card_starting`]): the built-in speakers of this Mac are not there yet.
    pub starting: Option<String>,
    /// The card whose own hardware volume control is NOT used although it has one (a dummy one, or Software chosen).
    pub hw_ignored: Option<u32>,
    /// The sound card is held by another program, so the volume control could not be created: the sentence to show.
    pub busy: Option<String>,
    /// How programs share the output.
    pub sharing: Sharing,
}

impl Status {
    /// One line for the SYSTEM tile and the Devices popover.
    pub fn summary(&self) -> String {
        let vol = if self.prefs.muted { " · muted".to_string() } else if self.prefs.volume_pct < 100 { format!(" · volume {}%", self.prefs.volume_pct) } else { String::new() };
        let vol = if self.sharing == Sharing::IyakkiMixer { format!("{vol} · IYAKKI mixer") } else { vol };
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
    mix: Option<crate::softvol::Mix>,
    hw_ignored: Option<u32>,
    busy: Option<String>,
    sharing: Sharing,
    /// The outputs of the last look, to tell a new one from one that was always there.
    seen_keys: Vec<String>,
    /// When a starting card was first seen (the 30 s window of [`STARTING_SECS`]).
    starting_since: Option<Instant>,
    starting_over: bool,
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
    /// The kernel facts (`/proc/asound`, who holds a device): the machine's under `root` unless a test or the core gives its own.
    route: std::sync::Arc<dyn AudioRoute>,
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
    /// Asks a device what it accepts (the analog cards that are not the T2's); None: the long-standing 16-bit stereo dmix.
    prober: Option<crate::soundprobe::Prober>,
    /// Where `/sys` is (for the model name, see [`card_starting`]); None: no look for starting cards.
    sys: Option<PathBuf>,
    /// Sentences for the front end ("Sound output ready: ..."), taken with [`take_notices`](Selector::take_notices).
    notices: Mutex<Vec<String>>,
}

impl Selector {
    /// A selector over `proc_root` (normally `/`, so `/proc/asound` is read). `mixer(card)` is `amixer -c card contents`;
    /// `server()` says whether a sound server runs.
    pub fn new(proc_root: impl Into<PathBuf>, mixer: impl Fn(u32) -> Option<String> + Send + Sync + 'static, server: impl Fn() -> bool + Send + Sync + 'static) -> Selector {
        let prefs = SoundSettings::default();
        let root: PathBuf = proc_root.into();
        Selector {
            route: std::sync::Arc::new(LocalAudioRoute::for_proc(&root)),
            root,
            mixer: Box::new(mixer),
            server: Box::new(server),
            state: Mutex::new(State { prefs, outputs: vec![], choice: choose(&[], ""), server: false, target: None, device: None, softvol: None, mix: None, hw_ignored: None, busy: None, sharing: Sharing::OneAtATime, seen_keys: vec![], starting_since: None, starting_over: false, level_key: None, at: None }),
            generation: AtomicU64::new(0),
            gain: AtomicU32::new(100),
            active: AtomicBool::new(true),
            softvol: None,
            soft_on: AtomicBool::new(false),
            on_prefs: Mutex::new(None),
            prober: None,
            sys: None,
            notices: Mutex::new(Vec::new()),
        }
    }

    /// Learn what each analog device accepts (see [`crate::soundprobe`]) instead of assuming 16-bit stereo at 48 kHz.
    pub fn with_prober(mut self, prober: crate::soundprobe::Prober) -> Selector {
        self.prober = Some(prober);
        self
    }

    /// Read the sound facts through this handle (the core's, or a made-up one) instead of the machine under the root. Call it after [`with_sys`](Selector::with_sys), which sets the machine's own.
    pub fn with_route(mut self, route: std::sync::Arc<dyn AudioRoute>) -> Selector {
        self.route = route;
        self
    }

    /// Look at the model name under this `/sys` to tell a slow-starting sound card from a missing one.
    pub fn with_sys(mut self, sys: impl Into<PathBuf>) -> Selector {
        let sys: PathBuf = sys.into();
        self.route = std::sync::Arc::new(LocalAudioRoute::at(&self.root, &sys));
        self.sys = Some(sys);
        self
    }

    /// What the front end should say now (a sound output that appeared); each is returned once.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Is a sound card still starting (for at most [`STARTING_SECS`])? The line to show, if so.
    pub fn starting(&self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        self.lock().starting_since.map(|_| STARTING_LINE.to_string())
    }

    /// Why the volume cannot be changed right now, in a sentence for the F-keys and Settings (None: it can). Only once
    /// [`install`]ed; a person pressing a volume key must hear about it instead of nothing happening.
    pub fn volume_note(&self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        let st = self.lock();
        if st.server {
            return None;
        }
        if st.choice.output.is_none() {
            return Some(if st.starting_since.is_some() { "Volume control not ready: starting".to_string() } else { "No sound output".to_string() });
        }
        if let Some(b) = &st.busy {
            return Some(format!("Volume control not ready: {b}"));
        }
        None
    }

    /// The channel count the shared device behind the volume has (None: not a shared device).
    pub fn chain_channels(&self) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        self.lock().mix.map(|m| m.channels)
    }

    /// The dmix parameters for this output: probed for an analog codec, else what [`mix_for`] says.
    fn mix_of(&self, o: &Output) -> crate::softvol::Mix {
        let mut default = mix_for(o);
        // a T2 speaker with more channels than the one the table assumes (`AppleT2x4`: four, the only count its device takes)
        if o.builtin_speaker {
            if let Some(n) = t2_channels(&self.route.cards_text(), o.card).filter(|n| *n > default.channels) {
                default.channels = n;
            }
        }
        let Some(p) = self.prober.as_ref().filter(|_| o.kind == Kind::Analog && !o.card_name.contains("Apple T2")) else { return default };
        let cs8409 = is_cs8409_card(&*self.route, o.card);
        match (p.constraints(o.card, o.device), cs8409) {
            // the CS8409 of the Apple path: all four speakers get the sound (two channels would reach only a pair of them)
            (Some(c), true) => {
                let m = c.mix().unwrap_or(CS8409_MIX);
                if c.channels.is_empty() || c.channels.iter().any(|r| r.lo <= 4 && 4 <= r.hi) { crate::softvol::Mix { channels: 4, probed: true, ..m } } else { m }
            }
            (Some(c), false) => c.mix().unwrap_or(default),
            // the card is busy (TALES or a player holds it) or does not answer: what was measured on these Macs
            (None, true) => CS8409_MIX,
            (None, false) => default,
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
        Status { outputs: st.outputs.clone(), choice: st.choice.clone(), sound_server: st.server, prefs: st.prefs.clone(), device: st.device.clone(), softvol: st.softvol, mix: st.mix, starting: st.starting_since.map(|_| STARTING_LINE.to_string()), hw_ignored: st.hw_ignored, busy: st.busy.clone(), sharing: st.sharing }
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
                    st.choice.output.as_ref().map(|o| if o.uses_hw_volume(st.prefs.volume_mode_for(&o.card_id)) { crate::softvol::Plan::Hw { card: o.card, device: o.device } } else { crate::softvol::Plan::Mix { card: o.card, device: o.device, mix: self.mix_of(o) } })
                };
                st.sharing = sharing_for(plan.as_ref(), st.prefs.mixer);
                st.mix = match &plan {
                    Some(crate::softvol::Plan::Mix { mix, .. }) => Some(*mix),
                    _ => None,
                };
                st.hw_ignored = match (&plan, st.choice.output.as_ref()) {
                    (Some(crate::softvol::Plan::Mix { card, .. }), Some(o)) if o.hw_volume.is_some() => Some(*card),
                    _ => None,
                };
                sv.set_boost(st.choice.output.as_ref().map_or(0, |o| st.prefs.boost_for(&o.card_id)));
                sv.sync(plan.as_ref(), st.prefs.gain())
            }
            _ => crate::softvol::Synced::default(),
        };
        if let (true, Some(o)) = (synced.busy, st.choice.output.as_ref()) {
            // an idle web page holds the device: let it go, the next look finds the device free
            release_idle_web_via(&*self.route, o.card, o.device);
        }
        st.busy = match (synced.busy, st.choice.output.as_ref()) {
            (true, Some(o)) => Some(busy_line(&*self.route, o)),
            _ => None,
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

    /// An analog output that was not there at the last look became the one in use (its card finished starting, headphones
    /// were plugged in): say so, once. Not at the first look, and not when another output stays in use.
    fn note_new_output(&self, st: &mut State, first: bool, before: &[String]) {
        if first || st.server {
            return;
        }
        if let Some(o) = st.choice.output.as_ref().filter(|o| o.kind == Kind::Analog && !before.contains(&o.key())) {
            let line = format!("Sound output ready: {}", o.label());
            log::info!("{line}");
            self.notices.lock().unwrap_or_else(|e| e.into_inner()).push(line);
        }
    }

    /// Keep the 30 s window of a starting card.
    fn note_starting(&self, st: &mut State) {
        if self.sys.is_none() {
            return;
        }
        if card_starting_via(&*self.route).is_none() {
            st.starting_since = None;
            st.starting_over = false;
        } else if st.starting_over {
            st.starting_since = None;
        } else {
            match st.starting_since {
                None => st.starting_since = Some(Instant::now()),
                Some(t) if t.elapsed() >= Duration::from_secs(STARTING_SECS) => {
                    st.starting_since = None;
                    st.starting_over = true; // waited long enough: it is missing, not slow
                }
                Some(_) => {}
            }
        }
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
        let outputs = scan_via(&*self.route, &*self.mixer);
        let server = (self.server)();
        let (status, restored) = {
            let mut st = self.lock();
            let first = st.at.is_none();
            let before = std::mem::take(&mut st.seen_keys);
            // a card that went away may come back as something else: ask it again then
            if let Some(p) = &self.prober {
                for gone in st.outputs.iter().map(|o| o.card).filter(|c| !outputs.iter().any(|o| o.card == *c)) {
                    p.forget(gone);
                }
            }
            st.outputs = outputs;
            st.seen_keys = st.outputs.iter().map(Output::key).collect();
            st.server = server;
            st.at = Some(Instant::now());
            let r = self.recompute(&mut st);
            self.note_new_output(&mut st, first, &before);
            self.note_starting(&mut st);
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
    let real = proc_root == Path::new(iyakki_core_local::kernelfacts::PROC);
    // no ALSA outside Linux: the one output is the system's default (the mixer's AudioQueue plays on it)
    #[cfg(not(target_os = "linux"))]
    if real {
        return vec![Output::system_default()];
    }
    if real && global().is_active() {
        return global().status().outputs;
    }
    if real { scan(proc_root, &amixer_contents) } else { scan(proc_root, &|_| None) }
}

/// [`outputs_at`] over the kernel facts of an [`AudioRoute`]: the watched picture when the route is the running machine's and the
/// selector is installed, else a fresh look (the mixer only through a route that has one).
pub fn outputs_via(route: &dyn AudioRoute) -> Vec<Output> {
    let real = route.is_machine();
    #[cfg(not(target_os = "linux"))]
    if real {
        return vec![Output::system_default()];
    }
    if real && global().is_active() {
        return global().status().outputs;
    }
    scan_via(route, &|c| route.mixer_contents(c))
}

/// One line about the sound output for the SYSTEM tile: where sound plays and why, or why it cannot.
pub fn summary_at(proc_root: &Path) -> String {
    if proc_root == Path::new(iyakki_core_local::kernelfacts::PROC) && global().is_active() {
        return global().status().summary();
    }
    let outputs = outputs_at(proc_root);
    let prefs = global().prefs();
    let choice = choose(&outputs, &prefs.output);
    Status { outputs, choice, sound_server: false, prefs, device: None, softvol: None, mix: None, starting: None, hw_ignored: None, busy: None, sharing: Sharing::OneAtATime }.summary()
}

/// `amixer -c N contents` (alsa-utils), or None when it is missing or fails.
pub fn amixer_contents(card: u32) -> Option<String> {
    LocalAudioRoute::system().mixer_contents(card)
}

/// Is a sound server (PulseAudio or PipeWire) running for this user? Then its `default` device already follows the person's
/// choice of output, and automatic mode leaves it alone.
pub fn sound_server_running() -> bool {
    if std::env::var_os("PULSE_SERVER").is_some() {
        return true;
    }
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).is_some_and(|d| d.join("pulse/native").exists() || d.join("pipewire-0").exists())
}

/// The part of the selector's state the players on the tile side read (the core's seam, `iyakki_core_api::OutputState`): which device, how loud,
/// and a number that changes when the device changes. The decisions (choose, learned format, softvol) stay in the [`Selector`].
pub use iyakki_core_api::soundctl::OutputState;

/// The state of this computer's sound output as the core serves it (`Os::output_state`): the players, the radio and the sinks read it here
/// and never name the process-wide selector. The selector is the source ([`GlobalOutput`]), put there the first time anybody asks.
pub fn output_state() -> std::sync::Arc<dyn OutputState> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| iyakki_core_local::outputstate::set_source(std::sync::Arc::new(GlobalOutput)));
    iyakki_core_local::outputstate::current()
}

impl OutputState for Selector {
    fn device(&self) -> Option<String> {
        Selector::device(self)
    }
    fn generation(&self) -> u64 {
        Selector::generation(self)
    }
    fn gain(&self) -> u32 {
        Selector::gain(self)
    }
}

/// The state of the process-wide selector, as the seam.
pub struct GlobalOutput;

impl OutputState for GlobalOutput {
    fn device(&self) -> Option<String> {
        global().device()
    }
    fn generation(&self) -> u64 {
        global().generation()
    }
    fn gain(&self) -> u32 {
        global().gain()
    }
}

static GLOBAL: OnceLock<Selector> = OnceLock::new();
static WATCHING: AtomicBool = AtomicBool::new(false);

/// The selector IYAKKI's players ask. Inactive (no `-D`, full volume) until [`install`].
pub fn global() -> &'static Selector {
    GLOBAL.get_or_init(|| {
        let s = Selector::new(iyakki_core_local::kernelfacts::PROC, amixer_contents, sound_server_running).with_softvol(crate::softvol::Softvol::system()).with_prober(crate::soundprobe::Prober::system()).with_sys(iyakki_core_local::kernelfacts::SYS);
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
    // the saved switches apply from boot, not only after they are changed in Settings -> Sound
    crate::mixer::set_enabled(prefs.mixer);
    crate::webmix::set_wanted(prefs.web_mix);
    crate::audiosink::set_highlight_offset_ms(prefs.highlight_offset_ms);
    crate::mixer::set_source_levels(&prefs.source_levels);
    g.lock().prefs = prefs;
    g.active.store(true, Ordering::SeqCst);
    start_watch();
}


/// Counts every change of the master volume (see [`Master::generation`]).
pub fn master_generation() -> u64 {
    MASTER_GEN.load(Ordering::SeqCst)
}

/// Settings -> Sound changed (only reaches the selector once it is installed). The highlight offset is applied by the caller when it changed.
pub fn apply_prefs(prefs: SoundSettings) {
    MASTER_GEN.fetch_add(1, Ordering::SeqCst);
    crate::mixer::set_enabled(prefs.mixer);
    crate::webmix::set_wanted(prefs.web_mix);
    crate::mixer::set_source_levels(&prefs.source_levels);
    let g = global();
    if g.is_active() {
        g.set_prefs(prefs);
    }
}

/// Is there a sound card in `proc_root/asound/cards` yet?
pub fn has_card(proc_root: &Path) -> bool {
    has_card_via(&LocalAudioRoute::for_proc(proc_root))
}

/// [`has_card`] over an [`AudioRoute`].
pub fn has_card_via(route: &dyn AudioRoute) -> bool {
    !parse_cards(&route.cards_text()).is_empty()
}

/// Wait in the background for the first sound card (it may appear long after the shell started: the MacBookPro14,1's card
/// registers about 11 s after boot, when the Apple codec setup is done), then run `then` once. `wait` says how long to sleep
/// between looks. Returns at once if a card is there.
pub fn when_card_appears(proc_root: PathBuf, wait: impl Fn() -> Duration + Send + 'static, then: impl FnOnce() + Send + 'static) {
    if has_card(&proc_root) {
        return then();
    }
    let _ = std::thread::Builder::new().name("iyakki-sound-wait".into()).spawn(move || {
        while !has_card(&proc_root) {
            std::thread::sleep(wait());
        }
        then();
    });
}

/// Like [`when_card_appears`] for the real machine and the global selector: [`install`] it when the card is there.
pub fn install_when_card_appears(prefs: impl Fn() -> SoundSettings + Send + 'static) {
    when_card_appears(PathBuf::from(iyakki_core_local::kernelfacts::PROC), || watch_wait(uptime()), move || install(prefs()));
}

/// How often the watcher reads the card, PCM and ELD files (cheap: a few small files). A full look follows at once when they
/// changed, and every [`WATCH_EVERY`] regardless.
pub const WATCH_POLL: Duration = Duration::from_millis(500);
/// After a change the watcher looks again once these long after it: the ELD turns valid a moment after a monitor is plugged in,
/// and the jack controls can lag. A look that finds nothing new changes nothing (no rewrite of `/etc/asound.conf`).
pub const SETTLE_LOOKS: [Duration; 2] = [Duration::from_millis(1200), Duration::from_millis(3000)];

/// One round of the watcher: look at the cards, PCMs and ELD pins, and take a full look when they changed, shortly after a
/// change (settle looks), or when it is due.
pub struct Watch {
    last_sig: Option<String>,
    last_look: Option<Instant>,
    /// When the signature last changed, and how many settle looks since.
    changed_at: Option<Instant>,
    settled: usize,
}

impl Watch {
    pub fn new() -> Watch {
        Watch { last_sig: None, last_look: None, changed_at: None, settled: SETTLE_LOOKS.len() }
    }

    /// Returns the status when a look was taken.
    pub fn step(&mut self, sel: &Selector, proc_root: &Path) -> Option<Status> {
        self.step_at(sel, proc_root, Instant::now())
    }

    /// [`step`](Self::step) with the time given (tests).
    pub fn step_at(&mut self, sel: &Selector, proc_root: &Path, now: Instant) -> Option<Status> {
        let sig = sound_signature(proc_root);
        let changed = self.last_sig.as_deref() != Some(sig.as_str());
        if changed && self.last_sig.is_some() {
            self.changed_at = Some(now);
            self.settled = 0;
        }
        self.last_sig = Some(sig);
        let settle_due = match (self.changed_at, SETTLE_LOOKS.get(self.settled)) {
            (Some(t), Some(after)) => now.saturating_duration_since(t) >= *after,
            _ => false,
        };
        if settle_due {
            self.settled += 1;
        }
        let since = self.last_look.map_or(WATCH_EVERY, |t| now.saturating_duration_since(t));
        if self.last_look.is_none() || settle_due || look_due(changed, since) {
            let before = sel.generation();
            let st = sel.refresh();
            if sel.generation() != before {
                log::info!("sound output: {}", st.summary());
            }
            self.last_look = Some(now);
            return Some(st);
        }
        None
    }
}

impl Default for Watch {
    fn default() -> Self {
        Watch::new()
    }
}

/// Read the sound hardware files every [`WATCH_POLL`] and take a full look on a change or every [`WATCH_EVERY`], in the background (once per process).
pub fn start_watch() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let started = std::thread::Builder::new().name("iyakki-audioout".into()).spawn(|| {
        let mut w = Watch::new();
        loop {
            // the files are read often so a monitor plugged in or out is noticed within a second; a full look is taken then
            if w.step(global(), Path::new(iyakki_core_local::kernelfacts::PROC)).is_some() {
                // a microphone that records something from the first start: capture controls get their default once per card
                crate::micdefaults::run();
            }
            std::thread::sleep(WATCH_POLL.min(watch_wait(uptime())));
        }
    });
    if started.is_err() {
        WATCHING.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_idle_web_process_holding_the_sound_device_is_found_and_a_playing_one_never() {
        let d = std::env::temp_dir().join(format!("iyakki-webhold-{}", std::process::id()));
        std::fs::create_dir_all(d.join("asound/card0/pcm0p/sub0")).unwrap();
        let mk = |pid: &str, comm: &str, link: bool| {
            let f = d.join(pid).join("fd");
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(d.join(pid).join("comm"), format!("{comm}\n")).unwrap();
            if link {
                std::os::unix::fs::symlink("/dev/snd/pcmC0D0p", f.join("37")).unwrap();
            }
        };
        mk("4001", "WPEWebProcess", true);
        mk("4002", "WPEWebProcess", false); // another web process: does not hold it
        mk("4003", "aplay", true); // not the engine
        for (state, found) in [("state: PREPARED", true), ("state: SETUP", true), ("closed", true), ("state: RUNNING", false), ("state: PAUSED", false), ("state: DRAINING", false)] {
            std::fs::write(d.join("asound/card0/pcm0p/sub0/status"), format!("{state}\nhw_ptr : 0\n")).unwrap();
            assert_eq!(idle_web_holders(&d, 0, 0), if found { vec![4001] } else { vec![] }, "{state}");
        }
        assert!(idle_web_holders(&d, 0, 2).is_empty(), "another device");
        std::fs::remove_dir_all(&d).ok();
    }

    use super::*;
    use std::sync::Arc;

    /// The decisions run over a made-up route (`iyakki_core_fake::FakeAudioRoute`) with no directory on this computer.
    #[test]
    fn the_outputs_the_holders_the_signature_and_the_starting_card_come_from_the_route_not_the_machine() {
        use iyakki_core_api::PcmHolder;
        let r = iyakki_core_fake::FakeAudioRoute::default();
        r.put("cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
        r.put("pcm", "00-00: ALC : ALC : playback 1\n");
        r.put("card0/pcm0p/info", "card: 0\ndevice: 0\nstream: PLAYBACK\nid: ALC\nname: ALC Analog\n");
        r.put("card0/pcm3p/info", "card: 0\ndevice: 3\nstream: PLAYBACK\nid: HDMI 0\nname: HDMI 0\n");
        r.put("card0/eld#0.0", "monitor_present\t\t1\neld_valid\t\t1\nmonitor_name\t\tDELL\nsad_count\t\t1\n");
        r.put("card0/pcm0p/sub0/status", "state: PREPARED\n");
        r.set_holders(0, 0, vec![PcmHolder { pid: 77, name: "WPEWebProcess".into() }, PcmHolder { pid: 78, name: String::new() }]);
        let outs = scan_via(&r, &|_| None);
        assert_eq!(outs.len(), 2);
        assert!(outs.iter().any(|o| o.kind == Kind::Hdmi && o.monitor.as_deref() == Some("DELL")), "{outs:?}");
        assert!(has_card_via(&r));
        assert_eq!(pcm_holders_via(&r, 0, 0), vec!["WPEWebProcess (77)", "process 78"]);
        assert_eq!(idle_web_holders_via(&r, 0, 0), vec![77]);
        r.put("card0/pcm0p/sub0/status", "state: RUNNING\n");
        assert!(idle_web_holders_via(&r, 0, 0).is_empty(), "a page that plays is never touched");
        let sig = sound_signature_via(&r);
        assert!(sig.contains("card0:0.0=true:DELL"), "{sig}");
        assert_eq!(card_starting_via(&r), None, "not a CS8409 model");
        let mac = iyakki_core_fake::FakeAudioRoute::default();
        mac.put("cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
        mac.put("card0/pcm3p/info", "id: HDMI 0\nname: HDMI 0\n");
        mac.set_product(Some("MacBookPro14,1"));
        mac.put("card0/codec#0", "Codec: Cirrus Logic CS8409\n");
        assert_eq!(card_starting_via(&mac), Some(0), "HDMI devices but no analog one yet on a CS8409 Mac");
        assert!(hw_volume_is_dummy_via(&mac, 0) && !hw_volume_is_dummy_via(&r, 0));
        // the selector reads through the route it is given
        let sel = Selector::new("/nonexistent", |_| None, || false).with_route(Arc::new(r));
        assert_eq!(sel.refresh().outputs.len(), 2);
    }

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
        // the kernel's pin and the mixer's jack agree: the monitor is on the first connected pin
        put(&t.0, "asound/card0/eld#2.6", &eld_file(true, "PDM-15T"));
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

    // ---- monitor hotplug (KI-180): the pins' ELD files, the watcher and /etc/asound.conf

    /// A Mac mini whose three HDMI devices (3, 7, 8) have one pin each (`eld#2.0`..`eld#2.2`, pin n is device n-th), plus a USB card.
    fn hotplug_tree(tag: &str, usb: bool) -> Tree {
        let t = tree(tag);
        let r = &t.0;
        let mut cards = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n".to_string();
        if usb {
            cards += " 1 [Device         ]: USB-Audio - USB Audio Device\n";
            put(r, "asound/card1/usbid", "0d8c:0014\n");
            pcm(r, 1, 0, "USB Audio");
        }
        put(r, "asound/cards", &cards);
        for (d, id) in [(3, "HDMI 0"), (7, "HDMI 1"), (8, "HDMI 2")] {
            pcm(r, 0, d, id);
        }
        pins(&t, None);
        t
    }

    /// Only the pin `on` (0..3) reports a monitor.
    fn pins(t: &Tree, on: Option<usize>) {
        for p in 0..3 {
            put(&t.0, &format!("asound/card0/eld#2.{p}"), &eld_file(on == Some(p), "NEW-MON"));
        }
    }

    fn conf(t: &Tree) -> String {
        std::fs::read_to_string(t.0.join("asound.conf")).unwrap_or_default()
    }

    fn hotplug_selector(t: &Tree) -> Selector {
        Selector::new(t.0.clone(), |_| None, || false).with_softvol(crate::softvol::Softvol::new(t.0.join("asound.conf"), crate::softvol::fake::Fake::default()))
    }

    /// Run the watcher over a made-up clock: 500 ms per poll.
    fn poll(w: &mut Watch, sel: &Selector, t: &Tree, clock: &mut Instant, ms: u64) -> usize {
        let mut looks = 0;
        for _ in 0..(ms / 500).max(1) {
            *clock += WATCH_POLL;
            looks += w.step_at(sel, &t.0, *clock).is_some() as usize;
        }
        looks
    }

    #[test]
    fn the_signature_changes_when_a_pin_gets_or_loses_its_monitor() {
        let t = hotplug_tree("sig", false);
        let a = sound_signature(&t.0);
        assert_eq!(a, sound_signature(&t.0), "nothing changed, same signature");
        pins(&t, Some(0));
        let b = sound_signature(&t.0);
        assert_ne!(a, b);
        pins(&t, Some(1));
        assert_ne!(b, sound_signature(&t.0));
        pins(&t, None);
        assert_eq!(a, sound_signature(&t.0));
    }

    #[test]
    fn automatic_output_follows_a_monitor_unplugged_and_another_plugged_in_with_few_rewrites() {
        let t = hotplug_tree("auto-seq", false);
        pins(&t, Some(1)); // the monitor on the second pin: device 7
        let sel = hotplug_selector(&t);
        let mut w = Watch::new();
        let mut clock = Instant::now();
        assert!(w.step_at(&sel, &t.0, clock).is_some(), "the first step looks");
        let c1 = conf(&t);
        assert!(c1.contains("hw:0,7"), "{c1}");
        let g1 = sel.generation();
        // quiet seconds: only the regular look every 3 s, nothing rewritten
        let m1 = std::fs::metadata(t.0.join("asound.conf")).unwrap().modified().unwrap();
        poll(&mut w, &sel, &t, &mut clock, 9000);
        assert_eq!(sel.generation(), g1, "an unchanged state is not a change");
        assert_eq!(std::fs::metadata(t.0.join("asound.conf")).unwrap().modified().unwrap(), m1, "an unchanged state does not rewrite asound.conf");
        // the monitor is unplugged: within two seconds the dead pin is not used any more
        pins(&t, None);
        let looks = poll(&mut w, &sel, &t, &mut clock, 1000);
        assert!(looks >= 1, "a change is looked at at once");
        let st = sel.status();
        assert!(st.device.is_none() && st.choice.output.is_none(), "no monitor, no other card: no output, not the dead pin; {:?}", st.choice);
        assert!(sel.generation() > g1, "the mixer's sink sees a new generation and closes (stale)");
        let g2 = sel.generation();
        // the same monitor comes back on another connector: the ELD turns valid, the next automatic choice is its pin
        pins(&t, Some(0));
        poll(&mut w, &sel, &t, &mut clock, 1000);
        let c3 = conf(&t);
        assert!(c3.contains("hw:0,3") && c3 != c1, "{c3}");
        assert!(sel.status().device.is_some());
        let g3 = sel.generation();
        assert!(g3 > g2);
        // settle looks and quiet seconds after the change rewrite nothing
        let m3 = std::fs::metadata(t.0.join("asound.conf")).unwrap().modified().unwrap();
        poll(&mut w, &sel, &t, &mut clock, 9000);
        assert_eq!(sel.generation(), g3, "no flapping");
        assert_eq!(std::fs::metadata(t.0.join("asound.conf")).unwrap().modified().unwrap(), m3);
    }

    #[test]
    fn a_settle_look_catches_an_eld_that_turns_valid_after_the_pin_changed() {
        let t = hotplug_tree("settle", false);
        let sel = hotplug_selector(&t);
        let mut w = Watch::new();
        let mut clock = Instant::now();
        w.step_at(&sel, &t.0, clock);
        // the pin says "present" with no name yet, then the name and formats arrive without any other change of the pin
        put(&t.0, "asound/card0/eld#2.0", "monitor_present\t\t1\neld_valid\t\t1\n");
        poll(&mut w, &sel, &t, &mut clock, 500);
        let looks_settle = poll(&mut w, &sel, &t, &mut clock, 3500);
        assert!(looks_settle >= 2, "two settle looks follow a change, got {looks_settle}");
    }

    #[test]
    fn an_explicit_choice_falls_back_when_its_pin_loses_the_monitor_and_returns_when_it_is_back() {
        let t = hotplug_tree("explicit", true);
        pins(&t, Some(1));
        let sel = hotplug_selector(&t);
        sel.set_prefs(SoundSettings { output: "PCH:7".into(), ..Default::default() });
        let mut w = Watch::new();
        let mut clock = Instant::now();
        w.step_at(&sel, &t.0, clock);
        let st = sel.status();
        assert!(st.choice.chosen && st.choice.output.as_ref().map(Output::key).as_deref() == Some("PCH:7"));
        assert!(conf(&t).contains("hw:0,7"));
        // the monitor is unplugged: the chosen pin is dead, the USB device plays, the file follows
        pins(&t, None);
        poll(&mut w, &sel, &t, &mut clock, 1000);
        let st = sel.status();
        assert!(!st.choice.chosen && st.choice.reason.contains("PCH:7") && st.choice.reason.contains("no monitor"), "{}", st.choice.reason);
        assert_eq!(st.choice.output.as_ref().map(Output::key).as_deref(), Some("Device:0"));
        assert!(conf(&t).contains("hw:1,0"), "{}", conf(&t));
        assert_eq!(sel.prefs().output, "PCH:7", "the choice itself is kept");
        // a monitor on a different pin does not take the chosen device: its pin is another one
        pins(&t, Some(0));
        poll(&mut w, &sel, &t, &mut clock, 1000);
        // the monitor is on the chosen device's pin again
        pins(&t, Some(1));
        poll(&mut w, &sel, &t, &mut clock, 1000);
        assert!(sel.status().choice.chosen);
        assert!(conf(&t).contains("hw:0,7"), "{}", conf(&t));
    }

    #[test]
    fn automatic_follows_the_next_choice_when_the_current_pin_is_dead_and_a_stale_jack_does_not_keep_it() {
        // the mixer still says the old jack is on after the kernel says no pin has a monitor
        let t = hotplug_tree("stale-jack", true);
        pins(&t, None);
        let sel = Selector::new(t.0.clone(), |c| (c == 0).then(|| "numid=1,iface=CARD,name='HDMI/DP,pcm=3 Jack'\n  : values=on\n".to_string()), || false);
        let st = sel.refresh();
        assert_eq!(st.device.as_deref(), Some("plughw:1,0"), "{:?}", st.choice);
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
        // a chosen HDMI output that has no monitor is not used while another output can play; it is the person's choice again
        // once a monitor is there (see the hotplug tests)
        sel.set_prefs(SoundSettings { output: "PCH:8".into(), ..Default::default() });
        let st = sel.status();
        assert_eq!(st.device.as_deref(), Some("plughw:0,3"));
        assert!(!st.choice.chosen && st.choice.reason.contains("PCH:8") && st.choice.reason.contains("no monitor"), "{}", st.choice.reason);
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

    #[test]
    fn the_t2_driver_name_says_how_many_channels_the_speaker_takes() {
        let cards = " 0 [Audio          ]: AppleT2x4 - Apple T2 Audio\n                      Apple T2 Audio\n 1 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1 irq 3\n";
        assert_eq!(t2_channels(cards, 0), Some(4));
        assert_eq!(t2_channels(cards, 1), None, "not a T2 card");
        assert_eq!(t2_channels(cards, 5), None, "no such card");
        assert_eq!(t2_channels(" 1 [Audio          ]: AppleT2x1 - Apple T2 Audio\n", 1), Some(1));
        assert_eq!(t2_channels(" 0 [Audio          ]: AppleT2x0 - Apple T2 Audio\n", 0), None);
        assert_eq!(t2_channels(" 0 [Audio          ]: AppleT2 - Apple T2 Audio\n", 0), None);
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
        assert_eq!(conf, crate::softvol::config_text_boost(&crate::softvol::Plan::Soft { card: 1, device: 0 }, DEFAULT_BOOST_DB), "the T2: no dmix");
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
        assert!(std::fs::read_to_string(t.0.join("asound.conf")).unwrap().contains("plughw:1,2"));
        assert_eq!(st.prefs.volume_pct, 70);
        // both unplugged: back on the speaker at 30 %
        *headphones.lock().unwrap() = false;
        plug_monitor(&t, false, &monitor);
        assert_eq!(sel.refresh().prefs.volume_pct, 30);
        assert_eq!(*saved.lock().unwrap(), [70, 30]);
        let c = fake.calls.lock().unwrap().clone();
        // the fake's control stays once created, so later levels are set without opening the device again (softvol.rs rule: a control that exists is set, never primed)
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%", "amixer -q -c 0 sset IYAKKI 70%", "amixer -q -c 1 sset IYAKKI 70%", "amixer -q -c 1 sset IYAKKI 30%"]);
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

    #[test]
    fn sharing_follows_the_plan_and_the_mixer_setting() {
        use crate::softvol::{Mix, Plan};
        let t2 = Plan::Mix { card: 1, device: 0, mix: Mix::T2_MONO };
        let hdmi = Plan::Mix { card: 0, device: 3, mix: Mix::STEREO };
        assert_eq!(sharing_for(Some(&t2), false), Sharing::OneAtATime, "the T2 default: plughw, one program at a time");
        assert_eq!(sharing_for(Some(&t2), true), Sharing::IyakkiMixer);
        assert_eq!(sharing_for(Some(&hdmi), true), Sharing::Dmix, "dmix stays where it is cheap");
        assert_eq!(sharing_for(Some(&Plan::Hw { card: 0, device: 0 }), true), Sharing::IyakkiMixer);
        assert_eq!(sharing_for(Some(&Plan::Soft { card: 0, device: 0 }), false), Sharing::OneAtATime);
        assert_eq!(sharing_for(None, true), Sharing::OneAtATime);
        assert!(SoundSettings::default().mixer, "on by default (owner decision 2026-10-28)");
        assert!(!SoundSettings::default().web_mix, "the browser's own route through the mixer stays off until it is tested with the web sound plugin");
        let old: SoundSettings = serde_json::from_str(r#"{"volume_pct":40}"#).unwrap();
        assert!(old.mixer, "settings saved before the mixer existed get the default");
        let off: SoundSettings = serde_json::from_str(r#"{"volume_pct":40,"mixer":false}"#).unwrap();
        assert!(!off.mixer, "a person who switched it off keeps it off");
    }

    #[test]
    fn the_selector_reports_the_mixer_for_a_t2_plan_only_when_the_setting_is_on() {
        let t = tree("sharing-t2");
        let fake = crate::softvol::fake::Fake::default();
        let sel = t2_with_softvol(&t, Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)), fake);
        sel.set_prefs(SoundSettings { mixer: false, ..Default::default() });
        assert_eq!(sel.refresh().sharing, Sharing::OneAtATime);
        sel.set_prefs(SoundSettings { mixer: true, ..Default::default() });
        let st = sel.refresh();
        assert_eq!(st.sharing, Sharing::IyakkiMixer);
        assert!(st.summary().contains("IYAKKI mixer"));
        let conf = std::fs::read_to_string(t.0.join("asound.conf")).unwrap();
        assert_eq!(conf, crate::softvol::config_text_boost(&crate::softvol::Plan::Soft { card: 1, device: 0 }, DEFAULT_BOOST_DB), "the mixer writes into the same plughw-based device");
    }
}

#[cfg(test)]
#[path = "audioout_macbook_tests.rs"]
mod macbook_tests;

#[cfg(test)]
mod system_default_tests {
    use super::*;

    #[test]
    fn the_system_default_output_is_one_honest_row_that_is_chosen_automatically() {
        let o = Output::system_default();
        assert_eq!(o.label(), "System default output");
        let c = choose(&[o.clone()], "");
        assert_eq!(c.output.as_ref().map(|x| x.label()), Some("System default output".to_string()));
        assert!(!o.uses_hw_volume(VolumeMode::Automatic), "the volume is IYAKKI's own");
    }
}
