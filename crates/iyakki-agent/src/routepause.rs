//! What pauses the video tile's players when the Bluetooth headphones leave (F-LIFE-14, P-05): the sound does not move to the room speakers,
//! the picture waits, and the person presses play. Every player the agent starts is remembered (weakly: a closed player is simply gone) and
//! [`pause_playing`] pauses those that are playing. [`pauser`] is what `BtRoute::on_lost` is given.

use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A player that can be paused by the route watcher.
pub trait Pausable: Send + Sync {
    /// It is playing now (not paused, not ended).
    fn playing(&self) -> bool;
    fn pause(&self);
}

impl Pausable for crate::player::Player {
    fn playing(&self) -> bool {
        let s = crate::player::Player::status(self);
        !s.paused && !s.ended && s.error.is_none()
    }
    fn pause(&self) {
        crate::player::Player::pause(self)
    }
}

/// The players one registry remembers.
#[derive(Default)]
pub struct Players {
    live: Mutex<Vec<Weak<dyn Pausable>>>,
}

impl Players {
    /// Remember a player (closed ones are dropped on the way).
    pub fn add(&self, p: &Arc<dyn Pausable>) {
        let mut v = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|w| w.strong_count() > 0);
        v.push(Arc::downgrade(p));
    }

    /// Pause every player that is playing; true when at least one was.
    pub fn pause_playing(&self) -> bool {
        let live: Vec<Arc<dyn Pausable>> = self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().filter_map(Weak::upgrade).collect();
        let mut any = false;
        for p in live {
            if p.playing() {
                p.pause();
                any = true;
            }
        }
        any
    }
}

/// The video players of this process.
pub fn global() -> &'static Players {
    static P: OnceLock<Players> = OnceLock::new();
    P.get_or_init(Players::default)
}

/// The pauser registered with the route watcher under `video-tile`: pauses the playing players, one audit line when something was paused.
pub fn pauser(players: &'static Players, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> crate::btroute::Pauser {
    let user = user.to_string();
    Arc::new(move || {
        let did = players.pause_playing();
        if did {
            audit(&user, "bluetooth.route_lost paused the video");
        }
        did
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Fake {
        playing: AtomicBool,
        paused_calls: std::sync::atomic::AtomicUsize,
    }
    impl Fake {
        fn new(playing: bool) -> Arc<Fake> {
            Arc::new(Fake { playing: playing.into(), paused_calls: 0.into() })
        }
    }
    impl Pausable for Fake {
        fn playing(&self) -> bool {
            self.playing.load(Ordering::SeqCst)
        }
        fn pause(&self) {
            self.paused_calls.fetch_add(1, Ordering::SeqCst);
            self.playing.store(false, Ordering::SeqCst);
        }
    }

    #[test]
    fn only_a_player_that_plays_is_paused_and_a_closed_one_is_forgotten() {
        let reg = Players::default();
        let (a, b) = (Fake::new(true), Fake::new(false));
        let (da, db): (Arc<dyn Pausable>, Arc<dyn Pausable>) = (a.clone(), b.clone());
        reg.add(&da);
        reg.add(&db);
        let gone = Fake::new(true);
        let dg: Arc<dyn Pausable> = gone.clone();
        reg.add(&dg);
        drop(dg);
        drop(gone);
        assert!(reg.pause_playing());
        assert_eq!(a.paused_calls.load(Ordering::SeqCst), 1);
        assert_eq!(b.paused_calls.load(Ordering::SeqCst), 0, "a paused player is left alone");
        assert!(!reg.pause_playing(), "nothing plays any more");
    }

    #[test]
    fn the_pauser_says_one_line_only_when_something_was_paused() {
        let reg: &'static Players = Box::leak(Box::new(Players::default()));
        let p = Fake::new(true);
        let dp: Arc<dyn Pausable> = p.clone();
        reg.add(&dp);
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        let f = pauser(reg, "alice", Arc::new(move |who, what| l2.lock().unwrap().push(format!("{who}: {what}"))));
        assert!(f());
        assert!(!f());
        assert_eq!(*lines.lock().unwrap(), ["alice: bluetooth.route_lost paused the video"]);
    }
}
