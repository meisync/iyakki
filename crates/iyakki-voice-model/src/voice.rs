//! Types moved out of `iyakki_agent::voice` (F-SPLIT-03); the agent re-exports them.

use serde::{Deserialize, Serialize};



pub const SAMPLE_RATE: u32 = 16_000;

pub const DEFAULT_RECORD_CMD: &str = "arecord -q -f S16_LE -r 16000 -c 1 -t raw";

pub const DEFAULT_PLAY_CMD: &str = "aplay -q";

/// Per-request choices for text to speech (the STORY tile): another voice than the engine's configured one, and a speaking
/// speed (1.0 = normal). An HTTP engine gets `voice` and `speed` in the request body; a command gets them where it says
/// `{voice}`, `{speed}` (for example 1.2) or `{length_scale}` (1 / speed, what piper's `--length_scale` takes). A command
/// without those words simply ignores them.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeakOpts {
    pub voice: Option<String>,
    pub speed: f32,
    /// Language tag of the text (`ta-IN`): an engine of this computer picks a voice that speaks it.
    pub lang: Option<String>,
}

impl Default for SpeakOpts {
    fn default() -> Self {
        SpeakOpts { voice: None, speed: 1.0, lang: None }
    }
}

// ---- configuration (moved from iyakki_agent::voice) ----

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SpeechEngine {
    /// OpenAI-compatible server (OpenAI, speaches/faster-whisper-server, LocalAI, whisper.cpp server, …).
    Http {
        url: String,
        model: String,
        #[serde(default)]
        key_ref: Option<String>,
        /// TTS voice name (ignored for speech-to-text).
        #[serde(default)]
        voice: Option<String>,
        /// Spoken language hint for speech-to-text, e.g. `en`.
        #[serde(default)]
        language: Option<String>,
    },
    /// Local program. STT: `{wav}` is replaced by the audio file, text comes from stdout.
    /// TTS: the text arrives on stdin; audio is read from the file named by `{out}`, or from stdout.
    Command { command: String },
    /// Text to speech only: the voices of the browser (or desktop window) the person is using, through the Web Speech API
    /// (`hostvoice`). Nothing is made on this machine, so there is no audio file.
    Browser,
    /// Text to speech only: a speech engine already installed on this computer (espeak-ng, speech-dispatcher, piper, macOS `say`,
    /// Windows speech, ...), found by `hostengines`. `engine` is its id (`espeak-ng`), `program` where it was found (looked up
    /// again when empty) and `voice` the voice (empty: chosen from the language of what is read).
    Host {
        engine: String,
        #[serde(default)]
        program: Option<String>,
        #[serde(default)]
        voice: Option<String>,
    },
    /// Speech to text only (EXPERIMENTAL, optional): Google Gemini, the recording sent inline in one request (`crate::gemini`). Used only
    /// when the person chose it in Settings, and only after they answered yes to the consent card.
    Gemini {
        model: String,
        /// Spoken language hint, e.g. `en`.
        #[serde(default)]
        language: Option<String>,
        /// Where the key is (the vault entry the Settings page filled).
        #[serde(default = "gemini_key_ref")]
        key_ref: String,
        /// Only changes the advice when the limit is reached.
        #[serde(default)]
        tier: crate::Tier,
    },
}

fn gemini_key_ref() -> String {
    crate::KEY_REF.to_string()
}

impl SpeechEngine {
    pub fn label(&self) -> String {
        match self {
            SpeechEngine::Http { url, model, .. } => format!("HTTP · {model} · {}", iyakki_sec::url_host(url).unwrap_or_default()),
            SpeechEngine::Command { command } => format!("command · {}", command.split_whitespace().next().unwrap_or("")),
            SpeechEngine::Browser => "browser voice".to_string(),
            SpeechEngine::Gemini { model, tier, .. } => format!("Google Gemini · {model} · {} tier (experimental)", tier.label()),
            SpeechEngine::Host { engine, .. } => crate::hostengines::Kind::from_id(engine).map_or_else(|| format!("{engine} (this computer)"), |k| format!("{} (on this computer)", k.label())),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            SpeechEngine::Http { url, model, key_ref, .. } => {
                iyakki_sec::url_host(url).ok_or("URL must be http(s)://host[:port]/…")?;
                if model.trim().is_empty() || model.len() > 128 {
                    return Err("model name is required".into());
                }
                if let Some(r) = key_ref {
                    if !["vault:", "env:", "file:", "ext:"].iter().any(|p| r.starts_with(p)) {
                        return Err("key reference must start with vault:, env:, file: or ext:".into());
                    }
                }
                Ok(())
            }
            SpeechEngine::Command { command } => validate_command(command),
            SpeechEngine::Gemini { model, language, key_ref, .. } => {
                let m = model.trim().trim_start_matches("models/");
                if !crate::valid_model_name(m) || m.contains('/') {
                    return Err("model name is required (letters, digits and . _ : - + @)".into());
                }
                if !["vault:", "env:", "file:", "ext:"].iter().any(|p| key_ref.starts_with(p)) {
                    return Err("the Gemini key is kept in the vault: the reference starts with vault: (or env:, file:, ext:)".into());
                }
                if language.as_deref().is_some_and(|l| l.len() > 16 || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
                    return Err("language hint such as en or ta".into());
                }
                Ok(())
            }
            SpeechEngine::Browser => Ok(()),
            SpeechEngine::Host { engine, program, voice } => crate::hostengines::validate_host(engine, program.as_deref(), voice.as_deref()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub stt: Option<SpeechEngine>,
    pub tts: Option<SpeechEngine>,
    pub record_cmd: String,
    pub play_cmd: String,
    /// Send the transcribed text straight away instead of leaving it in the input box.
    pub auto_send: bool,
    /// Read assistant replies aloud.
    pub speak_replies: bool,
    /// Speaking speed of replies and of the voice test, in percent (100 = normal), 50 to 200.
    pub speed_pct: u32,
    /// What is lit while text is read aloud: the sentence and the word, the sentence only, or nothing.
    pub highlight: Highlight,
    /// Time the lit word from the sound itself: made speech is given to a speech-to-text server and what it heard is matched
    /// to the text (`speechtime`). Off: the word times are estimated.
    pub align_words: bool,
    /// Speech servers on another machine (`scheme://host:port`) the person agreed to send text and sound to.
    pub remote_ok: Vec<String>,
    /// The microphone the person chose (an ALSA id such as `plughw:1,0`), empty for automatic. One choice for the Voice tile's dictation and
    /// for the web pages in the Browser (`voicedesk::share_mic_choice` hands it to the web engine's microphone plugin).
    pub mic: String,
    /// A dictation ends by itself when the person pauses after talking (`voicedesk::PauseDetector`).
    pub auto_stop: bool,
    /// How long that pause is, in milliseconds (500 to 4000).
    pub pause_ms: u32,
    /// After a dictation ended by a pause and its text is in, listen again by itself (until the person presses Stop).
    pub keep_listening: bool,
    /// Correct the dictated wording with the chat assistant before it lands in the box (`dictfix`). Off by default.
    pub fix_wording: bool,
    /// The Assistant pane beside the Voice tile's transcript is open (`revise`). On by default; the person can collapse it.
    pub assistant_open: bool,
    /// "My IYAKKI", the personal voice assistant (push to talk), is on. Off by default: nothing listens until the person turns it on.
    pub my_on: bool,
    /// The assistant's microphone is muted (kept across restarts until the person unmutes).
    pub my_muted: bool,
    /// The follow-up window after an answer, in milliseconds (0 turns it off, up to 20000).
    pub my_follow_ms: u32,
    /// The local wake phrase "My IYAKKI" (`iyakki-wake`) may listen: opt-in, off by default, a second choice after `my_on`. Nothing listens
    /// continuously until the person turns this on; `my_muted` stops it at once.
    pub my_wake: bool,
    /// How easily the wake phrase triggers: 0 low (fewer false wakes, more misses), 1 normal, 2 high (the reverse). Moves the model's
    /// suggested threshold (`wakelisten::threshold_for`).
    pub my_wake_sens: u8,
    /// EXPERIMENTAL second check of the wake phrase: the clip that made the spotter fire goes to the speech-to-text engine (never to another machine the
    /// person has not agreed to) and the wake is dropped when the words are not there. Off by default.
    pub my_wake_confirm: bool,
    /// My IYAKKI on a cloud machine may be sent this device's microphone sound (F-VOICE-38). Off by default; the consent card asks again in every session.
    pub my_cloud_stream: bool,
    /// Take the assistant's own voice out of what the microphone heard while it spoke (`echocancel`, F-VOICE-36). Off by default.
    pub echo_cancel: bool,
    /// A spoken "yes" may approve an ordinary ask in a voice session (`voiceassist`). Off by default: a tap or a key approves. The hard-ask list never accepts a spoken yes, whatever this says.
    pub spoken_approvals: bool,
}

/// What is highlighted while text is read aloud (TALES, chat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Highlight {
    /// The sentence being read, and inside it the word being said.
    #[default]
    SentenceAndWord,
    /// The sentence only.
    Sentence,
    /// Nothing is lit (the page still scrolls along with the reading).
    Off,
}

impl Highlight {
    pub const ALL: [Highlight; 3] = [Highlight::SentenceAndWord, Highlight::Sentence, Highlight::Off];

    pub fn label(self) -> &'static str {
        match self {
            Highlight::SentenceAndWord => "Sentence + word",
            Highlight::Sentence => "Sentence only",
            Highlight::Off => "Off",
        }
    }

    pub fn sentence(self) -> bool {
        self != Highlight::Off
    }

    pub fn word(self) -> bool {
        self == Highlight::SentenceAndWord
    }
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { stt: None, tts: None, record_cmd: DEFAULT_RECORD_CMD.into(), play_cmd: DEFAULT_PLAY_CMD.into(), auto_send: false, speak_replies: false, speed_pct: 100, highlight: Highlight::default(), align_words: true, remote_ok: Vec::new(), mic: String::new(), auto_stop: true, pause_ms: 1500, keep_listening: false, fix_wording: false, assistant_open: true, my_on: false, my_muted: false, my_follow_ms: 8000, my_wake: false, my_wake_sens: 1, my_wake_confirm: false, my_cloud_stream: false, echo_cancel: false, spoken_approvals: false }
    }
}

impl VoiceSettings {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(e) = &self.stt {
            if matches!(e, SpeechEngine::Browser | SpeechEngine::Host { .. }) {
                return Err("speech-to-text: the browser voice and the speech engines of this computer only speak; they cannot listen".into());
            }
            e.validate().map_err(|m| format!("speech-to-text: {m}"))?;
        }
        if let Some(e) = &self.tts {
            e.validate().map_err(|m| format!("text-to-speech: {m}"))?;
        }
        if !(50..=200).contains(&self.speed_pct) {
            return Err("speaking speed must be between 50 and 200 percent".into());
        }
        if !(500..=4000).contains(&self.pause_ms) {
            return Err("the pause that ends a dictation must be between 0.5 and 4 seconds".into());
        }
        if self.my_follow_ms > 20_000 {
            return Err("the follow-up window must be between 0 and 20 seconds".into());
        }
        if self.my_wake_sens > 2 {
            return Err("the wake phrase sensitivity must be 0 (low), 1 (normal) or 2 (high)".into());
        }
        if self.mic.len() > 30 || !self.mic.chars().all(|c| c.is_ascii_alphanumeric() || ":,_-".contains(c)) {
            return Err("microphone: not a sound device name".into());
        }
        validate_command(&self.record_cmd).map_err(|m| format!("record command: {m}"))?;
        validate_command(&self.play_cmd).map_err(|m| format!("play command: {m}"))?;
        Ok(())
    }
}

// ------------------------------------------------------------------ command lines (no shell)

/// Split a command line into arguments. Supports single and double quotes and backslash escapes
/// inside double quotes. There is no shell: no expansion, pipes or redirection.
pub fn split_command(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                in_word = true;
            }
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => match chars.next() {
                Some(n @ ('"' | '\\')) => cur.push(n),
                Some(n) => {
                    cur.push('\\');
                    cur.push(n);
                }
                None => return Err("dangling backslash".into()),
            },
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (_, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

pub fn validate_command(cmd: &str) -> Result<(), String> {
    let args = split_command(cmd)?;
    let prog = args.first().ok_or("command is empty")?;
    if prog.contains('\0') || cmd.contains('\n') || cmd.len() > 1000 {
        return Err("command contains invalid characters or is too long".into());
    }
    Ok(())
}


// ------------------------------------------------------------------ WAV and audio helpers

pub fn encode_wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut o = Vec::with_capacity(44 + pcm.len() * 2);
    o.extend(b"RIFF");
    o.extend((36 + data_len).to_le_bytes());
    o.extend(b"WAVEfmt ");
    o.extend(16u32.to_le_bytes());
    o.extend(1u16.to_le_bytes()); // PCM
    o.extend(1u16.to_le_bytes()); // mono
    o.extend(rate.to_le_bytes());
    o.extend((rate * 2).to_le_bytes());
    o.extend(2u16.to_le_bytes());
    o.extend(16u16.to_le_bytes());
    o.extend(b"data");
    o.extend(data_len.to_le_bytes());
    for s in pcm {
        o.extend(s.to_le_bytes());
    }
    o
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

/// Parse a WAV file: 16-bit PCM directly; 32/64-bit float (Kokoro answers 32-bit float), 24/32-bit and 8-bit integer samples
/// are brought to 16-bit first (`audioout::to_pcm16_wav`), so every caller (loudness and length of a sentence, alignment,
/// joining) sees the sound the speaker plays.
pub fn parse_wav(b: &[u8]) -> Result<Wav, String> {
    match parse_wav16(b) {
        Err(e) if e.starts_with("only 16-bit") => match crate::pcm::to_pcm16_wav(b) {
            Some(w16) => parse_wav16(&w16),
            None => Err(e),
        },
        r => r,
    }
}

fn parse_wav16(b: &[u8]) -> Result<Wav, String> {
    if b.len() < 44 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut pos, mut fmt, mut data): (usize, Option<(u16, u16, u32, u16)>, Option<&[u8]>) = (12, None, None);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = b.get(pos + 8..(pos + 8 + len).min(b.len())).ok_or("truncated WAV chunk")?;
        match id {
            b"fmt " if body.len() >= 16 => {
                fmt = Some((
                    u16::from_le_bytes([body[0], body[1]]),
                    u16::from_le_bytes([body[2], body[3]]),
                    u32::from_le_bytes(body[4..8].try_into().unwrap()),
                    u16::from_le_bytes([body[14], body[15]]),
                ))
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or("WAV has no fmt chunk")?;
    if format != 1 || bits != 16 {
        return Err(format!("only 16-bit PCM WAV is supported (got format {format}, {bits} bits)"));
    }
    if channels == 0 || rate == 0 {
        return Err("invalid WAV header".into());
    }
    let samples = data.ok_or("WAV has no data chunk")?.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    Ok(Wav { rate, channels, samples })
}

/// Downmix to mono and resample (linear) to 16 kHz — what speech engines expect.
pub fn to_mono_16k(w: &Wav) -> Vec<i16> {
    let ch = w.channels as usize;
    let mono: Vec<f32> = w.samples.chunks(ch).map(|f| f.iter().map(|&x| x as f32).sum::<f32>() / f.len() as f32).collect();
    if w.rate == SAMPLE_RATE || mono.is_empty() {
        return mono.iter().map(|&x| x as i16).collect();
    }
    let ratio = w.rate as f64 / SAMPLE_RATE as f64;
    let n = (mono.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * ratio;
            let (a, frac) = (p as usize, (p - p.floor()) as f32);
            let (x0, x1) = (mono[a.min(mono.len() - 1)], mono[(a + 1).min(mono.len() - 1)]);
            (x0 + (x1 - x0) * frac) as i16
        })
        .collect()
}

pub fn peak(pcm: &[i16]) -> i16 {
    pcm.iter().map(|s| s.saturating_abs()).max().unwrap_or(0)
}

/// A short test tone (for checking the speaker).
pub fn tone(freq: f32, ms: u32) -> Vec<i16> {
    let n = SAMPLE_RATE as usize * ms as usize / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (i.min(n - i) as f32 / 800.0).min(1.0); // fade in/out to avoid clicks
            ((t * freq * std::f32::consts::TAU).sin() * 9000.0 * env) as i16
        })
        .collect()
}


/// Remove things that sound terrible read aloud: code blocks, URLs, markdown marks.
pub fn speakable(text: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let l: String = line.split_whitespace().filter(|w| !w.starts_with("http://") && !w.starts_with("https://")).collect::<Vec<_>>().join(" ");
        let l: String = l.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>')).collect();
        out.push_str(l.trim());
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(1500).collect()
}

pub fn engine_takes_speed(engine: &SpeechEngine) -> bool {
    match engine {
        SpeechEngine::Http { .. } => true,
        SpeechEngine::Command { command } => command.contains("{speed}") || command.contains("{length_scale}"),
        SpeechEngine::Browser => true,
        SpeechEngine::Host { engine, .. } => engine != "pico2wave",
        SpeechEngine::Gemini { .. } => false,
    }
}
