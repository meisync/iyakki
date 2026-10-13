//! The browser's embedded full web engine: one helper program per tab renders the page offscreen, this file talks to it
//! (`iyakki-webview`, protocol in `docs/WEBVIEW-PROTOCOL.md`), keeps the latest picture, draws it in the page area and turns the
//! shell's keys, mouse and wheel into protocol messages. The browser (`browser.rs`) owns the chrome (tabs, address bar, buttons)
//! and decides what to do with the notes this view leaves ([`Note`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iyakki_webview::keys;
use iyakki_webview::proto::{self, Hello, MouseKind, Rect, ToHelper};
use iyakki_webview::{Config, Event as WvEvent, WebView};
use tiny_skia::Pixmap;

use crate::gfx::Gfx;
use crate::ui::{Key, Nav, R};

/// The page area as a hit region: `Event::AreaAt(AREA_FULL, x, y)` is a press at (x, y) inside it.
pub const AREA_FULL: usize = 902;
/// The helper gets this long to say `Ready`.
const START_TIMEOUT: Duration = Duration::from_secs(25);
/// A wheel notch in the protocol (1/120 notch units).
const NOTCH: i32 = 120;

/// What the view found out that the browser must act on (it owns the history, the policy and the downloads).
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// The page address changed (committed); the title may still be the old one.
    Navigated(String),
    Titled(String),
    /// The helper went to a host the account may not open; it has been told to go back.
    Refused { url: String, host: String },
    /// The helper wants to save a file; the shell's download code decides.
    Download { url: String, name: String },
    /// The page copied text.
    Clipboard(String),
    /// The helper program is gone (crashed, killed, failed to start in time): the browser falls back to its own engine.
    Gone(String),
    /// The engine reported a problem but is still running.
    Trouble(String),
}

pub struct Full {
    view: WebView,
    pub engine: String,
    pm: Option<Pixmap>,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub progress: u8,
    pub can_back: bool,
    pub can_forward: bool,
    pub cursor: u8,
    /// Keys go to the page (else the browser's own shortcuts get them).
    pub page_focus: bool,
    /// Where the page area was drawn last (screen pixels), for turning absolute mouse positions into page ones.
    rect: R,
    /// The left button is held down inside the page (a drag), so moves arrive as presses repeated by the shell.
    pub pressed: bool,
    last_mouse: Option<(i32, i32)>,
    redraw: bool,
    /// The last address that was allowed (what to return to after a refused one).
    pub good_url: String,
    /// The last address recorded in the history (so a title arriving later does not count a second visit).
    pub visited: String,
    scale_hello: f32,
    zoom_pct: u32,
    zoom_sent: u32,
    started: Instant,
    ready: bool,
    pub gone: Option<String>,
    notes: Vec<Note>,
    visible: bool,
    pub frames: u64,
    /// The helper's last messages and error output (for diagnosis and tests).
    pub log: std::collections::VecDeque<String>,
}

pub struct Start {
    pub helper: PathBuf,
    pub profile: PathBuf,
    pub env: Option<Vec<(String, String)>>,
    /// The page area in pixels and the shell's UI scale (so a bigger interface gets a bigger page).
    pub size: (u32, u32),
    pub scale: f32,
    /// The browser's zoom for this site (1.0 = normal).
    pub zoom: f32,
    pub allow_hosts: Vec<String>,
    pub user_agent: String,
    pub locale: String,
}

fn scale_pct(s: f32) -> u32 {
    (((s * 4.0).round() / 4.0).clamp(0.5, 4.0) * 100.0).round() as u32
}

impl Full {
    pub fn start(st: Start) -> Result<Full, String> {
        let (w, h) = (st.size.0.max(64), st.size.1.max(64));
        let hello = Hello {
            version: proto::VERSION,
            w,
            h,
            scale_pct: scale_pct(st.scale),
            max_w: w.max(3840),
            max_h: h.max(2160),
            user_agent: st.user_agent,
            profile: st.profile.to_string_lossy().into_owned(),
            locale: st.locale,
            allow_hosts: st.allow_hosts,
        };
        let mut cfg = Config::new(&st.helper, hello);
        cfg.env = st.env;
        cfg.cwd = Some(st.profile.clone());
        let view = WebView::start(cfg)?;
        let mut f = Full {
            view,
            engine: String::new(),
            pm: None,
            title: String::new(),
            url: String::new(),
            loading: true,
            progress: 0,
            can_back: false,
            can_forward: false,
            cursor: proto::cursor::DEFAULT,
            page_focus: true,
            rect: R::new(0.0, 0.0, w as f32, h as f32),
            pressed: false,
            last_mouse: None,
            redraw: true,
            good_url: String::new(),
            visited: String::new(),
            scale_hello: st.scale.max(0.1),
            zoom_pct: (st.zoom * 100.0).round() as u32,
            zoom_sent: 100,
            started: Instant::now(),
            ready: false,
            gone: None,
            notes: Vec::new(),
            visible: true,
            frames: 0,
            log: Default::default(),
        };
        f.send_zoom(st.scale);
        f.view.send(ToHelper::Focus(true));
        Ok(f)
    }

    fn send_zoom(&mut self, scale_now: f32) {
        let pct = ((self.zoom_pct as f32) * scale_now.max(0.1) / self.scale_hello).round().clamp(25.0, 500.0) as u32;
        if pct != self.zoom_sent {
            self.zoom_sent = pct;
            self.view.send(ToHelper::Zoom(pct));
        }
    }

    pub fn send(&mut self, m: ToHelper) -> bool {
        self.view.send(m)
    }

    pub fn navigate(&mut self, url: &str) {
        self.loading = true;
        self.view.send(ToHelper::Navigate(url.to_string()));
    }

    pub fn reload(&mut self) {
        self.loading = true;
        self.view.send(ToHelper::Reload { hard: false });
    }

    pub fn stop(&mut self) {
        self.view.send(ToHelper::Stop);
    }

    pub fn back(&mut self) {
        self.view.send(ToHelper::Back);
    }

    pub fn forward(&mut self) {
        self.view.send(ToHelper::Forward);
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom_pct = (zoom * 100.0).round() as u32;
        let s = self.scale_hello;
        self.send_zoom(s);
    }

    pub fn set_focus(&mut self, on: bool) {
        self.page_focus = on;
        self.view.send(ToHelper::Focus(on));
    }

    /// The tab is shown (or hidden behind another one): a hidden page makes no frames.
    pub fn set_visible(&mut self, on: bool) {
        if on != self.visible {
            self.visible = on;
            self.view.send(ToHelper::SetVisible(on));
            if on {
                self.redraw = true;
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    /// A new picture or state arrived since the last call (the shell then draws again).
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    pub fn frame_size(&self) -> (u32, u32) {
        self.pm.as_ref().map_or((0, 0), |p| (p.width(), p.height()))
    }

    pub fn size_wanted(&self) -> (u32, u32) {
        self.view.size_wanted()
    }

    /// The picture, as the shell's own pixels (RGBA, opaque).
    pub fn pixmap(&self) -> Option<&Pixmap> {
        self.pm.as_ref()
    }

    /// Take in what the helper sent. `allow` says whether the account may open a host: an address outside it is refused here
    /// as well as in the helper (which was given the list), because a redirect or a script can still end up there.
    pub fn pump(&mut self, allow: &dyn Fn(&str) -> bool) {
        if self.gone.is_some() {
            return;
        }
        for ev in self.view.poll() {
            match ev {
                WvEvent::Ready { engine, version } => {
                    self.engine = format!("{engine} {version}");
                    self.ready = true;
                    self.redraw = true;
                }
                WvEvent::Frame => {
                    self.take_frame();
                }
                WvEvent::Title(t) => {
                    self.title = t.clone();
                    self.notes.push(Note::Titled(t));
                    self.redraw = true;
                }
                WvEvent::Url(u) => {
                    if !url_allowed(&u, allow) {
                        let host = iyakki_sec::url_host(&u).unwrap_or_else(|| u.chars().take(60).collect());
                        // go back to where we were; with no earlier page, to a blank one
                        if self.can_back {
                            self.view.send(ToHelper::Back);
                        } else if self.good_url.is_empty() {
                            self.view.send(ToHelper::Navigate("about:blank".into()));
                        } else {
                            self.view.send(ToHelper::Navigate(self.good_url.clone()));
                        }
                        self.notes.push(Note::Refused { url: u, host });
                    } else {
                        if u.starts_with("http") {
                            self.good_url = u.clone();
                        }
                        self.url = u.clone();
                        self.notes.push(Note::Navigated(u));
                    }
                    self.redraw = true;
                }
                WvEvent::Loading { loading, progress } => {
                    self.loading = loading;
                    self.progress = progress;
                    self.redraw = true;
                }
                WvEvent::History { can_back, can_forward } => {
                    self.can_back = can_back;
                    self.can_forward = can_forward;
                    self.redraw = true;
                }
                WvEvent::Cursor(k) => self.cursor = k,
                WvEvent::Download { url, name } => self.notes.push(Note::Download { url, name }),
                WvEvent::Clipboard(t) => self.notes.push(Note::Clipboard(t)),
                WvEvent::Crashed(r) => self.notes.push(Note::Trouble(r)),
                WvEvent::Log(l) => {
                    log::debug!("webview: {l}");
                    if self.log.len() >= 60 {
                        self.log.pop_front();
                    }
                    self.log.push_back(l);
                }
                WvEvent::Exited(why) => {
                    self.gone = Some(why.clone());
                    self.notes.push(Note::Gone(why));
                    self.redraw = true;
                }
            }
        }
        if !self.ready && self.gone.is_none() && self.started.elapsed() > START_TIMEOUT {
            let why = "the web engine did not start in time".to_string();
            self.gone = Some(why.clone());
            self.notes.push(Note::Gone(why));
            self.redraw = true;
        }
    }

    /// Copy the changed part of the helper's picture into ours (BGRA premultiplied to opaque RGBA over white).
    fn take_frame(&mut self) {
        self.frames += 1;
        let (w, h) = self.view.frame_size();
        if w == 0 || h == 0 {
            return;
        }
        if self.pm.as_ref().is_none_or(|p| (p.width(), p.height()) != (w, h)) {
            self.pm = Pixmap::new(w, h);
            self.view.take_dirty();
            // a new size: the whole picture
            self.copy_rect(Rect::new(0, 0, w, h));
            self.redraw = true;
            return;
        }
        if let Some(r) = self.view.take_dirty() {
            self.copy_rect(r);
            self.redraw = true;
        }
    }

    fn copy_rect(&mut self, r: Rect) {
        let Some(pm) = self.pm.as_mut() else { return };
        let (w, h) = (pm.width(), pm.height());
        let r = r.clip(w, h);
        let src = self.view.pixels();
        let dst = pm.data_mut();
        if src.len() < (w * h * 4) as usize {
            return;
        }
        for y in r.y..r.y + r.h {
            let a = ((y * w + r.x) * 4) as usize;
            let b = a + (r.w * 4) as usize;
            for (d, s) in dst[a..b].chunks_exact_mut(4).zip(src[a..b].chunks_exact(4)) {
                let alpha = s[3];
                if alpha == 255 {
                    d.copy_from_slice(&[s[2], s[1], s[0], 255]);
                } else {
                    let rest = 255 - alpha;
                    d.copy_from_slice(&[s[2].saturating_add(rest), s[1].saturating_add(rest), s[0].saturating_add(rest), 255]);
                }
            }
        }
    }

    /// Draw the page into `view` (screen pixels) and tell the helper when the area's size or scale changed.
    pub fn draw(&mut self, g: &mut Gfx, view: R, s: f32) {
        self.rect = view;
        let want = ((view.w.max(64.0)) as u32, (view.h.max(64.0)) as u32);
        if want != self.view.size_wanted() {
            self.view.send(ToHelper::Resize { w: want.0, h: want.1 });
        }
        self.send_zoom(s);
        let (vx, vy) = (view.x.round() as i32, view.y.round() as i32);
        let (vw, vh) = (view.w as i32, view.h as i32);
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        // the page's own white where no picture has arrived yet (or the area grew and the helper has not caught up)
        g.rect(view.x, view.y, view.w, view.h, [255, 255, 255, 255]);
        let Some(pm) = &self.pm else { return };
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let cols = pw.min(vw).min(gw - vx).max(0);
        if cols == 0 {
            return;
        }
        let rows = ph.min(vh);
        let src = pm.data();
        let dst = g.pm.data_mut();
        for y in 0..rows {
            let dy = vy + y;
            if dy < 0 || dy >= gh {
                continue;
            }
            let (sx0, dx0) = if vx < 0 { (-vx, 0) } else { (0, vx) };
            let n = (cols - sx0).max(0);
            if n == 0 {
                continue;
            }
            let a = ((y * pw + sx0) * 4) as usize;
            let b = ((dy * gw + dx0) * 4) as usize;
            dst[b..b + (n * 4) as usize].copy_from_slice(&src[a..a + (n * 4) as usize]);
        }
    }

    // ------------------------------------------------------------------------------------------------- input

    /// An absolute screen position as a page position, when it is inside the page area.
    pub fn to_page(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        let (px, py) = (x - self.rect.x.round() as i32, y - self.rect.y.round() as i32);
        (px >= 0 && py >= 0 && (px as f32) < self.rect.w && (py as f32) < self.rect.h).then_some((px, py))
    }

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        if let Some((px, py)) = self.to_page(x, y) {
            if self.last_mouse != Some((px, py)) {
                self.last_mouse = Some((px, py));
                let buttons = if self.pressed { proto::HELD_LEFT } else { 0 };
                self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x: px, y: py, button: 0, buttons, mods: 0, clicks: 0 });
            }
        } else {
            self.last_mouse = None;
        }
    }

    /// A press at a position relative to the page area (the shell reports presses that way); `clicks` is 1, 2 or 3.
    pub fn mouse_down(&mut self, x: i32, y: i32, clicks: u8) {
        self.last_mouse = Some((x, y));
        self.pressed = true;
        self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: 0, mods: 0, clicks: 0 });
        self.view.send(ToHelper::Mouse { kind: MouseKind::Down, x, y, button: proto::BUTTON_LEFT, buttons: proto::HELD_LEFT, mods: 0, clicks: clicks.clamp(1, 3) });
    }

    /// The shell repeats the press while the button is held and the pointer moves (a drag).
    pub fn mouse_drag(&mut self, x: i32, y: i32) {
        if self.last_mouse != Some((x, y)) {
            self.last_mouse = Some((x, y));
            self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: proto::HELD_LEFT, mods: 0, clicks: 0 });
        }
    }

    pub fn mouse_up(&mut self, x: i32, y: i32) {
        if self.pressed {
            self.pressed = false;
            let (px, py) = (x - self.rect.x.round() as i32, y - self.rect.y.round() as i32);
            self.view.send(ToHelper::Mouse { kind: MouseKind::Up, x: px, y: py, button: proto::BUTTON_LEFT, buttons: 0, mods: 0, clicks: 1 });
        }
    }

    /// A wheel step: `notches` > 0 scrolls up (as the shell's `Event::Scroll`). Goes where the pointer last was in the page.
    pub fn wheel(&mut self, notches: i32) -> bool {
        let Some((x, y)) = self.last_mouse else { return false };
        self.view.send(ToHelper::Wheel { x, y, dx: 0, dy: -notches * NOTCH, mods: 0 });
        true
    }

    pub fn key(&mut self, k: &Key) {
        for m in key_events(k) {
            self.view.send(m);
        }
    }

    pub fn text(&mut self, t: &str) {
        self.view.send(ToHelper::Text(t.to_string()));
    }
}

/// An address the account may open: `about:`, `data:` and `blob:` pages have no host; anything else needs a host the
/// account is allowed.
pub fn url_allowed(url: &str, allow: &dyn Fn(&str) -> bool) -> bool {
    if url.starts_with("about:") || url.starts_with("data:") || url.starts_with("blob:") {
        return true;
    }
    match iyakki_sec::url_host(url) {
        Some(h) => allow(&h),
        None => false,
    }
}

fn nav_sym(n: Nav) -> u32 {
    match n {
        Nav::Left => keys::LEFT,
        Nav::Right => keys::RIGHT,
        Nav::Up => keys::UP,
        Nav::Down => keys::DOWN,
        Nav::Home => keys::HOME,
        Nav::End => keys::END,
        Nav::PageUp => keys::PAGE_UP,
        Nav::PageDown => keys::PAGE_DOWN,
    }
}

/// The press and release of a shell key as protocol messages: keysyms are X11's (`keys`), `text` is what the key types.
pub fn key_events(k: &Key) -> Vec<ToHelper> {
    let (sym, mods, text): (u32, u32, String) = match k {
        Key::Char(c) => (keys::from_char(*c), if c.is_uppercase() { proto::MOD_SHIFT } else { 0 }, c.to_string()),
        Key::Ctrl(c) => {
            let upper = c.is_ascii_uppercase();
            (keys::from_char(*c), proto::MOD_CTRL | if upper { proto::MOD_SHIFT } else { 0 }, String::new())
        }
        Key::Enter => (keys::RETURN, 0, "\r".into()),
        Key::Backspace => (keys::BACKSPACE, 0, String::new()),
        Key::Delete => (keys::DELETE, 0, String::new()),
        Key::Tab => (keys::TAB, 0, "\t".into()),
        Key::BackTab => (keys::LEFT_TAB, proto::MOD_SHIFT, String::new()),
        Key::Esc => (keys::ESCAPE, 0, String::new()),
        Key::Up => (keys::UP, 0, String::new()),
        Key::Down => (keys::DOWN, 0, String::new()),
        Key::Left => (keys::LEFT, 0, String::new()),
        Key::Right => (keys::RIGHT, 0, String::new()),
        Key::Home => (keys::HOME, 0, String::new()),
        Key::End => (keys::END, 0, String::new()),
        Key::PageUp => (keys::PAGE_UP, 0, String::new()),
        Key::PageDown => (keys::PAGE_DOWN, 0, String::new()),
        Key::F(n) => (keys::function_key(*n), 0, String::new()),
        Key::Nav { nav, shift, ctrl } => (nav_sym(*nav), if *shift { proto::MOD_SHIFT } else { 0 } | if *ctrl { proto::MOD_CTRL } else { 0 }, String::new()),
    };
    vec![ToHelper::Key { down: true, keysym: sym, mods, text }, ToHelper::Key { down: false, keysym: sym, mods, text: String::new() }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(k: Key) -> (u32, u32, String) {
        match key_events(&k).remove(0) {
            ToHelper::Key { down: true, keysym, mods, text } => (keysym, mods, text),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn shell_keys_become_x11_keysyms() {
        assert_eq!(first(Key::Char('a')), (0x61, 0, "a".into()));
        assert_eq!(first(Key::Char('A')), (0x41, proto::MOD_SHIFT, "A".into()));
        assert_eq!(first(Key::Char('é')), (0xe9, 0, "é".into()));
        assert_eq!(first(Key::Char('€')), (0x0100_20ac, 0, "€".into()));
        assert_eq!(first(Key::Char(' ')), (0x20, 0, " ".into()));
        assert_eq!(first(Key::Ctrl('c')), (0x63, proto::MOD_CTRL, String::new()));
        assert_eq!(first(Key::Ctrl('C')), (0x43, proto::MOD_CTRL | proto::MOD_SHIFT, String::new()));
        assert_eq!(first(Key::Enter), (0xff0d, 0, "\r".into()));
        assert_eq!(first(Key::Backspace).0, 0xff08);
        assert_eq!(first(Key::Delete).0, 0xffff);
        assert_eq!(first(Key::Tab).0, 0xff09);
        assert_eq!(first(Key::BackTab), (0xfe20, proto::MOD_SHIFT, String::new()));
        assert_eq!(first(Key::Esc).0, 0xff1b);
        assert_eq!([Key::Left, Key::Up, Key::Right, Key::Down].map(|k| first(k).0), [0xff51, 0xff52, 0xff53, 0xff54]);
        assert_eq!([Key::Home, Key::PageUp, Key::PageDown, Key::End].map(|k| first(k).0), [0xff50, 0xff55, 0xff56, 0xff57]);
        assert_eq!(first(Key::F(5)).0, 0xffc2);
        assert_eq!(first(Key::Nav { nav: Nav::Right, shift: true, ctrl: true }), (0xff53, proto::MOD_SHIFT | proto::MOD_CTRL, String::new()));
        // every key is a press and then a release of the same key
        let m = key_events(&Key::Char('x'));
        assert!(matches!(&m[1], ToHelper::Key { down: false, keysym: 0x78, .. }));
    }

    #[test]
    fn addresses_without_a_host_or_with_a_refused_one_are_not_allowed() {
        let allow = |h: &str| h == "example.org";
        assert!(url_allowed("https://example.org/x", &allow));
        assert!(!url_allowed("https://evil.example/x", &allow));
        assert!(url_allowed("about:blank", &allow));
        assert!(!url_allowed("https://u:p@example.org/", &allow), "credentials in the address cannot be vetted");
        assert!(!url_allowed("ftp://example.org/", &allow));
        assert!(!url_allowed("javascript:alert(1)", &allow));
    }

    #[test]
    fn the_scale_is_in_quarter_steps() {
        assert_eq!(scale_pct(1.0), 100);
        assert_eq!(scale_pct(1.4), 150);
        assert_eq!(scale_pct(0.2), 50);
        assert_eq!(scale_pct(9.0), 400);
    }
}
