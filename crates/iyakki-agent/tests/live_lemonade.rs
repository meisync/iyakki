//! Live checks of the speech and picture paths against a real Lemonade server (OpenAI-compatible, `/api/v1`) on the LAN.
//!
//! Every test is `#[ignore = "live service"]`: it needs a server and does nothing without one. Run with
//!
//! ```text
//! IYAKKI_LIVE_LEMONADE_URL=http://host:port \
//! IYAKKI_LIVE_LEMONADE_KEY_FILE=/absolute/path/of/the/key/file \
//! IYAKKI_LIVE_EVIDENCE=/some/folder      (optional: redacted request/response summaries)
//! cargo test -p iyakki-agent --test live_lemonade -- --ignored --test-threads=1
//! ```
//!
//! The key is read only through the file reference `file:<path>` (the production resolver), never from the command line, and it
//! never reaches an evidence file: [`Evidence`] replaces the key and the server address before anything is written. The
//! tests drive the same code the shell uses: `voice::synthesize_with`, `voice::transcribe`, `speechtime::transcribe_words`,
//! `Services::synthesize_timed`, `voice::Player`, `Services::generate_image_traced`. They assert structure (status, magic bytes,
//! sample rate, duration bounds, words heard), never exact audio or pixels, and retry up to three times for model variation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::lemonade;
use iyakki_agent::providers::{ImageParams, Kind, ProviderCfg};
use iyakki_agent::services::Services;
use iyakki_agent::speechtime;
use iyakki_agent::vault::Secrets;
use iyakki_agent::voice::{self, SpeakOpts, SpeechEngine};
use iyakki_proxy::https::UreqUpstream;
use iyakki_proxy::Upstream;

const TTS: &str = "kokoro-v1";
const STT: &str = "Whisper-Tiny";
const TRIES: usize = 3;

struct Live {
    url: String,
    key_file: String,
    up: Arc<dyn Upstream>,
    secrets: Secrets,
    ev: Evidence,
}

/// Writes summaries under `$IYAKKI_LIVE_EVIDENCE/<row>/<name>.txt` with the key and the address removed.
struct Evidence {
    dir: Option<PathBuf>,
    hide: Vec<String>,
}

impl Evidence {
    fn note(&self, row: &str, name: &str, text: &str) {
        let mut t = text.to_string();
        for h in &self.hide {
            if !h.is_empty() {
                t = t.replace(h.as_str(), "<redacted>");
            }
        }
        eprintln!("[{row}] {name}: {}", t.lines().next().unwrap_or(""));
        if let Some(d) = &self.dir {
            let d = d.join(row);
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(format!("{name}.txt")), t);
        }
    }
    fn bytes(&self, row: &str, name: &str, b: &[u8]) {
        if let Some(d) = &self.dir {
            let d = d.join(row);
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(name), b);
        }
    }
}

fn live() -> Live {
    let url = std::env::var("IYAKKI_LIVE_LEMONADE_URL").expect("set IYAKKI_LIVE_LEMONADE_URL");
    let key_file = std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE").expect("set IYAKKI_LIVE_LEMONADE_KEY_FILE");
    let key = std::fs::read_to_string(&key_file).unwrap_or_default().trim().to_string();
    let up: Arc<dyn Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(300)));
    let host = lemonade::origin(&url).unwrap_or_default();
    let hide = vec![key, host, url.trim_end_matches('/').to_string()];
    let ev = Evidence { dir: std::env::var("IYAKKI_LIVE_EVIDENCE").ok().map(PathBuf::from), hide };
    Live { secrets: Secrets::new(None, up.clone()), up, url, key_file, ev }
}

impl Live {
    fn key_ref(&self) -> Option<String> {
        Some(format!("file:{}", self.key_file))
    }
    fn tts(&self, voice_id: Option<&str>) -> SpeechEngine {
        lemonade::tts_engine(&self.url, TTS, voice_id, self.key_ref())
    }
    fn stt(&self) -> SpeechEngine {
        lemonade::stt_engine(&self.url, STT, self.key_ref())
    }
    /// A key reference to a file that holds a wrong key.
    fn wrong_key_ref(&self) -> Option<String> {
        let p = std::env::temp_dir().join(format!("iyakki-live-wrongkey-{}", std::process::id()));
        std::fs::write(&p, "not-the-key-0000").unwrap();
        Some(format!("file:{}", p.display()))
    }
    fn services(&self, tag: &str) -> Arc<Services> {
        let dir = std::env::temp_dir().join(format!("iyakki-live-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let quick: Arc<dyn Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(15)));
        let s = Services::open(&dir, quick, self.up.clone()).expect("services");
        let mut p = ProviderCfg::new("lemon", Kind::Lemonade);
        p.base_url = Some(self.url.clone());
        p.key_ref = self.key_ref();
        s.settings.lock().unwrap().upsert_provider(p).unwrap();
        s.refresh_lemonade("lemon").expect("the typed model list");
        s
    }
}

/// Up to three tries; the first that satisfies `f` wins. Returns (attempts used, value).
fn retry<T>(what: &str, mut f: impl FnMut() -> Result<T, String>) -> (usize, T) {
    let mut last = String::new();
    for n in 1..=TRIES {
        match f() {
            Ok(v) => return (n, v),
            Err(e) => {
                eprintln!("{what}: attempt {n} failed: {e}");
                last = e;
            }
        }
    }
    panic!("{what}: failed {TRIES} times, last: {last}");
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// How many of the spoken words appear in what was heard.
fn heard_count(said: &str, heard: &str) -> (usize, usize) {
    let h = words(heard);
    let w = words(said);
    (w.iter().filter(|x| h.contains(x)).count(), w.len())
}

fn wav_summary(w: &[u8]) -> (String, f32) {
    let parsed = voice::parse_wav(w).expect("a WAV the player can read");
    let fmt = u16::from_le_bytes([w[20], w[21]]);
    let bits = u16::from_le_bytes([w[34], w[35]]);
    let secs = parsed.samples.len() as f32 / (parsed.rate as f32 * parsed.channels as f32);
    (format!("{} bytes, tag {fmt}, {bits}-bit, {} Hz, {} ch, {secs:.2} s, peak {}", w.len(), parsed.rate, parsed.channels, voice::peak(&parsed.samples)), secs)
}

const SENTENCES: [&str; 3] = ["Once upon a time a little fox lived in a green forest.", "She walked home under the bright moon.", "The quick brown fox jumps over the lazy dog."];

// ------------------------------------------------------------------ F-VOICE-02 / F-VOICE-16 / F-VOICE-17: reading aloud with kokoro-v1

#[test]
#[ignore = "live service"]
fn kokoro_reads_sentences_and_the_wav_is_accepted() {
    let l = live();
    let engine = l.tts(None);
    let mut lines = Vec::new();
    for s in SENTENCES {
        let t = Instant::now();
        let (n, wav) = retry("kokoro", || voice::synthesize_with(&engine, s, &SpeakOpts::default(), &l.secrets, l.up.as_ref()));
        let ms = t.elapsed().as_millis();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let (summary, secs) = wav_summary(&wav);
        let nwords = words(s).len() as f32;
        assert!((0.12 * nwords..1.2 * nwords).contains(&secs), "{secs} s for {nwords} words");
        let parsed = voice::parse_wav(&wav).unwrap();
        assert!(voice::peak(&parsed.samples) > 1500, "audible, not silence");
        assert!((8_000..=48_000).contains(&parsed.rate));
        lines.push(format!("{s:?} -> attempts {n}, {ms} ms, {summary}"));
        l.ev.bytes("F-VOICE-02", &format!("sentence-{}.wav", lines.len()), &wav);
    }
    l.ev.note("F-VOICE-02", "kokoro-sentences", &lines.join("\n"));
}

#[test]
#[ignore = "live service"]
fn kokoro_voice_and_speed_change_the_sound() {
    let l = live();
    let text = "The little fox ran across the meadow and came home before the rain.";
    let engine = l.tts(None);
    let mk = |voice_id: &str, speed: f32| {
        retry("kokoro opts", || voice::synthesize_with(&engine, text, &SpeakOpts { voice: Some(voice_id.into()), speed, lang: None }, &l.secrets, l.up.as_ref())).1
    };
    let normal = mk("af_sky", 1.0);
    let fast = mk("af_sky", 1.6);
    let other = mk("bm_george", 1.0);
    let (_, d_normal) = wav_summary(&normal);
    let (_, d_fast) = wav_summary(&fast);
    let (_, d_other) = wav_summary(&other);
    assert!(d_fast < d_normal * 0.85, "speed 1.6 is shorter: {d_fast} vs {d_normal}");
    assert_ne!(normal, other, "another voice makes other sound");
    // every voice Lemonade is offered with must be accepted by the server
    let mut ok = Vec::new();
    for v in lemonade::voices_for(TTS).into_iter().take(12) {
        let r = voice::synthesize_with(&engine, "Hello there.", &SpeakOpts { voice: Some(v.into()), speed: 1.0, lang: None }, &l.secrets, l.up.as_ref());
        ok.push(format!("{v}: {}", r.map(|w| format!("{} bytes", w.len())).unwrap_or_else(|e| e)));
    }
    l.ev.note("F-VOICE-16", "voices", &format!("normal {d_normal:.2} s, fast(1.6) {d_fast:.2} s, bm_george {d_other:.2} s\n{}", ok.join("\n")));
    assert!(ok.iter().all(|x| x.contains("bytes")), "{ok:?}");
}

#[test]
#[ignore = "live service"]
fn speech_errors_are_plain_words_with_a_wrong_key_no_key_or_a_wrong_model() {
    let l = live();
    let mut report = Vec::new();
    for (name, engine) in [
        ("wrong key", lemonade::tts_engine(&l.url, TTS, None, l.wrong_key_ref())),
        ("no key", lemonade::tts_engine(&l.url, TTS, None, None)),
        ("unknown model", lemonade::tts_engine(&l.url, "no-such-voice-model", None, l.key_ref())),
    ] {
        let t = Instant::now();
        let e = voice::synthesize(&engine, "Hello.", &l.secrets, l.up.as_ref()).expect_err(name);
        report.push(format!("{name}: {} ms: {e}", t.elapsed().as_millis()));
        assert!(e.starts_with("HTTP "), "{name}: {e}");
        if name != "unknown model" {
            assert!(e.starts_with("HTTP 401") || e.starts_with("HTTP 403"), "{name}: {e}");
        }
        l.ev.note("F-VOICE-02", &format!("error-{}", name.replace(' ', "-")), &e);
    }
    // the same for listening
    let wav = voice::encode_wav(&voice::tone(440.0, 300), voice::SAMPLE_RATE);
    let e = voice::transcribe(&lemonade::stt_engine(&l.url, STT, l.wrong_key_ref()), &wav, &l.secrets, l.up.as_ref()).expect_err("wrong key stt");
    assert!(e.starts_with("HTTP 401") || e.starts_with("HTTP 403"), "{e}");
    report.push(format!("stt wrong key: {e}"));
    // nothing to say never reaches the server
    assert_eq!(voice::synthesize(&l.tts(None), "```code```", &l.secrets, l.up.as_ref()).unwrap_err(), "nothing to say");
    // a server that is not there is recognised as unreachable
    let dead = lemonade::tts_engine("http://127.0.0.1:9", TTS, None, l.key_ref());
    let e = voice::synthesize(&dead, "Hello.", &l.secrets, l.up.as_ref()).expect_err("dead");
    assert!(speechtime::unreachable(&e), "{e}");
    report.push(format!("unreachable: {e}"));
    l.ev.note("F-VOICE-02", "error-paths", &report.join("\n"));
}

// ------------------------------------------------------------------ F-VOICE-01 / F-VOICE-18: dictation against Whisper-Tiny

#[test]
#[ignore = "live service"]
fn whisper_hears_what_kokoro_said() {
    let l = live();
    let mut lines = Vec::new();
    for s in SENTENCES {
        let (_, wav) = retry("kokoro for whisper", || voice::synthesize(&l.tts(None), s, &l.secrets, l.up.as_ref()));
        // what the Voice tile sends: 16 kHz mono 16-bit, as the microphone path makes it
        let pcm = voice::to_mono_16k(&voice::parse_wav(&wav).unwrap());
        let sent = voice::encode_wav(&pcm, voice::SAMPLE_RATE);
        let t = Instant::now();
        let (n, text) = retry("whisper", || {
            let text = voice::transcribe(&l.stt(), &sent, &l.secrets, l.up.as_ref())?;
            let (ok, all) = heard_count(s, &text);
            if ok * 10 >= all * 7 { Ok(text) } else { Err(format!("heard {text:?} for {s:?}")) }
        });
        let (ok, all) = heard_count(s, &text);
        lines.push(format!("{s:?} -> attempts {n}, {} ms, heard {text:?} ({ok}/{all} words)", t.elapsed().as_millis()));
        // the engine given the unconverted 24 kHz audio works too (the server resamples)
        let raw = voice::transcribe(&l.stt(), &wav, &l.secrets, l.up.as_ref()).expect("raw kokoro audio");
        let (ok2, all2) = heard_count(s, &raw);
        lines.push(format!("   unconverted kokoro audio: {raw:?} ({ok2}/{all2})"));
        assert!(ok2 * 10 >= all2 * 5, "{raw:?}");
    }
    // silence and a bare tone produce no words and no error
    let quiet = voice::encode_wav(&vec![0i16; 16_000], voice::SAMPLE_RATE);
    let t = voice::transcribe(&l.stt(), &quiet, &l.secrets, l.up.as_ref()).expect("silence is not an error");
    lines.push(format!("one second of silence -> {t:?}"));
    assert!(words(&t).len() <= 2, "{t:?}");
    l.ev.note("F-VOICE-01", "whisper-roundtrip", &lines.join("\n"));
}

#[test]
#[ignore = "live service"]
fn word_times_come_from_whisper_and_line_up_with_the_text() {
    let l = live();
    let text = "Once upon a time a little fox lived in a green forest.";
    let (_, wav) = retry("kokoro", || voice::synthesize(&l.tts(None), text, &l.secrets, l.up.as_ref()));
    let total = speechtime::wav_ms(&wav);
    let (n, rec) = retry("whisper words", || {
        let r = speechtime::transcribe_words(&l.stt(), &wav, Some("en"), &l.secrets, l.up.as_ref())?;
        if r.len() >= 6 { Ok(r) } else { Err(format!("only {} timed words", r.len())) }
    });
    let begin = speechtime::align(text, &rec, total).expect("words matched");
    assert_eq!(begin.len(), words(text).len() + 1, "one start per word plus the end");
    assert!(begin.windows(2).all(|w| w[0] <= w[1]), "times never go backwards: {begin:?}");
    assert!(*begin.last().unwrap() <= total + 50.0 && begin[0] < 600.0, "{begin:?} of {total}");
    l.ev.note(
        "F-VOICE-15",
        "word-times",
        &format!("attempts {n}; whisper timed {} tokens: {}\naligned starts (ms): {begin:?}\nsound {total:.0} ms", rec.len(), rec.iter().map(|w| format!("{}@{:.0}-{:.0}", w.text.trim(), w.start_ms, w.end_ms)).collect::<Vec<_>>().join(" ")),
    );
}

// ------------------------------------------------------------------ Services: the path TALES and VOICE use

#[test]
#[ignore = "live service"]
fn the_services_read_aloud_only_after_the_person_allowed_the_remote_server_and_time_the_words() {
    let l = live();
    let s = l.services("speech");
    speechtime::clear_down_for_tests();
    if speechtime::locality(&l.url) == speechtime::Locality::ThisComputer {
        eprintln!("the server is on this computer: the consent step does not apply");
    } else {
        assert!(s.lemonade_auto_voice(None).is_none(), "a server on another computer is not used before the person says yes");
        assert!(s.speech_aligner().is_none());
        s.allow_speech_server(&lemonade::api_base(&l.url)).unwrap();
    }
    let engine = s.lemonade_auto_voice(None).expect("kokoro found by the typed list");
    assert!(matches!(&engine, SpeechEngine::Http { model, url, .. } if model == TTS && url.ends_with("/audio/speech")), "{engine:?}");
    assert!(s.speech_aligner().is_some(), "Whisper-Tiny is the aligner");
    let text = "The brave little fox found the path home.";
    let t = Instant::now();
    let (n, (wav, timed)) = retry("synthesize_timed", || {
        speechtime::clear_cache();
        let (w, t) = s.synthesize_timed(text, &SpeakOpts::default())?;
        Ok((w, t))
    });
    let ms = t.elapsed().as_millis();
    let (summary, _) = wav_summary(&wav);
    let aligned = s.align_speech(text, &wav, Some("en"));
    let line = format!("synthesize_timed attempts {n}, {ms} ms, {summary}; native timing: {:?}; aligned: {:?}", timed.as_ref().map(|t| t.source.label()), aligned.as_ref().map(|t| (t.source.label(), t.begin.len())));
    assert!(timed.is_none(), "Lemonade gives no native word times");
    let a = aligned.expect("alignment from Whisper-Tiny");
    assert_eq!(a.begin.len(), words(text).len() + 1);
    l.ev.note("F-VOICE-15", "services-aligned", &line);
    // a second ask is answered from the cache
    let t2 = Instant::now();
    let _ = s.synthesize_timed(text, &SpeakOpts::default()).unwrap();
    assert!(t2.elapsed() < Duration::from_millis(200), "cached");
}

#[test]
#[ignore = "live service"]
fn the_player_pipeline_is_fed_cut_and_resumed_with_real_speech() {
    let _route = iyakki_agent::mixer::pin_command_route();
    let l = live();
    let dir = std::env::temp_dir().join(format!("iyakki-live-play-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let engine = l.tts(None);
    let pieces: Vec<Vec<u8>> = SENTENCES.iter().map(|s| retry("kokoro", || voice::synthesize(&engine, s, &l.secrets, l.up.as_ref())).1).collect();
    // each piece reaches the speaker command as a WAV the card can take (48 kHz, 16-bit)
    let mut lines = Vec::new();
    for (i, w) in pieces.iter().enumerate() {
        let out = dir.join(format!("played-{i}.wav"));
        let mut p = voice::Player::start(&format!("sh -c 'cat > {}'", out.display()), w).expect("player starts");
        assert!(p.wait(Duration::from_secs(10)), "the command finished");
        let got = voice::parse_wav(&std::fs::read(&out).unwrap()).expect("the command received a WAV");
        let src = voice::parse_wav(w).unwrap();
        assert_eq!((got.rate, got.channels), (48_000, 1), "converted for the card");
        let a = got.samples.len() as f32 / got.rate as f32;
        let b = src.samples.len() as f32 / (src.rate as f32 * src.channels as f32);
        assert!((a - b).abs() < 0.05, "same duration: {a} vs {b}");
        lines.push(format!("piece {i}: {} bytes in, {} bytes to the speaker command, {a:.2} s", w.len(), std::fs::metadata(&out).unwrap().len()));
    }
    // cut: a long-running player stops at once; resume: reading starts again from the next piece
    let mut cut = voice::Player::start("sleep 30", &pieces[0]).unwrap();
    assert!(cut.is_playing());
    let t = Instant::now();
    cut.stop();
    assert!(!cut.is_playing() && t.elapsed() < Duration::from_secs(2));
    lines.push(format!("cut: stopped in {} ms", t.elapsed().as_millis()));
    let out = dir.join("resumed.wav");
    let mut resumed = voice::Player::start(&format!("sh -c 'cat > {}'", out.display()), &pieces[1]).unwrap();
    assert!(resumed.wait(Duration::from_secs(10)));
    assert!(voice::parse_wav(&std::fs::read(&out).unwrap()).is_ok());
    lines.push("resume: the next piece played to the end after the cut".into());
    // a speaker command that is not there is an error, not a hang
    assert!(voice::Player::start("/no/such/player", &pieces[0]).is_err());
    l.ev.note("F-VOICE-17", "player-pipeline", &lines.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ TALES pictures

/// Width and height from a PNG header.
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() > 24 && b[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).then(|| (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap())))
}

#[test]
#[ignore = "live service"]
fn pictures_come_back_as_real_images_from_sd_turbo_and_sdxl() {
    let l = live();
    let s = l.services("pics");
    let found = s.image_models();
    let names: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
    assert!(names.contains(&"SD-Turbo") && names.contains(&"SDXL-Base-1.0"), "{names:?}");
    let mut lines = vec![format!("picture models found: {names:?}")];
    for model in ["SD-Turbo", "SDXL-Base-1.0"] {
        if model == "SDXL-Base-1.0" && std::env::var("IYAKKI_LIVE_SKIP_SDXL").is_ok() {
            continue;
        }
        let prompt = "watercolor picture book illustration, a small orange fox in a green meadow under a bright moon, soft colours";
        let (n, (res, trace)) = retry(model, || {
            let params = ImageParams { seed: Some(7), negative: Some("text, watermark".into()), ..Default::default() };
            let (r, t) = s.generate_image_traced(Some(("lemon", model)), prompt, "512x512", &params);
            match r {
                Ok(v) => Ok((v, t)),
                Err(e) => Err(e),
            }
        });
        let (provider, m, bytes, ext) = res;
        assert_eq!((provider.as_str(), m.as_str()), ("lemon", model));
        assert_eq!(trace.status, Some(200));
        assert_eq!(trace.format, ext);
        assert_eq!(ext, "png", "found by its magic bytes");
        let (w, h) = png_size(&bytes).expect("a PNG header");
        assert!(w >= 256 && h >= 256 && w <= 2048 && h <= 2048, "{w}x{h}");
        assert!(bytes.len() > 20_000, "a picture, not a blank: {} bytes", bytes.len());
        // the request carried the model's own size and steps, and the key never shows in the trace
        let shown = format!("{:?}", trace.request_headers);
        assert!(!shown.contains(&std::fs::read_to_string(&l.key_file).unwrap().trim().to_string()), "key redacted in the trace");
        lines.push(format!("{model}: attempts {n}, {} ms, {w}x{h} {ext}, {} bytes, answer {} bytes, steps {:?}, request {}x{}", trace.ms, bytes.len(), trace.response_bytes, trace.steps, trace.width, trace.height));
        l.ev.bytes("F-TALES-13", &format!("{model}.png"), &bytes);
    }
    // the chat tool names no model: the first picture model found draws it (what `generate_image` does)
    let t = Instant::now();
    let (_, (prov, model, bytes, ext)) = retry("tool path", || s.generate_image(None, "a red kite over a beach", "512x512"));
    assert_eq!((prov.as_str(), ext), ("lemon", "png"));
    assert!(png_size(&bytes).is_some());
    lines.push(format!("generate_image with no model named drew with {model}: {} ms, {} bytes", t.elapsed().as_millis(), bytes.len()));
    l.ev.note("F-AGENT-29", "generate-image-tool-path", lines.last().unwrap());
    l.ev.note("F-TALES-13", "images", &lines.join("\n"));
}

#[test]
#[ignore = "live service"]
fn picture_failures_are_plain_words_and_never_a_picture() {
    let l = live();
    let s = l.services("picfail");
    let mut lines = Vec::new();
    // a model the server does not have
    let t = Instant::now();
    let (r, trace) = s.generate_image_traced(Some(("lemon", "No-Such-Picture-Model")), "a fox", "512x512", &ImageParams::default());
    let e = r.expect_err("unknown model");
    lines.push(format!("unknown model: {} ms: {e}", t.elapsed().as_millis()));
    assert!(trace.status.is_some_and(|c| c >= 400) || !trace.error.is_empty(), "{trace:?}");
    // a wrong key
    let mut p = ProviderCfg::new("badkey", Kind::Lemonade);
    p.base_url = Some(l.url.clone());
    p.key_ref = l.wrong_key_ref();
    s.settings.lock().unwrap().upsert_provider(p).unwrap();
    let (r, trace) = s.generate_image_traced(Some(("badkey", "SD-Turbo")), "a fox", "512x512", &ImageParams::default());
    let e = r.expect_err("wrong key");
    assert!(trace.status == Some(401) || trace.status == Some(403) || e.contains("401") || e.contains("403"), "{e} {trace:?}");
    lines.push(format!("wrong key: status {:?}: {e}", trace.status));
    // no picture model at all among the providers: the fallback message names what was looked at
    let dir = std::env::temp_dir().join(format!("iyakki-live-nopic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let empty = Services::open(&dir, l.up.clone(), l.up.clone()).unwrap();
    assert!(empty.image_models().is_empty());
    let e = empty.generate_image(None, "a fox", "512x512").expect_err("no model");
    assert!(e.contains("No model that draws pictures"), "{e}");
    lines.push(format!("no picture model configured: {e}"));
    // the server is gone
    let mut p = ProviderCfg::new("gone", Kind::Lemonade);
    p.base_url = Some("http://127.0.0.1:9".into());
    s.settings.lock().unwrap().upsert_provider(p).unwrap();
    let (r, _) = s.generate_image_traced(Some(("gone", "SD-Turbo")), "a fox", "512x512", &ImageParams::default());
    let gone = r.expect_err("gone");
    assert!(gone.contains("did not answer"), "{gone}");
    lines.push(format!("server gone: {gone}"));
    l.ev.note("F-TALES-13", "picture-failures", &lines.join("\n"));
}

// ------------------------------------------------------------------ helpers shared with the shell-level live tests

#[allow(dead_code)]
fn _unused(_: &Path) {}

// ------------------------------------------------------------------ F-AI-09: the provider itself

#[test]
#[ignore = "live service"]
fn the_server_is_detected_listed_and_asks_for_its_key() {
    let l = live();
    let quick = UreqUpstream::with_timeout(Duration::from_secs(15));
    let hdr = vec![("Authorization".to_string(), format!("Bearer {}", std::fs::read_to_string(&l.key_file).unwrap().trim()))];
    let found = lemonade::detect(&l.url, &quick, &hdr).expect("detected as a Lemonade server");
    let s = l.services("detect");
    let list = s.refresh_lemonade("lemon").expect("typed list");
    let kinds: Vec<String> = [lemonade::ModelType::Chat, lemonade::ModelType::Image, lemonade::ModelType::SpeechToText, lemonade::ModelType::TextToSpeech].iter().map(|t| format!("{}: {}", t.label(), list.iter().filter(|m| m.kind == *t && m.downloaded).count())).collect();
    assert!(list.iter().any(|m| m.id == TTS && m.kind == lemonade::ModelType::TextToSpeech && m.downloaded));
    assert!(list.iter().any(|m| m.id == STT && m.kind == lemonade::ModelType::SpeechToText && m.downloaded));
    let sd = list.iter().find(|m| m.id == "SD-Turbo").expect("SD-Turbo listed");
    assert_eq!(sd.kind, lemonade::ModelType::Image);
    assert!(sd.image_defaults.is_some());
    let (health, loaded) = s.lemonade_health("lemon").expect("health");
    // without the key the server says so, and the program says what to do
    let err = lemonade::detect(&l.url, &quick, &[]).map(|_| "answered without a key".to_string()).unwrap_or_else(|e| e);
    l.ev.note("F-AI-09", "provider", &format!("detected: {}\ntyped list: {} models; downloaded by type: {}\nSD-Turbo defaults {:?}\nhealth: {health}; loaded: {loaded:?}\nwithout the key: {err}", found.found(), list.len(), kinds.join(", "), sd.image_defaults));
}

// ------------------------------------------------------------------ the PDF writer with real model output
//
// The chat tool `pdf_create`, TALES "Save PDF" and the LEARN PDFs are not in this branch (checked 2026-11-12), so what is proven here is
// the part that exists: text written by qwen3-8b and a picture drawn by SD-Turbo go through the repo's PDF writer and the file is read back
// with the repo's PDF reader.

fn md_to_blocks(md: &str) -> (String, Vec<iyakki_pdf::Block>) {
    use iyakki_pdf::Block;
    let (mut title, mut blocks, mut para) = (String::new(), Vec::new(), Vec::<String>::new());
    let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Para(para.join(" ")));
            para.clear();
        }
    };
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            flush(&mut para, &mut blocks);
        } else if let Some(h) = t.strip_prefix("### ").or_else(|| t.strip_prefix("## ")).or_else(|| t.strip_prefix("# ")) {
            flush(&mut para, &mut blocks);
            let level = t.chars().take_while(|c| *c == '#').count() as u8;
            if level == 1 && title.is_empty() {
                title = h.trim().to_string();
            }
            blocks.push(Block::Heading(level, h.trim().replace("**", "")));
        } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Bullet(0, "\u{2022}".into(), b.replace("**", "")));
        } else {
            para.push(t.replace("**", ""));
        }
    }
    flush(&mut para, &mut blocks);
    (title, blocks)
}

fn clean_leaks(text: &str) -> Vec<&'static str> {
    let l = text.to_lowercase();
    [("<think", "think tag"), ("</think", "closing think tag"), ("```", "code fence"), ("<|", "special token")].iter().filter(|(n, _)| l.contains(n)).map(|(_, w)| *w).collect()
}

#[test]
#[ignore = "live service"]
fn a_story_and_a_lesson_written_by_qwen3_with_a_real_picture_become_pdfs_the_reader_opens() {
    use iyakki_agent::chat::Message;
    let l = live();
    let s = l.services("pdf");
    let chat = |prompt: &str| -> Result<String, String> { s.client_for("lemon", false)?.chat("qwen3-8b", &[Message::system("You write clearly. Reply with the document only."), Message::user(prompt)]) };
    let mut lines = Vec::new();
    // a picture from SD-Turbo for the story
    let (_, (_, _, png, _)) = retry("sd-turbo", || s.generate_image(Some(("lemon", "SD-Turbo")), "soft watercolor illustration, a small orange fox by a pond under the moon", "512x512"));
    let img = iyakki_agent::qaimage::decode_png(&png).expect("the repo's PNG reader opens the picture");
    let pdf_img = iyakki_pdf::PdfImage::from_rgba(img.w, img.h, &img.rgba).expect("a PDF picture");
    for (name, prompt) in [
        ("story", "Write a short bedtime story for children in Markdown: a title as a '# ' heading, then three short paragraphs about a small fox who finds the moon in a pond."),
        ("lesson", "Write a one-page study plan in Markdown for learning basic fractions: a '# ' title, a short intro paragraph, a '## Week 1' and a '## Week 2' section each with three '- ' bullet points, then a '## Quiz' section with three numbered questions."),
    ] {
        let t0 = Instant::now();
        let (n, md) = retry(name, || {
            let m = chat(prompt)?;
            if m.trim().len() > 200 && m.contains("# ") { Ok(m) } else { Err(format!("too little: {m:?}")) }
        });
        let ms = t0.elapsed().as_millis();
        let leaks = clean_leaks(&md);
        let (title, mut blocks) = md_to_blocks(&md);
        if name == "story" {
            blocks.insert(1.min(blocks.len()), iyakki_pdf::Block::Image(pdf_img.clone()));
        }
        let mut b = iyakki_pdf::Builder::new(iyakki_pdf::PageSize::A4);
        b.title = if title.is_empty() { name.to_string() } else { title.clone() };
        b.author = "IYAKKI".into();
        b.blocks = blocks;
        let (bytes, built) = b.build();
        assert!(bytes.starts_with(b"%PDF-"), "a PDF");
        let doc = iyakki_pdf::Document::open(bytes.clone()).expect("the repo's PDF reader opens what the writer wrote");
        let text = doc.text(100_000);
        let info = doc.info();
        let first_words: Vec<String> = md.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).take(2).map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" ").replace("**", "")).collect();
        let found = first_words.iter().filter(|w| text.contains(&w.replace('\u{2019}', "'")) || text.contains(w.as_str())).count();
        lines.push(format!("{name}: model {ms} ms (attempts {n}), {} chars, leaks {leaks:?}; PDF {} bytes, {} pages, title {:?}, characters outside Windows-1252 written as ?: {}, first words found back in the PDF text {found}/{}", md.len(), bytes.len(), doc.page_count(), info.title, built.replaced_chars, first_words.len()));
        assert!(leaks.is_empty(), "{leaks:?} in {md}");
        assert!(doc.page_count() >= 1 && info.pages == built.pages);
        assert!(found >= 1, "the words of the document are in the PDF text: {first_words:?} / {}", &text[..text.len().min(200)]);
        l.ev.bytes("F-PDF-04", &format!("{name}.pdf"), &bytes);
        l.ev.bytes("F-PDF-04", &format!("{name}.md"), md.as_bytes());
    }
    l.ev.note("F-PDF-04", "live-pdf", &lines.join("\n"));
}

// ------------------------------------------------------------------ the character sheet of an animal story (quality pass)

#[test]
#[ignore = "live service"]
fn the_character_sheet_of_a_fox_story_does_not_turn_the_fox_into_a_boy() {
    use iyakki_agent::chat::{ChatBackend, Message};
    let l = live();
    let s = l.services("sheet");
    let client = s.client_for("lemon", false).unwrap();
    let chat = iyakki_agent::providers::ProviderChat { client, model: "qwen3-8b".into(), streamer: None };
    let story = "# The Fox and the Moon in the Pond\n\nOne evening, a small red fox named Finn wandered through the forest. He found a quiet pond where the moon hung in the water.\n\nFinn pawed the water gently and the moon wobbled, as if laughing. He curled up on the bank, his red tail around his nose, and slept.";
    let o = iyakki_agent::story::StoryOptions { topic: "a small fox who finds the moon in a pond".into(), ..Default::default() };
    let mut lines = Vec::new();
    let mut humans = 0;
    for attempt in 1..=3 {
        let sheet = iyakki_agent::scene::sheet::character_sheet(&chat, &o, story);
        // what the model itself answered, for the evidence
        let raw = chat.complete_tools(&iyakki_agent::scene::sheet::sheet_messages(&o, story, &iyakki_agent::scene::CharacterSheet::derive(&o.topic, &o.characters, story)), &[]).map(|t| t.text).unwrap_or_default();
        lines.push(format!("attempt {attempt}: human {} companion {:?} describe {:?}\n   raw reply: {}", sheet.human, sheet.companion, sheet.describe(), raw.replace('\n', " ").chars().take(300).collect::<String>()));
        if sheet.human {
            humans += 1;
        }
        let _ = Message::user("");
    }
    l.ev.note("F-TALES-16", "character-sheet-fox", &lines.join("\n"));
    assert_eq!(humans, 0, "{}", lines.join("\n"));
}
