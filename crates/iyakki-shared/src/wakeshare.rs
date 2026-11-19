//! Who holds the microphone (F-VOICE-60, EXPERIMENTAL like the wake phrase it serves). The wake listener reads this machine's microphone; a
//! page in the browser (`getUserMedia`) and the Voice tile's recorder open it too, and a sound card opened twice either fails or gives one of
//! them silence. The rule is one reader at a time, and the wake listener is the one that gives way:
//!
//! * a page that captures the microphone (the full engine's `CaptureState` microphone bit) pauses the listener until the page lets go;
//! * the Voice tile recording pauses it for the length of the recording;
//! * when both are gone the listener opens the microphone again by itself.
//!
//! [`YieldingMic`] wraps the listener's own microphone and does exactly that on every `drain`. [`why_paused`] is the one line Settings and the
//! chat say about it ("Wake phrase paused: a page is using the microphone"). Nothing here listens, records or keeps sound.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::wakelisten::Mic;

static PAGE_MIC: AtomicBool = AtomicBool::new(false);
static TILE_RECORDING: AtomicBool = AtomicBool::new(false);

/// A page in the browser captures the microphone (true) or let go (false).
pub fn set_page_mic(on: bool) {
    PAGE_MIC.store(on, Ordering::SeqCst);
}

/// The Voice tile records (true) or stopped (false).
pub fn set_tile_recording(on: bool) {
    TILE_RECORDING.store(on, Ordering::SeqCst);
}

/// Why the wake listener is not reading the microphone now, in words for the person; `None` when it may.
pub fn why_paused() -> Option<&'static str> {
    if TILE_RECORDING.load(Ordering::SeqCst) {
        Some("Wake phrase paused: the Voice tile is recording.")
    } else if PAGE_MIC.load(Ordering::SeqCst) {
        Some("Wake phrase paused: a page is using the microphone.")
    } else {
        None
    }
}

/// The listener's microphone, giving way to the page and the Voice tile.
pub struct YieldingMic {
    inner: Box<dyn Mic>,
    wanted: bool,
    paused: bool,
    /// How many times the pause began (a test, and the debug line, read it).
    pub pauses: u32,
}

impl YieldingMic {
    pub fn new(inner: Box<dyn Mic>) -> YieldingMic {
        YieldingMic { inner, wanted: false, paused: false, pauses: 0 }
    }

    /// Look at who holds the microphone now and open or close the real one to match.
    fn settle(&mut self) {
        if !self.wanted {
            return;
        }
        let must_wait = why_paused().is_some();
        if must_wait && !self.paused {
            self.inner.close();
            self.paused = true;
            self.pauses += 1;
        } else if !must_wait && (self.paused || !self.inner.is_open()) {
            // let go and not yet back: try again on the next call when the device is not ready
            if self.inner.open().is_ok() {
                self.paused = false;
            }
        }
    }
}

impl Mic for YieldingMic {
    fn open(&mut self) -> Result<(), String> {
        self.wanted = true;
        if why_paused().is_some() {
            // asked to listen while somebody else has the microphone: it starts when they let go
            self.paused = true;
            self.pauses += 1;
            return Ok(());
        }
        self.paused = false;
        self.inner.open()
    }
    fn is_open(&self) -> bool {
        self.wanted
    }
    fn drain(&mut self) -> Vec<i16> {
        self.settle();
        if self.paused {
            return Vec::new();
        }
        self.inner.drain()
    }
    fn alive(&mut self) -> bool {
        if self.paused {
            return self.wanted;
        }
        self.inner.alive()
    }
    fn close(&mut self) {
        self.wanted = false;
        self.paused = false;
        self.inner.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The holders are process-wide: tests that set them take this lock.
    static LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<(bool, u32, u32)>>);
    impl Mic for Fake {
        fn open(&mut self) -> Result<(), String> {
            let mut g = self.0.lock().unwrap();
            g.0 = true;
            g.1 += 1;
            Ok(())
        }
        fn is_open(&self) -> bool {
            self.0.lock().unwrap().0
        }
        fn drain(&mut self) -> Vec<i16> {
            let g = self.0.lock().unwrap();
            if g.0 { vec![1, 2, 3] } else { Vec::new() }
        }
        fn alive(&mut self) -> bool {
            self.0.lock().unwrap().0
        }
        fn close(&mut self) {
            let mut g = self.0.lock().unwrap();
            g.0 = false;
            g.2 += 1;
        }
    }

    #[test]
    fn the_listener_lets_go_while_a_page_or_the_voice_tile_has_the_microphone_and_takes_it_back() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_page_mic(false);
        set_tile_recording(false);
        let hw = Fake::default();
        let mut m = YieldingMic::new(Box::new(hw.clone()));
        m.open().unwrap();
        assert_eq!(m.drain(), vec![1, 2, 3]);
        assert!(why_paused().is_none());
        // a page captures the microphone: the device is closed, nothing is read, the listener says it is still wanted
        set_page_mic(true);
        assert!(m.drain().is_empty());
        assert!(!hw.0.lock().unwrap().0, "the real microphone was let go");
        assert!(m.is_open() && m.alive(), "the listener is not told it died");
        assert_eq!(why_paused(), Some("Wake phrase paused: a page is using the microphone."));
        // the Voice tile records as well: still paused, the words name the tile first
        set_tile_recording(true);
        assert!(m.drain().is_empty());
        assert!(why_paused().unwrap().contains("Voice tile"));
        // both let go: it opens again by itself
        set_page_mic(false);
        set_tile_recording(false);
        assert_eq!(m.drain(), vec![1, 2, 3]);
        assert_eq!(m.pauses, 1, "one pause, not one per call");
        m.close();
        assert!(!m.is_open() && !hw.0.lock().unwrap().0);
    }

    #[test]
    fn asking_to_listen_while_a_page_has_the_microphone_waits_and_never_opens_the_device() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_page_mic(true);
        let hw = Fake::default();
        let mut m = YieldingMic::new(Box::new(hw.clone()));
        m.open().unwrap();
        assert_eq!(hw.0.lock().unwrap().1, 0, "the device was not opened");
        assert!(m.drain().is_empty());
        set_page_mic(false);
        assert_eq!(m.drain(), vec![1, 2, 3]);
        assert_eq!(hw.0.lock().unwrap().1, 1);
        set_page_mic(false);
    }
}
