//! REMOTE FILES: the tile itself lives in the `iyakki-tile-remotefiles` crate (it does not depend on the agent). This file is the shell's
//! side: the person's `remotefiles::Manager` (sessions, host key pins, transfer queue; shared with the assistant's tools) behind the
//! tile's `RemoteSource`, the conversion between the agent's types and the facade's, the person's vault and file rules, and the adapter
//! that makes the tile crate's `App` an `App` of the shell.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_agent_api::{RemoteKit};
use iyakki_tile_api::{App as TileApp, TileCx};

use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub const ACCENT: [u8; 4] = iyakki_tile_remotefiles::ACCENT;

/// The person's manager (the core's `RemoteKit`): their vault for passwords and keys, their file rules for the paths on this computer.
pub fn manager_for(cx: &AppCx) -> Arc<dyn RemoteKit> {
    let (user, policy, data) = (cx.session.user.clone(), cx.session.policy.clone(), cx.data_dir().to_path_buf());
    let core = cx.core.clone();
    let home = core.file_scope().roots(&policy, &user, &data).home;
    let downloads = home.join("Downloads");
    let _ = std::fs::create_dir_all(&downloads);
    let (p2, u2, d2) = (policy.clone(), user.clone(), data.clone());
    let local: Arc<dyn Fn(&str, bool) -> Result<PathBuf, String> + Send + Sync> = Arc::new(move |text: &str, write: bool| {
        let roots = core.file_scope().roots(&p2, &u2, &d2);
        let expanded = PathBuf::from(roots.expand(text));
        let path = if expanded.is_absolute() { expanded } else { roots.home.join(expanded) };
        if write {
            let name = path.file_name().ok_or("that path has no file name")?.to_owned();
            let parent = std::fs::canonicalize(path.parent().ok_or("that path has no folder")?).map_err(|_| "that folder does not exist or is not allowed".to_string())?;
            let real = parent.join(name);
            if roots.contains(&parent) && roots.write_ok(&p2, &u2, &real) { Ok(real) } else { Err("outside the folders your account may write to".to_string()) }
        } else {
            let real = std::fs::canonicalize(&path).map_err(|_| "not found, or not allowed".to_string())?;
            if roots.read_ok(&p2, &u2, &real) { Ok(real) } else { Err("outside the folders your account may read".to_string()) }
        }
    });
    let audit = cx.audit.clone();
    let who = user.clone();
    cx.services.remote_kit(&user, &data, &downloads.to_string_lossy(), local, Arc::new(move |e, d| audit.record(&who, e, d)))
}

/// The tile crate's app, as an app of the shell.
pub struct RemoteFilesApp(iyakki_tile_remotefiles::RemoteFilesApp);

impl RemoteFilesApp {
    pub fn new(_cx: &AppCx) -> RemoteFilesApp {
        RemoteFilesApp(iyakki_tile_remotefiles::RemoteFilesApp::new())
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let src = manager_for(cx);
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &super::usage::ShellUsage::of(cx), calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &*src, cameras: &iyakki_tile_api::cameras::NoCameras, radio: &iyakki_tile_api::radio::NoRadio })
    }
}

impl App for RemoteFilesApp {
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
        match Self::with(cx, |t| self.0.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.0.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
#[path = "remotefiles_tests.rs"]
mod tests;
