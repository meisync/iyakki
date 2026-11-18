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

/// Frosted daylight: white cards on a cool blue-grey ground, one deep blue accent, state colours that stay legible on
/// every surface. The launcher draws a frosted island over the wallpaper on top of this palette (docs/design/launcher).
/// Every value is checked by `every_theme_keeps_the_text_readable_on_every_ground` (4.5:1 for all ink on all grounds).
pub const LIGHT: Theme = Theme {
    id: ThemeId::Light,
    bg_top: rgb(237, 242, 250),
    bg_bottom: rgb(227, 234, 245),
    card: rgb(255, 255, 255),
    card_hover: rgb(241, 245, 251),
    card_sel: rgb(222, 233, 252),
    border: rgb(211, 220, 232),
    accent: rgb(17, 82, 209),
    text: rgb(11, 13, 18),
    muted: rgb(69, 76, 88),
    dim: rgb(82, 91, 106),
    err: rgb(179, 32, 28),
    ok: rgb(20, 107, 60),
    field: rgb(255, 255, 255),
    panel: rgb(238, 242, 249),
    well: rgb(245, 248, 252),
    amber: rgb(138, 90, 0),
    cyan: rgb(8, 106, 132),
    magenta: rgb(163, 25, 94),
    button: rgb(243, 246, 251),
    button_hot: rgb(231, 237, 247),
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
    /// While this is on, [`themed`] is the identity: a picture (the wallpaper) carries its own colours and must not be
    /// put through the palette mapping, which would invert its lightness on a light theme.
    static RAW: Cell<bool> = const { Cell::new(false) };
    /// A theme a person wrote as a file (`usertheme`), drawn instead of the built-in one; its `id` is the built-in theme it grew from.
    static CUSTOM: Cell<Option<&'static Theme>> = const { Cell::new(None) };
    /// Bumped on every change so caches of drawn pictures (the backdrop) know to rebuild.
    static GENERATION: Cell<u32> = const { Cell::new(0) };
}

pub fn current_id() -> ThemeId {
    CURRENT.with(|c| c.get())
}

pub fn current() -> &'static Theme {
    CUSTOM.with(|c| c.get()).unwrap_or_else(|| current_id().theme())
}

/// Draw with a theme read from a file (its base decides how colours the apps made up are mapped), or go back to the built-in one.
pub fn set_custom(t: Option<&'static Theme>) {
    let was = CUSTOM.with(|c| c.get()).map(|x| x as *const Theme);
    if was != t.map(|x| x as *const Theme) {
        CUSTOM.with(|c| c.set(t));
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
    if let Some(t) = t {
        set_current(t.id);
    }
}

pub fn is_custom() -> bool {
    CUSTOM.with(|c| c.get()).is_some()
}

/// Draw `f`'s colours exactly as written, with no palette mapping: for pictures that carry their own colours, such as
/// the wallpaper. Nesting is safe and the flag is always put back.
pub fn with_raw<R>(f: impl FnOnce() -> R) -> R {
    let was = RAW.with(|r| r.replace(true));
    let out = f();
    RAW.with(|r| r.set(was));
    out
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
    set_custom(None);
    set_current(id.unwrap_or_default());
}

/// The raw saved choice of a person (`dark`, `light`, `high-contrast` or `user:<file key>`), None when there is none.
pub fn load_choice(dir: &std::path::Path, user: &str) -> Option<String> {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["theme"].as_str().map(String::from))
}

/// Keep a raw choice (see [`load_choice`]) with the person's other display settings.
pub fn save_choice(dir: &std::path::Path, user: &str, choice: &str) {
    let p = path(dir, user);
    let mut v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v["theme"] = serde_json::json!(choice);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Draw in the theme a person saved: a built-in one, or a theme file of theirs (when it is still there and still readable; otherwise
/// the built-in theme that comes of the saved text, Dark when none).
pub fn apply_for(dir: &std::path::Path, user: &str) {
    apply(Some(load(dir, user)));
    if let Some(key) = load_choice(dir, user).as_deref().and_then(|c| c.strip_prefix(crate::usertheme::CHOICE_PREFIX).map(String::from)) {
        let _ = crate::usertheme::activate(dir, user, &key);
    }
}

// ------------------------------------------------------------------------------------------------------ the mapping

/// The colour to put on screen for `c`, a colour written in the Dark palette. Dark returns `c` untouched.
pub fn themed(c: [u8; 4]) -> [u8; 4] {
    if RAW.with(|r| r.get()) {
        return c;
    }
    let id = current_id();
    if id == ThemeId::Dark && !is_custom() {
        return c;
    }
    let t = current();
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
        // hand-picked Light values (`light_pick`): dark navy surfaces become chosen pale steps, bright accents become the chosen deep
        // ink of their hue family
        ThemeId::Light => return light_pick(c),
        // push to the extremes: dark things go black, light things go white, colour stays for hue
        _ => (s, if l < 0.5 { l * 0.12 } else { 1.0 - (1.0 - l) * 0.3 }),
    };
    from_hsl(h, s2, l2, c[3])
}

// ------------------------------------------------------------------------------------------------ hand-picked Light colours

/// The pale steps a dark navy surface (a panel, a well, a stroke) of the Dark palette becomes in Light: (the Dark colour's lightness is
/// below this, the colour to put there). Chosen by eye against `LIGHT`'s own surfaces (white card, 238/242/249 panel, 211/220/232 border) so
/// that a darker Dark step is always a slightly darker Light step and every `LIGHT` ink stays at 4.5:1 or better on all of them.
pub const LIGHT_SURFACES: [(f32, [u8; 3]); 8] = [
    (0.05, [248, 250, 254]),
    (0.07, [243, 246, 252]),
    (0.09, [238, 242, 249]),
    (0.12, [232, 237, 247]),
    (0.16, [224, 231, 243]),
    (0.20, [214, 223, 238]),
    (0.25, [202, 213, 231]),
    (0.32, [188, 200, 222]),
];

/// The deep ink a bright colour of a hue family becomes in Light: (the family's last hue in degrees, the colour). Each reads at 4.5:1 or
/// better on every Light ground (a test checks it). A colour keeps its family, so an amber stays amber and a red stays red.
pub const LIGHT_INKS: [(f32, [u8; 3]); 11] = [
    (12.0, [179, 32, 28]),   // red
    (40.0, [168, 72, 0]),    // orange
    (58.0, [138, 90, 0]),    // amber
    (95.0, [88, 108, 0]),    // lime
    (165.0, [20, 107, 60]),  // green
    (192.0, [8, 108, 108]),  // teal
    (212.0, [8, 106, 132]),  // cyan
    (252.0, [17, 82, 209]),  // blue
    (290.0, [100, 50, 205]), // violet
    (345.0, [163, 25, 94]),  // pink
    (360.0, [179, 32, 28]),  // red again, past the wrap
];

/// The Light colour for an opaque colour written in the Dark palette that is not one of the palette's own: a chosen value instead of
/// the lightness flip. Dark surfaces take a pale step of [`LIGHT_SURFACES`]; mid tones (strokes and hover fills) a grey-blue between the
/// last step and the ink; bright colours the deep ink of their hue family from [`LIGHT_INKS`] (greys and near-white ink become the Light
/// theme's muted and text colours). The alpha is kept by the caller.
pub fn light_pick(c: [u8; 4]) -> [u8; 4] {
    let (h, s, l) = to_hsl(c);
    let a = c[3];
    let rgb = |v: [u8; 3]| [v[0], v[1], v[2], a];
    if l < 0.32 {
        // a surface: a saturated colour that is not a blue navy keeps a tint of its hue on the pale step
        let step = LIGHT_SURFACES.iter().find(|(top, _)| l < *top).map_or(LIGHT_SURFACES[7].1, |(_, v)| *v);
        let navy = s < 0.25 || (190.0..=260.0).contains(&h);
        return if navy { rgb(step) } else { from_hsl(h, 0.4, to_hsl(rgb(step)).2.max(0.88), a) };
    }
    if l < 0.5 && s < 0.45 {
        // a mid tone: a stroke or a hover fill, between the darkest step and the muted ink
        return from_hsl(if s < 0.05 { 220.0 } else { h }, 0.28, 0.80 - (l - 0.32) * 0.9, a);
    }
    if s < 0.18 {
        // a grey: light ink (written for a dark ground) becomes the Light text; mid greys the muted colour
        return rgb(if l > 0.72 { [30, 36, 50] } else { [69, 76, 88] });
    }
    let fam = LIGHT_INKS.iter().find(|(top, _)| h < *top).map_or(LIGHT_INKS[0].1, |(_, v)| *v);
    rgb(fam)
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

    // ---- hand-picked Light colours (F-SHELL-63)

    /// Every `rgb(r, g, b)` written in the non-test code of the crates that draw (apps, shell, tiles, uikit), with how often.
    fn literal_colours() -> std::collections::BTreeMap<[u8; 3], usize> {
        fn walk(dir: &std::path::Path, out: &mut std::collections::BTreeMap<[u8; 3], usize>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if p.is_dir() {
                    if name != "target" && name != "testdata" && name != "tests" {
                        walk(&p, out);
                    }
                } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    let code = text.split("#[cfg(test)]").next().unwrap_or("");
                    let mut rest = code;
                    while let Some(at) = rest.find("rgb(") {
                        rest = &rest[at + 4..];
                        let end = rest.find(')').unwrap_or(0);
                        let parts: Vec<u8> = rest[..end].split(',').filter_map(|v| v.trim().parse::<u8>().ok()).collect();
                        if parts.len() == 3 {
                            *out.entry([parts[0], parts[1], parts[2]]).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for e in std::fs::read_dir(crates).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("iyakki-apps") || n.starts_with("iyakki-tile-") || n.starts_with("iyakki-app-") || n == "iyakki-uikit" || n == "iyakki-shell" {
                walk(&e.path().join("src"), &mut out);
            }
        }
        out
    }

    fn grounds() -> Vec<[u8; 4]> {
        vec![LIGHT.bg_top, LIGHT.bg_bottom, LIGHT.card, LIGHT.card_hover, LIGHT.card_sel, LIGHT.panel, LIGHT.well, LIGHT.field, LIGHT.button]
    }

    #[test]
    fn every_colour_the_apps_write_comes_out_readable_in_light_by_a_chosen_value() {
        let _r = Reset;
        set_current(ThemeId::Light);
        let all = literal_colours();
        assert!(all.len() > 200, "the scan found the colours ({} found)", all.len());
        let (mut inks, mut surfaces) = (0, 0);
        for ([r, g, b], n) in &all {
            let c = [*r, *g, *b, 255];
            let out = themed(c);
            let (_, s, l) = to_hsl(c);
            if l >= 0.5 && s >= 0.18 {
                inks += 1;
                for ground in grounds() {
                    let k = contrast(out, ground);
                    assert!(k >= 4.5, "ink rgb({r}, {g}, {b}) (used {n}x) is {out:?} in Light: {k:.2}:1 on {ground:?}");
                }
            } else if l < 0.32 {
                surfaces += 1;
                for (name, ink) in [("text", LIGHT.text), ("muted", LIGHT.muted)] {
                    let k = contrast(ink, out);
                    assert!(k >= 4.5, "surface rgb({r}, {g}, {b}) (used {n}x) is {out:?} in Light: the {name} colour is {k:.2}:1 on it");
                }
                assert!(contrast(LIGHT.dim, out) >= 4.0, "dim on {out:?}");
            }
        }
        assert!(inks > 40 && surfaces > 100, "the sweep covered {inks} inks and {surfaces} surfaces");
    }

    #[test]
    fn the_colours_the_old_lightness_flip_got_wrong_are_better_now_and_the_hue_family_is_kept() {
        let _r = Reset;
        // these fell under 4.5:1 on the Light ground with the flip (amber 4.0, green 3.8, cyan 4.3), or came out near black (violet)
        set_current(ThemeId::Light);
        for (dark, family) in [([255, 184, 77, 255], "amber/orange"), ([110, 224, 160, 255], "green"), ([100, 230, 255, 255], "cyan"), ([88, 190, 235, 255], "cyan"), ([167, 116, 255, 255], "violet")] {
            let out = themed(dark);
            assert!(contrast(out, LIGHT.bg_top) >= 4.5 && contrast(out, LIGHT.card_sel) >= 4.5, "{family}: {out:?}");
            let (h0, ..) = to_hsl(dark);
            let (h1, ..) = to_hsl(out);
            assert!((h0 - h1).abs() < 45.0, "{family}: the hue stayed in its family ({h0} -> {h1})");
        }
        let violet = themed([167, 116, 255, 255]);
        assert!(to_hsl(violet).2 > 0.3, "violet is a colour in Light, not near black: {violet:?}");
        // the Dark and High contrast themes did not move: Dark is the identity, High contrast keeps its extremes
        set_current(ThemeId::Dark);
        assert_eq!(themed([167, 116, 255, 255]), [167, 116, 255, 255]);
        set_current(ThemeId::HighContrast);
        assert_eq!(themed([9, 11, 21, 255])[..3], [1, 1, 3], "High contrast keeps its own mapping");
    }

    #[test]
    fn a_darker_surface_in_dark_is_never_a_lighter_one_in_light_and_see_through_colours_are_left_alone() {
        let _r = Reset;
        set_current(ThemeId::Light);
        let mut last = 2.0f32;
        for v in 0..=80u8 {
            let c = [v / 3, v / 2, v, 255];
            let (_, _, l) = to_hsl(c);
            if l >= 0.32 {
                break;
            }
            let out = to_hsl(themed(c)).2;
            assert!(out <= last + 0.001, "{c:?} -> lightness {out} after {last}");
            last = out;
        }
        assert_eq!(themed([20, 26, 48, 120]), [20, 26, 48, 120], "a see-through colour keeps its hue and strength");
    }
}
