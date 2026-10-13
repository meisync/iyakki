//! The shell as something a browser can look at: one shell per signed-in person, drawn into a picture of the size of their
//! window, with only the changed part of the picture sent when something changes.

use crate::input::Input;
use iyakki_shell::accounts::AccountError;
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::Event;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A piece of the picture, as a PNG, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub png: Vec<u8>,
}

impl Frame {
    /// The message sent to the page: four little-endian u16 (x, y, width, height) and then the PNG.
    pub fn to_message(&self) -> Vec<u8> {
        let mut m = Vec::with_capacity(8 + self.png.len());
        for v in [self.x, self.y, self.w, self.h] {
            m.extend_from_slice(&(v as u16).to_le_bytes());
        }
        m.extend_from_slice(&self.png);
        m
    }
}

/// What the web server needs from a person's screen (a fake one stands in for it in the server's tests).
pub trait Screen: Send {
    fn user(&self) -> String;
    fn input(&mut self, i: Input);
    /// Someone is (or no longer is) looking: the picture is only drawn and sent while they are.
    fn viewing(&mut self, on: bool);
    /// Do whatever is due (the shell's tick, drawing) and return the part of the picture that changed.
    fn step(&mut self) -> Option<Frame>;
    /// Text the person copied inside IYAKKI, for the page to put on their clipboard.
    fn take_copy(&mut self) -> Option<String>;
    /// True once when the shell wants the text on the person's clipboard (the context menu's Paste, the clipboard history).
    fn take_clip_request(&mut self) -> bool {
        false
    }
    /// The mouse pointer's CSS name when it changed since last asked (`pointer`, `text`, `default`...).
    fn take_cursor(&mut self) -> Option<&'static str> {
        None
    }
    /// The person signed out.
    fn finished(&self) -> bool;
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    /// Wrong name or password (one answer for both).
    Invalid,
    /// Too many wrong tries; seconds to wait.
    Throttled(u64),
    Other(String),
}

/// Checks a person's name and password and hands over their screen.
pub trait Gate: Send + Sync {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError>;
}

pub struct ShellGate {
    env: ShellEnv,
}

impl ShellGate {
    pub fn new(env: ShellEnv) -> ShellGate {
        ShellGate { env }
    }
}

impl Gate for ShellGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        let mut store = self.env.store.lock().map_err(|_| LoginError::Other("accounts are busy".into()))?;
        store.refresh();
        if store.is_empty() {
            return Err(LoginError::Other("This machine has no accounts yet. On the machine run: iyakki user add NAME --role admin".into()));
        }
        let r = store.authenticate(user.trim(), password);
        drop(store);
        match r {
            Ok(session) => {
                self.env.audit.record(&session.user, "login.web", session.role.name());
                let screen = ShellScreen::spawn(self.env.share(), session, password.to_string()).map_err(|e| LoginError::Other(e.to_string()))?;
                Ok(Box::new(screen))
            }
            Err(AccountError::Throttled(s)) => Err(LoginError::Throttled(s)),
            Err(AccountError::InvalidCredentials) => {
                let shown: String = user.chars().filter(|c| c.is_ascii_graphic()).take(32).collect();
                self.env.audit.record("-", "login.failed", &format!("web {shown}"));
                Err(LoginError::Invalid)
            }
            Err(e) => Err(LoginError::Other(e.to_string())),
        }
    }
}

enum Cmd {
    In(Input),
    Viewing(bool),
}

enum Out {
    Frame(Frame),
    Copy(String),
    Cursor(&'static str),
    /// The shell wants the text on the person's clipboard (the page reads it and sends it back as a paste).
    ReadClip,
}

/// A person's shell, living on a thread of its own (a shell is not something that can move between threads) and kept
/// running while nobody looks, so alarms and background work carry on.
pub struct ShellScreen {
    tx: std::sync::mpsc::Sender<Cmd>,
    rx: std::sync::mpsc::Receiver<Out>,
    user: String,
    done: Arc<AtomicBool>,
    copied: Option<String>,
    cursor: Option<&'static str>,
    read_clip: bool,
}

impl ShellScreen {
    pub fn spawn(env: ShellEnv, session: iyakki_shell::accounts::Session, password: String) -> std::io::Result<ShellScreen> {
        let (tx, cmds) = std::sync::mpsc::channel::<Cmd>();
        let (out, rx) = std::sync::mpsc::sync_channel::<Out>(4);
        let done = Arc::new(AtomicBool::new(false));
        let user = session.user.clone();
        let flag = done.clone();
        // the shell and the renderer are stack hungry, as on the console
        std::thread::Builder::new().name("iyakki-web-shell".into()).stack_size(32 * 1024 * 1024).spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut shell = Shell::new(env);
                shell.sign_in_checked(session, &password);
                drop(password);
                shell_loop(shell, cmds, out, &flag);
            }));
            if run.is_err() {
                log::error!("web: a person's shell crashed; their session ends");
            }
            flag.store(true, Ordering::Relaxed);
        })?;
        Ok(ShellScreen { tx, rx, user, done, copied: None, cursor: None, read_clip: false })
    }
}

/// The CSS cursor name for a pointer shape.
fn css_cursor(p: iyakki_shell::ui::Pointer) -> &'static str {
    use iyakki_shell::ui::Pointer;
    match p {
        Pointer::Default => "default",
        Pointer::Hand => "pointer",
        Pointer::Text => "text",
        Pointer::Wait => "progress",
        Pointer::Crosshair => "crosshair",
        Pointer::Move => "move",
        Pointer::NotAllowed => "not-allowed",
        Pointer::ResizeH => "ew-resize",
        Pointer::ResizeV => "ns-resize",
        Pointer::Grab => "grab",
        Pointer::Help => "help",
        Pointer::Hidden => "none",
    }
}

fn shell_loop(mut shell: Shell, cmds: std::sync::mpsc::Receiver<Cmd>, out: std::sync::mpsc::SyncSender<Out>, done: &AtomicBool) {
    let mut gfx = Gfx::new(1280, 720);
    let mut last: Vec<u8> = Vec::new();
    let (mut need_full, mut dirty, mut viewing) = (true, true, false);
    let (mut last_tick, mut last_render) = (Instant::now(), Instant::now());
    let mut last_cursor = "default";
    loop {
        let wait = Duration::from_millis(33).saturating_sub(last_tick.elapsed()).max(Duration::from_millis(1));
        let mut next = match cmds.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        while let Some(c) = next {
            match c {
                Cmd::In(Input::Event(e)) => {
                    shell.handle(&e);
                    dirty |= shell.take_stale();
                }
                Cmd::In(Input::Size(w, h)) => {
                    if (w, h) != (gfx.width(), gfx.height()) {
                        gfx.resize(w, h);
                        last.clear();
                    }
                    need_full = true;
                    dirty = true;
                }
                Cmd::In(Input::Shift(on)) => iyakki_shell::ui::set_shift_held(on),
                Cmd::In(Input::Full) => {
                    need_full = true;
                    dirty = true;
                }
                Cmd::Viewing(v) => {
                    viewing = v;
                    need_full |= v;
                    dirty |= v;
                }
            }
            next = cmds.try_recv().ok();
        }
        if last_tick.elapsed() >= Duration::from_millis(33) {
            last_tick = Instant::now();
            shell.handle(&Event::Tick);
            // an app with live pictures (the browser's full engine) asks for a redraw on the heartbeat
            dirty |= shell.take_stale();
        }
        if shell.session().is_none() {
            break;
        }
        if viewing {
            let c = css_cursor(shell.pointer());
            if c != last_cursor && out.try_send(Out::Cursor(c)).is_ok() {
                last_cursor = c;
            }
            if shell.take_host_paste_request() {
                let _ = out.try_send(Out::ReadClip);
            }
            if let Some(t) = shell.take_host_copy() {
                if out.try_send(Out::Copy(t)).is_err() {
                    log::debug!("web: a copied text was dropped (the page is slow)");
                }
            }
            if dirty || last_render.elapsed() >= Duration::from_millis(150) {
                dirty = false;
                last_render = Instant::now();
                shell.render(&mut gfx);
                let (w, h) = (gfx.width() as usize, gfx.height() as usize);
                let rect = if need_full { Some((0, 0, w, h)) } else { changed_rect(&last, gfx.rgba(), w, h) };
                if let Some(rect) = rect {
                    if let Some(f) = encode_region(gfx.rgba(), w, rect) {
                        match out.try_send(Out::Frame(f)) {
                            Ok(()) => {
                                need_full = false;
                                last = gfx.rgba().to_vec();
                            }
                            // the page has not caught up: send everything fresh once it has
                            Err(_) => {
                                need_full = true;
                                dirty = true;
                            }
                        }
                    }
                }
            }
        }
    }
    done.store(true, Ordering::Relaxed);
}

/// The smallest rectangle holding every pixel that differs, or `None` when the pictures are the same.
pub fn changed_rect(old: &[u8], new: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    if old.len() != new.len() || new.len() != w * h * 4 {
        return Some((0, 0, w, h));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        let (a, b) = (&old[y * w * 4..(y + 1) * w * 4], &new[y * w * 4..(y + 1) * w * 4]);
        if a == b {
            continue;
        }
        let first = a.chunks(4).zip(b.chunks(4)).position(|(p, q)| p != q).unwrap_or(0);
        let last = a.chunks(4).zip(b.chunks(4)).rposition(|(p, q)| p != q).unwrap_or(first);
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y);
        y1 = y1.max(y + 1);
    }
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

pub fn encode_region(data: &[u8], w: usize, rect: (usize, usize, usize, usize)) -> Option<Frame> {
    let (x, y, rw, rh) = rect;
    let mut part = Vec::with_capacity(rw * rh * 4);
    for row in y..y + rh {
        part.extend_from_slice(&data[(row * w + x) * 4..(row * w + x + rw) * 4]);
    }
    let pm = tiny_skia::Pixmap::from_vec(part, tiny_skia::IntSize::from_wh(rw as u32, rh as u32)?)?;
    Some(Frame { x: x as u32, y: y as u32, w: rw as u32, h: rh as u32, png: pm.encode_png().ok()? })
}

impl Screen for ShellScreen {
    fn user(&self) -> String {
        self.user.clone()
    }

    fn input(&mut self, i: Input) {
        let _ = self.tx.send(Cmd::In(i));
    }

    fn viewing(&mut self, on: bool) {
        let _ = self.tx.send(Cmd::Viewing(on));
        if !on {
            while self.rx.try_recv().is_ok() {}
        }
    }

    fn step(&mut self) -> Option<Frame> {
        // wait a moment for the next piece so the server's loop is not a busy one
        match self.rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Out::Frame(f)) => Some(f),
            Ok(Out::Copy(t)) => {
                self.copied = Some(t);
                None
            }
            Ok(Out::Cursor(c)) => {
                self.cursor = Some(c);
                None
            }
            Ok(Out::ReadClip) => {
                self.read_clip = true;
                None
            }
            Err(_) => None,
        }
    }

    fn take_copy(&mut self) -> Option<String> {
        self.copied.take()
    }

    fn take_cursor(&mut self) -> Option<&'static str> {
        self.cursor.take()
    }

    fn take_clip_request(&mut self) -> bool {
        std::mem::take(&mut self.read_clip)
    }

    fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_changed_part_of_the_picture_is_found() {
        let (w, h) = (10, 8);
        let a = vec![0u8; w * h * 4];
        assert_eq!(changed_rect(&a, &a, w, h), None);
        let mut b = a.clone();
        for (x, y) in [(2usize, 3usize), (6, 5)] {
            b[(y * w + x) * 4] = 255;
        }
        assert_eq!(changed_rect(&a, &b, w, h), Some((2, 3, 5, 3)));
        assert_eq!(changed_rect(&[], &b, w, h), Some((0, 0, w, h)), "no earlier picture means all of it");
    }

    #[test]
    fn a_region_is_cut_out_and_encoded_as_png() {
        let (w, h) = (6usize, 4usize);
        let mut d = vec![0u8; w * h * 4];
        for i in 0..w * h {
            d[i * 4 + 3] = 255;
            d[i * 4] = (i * 9) as u8;
        }
        let f = encode_region(&d, w, (1, 1, 3, 2)).expect("frame");
        assert_eq!((f.x, f.y, f.w, f.h), (1, 1, 3, 2));
        assert_eq!(&f.png[..8], b"\x89PNG\r\n\x1a\n");
        let msg = f.to_message();
        assert_eq!(&msg[..8], &[1, 0, 1, 0, 3, 0, 2, 0]);
        assert_eq!(&msg[8..], &f.png[..]);
    }
}
