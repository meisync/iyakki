//! A volume control for outputs that have none: IYAKKI writes `/etc/asound.conf` with an ALSA `softvol` device over the
//! chosen output and makes it ALSA's `default`.
//!
//! Why: many outputs have no hardware volume at all. On the Mac mini 2018 the HDMI devices of card 0 (`HDA Intel PCH`) have
//! only `IEC958` switches, and card 1 (`Apple T2 Audio`, apple-bce) has only its `Codec Output Jack`; Settings could not
//! change how loud the built-in speaker is, and programs other than IYAKKI's own players (ffplay) always played at full
//! level. ALSA's `softvol` plugin scales the samples of everything that plays through it and adds a mixer control
//! (`IYAKKI`) to the card, which `amixer` and Settings -> Sound then move like a hardware one.
//!
//! What [`Softvol::sync`] does, every time the output IYAKKI plays on is decided (start, a monitor or headphones plugged
//! in, a USB device, another output chosen; [`crate::audioout`] calls it):
//! * the output has a hardware volume (an analog or USB output whose card has a `Master`, `PCM`, `Speaker` or `Headphone`
//!   `Playback Volume`, the controls `iyakki-devctl` moves; HDMI and digital outputs never do): the file makes `default` (and `iyakki`) a plug over the output, and the hardware control stays;
//! * it has none: `iyakkivol` is a softvol over a `dmix` of the device (`hw:C,D` at 48 kHz, 16-bit stereo, or what the T2
//!   chip takes, see [`Mix`]) with the control `IYAKKI` on card C, and `default` and `iyakki` are plugs over it. The dmix
//!   is what lets IYAKKI's players and any other program (the web engine's `alsasink`) play at once: a softvol over the
//!   bare device is exclusive, and whoever opened it first made the other one silent (`Resource busy`). Where dmix cannot
//!   be opened the file falls back to the exclusive `plughw:C,D` (one program at a time) and says why in the log. The
//!   control exists only once the device has been opened, so it is opened once ("primed": 10 ms of silence through
//!   `aplay`), which is also the open test for the dmix; then the volume of Settings -> Sound is set with `amixer -c C
//!   sset IYAKKI N%`. Mute is 0 % (softvol has no switch); IYAKKI's own players also play silence while muted.
//! * a file that IYAKKI did not write (no [`MARKER`] on its first line) is never changed, and nothing is written while a
//!   sound server runs (it owns `default`); IYAKKI then keeps routing `-D plughw:C,D` and scaling its own sound.
//!
//! All machine access goes through a [`Runner`] and the file path is given, so tests use a fake `amixer`, a fake player
//! and a file in a temporary directory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The first line of every file IYAKKI writes; a file without it is someone else's and is left alone.
pub const MARKER: &str = "# Written by IYAKKI (sound output, iyakki-agent softvol.rs). Rewritten when the output changes; do not edit.";
/// Where ALSA reads the system-wide configuration.
pub const ASOUND_CONF: &str = "/etc/asound.conf";
/// The device IYAKKI's players are given (`aplay -D iyakki`): the chosen output, through the softvol when there is one.
pub const PCM: &str = "iyakki";
/// The softvol device itself.
pub const SOFTVOL_PCM: &str = "iyakkivol";
/// The mixer control softvol adds to the card (`amixer -c C sset IYAKKI 30%`; `iyakki-devctl` knows it). Its own name, not
/// `Master`: on a PC the HDMI device shares its card with an analog codec whose `Master Playback Volume` must stay apart,
/// and without a `Playback Volume` suffix it is never mistaken for a hardware control (see [`HW_CONTROLS`]).
pub const CONTROL: &str = "IYAKKI";
/// The hardware controls that count as a volume (the same names `iyakki-devctl` moves), as `<name> Playback Volume`.
pub const HW_CONTROLS: [&str; 4] = ["Master", "PCM", "Speaker", "Headphone"];
/// The softvol range: 0 % is muted, 100 % plays unchanged.
pub const MIN_DB: f32 = -51.0;
pub const MAX_DB: f32 = 0.0;

/// The hardware volume control among a card's control names (`amixer -c N contents`), if it has one.
pub fn hw_control(names: &[String]) -> Option<String> {
    HW_CONTROLS.iter().find(|c| names.iter().any(|n| *n == format!("{c} Playback Volume"))).map(|c| c.to_string())
}

/// How `dmix` opens the hardware device: it needs a fixed rate, channel count and format (the `plug` layers above convert).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mix {
    /// `S16_LE` (HDMI, USB) or `S24_LE` (the T2 chip's `aaudio`, which takes nothing else).
    pub format: &'static str,
    /// 2, or 1 for the T2 Mac's built-in speaker (a `route` plugin mixes stereo down to it).
    pub channels: u32,
    /// Ask for periods of 1024 and a buffer of 8192 frames (about 170 ms at 48 kHz). Devices with fixed periods (the T2
    /// chip's, about 20 ms) are left to choose.
    pub sized: bool,
    /// The rate the dmix runs at ([`MIX_RATE`], or the only rate a device has: 44100 on the MacBookPro14,1's codec).
    pub rate: u32,
    /// Learned from the device ([`crate::soundprobe`]) and different from the default: the file says so and sets the
    /// softvol's `resolution`. Plain `false` for every long-standing variant, whose text is unchanged.
    pub probed: bool,
}

impl Mix {
    /// Stereo 16-bit, what HDMI, USB and ordinary analog devices take.
    pub const STEREO: Mix = Mix { format: "S16_LE", channels: 2, sized: true, rate: MIX_RATE, probed: false };
    /// The T2 Mac's headphone jack: stereo, 24-bit in 32.
    pub const T2_STEREO: Mix = Mix { format: "S24_LE", channels: 2, sized: false, rate: MIX_RATE, probed: false };
    /// The T2 Mac's built-in speaker: one channel, 24-bit in 32.
    pub const T2_MONO: Mix = Mix { format: "S24_LE", channels: 1, sized: false, rate: MIX_RATE, probed: false };

    /// More than two channels: a `route` plugin puts the stereo signal on every channel (left on the even ones, right on
    /// the odd ones), so each speaker of a four-channel codec plays at full level.
    pub fn fans_out(&self) -> bool {
        self.channels > 2
    }
}

/// The largest software boost, in dB (Settings -> Sound -> Advanced -> Boost). More than this only distorts.
pub const MAX_BOOST_DB: u32 = 12;
/// The softvol's volume steps (alsa-lib's default, written out for devices IYAKKI learned about).
pub const RESOLUTION: u32 = 256;

/// The rate every dmix runs at; players of other rates are converted by `plug` above it.
pub const MIX_RATE: u32 = 48_000;

/// The `dmix` server key of a device: one per card and device, so outputs never share a server (which would refuse a
/// different slave).
pub fn ipc_key(card: u32, device: u32) -> u32 {
    24_680 + card * 64 + device.min(63)
}

/// What the file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The output has no volume control: a softvol over a `dmix` of the device, so IYAKKI's players and other programs
    /// (the web engine) share it.
    Mix { card: u32, device: u32, mix: Mix },
    /// The same softvol over the device alone (`plughw`): one program at a time. Used where `dmix` cannot open the device.
    Soft { card: u32, device: u32 },
    /// [`Plan::Soft`] for a device with more than two channels (the MacBookPro's four-channel codec): the stereo signal is copied to every
    /// channel before it reaches the device, as the shared mixer does, so each speaker plays at full level and not only the front pair.
    /// `format`: the device's format when it was learned from the device (`S32_LE` on the MacBookPro14,1's codec): named for the
    /// slave, because that codec also offers `S24_LE` and ALSA's `plug` picks it for 16-bit players; its 24 bits then sit in the low
    /// bits of the 32-bit word while the hardware reads the top ones, 48 dB too quiet (measured 2026-11-08: "ear at the speaker").
    SoftFan { card: u32, device: u32, channels: u32, format: Option<&'static str> },
    /// [`Plan::Soft`] for a device with fixed periods (the T2 Mac's `aaudio`), plus the engine's own PCM ([`WEB_PCM`]): `plug > softvol >
    /// plughw` plays the players' S16 stereo, but the engine's WebAudio output could not be installed on it (the page then froze on its
    /// synchronous start of the output), so the engine gets `plug > softvol > hw` (measured on a MacBookPro16,2: no freeze, a media
    /// element's tone plays). `channels` is what the device was probed to take.
    SoftWeb { card: u32, device: u32, channels: u32 },
    /// The same softvol over `hw` itself (no `plughw` under it): for a device with fixed periods, where `plug > softvol > plughw`
    /// cannot install its parameters at a rate other than the device's (the T2 Mac's `aaudio`: one-frame periods, 4 channels of
    /// 24-bit in 32). The `plug` above the volume does every conversion. One program at a time.
    Direct { card: u32, device: u32, channels: u32 },
    /// The card has a hardware volume: `default` is the output, the hardware control stays.
    Hw { card: u32, device: u32 },
}

impl Plan {
    pub fn card(&self) -> u32 {
        match self {
            Plan::Mix { card, .. } | Plan::Soft { card, .. } | Plan::SoftFan { card, .. } | Plan::SoftWeb { card, .. } | Plan::Direct { card, .. } | Plan::Hw { card, .. } => *card,
        }
    }

    fn device(&self) -> u32 {
        match self {
            Plan::Mix { device, .. } | Plan::Soft { device, .. } | Plan::SoftFan { device, .. } | Plan::SoftWeb { device, .. } | Plan::Direct { device, .. } | Plan::Hw { device, .. } => *device,
        }
    }

    /// What a `Mix` becomes where dmix did not open: a device with fixed periods (`!sized`) gets the volume over `hw` itself, any
    /// other the exclusive volume over `plughw`.
    fn exclusive_of(&self) -> Plan {
        match self {
            Plan::Mix { card, device, mix } if !mix.sized => Plan::Direct { card: *card, device: *device, channels: mix.channels },
            Plan::Mix { card, device, mix } if mix.fans_out() => Plan::SoftFan { card: *card, device: *device, channels: mix.channels, format: mix.probed.then_some(mix.format) },
            p => Plan::Soft { card: p.card(), device: p.device() },
        }
    }

    /// The software volume's card and whether it is shared (dmix).
    fn is_mix(&self) -> bool {
        matches!(self, Plan::Mix { .. })
    }
}

fn softvol_block(card: u32, slave: &str, boost_db: u32, resolution: bool) -> String {
    // 100 % is 0 dB, or +boost dB where the person asked for a boost (it clips, so it may distort)
    let max_db = MAX_DB + boost_db.min(MAX_BOOST_DB) as f32;
    let res = if resolution { format!("    resolution {RESOLUTION}\n") } else { String::new() };
    format!(
        "pcm.{SOFTVOL_PCM} {{\n    type softvol\n    slave.pcm \"{slave}\"\n    control {{\n        name \"{CONTROL}\"\n        card {card}\n    }}\n    min_dB {MIN_DB:.1}\n    max_dB {max_db:.1}\n{res}}}\n\
         pcm.{PCM} {{\n    type plug\n    slave.pcm \"{SOFTVOL_PCM}\"\n}}\n\
         pcm.!default {{\n    type plug\n    slave.pcm \"{SOFTVOL_PCM}\"\n}}\n\
         ctl.!default {{\n    type hw\n    card {card}\n}}\n"
    )
}

/// The text of `/etc/asound.conf` for this plan.
pub fn config_text(plan: &Plan) -> String {
    config_text_boost(plan, 0)
}

/// The route plugin that gives every channel of a multi-channel device the stereo signal at full level (weight 1.0):
/// left on the even channels, right on the odd ones.
fn fan_route(channels: u32) -> String {
    let mut t = format!("pcm.iyakkifan {{\n    type route\n    slave {{\n        pcm \"iyakkimix\"\n        channels {channels}\n    }}\n    ttable {{\n");
    for dst in 0..channels {
        t.push_str(&format!("        {}.{dst} 1.0\n", dst % 2));
    }
    t.push_str("    }\n}\n");
    // the dmix as it is (all channels apart, quiet test tones go through it); the software volume does not apply
    t.push_str("pcm.iyakkichan {\n    type plug\n    slave.pcm \"iyakkimix\"\n}\n");
    t
}

/// The PCM the web engine plays through on a device whose rate conversion in `plug` is broken (the T2 Mac's `aaudio`: with its
/// one-frame period, 24, 16, 12 and 32 kHz audio plays at the wrong speed and 32-bit float at 44.1 kHz cannot be set up). It is
/// built from the plugins that never convert the rate (`linear` for the format, `route` for the channels) over the same
/// softvol, so the only rate it offers is the device's 48000 Hz and the only formats are integer ones: GStreamer's own
/// `audioresample` and `audioconvert` do the rest. Client-side name: [`WEB_PCM`], selected for the engine by
/// [`WEB_ALSA_CONF`].
fn web_pcm(channels: u32) -> String {
    web_pcm_over(channels, SOFTVOL_PCM)
}

/// [`web_pcm`] over the softvol named `slave`.
fn web_pcm_over(channels: u32, slave: &str) -> String {
    if channels < 2 {
        // unknown (or the unprobed one-channel guess for the T2 speaker): no web PCM, the engine keeps `default`
        return String::new();
    }
    let mut t = format!("pcm.{WEB_PCM}route {{\n    type route\n    slave {{\n        pcm \"{slave}\"\n        channels {channels}\n    }}\n    ttable {{\n");
    for dst in 0..channels {
        t.push_str(&format!("        {}.{dst} 1.0\n", dst % 2));
    }
    t.push_str(&format!("    }}\n}}\npcm.{WEB_PCM} {{\n    type linear\n    slave {{\n        pcm \"{WEB_PCM}route\"\n        format S24_LE\n    }}\n}}\n"));
    t
}

/// The web engine's PCM (see [`web_pcm`]).
pub const WEB_PCM: &str = "iyakkiweb";

/// The ALSA configuration only the web engine's processes read (`ALSA_CONFIG_PATH`): the system's own, then `default` is
/// [`WEB_PCM`]. Written next to the web profile by [`crate::webengine`] when `/etc/asound.conf` defines that PCM.
pub const WEB_ALSA_CONF: &str = "</usr/share/alsa/alsa.conf>\npcm.!default iyakkiweb\n";

/// [`config_text`] with the software boost of the card: the softvol's range ends `boost_db` dB above unity (0 to
/// [`MAX_BOOST_DB`]; the sound may distort). Nothing changes for 0.
pub fn config_text_boost(plan: &Plan, boost_db: u32) -> String {
    match plan {
        Plan::Mix { card, device, mix } => {
            let key = ipc_key(*card, *device);
            let sized = if mix.sized { "\n        period_size 1024\n        buffer_size 8192" } else { "" };
            let (route, slave) = if mix.channels == 1 {
                (format!("pcm.iyakkimono {{\n    type route\n    slave {{\n        pcm \"iyakkimix\"\n        channels 1\n    }}\n    ttable {{\n        0.0 0.5\n        1.0 0.5\n    }}\n}}\n"), "iyakkimono")
            } else if mix.fans_out() {
                (fan_route(mix.channels), "iyakkifan")
            } else {
                (String::new(), "iyakkimix")
            };
            let learned = if mix.probed {
                format!("# The device was asked what it accepts: {} channels, {}, {} Hz.\n", mix.channels, mix.format, mix.rate)
            } else {
                String::new()
            };
            format!(
                "{MARKER}\n\
                 # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}) over a dmix, so\n\
                 # IYAKKI's players and other programs (the web engine) play at once. dmix server key {key}.\n\
                 {learned}\
                 pcm.iyakkimix {{\n    type dmix\n    ipc_key {key}\n    ipc_perm 0666\n    slave {{\n        pcm \"hw:{card},{device}\"\n        rate {rate}\n        channels {ch}\n        format {fmt}{sized}\n    }}\n}}\n\
                 {route}{rest}",
                rate = mix.rate,
                ch = mix.channels,
                fmt = mix.format,
                rest = softvol_block(*card, slave, boost_db, mix.probed)
            )
        }
        Plan::Soft { card, device } => format!(
            "{MARKER}\n\
             # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}); one program at a time.\n\
             {}",
            softvol_block(*card, &format!("plughw:{card},{device}"), boost_db, false)
        ),
        Plan::SoftFan { card, device, channels, format } => {
            // the device's own format, named: plug then converts to it instead of choosing one (see the variant's doc)
            let fmt_line = format.map(|f| format!("        format {f}\n")).unwrap_or_default();
            let mut route = format!("pcm.iyakkifan {{\n    type route\n    slave {{\n        pcm \"plughw:{card},{device}\"\n        channels {channels}\n{fmt_line}    }}\n    ttable {{\n");
            for dst in 0..*channels {
                route.push_str(&format!("        {}.{dst} 1.0\n", dst % 2));
            }
            route.push_str("    }\n}\n");
            // the speaker tests play every channel apart through this one (the same format, or the test is quiet too)
            match format {
                Some(f) => route.push_str(&format!("pcm.iyakkichan {{\n    type plug\n    slave {{\n        pcm \"plughw:{card},{device}\"\n        format {f}\n    }}\n}}\n")),
                None => route.push_str(&format!("pcm.iyakkichan {{\n    type plug\n    slave.pcm \"plughw:{card},{device}\"\n}}\n")),
            }
            format!(
                "{MARKER}\n\
                 # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}); one program at a time.\n\
                 # The device has {channels} channels: the stereo signal is copied to all of them (left on the even ones, right on the odd ones).\n\
                 {route}{}",
                softvol_block(*card, "iyakkifan", boost_db, false)
            )
        }
        // the channels are unknown (the unprobed one-channel guess): no engine PCM, exactly the plain plan
        Plan::SoftWeb { card, device, channels } if *channels < 2 => config_text_boost(&Plan::Soft { card: *card, device: *device }, boost_db),
        Plan::SoftWeb { card, device, .. } => {
            let max_db = MAX_DB + boost_db.min(MAX_BOOST_DB) as f32;
            // the same control (name, card, range) under a second softvol over hw itself: one slider, two ways in
            let hw_vol = {
                format!("pcm.{WEB_PCM}vol {{\n    type softvol\n    slave.pcm \"hw:{card},{device}\"\n    control {{\n        name \"{CONTROL}\"\n        card {card}\n    }}\n    min_dB {MIN_DB:.1}\n    max_dB {max_db:.1}\n}}\n")
            };
            format!(
                "{MARKER}\n\
                 # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}); one program at a time.\n\
                 # {WEB_PCM} is the web engine's way in, over hw itself (the engine's WebAudio output cannot be installed on plughw here).\n\
                 {}{hw_vol}pcm.{WEB_PCM} {{\n    type plug\n    slave.pcm \"{WEB_PCM}vol\"\n}}\n",
                softvol_block(*card, &format!("plughw:{card},{device}"), boost_db, false)
            )
        }
        Plan::Direct { card, device, channels } => format!(
            "{MARKER}\n\
             # The output has no hardware volume and fixed periods: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}) straight\n\
             # over the device, the plug above it converts; one program at a time.\n\
             {}{}",
            softvol_block(*card, &format!("hw:{card},{device}"), boost_db, false),
            web_pcm(*channels)
        ),
        Plan::Hw { card, device } => format!(
            "{MARKER}\n\
             # The output's card has a hardware volume control, which stays in use.\n\
             pcm.{PCM} {{\n    type plug\n    slave.pcm \"hw:{card},{device}\"\n}}\n\
             pcm.!default {{\n    type plug\n    slave.pcm \"hw:{card},{device}\"\n}}\n\
             ctl.!default {{\n    type hw\n    card {card}\n}}\n"
        ),
    }
}

/// How the machine is reached: `amixer` and a short play to open a device. Fakes in tests.
pub trait Runner: Send + Sync {
    /// Run `amixer` with these arguments; its output, or why it failed.
    fn amixer(&self, args: &[String]) -> Result<String, String>;
    /// Open `pcm` once and play a few milliseconds of silence, so softvol creates its control.
    fn prime(&self, pcm: &str) -> Result<(), String>;
    /// Does the volume control of `card` exist now (`amixer -c C sget IYAKKI`)? Asking never opens the device.
    /// (Default: unknown, so the device is opened as before.)
    fn has_control(&self, _card: u32) -> bool {
        false
    }
}

/// Who may veto an open of the speaker. The rules (a prime is an open of the one-owner speaker, so it must never race, or take the
/// speaker from, the program that plays or is about to):
/// 1. never open while any playback substream of the card is not `closed` (any state: [`crate::quiet::must_not_open`]), nor while
///    the web engine has a page that plays audio ([`note_web_media`]);
/// 2. a refused open is deferred with a backoff of [`BACKOFF`] (0.5 s, then 1, 2, 4 s, never longer), never a tight loop;
/// 3. an open is made only when the volume control is missing AND the level has to be set (the person moved it, or the saved one
///    is restored); a control that exists is set without opening anything;
/// 4. an open holds the device for at most [`PRIME_HOLD`] (10 ms of silence).
pub trait Gate: Send + Sync {
    /// Why the card must not be opened now, if it must not.
    fn refuse(&self, card: u32) -> Option<String>;
}

/// The delays of a deferred open, in order; the last one repeats.
pub const BACKOFF: [Duration; 4] = [Duration::from_millis(500), Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)];
/// The longest the agent's own open holds the speaker after it wrote its silence.
pub const PRIME_HOLD: Duration = Duration::from_millis(150);

static WEB_MEDIA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The browser tells whether the web engine has a page that plays audio (the helper's `AudioPlaying`).
pub fn note_web_media(on: bool) {
    WEB_MEDIA.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// The real gate: the kernel's stream states under `/proc/asound` and the web engine's flag.
pub struct ProcGate {
    pub root: PathBuf,
}

impl Gate for ProcGate {
    fn refuse(&self, card: u32) -> Option<String> {
        if WEB_MEDIA.load(std::sync::atomic::Ordering::SeqCst) {
            return Some("the web engine is playing audio".into());
        }
        crate::quiet::must_not_open(&crate::quiet::playback_streams(&self.root), card)
    }
}

/// The real `amixer` and `aplay`.
pub struct System;

/// 10 ms of 16-bit stereo silence at 48 kHz.
pub const PRIME_BYTES: usize = 48_000 / 100 * 2 * 2;

impl Runner for System {
    fn amixer(&self, args: &[String]) -> Result<String, String> {
        let out = std::process::Command::new("amixer").args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("amixer: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!("amixer {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
        }
    }

    fn has_control(&self, card: u32) -> bool {
        self.amixer(&["-c".to_string(), card.to_string(), "sget".to_string(), CONTROL.to_string()]).is_ok()
    }

    fn prime(&self, pcm: &str) -> Result<(), String> {
        use std::io::Write;
        let mut child = std::process::Command::new("aplay")
            .args(["-q", "-D", pcm, "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("aplay: {e}"))?;
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(&[0u8; PRIME_BYTES]);
        }
        // the control exists as soon as the device is open, so nothing waits for the sound to drain: a short hold, then the
        // player is ended (the speaker is a one-owner device, see [`Gate`])
        let end = Instant::now() + PRIME_HOLD;
        loop {
            match iyakki_core_api::child_status(&mut child) {
                st if st.success() => return Ok(()),
                // the status was taken by another reaper: it ended, and what it did is unknown, so it is not taken for a success
                st if st.is_over() => return Err(format!("aplay -D {pcm} ended ({}; the device may be busy)", st.describe())),
                _ if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // killed while draining: fine when the device had opened (the control is there)
                    let card = std::fs::read_to_string(ASOUND_CONF).ok().and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("slave.pcm \"plughw:").and_then(|r| r.split(',').next()?.parse::<u32>().ok())));
                    return match card {
                        Some(c) if self.has_control(c) => Ok(()),
                        _ => Err(format!("aplay -D {pcm} did not finish")),
                    };
                }
            }
        }
    }
}

/// What [`Softvol::sync`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    /// The file is IYAKKI's and describes the output: players use [`PCM`].
    pub managed: bool,
    /// The softvol's card, once its control holds the wanted level (IYAKKI's players then leave their samples alone).
    pub soft_card: Option<u32>,
    /// Why the file is not used, when it is not.
    pub note: Option<String>,
    /// The file is written but the device could not be opened to create the volume control (another program holds the
    /// card: `Resource busy`); tried again on the next call.
    pub busy: bool,
}

#[derive(Default)]
struct St {
    /// The plan the file holds (None: no file of ours).
    written: Option<Plan>,
    /// The softvol device has been opened since the file was written (its control exists).
    primed: bool,
    /// The level last set on the control.
    level: Option<u32>,
    /// Devices where `dmix` could not be opened: they use the exclusive softvol from then on.
    exclusive: Vec<(u32, u32)>,
    /// An open was refused: no new try before this time (on the softvol's own clock), and how many refusals in a row.
    defer_until: Option<Duration>,
    deferred: usize,
}

impl St {
    /// Forget the file and the control, but not which devices cannot do dmix.
    fn reset(&mut self) {
        *self = St { exclusive: std::mem::take(&mut self.exclusive), defer_until: self.defer_until, deferred: self.deferred, ..Default::default() };
    }
}

/// Keeps the ALSA configuration in step with the chosen output.
pub struct Softvol {
    path: PathBuf,
    runner: Box<dyn Runner>,
    st: Mutex<St>,
    /// The software boost in dB (see [`config_text_boost`]); applies to the next [`sync`](Self::sync).
    boost: std::sync::atomic::AtomicU32,
    /// May the speaker be opened now (none: always, as in tests that have no machine).
    gate: Option<Box<dyn Gate>>,
    /// Time since the start, replaceable so tests need no sleeping.
    clock: Box<dyn Fn() -> Duration + Send + Sync>,
}

impl Softvol {
    pub fn new(path: impl Into<PathBuf>, runner: impl Runner + 'static) -> Softvol {
        let start = Instant::now();
        Softvol { path: path.into(), runner: Box::new(runner), st: Mutex::new(St::default()), boost: Default::default(), gate: None, clock: Box::new(move || start.elapsed()) }
    }

    /// Ask `gate` before every open of the speaker (see [`Gate`]).
    pub fn with_gate(mut self, gate: impl Gate + 'static) -> Softvol {
        self.gate = Some(Box::new(gate));
        self
    }

    /// Use this clock for the backoff.
    pub fn with_clock(mut self, clock: impl Fn() -> Duration + Send + Sync + 'static) -> Softvol {
        self.clock = Box::new(clock);
        self
    }

    /// May the device be opened now? No while the backoff of an earlier refusal runs or the gate says no (the next try is then
    /// set later, 0.5 s to 4 s); the first refusal of a row is logged.
    fn may_open(&self, st: &mut St, card: u32) -> bool {
        let now = (self.clock)();
        if st.defer_until.is_some_and(|t| now < t) {
            return false;
        }
        match self.gate.as_ref().and_then(|g| g.refuse(card)) {
            None => {
                st.defer_until = None;
                st.deferred = 0;
                true
            }
            Some(why) => {
                if st.deferred == 0 {
                    log::info!("sound: not opening {PCM} to create its volume control ({why}); trying again shortly");
                }
                st.defer_until = Some(now + BACKOFF[st.deferred.min(BACKOFF.len() - 1)]);
                st.deferred += 1;
                false
            }
        }
    }

    /// The boost of the card in use, 0 to [`MAX_BOOST_DB`] dB.
    pub fn set_boost(&self, db: u32) {
        self.boost.store(db.min(MAX_BOOST_DB), std::sync::atomic::Ordering::SeqCst);
    }

    /// The real one: `/etc/asound.conf`, `amixer`, `aplay`.
    pub fn system() -> Softvol {
        Softvol::new(ASOUND_CONF, System).with_gate(ProcGate { root: PathBuf::from(crate::quiet::ASOUND) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bring the file and the control in line with `plan` (None: no output, or a sound server decides: IYAKKI's file is
    /// removed) and the volume `level` (0 to 100, 0 when muted). Cheap when nothing changed; a level that could not be set
    /// yet (no `amixer`, the device busy) is tried again on the next call.
    pub fn sync(&self, plan: Option<&Plan>, level: u32) -> Synced {
        let mut st = self.lock();
        let existing = std::fs::read_to_string(&self.path).ok();
        if existing.as_deref().is_some_and(|t| !t.starts_with(MARKER)) {
            st.reset();
            return Synced { note: Some(format!("{} was not written by IYAKKI and is left alone", self.path.display())), ..Default::default() };
        }
        let Some(wanted) = plan else {
            if existing.is_some() {
                let _ = std::fs::remove_file(&self.path);
            }
            st.reset();
            return Synced::default();
        };
        // a device where dmix did not open before is not tried again
        let mut plan = match wanted {
            p @ Plan::Mix { card, device, .. } if st.exclusive.contains(&(*card, *device)) => p.exclusive_of(),
            // the T2's aaudio (fixed-period class, `!sized`): dmix spins a whole core on it whatever its settings (measured on a
            // MacBookPro16,2: 90 % of a core for a stereo stream against 2 % without it, with the real periods of the patched
            // driver, apple-bce patch 0003), so the volume sits straight over plughw; one program at a time
            Plan::Mix { card, device, mix } if !mix.sized => Plan::SoftWeb { card: *card, device: *device, channels: mix.channels },
            p => p.clone(),
        };
        let level = level.min(100);
        let mut demoted = false;
        loop {
            if let Err(note) = self.write_plan(&mut st, &plan) {
                return Synced { note: Some(note), ..Default::default() };
            }
            let card = match plan {
                Plan::Hw { .. } => return Synced { managed: true, ..Default::default() },
                Plan::Mix { card, .. } | Plan::Soft { card, .. } | Plan::SoftFan { card, .. } | Plan::SoftWeb { card, .. } | Plan::Direct { card, .. } => card,
            };
            let mut prime_failed = false;
            if st.level != Some(level) {
                if !st.primed && self.runner.has_control(card) {
                    st.primed = true; // the control is there: set it without opening anything
                }
                if !st.primed {
                    if !self.may_open(&mut st, card) {
                        // someone plays or is about to, or an open was refused a moment ago: the level is set on a later call
                        return Synced { managed: true, ..Default::default() };
                    }
                    match self.runner.prime(PCM) {
                        Ok(()) => st.primed = true,
                        Err(e) if plan.is_mix() => {
                            // dmix (or the format it asks of the device) was refused: the exclusive softvol, as before
                            log::info!("sound: dmix on card {card} device {} could not be opened ({e}); using the exclusive software volume", plan.device());
                            st.exclusive.push((card, plan.device()));
                            plan = plan.exclusive_of();
                            demoted = true;
                            continue;
                        }
                        Err(e) if demoted => {
                            // not dmix's fault (the device is busy, or gone): try dmix again on the next call
                            log::info!("sound: could not open {PCM} on card {card} either way: {e}");
                            let device = plan.device();
                            st.exclusive.retain(|d| *d != (card, device));
                            if matches!(plan, Plan::Soft { .. } | Plan::SoftFan { .. } | Plan::SoftWeb { .. } | Plan::Direct { .. }) {
                                let back = wanted.clone();
                                let _ = self.write_plan(&mut st, &back);
                            }
                            return Synced { managed: true, busy: true, ..Default::default() };
                        }
                        Err(e) => {
                            prime_failed = true;
                            log::info!("sound: could not open {PCM} once to create its volume control: {e}")
                        }
                    }
                }
                let args = vec!["-q".to_string(), "-c".to_string(), card.to_string(), "sset".to_string(), CONTROL.to_string(), format!("{level}%")];
                let mut set = self.runner.amixer(&args);
                if set.is_err() && st.primed && self.may_open(&mut st, card) {
                    // the control was there once and is gone (every stream closed, or the card was reset): open the device
                    // briefly to create it again, and try once more
                    log::info!("sound: the volume control {CONTROL} is missing; opening {PCM} to create it");
                    if self.runner.prime(PCM).is_ok() {
                        set = self.runner.amixer(&args);
                    }
                }
                match set {
                    Ok(_) => {
                        st.primed = true; // the control is there
                        st.level = Some(level);
                    }
                    Err(e) => {
                        st.primed = false; // asked again, and the device opened again, on the next call
                        log::info!("sound: could not set the software volume yet: {e}");
                    }
                }
            }
            return Synced { managed: true, soft_card: (st.level == Some(level)).then_some(card), note: None, busy: prime_failed && st.level != Some(level) };
        }
    }

    /// Make the file say `plan` (a fresh start for the control when it changes).
    fn write_plan(&self, st: &mut St, plan: &Plan) -> Result<(), String> {
        let existing = std::fs::read_to_string(&self.path).ok();
        // the capture PCMs with a named format ride along (micpcm.rs); learned after the first write, they land on the next sync
        let text = config_text_boost(plan, self.boost.load(std::sync::atomic::Ordering::SeqCst)) + &crate::micpcm::sections();
        if existing.as_deref() != Some(text.as_str()) {
            if let Err(e) = write_atomic(&self.path, &text) {
                st.reset();
                return Err(format!("could not write {}: {e}", self.path.display()));
            }
            log::info!("sound: wrote {} ({plan:?})", self.path.display());
            st.written = Some(plan.clone());
            st.primed = false;
            st.level = None;
        } else if st.written.as_ref() != Some(plan) {
            // the file is already right (written before this process started): the control may not exist yet
            st.written = Some(plan.clone());
            st.primed = false;
            st.level = None;
        }
        Ok(())
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("iyakki-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A fake machine: records every call; the control exists once the device was primed (or `control_exists`).
    #[derive(Clone, Default)]
    pub struct Fake {
        pub calls: Arc<Mutex<Vec<String>>>,
        pub control_exists: Arc<Mutex<bool>>,
        pub prime_fails: Arc<Mutex<bool>>,
        pub no_amixer: Arc<Mutex<bool>>,
        /// The config file the fake "reads" when it opens a device, and whether a dmix in it cannot be opened.
        pub conf: Arc<Mutex<Option<PathBuf>>>,
        pub dmix_fails: Arc<Mutex<bool>>,
        /// Opening fails whatever the file says (the device is busy).
        pub busy: Arc<Mutex<bool>>,
    }

    impl Runner for Fake {
        fn has_control(&self, _card: u32) -> bool {
            !*self.no_amixer.lock().unwrap() && *self.control_exists.lock().unwrap()
        }
        fn amixer(&self, args: &[String]) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("amixer {}", args.join(" ")));
            if *self.no_amixer.lock().unwrap() {
                return Err("amixer: not found".into());
            }
            if *self.control_exists.lock().unwrap() { Ok(String::new()) } else { Err("Unable to find simple control 'IYAKKI',0".into()) }
        }
        fn prime(&self, pcm: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("prime {pcm}"));
            if *self.prime_fails.lock().unwrap() || *self.busy.lock().unwrap() {
                return Err("busy".into());
            }
            let conf = self.conf.lock().unwrap().clone();
            if *self.dmix_fails.lock().unwrap() && conf.is_some_and(|p| std::fs::read_to_string(p).unwrap_or_default().contains("type dmix")) {
                return Err("unable to open slave: Invalid argument".into());
            }
            *self.control_exists.lock().unwrap() = true;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn dir(tag: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("iyakki-softvol-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    #[test]
    fn the_config_text_is_a_softvol_over_the_output_or_a_plug_when_the_card_has_a_volume() {
        let t = config_text(&Plan::Soft { card: 1, device: 0 });
        assert!(t.starts_with(MARKER));
        // what was verified by hand on the Mac mini 2018
        for want in ["pcm.iyakkivol {", "type softvol", "slave.pcm \"plughw:1,0\"", "name \"IYAKKI\"", "card 1", "min_dB -51.0", "max_dB 0.0", "pcm.iyakki {", "pcm.!default {", "slave.pcm \"iyakkivol\"", "ctl.!default {", "type hw"] {
            assert!(t.contains(want), "{want} missing in\n{t}");
        }
        let hdmi = config_text(&Plan::Soft { card: 0, device: 3 });
        assert!(hdmi.contains("slave.pcm \"plughw:0,3\"") && hdmi.contains("card 0"), "{hdmi}");
        let hw = config_text(&Plan::Hw { card: 0, device: 0 });
        assert!(hw.starts_with(MARKER) && !hw.contains("type softvol") && hw.contains("slave.pcm \"hw:0,0\"") && hw.contains("pcm.!default") && hw.contains("pcm.iyakki {"), "{hw}");
        assert_eq!(braces(&t), 0);
        assert_eq!(braces(&hw), 0);
    }

    fn braces(t: &str) -> i32 {
        t.chars().map(|c| match c {
            '{' => 1,
            '}' => -1,
            _ => 0,
        }).sum()
    }

    #[test]
    fn hardware_volume_controls_are_found_and_the_softvol_control_is_not_one() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(hw_control(&s(&["Headphone Playback Switch", "Master Playback Volume", "PCM Playback Volume"])).as_deref(), Some("Master"));
        assert_eq!(hw_control(&s(&["Speaker Playback Volume"])).as_deref(), Some("Speaker"));
        // the T2 card, the HDMI card, and a card that has IYAKKI's own softvol control from before
        assert_eq!(hw_control(&s(&["Codec Output Jack"])), None);
        assert_eq!(hw_control(&s(&["IEC958 Playback Switch", "IEC958 Playback Default"])), None);
        assert_eq!(hw_control(&s(&["IYAKKI", "Codec Output Jack"])), None);
    }

    #[test]
    fn the_softvol_is_primed_once_then_the_level_is_set_and_repeated_syncs_do_nothing() {
        let d = dir("prime");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        let sv = Softvol::new(&conf, fake.clone());
        let plan = Plan::Soft { card: 1, device: 0 };
        let r = sv.sync(Some(&plan), 30);
        assert_eq!(r, Synced { managed: true, soft_card: Some(1), note: None, busy: false });
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&plan));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"], "opened once before the first volume change");
        fake.calls.lock().unwrap().clear();
        assert_eq!(sv.sync(Some(&plan), 30).soft_card, Some(1));
        assert!(fake.calls.lock().unwrap().is_empty(), "nothing changed, nothing run");
        sv.sync(Some(&plan), 55);
        assert_eq!(*fake.calls.lock().unwrap(), ["amixer -q -c 1 sset IYAKKI 55%"], "a new level is set without priming again");
    }

    #[test]
    fn mute_is_zero_percent_and_unmute_restores_the_level() {
        let d = dir("mute");
        let fake = Fake::default();
        let sv = Softvol::new(d.0.join("asound.conf"), fake.clone());
        let plan = Plan::Soft { card: 1, device: 0 };
        sv.sync(Some(&plan), 40);
        sv.sync(Some(&plan), 0);
        sv.sync(Some(&plan), 40);
        let c = fake.calls.lock().unwrap().clone();
        assert_eq!(c[1..], ["amixer -q -c 1 sset IYAKKI 40%", "amixer -q -c 1 sset IYAKKI 0%", "amixer -q -c 1 sset IYAKKI 40%"]);
    }

    #[test]
    fn switching_outputs_rewrites_the_file_and_primes_the_new_device() {
        let d = dir("switch");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        let sv = Softvol::new(&conf, fake.clone());
        sv.sync(Some(&Plan::Soft { card: 1, device: 0 }), 50);
        // headphones plugged in: same card, other device
        let r = sv.sync(Some(&Plan::Soft { card: 1, device: 2 }), 20);
        assert_eq!(r.soft_card, Some(1));
        assert!(std::fs::read_to_string(&conf).unwrap().contains("plughw:1,2"));
        // a monitor on HDMI (card 0, no volume either)
        let r = sv.sync(Some(&Plan::Soft { card: 0, device: 3 }), 70);
        assert_eq!(r.soft_card, Some(0));
        let t = std::fs::read_to_string(&conf).unwrap();
        assert!(t.contains("plughw:0,3") && t.contains("card 0") && !t.contains("plughw:1"), "{t}");
        let c = fake.calls.lock().unwrap().clone();
        // the fake's control stays once created, so the later levels are set WITHOUT opening the device (volume applied when the
        // control exists; before the sound doctor work every file change opened it again)
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 50%", "amixer -q -c 1 sset IYAKKI 20%", "amixer -q -c 0 sset IYAKKI 70%"]);
        // a USB card with a hardware volume: a plain plug, no softvol, nothing primed or set
        fake.calls.lock().unwrap().clear();
        let r = sv.sync(Some(&Plan::Hw { card: 2, device: 0 }), 70);
        assert_eq!(r, Synced { managed: true, soft_card: None, note: None, busy: false });
        assert!(!std::fs::read_to_string(&conf).unwrap().contains("type softvol"));
        assert!(fake.calls.lock().unwrap().is_empty(), "the hardware control is left alone");
        // no output any more: IYAKKI's file goes
        assert_eq!(sv.sync(None, 70), Synced::default());
        assert!(!conf.exists());
    }

    #[test]
    fn a_file_someone_else_wrote_is_never_changed_and_an_unwritable_path_is_said() {
        let d = dir("foreign");
        let conf = d.0.join("asound.conf");
        std::fs::write(&conf, "pcm.!default { type hw card 2 }\n").unwrap();
        let fake = Fake::default();
        let sv = Softvol::new(&conf, fake.clone());
        let r = sv.sync(Some(&Plan::Soft { card: 1, device: 0 }), 50);
        assert!(!r.managed && r.note.as_deref().unwrap().contains("not written by IYAKKI"), "{r:?}");
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "pcm.!default { type hw card 2 }\n");
        assert!(sv.sync(None, 50).note.is_some() && conf.exists(), "not removed either");
        assert!(fake.calls.lock().unwrap().is_empty());
        let sv = Softvol::new(d.0.join("missing-dir/asound.conf"), fake.clone());
        let r = sv.sync(Some(&Plan::Soft { card: 1, device: 0 }), 50);
        assert!(!r.managed && r.note.as_deref().unwrap().contains("could not write"), "{r:?}");
    }

    #[test]
    fn a_level_that_cannot_be_set_yet_is_tried_again_and_is_not_claimed() {
        let d = dir("retry");
        let fake = Fake::default();
        *fake.prime_fails.lock().unwrap() = true;
        let sv = Softvol::new(d.0.join("asound.conf"), fake.clone());
        let plan = Plan::Soft { card: 1, device: 0 };
        let r = sv.sync(Some(&plan), 30);
        assert!(r.managed && r.soft_card.is_none(), "the file is used but IYAKKI keeps scaling its own sound: {r:?}");
        // a player opened the device meanwhile (the control now exists)
        *fake.control_exists.lock().unwrap() = true;
        assert_eq!(sv.sync(Some(&plan), 30).soft_card, Some(1));
        let c = fake.calls.lock().unwrap().clone();
        // the control exists the second time: set without opening the device again
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%", "amixer -q -c 1 sset IYAKKI 30%"]);
        // without amixer at all the level is never claimed
        let fake = Fake::default();
        *fake.no_amixer.lock().unwrap() = true;
        let sv = Softvol::new(d.0.join("asound2.conf"), fake);
        assert_eq!(sv.sync(Some(&plan), 30).soft_card, None);
    }

    #[test]
    fn a_file_left_by_an_earlier_start_is_taken_over_and_the_control_set_again() {
        let d = dir("restart");
        let conf = d.0.join("asound.conf");
        let plan = Plan::Soft { card: 1, device: 0 };
        std::fs::write(&conf, config_text(&plan)).unwrap();
        let fake = Fake::default();
        let sv = Softvol::new(&conf, fake.clone());
        assert_eq!(sv.sync(Some(&plan), 65).soft_card, Some(1));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 1 sset IYAKKI 65%"], "the remembered volume is restored at start");
    }

    #[test]
    fn a_shared_softvol_puts_a_dmix_with_its_own_key_under_the_volume_for_each_kind_of_output() {
        // HDMI (verified by hand on the Mac mini 2018): hw slave, 48 kHz stereo 16-bit, 1024/8192
        let hdmi = config_text(&Plan::Mix { card: 1, device: 3, mix: Mix::STEREO });
        for want in ["pcm.iyakkimix {", "type dmix", "ipc_key 24747", "ipc_perm 0666", "pcm \"hw:1,3\"", "rate 48000", "channels 2", "format S16_LE", "period_size 1024", "buffer_size 8192", "pcm.iyakkivol {", "type softvol", "slave.pcm \"iyakkimix\"", "name \"IYAKKI\"", "card 1", "pcm.iyakki {", "pcm.!default {", "slave.pcm \"iyakkivol\"", "ctl.!default {"] {
            assert!(hdmi.contains(want), "{want} missing in\n{hdmi}");
        }
        assert!(!hdmi.contains("iyakkimono") && !hdmi.contains("plughw") && hdmi.starts_with(MARKER) && braces(&hdmi) == 0);
        // the T2 speaker: one channel, 24-bit, the chip's own period; stereo is mixed down by a route plugin under the volume
        let spk = config_text(&Plan::Mix { card: 0, device: 0, mix: Mix::T2_MONO });
        for want in ["pcm \"hw:0,0\"", "channels 1", "format S24_LE", "pcm.iyakkimono {", "type route", "0.0 0.5", "1.0 0.5", "slave.pcm \"iyakkimono\"", "ipc_key 24680"] {
            assert!(spk.contains(want), "{want} missing in\n{spk}");
        }
        assert!(!spk.contains("period_size") && braces(&spk) == 0, "{spk}");
        // the T2 headphone jack: stereo 24-bit, no route
        let hp = config_text(&Plan::Mix { card: 0, device: 2, mix: Mix::T2_STEREO });
        assert!(hp.contains("pcm \"hw:0,2\"") && hp.contains("format S24_LE") && hp.contains("channels 2") && !hp.contains("iyakkimono") && !hp.contains("period_size"), "{hp}");
        // every card and device has its own server key
        let mut keys = std::collections::BTreeSet::new();
        for card in 0..4 {
            for device in 0..16 {
                assert!(keys.insert(ipc_key(card, device)), "{card},{device}");
            }
        }
        assert_eq!((ipc_key(0, 0), ipc_key(1, 3)), (24_680, 24_747));
        // the exclusive form and the hardware-volume form are as before
        let excl = config_text(&Plan::Soft { card: 1, device: 0 });
        assert!(excl.contains("slave.pcm \"plughw:1,0\"") && !excl.contains("dmix"));
        assert!(!config_text(&Plan::Hw { card: 0, device: 0 }).contains("dmix"));
    }

    #[test]
    fn a_shared_softvol_is_primed_and_set_like_the_exclusive_one_and_switching_outputs_changes_the_key() {
        let d = dir("mix");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        let sv = Softvol::new(&conf, fake.clone());
        let hdmi = Plan::Mix { card: 1, device: 3, mix: Mix::STEREO };
        assert_eq!(sv.sync(Some(&hdmi), 30), Synced { managed: true, soft_card: Some(1), note: None, busy: false });
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&hdmi));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"]);
        // mute and unmute: 0 % and back, nothing else
        sv.sync(Some(&hdmi), 0);
        sv.sync(Some(&hdmi), 30);
        assert_eq!(fake.calls.lock().unwrap()[2..], ["amixer -q -c 1 sset IYAKKI 0%", "amixer -q -c 1 sset IYAKKI 30%"]);
        // headphones of the T2 card: no dmix (it spins a core there), the volume over plughw
        let hp = Plan::Mix { card: 0, device: 2, mix: Mix::T2_STEREO };
        assert_eq!(sv.sync(Some(&hp), 30).soft_card, Some(0));
        let t = std::fs::read_to_string(&conf).unwrap();
        assert!(!t.contains("dmix") && t.contains("plughw:0,2") && !t.contains("hw:1,3"), "{t}");
        // a card with a hardware volume: a plain plug again
        assert!(sv.sync(Some(&Plan::Hw { card: 2, device: 0 }), 30).soft_card.is_none());
        assert!(!std::fs::read_to_string(&conf).unwrap().contains("dmix"));
    }

    #[test]
    fn a_device_dmix_cannot_open_falls_back_to_the_exclusive_volume_and_stays_there() {
        let d = dir("fallback");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        *fake.conf.lock().unwrap() = Some(conf.clone());
        *fake.dmix_fails.lock().unwrap() = true;
        let sv = Softvol::new(&conf, fake.clone());
        let plan = Plan::Mix { card: 0, device: 0, mix: Mix::T2_MONO };
        let r = sv.sync(Some(&plan), 40);
        assert_eq!(r, Synced { managed: true, soft_card: Some(0), note: None, busy: false }, "the volume works anyway");
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&Plan::Soft { card: 0, device: 0 }), "the T2: no dmix, the volume over plughw");
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 0 sset IYAKKI 40%"], "a fixed-period device never tries dmix");
        // later calls (every 3 s) do not try dmix again for that device
        fake.calls.lock().unwrap().clear();
        assert_eq!(sv.sync(Some(&plan), 40).soft_card, Some(0));
        assert_eq!(sv.sync(Some(&plan), 50).soft_card, Some(0));
        assert_eq!(*fake.calls.lock().unwrap(), ["amixer -q -c 0 sset IYAKKI 50%"]);
        assert!(!std::fs::read_to_string(&conf).unwrap().contains("dmix"));
        // another device still gets its dmix (the headphones may be fine)
        *fake.dmix_fails.lock().unwrap() = false;
        sv.sync(Some(&Plan::Mix { card: 1, device: 3, mix: Mix::STEREO }), 50);
        assert!(std::fs::read_to_string(&conf).unwrap().contains("type dmix"));
    }

    #[test]
    fn a_fixed_period_device_gets_a_web_pcm_that_never_converts_the_rate() {
        // measured on a MacBookPro16,2: through plug the card played 24/16/12/32 kHz at the wrong speed and could not take
        // F32 at 44.1 kHz; linear (format) over route (channels) offers 48 kHz and integer formats only
        let t = config_text(&Plan::Direct { card: 0, device: 0, channels: 4 });
        let want = "pcm.iyakkiwebroute {\n    type route\n    slave {\n        pcm \"iyakkivol\"\n        channels 4\n    }\n    ttable {\n        0.0 1.0\n        1.1 1.0\n        0.2 1.0\n        1.3 1.0\n    }\n}\npcm.iyakkiweb {\n    type linear\n    slave {\n        pcm \"iyakkiwebroute\"\n        format S24_LE\n    }\n}\n";
        assert!(t.ends_with(want), "{t}");
        assert_eq!(braces(&t), 0);
        assert!(!want.contains("plug") && !want.contains("rate"));
        // other plans and the unprobed one-channel guess are unchanged
        assert!(!config_text(&Plan::Direct { card: 0, device: 0, channels: 1 }).contains(WEB_PCM));
        assert!(!config_text(&Plan::Soft { card: 0, device: 0 }).contains(WEB_PCM));
        assert!(!config_text(&Plan::Hw { card: 0, device: 0 }).contains(WEB_PCM));
        // the engine's own file: the system's configuration, then the web PCM is the default
        assert_eq!(WEB_ALSA_CONF, "</usr/share/alsa/alsa.conf>\npcm.!default iyakkiweb\n");
    }

    #[test]
    fn the_t2_keeps_plughw_for_the_players_and_gives_the_engine_a_web_pcm_over_hw_with_the_same_slider() {
        // MacBookPro16,2: the players' S16 stereo plays through plug > softvol > plughw, but the engine's WebAudio could not start
        // its output on that chain (the page froze on the synchronous start); it gets linear > route > softvol > hw
        let t = config_text(&Plan::SoftWeb { card: 0, device: 0, channels: 4 });
        assert!(t.starts_with(MARKER) && braces(&t) == 0, "{t}");
        assert!(t.contains("slave.pcm \"plughw:0,0\"") && t.contains("pcm.!default {\n    type plug\n    slave.pcm \"iyakkivol\""), "{t}");
        assert!(t.contains("pcm.iyakkiwebvol {\n    type softvol\n    slave.pcm \"hw:0,0\""), "{t}");
        assert_eq!(t.matches("name \"IYAKKI\"").count(), 2, "one control, two softvols");
        assert!(t.ends_with("pcm.iyakkiweb {\n    type plug\n    slave.pcm \"iyakkiwebvol\"\n}\n"), "{t}");
        assert!(!t.contains("iyakkiwebroute") && !t.contains("type linear"), "{t}");
        // the volume boost reaches both ways in
        assert!(config_text_boost(&Plan::SoftWeb { card: 0, device: 0, channels: 4 }, 6).matches("max_dB 6.0").count() == 2);
        // an unprobed one-channel guess is exactly the plain plan
        assert_eq!(config_text(&Plan::SoftWeb { card: 0, device: 0, channels: 1 }), config_text(&Plan::Soft { card: 0, device: 0 }));
    }

    #[test]
    fn the_exclusive_volume_over_a_fixed_period_device_has_no_plughw_under_it_and_other_devices_keep_plughw() {
        // MacBookPro16,2 (aaudio, period of one frame): plug > softvol > plughw cannot install its parameters (radio and the
        // story reader were silent); plug > softvol > hw plays at 44.1 kHz stereo.
        let direct = config_text(&Plan::Direct { card: 0, device: 0, channels: 4 });
        assert!(direct.starts_with(MARKER) && direct.contains("slave.pcm \"hw:0,0\"") && !direct.contains("plughw") && direct.contains("type softvol") && direct.contains("pcm.!default"), "{direct}");
        assert_eq!(braces(&direct), 0);
        let d = dir("direct-vs-soft");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        *fake.conf.lock().unwrap() = Some(conf.clone());
        *fake.dmix_fails.lock().unwrap() = true;
        let sv = Softvol::new(&conf, fake.clone());
        sv.sync(Some(&Plan::Mix { card: 1, device: 3, mix: Mix::STEREO }), 40);
        assert!(std::fs::read_to_string(&conf).unwrap().contains("slave.pcm \"plughw:1,3\""), "a device with sized periods keeps plughw");
    }

    #[test]
    fn when_the_device_is_busy_for_another_reason_dmix_is_kept_and_tried_again() {
        let d = dir("busy");
        let conf = d.0.join("asound.conf");
        let fake = Fake::default();
        *fake.busy.lock().unwrap() = true;
        let sv = Softvol::new(&conf, fake.clone());
        let plan = Plan::Mix { card: 1, device: 3, mix: Mix::STEREO };
        let r = sv.sync(Some(&plan), 40);
        assert!(r.managed && r.soft_card.is_none(), "{r:?}");
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&plan), "not demoted: the exclusive device was busy too");
        *fake.busy.lock().unwrap() = false;
        assert_eq!(sv.sync(Some(&plan), 40).soft_card, Some(1));
        assert!(std::fs::read_to_string(&conf).unwrap().contains("type dmix"));
    }

    struct Closed(Arc<Mutex<Option<String>>>);
    impl Gate for Closed {
        fn refuse(&self, _card: u32) -> Option<String> {
            self.0.lock().unwrap().clone()
        }
    }

    fn gated(d: &Dir, fake: &Fake, why: &Arc<Mutex<Option<String>>>, now: &Arc<Mutex<Duration>>) -> Softvol {
        let n = now.clone();
        Softvol::new(d.0.join("asound.conf"), fake.clone()).with_gate(Closed(why.clone())).with_clock(move || *n.lock().unwrap())
    }

    #[test]
    fn a_busy_speaker_is_not_opened_and_the_open_is_deferred_with_a_growing_backoff() {
        let d = dir("gate");
        let fake = Fake::default();
        let why = Arc::new(Mutex::new(Some("card0/pcm0p/sub0 is Running (process 9)".to_string())));
        let now = Arc::new(Mutex::new(Duration::from_secs(10)));
        let sv = gated(&d, &fake, &why, &now);
        let plan = Plan::Soft { card: 0, device: 0 };
        let r = sv.sync(Some(&plan), 40);
        assert!(r.managed && r.soft_card.is_none() && !r.busy, "deferred quietly, nobody is asked to let go: {r:?}");
        assert!(fake.calls.lock().unwrap().is_empty(), "nothing opened");
        // calls inside the backoff do not even ask the gate again; after each wait the delay doubles (0.5, 1, 2, 4, 4 s)
        let mut waits = Vec::new();
        let first = sv.st.lock().unwrap().defer_until.unwrap() - *now.lock().unwrap();
        waits.push(first);
        for _ in 0..5 {
            // inside the backoff: no new decision
            let until = sv.st.lock().unwrap().defer_until.unwrap();
            *now.lock().unwrap() = until - Duration::from_millis(1);
            sv.sync(Some(&plan), 40);
            assert_eq!(sv.st.lock().unwrap().defer_until, Some(until), "the gate is not asked inside the backoff");
            // after it: refused again, a longer wait
            *now.lock().unwrap() = until;
            sv.sync(Some(&plan), 40);
            waits.push(sv.st.lock().unwrap().defer_until.unwrap() - until);
        }
        assert!(fake.calls.lock().unwrap().is_empty(), "still never opened while the gate says no");
        assert_eq!(waits[..4], BACKOFF, "{waits:?}");
        assert!(waits[4..].iter().all(|w| *w == BACKOFF[3]), "{waits:?}");
        assert!(waits.iter().all(|w| *w <= Duration::from_secs(4)), "never longer than 4 s: {waits:?}");
        // free now: after the backoff the open and the level happen
        *why.lock().unwrap() = None;
        *now.lock().unwrap() += Duration::from_secs(10);
        assert_eq!(sv.sync(Some(&plan), 40).soft_card, Some(0));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 0 sset IYAKKI 40%"]);
    }

    #[test]
    fn a_control_that_exists_is_set_without_opening_even_while_the_speaker_is_busy() {
        let d = dir("gate-exists");
        let fake = Fake::default();
        *fake.control_exists.lock().unwrap() = true;
        let why = Arc::new(Mutex::new(Some("web engine".to_string())));
        let now = Arc::new(Mutex::new(Duration::ZERO));
        let sv = gated(&d, &fake, &why, &now);
        assert_eq!(sv.sync(Some(&Plan::Soft { card: 0, device: 0 }), 55).soft_card, Some(0));
        assert_eq!(*fake.calls.lock().unwrap(), ["amixer -q -c 0 sset IYAKKI 55%"], "no prime");
    }

    #[test]
    fn a_vanished_control_is_not_recreated_while_the_web_engine_or_a_stream_holds_the_card() {
        let d = dir("gate-vanish");
        let fake = Fake::default();
        let why = Arc::new(Mutex::new(None));
        let now = Arc::new(Mutex::new(Duration::ZERO));
        let sv = gated(&d, &fake, &why, &now);
        let plan = Plan::Soft { card: 0, device: 0 };
        assert_eq!(sv.sync(Some(&plan), 40).soft_card, Some(0));
        // every stream closed: the control is gone; the person moves the volume while the web page plays
        *fake.control_exists.lock().unwrap() = false;
        *why.lock().unwrap() = Some("the web engine is playing audio".into());
        fake.calls.lock().unwrap().clear();
        let r = sv.sync(Some(&plan), 60);
        assert_eq!(r.soft_card, None);
        assert!(!fake.calls.lock().unwrap().iter().any(|c| c.starts_with("prime")), "{:?}", fake.calls.lock().unwrap());
    }

    #[test]
    fn the_web_media_flag_and_open_streams_close_the_real_gate() {
        let root = std::env::temp_dir().join(format!("softvol-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("card0/pcm0p/sub0");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("status"), "closed\n").unwrap();
        let g = ProcGate { root: root.clone() };
        note_web_media(false);
        assert_eq!(g.refuse(0), None);
        std::fs::write(sub.join("status"), "state: PREPARED\nowner_pid : 5\n").unwrap();
        assert!(g.refuse(0).unwrap().contains("Prepared"));
        std::fs::write(sub.join("status"), "closed\n").unwrap();
        note_web_media(true);
        assert!(g.refuse(0).unwrap().contains("web engine"));
        note_web_media(false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_prime_holds_the_device_for_a_fraction_of_a_second_at_most() {
        assert!(PRIME_HOLD <= Duration::from_millis(150));
        assert_eq!(PRIME_BYTES, 1920, "10 ms of silence");
    }

    #[test]
    fn the_long_standing_stereo_config_is_byte_for_byte_what_it_was() {
        let got = config_text(&Plan::Mix { card: 0, device: 3, mix: Mix::STEREO });
        let want = format!("{MARKER}\n# The output has no hardware volume: iyakkivol is a software volume (control 'IYAKKI' on card 0) over a dmix, so\n# IYAKKI's players and other programs (the web engine) play at once. dmix server key 24683.\npcm.iyakkimix {{\n    type dmix\n    ipc_key 24683\n    ipc_perm 0666\n    slave {{\n        pcm \"hw:0,3\"\n        rate 48000\n        channels 2\n        format S16_LE\n        period_size 1024\n        buffer_size 8192\n    }}\n}}\npcm.iyakkivol {{\n    type softvol\n    slave.pcm \"iyakkimix\"\n    control {{\n        name \"IYAKKI\"\n        card 0\n    }}\n    min_dB -51.0\n    max_dB 0.0\n}}\npcm.iyakki {{\n    type plug\n    slave.pcm \"iyakkivol\"\n}}\npcm.!default {{\n    type plug\n    slave.pcm \"iyakkivol\"\n}}\nctl.!default {{\n    type hw\n    card 0\n}}\n");
        assert_eq!(got, want);
        assert!(config_text(&Plan::Mix { card: 1, device: 0, mix: Mix::T2_MONO }).contains("pcm.iyakkimono"));
        assert!(!config_text(&Plan::Mix { card: 1, device: 0, mix: Mix::T2_STEREO }).contains("resolution"));
    }

    #[test]
    fn the_level_chain_does_not_attenuate_at_100_percent_and_the_steps_are_usable() {
        let four = Mix { format: "S32_LE", channels: 4, sized: true, rate: 44100, probed: true };
        let t = config_text(&Plan::Mix { card: 0, device: 0, mix: four });
        // 100 % is 0 dB, 256 volume steps, every channel gets weight 1.0 (no -6 dB from a mono mix-down)
        assert!(t.contains("max_dB 0.0") && t.contains("min_dB -51.0") && t.contains("resolution 256"), "{t}");
        let weights: Vec<&str> = t.lines().filter(|l| l.trim().chars().next().is_some_and(|c| c.is_ascii_digit()) && l.contains('.') && l.trim().split(' ').count() == 2).collect();
        assert_eq!(weights.len(), 4, "{t}");
        assert!(weights.iter().all(|l| l.trim().ends_with(" 1.0")), "{weights:?}");
        assert!(t.contains("rate 44100") && t.contains("format S32_LE") && t.contains("channels 4"));
        // the softvol maps the slider linearly in dB: a 5 % step is about 2.5 dB, 20 % is 40 dB below 100 %
        let per_step = (MAX_DB - MIN_DB) * 5.0 / 100.0;
        assert!((per_step - 2.55).abs() < 0.01, "{per_step}");
        assert!((MAX_DB - MIN_DB) * 0.8 > 40.0);
        assert!(RESOLUTION as f32 >= (MAX_DB - MIN_DB) * 2.0, "steps finer than 0.5 dB");
        // the boost moves the top of the range up; 0 leaves the text alone; it is capped
        let b = config_text_boost(&Plan::Mix { card: 0, device: 0, mix: four }, 6);
        assert!(b.contains("max_dB 6.0") && b.contains("min_dB -51.0"), "{b}");
        assert_eq!(config_text_boost(&Plan::Mix { card: 0, device: 0, mix: four }, 0), t);
        assert!(config_text_boost(&Plan::Soft { card: 0, device: 0 }, 99).contains("max_dB 12.0"));
        assert_eq!(braces(&b), 0);
    }

    #[test]
    fn a_four_channel_codec_keeps_its_fan_out_when_the_shared_mixer_could_not_be_opened() {
        // the MacBookPro14,1's codec: the plan is the shared mixer over four channels; when dmix is refused the exclusive plan must still
        // copy the stereo signal to all four channels, or only the front pair plays and the speakers are quiet
        let plan = Plan::Mix { card: 0, device: 0, mix: Mix { format: "S32_LE", channels: 4, sized: true, rate: 44_100, probed: true } };
        let fallback = plan.exclusive_of();
        assert_eq!(fallback, Plan::SoftFan { card: 0, device: 0, channels: 4, format: Some("S32_LE") });
        let t = config_text(&fallback);
        assert!(t.contains("slave.pcm \"iyakkifan\""), "{t}");
        assert!(t.contains("pcm \"plughw:0,0\"") && t.contains("channels 4"), "{t}");
        // the learned format is named for the slave and for the speaker-test PCM: without it plug picked S24_LE on this codec and the
        // sound was 48 dB too quiet (2026-11-08, MacBookPro14,1, stream hw_params showed S24_LE)
        assert_eq!(t.matches("format S32_LE").count(), 2, "{t}");
        assert!(t.contains("        channels 4\n        format S32_LE\n    }"), "the format sits in the route's slave block: {t}");
        // a fan-out whose format was never learned keeps the old text (plug chooses)
        let unlearned = Plan::SoftFan { card: 0, device: 0, channels: 4, format: None };
        assert!(!config_text(&unlearned).contains("format "), "{}", config_text(&unlearned));
        for (src, dst) in [(0, 0), (1, 1), (0, 2), (1, 3)] {
            assert!(t.contains(&format!("{src}.{dst} 1.0")), "{src}.{dst} missing in {t}");
        }
        assert!(t.contains("pcm.iyakkichan"), "the speaker tests need it: {t}");
        assert!(t.contains(MARKER));
        // a plain stereo device is unchanged
        assert_eq!(Plan::Mix { card: 1, device: 2, mix: Mix::STEREO }.exclusive_of(), Plan::Soft { card: 1, device: 2 });
    }

}
