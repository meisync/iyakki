//! System sounds in the shell: when each cue is started, and what plays it (the real output chain, or a recording fake in tests).
//!
//! `start` returns at once; only sign-out waits, and for at most `LOGOUT_WAIT`, so the logout sound is over before the screen
//! changes without ever holding sign-out back.

use iyakki_core_api::soundtypes::{Cue, CueHandle, CueOut, Outcome};
use iyakki_core_api::Core;

/// What the shell remembers between heartbeats about the sounds.
#[derive(Default)]
pub struct CueState {
    /// The bell's count has been read once since signing in (that first count is not "new").
    pub badge_seen: bool,
    /// A batch of notifications the shell made itself (finished jobs) is being counted: they get their own sound.
    pub quiet: bool,
}

/// Start a cue now (asynchronously) on the real output. Cloud mode without sound hardware plays nothing.
pub fn start(core: &dyn Core, cloud: &iyakki_cloudmode::CloudMode, cue: Cue) -> CueHandle {
    if cloud.hides_page("sound") {
        return CueHandle::finished(Outcome::Off("cloud mode: no sound output".into()));
    }
    #[cfg(test)]
    {
        return core.cues().play(cue, fake::out());
    }
    #[cfg(not(test))]
    {
        core.cues().play(cue, core.cues().out())
    }
}

#[cfg(test)]
pub mod fake {
    //! A made-up output for the shell's tests: nothing is played; the cues that reached it are listed, per test thread.
    use iyakki_core_api::soundtypes::CueOut;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub struct Fake {
        pub played: Mutex<Vec<usize>>,
        pub delay_ms: Mutex<u64>,
        pub ready: Mutex<Result<(), String>>,
    }

    impl CueOut for Fake {
        fn ready(&self) -> Result<(), String> {
            self.ready.lock().unwrap().clone()
        }
        fn play(&self, wav: &[u8]) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(*self.delay_ms.lock().unwrap()));
            self.played.lock().unwrap().push(wav.len());
            Ok(())
        }
    }

    thread_local! {
        static OUT: RefCell<Arc<Fake>> = RefCell::new(Arc::new(Fake { played: Mutex::new(vec![]), delay_ms: Mutex::new(0), ready: Mutex::new(Ok(())) }));
    }

    pub fn out() -> Arc<dyn CueOut> {
        OUT.with(|o| o.borrow().clone())
    }

    pub fn current() -> Arc<Fake> {
        OUT.with(|o| o.borrow().clone())
    }

    /// The WAV sizes played so far on this thread's output (each cue has its own size).
    pub fn played() -> Vec<usize> {
        current().played.lock().unwrap().clone()
    }

    /// Give this thread a fresh output.
    pub fn reset(delay_ms: u64) -> Arc<Fake> {
        let f = Arc::new(Fake { played: Mutex::new(vec![]), delay_ms: Mutex::new(delay_ms), ready: Mutex::new(Ok(())) });
        OUT.with(|o| *o.borrow_mut() = f.clone());
        f
    }
}

/// The short chime of "I heard you" (the Notify cue, with the person's own System sounds choices): returns when it has ended, or after the
/// longest wait sign-out also uses, so a stuck output never holds the shell. The wake phrase uses it before the microphone opens again.
pub fn chime_and_wait(core: &dyn Core, cloud: &iyakki_cloudmode::CloudMode) {
    let _ = start(core, cloud, Cue::Notify).wait_short();
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::FakeCore;

    #[test]
    fn a_cue_goes_through_the_cores_player_and_cloud_mode_without_sound_plays_nothing() {
        // the fake core's player records what was asked; the shell's own fake output is only used by tests that run on the real core
        let core = FakeCore::new();
        let cloud = iyakki_cloudmode::CloudMode::desktop();
        let _ = start(&core, &cloud, Cue::Login);
        let _ = start(&core, &cloud, Cue::JobDone);
        assert_eq!(core.cues_fake().played(), vec![Cue::Login, Cue::JobDone]);
        let mut no_sound = iyakki_cloudmode::CloudMode::desktop();
        no_sound.hidden_settings_pages.push("sound");
        let h = start(&core, &no_sound, Cue::Notify);
        assert!(matches!(h.wait_short(), Some(Outcome::Off(_))), "cloud mode without sound hardware says why");
        assert_eq!(core.cues_fake().played().len(), 2, "nothing reached the player");
    }
}
