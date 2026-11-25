//! A Browse button and the file picker for a text box that is not a form (a tile's own path box): the screen keeps one [`PathPick`], draws
//! the button next to its box with [`PathPick::button`], sends events to [`PathPick::event`] first, and draws [`PathPick::render`] last.
//! The answer is the text the picker chose; the box and the checks that follow are the screen's own.

use crate::filepicker::{current_scope, FilePicker, PathKind, PickOut};
use crate::gfx::Gfx;
use crate::ui::{Event, R};
use std::path::PathBuf;

/// What the screen does with an event it gave to [`PathPick::event`].
#[derive(Debug, PartialEq, Eq)]
pub enum Picked {
    /// The picker is not open: the screen handles the event itself.
    NotOpen,
    /// The picker took the event (it is still open, or was closed without an answer).
    Taken,
    /// The person chose this path; the picker is closed.
    Chosen(PathBuf),
}

#[derive(Default)]
pub struct PathPick {
    picker: Option<Box<FilePicker>>,
}

impl PathPick {
    /// The click code of the Browse button: each screen passes its own number.
    pub fn button(g: &mut Gfx, x: f32, y: f32, s: f32, code: usize, accent: [u8; 4]) -> f32 {
        g.button(x, y, 12.0 * s, "Browse\u{2026}", accent, Event::Tab(code))
    }

    /// Open the picker where the box points (`start` may be empty or not a real path).
    pub fn open(&mut self, kind: PathKind, exts: &[&str], title: &str, start: &str) {
        let exts: Vec<String> = exts.iter().map(|e| e.to_string()).collect();
        self.picker = Some(Box::new(FilePicker::new(kind, &exts, title, start.trim(), current_scope())));
    }

    /// The names the open picker lists, in order (tests pick a row by name).
    pub fn names(&self) -> Vec<String> {
        self.picker.as_ref().map(|p| p.visible().iter().map(|e| e.name.clone()).collect()).unwrap_or_default()
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_some()
    }

    pub fn event(&mut self, ev: &Event) -> Picked {
        let Some(mut p) = self.picker.take() else { return Picked::NotOpen };
        match p.handle_event(ev) {
            PickOut::Stay => {
                self.picker = Some(p);
                Picked::Taken
            }
            PickOut::Cancel => Picked::Taken,
            PickOut::Chosen(path) => Picked::Chosen(path),
        }
    }

    /// Draw the picker over `area` when it is open.
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        if let Some(p) = self.picker.as_mut() {
            p.render(g, area, s, accent, blink);
        }
    }
}
