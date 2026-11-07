//! A pure-Rust stand-in for the real helper, speaking the whole protocol (docs/WEBVIEW-PROTOCOL.md) without any web engine.
//! It draws a coloured test pattern (four coloured quarters), a white bar that moves across it, and a magenta square at the
//! mouse position, reports every message it gets as a `Log` line (`got <message>`), and answers a few special addresses:
//!
//! * `fake://crash` exits at once (a crashing engine); `fake://hang` stops answering,
//! * `fake://download/NAME` reports a download of `https://example.org/files/NAME`,
//! * `fake://report/ADDRESS` reports a navigation event for exactly ADDRESS (may be empty, `about:blank`, `data:...`),
//! * `fake://goto/HOST` reports a top-level navigation to `https://HOST/landed` (a redirect the shell must vet),
//! * `fake://clipboard` reports a copy; any other address becomes the current page (title `fake page ADDRESS`).
//!
//! `Script` (protocol 1.1) is answered with canned replies: `JSON:x` -> x, `THROW:x` -> failure x, `HANG...` -> no answer.
//!
//! Stress addresses: `fake://crash-mid-frame`, `fake://badframes` (pictures that do not fit), `fake://spam` (frames without acks),
//! `fake://noisy-stderr` (binary and endless stderr); a script starting with `EXIT` kills the helper.
//!
//! A press in the top left 100x100 picture pixels focuses a "text box", the 100x100 next to it a "password box", anywhere else
//! blurs (the shell's focus probe is answered with that).
//!
//! Options: `--static` draws no animation (tests that look at exact pixels), `--fps N` (default 30).

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use iyakki_webview::proto::{self, FromHelper, Framer, Hello, MouseKind, Rect, ToHelper};

const BAR_W: u32 = 16;

struct Fake {
    stream: UnixStream,
    shm: std::fs::File,
    hello: Hello,
    w: u32,
    h: u32,
    img: Vec<u8>,
    bar: u32,
    mouse: (i32, i32),
    history: Vec<String>,
    idx: usize,
    busy: [bool; 2],
    /// What has the focus after the last press: 0 nothing, 1 a text box (top left 100x100), 2 a password box (the 100x100 next to it).
    input: u8,
    next_slot: u8,
    dirty: Option<Rect>,
    visible: bool,
    animate: bool,
    hung: bool,
    fs: bool,
    /// The page is playing audio (`fake://audio`).
    audio: bool,
    spam: u32,
    /// The kinds of the open permission request (`fake://media/...`), and what the "page" captures after an allow.
    asked: u8,
    capturing: u8,
}

fn paint(img: &mut [u8], w: u32, h: u32, r: Rect, bar: u32, mouse: (i32, i32)) {
    let r = r.clip(w, h);
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let (b, g, rr) = match (x < w / 2, y < h / 2) {
                (true, true) => (40, 40, 220),
                (false, true) => (60, 200, 40),
                (true, false) => (230, 80, 50),
                (false, false) => (60, 220, 240),
            };
            // a light checker so scaling or offsets show
            let shade = if ((x / 8) + (y / 8)) % 2 == 0 { 0 } else { 18 };
            let mut px = [b + shade, g + shade, rr.min(255 - shade) + shade, 255];
            if x >= bar && x < bar + BAR_W {
                px = [255, 255, 255, 255];
            }
            let (mx, my) = mouse;
            if (x as i32 - mx).abs() <= 3 && (y as i32 - my).abs() <= 3 {
                px = [255, 0, 255, 255];
            }
            let o = ((y * w + x) * 4) as usize;
            img[o..o + 4].copy_from_slice(&px);
        }
    }
}

impl Fake {
    fn send(&mut self, m: FromHelper) {
        let _ = self.stream.write_all(&m.encode());
    }

    fn log(&mut self, s: String) {
        self.send(FromHelper::Log(s));
    }

    fn damage(&mut self, r: Rect) {
        self.dirty = Some(self.dirty.map_or(r, |d| d.union(r)));
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.w = w.clamp(1, self.hello.max_w);
        self.h = h.clamp(1, self.hello.max_h);
        self.img = vec![0; (self.w * self.h * 4) as usize];
        self.bar = self.bar.min(self.w.saturating_sub(BAR_W));
        paint(&mut self.img, self.w, self.h, Rect::new(0, 0, self.w, self.h), self.bar, self.mouse);
        // a part that was waiting to be sent belongs to the old size: the whole new picture replaces it
        self.dirty = None;
        self.damage(Rect::new(0, 0, self.w, self.h));
    }

    fn navigate_to(&mut self, url: &str, push: bool) {
        if push {
            self.history.truncate(self.idx + 1);
            self.history.push(url.to_string());
            self.idx = self.history.len() - 1;
        }
        if std::mem::take(&mut self.fs) {
            self.send(FromHelper::Fullscreen(false)); // a navigation ends fullscreen, as in the engine
        }
        if std::mem::take(&mut self.capturing) != 0 {
            self.send(FromHelper::CaptureState(0)); // and stops the page's camera, microphone and screen share
        }
        if std::mem::take(&mut self.audio) {
            self.send(FromHelper::AudioPlaying(false)); // and stops the page's sound
        }
        self.send(FromHelper::Loading { loading: true, progress: 30 });
        self.send(FromHelper::Url(url.to_string()));
        self.send(FromHelper::Title(format!("fake page {url}")));
        self.send(FromHelper::Loading { loading: false, progress: 100 });
        self.send_history();
    }

    fn send_history(&mut self) {
        let (b, f) = (self.idx > 0, self.idx + 1 < self.history.len());
        self.send(FromHelper::History { can_back: b, can_forward: f });
    }

    fn handle(&mut self, m: ToHelper) -> bool {
        if self.hung {
            return true;
        }
        self.log(format!("got {m:?}"));
        match m {
            ToHelper::Hello(_) => {}
            ToHelper::Navigate(url) => {
                if let Some(name) = url.strip_prefix("fake://download/") {
                    self.send(FromHelper::Download { url: format!("https://example.org/files/{name}"), name: name.to_string() });
                } else if let Some(raw) = url.strip_prefix("fake://report/") {
                    // a navigation event with exactly this address, the page itself unchanged (about:blank, data:, blob:, empty ...)
                    self.send(FromHelper::Url(raw.to_string()));
                } else if let Some(host) = url.strip_prefix("fake://goto/") {
                    let u = format!("https://{host}/landed");
                    self.navigate_to(&u, true);
                } else if url == "fake://crash" {
                    eprintln!("fake helper: crashing on request");
                    std::process::exit(3);
                } else if url == "fake://hang" {
                    self.hung = true;
                } else if url == "fake://crash-mid-frame" {
                    // a picture announced, then the helper dies before anything else
                    let m = FromHelper::Frame { slot: 0, w: self.w, h: self.h, rects: vec![Rect::new(0, 0, self.w, self.h)] };
                    self.send(m);
                    std::process::exit(4);
                } else if url == "fake://badframes" {
                    // pictures that do not fit the shared file or the announced size
                    for (slot, w, h, rects) in [(0u8, 0u32, 0u32, vec![]), (9, 10, 10, vec![]), (0, u32::MAX, u32::MAX, vec![]), (1, self.hello.max_w + 1, 1, vec![]), (0, 50, 50, vec![Rect::new(40, 40, 500, 500), Rect::new(u32::MAX, u32::MAX, u32::MAX, u32::MAX)]), (1, self.w + 7, self.h + 3, vec![Rect::new(0, 0, 5, 5)])] {
                        self.send(FromHelper::Frame { slot, w, h, rects });
                    }
                } else if url == "fake://spam" {
                    self.spam = 3000;
                } else if url == "fake://noisy-stderr" {
                    let mut e = std::io::stderr();
                    for i in 0..2000 {
                        let _ = e.write_all(b"\xff\xfe not utf-8 \n");
                        let _ = e.write_all(format!("line {i} {}\n", "x".repeat(300)).as_bytes());
                    }
                } else if url == "fake://fullscreen" {
                    // the page asks for fullscreen (a player's button); it stays until the shell or the page leaves
                    self.fs = true;
                    self.send(FromHelper::Fullscreen(true));
                } else if url == "fake://audio" {
                    // the page starts playing audio (a video element), as `notify::is-playing-audio` would say
                    self.navigate_to(&url, true);
                    self.audio = true;
                    self.send(FromHelper::AudioPlaying(true));
                } else if url == "fake://needs-av" {
                    // a page whose H.264 video cannot be decoded here (the web engine's media probe reports it)
                    self.navigate_to(&url, true);
                    self.send(FromHelper::MediaUnsupported("video".into()));
                } else if url == "fake://dev" {
                    // a page with a failed request, a console error and page timings, as the injected dev probe reports them
                    self.navigate_to(&url, true);
                    for l in ["F\u{1f}GET\u{1f}404\u{1f}120\u{1f}https://a.org/missing.js?token=abc\u{1f}", "C\u{1f}error\u{1f}12\u{1f}3\u{1f}https://a.org/app.js\u{1f}TypeError: x is undefined", "P\u{1f}80\u{1f}300\u{1f}900\u{1f}https://a.org/\u{1f}"] {
                        self.send(FromHelper::DevEvent(l.into()));
                    }
                } else if let Some(rest) = url.strip_prefix("fake://media/") {
                    // a page that asks for devices: `fake://media/KINDS/ORIGIN-HOST` (KINDS = media_kind bits); the page
                    // is then "https://ORIGIN-HOST/"; the answer arrives as ToHelper::PermissionAnswer and is logged.
                    let (k, host) = rest.split_once('/').unwrap_or((rest, "meet.example"));
                    let u = format!("https://{host}/");
                    self.navigate_to(&u, true);
                    self.asked = k.parse().unwrap_or(1);
                    self.send(FromHelper::PermissionRequest { id: 1, kinds: self.asked, origin: u });
                } else if let Some(k) = url.strip_prefix("fake://capture/") {
                    self.capturing = k.parse().unwrap_or(0);
                    self.send(FromHelper::CaptureState(self.capturing));
                } else if url == "fake://clipboard" {
                    self.send(FromHelper::Clipboard("copied text".into()));
                } else {
                    self.navigate_to(&url, true);
                }
            }
            ToHelper::Reload { .. } => {
                if let Some(u) = self.history.get(self.idx).cloned() {
                    self.navigate_to(&u, false);
                }
            }
            ToHelper::Stop => self.send(FromHelper::Loading { loading: false, progress: 100 }),
            ToHelper::Back if self.idx > 0 => {
                self.idx -= 1;
                let u = self.history[self.idx].clone();
                self.navigate_to(&u, false);
            }
            ToHelper::Forward if self.idx + 1 < self.history.len() => {
                self.idx += 1;
                let u = self.history[self.idx].clone();
                self.navigate_to(&u, false);
            }
            ToHelper::Back | ToHelper::Forward => {}
            ToHelper::Resize { w, h } => {
                self.resize(w, h);
                self.send(FromHelper::Title(format!("size {}x{}", self.w, self.h)));
            }
            ToHelper::Mouse { kind, x, y, buttons, .. } => {
                if matches!(kind, MouseKind::Down) {
                    self.input = if (0..100).contains(&y) && (0..100).contains(&x) { 1 } else if (0..100).contains(&y) && (100..200).contains(&x) { 2 } else { 0 };
                }
                let old = self.mouse;
                self.mouse = (x, y);
                let pad = |p: (i32, i32)| if p.0 < -4 || p.1 < -4 { Rect::new(0, 0, 0, 0) } else { Rect::new((p.0 - 4).max(0) as u32, (p.1 - 4).max(0) as u32, 9, 9) };
                let (a, b) = (pad(old), pad(self.mouse));
                for r in [a, b].into_iter().filter(|r| !r.is_empty()) {
                    paint(&mut self.img, self.w, self.h, r, self.bar, self.mouse);
                    self.damage(r.clip(self.w, self.h));
                }
                self.send(FromHelper::Title(format!("mouse {x},{y} {} buttons {buttons}", match kind { MouseKind::Move => "move", MouseKind::Down => "down", MouseKind::Up => "up" })));
                self.send(FromHelper::Cursor(if x < 50 { proto::cursor::POINTER } else { proto::cursor::DEFAULT }));
            }
            ToHelper::Wheel { x, y, dx, dy, .. } => self.send(FromHelper::Title(format!("wheel {x},{y} {dx},{dy}"))),
            ToHelper::Key { down, keysym, mods, text } => self.send(FromHelper::Title(format!("key {} {keysym:#x} mods {mods} text {text:?}", if down { "down" } else { "up" }))),
            ToHelper::Text(t) => self.send(FromHelper::Title(format!("text {t:?}"))),
            ToHelper::Zoom(z) => self.send(FromHelper::Title(format!("zoom {z}"))),
            ToHelper::Focus(f) => self.send(FromHelper::Title(format!("focus {f}"))),
            ToHelper::SetVisible(v) => {
                self.visible = v;
                if v {
                    self.damage(Rect::new(0, 0, self.w, self.h));
                }
            }
            ToHelper::Ack(s) => {
                if let Some(b) = self.busy.get_mut(s as usize) {
                    *b = false;
                }
            }
            ToHelper::Script { id, js, .. } => {
                // canned, scriptable replies: "JSON:<text>" answers <text>; "THROW:<text>" fails with <text>; anything else answers its
                // own first 60 characters as a JSON string, so tests can see that the script arrived whole.
                if let Some(t) = js.strip_prefix("JSON:") {
                    self.send(FromHelper::ScriptResult { id, ok: true, json: t.to_string() });
                } else if let Some(t) = js.strip_prefix("THROW:") {
                    self.send(FromHelper::ScriptResult { id, ok: false, json: t.to_string() });
                } else if js.contains("document.activeElement") {
                    // the shell's probe of the focused element: \"t\" a text box, \"p\" a password box, empty anything else
                    let a = ["", "t", "p"][self.input as usize];
                    self.send(FromHelper::ScriptResult { id, ok: true, json: format!("{a:?}") });
                } else if js.starts_with("EXIT") {
                    eprintln!("fake helper: exiting during a script");
                    std::process::exit(5);
                } else if js.starts_with("HANG") {
                    // never answers: the shell's own timeout must fire
                } else {
                    let head: String = js.chars().take(60).collect();
                    self.send(FromHelper::ScriptResult { id, ok: true, json: format!("{:?}", head) });
                }
            }
            ToHelper::LeaveFullscreen => {
                if std::mem::take(&mut self.fs) {
                    self.send(FromHelper::Fullscreen(false));
                }
            }
            ToHelper::PermissionAnswer { allow, .. } => {
                // already logged as `got PermissionAnswer {..}`; an allow starts the capture the page asked for, as the engine would
                let bits = std::mem::take(&mut self.asked) & 7;
                if allow && bits != 0 {
                    self.capturing |= bits;
                    self.send(FromHelper::CaptureState(self.capturing));
                }
            }
            ToHelper::StopCapture { kinds } => {
                if self.capturing & kinds != 0 {
                    self.capturing &= !kinds;
                    self.send(FromHelper::CaptureState(self.capturing));
                }
            }
            ToHelper::ScreenSource(_) => {} // logged only
            ToHelper::Quit => return false,
        }
        true
    }

    /// Send the damaged part if a slot is free.
    fn flush(&mut self) {
        let Some(r) = self.dirty.map(|r| r.clip(self.w, self.h)) else { return };
        if r.is_empty() {
            self.dirty = None;
            return;
        }
        if !self.visible || self.hung {
            return;
        }
        let slot = if !self.busy[self.next_slot as usize] { self.next_slot } else if !self.busy[1 - self.next_slot as usize] { 1 - self.next_slot } else { return };
        let base = slot as u64 * self.hello.max_w as u64 * self.hello.max_h as u64 * 4;
        let stride = (self.w * 4) as usize;
        for row in r.y..r.y + r.h {
            let o = row as usize * stride + r.x as usize * 4;
            let _ = self.shm.write_all_at(&self.img[o..o + r.w as usize * 4], base + o as u64);
        }
        self.busy[slot as usize] = true;
        self.next_slot = 1 - slot;
        self.dirty = None;
        let m = FromHelper::Frame { slot, w: self.w, h: self.h, rects: vec![r] };
        self.send(m);
    }

    fn step_bar(&mut self) {
        let old = self.bar;
        self.bar = if self.bar + BAR_W + 6 >= self.w { 0 } else { self.bar + 6 };
        let r = Rect::new(old, 0, self.bar.max(old) + BAR_W - old.min(self.bar), self.h).clip(self.w, self.h);
        paint(&mut self.img, self.w, self.h, r, self.bar, self.mouse);
        self.damage(r);
    }
}

fn main() {
    let mut fd = 0i32;
    let mut shm_path = String::new();
    let mut animate = true;
    let mut fps = 30u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket-fd" => fd = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--shm" => shm_path = args.next().unwrap_or_default(),
            "--static" => animate = false,
            "--fps" => fps = args.next().and_then(|v| v.parse().ok()).unwrap_or(30).max(1),
            _ => {}
        }
    }
    // SAFETY: the shell passes a socket as this file descriptor (documented in WEBVIEW-PROTOCOL.md) and nothing else owns it.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
    let shm = OpenOptions::new().read(true).write(true).open(&shm_path).unwrap_or_else(|e| {
        eprintln!("fake helper: cannot open {shm_path}: {e}");
        std::process::exit(2)
    });
    let mut framer = Framer::default();
    let mut buf = [0u8; 16384];
    // the first message must be Hello
    let hello = loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => framer.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        if let Ok(Some((ty, p))) = framer.next() {
            match ToHelper::decode(ty, &p) {
                Ok(ToHelper::Hello(h)) => break h,
                _ => {
                    eprintln!("fake helper: the first message was not Hello");
                    std::process::exit(2);
                }
            }
        }
    };
    let mut f = Fake { stream: stream.try_clone().unwrap(), shm, w: 0, h: 0, img: Vec::new(), bar: 0, mouse: (-100, -100), history: Vec::new(), idx: 0, busy: [false; 2], input: 0, next_slot: 0, dirty: None, visible: true, animate, hung: false, fs: false, audio: false, spam: 0, asked: 0, capturing: 0, hello: hello.clone() };
    f.send(FromHelper::Ready { version: proto::VERSION, engine: "fake".into(), engine_version: "1".into() });
    f.resize(hello.w, hello.h);
    f.log(format!("got Hello {hello:?}"));
    let mut last_step = Instant::now();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => framer.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        loop {
            match framer.next() {
                Ok(Some((ty, p))) => match ToHelper::decode(ty, &p) {
                    Ok(m) => {
                        if !f.handle(m) {
                            return;
                        }
                    }
                    Err(e) => f.log(format!("bad message: {e}")),
                },
                Ok(None) => break,
                Err(e) => {
                    eprintln!("fake helper: {e}");
                    return;
                }
            }
        }
        if f.animate && f.visible && !f.hung && last_step.elapsed() >= Duration::from_millis(1000 / fps) {
            last_step = Instant::now();
            f.step_bar();
        }
        if f.spam > 0 {
            // frames without waiting for acknowledgements, as fast as the socket takes them
            for _ in 0..200 {
                let m = FromHelper::Frame { slot: (f.spam % 2) as u8, w: f.w, h: f.h, rects: vec![Rect::new(0, 0, f.w.min(8), f.h.min(8))] };
                f.send(m);
            }
            f.spam = f.spam.saturating_sub(200);
        }
        f.flush();
    }
}
