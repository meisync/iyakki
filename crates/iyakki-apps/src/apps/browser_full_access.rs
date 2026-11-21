//! The page shown by the full web engine in the toolkit's access tree (F-A11Y-06). The engine's picture is only pixels, so the helper's
//! injected script (`site-probe.js`) lists the headings, links, buttons, form fields, pictures and landmarks that are on screen with their
//! names and their boxes (`FromHelper::PageAccess`); each becomes a region over the picture, described to the screen reader with the same
//! words the page reader of the light engine uses. A press on a link, a button or a field clicks the middle of its box in the page; the
//! other kinds only put the reader there. The regions sit at the front of the frame's list, so the page still gets every mouse click.
//!
//! The list is only as fresh as the script's last report (at most twice a second); the reader says what was on screen then. Content the
//! script cannot see (canvas, closed shadow roots, other origins' frames) is not in it. A child module of `browser`.

use super::*;

/// First of the regions of the engine's page (`Event::Tab`); above the light engine's [`a11y`] ranges.
pub(super) const FULL_TAB: usize = 8600;
pub(super) const FULL_MAX: usize = 160;

/// One thing on the page the script listed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FullNode {
    pub role: String,
    pub name: String,
    pub level: u32,
    /// The box in CSS pixels of the viewport.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FullNode {
    /// What the screen reader says for it, the words of the light engine's reader ("Heading level 2, Pricing").
    pub fn words(&self) -> String {
        let name = self.name.trim();
        let with = |kind: &str| if name.is_empty() { kind.to_string() } else { format!("{kind}, {name}") };
        match self.role.as_str() {
            "heading" => with(&format!("Heading level {}", self.level.clamp(1, 6))),
            "link" => with("Link"),
            "button" => with("Button"),
            "img" => with("Image"),
            "field" => with("Edit"),
            "check" => with("Check box"),
            // a password box is said by its kind; its text is never in the list
            "password" => with("Password field"),
            "main" => "Main landmark".into(),
            "navigation" => with("Navigation landmark"),
            "banner" => "Banner landmark".into(),
            "contentinfo" => "Footer landmark".into(),
            "complementary" => "Complementary landmark".into(),
            "search" => "Search landmark".into(),
            "form" => with("Form"),
            _ => String::new(),
        }
    }

    /// A press clicks the page for these kinds.
    pub fn clickable(&self) -> bool {
        matches!(self.role.as_str(), "link" | "button" | "field" | "check" | "password")
    }
}

/// The helper's JSON list, cleaned: known kinds only, at most [`FULL_MAX`], names cut at 120 characters, boxes inside a sane range.
pub(super) fn parse(json: &str) -> Vec<FullNode> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_f64).filter(|n| n.is_finite() && n.abs() < 100_000.0).map(|n| n as f32);
    let mut out = Vec::new();
    for it in items.iter().take(FULL_MAX) {
        let role = it.get("r").and_then(serde_json::Value::as_str).unwrap_or("");
        let (Some(x), Some(y), Some(w), Some(h)) = (num(it, "x"), num(it, "y"), num(it, "w"), num(it, "h")) else { continue };
        let n = FullNode {
            role: role.to_string(),
            name: it.get("n").and_then(serde_json::Value::as_str).unwrap_or("").chars().filter(|c| !c.is_control()).take(120).collect(),
            level: it.get("l").and_then(serde_json::Value::as_u64).map_or(2, |l| l.min(6) as u32),
            x,
            y,
            w,
            h,
        };
        if !n.words().is_empty() && w >= 1.0 && h >= 1.0 {
            out.push(n);
        }
    }
    out
}

impl BrowserApp {
    /// Describe the page's things to the screen reader as regions over the engine's picture. Only while the screen reader is on.
    pub(super) fn full_access_register(&mut self, g: &mut Gfx, view: R) {
        self.full_access_rects.clear();
        if !iyakki_uikit::access::prefs().screen_reader || self.full_access.is_empty() {
            return;
        }
        let Some(f) = self.full.as_ref() else { return };
        let css = f.css_scale();
        let first = g.hits.len();
        for (k, n) in self.full_access.iter().enumerate() {
            let r = R::new(view.x + n.x * css, view.y + n.y * css, n.w * css, n.h * css);
            if r.y + r.h < view.y || r.y > view.y + view.h || r.x + r.w < view.x || r.x > view.x + view.w {
                continue;
            }
            let words = n.words();
            g.hit(r, Event::Tab(FULL_TAB + k), &words);
            iyakki_uikit::access::describe(&words, &Event::Tab(FULL_TAB + k), iyakki_uikit::access::Meta::new(iyakki_uikit::access::Role::Spoken, iyakki_uikit::access::State::default()).named(&words));
            self.full_access_rects.push((k, r));
        }
        let mine = g.hits.split_off(first);
        g.hits.splice(0..0, mine);
    }

    /// A press on one of those regions: a link, button or field is clicked in the page; the rest only has the reader's attention.
    pub(super) fn full_access_press(&mut self, n: usize) -> bool {
        if !(FULL_TAB..FULL_TAB + FULL_MAX).contains(&n) {
            return false;
        }
        let k = n - FULL_TAB;
        let (Some(node), Some((_, r))) = (self.full_access.get(k), self.full_access_rects.iter().find(|(i, _)| *i == k).copied()) else { return true };
        if node.clickable() {
            if let Some(f) = self.full.as_mut() {
                let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
                f.mouse_move(x, y);
                f.mouse_down(x, y, 1);
                f.mouse_up(x, y);
            }
        }
        true
    }
}
