//! IYAKKI shell: login, role-aware launcher, and the CHAT / RAG / MCP / CLI / Users apps.
#[cfg(test)]
mod testmem;
pub mod accounts;
pub use iyakki_uikit::gfx;
pub use iyakki_uikit::fx;
#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod upgrade_tests;
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
pub mod filepicker;
pub use iyakki_uikit::ui;
pub mod apps;
pub mod inputprofile;
pub use iyakki_uikit::scrolleng;
pub use iyakki_uikit::scrolllist;
pub use iyakki_uikit::listwindow;
pub use iyakki_uikit::tooltip;
#[cfg(test)]
mod filepicker_tests;
pub mod camera;
pub mod launcher;
pub mod shell;
pub mod samples;
pub mod backend;
pub mod pointer_sprites;
pub mod display;
pub mod splash;
pub mod external;
pub mod speechprobe;
pub mod fb;
pub mod panel;
pub mod saver;
pub mod clockbar;
pub mod cues;
pub mod devinput;
pub mod devpop;
pub mod devask;
pub mod about;
pub mod brandmenu;
pub mod agent_login;
pub mod agent_login_scan;
pub mod scan_core;
pub mod clip_scan;
pub mod installpop;
pub mod batterybar;
pub mod lossbar;
pub mod hostname;
pub mod input;
pub mod touchpad;
pub mod touchmap;
pub mod touchscreen;
pub mod fnkeys;
pub mod touchbar;
pub mod touchbar_host;
#[cfg(test)]
mod touchbar_tests;
pub mod loginbar;
pub mod cloudsession;
pub mod osk;
pub mod panes;
pub mod memgate;
pub mod backend_kms;
