//! Size classes instead of pixels: the app asks what kind of space it has, and lays out for that.

use crate::geom::Rect;
use crate::tokens::{Line, Radius, Space, Tokens};
use crate::typeface::{FontRole, TextOpts, TypeStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SizeClass {
    /// A phone, a narrow pane: one column.
    Compact,
    /// A small window, a tablet: two columns at most.
    Medium,
    /// A desktop screen or a large pane.
    Expanded,
}

/// Widths (logical units) where a class ends.
pub const COMPACT_BELOW: f32 = 600.0;
pub const MEDIUM_BELOW: f32 = 900.0;
/// The smallest size of something a finger presses.
pub const MIN_TOUCH: f32 = 44.0;
/// The smallest size of something a mouse presses.
pub const MIN_POINTER: f32 = 24.0;

impl SizeClass {
    pub fn for_width(w: f32) -> SizeClass {
        if w < COMPACT_BELOW {
            SizeClass::Compact
        } else if w < MEDIUM_BELOW {
            SizeClass::Medium
        } else {
            SizeClass::Expanded
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SizeClass::Compact => "compact",
            SizeClass::Medium => "medium",
            SizeClass::Expanded => "expanded",
        }
    }
}

/// What the app needs to know about its space, from one call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub class: SizeClass,
    pub area: Rect,
    /// Device pixels per logical unit.
    pub density: f32,
    /// The person's text size factor (1.0 = normal); multiply font sizes by it.
    pub text_scale: f32,
    /// The pointer is a finger: targets are at least [`MIN_TOUCH`].
    pub touch: bool,
    /// The skin's numbers beyond colour: radii, borders, gaps, shadow, glass, glyph size, motion (the default is the plain skin).
    pub tokens: Tokens,
    /// The person's type choices (line height, letter spacing, family).
    pub type_style: TypeStyle,
}

/// Everything an app needs to set one kind of text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSpec {
    pub style: crate::canvas::TextStyle,
    pub size: f32,
    pub leading: f32,
    pub opts: TextOpts,
}

/// The roles of text in an app, from small to large. An app asks for a role and the type scale decides the size
/// (`Metrics::size`); it does not write a point size of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRole {
    Caption,
    Small,
    Body,
    Label,
    Large,
    Title,
    Heading,
    Display,
}

impl TypeRole {
    /// The size in logical units at text size 1x.
    pub fn base(self) -> f32 {
        match self {
            TypeRole::Caption => 12.0,
            TypeRole::Small => 13.0,
            TypeRole::Body => 14.0,
            TypeRole::Label => 15.0,
            TypeRole::Large => 17.0,
            TypeRole::Title => 20.0,
            TypeRole::Heading => 22.0,
            TypeRole::Display => 24.0,
        }
    }
}

impl Metrics {
    pub fn new(w: f32, h: f32, density: f32, text_scale: f32, touch: bool) -> Metrics {
        let text_scale = if text_scale.is_finite() { text_scale.clamp(0.5, 3.0) } else { 1.0 };
        Metrics { class: SizeClass::for_width(w), area: Rect::new(0.0, 0.0, w, h), density, text_scale, touch, tokens: Tokens::default(), type_style: TypeStyle::default() }
    }

    pub fn with_tokens(mut self, tokens: Tokens) -> Metrics {
        self.tokens = tokens;
        self
    }

    pub fn with_type(mut self, t: TypeStyle) -> Metrics {
        self.type_style = t;
        self
    }

    /// What to draw a font role with: the canvas style and the person's size, line height and spacing.
    pub fn text_style(&self, role: FontRole, ty: TypeRole) -> TextSpec {
        let size = self.size(ty);
        TextSpec { style: role.style(), size, leading: self.leading(size), opts: self.type_style.opts() }
    }

    /// The font size of a text role, with the person's text size applied.
    pub fn size(&self, role: TypeRole) -> f32 {
        self.font(role.base())
    }

    /// The distance from one baseline to the next for text of this size.
    pub fn leading(&self, size: f32) -> f32 {
        size * self.type_style.line_height
    }

    /// A gap or padding of the skin's spacing scale.
    pub fn gap(&self, s: Space) -> f32 {
        self.tokens.space(s)
    }

    /// A corner radius of the skin's shape scale.
    pub fn radius(&self, r: Radius) -> f32 {
        self.tokens.radius(r)
    }

    /// A border width of the skin's edge scale.
    pub fn line(&self, l: Line) -> f32 {
        self.tokens.line(l)
    }

    /// A width that is a number of text sizes wide (a label button), so it grows with the person's text size.
    pub fn em(&self, n: f32) -> f32 {
        self.font(TypeRole::Label.base() * n)
    }

    /// The smallest height of a pressable thing here.
    pub fn min_target(&self) -> f32 {
        if self.touch {
            MIN_TOUCH
        } else {
            MIN_POINTER
        }
    }

    /// A font size with the person's text size applied.
    pub fn font(&self, size: f32) -> f32 {
        size * self.text_scale
    }

    /// Side padding that grows with the space.
    pub fn gutter(&self) -> f32 {
        let base = match self.class {
            SizeClass::Compact => 12.0,
            SizeClass::Medium => 20.0,
            SizeClass::Expanded => 28.0,
        };
        base * self.tokens.space(Space::Md) / 8.0
    }
}
