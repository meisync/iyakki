//! Microphone input that is audible from the first start.
//!
//! A sound card's capture controls start wherever the kernel left them: on many laptops (seen on a Lenovo Legion Pro 5 16ARX8, Realtek
//! ALC287) `Capture` is muted or at 0 %, so the built-in microphone records silence and the person sees "the microphone does not work".
//! IYAKKI manages the output volume itself but never touched the input. This module does it **once per card**: the first time a card is seen
//! with capture controls, every capture volume goes to [`LEVEL`] and every capture switch is switched on; a marker file on the data volume
//! records that, so a level the person sets later is never overwritten. Boost controls (`Mic Boost`, `Internal Mic Boost`) go to a
//! little up ([`BOOST_LEVEL`]) at the same time: too much boost distorts, none leaves a quiet microphone.
//!
//! The world is reached through two arguments (the `amixer` runner and the marker folder), so tests use fakes and a temporary folder.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// The capture level set on first sight, in percent.
pub const LEVEL: u32 = 70;
/// The level of the boost controls (`Mic Boost`, `Internal Mic Boost`) on first sight: a little up, since too much boost distorts.
pub const BOOST_LEVEL: u32 = 20;
/// Looks at a card that has no capture control before it is given up (the codec of a laptop registers its controls a little after the card).
const GIVE_UP_AFTER: u32 = 6;

/// One simple mixer control with a capture side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub name: String,
    pub index: u32,
    pub volume: bool,
    pub switch: bool,
}

/// The capture controls in the text of `amixer -c N scontents`: those with `cvolume` or `cswitch`, without boost controls and without
/// IYAKKI's own software volume.
pub fn capture_controls(scontents: &str) -> Vec<Control> {
    let mut out = Vec::new();
    let mut cur: Option<(String, u32)> = None;
    for line in scontents.lines() {
        if let Some(rest) = line.strip_prefix("Simple mixer control '") {
            cur = rest.rsplit_once("',").and_then(|(n, i)| Some((n.to_string(), i.trim().parse().ok()?)));
        } else if let (Some((name, index)), Some(caps)) = (&cur, line.trim().strip_prefix("Capabilities:")) {
            let has = |w: &str| caps.split_whitespace().any(|c| c == w);
            let (volume, switch) = (has("cvolume"), has("cswitch"));
            let low = name.to_ascii_lowercase();
            if (volume || switch) && !low.contains("boost") && name != "IYAKKI" {
                out.push(Control { name: name.clone(), index: *index, volume, switch });
            }
        }
    }
    out
}

/// The boost controls (`Mic Boost`, `Internal Mic Boost`, ...) with a volume, in the text of `amixer -c N scontents`.
pub fn boost_controls(scontents: &str) -> Vec<Control> {
    let mut out = Vec::new();
    let mut cur: Option<(String, u32)> = None;
    for line in scontents.lines() {
        if let Some(rest) = line.strip_prefix("Simple mixer control '") {
            cur = rest.rsplit_once("',").and_then(|(n, i)| Some((n.to_string(), i.trim().parse().ok()?)));
        } else if let (Some((name, index)), Some(caps)) = (&cur, line.trim().strip_prefix("Capabilities:")) {
            let has = |w: &str| caps.split_whitespace().any(|c| c == w);
            if (has("cvolume") || has("volume")) && name.to_ascii_lowercase().contains("boost") {
                out.push(Control { name: name.clone(), index: *index, volume: true, switch: false });
            }
        }
    }
    out
}

/// The `amixer` arguments that put each boost control at [`BOOST_LEVEL`].
pub fn boost_commands(controls: &[Control]) -> Vec<Vec<String>> {
    controls.iter().map(|c| vec!["-q".to_string(), "sset".to_string(), format!("{},{}", c.name, c.index), format!("{BOOST_LEVEL}%")]).collect()
}

/// The `amixer` arguments (after `-c N`) that give each control its default: the volume to [`LEVEL`], the capture switch on.
pub fn commands(controls: &[Control]) -> Vec<Vec<String>> {
    controls
        .iter()
        .map(|c| {
            let mut a = vec!["-q".to_string(), "sset".to_string(), format!("{},{}", c.name, c.index)];
            if c.volume {
                a.push(format!("{LEVEL}%"));
            }
            a.push("cap".to_string());
            a
        })
        .collect()
}

/// `(card number, id)` of every card in the text of `/proc/asound/cards`.
pub fn cards(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|l| {
            let (n, rest) = l.trim_start().split_once(' ')?;
            let n: u32 = n.parse().ok()?;
            let id = rest.trim_start().strip_prefix('[')?.split_once(']')?.0.trim().to_string();
            (!id.is_empty()).then_some((n, id))
        })
        .collect()
}

fn marker_name(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

static TRIES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
static SET_THIS_START: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// One look: every card not yet set in this start of IYAKKI gets its capture defaults, once per start. `amixer` runs `amixer <args>` and returns its text; `markers` is the
/// folder on the data volume. Returns one log line per card that was set up.
pub fn step(proc_root: &Path, markers: &Path, amixer: &dyn Fn(&[String]) -> Result<String, String>) -> Vec<String> {
    let mut lines = Vec::new();
    let text = std::fs::read_to_string(proc_root.join("asound/cards")).unwrap_or_default();
    for (n, id) in cards(&text) {
        let marker = markers.join(marker_name(&id));
        // nothing saves the mixer (no alsactl state), so every start begins muted again: the defaults go back once per start of IYAKKI, not once per card ever
        // (the marker only says whether the card was seen before, for the log line)
        let key = format!("{}:{id}", markers.display());
        if SET_THIS_START.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).contains(&key) {
            continue;
        }
        let first = !marker.exists();
        let mut tries = TRIES.lock().unwrap_or_else(|e| e.into_inner());
        let t = tries.get_or_insert_with(HashMap::new).entry(format!("{}:{id}", markers.display())).or_insert(0);
        if *t >= GIVE_UP_AFTER {
            continue;
        }
        *t += 1;
        drop(tries);
        let c = n.to_string();
        let Ok(out) = amixer(&["-c".into(), c.clone(), "scontents".into()]) else { continue };
        let controls = capture_controls(&out);
        if controls.is_empty() {
            continue;
        }
        let mut ok = 0;
        // the boost controls ride along (their failure does not matter: not every card has one)
        for cmd in boost_commands(&boost_controls(&out)) {
            let mut args = vec!["-c".to_string(), c.clone()];
            args.extend(cmd);
            let _ = amixer(&args);
        }
        for cmd in commands(&controls) {
            let mut args = vec!["-c".to_string(), c.clone()];
            args.extend(cmd);
            if amixer(&args).is_ok() {
                ok += 1;
            }
        }
        if ok > 0 {
            SET_THIS_START.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key);
            let _ = std::fs::create_dir_all(markers);
            let _ = std::fs::write(&marker, format!("{LEVEL}\n"));
            lines.push(format!("sound: card {n} ({id}): microphone input set to {LEVEL} % and switched on for {ok} of {} capture control(s){}", controls.len(), if first { ", first time" } else { ", again after the start" }));
        }
    }
    lines
}

// ------------------------------------------------------------------------------------------------ playback

/// The playback level given once to a card's hardware volume, in percent (the IYAKKI volume, one level above, then sets the loudness).
pub const PLAY_LEVEL: u32 = 100;
/// The most a start gives the headphone output by itself: loud into the ears, and the person can raise it in Settings -> Sound.
pub const HEADPHONE_LEVEL: u32 = 80;
/// The output controls of a laptop codec that carry sound to the speakers or the headphones. `Mic`, `Line`, `CD`, `Beep` and the like are left alone:
/// switching their playback on would feed the microphone back into the speakers.
const OUTPUT_NAMES: [&str; 10] = ["Master", "Headphone", "Speaker", "PCM", "Front", "Line Out", "Surround", "Center", "LFE", "Side"];

/// The output controls (`pvolume` or `pswitch`) in the text of `amixer -c N scontents`.
pub fn playback_controls(scontents: &str) -> Vec<Control> {
    let mut out = Vec::new();
    let mut cur: Option<(String, u32)> = None;
    for line in scontents.lines() {
        if let Some(rest) = line.strip_prefix("Simple mixer control '") {
            cur = rest.rsplit_once("',").and_then(|(n, i)| Some((n.to_string(), i.trim().parse().ok()?)));
        } else if let (Some((name, index)), Some(caps)) = (&cur, line.trim().strip_prefix("Capabilities:")) {
            let has = |w: &str| caps.split_whitespace().any(|c| c == w);
            let (volume, switch) = (has("pvolume"), has("pswitch"));
            if (volume || switch) && OUTPUT_NAMES.contains(&name.as_str()) {
                out.push(Control { name: name.clone(), index: *index, volume, switch });
            }
        }
    }
    out
}

/// The `amixer` arguments that switch every output on (`unmute`), and, when `volume` is true, set it to [`PLAY_LEVEL`] first.
pub fn playback_commands(controls: &[Control], volume: bool) -> Vec<Vec<String>> {
    playback_commands_at(controls, volume.then_some(PLAY_LEVEL))
}

/// Like [`playback_commands`] with the level to set, or `None` to only switch the outputs on.
pub fn playback_commands_at(controls: &[Control], level: Option<u32>) -> Vec<Vec<String>> {
    controls
        .iter()
        .map(|c| {
            let mut a = vec!["-q".to_string(), "sset".to_string(), format!("{},{}", c.name, c.index)];
            if let (true, Some(level)) = (c.volume, level) {
                let level = if c.name == "Headphone" { level.min(HEADPHONE_LEVEL) } else { level };
                a.push(format!("{level}%"));
            }
            if c.switch {
                a.push("unmute".to_string());
            }
            a
        })
        .filter(|a| a.len() > 3)
        .collect()
}

static PLAYED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// One look at the outputs. A kernel starts many laptop codecs with `Master` muted (nothing restores the mixer state before IYAKKI starts), so
/// the radio plays at 100 % and nothing is heard, and the hardware volume is back at the kernel's default (often 0 %) because nothing saves it.
/// Each card that has output controls gets them **switched on and set to [`PLAY_LEVEL`] once per start**; the IYAKKI volume is the loudness control.
/// A marker `<id>.out` in `markers` only says whether this is the first time the card is seen (for the log line).
pub fn playback_step(proc_root: &Path, markers: &Path, amixer: &dyn Fn(&[String]) -> Result<String, String>) -> Vec<String> {
    let mut lines = Vec::new();
    let text = std::fs::read_to_string(proc_root.join("asound/cards")).unwrap_or_default();
    for (n, id) in cards(&text) {
        let key = format!("{}:{id}", markers.display());
        if PLAYED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).contains(&key) {
            continue;
        }
        let c = n.to_string();
        let Ok(out) = amixer(&["-c".into(), c.clone(), "scontents".into()]) else { continue };
        let controls = playback_controls(&out);
        if controls.is_empty() {
            continue; // the codec may register its controls a little later; the next look tries again (cards without any are cheap to ask)
        }
        let marker = markers.join(format!("{}.out", marker_name(&id)));
        let first = !marker.exists();
        let mut ok = 0;
        // the hardware level is not saved by anything (no alsactl state), so every start begins at the kernel's default, often 0 %: it goes to PLAY_LEVEL
        // each time, and the IYAKKI volume is what the person turns
        // a level the person chose in Settings is kept per card and comes back here
        let level = iyakki_devctl::saved_level(markers, &id).unwrap_or(PLAY_LEVEL);
        for cmd in playback_commands_at(&controls, Some(level)) {
            let mut args = vec!["-c".to_string(), c.clone()];
            args.extend(cmd);
            if amixer(&args).is_ok() {
                ok += 1;
            }
        }
        PLAYED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key);
        if first && ok > 0 {
            let _ = std::fs::create_dir_all(markers);
            let _ = std::fs::write(&marker, format!("{PLAY_LEVEL}\n"));
        }
        lines.push(format!("sound: card {n} ({id}): output switched on and set to {level} % for {ok} of {} control(s){}", controls.len(), if first { " (first time)" } else { "" }));
    }
    lines
}

/// The real machine: `/proc`, the data volume's marker folder, the real `amixer`.
pub fn run() {
    let amixer = |args: &[String]| -> Result<String, String> {
        let o = std::process::Command::new("amixer").args(args).output().map_err(|e| e.to_string())?;
        if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
    };
    for l in step(Path::new("/proc"), Path::new("/var/lib/iyakki/sound-init"), &amixer).into_iter().chain(playback_step(Path::new("/proc"), Path::new("/var/lib/iyakki/sound-init"), &amixer)) {
        log::info!("{l}");
    }
    // the capture formats, once per start: a card that offers S24_LE and S32_LE gets a capture PCM with the format named (micpcm.rs)
    static FORMATS_LEARNED: std::sync::Once = std::sync::Once::new();
    FORMATS_LEARNED.call_once(|| {
        let prober = crate::soundprobe::Prober::capture_system();
        let list = std::fs::read_to_string("/proc/asound/pcm").unwrap_or_default();
        let probe = |c: u32, d: u32| prober.constraints(c, d).map(|k| k.formats);
        for (c, d) in crate::micpcm::learn_cards(&list, &probe) {
            log::info!("microphone: card {c} device {d}: recordings go through {} (its format is named)", crate::micpcm::pcm_name(c, d));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SCONTENTS: &str = "Simple mixer control 'Master',0\n  Capabilities: pvolume pswitch\n  Playback channels: Front Left - Front Right\nSimple mixer control 'Capture',0\n  Capabilities: cvolume cswitch cswitch-joined\n  Capture channels: Front Left - Front Right\nSimple mixer control 'Internal Mic Boost',0\n  Capabilities: volume\nSimple mixer control 'Mic',0\n  Capabilities: pvolume cvolume pswitch cswitch\nSimple mixer control 'Mic Boost',0\n  Capabilities: cvolume\nSimple mixer control 'IYAKKI',0\n  Capabilities: pvolume\nSimple mixer control 'Digital',0\n  Capabilities: cswitch\n";

    #[test]
    fn only_capture_controls_are_found_and_boost_and_the_software_volume_are_left_alone() {
        let c = capture_controls(SCONTENTS);
        let names: Vec<&str> = c.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Capture", "Mic", "Digital"]);
        assert!(c[0].volume && c[0].switch && c[2].switch && !c[2].volume);
        let cmds = commands(&c);
        assert_eq!(cmds[0], ["-q", "sset", "Capture,0", "70%", "cap"]);
        assert_eq!(cmds[2], ["-q", "sset", "Digital,0", "cap"], "a switch alone is only switched on");
    }

    #[test]
    fn outputs_are_switched_on_and_the_microphone_playback_is_left_off() {
        let c = playback_controls(SCONTENTS);
        assert_eq!(c.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Master"], "Mic playback would feed back; IYAKKI is the software volume");
        assert_eq!(playback_commands(&c, true)[0], ["-q", "sset", "Master,0", "100%", "unmute"]);
        assert_eq!(playback_commands(&c, false)[0], ["-q", "sset", "Master,0", "unmute"], "after the first time only the switch");
    }

    #[test]
    fn playback_is_switched_on_once_per_start_and_the_level_only_the_first_time() {
        let d = std::env::temp_dir().join(format!("iyakki-playdefaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/asound")).unwrap();
        std::fs::write(d.join("proc/asound/cards"), " 1 [Generic        ]: HDA-Intel - HD-Audio Generic\n").unwrap();
        let calls = RefCell::new(Vec::<String>::new());
        let amixer = |a: &[String]| -> Result<String, String> {
            calls.borrow_mut().push(a.join(" "));
            Ok(if a.last().map(String::as_str) == Some("scontents") { SCONTENTS.to_string() } else { String::new() })
        };
        let markers = d.join("m");
        let l = playback_step(&d.join("proc"), &markers, &amixer);
        assert!(l.len() == 1 && l[0].contains("first time"), "{l:?}");
        assert!(calls.borrow().iter().any(|c| c == "-c 1 -q sset Master,0 100% unmute"), "{:?}", calls.borrow());
        assert!(markers.join("Generic.out").exists());
        // the same start does not touch it again
        calls.borrow_mut().clear();
        assert!(playback_step(&d.join("proc"), &markers, &amixer).is_empty() && calls.borrow().is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_level_the_person_chose_comes_back_at_the_next_start() {
        let d = std::env::temp_dir().join(format!("iyakki-playlevel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/asound")).unwrap();
        std::fs::write(d.join("proc/asound/cards"), " 3 [Chosen          ]: HDA-Intel - HD-Audio\n").unwrap();
        let markers = d.join("m");
        iyakki_devctl::remember_level(&markers, "Chosen", 60);
        let calls = RefCell::new(Vec::<String>::new());
        let amixer = |a: &[String]| -> Result<String, String> {
            calls.borrow_mut().push(a.join(" "));
            Ok(if a.last().map(String::as_str) == Some("scontents") { SCONTENTS.to_string() } else { String::new() })
        };
        let l = playback_step(&d.join("proc"), &markers, &amixer);
        assert!(l[0].contains("set to 60 %"), "{l:?}");
        assert!(calls.borrow().iter().any(|c| c == "-c 3 -q sset Master,0 60% unmute"), "{:?}", calls.borrow());
        assert_eq!(iyakki_devctl::card_id(&d.join("proc"), 3).as_deref(), Some("Chosen"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn cards_are_read_from_proc() {
        let t = " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n                      HDA NVidia at 0xc1000000 irq 85\n 1 [Generic        ]: HDA-Intel - HD-Audio Generic\n";
        assert_eq!(cards(t), vec![(0, "NVidia".to_string()), (1, "Generic".to_string())]);
    }

    #[test]
    fn a_card_is_set_up_once_and_a_later_choice_is_never_overwritten() {
        let d = std::env::temp_dir().join(format!("iyakki-micdefaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/asound")).unwrap();
        std::fs::write(d.join("proc/asound/cards"), " 1 [Generic        ]: HDA-Intel - HD-Audio Generic\n").unwrap();
        let calls = RefCell::new(Vec::<String>::new());
        let amixer = |a: &[String]| -> Result<String, String> {
            calls.borrow_mut().push(a.join(" "));
            Ok(if a.last().map(String::as_str) == Some("scontents") { SCONTENTS.to_string() } else { String::new() })
        };
        let markers = d.join("markers");
        let lines = step(&d.join("proc"), &markers, &amixer);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("card 1 (Generic)") && lines[0].contains("3 of 3"), "{}", lines[0]);
        // three capture controls at 70 %, and the two boost controls a little up (20 %)
        assert_eq!(calls.borrow().iter().filter(|c| c.contains("sset")).count(), 5);
        assert!(calls.borrow().iter().any(|c| c == "-c 1 -q sset Mic Boost,0 20%") && calls.borrow().iter().any(|c| c == "-c 1 -q sset Internal Mic Boost,0 20%"), "{:?}", calls.borrow());
        assert!(calls.borrow().iter().any(|c| c == "-c 1 -q sset Capture,0 70% cap"), "{:?}", calls.borrow());
        assert!(markers.join("Generic").exists());
        // the next look finds the marker and does nothing, so the person's own level stays
        calls.borrow_mut().clear();
        assert!(step(&d.join("proc"), &markers, &amixer).is_empty());
        assert!(calls.borrow().is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_card_without_capture_controls_is_not_marked_and_is_given_up_after_a_few_looks() {
        let d = std::env::temp_dir().join(format!("iyakki-micdefaults-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/asound")).unwrap();
        std::fs::write(d.join("proc/asound/cards"), " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n").unwrap();
        let calls = RefCell::new(0u32);
        let amixer = |_: &[String]| -> Result<String, String> {
            *calls.borrow_mut() += 1;
            Ok("Simple mixer control 'IEC958',0\n  Capabilities: pswitch\n".to_string())
        };
        for _ in 0..20 {
            assert!(step(&d.join("proc"), &d.join("m"), &amixer).is_empty());
        }
        assert_eq!(*calls.borrow(), GIVE_UP_AFTER, "asked a few times, then left alone");
        assert!(!d.join("m").exists(), "no marker");
        std::fs::remove_dir_all(&d).ok();
    }
}
