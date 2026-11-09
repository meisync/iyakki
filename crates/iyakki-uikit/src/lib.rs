//! The toolkit of the framebuffer shell: software drawing ([`gfx`]), the design system ([`fx`], [`theme`]), events, colours and
//! text fields ([`ui`]), complex-script shaping ([`shaping`]), the UI size level ([`uisize`]), the safe `clamp` ([`safeclamp`])
//! and the context menu ([`ctxmenu`]). No `iyakki-*` dependency: tile crates build on this and never on the agent.
//! `iyakki-shell` re-exports every module under its old path (`crate::gfx`, `crate::ui` ...), so tiles compile unchanged.

pub mod access;
pub mod blink;
pub mod card;
pub mod controls;
pub mod ctxmenu;
pub mod filepicker;
pub mod fx;
pub mod ide_chrome;
pub mod gfx;
pub mod keys;
pub mod links;
pub mod listwindow;
pub mod mdtext;
pub mod neon;
pub mod safeclamp;
pub mod scrolleng;
pub mod scrolllist;
pub mod shaping;
pub mod textsel;
pub mod theme;
pub mod tooltip;
pub mod ui;
pub mod uisize;
pub mod widgets;
