//! The calm message over a video the full web engine cannot decode (H.264 video, AAC sound: TikTok and the like): instead of a
//! black player, one bar over the bottom of the page says that the optional audio and video add-on is needed, with three choices.
//! The add-on is downloaded from its vendor only when the person presses Install and then confirms in Store -> Setup (docs/FULL-BROWSER.md,
//! Codec policy); nothing is fetched by this module. The bar appears once per site and session, and "Don't ask for this site" is kept
//! per person in `browser-av-<user>.json`.

use std::collections::BTreeSet;

use iyakki_shared::avaddon::{Addon, Need};
use iyakki_shared::avaddon;
use serde::{Deserialize, Serialize};

use super::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, R, TEXT};

/// Button ids (`Event::Tab`) of the bar.
pub const TAB_INSTALL: usize = 11;
pub const TAB_NOT_NOW: usize = 12;
pub const TAB_NEVER: usize = 13;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// Sites the person chose not to be asked about.
    #[serde(default)]
    pub never: BTreeSet<String>,
}

fn path(cx: &AppCx) -> std::path::PathBuf {
    cx.data_dir().join(format!("browser-av-{}.json", super::browser_config::who(cx)))
}

pub fn load(cx: &AppCx) -> Prefs {
    iyakki_schema::read_plain("browser sound and picture choices", &path(cx), iyakki_schema::now_secs()).unwrap_or_default()
}

pub fn save(cx: &AppCx, p: &Prefs) {
    if let Ok(t) = serde_json::to_string(p) {
        super::browser_config::write_checked("browser sound and picture choices", &path(cx), &t);
    }
}

/// The site a bar and a "don't ask" belong to: the host, lower case, without a leading `www.`.
pub fn site_of(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub site: String,
    pub need: Need,
    pub addon: Addon,
}

#[derive(Debug, Default)]
pub struct AvState {
    pub prefs: Prefs,
    /// Sites already told about in this session (the bar is shown once per site).
    shown: BTreeSet<String>,
    shown_drm: BTreeSet<String>,
    pub bar: Option<Bar>,
    /// The site's video is sealed by a decryption module this computer does not have (F-BROW-101): the plain message over the page.
    pub drm: Option<String>,
    /// Install was pressed: when the libraries appear the engine is started again so it finds them.
    pub waiting: bool,
}

impl AvState {
    pub fn new(prefs: Prefs) -> AvState {
        AvState { prefs, ..Default::default() }
    }

    /// The helper says the page at `host` could not play media (`need`). Whether the bar is now shown.
    pub fn offer(&mut self, host: &str, need: Need, addon: Addon) -> bool {
        let site = site_of(host);
        if site.is_empty() || self.prefs.never.contains(&site) || !self.shown.insert(site.clone()) {
            return false;
        }
        self.bar = Some(Bar { site, need, addon });
        true
    }

    /// Not now: the bar goes away (it will not come back on this site until the next session).
    pub fn dismiss(&mut self) {
        self.bar = None;
        self.drm = None;
    }

    /// The page asked for a media key system (or the site is known to seal its video): the message is shown once per site and session.
    /// True when it was set now.
    pub fn offer_drm(&mut self, host: &str, key_system: Option<&str>) -> bool {
        let site = site_of(host);
        let Some(text) = crate::apps::site_apps::protection_message(&site, key_system) else { return false };
        if site.is_empty() || self.drm.is_some() || !self.shown_drm.insert(site) {
            return false;
        }
        self.drm = Some(text.to_string());
        true
    }

    /// Don't ask for this site: remembered. True when the list changed (the caller saves it).
    pub fn never_here(&mut self) -> bool {
        let Some(b) = self.bar.take() else { return false };
        self.prefs.never.insert(b.site)
    }

    /// How many sites have been told about protected video this session (a test reads it).
    pub fn shown_drm_count(&self) -> usize {
        self.shown_drm.len()
    }

    /// The engine started again: its sites may be told about once more (the add-on may not have helped).
    pub fn shown_reset(&mut self) {
        self.shown.clear();
        self.shown_drm.clear();
    }

    /// The page moved to another site: its bar goes.
    pub fn page_moved(&mut self, host: &str) {
        if self.bar.as_ref().is_some_and(|b| b.site != site_of(host)) {
            self.bar = None;
        }
        if self.drm.is_some() && !crate::apps::site_apps::protected_site(host) {
            self.drm = None;
        }
    }
}

/// What the bar says: the add-on's text and, for a page that needs only one of the two, which one.
pub fn bar_message(b: &Bar) -> String {
    let what = match b.need {
        Need::Video => "video",
        Need::Audio => "sound",
        Need::Both => "video and sound",
    };
    match b.addon {
        Addon::NotInImage | Addon::Ready => format!("{} ({what} of {})", avaddon::bar_text(&b.addon), b.site),
        _ => avaddon::bar_text(&b.addon).to_string(),
    }
}

/// Draw the bar over the bottom of `view` (the page keeps its size) and make its buttons clickable.
pub fn render(g: &mut Gfx, view: R, s: f32, b: &Bar) {
    let pad = 10.0 * s;
    let lh = g.line_height(13.0 * s, Face::Sans);
    let text = bar_message(b);
    let lines: Vec<String> = g.wrap(13.0 * s, &text, Face::Sans, view.w - 2.0 * pad).into_iter().take(3).collect();
    let bsize = 13.0 * s;
    let h = (pad + lines.len() as f32 * lh + 6.0 * s + bsize * 2.1 + pad).min(view.h);
    let y0 = view.y + view.h - h;
    let accent = rgb(110, 190, 255);
    g.rect(view.x, y0, view.w, h, rgb(18, 28, 46));
    g.rect(view.x, y0, view.w, 1.0, [accent[0], accent[1], accent[2], 150]);
    let mut y = y0 + pad;
    for l in &lines {
        g.text(view.x + pad, y, 13.0 * s, TEXT, l, Face::Sans);
        y += lh;
    }
    y += 6.0 * s;
    let mut x = view.x + pad;
    if matches!(b.addon, Addon::Absent | Addon::Incomplete(_)) {
        x += g.button(x, y, bsize, "Install", accent, Event::Tab(TAB_INSTALL)) + 8.0 * s;
    }
    x += g.button(x, y, bsize, "Not now", rgb(150, 160, 180), Event::Tab(TAB_NOT_NOW)) + 8.0 * s;
    g.button(x, y, bsize, "Don't ask for this site", rgb(150, 160, 180), Event::Tab(TAB_NEVER));
}

/// Draw the plain message about protected video over the bottom of `view`, with one button.
pub fn render_drm(g: &mut Gfx, view: R, s: f32, text: &str) {
    let pad = 10.0 * s;
    let lh = g.line_height(13.0 * s, Face::Sans);
    let lines: Vec<String> = g.wrap(13.0 * s, text, Face::Sans, view.w - 2.0 * pad).into_iter().take(4).collect();
    let bsize = 13.0 * s;
    let h = (pad + lines.len() as f32 * lh + 6.0 * s + bsize * 2.1 + pad).min(view.h);
    let y0 = view.y + view.h - h;
    let accent = rgb(230, 170, 90);
    g.rect(view.x, y0, view.w, h, rgb(40, 30, 18));
    g.rect(view.x, y0, view.w, 1.0, [accent[0], accent[1], accent[2], 150]);
    let mut y = y0 + pad;
    for l in &lines {
        g.text(view.x + pad, y, 13.0 * s, TEXT, l, Face::Sans);
        y += lh;
    }
    g.button(view.x + pad, y + 6.0 * s, bsize, "OK", rgb(150, 160, 180), Event::Tab(TAB_NOT_NOW));
}
