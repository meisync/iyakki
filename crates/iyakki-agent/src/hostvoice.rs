//! Browser voice: reading aloud with the speech synthesis of the browser (or desktop window) the person is using.
//!
//! Nothing is spoken on this machine and no audio is made here. The text of the piece being read goes to the page over the
//! front end's own connection (`docs/REMOTE-PROTOCOL.md`), the page calls `speechSynthesis.speak`, and the page reports back
//! when it started, ended, failed or passed a word. This module is the meeting point of the two sides and has no I/O of
//! its own:
//!
//! * [`HostVoice`] is one person's bridge, kept per shell thread ([`HostVoice::current`]). The front end drains what is to be
//!   sent ([`HostVoice::take_outgoing`]) and feeds what came back ([`HostVoice::event`]); the STORY reader speaks through
//!   [`BrowserSynth`] and [`BrowserSink`], which plug into the same reader the sound engines use.
//! * [`ToClient`] and [`FromClient`] are the wire messages (JSON, one per WebSocket text frame) with their encoders and
//!   decoders. Everything the page sends is checked and size-limited: it is the person's own session, but the server never
//!   trusts a page's text blindly.
//!
//! Only the text of the story pieces that are being read is ever sent, never anything else of the person's.

pub use iyakki_shared::hostvoice::*;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::time::{Duration, Instant};

/// The page side of the browser voice (speechSynthesis), shared by the web front end's page and the voice helper page.
pub const SPEECH_JS: &str = include_str!("speech.js");

// ------------------------------------------------------------------------------------------------------------ the reader's plug-ins

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{Chunk, Playing, Sink, Status, Synth};
    use crate::storykit::Reader;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn chunk(i: usize, text: &str) -> Chunk {
        Chunk { unit: i, start: 0, end: text.chars().count(), text: text.into() }
    }

    fn pieces() -> Vec<Chunk> {
        ["The fox woke early.", "She looked at the river.", "The moon was hiding.", "She went home."].iter().enumerate().map(|(i, t)| chunk(i, t)).collect()
    }

    /// A page that is only a thread: it takes what the server sends, "speaks" each piece for `talk`, and answers the way a
    /// browser would. `fail_on` makes it report an error for the piece with that text; `silent` makes it never start.
    struct FakePage {
        hv: Arc<HostVoice>,
        log: Arc<Mutex<Vec<ToClient>>>,
        stop: Arc<AtomicBool>,
    }

    impl FakePage {
        fn start(hv: &Arc<HostVoice>, talk: Duration, fail_on: Option<&'static str>, silent: bool) -> FakePage {
            hv.connect(Front::Browser);
            let log = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (h, l, s) = (hv.clone(), log.clone(), stop.clone());
            std::thread::spawn(move || {
                // the page answers the voice list request like the real one
                let mut current: Option<(u64, Instant, bool)> = None;
                while !s.load(Ordering::Relaxed) {
                    for m in h.take_outgoing() {
                        // the wire format is what travels: every message goes through its text form
                        let m = ToClient::decode(&m.encode()).expect("decodes");
                        l.lock().unwrap().push(m.clone());
                        match m {
                            ToClient::ListVoices => h.event(FromClient::decode(&FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "Test Voice".into(), lang: "en-US".into(), local: true }] }.encode()).unwrap()),
                            ToClient::Speak { id, text, .. } => {
                                if silent {
                                    continue;
                                }
                                h.event(FromClient::Started { id });
                                if fail_on.is_some_and(|f| text.contains(f)) {
                                    h.event(FromClient::Error { id, message: "synthesis-failed".into() });
                                } else {
                                    current = Some((id, Instant::now(), false));
                                }
                            }
                            ToClient::Cancel { id } => {
                                if id.is_none() || current.is_some_and(|c| Some(c.0) == id) {
                                    current = None;
                                }
                            }
                            ToClient::Pause => {}
                            ToClient::Resume => {}
                        }
                    }
                    if let Some((id, t0, _)) = current {
                        if t0.elapsed() >= talk {
                            h.event(FromClient::Boundary { id, char_index: 3 });
                            h.event(FromClient::Ended { id });
                            current = None;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            });
            FakePage { hv: hv.clone(), log, stop }
        }

        fn spoken(&self) -> Vec<String> {
            self.log.lock().unwrap().iter().filter_map(|m| if let ToClient::Speak { text, .. } = m { Some(text.clone()) } else { None }).collect()
        }

        fn cancels(&self) -> usize {
            self.log.lock().unwrap().iter().filter(|m| matches!(m, ToClient::Cancel { .. })).count()
        }
    }

    impl Drop for FakePage {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = &self.hv;
        }
    }

    fn sink(hv: &Arc<HostVoice>) -> Arc<dyn Sink> {
        Arc::new(BrowserSink { hv: hv.clone(), voice: Some("Test Voice".into()), lang: Some("en-US".into()), rate: 1.0 })
    }

    /// Wait for a state. The limit is wall time that only matters when something is wrong (a loaded machine is slow, not wrong).
    fn wait(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out: {what}");
    }

    /// A piece that the page never finishes by itself (the test ends it by pausing, seeking, stopping or closing the page).
    const NEVER: Duration = Duration::from_secs(3600);

    #[test]
    fn boundary_events_become_the_word_number_counting_utf16_like_the_browser() {
        let hv = Arc::new(HostVoice::new());
        hv.connect(Front::Browser);
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        let mut p = sink(&hv).play(&[MAGIC, "😀 Hello big world".as_bytes()].concat()).expect("plays");
        assert_eq!(p.spoken_word(), None, "no boundary event yet: not an error, just unknown");
        let id = hv.take_outgoing().iter().find_map(|m| if let ToClient::Speak { id, .. } = m { Some(*id) } else { None }).expect("sent");
        // the browser counts the emoji as two units: "big" starts at unit 9 (😀=2, space, Hello=5, space)
        hv.event(FromClient::Boundary { id, char_index: 9 });
        assert_eq!(p.spoken_word(), Some((1, 3)), "Hello is word 0 (the emoji is no word), big is word 1... of 3");
        hv.event(FromClient::Boundary { id, char_index: 13 });
        assert_eq!(p.spoken_word(), Some((2, 3)));
        // an event pointing past the text names no word
        hv.event(FromClient::Boundary { id, char_index: 999 });
        assert_eq!(p.spoken_word(), None);
        drop(p);
    }

    #[test]
    fn every_message_survives_its_wire_form_and_bad_ones_are_dropped() {
        let to = [
            ToClient::Speak { id: 7, text: "Hello, \"fox\" — நன்றி".into(), voice: Some("Google US English".into()), lang: Some("en-US".into()), rate: 1.25 },
            ToClient::Speak { id: 8, text: "x".into(), voice: None, lang: None, rate: 1.0 },
            ToClient::Cancel { id: Some(7) },
            ToClient::Cancel { id: None },
            ToClient::Pause,
            ToClient::Resume,
            ToClient::ListVoices,
        ];
        for m in to {
            assert_eq!(ToClient::decode(&m.encode()), Some(m.clone()), "{m:?}");
        }
        let from = [
            FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "Lekha".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "Samantha".into(), lang: "en-US".into(), local: false }] },
            FromClient::Voices { supported: false, voices: vec![] },
            FromClient::Started { id: 1 },
            FromClient::Ended { id: 2 },
            FromClient::Error { id: 3, message: "not-allowed".into() },
            FromClient::Boundary { id: 4, char_index: 12 },
        ];
        for m in from {
            assert_eq!(FromClient::decode(&m.encode()), Some(m.clone()), "{m:?}");
        }
        for bad in ["", "nope", "{}", r#"{"t":"sp_end"}"#, r#"{"t":"sp_end","id":-1}"#, r#"{"t":"sp_start","id":"x"}"#, r#"{"t":"unknown","id":1}"#, r#"{"t":"speak","text":"no id"}"#] {
            assert!(FromClient::decode(bad).is_none(), "{bad}");
        }
        assert!(ToClient::decode(r#"{"t":"speak","text":"no id"}"#).is_none());
        // the page's lists are bounded and cleaned
        let many: Vec<String> = (0..1000).map(|i| format!(r#"{{"n":"v{i}\u0007","l":"en"}}"#)).collect();
        let FromClient::Voices { voices, .. } = FromClient::decode(&format!(r#"{{"t":"sp_voices","ok":true,"voices":[{}]}}"#, many.join(","))).unwrap() else { panic!() };
        assert_eq!(voices.len(), MAX_VOICES);
        assert_eq!(voices[0].name, "v0", "control characters are removed");
        let FromClient::Error { message, .. } = FromClient::decode(&format!(r#"{{"t":"sp_error","id":1,"e":"{}"}}"#, "e".repeat(5000))).unwrap() else { panic!() };
        assert_eq!(message.len(), 200);
    }

    #[test]
    fn the_voice_is_not_available_until_the_page_says_it_can_speak() {
        let hv = HostVoice::new();
        assert_eq!(hv.support(), Support::Unknown);
        assert!(hv.speak("hello", None, None, 1.0).is_err(), "a console cannot speak");
        hv.connect(Front::Browser);
        assert_eq!(hv.support(), Support::Checking);
        assert!(hv.speak("hello", None, None, 1.0).is_err(), "not before the page answered");
        assert_eq!(hv.take_outgoing(), vec![ToClient::ListVoices], "the page is asked for its voices on connecting");
        hv.event(FromClient::Voices { supported: false, voices: vec![] });
        assert!(matches!(hv.support(), Support::No(ref why) if why.contains("speechSynthesis")));
        hv.event(FromClient::Voices { supported: true, voices: vec![VoiceInfo { name: "A".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "B".into(), lang: "en-GB".into(), local: true }] });
        assert!(hv.is_ready());
        assert_eq!(voices_for(&hv.voices(), "ta-IN").len(), 1);
        assert_eq!(voices_for(&hv.voices(), "en-US")[0].name, "B", "only the language part is compared");
        assert_eq!(lang_tag("Tamil"), "ta-IN");
        assert_eq!(lang_tag("English"), "en-US");
        let id = hv.speak("a very long piece ".repeat(500).as_str(), None, None, 99.0).unwrap();
        let ToClient::Speak { text, rate, .. } = hv.take_outgoing().remove(0) else { panic!() };
        assert!(text.chars().count() <= MAX_PIECE_CHARS && rate <= 2.0, "what is sent is bounded");
        hv.disconnect();
        assert!(matches!(hv.state(id), UttState::Failed(_)), "a piece in flight fails when the page goes away");
        assert_eq!(hv.support(), Support::Unknown);
        assert!(hv.take_outgoing().is_empty());
    }

    #[test]
    fn pieces_are_spoken_in_order_and_each_end_moves_on_to_the_next() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(30), None, false);
        wait("the page's voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("done", || r.status() == Status::Done);
        let want: Vec<String> = pieces().iter().map(|c| c.text.clone()).collect();
        assert_eq!(page.spoken(), want, "every piece once, in order");
        assert!(page.log.lock().unwrap().iter().all(|m| match m {
            ToClient::Speak { voice, lang, rate, .. } => voice.as_deref() == Some("Test Voice") && lang.as_deref() == Some("en-US") && *rate == 1.0,
            _ => true,
        }));
        assert_eq!(r.snapshot().fraction, 1.0);
    }

    #[test]
    fn reading_can_start_while_the_story_is_still_being_written() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(20), None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        let all = pieces();
        r.start(all[..1].to_vec(), 0, true, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("the first piece", || page.spoken().len() == 1);
        // the piece ends and the reader waits for more text: not finished
        wait("waiting for more", || matches!(r.status(), Status::Buffering));
        assert!(!matches!(r.status(), Status::Done), "waiting for more text, not finished: {:?}", r.status());
        r.append(all[1..3].to_vec());
        wait("two more", || page.spoken().len() == 3);
        r.append(all[3..].to_vec());
        r.close_input();
        wait("done", || r.status() == Status::Done);
        assert_eq!(page.spoken().len(), 4);
    }

    #[test]
    fn pause_stops_the_page_and_play_says_the_piece_again_stop_and_seek_cancel() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, NEVER, None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("playing", || r.status() == Status::Playing);
        r.pause();
        wait("the cancel", || page.cancels() >= 1);
        assert_eq!(r.status(), Status::Paused);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(page.spoken().len(), 1, "nothing new while paused");
        r.resume();
        wait("said again", || page.spoken().len() == 2);
        assert_eq!(page.spoken()[0], page.spoken()[1], "the interrupted piece starts again from its beginning");
        // seek: the piece in progress is cancelled and the chosen one is spoken
        r.seek(2);
        wait("piece 3", || page.spoken().last().is_some_and(|t| t == "The moon was hiding."));
        // a seek starts a new reading; the old one's thread cancels its piece on its own schedule, so the cancel can reach the page
        // after the new piece was sent: wait for it
        wait("the cancel of the interrupted piece", || page.cancels() >= 2);
        // stop
        let before = page.cancels();
        r.stop();
        wait("the stop reaches the page", || page.cancels() > before);
        assert_eq!(r.status(), Status::Idle);
        let n = page.spoken().len();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(page.spoken().len(), n, "nothing is said after stop");
    }

    /// A sink whose first `play` takes a while to reach the page (a busy thread), like a thread that was just scheduled out.
    struct SlowFirst {
        inner: Arc<dyn Sink>,
        entered: Arc<AtomicBool>,
        first: AtomicBool,
    }

    impl Sink for SlowFirst {
        fn play(&self, wav: &[u8]) -> Result<Box<dyn Playing>, String> {
            if !self.first.swap(true, Ordering::SeqCst) {
                self.entered.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
            }
            self.inner.play(wav)
        }
    }

    #[test]
    fn a_seek_while_the_previous_piece_is_on_its_way_to_the_page_still_reads_the_chosen_piece() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(500), None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        let entered = Arc::new(AtomicBool::new(false));
        let slow: Arc<dyn Sink> = Arc::new(SlowFirst { inner: sink(&hv), entered: entered.clone(), first: AtomicBool::new(false) });
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), slow);
        wait("the first piece is about to be sent", || entered.load(Ordering::SeqCst));
        // the old reading is committed to sending piece 1 when the seek replaces it: the page must never be left with the
        // old piece taking the place of the new one
        r.seek(3);
        wait("done", || r.status() == Status::Done);
        assert_eq!(page.spoken().last().map(String::as_str), Some("She went home."));
        assert!(matches!(hv.state(hv_last_id(&hv)), UttState::Ended));
    }

    fn hv_last_id(hv: &HostVoice) -> u64 {
        hv.last_id()
    }

    #[test]
    fn a_failed_piece_is_reported_with_its_number_and_the_reason() {
        let hv = Arc::new(HostVoice::new());
        let page = FakePage::start(&hv, Duration::from_millis(10), Some("moon"), false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("failure", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("part 3 of 4") && m.contains("synthesis-failed"), "{m}");
        assert_eq!(page.spoken().len(), 3, "the earlier pieces were said, nothing after the failure");
    }

    #[test]
    fn a_page_that_never_starts_speaking_is_reported_and_a_closed_page_ends_reading() {
        let hv = Arc::new(HostVoice::with_start_timeout(Duration::from_millis(150)));
        let _page = FakePage::start(&hv, Duration::from_millis(10), None, true);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("timeout", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("part 1 of 4") && m.contains("click once inside the page"), "{m}");
        // the page goes away mid-piece
        let hv = Arc::new(HostVoice::new());
        let _page = FakePage::start(&hv, NEVER, None, false);
        wait("voices", || hv.is_ready());
        let r = Reader::new();
        r.start(pieces(), 0, false, "browser:a@1.00", Arc::new(BrowserSynth), sink(&hv));
        wait("playing", || r.status() == Status::Playing);
        hv.disconnect();
        wait("failure", || matches!(r.status(), Status::Failed(_)));
        let Status::Failed(m) = r.status() else { panic!() };
        assert!(m.contains("closed or lost the connection"), "{m}");
    }

    #[test]
    fn the_synth_makes_no_audio_and_symbols_only_pieces_are_skipped_not_sent() {
        let blob = BrowserSynth.synth("**Bold** text, see https://example.com now").unwrap();
        assert!(blob.starts_with(MAGIC) && !blob.starts_with(b"RIFF"));
        assert_eq!(&blob[MAGIC.len()..], "Bold text, see now".as_bytes(), "markup and links are not read out");
        let hv = Arc::new(HostVoice::new());
        hv.connect(Front::Browser);
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        let _ = hv.take_outgoing();
        let s = sink(&hv);
        let mut p = s.play(&BrowserSynth.synth("***").unwrap()).unwrap();
        assert!(!p.is_playing(), "nothing to say, nothing waited for");
        assert!(hv.take_outgoing().is_empty());
        assert!(s.play(b"RIFF....").is_err(), "a sound from another engine is refused, not spoken");
    }

    #[test]
    fn the_reading_status_offers_the_browser_voice_first_and_says_why_it_cannot_be_used() {
        use crate::story::{effective_engine, status_text, tts_status_for, TtsStatus, NO_ENGINE_HELP};
        use crate::voice::{SpeechEngine, VoiceSettings};
        let path = "/nonexistent-dir";
        let none = VoiceSettings::default();
        let hv = HostVoice::new();
        // a console, nothing set up: the help leads with the browser voice and how to get it
        assert_eq!(tts_status_for(&none, true, path, &hv), TtsStatus::NoEngine);
        let help = status_text(&TtsStatus::NoEngine);
        assert_eq!(help, NO_ENGINE_HELP);
        assert!(help.find("browser voice").unwrap() < help.find("Settings → Voice").unwrap(), "{help}");
        assert!(help.contains("iyakki web") && help.contains("iyakki serve --web"), "{help}");
        // a browser that can speak and nothing else set up: the browser voice is used by itself
        hv.connect(Front::Browser);
        assert_eq!(tts_status_for(&none, true, path, &hv), TtsStatus::NoEngine, "still asking the page: not offered yet");
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        assert_eq!(effective_engine(&none, &hv), Some(SpeechEngine::Browser));
        assert!(matches!(tts_status_for(&none, true, path, &hv), TtsStatus::Ready { ref engine } if engine.contains("browser voice")));
        // the account's voice permission still applies
        assert_eq!(tts_status_for(&none, false, path, &hv), TtsStatus::NotAllowed);
        // another engine that is set up wins over the default
        let piper = VoiceSettings { tts: Some(SpeechEngine::Command { command: "piper -f {out}".into() }), ..VoiceSettings::default() };
        assert_eq!(effective_engine(&piper, &hv), piper.tts);
        // chosen explicitly but this screen cannot speak
        let chosen = VoiceSettings { tts: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        hv.disconnect();
        let TtsStatus::BrowserUnavailable(why) = tts_status_for(&chosen, true, path, &hv) else { panic!() };
        assert!(why.contains("console") && why.contains("iyakki web"), "{why}");
        hv.connect(Front::Window);
        hv.set_unsupported("this window's web engine has no speech synthesis");
        let text = status_text(&tts_status_for(&chosen, true, path, &hv));
        assert!(text.contains("no speech synthesis") && text.contains("Settings → Voice"), "{text}");
        // the window asks its engine only when somebody wants to read
        let hw = HostVoice::new();
        hw.connect(Front::Window);
        assert!(!hw.probe_wanted());
        hw.want_probe();
        assert!(hw.probe_wanted());
    }

    #[test]
    fn settings_written_before_the_browser_voice_existed_still_load_and_the_new_kind_round_trips() {
        use crate::voice::{SpeechEngine, VoiceSettings};
        let old = r#"{"stt":{"kind":"http","url":"http://127.0.0.1:8080/v1/audio/transcriptions","model":"tiny.en"},"tts":{"kind":"command","command":"piper -f {out}"},"record_cmd":"arecord -q","play_cmd":"aplay -q","auto_send":false,"speak_replies":false}"#;
        let v: VoiceSettings = serde_json::from_str(old).unwrap();
        assert!(matches!(v.tts, Some(SpeechEngine::Command { .. })) && v.validate().is_ok());
        let none: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert!(none.tts.is_none(), "nothing set up stays nothing set up");
        let b = VoiceSettings { tts: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        let text = serde_json::to_string(&b).unwrap();
        assert!(text.contains(r#""tts":{"kind":"browser"}"#), "{text}");
        assert_eq!(serde_json::from_str::<VoiceSettings>(&text).unwrap(), b);
        assert!(b.validate().is_ok());
        // it only speaks: it cannot be the listening engine, and it makes no audio for anyone who asks for a file
        let bad = VoiceSettings { stt: Some(SpeechEngine::Browser), ..VoiceSettings::default() };
        assert!(bad.validate().unwrap_err().contains("cannot listen"));
        assert_eq!(SpeechEngine::Browser.label(), "browser voice");
        assert!(crate::story::engine_takes_speed(&SpeechEngine::Browser) && crate::story::voice_choices(&SpeechEngine::Browser).is_empty());
    }
}
