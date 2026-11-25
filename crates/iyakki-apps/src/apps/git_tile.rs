//! GIT in the shell: the tile is the crate `iyakki-tile-git` (hosts, repositories and the clones on this computer, `docs/GIT-AND-SSH.md`; alpha.8 N1).
//! This file gives it the shell's side: on every call it builds the real port (`git_port::CxPort`: the account, the connections, the AI IDE's
//! workspaces, the jobs of the Git engine, the picture tools) and forwards the `App` methods. The tile's tests (the hosts as scripted fakes, the
//! jobs as plans) are `git_tile_tests.rs`, run here against the real context; the AI IDE draws the same tile in its Git tab. `Deref` shows the
//! tile to them.

use std::ops::{Deref, DerefMut};

use super::git_port::{system_rig, CxPort};
use super::Flow as ShellFlow;
use super::{App, AppCx};
use crate::ctxmenu::Item;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_git::git_tile::*;
pub use iyakki_tile_git::port::Flow as TileFlow;

pub struct GitApp(iyakki_tile_git::GitApp);

impl Deref for GitApp {
    type Target = iyakki_tile_git::GitApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for GitApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn flow_of(f: TileFlow) -> ShellFlow {
    match f {
        TileFlow::Stay => ShellFlow::Stay,
        TileFlow::Back => ShellFlow::Back,
    }
}

impl GitApp {
    pub fn new(cx: &AppCx) -> GitApp {
        Self::with_rig(cx, if crate::TESTING { Rig::offline() } else { system_rig() })
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> GitApp {
        GitApp(iyakki_tile_git::GitApp::with_rig(&CxPort(cx), rig))
    }
}

impl App for GitApp {
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
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        flow_of(self.0.handle(ev, &CxPort(cx)))
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        self.0.context_items(x, y)
    }
    fn context_pick(&mut self, id: &str, cx: &AppCx) -> ShellFlow {
        flow_of(self.0.context_pick(id, &CxPort(cx)))
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render(g, area, s, &CxPort(cx), tick)
    }
}

#[cfg(test)]
#[path = "git_tile_tests.rs"]
mod tests;
