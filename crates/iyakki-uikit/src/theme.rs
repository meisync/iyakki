//! Desktop themes: Dark (the approved look, the default), Light and High contrast.
//!
//! How it works (the least invasive correct way, because ~1000 call sites use the `ui::TEXT`, `ui::CARD`, ... constants,
//! some of them in `const` contexts): the constants keep the **Dark** values and every app keeps writing them. The one
//! place that turns a colour into pixels ([`crate::gfx::Gfx`]) asks [`themed`] first. With Dark that is the identity (so
//! every existing pixel assertion holds); with another theme the Dark palette colours map to the theme's own, and any other
//! opaque colour a screen made up (the `rgb(...)` literals) follows by a lightness mapping, so nothing is left dark on light.
//!
//! The active theme belongs to the thread that runs the shell (like the UI size): each person signed in through the browser
//! has their own. The choice is kept per person next to the UI size (`ui-<user>.json`, key `theme`).

use std::cell::Cell;

use crate::gfx::rgb;
use crate::ui;

/// The three themes, in the order Settings offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeId {
    #[default]
    Dark,
    Light,
    HighContrast,
}

pub const ALL: [ThemeId; 3] = [ThemeId::Dark, ThemeId::Light, ThemeId::HighContrast];

impl ThemeId {
    pub fn name(self) -> &'static str {
        match self {
            ThemeId::Dark => "Dark",
            ThemeId::Light => "Light",
            ThemeId::HighContrast => "High contrast",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ThemeId::Dark => "dark",
            ThemeId::Light => "light",
            ThemeId::HighContrast => "high-contrast",
        }
    }
    pub fn from_key(k: &str) -> ThemeId {
        ALL.into_iter().find(|t| t.key() == k).unwrap_or_default()
    }
    pub fn theme(self) -> &'static Theme {
        match self {
            ThemeId::Dark => &DARK,
            ThemeId::Light => &LIGHT,
            ThemeId::HighContrast => &HIGH_CONTRAST,
        }
    }
}

/// Every colour of the shared look. `DARK` is exactly the `ui` / `fx` constants.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub id: ThemeId,
    pub bg_top: [u8; 4],
    pub bg_bottom: [u8; 4],
    pub card: [u8; 4],
    pub card_hover: [u8; 4],
    pub card_sel: [u8; 4],
    pub border: [u8; 4],
    pub accent: [u8; 4],
    pub text: [u8; 4],
    pub muted: [u8; 4],
    pub dim: [u8; 4],
    pub err: [u8; 4],
    pub ok: [u8; 4],
    pub field: [u8; 4],
    pub panel: [u8; 4],
    pub well: [u8; 4],
    pub amber: [u8; 4],
    pub cyan: [u8; 4],
    pub magenta: [u8; 4],
    /// Button fills at rest and under the pointer (the pill buttons of `fx`).
    pub button: [u8; 4],
    pub button_hot: [u8; 4],
    /// Focus ring and outline thickness factor (High contrast draws them thick, with borders always visible).
    pub ring: f32,
}

pub const DARK: Theme = Theme {
    id: ThemeId::Dark,
    bg_top: ui::BG_TOP,
    bg_bottom: ui::BG_BOTTOM,
    card: ui::CARD,
    card_hover: ui::CARD_HOVER,
    card_sel: ui::CARD_SEL,
    border: ui::BORDER,
    accent: ui::ACCENT_MAIN,
    text: ui::TEXT,
    muted: ui::MUTED,
    dim: ui::DIM,
    err: ui::ERR,
    ok: ui::OK,
    field: ui::FIELD,
    panel: crate::fx::PANEL,
    well: crate::fx::WELL,
    amber: crate::fx::AMBER,
    cyan: crate::fx::CYAN,
    magenta: crate::fx::MAGENTA,
    button: rgb(28, 31, 40),
    button_hot: rgb(44, 49, 64),
    ring: 1.0,
};

/// Warm-neutral light: paper-like greys, the same elevation steps (page, card, hover, chosen), a deeper blue accent.
pub const LIGHT: Theme = Theme {
    id: ThemeId::Light,
    bg_top: rgb(250, 248, 244),
    bg_bottom: rgb(238, 235, 229),
    card: rgb(255, 254, 251),
    card_hover: rgb(243, 240, 234),
    card_sel: rgb(226, 233, 247),
    border: rgb(200, 195, 185),
    accent: rgb(37, 84, 196),
    text: rgb(28, 30, 36),
    muted: rgb(80, 84, 96),
    dim: rgb(92, 96, 108),
    err: rgb(172, 28, 36),
    ok: rgb(18, 112, 60),
    field: rgb(255, 255, 255),
    panel: rgb(233, 230, 223),
    well: rgb(242, 239, 233),
    amber: rgb(138, 78, 0),
    cyan: rgb(0, 106, 140),
    magenta: rgb(170, 30, 115),
    button: rgb(246, 243, 237),
    button_hot: rgb(232, 228, 220),
    ring: 1.0,
};

/// Black, white and a yellow accent; every border white and visible; thick focus ring. All text at least 7:1.
pub const HIGH_CONTRAST: Theme = Theme {
    id: ThemeId::HighContrast,
    bg_top: rgb(0, 0, 0),
    bg_bottom: rgb(0, 0, 0),
    card: rgb(8, 8, 8),
    card_hover: rgb(28, 28, 28),
    card_sel: rgb(0, 0, 80),
    border: rgb(255, 255, 255),
    accent: rgb(255, 221, 0),
    text: rgb(255, 255, 255),
    muted: rgb(235, 235, 235),
    dim: rgb(215, 215, 215),
    err: rgb(255, 160, 160),
    ok: rgb(120, 255, 150),
    field: rgb(0, 0, 0),
    panel: rgb(0, 0, 0),
    well: rgb(16, 16, 16),
    amber: rgb(255, 200, 0),
    cyan: rgb(0, 230, 255),
    magenta: rgb(255, 130, 230),
    button: rgb(16, 16, 16),
    button_hot: rgb(34, 34, 34),
    ring: 2.4,
};

thread_local! {
    static CURRENT: Cell<ThemeId> = const { Cell::new(ThemeId::Dark) };
    /// Bumped on every change so caches of drawn pictures (the backdrop) know to rebuild.
    static GENERATION: Cell<u32> = const { Cell::new(0) };
}

pub fn current_id() -> ThemeId {
    CURRENT.with(|c| c.get())
}

pub fn current() -> &'static Theme {
    current_id().theme()
}

pub fn generation() -> u32 {
    GENERATION.with(|g| g.get())
}

pub fn set_current(id: ThemeId) {
    if current_id() != id {
        CURRENT.with(|c| c.set(id));
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

pub fn is_high_contrast() -> bool {
    current_id() == ThemeId::HighContrast
}

/// Border/focus line thickness factor of the active theme (1.0 except High contrast).
pub fn ring() -> f32 {
    current().ring
}

// --------------------------------------------------------------------------------------------------- saved per person

fn path(dir: &std::path::Path, user: &str) -> std::path::PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

/// The person's saved theme, Dark when there is none.
pub fn load(dir: &std::path::Path, user: &str) -> ThemeId {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["theme"].as_str().map(ThemeId::from_key)).unwrap_or_default()
}

/// Keep the choice with the person's other display settings (UI size and so on stay as they are).
pub fn save(dir: &std::path::Path, user: &str, id: ThemeId) {
    let p = path(dir, user);
    let mut v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v["theme"] = serde_json::json!(id.key());
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Draw in the person's theme (or Dark with `None`, at the sign-in screen).
pub fn apply(id: Option<ThemeId>) {
    set_current(id.unwrap_or_default());
}

// ------------------------------------------------------------------------------------------------------ the mapping

/// The colour to put on screen for `c`, a colour written in the Dark palette. Dark returns `c` untouched.
pub fn themed(c: [u8; 4]) -> [u8; 4] {
    let id = current_id();
    if id == ThemeId::Dark {
        return c;
    }
    let t = id.theme();
    let with_a = |m: [u8; 4]| [m[0], m[1], m[2], c[3]];
    let pairs: [([u8; 4], [u8; 4]); 17] = [
        (DARK.bg_top, t.bg_top),
        (DARK.bg_bottom, t.bg_bottom),
        (DARK.card, t.card),
        (DARK.card_hover, t.card_hover),
        (DARK.card_sel, t.card_sel),
        (DARK.border, t.border),
        (DARK.accent, t.accent),
        (DARK.text, t.text),
        (DARK.muted, t.muted),
        (DARK.dim, t.dim),
        (DARK.err, t.err),
        (DARK.ok, t.ok),
        (DARK.field, t.field),
        (DARK.panel, t.panel),
        (DARK.well, t.well),
        (DARK.amber, t.amber),
        (DARK.button, t.button),
    ];
    for (d, m) in pairs {
        if d[..3] == c[..3] {
            return with_a(m);
        }
    }
    for (d, m) in [(DARK.cyan, t.cyan), (DARK.magenta, t.magenta), (DARK.button_hot, t.button_hot)] {
        if d[..3] == c[..3] {
            return with_a(m);
        }
    }
    // see-through colours (shadows, glows, highlights) keep their hue and strength; opaque made-up colours follow the lightness mapping
    if c[3] < 255 {
        return c;
    }
    let (h, s, l) = to_hsl(c);
    let (s2, l2) = match id {
        // flip the lightness (dark navy panels become pale, bright accents become deep), keeping the hue
        ThemeId::Light => (s * 0.9, (1.0 - l).clamp(0.06, 0.97)),
        // push to the extremes: dark things go black, light things go white, colour stays for hue
        _ => (s, if l < 0.5 { l * 0.12 } else { 1.0 - (1.0 - l) * 0.3 }),
    };
    from_hsl(h, s2, l2, c[3])
}

fn to_hsl(c: [u8; 4]) -> (f32, f32, f32) {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    (h * 60.0, s.clamp(0.0, 1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32, a: u8) -> [u8; 4] {
    let cc = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = cc * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - cc / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (cc, x, 0.0),
        1 => (x, cc, 0.0),
        2 => (0.0, cc, x),
        3 => (0.0, x, cc),
        4 => (x, 0.0, cc),
        _ => (cc, 0.0, x),
    };
    let q = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [q(r), q(g), q(b), a]
}

// ----------------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Gfx;
    use crate::ui::R;

    fn lum(c: [u8; 4]) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
    }
    fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_current(ThemeId::Dark);
        }
    }

    #[test]
    fn dark_is_exactly_the_approved_constants_and_the_default() {
        assert_eq!(current_id(), ThemeId::Dark);
        assert_eq!(DARK.text, ui::TEXT);
        assert_eq!(DARK.card, ui::CARD);
        assert_eq!(DARK.accent, ui::ACCENT_MAIN);
        for c in [ui::TEXT, ui::CARD, [1, 2, 3, 255], [9, 11, 20, 255], [0, 0, 0, 60]] {
            assert_eq!(themed(c), c, "Dark changes nothing");
        }
    }

    #[test]
    fn every_theme_keeps_the_text_readable_on_every_ground() {
        for (id, need) in [(ThemeId::Dark, 4.5), (ThemeId::Light, 4.5), (ThemeId::HighContrast, 7.0)] {
            let t = id.theme();
            let grounds = [("bg_top", t.bg_top), ("bg_bottom", t.bg_bottom), ("card", t.card), ("card_hover", t.card_hover), ("card_sel", t.card_sel), ("field", t.field), ("panel", t.panel), ("well", t.well), ("button", t.button), ("button_hot", t.button_hot)];
            for (gn, g) in grounds {
                for (tn, c) in [("text", t.text), ("muted", t.muted), ("dim", t.dim), ("err", t.err), ("ok", t.ok), ("accent", t.accent), ("amber", t.amber)] {
                    let r = contrast(c, g);
                    assert!(r >= need, "{} {tn} on {gn}: {r:.2}:1 (needs {need})", id.name());
                }
            }
            assert!(contrast(t.border, t.card) >= if id == ThemeId::HighContrast { 7.0 } else { 1.3 }, "{} border", id.name());
        }
    }

    #[test]
    fn switching_changes_what_the_helpers_return_and_back_again() {
        let _r = Reset;
        let before = themed(ui::TEXT);
        set_current(ThemeId::Light);
        assert_eq!(themed(ui::TEXT), LIGHT.text);
        assert_eq!(themed(ui::CARD), LIGHT.card);
        assert_ne!(themed(ui::TEXT), before);
        assert_eq!(themed([ui::CARD[0], ui::CARD[1], ui::CARD[2], 77])[3], 77, "alpha is kept");
        // a colour no theme knows still turns light on Light and stays opaque
        let navy = themed([22, 30, 58, 255]);
        assert!(lum(navy) > 0.6, "{navy:?}");
        set_current(ThemeId::HighContrast);
        assert_eq!(themed(ui::BORDER), HIGH_CONTRAST.border);
        assert!(is_high_contrast() && ring() > 2.0);
        let g = generation();
        set_current(ThemeId::Dark);
        assert_ne!(generation(), g);
        assert_eq!(themed(ui::TEXT), before);
    }

    #[test]
    fn the_drawing_layer_uses_the_theme_and_keeps_the_same_shape() {
        let _r = Reset;
        let mut px = Vec::new();
        for id in ALL {
            set_current(id);
            let mut g = Gfx::new(80, 40);
            g.clear(ui::BG_TOP);
            g.rrect(10.0, 10.0, 40.0, 20.0, 6.0, ui::CARD);
            g.text(14.0, 14.0, 14.0, ui::TEXT, "Hi", crate::gfx::Face::Bold);
            px.push(g.rgba().to_vec());
            let _ = R::new(0.0, 0.0, 1.0, 1.0);
        }
        assert_ne!(px[0], px[1]);
        assert_ne!(px[0], px[2]);
        assert_ne!(px[1], px[2]);
    }

    #[test]
    fn the_choice_is_kept_per_person_beside_the_ui_size() {
        let dir = std::env::temp_dir().join(format!("iyakki-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir, "amy"), ThemeId::Dark, "nothing saved means Dark");
        crate::uisize::save(&dir, "amy", 3);
        save(&dir, "amy", ThemeId::HighContrast);
        save(&dir, "bo", ThemeId::Light);
        assert_eq!(load(&dir, "amy"), ThemeId::HighContrast);
        assert_eq!(load(&dir, "bo"), ThemeId::Light);
        assert_eq!(load(&dir, "cy"), ThemeId::Dark);
        assert_eq!(crate::uisize::load(&dir, "amy"), 3, "the size is untouched");
        crate::uisize::save(&dir, "amy", 2);
        assert_eq!(load(&dir, "amy"), ThemeId::HighContrast, "and the size does not drop the theme");
        std::fs::write(path(&dir, "bad"), "{ not json").unwrap();
        assert_eq!(load(&dir, "bad"), ThemeId::Dark);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
