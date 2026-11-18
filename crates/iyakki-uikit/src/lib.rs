//! The toolkit of the framebuffer shell: software drawing ([`gfx`]), the design system ([`fx`], [`theme`]), events, colours and
//! text fields ([`ui`]), complex-script shaping ([`shaping`]), the UI size level ([`uisize`]), the safe `clamp` ([`safeclamp`])
//! and the context menu ([`ctxmenu`]). No `iyakki-*` dependency: tile crates build on this and never on the agent.
//! `iyakki-shell` re-exports every module under its old path (`crate::gfx`, `crate::ui` ...), so tiles compile unchanged.

pub mod access;
pub mod braille;
pub mod appicon;
pub mod askdialog;
pub mod barside;
pub mod blink;
pub mod board;
pub mod blur;
pub mod card;
pub mod controls;
pub mod ctxmenu;
pub mod filepicker;
pub mod fonts;
pub mod fx;
pub mod ide_chrome;
pub mod gfx;
pub mod keys;
pub mod links;
pub mod listwindow;
pub mod look;
pub mod memo;
pub mod prof;
pub mod look_registry;
pub mod skins;
pub mod mdtext;
pub mod neon;
pub mod platform;
pub mod safeclamp;
pub mod scrolleng;
pub mod scrolllist;
pub mod shaping;
pub mod textsel;
pub mod wallpaper;
pub mod theme;
pub mod typeset;
pub mod usertheme;
pub mod tooltip;
pub mod ui;
pub mod uisize;
pub mod widgets;
