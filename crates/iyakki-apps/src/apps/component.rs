//! The smallest embedding seam (docs/EMBEDDING-AND-BACKEND.md, slices B0 and B1): what a part of the shell must offer to be shown
//! inside a rectangle of another app, and the click-id routing a host uses for it.
//!
//! This is deliberately less than the full `Component` of section B.1: no registry, no `TileHost`, no `CompCx`. It has what the
//! first embedded part (the IDE's Browser tab, [`super::ide_browser`]) needs and nothing the IDE has to be rebuilt for:
//!
//! * [`Component`]: construction parameters ([`EmbedParams`]) as JSON, `render(rect, view)`, `handle(event)`, events for the host
//!   ([`HostEvent`]) drained after each call, focus in and out, and a way for the host to ask "is there a text caret in you?".
//! * [`route`]: the id scheme every host uses. A child draws with small click ids; the host moves them into its own range
//!   (`slot code * STRIDE`) while they are registered and moves them back before delivery, so ids of two parts never collide.
//!   The IDE's editor and chat use codes 1 to 4 ([`super::ide::remap`]); the embedded browser uses [`route::BROWSER`].

use serde::{Deserialize, Serialize};

use super::AppCx;
use crate::gfx::Gfx;
use crate::ui::{Event, R};

/// Typed construction and update parameters, kept as JSON so they can come from the assistant, from a saved layout or from another
/// component (section B.11 of the design).
pub type Params = serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EmbedMode {
    /// The built-in reader (no helper program).
    Reader,
    /// The web engine (WebKit or WPE, whichever the BROWSER tile uses), shared with the assistant's tab.
    #[default]
    Full,
}

/// How far the assistant may go on the page by default (the per-site approval questions are unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConsentTier {
    #[default]
    ReadOnly,
    Interactive,
}

/// What the browser view is constructed with (`{url, mode, readonly, start_consent_tier}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EmbedParams {
    pub url: Option<String>,
    pub mode: EmbedMode,
    /// The person's own clicks and typing on the page are ignored; the page can only be looked at (the assistant is unaffected).
    pub readonly: bool,
    pub start_consent_tier: ConsentTier,
}

impl EmbedParams {
    /// Read parameters; unknown fields are ignored, wrong types are an error that names the field.
    pub fn from_json(p: &Params) -> Result<EmbedParams, String> {
        serde_json::from_value(p.clone()).map_err(|e| format!("bad browser parameters: {e}"))
    }
    pub fn to_json(&self) -> Params {
        serde_json::to_value(self).unwrap_or(Params::Null)
    }
}

/// What a component tells its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// The page changed (address and title).
    Navigated { url: String, title: String },
    /// A page started or finished loading.
    Loading(bool),
    /// The component asks to be closed (Esc at its root).
    Closed,
    /// "Open externally": show this address in the stand-alone BROWSER tile.
    OpenInTab(String),
}

/// What the host passes to every draw: the scale it works at (a part never picks its own), the heartbeat, and whether it has the keyboard.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub scale: f32,
    pub tick: u64,
    pub focused: bool,
}

pub trait Component {
    fn kind(&self) -> &'static str;
    /// A title for the host's tab or header (the page title).
    fn title(&self) -> String;
    /// What should persist: exactly what `set_params` accepts.
    fn params(&self) -> Params;
    fn set_params(&mut self, p: &Params, cx: &AppCx) -> Result<(), String>;
    fn render(&mut self, g: &mut Gfx, rect: R, view: &View, cx: &AppCx);
    /// An event already moved back into the component's own ids (see [`route::decode_slot`]). True when it was used.
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> bool;
    /// Work without drawing; called on every heartbeat, also while hidden.
    fn tick(&mut self, cx: &AppCx);
    fn drain_events(&mut self) -> Vec<HostEvent>;
    fn focus(&mut self, on: bool);
    /// A text input inside the component owns the keyboard (the host shows no other caret and keeps its letters out of it).
    fn wants_text(&self) -> bool;
    /// Idempotent: stop work, release what the component holds.
    fn close(&mut self, cx: &AppCx);
}

/// Click-id routing for embedded parts.
pub mod route {
    use super::*;
    use crate::ui::Hit;

    /// A part's ids are moved up by `code * STRIDE`.
    pub const STRIDE: usize = super::super::ide::STRIDE;
    /// The embedded browser's code in the IDE (the editor, chat, tree and panel use 1 to 4).
    pub const BROWSER: usize = 5;
    const MARK: &str = "slot-mark";

    pub fn base(code: usize) -> usize {
        code * STRIDE
    }

    /// Move one event's ids into the range of `code`. Keys become `Sub(10 + code, key)`.
    pub fn remap(ev: Event, code: usize) -> Event {
        let b = base(code);
        match ev {
            Event::Tab(n) => Event::Tab(b + n),
            Event::Row(n) => Event::Row(b + n),
            Event::Focus(n) => Event::Focus(b + n),
            Event::Area(n) => Event::Area(b + n),
            Event::FocusAt(n, x) => Event::FocusAt(b + n, x),
            Event::AreaAt(n, x, y) => Event::AreaAt(b + n, x, y),
            Event::Key(k) => Event::Sub(10 + code as u8, k),
            other => other,
        }
    }

    /// The event behind an id that [`remap`] moved into the range of `code`.
    pub fn decode_slot(ev: &Event, code: usize) -> Option<Event> {
        let own = |n: usize| (n / STRIDE == code).then_some(n % STRIDE);
        match ev {
            Event::Tab(n) => own(*n).map(Event::Tab),
            Event::Row(n) => own(*n).map(Event::Row),
            Event::Focus(n) => own(*n).map(Event::Focus),
            Event::Area(n) => own(*n).map(Event::Area),
            Event::FocusAt(n, x) => own(*n).map(|m| Event::FocusAt(m, *x)),
            Event::AreaAt(n, x, y) => own(*n).map(|m| Event::AreaAt(m, *x, *y)),
            Event::Sub(c, k) if *c as usize == 10 + code => Some(Event::Key(k.clone())),
            _ => None,
        }
    }

    /// Is this event in the id range of `code`?
    pub fn is_slot(ev: &Event, code: usize) -> bool {
        decode_slot(ev, code).is_some()
    }

    /// Run a part's drawing and move the click regions it registered into its range.
    pub fn render_slot(g: &mut Gfx, code: usize, f: impl FnOnce(&mut Gfx)) {
        g.hit(R::new(-10.0, -10.0, 0.0, 0.0), Event::Tick, MARK);
        f(g);
        let start = g.hits.iter().rposition(|h| h.label == MARK).map_or(0, |i| i + 1);
        for h in g.hits[start..].iter_mut() {
            h.ev = remap(std::mem::replace(&mut h.ev, Event::Tick), code);
        }
    }

    /// Two regions of one frame with the same click id and a different rectangle are a bug (a click would reach one of
    /// them by accident). Returns a description of each such pair. Regions that exist to be stacked (the same id on the same
    /// rectangle) and the markers are not reported.
    pub fn duplicate_ids(hits: &[Hit]) -> Vec<String> {
        let key = |e: &Event| -> Option<(u8, usize)> {
            match e {
                Event::Tab(n) => Some((0, *n)),
                Event::Row(n) => Some((1, *n)),
                Event::Focus(n) | Event::FocusAt(n, _) => Some((2, *n)),
                Event::Area(n) | Event::AreaAt(n, ..) => Some((3, *n)),
                _ => None,
            }
        };
        let mut out = Vec::new();
        for (i, a) in hits.iter().enumerate() {
            let Some(ka) = key(&a.ev) else { continue };
            for b in &hits[i + 1..] {
                if key(&b.ev) == Some(ka) && (a.rect.x != b.rect.x || a.rect.y != b.rect.y) && a.rect.w > 0.0 && b.rect.w > 0.0 {
                    out.push(format!("{:?} is registered twice: \"{}\" at ({:.0},{:.0}) and \"{}\" at ({:.0},{:.0})", a.ev, a.label, a.rect.x, a.rect.y, b.label, b.rect.x, b.rect.y));
                }
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::ui::Key;

        #[test]
        fn ids_round_trip_and_stay_in_their_range() {
            for code in [1usize, 4, BROWSER, 7] {
                for ev in [Event::Tab(3), Event::Row(0), Event::Focus(49_000), Event::Area(901), Event::FocusAt(7, 12.5), Event::AreaAt(903, 1.0, 2.0), Event::Key(Key::Enter)] {
                    let moved = remap(ev.clone(), code);
                    assert_eq!(decode_slot(&moved, code), Some(ev.clone()), "{ev:?} in {code}");
                    for other in [1usize, 2, 3, 4, BROWSER, 7] {
                        if other != code {
                            assert_eq!(decode_slot(&moved, other), None, "{ev:?} moved to {code} must not decode in {other}");
                        }
                    }
                }
            }
            assert_eq!(decode_slot(&Event::Tab(9_003_000), BROWSER), None, "the host's own ids are not a part's");
            assert!(!is_slot(&Event::Tick, BROWSER));
        }

        #[test]
        fn duplicates_are_found_and_stacked_regions_are_not() {
            let mut g = Gfx::new(100, 100);
            g.hit(R::new(0.0, 0.0, 10.0, 10.0), Event::Tab(5), "a");
            g.hit(R::new(0.0, 0.0, 10.0, 10.0), Event::Tab(5), "same place");
            g.hit(R::new(20.0, 0.0, 10.0, 10.0), Event::Tab(6), "b");
            assert!(duplicate_ids(&g.hits).is_empty());
            g.hit(R::new(40.0, 40.0, 10.0, 10.0), Event::Tab(6), "elsewhere");
            let d = duplicate_ids(&g.hits);
            assert_eq!(d.len(), 1, "{d:?}");
        }

        #[test]
        fn render_slot_moves_only_the_regions_drawn_inside() {
            let mut g = Gfx::new(100, 100);
            g.hit(R::new(0.0, 0.0, 5.0, 5.0), Event::Tab(1), "outside");
            render_slot(&mut g, BROWSER, |g| g.hit(R::new(10.0, 10.0, 5.0, 5.0), Event::Tab(1), "inside"));
            let inside = g.hits.iter().find(|h| h.label == "inside").unwrap();
            let outside = g.hits.iter().find(|h| h.label == "outside").unwrap();
            assert_eq!(inside.ev, Event::Tab(BROWSER * STRIDE + 1));
            assert_eq!(outside.ev, Event::Tab(1));
        }

        #[test]
        fn parameters_read_defaults_and_refuse_wrong_types() {
            let p = EmbedParams::from_json(&serde_json::json!({"url": "http://a/", "extra": 1})).unwrap();
            assert_eq!((p.url.as_deref(), p.mode, p.readonly, p.start_consent_tier), (Some("http://a/"), EmbedMode::Full, false, ConsentTier::ReadOnly));
            let p = EmbedParams::from_json(&serde_json::json!({"mode": "reader", "readonly": true, "start_consent_tier": "interactive"})).unwrap();
            assert_eq!((p.mode, p.readonly, p.start_consent_tier), (EmbedMode::Reader, true, ConsentTier::Interactive));
            assert!(EmbedParams::from_json(&serde_json::json!({"mode": "pdf"})).is_err());
            assert!(EmbedParams::from_json(&serde_json::json!({"readonly": "yes"})).is_err());
            assert_eq!(EmbedParams::from_json(&p.to_json()).unwrap(), p);
        }
    }
}
