//! K8S in the shell: the tile is the crate `iyakki-tile-k8s` (a tile to pick a cluster and a namespace and explore it, docs/K8S-TILE.md; alpha.8 E7).
//! This file gives it the shell's side: the real port (`k8s_port.rs`: the session, the audit log, the core's files, the k3s service, the deploy kit),
//! and the forwarding of the `App` methods. The tile's own tests of the cluster switcher run in the crate; the tests of the whole screen with a fake
//! cluster and the real context are `k8s_tile_tests.rs`. `Deref` shows the tile to the tests and to the AI IDE, which shares its list and text widgets.

use std::ops::{Deref, DerefMut};

pub use iyakki_tile_k8s::tile::*;
use iyakki_tile_k8s::Flow as TileFlow;

use super::k8s_port::{CxPort, K8sPort};
use super::{App, AppCx, Flow};
use crate::ctxmenu::Item;
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub struct K8sApp(iyakki_tile_k8s::K8sApp);

impl Deref for K8sApp {
    type Target = iyakki_tile_k8s::K8sApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for K8sApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn flow_of(f: TileFlow) -> Flow {
    match f {
        TileFlow::Stay => Flow::Stay,
        TileFlow::Back => Flow::Back,
    }
}

impl K8sApp {
    pub fn new(cx: &AppCx) -> K8sApp {
        let port = CxPort(cx);
        K8sApp(iyakki_tile_k8s::K8sApp::with_port(&port, port.rig()))
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> K8sApp {
        K8sApp(iyakki_tile_k8s::K8sApp::with_port(&CxPort(cx), rig))
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn K8sPort, rig: Rig) -> K8sApp {
        K8sApp(iyakki_tile_k8s::K8sApp::with_port(port, rig))
    }
}

impl App for K8sApp {
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
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        flow_of(self.0.handle(ev, &CxPort(cx)))
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.0.take_host_save()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item> {
        self.0.context_items(x, y)
    }
    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> Flow {
        flow_of(self.0.context_pick(id))
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render(g, area, s, &CxPort(cx), tick);
    }
}

#[cfg(test)]
#[path = "k8s_tile_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "k8s_port_tests.rs"]
mod port_tests;
