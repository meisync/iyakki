//! The home screen's six pages (docs/design/launcher, docs/development/LAUNCHER-PLAN.md slice 1): *The machine*, then the groups.
//!
//! Page 0 is the machine page (a placeholder board: what needs a person, and the groups); the others are the person's groups, each
//! drawn as squircle icons on one frosted island. Paging keys, dots, arrows and the pop-in motion live here; the geometry is
//! `launcher.rs`, the icon and island drawing is `iyakki_uikit::appicon`.

use super::*;
use crate::appicon::{self, AppState};
use crate::fx::with_alpha;

/// Heartbeats (33 ms each) the slide after a swipe lasts.
const SLIDE_TICKS: u64 = 7;

impl Shell {
    /// How many pages the home screen has for this person: the machine page and one per group.
    pub(super) fn home_pages(&self) -> usize {
        1 + self.groups().len()
    }

    /// The page being shown: 0 is the machine page, 1.. are the groups in chip order.
    pub(super) fn home_page(&self) -> usize {
        if self.machine {
            return 0;
        }
        let cur = self.current_group();
        1 + self.groups().iter().position(|g| *g == cur).unwrap_or(0)
    }

    /// Turn to home page `i` (ignored when there is no such page): the machine page, or a group (remembered for this person).
    pub(super) fn show_page(&mut self, i: usize) {
        if i >= self.home_pages() {
            return;
        }
        if i == 0 {
            if !self.machine {
                self.machine = true;
                self.sel = 0;
                self.anim_at = Some(self.tick);
                mark_stale!(self);
            }
            return;
        }
        let g = self.groups()[i - 1].clone();
        self.set_group(g);
        mark_stale!(self);
    }

    /// One page to the left or right (no wrapping); true when there was one.
    pub(super) fn step_page(&mut self, back: bool) -> bool {
        match launcher::page_step(self.home_page(), back, self.home_pages()) {
            Some(p) => {
                self.show_page(p);
                true
            }
            None => false,
        }
    }

    /// A sideways swipe (a touchpad, or a finger on a touch screen). It moves between the groups of Home and does nothing anywhere
    /// else: another screen, any dialog, menu or card, a focused text field, a lock, no sign-in, or the setting turned off.
    pub(super) fn swipe_event(&mut self, back: bool, at: Option<(i32, i32)>) {
        if !self.swipe_allowed(at) {
            return;
        }
        if self.step_page(back) {
            self.slide = Some((back, self.tick));
        }
    }

    /// Whether a swipe from `at` (a touch screen's start point; `None` for a touchpad) may turn the page now.
    pub(super) fn swipe_allowed(&self, at: Option<(i32, i32)>) -> bool {
        if !crate::input::home_swipe().0 || !matches!(self.screen, Screen::Launcher) || self.session.is_none() || self.lockui.active {
            return false;
        }
        if self.overlay_open() || self.approval_card_open() || self.ws_active() || self.vol.open || self.dock_focus.is_some() || self.fields.iter().any(|f| f.focused) {
            return false;
        }
        match at {
            None => true,
            // a finger: only one that began on the group area (the island) turns the page
            Some((x, y)) => {
                let (w, h) = self.size;
                let s = scale_for(w, h);
                let isl = if self.machine { launcher::machine_rect(w, h, s) } else { launcher::island_rect(w, h, s, self.group_items().len()) };
                isl.contains(x, y)
            }
        }
    }

    /// Sideways shift (pixels) of the home page while it slides in after a swipe: it starts a little off to the side the fingers came
    /// from and eases to rest in about a fifth of a second. Zero with reduced motion or when nothing slides.
    pub(super) fn slide_offset(&self, w: f32) -> f32 {
        let Some((back, t0)) = self.slide else { return 0.0 };
        let age = self.tick.saturating_sub(t0);
        if crate::fx::reduced_motion() || age >= SLIDE_TICKS {
            return 0.0;
        }
        let k = 1.0 - age as f32 / SLIDE_TICKS as f32;
        (if back { -1.0 } else { 1.0 }) * k * k * 0.12 * w
    }

    /// Milliseconds since the page last changed (very large when it did not: nothing animates).
    pub(super) fn anim_age_ms(&self) -> u64 {
        self.anim_at.map_or(u64::MAX / 2, |t| self.tick.saturating_sub(t) * 33)
    }

    /// True while the island's pop or the icons' stagger is still moving. Off with reduced motion, and only on the home screen.
    pub(super) fn home_animating(&self) -> bool {
        !crate::fx::reduced_motion() && matches!(self.screen, Screen::Launcher) && self.anim_age_ms() <= appicon::animation_ms(usize::MAX)
    }

    /// What the icon of `app` says besides its name: the state for the chip and badge, and a count for the badge. Locked and Setup (a
    /// missing camera) are the shell's own knowledge; everything else is what the app says about itself (`App::status`, F-LAUNCH-09).
    pub(super) fn app_state(&self, app: PApp, permitted: bool, missing: bool) -> (AppState, usize) {
        if !permitted {
            return (AppState::Locked, 0);
        }
        if missing {
            return (AppState::Setup, 0);
        }
        match self.status_of(app) {
            Some(st) => (
                match st.kind {
                    iyakki_core_api::StatusKind::Running => AppState::Running,
                    iyakki_core_api::StatusKind::Attention => AppState::Attention,
                    iyakki_core_api::StatusKind::Setup => AppState::Setup,
                },
                st.count,
            ),
            None => (AppState::Ready, 0),
        }
    }

    /// Draws the island and what is on it for the shown page, the page dots and the arrows. Registers the hit regions of the tiles.
    pub(super) fn draw_home_page(&self, g: &mut Gfx, s: f32, tiles: &[LItem], rects: &[R], first: usize) {
        let _p = iyakki_uikit::prof::scope("shell.draw_home_page");
        let (w, h) = (g.width(), g.height());
        let reduced = crate::fx::reduced_motion();
        let age = self.anim_age_ms();
        let mut isl = if self.machine { launcher::machine_rect(w, h, s) } else { launcher::island_rect(w, h, s, tiles.len()) };
        let slide = self.slide_offset(w as f32);
        let shifted: Vec<R>;
        let rects = if slide != 0.0 {
            isl.x += slide;
            shifted = rects.iter().map(|r| R::new(r.x + slide, r.y, r.w, r.h)).collect();
            &shifted[..]
        } else {
            rects
        };
        appicon::island(g, isl, s, appicon::island_pop(age, reduced));
        if self.machine {
            self.draw_machine_page(g, isl, s);
        } else {
            self.draw_icons(g, s, tiles, rects, first, age, reduced);
        }
        self.draw_page_controls(g, s);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_icons(&self, g: &mut Gfx, s: f32, tiles: &[LItem], rects: &[R], first: usize, age: u64, reduced: bool) {
        let Some(sess) = self.session.as_ref() else { return };
        let list = self.look.launcher.layout == iyakki_uikit::look::LauncherLayout::List;
        let px = look_impl::icon_px(self.look.icons.size);
        let cs = CellStyle { il: self.icon_look(), icon: if list { px.min(look_impl::LIST_ICON) } else { px }, labels: self.look.icons.labels, list };
        for (k, r) in rects.iter().enumerate() {
            let (i, item) = (first + k, &tiles[first + k]);
            let click = Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
            let selected = i == self.sel;
            let hover = g.hovered(*r);
            let grow = match appicon::pop_scale(age, k, reduced) {
                Some(v) => v,
                None => continue, // not its turn yet: the island is there, the icon is not
            };
            match item {
                LItem::Empty => {
                    layout_impl::draw_empty_hint(g, *r, s, &self.current_group().name);
                }
                LItem::App(e) => {
                    g.hit(*r, click, &e.title);
                    draw_custom_cell(g, e, *r, s, selected, tilehub_allowed(&sess.policy), grow, &cs);
                }
                LItem::Sys(t) => {
                    g.hit(*r, click, t.label); // a region for the keyboard walk; the click itself is resolved by position
                    let permitted = sess.policy.allows_app(t.app);
                    // CAMERA without a camera: a hint instead of the usual line
                    let missing = t.app == PApp::Camera && !self.camera.available() && !self.browser_camera_offered();
                    let allowed = permitted && !missing;
                    let (state, count) = self.app_state(t.app, permitted, missing);
                    let sub = if allowed { self.tile_sub(t.app, t.sub) } else if permitted { self.camera.tile_hint().to_string() } else { "Restricted".to_string() };
                    let sq = cell(g, *r, s, selected, hover, grow, t.accent, allowed, t.label, &sub, state, count, false, &cs);
                    appicon::glyph_with(g, t.icon, sq, s, cs.il.effective().style);
                }
            }
        }
    }

    /// The page dots and the arrows at the screen's edges.
    fn draw_page_controls(&self, g: &mut Gfx, s: f32) {
        let (w, h) = (g.width(), g.height());
        let total = self.home_pages();
        let cur = self.home_page();
        let names: Vec<String> = std::iter::once("The machine".to_string()).chain(self.groups().iter().map(|gr| gr.label().to_string())).collect();
        let dots = launcher::page_dots(w, h, s, total);
        // the dots sit on a small plate of the theme's own surface, so they read over any wallpaper
        if let (Some(a), Some(z)) = (dots.first(), dots.last()) {
            g.rrect(a.x + a.w * 0.1, a.y + a.h * 0.18, z.x + z.w - a.x - a.w * 0.2, a.h * 0.64, a.h * 0.32, with_alpha(CARD, 210));
        }
        for (p, r) in dots.iter().enumerate() {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let on = p == cur;
            let hot = g.hovered(*r);
            let rad = if on { 5.5 * s } else if hot { 5.0 * s } else { 4.2 * s };
            g.circle(cx, cy, rad, if on { TEXT } else { DIM });
            g.hit(*r, Event::MouseDown(cx as i32, cy as i32), &format!("go to {}", names.get(p).map_or("", |n| n.as_str())));
        }
        let (prev, next) = launcher::edge_chevrons(w, h, s);
        for (r, back) in [(prev, true), (next, false)] {
            if launcher::page_step(cur, back, total).is_none() {
                continue;
            }
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let hot = g.hovered(r);
            let d = r.w.min(r.h) * 0.62;
            g.circle(cx, cy, d / 2.0, with_alpha(if hot { CARD_HOVER } else { CARD }, if hot { 230 } else { 170 }));
            let ink = if hot { TEXT } else { MUTED };
            let p = launcher::chevron_points(r, back);
            g.line(p[0].0, p[0].1, p[1].0, p[1].1, 2.2 * s, ink);
            g.line(p[1].0, p[1].1, p[2].0, p[2].1, 2.2 * s, ink);
            g.hit(r, Event::MouseDown(cx as i32, cy as i32), if back { "show previous group" } else { "show next group" });
        }
    }

    /// What needs a person, named with the real problem: the apps' own words (`AppStatus::needs`, F-LAUNCH-09: approvals, overdue tasks,
    /// updates, the boot drive) and the inbox, which is not an app. Until an app says something, nothing is made up.
    pub(super) fn machine_attention(&self) -> Vec<(String, String, AppState)> {
        let mut v = Vec::new();
        let state = |k: iyakki_core_api::StatusKind| if k == iyakki_core_api::StatusKind::Setup { AppState::Setup } else { AppState::Attention };
        let mut from_apps = self.app_status.iter().filter_map(|(_, st)| st.needs.as_ref().map(|(t, d)| (t.clone(), d.clone(), state(st.kind)))).collect::<Vec<_>>();
        // the first row is the most urgent the core named; the inbox follows the approvals
        let rest = from_apps.split_off(from_apps.len().min(1));
        v.extend(from_apps);
        if self.badge.unread > 0 {
            v.push(("Inbox".to_string(), format!("{} unread that need you", self.badge.unread), AppState::Attention));
        }
        v.extend(rest);
        v
    }

    /// The apps in each group for this person, in chip order: (name, how many).
    pub(super) fn machine_groups(&self) -> Vec<(String, usize)> {
        let tiles = self.visible_tiles();
        self.groups()
            .iter()
            .map(|gr| {
                let n = tiles.iter().filter(|t| self.tile_gid(t) == gr.id).count() + self.custom.iter().filter(|e| self.app_gid(e) == gr.id).count();
                (gr.label().to_string(), n)
            })
            .collect()
    }

    /// Page 0 on the island: the machine's name and the person's widget board (`shell_board.rs`).
    fn draw_machine_page(&self, g: &mut Gfx, isl: R, s: f32) {
        let pad = 40.0 * s;
        let (x, y) = (isl.x + pad, isl.y + 30.0 * s);
        let inner = isl.w - 2.0 * pad;
        let title = g.fit(34.0 * s, "The machine", Face::Bold, inner);
        g.text(x, y, 34.0 * s, TEXT, &title, Face::Bold);
        let host = format!("{}  ·  {}", self.env.hostname, self.env.host.detect_env());
        let btn = self.board_button(g, isl, s);
        let host = g.fit(14.0 * s, &host, Face::Sans, (btn.x - x - 12.0 * s).max(40.0 * s));
        g.text(x, y + 44.0 * s, 14.0 * s, MUTED, &host, Face::Sans);
        self.draw_board(g, isl, s);
    }
}

/// A custom app's icon cell (an emoji-less letter or the app's own picture on the hue the name gives it). `grow` is the pop-in scale.
#[cfg(test)]
pub(super) fn draw_custom_tile(g: &mut Gfx, e: &iyakki_shared::store_tilehub::TileEntry, r: R, s: f32, selected: bool, allowed: bool, grow: f32) {
    draw_custom_cell(g, e, r, s, selected, allowed, grow, &CellStyle::default());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_custom_cell(g: &mut Gfx, e: &iyakki_shared::store_tilehub::TileEntry, r: R, s: f32, selected: bool, allowed: bool, grow: f32, st: &CellStyle) {
    let hover = g.hovered(r);
    let devlike = e.dev || e.is_developer();
    let sub = if !allowed { "Restricted".to_string() } else if e.dev { "unsigned dev app".to_string() } else if e.is_developer() { e.trust.clone() } else { format!("custom app · {}", e.group) };
    let state = if allowed { AppState::Ready } else { AppState::Locked };
    let sq = cell(g, r, s, selected, hover, grow, custom_accent(&e.name), allowed, &e.title, &sub, state, 0, devlike, st);
    if let Some(pic) = custom_icon_picture(e) {
        // a picture icon (a .png in the app): fitted inside the squircle
        let side = sq.w * 0.62;
        let kx = side / pic.width().max(pic.height()) as f32;
        let (ox, oy) = (sq.x + (sq.w - pic.width() as f32 * kx) / 2.0, sq.y + (sq.h - pic.height() as f32 * kx) / 2.0);
        let paint = tiny_skia::PixmapPaint { opacity: if allowed { 1.0 } else { 0.4 }, quality: tiny_skia::FilterQuality::Bilinear, ..tiny_skia::PixmapPaint::default() };
        g.pm.draw_pixmap(0, 0, pic.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(ox, oy).pre_scale(kx, kx), None);
    } else {
        let letter: String = e.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
        let ls = sq.h * 0.5;
        let lw = g.text_width(ls, &letter, Face::Bold);
        let ly = sq.y + (sq.h - g.line_height(ls, Face::Bold)) / 2.0;
        crate::theme::with_raw(|| g.text(sq.x + (sq.w - lw) / 2.0, ly, ls, appicon::glyph_colour(), &letter, Face::Bold));
    }
    if devlike {
        crate::fx::ribbon(g, r, s, "DEV");
    }
}

/// How a cell is drawn, from the Look: the icon's shape and style, its size at the base scale, whether labels show, and whether a cell is
/// a list row (icon at the left, text beside it).
#[derive(Clone, Copy)]
pub(super) struct CellStyle {
    pub il: appicon::IconLook,
    pub icon: f32,
    pub labels: bool,
    pub list: bool,
}

impl Default for CellStyle {
    fn default() -> CellStyle {
        CellStyle { il: appicon::IconLook::default(), icon: 106.0, labels: true, list: false }
    }
}

#[allow(clippy::too_many_arguments)]
fn cell(g: &mut Gfx, r: R, s: f32, selected: bool, hover: bool, grow: f32, accent: [u8; 4], allowed: bool, label: &str, sub: &str, state: AppState, count: usize, dev: bool, st: &CellStyle) -> R {
    let ic = (st.icon * s).min((r.w - 16.0 * s).max(36.0 * s)).min(if st.list { r.h - 12.0 * s } else { r.h * 0.58 });
    let text_h = if st.labels { 8.0 * s + 22.0 * s + 17.0 * s + 3.0 * s + 17.0 * s } else { 0.0 };
    let stack = ic + text_h;
    let top = r.y + ((r.h - stack) / 2.0).max(2.0 * s) - if selected && !st.list { 3.0 * s } else { 0.0 };
    if selected || hover {
        crate::access::note_selected(r);
        // the plate hugs the icon, its label and its chip (not the whole cell, which is much taller in a group with few rows)
        let (ph, pw) = if st.list { (r.h - 4.0 * s, r.w - 4.0 * s) } else { (stack + 20.0 * s, r.w - 4.0 * s) };
        let plate = if st.list { R::new(r.x + 2.0 * s, r.y + 2.0 * s, pw, ph) } else { R::new(r.x + (r.w - pw) / 2.0, top - 10.0 * s, pw, ph) };
        g.rrect(plate.x, plate.y, plate.w, plate.h, 26.0 * s, with_alpha(if selected { CARD_SEL } else { CARD_HOVER }, if selected { 170 } else { 110 }));
        if selected {
            g.rrect_outline(plate.x, plate.y, plate.w, plate.h, 26.0 * s, 2.5 * s * crate::theme::ring().max(1.0), crate::ui::ACCENT_MAIN);
        }
    }
    let style = st.il.effective().style;
    if st.list {
        // one row: the icon at the left, the name and the line beside it, the state chip at the right edge
        let sq = appicon::body_with(g, R::new(r.x + 14.0 * s, r.y + (r.h - ic) / 2.0, ic, ic), accent, allowed, grow, s, st.il);
        appicon::badge(g, sq, s, state, count);
        let tx = r.x + 14.0 * s + ic + 18.0 * s;
        let chip_w = if state.chip_text().is_some() { 120.0 * s } else { 0.0 };
        let fit = (r.x + r.w - tx - chip_w - 16.0 * s).max(40.0 * s);
        if st.labels {
            let mut ls = 18.0 * s;
            while ls > 10.0 * s && g.text_width(ls, label, Face::Bold) > fit {
                ls -= s * 0.5;
            }
            let lt = if g.text_width(ls, label, Face::Bold) > fit { g.fit(ls, label, Face::Bold, fit) } else { label.to_string() };
            let ty = r.y + (r.h - ls - 17.0 * s) / 2.0;
            g.text(tx, ty, ls, if allowed { TEXT } else { DIM }, &lt, Face::Bold);
            let mut ss = 13.0 * s;
            while ss > 8.0 * s && g.text_width(ss, sub, Face::Sans) > fit {
                ss -= s * 0.5;
            }
            let sb = if g.text_width(ss, sub, Face::Sans) > fit { g.fit(ss, sub, Face::Sans, fit) } else { sub.to_string() };
            g.text(tx, ty + ls + 4.0 * s, ss, if dev { AMBER_DEV } else { MUTED }, &sb, Face::Sans);
        }
        if chip_w > 0.0 {
            appicon::chip(g, r.x + r.w - 16.0 * s - chip_w / 2.0, r.y + (r.h - 17.0 * s) / 2.0, s, state, chip_w);
        }
        let _ = style;
        return sq;
    }
    let sq = appicon::body_with(g, R::new(r.x + (r.w - ic) / 2.0, top, ic, ic), accent, allowed, grow, s, st.il);
    appicon::badge(g, sq, s, state, count);
    if !st.labels {
        return sq;
    }
    let fit = r.w - 12.0 * s;
    let cx = r.x + r.w / 2.0;
    let mut ls = 17.0 * s;
    while ls > 10.0 * s && g.text_width(ls, label, Face::Bold) > fit {
        ls -= s * 0.5;
    }
    let label = if g.text_width(ls, label, Face::Bold) > fit { g.fit(ls, label, Face::Bold, fit) } else { label.to_string() };
    let lw = g.text_width(ls, &label, Face::Bold);
    let ly = top + ic + 8.0 * s;
    g.text(cx - lw / 2.0, ly, ls, if allowed { TEXT } else { DIM }, &label, Face::Bold);
    let mut ss = 12.5 * s;
    while ss > 8.0 * s && g.text_width(ss, sub, Face::Sans) > fit {
        ss -= s * 0.5;
    }
    let sub = if g.text_width(ss, sub, Face::Sans) > fit { g.fit(ss, sub, Face::Sans, fit) } else { sub.to_string() };
    let sw = g.text_width(ss, &sub, Face::Sans);
    let sy = ly + 22.0 * s;
    g.text(cx - sw / 2.0, sy, ss, if dev { AMBER_DEV } else { MUTED }, &sub, Face::Sans);
    appicon::chip(g, cx, sy + 17.0 * s + 3.0 * s, s, state, fit);
    sq
}

