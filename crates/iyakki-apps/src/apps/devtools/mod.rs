//! DEV TOOLS in the shell: the tile is the crate `iyakki-tile-devtools` (thirteen small tools for developers on one tile; the tools themselves are plain
//! functions in `iyakki-devtools-engine`; alpha.8 E7). This file gives it the shell's side: the person's files under their file rules, the picture writer
//! and the clock for a saved QR picture, the audit line, and the two network tools that stay here (ICMP ping and the port checker open sockets, D-S1-6,
//! `net.rs`). `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use iyakki_devtools_engine::{b64, convert, diff, hashes, json, jwt, query, regex, tools, url, uuid_time};
pub use iyakki_tile_devtools::*;
use iyakki_tile_devtools::port::{DevToolsPort, NetJob};
use iyakki_tile_devtools::Flow as TileFlow;

pub mod net;
pub mod qrcode;

#[allow(hidden_glob_reexports)]
use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::ui::{Event, R};

impl NetJob for net::Job {
    fn finished(&self) -> bool {
        net::Job::finished(self)
    }
    fn stop(&self) {
        net::Job::stop(self)
    }
    fn text(&self) -> String {
        net::Job::text(self)
    }
}

/// The real port: the person's file rules, the picture writer and the network tools of the shell.
pub struct CxDevPort<'a>(pub &'a AppCx);

impl DevToolsPort for CxDevPort<'_> {
    fn read_text_file(&self, path: &str, max: u64) -> Result<String, String> {
        let cx = self.0;
        if !cx.session.policy.allows_read(&cx.session.user, path) {
            return Err(format!("This account may not read {path}."));
        }
        let meta = std::fs::metadata(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
        if !meta.is_file() || meta.len() > max {
            return Err(format!("{path} is not a file of at most {} MB.", max / 1024 / 1024));
        }
        let b = std::fs::read(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
        String::from_utf8(b).map_err(|_| format!("{path} is not UTF-8 text."))
    }
    fn home_dir(&self) -> PathBuf {
        PathBuf::from(format!("/home/{}", self.0.session.user))
    }
    fn save_picture(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let path = super::chatpic::save_file(&self.0.session.policy, &self.0.session.user, dir, name, bytes)?;
        crate::camera::own::give_saved(&path);
        Ok(path)
    }
    fn png_with_dpi(&self, w: u32, h: u32, rgba: &[u8], dpi: f32) -> Result<Vec<u8>, String> {
        crate::camera::pixels::png_with_dpi(w, h, rgba, dpi)
    }
    fn stamp(&self) -> String {
        crate::camera::stamp()
    }
    fn audit(&self, action: &str, detail: &str) {
        self.0.audit.record(&self.0.session.user, action, detail);
    }
    fn start_ping(&self, host: &str, count: usize) -> Result<Arc<dyn NetJob>, String> {
        net::start_ping(host, count).map(|j| j as Arc<dyn NetJob>)
    }
    fn start_ports(&self, host: &str, spec: &str) -> Result<Arc<dyn NetJob>, String> {
        net::start_ports(host, spec).map(|j| j as Arc<dyn NetJob>)
    }
}

pub struct DevToolsApp(iyakki_tile_devtools::DevToolsApp);

impl Deref for DevToolsApp {
    type Target = iyakki_tile_devtools::DevToolsApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DevToolsApp {
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

impl DevToolsApp {
    pub fn new(_cx: &AppCx) -> DevToolsApp {
        // the system's random bytes for UUIDs (the agent's generator: a tile does not read the kernel's devices itself)
        uuid_time::set_random(|n| crate::agent().sharedfolder_random_hex(n));
        DevToolsApp(iyakki_tile_devtools::DevToolsApp::new())
    }
}

impl App for DevToolsApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn page(&self) -> String {
        self.0.page()
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
        flow_of(self.0.handle(ev, &CxDevPort(cx)))
    }
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.0.scroll_px(px)
    }
    fn wants_redraw(&mut self) -> bool {
        self.0.wants_redraw()
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        self.0.render(g, area, s, tick);
    }
}

#[cfg(test)]
mod tests;
