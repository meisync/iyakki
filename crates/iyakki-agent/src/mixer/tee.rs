//! The mixer's output also goes to the browsers that listen to the session ("sound to the browser").
//!
//! [`TeeSink`] wraps the real sink (the card through `aplay`). Every mixed period that is written to the card is also offered to
//! a tap (the connected pages, `browsermedia::broadcast_audio`); a tap with nobody listening takes nothing and keeps nothing, so a
//! session without a listener costs one lock per period. On a machine without a usable sound card (a cloud pod) the card cannot be
//! opened; when someone listens the tee then stands in for the card (the mixer paces itself on its own clock), and when nobody
//! listens the open fails as it always did, so the callers still hear `Busy` and keep their own paths.

use super::sink::Sink;

/// Offers one period of 16-bit interleaved audio at this rate and channel count; true when somebody took it.
pub type Tap = Box<dyn Fn(u32, u8, &[i16]) -> bool + Send>;
/// Is somebody listening right now?
pub type Listening = Box<dyn Fn() -> bool + Send>;
/// Must the machine's own output stay silent right now (the controlling page plays the sound instead)?
pub type Muted = Box<dyn Fn() -> bool + Send>;
/// The output stopped (closed or aborted: a pause or a stop): whatever is queued for the listeners must not play on.
pub type OnStop = Box<dyn Fn() + Send>;

pub struct TeeSink {
    inner: Box<dyn Sink>,
    tap: Tap,
    listening: Listening,
    muted: Muted,
    on_stop: Option<OnStop>,
    silent: Vec<i16>,
    rate: u32,
    channels: usize,
    /// The card is not used: the tee itself is the output (only while somebody listens).
    stand_in: bool,
    /// Periods the tap took / was offered, for tests and the status line.
    pub offered: u64,
    pub taken: u64,
}

impl TeeSink {
    pub fn new(inner: Box<dyn Sink>, tap: Tap, listening: Listening) -> TeeSink {
        TeeSink { inner, tap, listening, muted: Box::new(|| false), on_stop: None, silent: Vec::new(), rate: 0, channels: 0, stand_in: false, offered: 0, taken: 0 }
    }

    /// While `muted` says yes the card is still opened and fed (so the mixer keeps its pace and the card does not have to be
    /// reopened), but with silence; the moment it says no again the next period is the real sound.
    pub fn with_mute(mut self, muted: Muted) -> TeeSink {
        self.muted = muted;
        self
    }

    /// Called when the output closes or aborts, so a listener's queue is flushed with it.
    pub fn with_on_stop(mut self, f: OnStop) -> TeeSink {
        self.on_stop = Some(f);
        self
    }

    /// The real wiring: the pages of `browsermedia`, and the controlling page's "This computer (the browser)" choice; also the client
    /// sessions that asked for the person's sound (`clientmedia`, a connected client's `audio_open`), which can silence the machine too.
    pub fn browser(inner: Box<dyn Sink>) -> TeeSink {
        let routes = crate::clientmedia::audio_routes();
        let (r1, r2, r3, r4) = (routes.clone(), routes.clone(), routes.clone(), routes);
        TeeSink::new(
            inner,
            Box::new(move |r, c, p| {
                // both taps always get the period; `|` would skip the second
                let a = crate::browsermedia::broadcast_audio(r, c, p);
                let b = r1.tap(r, c, p);
                a || b
            }),
            Box::new(move || crate::browsermedia::audio_listeners() || r2.listening()),
        )
        .with_mute(Box::new(move || crate::browsermedia::local_muted() || r3.machine_muted()))
        .with_on_stop(Box::new(move || r4.flush_all()))
    }
}

impl Sink for TeeSink {
    fn open(&mut self, rate: u32, channels: usize) -> Result<(), String> {
        self.rate = rate;
        self.channels = channels;
        self.stand_in = false;
        match self.inner.open(rate, channels) {
            Ok(()) => Ok(()),
            Err(e) if (self.listening)() => {
                log::info!("mixer: no sound card ({e}); the browser is the output");
                self.stand_in = true;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        if !self.stand_in {
            if (self.muted)() {
                self.silent.clear();
                self.silent.resize(pcm.len(), 0);
                self.inner.write(&self.silent)?;
            } else {
                self.inner.write(pcm)?;
            }
        }
        if (1..=2).contains(&self.channels) {
            self.offered += 1;
            if (self.tap)(self.rate, self.channels as u8, pcm) {
                self.taken += 1;
            }
        }
        Ok(())
    }

    fn close(&mut self) {
        if let Some(f) = &self.on_stop {
            f();
        }
        if !self.stand_in {
            self.inner.close();
        }
        self.stand_in = false;
    }

    fn is_open(&self) -> bool {
        self.stand_in || self.inner.is_open()
    }

    fn name(&self) -> String {
        if self.stand_in {
            "the browser".into()
        } else if (self.listening)() && (self.muted)() {
            "the browser (the machine is silent)".into()
        } else if (self.listening)() {
            format!("{} and the browser", self.inner.name())
        } else {
            self.inner.name()
        }
    }

    fn detail(&self) -> String {
        self.inner.detail()
    }

    fn set_low_latency(&mut self, profile: Option<(u32, u32)>) {
        self.inner.set_low_latency(profile);
    }

    fn low_latency(&self) -> Option<(u32, u32)> {
        self.inner.low_latency()
    }

    fn abort(&mut self) {
        if let Some(f) = &self.on_stop {
            f();
        }
        self.stand_in = false;
        self.inner.abort();
    }

    fn latency_ms(&self) -> u32 {
        if self.stand_in { 0 } else { self.inner.latency_ms() }
    }

    fn stale(&self) -> bool {
        !self.stand_in && self.inner.stale()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::sink::FakeSink;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    type Got = Arc<Mutex<Vec<(u32, u8, Vec<i16>)>>>;

    fn tee(card: &FakeSink, listen: &Arc<AtomicBool>) -> (TeeSink, Got) {
        let got: Got = Arc::default();
        let (g, l, l2) = (got.clone(), listen.clone(), listen.clone());
        let t = TeeSink::new(
            Box::new(card.clone()),
            Box::new(move |r, c, p| {
                if !l.load(Ordering::SeqCst) {
                    return false;
                }
                g.lock().unwrap().push((r, c, p.to_vec()));
                true
            }),
            Box::new(move || l2.load(Ordering::SeqCst)),
        );
        (t, got)
    }

    #[test]
    fn the_card_gets_everything_and_a_listener_gets_a_copy() {
        let card = FakeSink::new();
        let listen = Arc::new(AtomicBool::new(true));
        let (mut t, got) = tee(&card, &listen);
        t.open(48_000, 2).unwrap();
        t.write(&[1, 2, 3, 4]).unwrap();
        assert_eq!(card.lock().samples, vec![1, 2, 3, 4]);
        assert_eq!(got.lock().unwrap().as_slice(), &[(48_000, 2, vec![1, 2, 3, 4])]);
        assert!(t.name().ends_with("and the browser"));
    }

    #[test]
    fn a_muted_machine_gets_silence_of_the_same_length_and_the_browser_still_gets_the_sound() {
        let card = FakeSink::new();
        let listen = Arc::new(AtomicBool::new(true));
        let mute = Arc::new(AtomicBool::new(false));
        let (t, got) = tee(&card, &listen);
        let m = mute.clone();
        let mut t = t.with_mute(Box::new(move || m.load(Ordering::SeqCst)));
        t.open(48_000, 2).unwrap();
        t.write(&[1, 2, 3, 4]).unwrap();
        mute.store(true, Ordering::SeqCst);
        assert!(t.name().contains("machine is silent"));
        t.write(&[5, 6, 7, 8]).unwrap();
        mute.store(false, Ordering::SeqCst);
        t.write(&[9, 9]).unwrap();
        assert_eq!(card.lock().samples, vec![1, 2, 3, 4, 0, 0, 0, 0, 9, 9], "silence while muted, the real sound before and after");
        assert_eq!(got.lock().unwrap().len(), 3, "the browser gets every period");
        assert_eq!(got.lock().unwrap()[1].2, vec![5, 6, 7, 8]);
    }

    #[test]
    fn nobody_listening_means_nothing_is_kept() {
        let card = FakeSink::new();
        let listen = Arc::new(AtomicBool::new(false));
        let (mut t, got) = tee(&card, &listen);
        t.open(48_000, 2).unwrap();
        t.write(&[9, 9]).unwrap();
        assert_eq!(card.lock().samples, vec![9, 9], "the card is not affected");
        assert!(got.lock().unwrap().is_empty());
        assert_eq!((t.offered, t.taken), (1, 0));
        listen.store(true, Ordering::SeqCst);
        t.write(&[5, 5]).unwrap();
        assert_eq!(got.lock().unwrap().len(), 1, "a page that starts later hears from then on");
    }

    #[test]
    fn without_a_card_a_listener_makes_the_browser_the_output() {
        let card = FakeSink::new();
        card.lock().fail_open = Some("no card".into());
        let listen = Arc::new(AtomicBool::new(true));
        let (mut t, got) = tee(&card, &listen);
        t.open(44_100, 1).unwrap();
        assert!(t.is_open() && t.name() == "the browser" && !card.lock().open);
        t.write(&[7, 8]).unwrap();
        assert_eq!(got.lock().unwrap()[0], (44_100, 1, vec![7, 8]));
        assert!(!t.stale());
        t.close();
        assert!(!t.is_open());
        // and with nobody listening the failure is the card's, as before
        listen.store(false, Ordering::SeqCst);
        assert_eq!(t.open(44_100, 1), Err("no card".to_string()));
        assert!(!t.is_open());
    }

    #[test]
    fn a_card_that_fails_to_write_is_an_error_and_nothing_is_sent() {
        let card = FakeSink::new();
        let listen = Arc::new(AtomicBool::new(true));
        let (mut t, got) = tee(&card, &listen);
        t.open(48_000, 2).unwrap();
        card.lock().open = false;
        assert!(t.write(&[1, 2]).is_err());
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn the_mixer_plays_into_the_tap_end_to_end() {
        use crate::mixer::{Mixer, MixerCfg, SourceKind};
        let card = FakeSink::new();
        card.lock().fail_open = Some("no card".into());
        let listen = Arc::new(AtomicBool::new(true));
        let (t, got) = tee(&card, &listen);
        let cfg = MixerCfg { realtime: false, raise_priority: false, engine: crate::mixer::EngineCfg::default().no_prefill(), ..MixerCfg::default() };
        let m = Mixer::start(cfg, Box::new(t));
        let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
        let tone: Vec<i16> = (0..48_000).map(|i| ((i % 100) as i16 - 50) * 100).collect();
        w.write_i16(&tone);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while got.lock().unwrap().is_empty() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let g = got.lock().unwrap();
        assert!(!g.is_empty(), "the mixed sound reached the tap");
        assert!(g.iter().any(|(r, c, p)| *r == 48_000 && *c == 2 && p.iter().any(|s| *s != 0)));
        drop(g);
        m.shutdown();
    }
}
