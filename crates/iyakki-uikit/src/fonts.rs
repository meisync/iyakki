//! The bundled fonts a person can choose between (F-SHELL-42): one family for the interface (regular and bold) and one for code
//! (the mono face). Every family here is under a permissive licence and travels with the program (nothing is downloaded):
//!
//! | choice | files | licence |
//! |---|---|---|
//! | DejaVu Sans / DejaVu Sans Mono (the default) | `DejaVuSans.ttf`, `DejaVuSans-Bold.ttf`, `DejaVuSansMono.ttf` | Bitstream Vera / DejaVu |
//! | Liberation Sans / Liberation Mono | `LiberationSans-Regular.ttf`, `LiberationSans-Bold.ttf`, `LiberationMono-Regular.ttf` | SIL OFL 1.1 |
//!
//! The choice is kept per person beside the theme, the wallpaper and the UI size (`ui-<user>.json`, keys `font_ui` and `font_code`), so
//! the text size they chose stays as it is: only the shapes change. The sign-in screen always uses the default. A letter the chosen face
//! does not have is drawn from DejaVu (it covers more), so nothing turns into an empty box. Scripts that need shaping are loaded on demand
//! as before and do not depend on this choice.

use fontdue::{Font, FontSettings};
use std::cell::Cell;
use std::sync::OnceLock;

use crate::gfx::Face;

/// The family of the interface text (the `Sans` and `Bold` faces).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UiFont {
    #[default]
    DejaVu,
    Liberation,
}

/// The family of code and fixed-width text (the `Mono` face).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CodeFont {
    #[default]
    DejaVu,
    Liberation,
}

pub const UI_ALL: [UiFont; 2] = [UiFont::DejaVu, UiFont::Liberation];
pub const CODE_ALL: [CodeFont; 2] = [CodeFont::DejaVu, CodeFont::Liberation];

impl UiFont {
    pub fn key(self) -> &'static str {
        match self {
            UiFont::DejaVu => "dejavu",
            UiFont::Liberation => "liberation",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            UiFont::DejaVu => "DejaVu Sans",
            UiFont::Liberation => "Liberation Sans",
        }
    }
    pub fn licence(self) -> &'static str {
        match self {
            UiFont::DejaVu => "Bitstream Vera / DejaVu licence",
            UiFont::Liberation => "SIL Open Font License 1.1",
        }
    }
    pub fn from_key(k: &str) -> Option<UiFont> {
        UI_ALL.into_iter().find(|f| f.key() == k.trim().to_lowercase())
    }
}

impl CodeFont {
    pub fn key(self) -> &'static str {
        match self {
            CodeFont::DejaVu => "dejavu",
            CodeFont::Liberation => "liberation",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            CodeFont::DejaVu => "DejaVu Sans Mono",
            CodeFont::Liberation => "Liberation Mono",
        }
    }
    pub fn licence(self) -> &'static str {
        match self {
            CodeFont::DejaVu => "Bitstream Vera / DejaVu licence",
            CodeFont::Liberation => "SIL Open Font License 1.1",
        }
    }
    pub fn from_key(k: &str) -> Option<CodeFont> {
        CODE_ALL.into_iter().find(|f| f.key() == k.trim().to_lowercase())
    }
}

fn load(bytes: &[u8]) -> Font {
    Font::from_bytes(bytes, FontSettings::default()).expect("bundled font is valid")
}

/// Every bundled face, parsed on first use: DejaVu sans, bold, mono, then Liberation sans, bold, mono.
fn all() -> &'static [Font; 6] {
    static ALL: OnceLock<[Font; 6]> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            load(include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf")),
            load(include_bytes!("../../iyakki-shell/assets/DejaVuSans-Bold.ttf")),
            load(include_bytes!("../../iyakki-shell/assets/DejaVuSansMono.ttf")),
            load(include_bytes!("../../iyakki-shell/assets/LiberationSans-Regular.ttf")),
            load(include_bytes!("../../iyakki-shell/assets/LiberationSans-Bold.ttf")),
            load(include_bytes!("../../iyakki-shell/assets/LiberationMono-Regular.ttf")),
        ]
    })
}

thread_local! {
    static UI: Cell<UiFont> = const { Cell::new(UiFont::DejaVu) };
    static CODE: Cell<CodeFont> = const { Cell::new(CodeFont::DejaVu) };
    /// Bumped on every change so a frame buffer drops the letters it has drawn in the old face.
    static GENERATION: Cell<u32> = const { Cell::new(0) };
}

pub fn current_ui() -> UiFont {
    UI.with(Cell::get)
}

pub fn current_code() -> CodeFont {
    CODE.with(Cell::get)
}

pub fn generation() -> u32 {
    GENERATION.with(Cell::get)
}

pub fn set_ui(f: UiFont) {
    if current_ui() != f {
        UI.with(|c| c.set(f));
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

pub fn set_code(f: CodeFont) {
    if current_code() != f {
        CODE.with(|c| c.set(f));
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

fn index(face: Face) -> usize {
    let base = match face {
        Face::Sans => 0,
        Face::Bold => 1,
        Face::Mono => 2,
    };
    let liberation = match face {
        Face::Mono => current_code() == CodeFont::Liberation,
        _ => current_ui() == UiFont::Liberation,
    };
    base + if liberation { 3 } else { 0 }
}

/// The face to draw `face` in now (the person's choice).
pub fn font(face: Face) -> &'static Font {
    &all()[index(face)]
}

/// The face to draw the letter `c` of `face` in: the chosen one, or DejaVu when the chosen one has no glyph for it.
pub fn font_for(face: Face, c: char) -> &'static Font {
    let f = font(face);
    if index(face) >= 3 && f.lookup_glyph_index(c) == 0 {
        &all()[index(face) - 3]
    } else {
        f
    }
}

/// The face's name for a chosen family (for the Settings line and the chat tool).
pub fn describe() -> String {
    format!("{} for text, {} for code", current_ui().name(), current_code().name())
}

// --------------------------------------------------------------------------------------------------- saved per person

fn path(dir: &std::path::Path, user: &str) -> std::path::PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

fn read(dir: &std::path::Path, user: &str) -> serde_json::Value {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}))
}

/// The person's saved choice: the default family for a missing or unknown value.
pub fn load_choice(dir: &std::path::Path, user: &str) -> (UiFont, CodeFont) {
    let v = read(dir, user);
    (v["font_ui"].as_str().and_then(UiFont::from_key).unwrap_or_default(), v["font_code"].as_str().and_then(CodeFont::from_key).unwrap_or_default())
}

/// Keep the choice beside the person's other display settings, leaving everything else in the file alone.
pub fn save_choice(dir: &std::path::Path, user: &str, ui: UiFont, code: CodeFont) {
    let p = path(dir, user);
    let mut v = read(dir, user);
    v["font_ui"] = serde_json::json!(ui.key());
    v["font_code"] = serde_json::json!(code.key());
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Draw in the person's saved fonts (at their sign-in).
pub fn apply_for(dir: &std::path::Path, user: &str) {
    let (u, c) = load_choice(dir, user);
    set_ui(u);
    set_code(c);
    crate::typeset::set_current(crate::typeset::load(dir, user));
}

/// The default fonts (the sign-in screen, and after sign-out).
pub fn apply_default() {
    set_ui(UiFont::default());
    set_code(CodeFont::default());
    crate::typeset::set_current(crate::typeset::Prefs::default());
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
