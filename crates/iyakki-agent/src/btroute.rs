//! The sound of the whole computer into the Bluetooth A2DP stream (F-NET-28): the mixer's output (every mixed period that goes to the
//! card) is also offered here by the mixer's tee ([`crate::mixer::tee::TeeSink::browser`]); while a route is attached the periods are queued
//! and a worker thread writes them to the sender of [`iyakki_devctl::a2dp_send`] (which converts, encodes in SBC and paces to the speed of the
//! sound), so the mixer's own clock is never held up by the headphones. The route can silence the machine's own speakers while it runs
//! ("headphones only"); if the stream fails or the headphones go away the route removes itself and the speakers come back, with the reason
//! kept for Settings and the assistant.
//!
//! The queue holds at most [`MAX_QUEUE_MS`] of sound; when the headphones fall behind the oldest sound is dropped and counted, so a slow
//! link makes gaps, not a growing delay or memory.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use iyakki_devctl::a2dp_send::SendControl;

/// The most sound waiting for the headphones.
pub const MAX_QUEUE_MS: u64 = 600;

struct Period {
    rate: u32,
    channels: u8,
    pcm: Vec<i16>,
}

impl Period {
    fn ms(&self) -> u64 {
        self.pcm.len() as u64 * 1000 / (self.rate.max(1) as u64 * self.channels.max(1) as u64)
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<Period>,
    queued_ms: u64,
    dropped_ms: u64,
    sent_ms: u64,
    /// Bumped on every attach and detach, so a worker of an earlier attach ends.
    generation: u64,
    failed: String,
}

struct Attached {
    mute_local: bool,
}

/// The route. One per machine ([`global`]); tests make their own.
pub struct BtRoute {
    state: Mutex<State>,
    wake: Condvar,
    attached: Mutex<Option<Attached>>,
    /// Lock-free copies for the mixer's tee, which asks every period.
    on: AtomicBool,
    mute: AtomicBool,
}

impl Default for BtRoute {
    fn default() -> Self {
        BtRoute::new()
    }
}

impl BtRoute {
    pub fn new() -> BtRoute {
        BtRoute { state: Mutex::new(State::default()), wake: Condvar::new(), attached: Mutex::new(None), on: AtomicBool::new(false), mute: AtomicBool::new(false) }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Send the computer's sound through `sender` (which must already be streaming). `mute_local` silences the machine's own speakers while
    /// the route runs. Replaces an earlier route.
    pub fn attach(self: &Arc<Self>, sender: Arc<dyn SendControl>, mute_local: bool) {
        let generation = {
            let mut s = self.st();
            s.generation += 1;
            s.queue.clear();
            s.queued_ms = 0;
            s.dropped_ms = 0;
            s.sent_ms = 0;
            s.failed.clear();
            s.generation
        };
        *self.attached.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Attached { mute_local });
        self.mute.store(mute_local, Ordering::Relaxed);
        self.on.store(true, Ordering::Relaxed);
        let me = self.clone();
        std::thread::spawn(move || me.work(generation, sender));
        self.wake.notify_all();
    }

    /// Stop routing; the speakers are back at once. The sender itself is not stopped here (the caller owns it).
    pub fn detach(&self) {
        self.on.store(false, Ordering::Relaxed);
        self.mute.store(false, Ordering::Relaxed);
        *self.attached.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let mut s = self.st();
        s.generation += 1;
        s.queue.clear();
        s.queued_ms = 0;
        drop(s);
        self.wake.notify_all();
    }

    fn fail(&self, generation: u64, why: String) {
        let mut s = self.st();
        if s.generation != generation {
            return;
        }
        s.failed = why;
        drop(s);
        self.detach_keep_reason();
    }

    fn detach_keep_reason(&self) {
        let reason = self.st().failed.clone();
        self.detach();
        self.st().failed = reason;
    }

    fn work(&self, generation: u64, sender: Arc<dyn SendControl>) {
        loop {
            let p = {
                let mut s = self.st();
                loop {
                    if s.generation != generation {
                        return;
                    }
                    if let Some(p) = s.queue.pop_front() {
                        s.queued_ms = s.queued_ms.saturating_sub(p.ms());
                        break p;
                    }
                    s = self.wake.wait_timeout(s, std::time::Duration::from_millis(250)).unwrap_or_else(std::sync::PoisonError::into_inner).0;
                }
            };
            match sender.write(&p.pcm, p.rate, p.channels as usize) {
                Ok(()) => {
                    let mut s = self.st();
                    if s.generation == generation {
                        s.sent_ms += p.ms();
                    }
                }
                Err(e) => {
                    self.fail(generation, format!("the headphones stopped taking sound: {e}"));
                    return;
                }
            }
        }
    }

    /// The mixer's tap: one mixed period. Takes it (true) while a route is attached; never blocks.
    pub fn tap(&self, rate: u32, channels: u8, pcm: &[i16]) -> bool {
        if !self.on.load(Ordering::Relaxed) || pcm.is_empty() {
            return false;
        }
        let p = Period { rate, channels, pcm: pcm.to_vec() };
        let mut s = self.st();
        s.queued_ms += p.ms();
        s.queue.push_back(p);
        while s.queued_ms > MAX_QUEUE_MS {
            if let Some(old) = s.queue.pop_front() {
                let ms = old.ms();
                s.queued_ms = s.queued_ms.saturating_sub(ms);
                s.dropped_ms += ms;
            } else {
                break;
            }
        }
        drop(s);
        self.wake.notify_one();
        true
    }

    /// Is a route attached (the tee counts it as a listener)?
    pub fn listening(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Must the machine's own speakers be silent now?
    pub fn muted(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }

    /// One line for Settings and the assistant.
    pub fn status(&self) -> String {
        let s = self.st();
        if self.on.load(Ordering::Relaxed) {
            let local = self.attached.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|a| a.mute_local);
            format!("this computer's sound goes to the headphones ({}; {} s sent{}; {} ms waiting{})", if local { "the speakers are silent" } else { "the speakers play too" }, s.sent_ms / 1000, if s.dropped_ms > 0 { format!(", {} ms dropped because the headphones fell behind", s.dropped_ms) } else { String::new() }, s.queued_ms, "")
        } else if !s.failed.is_empty() {
            format!("not routed: {}; the speakers play again", s.failed)
        } else {
            "this computer's sound is not routed to headphones".into()
        }
    }

    pub fn sent_ms(&self) -> u64 {
        self.st().sent_ms
    }
    pub fn dropped_ms(&self) -> u64 {
        self.st().dropped_ms
    }
}

/// The machine's route, fed by the mixer's tee.
pub fn global() -> Arc<BtRoute> {
    static R: OnceLock<Arc<BtRoute>> = OnceLock::new();
    R.get_or_init(|| Arc::new(BtRoute::new())).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_devctl::a2dp_send::SendView;
    use std::time::{Duration, Instant};

    /// A sender that records what it was given; `slow` makes each write take that long, `fail_after` makes it fail.
    struct Rec {
        got: Mutex<Vec<(u32, usize, usize)>>,
        slow: Duration,
        fail_after: Option<usize>,
    }
    impl SendControl for Rec {
        fn view(&self) -> SendView {
            SendView { active: true, device: "fake".into(), codec: "SBC".into(), ..Default::default() }
        }
        fn start(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            Ok(())
        }
        fn write(&self, pcm: &[i16], rate: u32, channels: usize) -> Result<(), String> {
            let mut g = self.got.lock().unwrap();
            if self.fail_after.is_some_and(|n| g.len() >= n) {
                return Err("link lost".into());
            }
            g.push((rate, channels, pcm.len()));
            drop(g);
            std::thread::sleep(self.slow);
            Ok(())
        }
    }
    fn rec(slow: u64, fail_after: Option<usize>) -> Arc<Rec> {
        Arc::new(Rec { got: Mutex::new(vec![]), slow: Duration::from_millis(slow), fail_after })
    }
    fn wait(mut f: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
    const PERIOD: usize = 960; // 10 ms of 48 kHz stereo

    #[test]
    fn nothing_is_taken_until_a_route_is_attached_and_the_speakers_stay_on() {
        let r = Arc::new(BtRoute::new());
        assert!(!r.tap(48_000, 2, &[1; PERIOD]) && !r.listening() && !r.muted());
        assert!(r.status().contains("not routed to headphones"));
    }

    #[test]
    fn every_period_of_the_mixers_output_reaches_the_sender_in_order_and_the_speakers_can_be_muted() {
        let r = Arc::new(BtRoute::new());
        let s = rec(0, None);
        r.attach(s.clone(), true);
        assert!(r.listening() && r.muted());
        for _ in 0..20 {
            assert!(r.tap(48_000, 2, &[5; PERIOD]));
        }
        assert!(wait(|| s.got.lock().unwrap().len() == 20), "all twenty periods arrive");
        assert!(s.got.lock().unwrap().iter().all(|g| *g == (48_000, 2, PERIOD)));
        assert_eq!(r.sent_ms(), 200);
        assert!(r.status().contains("speakers are silent"));
        r.detach();
        assert!(!r.listening() && !r.muted(), "the speakers are back at once");
        assert!(!r.tap(48_000, 2, &[5; PERIOD]));
    }

    #[test]
    fn headphones_that_fall_behind_make_gaps_not_a_growing_queue() {
        let r = Arc::new(BtRoute::new());
        let s = rec(200, None); // each write takes 200 ms; the mixer offers 10 ms periods at once
        r.attach(s, false);
        for _ in 0..500 {
            r.tap(48_000, 2, &[1; PERIOD]);
        }
        assert!(r.dropped_ms() > 4000, "most of 5 s was dropped, not queued: {}", r.dropped_ms());
        assert!(r.status().contains("dropped"));
        assert!(r.st().queued_ms <= MAX_QUEUE_MS);
        r.detach();
    }

    #[test]
    fn a_failing_stream_removes_the_route_and_gives_the_speakers_back_with_the_reason() {
        let r = Arc::new(BtRoute::new());
        let s = rec(0, Some(3));
        r.attach(s, true);
        for _ in 0..10 {
            r.tap(48_000, 2, &[1; PERIOD]);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(wait(|| !r.listening()), "the route removes itself");
        assert!(!r.muted());
        let t = r.status();
        assert!(t.contains("link lost") && t.contains("speakers play again"), "{t}");
        // a new attach starts clean
        r.attach(rec(0, None), false);
        assert!(r.listening() && !r.status().contains("link lost"));
        r.detach();
    }

    #[test]
    fn the_mixers_tee_offers_each_period_to_the_route() {
        use crate::mixer::sink::Sink;
        // the real tee with the machine's route: a period written to the card is offered to the global route
        struct Null;
        impl Sink for Null {
            fn open(&mut self, _: u32, _: usize) -> Result<(), String> {
                Ok(())
            }
            fn write(&mut self, _: &[i16]) -> Result<(), String> {
                Ok(())
            }
            fn close(&mut self) {}
            fn is_open(&self) -> bool {
                true
            }
            fn name(&self) -> String {
                "null".into()
            }
        }
        let g = global();
        let s = rec(0, None);
        g.attach(s.clone(), true);
        let mut tee = crate::mixer::tee::TeeSink::browser(Box::new(Null));
        tee.open(48_000, 2).unwrap();
        for _ in 0..5 {
            tee.write(&[3; PERIOD]).unwrap();
        }
        assert!(wait(|| s.got.lock().unwrap().len() == 5), "the tee fed the route");
        assert!(tee.taken >= 5);
        assert!(tee.name().contains("silent"), "the machine is silent while the route mutes it: {}", tee.name());
        g.detach();
    }
}
