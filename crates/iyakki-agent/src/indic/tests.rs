use super::*;
use std::sync::Mutex;

/// A server that answers like the real one: a tone whose length follows the text, loudness that differs per piece.
struct Fake {
    installed: Vec<&'static str>,
    calls: Mutex<Vec<(String, String, f32)>>,
    fail_on: Option<&'static str>,
}

impl Fake {
    fn new(installed: &[&'static str]) -> Fake {
        Fake { installed: installed.to_vec(), calls: Mutex::new(Vec::new()), fail_on: None }
    }
}

impl Server for Fake {
    fn voices(&self) -> Result<Vec<SrvVoice>, Fail> {
        Ok(self.installed.iter().flat_map(|l| ["female", "male"].map(|g| SrvVoice { id: format!("{l}-{g}"), lang: l.to_string(), gender: g.to_string() })).collect())
    }

    fn speech(&self, voice: &str, text: &str, speed: f32) -> Result<Vec<u8>, Fail> {
        self.calls.lock().unwrap().push((voice.to_string(), text.to_string(), speed));
        if self.fail_on.is_some_and(|f| text.contains(f)) {
            return Err(Fail::Other("HTTP 500: boom".into()));
        }
        let lang = voice.split('-').next().unwrap_or("");
        if !self.installed.contains(&lang) {
            return Err(Fail::NotInstalled(lang.to_string()));
        }
        let n = text.chars().count() * 900;
        let amp = if text.len() % 2 == 0 { 0.2 } else { 0.9 };
        let mut s: Vec<i16> = vec![0; 3000];
        s.extend((0..n).map(|i| {
            let env = (i.min(n - 1 - i) as f32 / 400.0).min(1.0);
            ((i as f32 * 0.05).sin() * amp * env * 32767.0) as i16 + 400
        }));
        s.extend(vec![0i16; 4000]);
        Ok(voice::encode_wav(&s, 22050))
    }
}

fn read(fake: &Fake, lang: Option<&str>, voice: Option<&str>, text: &str) -> Result<Vec<u8>, Fail> {
    let none = || false;
    synthesize(&Reading { server: fake, lang, voice, speed: 1.0, cancelled: &none }, text)
}

#[test]
fn story_languages_map_to_model_languages_and_voices() {
    assert_eq!(lang_code("ta"), Some("ta"));
    assert_eq!(lang_code("ta-IN"), Some("ta"));
    assert_eq!(lang_code("TA_in"), Some("ta"));
    assert_eq!(lang_code("Tamil"), Some("ta"));
    assert_eq!(lang_code("tam"), Some("ta"));
    assert_eq!(lang_code("en-GB"), Some("en"));
    assert_eq!(lang_code("ja"), None);
    assert_eq!(parse_setting(Some("male")), (None, Some(Gender::Male)));
    assert_eq!(parse_setting(Some("ml-female")), (Some("ml"), Some(Gender::Female)));
    assert_eq!(parse_setting(Some("zz-female")), (None, None));
    assert_eq!(parse_setting(None), (None, None));
    let v = Fake::new(&["ta", "hi"]).voices().unwrap();
    assert_eq!(choose(&v, "ta", Gender::Male).unwrap().id, "ta-male");
    assert_eq!(choose(&v, "hi", Gender::Female).unwrap().id, "hi-female");
    assert_eq!(choose(&v, "te", Gender::Female), Err(Fail::NotInstalled("te".into())));
    // a language with one gender only gets the other one
    let one = vec![SrvVoice { id: "ta-male".into(), lang: "ta".into(), gender: "male".into() }];
    assert_eq!(choose(&one, "ta", Gender::Female).unwrap().id, "ta-male");
}

#[test]
fn server_answers_are_parsed_and_bad_ones_refused() {
    let v = parse_voices(br#"{"voices":[{"id":"ta-female"},{"id":"ta-male"},{"id":"xx-male"},{"id":"ta-other"},{"id":"nodash"}]}"#).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].label(), "Tamil · female");
    assert!(parse_voices(b"<html>").is_err() && parse_voices(b"{}").is_err());
    assert!(validate_url("http://127.0.0.1:8189").is_ok() && validate_url("https://tts.example.lan").is_ok());
    for bad in ["ftp://x", "http://", "http://u:p@host", "http://a b", "127.0.0.1:8189", "http://h\n"] {
        assert!(validate_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_reading_asks_for_each_piece_with_the_right_voice_and_joins_them_without_clicks() {
    let fake = Fake::new(&["ta"]);
    let text = "ஒரு ஊரிலே ஒரு சிறிய முயல் இருந்தது. அது தினமும் காட்டில் விளையாடியது.";
    let wav = read(&fake, Some("ta"), Some("male"), text).unwrap();
    let calls = fake.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "two sentences, two requests: {calls:?}");
    assert!(calls.iter().all(|(v, _, s)| v == "ta-male" && (*s - 1.0).abs() < 1e-6));
    let w = voice::parse_wav(&wav).unwrap();
    assert_eq!((w.rate, w.channels), (22050, 1));
    // the pause between the sentences is there, the whole recording starts and ends at zero, and no jump is larger than the tone's own
    let c = audio::check(&w.samples, w.rate);
    assert!(c.seconds > 1.0 && c.clipped == 0 && c.peak <= audio::PEAK_LIMIT + 0.01, "{c:?}");
    assert_eq!((w.samples[0], *w.samples.last().unwrap()), (0, 0));
    assert!(audio::max_step(&w.samples) < 3500, "{}", audio::max_step(&w.samples));
    let zeros = w.samples.windows(3000).filter(|x| x.iter().all(|&s| s == 0)).count();
    assert!(zeros > 0, "a pause of silence between the sentences");
}

#[test]
fn the_default_voice_is_female_and_speed_is_passed_on() {
    let fake = Fake::new(&["hi"]);
    let none = || false;
    synthesize(&Reading { server: &fake, lang: Some("hi-IN"), voice: None, speed: 1.4, cancelled: &none }, "नमस्ते, आप कैसे हैं?").unwrap();
    let c = fake.calls.lock().unwrap().clone();
    assert_eq!(c[0].0, "hi-female");
    assert!((c[0].2 - 1.4).abs() < 1e-6);
}

#[test]
fn a_missing_language_is_reported_and_never_read_in_another() {
    let fake = Fake::new(&["hi"]);
    assert_eq!(read(&fake, Some("ta"), None, "வணக்கம்"), Err(Fail::NotInstalled("ta".into())));
    assert!(fake.calls.lock().unwrap().is_empty(), "no request for a language without a voice");
    assert_eq!(Fail::NotInstalled("ta".into()).line(), "The Indic voice for Tamil is not installed");
    assert!(Fail::Unreachable("connection refused".into()).line().contains("connection refused"));
}

#[test]
fn digits_become_words_before_the_server_sees_the_text() {
    let fake = Fake::new(&["ta"]);
    read(&fake, Some("ta"), None, "இன்று 25 டிகிரி").unwrap();
    let t = fake.calls.lock().unwrap()[0].1.clone();
    assert!(!t.chars().any(|c| c.is_ascii_digit()) && t.contains("இருபத்தி ஐந்து"), "{t}");
}

#[test]
fn latin_words_use_the_english_voice_when_it_is_installed_and_the_script_otherwise() {
    let with = Fake::new(&["ta", "en"]);
    read(&with, Some("ta"), Some("male"), "என் கணினியில் WiFi வேலை செய்யவில்லை").unwrap();
    let c = with.calls.lock().unwrap().clone();
    assert!(c.iter().any(|(v, t, _)| v == "en-male" && t.contains("WiFi")), "{c:?}");
    assert!(c.iter().any(|(v, _, _)| v == "ta-male"));
    let without = Fake::new(&["ta"]);
    read(&without, Some("ta"), None, "என் கணினியில் WiFi வேலை செய்யவில்லை").unwrap();
    let c = without.calls.lock().unwrap().clone();
    assert!(c.iter().all(|(v, t, _)| v == "ta-female" && !t.chars().any(|c| c.is_ascii_alphabetic())), "{c:?}");
}

#[test]
fn errors_cancel_and_empty_text_are_reported() {
    let mut fake = Fake::new(&["en"]);
    fake.fail_on = Some("boom");
    assert!(matches!(read(&fake, Some("en"), None, "this will boom now."), Err(Fail::Other(m)) if m.contains("boom")));
    assert!(matches!(read(&Fake::new(&["en"]), Some("en"), None, "  \n "), Err(Fail::Other(m)) if m.contains("nothing")));
    let stop = || true;
    let f = Fake::new(&["en"]);
    assert!(matches!(synthesize(&Reading { server: &f, lang: Some("en"), voice: None, speed: 1.0, cancelled: &stop }, "Hello there."), Err(Fail::Other(m)) if m == "stopped"));
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn an_unknown_story_language_reads_in_english_when_it_is_installed() {
    let f = Fake::new(&["en", "ta"]);
    read(&f, None, None, "Hello there.").unwrap();
    assert_eq!(f.calls.lock().unwrap()[0].0, "en-female");
}

// ------------------------------------------------------------------------------------------------ real HTTP against a fake server

fn serve(handler: impl Fn(&str, &str, &[u8]) -> (u16, &'static str, Vec<u8>) + Send + 'static) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = vec![0u8; 65536];
            let mut n = 0;
            loop {
                let k = s.read(&mut buf[n..]).unwrap_or(0);
                n += k;
                let head_end = buf[..n].windows(4).position(|w| w == b"\r\n\r\n");
                if let Some(h) = head_end {
                    let head = String::from_utf8_lossy(&buf[..h]).to_string();
                    let cl: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                    if n >= h + 4 + cl || k == 0 {
                        let first = head.lines().next().unwrap_or("").to_string();
                        let mut it = first.split(' ');
                        let (m, p) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
                        let (st, ct, body) = handler(&m, &p, &buf[h + 4..n]);
                        let _ = write!(s, "HTTP/1.1 {st} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = s.write_all(&body);
                        break;
                    }
                }
                if k == 0 {
                    break;
                }
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn the_http_client_speaks_the_servers_protocol() {
    let wav = voice::encode_wav(&(0..6000).map(|i| ((i as f32 * 0.05).sin() * 9000.0) as i16).collect::<Vec<_>>(), 22050);
    let seen = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let base = serve(move |m, p, body| match (m, p) {
        ("GET", "/v1/voices") => (200, "application/json", br#"{"voices":[{"id":"ta-female"},{"id":"ta-male"}]}"#.to_vec()),
        ("POST", "/v1/audio/speech") => {
            let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            s2.lock().unwrap().push(v.to_string());
            if v["voice"] == "te-female" {
                return (404, "application/json", br#"{"error":"model for te not installed","code":"model_not_installed","lang":"te"}"#.to_vec());
            }
            (200, "audio/wav", wav.clone())
        }
        _ => (404, "text/plain", b"no".to_vec()),
    });
    let srv = HttpServer::new(&format!("{base}/"));
    assert_eq!(srv.voices().unwrap().len(), 2);
    let w = srv.speech("ta-female", "வணக்கம்", 1.25).unwrap();
    assert_eq!(&w[..4], b"RIFF");
    let sent: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!((sent["input"].as_str(), sent["voice"].as_str(), sent["speed"].as_f64(), sent["response_format"].as_str()), (Some("வணக்கம்"), Some("ta-female"), Some(1.25), Some("wav")));
    assert_eq!(srv.speech("te-female", "x", 1.0), Err(Fail::NotInstalled("te".into())));
    // the whole path through synth_wav (voice list cached)
    clear_cache();
    let out = synth_wav(&base, Some("female"), Some("ta"), "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?", 1.0).unwrap();
    assert!(voice::parse_wav(&out).unwrap().samples.len() > 4000);
    assert_eq!(synth_wav(&base, None, Some("te"), "x", 1.0), Err(Fail::NotInstalled("te".into())));
    // nothing listening: unreachable, with a reason, quickly
    let t = Instant::now();
    assert!(matches!(synth_wav("http://127.0.0.1:1", None, Some("ta"), "x", 1.0), Err(Fail::Unreachable(_))));
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_bad_address_is_refused_before_any_request() {
    assert!(matches!(synth_wav("ftp://x", None, Some("ta"), "x", 1.0), Err(Fail::Other(_))));
}


// ------------------------------------------------------------------------------------------------ the engine kind in hostengines

use crate::hostengines::{self, Kind, Spec};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-indic-eng-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fake_server(installed: &'static [&'static str]) -> String {
    let wav = voice::encode_wav(&(0..9000).map(|i| ((i as f32 * 0.05).sin() * 9000.0 * (i.min(8999 - i) as f32 / 400.0).min(1.0)) as i16).collect::<Vec<_>>(), 22050);
    serve(move |m, p, body| match (m, p) {
        ("GET", "/v1/voices") => {
            let v: Vec<String> = installed.iter().flat_map(|l| [format!("{{\"id\":\"{l}-female\"}}"), format!("{{\"id\":\"{l}-male\"}}")]).collect();
            (200, "application/json", format!("{{\"voices\":[{}]}}", v.join(",")).into_bytes())
        }
        ("POST", "/v1/audio/speech") => {
            let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            let lang = v["voice"].as_str().unwrap_or("").split('-').next().unwrap_or("").to_string();
            if installed.contains(&lang.as_str()) {
                (200, "audio/wav", wav.clone())
            } else {
                (404, "application/json", format!("{{\"code\":\"model_not_installed\",\"lang\":\"{lang}\"}}").into_bytes())
            }
        }
        _ => (404, "text/plain", b"no".to_vec()),
    })
}

#[test]
fn the_engine_kind_is_known_valid_and_shaped_like_the_other_host_engines() {
    assert_eq!(Kind::from_id("indic-tts"), Some(Kind::IndicTts));
    assert!(!Kind::ALL.contains(&Kind::IndicTts), "never found by the detection, chosen on purpose");
    assert!(Kind::IndicTts.can_file() && !Kind::IndicTts.can_speak());
    assert_eq!(Kind::IndicTts.label(), "Indic neural voice (local)");
    assert!(hostengines::validate_host("indic-tts", Some("http://127.0.0.1:8189"), Some("female")).is_ok());
    assert!(hostengines::validate_host("indic-tts", Some("/usr/bin/x"), None).is_err(), "a server address, not a program");
    assert!(hostengines::validate_host("indic-tts", Some("http://u:p@h"), None).is_err());
    assert!(hostengines::validate_host("indic-tts", None, Some("-bad")).is_err());
    let s = Spec::from_parts("indic-tts", None, None).unwrap();
    assert_eq!(s.program, DEFAULT_URL);
    let e = voice::SpeechEngine::Host { engine: "indic-tts".into(), program: Some("http://192.168.1.9:8189".into()), voice: Some("male".into()) };
    assert!(e.validate().is_ok());
    assert!(hostengines::can_save_audio(&e), "Save audio works: the engine makes WAV files");
    assert!(hostengines::needs_player(&e) && !hostengines::speaks_directly(&e));
    assert!(hostengines::plan(&s, Some("ta"), 1.0, "x", None).is_err(), "no process is ever started for it");
    let story = crate::story::tts_status_auto(&voice::VoiceSettings { tts: Some(e.clone()), ..voice::VoiceSettings::default() }, true, "/nonexistent", &crate::hostvoice::HostVoice::new(), None);
    assert!(!matches!(story, crate::story::TtsStatus::HostUnavailable(_)), "an address is not checked as a file: {story:?}");
}

#[test]
fn the_engine_lists_the_voices_of_the_server_and_makes_a_wav_through_synthesize_with() {
    clear_cache();
    let base = fake_server(&["ta", "hi"]);
    let spec = Spec::from_parts("indic-tts", Some(&base), Some("male")).unwrap();
    let v = hostengines::resolve(&spec, Some("ta")).0;
    assert_eq!(v.voice.as_deref(), Some("male"), "the setting is kept, the language is chosen at synthesis");
    let e = voice::SpeechEngine::Host { engine: "indic-tts".into(), program: Some(base.clone()), voice: Some("male".into()) };
    let up: std::sync::Arc<dyn iyakki_proxy::Upstream> = std::sync::Arc::new(iyakki_proxy::https::UreqUpstream::default());
    let wav = voice::synthesize_with(&e, "வணக்கம், நீங்கள் எப்படி இருக்கிறீர்கள்?", &voice::SpeakOpts { voice: None, speed: 1.0, lang: Some("ta".into()) }, &crate::vault::Secrets::new(None, up.clone()), up.as_ref()).unwrap();
    assert!(voice::parse_wav(&wav).unwrap().samples.len() > 3000);
}

#[test]
fn a_missing_language_falls_back_to_the_next_engine_with_one_line_why() {
    clear_cache();
    let base = fake_server(&["hi"]);
    let dir = tmpdir("fb");
    // a fake espeak-ng that writes a WAV to the -w file (and reads the text from stdin)
    let wav = voice::encode_wav(&vec![500i16; 4410], 22050);
    let wavfile = dir.join("fixed.wav");
    std::fs::write(&wavfile, &wav).unwrap();
    let prog = dir.join("espeak-ng");
    std::fs::write(&prog, format!("#!/bin/sh\ncat >/dev/null\nwhile [ \"$1\" != \"-w\" ] && [ -n \"$1\" ]; do shift; done\ncp {} \"$2\"\n", wavfile.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let report = std::sync::Arc::new(hostengines::Report {
        engines: vec![hostengines::Engine { kind: Kind::EspeakNg, program: prog.clone(), voices: vec![hostengines::HVoice { id: "ta".into(), name: "Tamil".into(), langs: vec!["ta".into()] }], problem: None }],
        missing: vec![],
        at: std::time::SystemTime::now(),
    });
    let spec = Spec::from_parts("indic-tts", Some(&base), None).unwrap();
    let out = hostengines::synth_indic_with(&spec, "வணக்கம்", Some("ta"), 1.0, &|| report.clone()).unwrap();
    assert_eq!(out, wav, "the espeak-ng output came back");
    let note = last_note().unwrap();
    assert!(note.contains("Tamil is not installed") && note.contains("espeak-ng") && !note.contains('\n'), "{note}");
    // a language that is installed does not fall back
    let hi = hostengines::synth_indic_with(&spec, "नमस्ते", Some("hi"), 1.0, &|| report.clone()).unwrap();
    assert_ne!(hi, wav);
    // nothing else on the computer: the error says both
    let empty = std::sync::Arc::new(hostengines::Report::empty());
    let e = hostengines::synth_indic_with(&spec, "வணக்கம்", Some("ta"), 1.0, &|| empty.clone()).unwrap_err();
    assert!(e.contains("not installed") && e.contains("no other speech engine"), "{e}");
    // a server that is down also falls back
    let down = Spec::from_parts("indic-tts", Some("http://127.0.0.1:1"), None).unwrap();
    assert_eq!(hostengines::synth_indic_with(&down, "x", Some("ta"), 1.0, &|| report.clone()).unwrap(), wav);
    assert!(last_note().unwrap().contains("cannot be reached"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_text_is_data_for_the_server_not_a_command() {
    let seen = std::sync::Arc::new(Mutex::new(String::new()));
    let s2 = seen.clone();
    let wav = voice::encode_wav(&(0..9000).map(|i| ((i as f32 * 0.05).sin() * 9000.0 * (i.min(8999 - i) as f32 / 400.0).min(1.0)) as i16).collect::<Vec<_>>(), 22050);
    let base = serve(move |m, p, body| match (m, p) {
        ("GET", "/v1/voices") => (200, "application/json", br#"{"voices":[{"id":"en-female"}]}"#.to_vec()),
        ("POST", _) => {
            *s2.lock().unwrap() = String::from_utf8_lossy(body).to_string();
            (200, "audio/wav", wav.clone())
        }
        _ => (404, "x", vec![]),
    });
    clear_cache();
    let text = "he said \"$(touch /tmp/pwned-indic)\" `rm -rf /` ; echo 'x' \\n -- --voice=../../etc";
    synth_wav(&base, Some("--voice"), Some("en"), text, 1.0).unwrap();
    let v: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()).unwrap();
    assert!(v["input"].as_str().unwrap().contains("$(touch"), "the text travels as JSON data: {v}");
    assert_eq!(v["voice"], "en-female", "a voice setting that is not a voice is ignored, never passed on");
    assert!(!std::path::Path::new("/tmp/pwned-indic").exists());
}

/// Needs a running server with models: `IYAKKI_INDIC_URL=http://127.0.0.1:8189 IYAKKI_INDIC_SENTENCES=<sentences.json> IYAKKI_INDIC_OUT=<dir>
/// cargo test -p iyakki-agent --lib indic::tests::real_server -- --ignored --nocapture`. Writes the whole-pipeline WAVs and prints facts.
#[test]
#[ignore]
fn real_server() {
    let Ok(base) = std::env::var("IYAKKI_INDIC_URL") else { return };
    let sentences: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("IYAKKI_INDIC_SENTENCES").unwrap()).unwrap()).unwrap();
    let out = std::path::PathBuf::from(std::env::var("IYAKKI_INDIC_OUT").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    clear_cache();
    for (lang, list) in sentences.as_object().unwrap() {
        for gender in ["female", "male"] {
            let (mut secs, mut took) = (0f32, 0f32);
            for (i, s) in list.as_array().unwrap().iter().enumerate() {
                let t = Instant::now();
                let wav = synth_wav(&base, Some(gender), Some(lang), s.as_str().unwrap(), 1.0).unwrap();
                took += t.elapsed().as_secs_f32();
                let w = voice::parse_wav(&wav).unwrap();
                let c = audio::check(&w.samples, w.rate);
                assert_eq!(c.clipped, 0, "{lang} {gender} {i}");
                let at_joins = join_steps(&w.samples);
                assert!(at_joins < 2500, "{lang} {gender} {i}: a click where pieces meet ({at_joins})");
                secs += c.seconds;
                std::fs::write(out.join(format!("pipeline_{lang}_{gender}_{}.wav", i + 1)), wav).unwrap();
                eprintln!("PIPE {lang} {gender} #{} {:.2}s peak {:.2} rms {:.1} dBFS lead {}ms trail {}ms", i + 1, c.seconds, c.peak, c.rms_db, c.leading_silence_ms, c.trailing_silence_ms);
            }
            eprintln!("PIPE-SUMMARY {lang} {gender} audio {secs:.1}s took {took:.1}s rtf {:.2}", took / secs);
        }
    }
}

/// The largest jump just before and after every stretch of at least 100 exact zeros (where pieces are joined).
fn join_steps(s: &[i16]) -> i32 {
    let mut worst = 0;
    let mut i = 0;
    while i < s.len() {
        if s[i] == 0 {
            let mut j = i;
            while j < s.len() && s[j] == 0 {
                j += 1;
            }
            if j - i >= 100 {
                if i >= 2 {
                    worst = worst.max((s[i - 1] as i32 - s[i - 2] as i32).abs());
                }
                if j + 1 < s.len() {
                    worst = worst.max((s[j + 1] as i32 - s[j] as i32).abs());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    worst
}
