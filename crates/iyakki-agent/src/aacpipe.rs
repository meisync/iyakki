//! The AAC decoder the radio uses: a separate program that reads the stream on its standard input and writes 16-bit PCM
//! on its standard output, so the decoder's code never runs inside IYAKKI's own process. The programs are not part of
//! IYAKKI: the person installs the audio add-on (Store -> Setup) from its vendor when they ask.
//!
//! Two kinds of program are found, in this order:
//! * `iyakki-audiodec`, IYAKKI's helper: no arguments; writes a 10-byte header (`IYPC`, sample rate as u32 little-endian,
//!   channel count as u16 little-endian) and then interleaved S16LE samples;
//! * `ffmpeg`, started with a fixed argument list (no shell): `-hide_banner -loglevel error -i pipe:0 -f s16le -ac 2 -ar
//!   48000 pipe:1`, which writes raw 48 kHz stereo S16LE with no header.
//!
//! Buffers are bounded (a few hundred KB each way), a program that stops is reported with the last words it said on its
//! standard error, and the process is killed when the decoder is dropped (the radio stopped or switched).

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::audiofmt::{AacDecoder, Pcm, NEEDS_AUDIO_ADDON};

/// The fixed arguments of the `ffmpeg` fallback.
pub const FFMPEG_ARGS: [&str; 12] = ["-hide_banner", "-loglevel", "error", "-i", "pipe:0", "-f", "s16le", "-ac", "2", "-ar", "48000", "pipe:1"];
pub const HELPER: &str = "iyakki-audiodec";
const MAGIC: &[u8; 4] = b"IYPC";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Our helper: a header, then samples.
    Helper,
    /// `ffmpeg`: raw 48 kHz stereo.
    Ffmpeg,
}

pub struct PipeDecoder {
    child: Child,
    tx: Option<SyncSender<Vec<u8>>>,
    rx: Receiver<Vec<u8>>,
    /// Bytes of sound read but not yet handed out.
    carry: Vec<u8>,
    format: Arc<Mutex<(u32, usize)>>,
    said: Arc<Mutex<String>>,
}

/// The program that decodes AAC here, or the plain reason it cannot (the add-on is not installed).
pub fn aac_decoder() -> Result<Box<dyn AacDecoder>, String> {
    if let Some(h) = crate::player::find_program(HELPER).filter(|p| crate::player::runs(p)) {
        return PipeDecoder::spawn(&h, Kind::Helper).map(|d| Box::new(d) as Box<dyn AacDecoder>);
    }
    if let Some(f) = crate::player::find_program("ffmpeg").filter(|p| crate::player::runs(p)) {
        return PipeDecoder::spawn(&f, Kind::Ffmpeg).map(|d| Box::new(d) as Box<dyn AacDecoder>);
    }
    Err(NEEDS_AUDIO_ADDON.into())
}

/// Whether an AAC decoder is installed (for the station lists).
pub fn installed() -> bool {
    crate::player::find_program(HELPER).is_some_and(|p| crate::player::runs(&p)) || crate::player::available()
}

impl PipeDecoder {
    pub fn spawn(program: &str, kind: Kind) -> Result<PipeDecoder, String> {
        let mut cmd = Command::new(program);
        if kind == Kind::Ffmpeg {
            cmd.args(FFMPEG_ARGS);
        }
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start the audio decoder: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("the audio decoder has no input")?;
        let mut stdout = child.stdout.take().ok_or("the audio decoder has no output")?;
        let mut stderr = child.stderr.take().ok_or("the audio decoder has no error output")?;
        let said = Arc::new(Mutex::new(String::new()));
        let s2 = said.clone();
        std::thread::spawn(move || {
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
        let (tx, wrx) = sync_channel::<Vec<u8>>(32);
        std::thread::spawn(move || {
            while let Ok(b) = wrx.recv() {
                if stdin.write_all(&b).is_err() {
                    break;
                }
            }
        });
        let format = Arc::new(Mutex::new((48_000u32, 2usize)));
        let f2 = format.clone();
        let (ptx, rx) = sync_channel::<Vec<u8>>(32);
        std::thread::spawn(move || {
            if kind == Kind::Helper {
                let mut h = [0u8; 10];
                if stdout.read_exact(&mut h).is_err() || &h[..4] != MAGIC {
                    return;
                }
                let (rate, ch) = (u32::from_le_bytes([h[4], h[5], h[6], h[7]]), u16::from_le_bytes([h[8], h[9]]) as usize);
                if !(1..=2).contains(&ch) || !(4000..=192_000).contains(&rate) {
                    return;
                }
                *f2.lock().unwrap_or_else(|e| e.into_inner()) = (rate, ch);
            }
            let mut b = vec![0u8; 16 * 1024];
            while let Ok(n) = stdout.read(&mut b) {
                if n == 0 || ptx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(PipeDecoder { child, tx: Some(tx), rx, carry: Vec::new(), format, said })
    }

    fn failure(&mut self) -> String {
        // give the program a moment to have said why
        std::thread::sleep(Duration::from_millis(60));
        let said = self.said.lock().unwrap_or_else(|e| e.into_inner()).trim().to_string();
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        if last.is_empty() { "the audio decoder stopped".into() } else { format!("the audio decoder stopped: {}", crate::player::plain_reason(&last)) }
    }
}

impl AacDecoder for PipeDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Result<bool, String> {
        let Some(tx) = &self.tx else { return Ok(true) };
        match tx.try_send(bytes.to_vec()) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => Err(self.failure()),
        }
    }

    fn finish(&mut self) {
        self.tx = None;
    }

    fn read(&mut self, out: &mut [i16], wait: Duration) -> Result<Pcm, String> {
        if self.carry.len() < 2 {
            match self.rx.recv_timeout(wait) {
                Ok(b) => self.carry.extend_from_slice(&b),
                Err(RecvTimeoutError::Timeout) => return Ok(Pcm::Pending),
                Err(RecvTimeoutError::Disconnected) => {
                    return match self.child.try_wait() {
                        Ok(Some(s)) if s.success() => Ok(Pcm::End),
                        Ok(None) | Err(_) if self.tx.is_none() => Ok(Pcm::End),
                        _ => Err(self.failure()),
                    };
                }
            }
        }
        let n = (self.carry.len() / 2).min(out.len());
        for (o, c) in out.iter_mut().zip(self.carry.chunks_exact(2)).take(n) {
            *o = i16::from_le_bytes([c[0], c[1]]);
        }
        self.carry.drain(..n * 2);
        Ok(Pcm::Samples(n))
    }

    fn format(&self) -> (u32, usize) {
        *self.format.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for PipeDecoder {
    fn drop(&mut self) {
        self.tx = None;
        crate::voice::kill_child(&mut self.child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(tag: &str, body: &str) -> String {
        let d = std::env::temp_dir().join(format!("iyakki-aacpipe-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("dec");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    fn drain(d: &mut PipeDecoder) -> (usize, Result<(), String>) {
        let mut total = 0;
        let mut out = vec![0i16; 4096];
        for _ in 0..400 {
            match d.read(&mut out, Duration::from_millis(50)) {
                Ok(Pcm::Samples(n)) => total += n,
                Ok(Pcm::Pending) => {}
                Ok(Pcm::End) => return (total, Ok(())),
                Err(e) => return (total, Err(e)),
            }
        }
        (total, Err("never ended".into()))
    }

    #[test]
    fn a_framed_helper_gives_its_format_and_samples_and_ends_with_its_input() {
        // header: IYPC, 22050 Hz (0x5622), 1 channel; then 1000 samples of 0x0102
        let p = script("framed", "printf 'IYPC\\042\\126\\000\\000\\001\\000'; head -c 2000 /dev/zero | tr '\\000' '\\001'; cat >/dev/null");
        let mut d = PipeDecoder::spawn(&p, Kind::Helper).unwrap();
        assert!(d.feed(b"some adts bytes").unwrap());
        d.finish();
        let (n, r) = drain(&mut d);
        assert_eq!(r, Ok(()));
        assert_eq!(n, 1000);
        assert_eq!(d.format(), (22050, 1));
        let mut out = [0i16; 4];
        // all of it was 0x0101
        let p2 = script("framed2", "printf 'IYPC\\200\\273\\000\\000\\002\\000'; printf '\\001\\001\\001\\001'; cat >/dev/null");
        let mut d2 = PipeDecoder::spawn(&p2, Kind::Helper).unwrap();
        d2.finish();
        assert_eq!(d2.read(&mut out, Duration::from_secs(2)).unwrap(), Pcm::Samples(2));
        assert_eq!(&out[..2], &[0x0101, 0x0101]);
        assert_eq!(d2.format(), (48000, 2));
    }

    #[test]
    fn a_program_that_fails_is_reported_with_its_last_words_and_a_full_decoder_says_so() {
        let p = script("fail", "echo 'libavcodec.so.60: cannot open shared object file' >&2; exit 3");
        let mut d = PipeDecoder::spawn(&p, Kind::Ffmpeg).unwrap();
        let (_, r) = drain(&mut d);
        let e = r.unwrap_err();
        assert!(e.contains("audio decoder stopped") && e.contains("incomplete"), "a linker complaint is put plainly: {e}");
        let p = script("fail2", "echo 'Invalid data found when processing input' >&2; exit 1");
        let mut d = PipeDecoder::spawn(&p, Kind::Ffmpeg).unwrap();
        let e = drain(&mut d).1.unwrap_err();
        assert!(e.contains("Invalid data found"), "{e}");
        // a program that never reads: the queue fills up and feed says "later"
        let p = script("stuck", "sleep 30");
        let mut d = PipeDecoder::spawn(&p, Kind::Ffmpeg).unwrap();
        let mut full = false;
        for _ in 0..200 {
            if !d.feed(&[0u8; 65536]).unwrap_or(true) {
                full = true;
                break;
            }
        }
        assert!(full, "bounded");
        drop(d);
        assert!(PipeDecoder::spawn("/nonexistent/dec", Kind::Ffmpeg).is_err());
    }

    #[test]
    fn the_ffmpeg_call_is_a_fixed_list_without_a_shell() {
        let rec = std::env::temp_dir().join(format!("iyakki-aacpipe-args-{}", std::process::id()));
        let p = script("args", &format!("echo \"$@\" > {}; cat >/dev/null", rec.display()));
        let mut d = PipeDecoder::spawn(&p, Kind::Ffmpeg).unwrap();
        d.finish();
        let _ = drain(&mut d);
        let said = std::fs::read_to_string(&rec).unwrap();
        assert_eq!(said.trim(), "-hide_banner -loglevel error -i pipe:0 -f s16le -ac 2 -ar 48000 pipe:1");
        let _ = std::fs::remove_file(rec);
    }
}
