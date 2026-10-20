//! Word times from the sound: parsing what recognisers answer, matching it to the text (misheard, missing and extra words,
//! numbers, punctuation, Tamil), where a server is, and the whole path against a fake Lemonade on loopback (a canned WAV
//! from `/audio/speech`, a canned whisper.cpp `verbose_json` from `/audio/transcriptions`, a server that fails or is slow).
//! No real network.

use super::*;
use crate::providers::{Kind, ProviderCfg};
use crate::services::Services;
use crate::story::{self, Chunk, Reader, Status, Synth};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The made-speech cache and the list of servers that did not answer are shared: tests that use them take turns.
static SHARED: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

fn words(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    wordtime::words_of(text).iter().map(|w| c[w.start..w.end].iter().collect()).collect()
}

/// Heard words at 400 ms each, starting at `from` ms.
fn heard(list: &[&str], from: f32) -> Vec<RecWord> {
    list.iter().enumerate().map(|(i, w)| RecWord { text: w.to_string(), start_ms: from + i as f32 * 400.0, end_ms: from + i as f32 * 400.0 + 350.0 }).collect()
}

/// whisper.cpp server's `verbose_json`: tokens with a leading space start a word.
fn whisper_json(list: &[&str], from_s: f64) -> Value {
    let mut toks = vec![json!({"word": "[_BEG_]", "start": 0.0, "end": 0.0})];
    for (i, w) in list.iter().enumerate() {
        let s = from_s + i as f64 * 0.4;
        // split longer words into two tokens, the second without a space, as whisper's tokenizer does
        let ch: Vec<char> = w.chars().collect();
        if ch.len() > 4 {
            let (a, b): (String, String) = (ch[..ch.len() / 2].iter().collect(), ch[ch.len() / 2..].iter().collect());
            toks.push(json!({"word": format!(" {a}"), "start": s, "end": s + 0.15, "probability": 0.9}));
            toks.push(json!({"word": b, "start": s + 0.15, "end": s + 0.35, "probability": 0.9}));
        } else {
            toks.push(json!({"word": format!(" {w}"), "start": s, "end": s + 0.35, "probability": 0.9}));
        }
    }
    toks.push(json!({"word": ".", "start": from_s + list.len() as f64 * 0.4, "end": from_s + list.len() as f64 * 0.4}));
    json!({"task": "transcribe", "language": "english", "duration": 3.0, "text": list.join(" "), "segments": [{"id": 0, "text": list.join(" "), "start": from_s, "end": from_s + list.len() as f64 * 0.4, "words": toks}]})
}

// ------------------------------------------------------------------ parsing

#[test]
fn whisper_cpp_tokens_are_joined_into_words_and_special_tokens_and_marks_are_dropped() {
    let v = whisper_json(&["The", "quick", "brown", "fox"], 0.5);
    let w = parse_words(&v);
    assert_eq!(w.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["The", "quick", "brown", "fox."]);
    assert_eq!((w[1].start_ms, w[1].end_ms), (900.0, 1250.0), "a word split into two tokens spans both");
    assert!((w[0].start_ms - 500.0).abs() < 0.01);
}

#[test]
fn openai_words_captioned_timestamps_and_segments_only_are_read_and_a_compact_answer_has_no_times() {
    let v = json!({"text": "hello there", "words": [{"word": "hello", "start": 0.1, "end": 0.4}, {"word": "there", "start": 0.5, "end": 0.9}]});
    assert_eq!(parse_words(&v), vec![RecWord { text: "hello".into(), start_ms: 100.0, end_ms: 400.0 }, RecWord { text: "there".into(), start_ms: 500.0, end_ms: 900.0 }]);
    let v = json!({"audio": "", "timestamps": [{"word": "hi", "start_time": 0.0, "end_time": 0.25}]});
    assert_eq!(parse_words(&v)[0], RecWord { text: "hi".into(), start_ms: 0.0, end_ms: 250.0 });
    let v = json!({"segments": [{"text": " one two", "start": 1.0, "end": 2.0}]});
    let w = parse_words(&v);
    assert_eq!(w.len(), 2);
    assert!(w[0].start_ms == 1000.0 && (w[1].end_ms - 2000.0).abs() < 0.1);
    assert!(parse_words(&json!({"text": "no times here"})).is_empty());
    assert!(parse_words(&json!({"segments": []})).is_empty());
}

// ------------------------------------------------------------------ alignment

const FOX: &str = "The quick brown fox jumps over the lazy dog.";

#[test]
fn a_clean_recognition_gives_each_word_its_heard_start() {
    let rec = heard(&["the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog"], 200.0);
    let b = align(FOX, &rec, 4000.0).unwrap();
    assert_eq!(b.len(), 10);
    for (i, x) in b[..9].iter().enumerate() {
        assert!((x - (200.0 + i as f32 * 400.0)).abs() < 0.01, "word {i}: {x}");
    }
    assert_eq!(b[9], 4000.0, "ends at the sound's end");
    // what is shown is always the text's words: the timeline is built on the text
    let tl = wordtime::WordTimeline::with_starts(FOX, b).unwrap();
    assert_eq!(tl.range_at(1500.0), Some((16, 19)), "fox, at 1400..1800 ms");
}

#[test]
fn misheard_missing_and_extra_words_do_not_derail_the_rest() {
    // "fox" heard as "box", "lazy" not heard, an extra "um" after "jumps"
    let rec = heard(&["the", "quick", "brown", "box", "jumps", "um", "over", "the", "dog"], 0.0);
    let b = align(FOX, &rec, 4000.0).unwrap();
    let start = |w: usize| b[w];
    assert!((start(4) - 1600.0).abs() < 0.01, "jumps keeps its time");
    assert!((start(5) - 2400.0).abs() < 0.01, "over is after the extra word");
    assert!((start(6) - 2800.0).abs() < 0.01, "the");
    // "box" is close enough? No: fox/box differ in a letter of three, so fox is placed between brown and jumps
    assert!(start(3) > start(2) && start(3) < start(4), "fox between brown and jumps: {b:?}");
    // lazy (not heard) is placed between "the" and "dog"
    assert!(start(7) > start(6) && start(7) < start(8), "lazy between: {b:?}");
    assert!((start(8) - 3200.0).abs() < 0.01, "dog keeps its time");
    let mut prev = -1.0;
    for x in &b {
        assert!(*x >= prev, "rising: {b:?}");
        prev = *x;
    }
}

#[test]
fn numbers_and_punctuation_in_the_text_are_handled() {
    let text = "It costs 42 dollars, she said: \"really?\"";
    assert_eq!(words(text), ["It", "costs", "42", "dollars", "she", "said", "really"]);
    // whisper writes the number as words, and a mark is its own token
    let rec = heard(&["it", "costs", "forty-two", "dollars,", "she", "said", "really?"], 0.0);
    let b = align(text, &rec, 3200.0).unwrap();
    assert!((b[3] - 1200.0).abs() < 0.01, "dollars keeps its time after the number said as two words: {b:?}");
    assert!(b[2] >= b[1] && b[2] < b[3], "42 between costs and dollars");
    assert!((b[6] - 2400.0).abs() < 0.01);
    // and the other way round: the text says the words, whisper writes digits
    let rec = heard(&["about", "1,000", "people"], 0.0);
    let b = align("About one thousand people", &rec, 1500.0).unwrap();
    assert_eq!((b[0], b[3]), (0.0, 800.0));
    assert!(b[1] < b[2] && b[2] < b[3]);
}

#[test]
fn tamil_words_match_by_their_letters_and_unmatched_ones_are_spread_between_anchors() {
    let text = "ஒரு ஊரிலே ஒரு சிறிய முயல் இருந்தது.";
    assert_eq!(words(text).len(), 6);
    // whisper hears most words, one misspelt (a vowel sign missing), one not at all
    let rec = heard(&["ஒரு", "ஊரில", "ஒரு", "முயல்", "இருந்தது"], 100.0);
    let b = align(text, &rec, 2400.0).unwrap();
    assert!((b[0] - 100.0).abs() < 0.01 && (b[1] - 500.0).abs() < 0.01, "ஊரிலே matched despite the missing sign: {b:?}");
    assert!((b[4] - 1300.0).abs() < 0.01, "முயல்: {b:?}");
    assert!(b[3] > b[2] && b[3] < b[4], "சிறிய (not heard) between: {b:?}");
    // whisper answers in another script (or translates): nothing matches, so the estimate stays
    let rec = heard(&["oru", "oorile", "oru", "siriya", "muyal", "irunthathu"], 0.0);
    assert!(align(text, &rec, 2400.0).is_none());
}

#[test]
fn too_few_matches_or_nothing_heard_keeps_the_estimate() {
    assert!(align(FOX, &[], 3000.0).is_none());
    assert!(align("", &heard(&["a"], 0.0), 3000.0).is_none());
    assert!(align(FOX, &heard(&["completely", "different", "words", "here"], 0.0), 3000.0).is_none());
    // one word of nine is not enough to trust
    assert!(align(FOX, &heard(&["xx", "yy", "fox", "zz"], 0.0), 3000.0).is_none());
    // times that run backwards are not used as anchors
    let mut rec = heard(&["the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog"], 0.0);
    rec[5].start_ms = 10.0;
    let b = align(FOX, &rec, 4000.0).unwrap();
    assert!(b.windows(2).all(|w| w[1] >= w[0]), "{b:?}");
}

#[test]
fn with_starts_checks_its_times_and_the_clock_can_take_new_ones() {
    assert!(wordtime::WordTimeline::with_starts("a b", vec![0.0, 10.0]).is_none(), "one time short");
    assert!(wordtime::WordTimeline::with_starts("a b", vec![0.0, 10.0, 5.0]).is_none(), "falling");
    assert!(wordtime::WordTimeline::with_starts("a b", vec![0.0, f32::NAN, 20.0]).is_none());
    let tl = wordtime::WordTimeline::with_starts("a b", vec![0.0, 10.0, 20.0]).unwrap();
    assert!(tl.is_fitted() && tl.total_ms() == 20.0);
    let mut c = wordtime::WordClock::new(wordtime::WordTimeline::fitted("a b c d", 4000.0), 0.0);
    c.start(0.0);
    assert_eq!(c.current(500.0), Some(0));
    c.set_timeline(wordtime::WordTimeline::with_starts("a b c d", vec![0.0, 100.0, 200.0, 300.0, 4000.0]).unwrap());
    assert_eq!(c.current(500.0), Some(3), "the same moment, the measured word");
}

// ------------------------------------------------------------------ where a server is

#[test]
fn servers_are_told_apart_by_where_they_are() {
    for u in ["http://localhost:13305/api/v1", "http://127.0.0.1:13305", "http://[::1]:13305/v1"] {
        assert_eq!(locality(u), Locality::ThisComputer, "{u}");
    }
    for u in ["http://192.168.1.20:13305", "http://10.0.0.9:13305/api/v1", "http://172.16.4.1", "http://amd-box:13305", "http://amd-box.local:13305", "http://100.80.1.2", "http://[fd00::5]:13305"] {
        assert_eq!(locality(u), Locality::Network, "{u}");
    }
    for u in ["https://lemonade.example.com/api/v1", "http://8.8.8.8:13305"] {
        assert_eq!(locality(u), Locality::Internet, "{u}");
    }
    assert_eq!(Locality::Internet.label(), "on the internet");
    assert_eq!(server_of("https://h.example.com:8443/api/v1/audio/speech"), "https://h.example.com:8443");
    assert!(unreachable("connection refused") && unreachable("HTTP 503: busy") && !unreachable("HTTP 404: model not found"));
}

// ------------------------------------------------------------------ a fake Lemonade on loopback

#[derive(Clone, Default)]
struct Behaviour {
    /// Answer `/audio/speech` with 500 this many times first.
    speech_fails: Arc<AtomicUsize>,
    /// Wait this long before answering `/audio/speech`.
    speech_delay_ms: u64,
    /// The `verbose_json` to answer with.
    heard: Arc<Mutex<Value>>,
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Fake {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1", self.port)
    }
    fn count(&self, path: &str) -> usize {
        self.seen.lock().unwrap().iter().filter(|(p, _)| p.starts_with(path)).count()
    }
    fn bodies(&self, path: &str) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|(p, _)| p.starts_with(path)).map(|(_, b)| b.clone()).collect()
    }
}

/// 3 s of tone at 24 kHz (Kokoro's rate), as Lemonade answers with `response_format: wav`.
fn speech_wav() -> Vec<u8> {
    let pcm: Vec<i16> = (0..72_000).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
    voice::encode_wav(&pcm, 24_000)
}

fn models() -> String {
    json!({"data": [
        {"id": "kokoro-v1", "recipe": "kokoro", "labels": ["tts"], "downloaded": true},
        {"id": "Whisper-Large-v3", "recipe": "whispercpp", "labels": ["transcription"], "downloaded": true},
        {"id": "Whisper-Base", "recipe": "whispercpp", "labels": ["transcription"], "downloaded": true},
        {"id": "Qwen3-0.6B-GGUF", "recipe": "llamacpp", "labels": [], "downloaded": true}
    ]})
    .to_string()
}

fn serve(b: Behaviour) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let (log, b) = (log.clone(), b.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; len];
                let _ = std::io::Read::read_exact(&mut r, &mut body);
                log.lock().unwrap().push((path.clone(), String::from_utf8_lossy(&body).into_owned()));
                let (status, ctype, out): (u16, &str, Vec<u8>) = match (method.as_str(), path.split('?').next().unwrap_or("")) {
                    ("GET", "/api/v1/health") => (200, "application/json", br#"{"status":"ok","version":"2026.42.1","all_models_loaded":[],"max_models":{"llm":1}}"#.to_vec()),
                    ("GET", "/api/v1/models") => (200, "application/json", models().into_bytes()),
                    ("POST", "/api/v1/audio/speech") => {
                        std::thread::sleep(Duration::from_millis(b.speech_delay_ms));
                        if b.speech_fails.load(Ordering::SeqCst) > 0 {
                            b.speech_fails.fetch_sub(1, Ordering::SeqCst);
                            (500, "application/json", br#"{"error":{"message":"backend crashed"}}"#.to_vec())
                        } else {
                            (200, "audio/wav", speech_wav())
                        }
                    }
                    ("POST", "/api/v1/audio/transcriptions") => (200, "application/json", b.heard.lock().unwrap().to_string().into_bytes()),
                    _ => (404, "application/json", br#"{"error":{"message":"not found"}}"#.to_vec()),
                };
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).as_bytes());
                let _ = s.write_all(&out);
            });
        }
    });
    Fake { port, seen }
}

fn services(tag: &str) -> (Arc<Services>, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-speechtime-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    let s = Services::open(&d, up.clone(), up).unwrap();
    // the engines of this computer are not looked for in tests (detection is off unless enabled), so a fallback is predictable
    (s, d)
}

fn with_lemonade(tag: &str, b: Behaviour) -> (Arc<Services>, std::path::PathBuf, Fake) {
    let fake = serve(b);
    let (svc, d) = services(tag);
    svc.use_local_lemonade(&fake.base(), None);
    svc.refresh_lemonade("lemonade-local").unwrap();
    (svc, d, fake)
}

const TEXT: &str = "The fox ran over the hill and slept.";

#[test]
fn a_local_lemonade_reads_by_itself_and_its_whisper_times_the_words_once() {
    let _turn = turn();
    clear_cache();
    let b = Behaviour::default();
    // "ran" heard as "rang", "slept" missing, an extra "uh"
    *b.heard.lock().unwrap() = whisper_json(&["The", "fox", "rang", "over", "uh", "the", "hill", "and"], 0.1);
    let (svc, d, fake) = with_lemonade("auto", b);
    // nothing set up in Settings → Voice: the Lemonade voice on this computer is used
    let e = svc.effective_tts(None).unwrap();
    assert!(matches!(&e, SpeechEngine::Http { url, model, .. } if url == &format!("{}/audio/speech", fake.base()) && model == "kokoro-v1"), "{e:?}");
    assert!(!svc.effective_tts(Some("ta-IN")).is_some_and(|e| matches!(e, SpeechEngine::Http { .. })), "Kokoro does not read Tamil");
    let opts = SpeakOpts { voice: Some("af_heart".into()), speed: 1.0, lang: Some("en-US".into()) };
    let (wav, native) = svc.synthesize_timed(TEXT, &opts).unwrap();
    assert!(native.is_none(), "Lemonade's speech endpoint gives no word times");
    let sent: Value = serde_json::from_str(&fake.bodies("/api/v1/audio/speech")[0]).unwrap();
    assert_eq!((sent["model"].as_str(), sent["voice"].as_str(), sent["response_format"].as_str()), (Some("kokoro-v1"), Some("af_heart"), Some("wav")));
    assert!((wav_ms(&wav) - 3000.0).abs() < 1.0);
    // the aligner is the small whisper on the same server
    assert!(matches!(svc.speech_aligner(), Some(SpeechEngine::Http { model, .. }) if model == "Whisper-Base"));
    let t = svc.align_speech(TEXT, &wav, Some("en-US")).unwrap();
    assert_eq!(t.source, TimingSource::Aligned);
    let tl = t.timeline(TEXT).unwrap();
    assert_eq!(tl.len(), 8);
    assert!((tl.start_ms(1).unwrap() - 500.0).abs() < 0.5, "fox at 0.5 s: {:?}", t.begin);
    assert!((tl.start_ms(3).unwrap() - 1300.0).abs() < 0.5, "over keeps its time after the misheard word");
    assert!((tl.start_ms(5).unwrap() - 2500.0).abs() < 0.5, "hill after the extra word");
    assert!(tl.start_ms(7).unwrap() > tl.start_ms(6).unwrap() && tl.total_ms() == 3000.0, "slept placed after and, before the end");
    let form = &fake.bodies("/api/v1/audio/transcriptions")[0];
    assert!(form.contains("verbose_json") && form.contains("timestamp_granularities[]") && form.contains("name=\"language\"\r\n\r\nen") && form.contains("Whisper-Base"));
    // the sound went as 16 kHz WAV
    assert!(form.contains("RIFF") && form.contains("audio/wav"));
    // cached: the same text again asks nothing
    let (n_s, n_t) = (fake.count("/api/v1/audio/speech"), fake.count("/api/v1/audio/transcriptions"));
    let (wav2, _) = svc.synthesize_timed(TEXT, &opts).unwrap();
    assert_eq!(wav2, wav);
    assert_eq!(svc.align_speech(TEXT, &wav, Some("en-US")), Some(t));
    assert_eq!((fake.count("/api/v1/audio/speech"), fake.count("/api/v1/audio/transcriptions")), (n_s, n_t));
    // word alignment can be turned off
    svc.settings.lock().unwrap().update(|s| s.voice.align_words = false).unwrap();
    assert!(svc.speech_aligner().is_none());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_whisper_answer_without_times_keeps_the_estimate() {
    let _turn = turn();
    clear_cache();
    let b = Behaviour::default();
    *b.heard.lock().unwrap() = json!({"text": "The fox ran over the hill and slept."});
    let (svc, d, _fake) = with_lemonade("notimes", b);
    let (wav, _) = svc.synthesize_timed(TEXT, &SpeakOpts::default()).unwrap();
    assert_eq!(svc.align_speech(TEXT, &wav, None), None);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_reader_starts_estimated_and_switches_to_aligned_times_from_lemonade() {
    let _turn = turn();
    clear_cache();
    let b = Behaviour::default();
    *b.heard.lock().unwrap() = whisper_json(&["The", "fox", "ran", "over", "the", "hill", "and", "slept"], 0.3);
    let (svc, d, _fake) = with_lemonade("reader", b);
    let r = Reader::new();
    let synth = Arc::new(story::ServicesSynth { svc: svc.clone(), opts: SpeakOpts::default() });
    let sink = Arc::new(LongSink);
    r.start(vec![Chunk { unit: 0, start: 0, end: TEXT.chars().count(), text: TEXT.into() }], 0, false, "v", synth, sink);
    let deadline = Instant::now() + Duration::from_secs(30);
    while r.snapshot().timing != TimingSource::Aligned {
        assert!(Instant::now() < deadline, "never aligned: {:?}", r.snapshot());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(r.status(), Status::Playing | Status::Buffering));
    r.stop();
    let _ = std::fs::remove_dir_all(d);
}

/// A speaker that plays for a minute.
struct LongSink;
struct LongChild(Instant);
impl story::Playing for LongChild {
    fn is_playing(&mut self) -> bool {
        self.0.elapsed() < Duration::from_secs(60)
    }
    fn stop(&mut self) {
        self.0 = Instant::now() - Duration::from_secs(120);
    }
}
impl story::Sink for LongSink {
    fn play(&self, _wav: &[u8]) -> Result<Box<dyn story::Playing>, String> {
        Ok(Box::new(LongChild(Instant::now())))
    }
}

#[test]
fn a_failing_server_is_retried_once_then_left_alone_and_a_slow_one_still_answers() {
    let _turn = turn();
    clear_cache();
    clear_down_for_tests();
    let b = Behaviour { speech_fails: Arc::new(AtomicUsize::new(1)), ..Default::default() };
    let (svc, d, fake) = with_lemonade("retry", b.clone());
    // one failure: the second try works
    let (wav, _) = svc.synthesize_timed("First try fails.", &SpeakOpts::default()).unwrap();
    assert!(wav.starts_with(b"RIFF"));
    assert_eq!(fake.count("/api/v1/audio/speech"), 2);
    // two failures: no engine of this computer here, so it fails, and the server is left alone for a while
    b.speech_fails.store(2, Ordering::SeqCst);
    let e = svc.synthesize_timed("Both tries fail.", &SpeakOpts::default()).unwrap_err();
    assert!(e.contains("500"), "{e}");
    assert!(is_down(&fake.base()));
    assert!(svc.lemonade_auto_voice(None).is_none(), "not offered while down");
    assert!(svc.speech_aligner().is_none());
    clear_down_for_tests();
    // nothing answering at all
    let (svc2, d2) = services("gone");
    let dead = format!("http://127.0.0.1:{}/api/v1", TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port());
    svc2.use_local_lemonade(&dead, None);
    svc2.settings.lock().unwrap().update(|s| { s.providers[0].model_kinds.insert("kokoro-v1".into(), "tts".into()); }).unwrap();
    assert!(svc2.synthesize_timed("Anyone there?", &SpeakOpts::default()).is_err());
    assert!(is_down(&dead));
    clear_down_for_tests();
    // a slow server (latency) still answers within the timeout
    let slow = Behaviour { speech_delay_ms: 300, ..Default::default() };
    let (svc3, d3, _f3) = with_lemonade("slow", slow);
    let t0 = Instant::now();
    assert!(svc3.synthesize_timed("Slowly now.", &SpeakOpts::default()).is_ok());
    assert!(t0.elapsed() >= Duration::from_millis(300));
    for d in [d, d2, d3] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_server_on_another_machine_needs_a_yes_before_anything_is_sent() {
    let _turn = turn();
    let (svc, d) = services("remote");
    let mut p = ProviderCfg::new("amd-box", Kind::Lemonade);
    p.base_url = Some("http://192.168.1.20:13305/api/v1".into());
    p.model_kinds.insert("kokoro-v1".into(), "tts".into());
    p.model_kinds.insert("Whisper-Base".into(), "stt".into());
    svc.settings.lock().unwrap().upsert_provider(p).unwrap();
    assert!(!svc.speech_server_allowed("http://192.168.1.20:13305/api/v1"));
    assert!(svc.lemonade_auto_voice(None).is_none(), "never used by default");
    assert!(svc.speech_aligner().is_none());
    let e = svc.use_lemonade_voice("amd-box", "kokoro-v1", Some("af_heart")).unwrap_err();
    assert!(e.contains("on the network") && e.contains("yes"), "{e}");
    assert!(svc.use_lemonade_stt("amd-box", "Whisper-Base").is_err());
    assert!(svc.voice_settings().tts.is_none(), "nothing was set");
    // the person says yes for that server: it can be used, and only that one
    svc.allow_speech_server("http://192.168.1.20:13305/api/v1/audio/speech").unwrap();
    assert_eq!(svc.voice_settings().remote_ok, vec!["http://192.168.1.20:13305".to_string()]);
    assert!(svc.speech_server_allowed("http://192.168.1.20:13305/api/v1"));
    assert!(!svc.speech_server_allowed("http://192.168.1.21:13305/api/v1"));
    assert!(svc.use_lemonade_voice("amd-box", "kokoro-v1", Some("af_heart")).unwrap().contains("kokoro-v1"));
    assert!(matches!(svc.lemonade_auto_voice(None), Some(SpeechEngine::Http { .. })));
    assert!(matches!(svc.speech_aligner(), Some(SpeechEngine::Http { model, .. }) if model == "Whisper-Base"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn captioned_speech_gives_native_word_times() {
    let _turn = turn();
    clear_cache();
    // Kokoro-FastAPI's captioned speech, served by a tiny fake
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Some(Ok(mut s)) = l.incoming().next() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; len];
            let _ = std::io::Read::read_exact(&mut r, &mut body);
            let out = json!({"audio": iyakki_flow::report::base64(&speech_wav()), "audio_format": "wav", "timestamps": [
                {"word": "Hello", "start_time": 0.1, "end_time": 0.5}, {"word": ",", "start_time": 0.5, "end_time": 0.5}, {"word": "world", "start_time": 0.9, "end_time": 1.4}]})
            .to_string();
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).as_bytes());
        }
    });
    let (svc, d) = services("captioned");
    svc.settings.lock().unwrap().update(|s| s.voice.tts = Some(SpeechEngine::Http { url: format!("http://127.0.0.1:{port}/dev/captioned_speech"), model: "kokoro".into(), key_ref: None, voice: Some("af_heart".into()), language: None })).unwrap();
    let (wav, t) = svc.synthesize_timed("Hello, world!", &SpeakOpts::default()).unwrap();
    assert!(wav.starts_with(b"RIFF"));
    let t = t.unwrap();
    assert_eq!(t.source, TimingSource::Native);
    assert_eq!(t.begin, vec![100.0, 900.0, 3000.0]);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_reader_synth_without_times_needs_nothing_new() {
    struct Plain;
    impl Synth for Plain {
        fn synth(&self, _t: &str) -> Result<Vec<u8>, String> {
            Ok(vec![1])
        }
    }
    assert_eq!(Plain.synth_timed("x").unwrap(), (vec![1], None));
    assert_eq!(Plain.align("x", &[]), None);
}
