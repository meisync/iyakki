//! Raw pointers to gestures: tap, long press, drag and swipe for one finger, and pinch for two (the second finger turns the first into
//! half of a pinch: no tap, drag or swipe follows from either finger of it).

use crate::event::{Dir, Gesture, Phase, Pointer};

/// Moves shorter than this are still a tap or a hold, in logical units.
pub const SLOP: f32 = 10.0;
pub const TAP_MS: u64 = 300;
pub const LONG_PRESS_MS: u64 = 500;
/// A drag that ends after at least this distance within [`SWIPE_MS`] is also a swipe.
pub const SWIPE_MIN: f32 = 60.0;
pub const SWIPE_MS: u64 = 300;

#[derive(Default)]
pub struct Recognizer {
    down: Option<Down>,
    /// A second finger: its id and place, and the pinch it makes with the first.
    second: Option<Second>,
}

struct Second {
    id: u32,
    x: f32,
    y: f32,
    start: f32,
    scale: f32,
}

/// A pinch reports again only when the scale moved by at least this much.
pub const PINCH_STEP: f32 = 0.01;

struct Down {
    id: u32,
    x: f32,
    y: f32,
    t: u64,
    dragging: bool,
    long: bool,
    /// Part of a pinch: nothing but the pinch comes out of this finger.
    pinched: bool,
}

impl Recognizer {
    pub fn new() -> Recognizer {
        Recognizer::default()
    }

    pub fn feed(&mut self, p: &Pointer) -> Vec<Gesture> {
        let mut out = Vec::new();
        match p.phase {
            Phase::Down => {
                if self.down.is_none() {
                    self.down = Some(Down { id: p.id, x: p.x, y: p.y, t: p.t_ms, dragging: false, long: false, pinched: false });
                } else if self.second.is_none() && self.down.as_ref().is_some_and(|d| d.id != p.id) {
                    let d = self.down.as_mut().expect("checked");
                    if d.dragging {
                        out.push(Gesture::DragEnd { dx: 0.0, dy: 0.0 });
                        d.dragging = false;
                    }
                    d.pinched = true;
                    let start = (p.x - d.x).hypot(p.y - d.y).max(1.0);
                    self.second = Some(Second { id: p.id, x: p.x, y: p.y, start, scale: 1.0 });
                }
            }
            Phase::Move => {
                if self.second.is_some() {
                    return self.pinch_move(p);
                }
                if let Some(d) = self.down.as_mut().filter(|d| d.id == p.id) {
                    let (dx, dy) = (p.x - d.x, p.y - d.y);
                    if !d.dragging && !d.long && dx.hypot(dy) > SLOP {
                        d.dragging = true;
                        out.push(Gesture::DragStart { x: d.x, y: d.y });
                    }
                    if d.dragging {
                        out.push(Gesture::Drag { dx, dy });
                    }
                }
            }
            Phase::Up | Phase::Cancel if self.second.is_some() && self.in_pinch(p.id) => {
                let sc = self.second.take().map_or(1.0, |s| s.scale);
                out.push(Gesture::PinchEnd { scale: sc });
                // the finger left behind stays part of the pinch: it makes no tap, drag or swipe
                if let Some(d) = self.down.as_mut() {
                    if d.id == p.id {
                        self.down = None;
                    } else {
                        d.pinched = true;
                    }
                } else {
                    self.down = None;
                }
            }
            Phase::Up => {
                if self.down.as_ref().is_some_and(|d| d.id == p.id) {
                    let d = self.down.take().expect("checked");
                    let (dx, dy) = (p.x - d.x, p.y - d.y);
                    if d.pinched {
                        // the last finger of a pinch lifting is not a tap
                    } else if d.dragging {
                        out.push(Gesture::DragEnd { dx, dy });
                        if p.t_ms.saturating_sub(d.t) <= SWIPE_MS && dx.hypot(dy) >= SWIPE_MIN {
                            out.push(Gesture::Swipe(if dx.abs() >= dy.abs() { if dx > 0.0 { Dir::Right } else { Dir::Left } } else if dy > 0.0 { Dir::Down } else { Dir::Up }));
                        }
                    } else if !d.long && p.t_ms.saturating_sub(d.t) <= TAP_MS {
                        out.push(Gesture::Tap { x: d.x, y: d.y });
                    }
                }
            }
            Phase::Cancel => {
                if self.down.as_ref().is_some_and(|d| d.id == p.id) {
                    self.down = None;
                }
            }
        }
        out
    }

    fn in_pinch(&self, id: u32) -> bool {
        self.second.as_ref().is_some_and(|s| s.id == id) || self.down.as_ref().is_some_and(|d| d.id == id)
    }

    fn pinch_move(&mut self, p: &Pointer) -> Vec<Gesture> {
        let (Some(d), Some(s)) = (self.down.as_mut(), self.second.as_mut()) else { return Vec::new() };
        if p.id == d.id {
            d.x = p.x;
            d.y = p.y;
        } else if p.id == s.id {
            s.x = p.x;
            s.y = p.y;
        } else {
            return Vec::new();
        }
        let scale = (s.x - d.x).hypot(s.y - d.y) / s.start;
        if (scale - s.scale).abs() >= PINCH_STEP {
            s.scale = scale;
            return vec![Gesture::Pinch { scale, x: (s.x + d.x) / 2.0, y: (s.y + d.y) / 2.0 }];
        }
        Vec::new()
    }

    /// The clock moved on: a finger held still long enough is a long press (once).
    pub fn tick(&mut self, now_ms: u64) -> Vec<Gesture> {
        match self.down.as_mut() {
            Some(d) if !d.dragging && !d.long && !d.pinched && now_ms.saturating_sub(d.t) >= LONG_PRESS_MS => {
                d.long = true;
                vec![Gesture::LongPress { x: d.x, y: d.y }]
            }
            _ => Vec::new(),
        }
    }
}
