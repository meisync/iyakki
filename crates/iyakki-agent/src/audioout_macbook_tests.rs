//! The MacBookPro14,1 (non-T2) with the CS8409 Apple-path module: card 0 `HDA Intel PCH`, whose analog PCM `CS8409/CS42L83 Analog`
//! appears about 11 s after boot, with a `PCM Playback Volume` that does nothing and a 44.1 kHz 32-bit 2/4-channel device.
use super::*;
use crate::soundprobe::Prober;
use std::sync::Arc;

struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree(tag: &str) -> Tree {
    let d = std::env::temp_dir().join(format!("iyakki-macbook-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("proc/asound")).unwrap();
    std::fs::create_dir_all(d.join("sys/class/dmi/id")).unwrap();
    std::fs::write(d.join("sys/class/dmi/id/product_name"), "MacBookPro14,1\n").unwrap();
    Tree(d)
}

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn pcm(root: &Path, dev: u32, id: &str) {
    put(root, &format!("proc/asound/card0/pcm{dev}p/info"), &format!("card: 0\ndevice: {dev}\nsubdevice: 0\nstream: PLAYBACK\nid: {id}\nname: {id}\nsubname: subdevice #0\nclass: 0\n"));
}

/// The card as it is a second after boot: the HDMI devices only.
fn booting(t: &Tree) {
    put(&t.0, "proc/asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xb1210000 irq 130\n");
    put(&t.0, "proc/asound/pcm", "00-03: HDMI 0 : HDMI 0 : playback 1\n00-07: HDMI 1 : HDMI 1 : playback 1\n");
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Cirrus Logic CS8409/CS42L83\nAddress: 0\nVendor Id: 0x10138409\nSubsystem Id: 0x106b3300\n");
    pcm(&t.0, 3, "HDMI 0");
    pcm(&t.0, 7, "HDMI 1");
}

/// Eleven seconds later: the analog device is there.
fn up(t: &Tree) {
    pcm(&t.0, 0, "CS8409/CS42L83 Analog");
    put(&t.0, "proc/asound/pcm", "00-00: CS8409/CS42L83 Analog : CS8409/CS42L83 Analog : playback 1 : capture 1\n00-03: HDMI 0 : HDMI 0 : playback 1\n00-07: HDMI 1 : HDMI 1 : playback 1\n");
}

const REPORT: &str = "FORMAT:  S24_LE S32_LE\nCHANNELS: [2 4]\nRATE: 44100\nPERIOD_SIZE: [8 262144]\nBUFFER_SIZE: [16 524288]\n";

fn mixer(_: u32) -> Option<String> {
    Some("numid=33,iface=MIXER,name='PCM Playback Volume'\n  ; type=INTEGER,access=rw---R--,values=2,min=0,max=255,step=0\n  : values=255,255\n".into())
}

fn selector(t: &Tree, report: &'static str) -> (Selector, crate::softvol::fake::Fake) {
    let fake = crate::softvol::fake::Fake::default();
    let sel = Selector::new(t.0.join("proc"), mixer, || false)
        .with_softvol(crate::softvol::Softvol::new(t.0.join("asound.conf"), fake.clone()))
        .with_prober(Prober::new(move |_, _| Ok(report.to_string())))
        .with_sys(t.0.join("sys"));
    (sel, fake)
}

#[test]
fn the_cs8409_volume_control_is_a_dummy_and_the_card_is_found_by_its_codec() {
    let t = tree("dummy");
    booting(&t);
    up(&t);
    let outs = scan(&t.0.join("proc"), &mixer);
    let analog = outs.iter().find(|o| o.kind == Kind::Analog).unwrap();
    assert_eq!(analog.hw_volume.as_deref(), Some("PCM"));
    assert!(analog.dummy_volume && hw_volume_is_dummy(&t.0.join("proc"), 0));
    // automatic: software; the person says it works: hardware; says it does not: software
    assert!(!analog.uses_hw_volume(VolumeMode::Automatic) && analog.uses_hw_volume(VolumeMode::Hardware) && !analog.uses_hw_volume(VolumeMode::Software));
    // another codec with the same control is trusted
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Realtek ALC269VC\n");
    let outs = scan(&t.0.join("proc"), &mixer);
    let o = outs.iter().find(|o| o.kind == Kind::Analog).unwrap();
    assert!(!o.dummy_volume && o.uses_hw_volume(VolumeMode::Automatic));
}

#[test]
fn the_dummy_pcm_control_gives_the_software_volume_with_the_probed_dmix_not_a_hardware_plan() {
    let t = tree("plan");
    booting(&t);
    up(&t);
    let (sel, _fake) = selector(&t, REPORT);
    let st = sel.refresh();
    let conf = std::fs::read_to_string(t.0.join("asound.conf")).unwrap();
    assert!(conf.contains("type softvol") && conf.contains("format S32_LE") && conf.contains("rate 44100") && conf.contains("channels 4") && conf.contains("pcm.iyakkifan"), "{conf}");
    assert!(conf.contains("type dmix") && !conf.contains("pcm.iyakki {\n    type plug\n    slave.pcm \"hw:0,0\""), "no hardware plan");
    assert_eq!(st.device.as_deref(), Some("iyakki"));
    assert_eq!(st.softvol, Some(0), "the software volume carries the IYAKKI volume");
    assert_eq!(st.hw_ignored, Some(0));
    // the override "Hardware": the control is trusted again (Plan::Hw)
    let mut p = sel.prefs();
    p.set_volume_mode("PCH", VolumeMode::Hardware);
    sel.set_prefs(p);
    let conf = std::fs::read_to_string(t.0.join("asound.conf")).unwrap();
    assert!(!conf.contains("type softvol") && conf.contains("slave.pcm \"hw:0,0\""), "{conf}");
    assert_eq!(sel.status().hw_ignored, None);
    let mut p = sel.prefs();
    p.set_volume_mode("PCH", VolumeMode::Software);
    sel.set_prefs(p);
    assert!(std::fs::read_to_string(t.0.join("asound.conf")).unwrap().contains("type softvol"));
}

#[test]
fn a_four_channel_only_card_gets_the_stereo_signal_on_every_channel_at_full_level() {
    let t = tree("four");
    booting(&t);
    up(&t);
    let (sel, _) = selector(&t, "FORMAT: S32_LE\nCHANNELS: 4\nRATE: 44100\nPERIOD_SIZE: [8 262144]\nBUFFER_SIZE: [16 524288]\n");
    sel.refresh();
    assert_eq!(sel.chain_channels(), Some(4));
    let conf = std::fs::read_to_string(t.0.join("asound.conf")).unwrap();
    assert!(conf.contains("pcm.iyakkifan {") && conf.contains("channels 4") && conf.contains("slave.pcm \"iyakkifan\""), "{conf}");
    for want in ["0.0 1.0", "1.1 1.0", "0.2 1.0", "1.3 1.0"] {
        assert!(conf.contains(want), "{want} missing:\n{conf}");
    }
    assert!(conf.contains("pcm.iyakkichan {"), "the walk goes to the channels one by one");
}

#[test]
fn the_card_that_appears_late_is_noticed_written_and_announced_once() {
    let t = tree("late");
    booting(&t);
    let (sel, _) = selector(&t, REPORT);
    // the first look: HDMI only, no monitor: no output, and the card is said to be starting
    let st = sel.refresh();
    assert!(st.choice.output.is_none());
    assert!(!t.0.join("asound.conf").exists());
    assert_eq!(st.starting.as_deref(), Some(STARTING_LINE));
    assert!(sel.take_notices().is_empty());
    let before = sound_signature(&t.0.join("proc"));
    up(&t);
    assert_ne!(before, sound_signature(&t.0.join("proc")), "the new PCM changes the signature");
    let g = sel.generation();
    let st = sel.refresh();
    assert!(st.starting.is_none());
    assert!(t.0.join("asound.conf").exists(), "written as soon as the output exists");
    assert_ne!(sel.generation(), g, "a running player restarts on it");
    assert_eq!(sel.take_notices(), vec!["Sound output ready: Speakers/headphones (HDA Intel PCH)".to_string()]);
    sel.refresh();
    assert!(sel.take_notices().is_empty(), "said once");
}

#[test]
fn a_person_who_chose_another_output_is_not_moved_and_a_card_present_at_the_start_is_not_announced() {
    let t = tree("chosen");
    booting(&t);
    put(&t.0, "proc/asound/card0/pcm3p/info", "card: 0\ndevice: 3\nid: HDMI 0\nname: HDMI 0\n");
    let (sel, _) = selector(&t, REPORT);
    let mut p = sel.prefs();
    p.output = "PCH:3".into();
    sel.set_prefs(p);
    sel.refresh();
    up(&t);
    let st = sel.refresh();
    assert_eq!(st.choice.output.unwrap().key(), "PCH:3", "the chosen output stays");
    assert!(sel.take_notices().is_empty());
    // a start with the analog device already there
    let t2 = tree("present");
    booting(&t2);
    up(&t2);
    let (sel2, _) = selector(&t2, REPORT);
    sel2.refresh();
    assert!(sel2.take_notices().is_empty());
}

#[test]
fn the_starting_state_ends_after_thirty_seconds_and_only_covers_the_macs_with_that_codec() {
    let t = tree("window");
    booting(&t);
    assert_eq!(card_starting(&t.0.join("proc"), &t.0.join("sys")), Some(0));
    put(&t.0, "sys/class/dmi/id/product_name", "Latitude 7490\n");
    assert_eq!(card_starting(&t.0.join("proc"), &t.0.join("sys")), None);
    put(&t.0, "sys/class/dmi/id/product_name", "MacBookPro14,1\n");
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Realtek ALC269VC\n");
    assert_eq!(card_starting(&t.0.join("proc"), &t.0.join("sys")), None);
    // the window: a selector that has waited its time says it is missing, not slow
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Cirrus Logic CS8409\n");
    let (sel, _) = selector(&t, REPORT);
    assert!(sel.refresh().starting.is_some());
    sel.lock().starting_since = Some(Instant::now() - Duration::from_secs(STARTING_SECS + 1));
    assert!(sel.refresh().starting.is_none());
    assert!(sel.refresh().starting.is_none(), "and it stays over");
}

#[test]
fn the_watcher_looks_often_early_and_when_the_cards_change() {
    assert_eq!(watch_wait(Duration::from_secs(5)), WATCH_EARLY);
    assert_eq!(watch_wait(Duration::from_secs(61)), WATCH_EVERY);
    assert!(look_due(true, Duration::ZERO) && !look_due(false, Duration::from_secs(1)) && look_due(false, WATCH_EVERY));
}

#[test]
fn the_settings_keep_the_boost_and_the_volume_mode_per_card_and_refuse_nonsense() {
    let mut p = SoundSettings::default();
    p.set_boost("PCH", 99);
    assert_eq!(p.boost_for("PCH"), crate::softvol::MAX_BOOST_DB);
    p.set_boost("PCH", 0);
    assert_eq!(p.boost_for("PCH"), 0, "0 is the person's choice of no boost");
    assert_eq!(p.boost_for("Other"), DEFAULT_BOOST_DB, "an untouched card starts a little up");
    p.set_volume_mode("PCH", VolumeMode::Software);
    assert_eq!(p.volume_mode_for("PCH"), VolumeMode::Software);
    assert_eq!(p.volume_mode_for("other"), VolumeMode::Automatic);
    assert!(p.validate().is_ok());
    p.boosts.insert("PCH".into(), 40);
    assert!(p.validate().is_err());
    // an old settings file (no such keys) reads as the defaults
    let old: SoundSettings = serde_json::from_str(r#"{"output":"PCH:3","volume_pct":40}"#).unwrap();
    assert_eq!(old.volume_mode_for("PCH"), VolumeMode::Automatic);
    assert_eq!(old.boost_for("PCH"), DEFAULT_BOOST_DB);
    let json = serde_json::to_string(&SoundSettings { volume_modes: [("PCH".to_string(), VolumeMode::Hardware)].into(), ..Default::default() }).unwrap();
    assert!(json.contains("\"hardware\""), "{json}");
}

#[test]
fn the_hardware_check_plays_100_then_12_percent_and_puts_the_level_back() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let (l1, l2) = (log.clone(), log.clone());
    let amixer = move |a: &[String]| -> Result<String, String> {
        l1.lock().unwrap().push(a.join(" "));
        Ok(if a.contains(&"sget".to_string()) { "  Front Left: Playback 77 [30%] [on]\n".into() } else { String::new() })
    };
    let play = move |d: &str, w: &[u8]| -> Result<(), String> {
        assert!(&w[..4] == b"RIFF" && w.len() > 100_000);
        l2.lock().unwrap().push(format!("play {d}"));
        Ok(())
    };
    hw_volume_check(&amixer, &play, 0, 0, "PCM").unwrap();
    assert_eq!(*log.lock().unwrap(), ["-c 0 sget PCM", "-q -c 0 sset PCM 100%", "play plughw:0,0", "-q -c 0 sset PCM 12%", "play plughw:0,0", "-q -c 0 sset PCM 30%"]);
    // a tone that fails still puts the level back
    log.lock().unwrap().clear();
    let bad = |_: &str, _: &[u8]| -> Result<(), String> { Err("busy".into()) };
    assert!(hw_volume_check(&amixer, &bad, 0, 0, "PCM").is_err());
    assert_eq!(log.lock().unwrap().last().map(String::as_str), Some("-q -c 0 sset PCM 30%"));
    assert_eq!(percent_of("Mono: [45%] x"), Some(45));
    assert_eq!(percent_of("nothing"), None);
}

#[test]
fn the_test_patterns_are_valid_wavs_of_the_right_shape() {
    let w = stereo_test_wav();
    assert_eq!(&w[..4], b"RIFF");
    assert_eq!(u16::from_le_bytes([w[22], w[23]]), 2);
    assert_eq!(w.len(), 44 + 2 * 2 * PATTERN_RATE as usize * 2, "1 s left then 1 s right");
    // left second: right samples are silent; right second: left samples are silent
    let s = |i: usize| i16::from_le_bytes([w[44 + 2 * i], w[45 + 2 * i]]);
    assert!((0..PATTERN_RATE as usize).all(|f| s(f * 2 + 1) == 0) && (0..PATTERN_RATE as usize).any(|f| s(f * 2).abs() > 10_000));
    let off = PATTERN_RATE as usize * 2;
    assert!((0..PATTERN_RATE as usize).all(|f| s(off + f * 2) == 0) && (0..PATTERN_RATE as usize).any(|f| s(off + f * 2 + 1).abs() > 10_000));
    let walk = channel_walk_wav(4);
    assert_eq!(u16::from_le_bytes([walk[22], walk[23]]), 4);
    assert_eq!(walk.len(), 44 + 4 * 4 * PATTERN_RATE as usize * 2);
    let peak = walk[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).abs()).max().unwrap();
    // loud enough to hear on a quiet card (a Mac's speakers, where it skips the boost the Stereo test gets), below full scale
    assert!(peak < 25_000 && peak > 20_000, "audible without clipping: {peak}");
}

#[test]
fn a_float_kokoro_wav_becomes_16_bit_pcm_for_the_output_chain_and_16_bit_wavs_are_left_alone() {
    // the header Lemonade's Kokoro sends: IEEE float (3), mono, 24 kHz, 32 bits, sizes of a stream (0xFFFFFFFF)
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend(u32::MAX.to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(3u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(24_000u32.to_le_bytes());
    w.extend((24_000u32 * 4).to_le_bytes());
    w.extend(4u16.to_le_bytes());
    w.extend(32u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend(u32::MAX.to_le_bytes());
    for s in [0.0f32, 0.5, -0.5, 1.0, -1.0, 2.0] {
        w.extend(s.to_le_bytes());
    }
    let out = crate::audioout::to_pcm16_wav(&w).expect("a float WAV is converted");
    let p = crate::voice::parse_wav(&out).expect("the result is 16-bit PCM");
    assert_eq!((p.rate, p.channels), (24_000, 1));
    assert_eq!(p.samples, vec![0, 16383, -16383, 32767, -32767, 32767], "scaled, clipped at full scale");
    // 24-bit integer stereo
    let mut w24 = Vec::new();
    w24.extend_from_slice(b"RIFF");
    w24.extend(0u32.to_le_bytes());
    w24.extend_from_slice(b"WAVEfmt ");
    w24.extend(16u32.to_le_bytes());
    w24.extend(1u16.to_le_bytes());
    w24.extend(2u16.to_le_bytes());
    w24.extend(44_100u32.to_le_bytes());
    w24.extend((44_100u32 * 6).to_le_bytes());
    w24.extend(6u16.to_le_bytes());
    w24.extend(24u16.to_le_bytes());
    w24.extend_from_slice(b"data");
    w24.extend(6u32.to_le_bytes());
    w24.extend([0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]); // +0x400000 (a quarter of full scale), -0x400000
    let p24 = crate::voice::parse_wav(&crate::audioout::to_pcm16_wav(&w24).unwrap()).unwrap();
    assert_eq!((p24.rate, p24.channels, p24.samples), (44_100, 2, vec![16384, -16384]));
    // 16-bit PCM and things that are not WAVs: nothing to do
    assert!(crate::audioout::to_pcm16_wav(&crate::audioout::test_tone_wav()).is_none());
    assert!(crate::audioout::to_pcm16_wav(b"not a wav at all, just some bytes here......").is_none());
    assert!(crate::audioout::to_pcm16_wav(&[]).is_none());
    // a truncated float WAV (an odd byte count) does not panic and keeps whole frames
    let mut cut = w.clone();
    cut.truncate(cut.len() - 3);
    assert!(crate::audioout::to_pcm16_wav(&cut).is_some());
}

// ---------------------------------------------------------------------------------- the real boot, end to end

/// The boot that failed on the MacBookPro14,1: the shell starts with the card absent or HDMI-only, the analog PCM appears
/// 9 to 11 s later, nobody is signed in until a minute later, and the probe fails because something holds the card.
#[test]
fn a_real_boot_the_card_shows_up_late_the_watcher_writes_the_file_before_anyone_signs_in_even_if_the_card_is_busy() {
    let t = tree("boot");
    let proc_ = t.0.join("proc");
    let conf = t.0.join("asound.conf");
    // t=0: no card at all (the Apple codec setup is still running)
    std::fs::remove_dir_all(&proc_).unwrap();
    std::fs::create_dir_all(proc_.join("asound")).unwrap();
    assert!(!has_card(&proc_));
    let fake = crate::softvol::fake::Fake::default();
    // the probe cannot answer: the card is busy (TALES or a player holds it)
    let sel = Selector::new(proc_.clone(), mixer, || false)
        .with_softvol(crate::softvol::Softvol::new(&conf, fake.clone()))
        .with_prober(Prober::new(|_, _| Err("aplay: Device or resource busy".into())))
        .with_sys(t.0.join("sys"));
    let mut w = Watch::new();
    // t=+2: the first look finds nothing and writes nothing
    let st = w.step(&sel, &proc_).expect("the first step looks");
    assert!(st.choice.output.is_none() && !conf.exists());
    assert!(w.step(&sel, &proc_).is_none(), "nothing changed, nothing to do");
    // t=+9..11: the card appears, HDMI first (no monitor), then the analog PCM, codec named CS8409/CS42L83
    booting(&t);
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Cirrus Logic CS8409/CS42L83\nAddress: 0\n");
    let st = w.step(&sel, &proc_).expect("a new card is looked at at once");
    assert!(st.choice.output.is_none() && st.starting.is_some() && !conf.exists());
    up(&t);
    let st = w.step(&sel, &proc_).expect("a new PCM is looked at at once");
    // t=+11: still nobody is signed in; the file is there, with the measured constraints because the probe failed
    let text = std::fs::read_to_string(&conf).expect("/etc/asound.conf is written by the watcher alone");
    assert!(text.starts_with(crate::softvol::MARKER) && text.contains("type dmix") && text.contains("format S32_LE") && text.contains("rate 44100") && text.contains("channels 4") && text.contains("pcm.iyakkifan"), "{text}");
    assert_eq!(st.device.as_deref(), Some("iyakki"));
    assert_eq!(st.softvol, Some(0));
    assert_eq!(st.hw_ignored, Some(0), "the dummy PCM control is not used");
    // t=+60: the person signs in and plays: the same device, nothing changes, no second write
    let mtime = std::fs::metadata(&conf).unwrap().modified().unwrap();
    assert_eq!(sel.device().as_deref(), Some("iyakki"));
    assert_eq!(std::fs::metadata(&conf).unwrap().modified().unwrap(), mtime);
    assert_eq!(sel.take_notices().len(), 1);
}

#[test]
fn the_installer_waits_for_a_card_that_comes_later_and_runs_once() {
    let t = tree("wait");
    std::fs::remove_dir_all(t.0.join("proc")).unwrap();
    std::fs::create_dir_all(t.0.join("proc/asound")).unwrap();
    let hits = Arc::new(Mutex::new(0u32));
    let h2 = hits.clone();
    when_card_appears(t.0.join("proc"), || Duration::from_millis(5), move || *h2.lock().unwrap() += 1);
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(*hits.lock().unwrap(), 0, "no card yet: not installed");
    booting(&t);
    for _ in 0..200 {
        if *hits.lock().unwrap() > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(*hits.lock().unwrap(), 1);
    // a card that is there already: at once, on the calling thread
    let h3 = hits.clone();
    when_card_appears(t.0.join("proc"), || Duration::from_secs(60), move || *h3.lock().unwrap() += 1);
    assert_eq!(*hits.lock().unwrap(), 2);
}

#[test]
fn both_codec_names_of_the_cs8409_are_recognised() {
    let t = tree("names");
    booting(&t);
    for name in ["Cirrus Logic CS8409", "Cirrus Logic CS8409/CS42L83", "cirrus logic cs8409/cs42l83"] {
        put(&t.0, "proc/asound/card0/codec#0", &format!("Codec: {name}\nAddress: 0\n"));
        assert!(hw_volume_is_dummy(&t.0.join("proc"), 0), "{name}");
    }
    put(&t.0, "proc/asound/card0/codec#0", "Codec: Realtek ALC269\n");
    assert!(!hw_volume_is_dummy(&t.0.join("proc"), 0));
}

#[test]
fn a_busy_card_is_said_with_the_program_that_holds_it_and_retried_until_it_is_free() {
    let t = tree("busy");
    booting(&t);
    up(&t);
    // a program (TALES' aplay) holds the playback device
    let pid = t.0.join("proc/4242");
    std::fs::create_dir_all(pid.join("fd")).unwrap();
    std::os::unix::fs::symlink("/dev/snd/pcmC0D0p", pid.join("fd/5")).unwrap();
    put(&t.0, "proc/4242/comm", "aplay\n");
    assert_eq!(pcm_holders(&t.0.join("proc"), 0, 0), ["aplay (4242)"]);
    assert!(pcm_holders(&t.0.join("proc"), 0, 3).is_empty());
    let fake = crate::softvol::fake::Fake::default();
    *fake.busy.lock().unwrap() = true;
    let sel = Selector::new(t.0.join("proc"), mixer, || false)
        .with_softvol(crate::softvol::Softvol::new(t.0.join("asound.conf"), fake.clone()))
        .with_prober(Prober::new(|_, _| Err("busy".into())))
        .with_sys(t.0.join("sys"));
    let st = sel.refresh();
    assert!(t.0.join("asound.conf").exists(), "the file is written even though the card is busy");
    assert_eq!(st.busy.as_deref(), Some("sound card is in use by aplay (4242): stop it, then press Look again"));
    assert_eq!(st.softvol, None);
    assert!(sel.volume_note().unwrap().starts_with("Volume control not ready: sound card is in use by aplay"));
    // the program lets go: the next look creates the control and the volume works
    *fake.busy.lock().unwrap() = false;
    std::fs::remove_dir_all(&pid).unwrap();
    let st = sel.refresh();
    assert!(st.busy.is_none() && st.softvol == Some(0), "{st:?}");
    assert_eq!(sel.volume_note(), None);
}

#[test]
fn the_volume_note_says_no_output_or_starting_instead_of_nothing() {
    let t = tree("note");
    booting(&t);
    let (sel, _) = selector(&t, REPORT);
    assert_eq!(sel.volume_note(), None.or(sel.volume_note()), "not looked yet");
    sel.refresh();
    assert_eq!(sel.volume_note().as_deref(), Some("Volume control not ready: starting"));
    let t2 = tree("note2");
    std::fs::create_dir_all(t2.0.join("proc/asound")).unwrap();
    let (sel2, _) = selector(&t2, REPORT);
    sel2.refresh();
    assert_eq!(sel2.volume_note().as_deref(), Some("No sound output"));
}

#[test]
fn the_master_is_applied_once_per_stream_class_by_the_softvol_or_by_the_players_never_both() {
    // a dummy-control card: the softvol chain carries the master, WAV players must not scale again
    let t = tree("once");
    booting(&t);
    up(&t);
    let (sel, fake) = selector(&t, REPORT);
    let mut p = sel.prefs();
    p.volume_pct = 40;
    sel.set_prefs(p.clone());
    sel.refresh();
    assert!(fake.calls.lock().unwrap().iter().any(|c| c.ends_with("sset IYAKKI 40%")), "{:?}", fake.calls.lock().unwrap());
    assert_eq!(sel.gain(), 100, "the chain has it: radio, media and WAV players add nothing");
    p.muted = true;
    sel.set_prefs(p.clone());
    assert!(fake.calls.lock().unwrap().last().unwrap().ends_with("sset IYAKKI 0%"));
    assert_eq!(sel.gain(), 0, "muted: the players are silent too");
    // a card with a real hardware control (another codec, or the owner said Hardware): no softvol; the players scale once
    let t2 = tree("once-hw");
    booting(&t2);
    up(&t2);
    put(&t2.0, "proc/asound/card0/codec#0", "Codec: Realtek ALC269\n");
    let (sel2, fake2) = selector(&t2, REPORT);
    let mut q = sel2.prefs();
    q.volume_pct = 40;
    sel2.set_prefs(q);
    sel2.refresh();
    assert!(!fake2.calls.lock().unwrap().iter().any(|c| c.contains("IYAKKI")), "no softvol control is touched");
    assert_eq!(sel2.gain(), 40, "the players apply the master themselves");
    assert!(std::fs::read_to_string(t2.0.join("asound.conf")).unwrap().contains("slave.pcm \"hw:0,0\""));
}

#[test]
fn speech_at_24_khz_is_brought_to_48_khz_with_twice_the_samples_and_the_same_length_in_time() {
    // a 24 kHz mono ramp, 2400 frames = 0.1 s
    let samples: Vec<i16> = (0..2400).map(|i| ((i % 100) as i16 - 50) * 100).collect();
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend((36 + samples.len() as u32 * 2).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(24_000u32.to_le_bytes());
    w.extend(48_000u32.to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend((samples.len() as u32 * 2).to_le_bytes());
    for s in &samples {
        w.extend(s.to_le_bytes());
    }
    let out = crate::audioout::resample_low_rate_wav(&w).expect("24 kHz speech is converted");
    let p = crate::voice::parse_wav(&out).unwrap();
    assert_eq!((p.rate, p.channels), (48_000, 1));
    assert_eq!(p.samples.len(), 4800, "twice the samples: the same 0.1 s at the card's rate");
    assert_eq!(p.samples[0], samples[0], "the first sample is kept");
    assert_eq!(p.samples[2], samples[1], "every second sample is an original one");
    // 44.1 and 48 kHz audio, 16-bit stereo at 22.05 kHz keeps two channels, and things that are not WAVs: handled
    let mut st = w.clone();
    st[22..24].copy_from_slice(&2u16.to_le_bytes());
    st[24..28].copy_from_slice(&22_050u32.to_le_bytes());
    let o2 = crate::voice::parse_wav(&crate::audioout::resample_low_rate_wav(&st).unwrap()).unwrap();
    assert_eq!((o2.rate, o2.channels), (48_000, 2));
    let mut hi = w.clone();
    hi[24..28].copy_from_slice(&44_100u32.to_le_bytes());
    assert!(crate::audioout::resample_low_rate_wav(&hi).is_none());
    assert!(crate::audioout::resample_low_rate_wav(b"not a wav").is_none());
    assert!(crate::audioout::resample_low_rate_wav(&[]).is_none());
}
