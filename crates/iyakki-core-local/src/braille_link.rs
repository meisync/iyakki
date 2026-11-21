//! The core's door to a braille display (F-A11Y-07): the displays that follow the USB HID braille standard are found through `/sys` and
//! driven through their `hidraw` node by `crate::devctl::braillehid`; the shell only gets the `BrailleCells` this makes.

use iyakki_core_api::{BrailleCells, BrailleKey};
use crate::devctl::braillehid::{self, Action, Display, Hid};
use crate::devctl::Roots;
use std::path::PathBuf;

/// A display seen through the `braillehid::Display` trait as the core's `BrailleCells`.
pub struct Cells<D: Display + Send> {
    pub display: D,
}

impl<D: Display + Send> BrailleCells for Cells<D> {
    fn width(&self) -> usize {
        self.display.cells()
    }

    fn put(&mut self, cells: &str) -> Result<(), String> {
        self.display.show(cells)
    }

    fn keys(&mut self) -> Vec<BrailleKey> {
        self.display
            .keys()
            .into_iter()
            .filter_map(braillehid::action)
            .map(|a| match a {
                Action::PanLeft => BrailleKey::PanLeft,
                Action::PanRight => BrailleKey::PanRight,
                Action::FocusPrev => BrailleKey::FocusPrev,
                Action::FocusNext => BrailleKey::FocusNext,
                Action::Route(n) => BrailleKey::Route(n),
            })
            .collect()
    }
}

/// Where the displays are looked for (the real machine, or a made-up tree in a test).
pub struct HidLink {
    pub roots: Roots,
    pub dev: PathBuf,
}

impl HidLink {
    pub fn real() -> HidLink {
        HidLink { roots: Roots::real(), dev: crate::kernelfacts::DEV.into() }
    }
}

/// The real machine's door: `/sys` and `/dev`.
pub static REAL: RealLink = RealLink;

pub struct RealLink;

impl iyakki_core_api::BrailleLink for RealLink {
    fn open(&self) -> Option<Box<dyn BrailleCells>> {
        HidLink::real().open()
    }

    fn status(&self) -> String {
        HidLink::real().status()
    }
}

impl iyakki_core_api::BrailleLink for HidLink {
    fn open(&self) -> Option<Box<dyn BrailleCells>> {
        let found = braillehid::find(&self.roots, &self.dev);
        found.iter().find_map(|f| Hid::open(f).ok()).map(|h| Box::new(Cells { display: h }) as Box<dyn BrailleCells>)
    }

    fn status(&self) -> String {
        match braillehid::find(&self.roots, &self.dev).first() {
            Some(f) => format!("A braille display with {} cells is connected.", f.layout.cells()),
            None => "No braille display that follows the USB HID braille standard is connected.".into(),
        }
    }
}

#[cfg(test)]
#[path = "braille_link_tests.rs"]
mod tests;
