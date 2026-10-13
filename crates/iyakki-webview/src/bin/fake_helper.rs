//! A pure-Rust stand-in for the real helper, speaking the whole protocol (docs/WEBVIEW-PROTOCOL.md) without any web engine.
//! It draws a coloured test pattern (four coloured quarters), a white bar that moves across it, and a magenta square at the
//! mouse position, reports every message it gets as a `Log` line (`got <message>`), and answers a few special addresses:
//!
//! * `fake://crash` exits at once (a crashing engine); `fake://hang` stops answering,
//! * `fake://download/NAME` reports a download of `https://example.org/files/NAME`,
//! * `fake://goto/HOST` reports a top-level navigation to `https://HOST/landed` (a redirect the shell must vet),
//! * `fake://clipboard` reports a copy; any other address becomes the current page (title `fake page ADDRESS`).
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
    next_slot: u8,
    dirty: Option<Rect>,
    visible: bool,
    animate: bool,
    hung: bool,
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
        self.damage(Rect::new(0, 0, self.w, self.h));
    }

    fn navigate_to(&mut self, url: &str, push: bool) {
        if push {
            self.history.truncate(self.idx + 1);
            self.history.push(url.to_string());
            self.idx = self.history.len() - 1;
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
                } else if let Some(host) = url.strip_prefix("fake://goto/") {
                    let u = format!("https://{host}/landed");
                    self.navigate_to(&u, true);
                } else if url == "fake://crash" {
                    eprintln!("fake helper: crashing on request");
                    std::process::exit(3);
                } else if url == "fake://hang" {
                    self.hung = true;
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
            ToHelper::Quit => return false,
        }
        true
    }

    /// Send the damaged part if a slot is free.
    fn flush(&mut self) {
        let Some(r) = self.dirty else { return };
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
    let mut f = Fake { stream: stream.try_clone().unwrap(), shm, w: 0, h: 0, img: Vec::new(), bar: 0, mouse: (-100, -100), history: Vec::new(), idx: 0, busy: [false; 2], next_slot: 0, dirty: None, visible: true, animate, hung: false, hello: hello.clone() };
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
        f.flush();
    }
}
