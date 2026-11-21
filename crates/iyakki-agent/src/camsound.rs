//! Camera sound started by a chat tool (`camera_listen`, F-CAM-20). The CAMERAS tile feeds the sound from its own draw loop (a call of
//! `CameraSource::listen` on every drawn frame); a chat start has no draw loop, so [`Pump`] is its own thread that makes the same call a
//! few times a second, for a bounded time, and lets the speaker go when it ends.
//!
//! The loop is [`pump`]: its waiting is a closure (`wait` returns false to stop), so tests drive it with no sleeping and no clock. The real
//! [`Pump::start`] waits on a condition variable (100 ms, or woken at once by a stop).

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use iyakki_tile_api::cameras::{CameraSource, Listen};

/// How often the pump asks the hub for the newest sound (the hub keeps two seconds).
pub const EVERY: Duration = Duration::from_millis(100);
/// Longest and shortest bounded session a tool may ask for, seconds.
pub const MAX_SECS: u32 = 600;
pub const MIN_SECS: u32 = 5;
/// Rounds of "no sound yet" after which a camera without a microphone is given up on (30 seconds).
pub const GIVE_UP_WAITING: u32 = 300;

/// Why a pump ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The time asked for ran out, or it was stopped.
    Done,
    /// The camera or the speaker cannot do it (the sentence).
    Unavailable(String),
    /// The camera never sent any sound (no microphone, or the stream is not a sound one).
    NoSound,
}

/// Run the loop on this thread: ask for the sound, wait, again; at most `secs * 10` rounds. The speaker is let go at the end.
pub fn pump(src: &dyn CameraSource, id: &str, secs: u32, mut wait: impl FnMut() -> bool) -> Ended {
    let rounds = u64::from(secs.clamp(MIN_SECS, MAX_SECS)) * 10;
    let mut silent = 0u32;
    let mut ended = Ended::Done;
    for _ in 0..rounds {
        match src.listen(id, true) {
            Listen::Playing => silent = 0,
            Listen::Waiting => {
                silent += 1;
                if silent >= GIVE_UP_WAITING {
                    ended = Ended::NoSound;
                    break;
                }
            }
            Listen::Unavailable(e) => {
                ended = Ended::Unavailable(e);
                break;
            }
            Listen::Off => {}
        }
        if !wait() {
            break;
        }
    }
    src.listen(id, false);
    ended
}

struct Shared {
    stop: Mutex<bool>,
    cv: Condvar,
    ended: Mutex<Option<Ended>>,
}

/// One running camera sound: its thread, the camera and the way to stop it. Dropping it stops the sound and joins the thread.
pub struct Pump {
    pub camera: String,
    shared: Arc<Shared>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Pump {
    /// Start the sound of camera `id` (a name shown to the person is `camera`) for `secs` seconds on a thread of its own.
    pub fn start(src: Arc<dyn CameraSource + Send + Sync>, id: &str, camera: &str, secs: u32) -> Result<Pump, String> {
        let shared = Arc::new(Shared { stop: Mutex::new(false), cv: Condvar::new(), ended: Mutex::new(None) });
        let (sh, id2) = (shared.clone(), id.to_string());
        let join = std::thread::Builder::new()
            .name("iyakki-camera-sound".into())
            .spawn(move || {
                let end = pump(&*src, &id2, secs, || {
                    let stop = sh.stop.lock().unwrap_or_else(|e| e.into_inner());
                    let (stop, _) = sh.cv.wait_timeout_while(stop, EVERY, |s| !*s).unwrap_or_else(|e| e.into_inner());
                    !*stop
                });
                *sh.ended.lock().unwrap_or_else(|e| e.into_inner()) = Some(end);
            })
            .map_err(|e| format!("could not start the camera sound: {e}"))?;
        Ok(Pump { camera: camera.to_string(), shared, join: Some(join) })
    }

    /// The sentence of how it ended, when it has.
    pub fn ended(&self) -> Option<Ended> {
        self.shared.ended.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn running(&self) -> bool {
        self.ended().is_none()
    }

    /// Stop it now and wait for the thread (it checks every round, so this is quick).
    pub fn stop(&mut self) -> Ended {
        *self.shared.stop.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.shared.cv.notify_all();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.ended().unwrap_or(Ended::Done)
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        script: Mutex<Vec<Listen>>,
        on: AtomicUsize,
        off: AtomicUsize,
    }
    impl Fake {
        fn new(script: Vec<Listen>) -> Fake {
            Fake { script: Mutex::new(script), on: AtomicUsize::new(0), off: AtomicUsize::new(0) }
        }
    }
    impl CameraSource for Fake {
        fn listen(&self, _id: &str, on: bool) -> Listen {
            if !on {
                self.off.fetch_add(1, Ordering::SeqCst);
                return Listen::Off;
            }
            self.on.fetch_add(1, Ordering::SeqCst);
            let mut s = self.script.lock().unwrap();
            if s.len() > 1 { s.remove(0) } else { s.first().cloned().unwrap_or(Listen::Waiting) }
        }
        fn cameras(&self) -> Vec<iyakki_tile_api::cameras::CamInfo> {
            Vec::new()
        }
        fn view(&self, _: &str, _: bool) -> Option<iyakki_tile_api::cameras::Frame> {
            None
        }
        fn release_local(&self) {}
        fn release_local_except(&self, _: &[String]) {}
        fn snapshot(&self, _: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn save(&self, _: Option<&str>, _: &iyakki_tile_api::cameras::NewCam, _: Option<&str>) -> Result<String, String> {
            Err("no".into())
        }
        fn remove(&self, _: &str) -> Result<(), String> {
            Err("no".into())
        }
        fn people(&self) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn the_loop_asks_once_per_round_for_the_time_given_and_lets_the_speaker_go() {
        let f = Fake::new(vec![Listen::Playing]);
        let mut waits = 0;
        let end = pump(&f, "c1", 5, || {
            waits += 1;
            true
        });
        assert_eq!(end, Ended::Done);
        assert_eq!(f.on.load(Ordering::SeqCst), 50, "five seconds is fifty rounds of a tenth");
        assert_eq!(waits, 50);
        assert_eq!(f.off.load(Ordering::SeqCst), 1, "the speaker is let go once at the end");
    }

    #[test]
    fn a_stop_from_the_wait_ends_it_at_once_and_an_unavailable_camera_ends_it_with_the_sentence() {
        let f = Fake::new(vec![Listen::Playing]);
        let mut n = 0;
        assert_eq!(pump(&f, "c1", 600, || { n += 1; n < 3 }), Ended::Done);
        assert_eq!(f.on.load(Ordering::SeqCst), 3);
        assert_eq!(f.off.load(Ordering::SeqCst), 1);
        let f = Fake::new(vec![Listen::Waiting, Listen::Unavailable("Camera sound plays through IYAKKI's mixer; switch it on in Settings, Sound.".into())]);
        match pump(&f, "c1", 60, || true) {
            Ended::Unavailable(e) => assert!(e.contains("mixer")),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.off.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_camera_that_never_sends_sound_is_given_up_on_and_the_time_is_bounded() {
        let f = Fake::new(vec![Listen::Waiting]);
        assert_eq!(pump(&f, "c1", MAX_SECS, || true), Ended::NoSound);
        assert_eq!(f.on.load(Ordering::SeqCst) as u32, GIVE_UP_WAITING);
        // more than the longest time asked for is cut to it, less is raised to the shortest
        let f = Fake::new(vec![Listen::Playing]);
        pump(&f, "c1", 100_000, || true);
        assert_eq!(f.on.load(Ordering::SeqCst) as u32, MAX_SECS * 10);
        let f = Fake::new(vec![Listen::Playing]);
        pump(&f, "c1", 0, || true);
        assert_eq!(f.on.load(Ordering::SeqCst) as u32, MIN_SECS * 10);
    }

    #[test]
    fn the_thread_pump_runs_until_stopped_and_stopping_lets_the_speaker_go() {
        let f = Arc::new(Fake::new(vec![Listen::Playing]));
        let mut p = Pump::start(f.clone(), "c1", "Front door", MAX_SECS).unwrap();
        assert_eq!(p.camera, "Front door");
        assert_eq!(p.stop(), Ended::Done);
        assert!(!p.running());
        assert_eq!(f.off.load(Ordering::SeqCst), 1);
        // dropping a pump that was never stopped by hand also stops the thread
        let f = Arc::new(Fake::new(vec![Listen::Playing]));
        drop(Pump::start(f.clone(), "c1", "x", MAX_SECS).unwrap());
        assert_eq!(f.off.load(Ordering::SeqCst), 1);
    }
}
