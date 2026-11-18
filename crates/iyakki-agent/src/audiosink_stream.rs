//! One long-lived play process for a whole reading (mixer off).
//!
//! `play_wav_command` starts a player per WAV, which for reading aloud meant a new `aplay` per sentence: on the T2 the card went
//! RUNNING -> PREPARED -> DRAINING -> RUNNING at every sentence and its 347 ms buffer had to fill again before sound resumed.
//! A [`Session`] starts the player once, writes the WAV header once (a stream of unknown length), then only raw PCM: every
//! sentence is appended to the same standard input as soon as it is ready. The player is closed when nothing new arrives for a
//! short while after the queued sound ended, or killed by [`Piece::stop`]. Each sentence's processing is as before (16-bit,
//! low rates to 48 kHz, IYAKKI's volume).
//!
//! Every sentence is a [`Piece`]. The session keeps a model of the card's timeline (the sound queued plays at the real-time
//! rate), so a piece knows when it ends and how much of it has been heard. A piece reports itself finished
//! [`HANDOFF`] before its sound ends: the caller then queues the next sentence while the card still holds sound, and nothing is
//! audible between them. Stopping a piece that already reported itself finished does not cut it off.

use std::io::Write;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A piece says it is done this long before its sound ends, so the next one is queued in time.
pub const HANDOFF: Duration = Duration::from_millis(300);
/// Silence written when the player starts: its buffer (347 ms on the T2, and playback only starts once it is full) never begins
/// empty, and the first sentence does not wait for the buffer to fill with speech.
pub const PRIME: Duration = Duration::from_millis(350);
/// The player is closed this long after the queued sound ended if nothing else was queued.
pub const GRACE: Duration = Duration::from_millis(300);

struct Session {
    key: String,
    rate: u32,
    channels: u16,
    tx: Mutex<Sender<Vec<u8>>>,
    /// When everything queued so far has been played, on the real-time model.
    end: Mutex<Instant>,
    child: Mutex<Option<Child>>,
    /// The writer has finished (closed the player) or the player was killed.
    over: AtomicBool,
}

static CURRENT: Mutex<Option<Arc<Session>>> = Mutex::new(None);

impl Session {
    fn start(args: &[String], rate: u32, channels: u16) -> Result<Arc<Session>, String> {
        let prog = args.first().ok_or("play command is empty")?;
        let mut child = crate::voice::command_for(args, None)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start `{prog}`: {e} (change the play command in Settings → Voice)"))?;
        let mut stdin = child.stdin.take().ok_or("the player has no input")?;
        let (tx, rx) = channel::<Vec<u8>>();
        let prime = vec![0u8; (rate as u64 * channels as u64 * 2 * PRIME.as_millis() as u64 / 1000) as usize / (channels as usize * 2) * (channels as usize * 2)];
        let s = Arc::new(Session { key: args.join("\u{1f}"), rate, channels, tx: Mutex::new(tx), end: Mutex::new(Instant::now() + PRIME), child: Mutex::new(Some(child)), over: AtomicBool::new(false) });
        let me = s.clone();
        let header = super::wav_header(rate, channels);
        std::thread::Builder::new().name("iyakki-speech-out".into()).spawn(move || {
            let mut ok = stdin.write_all(&header).is_ok() && stdin.write_all(&prime).is_ok();
            while ok {
                let end = *me.end.lock().unwrap_or_else(|e| e.into_inner());
                let wait = (end + GRACE).saturating_duration_since(Instant::now()).max(Duration::from_millis(20));
                match rx.recv_timeout(wait) {
                    Ok(buf) => ok = stdin.write_all(&buf).is_ok(),
                    Err(RecvTimeoutError::Timeout) => {
                        // nothing new, and what was queued has played: let the player finish and release the card
                        if *me.end.lock().unwrap_or_else(|e| e.into_inner()) + GRACE <= Instant::now() {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            me.over.store(true, Ordering::SeqCst);
            drop(stdin);
            let t0 = Instant::now();
            loop {
                let mut g = me.child.lock().unwrap_or_else(|e| e.into_inner());
                let Some(c) = g.as_mut() else { return };
                match iyakki_core_api::child_status(c) {
                    s if s.is_running() && t0.elapsed() < Duration::from_secs(3) => {}
                    s if s.is_running() => {
                        crate::voice::kill_child(c);
                        let _ = c.wait();
                        return;
                    }
                    _ => return,
                }
                drop(g);
                std::thread::sleep(Duration::from_millis(20));
            }
        }).map_err(|e| format!("cannot start the sound thread: {e}"))?;
        Ok(s)
    }

    fn alive(&self) -> bool {
        !self.over.load(Ordering::SeqCst)
    }

    /// Cut everything off at once.
    fn kill(&self) {
        self.over.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            crate::voice::kill_child(c);
        }
    }
}

/// One sentence in the session.
pub struct Piece {
    s: Arc<Session>,
    /// When its sound starts (after what was queued before it) and ends, on the model's clock.
    start: Instant,
    end: Instant,
}

/// Queue a WAV in the current session (starting the player if there is none, or if the format or command changed).
pub fn queue(wav: &[u8], play_cmd: &str) -> Result<Piece, String> {
    let out = crate::audioout::global();
    let args = crate::audioout::route_args(crate::voice::split_command(play_cmd)?, out.device().as_deref());
    // the same preparation as the one-player-per-WAV path: 16-bit, low rates to 48 kHz, IYAKKI's volume
    let wav16 = crate::audioout::to_pcm16_wav(wav);
    let base = wav16.as_deref().unwrap_or(wav);
    let rated = crate::audioout::resample_low_rate_wav(base);
    let data = crate::audioout::scale_wav(rated.as_deref().unwrap_or(base), crate::audioout::gain_for(out, &args));
    let w = crate::voice::parse_wav(&data)?;
    let mut bytes = Vec::with_capacity(w.samples.len() * 2);
    for s in &w.samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let dur = Duration::from_secs_f64(w.samples.len() as f64 / (w.rate as f64 * w.channels as f64));
    let key = args.join("\u{1f}");
    let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    let reuse = cur.as_ref().filter(|s| s.alive() && s.key == key && s.rate == w.rate && s.channels == w.channels).cloned();
    let s = match reuse {
        Some(s) => s,
        None => {
            if let Some(old) = cur.take() {
                // another format or command: the old sound plays out (the card is held by one player at a time)
                let left = old.end.lock().unwrap_or_else(|e| e.into_inner()).saturating_duration_since(Instant::now());
                drop(cur);
                if old.alive() {
                    std::thread::sleep(left.min(Duration::from_secs(3)) + GRACE);
                }
                old.kill();
                cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
            }
            let s = Session::start(&args, w.rate, w.channels)?;
            *cur = Some(s.clone());
            s
        }
    };
    let now = Instant::now();
    let (start, end) = {
        let mut e = s.end.lock().unwrap_or_else(|e| e.into_inner());
        let start = (*e).max(now);
        *e = start + dur;
        (start, *e)
    };
    if s.tx.lock().unwrap_or_else(|e| e.into_inner()).send(bytes).is_err() {
        return Err("the player has ended".into());
    }
    Ok(Piece { s, start, end })
}

impl Piece {
    /// Still sounding, up to [`HANDOFF`] before its end (and not cut off).
    pub fn is_playing(&self) -> bool {
        self.s.alive() && Instant::now() + HANDOFF < self.end
    }

    /// Milliseconds of this piece heard so far, by the model's clock.
    pub fn played_ms(&self) -> u64 {
        let now = Instant::now();
        (now.saturating_duration_since(self.start).min(self.end - self.start)).as_millis() as u64
    }

    /// Cut the sound off, unless this piece already reported itself finished (its last 300 ms and the next sentence play on).
    pub fn stop(&self) {
        if Instant::now() + HANDOFF < self.end {
            self.s.kill();
        }
    }

    /// Wait for the sound to end (at most `max`, while `keep_waiting()`).
    pub fn finish(&self, max: Duration, keep_waiting: &dyn Fn() -> bool) {
        let t0 = Instant::now();
        while t0.elapsed() < max && keep_waiting() && Instant::now() < self.end && self.s.alive() {
            std::thread::sleep(Duration::from_millis(25));
        }
        self.stop();
    }

    pub fn failure(&self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(ms: u32, v: i16) -> Vec<u8> {
        let n = 48_000 * ms as usize / 1000;
        let mut b = super::super::wav_header(48_000, 1);
        // a real length, so that parse_wav is happy
        b.truncate(0);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&96_000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&((n * 2) as u32).to_le_bytes());
        for _ in 0..n {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-stream-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // the session is process-wide: these tests share it, so one test walks through the whole story
    #[test]
    fn sentences_go_to_one_player_with_one_header_and_the_hand_off_is_early_but_never_cuts_sound() {
        let d = dir("one");
        let cmd = format!("sh -c 'echo started >> {s}; cat > {o}'", s = d.join("starts").display(), o = d.join("out").display());
        let a = queue(&wav(1000, 1000), &cmd).unwrap();
        assert!(a.is_playing());
        assert_eq!(a.played_ms(), 0, "nothing of the first sentence is heard while the priming silence plays");
        std::thread::sleep(Duration::from_millis(900));
        // the first sentence (priming 350 ms + 1000 ms) is within the hand-off of its end only after about 1050 ms
        assert!(a.is_playing());
        let b = queue(&wav(500, 2000), &cmd).unwrap(); // queued while the first still sounds: no new player
        std::thread::sleep(Duration::from_millis(250));
        assert!(!a.is_playing(), "reported finished 300 ms before its end");
        a.stop(); // a finished piece is not a cut: the second keeps going
        assert!(b.s.alive());
        assert!(b.played_ms() > 0 || b.start > Instant::now(), "the second sentence's clock starts with its own sound");
        // the idle player closes by itself after the queued sound
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!b.s.alive());
        let starts = std::fs::read_to_string(d.join("starts")).unwrap();
        assert_eq!(starts.lines().count(), 1, "one player for both sentences");
        let out = std::fs::read(d.join("out")).unwrap();
        assert_eq!(&out[..4], b"RIFF");
        assert_eq!(out.windows(4).filter(|w| *w == b"RIFF").count(), 1, "the header is written once");
        let pcm: Vec<i16> = out[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let prime = 48_000 * 350 / 1000;
        assert!(pcm[..prime].iter().all(|&s| s == 0), "the player's buffer starts with silence");
        assert!(pcm[prime..prime + 48_000].iter().all(|&s| s == 1000));
        assert!(pcm[prime + 48_000..].iter().all(|&s| s == 2000), "the second sentence follows the first with nothing in between");
        assert_eq!(pcm.len(), prime + 48_000 + 24_000);
        // stopping a piece that still plays kills the player at once
        let c = queue(&wav(3000, 500), &cmd).unwrap();
        assert!(c.is_playing());
        c.stop();
        assert!(!c.s.alive());
        let _ = std::fs::remove_dir_all(&d);
    }
}
