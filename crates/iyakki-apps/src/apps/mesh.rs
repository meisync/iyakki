//! MESH: the tile itself lives in the `iyakki-tile-mesh` crate (it does not depend on the agent or the shared crate). This file is the shell's
//! side of it: the adapter that makes the tile an `App` of the shell, over the real port in `mesh_port.rs` (the saved address, the account's
//! host policy, the Consul client with the vault token, the audit log). Look at and change a Consul cluster: the services registered and how
//! healthy each instance is, the machines in the cluster and the key/value store; the address is set by an administrator in Setup, the ACL
//! token goes into the system vault, every request is checked against the account's host policy and changes to the store are audited.

pub use super::mesh_port::load_address;
use super::mesh_port::{Bridge, CxPort, MeshPort};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_mesh::{Answer, Mode, FormKind, Tab, ACCENT};
use iyakki_tile_mesh as tile;

pub struct MeshApp {
    tile: tile::MeshTile,
}

impl MeshApp {
    pub fn new(cx: &AppCx) -> MeshApp {
        MeshApp::with_port(&CxPort(cx))
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn MeshPort) -> MeshApp {
        MeshApp { tile: tile::MeshTile::new(&Bridge(port)) }
    }

    /// Hand the tile the answers that came in (the tests drive this without a frame).
    #[cfg(test)]
    pub(super) fn poll(&mut self, port: &dyn MeshPort) {
        self.tile.poll(&Bridge(port));
    }

    /// Submit a form on a port of the caller's choosing.
    #[cfg(test)]
    pub(super) fn submit(&mut self, form: &iyakki_uikit::widgets::Form, kind: FormKind, port: &dyn MeshPort) {
        self.tile.submit(form, kind, &Bridge(port));
    }
}

#[cfg(test)]
impl std::ops::Deref for MeshApp {
    type Target = tile::MeshTile;
    fn deref(&self) -> &tile::MeshTile {
        &self.tile
    }
}

#[cfg(test)]
impl std::ops::DerefMut for MeshApp {
    fn deref_mut(&mut self) -> &mut tile::MeshTile {
        &mut self.tile
    }
}

impl App for MeshApp {
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
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.tile.handle(ev, &Bridge(&CxPort(cx))) {
            Answer::Stay => Flow::Stay,
            Answer::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, &Bridge(&CxPort(cx)), tick);
    }
}
