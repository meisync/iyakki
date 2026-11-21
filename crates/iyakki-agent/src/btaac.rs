//! Bluetooth AAC through the person's ffmpeg (the audio and video add-on, installed at their click): the coders that
//! `iyakki-devctl::a2dp_aac` asks for. Nothing here codes AAC itself: one `ffmpeg` process per stream, started with a fixed argument
//! list (the numbers come from a validated stream setting, never from text) and no shell, only when a Bluetooth stream starts (the
//! person connected the device or asked for the sound to go to it); it is killed when the stream ends.
//!
//! * decode: ADTS frames on standard input, 16-bit PCM at the stream's rate and channel count on standard output;
//! * encode: PCM on standard input, ADTS (AAC-LC) on standard output, cut into raw frames for the packets.
//!
//! The calls never block: input goes to a bounded queue (a decoder that cannot keep up loses sound, it does not stall the radio link),
//! output is whatever the program has written so far. A program that stops is reported with its last words.

use std::io::{Read, Write};
use std::process::Child;
use iyakki_core_local::starters_launch::{Launch, Stream};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use iyakki_core_local::devctl::a2dp_aac::{adts_take_frames, AacBackend, AacConfig, AacDecode, AacEncode};

/// The most output bytes kept when nobody collects them (about two seconds of CD sound): older sound is dropped.
const MAX_OUT: usize = 400_000;

/// The fixed arguments of the decoder for a stream of `rate` Hz and `channels` channels.
pub fn decode_args(cfg: &AacConfig) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer", "-probesize", "2048", "-analyzeduration", "0", "-f", "aac", "-i", "pipe:0", "-f", "s16le", "-ac"].map(String::from).to_vec();
    a.push(cfg.channels.to_string());
    a.push("-ar".into());
    a.push(cfg.rate.to_string());
    a.push("pipe:1".into());
    a
}

/// The fixed arguments of the encoder.
pub fn encode_args(cfg: &AacConfig) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-f", "s16le", "-ar"].map(String::from).to_vec();
    a.push(cfg.rate.to_string());
    a.push("-ac".into());
    a.push(cfg.channels.to_string());
    a.extend(["-i", "pipe:0", "-c:a", "aac", "-profile:a", "aac_low", "-b:a"].map(String::from));
    a.push(cfg.bit_rate.clamp(16_000, 320_000).to_string());
    a.extend(["-flush_packets", "1", "-f", "adts", "pipe:1"].map(String::from));
    a
}

struct Out {
    buf: Mutex<(Vec<u8>, bool)>,
    cv: Condvar,
}

/// One running program with a bounded input queue and collected output.
pub(crate) struct Pipe {
    child: Child,
    tx: Option<SyncSender<Vec<u8>>>,
    out: Arc<Out>,
    said: Arc<Mutex<String>>,
    stderr_thread: Option<JoinHandle<()>>,
    _live: crate::compatreport::LiveGuard,
}

impl Pipe {
    pub(crate) fn spawn(program: &str, args: &[String]) -> Result<Pipe, String> {
        let mut child = Launch::new(program)
            .args(args)
            .stdin(Stream::Piped)
            .stdout(Stream::Piped)
            .stderr(Stream::Piped)
            .spawn_retrying(51, std::time::Duration::from_millis(20))
            .map_err(|e| format!("cannot start the audio and video add-on: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("the audio add-on has no input")?;
        let mut stdout = child.stdout.take().ok_or("the audio add-on has no output")?;
        let mut stderr = child.stderr.take().ok_or("the audio add-on has no error output")?;
        let said = Arc::new(Mutex::new(String::new()));
        let s2 = said.clone();
        let stderr_thread = std::thread::spawn(move || {
            let mut b = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut b) {
                if n == 0 {
                    break;
                }
                let mut g = s2.lock().unwrap_or_else(|e| e.into_inner());
                g.push_str(&String::from_utf8_lossy(&b[..n]));
                if g.len() > 2048 {
                    let cut = g.len() - 2048;
                    let cut = (cut..g.len()).find(|i| g.is_char_boundary(*i)).unwrap_or(g.len());
                    g.drain(..cut);
                }
            }
        });
        let (tx, wrx) = sync_channel::<Vec<u8>>(64);
        std::thread::spawn(move || {
            while let Ok(b) = wrx.recv() {
                if stdin.write_all(&b).is_err() {
                    break;
                }
            }
        });
        let out = Arc::new(Out { buf: Mutex::new((Vec::new(), false)), cv: Condvar::new() });
        let o2 = out.clone();
        std::thread::spawn(move || {
            let mut b = vec![0u8; 16 * 1024];
            while let Ok(n) = stdout.read(&mut b) {
                if n == 0 {
                    break;
                }
                let mut g = o2.buf.lock().unwrap_or_else(|e| e.into_inner());
                g.0.extend_from_slice(&b[..n]);
                if g.0.len() > MAX_OUT {
                    let cut = g.0.len() - MAX_OUT;
                    g.0.drain(..cut);
                }
                o2.cv.notify_all();
            }
            o2.buf.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
            o2.cv.notify_all();
        });
        Ok(Pipe { child, tx: Some(tx), out, said, stderr_thread: Some(stderr_thread), _live: crate::compatreport::note_live("helper", "audio decoder", "audio add-on") })
    }

    /// Queues input. A full queue drops it (the program is behind); a program that has stopped is an error with its last words.
    pub(crate) fn put(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let Some(tx) = &self.tx else { return Ok(()) };
        match tx.try_send(bytes) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(self.failure()),
        }
    }

    /// What the program has written since the last call. When it has ended and nothing is left, the plain reason.
    pub(crate) fn take(&mut self) -> Result<Vec<u8>, String> {
        let (data, eof) = {
            let mut g = self.out.buf.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut g.0), g.1)
        };
        if data.is_empty() && eof && self.tx.is_some() {
            return Err(self.failure());
        }
        Ok(data)
    }

    /// No more input (the program then ends its output).
    pub(crate) fn finish(&mut self) {
        self.tx = None;
    }

    /// Waits until the program has closed its output (the end of its work). For tests and for orderly ends; never called on the
    /// real-time path.
    pub(crate) fn wait_end(&self, limit: Duration) -> bool {
        let g = self.out.buf.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self.out.cv.wait_timeout_while(g, limit, |g| !g.1).unwrap_or_else(|e| e.into_inner());
        g.1
    }

    fn failure(&mut self) -> String {
        // the output has closed, so the program is gone and its error output ends: wait for the thread that reads it
        if self.out.buf.lock().unwrap_or_else(|e| e.into_inner()).1 {
            if let Some(t) = self.stderr_thread.take() {
                let _ = t.join();
            }
        }
        let said = self.said.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        if last.is_empty() { "the audio add-on stopped".into() } else { format!("the audio add-on stopped: {}", crate::player::plain_reason(&last)) }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.tx = None;
        crate::voice::kill_child(&mut self.child);
    }
}

/// The decoder: ADTS frames in, PCM out.
pub struct FfmpegDecoder {
    pipe: Pipe,
}

impl FfmpegDecoder {
    pub fn spawn(program: &str, cfg: &AacConfig) -> Result<FfmpegDecoder, String> {
        Pipe::spawn(program, &decode_args(cfg)).map(|pipe| FfmpegDecoder { pipe })
    }

    /// See [`Pipe::wait_end`] (tests).
    pub fn finish_and_wait(&mut self, limit: Duration) -> bool {
        self.pipe.finish();
        self.pipe.wait_end(limit)
    }
}

impl AacDecode for FfmpegDecoder {
    fn feed(&mut self, adts: &[u8]) -> Result<(), String> {
        self.pipe.put(adts.to_vec())
    }

    fn drain(&mut self) -> Result<Vec<i16>, String> {
        let b = self.pipe.take()?;
        Ok(b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
    }
}

/// The encoder: PCM in, raw AAC frames out.
pub struct FfmpegEncoder {
    pipe: Pipe,
    adts: Vec<u8>,
}

impl FfmpegEncoder {
    pub fn spawn(program: &str, cfg: &AacConfig) -> Result<FfmpegEncoder, String> {
        Pipe::spawn(program, &encode_args(cfg)).map(|pipe| FfmpegEncoder { pipe, adts: Vec::new() })
    }

    /// See [`Pipe::wait_end`] (tests).
    pub fn finish_and_wait(&mut self, limit: Duration) -> bool {
        self.pipe.finish();
        self.pipe.wait_end(limit)
    }
}

impl AacEncode for FfmpegEncoder {
    fn feed(&mut self, pcm: &[i16]) -> Result<(), String> {
        let mut b = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            b.extend_from_slice(&s.to_le_bytes());
        }
        self.pipe.put(b)
    }

    fn drain(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let b = self.pipe.take()?;
        self.adts.extend_from_slice(&b);
        Ok(adts_take_frames(&mut self.adts))
    }
}

/// The backend of `iyakki-devctl::a2dp_aac`: the person's ffmpeg. `program` is asked each time, so installing the add-on takes effect at
/// the next connection.
pub struct FfmpegBackend {
    program: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl FfmpegBackend {
    /// The ffmpeg that is installed and starts (the one the radio and the media player use).
    pub fn system() -> FfmpegBackend {
        FfmpegBackend { program: Box::new(|| crate::player::find_program("ffmpeg").filter(|p| crate::player::runs(p))) }
    }

    /// A given program (tests, a person's own build).
    pub fn with_program(f: impl Fn() -> Option<String> + Send + Sync + 'static) -> FfmpegBackend {
        FfmpegBackend { program: Box::new(f) }
    }
}

impl AacBackend for FfmpegBackend {
    fn available(&self) -> bool {
        (self.program)().is_some()
    }

    fn decoder(&self, cfg: &AacConfig) -> Result<Box<dyn AacDecode>, String> {
        let p = (self.program)().ok_or(iyakki_core_local::devctl::a2dp_aac::NEEDS_ADDON)?;
        FfmpegDecoder::spawn(&p, cfg).map(|d| Box::new(d) as Box<dyn AacDecode>)
    }

    fn encoder(&self, cfg: &AacConfig) -> Result<Box<dyn AacEncode>, String> {
        let p = (self.program)().ok_or(iyakki_core_local::devctl::a2dp_aac::NEEDS_ADDON)?;
        FfmpegEncoder::spawn(&p, cfg).map(|d| Box::new(d) as Box<dyn AacEncode>)
    }
}

/// Registers the ffmpeg of the add-on as the AAC coder for Bluetooth (the program calls this once at start). Nothing is started by it.
pub fn install() {
    iyakki_core_local::devctl::a2dp_aac::set_backend(Arc::new(FfmpegBackend::system()));
}

#[cfg(test)]
#[path = "btaac_tests.rs"]
mod tests;
