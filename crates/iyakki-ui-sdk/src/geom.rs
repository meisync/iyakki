//! Plain geometry and colour. All SDK coordinates are logical units: the canvas hides the device's pixel density.

/// Red, green, blue, alpha.
pub type Color = [u8; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    /// Shrunk by `d` on every side (never below zero size).
    pub fn inset(&self, d: f32) -> Rect {
        Rect { x: self.x + d, y: self.y + d, w: (self.w - 2.0 * d).max(0.0), h: (self.h - 2.0 * d).max(0.0) }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Cut `w` off the left: (the cut piece, the rest). `w` is limited to the width.
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w.max(0.0));
        (Rect::new(self.x, self.y, w, self.h), Rect::new(self.x + w, self.y, self.w - w, self.h))
    }

    /// Cut `h` off the top: (the cut piece, the rest). `h` is limited to the height.
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h.max(0.0));
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h, self.w, self.h - h))
    }

    /// Cut `h` off the bottom: (the rest, the cut piece).
    pub fn split_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h.max(0.0));
        (Rect::new(self.x, self.y, self.w, self.h - h), Rect::new(self.x, self.y + self.h - h, self.w, h))
    }

    /// This rectangle without the safe-area insets (a notch, a rounded corner, a home bar).
    pub fn without(&self, i: Insets) -> Rect {
        Rect { x: self.x + i.left, y: self.y + i.top, w: (self.w - i.left - i.right).max(0.0), h: (self.h - i.top - i.bottom).max(0.0) }
    }
}

/// Space to keep clear on each side (the safe area of a phone, the dock of a screen).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
