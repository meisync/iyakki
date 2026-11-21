//! Pure data of the host speech engines (moved out of `iyakki_agent::hostengines`, F-SPLIT-03); the agent re-exports them.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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


pub fn norm_lang(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('_', "-")
}

pub fn primary(s: &str) -> String {
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


#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HVoice {
    /// What is passed to the engine (a voice name, a language code or the path of a piper model).
    pub id: String,
    pub name: String,
    /// Normalised language tags it speaks (`ta`, `en-gb`).
    pub langs: Vec<String>,
}

impl HVoice {
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


#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
}


#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::voice::SpeechEngine;
use crate::voice;

/// The command that plays a WAV from standard input: the configured one when its program exists, else the first usual one.
use crate::story::which;

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

/// Where to look.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

// ---- moved from iyakki-agent (F-SPLIT-03) ----

/// The setting that makes the speech of this engine the one used.
pub fn engine_setting(e: &Engine) -> SpeechEngine {
    SpeechEngine::Host { engine: e.kind.id().to_string(), program: Some(e.program.to_string_lossy().to_string()), voice: None }
}
