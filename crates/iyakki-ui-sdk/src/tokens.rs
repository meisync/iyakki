//! Design tokens beyond colour (F-SDK-120): the numbers a skin sets and the SDK widgets and apps read. A [`Tokens`] is made from a
//! Look (its `surface`, its `motion` and the theme's focus ring) and holds only numbers, so a widget never has a radius, a border
//! width, a gap, a shadow, a glyph size or a duration of its own: it asks for a step of the scale and the skin decides what the step is.
//!
//! | scale | steps | what a skin changes |
//! |---|---|---|
//! | [`Radius`] | `Sm`, `Md`, `Lg` | `shape`: sharp, soft (what the SDK drew before skins), round, pill |
//! | [`Line`] | `Hair`, `Normal`, `Thick` | `edge`: thin, regular, heavy |
//! | [`Space`] | `Xxs` .. `Xl` | `spacing`: tight, normal, airy (a factor on the base steps) |
//! | shadow | spread and strength | `elevation`: flat, low, high |
//! | glass | on or off, blur radius | `glass`, `blur` (the canvas has no blur operation yet: a renderer draws the translucent fill) |
//! | glyph | one size | `glyph_size` |
//! | motion | `Fast`, `Normal`, `Slow` in ms | `motion`: reduced gives 0 (nothing animates) |
//! | focus ring | one width | the theme's `ring` factor |


/// Corner radius steps (small controls and fields, buttons and rows, cards and panels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radius {
    Sm,
    Md,
    Lg,
}

/// Border and rule widths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Hair,
    Normal,
    Thick,
}

/// Gaps and paddings, smallest to largest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Xxs,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
}

/// Animation durations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Fast,
    Normal,
    Slow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    radius: [f32; 3],
    line: [f32; 3],
    space: [f32; 6],
    shadow_spread: f32,
    shadow_alpha: u8,
    pub glass: bool,
    pub blur: f32,
    glyph: f32,
    pub reduced_motion: bool,
    ring: f32,
}

/// Base steps of the spacing scale at "normal"; 2, 4, 6, 8, 12 and 16 are the gaps the SDK used before skins.
const SPACE_BASE: [f32; 6] = [2.0, 4.0, 6.0, 8.0, 12.0, 16.0];
const MOTION_MS: [u32; 3] = [120, 200, 320];

/// Rows of the scales, one per word of the skin (the words are in `iyakki_uikit::look`).
const RADII: [[f32; 3]; 4] = [[0.0, 2.0, 3.0], [6.0, 8.0, 10.0], [8.0, 12.0, 16.0], [12.0, 20.0, 24.0]];
const LINES: [[f32; 3]; 3] = [[0.5, 0.5, 1.0], [1.0, 1.0, 2.0], [1.0, 2.0, 3.0]];
const SPACING_K: [f32; 3] = [0.75, 1.0, 1.25];
const SHADOWS: [(f32, u8); 3] = [(0.0, 0), (6.0, 90), (14.0, 140)];

impl Default for Tokens {
    /// What the widgets drew before skins existed: soft corners, regular borders, normal spacing, flat, no glass, full motion.
    fn default() -> Tokens {
        Tokens::build(1, 1, 1, 0, false, 12, 20, false, 1.0)
    }
}

impl Tokens {
    #[allow(clippy::too_many_arguments)]
    fn build(shape: usize, edge: usize, spacing: usize, elevation: usize, glass: bool, blur: u8, glyph: u16, reduced: bool, ring: f32) -> Tokens {
        let k = SPACING_K[spacing];
        Tokens {
            radius: RADII[shape],
            line: LINES[edge],
            space: SPACE_BASE.map(|b| b * k),
            shadow_spread: SHADOWS[elevation].0,
            shadow_alpha: SHADOWS[elevation].1,
            glass,
            blur: if glass { f32::from(blur) } else { 0.0 },
            glyph: f32::from(glyph),
            reduced_motion: reduced,
            ring,
        }
    }

    /// The tokens of a Look: its `surface`, its `motion` and the theme's focus ring.
    #[cfg(feature = "uikit")]
    pub fn of(look: &iyakki_uikit::look::Look) -> Tokens {
        use iyakki_uikit::look::{Edge, Elevation, Motion, Shape, Spacing};
        let s = &look.surface;
        let shape = match s.shape {
            Shape::Sharp => 0,
            Shape::Soft => 1,
            Shape::Round => 2,
            Shape::Pill => 3,
        };
        let edge = match s.edge {
            Edge::Thin => 0,
            Edge::Regular => 1,
            Edge::Heavy => 2,
        };
        let spacing = match s.spacing {
            Spacing::Tight => 0,
            Spacing::Normal => 1,
            Spacing::Airy => 2,
        };
        let elev = match s.elevation {
            Elevation::Flat => 0,
            Elevation::Low => 1,
            Elevation::High => 2,
        };
        Tokens::build(shape, edge, spacing, elev, s.glass, s.blur, s.glyph, look.motion == Motion::Reduced, look.theme.ring)
    }

    pub fn radius(&self, r: Radius) -> f32 {
        self.radius[r as usize]
    }

    /// The radius for a thing of height `h`: never more than half of it (a pill is a half-circle end, not a bulge).
    pub fn radius_for(&self, r: Radius, h: f32) -> f32 {
        self.radius(r).min((h / 2.0).max(0.0))
    }

    pub fn line(&self, l: Line) -> f32 {
        self.line[l as usize]
    }

    pub fn space(&self, s: Space) -> f32 {
        self.space[s as usize]
    }

    /// How far the soft shadow of a raised surface spreads; 0 is flat (draw none).
    pub fn shadow_spread(&self) -> f32 {
        self.shadow_spread
    }

    /// The alpha of a shadow colour (0 when flat).
    pub fn shadow_alpha(&self) -> u8 {
        self.shadow_alpha
    }

    /// The size of a glyph or small icon in a widget.
    pub fn glyph(&self) -> f32 {
        self.glyph
    }

    /// The width of the focus ring: twice the line, times the theme's ring factor (High contrast draws it thick).
    pub fn focus_width(&self) -> f32 {
        2.0 * self.ring.max(1.0)
    }

    /// Milliseconds an animation of this speed takes; 0 when the person asked for reduced motion.
    pub fn duration_ms(&self, s: Speed) -> u32 {
        if self.reduced_motion {
            0
        } else {
            MOTION_MS[s as usize]
        }
    }

    pub fn animates(&self) -> bool {
        !self.reduced_motion
    }
}
