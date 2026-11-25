//! RUNS: the tile itself lives in the `iyakki-tile-runlog` crate (it does not depend on the agent or the shared crate). This file is the
//! shell's side of it: the port over the person's run store and jobs in the core (`runs_port.rs`, `jobs_ui.rs`) and the adapter that
//! makes the tile an `App` of the shell. The run records and their integrity check stay in `iyakki_shared::runs`.

use iyakki_tile_runlog as tile;

use super::jobs_ui::JobsCx;
use super::runs_port::{Bridge, CxPort, RunsPort};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use tile::{fmt_time, timeline};

pub struct RunsApp {
    pub(super) tile: tile::RunsTile,
}

impl RunsApp {
    pub fn new(cx: &AppCx) -> RunsApp {
        RunsApp::with_port(&CxPort(cx))
    }

    /// The tile over any run port (a fake in tests); the jobs are not asked for until the Jobs list is opened.
    pub fn with_port(port: &dyn RunsPort) -> RunsApp {
        RunsApp { tile: tile::RunsTile::new(&Bridge::runs_only(port)) }
    }

    pub fn runs(&self) -> &[iyakki_tile_api::runs::RunMeta] {
        self.tile.runs()
    }
    pub fn integrity(&self) -> Option<&iyakki_tile_api::runs::Integrity> {
        self.tile.integrity()
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.tile.status()
    }

    /// Open the Jobs list (the shell's link from a notification, and the `j` key).
    pub fn show_jobs(&mut self, cx: &AppCx) {
        self.tile.show_jobs(&JobsCx(cx));
    }

    pub fn jobs_mode(&self) -> bool {
        self.tile.jobs_mode()
    }

    pub fn jobs(&self) -> &tile::jobs::JobsUi {
        self.tile.jobs()
    }
}

impl App for RunsApp {
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
    fn wants_redraw(&mut self) -> bool {
        self.tile.wants_redraw()
    }
    /// A notification about a job opens the Jobs list.
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        self.tile.open_item(item, &Bridge::new(cx));
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.tile.take_request()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.tile.context_items(x, y)
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.tile.handle(ev, &Bridge::new(cx)) {
            tile::Answer::Stay => Flow::Stay,
            tile::Answer::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, &JobsCx(cx), tick);
    }
}
