//! Speech engines that are already on this computer, found and driven without any setup.
//!
//! Nothing is bundled (the good free engines are GPL; see `docs/VOICE.md`): IYAKKI only *calls* a program the person already
//! has, the way it calls `aplay`. [`detect`] looks for each known engine once (the result is cached by [`HostTts`], and a
//! re-check button runs it again), lists the voices of each, and says why an engine cannot be used (missing program, no
//! voice files, no voice for a language). An engine is then used through [`plan`], which builds the exact command line:
//!
//! * the text of the story or reply is **never** put into a shell and, except for `pico2wave` whose only input is its last
//!   argument (given after `--`), never into the arguments either: it goes to the program's standard input;
//! * voice names and paths are checked (no leading `-`, no control characters, the allowed characters for festival's
//!   scheme), and the speed is mapped to what each engine understands (words per minute, -100..100, SAPI's -10..10,
//!   piper's length scale);
//! * every process has a time limit, is killed with its whole process group when the reader is stopped, and a speech
//!   service that keeps talking after its client dies (speech-dispatcher) gets its own stop command.
//!
//! An engine can *speak* directly (espeak, spd-say, `say`, SAPI) and/or *write a WAV file* (everything but spd-say), which
//! is what "Save audio" needs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::voice::{self, SpeechEngine};

/// Longest one piece may take to make or to speak.
pub const MAX_PIECE_TIME: Duration = Duration::from_secs(180);
/// How long listing the voices of an engine may take.
const LIST_TIME: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Piper,
    MacSay,
    Sapi,
    EspeakNg,
    SpdSay,
    Pico,
    Festival,
    Espeak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

impl Kind {
    /// Best first: what is chosen when the person has set nothing.
    pub const ALL: [Kind; 8] = [Kind::Piper, Kind::MacSay, Kind::Sapi, Kind::EspeakNg, Kind::SpdSay, Kind::Pico, Kind::Festival, Kind::Espeak];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Piper => "piper",
            Kind::MacSay => "say",
            Kind::Sapi => "sapi",
            Kind::EspeakNg => "espeak-ng",
            Kind::SpdSay => "spd-say",
            Kind::Pico => "pico2wave",
            Kind::Festival => "festival",
            Kind::Espeak => "espeak",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Piper => "piper (neural voices)",
            Kind::MacSay => "macOS say",
            Kind::Sapi => "Windows speech (SAPI)",
            Kind::EspeakNg => "espeak-ng",
            Kind::SpdSay => "speech-dispatcher (spd-say)",
            Kind::Pico => "SVOX pico (pico2wave)",
            Kind::Festival => "festival",
            Kind::Espeak => "espeak",
        }
    }

    /// The program that is looked for.
    pub fn program(self) -> &'static str {
        match self {
            Kind::Sapi => "powershell",
            Kind::Festival => "text2wave",
            other => other.id(),
        }
    }

    pub fn os_ok(self, os: Os) -> bool {
        match self {
            Kind::MacSay => os == Os::Mac,
            Kind::Sapi => os == Os::Windows,
            Kind::Piper | Kind::EspeakNg | Kind::Espeak => true,
            Kind::SpdSay | Kind::Pico | Kind::Festival => os == Os::Linux,
        }
    }

    /// Speaks by itself, through the sound system.
    pub fn can_speak(self) -> bool {
        matches!(self, Kind::EspeakNg | Kind::Espeak | Kind::SpdSay | Kind::MacSay | Kind::Sapi)
    }

    /// Can write a WAV file (needed for "Save audio").
    pub fn can_file(self) -> bool {
        !matches!(self, Kind::SpdSay)
    }

    /// How to get it, for the page that says why an engine is missing.
    pub fn install_hint(self) -> &'static str {
        match self {
            Kind::Piper => "download piper and a voice (.onnx and .onnx.json) from the piper project and put the voice in ~/.local/share/piper",
            Kind::MacSay => "built into macOS",
            Kind::Sapi => "built into Windows (PowerShell with System.Speech)",
            Kind::EspeakNg => "install espeak-ng with your package manager (for example: sudo apt install espeak-ng)",
            Kind::SpdSay => "install speech-dispatcher with your package manager (for example: sudo apt install speech-dispatcher)",
            Kind::Pico => "install libttspico-utils (Debian and Ubuntu: non-free) for pico2wave",
            Kind::Festival => "install festival with your package manager",
            Kind::Espeak => "install espeak with your package manager",
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ languages

/// `ta_IN` and `TA-in` are `ta-in`.
pub fn norm_lang(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('_', "-")
}

fn primary(s: &str) -> String {
    norm_lang(s).split('-').next().unwrap_or("").to_string()
}

/// The name of the language of a tag for the messages (`ta-IN` becomes Tamil).
pub fn lang_name(tag: &str) -> String {
    match primary(tag).as_str() {
        "en" => "English",
        "ta" => "Tamil",
        "hi" => "Hindi",
        "te" => "Telugu",
        "ml" => "Malayalam",
        "kn" => "Kannada",
        "bn" => "Bengali",
        "mr" => "Marathi",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "pt" => "Portuguese",
        "ar" => "Arabic",
        "ja" => "Japanese",
        "it" => "Italian",
        other => return other.to_string(),
    }
    .to_string()
}

/// A short sentence in the language for the Test button (English where there is none).
pub fn sample_sentence(tag: &str) -> &'static str {
    match primary(tag).as_str() {
        "ta" => "வணக்கம், இது இயக்கி குரல் சோதனை.",
        "hi" => "नमस्ते, यह इयक्की की आवाज़ का परीक्षण है।",
        "te" => "నమస్కారం, ఇది ఇయక్కి వాయిస్ పరీక్ష.",
        "ml" => "നമസ്കാരം, ഇത് ഇയക്കി ശബ്ദ പരിശോധനയാണ്.",
        "es" => "Hola, esta es la prueba de voz de IYAKKI.",
        "fr" => "Bonjour, ceci est le test de voix d'IYAKKI.",
        "de" => "Hallo, dies ist der IYAKKI-Stimmtest.",
        _ => "This is the IYAKKI voice test.",
    }
}

// ------------------------------------------------------------------------------------------------------------ voices and engines

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HVoice {
    /// What is passed to the engine (a voice name, a language code or the path of a piper model).
    pub id: String,
    pub name: String,
    /// Normalised language tags it speaks (`ta`, `en-gb`).
    pub langs: Vec<String>,
}

impl HVoice {
    fn new(id: &str, name: &str, langs: &[&str]) -> HVoice {
        HVoice { id: id.to_string(), name: name.to_string(), langs: langs.iter().map(|l| norm_lang(l)).collect() }
    }

    /// 2 for the same tag, 1 for the same language, 0 for none.
    pub fn fit(&self, want: &str) -> u8 {
        let w = norm_lang(want);
        if self.langs.iter().any(|l| *l == w) {
            2
        } else if self.langs.iter().any(|l| primary(l) == primary(&w)) {
            1
        } else {
            0
        }
    }
}

/// What picking a voice for a language came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// This voice speaks the language.
    Voice(String),
    /// The engine chooses by itself (it was told the language, or has one voice).
    Auto,
    /// The engine has voices, none speaks the language: the default voice is used.
    Missing(String),
}

/// The voice for a story language: the one asked for by name when there is one, else the best match of the language.
pub fn pick_voice(voices: &[HVoice], requested: Option<&str>, lang: Option<&str>) -> Pick {
    if let Some(r) = requested.map(str::trim).filter(|r| !r.is_empty()) {
        return Pick::Voice(r.to_string());
    }
    let Some(lang) = lang.map(str::trim).filter(|l| !l.is_empty()) else { return Pick::Auto };
    if voices.is_empty() {
        return Pick::Auto;
    }
    let best = voices.iter().map(|v| (v.fit(lang), v)).filter(|(f, _)| *f > 0).max_by_key(|(f, v)| (*f, v.langs.len() == 1, std::cmp::Reverse(v.id.len())));
    match best {
        Some((_, v)) => Pick::Voice(v.id.clone()),
        None => Pick::Missing(lang_name(lang)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Engine {
    pub kind: Kind,
    pub program: PathBuf,
    pub voices: Vec<HVoice>,
    /// Installed but not usable, and why (no voice files, speech-dispatcher has no voices, ...).
    pub problem: Option<String>,
}

impl Engine {
    pub fn usable(&self) -> bool {
        self.problem.is_none()
    }

    pub fn speaks(&self, lang: &str) -> bool {
        self.voices.iter().any(|v| v.fit(lang) > 0)
    }

    /// The setting that makes the speech of this engine the one used.
    pub fn setting(&self) -> SpeechEngine {
        SpeechEngine::Host { engine: self.kind.id().to_string(), program: Some(self.program.to_string_lossy().to_string()), voice: None }
    }
}

/// Where to look.
#[derive(Clone, Debug)]
pub struct Env {
    pub path: String,
    pub os: Os,
    pub piper_dirs: Vec<PathBuf>,
}

impl Env {
    /// This computer: the PATH of the process plus the places programs usually are, and the usual places of piper voices.
    pub fn current(extra_voice_dirs: &[PathBuf]) -> Env {
        let mut path = std::env::var("PATH").unwrap_or_default();
        for d in ["/usr/local/bin", "/usr/bin", "/bin", "/opt/homebrew/bin", "/snap/bin"] {
            if !path.split(':').any(|p| p == d) {
                if !path.is_empty() {
                    path.push(':');
                }
                path.push_str(d);
            }
        }
        let mut piper_dirs: Vec<PathBuf> = extra_voice_dirs.to_vec();
        if let Ok(home) = std::env::var("HOME") {
            for d in [".local/share/piper", "piper", ".local/share/piper-voices", "piper-voices"] {
                piper_dirs.push(Path::new(&home).join(d));
            }
        }
        for d in ["/usr/share/piper-voices", "/usr/local/share/piper", "/opt/piper"] {
            piper_dirs.push(PathBuf::from(d));
        }
        // /usr/share/piper*
        if let Ok(rd) = std::fs::read_dir("/usr/share") {
            let mut more: Vec<PathBuf> = rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("piper")).map(|e| e.path()).collect();
            more.sort();
            piper_dirs.extend(more);
        }
        piper_dirs.dedup();
        Env { path, os: Os::current(), piper_dirs }
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub engines: Vec<Engine>,
    /// Engines that are not here, with why and how to get them (for this system only).
    pub missing: Vec<(Kind, String)>,
    pub at: SystemTime,
}

impl Report {
    pub fn empty() -> Report {
        Report { engines: Vec::new(), missing: Vec::new(), at: SystemTime::UNIX_EPOCH }
    }

    pub fn get(&self, kind: Kind) -> Option<&Engine> {
        self.engines.iter().find(|e| e.kind == kind)
    }

    /// The engine that is used when the person chose none: the best usable one, preferring (when the language is known) one
    /// that has a voice for it.
    pub fn best(&self, lang: Option<&str>) -> Option<&Engine> {
        let usable = || self.engines.iter().filter(|e| e.usable());
        if let Some(l) = lang.filter(|l| !l.is_empty()) {
            if let Some(e) = usable().find(|e| e.speaks(l)) {
                return Some(e);
            }
        }
        usable().next()
    }

    /// Why this kind cannot be used here (None when it can).
    pub fn why_not(&self, kind: Kind) -> Option<String> {
        match self.get(kind) {
            Some(e) => e.problem.clone(),
            None => self.missing.iter().find(|(k, _)| *k == kind).map(|(_, w)| w.clone()),
        }
    }
}

fn which(prog: &str, path: &str) -> Option<PathBuf> {
    crate::story::which(prog, path)
}

// ------------------------------------------------------------------------------------------------------------ parsing the lists

/// `espeak-ng --voices`: `Pty Language Age/Gender VoiceName File Other`.
pub fn parse_espeak_voices(out: &str) -> Vec<HVoice> {
    let mut v = Vec::new();
    for l in out.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 4 || f[0].parse::<u32>().is_err() {
            continue;
        }
        v.push(HVoice::new(f[1], &f[3].replace('_', " "), &[f[1]]));
    }
    v
}

/// `spd-say -L`: `NAME LANGUAGE VARIANT`. Only the plain voice of each language is kept (there are hundreds of variants).
pub fn parse_spd_voices(out: &str) -> Vec<HVoice> {
    let mut v: Vec<HVoice> = Vec::new();
    for l in out.lines() {
        // the name may have spaces and brackets ("English (America)"): language and variant are the last two columns
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || f[0] == "NAME" || f[f.len() - 1] != "none" {
            continue;
        }
        let (lang, name) = (f[f.len() - 2], f[..f.len() - 2].join(" "));
        if name.starts_with('-') || name.is_empty() {
            continue;
        }
        if !v.iter().any(|x| x.id == name) {
            v.push(HVoice::new(&name, &name, &[lang]));
        }
    }
    v
}

/// `say -v '?'`: `Alex  en_US    # sample text`. Names may contain spaces and brackets.
pub fn parse_say_voices(out: &str) -> Vec<HVoice> {
    let mut v = Vec::new();
    for l in out.lines() {
        let head = l.split('#').next().unwrap_or("").trim_end();
        let Some((name, lang)) = head.rsplit_once(char::is_whitespace) else { continue };
        let (name, lang) = (name.trim(), lang.trim());
        if name.is_empty() || !lang.chars().all(|c| c.is_ascii_alphabetic() || c == '_' || c == '-') || lang.is_empty() {
            continue;
        }
        v.push(HVoice::new(name, name, &[lang]));
    }
    v
}

/// The script listing SAPI voices: `Name|culture`.
pub fn parse_sapi_voices(out: &str) -> Vec<HVoice> {
    out.lines().filter_map(|l| l.trim().split_once('|')).filter(|(n, _)| !n.trim().is_empty()).map(|(n, c)| HVoice::new(n.trim(), n.trim(), &[c.trim()])).collect()
}

/// Piper voices found on disk: a `.onnx` next to its `.onnx.json`; the language comes from the name (`ta_IN-xyz-medium`) or
/// from the json.
pub fn scan_piper(dirs: &[PathBuf]) -> Vec<HVoice> {
    let mut found: Vec<HVoice> = Vec::new();
    for d in dirs {
        let mut stack = vec![(d.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && depth < 3 {
                    stack.push((p, depth + 1));
                } else if p.extension().is_some_and(|x| x == "onnx") {
                    let json = PathBuf::from(format!("{}.json", p.display()));
                    if !json.is_file() {
                        continue;
                    }
                    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let from_name = stem.split('-').next().unwrap_or("").to_string();
                    let code = if from_name.len() >= 2 && from_name.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
                        from_name
                    } else {
                        std::fs::read_to_string(&json).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|j| j["language"]["code"].as_str().map(str::to_string)).unwrap_or_else(|| "en".into())
                    };
                    let id = p.to_string_lossy().to_string();
                    if !found.iter().any(|f| f.id == id) {
                        found.push(HVoice::new(&id, &stem, &[&code]));
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| (!a.langs.iter().any(|l| primary(l) == "en"), &a.id).cmp(&(!b.langs.iter().any(|l| primary(l) == "en"), &b.id)));
    found
}

pub const PICO_VOICES: [(&str, &str); 6] = [("en-US", "English (US)"), ("en-GB", "English (UK)"), ("de-DE", "German"), ("es-ES", "Spanish"), ("fr-FR", "French"), ("it-IT", "Italian")];

const SAPI_LIST_SCRIPT: &str = "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name + '|' + $_.VoiceInfo.Culture.Name }";

/// Text goes in on standard input; voice, rate and output file come in the environment, never in the script text.
const SAPI_SPEAK_SCRIPT: &str = "[Console]::InputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; if ($env:IYAKKI_VOICE) { $s.SelectVoice($env:IYAKKI_VOICE) }; $s.Rate = [int]$env:IYAKKI_RATE; if ($env:IYAKKI_OUT) { $s.SetOutputToWaveFile($env:IYAKKI_OUT) }; $t = [Console]::In.ReadToEnd(); $s.Speak($t); $s.Dispose()";

fn run_list(argv: &[String], extra_env: &[(String, String)]) -> Result<String, String> {
    let mut c = voice::command_for(argv, None);
    c.envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let (out, _) = voice::run_capture(c, None, LIST_TIME)?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn list_voices(kind: Kind, program: &Path, env: &Env) -> Result<Vec<HVoice>, String> {
    let p = program.to_string_lossy().to_string();
    match kind {
        Kind::EspeakNg | Kind::Espeak => run_list(&[p, "--voices".into()], &[]).map(|o| parse_espeak_voices(&o)),
        Kind::SpdSay => run_list(&[p, "-L".into()], &[]).map(|o| parse_spd_voices(&o)),
        Kind::MacSay => run_list(&[p, "-v".into(), "?".into()], &[]).map(|o| parse_say_voices(&o)),
        Kind::Sapi => run_list(&[p, "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), SAPI_LIST_SCRIPT.into()], &[]).map(|o| parse_sapi_voices(&o)),
        Kind::Piper => Ok(scan_piper(&env.piper_dirs)),
        Kind::Pico => Ok(PICO_VOICES.iter().map(|(c, n)| HVoice::new(c, n, &[c])).collect()),
        // text2wave has no list of its own; the voices are those of the festival program next to it
        Kind::Festival => {
            let fest = which("festival", &env.path);
            match fest {
                Some(f) => run_list(&[f.to_string_lossy().to_string(), "-b".into(), "(mapcar (lambda (v) (format t \"voice %s\\n\" v)) (voice.list))".into()], &[]).map(|o| parse_festival_voices(&o)),
                None => Ok(Vec::new()),
            }
        }
    }
}

pub fn parse_festival_voices(out: &str) -> Vec<HVoice> {
    out.lines()
        .filter_map(|l| l.trim().strip_prefix("voice "))
        .map(str::trim)
        .filter(|n| valid_scheme_name(n))
        .map(|n| {
            let lang = if n.starts_with("el_") { "es" } else if n.starts_with("suo_") { "fi" } else if n.starts_with("ked_") || n.starts_with("kal_") || n.starts_with("rab_") || n.starts_with("cmu_") || n.starts_with("nitech_") { "en" } else { "en" };
            HVoice::new(n, n, &[lang])
        })
        .collect()
}

/// The names that may go into festival's scheme expression.
fn valid_scheme_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Look for every engine of this system, list their voices and judge whether each can be used.
pub fn detect(env: &Env) -> Report {
    let mut engines = Vec::new();
    let mut missing = Vec::new();
    for kind in Kind::ALL {
        if !kind.os_ok(env.os) {
            continue;
        }
        let found = match kind {
            Kind::Sapi => which("powershell", &env.path).or_else(|| which("pwsh", &env.path)),
            Kind::MacSay => which("say", &env.path).or_else(|| Path::new("/usr/bin/say").is_file().then(|| PathBuf::from("/usr/bin/say"))),
            k => which(k.program(), &env.path),
        };
        let Some(program) = found else {
            missing.push((kind, format!("{} was not found; {}", kind.program(), kind.install_hint())));
            continue;
        };
        let (voices, mut problem) = match list_voices(kind, &program, env) {
            Ok(v) => (v, None),
            Err(e) => (Vec::new(), Some(format!("{} did not list its voices: {e}", kind.program()))),
        };
        if problem.is_none() {
            problem = match kind {
                Kind::Piper if voices.is_empty() => Some("piper is installed but no voice file was found (a .onnx with its .onnx.json in ~/.local/share/piper, ~/piper or /usr/share/piper*); download a voice from the piper project".to_string()),
                Kind::SpdSay if voices.is_empty() => Some("speech-dispatcher has no voices: install a speech module such as espeak-ng for it".to_string()),
                Kind::MacSay | Kind::Sapi | Kind::EspeakNg | Kind::Espeak if voices.is_empty() => Some(format!("{} lists no voices", kind.program())),
                _ => None,
            };
        }
        engines.push(Engine { kind, program, voices, problem });
    }
    Report { engines, missing, at: SystemTime::now() }
}

// ------------------------------------------------------------------------------------------------------------ cache

/// The detection result of this computer: made once in the background, kept, and made again on request. Off by default
/// (tests build many [`crate::services::Services`]); the programs turn it on ([`enable_by_default`]).
pub struct HostTts {
    enabled: std::sync::atomic::AtomicBool,
    /// The command that opens the browser for the voice helper (None: the usual one for this system).
    opener: Mutex<Option<Vec<String>>>,
    env: Mutex<Option<Env>>,
    state: Mutex<(Option<Arc<Report>>, bool)>,
    /// The browser voice helper, while there is one (see `voicehelper`).
    helper: Mutex<Option<crate::voicehelper::VoiceHelper>>,
}

static DEFAULT_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The IYAKKI programs call this at start: speech engines of the computer are detected and used.
pub fn enable_by_default() {
    DEFAULT_ON.store(true, std::sync::atomic::Ordering::SeqCst);
}

impl HostTts {
    pub fn new(voice_dirs: &[PathBuf]) -> HostTts {
        let enabled = DEFAULT_ON.load(std::sync::atomic::Ordering::SeqCst);
        HostTts { enabled: enabled.into(), opener: Mutex::new(None), env: Mutex::new(enabled.then(|| Env::current(voice_dirs))), state: Mutex::new((None, false)), helper: Mutex::new(None) }
    }

    /// For tests and for the programs that know better: detect in this environment.
    pub fn with_env(env: Env) -> HostTts {
        HostTts { enabled: true.into(), opener: Mutex::new(None), env: Mutex::new(Some(env)), state: Mutex::new((None, false)), helper: Mutex::new(None) }
    }

    pub fn disabled() -> HostTts {
        HostTts { enabled: false.into(), opener: Mutex::new(None), env: Mutex::new(None), state: Mutex::new((None, false)), helper: Mutex::new(None) }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Detect in this environment from now on (tests, and programs that know where the engines are). The old result is dropped.
    pub fn configure(&self, env: Env) {
        *self.env.lock().unwrap_or_else(|e| e.into_inner()) = Some(env);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = (None, false);
        self.enabled.store(true, std::sync::atomic::Ordering::SeqCst);
        clear_voice_cache();
    }

    /// Open the voice helper's page with this command (the address is added as its last argument) instead of the system's.
    pub fn set_opener(&self, opener: Option<Vec<String>>) {
        *self.opener.lock().unwrap_or_else(|e| e.into_inner()) = opener;
    }

    pub fn env(&self) -> Option<Env> {
        self.env.lock().ok().and_then(|e| e.clone())
    }

    /// Start the detection in the background if it has not run (and is not running). Cheap to call every frame.
    pub fn ensure(self: &Arc<Self>) {
        if !self.is_enabled() {
            return;
        }
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.0.is_some() || s.1 {
                return;
            }
            s.1 = true;
        }
        self.spawn_scan();
    }

    /// Look again (the Re-check button): the old result stays until the new one is there.
    pub fn recheck(self: &Arc<Self>) {
        if !self.is_enabled() {
            return;
        }
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.1 {
                return;
            }
            s.1 = true;
        }
        clear_voice_cache();
        self.spawn_scan();
    }

    fn spawn_scan(self: &Arc<Self>) {
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("iyakki-host-tts-scan".into()).spawn(move || {
            let report = me.env().map(|e| detect(&e)).unwrap_or_else(Report::empty);
            let mut s = me.state.lock().unwrap_or_else(|e| e.into_inner());
            *s = (Some(Arc::new(report)), false);
        });
        if spawned.is_err() {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = false;
        }
    }

    /// The last result (None before the first scan ended or when detection is off).
    pub fn report(&self) -> Option<Arc<Report>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0.clone()
    }

    pub fn scanning(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1
    }

    /// Detect now, on this thread (tests; the first reading when nothing was scanned yet).
    pub fn scan_now(&self) -> Arc<Report> {
        let report = Arc::new(self.env().map(|e| detect(&e)).unwrap_or_else(Report::empty));
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = (Some(report.clone()), false);
        report
    }

    /// Start the browser voice helper for `hv` and open the default browser on it (`opener` replaces the usual command).
    /// A helper that is already running is kept (its link is not reopened: it works once).
    pub fn helper_open(&self, hv: Arc<crate::hostvoice::HostVoice>, opener: Option<&[String]>) -> Result<String, String> {
        let mut g = self.helper.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|h| !matches!(h.state(), crate::voicehelper::HelperState::Closed(_))) {
            return Err("The voice helper is already running. Use its tab in your browser, or press Disconnect first.".into());
        }
        let h = crate::voicehelper::VoiceHelper::start(hv)?;
        let url = h.url().to_string();
        let path = Env::current(&[]).path;
        let custom = self.opener.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Err(e) = crate::voicehelper::open_browser(&url, opener.or(custom.as_deref()), &path) {
            h.stop();
            return Err(e);
        }
        *g = Some(h);
        Ok(url)
    }

    pub fn helper_state(&self) -> Option<crate::voicehelper::HelperState> {
        self.helper.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|h| h.state())
    }

    pub fn helper_stop(&self) {
        if let Some(h) = self.helper.lock().unwrap_or_else(|e| e.into_inner()).take() {
            h.stop();
        }
    }

    /// The engine to use when the person set nothing (None: detection is off, has not finished or found nothing).
    pub fn auto_engine(&self, lang: Option<&str>) -> Option<SpeechEngine> {
        self.report()?.best(lang).map(Engine::setting)
    }
}

// ------------------------------------------------------------------------------------------------------------ the command lines

/// An engine chosen, with where its program is and which voice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
    pub program: String,
    pub voice: Option<String>,
}

pub fn validate_voice(v: &str) -> Result<(), String> {
    if v.is_empty() || v.len() > 400 || v.starts_with('-') || v.chars().any(|c| c.is_control()) {
        return Err("the voice name is not valid".into());
    }
    Ok(())
}

pub fn validate_host(engine: &str, program: Option<&str>, voice: Option<&str>) -> Result<(), String> {
    Kind::from_id(engine).ok_or_else(|| format!("unknown speech engine `{engine}`"))?;
    if let Some(p) = program {
        if p.is_empty() || p.contains('\0') || p.contains('\n') || p.len() > 400 {
            return Err("the engine's program path is not valid".into());
        }
    }
    if let Some(v) = voice.filter(|v| !v.is_empty()) {
        validate_voice(v)?;
    }
    Ok(())
}

impl Spec {
    /// From the parts of the setting; the program is looked up in the usual places when it was not stored.
    pub fn from_parts(engine: &str, program: Option<&str>, voice: Option<&str>) -> Result<Spec, String> {
        validate_host(engine, program, voice)?;
        let kind = Kind::from_id(engine).ok_or("unknown speech engine")?;
        let program = match program.filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None => {
                let env = Env::current(&[]);
                let found = match kind {
                    Kind::Sapi => which("powershell", &env.path).or_else(|| which("pwsh", &env.path)),
                    k => which(k.program(), &env.path),
                };
                found.ok_or_else(|| format!("{} was not found on this computer; {}", kind.program(), kind.install_hint()))?.to_string_lossy().to_string()
            }
        };
        Ok(Spec { kind, program, voice: voice.filter(|v| !v.is_empty()).map(str::to_string) })
    }

    pub fn from_setting(e: &SpeechEngine) -> Option<Spec> {
        match e {
            SpeechEngine::Host { engine, program, voice } => Spec::from_parts(engine, program.as_deref(), voice.as_deref()).ok(),
            _ => None,
        }
    }
}

/// One process to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    /// What goes to its standard input (the text).
    pub stdin: Vec<u8>,
    pub env: Vec<(String, String)>,
    /// Run when it is stopped while still speaking.
    pub stop: Option<Vec<String>>,
}

pub fn wpm(speed: f32) -> u32 {
    (175.0 * speed.clamp(0.5, 2.0)).round().clamp(80.0, 450.0) as u32
}

/// speech-dispatcher's rate: -100 (slowest) to 100.
pub fn spd_rate(speed: f32) -> i32 {
    ((speed.clamp(0.5, 2.0) - 1.0) * 100.0).round().clamp(-100.0, 100.0) as i32
}

/// SAPI's rate: -10 to 10.
pub fn sapi_rate(speed: f32) -> i32 {
    ((speed.clamp(0.5, 2.0) - 1.0) * 10.0).round().clamp(-10.0, 10.0) as i32
}

pub fn length_scale(speed: f32) -> f32 {
    1.0 / speed.clamp(0.5, 2.0)
}

fn pico_lang(lang: Option<&str>) -> &'static str {
    let l = norm_lang(lang.unwrap_or(""));
    PICO_VOICES.iter().find(|(c, _)| norm_lang(c) == l).or_else(|| PICO_VOICES.iter().find(|(c, _)| primary(c) == primary(&l))).map_or("en-US", |(c, _)| c)
}

/// Build the command for speaking `text` (when `out` is None, directly) or writing it to the WAV file `out`. `voice` is
/// already chosen (see [`pick_voice`]); `lang` is only used by engines that choose by language themselves.
pub fn plan(spec: &Spec, lang: Option<&str>, speed: f32, text: &str, out: Option<&Path>) -> Result<Plan, String> {
    let speed = if speed.is_finite() { speed.clamp(0.5, 2.0) } else { 1.0 };
    if let Some(v) = &spec.voice {
        validate_voice(v)?;
    }
    let clean: String = text.chars().filter(|c| *c != '\0').collect();
    let mut stdin = clean.clone().into_bytes();
    if !stdin.ends_with(b"\n") {
        stdin.push(b'\n');
    }
    let kind = spec.kind;
    if out.is_none() && !kind.can_speak() {
        return Err(format!("{} makes audio files only", kind.label()));
    }
    if out.is_some() && !kind.can_file() {
        return Err(format!("{} plays the speech itself and cannot save it as an audio file", kind.label()));
    }
    let o = |p: &Path| p.to_string_lossy().to_string();
    let mut argv = vec![spec.program.clone()];
    let mut env: Vec<(String, String)> = Vec::new();
    let mut stop = None;
    let mut input = stdin;
    match kind {
        Kind::EspeakNg | Kind::Espeak => {
            if let Some(v) = &spec.voice {
                argv.extend(["-v".into(), v.clone()]);
            }
            argv.extend(["-s".into(), wpm(speed).to_string()]);
            if let Some(p) = out {
                argv.extend(["-w".into(), o(p)]);
            }
            argv.push("--stdin".into());
        }
        Kind::SpdSay => {
            argv.extend(["-e".into(), "-w".into()]);
            if let Some(v) = &spec.voice {
                argv.extend(["-y".into(), v.clone()]);
            } else if let Some(l) = lang.filter(|l| !l.is_empty()) {
                argv.extend(["-l".into(), primary(l)]);
            }
            argv.extend(["-r".into(), spd_rate(speed).to_string()]);
            stop = Some(vec![spec.program.clone(), "-C".into()]);
        }
        Kind::Festival => {
            let out = out.ok_or("festival makes audio files only")?;
            argv.extend(["-o".into(), o(out)]);
            let mut expr = String::new();
            if let Some(v) = &spec.voice {
                if !valid_scheme_name(v) {
                    return Err("the festival voice name may only have letters, digits and underscores".into());
                }
                expr.push_str(&format!("(voice_{v})"));
            }
            expr.push_str(&format!("(Parameter.set 'Duration_Stretch {:.2})", length_scale(speed)));
            argv.extend(["-eval".into(), expr]);
        }
        Kind::Piper => {
            let out = out.ok_or("piper makes audio files only")?;
            let model = spec.voice.clone().ok_or("piper needs a voice file (a .onnx model)")?;
            argv.extend(["--model".into(), model, "--output_file".into(), o(out), "--length_scale".into(), format!("{:.2}", length_scale(speed))]);
        }
        Kind::Pico => {
            let out = out.ok_or("pico2wave makes audio files only")?;
            // its only way to take text is the last argument: after `--`, so it can never be taken for an option
            argv.extend(["-l".into(), spec.voice.clone().filter(|v| PICO_VOICES.iter().any(|(c, _)| c == v)).unwrap_or_else(|| pico_lang(lang).to_string()), "-w".into(), o(out), "--".into(), clean.trim().to_string()]);
            input = Vec::new();
        }
        Kind::MacSay => {
            if let Some(v) = &spec.voice {
                argv.extend(["-v".into(), v.clone()]);
            }
            argv.extend(["-r".into(), wpm(speed).to_string()]);
            if let Some(p) = out {
                argv.extend(["-o".into(), o(p), "--file-format=WAVE".into(), "--data-format=LEI16@22050".into()]);
            }
            argv.extend(["-f".into(), "-".into()]);
        }
        Kind::Sapi => {
            argv.extend(["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-Command".into(), SAPI_SPEAK_SCRIPT.into()]);
            if let Some(v) = &spec.voice {
                env.push(("IYAKKI_VOICE".into(), v.clone()));
            }
            env.push(("IYAKKI_RATE".into(), sapi_rate(speed).to_string()));
            if let Some(p) = out {
                env.push(("IYAKKI_OUT".into(), o(p)));
            }
        }
    }
    Ok(Plan { argv, stdin: input, env, stop })
}

fn start_process(p: &Plan) -> Result<std::process::Child, String> {
    let mut c = voice::command_for(&p.argv, None);
    c.envs(p.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    c.spawn().map_err(|e| format!("cannot start `{}`: {e}", p.argv[0]))
}

// ------------------------------------------------------------------------------------------------------------ the voice choice at speaking time

static VOICES: Mutex<Option<HashMap<(String, String), Arc<Vec<HVoice>>>>> = Mutex::new(None);

pub fn clear_voice_cache() {
    if let Ok(mut g) = VOICES.lock() {
        *g = None;
    }
}

/// The voices of an engine (kept after the first listing, per program and piper folders).
fn voices_of(spec: &Spec) -> Arc<Vec<HVoice>> {
    let key = (spec.kind.id().to_string(), spec.program.clone());
    if spec.kind != Kind::Piper {
        if let Some(v) = VOICES.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&key).cloned())) {
            return v;
        }
    }
    let env = Env::current(&[]);
    let list = Arc::new(list_voices(spec.kind, Path::new(&spec.program), &env).unwrap_or_default());
    if spec.kind != Kind::Piper {
        if let Ok(mut g) = VOICES.lock() {
            g.get_or_insert_with(HashMap::new).insert(key, list.clone());
        }
    }
    list
}

/// The spec with its voice chosen for `lang` (a voice named in the spec is kept).
pub fn resolve(spec: &Spec, lang: Option<&str>) -> (Spec, Pick) {
    if spec.voice.is_some() || matches!(spec.kind, Kind::Pico) && lang.is_none() {
        return (spec.clone(), Pick::Auto);
    }
    // spd-say, pico and festival choose by themselves from the language when there is nothing better
    let voices = voices_of(spec);
    let pick = pick_voice(&voices, None, lang);
    let mut s = spec.clone();
    match &pick {
        Pick::Voice(v) => {
            if spec.kind == Kind::Festival && !valid_scheme_name(v) {
                return (s, Pick::Auto);
            }
            s.voice = Some(v.clone());
        }
        Pick::Missing(_) | Pick::Auto => {
            if spec.kind == Kind::Piper {
                s.voice = voices.first().map(|v| v.id.clone());
            }
        }
    }
    (s, pick)
}

// ------------------------------------------------------------------------------------------------------------ making and playing

/// Make a WAV file of `text` (the engine writes it; nothing is spoken).
pub fn synth_wav(spec: &Spec, text: &str, lang: Option<&str>, speed: f32) -> Result<Vec<u8>, String> {
    let (spec, _) = resolve(spec, lang);
    let dir = voice::private_tempdir("hosttts")?;
    let out = dir.join("speech.wav");
    let result = (|| {
        let p = plan(&spec, lang, speed, text, Some(&out))?;
        let mut c = voice::command_for(&p.argv, Some(&dir));
        c.envs(p.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        voice::run_capture(c, Some(&p.stdin), MAX_PIECE_TIME).map_err(|e| format!("{}: {e}", spec.kind.label()))?;
        let wav = std::fs::read(&out).map_err(|e| format!("{} wrote no audio file: {e}", spec.kind.label()))?;
        if wav.len() < 44 || &wav[..4] != b"RIFF" {
            return Err(format!("{} did not write WAV audio", spec.kind.label()));
        }
        Ok(wav)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Start speaking `text` on the speaker. Engines that speak by themselves do; the others make a WAV that `play_cmd` (or
/// the first sound player found) plays.
pub fn speak(spec: &Spec, text: &str, lang: Option<&str>, speed: f32, play_cmd: &str) -> Result<voice::Player, String> {
    let (spec, _) = resolve(spec, lang);
    if spec.kind.can_speak() {
        let p = plan(&spec, lang, speed, text, None)?;
        let child = start_process(&p)?;
        return Ok(voice::Player::from_speech(child, p.stdin, p.stop, MAX_PIECE_TIME));
    }
    let wav = synth_wav(&spec, text, lang, speed)?;
    let cmd = player_command(play_cmd, &Env::current(&[]).path).ok_or("no sound player was found (install alsa-utils for aplay, or set the play command in Settings → Voice)")?;
    voice::Player::start(&cmd, &wav)
}

/// The command that plays a WAV from standard input: the configured one when its program exists, else the first usual one.
pub fn player_command(configured: &str, path: &str) -> Option<String> {
    let first = voice::split_command(configured).ok().and_then(|a| a.into_iter().next());
    if first.as_deref().is_some_and(|p| which(p, path).is_some()) {
        return Some(configured.to_string());
    }
    ["aplay -q", "pw-play -", "play -q -t wav -"].into_iter().find(|c| which(c.split(' ').next().unwrap_or(""), path).is_some()).map(str::to_string)
}

/// Does reading with this engine need a separate sound player?
pub fn needs_player(e: &SpeechEngine) -> bool {
    match e {
        SpeechEngine::Host { engine, .. } => Kind::from_id(engine).is_some_and(|k| !k.can_speak()),
        _ => true,
    }
}

/// Can "Save audio" be made with this engine?
pub fn can_save_audio(e: &SpeechEngine) -> bool {
    match e {
        SpeechEngine::Host { engine, .. } => Kind::from_id(engine).is_some_and(Kind::can_file),
        SpeechEngine::Browser => false,
        _ => true,
    }
}

/// An engine of this computer that speaks by itself: the reader hands it the text of each piece.
pub fn speaks_directly(e: &SpeechEngine) -> bool {
    matches!(e, SpeechEngine::Host { engine, .. } if Kind::from_id(engine).is_some_and(Kind::can_speak))
}

// ------------------------------------------------------------------------------------------------------------ the reader's plug-ins

const MAGIC: &[u8] = b"IYHV\0";

/// The reader's synthesiser for an engine that speaks by itself: the "sound" of a piece is its text (see
/// `hostvoice::BrowserSynth`, which works the same way).
pub struct DirectSynth;

impl crate::story::Synth for DirectSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        let t = voice::speakable(text);
        Ok([MAGIC, if t.is_empty() { b" " } else { t.as_bytes() }].concat())
    }
}

/// The reader's sink for an engine that speaks by itself.
pub struct DirectSink {
    pub spec: Spec,
    pub lang: Option<String>,
    pub speed: f32,
}

struct Speaking(voice::Player);

impl crate::story::Playing for Speaking {
    fn is_playing(&mut self) -> bool {
        self.0.is_playing()
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

struct Pause;
impl crate::story::Playing for Pause {
    fn is_playing(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
}

impl crate::story::Sink for DirectSink {
    fn play(&self, wav: &[u8]) -> Result<Box<dyn crate::story::Playing>, String> {
        let text = wav.strip_prefix(MAGIC).map(|t| String::from_utf8_lossy(t).to_string()).ok_or("not a piece of this engine")?;
        if text.trim().is_empty() {
            return Ok(Box::new(Pause));
        }
        Ok(Box::new(Speaking(speak(&self.spec, &text, self.lang.as_deref(), self.speed, "")?)))
    }
}

/// The Test button: say a sentence in the language and wait for the end. Says what was used.
pub fn test_speak(spec: &Spec, lang: &str, speed: f32, play_cmd: &str) -> Result<String, String> {
    let (resolved, pick) = resolve(spec, Some(lang));
    let mut p = speak(&resolved, sample_sentence(lang), Some(lang), speed, play_cmd)?;
    let t = Instant::now();
    while p.is_playing() && t.elapsed() < Duration::from_secs(40) {
        std::thread::sleep(Duration::from_millis(30));
    }
    p.stop();
    let what = resolved.voice.as_deref().map(|v| format!(" with the voice {}", Path::new(v).file_stem().map_or(v.to_string(), |s| s.to_string_lossy().to_string()))).unwrap_or_default();
    Ok(match pick {
        Pick::Missing(l) => format!("Spoke a sentence with {}{what}. It has no {l} voice, so the default voice was used.", spec.kind.label()),
        _ => format!("Spoke a sentence in {} with {}{what}.", lang_name(lang), spec.kind.label()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-he-{tag}-{}-{}", std::process::id(), SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn env_of(dir: &Path, os: Os) -> Env {
        Env { path: dir.to_string_lossy().to_string(), os, piper_dirs: vec![dir.join("piper-voices")] }
    }

    const ESPEAK_LIST: &str = "Pty Language       Age/Gender VoiceName          File                 Other Languages\n 5  af              --/M      Afrikaans          gmw/af\n 5  ta              --/M      Tamil              inc/ta\n 5  hi              --/M      Hindi              inc/hi\n 2  en-gb           --/M      English_(Great_Britain) gmw/en             (en 2)\n 5  en-us           --/M      English_(America)  gmw/en-US           (en 3)\n";

    #[test]
    fn lists_are_parsed() {
        let v = parse_espeak_voices(ESPEAK_LIST);
        assert_eq!(v.len(), 5);
        assert_eq!((v[1].id.as_str(), v[1].langs.clone()), ("ta", vec!["ta".to_string()]));
        let s = parse_spd_voices("                    NAME                 LANGUAGE                  VARIANT\n                Hindi                       hi                     none\n           Hindi+Adam                       hi                     Adam\n             Tamil                       ta                     none\n        English (America)                    en-US                     none\n");
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["Hindi", "Tamil", "English (America)"], "only the plain voice of a language, names with spaces kept");
        assert_eq!(s[2].langs, ["en-us"]);
        let m = parse_say_voices("Alex                en_US    # Most people recognize me by my voice.\nEddy (English (US)) en_US    # Hello! My name is Eddy.\nLekha               hi_IN    # नमस्ते\n");
        assert_eq!(m.len(), 3);
        assert_eq!((m[1].id.as_str(), m[1].langs[0].as_str()), ("Eddy (English (US))", "en-us"));
        assert_eq!(m[2].langs[0], "hi-in");
        let w = parse_sapi_voices("Microsoft David Desktop|en-US\r\nMicrosoft Heera|hi-IN\r\n");
        assert_eq!((w.len(), w[1].id.as_str()), (2, "Microsoft Heera"));
        let f = parse_festival_voices("voice kal_diphone\nvoice el_diphone\nvoice bad;name\nnoise\n");
        assert_eq!(f.iter().map(|x| (x.id.as_str(), x.langs[0].as_str())).collect::<Vec<_>>(), [("kal_diphone", "en"), ("el_diphone", "es")]);
    }

    #[test]
    fn voice_is_chosen_for_the_language() {
        let v = parse_espeak_voices(ESPEAK_LIST);
        assert_eq!(pick_voice(&v, None, Some("ta-IN")), Pick::Voice("ta".into()), "Tamil gets the Tamil voice");
        assert_eq!(pick_voice(&v, None, Some("hi-IN")), Pick::Voice("hi".into()));
        assert_eq!(pick_voice(&v, None, Some("en-US")), Pick::Voice("en-us".into()), "the same tag beats the same language");
        assert!(matches!(pick_voice(&v, None, Some("en-AU")), Pick::Voice(ref x) if x.starts_with("en")), "some English voice");
        assert_eq!(pick_voice(&v, None, Some("te-IN")), Pick::Missing("Telugu".into()), "no Telugu voice here: said so");
        assert_eq!(pick_voice(&v, Some("af"), Some("ta-IN")), Pick::Voice("af".into()), "a voice chosen by name wins");
        assert_eq!(pick_voice(&[], None, Some("ta-IN")), Pick::Auto);
        assert_eq!(pick_voice(&v, None, None), Pick::Auto);
    }

    #[test]
    fn speeds_are_mapped() {
        assert_eq!((wpm(1.0), wpm(2.0), wpm(0.5)), (175, 350, 88));
        assert_eq!((spd_rate(1.0), spd_rate(2.0), spd_rate(0.5)), (0, 100, -50));
        assert_eq!((sapi_rate(1.0), sapi_rate(2.0), sapi_rate(0.5)), (0, 10, -5));
        assert_eq!((length_scale(2.0), length_scale(0.5)), (0.5, 2.0));
        assert_eq!(wpm(9.0), 350, "clamped");
    }

    #[test]
    fn capability_flags() {
        for (k, speak, file) in [(Kind::EspeakNg, true, true), (Kind::Espeak, true, true), (Kind::SpdSay, true, false), (Kind::Festival, false, true), (Kind::Piper, false, true), (Kind::Pico, false, true), (Kind::MacSay, true, true), (Kind::Sapi, true, true)] {
            assert_eq!((k.can_speak(), k.can_file()), (speak, file), "{k:?}");
        }
        let spd = SpeechEngine::Host { engine: "spd-say".into(), program: None, voice: None };
        let pip = SpeechEngine::Host { engine: "piper".into(), program: None, voice: None };
        assert!(speaks_directly(&spd) && !can_save_audio(&spd) && !needs_player(&spd), "spd-say: play only, so Save audio is unavailable");
        assert!(!speaks_directly(&pip) && can_save_audio(&pip) && needs_player(&pip));
        assert!(!can_save_audio(&SpeechEngine::Browser));
        let spec = Spec { kind: Kind::SpdSay, program: "/x/spd-say".into(), voice: None };
        assert!(plan(&spec, None, 1.0, "hi", Some(Path::new("/tmp/o.wav"))).unwrap_err().contains("cannot save"));
        let spec = Spec { kind: Kind::Piper, program: "/x/piper".into(), voice: Some("/v/a.onnx".into()) };
        assert!(plan(&spec, None, 1.0, "hi", None).unwrap_err().contains("audio files only"));
    }

    const HOSTILE: &str = "He said \"hi\"; $(touch /tmp/pwned) `id` 'x' && rm -rf ~\n-rf --version \\ \u{0}end";

    #[test]
    fn hostile_text_never_reaches_a_shell_or_the_arguments() {
        let out = Path::new("/tmp/o.wav");
        for (kind, voice, file) in [(Kind::EspeakNg, Some("ta"), true), (Kind::EspeakNg, None, false), (Kind::SpdSay, Some("Tamil"), false), (Kind::Festival, Some("kal_diphone"), true), (Kind::Piper, Some("/v/a.onnx"), true), (Kind::MacSay, Some("Alex"), true), (Kind::MacSay, None, false), (Kind::Sapi, Some("Microsoft David"), true)] {
            let spec = Spec { kind, program: format!("/x/{}", kind.program()), voice: voice.map(str::to_string) };
            let p = plan(&spec, Some("en-US"), 1.0, HOSTILE, file.then_some(out)).unwrap();
            assert!(p.argv.iter().all(|a| !a.contains("touch") && !a.contains("pwned") && !a.contains("rm -rf")), "{kind:?} {:?}", p.argv);
            assert!(!p.argv.iter().any(|a| a == "sh" || a == "-c" || a == "bash"), "no shell: {:?}", p.argv);
            let text = String::from_utf8(p.stdin.clone()).unwrap();
            assert!(text.contains("$(touch /tmp/pwned)") && text.contains("\"hi\"") && !text.contains('\0'), "{kind:?} gets the text on stdin, whole, without NUL");
            assert!(p.env.iter().all(|(_, v)| !v.contains("pwned")));
        }
        // pico2wave takes its text as the last argument: after `--`, as one argument
        let spec = Spec { kind: Kind::Pico, program: "/x/pico2wave".into(), voice: None };
        let p = plan(&spec, Some("en-GB"), 1.0, "-rf --help", Some(out)).unwrap();
        let i = p.argv.iter().position(|a| a == "--").expect("--");
        assert_eq!((p.argv.len(), p.argv[i + 1].as_str()), (i + 2, "-rf --help"));
        assert!(p.stdin.is_empty());
        assert_eq!(p.argv[2], "en-GB");
        // voices cannot become options or inject scheme
        for bad in ["-evil", "a\nb", ""] {
            let spec = Spec { kind: Kind::EspeakNg, program: "/x/e".into(), voice: Some(bad.into()) };
            assert!(plan(&spec, None, 1.0, "hi", None).is_err(), "{bad:?}");
        }
        let spec = Spec { kind: Kind::Festival, program: "/x/t".into(), voice: Some("x) (system \"id\")".into()) };
        assert!(plan(&spec, None, 1.0, "hi", Some(out)).unwrap_err().contains("letters"));
    }

    #[test]
    fn command_lines_per_engine() {
        let out = Path::new("/tmp/o.wav");
        let s = |kind, voice: Option<&str>| Spec { kind, program: format!("/x/{}", kind.program()), voice: voice.map(str::to_string) };
        let p = plan(&s(Kind::EspeakNg, Some("ta")), None, 2.0, "hi", Some(out)).unwrap();
        assert_eq!(p.argv, ["/x/espeak-ng", "-v", "ta", "-s", "350", "-w", "/tmp/o.wav", "--stdin"]);
        let p = plan(&s(Kind::SpdSay, None), Some("hi-IN"), 0.5, "hi", None).unwrap();
        assert_eq!(p.argv, ["/x/spd-say", "-e", "-w", "-l", "hi", "-r", "-50"]);
        assert_eq!(p.stop, Some(vec!["/x/spd-say".to_string(), "-C".to_string()]));
        let p = plan(&s(Kind::Piper, Some("/v/ta_IN-x-medium.onnx")), None, 2.0, "hi", Some(out)).unwrap();
        assert_eq!(p.argv, ["/x/piper", "--model", "/v/ta_IN-x-medium.onnx", "--output_file", "/tmp/o.wav", "--length_scale", "0.50"]);
        let p = plan(&s(Kind::MacSay, Some("Lekha")), None, 1.0, "hi", Some(out)).unwrap();
        assert_eq!(p.argv, ["/x/say", "-v", "Lekha", "-r", "175", "-o", "/tmp/o.wav", "--file-format=WAVE", "--data-format=LEI16@22050", "-f", "-"]);
        let p = plan(&s(Kind::Festival, Some("kal_diphone")), None, 0.5, "hi", Some(out)).unwrap();
        assert_eq!(p.argv, ["/x/text2wave", "-o", "/tmp/o.wav", "-eval", "(voice_kal_diphone)(Parameter.set 'Duration_Stretch 2.00)"]);
        let p = plan(&s(Kind::Sapi, Some("Microsoft Heera")), None, 2.0, "hi", Some(out)).unwrap();
        assert!(p.argv[0].ends_with("powershell") && p.argv.contains(&"-NoProfile".to_string()));
        assert_eq!(p.env, [("IYAKKI_VOICE".to_string(), "Microsoft Heera".to_string()), ("IYAKKI_RATE".to_string(), "10".to_string()), ("IYAKKI_OUT".to_string(), "/tmp/o.wav".to_string())]);
        assert!(p.argv.iter().all(|a| !a.contains("Heera")), "the voice is in the environment, not in the script");
    }

    /// Fake programs standing in for each engine, on a temp PATH. They log next to themselves (the engines run with a clean
    /// environment, so nothing can be passed through it).
    fn fakes(dir: &Path) {
        std::fs::write(dir.join("voices.txt"), ESPEAK_LIST).unwrap();
        script(dir, "espeak-ng", "D=$(dirname \"$0\")\ncase \"$1\" in --voices) cat \"$D/voices.txt\";; *) cat > \"$D/log.in\"; echo \"$@\" > \"$D/log.args\"; out=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -w ] && out=\"$a\"; prev=\"$a\"; done; if [ -n \"$out\" ]; then printf 'RIFF....WAVE' > \"$out\"; head -c 60 /dev/zero >> \"$out\"; fi;; esac");
        script(dir, "spd-say", "D=$(dirname \"$0\")\ncase \"$1\" in -L) printf '  NAME LANGUAGE VARIANT\\n Tamil ta none\\n English en none\\n';; -O) echo espeak-ng;; -C) echo cancelled > \"$D/cancelled\";; *) echo \"$$\" > \"$D/pid\"; cat > \"$D/log.in\"; sleep 30;; esac");
        script(dir, "say", "echo \"$@\"");
        script(dir, "powershell", "echo 'Microsoft Heera|hi-IN'");
        script(dir, "pico2wave", "exit 0");
        script(dir, "text2wave", "exit 0");
        script(dir, "festival", "echo 'voice kal_diphone'");
        script(dir, "piper", "exit 0");
        std::fs::create_dir_all(dir.join("piper-voices")).unwrap();
    }

    #[test]
    fn each_engine_kind_is_detected_with_its_voices() {
        let d = tmp("detect");
        fakes(&d);
        std::fs::write(d.join("piper-voices/ta_IN-test-medium.onnx"), "x").unwrap();
        std::fs::write(d.join("piper-voices/ta_IN-test-medium.onnx.json"), "{}").unwrap();
        let r = detect(&env_of(&d, Os::Linux));
        let ids: Vec<&str> = r.engines.iter().map(|e| e.kind.id()).collect();
        assert_eq!(ids, ["piper", "espeak-ng", "spd-say", "pico2wave", "festival"], "linux kinds, best first; say and sapi belong to other systems");
        assert!(r.engines.iter().all(|e| e.usable()), "{:?}", r.engines.iter().map(|e| &e.problem).collect::<Vec<_>>());
        assert_eq!(r.get(Kind::Piper).unwrap().voices[0].langs, ["ta-in"]);
        assert!(r.get(Kind::EspeakNg).unwrap().speaks("ta-IN") && !r.get(Kind::EspeakNg).unwrap().speaks("te-IN"));
        assert_eq!(r.get(Kind::SpdSay).unwrap().voices.len(), 2);
        assert_eq!(r.get(Kind::Pico).unwrap().voices.len(), 6);
        assert_eq!(r.get(Kind::Festival).unwrap().voices[0].id, "kal_diphone");
        assert_eq!(r.best(Some("ta-IN")).unwrap().kind, Kind::Piper, "best first, and it has Tamil");
        assert_eq!(r.best(Some("hi-IN")).unwrap().kind, Kind::EspeakNg, "piper has no Hindi, the next best that does is chosen");
        let mac = detect(&env_of(&d, Os::Mac));
        assert!(mac.get(Kind::MacSay).is_some() && mac.get(Kind::SpdSay).is_none() && mac.get(Kind::EspeakNg).is_some());
        let win = detect(&env_of(&d, Os::Windows));
        assert_eq!(win.get(Kind::Sapi).unwrap().voices[0].id, "Microsoft Heera");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_and_unusable_engines_say_why() {
        let d = tmp("why");
        script(&d, "piper", "exit 0");
        script(&d, "spd-say", "case \"$1\" in -L) echo '  NAME LANGUAGE VARIANT';; esac");
        let r = detect(&env_of(&d, Os::Linux));
        assert!(r.why_not(Kind::Piper).unwrap().contains("no voice file"), "{:?}", r.why_not(Kind::Piper));
        assert!(r.why_not(Kind::SpdSay).unwrap().contains("no voices"));
        let e = r.why_not(Kind::EspeakNg).unwrap();
        assert!(e.contains("not found") && e.contains("apt install espeak-ng"), "{e}");
        assert!(r.best(None).is_none(), "nothing usable");
        assert_eq!(r.why_not(Kind::MacSay), None, "not on this system: nothing to report");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn host_tts_scans_in_the_background_and_rechecks() {
        let d = tmp("cache");
        fakes(&d);
        let h = Arc::new(HostTts::with_env(env_of(&d, Os::Linux)));
        assert!(h.report().is_none() && h.auto_engine(None).is_none());
        h.ensure();
        for _ in 0..300 {
            if h.report().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let r = h.report().expect("scanned");
        assert!(r.get(Kind::EspeakNg).is_some());
        assert!(matches!(h.auto_engine(Some("ta-IN")), Some(SpeechEngine::Host { engine, .. }) if engine == "espeak-ng"));
        std::fs::remove_file(d.join("espeak-ng")).unwrap();
        h.ensure();
        assert!(h.report().unwrap().get(Kind::EspeakNg).is_some(), "cached: ensure does not look again");
        h.recheck();
        for _ in 0..300 {
            if !h.scanning() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(h.report().unwrap().get(Kind::EspeakNg).is_none(), "the re-check found the change");
        assert!(HostTts::disabled().report().is_none() && !HostTts::disabled().is_enabled());
        let off = Arc::new(HostTts::disabled());
        off.ensure();
        assert!(off.report().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writes_a_wav_through_a_fake_engine_with_hostile_text_on_stdin() {
        let d = tmp("wav");
        fakes(&d);
        let spec = Spec { kind: Kind::EspeakNg, program: d.join("espeak-ng").to_string_lossy().to_string(), voice: None };
        let before = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("iyakki-hosttts")).count();
        let wav = synth_wav(&spec, HOSTILE, Some("ta-IN"), 2.0).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        let args = std::fs::read_to_string(d.join("log.args")).unwrap();
        assert!(args.starts_with("-v ta -s 350 -w ") && args.trim_end().ends_with("--stdin"), "{args}");
        assert!(!args.contains("pwned"));
        let sent = std::fs::read_to_string(d.join("log.in")).unwrap();
        assert!(sent.contains("$(touch /tmp/pwned)") && sent.contains("`id`"), "{sent:?}");
        assert!(!Path::new("/tmp/pwned").exists());
        let after = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("iyakki-hosttts")).count();
        assert!(after <= before, "the private folder of the piece is removed");
        // an engine that fails or writes nothing is an error, not silence
        let bad = Spec { kind: Kind::Pico, program: d.join("pico2wave").to_string_lossy().to_string(), voice: None };
        assert!(synth_wav(&bad, "hi", None, 1.0).unwrap_err().contains("wrote no audio file"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stopping_kills_the_process_and_cancels_the_speech_service() {
        let d = tmp("stop");
        fakes(&d);
        let spec = Spec { kind: Kind::SpdSay, program: d.join("spd-say").to_string_lossy().to_string(), voice: None };
        let mut p = speak(&spec, "Hello there", Some("ta-IN"), 1.0, "").unwrap();
        for _ in 0..100 {
            if d.join("pid").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let pid: i32 = std::fs::read_to_string(d.join("pid")).unwrap().trim().parse().unwrap();
        assert!(p.is_playing());
        let t = Instant::now();
        p.stop();
        assert!(t.elapsed() < Duration::from_secs(5) && !p.is_playing());
        assert!(d.join("cancelled").exists(), "spd-say -C was run: the service stops talking too");
        std::thread::sleep(Duration::from_millis(100));
        // SAFETY: signal 0 only asks whether the process exists
        assert!(unsafe { libc::kill(pid, 0) } != 0, "the process is gone");
        assert_eq!(std::fs::read_to_string(d.join("log.in")).unwrap().trim(), "Hello there");
        // the same through the reader's plug-ins: a piece is its text, stopping ends it
        use crate::story::{Sink, Synth};
        let blob = DirectSynth.synth("Some **bold** words https://x.example").unwrap();
        let sink = DirectSink { spec, lang: None, speed: 1.0 };
        let mut pl = sink.play(&blob).unwrap();
        assert!(pl.is_playing());
        pl.stop();
        assert!(!pl.is_playing());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_player_is_found_or_the_reason_is_clear() {
        let d = tmp("player");
        script(&d, "aplay", "cat >/dev/null");
        let path = d.to_string_lossy().to_string();
        assert_eq!(player_command("aplay -q", &path).as_deref(), Some("aplay -q"));
        assert_eq!(player_command("nosuchplayer -x", &path).as_deref(), Some("aplay -q"), "falls back to the usual one");
        assert_eq!(player_command("nosuch", "/nonexistent"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn chat_replies_are_read_with_the_engine_found_at_the_speed_of_the_setting() {
        let d = tmp("chat");
        fakes(&d);
        let svc = crate::services::Services::open(&d.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        assert!(svc.effective_tts(None).is_none(), "detection is off until a program turns it on");
        svc.host_tts.configure(env_of(&d, Os::Linux));
        svc.host_tts.scan_now();
        assert!(matches!(svc.effective_tts(None), Some(SpeechEngine::Host { ref engine, .. }) if engine == "espeak-ng"));
        let mut p = svc.speak_player("Hello **there** https://example.org/x").unwrap();
        for _ in 0..100 {
            if d.join("log.in").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        p.stop();
        assert_eq!(std::fs::read_to_string(d.join("log.in")).unwrap().trim(), "Hello there", "markdown and addresses are not read");
        assert!(std::fs::read_to_string(d.join("log.args")).unwrap().contains("-s 175"));
        svc.settings.lock().unwrap().update(|s| s.voice.speed_pct = 200).unwrap();
        let mut p = svc.speak_player("Faster please").unwrap();
        for _ in 0..100 {
            if std::fs::read_to_string(d.join("log.args")).is_ok_and(|a| a.contains("-s 350")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        p.stop();
        assert!(std::fs::read_to_string(d.join("log.args")).unwrap().contains("-s 350"));
        // the story's file route works the same way
        let wav = svc.synthesize_with_opts("A sentence.", &voice::SpeakOpts { voice: None, speed: 1.0, lang: Some("ta-IN".into()) }).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert!(std::fs::read_to_string(d.join("log.args")).unwrap().contains("-v ta "), "the language picked the Tamil voice");
        // what the person configured always wins over what was found
        let own = SpeechEngine::Command { command: "sh -c 'cat >/dev/null'".into() };
        svc.settings.lock().unwrap().update(|s| s.voice.tts = Some(own.clone())).unwrap();
        assert_eq!(svc.effective_tts(None), Some(own));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_default_choice_order_is_configured_then_computer_then_browser_then_text() {
        use crate::hostvoice::{FromClient, Front, HostVoice};
        use crate::story::{effective_engine_auto, tts_status_auto, TtsStatus};
        let hv = HostVoice::new();
        let host = SpeechEngine::Host { engine: "espeak-ng".into(), program: Some("/nonexistent/espeak-ng".into()), voice: None };
        let own = SpeechEngine::Http { url: "http://127.0.0.1:1/v1/audio/speech".into(), model: "tts-1".into(), key_ref: None, voice: None, language: None };
        let none = voice::VoiceSettings::default();
        let set = voice::VoiceSettings { tts: Some(own.clone()), ..none.clone() };
        // nothing: text only, with the hint to set up
        assert_eq!(effective_engine_auto(&none, &hv, None), None);
        assert_eq!(tts_status_auto(&none, true, "/usr/bin", &hv, None), TtsStatus::NoEngine);
        // a browser that can speak
        hv.connect(Front::Browser);
        hv.event(FromClient::Voices { supported: true, voices: vec![] });
        assert_eq!(effective_engine_auto(&none, &hv, None), Some(SpeechEngine::Browser));
        // the engine of this computer beats the browser voice
        assert_eq!(effective_engine_auto(&none, &hv, Some(&host)), Some(host.clone()));
        // and what is configured beats both
        assert_eq!(effective_engine_auto(&set, &hv, Some(&host)), Some(own));
        // a found engine whose program went away is reported, not silently skipped
        assert!(matches!(tts_status_auto(&none, true, "/usr/bin", &hv, Some(&host)), TtsStatus::HostUnavailable(w) if w.contains("no longer there") && w.contains("Re-check")));
        // an account without voice gets none of it
        assert_eq!(tts_status_auto(&none, false, "/usr/bin", &hv, Some(&host)), TtsStatus::NotAllowed);
    }

    #[test]
    fn host_engine_settings_are_checked_and_round_trip() {
        let e = SpeechEngine::Host { engine: "piper".into(), program: Some("/usr/bin/piper".into()), voice: Some("/v/ta_IN-x-medium.onnx".into()) };
        assert!(e.validate().is_ok());
        let back: SpeechEngine = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        let bare: SpeechEngine = serde_json::from_str(r#"{"kind":"host","engine":"espeak-ng"}"#).unwrap();
        assert_eq!(bare, SpeechEngine::Host { engine: "espeak-ng".into(), program: None, voice: None });
        for bad in [SpeechEngine::Host { engine: "rm".into(), program: None, voice: None }, SpeechEngine::Host { engine: "piper".into(), program: None, voice: Some("-x".into()) }, SpeechEngine::Host { engine: "piper".into(), program: Some("a\nb".into()), voice: None }] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let stt = voice::VoiceSettings { stt: Some(e.clone()), ..voice::VoiceSettings::default() };
        assert!(stt.validate().unwrap_err().contains("only speak"), "an engine of this computer cannot be a speech-to-text engine");
        assert!(voice::VoiceSettings { speed_pct: 10, ..voice::VoiceSettings::default() }.validate().is_err());
        assert!(e.label().contains("piper") && e.label().contains("on this computer"));
        assert!(voice::transcribe(&e, b"", &crate::vault::Secrets::new(None, Arc::new(Off)), &Off).is_err());
    }

    /// What this computer really has (not part of the normal run): `cargo test -p iyakki-agent --lib real_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_machine_report() {
        let r = detect(&Env::current(&[]));
        for e in &r.engines {
            println!("{:?} at {} voices={} problem={:?} speaks ta={} hi={} te={} ml={} en={}", e.kind, e.program.display(), e.voices.len(), e.problem, e.speaks("ta-IN"), e.speaks("hi-IN"), e.speaks("te-IN"), e.speaks("ml-IN"), e.speaks("en-US"));
        }
        for (k, why) in &r.missing {
            println!("missing {k:?}: {why}");
        }
        println!("best: {:?}", r.best(Some("ta-IN")).map(|e| e.kind));
    }
}
