//! The one place a program of the agent asks for the speaker.
//!
//! The radio, speech, the system sounds and the SDK's `speech.say` all call [`open_output`] / [`play_wav`] and get an [`Output`].
//! Whether the sound then goes through IYAKKI's mixer ([`crate::mixer`], Settings -> Sound "Mix sounds from several apps") or
//! through the play command of Settings -> Voice (`aplay` on the chosen output, one program at a time) is decided here and nowhere
//! else: no caller reads `sound.mixer`, and none has an `if mixer` branch.
//!
//! * mixer on: a mixer source of the kind's priority ([`crate::mixer::SourceKind`]: alarm > speech > notification > media >
//!   radio; speech and alarms duck the rest). The master volume and mute are the mixer's (the same `iyakki` PCM); the output's own
//!   volume is the source's gain. If the mixer cannot take the sound (a program outside it holds the card), the old path is tried.
//! * mixer off: exactly the old behaviour. The play command is started with a pipe, the output of Settings -> Sound is given to it,
//!   the software volume plan and `gain()` apply, and a second program finds the device busy.

/// How far the highlight of read-aloud words is moved against the heard position of the sound, ms (Settings -> Sound "Highlight
/// timing offset", -500..500, default 0). Positive lights the words later, negative earlier. The mixer reports where the sound is
/// in the speaker by its own model of the pipe, the card cannot say how much it holds, so the person tunes what is left over.
pub fn highlight_offset_ms() -> i32 {
    HIGHLIGHT_OFFSET_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by the agent when the sound settings change (see `audioout::apply_prefs`).
pub fn set_highlight_offset_ms(ms: i32) {
    HIGHLIGHT_OFFSET_MS.store(ms.clamp(-500, 500), std::sync::atomic::Ordering::Relaxed);
}

static HIGHLIGHT_OFFSET_MS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

use std::io::Write;
use std::process::{Child, ChildStdin};
use std::time::{Duration, Instant};

use crate::mixer::{Mixer, SourceKind, SourceWriter};

/// Which way sound goes right now (for the SDK, tests and the settings page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Settings -> Sound has the mixer on.
    Mixer,
    /// One program at a time through the play command.
    PlayCommand,
}

/// The way new sound goes.
pub fn route() -> Route {
    if crate::mixer::enabled() { Route::Mixer } else { Route::PlayCommand }
}

/// Whether several programs can sound at once through the mixer (the SDK's `Busy` answers depend on it).
pub fn mixing() -> bool {
    route() == Route::Mixer
}

#[path = "audiosink_stream.rs"]
mod stream;

enum Imp {
    Mixed(SourceWriter),
    /// A sentence in the long-lived player of a reading (mixer off).
    Stream(stream::Piece),
    Cmd { child: Child, stdin: Option<ChildStdin> },
}

/// Sound on its way to the speaker. Write PCM (or give a WAV to [`play_wav`]), [`finish`](Output::finish) at the end or
/// [`stop`](Output::stop) to cut it off. Dropping it stops it.
pub struct Output {
    kind: SourceKind,
    imp: Imp,
    volume: u8,
}

/// A place on the speaker for sound that arrives as it goes (`rate` Hz, `channels` interleaved 16-bit channels).
/// `play_cmd` is the play command of Settings -> Voice; it is used when the mixer is off or cannot take the sound.
pub fn open_output(kind: SourceKind, rate: u32, channels: u16, label: &str, play_cmd: &str) -> Result<Output, String> {
    open_output_via(crate::mixer::global_if_enabled(), kind, rate, channels, label, play_cmd)
}

/// A WAV played to its end (or until stopped). 16-bit PCM goes to the mixer; the play command gets the WAV as it always did.
pub fn play_wav(kind: SourceKind, label: &str, wav: &[u8], play_cmd: &str) -> Result<Output, String> {
    play_wav_via(crate::mixer::global_if_enabled(), kind, label, wav, play_cmd)
}

fn open_output_via(mixer: Option<&Mixer>, kind: SourceKind, rate: u32, channels: u16, label: &str, play_cmd: &str) -> Result<Output, String> {
    if let Some(m) = mixer {
        match m.open_source_with(kind, rate, channels as usize, label, false) {
            Ok(src) => return Ok(Output { kind, imp: Imp::Mixed(src), volume: 100 }),
            Err(e) => log::info!("sound: the mixer did not take {label} ({e}); playing it directly"),
        }
    }
    let mut child = crate::voice::spawn_player(play_cmd)?;
    let mut stdin = child.stdin.take().ok_or("the player has no input")?;
    stdin.write_all(&wav_header(rate, channels)).map_err(|_| "the player stopped".to_string())?;
    Ok(Output { kind, imp: Imp::Cmd { child, stdin: Some(stdin) }, volume: 100 })
}

fn play_wav_via(mixer: Option<&Mixer>, kind: SourceKind, label: &str, wav: &[u8], play_cmd: &str) -> Result<Output, String> {
    if let Some(m) = mixer {
        let w16 = crate::audioout::to_pcm16_wav(wav);
        if let Ok(w) = crate::voice::parse_wav(w16.as_deref().unwrap_or(wav)) {
            match m.open_source_with(kind, w.rate, w.channels as usize, label, false) {
                Ok(src) => {
                    let samples = w.samples;
                    // a long WAV waits for room in the ring: a thread feeds it, the caller gets the handle at once
                    let feeder = std::thread::Builder::new().name("iyakki-sound".into());
                    let src2 = src.clone_handle();
                    let spawned = feeder.spawn(move || {
                        if src2.write_i16_wait(&samples, Duration::from_secs(600)).is_ok() {
                            src2.finish();
                        }
                    });
                    if spawned.is_ok() {
                        return Ok(Output { kind, imp: Imp::Mixed(src), volume: 100 });
                    }
                }
                Err(e) => log::info!("sound: the mixer did not take {label} ({e}); playing it directly"),
            }
        }
    }
    play_wav_command(kind, wav, play_cmd)
}

/// Like [`play_wav`], for the sentences of one reading: with the mixer off they all go to ONE play process (header once, PCM
/// after), so the card is not closed and primed again at every sentence. The output reports itself finished a little before its
/// sound ends ([`stream::HANDOFF`]) so the caller queues the next sentence in time; stopping a finished output cuts nothing.
pub fn play_wav_streamed(kind: SourceKind, label: &str, wav: &[u8], play_cmd: &str) -> Result<Output, String> {
    if let Some(m) = crate::mixer::global_if_enabled() {
        return play_wav_via(Some(m), kind, label, wav, play_cmd);
    }
    Ok(Output { kind, imp: Imp::Stream(stream::queue(wav, play_cmd)?), volume: 100 })
}

/// A short sound (a cue, a chime) played to its end; returns when it has been played, for at most `max` on the play command.
pub fn play_wav_blocking(kind: SourceKind, label: &str, wav: &[u8], play_cmd: &str, max: Duration) -> Result<(), String> {
    play_wav_blocking_via(crate::mixer::global_if_enabled(), kind, label, wav, play_cmd, max)
}

fn play_wav_blocking_via(mixer: Option<&Mixer>, kind: SourceKind, label: &str, wav: &[u8], play_cmd: &str, max: Duration) -> Result<(), String> {
    if let Some(m) = mixer {
        match m.play_wav(kind, wav, label) {
            Ok(()) => return Ok(()),
            Err(e) => log::info!("sound: the mixer did not take {label} ({e}); playing it directly"),
        }
    }
    let out = crate::audioout::global();
    let args = crate::audioout::route_args(crate::voice::split_command(play_cmd)?, out.device().as_deref());
    if args.is_empty() {
        return Err("the play command is empty (Settings -> Voice)".into());
    }
    let data = crate::audioout::scale_wav(wav, crate::audioout::gain_for(out, &args));
    crate::voice::run_capture(crate::voice::command_for(&args, None), Some(&data), max).map(|_| ())
}

/// The play command path of a WAV: as `voice::Player::start` always did it.
fn play_wav_command(kind: SourceKind, wav: &[u8], play_cmd: &str) -> Result<Output, String> {
    use std::process::Stdio;
    let out = crate::audioout::global();
    let args = crate::audioout::route_args(crate::voice::split_command(play_cmd)?, out.device().as_deref());
    let prog = args.first().ok_or("play command is empty")?;
    let mut child = crate::voice::command_for(&args, None)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start `{prog}`: {e} (change the play command in Settings → Voice)"))?;
    if let Some(mut si) = child.stdin.take() {
        // float or 24/32-bit WAVs (Kokoro answers 32-bit float) become 16-bit first: the output chain converts rates and cannot take float
        let wav16 = crate::audioout::to_pcm16_wav(wav);
        let base = wav16.as_deref().unwrap_or(wav);
        // speech below 44.1 kHz (Kokoro: 24 kHz) is brought to 48 kHz here: the T2 MacBook's chain played it at twice the speed
        let rated = crate::audioout::resample_low_rate_wav(base);
        let data = crate::audioout::scale_wav(rated.as_deref().unwrap_or(base), crate::audioout::gain_for(out, &args));
        std::thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    Ok(Output { kind, imp: Imp::Cmd { child, stdin: None }, volume: 100 })
}

impl Output {
    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    /// Whether this output goes through the mixer.
    pub fn is_mixed(&self) -> bool {
        matches!(self.imp, Imp::Mixed(_))
    }

    /// The caller's own volume, 0 to 100 percent (the radio's slider). The master volume and mute are applied on top, once.
    pub fn set_volume(&mut self, pct: u8) {
        self.volume = pct.min(100);
        if let Imp::Mixed(s) = &self.imp {
            s.set_gain(self.volume as f32 / 100.0);
        }
    }

    /// Hand over interleaved 16-bit samples (blocks while the speaker is full). `Err`: the player stopped, with its reason.
    pub fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        match &mut self.imp {
            Imp::Mixed(s) => s.write_i16_wait(pcm, Duration::from_secs(30)),
            Imp::Stream(_) => Err("a sentence of a reading is complete when it is queued".into()),
            Imp::Cmd { child, stdin } => {
                // the output's own volume, then IYAKKI's volume and mute (HDMI has no hardware volume)
                let vol = self.volume as i32 * crate::audioout::global().gain() as i32 / 100;
                let mut bytes = Vec::with_capacity(pcm.len() * 2);
                for s in pcm {
                    bytes.extend_from_slice(&((*s as i32 * vol / 100) as i16).to_le_bytes());
                }
                let Some(si) = stdin.as_mut() else { return Err("the player has ended".into()) };
                if si.write_all(&bytes).is_err() {
                    // say what the player said: the output refusing the format is not a matter of Settings → Voice
                    return Err(crate::voice::player_failure(child));
                }
                Ok(())
            }
        }
    }

    /// How much of this sound has been heard so far, ms (the mixer's lead and pipe subtracted, by its own model; it stands still while the sound
    /// is starved). None on the play-command path, which reports nothing.
    pub fn played_ms(&self) -> Option<u64> {
        match &self.imp {
            Imp::Mixed(s) => Some(s.played_ms()),
            Imp::Stream(p) => Some(p.played_ms()),
            Imp::Cmd { .. } => None,
        }
    }

    /// Why the sound stopped, in the player's own words when it had any (empty: it said nothing).
    pub fn failure(&mut self) -> String {
        match &mut self.imp {
            Imp::Mixed(_) => String::new(),
            Imp::Stream(p) => p.failure(),
            Imp::Cmd { child, .. } => crate::voice::player_failure(child),
        }
    }

    /// No more sound will come: what was handed over plays to its end, for at most `max` and only while `keep_waiting()`. Then
    /// the output is stopped.
    pub fn finish(&mut self, max: Duration, keep_waiting: &dyn Fn() -> bool) {
        match &mut self.imp {
            Imp::Mixed(s) => {
                s.finish();
                let t0 = Instant::now();
                while t0.elapsed() < max && keep_waiting() && !s.is_done() {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            Imp::Stream(p) => p.finish(max, keep_waiting),
            Imp::Cmd { child, stdin } => {
                drop(stdin.take());
                let t0 = Instant::now();
                while t0.elapsed() < max && keep_waiting() && iyakki_core_api::child_status(child).is_running() {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
        self.stop();
    }

    /// Cut the sound off now.
    pub fn stop(&mut self) {
        match &mut self.imp {
            Imp::Mixed(s) => {
                s.flush();
                s.finish();
            }
            Imp::Stream(p) => p.stop(),
            Imp::Cmd { child, stdin } => {
                drop(stdin.take());
                crate::voice::kill_child(child);
            }
        }
    }

    /// Still sounding (not ended, not stopped).
    pub fn is_playing(&mut self) -> bool {
        match &mut self.imp {
            Imp::Mixed(s) => !s.is_done(),
            Imp::Stream(p) => p.is_playing(),
            Imp::Cmd { child, .. } => iyakki_core_api::child_status(child).is_running(),
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Imp::Cmd { child, stdin } = &mut self.imp {
            drop(stdin.take());
            crate::voice::kill_child(child);
        }
        // a mixer source finishes by itself when its writer is dropped
    }
}

/// The header of a WAV stream of unknown length (the play command reads until its input closes).
pub(crate) fn wav_header(rate: u32, channels: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    let data_len: u32 = 0x7FFF_F000;
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(data_len + 36).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    h.extend_from_slice(&(channels * 2).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

impl crate::services::Services {
    /// A place on the speaker for sound that arrives as it goes, with the play command of Settings -> Voice.
    pub fn open_output(&self, kind: SourceKind, rate: u32, channels: u16, label: &str) -> Result<Output, String> {
        open_output(kind, rate, channels, label, &self.voice_settings().play_cmd)
    }

    /// A WAV played on the speaker, with the play command of Settings -> Voice.
    pub fn play_wav_output(&self, kind: SourceKind, label: &str, wav: &[u8]) -> Result<Output, String> {
        play_wav(kind, label, wav, &self.voice_settings().play_cmd)
    }
}

#[cfg(test)]
#[path = "audiosink_tests.rs"]
mod tests;
