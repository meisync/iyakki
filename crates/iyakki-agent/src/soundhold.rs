//! Who holds the sound card, so the shell can ask before taking it ("Radio is playing. Stop it so the browser can play?").
//!
//! One look at a card that cannot be shared (the T2 Mac's, or any card while a program outside the mixer holds it) says only
//! "an `aplay` process owns it". The people need the human name, and IYAKKI's own players need a clean stop (not a kill). So:
//! * IYAKKI's own players (the radio, TALES' reader, the browser) [`announce`] themselves once: a function that answers "do you
//!   hold the speaker now, and what do I say about it" and a function that stops them cleanly;
//! * [`find`] picks the holder to show: an announced own player first, else a program outside IYAKKI that the kernel says owns an
//!   open playback stream (not the web engine itself, not anything below this process, never init);
//! * [`stop`] stops an own player by its own function and an outside program through [`crate::quiet::stop_owner`] (SIGTERM, then
//!   SIGKILL, only the owner of an open playback stream, never pid 1 or this process).
//!
//! Players announce themselves to a [`Registry`]; the process has one ([`global`]), tests make their own.
//!
//! Nothing here asks the person: the shell shows the dialog (`apps/browser_sound.rs`, `apps/radio.rs`) and calls [`stop`] on
//! "Stop it". Nothing is started by this module.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::quiet::Env;
use crate::sounddoctor::Holder;

/// Answers with a sentence ("Radio is playing Radio X") while the player holds the speaker, else None.
pub type Holds = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Stops the player cleanly.
pub type StopFn = Arc<dyn Fn() + Send + Sync>;

struct Own {
    key: String,
    holds: Holds,
    stop: StopFn,
}

/// The players that announced themselves. The process has one ([`global`]); tests make their own, so parallel tests cannot
/// see each other's players.
#[derive(Default)]
pub struct Registry {
    items: Mutex<Vec<Own>>,
}

static GLOBAL: OnceLock<Arc<Registry>> = OnceLock::new();

/// The registry of this process.
pub fn global() -> Arc<Registry> {
    GLOBAL.get_or_init(|| Arc::new(Registry::default())).clone()
}

/// Register (or replace, by `key`) one of IYAKKI's own players in the process-wide registry.
pub fn announce(key: &str, holds: Holds, stop: StopFn) {
    global().announce(key, holds, stop);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    /// One of IYAKKI's own players, by the key it announced itself with.
    Own(String),
    /// A program outside IYAKKI that owns an open playback stream.
    Outside { pid: u32, name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub who: Who,
    /// "Radio is playing Radio X", "TALES is reading", "mpv is using the sound card".
    pub says: String,
}

/// A program name that is safe to show: printable, short.
fn tidy(name: &str) -> String {
    let t: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    if t.trim().is_empty() { "another program".to_string() } else { t.trim().to_string() }
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Own>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register (or replace, by `key`) one of IYAKKI's own players.
    pub fn announce(&self, key: &str, holds: Holds, stop: StopFn) {
        let mut r = self.lock();
        r.retain(|o| o.key != key);
        r.push(Own { key: key.to_string(), holds, stop });
    }

    /// A player went away for good.
    pub fn forget(&self, key: &str) {
        self.lock().retain(|o| o.key != key);
    }

    /// What the announced player `key` says right now (None: it is not announced or holds nothing).
    pub fn holds(&self, key: &str) -> Option<String> {
        let f = self.lock().iter().find(|o| o.key == key).map(|o| o.holds.clone())?;
        f()
    }

    /// The holder to show to `wanter` (the key of the player that wants the card: its own entry is skipped). `outside` is what the
    /// kernel scan found ([`crate::sounddoctor::scan`]); `ours(pid)` says whether a process belongs to IYAKKI itself (the mixer's
    /// `aplay`, a chime): those are never offered.
    pub fn find(&self, wanter: &str, outside: Option<&Holder>, ours: &dyn Fn(u32) -> bool) -> Option<Found> {
        let own: Vec<(String, Holds)> = self.lock().iter().filter(|o| o.key != wanter).map(|o| (o.key.clone(), o.holds.clone())).collect();
        for (key, holds) in own {
            if let Some(says) = holds() {
                return Some(Found { who: Who::Own(key), says });
            }
        }
        let h = outside?;
        if h.pid <= 1 || ours(h.pid) {
            return None;
        }
        let name = tidy(&h.name);
        Some(Found { who: Who::Outside { pid: h.pid, name: name.clone() }, says: format!("{name} is using the sound card") })
    }

    /// The holder a player that is about to START should ask about (the radio): an announced own player, except that with the mixer
    /// on (`mixing`) the players that go through the mixer share the card and are no conflict; the browser does not go through it
    /// (unless [`crate::webmix`] is active, then it announces nothing). Programs outside IYAKKI are not looked for here.
    pub fn conflict_for_new_player(&self, wanter: &str, mixing: bool) -> Option<Found> {
        let keys: Vec<String> = self.lock().iter().filter(|o| o.key != wanter).map(|o| o.key.clone()).collect();
        for k in keys {
            if mixing && k != "browser" {
                continue;
            }
            let holds = self.lock().iter().find(|o| o.key == k).map(|o| o.holds.clone());
            if let Some(says) = holds.and_then(|h| h()) {
                return Some(Found { who: Who::Own(k), says });
            }
        }
        None
    }

    /// Stop what `found` names. For an own player: its stop function, then up to `wait` for it to say it no longer holds the
    /// speaker. For a program outside: [`crate::quiet::stop_owner`]. The answer is a calm sentence for the status line.
    pub fn stop(&self, found: &Found, env: &dyn Env, asound: &Path, wait: Duration) -> Result<String, String> {
        match &found.who {
            Who::Own(key) => {
                let entry = self.lock().iter().find(|o| &o.key == key).map(|o| (o.holds.clone(), o.stop.clone()));
                let Some((holds, stop)) = entry else { return Err("that player is gone".into()) };
                if holds().is_none() {
                    return Ok("It had already stopped.".into());
                }
                stop();
                let t0 = Instant::now();
                while holds().is_some() && t0.elapsed() < wait {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if holds().is_some() { Err("It did not stop.".into()) } else { Ok("Stopped.".into()) }
            }
            Who::Outside { pid, name } => match crate::quiet::stop_owner(env, asound, *pid)? {
                crate::quiet::Stopped::Ended => Ok(format!("{name} was ended.")),
                crate::quiet::Stopped::Killed => Ok(format!("{name} was ended by force: it did not answer.")),
            },
        }
    }
}

/// [`Registry::find`] on the process-wide registry.
pub fn find(wanter: &str, outside: Option<&Holder>, ours: &dyn Fn(u32) -> bool) -> Option<Found> {
    global().find(wanter, outside, ours)
}

/// [`Registry::conflict_for_new_player`] on the process-wide registry.
pub fn conflict_for_new_player(wanter: &str, mixing: bool) -> Option<Found> {
    global().conflict_for_new_player(wanter, mixing)
}

/// [`Registry::stop`] on the process-wide registry.
pub fn stop(found: &Found, env: &dyn Env, asound: &Path, wait: Duration) -> Result<String, String> {
    global().stop(found, env, asound, wait)
}

/// The audit detail of a stop: what kind of holder and its name or key, nothing else.
pub fn audit_detail(found: &Found, answer: &str) -> String {
    match &found.who {
        Who::Own(k) => format!("own:{k} {answer}"),
        Who::Outside { name, .. } => format!("outside:{name} {answer}"),
    }
}

/// The dialog's question, for `wanter_name` ("the browser", "the radio").
pub fn question(found: &Found, wanter_name: &str) -> String {
    format!("{}. Stop it so {wanter_name} can play?", found.says)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct NoEnv;
    impl Env for NoEnv {
        fn amixer(&self, _: &[String]) -> Result<(), String> {
            Ok(())
        }
        fn signal(&self, _: u32, _: i32) {
            panic!("an own player is not signalled");
        }
        fn sleep(&self, _: Duration) {}
        fn silence(&self, _: Duration) {}
        fn own_pid(&self) -> u32 {
            4242
        }
    }

    fn player(reg: &Registry, key: &str, says: &str) -> Arc<AtomicBool> {
        let on = Arc::new(AtomicBool::new(true));
        let (a, b, s) = (on.clone(), on.clone(), says.to_string());
        reg.announce(key, Arc::new(move || a.load(Ordering::SeqCst).then(|| s.clone())), Arc::new(move || b.store(false, Ordering::SeqCst)));
        on
    }

    fn outside(pid: u32, name: &str) -> Holder {
        Holder { pid, name: name.into(), running: true }
    }

    #[test]
    fn an_own_player_is_named_first_skipping_the_one_that_asks_and_a_quiet_one_is_not_a_holder() {
        let reg = Registry::default();
        let radio = player(&reg, "radio", "Radio is playing Radio X");
        let _ = player(&reg, "browser", "The browser is playing sound");
        let f = reg.find("browser", Some(&outside(900, "mpv")), &|_| false).unwrap();
        assert_eq!(f.who, Who::Own("radio".into()));
        assert_eq!(question(&f, "the browser"), "Radio is playing Radio X. Stop it so the browser can play?");
        radio.store(false, Ordering::SeqCst);
        // the radio is quiet now: the outside program is next; the asking player never offers itself
        let f = reg.find("browser", Some(&outside(900, "mpv")), &|_| false).unwrap();
        assert_eq!(f.who, Who::Outside { pid: 900, name: "mpv".into() });
        assert_eq!(f.says, "mpv is using the sound card");
        assert!(reg.find("browser", None, &|_| false).is_none());
        reg.forget("radio");
        assert_eq!(reg.holds("radio"), None);
    }

    #[test]
    fn init_and_iyakkis_own_children_are_never_offered_and_odd_names_are_tidied() {
        let reg = Registry::default();
        assert!(reg.find("x", Some(&outside(1, "systemd")), &|_| false).is_none());
        assert!(reg.find("x", Some(&outside(0, "another program")), &|_| false).is_none());
        assert!(reg.find("x", Some(&outside(500, "aplay")), &|p| p == 500).is_none());
        let f = reg.find("x", Some(&outside(501, "ev\u{1b}[31mil\nname-that-is-far-too-long-to-show-in-a-dialog")), &|_| false).unwrap();
        let Who::Outside { name, .. } = &f.who else { panic!() };
        assert!(name.chars().count() <= 32 && !name.chars().any(|c| c.is_control()), "{name:?}");
        assert!(tidy("  ") == "another program");
    }

    #[test]
    fn a_player_that_starts_asks_about_a_player_that_would_lock_it_out_but_not_about_one_the_mixer_shares() {
        let reg = Registry::default();
        let _ = player(&reg, "tales", "TALES is reading");
        // mixer off: any player holding the speaker is a conflict
        assert_eq!(reg.conflict_for_new_player("radio", false).map(|f| f.who), Some(Who::Own("tales".into())));
        // mixer on: TALES shares it
        assert!(reg.conflict_for_new_player("radio", true).is_none());
        // the browser's engine opens the card itself, so it conflicts even with the mixer on
        let _ = player(&reg, "browser", "The browser is playing sound");
        assert_eq!(reg.conflict_for_new_player("radio", true).map(|f| f.who), Some(Who::Own("browser".into())));
        assert!(reg.conflict_for_new_player("browser", true).is_none(), "a player never conflicts with itself");
        reg.forget("tales");
        reg.forget("browser");
        assert!(reg.conflict_for_new_player("radio", false).is_none());
    }

    #[test]
    fn stopping_an_own_player_uses_its_own_stop_waits_for_it_and_does_not_signal_anything() {
        let reg = Registry::default();
        let on = player(&reg, "tales", "TALES is reading");
        let f = reg.find("x", None, &|_| false).unwrap();
        assert_eq!(f.who, Who::Own("tales".into()));
        let nowhere = Path::new("/nonexistent");
        assert_eq!(reg.stop(&f, &NoEnv, nowhere, Duration::from_secs(1)).unwrap(), "Stopped.");
        assert!(!on.load(Ordering::SeqCst));
        // asked again: it had already stopped
        assert_eq!(reg.stop(&f, &NoEnv, nowhere, Duration::from_secs(1)).unwrap(), "It had already stopped.");
        assert_eq!(audit_detail(&f, "stopped"), "own:tales stopped");
        // a player that ignores the stop is reported, not claimed stopped
        reg.announce("stuck", Arc::new(|| Some("stuck".into())), Arc::new(|| {}));
        let s = Found { who: Who::Own("stuck".into()), says: "stuck".into() };
        assert_eq!(reg.stop(&s, &NoEnv, nowhere, Duration::from_millis(60)), Err("It did not stop.".into()));
        reg.forget("stuck");
        assert!(reg.stop(&s, &NoEnv, nowhere, Duration::from_millis(10)).is_err());
    }

    #[test]
    fn an_outside_program_goes_through_the_guarded_stop_of_quiet() {
        let reg = Registry::default();
        let r = std::env::temp_dir().join(format!("soundhold-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        let d = r.join("card0/pcm0p/sub0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("status"), "state: RUNNING\nowner_pid : 700\n").unwrap();
        struct E(std::path::PathBuf);
        impl Env for E {
            fn amixer(&self, _: &[String]) -> Result<(), String> {
                Ok(())
            }
            fn signal(&self, pid: u32, sig: i32) {
                assert_eq!((pid, sig), (700, libc::SIGTERM));
                std::fs::write(self.0.join("status"), "closed\n").unwrap();
            }
            fn sleep(&self, _: Duration) {}
            fn silence(&self, _: Duration) {}
            fn own_pid(&self) -> u32 {
                4242
            }
        }
        let f = Found { who: Who::Outside { pid: 700, name: "mpv".into() }, says: "mpv is using the sound card".into() };
        assert_eq!(reg.stop(&f, &E(d.clone()), &r, Duration::from_secs(1)).unwrap(), "mpv was ended.");
        assert_eq!(audit_detail(&f, "ended"), "outside:mpv ended");
        // it is gone from the card: a second stop is refused, and init can never be named
        assert!(reg.stop(&f, &E(d), &r, Duration::from_secs(1)).is_err());
        let init = Found { who: Who::Outside { pid: 1, name: "init".into() }, says: String::new() };
        assert!(reg.stop(&init, &NoEnv, &r, Duration::from_secs(1)).is_err());
        let _ = std::fs::remove_dir_all(&r);
    }
}
