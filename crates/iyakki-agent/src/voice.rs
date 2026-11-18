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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::vault::{LocalVault, Secrets};
pub use iyakki_shared::voice::{DEFAULT_PLAY_CMD, SAMPLE_RATE, SpeakOpts};

pub const MAX_RECORD_SECS: usize = 120;
pub use iyakki_shared::voice::DEFAULT_RECORD_CMD;

// ------------------------------------------------------------------ configuration

pub use iyakki_shared::voice::{validate_command, split_command, Highlight, SpeechEngine, VoiceSettings};

pub(crate) fn private_tempdir(tag: &str) -> Result<PathBuf, String> {
    private_tempdir_in(&std::env::temp_dir(), tag)
}

/// The same in a folder of the caller's choice (tests give their own, so that other tests' folders cannot be counted by mistake).
pub(crate) fn private_tempdir_in(base: &Path, tag: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut r = [0u8; 8];
    iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
    let name: String = r.iter().map(|b| format!("{b:02x}")).collect();
    let dir = base.join(format!("iyakki-{tag}-{name}"));
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

/// Whether this process is a test binary (cargo puts them in `target/<profile>/deps`; the product never runs from there).
pub(crate) fn is_test_binary() -> bool {
    static IS: OnceLock<bool> = OnceLock::new();
    // cargo before 1.100 puts a test program in target/<profile>/deps, cargo 1.100 in target/<profile>/build/<package>/<hash>/out (the product runs from neither)
    *IS.get_or_init(|| {
        cfg!(test)
            || std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|p| p.file_name().is_some_and(|n| n == "deps") || (p.file_name().is_some_and(|n| n == "out") && p.ancestors().any(|a| a.file_name().is_some_and(|n| n == "build")))))
                .unwrap_or(false)
    })
}

/// The command to run for a sound player. In a test binary the real `aplay` (a bare name, or the usual system paths) is swapped
/// for a program that swallows its input, so a test never opens (and never holds) this computer's sound device. Any other
/// program, such as a fake `aplay` in a test's own folder, is left as it is.
pub fn sound_player_argv(args: &[String]) -> Vec<String> {
    let real = args.first().is_some_and(|a| matches!(a.as_str(), "aplay" | "/usr/bin/aplay" | "/bin/aplay" | "/usr/local/bin/aplay"));
    if real && is_test_binary() {
        return vec!["/bin/sh".into(), "-c".into(), "exec cat >/dev/null".into()];
    }
    args.to_vec()
}

pub(crate) fn command_for(args: &[String], cwd: Option<&Path>) -> Command {
    use std::os::unix::process::CommandExt;
    let args = &sound_player_argv(args);
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
pub(crate) fn run_capture(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> Result<(Vec<u8>, String), String> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    // ETXTBSY: a program written a moment ago can still be open for writing in a descriptor another thread's fork holds
    // for a few microseconds; the exec succeeds when tried again.
    let mut tries = 0;
    let mut child = loop {
        match cmd.spawn() {
            Ok(c) => break c,
            Err(e) if e.raw_os_error() == Some(26) && tries < 50 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("cannot start `{}`: {e}", cmd.get_program().to_string_lossy())),
        }
    };
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
        match iyakki_core_api::child_status(&mut child) {
            iyakki_core_api::ChildState::Running if start.elapsed() > timeout => {
                kill_tree(&mut child);
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            iyakki_core_api::ChildState::Running => std::thread::sleep(Duration::from_millis(20)),
            s => break s,
        }
    };
    let (out, err) = (t_out.join().unwrap_or_default(), t_err.join().unwrap_or_default());
    // `Gone`: another reaper took the status; the output is complete but the result is unknown, so it is taken as given
    if !status.success() && status != iyakki_core_api::ChildState::Gone {
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        return Err(format!("exited with {}{}", status.describe(), if tail.is_empty() { String::new() } else { format!(": {tail}") }));
    }
    Ok((out, err))
}

pub use iyakki_shared::voice::{encode_wav, parse_wav, peak, tone, to_mono_16k, Wav};

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
    /// The most seconds kept (`MAX_RECORD_SECS` unless started with [`Recorder::start_max`]).
    max_secs: usize,
}

impl Recorder {
    pub fn start(record_cmd: &str) -> Result<Recorder, String> {
        Recorder::start_max(record_cmd, MAX_RECORD_SECS)
    }

    /// [`Recorder::start`] with another longest recording (the VOICE tile takes five minutes).
    pub fn start_max(record_cmd: &str, max_secs: usize) -> Result<Recorder, String> {
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
            let max = SAMPLE_RATE as usize * 2 * max_secs;
            let mut chunk = [0u8; 4096];
            while let Ok(n) = so.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut g = b2.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        Ok(Recorder { child, buf, started: Instant::now(), reader: Some(reader), stderr: Some(stderr), max_secs })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn is_full(&self) -> bool {
        self.buf.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len() >= SAMPLE_RATE as usize * 2 * self.max_secs
    }

    /// Loudness of the last ~150 ms, 0.0–1.0 (for a level meter).
    pub fn level(&self) -> f32 {
        let g = self.buf.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = (SAMPLE_RATE as usize * 2 * 150 / 1000).min(g.len()) & !1;
        let tail = &g[g.len() - n..];
        let pk = tail.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).saturating_abs()).max().unwrap_or(0);
        (pk as f32 / 12000.0).min(1.0)
    }

    /// Hand over the samples captured since the last call and forget them (the wake listener reads small frames this way, so the buffer
    /// never grows and nothing is kept). A half sample at the end stays for the next call.
    pub fn drain_samples(&self) -> Vec<i16> {
        let mut g = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        let n = g.len() & !1;
        let bytes: Vec<u8> = g.drain(..n).collect();
        bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }

    /// Whether the recording process is still running (it exits early if the device is missing/busy).
    pub fn is_alive(&mut self) -> bool {
        iyakki_core_api::child_status(&mut self.child).is_running()
    }

    /// Stop recording and return the samples.
    pub fn stop(mut self) -> Result<Vec<i16>, String> {
        kill_tree(&mut self.child);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        let err = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
        let bytes = std::mem::take(&mut *self.buf.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
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
/// The output of Settings -> Sound is given to `aplay` (`-D plughw:C,D`) unless the command names a device itself.
pub fn spawn_player(play_cmd: &str) -> Result<Child, String> {
    let args = crate::audioout::route_args(split_command(play_cmd)?, crate::audioout::global().device().as_deref());
    let prog = args.first().ok_or("play command is empty")?;
    command_for(&args, None)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start `{prog}`: {e} (change the play command in Settings → Voice)"))
}

/// Why a sound player stopped: its own first line of complaint (`aplay: set_params:1462: Unable to install hw params:`), read
/// from the standard error of a [`spawn_player`] child that has ended or is about to (waits up to 400 ms). Empty when it said
/// nothing; the child is left to the caller to stop.
pub fn player_failure(child: &mut Child) -> String {
    use std::io::Read;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(400) && iyakki_core_api::child_status(child).is_running() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let Some(mut err) = child.stderr.take() else { return String::new() };
    if iyakki_core_api::child_status(child).is_running() {
        return String::new(); // still running: nothing to read without blocking
    }
    let mut text = String::new();
    let _ = err.by_ref().take(4096).read_to_string(&mut text);
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
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
        match iyakki_core_api::child_status(&mut child) {
            s if s.success() => break,
            iyakki_core_api::ChildState::Running => std::thread::sleep(std::time::Duration::from_millis(50)),
            s => return Err(format!("{} stopped right away ({}); it may have no window or sound here", argv[0].rsplit('/').next().unwrap_or(&argv[0]), s.describe())),
        }
    }
    Ok(child)
}

pub fn kill_child(child: &mut Child) {
    kill_tree(child);
}

pub struct Player {
    /// A host speech engine's own process (`from_speech`); None for sound that goes through [`crate::audiosink`].
    child: Option<Child>,
    out: Option<crate::audiosink::Output>,
    /// Run once when it is stopped while still playing (speech-dispatcher keeps talking after its client is killed).
    stop_cmd: Option<Vec<String>>,
    /// A speech process that has not ended by then is killed (an engine that hangs must not hold the speaker forever).
    deadline: Option<Instant>,
}

/// Whether the play command is `aplay` (possibly with options, or given by path).
pub fn streams_to_one_player(play_cmd: &str) -> bool {
    play_cmd.split_whitespace().next().and_then(|p| p.rsplit('/').next()).is_some_and(|p| p == "aplay")
}

impl Player {
    /// Start playing a WAV file as speech (audio is written to the play command's stdin, or mixed when the mixer is on). The
    /// output and the volume of Settings -> Sound apply (see `audioout`, `audiosink`).
    pub fn start(play_cmd: &str, wav: &[u8]) -> Result<Player, String> {
        Player::start_as(crate::mixer::SourceKind::Speech, play_cmd, wav)
    }

    /// The sentences of one reading: with the mixer off they share one play process, so the card is not closed and primed again
    /// between sentences (see `audiosink::play_wav_streamed`). It reports itself finished slightly before its sound ends.
    /// Only `aplay` is shared this way (it is the player whose card is primed again at every start); any other play command
    /// keeps one process per sentence, as before.
    pub fn start_streamed(play_cmd: &str, wav: &[u8]) -> Result<Player, String> {
        if !streams_to_one_player(play_cmd) {
            return Player::start(play_cmd, wav);
        }
        let out = crate::audiosink::play_wav_streamed(crate::mixer::SourceKind::Speech, "speech", wav, play_cmd)?;
        Ok(Player { child: None, out: Some(out), stop_cmd: None, deadline: None })
    }

    /// The same for another kind of sound (it fixes the priority when sounds are mixed).
    pub fn start_as(kind: crate::mixer::SourceKind, play_cmd: &str, wav: &[u8]) -> Result<Player, String> {
        let out = crate::audiosink::play_wav(kind, kind.label(), wav, play_cmd)?;
        Ok(Player { child: None, out: Some(out), stop_cmd: None, deadline: None })
    }

    /// A process that speaks by itself (a host speech engine): the text goes to its standard input, which is then closed.
    /// `stop_cmd` is run, without a shell, when it is stopped while still speaking; `max` is the longest it may take.
    /// (Its own sound is not mixed: the engine opens the speaker itself.)
    pub fn from_speech(mut child: Child, text: Vec<u8>, stop_cmd: Option<Vec<String>>, max: Duration) -> Player {
        if let Some(mut si) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = si.write_all(&text);
            });
        }
        Player { child: Some(child), out: None, stop_cmd, deadline: Some(Instant::now() + max) }
    }

    /// How much of the sound was heard, ms (mixer only; None when the player reports nothing).
    pub fn played_ms(&self) -> Option<u64> {
        self.out.as_ref().and_then(|o| o.played_ms())
    }

    pub fn is_playing(&mut self) -> bool {
        if let Some(o) = self.out.as_mut() {
            return o.is_playing();
        }
        let Some(child) = self.child.as_mut() else { return false };
        if self.deadline.is_some_and(|d| Instant::now() > d) && iyakki_core_api::child_status(child).is_running() {
            self.stop();
            return false;
        }
        self.child.as_mut().is_some_and(|c| iyakki_core_api::child_status(c).is_running())
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
        if let Some(o) = self.out.as_mut() {
            o.stop();
        }
        if let Some(child) = self.child.as_mut() {
            if let Some(cmd) = self.stop_cmd.take() {
                if iyakki_core_api::child_status(child).is_running() && !cmd.is_empty() {
                    let _ = run_capture(command_for(&cmd, None), None, Duration::from_secs(3));
                }
            }
            kill_tree(child);
        }
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
        SpeechEngine::Gemini { model, language, key_ref, tier } => crate::gemini::transcribe(model, language.as_deref(), key_ref, *tier, wav, secrets, upstream).map(|t| clean_transcript(&t)),
        SpeechEngine::Browser | SpeechEngine::Host { .. } => Err("the browser voice and the speech engines of this computer only speak; choose an HTTP endpoint or a local command for speech-to-text".into()),
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

pub use iyakki_shared::voice::speakable;



/// Text to speech; returns a WAV file.
pub fn synthesize(engine: &SpeechEngine, text: &str, secrets: &Secrets, upstream: &dyn Upstream) -> Result<Vec<u8>, String> {
    synthesize_with(engine, text, &SpeakOpts::default(), secrets, upstream)
}

/// [`synthesize`] with a voice and a speed chosen for this request.
pub fn synthesize_with(engine: &SpeechEngine, text: &str, opts: &SpeakOpts, secrets: &Secrets, upstream: &dyn Upstream) -> Result<Vec<u8>, String> {
    let text = speakable(text);
    if text.is_empty() {
        return Err("nothing to say".into());
    }
    let speed = if opts.speed.is_finite() { opts.speed.clamp(0.5, 2.0) } else { 1.0 };
    let audio = match engine {
        SpeechEngine::Browser => return Err("the browser voice speaks inside the browser and makes no audio file".into()),
        SpeechEngine::Gemini { .. } => return Err("Google Gemini is used for speech-to-text only; choose another reading voice".into()),
        SpeechEngine::Host { engine, program, voice } => {
            let spec = crate::hostengines::Spec::from_parts(engine, program.as_deref(), opts.voice.as_deref().filter(|v| !v.trim().is_empty()).or(voice.as_deref()))?;
            return crate::hostengines::synth_wav(&spec, &text, opts.lang.as_deref(), speed);
        }
        SpeechEngine::Http { url, model, key_ref, voice, .. } => {
            let mut headers = key_headers(key_ref, secrets)?;
            headers.push(("Content-Type".into(), "application/json".into()));
            let voice = opts.voice.as_deref().filter(|v| !v.trim().is_empty()).or(voice.as_deref()).unwrap_or("alloy");
            let mut body = json!({"model": model, "input": text, "voice": voice, "response_format": "wav"});
            if (speed - 1.0).abs() > 0.01 {
                body["speed"] = json!(speed);
            }
            let r = upstream.request("POST", url, &headers, body.to_string().as_bytes())?;
            if !(200..300).contains(&r.status) {
                return Err(format!("HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim()));
            }
            r.body
        }
        SpeechEngine::Command { command } => {
            let dir = private_tempdir("tts")?;
            let out_path = dir.join("speech.wav");
            let result = (|| {
                let voice = opts.voice.clone().unwrap_or_default();
                let args: Vec<String> = split_command(command)?
                    .into_iter()
                    .map(|a| a.replace("{out}", &out_path.to_string_lossy()).replace("{voice}", &voice).replace("{speed}", &format!("{speed:.2}")).replace("{length_scale}", &format!("{:.2}", 1.0 / speed)))
                    .collect();
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

/// Helpers for tests that watch the processes a test binary starts.
#[cfg(test)]
pub(crate) mod test_procs {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// A script in a fresh temp folder that writes its pid to a file, then runs `body`. Returns (script path, pid file).
    pub fn script(tag: &str, body: &str) -> (String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iyakki-proc-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        let (sh, pidfile) = (d.join("fake-player"), d.join("pid"));
        std::fs::write(&sh, format!("#!/bin/sh\necho $$ > {}.tmp && mv {}.tmp {}\n{body}\n", pidfile.display(), pidfile.display(), pidfile.display())).unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (sh.display().to_string(), pidfile)
    }

    /// The pid the script wrote (waits for it).
    pub fn pid_from(pidfile: &PathBuf) -> u32 {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(p) = std::fs::read_to_string(pidfile).ok().and_then(|t| t.trim().parse().ok()) {
                return p;
            }
            assert!(Instant::now() < end, "the player did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether no process (not even a zombie waiting to be reaped) has this pid, waiting up to `within`.
    pub fn gone(pid: u32, within: Duration) -> bool {
        let end = Instant::now() + within;
        loop {
            // SAFETY: signal 0 only checks that the process exists
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            if !alive {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The direct children of this process whose program name is `name`.
    pub fn children_named(name: &str) -> Vec<u32> {
        let me = std::process::id();
        let mut out = Vec::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
            // "pid (comm) S ppid ...": comm may hold spaces and brackets, so cut after the last ')'
            let ppid = stat.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|p| p.parse::<u32>().ok());
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            if ppid == Some(me) && comm.trim() == name {
                out.push(pid);
            }
        }
        out
    }
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
        assert_eq!((p.rate, p.channels, p.samples.clone()), (16_000, 1, pcm.clone()));
        assert!(parse_wav(b"nope").is_err());
        assert!(parse_wav(&wav[..40]).is_err());
        let mut bad = wav.clone();
        bad[34] = 12; // 12 bits per sample: not a format this reads
        assert!(parse_wav(&bad).unwrap_err().contains("16-bit"));
        // 32-bit float (what Kokoro answers) is read as 16-bit
        let mut fl = wav[..36].to_vec();
        fl[20] = 3;
        fl[34] = 32;
        fl.extend(b"data");
        fl.extend(((pcm.len() * 4) as u32).to_le_bytes());
        for s in &pcm {
            fl.extend((*s as f32 / 32768.0).to_le_bytes());
        }
        let p = parse_wav(&fl).unwrap();
        assert_eq!((p.rate, p.channels, p.samples.len()), (16_000, 1, 1000));
        assert!(p.samples.iter().zip(&pcm).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 1));
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
        let _route = crate::mixer::pin_command_route();
        let d = tmp("play");
        let out = d.join("played.wav");
        let wav = encode_wav(&tone(440.0, 200), 48_000);
        let mut p = Player::start(&format!("sh -c 'cat > {}'", out.display()), &wav).unwrap();
        assert!(p.wait(Duration::from_secs(5)));
        assert_eq!(std::fs::read(&out).unwrap(), wav, "the speaker command received exactly the WAV (already at the card's rate)");
        // speech below 44.1 kHz reaches the speaker command at 48 kHz (the T2 MacBook played 24 kHz speech at double speed)
        let low = encode_wav(&tone(440.0, 200), 16_000);
        let out2 = d.join("played-low.wav");
        let mut p2 = Player::start(&format!("sh -c 'cat > {}'", out2.display()), &low).unwrap();
        assert!(p2.wait(Duration::from_secs(5)));
        let got = parse_wav(&std::fs::read(&out2).unwrap()).unwrap();
        assert_eq!((got.rate, got.channels), (48_000, 1));
        assert_eq!(got.samples.len(), parse_wav(&low).unwrap().samples.len() * 3, "the same time at three times the samples");
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

    #[test]
    fn a_test_binary_never_starts_the_real_aplay() {
        use test_procs::*;
        for cmd in ["aplay -q", "/usr/bin/aplay -q -D plughw:0,0"] {
            let mut c = spawn_player(cmd).unwrap();
            let comm = std::fs::read_to_string(format!("/proc/{}/comm", c.id())).unwrap_or_default();
            assert_ne!(comm.trim(), "aplay", "{cmd}");
            kill_child(&mut c);
        }
        // a fake of the same name in a test's own folder still runs as written
        let argv = sound_player_argv(&["/tmp/x/aplay".to_string(), "-q".to_string()]);
        assert_eq!(argv, ["/tmp/x/aplay", "-q"]);
        // only the system's aplay counts: another test running in parallel may have a fake player named aplay in its own folder (still a child named aplay)
        let real: Vec<u32> = children_named("aplay")
            .into_iter()
            .filter(|p| std::fs::read_link(format!("/proc/{p}/exe")).is_ok_and(|e| ["/usr/", "/bin/", "/sbin/"].iter().any(|d| e.starts_with(d))))
            .collect();
        assert!(real.is_empty(), "this test binary has an aplay child: some test opened the real sound device: {real:?}");
    }

    #[test]
    fn a_hung_player_is_killed_and_reaped_when_the_player_is_dropped_or_its_thread_panics() {
        use test_procs::*;
        let _route = crate::mixer::pin_command_route();
        let wav = encode_wav(&tone(440.0, 200), SAMPLE_RATE);
        for how in ["drop", "stop", "panic"] {
            let (prog, pidfile) = script(how, "trap '' TERM\nexec sleep 1000");
            let p = Player::start(&format!("/bin/sh {prog}"), &wav).unwrap();
            let pid = pid_from(&pidfile);
            assert!(!gone(pid, Duration::ZERO), "{how}: running");
            match how {
                "drop" => drop(p),
                "stop" => {
                    let mut p = p;
                    p.stop();
                    assert!(gone(pid, Duration::from_secs(3)), "stop reaps it before the drop");
                }
                _ => {
                    let t = std::thread::spawn(move || {
                        let _owned = p;
                        panic!("the owner died");
                    });
                    assert!(t.join().is_err());
                }
            }
            assert!(gone(pid, Duration::from_secs(3)), "{how}: gone and reaped");
        }
    }

    #[test]
    fn a_speech_engine_process_is_killed_and_reaped_when_its_player_is_dropped() {
        use test_procs::*;
        let (prog, pidfile) = script("speech", "trap '' TERM\nexec sleep 1000");
        let child = command_for(&["/bin/sh".to_string(), prog], None).stdin(Stdio::piped()).spawn().unwrap();
        let p = Player::from_speech(child, b"hi".to_vec(), None, Duration::from_secs(60));
        let pid = pid_from(&pidfile);
        drop(p);
        assert!(gone(pid, Duration::from_secs(3)));
    }
}
