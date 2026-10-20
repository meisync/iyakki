//! `screen_capture`: the assistant takes a picture of the person's current IYAKKI screen.
//!
//! Apps cannot draw the whole screen themselves, only the shell can. So the tool puts a request in [`ScreenCapture`] and waits;
//! the shell, which owns the last frame it drew, answers on its next drawing (the same hand-over as `browserjobs`). The person
//! is asked **every time** (an inline approval in the chat, `Risk::Ask`), with one extra choice, "Allow for 5 minutes", which
//! skips the question for that long. Nothing is blanked out automatically, so the question says the picture may show private
//! information. Every capture is an audit entry `screen.capture`. The tool exists only while a shell with a screen is running.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::Policy;

pub const ASK: &str = "Let the assistant take a picture of your IYAKKI screen? It may show private information.";
pub const GRANT_SECS: u64 = 300;

/// One picture of the screen: RGBA, opaque.
#[derive(Clone, Debug)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
struct State {
    next: u64,
    waiting: VecDeque<(u64, String)>,
    done: HashMap<u64, Result<Frame, String>>,
    grants: HashMap<String, Instant>,
}

pub struct ScreenCapture {
    st: Mutex<State>,
    cv: Condvar,
    present: AtomicBool,
    wait_ms: AtomicU64,
}

impl Default for ScreenCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenCapture {
    pub fn new() -> ScreenCapture {
        ScreenCapture { st: Mutex::new(State::default()), cv: Condvar::new(), present: AtomicBool::new(false), wait_ms: AtomicU64::new(10_000) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A shell with a screen is running (window, framebuffer or web front end): the tool is offered.
    pub fn set_present(&self, on: bool) {
        self.present.store(on, Ordering::SeqCst);
        if !on {
            let mut g = self.lock();
            let ids: Vec<u64> = g.waiting.drain(..).map(|(i, _)| i).collect();
            for i in ids {
                g.done.insert(i, Err("the screen went away".into()));
            }
            self.cv.notify_all();
        }
    }

    /// How long a capture waits for the shell to answer (tests shorten it).
    pub fn set_wait_ms(&self, ms: u64) {
        self.wait_ms.store(ms, Ordering::SeqCst);
    }

    pub fn present(&self) -> bool {
        self.present.load(Ordering::SeqCst)
    }

    /// The person chose "Allow for 5 minutes".
    pub fn grant(&self, user: &str, secs: u64) {
        self.lock().grants.insert(user.to_string(), Instant::now() + Duration::from_secs(secs));
    }

    pub fn granted(&self, user: &str) -> bool {
        let mut g = self.lock();
        match g.grants.get(user) {
            Some(t) if *t > Instant::now() => true,
            Some(_) => {
                g.grants.remove(user);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, user: &str) {
        self.lock().grants.remove(user);
    }

    /// The tool side: ask for a picture and wait for the shell.
    pub fn request(&self, user: &str, timeout: Duration) -> Result<Frame, String> {
        if !self.present() {
            return Err("there is no screen to capture here (no IYAKKI window or display is running)".into());
        }
        let id = {
            let mut g = self.lock();
            g.next += 1;
            let id = g.next;
            g.waiting.push_back((id, user.to_string()));
            id
        };
        let deadline = Instant::now() + timeout;
        let mut g = self.lock();
        loop {
            if let Some(r) = g.done.remove(&id) {
                return r;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                g.waiting.retain(|(i, _)| *i != id);
                g.done.remove(&id);
                return Err("the screen did not answer in time (the display may be busy); try again".into());
            }
            g = self.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// The shell side: is a picture wanted for this person? (cheap; called on every drawing)
    pub fn wanted(&self, user: &str) -> bool {
        let g = self.lock();
        !g.waiting.is_empty() && g.waiting.iter().any(|(_, u)| u == user)
    }

    /// The shell side: answer every waiting request of this person with the frame it just drew.
    pub fn serve(&self, user: &str, frame: impl FnOnce() -> Option<Frame>) -> usize {
        let ids: Vec<u64> = {
            let mut g = self.lock();
            let mine: Vec<u64> = g.waiting.iter().filter(|(_, u)| u == user).map(|(i, _)| *i).collect();
            g.waiting.retain(|(_, u)| u != user);
            mine
        };
        if ids.is_empty() {
            return 0;
        }
        let f = frame();
        let mut g = self.lock();
        for i in &ids {
            g.done.insert(*i, f.clone().ok_or_else(|| "the screen could not be read".to_string()));
        }
        self.cv.notify_all();
        ids.len()
    }
}

/// Take the picture, keep it in the chat's folder and answer in the form the chat draws under the call.
pub fn capture(svc: &crate::services::Services, p: &Policy, user: &str, cwd: &std::path::Path) -> Result<String, String> {
    if p.fs_write.is_empty() {
        return Err("your account may not save files, so the picture cannot be kept for the chat".into());
    }
    let f = svc.screen.request(user, Duration::from_millis(svc.screen.wait_ms.load(Ordering::SeqCst)))?;
    if f.w == 0 || f.h == 0 || f.rgba.len() != f.w as usize * f.h as usize * 4 {
        return Err("the screen picture was empty or damaged; try again".into());
    }
    let png = crate::browserjobs::png::encode_rgba(f.w, f.h, &f.rgba);
    let stamp = crate::clock::format(&crate::clock::Zone::system(), crate::clock::now_unix(), "%Y%m%d-%H%M%S");
    for n in 0..1000 {
        let file = if n == 0 { format!("screen-{stamp}.png") } else { format!("screen-{stamp}-{}.png", n + 1) };
        let path = crate::scope::resolve(cwd, &file)?;
        crate::scope::check_write(p, user, &path)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut out) => {
                std::io::Write::write_all(&mut out, &png).map_err(|e| e.to_string())?;
                svc.browser.audit(user, "screen.capture", &format!("{}x{}", f.w, f.h));
                return Ok(format!("Your IYAKKI screen ({}x{}) is shown in the chat (saved as {}, {} KB). It was captured as it was; nothing was hidden.\nIMAGE: {}", f.w, f.h, path.display(), png.len().div_ceil(1024), path.display()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("could not find a free file name".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_request_waits_for_the_shell_and_gets_its_frame() {
        let s = Arc::new(ScreenCapture::new());
        assert!(s.request("amy", Duration::from_millis(50)).unwrap_err().contains("no screen"), "not offered without a shell");
        s.set_present(true);
        let t = {
            let s = s.clone();
            std::thread::spawn(move || s.request("amy", Duration::from_secs(5)))
        };
        let mut served = 0;
        for _ in 0..200 {
            if s.wanted("amy") {
                assert!(!s.wanted("bob"), "only the person's own request");
                served = s.serve("amy", || Some(Frame { w: 2, h: 1, rgba: vec![1, 2, 3, 255, 4, 5, 6, 255] }));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(served, 1);
        let f = t.join().unwrap().unwrap();
        assert_eq!((f.w, f.h, f.rgba.len()), (2, 1, 8));
        assert!(!s.wanted("amy"));
    }

    #[test]
    fn nobody_answering_is_a_clear_error_and_a_frame_failure_too() {
        let s = ScreenCapture::new();
        s.set_present(true);
        assert!(s.request("amy", Duration::from_millis(60)).unwrap_err().contains("did not answer"));
        assert!(!s.wanted("amy"), "the abandoned request is gone");
        let s = Arc::new(s);
        let t = {
            let s = s.clone();
            std::thread::spawn(move || s.request("amy", Duration::from_secs(5)))
        };
        while !s.wanted("amy") {
            std::thread::sleep(Duration::from_millis(2));
        }
        s.serve("amy", || None);
        assert!(t.join().unwrap().unwrap_err().contains("could not be read"));
    }

    #[test]
    fn the_five_minute_grant_expires_and_can_be_revoked() {
        let s = ScreenCapture::new();
        assert!(!s.granted("amy"));
        s.grant("amy", 300);
        assert!(s.granted("amy") && !s.granted("bob"));
        s.revoke("amy");
        assert!(!s.granted("amy"));
        s.grant("amy", 0);
        std::thread::sleep(Duration::from_millis(5));
        assert!(!s.granted("amy"));
    }
}
