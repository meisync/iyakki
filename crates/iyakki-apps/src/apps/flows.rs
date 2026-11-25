//! FLOWS in the shell: the tile is the crate `iyakki-tile-flows` (the workflow IDE, `docs/FLOWS.md`; alpha.8 N1). This file gives it the shell's
//! side: the account's flows jobs behind the model crate's `FlowsPort` (`flows_port.rs`), the assistant's pane (`ChatApp` with the flows link, behind
//! `ChatPane`), the person's files under their file rules (`FlowsShell::read_import`), the program's own picture files (`PictureFiles`) and the
//! forwarding of the `App` methods. The tile's own tests run in the crate against a fake port; the tests of the whole screen with the real
//! environment and the real chat are `flows_tests.rs` and `flows_jobs_tests.rs`. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::FlowsJobs;
use iyakki_flows_model::failfix::FailBook;
use iyakki_tile_flows::{AppFlow as TileFlow, ChatPane, DropAction as TileDrop, FlowsShell, PictureFiles};

use super::chat::{ChatApp, FlowsLink};
use super::component::route;
use super::flows_port::JobsPort;
use super::Flow as ShellFlow;
use super::{App, AppCx, DropAction};
use crate::ctxmenu::Item;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_flows::flows::*;
pub use iyakki_tile_flows::{flows_assist, flows_board, flows_canvas, flows_defaults, flows_fail, flows_hook, flows_icons, flows_keys, flows_paint, flows_props, flows_run, flows_tip, flows_view};

/// The shell's side of the tile for one call: the file rules of the core decide what may be imported.
pub struct CxPort<'a> {
    pub cx: &'a AppCx,
    pub env: &'a Arc<dyn FlowsJobs>,
}

impl FlowsShell for CxPort<'_> {
    fn shell(&self) -> &dyn std::any::Any {
        self.cx
    }
    fn agent_enabled(&self) -> bool {
        self.cx.session.policy.agent
    }
    fn read_import(&self, path: &str, max: u64) -> Result<String, String> {
        let (policy, user, svc) = (self.env.policy(), self.env.user(), self.env.svc_dir());
        let roots = crate::file_scope().roots(&policy, &user, &svc);
        let home = crate::agent().scope_home_root(&user, &svc);
        let real = roots.resolve(&home, path)?;
        if !roots.read_ok(&policy, &user, &real) {
            return Err("that file is outside the folders you may read".into());
        }
        let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
        if meta.len() > max {
            return Err("that file is over 2 MB".into());
        }
        std::fs::read_to_string(&real).map_err(|e| e.to_string())
    }
}

/// The program's own picture files: only what lies inside `<data>/artifacts` is read.
pub struct OwnPictures;

impl OwnPictures {
    fn inside(data: &Path, path: &str) -> Option<std::path::PathBuf> {
        let root = std::fs::canonicalize(data.join("artifacts")).ok()?;
        let real = std::fs::canonicalize(path).ok()?;
        real.starts_with(&root).then_some(real)
    }
}

impl PictureFiles for OwnPictures {
    fn pixmap(&self, data: &Path, path: &str, max: usize) -> Option<Arc<tiny_skia::Pixmap>> {
        let real = Self::inside(data, path)?;
        std::fs::read(real).ok().filter(|b| b.len() < max).and_then(|b| super::browser_images::to_pixmap(&b).ok()).map(Arc::new)
    }
    fn bytes(&self, data: &Path, path: &str) -> Option<Vec<u8>> {
        std::fs::read(Self::inside(data, path)?).ok()
    }
}

/// The assistant's pane: the chat app, in the click-id slot of the tile.
pub struct ShellChat(pub ChatApp);

fn app_of<'a>(cx: &'a dyn FlowsShell) -> &'a AppCx {
    cx.shell().downcast_ref::<AppCx>().expect("the shell's port carries the shell's context")
}

fn flow_of(f: ShellFlow) -> TileFlow {
    match f {
        ShellFlow::Stay => TileFlow::Stay,
        ShellFlow::Back => TileFlow::Back,
    }
}

impl ChatPane for ShellChat {
    fn handle(&mut self, ev: &Event, cx: &dyn FlowsShell) -> TileFlow {
        flow_of(App::handle(&mut self.0, ev, app_of(cx)))
    }
    fn decode_slot(&self, ev: &Event) -> Option<Event> {
        route::decode_slot(ev, CHAT_SLOT)
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &dyn FlowsShell, tick: u64) {
        let chat = &mut self.0;
        route::render_slot(g, CHAT_SLOT, |g| App::render(chat, g, area, s, app_of(cx), tick))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn is_running(&self) -> bool {
        self.0.is_running()
    }
    fn pending_approval(&self) -> bool {
        self.0.pending_approval().is_some()
    }
    fn local_note(&mut self, cx: &dyn FlowsShell, text: &str) {
        self.0.local_note(app_of(cx), text)
    }
    fn set_agent_mode(&mut self, on: bool) {
        self.0.set_agent_mode(on)
    }
    fn auto_active(&self) -> bool {
        self.0.approvals().is_auto() || self.0.autopilot()
    }
    fn end_auto_unless_pinned(&mut self) {
        self.0.end_auto_unless_pinned()
    }
    fn input_text(&self) -> String {
        self.0.input_text().to_string()
    }
    fn ask(&mut self, cx: &dyn FlowsShell, text: &str) {
        self.0.ask(app_of(cx), text)
    }
    fn prefill(&mut self, text: &str) {
        self.0.prefill(text)
    }
}

pub struct FlowsApp {
    tile: iyakki_tile_flows::FlowsApp,
    jobs: Arc<dyn FlowsJobs>,
}

impl Deref for FlowsApp {
    type Target = iyakki_tile_flows::FlowsApp;
    fn deref(&self) -> &Self::Target {
        &self.tile
    }
}

impl DerefMut for FlowsApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tile
    }
}

impl FlowsApp {
    pub fn new(cx: &AppCx) -> FlowsApp {
        let env = cx.services.flows_jobs(&cx.session.user, &cx.session.policy).expect("the core serves the flows jobs");
        // what the engine does goes to the core's audit log by name and place, never by value (the core owns the sink)
        FlowsApp::with_env(cx, env)
    }

    /// Tests give the tile an environment of their own.
    pub fn with_env(cx: &AppCx, env: Arc<dyn FlowsJobs>) -> FlowsApp {
        let chat_env = env.chat_handle();
        let tile = iyakki_tile_flows::FlowsApp::new(Arc::new(JobsPort(env.clone())), Box::new(OwnPictures), Box::new(FailBook::open(&env.data())), |open| {
            Box::new(ShellChat(ChatApp::new().with_agent(cx.session.policy.agent).with_flows(FlowsLink { env: chat_env, open }).resumed(cx)))
        });
        FlowsApp { tile, jobs: env }
    }

    /// Import a flow file through the shell's file rules (the tests' seam for what a person's Import dialog does).
    pub fn import_path_in(&mut self, cx: &AppCx, path: &str) -> Result<String, String> {
        let port = CxPort { cx, env: &self.jobs };
        self.tile.import_path(path, &port)
    }

    /// The assistant's chat app behind the pane (the tests read its thread).
    pub fn chat_app(&self) -> &ChatApp {
        &self.tile.chat.as_any().downcast_ref::<ShellChat>().expect("the shell's pane is a ShellChat").0
    }
}

impl App for FlowsApp {
    fn take_jump(&mut self) -> Option<(String, String)> {
        self.tile.take_jump()
    }
    fn title(&self) -> &'static str {
        self.tile.title()
    }
    fn icon(&self) -> Icon {
        self.tile.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.tile.accent()
    }
    fn hint(&self) -> String {
        self.tile.hint()
    }
    fn help(&self) -> Vec<String> {
        self.tile.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        let port = CxPort { cx, env: &self.jobs };
        flow_back(self.tile.handle(ev, &port))
    }
    fn wants_redraw(&mut self) -> bool {
        self.tile.wants_redraw()
    }
    fn pointer_hint(&self, x: i32, y: i32) -> Option<crate::ui::Pointer> {
        self.tile.pointer_hint(x, y)
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.tile.take_host_save()
    }
    fn on_drop(&mut self, paths: &[std::path::PathBuf], cx: &AppCx) -> DropAction {
        let port = CxPort { cx, env: &self.jobs };
        match self.tile.on_drop(paths, &port) {
            TileDrop::Taken => DropAction::Taken,
            TileDrop::Ignore(why) => DropAction::Ignore(why),
        }
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        self.tile.context_items(x, y)
    }
    fn context_pick(&mut self, id: &str, cx: &AppCx) -> ShellFlow {
        flow_back(self.tile.context_pick(id, &CxPort { cx, env: &self.jobs }))
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let port = CxPort { cx, env: &self.jobs };
        self.tile.render(g, area, s, &port, tick)
    }
}

fn flow_back(f: TileFlow) -> ShellFlow {
    match f {
        TileFlow::Stay => ShellFlow::Stay,
        TileFlow::Back => ShellFlow::Back,
    }
}
