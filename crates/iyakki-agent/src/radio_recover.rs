//! What the radio does when a live station's connection ends or fails while it plays (F-RADIO-17, owner order 2026-11-19: "switch to the
//! next radio or retry 1 or 2"). The decisions are pure ([`Recovery`]) so tests drive them without a thread, a socket or a sleep; the
//! player ([`crate::radio::RadioPlayer`]) asks it after every end and does what it says. The clock is injected ([`RecoverClock`]): the real
//! one sleeps in short slices, a test's one only moves a number.
//!
//! The rules, in order:
//! 1. The person's explicit start is needed for the first play. A station that never made a sound on that first start is NOT retried
//!    and NOT replaced: the plain failure is shown, as before.
//! 2. A station that was playing and ended is reconnected up to `retries` times (default 2, 0 to 5), each after a short bounded wait.
//! 3. When the retries fail and `auto_switch` is on (default), the radio goes on to the NEXT station of the person's current list,
//!    wrapping at its end. A station reached that way that never starts is skipped without retries.
//! 4. It never loops: every station that failed in this run is remembered, and when every station of the list has failed the radio
//!    stops and says so in words. Only a station that then plays for a while ([`STABLE_MS`]) clears that memory.
//! 5. After that full pass it is parked, not dead (F-LIFE-11, owner decision P-01): it shows the plain message and starts again by
//!    itself when the network link returns, and meanwhile after 1, then 5, then every 15 minutes ([`ParkWait`], on the injected clock).

use std::time::Duration;

use iyakki_tile_api::radio::Station;

/// A station that played this long without ending counts as a success: its retries and the failed list start over.
pub const STABLE_MS: u64 = 30_000;

/// Time for the recovery: what time it is, and waiting a bounded while (leaving at once when `keep_going` turns false).
pub trait RecoverClock: Send + Sync {
    fn now_ms(&self) -> u64;
    fn wait(&self, d: Duration, keep_going: &dyn Fn() -> bool);
}

/// The real clock: waits in slices of 20 ms so a stop is noticed quickly.
pub struct RealClock(std::time::Instant);

impl Default for RealClock {
    fn default() -> RealClock {
        RealClock(std::time::Instant::now())
    }
}

impl RecoverClock for RealClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn wait(&self, d: Duration, keep_going: &dyn Fn() -> bool) {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < d && keep_going() {
            std::thread::sleep(Duration::from_millis(20).min(d));
        }
    }
}

/// What to do after one end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Connect the same station again (`n` of `of`).
    Retry { n: u32, of: u32 },
    /// Go on to another station of the list.
    Switch { from: String, to: Station },
    /// Stop; the text is the failure shown to the person.
    GiveUp(String),
    /// Every station of the list failed in one pass: stop with this text, then start again by itself ([`ParkWait`]).
    Park(String),
}

/// The slow retries after a full pass of failing stations: 1, 5 and then every 15 minutes (milliseconds).
pub const PARK_DELAYS_MS: [u64; 3] = [60_000, 300_000, 900_000];

/// One look at the clock and the link every this long while parked (the real clock sleeps in 20 ms slices inside this).
pub const PARK_SLICE: Duration = Duration::from_secs(5);

/// Why a parked radio tries again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    /// The network link came back (it was down at the last look and is up now).
    LinkReturned,
    /// The slow retry time of this round is over.
    Timer,
}

/// The waiting of a parked radio, pure: feed it the clock and the link at each look, it says when to try again.
pub struct ParkWait {
    round: usize,
    since_ms: u64,
    link_was_up: bool,
}

impl ParkWait {
    /// Parked at `now_ms`, `round` slow retries already used (0 for the first), `link_up` as seen now.
    pub fn new(now_ms: u64, round: usize, link_up: bool) -> ParkWait {
        ParkWait { round, since_ms: now_ms, link_was_up: link_up }
    }
    pub fn delay_ms(&self) -> u64 {
        PARK_DELAYS_MS[self.round.min(PARK_DELAYS_MS.len() - 1)]
    }
    /// The words for the status line while parked.
    pub fn notice(&self, link_up: bool) -> String {
        let m = self.delay_ms() / 60_000;
        if link_up {
            format!("Trying again in {m} minute{}", if m == 1 { "" } else { "s" })
        } else {
            "Waiting for the network".into()
        }
    }
    /// One look: `Some` when to start again.
    pub fn look(&mut self, now_ms: u64, link_up: bool) -> Option<Wake> {
        let returned = link_up && !self.link_was_up;
        self.link_was_up = link_up;
        if returned {
            return Some(Wake::LinkReturned);
        }
        (now_ms.saturating_sub(self.since_ms) >= self.delay_ms()).then_some(Wake::Timer)
    }
}

pub struct Recovery {
    retries: u32,
    auto_switch: bool,
    list: Vec<Station>,
    /// Addresses of the stations that failed since the last success.
    failed: Vec<String>,
    used: u32,
    attempts: u32,
    switched: bool,
}

impl Recovery {
    pub fn new(retries: u32, auto_switch: bool, list: &[Station]) -> Recovery {
        let mut seen: Vec<Station> = Vec::new();
        for s in list {
            if !seen.iter().any(|x| x.url == s.url) {
                seen.push(s.clone());
            }
        }
        Recovery { retries, auto_switch, list: seen, failed: Vec::new(), used: 0, attempts: 0, switched: false }
    }

    /// The connection of `current` ended. `played_ms` is how long it made sound (`None`: never).
    pub fn on_end(&mut self, current: &Station, played_ms: Option<u64>, why: &str) -> Step {
        self.attempts += 1;
        if played_ms.is_some_and(|ms| ms >= STABLE_MS) {
            self.used = 0;
            self.failed.clear();
        }
        if played_ms.is_none() && self.attempts == 1 {
            if !self.switched {
                // the person's own start: the plain failure, no retries, no other station
                return Step::GiveUp(why.to_string());
            }
            // a station the radio moved to that does not start: next one, no retries
            return self.move_on(current, why);
        }
        if self.used < self.retries {
            self.used += 1;
            return Step::Retry { n: self.used, of: self.retries };
        }
        self.move_on(current, why)
    }

    fn tries_said(&self) -> String {
        match self.used {
            0 => String::new(),
            1 => " Retried once.".into(),
            n => format!(" Retried {n} times."),
        }
    }

    fn move_on(&mut self, current: &Station, why: &str) -> Step {
        if !self.failed.contains(&current.url) {
            self.failed.push(current.url.clone());
        }
        let tries = self.tries_said();
        if !self.auto_switch {
            return Step::GiveUp(format!("{why}.{tries} Switching to another station is off in Settings -> Radio."));
        }
        let Some(at) = self.list.iter().position(|s| s.url == current.url) else {
            return Step::GiveUp(format!("{why}.{tries} There is no list of stations to move on in."));
        };
        if self.list.len() < 2 {
            return Step::Park(format!("{why}.{tries} There is no other station in the list, so the radio stopped."));
        }
        for i in 1..self.list.len() {
            let cand = &self.list[(at + i) % self.list.len()];
            if !self.failed.contains(&cand.url) {
                self.used = 0;
                self.attempts = 0;
                self.switched = true;
                return Step::Switch { from: current.name.clone(), to: cand.clone() };
            }
        }
        Step::Park(format!("{why}.{tries} Every one of the {} stations in the list failed, so the radio stopped.", self.list.len()))
    }

    /// The same memory after a parked wait: the list starts over and the first station is no longer "the person's own first start"
    /// (a station that does not start is skipped without retries, as for one the radio moved to).
    pub fn after_park(&mut self) {
        self.failed.clear();
        self.used = 0;
        self.attempts = 0;
        self.switched = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slow_retries_are_one_five_and_then_fifteen_minutes() {
        for (round, ms) in [(0, 60_000), (1, 300_000), (2, 900_000), (3, 900_000), (9, 900_000)] {
            let mut w = ParkWait::new(1_000, round, true);
            assert_eq!(w.delay_ms(), ms);
            assert_eq!(w.look(1_000 + ms - 1, true), None, "not before the time");
            assert_eq!(w.look(1_000 + ms, true), Some(Wake::Timer));
        }
    }

    #[test]
    fn a_link_that_comes_back_wakes_at_once_but_one_that_was_always_up_does_not() {
        let mut w = ParkWait::new(0, 0, false);
        assert_eq!(w.look(5_000, false), None);
        assert_eq!(w.look(10_000, true), Some(Wake::LinkReturned));
        assert_eq!(w.look(15_000, true), None, "still up is not a return");
        assert_eq!(w.look(20_000, false), None);
        assert_eq!(w.look(25_000, true), Some(Wake::LinkReturned), "down and up again is another return");
        let mut up = ParkWait::new(0, 0, true);
        assert_eq!(up.look(30_000, true), None);
    }

    #[test]
    fn the_notice_says_what_it_waits_for() {
        assert_eq!(ParkWait::new(0, 0, false).notice(false), "Waiting for the network");
        assert_eq!(ParkWait::new(0, 0, true).notice(true), "Trying again in 1 minute");
        assert_eq!(ParkWait::new(0, 1, true).notice(true), "Trying again in 5 minutes");
    }

    #[test]
    fn a_list_that_failed_once_through_starts_over_after_a_parked_wait() {
        let mut r = Recovery::new(0, true, &list(&["a", "b"]));
        assert!(matches!(r.on_end(&st("a"), Some(1), "x"), Step::Switch { .. }));
        assert!(matches!(r.on_end(&st("b"), None, "x"), Step::Park(_)));
        r.after_park();
        // a that does not start now is skipped to b (not shown as the person's own first start), and only then is it parked again
        assert!(matches!(r.on_end(&st("a"), None, "x"), Step::Switch { .. }));
        assert!(matches!(r.on_end(&st("b"), None, "x"), Step::Park(_)));
    }

    fn st(n: &str) -> Station {
        Station::new(n, &format!("https://{n}.example.org/live"))
    }
    fn list(names: &[&str]) -> Vec<Station> {
        names.iter().map(|n| st(n)).collect()
    }

    #[test]
    fn a_first_start_that_never_played_is_shown_as_it_is_without_retries_or_switching() {
        let mut r = Recovery::new(2, true, &list(&["a", "b"]));
        assert_eq!(r.on_end(&st("a"), None, "404"), Step::GiveUp("404".into()));
    }

    #[test]
    fn a_dropped_station_is_retried_twice_and_then_the_next_one_plays() {
        let mut r = Recovery::new(2, true, &list(&["a", "b", "c"]));
        assert_eq!(r.on_end(&st("a"), Some(5_000), "gone"), Step::Retry { n: 1, of: 2 });
        assert_eq!(r.on_end(&st("a"), None, "refused"), Step::Retry { n: 2, of: 2 });
        assert_eq!(r.on_end(&st("a"), None, "refused"), Step::Switch { from: "a".into(), to: st("b") });
    }

    #[test]
    fn the_list_wraps_and_the_radio_stops_after_one_full_pass_saying_so() {
        let mut r = Recovery::new(0, true, &list(&["a", "b", "c"]));
        // c is playing, drops: next is a (wrapped)
        assert_eq!(r.on_end(&st("c"), Some(1_000), "gone"), Step::Switch { from: "c".into(), to: st("a") });
        // a never starts: skipped, b
        assert_eq!(r.on_end(&st("a"), None, "404"), Step::Switch { from: "a".into(), to: st("b") });
        // b never starts: every station has failed once
        match r.on_end(&st("b"), None, "404") {
            Step::Park(t) => assert!(t.contains("Every one of the 3 stations"), "{t}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_stable_station_clears_the_memory_of_failures() {
        let mut r = Recovery::new(0, true, &list(&["a", "b"]));
        assert_eq!(r.on_end(&st("a"), Some(1), "x"), Step::Switch { from: "a".into(), to: st("b") });
        // b plays for a long time, then drops: a is tried again, it is not "already failed"
        assert_eq!(r.on_end(&st("b"), Some(STABLE_MS), "x"), Step::Switch { from: "b".into(), to: st("a") });
    }

    #[test]
    fn a_short_lived_station_does_not_loop_forever() {
        let mut r = Recovery::new(1, true, &list(&["a", "b"]));
        let mut steps = 0;
        let mut cur = st("a");
        loop {
            steps += 1;
            assert!(steps < 20, "no end");
            match r.on_end(&cur, Some(1_000), "x") {
                Step::Retry { .. } => {}
                Step::Switch { to, .. } => cur = to,
                Step::GiveUp(_) | Step::Park(_) => break,
            }
        }
    }

    #[test]
    fn switching_off_one_station_lists_and_unknown_stations_give_up_in_words() {
        let mut r = Recovery::new(1, false, &list(&["a", "b"]));
        assert!(matches!(r.on_end(&st("a"), Some(1), "x"), Step::Retry { .. }));
        match r.on_end(&st("a"), Some(1), "gone") {
            Step::GiveUp(t) => assert!(t.contains("Retried once") && t.contains("is off in Settings -> Radio"), "{t}"),
            o => panic!("{o:?}"),
        }
        let mut one = Recovery::new(0, true, &list(&["a"]));
        assert!(matches!(one.on_end(&st("a"), Some(1), "x"), Step::Park(t) if t.contains("no other station")));
        let mut none = Recovery::new(0, true, &[]);
        assert!(matches!(none.on_end(&st("a"), Some(1), "x"), Step::GiveUp(t) if t.contains("no list")));
    }

    // ---- through the player, with a scripted station, a manual clock and no sleep ----

    use crate::radio::{RadioPlayer, RadioState};
    use iyakki_proxy::{StreamHead, Streamer};
    use iyakki_testkit::{wait_until, ManualClock, BUDGET};
    use std::collections::{HashMap, VecDeque};
    use std::io::{Cursor, Read};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    /// What one connection to a station does.
    enum Conn {
        /// Sends `n` frames of sound and ends.
        Play(usize),
        /// Sends `n` frames and then stays silent and open until the test lets go.
        Hold(usize),
        Refuse,
        NotFound,
    }

    fn silence(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..n {
            let mut f = vec![0u8; 417];
            f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            v.extend_from_slice(&f);
        }
        v
    }

    struct Held {
        data: Cursor<Vec<u8>>,
        gate: Receiver<()>,
    }

    impl Read for Held {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.data.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            let _ = self.gate.recv();
            Ok(0)
        }
    }

    #[derive(Default)]
    struct Script {
        conns: Mutex<HashMap<String, VecDeque<Conn>>>,
        asked: Mutex<Vec<String>>,
        gates: Mutex<Vec<Sender<()>>>,
    }

    impl Script {
        fn new(v: Vec<(&str, Vec<Conn>)>) -> Arc<Script> {
            let s = Script::default();
            for (n, c) in v {
                s.conns.lock().unwrap().insert(format!("https://{n}.example.org/live"), c.into());
            }
            Arc::new(s)
        }
        fn asked(&self, n: &str) -> usize {
            let u = format!("https://{n}.example.org/live");
            self.asked.lock().unwrap().iter().filter(|x| **x == u).count()
        }
    }

    impl Streamer for Script {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.asked.lock().unwrap().push(url.to_string());
            let next = self.conns.lock().unwrap().get_mut(url).and_then(VecDeque::pop_front);
            let mp3 = |body: Box<dyn Read + Send>| StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body };
            match next {
                Some(Conn::Play(n)) => Ok(mp3(Box::new(Cursor::new(silence(n))))),
                Some(Conn::Hold(n)) => {
                    let (tx, rx) = channel();
                    self.gates.lock().unwrap().push(tx);
                    Ok(mp3(Box::new(Held { data: Cursor::new(silence(n)), gate: rx })))
                }
                Some(Conn::NotFound) => Ok(StreamHead { status: 404, headers: vec![], body: Box::new(Cursor::new(Vec::new())) }),
                Some(Conn::Refuse) | None => Err("connection refused".into()),
            }
        }
    }

    /// Time that only moves when the player waits: every wait is recorded and nothing sleeps. A parked radio's look (`PARK_SLICE`)
    /// is held until the test grants it, so a test decides when the 5 seconds pass.
    struct Manual {
        clock: ManualClock,
        waits: Mutex<Vec<Duration>>,
        /// (permits granted and not yet used, looks passed so far, held at a look now)
        gate: Mutex<(u64, u64, bool)>,
        cv: std::sync::Condvar,
    }

    impl Manual {
        /// Lets `n` looks of the parked radio pass, then waits until it is held at the next one (so everything it did has happened).
        fn grant(&self, n: u64) {
            let target = {
                let mut g = self.gate.lock().unwrap();
                g.0 += n;
                g.1 + n
            };
            self.cv.notify_all();
            wait_until(BUDGET, || {
                let g = self.gate.lock().unwrap();
                g.1 == target && g.2
            });
        }
        /// Lets `n` looks pass without waiting for the next hold (the radio is expected to leave the parked state).
        fn release(&self, n: u64) {
            self.gate.lock().unwrap().0 += n;
            self.cv.notify_all();
        }
        fn wake(&self) {
            self.cv.notify_all();
        }
    }

    impl RecoverClock for Manual {
        fn now_ms(&self) -> u64 {
            self.clock.now_ms()
        }
        fn wait(&self, d: Duration, keep_going: &dyn Fn() -> bool) {
            if d == PARK_SLICE {
                let mut g = self.gate.lock().unwrap();
                g.2 = true;
                while g.0 == 0 {
                    if !keep_going() {
                        g.2 = false;
                        return;
                    }
                    g = self.cv.wait(g).unwrap();
                }
                g.0 -= 1;
                g.1 += 1;
                g.2 = false;
                drop(g);
                self.clock.advance(d);
                return;
            }
            self.waits.lock().unwrap().push(d);
            self.clock.advance(d);
        }
    }

    struct Rig {
        radio: RadioPlayer,
        script: Arc<Script>,
        clock: Arc<Manual>,
        audit: Arc<Mutex<Vec<String>>>,
        link: Arc<std::sync::atomic::AtomicBool>,
        _route: Box<dyn std::any::Any>,
    }

    fn rig(script: Vec<(&str, Vec<Conn>)>, retries: u32, auto: bool) -> Rig {
        let route = crate::mixer::pin_command_route();
        let script = Script::new(script);
        let clock = Arc::new(Manual { clock: ManualClock::new(1_000), waits: Mutex::new(Vec::new()), gate: Mutex::new((0, 0, false)), cv: std::sync::Condvar::new() });
        let link = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let probe = link.clone();
        let radio = RadioPlayer::default();
        radio.set_streamer(script.clone());
        radio.set_recover_clock(clock.clone());
        radio.set_link_probe(Arc::new(move || probe.load(std::sync::atomic::Ordering::SeqCst)));
        radio.set_recover_source(Arc::new(move || (retries, auto)));
        radio.set_retry_wait(Duration::from_secs(3));
        Rig { radio, script, clock, audit: Arc::new(Mutex::new(Vec::new())), link, _route: Box::new(route) }
    }

    impl Rig {
        fn start(&self, from: &str, names: &[&str], cmd: &str) {
            let audit = self.audit.clone();
            let around = crate::radio::Around { list: list(names), audit: Some(Arc::new(move |a, s| audit.lock().unwrap().push(format!("{a} {s}")))) };
            self.radio.play_in(&st(from), Arc::new(|_| true), cmd, around);
        }
        fn playing(&self, name: &str) {
            wait_until(BUDGET, || matches!(self.radio.state(), RadioState::Playing { ref station, .. } if station == name));
        }
        fn failed(&self) -> String {
            wait_until(BUDGET, || matches!(self.radio.state(), RadioState::Failed { .. }));
            match self.radio.state() {
                RadioState::Failed { why, .. } => why,
                s => panic!("{s:?}"),
            }
        }
        fn waits(&self) -> usize {
            self.clock.waits.lock().unwrap().len()
        }
        fn end(self) {
            self.radio.stop();
            self.clock.wake();
            self.script.gates.lock().unwrap().clear();
        }
    }

    #[test]
    fn a_dropped_station_that_comes_back_on_the_first_retry_plays_on_and_says_so() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Hold(8)]), ("b", vec![Conn::Hold(8)])], 2, true);
        r.start("a", &["a", "b"], "tee /dev/null");
        wait_until(BUDGET, || r.radio.notice().as_deref() == Some("Reconnected") && matches!(r.radio.state(), RadioState::Playing { .. }));
        assert_eq!((r.script.asked("a"), r.script.asked("b"), r.waits()), (2, 0, 1));
        assert_eq!(r.clock.waits.lock().unwrap()[0], Duration::from_secs(3), "the wait is the short bounded one");
        assert!(r.audit.lock().unwrap().is_empty(), "no switch, no audit line");
        r.end();
    }

    #[test]
    fn a_station_that_comes_back_on_the_second_retry_says_each_step() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Refuse, Conn::Hold(8)])], 2, true);
        r.start("a", &["a"], "tee /dev/null");
        r.playing("a");
        wait_until(BUDGET, || r.script.asked("a") == 3 && r.radio.notice().as_deref() == Some("Reconnected"));
        assert_eq!(r.waits(), 2);
        r.end();
    }

    #[test]
    fn failed_retries_go_on_to_the_next_station_with_one_audit_line() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Refuse, Conn::Refuse]), ("b", vec![Conn::Hold(8)])], 2, true);
        r.start("a", &["a", "b"], "tee /dev/null");
        r.playing("b");
        assert_eq!(r.radio.notice().as_deref(), Some("Switched to b"));
        assert_eq!((r.script.asked("a"), r.script.asked("b"), r.waits()), (3, 1, 2));
        let audit = r.audit.lock().unwrap().clone();
        assert_eq!(audit.len(), 1, "{audit:?}");
        assert!(audit[0].starts_with("radio.switch ") && audit[0].contains("a.example.org -> b.example.org"), "{audit:?}");
        r.end();
    }

    #[test]
    fn the_list_wraps_at_its_end() {
        let r = rig(vec![("c", vec![Conn::Play(8)]), ("a", vec![Conn::Hold(8)])], 0, true);
        r.start("c", &["a", "b", "c"], "tee /dev/null");
        r.playing("a");
        assert_eq!(r.waits(), 0, "no retries asked for");
        assert_eq!(r.script.asked("b"), 0);
        r.end();
    }

    #[test]
    fn after_one_full_pass_without_success_the_radio_stops_and_says_so() {
        let r = rig(vec![("a", vec![Conn::Play(8)]), ("b", vec![Conn::Refuse])], 0, true);
        r.start("a", &["a", "b"], "tee /dev/null");
        let why = r.failed();
        assert!(why.contains("Every one of the 2 stations in the list failed"), "{why}");
        assert_eq!((r.script.asked("a"), r.script.asked("b")), (1, 1), "nobody is asked twice: it does not loop");
        assert_eq!(r.audit.lock().unwrap().len(), 1);
        r.end();
    }

    #[test]
    fn with_switching_off_the_radio_retries_and_then_stops_where_it_is() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Refuse]), ("b", vec![Conn::Hold(8)])], 1, false);
        r.start("a", &["a", "b"], "tee /dev/null");
        let why = r.failed();
        assert!(why.contains("Retried once") && why.contains("off in Settings -> Radio"), "{why}");
        assert_eq!(r.script.asked("b"), 0);
        assert!(r.audit.lock().unwrap().is_empty());
        r.end();
    }

    #[test]
    fn a_first_start_that_fails_is_shown_and_neither_retried_nor_replaced() {
        let r = rig(vec![("a", vec![Conn::NotFound]), ("b", vec![Conn::Hold(8)])], 2, true);
        r.start("a", &["a", "b"], "tee /dev/null");
        let why = r.failed();
        assert!(why.contains("404"), "{why}");
        assert_eq!((r.script.asked("a"), r.script.asked("b"), r.waits()), (1, 0, 0));
        r.end();
    }

    #[test]
    fn sound_already_handed_over_plays_out_when_the_stream_drops() {
        // the same silence once as a file that ends (which always played out) and once as a live stream that drops: the same bytes reach the player
        let dir = std::env::temp_dir().join(format!("iyakki-recover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (live_out, file_out, mp3) = (dir.join("live.wav"), dir.join("file.wav"), dir.join("ref.mp3"));
        std::fs::write(&mp3, silence(30)).unwrap();
        let reference = rig(vec![], 0, false);
        reference.radio.play(&crate::radio::Station::new("file", &format!("file://{}", mp3.display())), Arc::new(|_| true), &format!("tee {}", file_out.display()));
        wait_until(BUDGET, || reference.radio.state() == RadioState::Idle);
        let want = std::fs::metadata(&file_out).unwrap().len();
        assert!(want > 44 + 4608, "the reference played: {want}");
        reference.end();
        let r = rig(vec![("a", vec![Conn::Play(30)])], 0, false);
        r.start("a", &["a"], &format!("tee {}", live_out.display()));
        let _ = r.failed();
        assert_eq!(std::fs::metadata(&live_out).unwrap().len(), want, "nothing the radio had decoded was cut off");
        r.end();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_network_the_radio_says_so_and_starts_again_when_the_link_returns() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Hold(8)])], 0, true);
        r.link.store(false, std::sync::atomic::Ordering::SeqCst);
        // a plays, drops; the list has no other station: parked
        r.start("a", &["a"], "tee /dev/null");
        let why = r.failed();
        assert!(why.contains("There is no network") && why.contains("starts again by itself when the network returns"), "{why}");
        assert_eq!(r.radio.notice().as_deref(), Some("Waiting for the network"));
        r.clock.grant(10);
        assert_eq!(r.script.asked("a"), 1, "no network: nothing is asked, however long it waits (under the slow retry time)");
        r.link.store(true, std::sync::atomic::Ordering::SeqCst);
        r.clock.release(1);
        r.playing("a");
        assert_eq!(r.script.asked("a"), 2);
        r.end();
    }

    #[test]
    fn a_parked_radio_tries_again_after_one_then_five_minutes_on_the_injected_clock() {
        let r = rig(vec![("a", vec![Conn::Play(8), Conn::Refuse, Conn::Hold(8)]), ("b", vec![])], 0, true);
        r.start("a", &["a", "b"], "tee /dev/null");
        let why = r.failed();
        assert!(why.contains("Every one of the 2 stations") && why.contains("tries again by itself"), "{why}");
        assert_eq!(r.radio.notice().as_deref(), Some("Trying again in 1 minute"));
        r.clock.grant(11); // 55 seconds
        assert_eq!((r.script.asked("a"), r.script.asked("b")), (1, 1));
        r.clock.grant(1); // 60 seconds: the first slow retry; a and b fail again
        wait_until(BUDGET, || r.script.asked("b") == 2 && r.radio.notice().as_deref() == Some("Trying again in 5 minutes"));
        assert_eq!(r.script.asked("a"), 2);
        r.clock.grant(59); // 295 seconds
        assert_eq!(r.script.asked("a"), 2);
        r.clock.release(1); // 300 seconds: the second slow retry; a plays again
        r.playing("a");
        assert_eq!(r.script.asked("a"), 3);
        r.end();
    }

    #[test]
    fn stopping_a_parked_radio_ends_the_waiting_and_nothing_is_asked_afterwards() {
        let r = rig(vec![("a", vec![Conn::Play(8)])], 0, true);
        r.start("a", &["a"], "tee /dev/null");
        r.failed();
        r.clock.grant(3);
        let before = r.script.asked("a");
        r.radio.stop();
        r.clock.wake();
        assert_eq!(r.radio.state(), RadioState::Idle);
        assert_eq!(r.script.asked("a"), before);
        r.end();
    }
}
