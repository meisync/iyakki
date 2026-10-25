//! The browser's embedded full web engine: one helper program per tab renders the page offscreen, this file talks to it
//! (`iyakki-webview`, protocol in `docs/WEBVIEW-PROTOCOL.md`), keeps the latest picture, draws it in the page area and turns the
//! shell's keys, mouse and wheel into protocol messages. The browser (`browser.rs`) owns the chrome (tabs, address bar, buttons)
//! and decides what to do with the notes this view leaves ([`Note`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iyakki_webview::blit::{bgra_to_rgba_opaque, blit_region, blit_region_x2, stretch_for, uncovered};
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
/// A pixel step of the shell's scroll engine (positive = up) as the protocol's wheel units (positive = down, 120 a notch).
pub(super) fn wheel_units(px: i32) -> i32 {
    (-(px as f32) * NOTCH as f32 / crate::scrolleng::NOTCH_PX).round() as i32
}

/// Rows at the top of the page area that the browser's own chrome draws over (see `Full::draw_pending`).
const CHROME_ROWS: i32 = 1;

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
    /// The sound doctor's short note for the browser bar (`Some`), or the end of it (`None`).
    Sound(Option<String>),
}

/// What a view needs the shell to do about a change (see [`Full::redraw_kind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redraw {
    None,
    /// Only the engine's picture changed: [`Full::draw_pending`] can paint just that part.
    Live,
    Full,
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
    /// The page is in DOM fullscreen: the browser shows only the page, over the whole pane, until the page leaves (or Esc asks it to).
    pub fullscreen: bool,
    /// Where the page area was drawn last (screen pixels), for turning absolute mouse positions into page ones.
    rect: R,
    /// The left button is held down inside the page (a drag), so moves arrive as presses repeated by the shell.
    pub pressed: bool,
    last_mouse: Option<(i32, i32)>,
    redraw: bool,
    /// The part of the picture (picture pixels) that changed since it was last drawn on the screen.
    tile_pending: Option<Rect>,
    /// The picture size at the last full draw, and how many screen pixels one picture pixel took then (1, or 2 when smooth).
    drawn_size: (i32, i32),
    up: i32,
    /// "Smooth": the engine draws at half the width and height (a quarter of the pixels) and the shell stretches the picture.
    smooth: bool,
    /// The UI scale at the last `send_zoom` (kept so a change of the smooth setting can send the zoom again).
    scale_now: f32,
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
    /// Answers to `eval` scripts not yet taken: `(id, ok, json or error text)`.
    script_results: Vec<(u32, bool, String)>,
    next_script: u32,
    /// The sound doctor: is the page playing, and is the sound connected (see `browser_sound.rs`).
    sound: super::browser_sound::SoundWatch,
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
            fullscreen: false,
            rect: R::new(0.0, 0.0, w as f32, h as f32),
            pressed: false,
            last_mouse: None,
            redraw: true,
            tile_pending: None,
            drawn_size: (0, 0),
            up: 1,
            smooth: false,
            scale_now: st.scale.max(0.1),
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
            script_results: Vec::new(),
            next_script: 1,
            sound: super::browser_sound::SoundWatch::new(),
        };
        f.send_zoom(st.scale);
        f.view.send(ToHelper::Focus(true));
        Ok(f)
    }

    /// Smooth: draw the page at half the width and height and stretch it (about four times fewer pixels for the engine to paint,
    /// at the price of softer text). The page keeps its layout: the engine's zoom is halved to make up for the smaller picture.
    pub fn set_smooth(&mut self, on: bool) {
        if self.smooth != on {
            self.smooth = on;
            self.redraw = true;
            let s = self.scale_now;
            self.send_zoom(s);
        }
    }

    fn div(&self) -> u32 {
        if self.smooth {
            2
        } else {
            1
        }
    }

    fn send_zoom(&mut self, scale_now: f32) {
        self.scale_now = scale_now.max(0.1);
        let pct = ((self.zoom_pct as f32) * scale_now.max(0.1) / self.scale_hello / self.div() as f32).round().clamp(25.0, 500.0) as u32;
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

    /// Run JavaScript in the page (protocol 1.1 `Script`); the answer arrives through [`Full::take_script_result`] after a `pump`.
    pub fn eval(&mut self, js: &str, timeout_ms: u32) -> u32 {
        let id = self.next_script;
        self.next_script = self.next_script.wrapping_add(1).max(1);
        self.view.send(ToHelper::Script { id, js: js.to_string(), timeout_ms });
        id
    }

    pub fn shm_path(&self) -> PathBuf {
        self.view.shm_path().to_path_buf()
    }

    pub fn take_script_result(&mut self, id: u32) -> Option<(bool, String)> {
        let i = self.script_results.iter().position(|r| r.0 == id)?;
        let (_, ok, j) = self.script_results.remove(i);
        Some((ok, j))
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

    /// Esc in fullscreen: ask the page to leave (it answers with `Fullscreen(false)`, which restores the bars). The bars stay
    /// hidden until it does, so the page never sits in a half state.
    pub fn leave_fullscreen(&mut self) {
        if self.fullscreen {
            self.view.send(ToHelper::LeaveFullscreen);
        }
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
        let kind = self.redraw_kind();
        if kind != Redraw::None {
            // the caller draws the whole view in answer: nothing stays pending (a view nobody draws must not ask again at every beat)
            self.tile_pending = None;
        }
        kind != Redraw::None
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
                WvEvent::Fullscreen(on) => {
                    if self.fullscreen != on {
                        self.fullscreen = on;
                        self.page_focus = true; // the page has the keys in fullscreen (the browser's bars are gone)
                        self.view.send(ToHelper::Focus(true));
                        self.last_mouse = None;
                        self.redraw = true;
                    }
                }
                WvEvent::Download { url, name } => self.notes.push(Note::Download { url, name }),
                WvEvent::Clipboard(t) => self.notes.push(Note::Clipboard(t)),
                WvEvent::ScriptResult { id, ok, json } => {
                    if self.script_results.len() >= 32 {
                        self.script_results.remove(0);
                    }
                    self.script_results.push((id, ok, json));
                }
                WvEvent::AudioPlaying(on) => self.sound.set_playing(on),
                WvEvent::Crashed(r) => self.notes.push(Note::Trouble(r)),
                WvEvent::Log(l) => {
                    self.sound.on_log(&l);
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
        self.sound_tick();
        if !self.ready && self.gone.is_none() && self.started.elapsed() > START_TIMEOUT {
            let why = "the web engine did not start in time".to_string();
            self.gone = Some(why.clone());
            self.notes.push(Note::Gone(why));
            self.redraw = true;
        }
    }

    /// The sound doctor: while the page plays audio, compare it with the kernel's playback streams and recover (see `browser_sound.rs`).
    fn sound_tick(&mut self) {
        let Some(helper) = self.view.pid() else { return };
        let actions = self.sound.tick(helper, std::path::Path::new(iyakki_agent::quiet::ASOUND), std::path::Path::new("/proc"), std::process::id());
        for a in actions {
            match a {
                iyakki_agent::sounddoctor::Action::Note(n) => self.notes.push(Note::Sound(Some(n))),
                iyakki_agent::sounddoctor::Action::Clear => self.notes.push(Note::Sound(None)),
                iyakki_agent::sounddoctor::Action::CloseHolder(p) => {
                    iyakki_agent::sounddoctor::close_holder(p, std::process::id());
                }
                iyakki_agent::sounddoctor::Action::Reconnect => {
                    self.eval(iyakki_agent::sounddoctor::RECONNECT_JS, 2000);
                }
            }
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
            let r = r.clip(w, h);
            self.tile_pending = Some(self.tile_pending.map_or(r, |p| p.union(r)));
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
            bgra_to_rgba_opaque(&src[a..b], &mut dst[a..b]);
        }
    }

    /// Draw the page into `view` (screen pixels) and tell the helper when the area's size or scale changed.
    pub fn draw(&mut self, g: &mut Gfx, view: R, s: f32) {
        self.rect = view;
        let div = self.div();
        let want = (((view.w.max(64.0)) as u32 / div).max(32), ((view.h.max(64.0)) as u32 / div).max(32));
        if want != self.view.size_wanted() {
            self.view.send(ToHelper::Resize { w: want.0, h: want.1 });
        }
        self.send_zoom(s);
        let (vx, vy) = (view.x.round() as i32, view.y.round() as i32);
        let (vw, vh) = (view.w as i32, view.h as i32);
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        // the page's own white where no picture has arrived yet (or the area grew and the helper has not caught up): only the part
        // the picture does not cover (the picture is drawn over the rest, so filling it first would write the whole area twice)
        let Some(pm) = &self.pm else {
            g.rect(view.x, view.y, view.w, view.h, [255, 255, 255, 255]);
            return;
        };
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let up = stretch_for(self.smooth, pw, vw);
        for r in uncovered(vx, vy, vw, vh, pw * up, ph * up) {
            g.rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32, [255, 255, 255, 255]);
        }
        self.tile_pending = None;
        self.drawn_size = (pw, ph);
        self.up = up;
        if up == 2 {
            blit_region_x2(pm.data(), pw, ph, g.pm.data_mut(), gw, gh, (vx, vy, vw, vh), (0, 0, pw, ph));
        } else {
            blit_region(pm.data(), pw, ph, g.pm.data_mut(), gw, gh, (vx, vy, vw, vh), (0, 0, pw, ph));
        }
    }

    /// Draw only what changed in the picture since it was last drawn, in place over the scene the last full draw left in `g`
    /// (the shell does this between full redraws when nothing else on screen changed). Returns the screen rectangle written, or
    /// `None` when there is nothing to draw or the area changed size (a full draw is needed).
    pub fn draw_pending(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        let r = self.tile_pending?;
        let pm = self.pm.as_ref()?;
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let view = self.rect;
        let (vx, vy) = (view.x.round() as i32, view.y.round() as i32);
        let (vw, vh) = (view.w as i32, view.h as i32);
        if (pw, ph) != self.drawn_size || stretch_for(self.smooth, pw, vw) != self.up {
            return None; // the picture is not the size the last full draw laid out
        }
        self.tile_pending = None;
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        // the browser's tool bar draws one translucent row over the page's first row (the accent line's faint underline): that row
        // is left for the next whole redraw, which draws the bar over the picture again
        let top = (r.y as i32).max(if self.fullscreen { 0 } else { CHROME_ROWS });
        let region = (r.x as i32, top, r.w as i32, (r.y as i32 + r.h as i32 - top).max(0));
        if region.3 == 0 {
            return Some((0, 0, 0, 0));
        }
        if self.up == 2 {
            blit_region_x2(pm.data(), pw, ph, g.pm.data_mut(), gw, gh, (vx, vy, vw, vh), region)
        } else {
            blit_region(pm.data(), pw, ph, g.pm.data_mut(), gw, gh, (vx, vy, vw, vh), region)
        }
    }

    /// What the view needs from the shell: a full redraw (chrome, state or size changed), only the picture (frames arrived), or nothing.
    pub fn redraw_kind(&mut self) -> Redraw {
        if std::mem::take(&mut self.redraw) {
            Redraw::Full
        } else if self.tile_pending.is_some() {
            Redraw::Live
        } else {
            Redraw::None
        }
    }

    // ------------------------------------------------------------------------------------------------- input

    /// An absolute screen position as a page position, when it is inside the page area.
    pub fn to_page(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        let (px, py) = (x - self.rect.x.round() as i32, y - self.rect.y.round() as i32);
        (px >= 0 && py >= 0 && (px as f32) < self.rect.w && (py as f32) < self.rect.h).then_some((px, py))
    }

    /// A position in screen pixels relative to the page area as the engine's picture pixels (they differ when smooth is on).
    fn dev(&self, x: i32, y: i32) -> (i32, i32) {
        let u = self.up.max(1);
        (x / u, y / u)
    }

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        if let Some((px, py)) = self.to_page(x, y) {
            if self.last_mouse != Some((px, py)) {
                self.last_mouse = Some((px, py));
                let buttons = if self.pressed { proto::HELD_LEFT } else { 0 };
                let (dx, dy) = self.dev(px, py);
                self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x: dx, y: dy, button: 0, buttons, mods: 0, clicks: 0 });
            }
        } else {
            self.last_mouse = None;
        }
    }

    /// A press at a position relative to the page area (the shell reports presses that way); `clicks` is 1, 2 or 3.
    pub fn mouse_down(&mut self, x: i32, y: i32, clicks: u8) {
        self.last_mouse = Some((x, y));
        self.pressed = true;
        let (x, y) = self.dev(x, y);
        self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: 0, mods: 0, clicks: 0 });
        self.view.send(ToHelper::Mouse { kind: MouseKind::Down, x, y, button: proto::BUTTON_LEFT, buttons: proto::HELD_LEFT, mods: 0, clicks: clicks.clamp(1, 3) });
    }

    /// The shell repeats the press while the button is held and the pointer moves (a drag).
    pub fn mouse_drag(&mut self, x: i32, y: i32) {
        if self.last_mouse != Some((x, y)) {
            self.last_mouse = Some((x, y));
            let (x, y) = self.dev(x, y);
            self.view.send(ToHelper::Mouse { kind: MouseKind::Move, x, y, button: 0, buttons: proto::HELD_LEFT, mods: 0, clicks: 0 });
        }
    }

    pub fn mouse_up(&mut self, x: i32, y: i32) {
        if self.pressed {
            self.pressed = false;
            let (px, py) = self.dev(x - self.rect.x.round() as i32, y - self.rect.y.round() as i32);
            self.view.send(ToHelper::Mouse { kind: MouseKind::Up, x: px, y: py, button: proto::BUTTON_LEFT, buttons: 0, mods: 0, clicks: 1 });
        }
    }

    /// A wheel step: `notches` > 0 scrolls up (as the shell's `Event::Scroll`). Goes where the pointer last was in the page.
    pub fn wheel(&mut self, notches: i32) -> bool {
        let Some((x, y)) = self.last_mouse else { return false };
        let (x, y) = self.dev(x, y);
        self.view.send(ToHelper::Wheel { x, y, dx: 0, dy: -notches * NOTCH, mods: 0 });
        true
    }

    /// A smooth wheel step in pixels of a nominal screen (`px` > 0 scrolls up; [`crate::scrolleng::NOTCH_PX`] is a notch). It goes to
    /// the page engine as a fraction of a notch (the protocol counts 1/120 of one), once, without easing here: the engine's own
    /// pictures are the smoothness.
    pub fn wheel_px(&mut self, px: i32) -> bool {
        let Some((x, y)) = self.last_mouse else { return false };
        let (x, y) = self.dev(x, y);
        self.view.send(ToHelper::Wheel { x, y, dx: 0, dy: wheel_units(px), mods: 0 });
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
    fn smooth_pixel_steps_become_fractions_of_a_notch_in_the_protocols_units() {
        // 54 nominal pixels are one notch (120 units); positive pixels scroll up, positive units scroll down
        assert_eq!(wheel_units(54), -120);
        assert_eq!(wheel_units(-54), 120);
        assert_eq!(wheel_units(27), -60);
        assert_eq!(wheel_units(3), -7, "a 3 px step is a small fraction, not a whole notch");
        assert_eq!(wheel_units(0), 0);
        // nothing is lost over a click delivered as steps: 9 steps of 6 px = one notch (within rounding)
        let sum: i32 = (0..9).map(|_| wheel_units(6)).sum();
        assert!((sum + 120).abs() <= 5, "{sum}");
    }

    #[test]
    fn shell_keys_become_x11_keysyms() {
        assert_eq!(first(Key::Char('a')), (0x61, 0, "a".into()));
        assert_eq!(first(Key::Char('A')), (0x41, proto::MOD_SHIFT, "A".into()));
        assert_eq!(first(Key::Char('é')), (0xe9, 0, "é".into()));
        assert_eq!(first(Key::Char('€')), (0x0100_20ac, 0, "€".into()));
        assert_eq!(first(Key::Char(' ')), (0x20, 0, " ".into()));
        // shifted symbols and digits are their own keysyms (ASCII is the X11 keysym); Ctrl+A/C/V/X carry the control modifier only
        assert_eq!(first(Key::Char('!')), (0x21, 0, "!".into()));
        assert_eq!(first(Key::Char('@')), (0x40, 0, "@".into()));
        assert_eq!(first(Key::Char('7')), (0x37, 0, "7".into()));
        for c in ['a', 'c', 'v', 'x'] {
            assert_eq!(first(Key::Ctrl(c)), (c as u32, proto::MOD_CTRL, String::new()));
        }
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
