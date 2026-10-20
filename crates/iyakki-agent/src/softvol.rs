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
}

impl Mix {
    /// Stereo 16-bit, what HDMI, USB and ordinary analog devices take.
    pub const STEREO: Mix = Mix { format: "S16_LE", channels: 2, sized: true };
    /// The T2 Mac's headphone jack: stereo, 24-bit in 32.
    pub const T2_STEREO: Mix = Mix { format: "S24_LE", channels: 2, sized: false };
    /// The T2 Mac's built-in speaker: one channel, 24-bit in 32.
    pub const T2_MONO: Mix = Mix { format: "S24_LE", channels: 1, sized: false };
}

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
    /// The card has a hardware volume: `default` is the output, the hardware control stays.
    Hw { card: u32, device: u32 },
}

impl Plan {
    pub fn card(&self) -> u32 {
        match self {
            Plan::Mix { card, .. } | Plan::Soft { card, .. } | Plan::Hw { card, .. } => *card,
        }
    }

    fn device(&self) -> u32 {
        match self {
            Plan::Mix { device, .. } | Plan::Soft { device, .. } | Plan::Hw { device, .. } => *device,
        }
    }

    /// The software volume's card and whether it is shared (dmix).
    fn is_mix(&self) -> bool {
        matches!(self, Plan::Mix { .. })
    }
}

fn softvol_block(card: u32, slave: &str) -> String {
    format!(
        "pcm.{SOFTVOL_PCM} {{\n    type softvol\n    slave.pcm \"{slave}\"\n    control {{\n        name \"{CONTROL}\"\n        card {card}\n    }}\n    min_dB {MIN_DB:.1}\n    max_dB {MAX_DB:.1}\n}}\n\
         pcm.{PCM} {{\n    type plug\n    slave.pcm \"{SOFTVOL_PCM}\"\n}}\n\
         pcm.!default {{\n    type plug\n    slave.pcm \"{SOFTVOL_PCM}\"\n}}\n\
         ctl.!default {{\n    type hw\n    card {card}\n}}\n"
    )
}

/// The text of `/etc/asound.conf` for this plan.
pub fn config_text(plan: &Plan) -> String {
    match plan {
        Plan::Mix { card, device, mix } => {
            let key = ipc_key(*card, *device);
            let sized = if mix.sized { "\n        period_size 1024\n        buffer_size 8192" } else { "" };
            let (route, slave) = if mix.channels == 1 {
                (format!("pcm.iyakkimono {{\n    type route\n    slave {{\n        pcm \"iyakkimix\"\n        channels 1\n    }}\n    ttable {{\n        0.0 0.5\n        1.0 0.5\n    }}\n}}\n"), "iyakkimono")
            } else {
                (String::new(), "iyakkimix")
            };
            format!(
                "{MARKER}\n\
                 # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}) over a dmix, so\n\
                 # IYAKKI's players and other programs (the web engine) play at once. dmix server key {key}.\n\
                 pcm.iyakkimix {{\n    type dmix\n    ipc_key {key}\n    ipc_perm 0666\n    slave {{\n        pcm \"hw:{card},{device}\"\n        rate {MIX_RATE}\n        channels {ch}\n        format {fmt}{sized}\n    }}\n}}\n\
                 {route}{rest}",
                ch = mix.channels,
                fmt = mix.format,
                rest = softvol_block(*card, slave)
            )
        }
        Plan::Soft { card, device } => format!(
            "{MARKER}\n\
             # The output has no hardware volume: {SOFTVOL_PCM} is a software volume (control '{CONTROL}' on card {card}); one program at a time.\n\
             {}",
            softvol_block(*card, &format!("plughw:{card},{device}"))
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
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(()),
                Ok(Some(st)) => return Err(format!("aplay -D {pcm} ended with {st} (the device may be busy)")),
                Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("aplay -D {pcm} did not finish"));
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
}

impl St {
    /// Forget the file and the control, but not which devices cannot do dmix.
    fn reset(&mut self) {
        *self = St { exclusive: std::mem::take(&mut self.exclusive), ..Default::default() };
    }
}

/// Keeps the ALSA configuration in step with the chosen output.
pub struct Softvol {
    path: PathBuf,
    runner: Box<dyn Runner>,
    st: Mutex<St>,
}

impl Softvol {
    pub fn new(path: impl Into<PathBuf>, runner: impl Runner + 'static) -> Softvol {
        Softvol { path: path.into(), runner: Box::new(runner), st: Mutex::new(St::default()) }
    }

    /// The real one: `/etc/asound.conf`, `amixer`, `aplay`.
    pub fn system() -> Softvol {
        Softvol::new(ASOUND_CONF, System)
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
            Plan::Mix { card, device, .. } if st.exclusive.contains(&(*card, *device)) => Plan::Soft { card: *card, device: *device },
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
                Plan::Mix { card, .. } | Plan::Soft { card, .. } => card,
            };
            if st.level != Some(level) {
                if !st.primed {
                    match self.runner.prime(PCM) {
                        Ok(()) => st.primed = true,
                        Err(e) if plan.is_mix() => {
                            // dmix (or the format it asks of the device) was refused: the exclusive softvol, as before
                            log::info!("sound: dmix on card {card} device {} could not be opened ({e}); using the exclusive software volume", plan.device());
                            st.exclusive.push((card, plan.device()));
                            plan = Plan::Soft { card, device: plan.device() };
                            demoted = true;
                            continue;
                        }
                        Err(e) if demoted => {
                            // not dmix's fault (the device is busy, or gone): try dmix again on the next call
                            log::info!("sound: could not open {PCM} on card {card} either way: {e}");
                            let device = plan.device();
                            st.exclusive.retain(|d| *d != (card, device));
                            if let Plan::Soft { .. } = plan {
                                let back = wanted.clone();
                                let _ = self.write_plan(&mut st, &back);
                            }
                            return Synced { managed: true, ..Default::default() };
                        }
                        Err(e) => log::info!("sound: could not open {PCM} once to create its volume control: {e}"),
                    }
                }
                let args = vec!["-q".to_string(), "-c".to_string(), card.to_string(), "sset".to_string(), CONTROL.to_string(), format!("{level}%")];
                match self.runner.amixer(&args) {
                    Ok(_) => {
                        st.primed = true; // the control is there
                        st.level = Some(level);
                    }
                    Err(e) => log::info!("sound: could not set the software volume yet: {e}"),
                }
            }
            return Synced { managed: true, soft_card: (st.level == Some(level)).then_some(card), note: None };
        }
    }

    /// Make the file say `plan` (a fresh start for the control when it changes).
    fn write_plan(&self, st: &mut St, plan: &Plan) -> Result<(), String> {
        let existing = std::fs::read_to_string(&self.path).ok();
        let text = config_text(plan);
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
        assert_eq!(r, Synced { managed: true, soft_card: Some(1), note: None });
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
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 50%", "prime iyakki", "amixer -q -c 1 sset IYAKKI 20%", "prime iyakki", "amixer -q -c 0 sset IYAKKI 70%"]);
        // a USB card with a hardware volume: a plain plug, no softvol, nothing primed or set
        fake.calls.lock().unwrap().clear();
        let r = sv.sync(Some(&Plan::Hw { card: 2, device: 0 }), 70);
        assert_eq!(r, Synced { managed: true, soft_card: None, note: None });
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
        assert_eq!(c, ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%", "prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"]);
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
        assert_eq!(sv.sync(Some(&hdmi), 30), Synced { managed: true, soft_card: Some(1), note: None });
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&hdmi));
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "amixer -q -c 1 sset IYAKKI 30%"]);
        // mute and unmute: 0 % and back, nothing else
        sv.sync(Some(&hdmi), 0);
        sv.sync(Some(&hdmi), 30);
        assert_eq!(fake.calls.lock().unwrap()[2..], ["amixer -q -c 1 sset IYAKKI 0%", "amixer -q -c 1 sset IYAKKI 30%"]);
        // headphones of the T2 card: another dmix server, another key
        let hp = Plan::Mix { card: 0, device: 2, mix: Mix::T2_STEREO };
        assert_eq!(sv.sync(Some(&hp), 30).soft_card, Some(0));
        let t = std::fs::read_to_string(&conf).unwrap();
        assert!(t.contains("ipc_key 24682") && t.contains("hw:0,2") && !t.contains("hw:1,3"), "{t}");
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
        assert_eq!(r, Synced { managed: true, soft_card: Some(0), note: None }, "the volume works anyway");
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), config_text(&Plan::Soft { card: 0, device: 0 }), "the exclusive device, as before");
        assert_eq!(*fake.calls.lock().unwrap(), ["prime iyakki", "prime iyakki", "amixer -q -c 0 sset IYAKKI 40%"]);
        // later calls (every 3 s) do not try dmix again for that device
        fake.calls.lock().unwrap().clear();
        assert_eq!(sv.sync(Some(&plan), 40).soft_card, Some(0));
        assert_eq!(sv.sync(Some(&plan), 50).soft_card, Some(0));
        assert_eq!(*fake.calls.lock().unwrap(), ["amixer -q -c 0 sset IYAKKI 50%"]);
        assert!(!std::fs::read_to_string(&conf).unwrap().contains("dmix"));
        // another device still gets its dmix (the headphones may be fine)
        *fake.dmix_fails.lock().unwrap() = false;
        sv.sync(Some(&Plan::Mix { card: 0, device: 2, mix: Mix::T2_STEREO }), 50);
        assert!(std::fs::read_to_string(&conf).unwrap().contains("type dmix"));
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
}
