use super::*;

const CARDS: &str = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7e10000 irq 33\n 1 [Device         ]: USB-Audio - USB PnP Sound Device\n                      C-Media USB PnP Sound Device at usb-0000:00:14.0-2, full speed\n";
const PCM: &str = "00-00: ALC3246 Analog : ALC3246 Analog : playback 1 : capture 1\n00-03: HDMI 0 : HDMI 0 : playback 1\n01-00: USB Audio : USB Audio : playback 1 : capture 1\n";

#[test]
fn microphones_are_the_devices_with_a_capture_stream_and_usb_ones_are_marked() {
    let m = parse_mics(CARDS, PCM);
    assert_eq!(m.len(), 2, "HDMI has no capture: {m:?}");
    assert_eq!((m[0].id.as_str(), m[0].usb), ("plughw:0,0", false));
    assert_eq!((m[1].id.as_str(), m[1].usb, m[1].label.as_str()), ("plughw:1,0", true, "USB Audio (USB)"));
    assert!(parse_mics("", "").is_empty());
    assert!(parse_mics(CARDS, "00-03: HDMI 0 : HDMI 0 : playback 1\n").is_empty());
}

/// The 2020 MacBook Pro (T2), as read from the machine: card 0 is `AppleT2x4` with the capture streams "Digital Mic" (device 1), "Codec Input"
/// (3) and "Bridge Loopback" (4); card 1 is the webcam's USB microphone. Dictation records through `plughw:C,D` of the first microphone (the
/// route the browser's microphone plugin uses too), never through `default` (which is the playback-only softvol device of IYAKKI's sound).
#[test]
fn the_t2_macbook_dictation_uses_plughw_of_the_digital_mic_and_never_the_loopback() {
    let cards = " 0 [AppleT2x4      ]: AppleT2x4 - Apple T2 Audio\n                      Apple T2 Audio\n 1 [Nova           ]: USB-Audio - EMEET SmartCam Nova 4K\n                      EMEET SmartCam Nova 4K at usb-0000:00:14.0-2, high speed\n 2 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x600c110000 irq 140\n";
    let pcm = "00-00: Speaker : Speaker : playback 1\n00-01: Digital Mic : Digital Mic : capture 1\n00-02: Headphones : Headphones : playback 1\n00-03: Codec Input : Codec Input : capture 1\n00-04: Bridge Loopback : Bridge Loopback : playback 1 : capture 1\n01-00: USB Audio : USB Audio : playback 1 : capture 1\n02-03: HDMI 0 : HDMI 0 : playback 1\n";
    // the capture switch of the driver is off: only the USB microphone
    let off = parse_mics_with(cards, pcm, false);
    assert_eq!(off.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["plughw:1,0"]);
    // on: the built-in ones, never the loopback
    let on = parse_mics_with(cards, pcm, true);
    assert_eq!(on.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["plughw:0,1", "plughw:0,3", "plughw:1,0"], "{on:?}");
    assert_eq!(record_cmd_for("arecord -q -f S16_LE -r 16000 -c 1 -t raw", on.first()), "arecord -D plughw:0,1 -q -f S16_LE -r 16000 -c 1 -t raw");
    // other machines: digital captures and loopback cards are no microphones either
    let pc = parse_mics(" 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n", "00-00: ALC3246 Analog : ALC3246 Analog : playback 1 : capture 1\n00-01: ALC3246 Digital : ALC3246 Digital (IEC958) : playback 1 : capture 1\n01-00: Loopback PCM : Loopback PCM : playback 8 : capture 8\n");
    assert_eq!(pc.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["plughw:0,0"]);
}

#[test]
fn the_record_command_is_aimed_at_the_chosen_microphone_unless_it_names_one() {
    let usb = Mic { id: "plughw:1,0".into(), label: "USB".into(), usb: true };
    assert_eq!(record_cmd_for("arecord -q -f S16_LE -r 16000 -c 1 -t raw", Some(&usb)), "arecord -D plughw:1,0 -q -f S16_LE -r 16000 -c 1 -t raw");
    assert_eq!(record_cmd_for("arecord -q -D hw:2,0 -t raw", Some(&usb)), "arecord -q -D hw:2,0 -t raw", "a device named by hand wins");
    assert_eq!(record_cmd_for("arecord -q", None), "arecord -q");
    // a custom recorder (a script, parecord, ffmpeg) is never given an ALSA option it would not understand
    for custom in ["sh -c 'cat x; sleep 1'", "parecord --raw", "/usr/bin/ffmpeg -f alsa -i default -"] {
        assert_eq!(record_cmd_for(custom, Some(&usb)), custom);
    }
    assert_eq!(record_cmd_for("/usr/bin/arecord -q", Some(&usb)), "/usr/bin/arecord -D plughw:1,0 -q");
}

#[test]
fn the_no_microphone_text_says_usb_works_and_names_the_unsupported_laptop() {
    let t = no_mic_text("Dell XPS");
    assert!(t.contains("USB microphones work") && t.contains("not supported yet") && t.contains("MacBookPro14,1"), "{t}");
    let t = no_mic_text("MacBookPro14,1");
    assert!(t.contains("USB microphones work") && t.contains("built-in microphone of this MacBookPro14,1 is not supported"), "{t}");
}

#[test]
fn the_language_is_guessed_from_the_letters_and_a_voice_that_cannot_read_it_is_flagged() {
    assert_eq!(detect_language("வணக்கம், இது ஒரு சோதனை வாக்கியம்.").map(|h| h.name), Some("Tamil"));
    assert_eq!(detect_language("यह एक परीक्षण वाक्य है और यह हिंदी में है").map(|h| h.tag), Some("hi-IN"));
    assert_eq!(detect_language("The quick brown fox jumps over the lazy dog.").map(|h| h.name), Some("English"));
    assert_eq!(detect_language("ok"), None, "too short to say");
    assert_eq!(detect_language("12345 67890 !!!"), None);
    let kokoro = ReadVoice { id: "k".into(), label: "Kokoro · af_heart".into(), source: "".into(), engine: SpeechEngine::Command { command: "x".into() }, voice: None, english_only: true };
    let w = language_warning("வணக்கம், இது ஒரு சோதனை வாக்கியம்.", Some(&kokoro)).unwrap();
    assert!(w.contains("Tamil") && w.contains("Kokoro reads English only"), "{w}");
    assert!(language_warning("Hello there, this is plain English text.", Some(&kokoro)).is_none());
    assert!(language_warning("வணக்கம், இது ஒரு சோதனை வாக்கியம்.", None).is_none());
}

#[test]
fn twenty_thousand_characters_become_short_pieces_that_cover_the_text_and_empty_ones_are_dropped() {
    let sentence = "This is one sentence of the long text, with a comma. ";
    let text = sentence.repeat(400); // about 21k: the box would cut it, the cutter handles whatever it is given
    let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
    let p = pieces(&text);
    assert!(p.len() >= 300, "{}", p.len());
    assert!(p.iter().all(|c| c.text.chars().count() <= story::MAX_CHUNK_CHARS && c.unit == 0));
    let chars: Vec<char> = text.chars().collect();
    for c in &p {
        let s: String = chars[c.start..c.end].iter().collect();
        assert_eq!(s, c.text, "a piece is where it says it is, so the lit sentence is the right one");
    }
    assert!(p.windows(2).all(|w| w[0].end <= w[1].start));
    // one very long sentence without stops is cut at spaces
    let long = "word ".repeat(300);
    let p = pieces(&long);
    assert!(p.len() >= 6 && p.iter().all(|c| c.text.chars().count() <= story::MAX_CHUNK_CHARS), "{}", p.len());
    // a bare address or marks have nothing to say
    assert!(pieces("https://example.org/a/b").is_empty() && pieces("*** ###").is_empty());
    assert!(pieces("").is_empty());
}

#[test]
fn the_built_in_t2_microphone_can_be_allowed_and_forbidden_again_and_a_pc_has_no_switch() {
    let d = std::env::temp_dir().join(format!("iyakki-t2sw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let proc = d.join("proc");
    std::fs::create_dir_all(proc.join("asound")).unwrap();
    std::fs::create_dir_all(d.join("sys/module/apple_bce/parameters")).unwrap();
    std::fs::write(proc.join("asound/cards"), " 0 [Audio          ]: AppleT2x1 - Apple T2 Audio\n 1 [PCH            ]: HDA-Intel - HDA Intel PCH\n").unwrap();
    std::fs::write(d.join("sys/module/apple_bce/parameters/capture"), "N\n").unwrap();
    assert_eq!(t2_mic(&proc), T2Mic::Off);
    set_t2_capture(&proc, true).unwrap();
    assert_eq!(t2_mic(&proc), T2Mic::On);
    assert_eq!(std::fs::read_to_string(d.join("sys/module/apple_bce/parameters/capture")).unwrap().trim(), "1");
    set_t2_capture(&proc, false).unwrap();
    assert_eq!(t2_mic(&proc), T2Mic::Off);
    // a PC: no T2 card, and no driver parameter to write
    std::fs::write(proc.join("asound/cards"), " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n").unwrap();
    assert_eq!(t2_mic(&proc), T2Mic::Absent);
    std::fs::remove_dir_all(d.join("sys/module")).unwrap();
    assert!(set_t2_capture(&proc, true).unwrap_err().contains("no switch"));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_t2_macs_inputs_are_hidden_because_recording_there_can_hang_the_computer_unless_an_administrator_allows_it() {
    let cards = " 0 [Audio          ]: AppleT2x4 - Apple T2 Audio\n                      Apple T2 Audio\n 1 [PCH            ]: HDA-Intel - HDA Intel PCH\n 2 [Device         ]: USB-Audio - USB PnP Sound Device\n";
    let pcm = "00-00: Speaker : Speaker : playback 1\n00-01: Digital Mic : Digital Mic : capture 1\n00-04: Bridge Loopback : Bridge Loopback : playback 1 : capture 1\n02-00: USB Audio : USB Audio : playback 1 : capture 1\n";
    assert_eq!(t2_card_numbers(cards), vec![0]);
    let ids: Vec<String> = parse_mics(cards, pcm).into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["plughw:2,0"], "only the USB microphone");
    let all: Vec<String> = parse_mics_with(cards, pcm, true).into_iter().map(|m| m.id).collect();
    assert_eq!(all, ["plughw:0,1", "plughw:2,0"], "the Bridge Loopback is no microphone");
    let d = std::env::temp_dir().join(format!("iyakki-t2cap-{}", std::process::id()));
    std::fs::create_dir_all(d.join("module/apple_bce/parameters")).unwrap();
    assert!(!t2_capture_allowed(&d));
    std::fs::write(d.join("module/apple_bce/parameters/capture"), "N\n").unwrap();
    assert!(!t2_capture_allowed(&d));
    std::fs::write(d.join("module/apple_bce/parameters/capture"), "Y\n").unwrap();
    assert!(t2_capture_allowed(&d));
    std::fs::remove_dir_all(&d).ok();
    let t = no_mic_text_t2("MacBookPro16,2", true);
    assert!(t.contains("microphone of this MacBookPro16,2 is not supported yet") && t.contains("USB microphone") && t.contains("apple_bce/parameters/capture"), "{t}");
    assert_eq!(no_mic_text_t2("Dell XPS", false), no_mic_text("Dell XPS"));
}

#[test]
fn the_chosen_microphone_wins_when_present_else_the_first_and_the_choice_is_handed_to_the_web_engine() {
    let m = |id: &str| Mic { id: id.into(), label: id.into(), usb: false };
    let list = vec![m("plughw:0,1"), m("plughw:1,0")];
    assert_eq!(pick_mic(&list, "plughw:1,0").unwrap().id, "plughw:1,0");
    assert_eq!(pick_mic(&list, "plughw:9,9").unwrap().id, "plughw:0,1", "a device that is gone: the first one");
    assert_eq!(pick_mic(&list, "").unwrap().id, "plughw:0,1");
    assert!(pick_mic(&[], "plughw:1,0").is_none());
    let d = std::env::temp_dir().join(format!("iyakki-micshare-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    share_mic_choice(&d, "plughw:1,0");
    assert_eq!(std::fs::read_to_string(d.join(BROWSER_MIC_CHOICE)).unwrap(), "plughw:1,0\n");
    share_mic_choice(&d, "");
    assert!(!d.join(BROWSER_MIC_CHOICE).exists(), "automatic removes the file");
    assert!(browser_mic_failure(&d).is_none());
    std::fs::write(d.join(BROWSER_MIC_STATUS), "Digital Mic\tDevice or resource busy\n").unwrap();
    assert_eq!(browser_mic_failure(&d), Some(("Digital Mic".into(), "Device or resource busy".into())));
    let t = mic_failure_text("Digital Mic", "Device or resource busy");
    assert!(t.contains("Digital Mic") && t.contains("another program is using it"), "{t}");
    assert!(mic_failure_text("X", "weird").contains("the sound system said: weird"));
    let mut v = crate::voice::VoiceSettings::default();
    v.mic = "plughw:1,0".into();
    assert!(v.validate().is_ok() && serde_json::from_str::<crate::voice::VoiceSettings>("{}").unwrap().mic.is_empty(), "old settings files have no mic");
    v.mic = "rm -rf /".into();
    assert!(v.validate().is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_pause_after_speech_ends_the_dictation_but_waiting_and_speaking_do_not() {
    let mut d = PauseDetector::new();
    // the person has not started yet: a quiet room for ten seconds never ends it
    for t in (0..10_000).step_by(100) {
        assert!(!d.tick(0.01, t), "waiting to start must not end it");
    }
    // speech with the little dips between words: never ends, however long
    let mut t = 10_000;
    for i in 0..300 {
        let level = if i % 7 == 0 { 0.02 } else { 0.5 };
        assert!(!d.tick(level, t), "talking must not end it (at {t})");
        t += 100;
    }
    // then quiet: ends after the pause, not before
    let start = t;
    let mut ended = None;
    while t < start + 5_000 {
        if d.tick(0.01, t) {
            ended = Some(t - start);
            break;
        }
        t += 100;
    }
    let ended = ended.expect("a pause ends it");
    assert!((PAUSE_END_MS - 200..PAUSE_END_MS + 300).contains(&ended), "ended after {ended} ms (counted from the last loud moment)");
}

#[test]
fn a_noisy_room_is_not_mistaken_for_speech_and_a_short_breath_does_not_end_it() {
    let mut d = PauseDetector::new();
    let mut t = 0;
    for _ in 0..50 {
        assert!(!d.tick(0.08, t)); // steady hum
        t += 100;
    }
    for _ in 0..10 {
        assert!(!d.tick(0.6, t)); // speech over the hum
        t += 100;
    }
    for _ in 0..8 {
        assert!(!d.tick(0.08, t), "a 0.8 s breath is not a pause");
        t += 100;
    }
    assert!(!d.tick(0.6, t)); // talking again resets it
    t += 100;
    let mut ended = false;
    for _ in 0..20 {
        ended |= d.tick(0.08, t);
        t += 100;
    }
    assert!(ended, "the hum alone after speech is a pause");
}

#[test]
fn the_dictation_pause_follows_the_settings_and_can_be_switched_off() {
    let mut v = crate::voice::VoiceSettings::default();
    assert!(v.auto_stop && v.pause_ms == 1500, "on, 1.5 s by default");
    assert!(PauseDetector::from_settings(&v).is_some());
    v.pause_ms = 3000;
    let mut d = PauseDetector::from_settings(&v).unwrap();
    assert!(!d.tick(0.01, 0));
    for t in (100..=600).step_by(100) {
        assert!(!d.tick(0.6, t)); // half a second of talking
    }
    assert!(!d.tick(0.01, 700));
    assert!(!d.tick(0.01, 3_400), "2.8 s of quiet is not yet 3 s");
    assert!(d.tick(0.01, 3_700));
    v.pause_ms = 100;
    assert!(v.validate().is_err(), "too short is refused");
    v.pause_ms = 1500;
    v.auto_stop = false;
    assert!(PauseDetector::from_settings(&v).is_none(), "off: nothing ends a dictation but the person");
    // old settings files have neither field: they load as on, 1.5 s
    let old: crate::voice::VoiceSettings = serde_json::from_str(r#"{"auto_send":true}"#).unwrap();
    assert!(old.auto_stop && old.pause_ms == 1500);
}

#[test]
fn clicks_coughs_and_other_short_noises_are_not_talking_and_do_not_keep_a_dictation_open() {
    // a quiet room with a bump now and then (0.1 s, loud): never counts as talking, so waiting is never ended and nothing is "heard"
    let mut d = PauseDetector::new();
    let mut t = 0;
    for i in 0..200 {
        let level = if i % 7 == 0 { 0.7 } else { 0.03 };
        assert!(!d.tick(level, t), "a bump is not talking (at {t})");
        t += 100;
    }
    // real talking, then the room goes back to bumps: the pause still ends it, the bumps do not hold it open
    for _ in 0..8 {
        assert!(!d.tick(0.6, t));
        t += 100;
    }
    let start = t;
    let mut ended = None;
    for i in 0..60 {
        let level = if i % 5 == 0 { 0.7 } else { 0.03 };
        if d.tick(level, t) {
            ended = Some(t - start);
            break;
        }
        t += 100;
    }
    let ended = ended.expect("bumps after the talking do not keep it listening");
    assert!(ended <= PAUSE_END_MS + 400, "ended after {ended} ms");
}

#[test]
fn a_room_that_is_loud_all_the_time_still_ends_after_talking_stops() {
    let mut d = PauseDetector::new();
    let mut t = 0;
    for _ in 0..30 {
        assert!(!d.tick(0.12, t)); // a steady fan
        t += 100;
    }
    for _ in 0..10 {
        assert!(!d.tick(0.7, t)); // talking over the fan
        t += 100;
    }
    let mut ended = false;
    for _ in 0..30 {
        ended |= d.tick(0.12, t);
        t += 100;
    }
    assert!(ended, "the fan alone is a pause");
}

/// MacBookPro14,1 (2026-11-08): its codec offers S24_LE and S32_LE for capture; through plughw the recorder got full-scale noise. The
/// learned card gets a capture PCM with the format named, and the record command is aimed at it instead of plughw.
#[test]
fn a_card_whose_capture_format_was_learned_records_through_its_named_pcm() {
    let args: Vec<String> = ["arecord", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw"].iter().map(|s| s.to_string()).collect();
    assert_eq!(record_cmd_for_device(&args, "iyakkimic0d0"), "arecord -D iyakkimic0d0 -q -f S16_LE -r 16000 -c 1 -t raw");
    assert_eq!(record_cmd_for_device(&args, "plughw:2,0"), "arecord -D plughw:2,0 -q -f S16_LE -r 16000 -c 1 -t raw");
    // the real path: a learned card 7 (a number no other test uses) maps its plughw id; an unlearned card keeps plughw
    assert!(crate::micpcm::learn(7, 0, &["S24_LE".to_string(), "S32_LE".to_string()]));
    let learned = Mic { id: "plughw:7,0".into(), label: "x".into(), usb: false };
    let other = Mic { id: "plughw:8,0".into(), label: "y".into(), usb: true };
    assert_eq!(record_cmd_for("arecord -q -t raw", Some(&learned)), "arecord -D iyakkimic7d0 -q -t raw");
    assert_eq!(record_cmd_for("arecord -q -t raw", Some(&other)), "arecord -D plughw:8,0 -q -t raw");
}
