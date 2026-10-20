//! Indic neural voices: AI4Bharat Indic-TTS (FastPitch + HiFi-GAN) as a local text-to-speech engine for Tamil, Telugu, Malayalam,
//! Kannada, Hindi, Bengali, Marathi and the other Indic languages, plus Indian English.
//!
//! **Architecture.** The two neural networks run in a small OpenAI-compatible server (`dist/tts-indic/server.py`: numpy and
//! onnxruntime, on this computer, a PC, or a k3s service, see `docs/VOICE.md`). Everything that is language knowledge lives here, in
//! Rust, and is the same wherever the server runs: the text frontend (`text`, `numbers`), the cutting into pieces, the choice of
//! voice for the story's language, loudness and joins (`audio`), the model files on the device (`models`) and the fallback to the next
//! engine when a language is missing. The reader's engine setting is `Host { engine: "indic-tts", program: <server address>,
//! voice: <female | male | ta-female | ...> }` (see `hostengines`).
//!
//! The models never leave the device that holds them: IYAKKI sends only the text of one piece at a time, to the address in the
//! setting, and that address is shown wherever the engine is.

pub mod audio;
pub mod models;
pub mod numbers;
pub mod service;
pub mod text;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::voice;

/// Where the server listens when nothing else is set.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8189";
/// The engine's id in the setting.
pub const ENGINE_ID: &str = "indic-tts";
/// One request of one piece may take this long.
pub const PIECE_TIME: Duration = Duration::from_secs(90);
/// A whole reading request (many pieces) may take this long.
pub const TOTAL_TIME: Duration = Duration::from_secs(600);

/// What the Indic voices page shows about where the voices come from and under what terms. Kept short and exact: the same facts,
/// with their sources, are in docs/LICENSES.md and docs/VOICE.md.
pub const LICENCE_NOTICE: &str = "AI4Bharat Indic-TTS (github.com/AI4Bharat/Indic-TTS): the code is MIT-licensed. AI4Bharat states no licence for the released model files (the release page and the paper give none), and the models were trained on the IIT Madras Indic TTS database, whose end-user licence (iitm.ac.in/donlab/indictts/downloads/license.pdf) allows copying, changing and free distribution with the notice kept but forbids a buyer of a derivative work from selling or sub-licensing it. IYAKKI ships none of it and downloads nothing by itself; check these terms before you use or hand on the voices.";
pub const SOURCE_LINK: &str = "https://github.com/AI4Bharat/Indic-TTS/releases/tag/v1-checkpoints-release";

pub struct LangInfo {
    pub code: &'static str,
    pub name: &'static str,
    pub native: &'static str,
    /// Size of AI4Bharat's checkpoint archive for it (what the exporter downloads once, on a PC).
    pub zip_bytes: u64,
}

/// The 13 Indian languages of the AI4Bharat release and the Indian English voice.
pub const LANGS: [LangInfo; 14] = [
    LangInfo { code: "ta", name: "Tamil", native: "தமிழ்", zip_bytes: 1_520_799_925 },
    LangInfo { code: "te", name: "Telugu", native: "తెలుగు", zip_bytes: 1_513_806_634 },
    LangInfo { code: "ml", name: "Malayalam", native: "മലയാളം", zip_bytes: 1_514_083_272 },
    LangInfo { code: "kn", name: "Kannada", native: "ಕನ್ನಡ", zip_bytes: 1_519_176_042 },
    LangInfo { code: "hi", name: "Hindi", native: "हिन्दी", zip_bytes: 1_519_601_621 },
    LangInfo { code: "bn", name: "Bengali", native: "বাংলা", zip_bytes: 1_512_955_815 },
    LangInfo { code: "mr", name: "Marathi", native: "मराठी", zip_bytes: 1_514_815_493 },
    LangInfo { code: "en", name: "English (Indian)", native: "English", zip_bytes: 1_533_476_097 },
    LangInfo { code: "gu", name: "Gujarati", native: "ગુજરાતી", zip_bytes: 1_518_484_594 },
    LangInfo { code: "pa", name: "Punjabi", native: "ਪੰਜਾਬੀ", zip_bytes: 1_532_171_142 },
    LangInfo { code: "or", name: "Odia", native: "ଓଡ଼ିଆ", zip_bytes: 1_516_409_500 },
    LangInfo { code: "as", name: "Assamese", native: "অসমীয়া", zip_bytes: 1_512_387_643 },
    LangInfo { code: "brx", name: "Bodo", native: "बड़ो", zip_bytes: 1_522_282_712 },
    LangInfo { code: "mni", name: "Manipuri", native: "মৈতৈলোন্", zip_bytes: 1_518_195_233 },
];

pub fn lang_info(code: &str) -> Option<&'static LangInfo> {
    LANGS.iter().find(|l| l.code == code)
}

/// The model's language code for a story language tag (`ta`, `ta-IN`, `TA_in`, `tam`, `Tamil`).
pub fn lang_code(tag: &str) -> Option<&'static str> {
    let t = tag.trim().to_ascii_lowercase().replace('_', "-");
    let p = t.split('-').next().unwrap_or("");
    if let Some(l) = LANGS.iter().find(|l| l.code == p) {
        return Some(l.code);
    }
    let by_name = LANGS.iter().find(|l| l.name.eq_ignore_ascii_case(&t) || l.name.split(' ').next().is_some_and(|n| n.eq_ignore_ascii_case(&t)));
    if let Some(l) = by_name {
        return Some(l.code);
    }
    Some(match p {
        "tam" => "ta",
        "tel" => "te",
        "mal" => "ml",
        "kan" => "kn",
        "hin" => "hi",
        "ben" => "bn",
        "mar" => "mr",
        "eng" => "en",
        "guj" => "gu",
        "pan" => "pa",
        "ori" | "ory" => "or",
        "asm" => "as",
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Female,
    Male,
}

impl Gender {
    pub fn word(self) -> &'static str {
        match self {
            Gender::Female => "female",
            Gender::Male => "male",
        }
    }
    pub fn other(self) -> Gender {
        match self {
            Gender::Female => Gender::Male,
            Gender::Male => Gender::Female,
        }
    }
}

/// What the setting's `voice` says: `female`, `male`, `ta-male`, or nothing.
pub fn parse_setting(v: Option<&str>) -> (Option<&'static str>, Option<Gender>) {
    let v = v.map(str::trim).unwrap_or("");
    let g = |s: &str| match s {
        "female" => Some(Gender::Female),
        "male" => Some(Gender::Male),
        _ => None,
    };
    if let Some((l, gw)) = v.split_once('-') {
        if let (Some(lc), Some(gg)) = (lang_code(l), g(gw)) {
            return (Some(lc), Some(gg));
        }
    }
    (None, g(v))
}

/// A voice the server has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrvVoice {
    pub id: String,
    pub lang: String,
    pub gender: String,
}

impl SrvVoice {
    pub fn label(&self) -> String {
        let n = lang_info(&self.lang).map_or(self.lang.as_str(), |l| l.name);
        format!("{n} · {}", self.gender)
    }
}

/// Why a reading could not be made by this engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// The model for this language is not installed on the server (or at all).
    NotInstalled(String),
    /// The server did not answer.
    Unreachable(String),
    /// Anything else (an error from the server, audio that cannot be read, the time limit).
    Other(String),
}

impl Fail {
    /// One line for a person.
    pub fn line(&self) -> String {
        match self {
            Fail::NotInstalled(l) => format!("The Indic voice for {} is not installed", lang_info(l).map_or(l.as_str(), |i| i.name)),
            Fail::Unreachable(w) => format!("The Indic voice server cannot be reached ({w})"),
            Fail::Other(w) => format!("The Indic voice failed ({w})"),
        }
    }
}

/// The server, as far as this module needs it (a fake one in the tests).
pub trait Server: Send + Sync {
    fn voices(&self) -> Result<Vec<SrvVoice>, Fail>;
    /// WAV bytes for one piece.
    fn speech(&self, voice: &str, text: &str, speed: f32) -> Result<Vec<u8>, Fail>;
}

/// The real server at `base` (an address such as `http://127.0.0.1:8189`).
pub struct HttpServer {
    pub base: String,
}

impl HttpServer {
    pub fn new(base: &str) -> HttpServer {
        HttpServer { base: base.trim().trim_end_matches('/').to_string() }
    }
}

/// `http://host:port` or `https://host`, no credentials, no spaces: what may be the server's address.
pub fn validate_url(u: &str) -> Result<(), String> {
    let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("https://")).ok_or("the address starts with http:// or https://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || u.len() > 300 || u.chars().any(|c| c.is_control() || c == ' ') {
        return Err("the address is not valid (no spaces, no user name or password in it)".into());
    }
    Ok(())
}

fn upstream(t: Duration) -> iyakki_proxy::https::UreqUpstream {
    iyakki_proxy::https::UreqUpstream::with_timeout(t)
}

fn short(b: &[u8]) -> String {
    String::from_utf8_lossy(b).chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim().to_string()
}

impl Server for HttpServer {
    fn voices(&self) -> Result<Vec<SrvVoice>, Fail> {
        use iyakki_proxy::Upstream;
        let r = upstream(Duration::from_secs(8)).request("GET", &format!("{}/v1/voices", self.base), &[], &[]).map_err(|e| Fail::Unreachable(short(e.as_bytes())))?;
        if !(200..300).contains(&r.status) {
            return Err(Fail::Other(format!("HTTP {} from /v1/voices: {}", r.status, short(&r.body))));
        }
        parse_voices(&r.body).map_err(Fail::Other)
    }

    fn speech(&self, voice: &str, text: &str, speed: f32) -> Result<Vec<u8>, Fail> {
        use iyakki_proxy::Upstream;
        let body = serde_json::json!({"model": "indic-tts", "input": text, "voice": voice, "speed": speed, "response_format": "wav"});
        let h = vec![("Content-Type".to_string(), "application/json".to_string())];
        let r = upstream(PIECE_TIME).request("POST", &format!("{}/v1/audio/speech", self.base), &h, body.to_string().as_bytes()).map_err(|e| Fail::Unreachable(short(e.as_bytes())))?;
        if r.status == 404 {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&r.body) {
                if v["code"] == "model_not_installed" {
                    return Err(Fail::NotInstalled(v["lang"].as_str().unwrap_or("").to_string()));
                }
            }
        }
        if !(200..300).contains(&r.status) {
            return Err(Fail::Other(format!("HTTP {}: {}", r.status, short(&r.body))));
        }
        Ok(r.body)
    }
}

pub fn parse_voices(body: &[u8]) -> Result<Vec<SrvVoice>, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| "the server's list of voices is not JSON".to_string())?;
    let arr = v["voices"].as_array().ok_or("the server's list of voices has no `voices`")?;
    Ok(arr
        .iter()
        .filter_map(|x| {
            let id = x["id"].as_str()?.to_string();
            let (lang, gender) = id.split_once('-')?;
            (lang_code(lang) == Some(lang_info(lang)?.code) && matches!(gender, "female" | "male")).then(|| SrvVoice { lang: lang.to_string(), gender: gender.to_string(), id: id.clone() })
        })
        .collect())
}

/// The voice for `lang` among those the server has: the wanted gender when there is one, else the other.
pub fn choose(voices: &[SrvVoice], lang: &str, gender: Gender) -> Result<SrvVoice, Fail> {
    let of = |g: Gender| voices.iter().find(|v| v.lang == lang && v.gender == g.word());
    of(gender).or_else(|| of(gender.other())).cloned().ok_or_else(|| Fail::NotInstalled(lang.to_string()))
}

/// The language to read in for a story language: a model language, or English when the story's language has no voice but English
/// is installed? No: a missing language is reported, never read in another one (a Tamil story in an English voice is noise).
pub fn language_of(tag: Option<&str>, setting_lang: Option<&str>) -> Option<&'static str> {
    tag.and_then(lang_code).or(setting_lang.and_then(lang_code))
}

// ------------------------------------------------------------------------------------------------------------ the reading

/// Everything one reading needs.
pub struct Reading<'a> {
    pub server: &'a dyn Server,
    /// The story language tag (`ta`), when known.
    pub lang: Option<&'a str>,
    /// The engine setting's voice (`female`, `male`, `ta-male`, or none).
    pub voice: Option<&'a str>,
    pub speed: f32,
    /// Stops the reading early (returns true).
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
}

/// Make one WAV of `text`: normalise, cut by voice and into pieces, ask the server for each piece, set the loudness, join.
pub fn synthesize(r: &Reading<'_>, text: &str) -> Result<Vec<u8>, Fail> {
    let started = Instant::now();
    let (set_lang, set_gender) = parse_setting(r.voice);
    let voices = r.server.voices()?;
    let story = r.lang.and_then(lang_code).or(set_lang);
    // no language known: English when the server has it, else the first language it has
    let lang = match story {
        Some(l) => l,
        None => {
            if voices.iter().any(|v| v.lang == "en") {
                "en"
            } else {
                voices.first().and_then(|v| lang_code(&v.lang)).ok_or_else(|| Fail::Other("the server has no voices".into()))?
            }
        }
    };
    let gender = set_gender.unwrap_or(Gender::Female);
    let english = lang != "en" && voices.iter().any(|v| v.lang == "en");
    let normalised = text::normalise(&voice::speakable(text), lang);
    let segs = text::segments(&normalised, lang, english);
    let mut parts: Vec<audio::Part> = Vec::new();
    let mut rate = 0u32;
    for seg in &segs {
        let v = choose(&voices, &seg.lang, gender)?;
        for p in text::pieces(&seg.text, text::max_chars(&seg.lang)) {
            if (r.cancelled)() {
                return Err(Fail::Other("stopped".into()));
            }
            if started.elapsed() > TOTAL_TIME {
                return Err(Fail::Other("the time limit for one reading was reached".into()));
            }
            let wav = match r.server.speech(&v.id, &p.text, r.speed) {
                Ok(w) => w,
                // a piece with nothing the model knows is skipped, not an error
                Err(Fail::Other(m)) if m.contains("422") => continue,
                Err(e) => return Err(e),
            };
            let w = voice::parse_wav(&wav).map_err(|e| Fail::Other(format!("the server's audio: {e}")))?;
            if w.channels != 1 {
                return Err(Fail::Other("the server's audio is not mono".into()));
            }
            let prepared = audio::prepare(&w.samples, w.rate);
            if prepared.is_empty() {
                continue;
            }
            if rate == 0 {
                rate = w.rate;
            }
            parts.push(audio::Part { samples: prepared, rate: w.rate, pause_ms: p.pause_ms });
        }
    }
    if parts.is_empty() {
        return Err(Fail::Other("nothing speakable in the text".into()));
    }
    Ok(voice::encode_wav(&audio::join(&parts, rate), rate))
}

// ------------------------------------------------------------------------------------------------------------ the engine hooks

static NOTE: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// Remember why the last reading used another engine (shown by Settings and the reader's status).
pub fn set_note(s: &str) {
    if let Ok(mut g) = NOTE.lock() {
        *g = Some((Instant::now(), s.to_string()));
    }
}

/// The one-line reason of the last fallback, while it is recent (ten minutes).
pub fn last_note() -> Option<String> {
    NOTE.lock().ok().and_then(|g| g.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(600)).map(|(_, s)| s.clone()))
}

static VOICE_CACHE: Mutex<Vec<(String, Instant, Vec<SrvVoice>)>> = Mutex::new(Vec::new());

/// The voices of the server at `base`, kept for 20 seconds (the voice list is asked for by every piece of every reading).
pub fn cached_voices(base: &str) -> Result<Vec<SrvVoice>, Fail> {
    if let Some(v) = VOICE_CACHE.lock().ok().and_then(|g| g.iter().find(|(b, t, _)| b == base && t.elapsed() < Duration::from_secs(20)).map(|(_, _, v)| v.clone())) {
        return Ok(v);
    }
    let v = HttpServer::new(base).voices()?;
    if let Ok(mut g) = VOICE_CACHE.lock() {
        g.retain(|(b, _, _)| b != base);
        g.push((base.to_string(), Instant::now(), v.clone()));
    }
    Ok(v)
}

pub fn clear_cache() {
    if let Ok(mut g) = VOICE_CACHE.lock() {
        g.clear();
    }
}

struct Cached<'a>(&'a HttpServer);

impl Server for Cached<'_> {
    fn voices(&self) -> Result<Vec<SrvVoice>, Fail> {
        cached_voices(&self.0.base)
    }
    fn speech(&self, voice: &str, text: &str, speed: f32) -> Result<Vec<u8>, Fail> {
        self.0.speech(voice, text, speed)
    }
}

/// Ask the server at `base` for its voices, for the Check server button: the voices, or the reason in words.
pub fn check_server(base: &str) -> Result<Vec<SrvVoice>, String> {
    validate_url(base)?;
    HttpServer::new(base).voices().map_err(|e| e.line())
}

/// The engine as `hostengines` uses it: a WAV for `text`, or the reason it cannot.
pub fn synth_wav(base: &str, voice: Option<&str>, lang: Option<&str>, text: &str, speed: f32) -> Result<Vec<u8>, Fail> {
    let base = if base.trim().is_empty() { DEFAULT_URL } else { base };
    validate_url(base).map_err(Fail::Other)?;
    let srv = HttpServer::new(base);
    let none = || false;
    synthesize(&Reading { server: &Cached(&srv), lang, voice, speed, cancelled: &none }, text)
}

#[cfg(test)]
mod models_tests;
#[cfg(test)]
mod tests;
