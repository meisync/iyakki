//! Font abilities of the SDK (F-SKIN-04): the family the person chose, line height, letter spacing, tabular figures, the roles an app
//! asks for (it never names a file), and what the bundled fonts cover per script. The numbers and words here are plain data: no font
//! file is read in this module, so the client engine links it too.
//!
//! What is bundled (legal/assets.txt): DejaVu Sans, Sans Bold and Sans Mono (the default), Liberation Sans, Sans Bold and Mono, and Noto
//! Sans for Arabic, Bengali, Devanagari, Gujarati, Gurmukhi, Hebrew, Kannada, Malayalam, Tamil, Telugu and Thai. Latin, Greek and Cyrillic
//! come from DejaVu and Liberation. **No CJK font and no dyslexia-friendly face is bundled**, so none is offered: a Han, Kana or Hangul
//! letter is reported as not covered ([`TypeStyle::uncovered`]) and draws as the toolkit's fallback box. A font with a licence entry is
//! the way to add either.

use crate::canvas::TextStyle;

/// The family of the interface text, as the person chose it (their choice is kept by the core, `iyakki_uikit::fonts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Family {
    /// The platform default (DejaVu Sans).
    #[default]
    Default,
    DejaVu,
    Liberation,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Default, Family::DejaVu, Family::Liberation];
    pub fn key(self) -> &'static str {
        match self {
            Family::Default => "default",
            Family::DejaVu => "dejavu",
            Family::Liberation => "liberation",
        }
    }
    pub fn from_key(k: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.key() == k)
    }
    /// The family name a browser draws with, with the metric-compatible fallbacks (a browser has the same files only when the page
    /// loads them; otherwise the first installed name wins).
    pub fn css(self) -> &'static str {
        match self {
            Family::Default | Family::DejaVu => "'DejaVu Sans', 'Bitstream Vera Sans', sans-serif",
            Family::Liberation => "'Liberation Sans', Arimo, Arial, sans-serif",
        }
    }
}

/// What an app asks for instead of a file: a role. The SDK and the person's choices decide the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    /// Running text.
    Body,
    /// Controls, labels and menus (bold in a button).
    Ui,
    /// Code and fixed-width text.
    Mono,
    /// Headings.
    Display,
}

impl FontRole {
    /// The text style the canvas draws this role with (the family is the person's, applied by the canvas).
    pub fn style(self) -> TextStyle {
        match self {
            FontRole::Body | FontRole::Ui => TextStyle::Regular,
            FontRole::Mono => TextStyle::Mono,
            FontRole::Display => TextStyle::Bold,
        }
    }
}

/// How a canvas sets text from now on: set once a frame by the host from the person's choices (and by a replayed list for one run).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TextOpts {
    pub family: Family,
    /// Extra space after each letter, in text sizes (0 to 0.2). Not applied to connected scripts, as in a browser.
    pub tracking: f32,
    /// Digits of one width (both bundled families have them already; a browser is asked for them).
    pub tabular: bool,
}

impl TextOpts {
    pub const MAX_TRACKING: f32 = 0.2;
    pub fn is_default(&self) -> bool {
        *self == TextOpts::default()
    }
    pub fn clamped(mut self) -> TextOpts {
        self.tracking = if self.tracking.is_finite() { self.tracking.clamp(0.0, Self::MAX_TRACKING) } else { 0.0 };
        self
    }
}

/// The person's type choices as an app sees them (`UiCx::fonts`, `Metrics::type_style`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    pub family: Family,
    /// Distance between baselines in text sizes (1.2 to 1.8).
    pub line_height: f32,
    pub tracking: f32,
    pub tabular: bool,
}

impl Default for TypeStyle {
    fn default() -> TypeStyle {
        TypeStyle { family: Family::Default, line_height: TypeStyle::NORMAL_LINE, tracking: 0.0, tabular: false }
    }
}

impl TypeStyle {
    pub const NORMAL_LINE: f32 = 1.35;
    pub const LINE: std::ops::RangeInclusive<f32> = 1.2..=1.8;

    pub fn new(family: Family, line_height: f32, tracking: f32, tabular: bool) -> TypeStyle {
        let line_height = if line_height.is_finite() { line_height.clamp(*Self::LINE.start(), *Self::LINE.end()) } else { Self::NORMAL_LINE };
        TypeStyle { family, line_height, tracking, tabular }.clamped()
    }

    fn clamped(mut self) -> TypeStyle {
        self.tracking = TextOpts { tracking: self.tracking, ..TextOpts::default() }.clamped().tracking;
        self
    }

    /// What the canvas needs.
    pub fn opts(&self) -> TextOpts {
        TextOpts { family: self.family, tracking: self.tracking, tabular: self.tabular }.clamped()
    }

    /// The text style (regular, bold or mono) for a font role.
    pub fn style(&self, role: FontRole) -> TextStyle {
        role.style()
    }

    /// The scripts of `text` that no bundled font covers (empty when everything can be drawn).
    pub fn uncovered(&self, text: &str) -> Vec<Script> {
        let mut out: Vec<Script> = Vec::new();
        for c in text.chars() {
            let s = script_of(c);
            if !s.bundled() && !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }
}

/// Scripts the SDK tells apart for font fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Greek,
    Cyrillic,
    Arabic,
    Hebrew,
    Devanagari,
    Bengali,
    Gurmukhi,
    Gujarati,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    Thai,
    /// Han, Hiragana, Katakana and Hangul.
    Cjk,
    /// Symbols, punctuation, digits and everything else (the Latin faces carry the common ones).
    Common,
}

impl Script {
    /// Is a font for this script bundled? (CJK is not.)
    pub fn bundled(self) -> bool {
        !matches!(self, Script::Cjk)
    }
    /// The Noto face that carries the script, None when DejaVu and Liberation already do.
    pub fn noto(self) -> Option<&'static str> {
        Some(match self {
            Script::Arabic => "Noto Sans Arabic",
            Script::Hebrew => "Noto Sans Hebrew",
            Script::Devanagari => "Noto Sans Devanagari",
            Script::Bengali => "Noto Sans Bengali",
            Script::Gurmukhi => "Noto Sans Gurmukhi",
            Script::Gujarati => "Noto Sans Gujarati",
            Script::Tamil => "Noto Sans Tamil",
            Script::Telugu => "Noto Sans Telugu",
            Script::Kannada => "Noto Sans Kannada",
            Script::Malayalam => "Noto Sans Malayalam",
            Script::Thai => "Noto Sans Thai",
            _ => return None,
        })
    }
}

pub fn script_of(c: char) -> Script {
    match c as u32 {
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
        0x0400..=0x052F => Script::Cyrillic,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0980..=0x09FF => Script::Bengali,
        0x0A00..=0x0A7F => Script::Gurmukhi,
        0x0A80..=0x0AFF => Script::Gujarati,
        0x0B80..=0x0BFF => Script::Tamil,
        0x0C00..=0x0C7F => Script::Telugu,
        0x0C80..=0x0CFF => Script::Kannada,
        0x0D00..=0x0D7F => Script::Malayalam,
        0x0E00..=0x0E7F => Script::Thai,
        0x2E80..=0x2FFF | 0x3040..=0x30FF | 0x3100..=0x312F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F => Script::Cjk,
        0x0041..=0x005A | 0x0061..=0x007A | 0x00C0..=0x024F | 0x1E00..=0x1EFF => Script::Latin,
        _ => Script::Common,
    }
}
