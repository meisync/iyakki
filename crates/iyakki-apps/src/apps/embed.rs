//! An app drawn into a pane of another app, with its own input (F-IDE-25). Small and additive: the App trait is unchanged; the host app
//! (today the AI IDE's docked Preview) owns an [`Embedded`], draws it in a rectangle and gives it the events that belong to it.
//!
//! * **Drawing.** The child is drawn with the rectangle as its whole screen and clipped to it. Whatever hit regions it registers are taken
//!   off the host's list and registered again as `Event::AreaAt(BASE + n, 0, 0)` over the part that is inside the rectangle, so the host
//!   can tell a click on the child's control from a click on its own, and hands the child the event it registered ([`Embedded::handle`]).
//! * **Input.** Clicks and pointer moves inside the rectangle go to the child, and a press there gives it the keyboard. Keys go to the
//!   child only while it has the keyboard; Esc takes the keyboard back to the host without the child seeing it. Ticks always reach it. If the
//!   child asks to go back (`Flow::Back`) the host is told it closed.
//! * The child's own jumps and requests (`take_request`, `take_jump`) are handed to the host to pass on to the shell.

use super::{App, AppCx, Flow};
use crate::gfx::Gfx;
use crate::ui::{Event, Hit, Key, R};

/// The first `AreaAt` id used for the child's hit regions; the host's own ids stay below it.
pub const BASE: usize = 0x00E0_0000;
/// How many hit regions of the child are kept (the rest stay dead rather than grow the list for ever).
pub const MAX_HITS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Nothing for the host to do.
    Stay,
    /// The child went back (or asked to close): the host drops it.
    Closed,
    /// The child gave the keyboard back (Esc): the host takes it.
    GaveKeyboard,
}

pub struct Embedded {
    app: Box<dyn App>,
    rect: R,
    hits: Vec<Event>,
    /// The child has the keyboard.
    pub focused: bool,
}

/// The part of `a` inside `b`, `None` when they do not overlap.
pub fn inside(a: R, b: R) -> Option<R> {
    let (x0, y0, x1, y1) = (a.x.max(b.x), a.y.max(b.y), (a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    (x1 > x0 && y1 > y0).then(|| R::new(x0, y0, x1 - x0, y1 - y0))
}

impl Embedded {
    pub fn new(app: Box<dyn App>) -> Embedded {
        Embedded { app, rect: R::new(0.0, 0.0, 0.0, 0.0), hits: Vec::new(), focused: true }
    }

    pub fn title(&self) -> &'static str {
        self.app.title()
    }

    pub fn rect(&self) -> R {
        self.rect
    }

    /// Draw the child in `area`.
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.rect = area;
        let before = g.hits.len();
        let app = &mut self.app;
        g.clipped(area, |g| app.render(g, area, s, cx, tick));
        let mine: Vec<Hit> = g.hits.drain(before.min(g.hits.len())..).collect();
        self.hits.clear();
        for h in mine {
            let Some(r) = inside(h.rect, area) else { continue };
            if self.hits.len() >= MAX_HITS {
                break;
            }
            g.hits.push(Hit { rect: r, ev: Event::AreaAt(BASE + self.hits.len(), 0.0, 0.0), label: h.label });
            self.hits.push(h.ev);
        }
    }

    /// Is this event one of the child's hit regions?
    pub fn owns(ev: &Event) -> bool {
        matches!(ev, Event::AreaAt(id, ..) if *id >= BASE)
    }

    fn forward(&mut self, ev: &Event, cx: &AppCx) -> Out {
        if matches!(self.app.handle(ev, cx), Flow::Back) { Out::Closed } else { Out::Stay }
    }

    /// Give the child an event of the host's, if it is the child's: its hit regions, the pointer inside the rectangle, keys while it has
    /// the keyboard, and ticks. Returns what the host has to do; `None` when the event is not the child's (the host handles it).
    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> Option<Out> {
        match ev {
            Event::AreaAt(id, ..) if *id >= BASE => {
                self.focused = true;
                let inner = self.hits.get(*id - BASE).cloned()?;
                Some(self.forward(&inner, cx))
            }
            Event::Tick => Some(self.forward(ev, cx)),
            Event::MouseMove(x, y) | Event::MouseUp(x, y) | Event::MouseRight(x, y) if self.rect.contains(*x, *y) => Some(self.forward(ev, cx)),
            Event::MouseDown(x, y) if self.rect.contains(*x, *y) => {
                self.focused = true;
                Some(self.forward(ev, cx))
            }
            Event::Scroll(_) | Event::ScrollPx(_) if self.focused => Some(self.forward(ev, cx)),
            Event::Key(Key::Esc) if self.focused => {
                self.focused = false;
                Some(Out::GaveKeyboard)
            }
            Event::Key(_) | Event::Paste(_) if self.focused => Some(self.forward(ev, cx)),
            _ => None,
        }
    }

    /// What the child wants opened next (a request or a jump), for the host to pass on.
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.app.take_request().or_else(|| self.app.take_jump())
    }

    pub fn wants_redraw(&mut self) -> bool {
        self.app.wants_redraw()
    }
}
