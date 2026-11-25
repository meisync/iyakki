//! DASH in the shell: the tile is the crate `iyakki-tile-dash` (dashboards with graphs and tables over pluggable data sources, `docs/DASHBOARD.md`;
//! alpha.8 E3). This file gives it the shell's side: the engine's environment (the host, the audit sink, the accounts of this machine for sharing),
//! the assistant's pane (`ChatApp` with the dashboard link, behind `ChatPane`), the person's files under their file rules (`DashPort`), and the
//! forwarding of the `App` methods. The tile's own tests (the maths of the charts, the query box) run in the crate; the tests of the whole screen
//! with fake connectors and the real chat are `dash_tests.rs`. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use iyakki_dash::tools::{Env, People};
use iyakki_tile_dash::{ChatPane, DashPort, Flow as TileFlow};

use super::chat::{ChatApp, DashLink};
use super::component::route;
use super::Flow as ShellFlow;
use super::{App, AppCx};
use crate::ctxmenu::Item;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_dash::app::*;

/// The accounts of this machine, for sharing: names from the user store, homes where the agent puts them.
struct StorePeople(crate::accounts::UsersRef);

impl People for StorePeople {
    fn exists(&self, name: &str) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(name).is_some()
    }
    fn home(&self, name: &str, data_dir: &std::path::Path) -> std::path::PathBuf {
        crate::agent().scope_home_root(name, data_dir)
    }
}

/// The person's files for one call: the file rules of the core decide.
pub struct CxPort<'a>(pub &'a AppCx);

impl DashPort for CxPort<'_> {
    fn shell(&self) -> &dyn std::any::Any {
        self.0
    }
    fn read_dashboard_file(&self, env: &Env, path: &str, max: u64) -> Result<String, String> {
        let roots = crate::file_scope().roots(&env.policy, &env.user, &env.data_dir);
        let real = roots.resolve(&env.home, path)?;
        if !roots.read_ok(&env.policy, &env.user, &real) {
            return Err("you may not read that file".into());
        }
        let meta = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
        if meta.len() > max {
            return Err("that file is too large to be a dashboard".into());
        }
        std::fs::read_to_string(&real).map_err(|e| format!("{}: {e}", real.display()))
    }
    fn write_export(&self, env: &Env, name: &str, bytes: &[u8]) -> Result<bool, String> {
        let dir = env.home.join("Dashboards").join("exports");
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(name), bytes)).map_err(|e| e.to_string())?;
        Ok(iyakki_shared::hostfiles::enabled())
    }
}

/// The assistant's pane: the chat app, in the click-id slot of the tile.
struct ShellChat(ChatApp);

fn app_of<'a>(cx: &'a dyn DashPort) -> &'a AppCx {
    cx.shell().downcast_ref::<AppCx>().expect("the shell's port carries the shell's context")
}

fn flow_of(f: ShellFlow) -> TileFlow {
    match f {
        ShellFlow::Stay => TileFlow::Stay,
        ShellFlow::Back => TileFlow::Back,
    }
}

impl ChatPane for ShellChat {
    fn handle(&mut self, ev: &Event, cx: &dyn DashPort) -> TileFlow {
        flow_of(App::handle(&mut self.0, ev, app_of(cx)))
    }
    fn decode_slot(&self, ev: &Event) -> Option<Event> {
        route::decode_slot(ev, CHAT_SLOT)
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &dyn DashPort, tick: u64) {
        let chat = &mut self.0;
        route::render_slot(g, CHAT_SLOT, |g| App::render(chat, g, area, s, app_of(cx), tick))
    }
    fn wants_redraw(&mut self) -> bool {
        App::wants_redraw(&mut self.0)
    }
    fn is_running(&self) -> bool {
        self.0.is_running()
    }
    fn pending_approval(&self) -> bool {
        self.0.pending_approval().is_some()
    }
    fn prefill(&mut self, text: &str) {
        self.0.prefill(text)
    }
    fn input_text(&self) -> String {
        self.0.input_text().to_string()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        App::context_items(&mut self.0, x, y)
    }
    fn context_pick(&mut self, id: &str, cx: &dyn DashPort) -> TileFlow {
        flow_of(App::context_pick(&mut self.0, id, app_of(cx)))
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        App::take_request(&mut self.0)
    }
}

pub struct DashApp(iyakki_tile_dash::DashApp);

impl Deref for DashApp {
    type Target = iyakki_tile_dash::DashApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DashApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl DashApp {
    pub fn new(cx: &AppCx) -> DashApp {
        let host = cx.services.dash_host();
        let mut env = Env::from_host_in(&host, &cx.session.user, &cx.session.policy, host.home_root(&cx.session.user));
        // names and ids only reach the audit log, never a value
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        env.audit = Arc::new(move |kind, detail| audit.record(&user, kind, detail));
        env.people = Arc::new(StorePeople(cx.store.clone()));
        DashApp::with_env(cx, Arc::new(env))
    }

    /// Tests give the app an environment with fake connectors.
    pub fn with_env(cx: &AppCx, env: Arc<Env>) -> DashApp {
        let link_env = env.clone();
        DashApp(iyakki_tile_dash::DashApp::new(env, |state| {
            Box::new(ShellChat(ChatApp::new().with_agent(cx.session.policy.agent).with_dash(DashLink { env: link_env, state }).resumed(cx)))
        }))
    }
}

impl App for DashApp {
    fn dialog_title(&self) -> Option<String> {
        self.0.dialog_title()
    }
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.0.take_host_save()
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn pointer_hint(&self, x: i32, y: i32) -> Option<crate::ui::Pointer> {
        self.0.pointer_hint(x, y)
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        self.0.context_items(x, y)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn context_pick(&mut self, id: &str, cx: &AppCx) -> ShellFlow {
        match self.0.context_pick(id, &CxPort(cx)) {
            TileFlow::Stay => ShellFlow::Stay,
            TileFlow::Back => ShellFlow::Back,
        }
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        match self.0.handle(ev, &CxPort(cx)) {
            TileFlow::Stay => ShellFlow::Stay,
            TileFlow::Back => ShellFlow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render(g, area, s, &CxPort(cx), tick)
    }
}
