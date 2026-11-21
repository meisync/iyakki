//! CLI as an SDK app (F-SDK-34, batch 4): the IYAKKI command line and the whole-session terminal on `TerminalView`, written only against
//! `iyakki-ui-sdk`, so the same source runs in the shell, in a browser or client and as a WASM tile (`src/guest.rs`).
//!
//! * The command line: a line that changes something does not run; the core asks, the view shows Allow and Deny, and the answer goes back to
//!   the core. The app runs nothing itself.
//! * The whole session (`Full`): a real program on a pseudo-terminal, asked for once; every key goes to the program, the Leave button is the
//!   way out. A host that cannot carry it (a tile's door does not) says so in words.
//! * Text size: two buttons and the pinch (delivered to a tile that declares tile ABI 3, as this one does with feature `tile`); the size is
//!   kept through the storage port when there is one.
//!
//! The old CLI screen (`iyakki-apps` `cli*.rs`) stays until the apps move over (the owner's order: not before Impl is 100).

use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, Hit, Role};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::ports::StoragePort;
use iyakki_ui_sdk::terminal::TerminalPort;
use iyakki_ui_sdk::terminalview::{self, TerminalView};
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;
use iyakki_ui_sdk::zoom::{TextZoom, ZoomChange};

#[cfg(all(target_arch = "wasm32", feature = "tile"))]
mod guest;

/// The app id the launcher, the dock and the policy know it by.
pub const ID: &str = "cli";
/// The storage key of the text size.
pub const ZOOM_KEY: &str = "text-size";

const A_SMALLER: u32 = 1;
const A_BIGGER: u32 = 2;
const TERM_BASE: u32 = 1_000;
const _: () = assert!(terminalview::ACTIONS < 1_000);

pub struct CliApp {
    term: TerminalView,
    zoom: TextZoom,
    storage: Option<Box<dyn StoragePort>>,
}

impl CliApp {
    pub fn new(port: Box<dyn TerminalPort>) -> CliApp {
        CliApp { term: TerminalView::new(port, TERM_BASE), zoom: TextZoom::default(), storage: None }
    }

    /// The text size is read from, and kept in, this port (the core's, in a tile).
    pub fn with_storage(mut self, mut port: Box<dyn StoragePort>) -> CliApp {
        self.zoom = TextZoom::from_text(port.get(ZOOM_KEY).as_deref());
        self.storage = Some(port);
        self
    }

    pub fn terminal(&self) -> &TerminalView {
        &self.term
    }

    /// The text size as a thousandth of the person's own size.
    pub fn zoom(&self) -> u32 {
        self.zoom.milli()
    }

    fn keep_zoom(&mut self) {
        if let Some(s) = self.storage.as_mut() {
            let _ = s.set(ZOOM_KEY, &self.zoom.keep_text());
        }
    }
}

impl UiApp for CliApp {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> String {
        "CLI".into()
    }

    fn wants_keyboard(&self) -> bool {
        self.term.wants_keyboard()
    }

    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Gesture(g) => {
                if self.zoom.gesture(g) == ZoomChange::Ended {
                    self.keep_zoom();
                }
            }
            UiEvent::Tick => self.term.tick(),
            UiEvent::Action(A_SMALLER) => {
                self.zoom.smaller();
                self.keep_zoom();
            }
            UiEvent::Action(A_BIGGER) => {
                self.zoom.bigger();
                self.keep_zoom();
            }
            UiEvent::Action(a) if self.term.owns(*a) => {
                self.term.action(*a);
            }
            UiEvent::Wheel(d) => self.term.wheel(*d),
            UiEvent::Text(t) => {
                self.term.text(t);
            }
            UiEvent::Key(k) => {
                // the view takes every key it can use (the field, a question, a whole session); Esc it does not use leaves the app
                if !self.term.key(*k) && *k == UiKey::Esc {
                    self.term.close();
                    return Flow::Back;
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        let m = self.zoom.metrics(cx);
        let (sm, md) = (m.gap(Space::Sm), m.gap(Space::Md));
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let area = Rect::new(0.0, 0.0, w, h).inset(md);
        let t = m.min_target();
        let (bar, rest) = area.split_top(t);
        let bw = ((bar.w - sm) / 2.0).clamp(0.0, m.em(8.0));
        widgets::button(c, th, &m, Rect::new(bar.x, bar.y, bw, t), "A-", A_SMALLER, false);
        widgets::button(c, th, &m, Rect::new(bar.x + bw + sm, bar.y, bw, t), "A+", A_BIGGER, false);
        let x = bar.x + 2.0 * (bw + sm);
        c.hit(Rect::new(x, bar.y, (bar.right() - x).max(0.0), t), Hit::new(0, self.zoom.label(), Role::Text));
        self.term.draw(c, th, &m, Rect::new(rest.x, rest.y + sm, rest.w, (rest.h - sm).max(0.0)));
    }
}

#[cfg(test)]
mod tests;
