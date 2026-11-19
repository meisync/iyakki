//! Desktop wallpapers: what is drawn behind the launcher, the sign-in screen and the lock screen.
//!
//! A wallpaper here is **drawn, never decoded**. Each one is a handful of gradients and soft glows that [`Gfx`] paints at
//! whatever size the screen is, so nothing is added to the image (the libre ISO is about 60 MB), there is no picture
//! format to parse at boot, and every wallpaper is sharp at 720p and at 4K alike. A later step can add a wallpaper read
//! from a file; the drawing side is the same, only the source of the pixels changes.
//!
//! It costs nothing per frame: [`crate::fx::background`] keeps the finished picture in a cache keyed by the size, the
//! theme and [`generation`], and copies it. Choosing another wallpaper bumps the generation, so the next frame rebuilds
//! once and then copies again.
//!
//! The choice belongs to the person, like the theme and the UI size, and is kept beside them in `ui-<user>.json`
//! (key `wallpaper`). [`Id::Theme`] is the default and draws exactly what the shell drew before wallpapers existed, so
//! nobody's screen changes until they choose one.

use std::cell::Cell;

use crate::gfx::{rgb, Gfx};

/// The wallpapers, in the order Settings offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Id {
    /// The theme's own background: the gradient, corner glows, grid and particles the shell has always drawn.
    #[default]
    Theme,
    /// Cool morning blue falling through white into a warm horizon.
    Daylight,
    /// Deep indigo into magenta and amber, for a dark room.
    Dusk,
    /// Mint and cream, quiet and pale.
    Meadow,
    /// One neutral blue-grey wash: the quietest of them.
    Slate,
    /// A picture the person put in their theme folder (`wallpaper.png` or `wallpaper.jpg`), decoded by the shell and handed over with
    /// [`set_picture`]. Not in [`ALL`] (nothing offers it as a button): a Look file or a chat tool names it by key. With no picture loaded
    /// (none there, too big, unreadable) it draws as [`Id::Theme`], so a bad file never leaves a person without a background.
    Picture,
}

pub const ALL: [Id; 5] = [Id::Theme, Id::Daylight, Id::Dusk, Id::Meadow, Id::Slate];

impl Id {
    /// The name Settings shows.
    pub fn name(self) -> &'static str {
        match self {
            Id::Theme => "Theme",
            Id::Daylight => "Daylight",
            Id::Dusk => "Dusk",
            Id::Meadow => "Meadow",
            Id::Slate => "Slate",
            Id::Picture => "Picture",
        }
    }

    /// The key kept on disk and used by `iyakki dev config`.
    pub fn key(self) -> &'static str {
        match self {
            Id::Theme => "theme",
            Id::Daylight => "daylight",
            Id::Dusk => "dusk",
            Id::Meadow => "meadow",
            Id::Slate => "slate",
            Id::Picture => "picture",
        }
    }

    /// The wallpaper for a saved key; anything not understood is [`Id::Theme`], so a file from a newer release never
    /// leaves a person without a background.
    pub fn from_key(k: &str) -> Id {
        ALL.into_iter().chain([Id::Picture]).find(|w| w.key() == k).unwrap_or_default()
    }

    /// Is this one light enough that dark text reads on it? Settings says so beside the name, because a person who
    /// chose the Dark theme and then a pale wallpaper would otherwise wonder why the sign-in text is hard to read.
    pub fn is_light(self) -> bool {
        match self {
            Id::Picture => PICTURE.with(|p| p.borrow().as_ref().is_some_and(|p| p.light)),
            _ => matches!(self, Id::Daylight | Id::Meadow | Id::Slate),
        }
    }
}

/// The most pixels a wallpaper picture keeps (a larger one is shrunk by whole factors when it is handed over) and the largest side accepted.
pub const PICTURE_MAX_PIXELS: usize = 4096 * 2304;
pub const PICTURE_MAX_SIDE: u32 = 16384;

struct Picture {
    w: u32,
    h: u32,
    /// Opaque RGBA, `w * h * 4`.
    rgba: Vec<u8>,
    /// Light enough that dark text reads on it (mean brightness above the middle).
    light: bool,
}

thread_local! {
    static PICTURE: std::cell::RefCell<Option<Picture>> = const { std::cell::RefCell::new(None) };
    static CURRENT: Cell<Id> = const { Cell::new(Id::Theme) };
    /// Bumped on every change so the background cache knows to rebuild.
    static GENERATION: Cell<u32> = const { Cell::new(0) };
}

pub fn current() -> Id {
    CURRENT.with(|c| c.get())
}

/// Bumped whenever the wallpaper changes; [`crate::fx::background`] keys its cache on it.
pub fn generation() -> u32 {
    GENERATION.with(|g| g.get())
}

/// Hand over the decoded picture for [`Id::Picture`] (`None` takes it away). Refused (and the old one dropped) when the size is zero, a side is
/// over [`PICTURE_MAX_SIDE`] or the bytes do not match the size. A picture over [`PICTURE_MAX_PIXELS`] is shrunk by a whole factor (box average).
/// Returns whether a picture is held now.
pub fn set_picture(p: Option<(u32, u32, Vec<u8>)>) -> bool {
    let mut held = None;
    if let Some((w, h, rgba)) = p {
        if w > 0 && h > 0 && w <= PICTURE_MAX_SIDE && h <= PICTURE_MAX_SIDE && rgba.len() == w as usize * h as usize * 4 {
            let (mut w, mut h, mut rgba) = (w, h, rgba);
            while w as usize * h as usize > PICTURE_MAX_PIXELS && w >= 2 && h >= 2 {
                let (nw, nh) = (w / 2, h / 2);
                let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
                for y in 0..nh as usize {
                    for x in 0..nw as usize {
                        for c in 0..3 {
                            let at = |dx: usize, dy: usize| rgba[((2 * y + dy) * w as usize + 2 * x + dx) * 4 + c] as u32;
                            out.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4) as u8);
                        }
                        out.push(255);
                    }
                }
                (w, h, rgba) = (nw, nh, out);
            }
            let mut sum = 0u64;
            let step = (rgba.len() / 4 / 4096).max(1);
            let mut n = 0u64;
            for px in rgba.chunks_exact(4).step_by(step) {
                sum += (px[0] as u64 * 299 + px[1] as u64 * 587 + px[2] as u64 * 114) / 1000;
                n += 1;
            }
            let light = n > 0 && sum / n > 150;
            held = Some(Picture { w, h, rgba, light });
        }
    }
    let on = held.is_some();
    PICTURE.with(|c| *c.borrow_mut() = held);
    GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    on
}

pub fn has_picture() -> bool {
    PICTURE.with(|p| p.borrow().is_some())
}

/// Scale the picture to cover the whole of `g` (centred, the overflow cropped), nearest-neighbour from a box-averaged source.
fn draw_picture(g: &mut Gfx) {
    let (gw, gh) = (g.width() as usize, g.height() as usize);
    PICTURE.with(|p| {
        let p = p.borrow();
        let Some(p) = p.as_ref() else { return };
        let (pw, ph) = (p.w as f32, p.h as f32);
        let k = (gw as f32 / pw).max(gh as f32 / ph);
        let (ox, oy) = ((pw * k - gw as f32) / 2.0, (ph * k - gh as f32) / 2.0);
        let dst = g.pm.data_mut();
        if dst.len() != gw * gh * 4 {
            return;
        }
        for y in 0..gh {
            let sy = (((y as f32 + oy) / k) as usize).min(p.h as usize - 1);
            let row = &p.rgba[sy * p.w as usize * 4..(sy + 1) * p.w as usize * 4];
            for x in 0..gw {
                let sx = (((x as f32 + ox) / k) as usize).min(p.w as usize - 1);
                let o = (y * gw + x) * 4;
                dst[o..o + 4].copy_from_slice(&row[sx * 4..sx * 4 + 4]);
            }
        }
    });
}

pub fn set_current(id: Id) {
    if current() != id {
        CURRENT.with(|c| c.set(id));
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

// --------------------------------------------------------------------------------------------------- saved per person

fn path(dir: &std::path::Path, user: &str) -> std::path::PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("ui-{who}.json"))
}

/// The person's saved wallpaper, [`Id::Theme`] when there is none.
pub fn load(dir: &std::path::Path, user: &str) -> Id {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["wallpaper"].as_str().map(Id::from_key)).unwrap_or_default()
}

/// Keep the choice beside the person's theme and UI size, leaving everything else in the file alone.
pub fn save(dir: &std::path::Path, user: &str, id: Id) {
    let p = path(dir, user);
    let mut v = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    v["wallpaper"] = serde_json::json!(id.key());
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Draw in the person's wallpaper (or the theme's own at the sign-in screen, with `None`).
pub fn apply(id: Option<Id>) {
    set_current(id.unwrap_or_default());
}

/// Draw in the wallpaper this person saved.
pub fn apply_for(dir: &std::path::Path, user: &str) {
    set_current(load(dir, user));
}

// ------------------------------------------------------------------------------------------------------- the drawing

/// Paint the current wallpaper over the whole of `g`. Returns false for [`Id::Theme`], which means "the caller draws
/// the theme's own background instead"; this is the only place that decides what a wallpaper looks like.
pub fn draw(g: &mut Gfx) -> bool {
    let id = current();
    if id == Id::Theme || (id == Id::Picture && !has_picture()) {
        return false;
    }
    let (w, h) = (g.width() as f32, g.height() as f32);
    // a wallpaper carries its own colours: without this the palette mapping would invert its lightness on a light theme
    crate::theme::with_raw(|| match id {
        Id::Theme => {}
        Id::Picture => draw_picture(g),
        Id::Daylight => {
            g.gradient_rect(0.0, 0.0, w, h, (0.0, 0.0), (w, h), &[(0.0, rgb(173, 206, 255)), (0.42, rgb(236, 243, 252)), (0.72, rgb(252, 244, 236)), (1.0, rgb(248, 214, 190))]);
            g.radial_glow(w * 0.24, h * 0.14, w * 0.46, [255, 255, 255, 120]);
            g.radial_glow(w * 0.88, h * 0.92, w * 0.42, [120, 170, 240, 46]);
            ribbon(g, w, h, 0.30, w * 0.17, [255, 255, 255, 54]);
            ribbon(g, w, h, 0.56, w * 0.11, [146, 190, 248, 44]);
        }
        Id::Dusk => {
            g.gradient_rect(0.0, 0.0, w, h, (0.0, 0.0), (w * 0.3, h), &[(0.0, rgb(18, 22, 54)), (0.45, rgb(48, 32, 86)), (0.78, rgb(122, 48, 96)), (1.0, rgb(196, 104, 72))]);
            g.radial_glow(w * 0.82, h * 0.86, w * 0.5, [255, 170, 110, 54]);
            g.radial_glow(w * 0.12, h * 0.1, w * 0.4, [90, 120, 255, 40]);
            ribbon(g, w, h, 0.38, w * 0.13, [255, 255, 255, 20]);
        }
        Id::Meadow => {
            g.gradient_rect(0.0, 0.0, w, h, (0.0, h), (w, 0.0), &[(0.0, rgb(214, 238, 224)), (0.5, rgb(243, 248, 240)), (1.0, rgb(250, 247, 232))]);
            g.radial_glow(w * 0.18, h * 0.86, w * 0.44, [150, 206, 176, 52]);
            g.radial_glow(w * 0.9, h * 0.12, w * 0.36, [255, 246, 206, 60]);
            ribbon(g, w, h, 0.62, w * 0.14, [255, 255, 255, 58]);
        }
        Id::Slate => {
            g.gradient_rect(0.0, 0.0, w, h, (0.0, 0.0), (0.0, h), &[(0.0, rgb(226, 232, 240)), (1.0, rgb(203, 213, 227))]);
            g.radial_glow(w * 0.5, h * -0.1, w * 0.7, [255, 255, 255, 70]);
        }
    });
    // The shell writes the top bar and the footer hints straight onto the background in the theme's own ink. A wallpaper
    // can put any colour under them, so give that chrome a ground to sit on: a band of the theme's background fading out
    // after it. These are themed (they belong to the chrome, not to the picture), so they follow Dark, Light and High
    // contrast with the text they protect.
    let ground = crate::theme::themed(crate::ui::BG_TOP);
    let band = |a: u8| [ground[0], ground[1], ground[2], a];
    // a share of the height with a ceiling, never a fixed floor: a floor taller than the screen would cover the picture
    let top = (h * 0.17).min(190.0);
    let foot = (h * 0.10).min(120.0);
    g.gradient_rect(0.0, 0.0, w, top, (0.0, 0.0), (0.0, top), &[(0.0, band(236)), (0.8, band(224)), (1.0, band(0))]);
    g.gradient_rect(0.0, h - foot, w, foot, (0.0, h - foot), (0.0, h), &[(0.0, band(0)), (0.3, band(224)), (1.0, band(236))]);
    true
}

/// One soft band sweeping across the screen: a row of overlapping glows, so it costs the same few draws at any size and
/// needs no path or blur. `at` is where it crosses the middle of the screen, 0 to 1 of the height.
fn ribbon(g: &mut Gfx, w: f32, h: f32, at: f32, thickness: f32, c: [u8; 4]) {
    let steps = 14;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = -w * 0.08 + t * w * 1.16;
        // a gentle S, so the band is never a straight line
        let y = h * at + (t * std::f32::consts::PI * 1.6).sin() * h * 0.11;
        g.radial_glow(x, y, thickness, c);
    }
}

#[cfg(test)]
#[path = "wallpaper_tests.rs"]
mod tests;
