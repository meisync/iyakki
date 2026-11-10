//! The wake phrase listener: connects the in-house spotter (`crates/iyakki-wake`) to the microphone, strictly opt-in
//! (`docs/development/WAKE-PHRASE.md`, `MY-IYAKKI-PLAN.md` step 4).
//!
//! The microphone is opened through the one shared path of the Voice tile (`voiceassist_ports::open_capture`: the saved microphone
//! choice, the record command, the named-format PCM of `micpcm`). This file opens no device, no file and no network connection (a test
//! reads the source). It reads the capture in small frames (`Capture::drain`), converts them to floats, hands them to the spotter and
//! forgets them: the only sound that exists is the spotter's three-second ring in memory.
//!
//! Rules, each a test with a fake microphone and a fake spotter:
//!
//! * Nothing is opened unless the person opted in, My IYAKKI is on, it is not muted and a model file exists; muting, turning off or
//!   removing the model closes the microphone at once and wipes the spotter.
//! * While the Voice tile (the assistant's own session) or a locked screen needs the microphone, the listener lets go of it.
//! * [`Listener::indicator`] is true exactly while the microphone is open to the spotter; the shell draws the red sign from it.
//! * A detection closes the microphone first and then reports; the listener stays quiet until the session has had the microphone
//!   (it was paused once) or 30 seconds passed, so it never races the session for the device.
//! * A microphone that fails to open is retried every few seconds; the reason is kept as a plain-words note.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_wake::spotter::{Settings, Spotter};
use iyakki_wake::weights::{self, Header};
use iyakki_wake::{WakeEvent, WakeSpotter};

use crate::services::Services;
use crate::voicedesk::{Capture, Recorders};

/// The model file (made by `iyakki-wake-train`) under the data folder.
pub use crate::voiceassisttools::WAKE_WEIGHTS_FILE as MODEL_FILE;

/// The samples handed to the spotter at once (100 ms at 16 kHz).
pub const FRAME: usize = 1600;
/// After a failure to open the microphone, wait this long before the next try.
pub const RETRY_MS: u64 = 5000;
/// After a detection the listener waits for the session to take the microphone, at most this long.
pub const HOLD_MS: u64 = 30_000;
/// Words for the sensitivity setting.
pub const SENSITIVITY: [&str; 3] = ["low", "normal", "high"];

/// The threshold for a sensitivity (0 low, 1 normal, 2 high) from the model's suggested one: low asks for more certainty, high for less.
pub fn threshold_for(base: f32, sens: u8) -> f32 {
    let d = match sens {
        0 => 0.07,
        1 => 0.0,
        _ => -0.10,
    };
    (base + d).clamp(0.50, 0.99)
}

/// The sensitivity from its word (`low`, `normal`, `high`) or digit.
pub fn parse_sensitivity(s: &str) -> Option<u8> {
    let t = s.trim().to_lowercase();
    SENSITIVITY.iter().position(|w| *w == t).map(|i| i as u8).or_else(|| t.parse::<u8>().ok().filter(|n| *n <= 2))
}

/// The model file in plain words.
#[derive(Clone, Debug)]
pub enum Model {
    Missing,
    Bad(String),
    Found { net: iyakki_wake::model::Net, header: Header },
}

/// Read the model file under `dir`. A file that is damaged or from another version is `Bad`, never guessed at.
pub fn load_model(dir: &Path) -> Model {
    match std::fs::read(dir.join(MODEL_FILE)) {
        Err(_) => Model::Missing,
        Ok(b) => match weights::load_bytes(&b) {
            Ok((net, header)) => Model::Found { net, header },
            Err(e) => Model::Bad(e),
        },
    }
}

/// Why the switch stays off when there is no usable model (the one sentence Settings and the chat tool both use).
pub fn no_model_text(m: &Model) -> Option<String> {
    match m {
        Model::Found { .. } => None,
        Model::Missing => Some("There is no wake phrase model yet, so the wake phrase stays off. A model has to be recorded and trained first (docs/development/WAKE-PHRASE.md); until then use push to talk.".into()),
        Model::Bad(e) => Some(format!("The wake phrase model file cannot be used ({e}), so the wake phrase stays off.")),
    }
}

/// May the wake phrase be switched on? My IYAKKI must be on and a usable model file must exist; the error is the plain-words reason
/// (Settings and the chat tool show it, and the switch stays off).
pub fn can_enable(svc: &Services) -> Result<(), String> {
    if !svc.voice_settings().my_on {
        return Err("turn My IYAKKI on first; the wake phrase is an extra on top of it".into());
    }
    match no_model_text(&load_model(svc.dir())) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Save the opt-in (the one writer for the Settings switch, the Assistant tab and the sign's click). Turning it on is checked by
/// [`can_enable`]; turning it off always works. Returns whether the saved value changed.
pub fn set_opt_in(svc: &Services, on: bool) -> Result<bool, String> {
    if on {
        can_enable(svc)?;
    }
    let was = svc.voice_settings().my_wake;
    svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_wake = on).map_err(|e| e.to_string())?;
    Ok(was != on)
}

/// Save the sensitivity (0 low, 1 normal, 2 high).
pub fn set_sensitivity(svc: &Services, sens: u8) -> Result<(), String> {
    if sens > 2 {
        return Err("sensitivity must be low, normal or high".into());
    }
    svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_wake_sens = sens).map_err(|e| e.to_string())
}

static SESSION_REQUEST: AtomicBool = AtomicBool::new(false);

/// A wake phrase was heard: ask for the My IYAKKI voice session (the Voice tile that opens takes it with [`take_session_request`]).
pub fn request_session() {
    SESSION_REQUEST.store(true, Ordering::SeqCst);
}

/// True once per [`request_session`].
pub fn take_session_request() -> bool {
    SESSION_REQUEST.swap(false, Ordering::SeqCst)
}

/// The live inputs of the listener from the saved settings: `(want, sensitivity)`.
pub fn live_from(svc: &Services) -> (bool, u8) {
    let v = svc.voice_settings();
    (v.my_wake && v.my_on && !v.my_muted, v.my_wake_sens.min(2))
}

// ------------------------------------------------------------------------------------------------ the microphone port

/// The microphone as the listener sees it: open, read what arrived since the last read, close.
pub trait Mic: Send {
    fn open(&mut self) -> Result<(), String>;
    fn is_open(&self) -> bool;
    /// 16 kHz mono samples since the last call (nothing is kept here).
    fn drain(&mut self) -> Vec<i16>;
    /// The capture is still running.
    fn alive(&mut self) -> bool;
    /// Close the microphone and drop anything unread.
    fn close(&mut self);
}

/// The real microphone: the Voice tile's recorders and open path.
pub struct CaptureMic {
    recorders: Arc<dyn Recorders>,
    svc: Arc<Services>,
    cap: Option<Box<dyn Capture>>,
}

impl CaptureMic {
    pub fn new(recorders: Arc<dyn Recorders>, svc: Arc<Services>) -> CaptureMic {
        CaptureMic { recorders, svc, cap: None }
    }
}

impl Mic for CaptureMic {
    fn open(&mut self) -> Result<(), String> {
        if self.cap.is_none() {
            self.cap = Some(crate::voiceassist_ports::open_capture(self.recorders.as_ref(), &self.svc, 30)?);
        }
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.cap.is_some()
    }
    fn drain(&mut self) -> Vec<i16> {
        self.cap.as_mut().map(|c| c.drain()).unwrap_or_default()
    }
    fn alive(&mut self) -> bool {
        self.cap.as_mut().is_some_and(|c| c.is_alive())
    }
    fn close(&mut self) {
        if let Some(c) = self.cap.take() {
            let _ = c.stop(); // the samples are dropped here
        }
    }
}

impl Drop for CaptureMic {
    fn drop(&mut self) {
        self.close();
    }
}

// ------------------------------------------------------------------------------------------------ the listener

/// What the listener is told on every step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The person opted in, My IYAKKI is on and it is not muted.
    pub want: bool,
    /// The assistant's own session or the lock screen needs the microphone (or must not hear): let go of it.
    pub paused: bool,
    /// 0 low, 1 normal, 2 high.
    pub sens: u8,
}

/// Builds a spotter for a threshold (the real one wraps the model; tests pass a fake).
pub type MakeSpotter = Box<dyn Fn(f32) -> Box<dyn WakeSpotter + Send> + Send>;

pub struct Listener {
    mic: Box<dyn Mic>,
    make: MakeSpotter,
    base: f32,
    spotter: Option<Box<dyn WakeSpotter + Send>>,
    sens: u8,
    retry_at: u64,
    held_since: Option<u64>,
    saw_pause: bool,
    note: String,
}

impl Listener {
    pub fn new(mic: Box<dyn Mic>, make: MakeSpotter, base_threshold: f32) -> Listener {
        Listener { mic, make, base: base_threshold, spotter: None, sens: 1, retry_at: 0, held_since: None, saw_pause: false, note: String::new() }
    }

    /// The real listener for a model.
    pub fn for_model(mic: Box<dyn Mic>, net: iyakki_wake::model::Net, header: &Header) -> Listener {
        let make: MakeSpotter = Box::new(move |thr| Box::new(Spotter::new(net.clone(), Settings { threshold: thr, ..Settings::default() })));
        Listener::new(mic, make, header.threshold)
    }

    /// True exactly while the microphone is open to the spotter.
    pub fn indicator(&self) -> bool {
        self.mic.is_open() && self.spotter.as_ref().is_some_and(|s| s.indicator())
    }

    /// The plain-words reason the microphone is not open when it should be (empty otherwise).
    pub fn note(&self) -> &str {
        &self.note
    }

    fn stop(&mut self) {
        if let Some(s) = self.spotter.as_mut() {
            s.disarm(); // wipes the ring
        }
        self.spotter = None;
        self.mic.close();
    }

    /// One step at `now_ms`: follow the inputs, read what the microphone delivered, feed the spotter. A detection closes the
    /// microphone and is returned.
    pub fn step(&mut self, inp: Inputs, now_ms: u64) -> Option<WakeEvent> {
        if inp.paused {
            self.saw_pause = true;
        }
        if let Some(t) = self.held_since {
            if self.saw_pause || now_ms.saturating_sub(t) >= HOLD_MS {
                self.held_since = None;
            }
        }
        if !inp.want || inp.paused || self.held_since.is_some() {
            if self.mic.is_open() || self.spotter.is_some() {
                self.stop();
            }
            if !inp.want {
                self.note.clear();
            }
            return None;
        }
        if self.spotter.is_some() && inp.sens != self.sens {
            self.stop(); // a new threshold starts from nothing
        }
        if !self.mic.is_open() {
            if now_ms < self.retry_at {
                return None;
            }
            if let Err(e) = self.mic.open() {
                self.note = e;
                self.retry_at = now_ms + RETRY_MS;
                return None;
            }
            self.note.clear();
        }
        if self.spotter.is_none() {
            self.sens = inp.sens;
            let mut s = (self.make)(threshold_for(self.base, inp.sens));
            s.arm();
            s.set_muted(false);
            self.spotter = Some(s);
        }
        let samples = self.mic.drain();
        let mut found = None;
        if let Some(s) = self.spotter.as_mut() {
            for frame in samples.chunks(FRAME) {
                let f: Vec<f32> = frame.iter().map(|v| *v as f32 / 32768.0).collect();
                if let Some(e) = s.feed(&f).into_iter().next() {
                    found = Some(e);
                    break;
                }
            }
        }
        if found.is_some() {
            self.stop();
            self.held_since = Some(now_ms);
            self.saw_pause = false;
            return found;
        }
        if !self.mic.alive() {
            self.note = "The microphone stopped (another program may be using it); trying again shortly.".into();
            self.stop();
            self.retry_at = now_ms + RETRY_MS;
        }
        None
    }
}

// ------------------------------------------------------------------------------------------------ the thread

/// A listener on its own thread. Dropping the handle stops it and closes the microphone.
pub struct WakeHandle {
    indicator: Arc<AtomicBool>,
    woke: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    note: Arc<Mutex<String>>,
    quit: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl WakeHandle {
    /// The microphone is open to the spotter right now.
    pub fn indicator(&self) -> bool {
        self.indicator.load(Ordering::SeqCst)
    }
    /// A wake phrase was heard since the last call.
    pub fn take_wake(&self) -> bool {
        self.woke.swap(false, Ordering::SeqCst)
    }
    /// The shell says whether something else needs the microphone (the Voice tile) or must not hear (the lock screen).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }
    pub fn note(&self) -> String {
        self.note.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for WakeHandle {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.indicator.store(false, Ordering::SeqCst);
    }
}

/// Run `listener` on a thread, one step every `tick_ms`. `live` gives `(want, sensitivity)` from the settings and is read each step, so
/// a change from Settings or the chat tool acts at once.
pub fn spawn(mut listener: Listener, live: Box<dyn Fn() -> (bool, u8) + Send>, tick_ms: u64) -> WakeHandle {
    let (indicator, woke, paused, quit) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let note = Arc::new(Mutex::new(String::new()));
    let (i2, w2, p2, q2, n2) = (indicator.clone(), woke.clone(), paused.clone(), quit.clone(), note.clone());
    let join = std::thread::spawn(move || {
        let t0 = Instant::now();
        while !q2.load(Ordering::SeqCst) {
            let (want, sens) = live();
            let inp = Inputs { want, paused: p2.load(Ordering::SeqCst), sens };
            let ev = listener.step(inp, t0.elapsed().as_millis() as u64);
            i2.store(listener.indicator(), Ordering::SeqCst);
            if ev.is_some() {
                w2.store(true, Ordering::SeqCst);
            }
            {
                let mut g = n2.lock().unwrap_or_else(|e| e.into_inner());
                if g.as_str() != listener.note() {
                    *g = listener.note().to_string();
                }
            }
            std::thread::sleep(Duration::from_millis(tick_ms));
        }
        listener.stop();
        i2.store(false, Ordering::SeqCst);
    });
    WakeHandle { indicator, woke, paused, note, quit, join: Some(join) }
}

/// Stand-ins for the tests of other crates (a fake spotter that wakes on a loud frame, a model file): not used at run time.
#[doc(hidden)]
pub mod testing {
    use super::*;
    use iyakki_wake::{FakeSpotter, WakeState};

    /// Follows the real arm/mute/indicator rules and "hears" the phrase in any frame with a sample above 0.9.
    pub struct LoudSpotter(pub FakeSpotter);

    impl WakeSpotter for LoudSpotter {
        fn state(&self) -> WakeState {
            self.0.state()
        }
        fn arm(&mut self) {
            self.0.arm()
        }
        fn disarm(&mut self) {
            self.0.disarm()
        }
        fn set_muted(&mut self, m: bool) {
            self.0.set_muted(m)
        }
        fn feed(&mut self, s: &[f32]) -> Vec<WakeEvent> {
            if s.iter().any(|v| v.abs() > 0.9) {
                self.0.will_hear_wake(0.99);
            }
            self.0.feed(s)
        }
    }

    /// A listener with this microphone and the loud-frame spotter.
    pub fn listener(mic: Box<dyn Mic>) -> Listener {
        Listener::new(mic, Box::new(|_| Box::new(LoudSpotter(FakeSpotter::default()))), 0.85)
    }

    /// Write a (random, untrained) model file where the listener looks for it.
    pub fn write_model(dir: &Path) {
        let h = Header { data_kind: weights::DATA_SYNTHETIC, threshold: 0.8, seed: 1, epochs: 1, notes: String::new() };
        let p = dir.join(MODEL_FILE);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, weights::save_bytes(&iyakki_wake::model::Net::random(1), &h)).unwrap();
    }
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_wake::FakeSpotter;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct MicLog {
        opens: u32,
        closes: u32,
        fail: Option<String>,
        open: bool,
        dies: bool,
        frames: VecDeque<Vec<i16>>,
        read: usize,
    }

    #[derive(Clone, Default)]
    struct FakeMic(Arc<Mutex<MicLog>>);

    impl Mic for FakeMic {
        fn open(&mut self) -> Result<(), String> {
            let mut l = self.0.lock().unwrap();
            if let Some(e) = l.fail.clone() {
                return Err(e);
            }
            l.opens += 1;
            l.open = true;
            Ok(())
        }
        fn is_open(&self) -> bool {
            self.0.lock().unwrap().open
        }
        fn drain(&mut self) -> Vec<i16> {
            let mut l = self.0.lock().unwrap();
            let f = l.frames.pop_front().unwrap_or_default();
            l.read += f.len();
            f
        }
        fn alive(&mut self) -> bool {
            !self.0.lock().unwrap().dies
        }
        fn close(&mut self) {
            let mut l = self.0.lock().unwrap();
            if l.open {
                l.closes += 1;
            }
            l.open = false;
            l.frames.clear();
        }
    }

    /// A spotter that wakes when a frame carries a loud sample (a fake model), following the real arm/mute/indicator rules.
    struct LoudSpotter {
        inner: FakeSpotter,
        thr: f32,
    }
    impl WakeSpotter for LoudSpotter {
        fn state(&self) -> iyakki_wake::WakeState {
            self.inner.state()
        }
        fn arm(&mut self) {
            self.inner.arm()
        }
        fn disarm(&mut self) {
            self.inner.disarm()
        }
        fn set_muted(&mut self, m: bool) {
            self.inner.set_muted(m)
        }
        fn feed(&mut self, s: &[f32]) -> Vec<WakeEvent> {
            if s.iter().any(|v| v.abs() > 0.9) {
                self.inner.will_hear_wake(self.thr);
            }
            self.inner.feed(s)
        }
    }

    fn listener(mic: &FakeMic) -> (Listener, Arc<Mutex<Vec<f32>>>) {
        let thresholds = Arc::new(Mutex::new(Vec::new()));
        let t2 = thresholds.clone();
        let make: MakeSpotter = Box::new(move |thr| {
            t2.lock().unwrap().push(thr);
            Box::new(LoudSpotter { inner: FakeSpotter::default(), thr })
        });
        (Listener::new(Box::new(mic.clone()), make, 0.85), thresholds)
    }

    const ON: Inputs = Inputs { want: true, paused: false, sens: 1 };
    fn loud() -> Vec<i16> {
        vec![32000; 1600]
    }
    fn quiet() -> Vec<i16> {
        vec![100; 1600]
    }

    #[test]
    fn it_is_off_until_wanted_and_nothing_is_opened_or_read() {
        let mic = FakeMic::default();
        let (mut l, _) = listener(&mic);
        mic.0.lock().unwrap().frames.push_back(loud());
        for t in 0..5 {
            assert!(l.step(Inputs { want: false, ..ON }, t * 100).is_none());
        }
        let (opens, read) = {
            let m = mic.0.lock().unwrap();
            (m.opens, m.read)
        };
        assert_eq!((opens, read), (0, 0), "off means the microphone is never opened and no sample is read");
        assert!(!l.indicator());
    }

    #[test]
    fn the_indicator_is_true_exactly_while_the_microphone_is_open_to_the_spotter() {
        let mic = FakeMic::default();
        let (mut l, _) = listener(&mic);
        assert!(!l.indicator());
        l.step(ON, 0);
        assert!(l.indicator() && mic.is_open());
        l.step(Inputs { want: false, ..ON }, 100);
        assert!(!l.indicator() && !mic.is_open(), "muting or opting out closes the microphone at once");
        l.step(ON, 200);
        assert!(l.indicator());
        l.step(Inputs { paused: true, ..ON }, 300);
        assert!(!l.indicator() && !mic.is_open(), "the session or the lock screen takes over");
    }

    #[test]
    fn a_wake_closes_the_microphone_reports_once_and_waits_for_the_session() {
        let mic = FakeMic::default();
        let (mut l, _) = listener(&mic);
        l.step(ON, 0);
        mic.0.lock().unwrap().frames.push_back(quiet());
        assert!(l.step(ON, 100).is_none());
        mic.0.lock().unwrap().frames.push_back(loud());
        assert!(l.step(ON, 200).is_some());
        assert!(!mic.is_open() && !l.indicator(), "the microphone is closed before the session opens it");
        for t in 3..10 {
            assert!(l.step(ON, t * 100).is_none());
            assert!(!mic.is_open(), "held: no race with the session for the device");
        }
        l.step(Inputs { paused: true, ..ON }, 1000);
        l.step(ON, 1100);
        assert!(mic.is_open(), "after the session had the microphone the listener returns");
        assert_eq!(mic.0.lock().unwrap().opens, 2);
    }

    #[test]
    fn a_hold_ends_by_itself_when_no_session_comes() {
        let mic = FakeMic::default();
        let (mut l, _) = listener(&mic);
        l.step(ON, 0);
        mic.0.lock().unwrap().frames.push_back(loud());
        assert!(l.step(ON, 100).is_some());
        l.step(ON, 100 + HOLD_MS - 1);
        assert!(!mic.is_open());
        l.step(ON, 100 + HOLD_MS);
        assert!(mic.is_open());
    }

    #[test]
    fn sensitivity_moves_the_threshold_and_restarts_from_nothing() {
        let mic = FakeMic::default();
        let (mut l, thr) = listener(&mic);
        l.step(ON, 0);
        l.step(Inputs { sens: 2, ..ON }, 100);
        l.step(Inputs { sens: 0, ..ON }, 200);
        let t = thr.lock().unwrap().clone();
        assert_eq!(t.len(), 3);
        assert!(t[1] < t[0] && t[0] < t[2], "high needs less certainty than normal, low more: {t:?}");
        assert_eq!(threshold_for(0.85, 1), 0.85);
        assert_eq!(threshold_for(0.95, 0), 0.99, "clamped");
        assert_eq!(threshold_for(0.55, 2), 0.50, "clamped");
        assert_eq!((parse_sensitivity("High"), parse_sensitivity("1"), parse_sensitivity("9"), parse_sensitivity("x")), (Some(2), Some(1), None, None));
    }

    #[test]
    fn a_microphone_that_will_not_open_is_retried_with_a_plain_note() {
        let mic = FakeMic::default();
        mic.0.lock().unwrap().fail = Some("no microphone found".into());
        let (mut l, _) = listener(&mic);
        l.step(ON, 0);
        assert!(!l.indicator() && l.note().contains("no microphone"));
        mic.0.lock().unwrap().fail = None;
        l.step(ON, 1000);
        assert!(!mic.is_open(), "not before the retry time");
        l.step(ON, RETRY_MS);
        assert!(mic.is_open() && l.note().is_empty());
    }

    #[test]
    fn a_capture_that_dies_closes_everything_and_says_so() {
        let mic = FakeMic::default();
        let (mut l, _) = listener(&mic);
        l.step(ON, 0);
        mic.0.lock().unwrap().dies = true;
        l.step(ON, 100);
        assert!(!l.indicator() && !mic.is_open() && l.note().contains("stopped"));
    }

    #[test]
    fn the_thread_follows_the_settings_and_closes_the_microphone_when_dropped() {
        let mic = FakeMic::default();
        let (l, _) = listener(&mic);
        let want = Arc::new(AtomicBool::new(false));
        let w2 = want.clone();
        let h = spawn(l, Box::new(move || (w2.load(Ordering::SeqCst), 1)), 5);
        std::thread::sleep(Duration::from_millis(60));
        assert!(!h.indicator() && mic.0.lock().unwrap().opens == 0);
        want.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        while !h.indicator() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.indicator());
        mic.0.lock().unwrap().frames.push_back(loud());
        let t0 = Instant::now();
        while !h.take_wake() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!h.indicator());
        h.set_paused(true);
        drop(h);
        assert!(!mic.is_open());
    }

    #[test]
    fn dropping_a_listening_handle_closes_the_microphone() {
        let mic = FakeMic::default();
        let (l, _) = listener(&mic);
        let h = spawn(l, Box::new(|| (true, 1)), 5);
        let t0 = Instant::now();
        while !h.indicator() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(mic.is_open());
        drop(h);
        assert!(!mic.is_open());
    }

    #[test]
    fn the_model_file_states_are_told_in_plain_words() {
        let d = std::env::temp_dir().join(format!("wl-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(no_model_text(&load_model(&d)).unwrap().contains("no wake phrase model yet"));
        std::fs::create_dir_all(d.join("voice")).unwrap();
        std::fs::write(d.join(MODEL_FILE), b"garbage").unwrap();
        let m = load_model(&d);
        assert!(matches!(m, Model::Bad(_)) && no_model_text(&m).unwrap().contains("cannot be used"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_real_trained_file_loads() {
        let net = iyakki_wake::model::Net::random(1);
        let h = Header { data_kind: weights::DATA_SYNTHETIC, threshold: 0.8, seed: 1, epochs: 1, notes: String::new() };
        let d = std::env::temp_dir().join(format!("wl-model-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("voice")).unwrap();
        std::fs::write(d.join(MODEL_FILE), weights::save_bytes(&net, &h)).unwrap();
        let m = load_model(&d);
        assert!(no_model_text(&m).is_none());
        let mic = FakeMic::default();
        if let Model::Found { net, header } = m {
            let mut l = Listener::for_model(Box::new(mic.clone()), net, &header);
            l.step(ON, 0);
            assert!(l.indicator(), "the real spotter with a real model file runs behind the fake microphone");
            mic.0.lock().unwrap().frames.push_back(quiet());
            assert!(l.step(ON, 100).is_none(), "silence does not wake it");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn this_file_touches_no_device_file_or_network() {
        let src = include_str!("wakelisten.rs");
        let code = &src[..src.find("/// Stand-ins for the tests of other crates").unwrap()];
        for bad in ["/proc", "/dev/", "TcpStream", "UdpSocket", "File::create", "fs::write", "Command::new", "hw:", "plughw"] {
            assert!(!code.contains(bad), "wakelisten.rs must not contain {bad}");
        }
    }
}
