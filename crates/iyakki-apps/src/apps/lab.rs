//! LAB in the shell: the tile is the crate `iyakki-tile-lab` (alpha.9 P5). This file gives it the shell's context: on every call it builds the real
//! port (`lab_port::CxPort`: the picture and vision backends the core serves, the catalogue, the file scope, the audit log) and forwards the `App`
//! methods. The tile's own tests run in the crate against a fake port; the tests here (`lab_tests.rs`) run it over the real services. `Deref` shows
//! the tile to the tests.

use std::ops::{Deref, DerefMut};

use super::lab_port::CxPort;
use super::{App, AppCx, Flow as ShellFlow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_lab::app::*;

pub struct LabApp(iyakki_tile_lab::LabApp);

impl Deref for LabApp {
    type Target = iyakki_tile_lab::LabApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for LabApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl LabApp {
    pub fn new(cx: &AppCx) -> LabApp {
        LabApp(iyakki_tile_lab::LabApp::with_port(&CxPort(cx)))
    }

    pub fn with_rig(cx: &AppCx, rig: Rig) -> LabApp {
        LabApp(iyakki_tile_lab::LabApp::with_port_rig(&CxPort(cx), rig))
    }
}

impl App for LabApp {
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
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        match self.0.handle_with(ev, &CxPort(cx)) {
            iyakki_tile_lab::Flow::Stay => ShellFlow::Stay,
            iyakki_tile_lab::Flow::Back => ShellFlow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render_with(g, area, s, &CxPort(cx), tick);
    }
}

#[cfg(test)]
#[path = "lab_tests.rs"]
mod tests;
