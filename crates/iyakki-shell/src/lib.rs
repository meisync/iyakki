//! IYAKKI shell: login, role-aware launcher, and the CHAT / RAG / MCP / CLI / Users apps.
#[cfg(test)]
mod testmem;
pub mod wiring;
/// The accounts live in their own crate (`iyakki-accounts`); this keeps every `iyakki_shell::accounts::...` path as it was.
pub mod accounts {
    pub use iyakki_accounts::*;
}
pub use iyakki_uikit::access;
pub use iyakki_uikit::appicon;
pub use iyakki_uikit::gfx;
pub use iyakki_uikit::fx;
#[cfg(test)]
mod apps_split_tests;
#[cfg(test)]
mod settings_displays_tests;
#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod upgrade_tests;
#[cfg(test)]
mod compat_tests;
pub use iyakki_uikit::shaping;
pub use iyakki_uikit::theme;
pub use iyakki_uikit::uisize;
pub use iyakki_uikit::links;
pub use iyakki_uikit::mdtext;
pub use iyakki_uikit::safeclamp;
pub mod crashlog;
#[cfg(test)]
mod monkey_tests;
pub use iyakki_uikit::textsel;
pub use iyakki_uikit::ctxmenu;
pub mod hwcheck;
pub use iyakki_uikit::ui;
pub use iyakki_uikit::scrolleng;
pub use iyakki_uikit::scrolllist;
pub use iyakki_uikit::listwindow;
pub use iyakki_uikit::tooltip;
pub mod launcher;
pub mod stale_stats;
pub mod shell;
pub mod backend;
pub mod pointer_sprites;
pub mod speechprobe;
pub mod fb;
pub mod screen;
pub mod panel;
pub mod saver;
pub mod clockbar;
pub mod cues;
pub mod devinput;
pub mod devask;
pub mod about;
pub mod brandmenu;
pub mod clip_scan;
pub mod installpop;
pub mod lossbar;
pub mod crashcard;
pub mod touchbar_host;
pub mod loginbar;
pub mod soundowner;
pub mod cloudsession;
pub mod backend_kms;

pub use iyakki_apps::{splash, filepicker, pathpick, apps, inputprofile, camera, display, samples, external, agent_login, agent_login_scan, scan_core, batterybar, btmodel, devpop, hostname, input, touchpad, touchmap, touchscreen, fnkeys, touchbar, osk, panes, memgate};
