//! LINUX WINDOW: the tile itself lives in the `iyakki-tile-linuxwin` crate (it does not depend on the agent, the shared crate or the RFB crate).
//! This file is the shell's side of it: the adapter that makes the tile an `App` of the shell, over the real port in `linuxwin_port.rs` (who is
//! signed in and what the policy allows, the install job, the sandbox rig with its spawner, process table and RFB screen). The sandbox's safety
//! rules stay in `iyakki_shared::linuxwin`; the tile never builds a sandbox command.

use std::sync::Arc;

pub use super::linuxwin_port::Rig;
use super::linuxwin_port::{Bridge, CxPort, LinuxWinPort, ShellRig};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_linuxwin::{key_for_program, key_syms, layout, mem_text, paste_keys, terminal_like, to_remote, wanted_size, Answer, Phase, ACCENT, AREA_WIN};
use iyakki_tile_api::linuxwin::WinRig;
use iyakki_tile_linuxwin as tile;

pub struct LinuxWinApp {
    tile: tile::LinuxWinTile,
    rig: Arc<dyn WinRig>,
}

impl LinuxWinApp {
    pub fn new(cx: &AppCx) -> LinuxWinApp {
        let port = CxPort(cx);
        LinuxWinApp::with_port(&port, port.rig())
    }

    /// The tile with a rig of the caller's (tests) and the shell's context.
    pub fn with_rig(cx: &AppCx, rig: Rig) -> LinuxWinApp {
        LinuxWinApp::with_port(&CxPort(cx), rig)
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn LinuxWinPort, rig: Rig) -> LinuxWinApp {
        let rig: Arc<dyn WinRig> = Arc::new(ShellRig(rig));
        LinuxWinApp { tile: tile::LinuxWinTile::with_rig(&Bridge { port, rig: rig.clone() }, rig.clone()), rig }
    }

    fn bridge<'a>(&self, port: &'a dyn LinuxWinPort) -> Bridge<'a> {
        Bridge { port, rig: self.rig.clone() }
    }

    /// Read what is installed on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn refresh(&mut self, port: &dyn LinuxWinPort) {
        let b = self.bridge(port);
        self.tile.refresh(&b);
    }

    /// Install the base system on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn install_base(&mut self, port: &dyn LinuxWinPort) {
        let b = self.bridge(port);
        self.tile.install_base(&b);
    }
}

#[cfg(test)]
impl std::ops::Deref for LinuxWinApp {
    type Target = tile::LinuxWinTile;
    fn deref(&self) -> &tile::LinuxWinTile {
        &self.tile
    }
}

#[cfg(test)]
impl std::ops::DerefMut for LinuxWinApp {
    fn deref_mut(&mut self) -> &mut tile::LinuxWinTile {
        &mut self.tile
    }
}

impl Drop for LinuxWinApp {
    fn drop(&mut self) {
        self.tile.stop_all();
        crate::input::set_alt_passthrough(false);
    }
}

impl App for LinuxWinApp {
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
    fn wants_alt(&self) -> bool {
        self.tile.wants_alt()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let port = CxPort(cx);
        let b = self.bridge(&port);
        match self.tile.handle(ev, &b) {
            Answer::Stay => Flow::Stay,
            Answer::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let port = CxPort(cx);
        let b = self.bridge(&port);
        self.tile.render(g, area, s, &b, tick)
    }
}
