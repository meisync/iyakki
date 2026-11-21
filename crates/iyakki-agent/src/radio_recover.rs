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
            return Step::GiveUp(format!("{why}.{tries} There is no other station in the list."));
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
        Step::GiveUp(format!("{why}.{tries} Every one of the {} stations in the list failed, so the radio stopped; press play to try again.", self.list.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Step::GiveUp(t) => assert!(t.contains("Every one of the 3 stations"), "{t}"),
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
                Step::GiveUp(_) => break,
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
        assert!(matches!(one.on_end(&st("a"), Some(1), "x"), Step::GiveUp(t) if t.contains("no other station")));
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

    /// Time that only moves when the player waits: every wait is recorded and nothing sleeps.
    struct Manual {
        clock: ManualClock,
        waits: Mutex<Vec<Duration>>,
    }

    impl RecoverClock for Manual {
        fn now_ms(&self) -> u64 {
            self.clock.now_ms()
        }
        fn wait(&self, d: Duration, _keep_going: &dyn Fn() -> bool) {
            self.waits.lock().unwrap().push(d);
            self.clock.advance(d);
        }
    }

    struct Rig {
        radio: RadioPlayer,
        script: Arc<Script>,
        clock: Arc<Manual>,
        audit: Arc<Mutex<Vec<String>>>,
        _route: Box<dyn std::any::Any>,
    }

    fn rig(script: Vec<(&str, Vec<Conn>)>, retries: u32, auto: bool) -> Rig {
        let route = crate::mixer::pin_command_route();
        let script = Script::new(script);
        let clock = Arc::new(Manual { clock: ManualClock::new(1_000), waits: Mutex::new(Vec::new()) });
        let radio = RadioPlayer::default();
        radio.set_streamer(script.clone());
        radio.set_recover_clock(clock.clone());
        radio.set_recover_source(Arc::new(move || (retries, auto)));
        radio.set_retry_wait(Duration::from_secs(3));
        Rig { radio, script, clock, audit: Arc::new(Mutex::new(Vec::new())), _route: Box::new(route) }
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
}
