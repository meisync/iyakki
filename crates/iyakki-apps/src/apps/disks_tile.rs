//! DISKS (System group, F-DISKS-01..08): the tile itself lives in the `iyakki-tile-disks` crate (it knows the drive model and a `DisksPort`, no
//! engine, no agent). This file is the shell's side of it: the real port over the core's `DiskEngine` (`cx.os.disk_engine()`), the signed-in
//! person and the audit log, and the adapter that makes the tile an `App` of the shell. The tests of this tile run through this adapter
//! on the core's engine over a made-up machine (`disks_tile_tests.rs`).

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use iyakki_core_api::diskmodel::{Actor, DiskEngine, FormatRequest};
use iyakki_tile_disks as tile;

use super::{App, AppCx, Flow};
use crate::gfx::Icon;
use crate::ui::{Event, R};

pub use tile::ui::*;
pub use tile::{human_bytes, parse_size, DisksPort, DisksTile, Focus, Form, FormKind, Modal, Page, Review, Running, UnlockForm, ACCENT};
pub use tile::{A_DETAIL, A_LIST, FS_CHOICES, F_CONFIRM, F_FILTER, F_LABEL, F_SECRET, F_SIZE};
pub use tile::{T_ADD, T_ADV, T_ADV_CLOSE, T_ALLOW_WRITE, T_APPLY, T_BACK, T_CANCEL, T_CLEAR_FILTER, T_DELETE, T_DISMISS, T_DO_UNLOCK, T_EJECT, T_EJECT_ROW, T_EXEC, T_FORMAT, T_FS, T_GPT, T_LOCK, T_MBR, T_MOUNT, T_NEWTABLE, T_OPEN, T_PART, T_PROPS, T_REFRESH, T_RENAME, T_REREAD, T_RESIZE, T_RO, T_REVIEW, T_UNLOCK, T_UNMOUNT};

/// The owner when the account has no passwd entry: the user IYAKKI itself runs as.
#[cfg(not(any(test, feature = "testing")))]
fn process_owner() -> (u32, u32) {
    crate::host().self_owner()
}

/// The real port: the core's engine, the signed-in person (rights, owner ids, home) and the shell's audit log.
struct ShellPort<'a> {
    engine: Arc<dyn DiskEngine>,
    who: Actor,
    cx: &'a AppCx,
}

impl tile::DisksPort for ShellPort<'_> {
    fn engine(&self) -> Arc<dyn DiskEngine> {
        self.engine.clone()
    }
    fn who(&self) -> Actor {
        self.who.clone()
    }
    fn audit_sink(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let (audit, user) = (self.cx.audit.clone(), self.cx.session.user.clone());
        Arc::new(move |a: &str, d: &str| audit.record(&user, a, d))
    }
    fn take_unlock_request(&self) -> Option<String> {
        self.cx.agent.disksunlocktools_take_request(self.cx.data_dir(), &self.cx.session.user)
    }
    fn take_format_request(&self) -> Option<FormatRequest> {
        self.engine.take_format_request(self.cx.data_dir(), &self.cx.session.user)
    }
}

pub struct DisksApp {
    tile: tile::DisksTile,
    engine: Arc<dyn DiskEngine>,
    who: Actor,
}

impl Deref for DisksApp {
    type Target = tile::DisksTile;
    fn deref(&self) -> &tile::DisksTile {
        &self.tile
    }
}

impl DerefMut for DisksApp {
    fn deref_mut(&mut self) -> &mut tile::DisksTile {
        &mut self.tile
    }
}

impl DisksApp {
    pub fn new(cx: &AppCx) -> DisksApp {
        // the shell's tests (and the monkey) never see a real drive: they get the made-up machine
        #[cfg(any(test, feature = "testing"))]
        let engine: Arc<dyn DiskEngine> = iyakki_disksx::disks_fake::machine_engine();
        #[cfg(not(any(test, feature = "testing")))]
        let engine: Arc<dyn DiskEngine> = cx.os.disk_engine();
        DisksApp::with_engine(cx, engine, false)
    }

    pub fn with_engine(cx: &AppCx, engine: Arc<dyn DiskEngine>, inline: bool) -> DisksApp {
        // the drives go into the signed-in person's own home, owned by that account (its passwd entry; the IYAKKI process's own
        // user when the account has none)
        let user = cx.session.user.clone();
        #[cfg(any(test, feature = "testing"))]
        let (home, uid, gid) = (format!("/home/{user}"), 1000, 1000);
        #[cfg(not(any(test, feature = "testing")))]
        let (home, (uid, gid)) = (cx.agent.scope_home_root(&user, std::path::Path::new("/var/lib/iyakki")).display().to_string(), cx.agent.disks_account_owner(&user, "/etc/passwd").unwrap_or_else(process_owner));
        // exFAT, NTFS, resizing and the unlock helpers use the programs the person installed (the tests never start one)
        #[cfg(not(any(test, feature = "testing")))]
        engine.use_programs(&cx.clis().bin_dir().to_string_lossy());
        let who = Actor { user, role: cx.session.role, uid, gid, home };
        let port = ShellPort { engine: engine.clone(), who: who.clone(), cx };
        let tile = tile::DisksTile::new(&port, inline);
        DisksApp { tile, engine, who }
    }

    fn port<'a>(&self, cx: &'a AppCx) -> ShellPort<'a> {
        ShellPort { engine: self.engine.clone(), who: self.who.clone(), cx }
    }
}

impl App for DisksApp {
    fn dialog_title(&self) -> Option<String> {
        self.tile.dialog_title()
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
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let port = self.port(cx);
        match self.tile.handle_event(ev, &port) {
            tile::Flow::Stay => Flow::Stay,
            tile::Flow::Back => Flow::Back,
        }
    }
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.tile.scroll_px(px)
    }
    fn wants_redraw(&mut self) -> bool {
        self.tile.wants_redraw()
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, tick);
    }
}
