//! The apps behind the launcher tiles. Each app gets the signed-in user's [`Session`] (and so
//! their policy) through [`AppCx`]; none of them can reach beyond what that policy allows.

use std::sync::{Arc, Mutex};

use iyakki_agent::chat::ChatBackend;
use iyakki_agent::config::AiConfig;
use iyakki_agent::mcp::ServerSpec;
use iyakki_agent::services::Services;
use iyakki_agent::HostApi;

use crate::accounts::{AuditLog, Session, UserStore};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, R, TEXT};

pub mod chat;
pub mod chat_view;
pub(crate) mod model_picker;
pub mod chatpic;
pub mod browser;
pub mod browser_assist;
pub mod browser_av;
pub mod browser_block;
pub mod browser_config;
pub mod browser_dark;
pub mod browser_data;
pub mod browser_full;
pub mod web_agent;
pub mod browser_consent;
pub mod browser_devtools;
pub mod browser_images;
pub mod browser_sound;
pub mod browser_speed;
pub mod browser_typo;
pub mod browser_logins;
pub mod browser_persist;
pub mod browser_layout;
pub mod cli;
pub mod controls;
pub mod dash;
pub mod dash_charts;
pub mod dash_query;
pub mod flows;
pub mod flows_board;
pub mod flows_canvas;
pub mod flows_keys;
pub mod flows_paint;
pub mod flows_props;
pub mod flows_run;
pub mod flows_view;
pub mod data;
pub mod dropattach;
pub mod hostfiles;
pub mod mcp;
pub mod media;
pub mod mediaplay;
pub mod multiline;
pub mod neon;
pub mod mesh;
pub mod radio;
pub mod rag;
pub mod runs;
pub mod scan;
pub mod skills;
pub mod store;
pub(crate) mod store_drivers;
pub mod usage;
pub mod voice;
mod voice_ui;
pub mod lab;
mod lab_pic;
mod lab_ui;
pub mod srepilot;
pub mod story;
pub mod system;
pub mod calendar;
pub mod camera;
pub mod card;
pub mod tile;
pub mod inbox;
pub mod jobs_ui;
pub mod edit;
pub mod board;
pub mod component;
pub mod ide;
pub mod ide_browser;
pub mod ide_bottom;
pub mod ide_chrome;
pub mod ide_deploy;
pub mod ide_k8s;
pub mod k8s_tile;
pub mod git_tile;
mod git_tile_repo;
mod git_tile_repo_ui;
mod git_tile_ui;
pub mod disks_tile;
mod disks_tile_ui;
pub mod cluster_host;
pub mod cluster_switch;
pub mod clusters_panel;
pub mod linuxwin;
mod k8s_tile_ui;
pub mod platform_services;
pub mod ide_git;
pub mod ide_git_ui;
pub mod git_wizard;
pub mod ide_git_connect;
pub mod ide_notes;
pub mod ide_tasks;
pub mod ide_tile;
pub mod ide_tasks_run;
pub mod ide_layout;
pub mod ide_tree;
pub mod ide_ui;
pub mod devices;
pub mod settings;
pub mod settings_nav;
pub mod users;
pub mod widgets;

/// What an app asks the shell to do with files dropped on the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// Offer to import them into the Uploads folder (the default).
    Uploads,
    /// Offer to import them into this folder (named by `label` in the dialog); the app is told what arrived.
    Into { dest: std::path::PathBuf, label: String },
    /// The app handled them itself.
    Taken,
    /// Nothing sensible to do here: say so.
    Ignore(String),
}

pub enum Flow {
    Stay,
    Back,
}

/// Everything an app may use. Cloning is cheap (shared handles).
#[derive(Clone)]
pub struct AppCx {
    pub session: Session,
    pub host: Arc<dyn HostApi>,
    pub cfg: AiConfig,
    pub chat: Arc<dyn ChatBackend>,
    pub audit: Arc<AuditLog>,
    pub store: Arc<Mutex<UserStore>>,
    pub mcp_servers: Vec<ServerSpec>,
    pub services: Arc<Services>,
    /// Web access for the browser (checked against the user's host policy by the browser itself).
    pub web: Arc<dyn iyakki_web::Fetch>,
}

pub trait App {
    fn title(&self) -> &'static str;
    fn icon(&self) -> Icon;
    fn accent(&self) -> [u8; 4];
    /// One-line key help shown in the header.
    fn hint(&self) -> String;
    /// Every key and click this screen understands, one per line, for the F1 help popup. By default the header
    /// hint split into its parts.
    fn help(&self) -> Vec<String> {
        self.hint().split(" · ").map(str::to_string).collect()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow;
    /// A smooth scroll step of `px` pixels of a nominal screen (positive = up; [`crate::scrolleng::NOTCH_PX`] is one wheel click) while
    /// nothing is drawn over this app. True when something under the pointer scrolled by it; false (the default) makes the shell send
    /// whole [`Event::Scroll`] notches instead, which every app understands.
    fn scroll_px(&mut self, _px: i32, _cx: &AppCx) -> bool {
        false
    }
    /// The app draws its own smooth pictures (the browser's full engine): the shell's wheel steps are not eased again for it.
    fn scroll_raw(&self) -> bool {
        false
    }
    /// A media key (play/pause, next, previous, stop) pressed while this app is open: true when the app took it. The shell does
    /// the rest (the radio keeps playing in the background, so it answers the keys when no app does).
    fn media_key(&mut self, _k: crate::fnkeys::FnKey, _cx: &AppCx) -> bool {
        false
    }
    /// An app the shell should open next (the INBOX opens the app a notification points to): its id and the item in it.
    fn take_request(&mut self) -> Option<(String, String)> {
        None
    }
    /// Something the person is watching or listening to goes on in this app (a story being read aloud): the screen stays on
    /// (`shell_saver.rs`). The default says no.
    fn keeps_screen_on(&self) -> bool {
        false
    }
    /// For the Touch Bar's AI strip: (an agent run is going, a tool call waits for a decision). Only the chat knows.
    fn touchbar_state(&self) -> (bool, bool) {
        (false, false)
    }
    /// The shell opened this app because another one asked for a particular thing in it (`platform:private-store` in Settings).
    fn open_item(&mut self, _item: &str, _cx: &AppCx) {}
    /// Something changed on screen without a key or click (a frame arrived from an embedded engine): the shell draws again.
    /// Asked after every heartbeat; true once per change.
    fn wants_redraw(&mut self) -> bool {
        false
    }
    /// Like [`App::wants_redraw`], but says how much: an app whose live picture is a rectangle of the screen (the browser's full
    /// engine) answers `Live` when only that picture changed, so the shell can paint just it ([`App::live_draw`]) instead of the
    /// whole scene. The default is `Full` whenever `wants_redraw` says so.
    fn redraw_kind(&mut self) -> browser_full::Redraw {
        if self.wants_redraw() {
            browser_full::Redraw::Full
        } else {
            browser_full::Redraw::None
        }
    }
    /// Paint only the changed live picture over the scene drawn last (after `redraw_kind` said `Live`, and only when the shell
    /// knows nothing else on screen changed or overlaps it). Returns the screen rectangle `(x, y, w, h)` written; `None` makes
    /// the shell draw the whole scene instead.
    fn live_draw(&mut self, _g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        None
    }
    /// The pointer shape the app wants at (x, y) when the shell's own hit regions have no opinion (an embedded engine
    /// draws its own page and knows what is under the pointer).
    fn pointer_hint(&self, _x: i32, _y: i32) -> Option<crate::ui::Pointer> {
        None
    }
    /// Files or folders dropped on the window (window mode only): what this app wants done with them.
    fn on_drop(&mut self, _paths: &[std::path::PathBuf], _cx: &AppCx) -> DropAction {
        DropAction::Uploads
    }
    /// A file the app wants saved on the host computer (window mode): its suggested name and bytes. The shell shows the save
    /// dialog (rooted at the host home or a writable shared folder; nothing is replaced silently).
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        None
    }
    /// The app asks for the options of one shared host folder (convert to a copy, stop sharing, ...): its name.
    fn take_host_mount_menu(&mut self) -> Option<String> {
        None
    }
    /// The files a drop brought in (copied into the destination the app asked for), or empty after a folder was shared.
    fn dropped_done(&mut self, _files: &[std::path::PathBuf], _cx: &AppCx) {}
    /// Entries the app adds to the right-click menu at (x, y) (the shell adds Cut / Copy / Paste / Select all over a text
    /// field and always ends with "Clipboard history…"). Entries with the ids `copy`, `cut`, `paste` and `selall` are
    /// carried out by the shell as Ctrl+C / X / V / A sent to the app; any other id comes back through [`App::context_pick`].
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        Vec::new()
    }
    /// An entry the app added to the context menu was chosen.
    fn context_pick(&mut self, _id: &str, _cx: &AppCx) -> Flow {
        Flow::Stay
    }
    /// A custom app answers with itself so the shell's tests can look inside (every other app: nothing).
    #[cfg(test)]
    fn as_any_tile(&mut self) -> Option<&mut tile::TileApp> {
        None
    }
    /// Draw into `area` (below the header). `tick` advances ~30 times a second.
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64);
}

/// Scrollback of coloured text lines, wrapped to the area width. `scroll` counts visual lines
/// from the bottom; the newest content is shown by default.
///
/// The text can be selected with the pointer (press and drag, double click a word, triple click an entry, Shift+click to
/// extend) and copied; the owner passes its events to [`ScrollLog::handle`] first.
pub struct ScrollLog {
    pub lines: Vec<(String, [u8; 4])>,
    pub scroll: usize,
    /// Selection over what was drawn last.
    pub pane: crate::textsel::Pane,
    lh: f32,
    /// Keep each entry on one line (cut with an ellipsis) instead of wrapping it.
    pub nowrap: bool,
}

/// The id of the area a scroll log registers for presses (`Event::Area`).
pub const LOG_AREA: usize = 61_100;

impl Default for ScrollLog {
    fn default() -> Self {
        ScrollLog { lines: Vec::new(), scroll: 0, pane: crate::textsel::Pane::new(LOG_AREA), lh: 16.0, nowrap: false }
    }
}

impl ScrollLog {
    pub fn push(&mut self, text: impl Into<String>, color: [u8; 4]) {
        self.lines.push((text.into(), color));
        self.scroll = 0;
        if self.lines.len() > 2000 {
            self.lines.drain(..500);
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
        self.pane.clear();
    }

    /// Presses, drags and Ctrl+C on the text; true when the event was used.
    pub fn handle(&mut self, ev: &Event) -> bool {
        self.pane.handle(ev)
    }

    /// One heartbeat: a drag held above or below the text scrolls it.
    pub fn on_tick(&mut self) {
        let a = self.pane.auto_scroll();
        if a != 0.0 {
            self.scroll_by((a / self.lh.max(1.0)).round() as i32 + a.signum() as i32);
            self.pane.refresh_drag();
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).max(0) as usize;
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, size: f32, face: Face) {
        let lh = g.line_height(size, face);
        let mut visual: Vec<([u8; 4], String)> = Vec::new();
        let mut entry_of: Vec<u32> = Vec::new();
        for (n, (t, c)) in self.lines.iter().enumerate() {
            if self.nowrap {
                visual.push((*c, g.fit(size, t, face, area.w)));
                entry_of.push(n as u32);
                continue;
            }
            for l in g.wrap(size, t, face, area.w) {
                visual.push((*c, l));
                entry_of.push(n as u32);
            }
        }
        let fit = (area.h / lh).floor().max(1.0) as usize;
        let max_scroll = visual.len().saturating_sub(fit);
        self.scroll = self.scroll.min(max_scroll);
        let end = visual.len() - self.scroll;
        let start = end.saturating_sub(fit);
        for (i, (c, l)) in visual[start..end].iter().enumerate() {
            g.text(area.x, area.y + i as f32 * lh, size, if *c == [0, 0, 0, 0] { TEXT } else { *c }, l, face);
        }
        // the selectable text: every line, those out of view without geometry
        self.lh = lh;
        self.pane.begin(area);
        for (i, (_, l)) in visual.iter().enumerate() {
            self.pane.line(g, area.x, area.y + (i as f32 - start as f32) * lh, lh, size, face, l, 0, entry_of[i]);
        }
        self.pane.end(g, crate::fx::CYAN);
    }
}

/// The blinking caret: on for half a second, off for half, and never while the pane being drawn does not own the keyboard or
/// the window has lost the system focus (see [`with_caret`]). Every text input and the editor take their caret from here.
pub fn blink(tick: u64) -> bool {
    crate::ui::input_focus_allowed() && (tick / 15) % 2 == 0
}

/// Whether anything that marks "typing goes here" (caret, current-line wash, bracket box) may be drawn now.
pub fn caret_allowed() -> bool {
    crate::ui::input_focus_allowed()
}

/// Draw with the caret allowed (`on`) or hidden: the AI IDE draws the editor and the chat side by side and only the pane
/// that has the keyboard may show a caret, so a blinking caret always means "typing goes here".
pub fn with_caret<T>(on: bool, f: impl FnOnce() -> T) -> T {
    crate::ui::with_input_focus(on, f)
}

#[cfg(test)]
mod disks_tile_tests;
#[cfg(test)]
mod linuxwin_tests;
#[cfg(test)]
mod ide_tests;
#[cfg(test)]
mod jobs_screens_tests;
#[cfg(test)]
mod dash_tests;
#[cfg(test)]
mod flows_tests;
#[cfg(test)]
pub(crate) mod lemonade_fake;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use iyakki_sec::policy::{Policy, Role};

    #[derive(Default)]
    pub struct MockHost {
        pub powered_off: Mutex<Option<String>>,
    }
    impl HostApi for MockHost {
        fn detect_env(&self) -> String {
            "baremetal".into()
        }
        fn net_status(&self) -> String {
            "eth0 Ethernet".into()
        }
        fn run_wasm(&self, _u: &str, _p: &Policy, _w: &[u8], _e: &str) -> Result<String, String> {
            Ok("ok".into())
        }
        fn power_off(&self) {
            *self.powered_off.lock().unwrap() = Some("off".into());
        }
        fn reboot(&self) {
            *self.powered_off.lock().unwrap() = Some("reboot".into());
        }
    }

    pub fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-shell-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An admin session (user "tester") with a store containing only that admin.
    /// Upstream that always fails: tests that need network behaviour install their own.
    pub struct Offline;
    impl iyakki_agent::iyakki_proxy::Upstream for Offline {
        fn request(&self, _m: &str, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    pub fn services_in(d: &std::path::Path, up: Arc<dyn iyakki_agent::iyakki_proxy::Upstream>) -> Arc<Services> {
        Services::open(&d.join("svc"), up.clone(), up).unwrap()
    }

    pub fn cx_with_admin_store(tag: &str) -> AppCx {
        let d = temp_dir(tag);
        let mut store = UserStore::open(&d.join("users.json")).unwrap();
        store.create_first_admin("root", "rootpass12").unwrap();
        AppCx {
            session: Session { user: "root".into(), role: Role::Admin, policy: Role::Admin.template() },
            host: Arc::new(MockHost::default()),
            cfg: AiConfig::parse("").unwrap(),
            chat: Arc::new(iyakki_agent::chat::Unconfigured),
            audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
            store: Arc::new(Mutex::new(store)),
            mcp_servers: vec![],
            services: services_in(&d, Arc::new(Offline)),
            web: Arc::new(iyakki_web::fetch::NoNetwork),
        }
    }

    pub fn cx_with_backend(tag: &str, backend: impl ChatBackend + 'static) -> (AppCx, std::path::PathBuf) {
        let mut cx = cx_with_admin_store(tag);
        cx.session.user = "tester".into();
        cx.chat = Arc::new(backend);
        let d = temp_dir(&format!("{tag}-x"));
        cx.audit = Arc::new(AuditLog::new(&d.join("audit.log")));
        (cx, d)
    }
}
