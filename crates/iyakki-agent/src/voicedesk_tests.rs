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

#[test]
fn the_record_command_is_aimed_at_the_chosen_microphone_unless_it_names_one() {
    let usb = Mic { id: "plughw:1,0".into(), label: "USB".into(), usb: true };
    assert_eq!(record_cmd_for("arecord -q -f S16_LE -r 16000 -c 1 -t raw", Some(&usb)), "arecord -D plughw:1,0 -q -f S16_LE -r 16000 -c 1 -t raw");
    assert_eq!(record_cmd_for("arecord -q -D hw:2,0 -t raw", Some(&usb)), "arecord -q -D hw:2,0 -t raw", "a device named by hand wins");
    assert_eq!(record_cmd_for("arecord -q", None), "arecord -q");
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
fn a_t2_macs_inputs_are_hidden_because_recording_there_can_hang_the_computer_unless_an_administrator_allows_it() {
    let cards = " 0 [Audio          ]: AppleT2x4 - Apple T2 Audio\n                      Apple T2 Audio\n 1 [PCH            ]: HDA-Intel - HDA Intel PCH\n 2 [Device         ]: USB-Audio - USB PnP Sound Device\n";
    let pcm = "00-00: Speaker : Speaker : playback 1\n00-01: Digital Mic : Digital Mic : capture 1\n00-04: Bridge Loopback : Bridge Loopback : playback 1 : capture 1\n02-00: USB Audio : USB Audio : playback 1 : capture 1\n";
    assert_eq!(t2_card_numbers(cards), vec![0]);
    let ids: Vec<String> = parse_mics(cards, pcm).into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["plughw:2,0"], "only the USB microphone");
    let all: Vec<String> = parse_mics_with(cards, pcm, true).into_iter().map(|m| m.id).collect();
    assert_eq!(all, ["plughw:0,1", "plughw:0,4", "plughw:2,0"]);
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
