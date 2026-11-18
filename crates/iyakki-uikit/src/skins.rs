//! Skins (F-SDK-122): a skin is a Look file (the existing format, `docs/development/LOOK-DESIGN.md`) that restyles the SDK widgets'
//! shape, edges, spacing, shadow, glass and motion as well as the colours. This module holds the **registry of shipped skins** (files
//! in `crates/iyakki-uikit/skins/`, compiled in), the **validator** every skin passes (the Look rules plus the readability rules of
//! the widgets' own pairs: the button against its ground, and a label colour that reads on it) and the **accent fit** an app uses to ship
//! its own accent colour without breaking the person's contrast.
//!
//! Choosing a skin is choosing a Look: the shipped skins are listed after the three built-in Looks by [`crate::look_registry`], kept per
//! person as the Look choice, and applied live (the shell reads the choice again at its next heartbeat, an SDK host takes
//! `Host::set_look`); no app restarts.

use crate::look::{self, Accepted, Look, LookProblem};
use crate::theme::Theme;
use crate::usertheme::{self, contrast};

/// The shipped skins: `(file name without extension, file text)`.
pub const SHIPPED: [(&str, &str); 4] = [
    ("paper", include_str!("../skins/paper.look")),
    ("glass-night", include_str!("../skins/glass-night.look")),
    ("blueprint", include_str!("../skins/blueprint.look")),
    ("calm", include_str!("../skins/calm.look")),
];

/// Text on a primary button (WCAG AA for normal text); [`label_on`] picks the colour that reaches it.
pub const LABEL_ON_ACCENT: f64 = 4.5;
/// A control against what it sits on (WCAG AA for components).
pub const ACCENT_ON_GROUND: f64 = 3.0;

/// Every shipped skin as a Look that passed [`validate`], in order.
pub fn shipped() -> Vec<(String, Result<Accepted, Vec<LookProblem>>)> {
    SHIPPED.iter().map(|(k, t)| (k.to_string(), validate(t))).collect()
}

/// The Look rules of [`look::parse`] and the widgets' own pairs: the accent against the card and the page ground (3:1), and a label that
/// reaches 4.5:1 on it ([`label_on`] always finds one, so a skin never fails on that). A skin that fails is refused with every reason.
pub fn validate(text: &str) -> Result<Accepted, Vec<LookProblem>> {
    let a = look::parse(text)?;
    let bad = pair_problems(&a.look.theme);
    if bad.is_empty() { Ok(a) } else { Err(bad) }
}

/// The colour of a label drawn on `fill` (a primary button): the theme's text when it reads at 4.5:1, else white or black, whichever reads
/// better. A pale or a deep accent therefore never gets an unreadable label.
pub fn label_on(t: &Theme, fill: [u8; 4]) -> [u8; 4] {
    let mut best = t.text;
    if contrast(best, fill) >= LABEL_ON_ACCENT {
        return best;
    }
    for c in [[255, 255, 255, 255], [0, 0, 0, 255]] {
        if contrast(c, fill) > contrast(best, fill) {
            best = c;
        }
    }
    best
}

fn pair_problems(t: &Theme) -> Vec<LookProblem> {
    let mut v = Vec::new();
    let mut say = |m: String| v.push(LookProblem { line: 0, message: m });
    let r = contrast(label_on(t, t.accent), t.accent);
    if r < LABEL_ON_ACCENT {
        say(format!("no label colour reads on `accent` (best {r:.1}:1, needs {LABEL_ON_ACCENT}:1)"));
    }
    for (name, g) in [("card", t.card), ("bg_top", t.bg_top)] {
        let r = contrast(t.accent, g);
        if r < ACCENT_ON_GROUND {
            say(format!("`accent` on `{name}` is {r:.1}:1 and needs at least {ACCENT_ON_GROUND}:1"));
        }
    }
    v
}

/// Does this accent keep the theme's pairs readable: its label, and the button against the card and the page?
pub fn accent_ok(t: &Theme, accent: [u8; 4]) -> bool {
    let probe = Theme { accent, ..*t };
    pair_problems(&probe).is_empty()
}

/// An app's wish for its own accent, after the contrast check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    /// The colour to draw with.
    pub colour: [u8; 4],
    /// What the app asked for.
    pub wanted: [u8; 4],
    /// The colour was changed (moved towards the theme's own, or replaced by it).
    pub adjusted: bool,
}

/// The accent an app may draw with: its own when the pairs of [`accent_ok`] hold on this theme; otherwise the nearest colour on the line
/// from its own towards the theme's accent that does hold (so the app's hue is kept as long as it can be); the theme's accent when none
/// on that line does. Alpha is ignored (an accent is opaque). Never returns a colour that fails, unless the theme's own accent does.
pub fn fit_accent(t: &Theme, wanted: [u8; 4]) -> Accent {
    let wanted = [wanted[0], wanted[1], wanted[2], 255];
    if accent_ok(t, wanted) {
        return Accent { colour: wanted, wanted, adjusted: false };
    }
    let own = t.accent;
    for step in 1..=20u32 {
        let k = step as f32 / 20.0;
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * k).round() as u8;
        let c = [mix(wanted[0], own[0]), mix(wanted[1], own[1]), mix(wanted[2], own[2]), 255];
        if accent_ok(t, c) {
            return Accent { colour: c, wanted, adjusted: true };
        }
    }
    Accent { colour: own, wanted, adjusted: true }
}

/// What an app may ask about the skin in use (`UiCx::skin`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinInfo {
    /// The key it is chosen by (the file name for a file).
    pub key: String,
    pub name: String,
    /// The theme it grew from (`dark`, `light`, `high-contrast`).
    pub base: &'static str,
    /// The page ground is dark.
    pub dark: bool,
    /// The skin wants frosted surfaces.
    pub glass: bool,
    /// The person asked for reduced motion.
    pub reduced_motion: bool,
    /// Shape, edge and spacing words of the skin, as written in a Look file.
    pub shape: &'static str,
    pub edge: &'static str,
    pub spacing: &'static str,
}

impl SkinInfo {
    pub fn of(l: &Look) -> SkinInfo {
        let bg = l.theme.bg_top;
        SkinInfo {
            key: l.key(),
            name: l.name.clone(),
            base: l.theme.id.key(),
            dark: (u32::from(bg[0]) * 299 + u32::from(bg[1]) * 587 + u32::from(bg[2]) * 114) / 1000 < 128,
            glass: l.surface.glass,
            reduced_motion: l.motion == look::Motion::Reduced,
            shape: l.surface.shape.key(),
            edge: l.surface.edge.key(),
            spacing: l.surface.spacing.key(),
        }
    }
}

/// The key of a shipped skin by its file name is also a Look key; true when `key` is one of them.
pub fn is_shipped(key: &str) -> bool {
    SHIPPED.iter().any(|(k, _)| usertheme::key_of(k) == key)
}

#[cfg(test)]
#[path = "skins_tests.rs"]
mod tests;
