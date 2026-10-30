//! The dialog "Radio is playing. Stop it so the browser can play?" with Stop it / Keep it / Cancel, shared by the apps that want the
//! sound card (the browser when its page starts to play, the radio when it starts a station).
//!
//! Who holds the card, and how to stop it, is `iyakki_agent::soundhold` (IYAKKI's own players stop cleanly; a program outside is
//! ended through `quiet::stop_owner`: only the owner of an open playback stream, never init or this process, SIGTERM then SIGKILL).
//! This file is only the question, its three answers (keys and clicks), the stop on a thread (an outside program may take a second
//! and a half to let go) and one audit line without anything but the kind and the name of the holder.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::quiet::Env;
use iyakki_agent::soundhold::{self, Found, Registry};

use super::AppCx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, MUTED, R, TEXT};

/// Button ids (`Event::Tab`) in the browser; the radio uses keys.
pub const TAB_STOP: usize = 14;
pub const TAB_KEEP: usize = 15;
pub const TAB_CANCEL: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stop,
    Keep,
    Cancel,
}

/// y or Enter stops it, n or k keeps it, Esc cancels.
pub fn answer_for_key(k: &Key) -> Option<Answer> {
    match k {
        Key::Char('y') | Key::Enter => Some(Answer::Stop),
        Key::Char('n') | Key::Char('k') => Some(Answer::Keep),
        Key::Esc => Some(Answer::Cancel),
        _ => None,
    }
}

pub fn answer_for_tab(n: usize) -> Option<Answer> {
    match n {
        TAB_STOP => Some(Answer::Stop),
        TAB_KEEP => Some(Answer::Keep),
        TAB_CANCEL => Some(Answer::Cancel),
        _ => None,
    }
}

/// A question on the screen, and the stop while it runs.
pub struct Ask {
    pub found: Found,
    /// "the browser", "the radio": who wants the card.
    pub wanter: &'static str,
    reg: Arc<Registry>,
    working: Option<Receiver<Result<String, String>>>,
}

impl Ask {
    pub fn new(found: Found, wanter: &'static str) -> Ask {
        Ask::with_registry(soundhold::global(), found, wanter)
    }

    /// The players are looked up in `reg` (the process-wide registry for `new`).
    pub fn with_registry(reg: Arc<Registry>, found: Found, wanter: &'static str) -> Ask {
        Ask { found, wanter, reg, working: None }
    }

    pub fn question(&self) -> String {
        soundhold::question(&self.found, self.wanter)
    }

    pub fn stopping(&self) -> bool {
        self.working.is_some()
    }

    /// Stop the holder on a thread. `wait` is how long an own player gets to let go. Nothing happens twice.
    pub fn start_stop(&mut self, env: Arc<dyn Env>, asound: PathBuf, wait: Duration) {
        if self.working.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (found, reg) = (self.found.clone(), self.reg.clone());
        let spawned = std::thread::Builder::new().name("iyakki-soundstop".into()).spawn(move || {
            let _ = tx.send(reg.stop(&found, &*env, &asound, wait));
        });
        if spawned.is_ok() {
            self.working = Some(rx);
        }
    }

    /// `Some` once the stop has ended: the calm sentence for the status line, or why not.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let r = self.working.as_ref()?.try_recv();
        match r {
            Ok(v) => {
                self.working = None;
                Some(v)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => {
                self.working = None;
                Some(Err("the stop did not finish".into()))
            }
        }
    }
}

/// The real machine: signals through `libc`, the kernel's `/proc/asound`.
pub fn start_stop_real(ask: &mut Ask, wait: Duration) {
    ask.start_stop(Arc::new(iyakki_agent::quiet::System), PathBuf::from(iyakki_agent::quiet::ASOUND), wait);
}

/// One audit line per answer: what kind of holder and its name, nothing else.
pub fn audit(cx: &AppCx, action: &str, found: &Found, outcome: &str) {
    cx.audit.record(&cx.session.user, action, &soundhold::audit_detail(found, outcome));
}

/// Draw the dialog over `area` with the three buttons sending `stop`, `keep` and `cancel`.
pub fn render(g: &mut Gfx, area: R, s: f32, ask: &Ask, stop: Event, keep: Event, cancel: Event) {
    let accent = crate::gfx::rgb(110, 190, 255);
    let p = crate::fx::modal(g, area, s, 560.0 * s, 210.0 * s, accent, false);
    let q = ask.question();
    for (i, l) in g.wrap(18.0 * s, &q, Face::Bold, p.w - 48.0 * s).into_iter().take(3).enumerate() {
        g.text(p.x + 24.0 * s, p.y + 24.0 * s + i as f32 * 26.0 * s, 18.0 * s, TEXT, &l, Face::Bold);
    }
    let hint = if ask.stopping() { "Stopping..." } else { "y or Enter stops it, n keeps it, Esc cancels." };
    g.text(p.x + 24.0 * s, p.y + 104.0 * s, 14.0 * s, MUTED, hint, Face::Sans);
    let y = p.y + p.h - 58.0 * s;
    if ask.stopping() {
        return;
    }
    let mut x = p.x + 24.0 * s;
    x += g.button(x, y, 14.0 * s, "Stop it", accent, stop) + 10.0 * s;
    x += g.button(x, y, 14.0 * s, "Keep it", crate::gfx::rgb(150, 160, 180), keep) + 10.0 * s;
    g.button(x, y, 14.0 * s, "Cancel", crate::gfx::rgb(150, 160, 180), cancel);
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_agent::soundhold::Who;

    struct E;
    impl Env for E {
        fn amixer(&self, _: &[String]) -> Result<(), String> {
            Ok(())
        }
        fn signal(&self, _: u32, _: i32) {}
        fn sleep(&self, _: Duration) {}
        fn silence(&self, _: Duration) {}
        fn own_pid(&self) -> u32 {
            4242
        }
    }

    fn found() -> Found {
        Found { who: Who::Own("soundask-test".into()), says: "Radio is playing Radio X".into() }
    }

    #[test]
    fn keys_and_buttons_give_the_three_answers_and_nothing_else_answers() {
        assert_eq!(answer_for_key(&Key::Char('y')), Some(Answer::Stop));
        assert_eq!(answer_for_key(&Key::Enter), Some(Answer::Stop));
        assert_eq!(answer_for_key(&Key::Char('n')), Some(Answer::Keep));
        assert_eq!(answer_for_key(&Key::Esc), Some(Answer::Cancel));
        assert_eq!(answer_for_key(&Key::Char('x')), None);
        assert_eq!(answer_for_tab(TAB_STOP), Some(Answer::Stop));
        assert_eq!(answer_for_tab(TAB_KEEP), Some(Answer::Keep));
        assert_eq!(answer_for_tab(TAB_CANCEL), Some(Answer::Cancel));
        assert_eq!(answer_for_tab(13), None);
    }

    #[test]
    fn the_dialog_names_the_holder_has_three_buttons_and_fits_a_small_screen() {
        for (w, h, s) in [(1280.0, 800.0, 1.0), (640.0, 400.0, 1.0), (2880.0, 1800.0, 2.0)] {
            let mut g = Gfx::new(w as u32, h as u32);
            let ask = Ask::new(found(), "the browser");
            render(&mut g, R::new(0.0, 0.0, w, h), s, &ask, Event::Tab(TAB_STOP), Event::Tab(TAB_KEEP), Event::Tab(TAB_CANCEL));
            let labels: Vec<String> = g.hits.iter().map(|h| h.label.clone()).collect();
            for want in ["Stop it", "Keep it", "Cancel"] {
                assert!(labels.iter().any(|l| l == want), "{want} in {labels:?} at {w}x{h}");
            }
            for hit in g.hits.iter().filter(|x| x.label != "backdrop") {
                assert!(hit.rect.x >= 0.0 && hit.rect.y >= 0.0 && hit.rect.x + hit.rect.w <= w && hit.rect.y + hit.rect.h <= h, "{} is off the screen at {w}x{h}", hit.label);
            }
        }
        assert_eq!(Ask::new(found(), "the browser").question(), "Radio is playing Radio X. Stop it so the browser can play?");
    }

    #[test]
    fn stopping_runs_on_a_thread_once_and_reports_when_it_is_done() {
        let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (a, b) = (on.clone(), on.clone());
        let reg = Arc::new(Registry::default());
        reg.announce("soundask-test", Arc::new(move || a.load(std::sync::atomic::Ordering::SeqCst).then(|| "Radio is playing Radio X".to_string())), Arc::new(move || b.store(false, std::sync::atomic::Ordering::SeqCst)));
        let mut ask = Ask::with_registry(reg, found(), "the browser");
        assert!(ask.poll().is_none());
        ask.start_stop(Arc::new(E), PathBuf::from("/nonexistent"), Duration::from_secs(1));
        assert!(ask.stopping());
        ask.start_stop(Arc::new(E), PathBuf::from("/nonexistent"), Duration::from_secs(1));
        let t0 = std::time::Instant::now();
        let r = loop {
            if let Some(r) = ask.poll() {
                break r;
            }
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(r, Ok("Stopped.".to_string()));
        assert!(!on.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!ask.stopping());
    }
}
