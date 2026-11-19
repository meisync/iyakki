//! SAMPLE in the shell: the app itself is the crate `iyakki-app-sample`, written only against the UI SDK (`iyakki-ui-sdk`). This file is the
//! whole shell side: it turns the shell's events into SDK events, draws the SDK's frame on the screen's `Gfx` through the raster canvas, and
//! hands the app a Look made from the screen's current theme. The first time the tile gets the session (`bind`) it hands the app its ports,
//! which are the core's (`cx.core`): the notes are kept by the core's app data (one file per person and app), the pin button changes the
//! person's pinned dock apps and the wallpaper picker changes the person's wallpaper, each audited. Until then (and in a preview) the
//! ports are in memory.

use iyakki_app_sample::SampleApp;
use iyakki_ui_sdk::app::Flow as UiFlow;
use std::sync::Arc;

use iyakki_core_api::Core;
use iyakki_ui_sdk::desktop::{Arrangement, DeskError, DockPort, DockStatus, MemDock, MemWallpaper, Screen, WallpaperPort};
use iyakki_ui_sdk::ports::{check_key, PortError, StoragePort};
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::raster::RasterCanvas;

use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx, Icon};
use crate::ui::{Event, Key, R};

pub const ACCENT: [u8; 4] = rgb(150, 210, 140);

/// The app's own saved text, through the core's app data for this person.
struct CoreStorage {
    core: Arc<dyn Core>,
    user: String,
}

impl StoragePort for CoreStorage {
    fn get(&mut self, key: &str) -> Option<String> {
        self.core.app_data().get(&self.user, iyakki_app_sample::ID, key)
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError> {
        check_key(key)?;
        self.core.app_data().set(&self.user, iyakki_app_sample::ID, key, value).map_err(|e| if e.contains("256") { PortError::TooBig } else { PortError::Denied })
    }
    fn remove(&mut self, key: &str) -> Result<(), PortError> {
        self.core.app_data().remove(&self.user, iyakki_app_sample::ID, key).map_err(|_| PortError::Denied)
    }
    fn keys(&mut self) -> Vec<String> {
        self.core.app_data().keys(&self.user, iyakki_app_sample::ID)
    }
}

/// The person's home folder, through the core's files facet: listing and reading text, nothing else. The names are relative to the home;
/// the app never learns where it is.
pub struct CoreFilesPort {
    pub core: Arc<dyn Core>,
    pub user: String,
}

impl iyakki_ui_sdk::files::FilesPort for CoreFilesPort {
    fn list(&mut self, dir: &str) -> Result<iyakki_ui_sdk::files::DirListing, iyakki_ui_sdk::files::FilesError> {
        use iyakki_ui_sdk::files::{DirListing, EntryInfo, FilesError};
        match self.core.files().list(&self.user, dir) {
            Ok(l) => Ok(DirListing { dir: l.dir, entries: l.entries.into_iter().map(|e| EntryInfo { name: e.name, is_dir: e.is_dir, size: e.size }).collect(), truncated: l.truncated }),
            Err(e) if e == iyakki_core_api::files::NO_FILES => Err(FilesError::Unavailable),
            Err(e) => Err(FilesError::Refused(e)),
        }
    }
    fn read_text(&mut self, path: &str, max: usize) -> Result<String, iyakki_ui_sdk::files::FilesError> {
        use iyakki_ui_sdk::files::FilesError;
        self.core.files().read_text(&self.user, path, max).map_err(|e| if e == iyakki_core_api::files::NO_FILES { FilesError::Unavailable } else { FilesError::Refused(e) })
    }
    fn change(&mut self, c: &iyakki_ui_sdk::files::Change) -> Result<iyakki_ui_sdk::files::Ask, iyakki_ui_sdk::files::FilesError> {
        use iyakki_core_api::FileChange as F;
        use iyakki_ui_sdk::files::{Ask, Change, FilesError};
        let fc = match c.clone() {
            Change::MakeDir { dir, name } => F::MakeDir { dir, name },
            Change::Rename { path, new_name } => F::Rename { path, new_name },
            Change::Trash { path } => F::Trash { path },
            Change::Save { path, text } => F::Save { path, text },
        };
        self.core.files().change(&self.user, &fc).map(|a| Ask { call: a.call, summary: a.summary }).map_err(|e| if e == iyakki_core_api::files::NO_FILES { FilesError::Unavailable } else { FilesError::Refused(e) })
    }
    fn decide(&mut self, call: &str, approve: bool) -> Result<String, iyakki_ui_sdk::files::FilesError> {
        use iyakki_ui_sdk::files::FilesError;
        self.core.files().decide(&self.user, call, approve).map_err(|e| if e == iyakki_core_api::files::NO_FILES { FilesError::Unavailable } else { FilesError::Refused(e) })
    }
}

/// One session of the core's terminal for this person: lines are checked by the core against the person's policy, and a line that changes
/// something comes back as a question the person answers (the core asks; the view only shows the card).
pub struct CoreTerminalPort {
    pub core: Arc<dyn Core>,
    pub user: String,
    session: Option<String>,
    /// The whole-session terminal (F-SDK-103), separate from the line session.
    screen: Option<String>,
}

impl CoreTerminalPort {
    pub fn new(core: Arc<dyn Core>, user: String) -> CoreTerminalPort {
        CoreTerminalPort { core, user, session: None, screen: None }
    }

    fn screen_id(&self) -> Result<String, iyakki_ui_sdk::terminal::TermError> {
        self.screen.clone().ok_or_else(|| iyakki_ui_sdk::terminal::TermError::Refused("that terminal is not open".into()))
    }
}

fn reply_of(r: iyakki_core_api::TermReply) -> iyakki_ui_sdk::terminal::Reply {
    iyakki_ui_sdk::terminal::Reply { text: r.text, prompt: r.prompt, clear: r.clear, cut: r.cut, question: r.approval.map(|a| iyakki_ui_sdk::terminal::Question { call: a.call, summary: a.summary }) }
}

fn term_err(e: String) -> iyakki_ui_sdk::terminal::TermError {
    if e == iyakki_core_api::terminal::NO_TERMINAL {
        iyakki_ui_sdk::terminal::TermError::Unavailable
    } else {
        iyakki_ui_sdk::terminal::TermError::Refused(e)
    }
}

impl iyakki_ui_sdk::terminal::TerminalPort for CoreTerminalPort {
    fn open(&mut self) -> Result<iyakki_ui_sdk::terminal::Opened, iyakki_ui_sdk::terminal::TermError> {
        let o = self.core.terminal().open(&self.user, iyakki_app_sample::ID).map_err(term_err)?;
        self.session = Some(o.id);
        Ok(iyakki_ui_sdk::terminal::Opened { prompt: o.prompt, banner: o.banner })
    }
    fn run(&mut self, line: &str) -> Result<iyakki_ui_sdk::terminal::Reply, iyakki_ui_sdk::terminal::TermError> {
        let id = self.session.clone().ok_or_else(|| iyakki_ui_sdk::terminal::TermError::Refused("that terminal is not open".into()))?;
        self.core.terminal().run(&self.user, &id, line).map(reply_of).map_err(term_err)
    }
    fn decide(&mut self, call: &str, approve: bool) -> Result<iyakki_ui_sdk::terminal::Reply, iyakki_ui_sdk::terminal::TermError> {
        let id = self.session.clone().ok_or_else(|| iyakki_ui_sdk::terminal::TermError::Refused("that terminal is not open".into()))?;
        self.core.terminal().decide(&self.user, &id, call, approve).map(reply_of).map_err(term_err)
    }
    fn close(&mut self) {
        if let Some(id) = self.session.take() {
            self.core.terminal().close(&self.user, &id);
        }
        self.screen_close();
    }

    fn screen_open(&mut self, cols: usize, rows: usize) -> Result<iyakki_ui_sdk::terminal::Question, iyakki_ui_sdk::terminal::TermError> {
        self.screen_close();
        let o = self.core.terminal().pty_open(&self.user, iyakki_app_sample::ID, cols, rows).map_err(term_err)?;
        self.screen = Some(o.id);
        Ok(iyakki_ui_sdk::terminal::Question { call: o.approval.call, summary: o.approval.summary })
    }
    fn screen_decide(&mut self, call: &str, approve: bool) -> Result<(), iyakki_ui_sdk::terminal::TermError> {
        let id = self.screen_id()?;
        let r = self.core.terminal().pty_decide(&self.user, &id, call, approve).map_err(term_err);
        if r.is_err() || !approve {
            self.screen = None;
            self.core.terminal().pty_close(&self.user, &id);
        }
        r
    }
    fn screen_key(&mut self, key: iyakki_ui_sdk::terminal::ScreenKey, ctrl: bool, alt: bool) -> Result<(), iyakki_ui_sdk::terminal::TermError> {
        use iyakki_core_api::PtyKey as K;
        use iyakki_ui_sdk::terminal::ScreenKey as S;
        let id = self.screen_id()?;
        let k = match key {
            S::Char(c) => K::Char(c),
            S::Enter => K::Enter,
            S::Tab => K::Tab,
            S::BackTab => K::BackTab,
            S::Backspace => K::Backspace,
            S::Esc => K::Esc,
            S::Up => K::Up,
            S::Down => K::Down,
            S::Left => K::Left,
            S::Right => K::Right,
            S::Home => K::Home,
            S::End => K::End,
            S::PageUp => K::PageUp,
            S::PageDown => K::PageDown,
            S::Insert => K::Insert,
            S::Delete => K::Delete,
            S::F(n) => K::F(n),
        };
        self.core.terminal().pty_key(&self.user, &id, k, iyakki_core_api::PtyMods { ctrl, alt, shift: false }).map_err(term_err)
    }
    fn screen_text(&mut self, text: &str) -> Result<(), iyakki_ui_sdk::terminal::TermError> {
        let id = self.screen_id()?;
        self.core.terminal().pty_text(&self.user, &id, text).map_err(term_err)
    }
    fn screen_resize(&mut self, cols: usize, rows: usize) -> Result<(), iyakki_ui_sdk::terminal::TermError> {
        let id = self.screen_id()?;
        self.core.terminal().pty_resize(&self.user, &id, cols, rows).map_err(term_err)
    }
    fn screen_scroll(&mut self, lines: isize) -> Result<(), iyakki_ui_sdk::terminal::TermError> {
        let id = self.screen_id()?;
        self.core.terminal().pty_scroll(&self.user, &id, lines).map_err(term_err)
    }
    fn screen_frame(&mut self) -> Result<iyakki_ui_sdk::terminal::Frame, iyakki_ui_sdk::terminal::TermError> {
        use iyakki_core_api::PtyColor as C;
        use iyakki_ui_sdk::terminal::{ScreenCell, ScreenColor};
        let id = self.screen_id()?;
        let f = self.core.terminal().pty_frame(&self.user, &id).map_err(term_err)?;
        let col = |c: C| match c {
            C::Default => ScreenColor::Default,
            C::Indexed(i) => ScreenColor::Indexed(i),
            C::Rgb(r, g, b) => ScreenColor::Rgb(r, g, b),
        };
        let cells = f.cells.iter().map(|r| r.iter().map(|c| ScreenCell { ch: c.ch, fg: col(c.fg), bg: col(c.bg), attrs: c.attrs, tail: c.tail }).collect()).collect();
        Ok(iyakki_ui_sdk::terminal::Frame { seq: f.seq, cols: f.cols, rows: f.rows, cells, cursor: f.cursor, cursor_visible: f.cursor_visible, back: f.back, history: f.history, exited: f.exited })
    }
    fn screen_close(&mut self) {
        if let Some(id) = self.screen.take() {
            self.core.terminal().pty_close(&self.user, &id);
        }
    }
}

/// The person's dock, through the core. A person who never chose has the Look's usual apps, which the first pin keeps.
struct CoreDock {
    core: Arc<dyn Core>,
    cx: AppCx,
    /// The app the SDK app asked the dock to open; the tile hands it to the shell (`App::take_jump`), once.
    asked: Arc<std::sync::Mutex<Option<String>>>,
}

impl CoreDock {
    fn known(&self, app: &str) -> Result<(), DeskError> {
        let a = iyakki_sec::policy::App::ALL.into_iter().find(|a| a.id() == app).ok_or(DeskError::UnknownApp)?;
        if self.cx.session.policy.allows_app(a) {
            Ok(())
        } else {
            Err(DeskError::NotAllowed)
        }
    }
}

impl DockPort for CoreDock {
    fn pinned(&self) -> Vec<String> {
        self.core.desktop().dock(&self.cx.session.user).unwrap_or_else(|| iyakki_uikit::look_registry::DEFAULT_DOCK_IDS.iter().map(|s| s.to_string()).collect())
    }
    fn pin(&mut self, app: &str) -> Result<(), DeskError> {
        self.known(app)?;
        let mut v = self.pinned();
        if !v.iter().any(|a| a == app) {
            v.push(app.to_string());
            self.core.desktop().set_dock(&self.cx.session.user, &v).map_err(DeskError::Refused)?;
            self.cx.audit.record(&self.cx.session.user, "desktop.dock.pin", app);
        }
        Ok(())
    }
    fn unpin(&mut self, app: &str) -> Result<(), DeskError> {
        let mut v = self.pinned();
        if v.iter().any(|a| a == app) {
            v.retain(|a| a != app);
            self.core.desktop().set_dock(&self.cx.session.user, &v).map_err(DeskError::Refused)?;
            self.cx.audit.record(&self.cx.session.user, "desktop.dock.unpin", app);
        }
        Ok(())
    }
    fn open(&mut self, app: &str) -> Result<(), DeskError> {
        // Opening another app is the shell's to do: the tile keeps the request and the shell takes it after the heartbeat or the key
        // (`App::take_jump`); it is the person's own dock and policy that decide (`known`), and the audit says it.
        self.known(app)?;
        *self.asked.lock().unwrap_or_else(|e| e.into_inner()) = Some(app.to_string());
        self.cx.audit.record(&self.cx.session.user, "desktop.dock.open", app);
        Ok(())
    }
    fn set_status(&mut self, _app: &str, _status: DockStatus) {}
}

/// The person's wallpaper, through the core. One wallpaper for the person: every screen gets the same.
struct CoreWallpaper {
    core: Arc<dyn Core>,
    cx: AppCx,
}

impl WallpaperPort for CoreWallpaper {
    fn get(&self, screen: u32) -> iyakki_uikit::wallpaper::Id {
        let d = self.core.desktop();
        d.screen_wallpaper(&self.cx.session.user, screen).or_else(|| d.wallpaper(&self.cx.session.user)).map(|k| iyakki_uikit::wallpaper::Id::from_key(&k)).unwrap_or_default()
    }
    /// The first (primary) screen's choice is the person's own wallpaper, which the shell draws; another screen keeps a choice of its own.
    fn set(&mut self, screen: u32, id: iyakki_uikit::wallpaper::Id) -> Result<(), DeskError> {
        let d = self.core.desktop();
        if screen == 0 {
            d.set_wallpaper(&self.cx.session.user, id.key()).map_err(DeskError::Refused)?;
            iyakki_uikit::wallpaper::set_current(id);
        } else {
            d.set_screen_wallpaper(&self.cx.session.user, screen, id.key()).map_err(DeskError::Refused)?;
        }
        self.cx.audit.record(&self.cx.session.user, "desktop.wallpaper", &format!("screen {screen} {}", id.key()));
        Ok(())
    }
}

pub struct SampleTile {
    host: Host<SampleApp>,
    /// The ports are the core's once the tile has seen the session.
    bound: bool,
    /// The area and scale the arrangement was made for.
    shape: (u32, u32, f32),
    /// The tile's clock: the SDK's heartbeat carries milliseconds since the tile began (for pacing, never the date).
    started: std::time::Instant,
    /// What the app asked the dock to open (see `CoreDock::open`).
    asked: Arc<std::sync::Mutex<Option<String>>>,
}

impl Default for SampleTile {
    fn default() -> Self {
        SampleTile::new()
    }
}

impl SampleTile {
    pub fn new() -> SampleTile {
        let host = Host::new(SampleApp::new(), look_now(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[iyakki_app_sample::ID])), Box::new(MemWallpaper::default()));
        SampleTile { host, bound: false, shape: (1280, 720, 1.0), started: std::time::Instant::now(), asked: Default::default() }
    }

    /// Give the app the core's ports (once): its notes come back from the core and every change is kept there.
    fn bind(&mut self, cx: &AppCx) {
        if self.bound {
            return;
        }
        self.bound = true;
        let user = cx.session.user.clone();
        if user.is_empty() {
            return;
        }
        let core = cx.core.clone();
        let who = user.clone();
        let cam_user = user.clone();
        // the assistant pane is the core's: the port is bound to this person and this app, and the core answers or refuses in a sentence
        let port = super::assistant_port::CoreAssistantPort::new(cx.core.clone(), &user, iyakki_app_sample::ID);
        let files = CoreFilesPort { core: cx.core.clone(), user: user.clone() };
        let edit_port = super::assistant_port::CoreAssistantPort::new(cx.core.clone(), &user, iyakki_app_sample::ID);
        let term = CoreTerminalPort::new(cx.core.clone(), user.clone());
        let media = super::sdk_media::CoreMediaPort::new(cx.core.clone(), &user, iyakki_app_sample::ID);
        self.host.app = SampleApp::with_storage(Box::new(CoreStorage { core: cx.core.clone(), user })).with_assistant(Box::new(port)).with_files(Box::new(files)).with_terminal(Box::new(term)).with_editor().with_editor_assistant(Box::new(edit_port)).with_media(Box::new(media)).with_browser(move || Box::new(super::sdk_browser::CoreBrowserPort::new(core.clone(), &who, iyakki_app_sample::ID))).with_cameras(Box::new(super::cameras_port::CoreCameraPort::new(cx.core.clone(), &cam_user, iyakki_app_sample::ID))).with_events(Box::new(super::events_port::CoreEventsPort::new(cx.core.clone(), &cam_user, iyakki_app_sample::ID)), Box::new(super::events_port::CoreEventActions::new(cx.core.clone(), &cam_user, iyakki_app_sample::ID)));
        self.host.dock = Box::new(CoreDock { core: cx.core.clone(), cx: cx.clone(), asked: self.asked.clone() });
        self.host.wallpaper = Box::new(CoreWallpaper { core: cx.core.clone(), cx: cx.clone() });
    }

    /// The dock positions and wallpapers this person chose per screen go into the arrangement the app sees.
    fn apply_saved_screens(&mut self, cx: &AppCx) {
        let (user, d) = (cx.session.user.clone(), cx.core.desktop());
        if user.is_empty() {
            return;
        }
        let ids: Vec<u32> = self.host.arrangement.screens().iter().map(|s| s.id).collect();
        for id in ids {
            use iyakki_uikit::look::DockPos;
            if let Some(p) = d.screen_dock(&user, id) {
                let pos = match p.as_str() {
                    "left" => DockPos::Left,
                    "right" => DockPos::Right,
                    "off" => DockPos::Off,
                    _ => DockPos::Bottom,
                };
                self.host.arrangement.set_dock(id, pos);
            }
            if let Some(w) = d.screen_wallpaper(&user, id) {
                self.host.arrangement.set_wallpaper(id, iyakki_uikit::wallpaper::Id::from_key(&w));
            }
        }
    }

    /// The SDK event for a shell event, if the app has a use for it.
    pub(super) fn map(ev: &Event) -> Option<UiEvent> {
        Some(match ev {
            Event::Key(k) => UiEvent::Key(match k {
                Key::Char(c) => UiKey::Char(*c),
                Key::Enter => UiKey::Enter,
                Key::Backspace => UiKey::Backspace,
                Key::Delete => UiKey::Delete,
                Key::Esc => UiKey::Esc,
                Key::Tab => UiKey::Tab,
                Key::Up => UiKey::Up,
                Key::Down => UiKey::Down,
                Key::Left => UiKey::Left,
                Key::Right => UiKey::Right,
                _ => return None,
            }),
            Event::Row(n) => UiEvent::Action(*n as u32),
            Event::Scroll(n) => UiEvent::Wheel(*n),
            Event::Paste(s) => UiEvent::Text(s.clone()),
            Event::Tick => UiEvent::Tick,
            _ => return None,
        })
    }
}

/// The person's type choices as the SDK wants them: the family they chose, and their line height, letter spacing and tabular figures.
pub(super) fn type_now() -> iyakki_ui_sdk::typeface::TypeStyle {
    use iyakki_ui_sdk::typeface::{Family, TypeStyle};
    let p = iyakki_uikit::typeset::current();
    let family = match iyakki_uikit::fonts::current_ui() {
        iyakki_uikit::fonts::UiFont::DejaVu => Family::DejaVu,
        iyakki_uikit::fonts::UiFont::Liberation => Family::Liberation,
    };
    TypeStyle::new(family, p.line.factor(), p.tracking.em(), p.tabular)
}

/// The Look to draw with: the screen's current theme and wallpaper in the default Look.
pub(super) fn look_now() -> Look {
    let mut l = Look::default();
    l.theme = *crate::theme::current();
    l.wallpaper = iyakki_uikit::wallpaper::current();
    l
}

impl App for SampleTile {
    fn title(&self) -> &'static str {
        "SAMPLE"
    }
    fn icon(&self) -> Icon {
        Icon::Clipboard
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "Tab type a note · Enter add · Space check · Up Down choose · Del remove · Esc back".into()
    }
    fn wants_keyboard(&self) -> bool {
        self.host.wants_keyboard()
    }
    fn place(&mut self, w: f32, h: f32, view: &str, index: usize, count: usize) {
        let pane = iyakki_ui_sdk::desktop::Pane { screen: self.host.pane.screen, view: view.to_string(), w, h };
        self.host.place(pane, iyakki_ui_sdk::desktop::ViewInfo { name: view.to_string(), index, count });
    }
    fn take_jump(&mut self) -> Option<(String, String)> {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).take().map(|a| (a, String::new()))
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.bind(cx);
        // the heartbeat carries the tile's clock, so the embedded views can pace themselves
        let flow = match ev {
            Event::Tick => Some(self.host.tick(self.started.elapsed().as_millis() as u64)),
            _ => Self::map(ev).map(|e| self.host.event(&e)),
        };
        match flow {
            Some(UiFlow::Back) => Flow::Back,
            _ => Flow::Stay,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        self.bind(cx);
        let want = (area.w.round() as u32, area.h.round() as u32, s);
        if want != self.shape {
            self.shape = want;
            self.host.arrangement = Arrangement::new(vec![Screen { id: 0, name: "screen".into(), width: want.0, height: want.1, scale: want.2, x: 0, y: 0, primary: true }]);
            self.apply_saved_screens(cx);
        }
        self.host.look = look_now();
        self.host.type_style = super::sample::type_now();
        self.host.draw(&mut RasterCanvas::new(g, area, s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound_cx(tag: &str) -> (AppCx, Arc<iyakki_core_fake::FakeCore>) {
        let mut cx = crate::apps::tests::cx_with_admin_store(tag);
        let core = Arc::new(iyakki_core_fake::FakeCore::new());
        cx.core = core.clone();
        (cx, core)
    }

    #[test]
    fn the_notes_the_pin_and_the_wallpaper_go_through_the_core_and_are_audited() {
        let (cx, core) = bound_cx("sample-core");
        let user = cx.session.user.clone();
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        assert!(t.bound);
        // a note is added and kept by the core
        t.host.app.focused = true;
        for c in "milk".chars() {
            t.handle(&Event::Key(Key::Char(c)), &cx);
        }
        t.handle(&Event::Key(Key::Enter), &cx);
        let kept = core.app_data().get(&user, "sample", "notes").expect("the core holds the notes");
        assert!(kept.contains("0 milk"), "{kept}");
        // a new tile (another sign-in) gets them back
        let mut t2 = SampleTile::new();
        t2.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        assert!(t2.host.app.items.iter().any(|i| i.text == "milk"));
        // pin and unpin change the person's dock list and say so in the audit
        assert!(!t2.host.dock.pinned().iter().any(|a| a == "sample"));
        t2.host.dock.pin("sample").unwrap();
        assert!(core.desktop().dock(&user).unwrap().iter().any(|a| a == "sample"));
        assert!(core.desktop().dock(&user).unwrap().len() > 1, "the usual apps are kept when the first pin makes the list the person's own");
        assert_eq!(t2.host.dock.pin("no-such-app"), Err(DeskError::UnknownApp));
        t2.host.dock.unpin("sample").unwrap();
        assert!(!core.desktop().dock(&user).unwrap().iter().any(|a| a == "sample"));
        // the wallpaper
        t2.host.wallpaper.set(0, iyakki_uikit::wallpaper::Id::Dusk).unwrap();
        assert_eq!(core.desktop().wallpaper(&user).as_deref(), Some("dusk"));
        assert_eq!(t2.host.wallpaper.get(0), iyakki_uikit::wallpaper::Id::Dusk);
        iyakki_uikit::wallpaper::set_current(iyakki_uikit::wallpaper::Id::Theme);
        let log = cx.audit.recent(20).join("\n");
        assert!(log.contains("desktop.dock.pin sample") && log.contains("desktop.dock.unpin sample") && log.contains("desktop.wallpaper screen 0 dusk"), "{log}");
        // another screen keeps its own wallpaper and leaves the person's alone
        t2.host.wallpaper.set(2, iyakki_uikit::wallpaper::Id::Meadow).unwrap();
        assert_eq!(core.desktop().screen_wallpaper(&user, 2).as_deref(), Some("meadow"));
        assert_eq!(core.desktop().wallpaper(&user).as_deref(), Some("dusk"));
        assert_eq!(t2.host.wallpaper.get(2), iyakki_uikit::wallpaper::Id::Meadow);
        assert_eq!(t2.host.wallpaper.get(5), iyakki_uikit::wallpaper::Id::Dusk, "a screen with no choice of its own has the person's");
    }

    #[test]
    fn dock_open_asks_the_shell_for_an_app_the_person_may_use_once_and_is_audited() {
        let (cx, _core) = bound_cx("sample-dock-open");
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        assert_eq!(t.take_jump(), None);
        assert_eq!(t.host.dock.open("no-such-app"), Err(DeskError::UnknownApp));
        assert_eq!(t.take_jump(), None, "an unknown app is not asked for");
        t.host.dock.open("markdown").unwrap();
        assert_eq!(t.take_jump(), Some(("markdown".to_string(), String::new())));
        assert_eq!(t.take_jump(), None, "taken once");
        assert!(cx.audit.recent(20).join("\n").contains("desktop.dock.open markdown"));
    }

    #[test]
    fn the_files_pane_lists_the_real_home_folder_through_the_real_core_and_never_leaves_it() {
        let cx = crate::apps::tests::cx_with_admin_store("sample-files");
        let user = cx.session.user.clone();
        let home = crate::agent().host_probe().home_root_for_writes(&user, cx.data_dir());
        std::fs::create_dir_all(home.join("Projects")).unwrap();
        std::fs::write(home.join("todo.txt"), "buy bread\nand milk").unwrap();
        std::fs::write(home.join("Projects/idea.md"), "# Idea").unwrap();
        std::fs::write(home.join(".ssh_key"), "private").unwrap();
        let outside = cx.data_dir().join("outside-secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        let _ = std::os::unix::fs::symlink(&outside, home.join("link.txt"));
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        assert!(t.host.app.has_files(), "bound with the core's files");
        t.host.event(&UiEvent::Key(UiKey::Char('f')));
        t.host.event(&UiEvent::Tick);
        let names = t.host.app.files().unwrap().names();
        assert!(names.contains(&"Projects/".to_string()) && names.contains(&"todo.txt".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("ssh_key") || n.contains("link")), "hidden names and links are not listed: {names:?}");
        t.host.event(&UiEvent::Key(UiKey::Enter));
        assert_eq!(t.host.app.files().unwrap().dir, "Projects");
        t.host.event(&UiEvent::Key(UiKey::Enter));
        assert_eq!(t.host.app.note, "Projects/idea.md: # Idea");
        // the port refuses what leaves the home, in the core's words
        let mut p = CoreFilesPort { core: cx.core.clone(), user: user.clone() };
        use iyakki_ui_sdk::files::{FilesError, FilesPort};
        for bad in ["..", "../x", "/etc", "link.txt", ".ssh_key"] {
            assert!(matches!(p.list(bad), Err(FilesError::Refused(_))), "{bad}");
            assert!(matches!(p.read_text(bad, 1000), Err(FilesError::Refused(_))), "{bad}");
        }
        assert_eq!(p.read_text("todo.txt", 1000).unwrap(), "buy bread\nand milk");
        // another person has another home
        let mut other = CoreFilesPort { core: cx.core.clone(), user: "bob".into() };
        assert!(other.list("").unwrap().entries.iter().all(|e| e.name != "todo.txt"));
        // a core with no files says so
        let mut none = CoreFilesPort { core: Arc::new(NoFilesCore), user };
        assert_eq!(none.list(""), Err(FilesError::Unavailable));
    }

    #[test]
    fn files_are_changed_through_the_real_core_only_after_the_person_allows_and_every_step_is_audited_by_name() {
        use iyakki_ui_sdk::files::{Change, FilesError, FilesPort};
        let cx = crate::apps::tests::cx_with_admin_store("sample-files-write");
        let user = cx.session.user.clone();
        let home = crate::agent().host_probe().home_root_for_writes(&user, cx.data_dir());
        std::fs::write(home.join("note.txt"), "old words").unwrap();
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        cx.browser().set_audit(Arc::new(move |u: &str, e: &str, d: &str| l2.lock().unwrap().push(format!("{u} {e} {d}"))));
        let mut p = CoreFilesPort { core: cx.core.clone(), user: user.clone() };
        // a new folder: asked, nothing made, then allowed
        let a = p.change(&Change::MakeDir { dir: String::new(), name: "Pictures".into() }).unwrap();
        assert!(a.summary.contains("Pictures") && !home.join("Pictures").exists());
        assert!(matches!(p.decide("f-not-this", true), Err(FilesError::Refused(_))));
        assert!(p.decide(&a.call, true).unwrap().contains("Made"));
        assert!(home.join("Pictures").is_dir());
        assert!(matches!(p.decide(&a.call, true), Err(FilesError::Refused(_))), "answered once");
        // a save: a no leaves the old text, a yes replaces it whole
        let a = p.change(&Change::Save { path: "note.txt".into(), text: "new words".into() }).unwrap();
        assert_eq!(std::fs::read_to_string(home.join("note.txt")).unwrap(), "old words");
        assert!(p.decide(&a.call, false).unwrap().contains("said no"));
        assert_eq!(std::fs::read_to_string(home.join("note.txt")).unwrap(), "old words");
        let a = p.change(&Change::Save { path: "note.txt".into(), text: "new words".into() }).unwrap();
        p.decide(&a.call, true).unwrap();
        assert_eq!(std::fs::read_to_string(home.join("note.txt")).unwrap(), "new words");
        // rename, then delete to the trash (moved, not erased)
        let a = p.change(&Change::Rename { path: "note.txt".into(), new_name: "memo.txt".into() }).unwrap();
        p.decide(&a.call, true).unwrap();
        assert!(home.join("memo.txt").exists() && !home.join("note.txt").exists());
        let a = p.change(&Change::Trash { path: "memo.txt".into() }).unwrap();
        p.decide(&a.call, true).unwrap();
        assert!(!home.join("memo.txt").exists());
        let trash = cx.data_dir().join("trash");
        let moved: Vec<_> = std::fs::read_dir(&trash).unwrap().flatten().flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten().collect::<Vec<_>>()).collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read_to_string(moved[0].path()).unwrap(), "new words");
        // what the app may not ask for
        for bad in [Change::Trash { path: "..".into() }, Change::MakeDir { dir: "../x".into(), name: "y".into() }, Change::Rename { path: "Pictures".into(), new_name: "a/b".into() }, Change::Save { path: ".secret".into(), text: "x".into() }, Change::Save { path: "/etc/passwd".into(), text: "x".into() }] {
            assert!(matches!(p.change(&bad), Err(FilesError::Refused(_))), "{bad:?}");
        }
        // another person cannot answer this person's question
        let a = p.change(&Change::MakeDir { dir: String::new(), name: "Mine".into() }).unwrap();
        let mut other = CoreFilesPort { core: cx.core.clone(), user: "bob".into() };
        assert!(matches!(other.decide(&a.call, true), Err(FilesError::Refused(_))));
        assert!(!home.join("Mine").exists());
        // four questions at most wait
        for i in 0..3 {
            p.change(&Change::MakeDir { dir: String::new(), name: format!("q{i}") }).unwrap();
        }
        assert!(matches!(p.change(&Change::MakeDir { dir: String::new(), name: "q9".into() }), Err(FilesError::Refused(m)) if m.contains("already waiting")));
        // the audit names what was asked and done, and never the text
        let log = lines.lock().unwrap().join("\n");
        assert!(log.contains("files.change.asked Make the folder `Pictures`") && log.contains("files.change.approved Make the folder `Pictures`") && log.contains("files.change.done Move `memo.txt` to the trash") && log.contains("files.change.denied Save `note.txt`"), "{log}");
        assert!(!log.contains("new words") && !log.contains("old words"), "{log}");
    }

    #[test]
    fn the_terminal_pane_runs_the_real_command_line_through_the_real_core_and_asks_before_a_change() {
        use iyakki_ui_sdk::terminal::{TermError, TerminalPort};
        let cx = crate::apps::tests::cx_with_admin_store("sample-term");
        let user = cx.session.user.clone();
        let home = crate::agent().host_probe().home_root_for_writes(&user, cx.data_dir());
        std::fs::write(home.join("hello.txt"), "line one\nline two").unwrap();
        let mut p = CoreTerminalPort::new(cx.core.clone(), user.clone());
        assert!(matches!(p.run("ls"), Err(TermError::Refused(_))), "not open yet");
        let o = p.open().unwrap();
        assert!(!o.prompt.is_empty() && o.banner.contains("terminal"));
        assert_eq!(p.run("echo hi there").unwrap().text.trim(), "hi there");
        assert!(p.run("cat hello.txt").unwrap().text.contains("line two"));
        assert!(p.run("ls").unwrap().text.contains("hello.txt"));
        // a change waits: nothing is made until the person says yes, and a second line is refused meanwhile
        let r = p.run("mkdir made").unwrap();
        let q = r.question.expect("a question waits");
        assert!(q.summary.contains("mkdir made") && !home.join("made").exists());
        assert!(matches!(p.run("ls"), Err(TermError::Refused(m)) if m.contains("waiting")));
        assert!(matches!(p.decide("not-that-call", true), Err(TermError::Refused(_))));
        assert!(p.decide(&q.call, false).unwrap().text.contains("said no"));
        assert!(!home.join("made").exists());
        let q = p.run("mkdir made").unwrap().question.unwrap();
        p.decide(&q.call, true).unwrap();
        assert!(home.join("made").is_dir(), "allowed: it ran");
        // the machine is never switched off from here, and a read through a pipe still runs without a card
        let r = p.run("poweroff").unwrap();
        assert!(r.question.is_none() && r.text.contains("Power button"));
        assert!(p.run("ls | wc").unwrap().question.is_none());
        // the person's own file scope applies: the CLI refuses a place outside it for a plain user
        p.close();
        assert!(matches!(p.run("ls"), Err(TermError::Refused(_))));
        // no account, no terminal
        let mut nobody = CoreTerminalPort::new(cx.core.clone(), "nobody-here".into());
        assert!(matches!(nobody.open(), Err(TermError::Refused(m)) if m.contains("no terminal")));
        // a core with no terminal says so
        let mut none = CoreTerminalPort::new(Arc::new(NoFilesCore), user);
        assert_eq!(none.open(), Err(TermError::Unavailable));
    }

    #[test]
    fn the_assistant_pane_goes_through_the_core_with_approval_audit_and_the_apps_own_tool() {
        use iyakki_core_fake::FakeTurn;
        let (cx, core) = bound_cx("sample-assist");
        core.assistant_fake().script(vec![FakeTurn::call("Adding.", "add_note", "{\"text\":\"tea\"}"), FakeTurn::say("Added tea.")]);
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        assert!(t.host.app.has_assistant(), "the tile hands the app the core's assistant once it has the session");
        for k in ['?', 'a', 'd', 'd'] {
            t.handle(&Event::Key(Key::Char(k)), &cx);
        }
        t.handle(&Event::Key(Key::Enter), &cx);
        let before = t.host.app.items.len();
        t.handle(&Event::Tick, &cx);
        assert_eq!(t.host.app.items.len(), before, "nothing changes before the person allows it");
        assert!(t.host.app.chat().unwrap().pending_approval().is_some());
        t.host.event(&UiEvent::Action(20_003));
        t.handle(&Event::Tick, &cx);
        assert_eq!(t.host.app.items.last().map(|i| i.text.as_str()), Some("tea"));
        assert_eq!(t.host.app.chat().unwrap().transcript().last().map(String::as_str), Some("Assistant: Added tea."));
        let log = core.assistant_fake().audit().join("\n");
        assert!(log.contains("assistant.open sample notes") && log.contains("assistant.tool sample add_note approved by user") && log.contains("assistant.tool sample add_note ran ok"), "{log}");
        // the model was shown the app's notes as context
        assert!(core.assistant_fake().seen()[0].contains("document Notes: 1 [ ] Try the wallpaper picker"));
    }

    #[test]
    fn the_editors_assistant_goes_through_the_core_and_its_reply_becomes_a_proposal() {
        use iyakki_core_fake::FakeTurn;
        let (cx, core) = bound_cx("sample-edit-assist");
        core.assistant_fake().script(vec![FakeTurn::say("<<<<<<< SEARCH\nResize the window\n=======\nResize the window twice\n>>>>>>> REPLACE")]);
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        t.host.event(&UiEvent::Key(UiKey::Char('e')));
        assert!(t.host.app.editor_open());
        let c = t.host.app.editor_chat().is_some();
        assert!(c, "the tile hands the editor the core's assistant");
        // focus the assistant's field by a tap on its region, type and send
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        let field = t.host.record().a11y().iter().position(|n| n.name == "Ask the assistant").is_some();
        assert!(field);
        t.host.event(&UiEvent::Action(53_000));
        t.host.event(&UiEvent::Text("make it twice".into()));
        t.host.event(&UiEvent::Key(UiKey::Enter));
        assert!(t.host.app.editor().unwrap().reviewing(), "{:?}", t.host.app.editor_chat().unwrap().transcript());
        let seen = core.assistant_fake().seen().join("\n");
        assert!(seen.contains("How to change the text") && seen.contains("Resize the window"), "{seen}");
        let log = core.assistant_fake().audit().join("\n");
        assert!(log.contains("assistant.open sample editor"), "{log}");
    }

    #[test]
    fn a_core_without_an_assistant_leaves_the_pane_saying_so_and_the_notes_working() {
        let (mut cx, _core) = bound_cx("sample-noassist");
        cx.core = Arc::new(NoAssistCore);
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, &cx, 0);
        for k in ['?', 'h', 'i'] {
            t.handle(&Event::Key(Key::Char(k)), &cx);
        }
        t.handle(&Event::Key(Key::Enter), &cx);
        assert_eq!(t.host.app.chat().unwrap().error(), Some("The assistant is not available here."));
    }

    /// A core that serves the mixer and statuses of the fake and the default (no) assistant.
    struct NoAssistCore;
    impl Core for NoAssistCore {
        fn mixer(&self) -> &dyn iyakki_core_api::Mixer {
            unreachable!("not used")
        }
        fn app_status(&self) -> &dyn iyakki_core_api::AppStatuses {
            unreachable!("not used")
        }
    }

    #[test]
    fn the_choices_kept_for_a_screen_reach_the_arrangement_the_app_sees() {
        use iyakki_uikit::look::DockPos;
        let (cx, core) = bound_cx("sample-screens");
        let user = cx.session.user.clone();
        core.desktop().set_screen_dock(&user, 0, "left").unwrap();
        core.desktop().set_screen_wallpaper(&user, 0, "slate").unwrap();
        let mut t = SampleTile::new();
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 0);
        assert_eq!(t.host.arrangement.dock_for(&t.host.look, 0), DockPos::Left);
        assert_eq!(t.host.arrangement.wallpaper_for(&t.host.look, 0), iyakki_uikit::wallpaper::Id::Slate);
    }

    #[test]
    fn a_person_who_may_not_open_an_app_cannot_pin_it() {
        let (mut cx, _core) = bound_cx("sample-deny");
        cx.session.role = iyakki_sec::policy::Role::User;
        cx.session.policy = iyakki_sec::policy::Role::User.template();
        let mut t = SampleTile::new();
        let mut g = Gfx::new(800, 480);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 800.0, h: 480.0 }, 1.0, &cx, 0);
        assert_eq!(t.host.dock.pin("users"), Err(DeskError::NotAllowed));
    }

    #[test]
    fn shell_keys_and_clicks_become_sdk_events_and_the_tile_draws_with_clickable_regions() {
        assert_eq!(SampleTile::map(&Event::Key(Key::Char('a'))), Some(UiEvent::Key(UiKey::Char('a'))));
        assert_eq!(SampleTile::map(&Event::Row(4)), Some(UiEvent::Action(4)));
        assert_eq!(SampleTile::map(&Event::Scroll(1)), Some(UiEvent::Wheel(1)));
        assert_eq!(SampleTile::map(&Event::MouseMove(1, 1)), None);
        let mut t = SampleTile::new();
        let mut g = Gfx::new(1280, 720);
        g.clear_hits();
        t.host.look = look_now();
        t.host.draw(&mut RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0));
        assert!(g.rgba().iter().any(|b| *b != 0), "something was drawn");
        // a click event from a hit region reaches the app as its action
        let before = t.host.app.items.len();
        t.host.app.focused = true;
        t.host.app.input = "x".into();
        t.host.event(&UiEvent::Action(2));
        assert_eq!(t.host.app.items.len(), before + 1);
        assert_eq!(t.host.event(&UiEvent::Key(UiKey::Esc)), UiFlow::Stay, "Esc first leaves the field");
        assert_eq!(t.host.event(&UiEvent::Key(UiKey::Esc)), UiFlow::Back);
    }

    /// A core that keeps no files and no terminal (only the defaults), for the "unavailable" answer.
    struct NoFilesCore;
    impl Core for NoFilesCore {
        fn mixer(&self) -> &dyn iyakki_core_api::Mixer {
            unreachable!("not used")
        }
        fn app_status(&self) -> &dyn iyakki_core_api::AppStatuses {
            unreachable!("not used")
        }
    }
}
