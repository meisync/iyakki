//! EDIT and CLI written on the UI SDK, in the shell (F-SDK-34): the apps are the crates `iyakki-app-edit` and `iyakki-app-terminal`; this file
//! is the shell side they share, the same as SAMPLE's (`sample.rs`): shell events become SDK events (and the touch screen's pinch becomes the
//! SDK's pinch gesture, which changes the text size), the SDK's frame is drawn on the screen through the raster canvas, and the first time the
//! tile gets the session its ports become the core's (the person's own files, assistant and terminal, the dock, the wallpaper and the app's own
//! kept text size). The old EDIT and CLI screens stay; the person chooses the SDK version (`iyakki_shared::sdkapps`, Settings -> Appearance or
//! the chat tool `display_sdk_apps_set`) and the shell then opens this tile under the same app id, so the policy, the dock, the launcher and
//! the audit are the old app's own.

use iyakki_app_edit::EditApp;
use iyakki_app_terminal::CliApp;
use iyakki_core_api::Core;
use iyakki_ui_sdk::app::{Flow as UiFlow, UiApp};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper, Screen};
use iyakki_ui_sdk::event::{Gesture, UiEvent};
use iyakki_ui_sdk::files::MemFiles;
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::ports::{check_key, PortError, StoragePort};
use iyakki_ui_sdk::raster::RasterCanvas;
use iyakki_ui_sdk::terminal::MemTerminal;
use std::sync::Arc;

use super::sample::{look_now, type_now, CoreDock, CoreFilesPort, CoreTerminalPort, CoreWallpaper, SampleTile};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

/// What the shell says about the app: the old app's own title, icon and colour, so the tile is the same app to the person.
struct Look {
    title: &'static str,
    icon: Icon,
    accent: [u8; 4],
    hint: &'static str,
}

/// The app's kept text (the text size) in the core's app data for this person, under the app's own id.
pub struct AppStorage {
    core: Arc<dyn Core>,
    user: String,
    app: &'static str,
}

impl AppStorage {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &'static str) -> AppStorage {
        AppStorage { core, user: user.to_string(), app }
    }
}

impl StoragePort for AppStorage {
    fn get(&mut self, key: &str) -> Option<String> {
        self.core.app_data().get(&self.user, self.app, key)
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError> {
        check_key(key)?;
        self.core.app_data().set(&self.user, self.app, key, value).map_err(|e| if e.contains("256") { PortError::TooBig } else { PortError::Denied })
    }
    fn remove(&mut self, key: &str) -> Result<(), PortError> {
        self.core.app_data().remove(&self.user, self.app, key).map_err(|_| PortError::Denied)
    }
    fn keys(&mut self) -> Vec<String> {
        self.core.app_data().keys(&self.user, self.app)
    }
}

/// One SDK app as the shell's tile.
pub struct SdkAppTile<A: UiApp> {
    host: Host<A>,
    look: Look,
    /// Makes the app with the core's ports for this person (once, when the tile has seen the session).
    make: fn(&AppCx, &str) -> A,
    bound: bool,
    shape: (u32, u32, f32),
    /// Where the app was drawn last (the pinch is told in screen pixels; the app wants its own units).
    area: R,
    started: std::time::Instant,
    asked: Arc<std::sync::Mutex<Option<String>>>,
}

impl<A: UiApp> SdkAppTile<A> {
    fn with(app: A, look: Look, make: fn(&AppCx, &str) -> A) -> SdkAppTile<A> {
        let host = Host::new(app, look_now(), Arrangement::single(1280, 720), Box::new(MemDock::knowing(&[])), Box::new(MemWallpaper::default()));
        SdkAppTile { host, look, make, bound: false, shape: (1280, 720, 1.0), area: R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, started: std::time::Instant::now(), asked: Default::default() }
    }

    fn bind(&mut self, cx: &AppCx) {
        if self.bound {
            return;
        }
        self.bound = true;
        let user = cx.session.user.clone();
        if user.is_empty() {
            return;
        }
        self.host.app = (self.make)(cx, &user);
        self.host.dock = Box::new(CoreDock { core: cx.core.clone(), cx: cx.clone(), asked: self.asked.clone() });
        self.host.wallpaper = Box::new(CoreWallpaper { core: cx.core.clone(), cx: cx.clone() });
    }

    /// The SDK event for a shell event: the usual ones (`SampleTile::map`), and the touch screen's pinch in the app's own units.
    fn map(&self, ev: &Event) -> Option<UiEvent> {
        match ev {
            Event::Pinch { scale_milli, x, y } => Some(UiEvent::Gesture(Gesture::Pinch { scale: *scale_milli as f32 / 1000.0, x: (*x as f32 - self.area.x) / self.shape.2, y: (*y as f32 - self.area.y) / self.shape.2 })),
            Event::PinchEnd { scale_milli } => Some(UiEvent::Gesture(Gesture::PinchEnd { scale: *scale_milli as f32 / 1000.0 })),
            other => SampleTile::map(other),
        }
    }

    /// The app, for a test.
    pub fn app(&self) -> &A {
        &self.host.app
    }
}

impl<A: UiApp> App for SdkAppTile<A> {
    fn title(&self) -> &'static str {
        self.look.title
    }
    fn icon(&self) -> Icon {
        self.look.icon
    }
    fn accent(&self) -> [u8; 4] {
        self.look.accent
    }
    fn hint(&self) -> String {
        self.look.hint.into()
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
        let flow = match ev {
            Event::Tick => Some(self.host.tick(self.started.elapsed().as_millis() as u64)),
            _ => self.map(ev).map(|e| self.host.event(&e)),
        };
        match flow {
            Some(UiFlow::Back) => Flow::Back,
            _ => Flow::Stay,
        }
    }
    fn list_frame(&mut self, w: f32, h: f32, cx: &AppCx) -> Option<String> {
        self.bind(cx);
        self.host.look = look_now();
        self.host.type_style = type_now();
        self.host.resize(w, h);
        Some(self.host.record().to_text())
    }
    fn list_apply(&mut self, batch: &str, cx: &AppCx) -> Option<Result<(String, bool), String>> {
        self.bind(cx);
        self.host.look = look_now();
        self.host.type_style = type_now();
        Some(iyakki_ui_sdk::wire::apply_batch(&mut self.host, batch))
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, _tick: u64) {
        self.bind(cx);
        let want = (area.w.round() as u32, area.h.round() as u32, s);
        if want != self.shape {
            self.shape = want;
            self.host.arrangement = Arrangement::new(vec![Screen { id: 0, name: "screen".into(), width: want.0, height: want.1, scale: want.2, x: 0, y: 0, primary: true }]);
        }
        self.area = area;
        self.host.look = look_now();
        self.host.type_style = type_now();
        self.host.draw(&mut RasterCanvas::new(g, area, s));
    }
}

/// EDIT on the UI SDK: the person's files, the assistant beside the editor and the kept text size, all through the core.
pub type EditTile = SdkAppTile<EditApp>;
/// CLI on the UI SDK: the core's terminal (lines and the whole session) and the kept text size.
pub type CliTile = SdkAppTile<CliApp>;

fn make_edit(cx: &AppCx, user: &str) -> EditApp {
    let files = CoreFilesPort { core: cx.core.clone(), user: user.to_string() };
    let assistant = super::assistant_port::CoreAssistantPort::new(cx.core.clone(), user, iyakki_app_edit::ID);
    EditApp::new(Box::new(files)).with_assistant(Box::new(assistant)).with_storage(Box::new(AppStorage::new(cx.core.clone(), user, iyakki_app_edit::ID)))
}

fn make_cli(cx: &AppCx, user: &str) -> CliApp {
    CliApp::new(Box::new(CoreTerminalPort::for_app(cx.core.clone(), user.to_string(), iyakki_app_terminal::ID))).with_storage(Box::new(AppStorage::new(cx.core.clone(), user, iyakki_app_terminal::ID)))
}

impl EditTile {
    pub fn new() -> EditTile {
        SdkAppTile::with(EditApp::new(Box::new(MemFiles::new())), Look { title: "EDIT", icon: Icon::Edit, accent: super::edit::ACCENT, hint: "SDK version · pinch or the size buttons change the text size · Esc back" }, make_edit)
    }
}

impl Default for EditTile {
    fn default() -> Self {
        EditTile::new()
    }
}

impl CliTile {
    pub fn new() -> CliTile {
        SdkAppTile::with(CliApp::new(Box::new(MemTerminal::new())), Look { title: "CLI", icon: Icon::Cli, accent: super::cli::ACCENT, hint: "SDK version · type a line · a change asks first · pinch changes the text size · Esc back" }, make_cli)
    }
}

impl Default for CliTile {
    fn default() -> Self {
        CliTile::new()
    }
}

/// Whether the person chose the SDK version of this app (`iyakki_shared::sdkapps`; the old screen is the default).
pub fn chosen(cx: &AppCx, app: &str) -> bool {
    iyakki_shared::sdkapps::is_chosen(cx.core.app_data().get(&cx.session.user, iyakki_shared::sdkapps::APP, iyakki_shared::sdkapps::KEY).as_deref(), app)
}

/// The tile to open for `app` when the person chose the SDK version, else `None` (the old screen opens).
pub fn tile_for(cx: &AppCx, app: iyakki_sec::policy::App) -> Option<Box<dyn App>> {
    use iyakki_sec::policy::App as P;
    if !chosen(cx, app.id()) {
        return None;
    }
    match app {
        P::Edit => Some(Box::new(EditTile::new())),
        P::Cli => Some(Box::new(CliTile::new())),
        _ => None,
    }
}

/// Keep the person's choice for `app` (the Settings page and the chat tool call this; the shell reads it when the app opens).
pub fn set_chosen(core: &dyn Core, user: &str, app: &str, on: bool) -> Result<String, String> {
    use iyakki_shared::sdkapps as s;
    let data = core.app_data();
    let now = data.get(user, s::APP, s::KEY);
    let next = s::with(now.as_deref(), app, on)?;
    data.set(user, s::APP, s::KEY, &next)?;
    Ok(s::describe(Some(&next)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Key;
    use iyakki_ui_sdk::event::Phase;
    use iyakki_ui_sdk::list::Op;

    fn bound_cx(tag: &str) -> (AppCx, Arc<iyakki_core_fake::FakeCore>) {
        let mut cx = crate::apps::tests::cx_with_admin_store(tag);
        let core = Arc::new(iyakki_core_fake::FakeCore::new());
        cx.core = core.clone();
        (cx, core)
    }

    fn draw<A: UiApp>(t: &mut SdkAppTile<A>, cx: &AppCx) {
        let mut g = Gfx::new(1280, 720);
        t.render(&mut g, R { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 }, 1.0, cx, 0);
    }

    fn tap<A: UiApp>(t: &mut SdkAppTile<A>, name: &str) {
        let l = t.host.record();
        let (x, y) = l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"));
        t.host.mouse(x, y, Phase::Down, 0);
        t.host.mouse(x, y, Phase::Up, 60);
    }

    fn type_line(t: &mut CliTile, cx: &AppCx, line: &str) {
        t.handle(&Event::Paste(line.into()), cx);
        t.handle(&Event::Key(Key::Enter), cx);
    }

    #[test]
    fn the_two_tiles_are_the_old_apps_in_name_icon_and_colour_and_say_what_they_are() {
        let (e, c) = (EditTile::new(), CliTile::new());
        assert_eq!((e.title(), c.title()), ("EDIT", "CLI"));
        assert_eq!((e.accent(), c.accent()), (super::super::edit::ACCENT, super::super::cli::ACCENT));
        assert!(e.hint().starts_with("SDK version") && c.hint().starts_with("SDK version"));
        assert!(!e.wants_keyboard(), "the file list has no field in front: no keyboard");
        assert!(c.wants_keyboard(), "the command line has the focus at the start: the shell shows the keyboard button");
    }

    #[test]
    fn an_edit_tile_lists_the_persons_files_through_the_core_keeps_the_text_size_and_takes_the_pinch() {
        let (cx, core) = bound_cx("sdkapps-edit");
        let user = cx.session.user.clone();
        core.file_tree_fake().put_file(&user, "todo.txt", "buy bread");
        let mut t = EditTile::new();
        draw(&mut t, &cx);
        assert!(t.bound, "bound to the core on the first draw");
        t.handle(&Event::Tick, &cx);
        assert!(t.app().files().names().contains(&"todo.txt".to_string()), "{:?}", t.app().files().names());
        let before = t.app().zoom();
        assert_eq!(before, iyakki_app_edit::ZOOM_START);
        // two fingers apart: the text grows from the size it began at, and the end keeps it for the person
        t.handle(&Event::Pinch { scale_milli: 1500, x: 600, y: 300 }, &cx);
        t.handle(&Event::PinchEnd { scale_milli: 1500 }, &cx);
        let after = t.app().zoom();
        assert!(after > before && after <= iyakki_app_edit::ZOOM_MAX, "{before} -> {after}");
        assert_eq!(core.app_data().get(&user, "edit", iyakki_app_edit::ZOOM_KEY), Some(after.to_string()), "kept in the core under the app's own id");
        // the pinch is told in the app's own units: the screen scale and the area's corner are taken off
        let mut other = EditTile::new();
        draw(&mut other, &cx);
        assert_eq!(other.app().zoom(), after, "the next tile gets the size back");
        // a pinch that goes the other way never goes past the smallest size
        other.handle(&Event::Pinch { scale_milli: 10, x: 0, y: 0 }, &cx);
        other.handle(&Event::PinchEnd { scale_milli: 10 }, &cx);
        assert!(other.app().zoom() >= iyakki_app_edit::ZOOM_MIN);
    }

    #[test]
    fn a_cli_tile_runs_a_reading_line_asks_before_a_change_and_a_no_runs_nothing() {
        let (cx, core) = bound_cx("sdkapps-cli");
        let user = cx.session.user.clone();
        let mut t = CliTile::new();
        draw(&mut t, &cx);
        t.handle(&Event::Tick, &cx);
        type_line(&mut t, &cx, "echo hello");
        assert!(t.app().terminal().transcript().contains(&"ran: echo hello".to_string()), "{:?}", t.app().terminal().transcript());
        type_line(&mut t, &cx, "mkdir notes");
        assert!(t.app().terminal().pending_question().is_some(), "the core asked");
        assert!(!core.terminal_fake().log().iter().any(|l| l.contains("mkdir")), "nothing ran before the answer");
        tap(&mut t, "Deny");
        assert!(!core.terminal_fake().log().iter().any(|l| l.contains("mkdir")), "a no runs nothing");
        type_line(&mut t, &cx, "mkdir notes");
        tap(&mut t, "Allow");
        assert_eq!(core.terminal_fake().log().last().map(String::as_str), Some(format!("{user}: mkdir notes").as_str()));
        // the core knows the session by the app's own id, not SAMPLE's
        assert_eq!(t.look.title, "CLI");
    }

    #[test]
    fn a_cli_tile_opens_the_whole_session_only_after_the_yes_and_leaves_it() {
        let (cx, _core) = bound_cx("sdkapps-cli-full");
        let mut t = CliTile::new();
        draw(&mut t, &cx);
        t.handle(&Event::Tick, &cx);
        tap(&mut t, "Full");
        assert!(t.app().terminal().in_screen() && t.app().terminal().screen_text().is_none(), "asked, not running");
        t.handle(&Event::Key(Key::Char('y')), &cx);
        t.handle(&Event::Tick, &cx);
        assert!(t.app().terminal().screen_text().unwrap_or_default().contains("fake$"), "the session shows its screen");
        t.handle(&Event::Key(Key::Char('l')), &cx);
        t.handle(&Event::Key(Key::Char('s')), &cx);
        t.handle(&Event::Key(Key::Enter), &cx);
        t.handle(&Event::Tick, &cx);
        assert!(t.app().terminal().screen_text().unwrap_or_default().contains("ran: ls"), "{:?}", t.app().terminal().screen_text());
        tap(&mut t, "Leave");
        assert!(!t.app().terminal().in_screen());
    }

    #[test]
    fn the_choice_is_the_persons_own_and_only_edit_and_cli_have_a_version() {
        use iyakki_sec::policy::App as P;
        let (cx, core) = bound_cx("sdkapps-choice");
        let user = cx.session.user.clone();
        assert!(!chosen(&cx, "cli") && tile_for(&cx, P::Cli).is_none() && tile_for(&cx, P::Edit).is_none(), "the old screen is the default");
        let said = set_chosen(&*core, &user, "cli", true).unwrap();
        assert_eq!(said, "EDIT: the usual screen. CLI: the SDK version.");
        assert!(chosen(&cx, "cli") && !chosen(&cx, "edit"));
        assert_eq!(tile_for(&cx, P::Cli).map(|t| t.title()), Some("CLI"));
        assert!(tile_for(&cx, P::Edit).is_none(), "EDIT was not chosen");
        set_chosen(&*core, &user, "edit", true).unwrap();
        assert_eq!(tile_for(&cx, P::Edit).map(|t| t.title()), Some("EDIT"));
        assert!(tile_for(&cx, P::Chat).is_none(), "an app with no SDK version never gets one");
        assert!(set_chosen(&*core, &user, "chat", true).unwrap_err().contains("edit and cli"));
        // another person has nothing chosen
        assert_eq!(core.app_data().get("bob", iyakki_shared::sdkapps::APP, iyakki_shared::sdkapps::KEY), None);
        set_chosen(&*core, &user, "cli", false).unwrap();
        assert!(tile_for(&cx, P::Cli).is_none() && tile_for(&cx, P::Edit).is_some());
    }

    #[test]
    fn the_tiles_hand_a_dock_open_to_the_shell_once() {
        let (cx, _core) = bound_cx("sdkapps-dock");
        let mut t = CliTile::new();
        draw(&mut t, &cx);
        assert_eq!(t.take_jump(), None);
        t.host.dock.open("markdown").unwrap();
        assert_eq!(t.take_jump(), Some(("markdown".to_string(), String::new())));
        assert_eq!(t.take_jump(), None);
    }
}
