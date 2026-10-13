//! Voice: microphone → speech-to-text, and text-to-speech → speaker.
//!
//! Design: *nothing is linked into the binary*. Local hardware is reached by plain commands
//! (`arecord`/`aplay` by default; PipeWire or PulseAudio tools work too), and speech engines are
//! either an OpenAI-compatible HTTP endpoint (`/v1/audio/transcriptions`, `/v1/audio/speech`) or a
//! local command such as `whisper-cli` or `piper`. Commands are run **without a shell**, in a private
//! temp directory, with a minimal environment and a timeout. They are configured by admins only.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::vault::{LocalVault, Secrets};

pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_RECORD_SECS: usize = 120;
pub const DEFAULT_RECORD_CMD: &str = "arecord -q -f S16_LE -r 16000 -c 1 -t raw";
pub const DEFAULT_PLAY_CMD: &str = "aplay -q";

// ------------------------------------------------------------------ configuration

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
}

impl SpeechEngine {
    pub fn label(&self) -> String {
        match self {
            SpeechEngine::Http { url, model, .. } => format!("HTTP · {model} · {}", iyakki_sec::url_host(url).unwrap_or_default()),
            SpeechEngine::Command { command } => format!("command · {}", command.split_whitespace().next().unwrap_or("")),
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
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self { stt: None, tts: None, record_cmd: DEFAULT_RECORD_CMD.into(), play_cmd: DEFAULT_PLAY_CMD.into(), auto_send: false, speak_replies: false }
    }
}

impl VoiceSettings {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(e) = &self.stt {
            e.validate().map_err(|m| format!("speech-to-text: {m}"))?;
        }
        if let Some(e) = &self.tts {
            e.validate().map_err(|m| format!("text-to-speech: {m}"))?;
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

fn private_tempdir(tag: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut r = [0u8; 8];
    iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
    let name: String = r.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("iyakki-{tag}-{name}"));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Kill a child *and everything it started* (it runs in its own process group), so a wrapper such as
/// `sh -c '… | …'` cannot leave a grandchild holding the audio device or the output pipe open.
fn kill_tree(child: &mut Child) {
    // SAFETY: signalling a process group we created with `process_group(0)`.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

fn command_for(args: &[String], cwd: Option<&Path>) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(&args[0]);
    c.process_group(0);
    c.args(&args[1..]).env_clear().env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").env("LANG", "C.UTF-8");
    if let Some(h) = std::env::var_os("HOME") {
        c.env("HOME", h);
    }
    for k in ["XDG_RUNTIME_DIR", "PULSE_SERVER", "PIPEWIRE_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
        // audio servers on a desktop need these; harmless elsewhere
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c
}

/// Run to completion with a timeout; returns (stdout, stderr). The child is killed on timeout.
fn run_capture(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<(Vec<u8>, String), String> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start `{}`: {e}", cmd.get_program().to_string_lossy()))?;
    if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = si.write_all(&data); // drop closes stdin
        });
    }
    let (mut so, mut se) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    let t_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.by_ref().take(64 << 20).read_to_end(&mut v);
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.by_ref().take(1 << 16).read_to_end(&mut v);
        String::from_utf8_lossy(&v).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                kill_tree(&mut child);
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (out, err) = (t_out.join().unwrap_or_default(), t_err.join().unwrap_or_default());
    if !status.success() {
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        return Err(format!("exited with {status}{}", if tail.is_empty() { String::new() } else { format!(": {tail}") }));
    }
    Ok((out, err))
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

/// Parse a 16-bit PCM WAV file.
pub fn parse_wav(b: &[u8]) -> Result<Wav, String> {
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

pub fn multipart(fields: &[(&str, &str)], file_field: &str, filename: &str, mime: &str, data: &[u8]) -> (Vec<u8>, String) {
    let mut r = [0u8; 12];
    let _ = iyakki_sec::random_fill(&mut r);
    let boundary = format!("----iyakki{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend(data);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    (body, boundary)
}

// ------------------------------------------------------------------ microphone

/// Records raw 16 kHz mono PCM from the configured command until [`Recorder::stop`].
pub struct Recorder {
    child: Child,
    buf: Arc<Mutex<Vec<u8>>>,
    started: Instant,
    reader: Option<std::thread::JoinHandle<()>>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Recorder {
    pub fn start(record_cmd: &str) -> Result<Recorder, String> {
        let args = split_command(record_cmd)?;
        let prog = args.first().ok_or("record command is empty")?;
        let mut child = command_for(&args, None)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start `{prog}`: {e} (is alsa-utils installed? change the record command in Settings → Voice)"))?;
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (mut so, mut se) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
        let b2 = buf.clone();
        let reader = std::thread::spawn(move || {
            let max = SAMPLE_RATE as usize * 2 * MAX_RECORD_SECS;
            let mut chunk = [0u8; 4096];
            while let Ok(n) = so.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut g = b2.lock().unwrap();
                if g.len() < max {
                    let room = max - g.len();
                    g.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut v = Vec::new();
            let _ = se.by_ref().take(1 << 16).read_to_end(&mut v);
            String::from_utf8_lossy(&v).into_owned()
        });
        Ok(Recorder { child, buf, started: Instant::now(), reader: Some(reader), stderr: Some(stderr) })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn is_full(&self) -> bool {
        self.buf.lock().unwrap().len() >= SAMPLE_RATE as usize * 2 * MAX_RECORD_SECS
    }

    /// Loudness of the last ~150 ms, 0.0–1.0 (for a level meter).
    pub fn level(&self) -> f32 {
        let g = self.buf.lock().unwrap();
        let n = (SAMPLE_RATE as usize * 2 * 150 / 1000).min(g.len()) & !1;
        let tail = &g[g.len() - n..];
        let pk = tail.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).saturating_abs()).max().unwrap_or(0);
        (pk as f32 / 12000.0).min(1.0)
    }

    /// Whether the recording process is still running (it exits early if the device is missing/busy).
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stop recording and return the samples.
    pub fn stop(mut self) -> Result<Vec<i16>, String> {
        kill_tree(&mut self.child);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        let err = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
        let bytes = std::mem::take(&mut *self.buf.lock().unwrap());
        if bytes.is_empty() {
            let why: String = err.lines().next().unwrap_or("no audio was captured").chars().take(160).collect();
            return Err(format!("the microphone produced no audio: {why}"));
        }
        Ok(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        kill_tree(&mut self.child);
    }
}

// ------------------------------------------------------------------ speaker

/// Start the play command with a pipe on its stdin, for audio that arrives as it goes (radio). Kill it with [`kill_child`].
pub fn spawn_player(play_cmd: &str) -> Result<Child, String> {
    let args = split_command(play_cmd)?;
    let prog = args.first().ok_or("play command is empty")?;
    command_for(&args, None)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start `{prog}`: {e} (change the play command in Settings → Voice)"))
}

// ------------------------------------------------------------------ media players

/// The program and arguments that play media: the configured command, else the first of the usual players found.
pub fn find_media_player(configured: &str) -> Option<Vec<String>> {
    let on_path = |prog: &str| -> Option<String> {
        if prog.contains('/') {
            return std::path::Path::new(prog).is_file().then(|| prog.to_string());
        }
        let path = std::env::var("PATH").unwrap_or_default();
        let found = path.split(':').chain(["/snap/bin", "/usr/local/bin", "/usr/bin"]).map(|d| format!("{d}/{prog}")).find(|p| std::path::Path::new(p).is_file());
        found
    };
    if !configured.trim().is_empty() {
        let mut argv = split_command(configured).ok()?;
        argv[0] = on_path(&argv[0])?;
        return Some(argv);
    }
    for (prog, args) in [("mpv", &["--force-window=immediate", "--no-terminal"][..]), ("vlc", &["--play-and-exit"][..]), ("ffplay", &["-autoexit", "-loglevel", "quiet"][..]), ("mplayer", &[][..])] {
        if let Some(p) = on_path(prog) {
            return Some(std::iter::once(p).chain(args.iter().map(|a| a.to_string())).collect());
        }
    }
    None
}

/// Start the player on its own: no shell, the address is one argument, and the program is left running.
pub fn launch_media_player(argv: &[String], url: &str) -> Result<(), String> {
    let mut child = spawn_media_player(argv, url)?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Like [`launch_media_player`] but the caller keeps the process, to see whether it still plays and to stop it
/// ([`kill_child`] ends the whole group).
pub fn spawn_media_player(argv: &[String], url: &str) -> Result<Child, String> {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg(url)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", argv[0]))?;
    // a player that quits at once (no display, a missing codec, a sandbox that blocks it) is reported, not assumed to work
    for _ in 0..40 {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("{} stopped right away (exit status {}); it may have no window or sound here", argv[0].rsplit('/').next().unwrap_or(&argv[0]), status.code().map_or("signal".into(), |c| c.to_string()))),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    Ok(child)
}

pub fn kill_child(child: &mut Child) {
    kill_tree(child);
}

pub struct Player {
    child: Child,
}

impl Player {
    /// Start playing a WAV file through the play command (audio is written to its stdin).
    pub fn start(play_cmd: &str, wav: &[u8]) -> Result<Player, String> {
        let args = split_command(play_cmd)?;
        let prog = args.first().ok_or("play command is empty")?;
        let mut child = command_for(&args, None)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start `{prog}`: {e} (change the play command in Settings → Voice)"))?;
        if let Some(mut si) = child.stdin.take() {
            let data = wav.to_vec();
            std::thread::spawn(move || {
                let _ = si.write_all(&data);
            });
        }
        Ok(Player { child })
    }

    pub fn is_playing(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn wait(&mut self, timeout: Duration) -> bool {
        let t = Instant::now();
        while self.is_playing() {
            if t.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    pub fn stop(&mut self) {
        kill_tree(&mut self.child);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------------ engines

fn key_headers(engine_key: &Option<String>, secrets: &Secrets) -> Result<Vec<(String, String)>, String> {
    let mut h = Vec::new();
    if let Some(r) = engine_key {
        let k = secrets.resolve(r, None::<&LocalVault>).map_err(|e| format!("API key unavailable: {e}"))?;
        h.push(("Authorization".to_string(), format!("Bearer {}", &*k)));
    }
    Ok(h)
}

fn clean_transcript(raw: &str) -> String {
    // whisper.cpp prints markers like [BLANK_AUDIO] or (music); drop bracketed annotations
    let mut out = String::new();
    let mut depth = 0u32;
    for c in raw.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Speech to text. `wav` must be a WAV file (16 kHz mono works everywhere).
pub fn transcribe(engine: &SpeechEngine, wav: &[u8], secrets: &Secrets, upstream: &dyn Upstream) -> Result<String, String> {
    match engine {
        SpeechEngine::Http { url, model, key_ref, language, .. } => {
            let mut fields = vec![("model", model.as_str()), ("response_format", "json")];
            if let Some(l) = language {
                fields.push(("language", l.as_str()));
            }
            let (body, boundary) = multipart(&fields, "file", "audio.wav", "audio/wav", wav);
            let mut headers = key_headers(key_ref, secrets)?;
            headers.push(("Content-Type".into(), format!("multipart/form-data; boundary={boundary}")));
            let r = upstream.request("POST", url, &headers, &body)?;
            if !(200..300).contains(&r.status) {
                let msg = serde_json::from_slice::<Value>(&r.body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| String::from_utf8_lossy(&r.body).chars().take(160).collect());
                return Err(format!("HTTP {}: {}", r.status, msg.trim()));
            }
            let v: Value = serde_json::from_slice(&r.body).map_err(|_| "speech endpoint returned something that is not JSON".to_string())?;
            let text = v.get("text").and_then(Value::as_str).ok_or("response has no `text` field")?;
            Ok(clean_transcript(text))
        }
        SpeechEngine::Command { command } => {
            let dir = private_tempdir("stt")?;
            let wav_path = dir.join("audio.wav");
            let result = (|| {
                std::fs::write(&wav_path, wav).map_err(|e| e.to_string())?;
                let args: Vec<String> = split_command(command)?.into_iter().map(|a| a.replace("{wav}", &wav_path.to_string_lossy())).collect();
                if !command.contains("{wav}") {
                    return Err("the command must contain {wav} where the audio file path goes".to_string());
                }
                let (out, _) = run_capture(command_for(&args, Some(&dir)), None, Duration::from_secs(180))?;
                Ok(clean_transcript(&String::from_utf8_lossy(&out)))
            })();
            let _ = std::fs::remove_dir_all(&dir);
            result
        }
    }
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

/// Text to speech; returns a WAV file.
pub fn synthesize(engine: &SpeechEngine, text: &str, secrets: &Secrets, upstream: &dyn Upstream) -> Result<Vec<u8>, String> {
    let text = speakable(text);
    if text.is_empty() {
        return Err("nothing to say".into());
    }
    let audio = match engine {
        SpeechEngine::Http { url, model, key_ref, voice, .. } => {
            let mut headers = key_headers(key_ref, secrets)?;
            headers.push(("Content-Type".into(), "application/json".into()));
            let body = json!({"model": model, "input": text, "voice": voice.as_deref().unwrap_or("alloy"), "response_format": "wav"}).to_string();
            let r = upstream.request("POST", url, &headers, body.as_bytes())?;
            if !(200..300).contains(&r.status) {
                return Err(format!("HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim()));
            }
            r.body
        }
        SpeechEngine::Command { command } => {
            let dir = private_tempdir("tts")?;
            let out_path = dir.join("speech.wav");
            let result = (|| {
                let args: Vec<String> = split_command(command)?.into_iter().map(|a| a.replace("{out}", &out_path.to_string_lossy())).collect();
                let (stdout, _) = run_capture(command_for(&args, Some(&dir)), Some(text.as_bytes()), Duration::from_secs(180))?;
                if command.contains("{out}") { std::fs::read(&out_path).map_err(|e| format!("the command did not write {{out}}: {e}")) } else { Ok(stdout) }
            })();
            let _ = std::fs::remove_dir_all(&dir);
            result?
        }
    };
    if audio.len() < 44 || &audio[..4] != b"RIFF" {
        return Err("the speech engine did not return WAV audio (set its output format to wav)".into());
    }
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-voice-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn wav_roundtrip_and_validation() {
        let pcm: Vec<i16> = (0..1000).map(|i| ((i as f32 * 0.1).sin() * 8000.0) as i16).collect();
        let wav = encode_wav(&pcm, 16_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 2000);
        let p = parse_wav(&wav).unwrap();
        assert_eq!((p.rate, p.channels, p.samples.clone()), (16_000, 1, pcm));
        assert!(parse_wav(b"nope").is_err());
        assert!(parse_wav(&wav[..40]).is_err());
        let mut bad = wav.clone();
        bad[34] = 8; // 8 bits per sample
        assert!(parse_wav(&bad).unwrap_err().contains("16-bit"));
        // extra chunks before `data` are skipped
        let mut with_list = wav[..36].to_vec();
        with_list.extend(b"LIST");
        with_list.extend(4u32.to_le_bytes());
        with_list.extend(b"abcd");
        with_list.extend(&wav[36..]);
        assert_eq!(parse_wav(&with_list).unwrap().samples.len(), 1000);
    }

    #[test]
    fn downmix_and_resample() {
        let stereo = Wav { rate: 16_000, channels: 2, samples: vec![1000, 3000, -2000, 2000] };
        assert_eq!(to_mono_16k(&stereo), vec![2000, 0]);
        let high = Wav { rate: 48_000, channels: 1, samples: (0..4800).map(|i| (i % 100) as i16).collect() };
        let down = to_mono_16k(&high);
        assert!((1595..=1605).contains(&down.len()), "{}", down.len());
        let low = Wav { rate: 8_000, channels: 1, samples: vec![0; 800] };
        assert!((1595..=1605).contains(&to_mono_16k(&low).len()), "upsampled to 16 kHz");
        assert!(to_mono_16k(&Wav { rate: 44_100, channels: 1, samples: vec![] }).is_empty());
        assert_eq!(peak(&[5, -9000, 100]), 9000);
        assert_eq!(peak(&[i16::MIN]), i16::MAX);
        let t = tone(440.0, 100);
        assert_eq!(t.len(), 1600);
        assert!(peak(&t) > 5000 && t[0].abs() < 200, "faded in");
    }

    #[test]
    fn command_lines_without_a_shell() {
        assert_eq!(split_command("a b  c").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(split_command(r#"sh -c 'echo "hi there"; sleep 1'"#).unwrap(), vec!["sh", "-c", "echo \"hi there\"; sleep 1"]);
        assert_eq!(split_command(r#"x "a \"quoted\" b" ''"#).unwrap(), vec!["x", "a \"quoted\" b", ""]);
        assert!(split_command("a 'unterminated").is_err());
        assert!(validate_command("").is_err());
        assert!(validate_command("   ").is_err());
        assert!(validate_command("arecord -q").is_ok());
        assert!(validate_command("a\nb").is_err());
        // `;` `|` `$()` are plain characters, not shell syntax
        assert_eq!(split_command("echo a;b|c $(x)").unwrap(), vec!["echo", "a;b|c", "$(x)"]);
    }

    #[test]
    fn multipart_body_is_well_formed() {
        let (body, boundary) = multipart(&[("model", "whisper-1"), ("language", "en")], "file", "audio.wav", "audio/wav", b"RIFFdata");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n")));
        assert!(text.contains("name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFdata\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn recorder_collects_audio_reports_level_and_stops_promptly() {
        let d = tmp("rec");
        let pcm: Vec<u8> = (0..16_000).flat_map(|i| (((i as f32 * 0.2).sin() * 9000.0) as i16).to_le_bytes()).collect();
        std::fs::write(d.join("tone.raw"), &pcm).unwrap();
        // a fake microphone: emit one second of audio, then keep the device "open"
        let cmd = format!("sh -c 'cat {}; sleep 30'", d.join("tone.raw").display());
        let rec = Recorder::start(&cmd).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(rec.level() > 0.3, "level meter sees the signal: {}", rec.level());
        assert!(rec.elapsed() >= Duration::from_millis(300));
        let t = Instant::now();
        let samples = rec.stop().unwrap();
        assert!(t.elapsed() < Duration::from_secs(3), "stop() does not wait for the 30 s sleep");
        assert_eq!(samples.len(), 16_000);
        assert_eq!(peak(&samples), peak(&pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()));
    }

    #[test]
    fn recorder_errors_are_explained() {
        let e = Recorder::start("/no/such/mic-tool -x").err().unwrap();
        assert!(e.contains("cannot start") && e.contains("Settings"), "{e}");
        let rec = Recorder::start("sh -c 'echo \"arecord: main:830: audio open error: No such file or directory\" >&2'").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let e = rec.stop().unwrap_err();
        assert!(e.contains("produced no audio") && e.contains("audio open error"), "{e}");
    }

    #[test]
    fn player_feeds_wav_to_the_command_and_can_be_stopped() {
        let d = tmp("play");
        let out = d.join("played.wav");
        let wav = encode_wav(&tone(440.0, 200), 16_000);
        let mut p = Player::start(&format!("sh -c 'cat > {}'", out.display()), &wav).unwrap();
        assert!(p.wait(Duration::from_secs(5)));
        assert_eq!(std::fs::read(&out).unwrap(), wav, "the speaker command received exactly the WAV");
        let mut long = Player::start("sleep 30", &wav).unwrap();
        assert!(long.is_playing());
        let t = Instant::now();
        long.stop();
        assert!(!long.is_playing() && t.elapsed() < Duration::from_secs(2));
        assert!(Player::start("/no/such/player", &wav).is_err());
    }

    struct Mock {
        calls: Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>,
        reply: Box<dyn Fn() -> (u16, Vec<u8>) + Send + Sync>,
    }
    impl Upstream for Mock {
        fn request(&self, _m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((url.into(), h.to_vec(), b.to_vec()));
            let (status, body) = (self.reply)();
            Ok(UpstreamResponse { status, body })
        }
    }
    fn mock(f: impl Fn() -> (u16, Vec<u8>) + Send + Sync + 'static) -> Arc<Mock> {
        Arc::new(Mock { calls: Default::default(), reply: Box::new(f) })
    }
    fn secrets(m: &Arc<Mock>) -> Secrets {
        Secrets::new(None, m.clone())
    }

    #[test]
    fn http_transcription_request_and_response() {
        std::env::set_var("IYAKKI_TEST_STT_KEY", "STTKEY");
        let m = mock(|| (200, br#"{"text":" Hello from the microphone. [BLANK_AUDIO] "}"#.to_vec()));
        let e = SpeechEngine::Http { url: "http://stt.lan:8080/v1/audio/transcriptions".into(), model: "whisper-1".into(), key_ref: Some("env:IYAKKI_TEST_STT_KEY".into()), voice: None, language: Some("en".into()) };
        let wav = encode_wav(&tone(300.0, 100), 16_000);
        let text = transcribe(&e, &wav, &secrets(&m), m.as_ref()).unwrap();
        assert_eq!(text, "Hello from the microphone.", "annotations stripped");
        let c = m.calls.lock().unwrap()[0].clone();
        assert_eq!(c.0, "http://stt.lan:8080/v1/audio/transcriptions");
        assert!(c.1.contains(&("Authorization".into(), "Bearer STTKEY".into())));
        let ct = c.1.iter().find(|(k, _)| k == "Content-Type").unwrap().1.clone();
        assert!(ct.starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8_lossy(&c.2).into_owned();
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1") && body.contains("name=\"language\"\r\n\r\nen") && body.contains("RIFF"));
    }

    #[test]
    fn http_transcription_errors() {
        let e = SpeechEngine::Http { url: "http://x/v1".into(), model: "m".into(), key_ref: None, voice: None, language: None };
        let m = mock(|| (401, br#"{"error":{"message":"bad key"}}"#.to_vec()));
        assert_eq!(transcribe(&e, b"RIFF", &secrets(&m), m.as_ref()).unwrap_err(), "HTTP 401: bad key");
        let m = mock(|| (200, b"<html>".to_vec()));
        assert!(transcribe(&e, b"RIFF", &secrets(&m), m.as_ref()).is_err());
        let m = mock(|| (200, br#"{"nope":1}"#.to_vec()));
        assert!(transcribe(&e, b"RIFF", &secrets(&m), m.as_ref()).unwrap_err().contains("text"));
        // a missing key reference fails before any request
        let keyed = SpeechEngine::Http { url: "http://x/v1".into(), model: "m".into(), key_ref: Some("vault:nope".into()), voice: None, language: None };
        let m = mock(|| (200, b"{}".to_vec()));
        assert!(transcribe(&keyed, b"RIFF", &secrets(&m), m.as_ref()).unwrap_err().contains("API key unavailable"));
        assert!(m.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn command_transcription_runs_without_a_shell_and_cleans_up() {
        let d = tmp("sttcmd");
        let script = d.join("fake-whisper.sh");
        std::fs::write(&script, "#!/bin/sh\n# $1 = wav path; prove it exists and is a RIFF file, then 'transcribe'\nhead -c 4 \"$1\" | grep -q RIFF || exit 3\necho '[BLANK_AUDIO] turn on the lights (music)'\n").unwrap();
        let m = mock(|| (500, vec![]));
        let e = SpeechEngine::Command { command: format!("sh {} {{wav}}", script.display()) };
        let text = transcribe(&e, &encode_wav(&tone(300.0, 100), 16_000), &secrets(&m), m.as_ref()).unwrap();
        assert_eq!(text, "turn on the lights");
        // missing placeholder, failing command, hanging command
        let no_ph = SpeechEngine::Command { command: "echo hi".into() };
        assert!(transcribe(&no_ph, b"RIFF", &secrets(&m), m.as_ref()).unwrap_err().contains("{wav}"));
        let fails = SpeechEngine::Command { command: "sh -c 'echo model not found >&2; exit 2' {wav}".into() };
        let err = transcribe(&fails, b"RIFF", &secrets(&m), m.as_ref()).unwrap_err();
        assert!(err.contains("exited with") && err.contains("model not found"), "{err}");
        // no temp directories are left behind
        let leftovers = std::fs::read_dir(std::env::temp_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("iyakki-stt-")).count();
        assert_eq!(leftovers, 0, "private temp dirs are removed");
    }

    #[test]
    fn hanging_commands_are_killed_including_their_children() {
        let m = mock(|| (500, vec![]));
        // stdin-less STT command that spawns a long-running grandchild and never exits by itself
        let e = SpeechEngine::Command { command: "sh -c 'sleep 60 & wait' {wav}".into() };
        let t = Instant::now();
        // use a short internal timeout by calling the runner directly
        let r = run_capture(command_for(&split_command("sh -c 'sleep 60 & wait'").unwrap(), None), None, Duration::from_millis(400));
        assert!(r.unwrap_err().contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(5), "returned promptly: {:?}", t.elapsed());
        let _ = (e, m);
    }

    #[test]
    fn tts_http_and_command_engines() {
        let wav = encode_wav(&tone(500.0, 100), 22_050);
        let w2 = wav.clone();
        let m = mock(move || (200, w2.clone()));
        let e = SpeechEngine::Http { url: "http://tts.lan/v1/audio/speech".into(), model: "tts-1".into(), key_ref: None, voice: Some("nova".into()), language: None };
        assert_eq!(synthesize(&e, "Hello **there**, see https://example.com/x for `code`.", &secrets(&m), m.as_ref()).unwrap(), wav);
        let sent: Value = serde_json::from_slice(&m.calls.lock().unwrap()[0].2).unwrap();
        assert_eq!((sent["voice"].as_str(), sent["response_format"].as_str(), sent["model"].as_str()), (Some("nova"), Some("wav"), Some("tts-1")));
        assert_eq!(sent["input"], "Hello there, see for code.", "markdown and URLs are not read aloud");
        // an engine that returns mp3 is rejected with a helpful message
        let m = mock(|| (200, b"ID3\x04mp3data....................................................".to_vec()));
        assert!(synthesize(&e, "hi", &secrets(&m), m.as_ref()).unwrap_err().contains("wav"));
        let m = mock(|| (429, b"slow down".to_vec()));
        assert!(synthesize(&e, "hi", &secrets(&m), m.as_ref()).unwrap_err().starts_with("HTTP 429"));
        assert!(synthesize(&e, "```\ncode only\n```", &secrets(&m), m.as_ref()).unwrap_err().contains("nothing to say"));
        // command engine: text on stdin, WAV via {out}
        let d = tmp("ttscmd");
        let src = d.join("canned.wav");
        std::fs::write(&src, &wav).unwrap();
        let m = mock(|| (500, vec![]));
        let via_out = SpeechEngine::Command { command: format!("sh -c 'cat > {d}/said.txt; cp {src} \"$0\"' {{out}}", d = d.display(), src = src.display()) };
        assert_eq!(synthesize(&via_out, "say this", &secrets(&m), m.as_ref()).unwrap(), wav);
        assert_eq!(std::fs::read_to_string(d.join("said.txt")).unwrap(), "say this", "text arrived on stdin, not in argv");
        // command engine: WAV on stdout
        let via_stdout = SpeechEngine::Command { command: format!("sh -c 'cat > /dev/null; cat {}'", src.display()) };
        assert_eq!(synthesize(&via_stdout, "x y", &secrets(&m), m.as_ref()).unwrap(), wav);
        let garbage = SpeechEngine::Command { command: "sh -c 'cat >/dev/null; echo not-audio'".into() };
        assert!(synthesize(&garbage, "x y", &secrets(&m), m.as_ref()).is_err());
    }

    #[test]
    fn engine_and_settings_validation() {
        let ok = SpeechEngine::Http { url: "https://api.openai.com/v1/audio/transcriptions".into(), model: "whisper-1".into(), key_ref: Some("vault:openai-key".into()), voice: None, language: None };
        assert!(ok.validate().is_ok());
        assert!(ok.label().contains("api.openai.com"));
        for bad in [
            SpeechEngine::Http { url: "ftp://x".into(), model: "m".into(), key_ref: None, voice: None, language: None },
            SpeechEngine::Http { url: "http://x".into(), model: " ".into(), key_ref: None, voice: None, language: None },
            SpeechEngine::Http { url: "http://x".into(), model: "m".into(), key_ref: Some("sk-raw-key".into()), voice: None, language: None },
            SpeechEngine::Command { command: "".into() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let mut v = VoiceSettings::default();
        assert!(v.validate().is_ok());
        assert_eq!((v.record_cmd.as_str(), v.play_cmd.as_str()), (DEFAULT_RECORD_CMD, DEFAULT_PLAY_CMD));
        v.record_cmd = "'unterminated".into();
        assert!(v.validate().unwrap_err().contains("record command"));
    }

    #[test]
    fn speakable_text() {
        assert_eq!(speakable("# Title\nSome *bold* text. See https://a.b/c now.\n```rust\nlet x = 1;\n```\nDone."), "Title Some bold text. See now. Done.");
        let long = speakable(&"word ".repeat(1000));
        assert!((1495..=1500).contains(&long.len()), "capped: {}", long.len());
    }

    fn norm(t: &str) -> String {
        t.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Real speech round trip. Set IYAKKI_LIVE_VOICE_DIR to the scratch dir holding whisper.cpp, a ggml model and a
    /// Piper venv + voice (see the setup script); optional IYAKKI_LIVE_WHISPER_URL tests the HTTP engine too.
    #[test]
    fn live_speech_round_trip() {
        let Ok(dir) = std::env::var("IYAKKI_LIVE_VOICE_DIR") else { return };
        let up = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(120)));
        let secrets = Secrets::new(None, up.clone());
        let tts = SpeechEngine::Command { command: format!("{dir}/venv/bin/piper -m en_US-lessac-low --data-dir {dir}/voices -f {{out}}") };
        let sentence = "The deploy key is stored in the vault under production.";
        let spoken = synthesize(&tts, sentence, &secrets, up.as_ref()).expect("piper synthesises speech");
        let w = parse_wav(&spoken).unwrap();
        println!("piper produced {:.1} s of audio at {} Hz", w.samples.len() as f32 / w.rate as f32, w.rate);
        assert!(w.samples.len() > 16_000 && peak(&w.samples) > 2000);
        // what the microphone path produces: 16 kHz mono
        let wav16 = encode_wav(&to_mono_16k(&w), SAMPLE_RATE);
        let stt = SpeechEngine::Command { command: format!("env LD_LIBRARY_PATH={dir}/whisper.cpp-1.9.5/build/bin {dir}/whisper.cpp-1.9.5/build/bin/whisper-cli -m {dir}/ggml-tiny.en.bin -f {{wav}} -nt -np") };
        let t0 = Instant::now();
        let heard = transcribe(&stt, &wav16, &secrets, up.as_ref()).expect("whisper transcribes");
        println!("whisper-cli heard {:?} in {} ms", heard, t0.elapsed().as_millis());
        assert!(norm(&heard).contains("deploy key") && norm(&heard).contains("vault"), "heard: {heard}");
        if let Ok(url) = std::env::var("IYAKKI_LIVE_WHISPER_URL") {
            let http = SpeechEngine::Http { url, model: "tiny.en".into(), key_ref: None, voice: None, language: Some("en".into()) };
            let heard = transcribe(&http, &wav16, &secrets, up.as_ref()).expect("whisper server transcribes");
            println!("whisper-server heard {:?}", heard);
            assert!(norm(&heard).contains("deploy key") && norm(&heard).contains("vault"), "heard: {heard}");
        }
    }
}
