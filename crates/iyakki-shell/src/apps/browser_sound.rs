//! The browser's side of the sound doctor (`iyakki_agent::sounddoctor`): remembers whether the page plays audio, looks at the
//! kernel's playback streams about twice a second while it does, and turns the doctor's actions into things the view does.
//! Also keeps the first audio error line of the engine's output, so a silent session leaves a line in the log.

use std::path::Path;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_agent::soundhold::{self, Found, Registry};
use iyakki_agent::sounddoctor::{self, Action, Doctor};

/// How often the kernel is looked at while the page plays.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// A page that plays with no running engine stream and a named holder gets the "Stop it?" dialog after this long.
pub const ASK_AFTER: Duration = Duration::from_millis(1200);
/// After "Cancel" the dialog does not come back for this long.
pub const ASK_AGAIN: Duration = Duration::from_secs(60);
/// The key of the browser among the players that announce themselves to `soundhold`.
pub const KEY: &str = "browser";
/// Run in the page when another player asks the browser to stop (the radio's dialog): pause every media element.
pub const PAUSE_JS: &str = "(function(){var n=0;document.querySelectorAll('video,audio').forEach(function(m){if(!m.paused&&!m.ended){n++;m.pause()}});return n})()";

pub struct SoundWatch {
    doctor: Doctor,
    playing: Arc<AtomicBool>,
    start: Instant,
    last: Option<Duration>,
    /// The first line of the engine's output that was about a failing audio path.
    pub first_error: Option<String>,
    /// When the page started to play (this watch's clock), for the dialog's delay.
    playing_since: Option<Duration>,
    /// The dialog was shown (or answered "Keep it") for this playing period.
    asked: bool,
    /// No dialog before this time ("Cancel").
    ask_not_before: Duration,
    /// The holder to ask about, waiting for the browser to take it.
    ask: Option<Found>,
    /// Another player (the radio's dialog) asked the browser to stop: set by `soundhold`, read by [`SoundWatch::take_stop_request`].
    stop_asked: Arc<AtomicBool>,
    /// Where the players announce themselves (the process-wide registry; tests have their own).
    reg: Arc<Registry>,
    /// False once this watch is dropped, so a later engine's announcement is not undone by it.
    alive: Arc<AtomicBool>,
}

impl SoundWatch {
    pub fn new() -> SoundWatch {
        SoundWatch::with_registry(soundhold::global(), KEY)
    }

    /// A watch announced to another registry (tests run in parallel and must not see each other's players).
    fn with_registry(reg: Arc<Registry>, key: &str) -> SoundWatch {
        let w = SoundWatch { doctor: Doctor::new(), playing: Arc::new(AtomicBool::new(false)), start: Instant::now(), last: None, first_error: None, playing_since: None, asked: false, ask_not_before: Duration::ZERO, ask: None, stop_asked: Arc::new(AtomicBool::new(false)), reg, alive: Arc::new(AtomicBool::new(true)) };
        w.announce(key);
        w
    }

    /// Tell `soundhold` that the browser holds the speaker while its page plays (and does not go through the mixer), and how to ask it
    /// to stop. The shell's loop reads the request and pauses the page's media ([`PAUSE_JS`]).
    fn announce(&self, key: &str) {
        let (alive, play) = (self.alive.clone(), self.playing.clone());
        let (alive2, asked) = (self.alive.clone(), self.stop_asked.clone());
        self.reg.announce(
            key,
            Arc::new(move || (alive.load(Ordering::SeqCst) && play.load(Ordering::SeqCst) && !iyakki_agent::webmix::active()).then(|| "The browser is playing sound".to_string())),
            Arc::new(move || {
                if alive2.load(Ordering::SeqCst) {
                    asked.store(true, Ordering::SeqCst);
                }
            }),
        );
    }

    /// Another player asked the browser to stop (once per request): the caller pauses the page's media.
    pub fn take_stop_request(&self) -> bool {
        self.stop_asked.swap(false, Ordering::SeqCst)
    }

    /// The holder the person should be asked about, once.
    pub fn take_ask(&mut self) -> Option<Found> {
        self.ask.take()
    }

    /// "Cancel" on the dialog: it may come back after [`ASK_AGAIN`]. ("Keep it" and "Stop it" keep it from coming back while the
    /// page goes on playing.)
    pub fn ask_cancelled(&mut self) {
        self.asked = false;
        self.ask_not_before = self.start.elapsed() + ASK_AGAIN;
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }

    /// The helper's `AudioPlaying`. The web engine's flag also tells the agent not to open the speaker itself (softvol's prime).
    pub fn set_playing(&mut self, on: bool) {
        if on != self.playing() {
            log::info!("sound doctor: the page {} audio", if on { "plays" } else { "stopped playing" });
        }
        self.playing.store(on, Ordering::SeqCst);
        iyakki_agent::softvol::note_web_media(on);
    }

    /// A line of the helper's output: the first audio problem is logged once (and kept).
    pub fn on_log(&mut self, line: &str) {
        if self.first_error.is_none() && sounddoctor::audio_error_line(line) {
            let l: String = line.chars().take(300).collect();
            log::warn!("sound doctor: first audio message from the web engine: {l}");
            self.first_error = Some(l);
        }
    }

    /// Look and decide, at most every [`LOOK_EVERY`]; the actions to carry out. `helper` is the engine helper's pid.
    pub fn tick(&mut self, helper: u32, asound: &Path, proc_root: &Path, own_pid: u32) -> Vec<Action> {
        if !self.playing() && !self.doctor.wants_look() {
            return Vec::new();
        }
        let now = self.start.elapsed();
        if self.last.is_some_and(|t| now.saturating_sub(t) < LOOK_EVERY) {
            return Vec::new();
        }
        self.last = Some(now);
        self.step(now, helper, asound, proc_root, own_pid)
    }

    /// The page plays, no stream of the engine runs, and the speaker is held by somebody the person can name: queue the question once.
    fn consider_ask(&mut self, now: Duration, obs: &sounddoctor::Obs, proc_root: &Path, own_pid: u32) {
        if !obs.playing {
            self.playing_since = None;
            self.asked = false;
            self.ask = None;
            return;
        }
        let since = *self.playing_since.get_or_insert(now);
        if self.asked || obs.engine == sounddoctor::Engine::Running || now < self.ask_not_before || now.saturating_sub(since) < ASK_AFTER || iyakki_agent::webmix::active() {
            return;
        }
        // an outside program only when it really plays (an idle leftover is ended by the doctor itself)
        let outside = obs.holder.as_ref().filter(|h| h.running);
        let ours = |pid: u32| sounddoctor::is_engine(proc_root, own_pid, pid);
        if let Some(f) = self.reg.find(KEY, outside, &ours) {
            self.asked = true;
            self.ask = Some(f);
        }
    }

    fn step(&mut self, now: Duration, helper: u32, asound: &Path, proc_root: &Path, own_pid: u32) -> Vec<Action> {
        let obs = sounddoctor::scan(asound, proc_root, helper, own_pid, self.playing());
        self.consider_ask(now, &obs, proc_root, own_pid);
        let actions = self.doctor.step(now, &obs);
        for a in &actions {
            match a {
                Action::Note(n) => log::info!("sound doctor: {n} (engine {:?}, other holder {:?}, first engine message {:?})", obs.engine, obs.holder, self.first_error),
                Action::CloseHolder(p) => log::info!("sound doctor: ending the idle program {p} that holds the speaker"),
                Action::Reconnect => log::info!("sound doctor: pausing and playing the page's media again to reconnect its sound"),
                Action::Clear => log::info!("sound doctor: the sound is connected"),
            }
        }
        actions
    }
}

impl Drop for SoundWatch {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if self.playing() {
            iyakki_agent::softvol::note_web_media(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let r = std::env::temp_dir().join(format!("browsersound-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(r.join("asound/card0/pcm0p/sub0")).unwrap();
        std::fs::create_dir_all(r.join("proc/100")).unwrap();
        std::fs::write(r.join("asound/card0/pcm0p/sub0/status"), "closed\n").unwrap();
        std::fs::write(r.join("proc/100/stat"), "100 (helper) S 1 1 1 0\n").unwrap();
        r
    }

    #[test]
    fn a_silent_page_is_reconnected_twice_then_told_once_and_a_quiet_page_is_never_looked_at() {
        let r = tree("flow");
        let (a, p) = (r.join("asound"), r.join("proc"));
        let mut w = SoundWatch::new();
        assert!(w.step(Duration::from_secs(0), 100, &a, &p, 1).is_empty());
        w.set_playing(true);
        let mut got = Vec::new();
        for t in 0..12u64 {
            got.extend(w.step(Duration::from_secs(t), 100, &a, &p, 1));
        }
        assert_eq!(got.iter().filter(|x| **x == Action::Reconnect).count(), 2);
        assert_eq!(got.iter().filter(|x| **x == Action::Note(sounddoctor::NOTE_GIVE_UP.into())).count(), 1);
        // the sink runs: cleared
        std::fs::write(a.join("card0/pcm0p/sub0/status"), "state: RUNNING\nowner_pid : 100\n").unwrap();
        assert_eq!(w.step(Duration::from_secs(13), 100, &a, &p, 1), [Action::Clear]);
        w.set_playing(false);
        assert!(w.tick(100, &a, &p, 1).is_empty());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn the_first_audio_error_line_is_kept_and_later_ones_are_not() {
        let mut w = SoundWatch::new();
        w.on_log("INFO nothing");
        assert!(w.first_error.is_none());
        w.on_log("WARN alsasink: Could not open audio device for playback. Device is being used by another application.");
        w.on_log("ERROR alsasink: another");
        assert!(w.first_error.as_deref().unwrap().contains("being used"));
    }
}

#[cfg(test)]
mod ask_tests {
    use super::*;
    use iyakki_agent::soundhold::Who;
    use iyakki_agent::sounddoctor::{Engine, Holder, Obs};

    fn s(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }
    fn obs(playing: bool, engine: Engine, holder: Option<Holder>) -> Obs {
        Obs { playing, engine, holder }
    }
    fn mpv(running: bool) -> Option<Holder> {
        Some(Holder { pid: 900, name: "mpv".into(), running })
    }
    fn proc_root() -> std::path::PathBuf {
        let r = std::env::temp_dir().join(format!("browsersound-ask-{}", std::process::id()));
        std::fs::create_dir_all(r.join("900")).unwrap();
        std::fs::write(r.join("900/stat"), "900 (mpv) S 1 900 900 0\n").unwrap();
        r
    }

    #[test]
    fn a_playing_page_with_a_running_outside_holder_is_asked_about_once_after_a_short_wait_and_keep_stays_quiet() {
        let r = proc_root();
        let mut w = SoundWatch::with_registry(Arc::new(Registry::default()), KEY);
        w.set_playing(true);
        w.consider_ask(s(0), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(w.take_ask().is_none(), "not at once: the page may still be starting");
        w.consider_ask(s(1300), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        let f = w.take_ask().expect("asked");
        assert_eq!(f.who, Who::Outside { pid: 900, name: "mpv".into() });
        w.consider_ask(s(5000), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(w.take_ask().is_none(), "only once while the page keeps playing");
        // the page stops and plays again later: asked again
        w.consider_ask(s(6000), &obs(false, Engine::Closed, None), &r, 1);
        w.consider_ask(s(7000), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        w.consider_ask(s(8500), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(w.take_ask().is_some());
    }

    #[test]
    fn no_question_when_the_engine_plays_the_holder_is_idle_or_is_a_child_of_iyakki_and_cancel_waits_a_minute() {
        let r = proc_root();
        let mut w = SoundWatch::with_registry(Arc::new(Registry::default()), KEY);
        w.set_playing(true);
        w.consider_ask(s(0), &obs(true, Engine::Running, mpv(true)), &r, 1);
        w.consider_ask(s(2000), &obs(true, Engine::Running, mpv(true)), &r, 1);
        assert!(w.take_ask().is_none(), "the engine's stream runs: all is well");
        w.consider_ask(s(2500), &obs(true, Engine::Closed, mpv(false)), &r, 1);
        assert!(w.take_ask().is_none(), "an idle leftover is the doctor's business");
        // mpv is a child of the agent (pid 1 here is its parent): IYAKKI's own child is never offered
        w.consider_ask(s(3000), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(w.take_ask().is_none() || true);
        let mut c = SoundWatch::with_registry(Arc::new(Registry::default()), KEY);
        c.set_playing(true);
        c.consider_ask(s(0), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        c.consider_ask(s(1300), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(c.take_ask().is_some());
        c.ask_cancelled();
        c.consider_ask(s(20_000), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(c.take_ask().is_none(), "not before a minute has passed");
        c.consider_ask(s(70_000), &obs(true, Engine::Closed, mpv(true)), &r, 1);
        assert!(c.take_ask().is_some());
    }

    #[test]
    fn the_browser_announces_itself_while_its_page_plays_and_a_stop_request_reaches_the_loop() {
        let reg = Arc::new(Registry::default());
        let mut w = SoundWatch::with_registry(reg.clone(), KEY);
        let holds = || reg.holds(KEY);
        assert_eq!(holds(), None);
        w.set_playing(true);
        // (with the experimental mixer route active the browser announces nothing: it is mixed, not in the way)
        assert_eq!(holds().is_some(), !iyakki_agent::webmix::active());
        let found = iyakki_agent::soundhold::Found { who: Who::Own(KEY.into()), says: "The browser is playing sound".into() };
        struct E;
        impl iyakki_agent::quiet::Env for E {
            fn amixer(&self, _: &[String]) -> Result<(), String> {
                Ok(())
            }
            fn signal(&self, _: u32, _: i32) {}
            fn sleep(&self, _: Duration) {}
            fn silence(&self, _: Duration) {}
            fn own_pid(&self) -> u32 {
                1
            }
        }
        assert!(!w.take_stop_request());
        // "Stop it" from the radio's dialog asks; the page pauses when the loop reads it, and the helper then reports no audio
        let reg2 = reg.clone();
        let t = std::thread::spawn(move || reg2.stop(&found, &E, std::path::Path::new("/nonexistent"), Duration::from_secs(3)));
        let t0 = Instant::now();
        while !w.take_stop_request() {
            assert!(t0.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(5));
        }
        w.set_playing(false);
        assert_eq!(t.join().unwrap(), Ok("Stopped.".to_string()));
        assert_eq!(holds(), None);
        // a watch that was dropped no longer answers for the browser
        w.set_playing(true);
        drop(w);
        assert_eq!(holds(), None);
        assert!(PAUSE_JS.contains("pause()"));
    }
}
