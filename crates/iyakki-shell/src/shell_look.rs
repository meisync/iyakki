//! The shell reads the person's Look (`iyakki_uikit::look`): the launcher's icon shape, size and style, its layout (paged, grid or list,
//! columns, density), the dock of pinned apps, and (with the bars and the menu) the rest of the closed lists the Look may change.
//! Defaults are the launcher as it always was; the person's accessibility settings (larger text, reduced motion, High contrast) win.

use super::*;
use crate::appicon::{self, IconLook};
use crate::fx::with_alpha;
use iyakki_uikit::look::{self, DockPos, Density, IconSize, LauncherLayout, Look};
use iyakki_uikit::look_registry;

/// The apps a role pins by default (in this order), when the person has not pinned their own and the Look names none.
const DEFAULT_DOCK: [PApp; 6] = [PApp::Chat, PApp::Browser, PApp::Ide, PApp::Edit, PApp::Cli, PApp::System];

/// The corner handle of a hidden top bar.
pub(crate) fn bar_handle(w: u32, s: f32) -> R {
    let d = 40.0 * s;
    R::new(w as f32 - 40.0 * s - d, 18.0 * s, d, d)
}

/// The wallpaper shipped with IYAKKI (F-LAUNCH-22): a dusk over four ridges drawn from numbers by dist/brand/make-wallpaper.py, our own
/// drawing under the repository's licence, 28 KB. It is drawn when `wallpaper = picture` is chosen and the person has put no picture of
/// their own in their themes folder; a file of their own always wins.
const SHIPPED_WALLPAPER: &[u8] = include_bytes!("../../../dist/brand/wallpaper-default.png");

/// The shipped picture, decoded.
pub(crate) fn shipped_wallpaper_picture() -> Result<(u32, u32, Vec<u8>), String> {
    let b = iyakki_web::png::decode(SHIPPED_WALLPAPER)?;
    Ok((b.width, b.height, b.rgba))
}

/// The biggest wallpaper picture file read.
pub(crate) const PICTURE_FILE_MAX: u64 = 12 << 20;

/// Find and decode the person's wallpaper picture: `<data>/themes/<person>/wallpaper.{png,jpg,jpeg}`, else `<data>/themes/wallpaper.*`.
/// PNG by the in-house reader, JPEG by the browser's decoder. Never more than [`PICTURE_FILE_MAX`] bytes are read.
pub(crate) fn read_wallpaper_picture(data: &std::path::Path, user: &str) -> Result<(u32, u32, Vec<u8>), String> {
    use std::io::Read;
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let dirs = [data.join("themes").join(who), data.join("themes")];
    for d in dirs {
        for name in ["wallpaper.png", "wallpaper.jpg", "wallpaper.jpeg"] {
            let p = d.join(name);
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if !meta.is_file() {
                return Err(format!("{name} is not a plain file"));
            }
            if meta.len() > PICTURE_FILE_MAX {
                return Err(format!("{name} is larger than {} MB", PICTURE_FILE_MAX >> 20));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&p).and_then(|f| f.take(PICTURE_FILE_MAX + 1).read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
            return if name.ends_with(".png") {
                let b = iyakki_web::png::decode(&bytes)?;
                Ok((b.width, b.height, b.rgba))
            } else {
                let b = iyakki_web::image::decode(&bytes)?;
                Ok((b.width, b.height, b.rgba))
            };
        }
    }
    Err("there is no wallpaper.png or wallpaper.jpg in the themes folder".into())
}

/// The ids of the default dock set (a test keeps them equal to `look::DEFAULT_DOCK_IDS`, which the chat tools use).
#[cfg(test)]
pub(crate) fn default_dock_ids() -> Vec<&'static str> {
    DEFAULT_DOCK.iter().map(|a| a.id()).collect()
}

#[cfg(test)]
#[test]
fn the_default_dock_is_the_one_the_sdk_dock_port_starts_from() {
    let ids: Vec<&str> = DEFAULT_DOCK.iter().map(|a| a.id()).collect();
    assert_eq!(ids, look_registry::DEFAULT_DOCK_IDS.to_vec());
}

/// A list row's icon is at most this big (rows stay low enough for a screenful).
pub(super) const LIST_ICON: f32 = 64.0;

/// The pixel size of an icon body at the base scale (the 108 px of the design at 1080p).
pub(super) fn icon_px(s: IconSize) -> f32 {
    match s {
        IconSize::S => 80.0,
        IconSize::M => 106.0,
        IconSize::L => 132.0,
        IconSize::Px(p) => p as f32,
    }
}

/// The launcher geometry a Look asks for (pure: tested without a shell). `dock_apps` is how many pinned apps the dock shows (0: no dock).
pub(crate) fn params_for(look: &Look, dock_apps: usize) -> launcher::Params {
    let mut p = launcher::Params::default();
    let icon = icon_px(look.icons.size);
    match look.launcher.density {
        Density::Compact => {
            p.gap = 16.0;
            p.min_tile = 120.0;
            p.ratio = 1.24;
        }
        Density::Comfortable => {}
        Density::Roomy => {
            p.gap = 40.0;
            p.min_tile = 160.0;
            p.ratio = 1.4;
            p.max_tile = 240.0;
        }
    }
    // a cell is never narrower than its icon and some air
    p.min_tile = p.min_tile.max(icon + 28.0);
    p.max_tile = p.max_tile.max(p.min_tile);
    if let look::Columns::Fixed(c) = look.launcher.columns {
        p.fixed_cols = Some(c as usize);
        p.max_cols = p.max_cols.max(c as usize);
    }
    if look.launcher.layout == LauncherLayout::List {
        // one row per app: icon, name and line side by side
        p.fixed_cols = Some(1);
        p.max_cols = 1;
        p.min_tile = 380.0;
        p.max_tile = 760.0;
        p.ratio = (icon.min(LIST_ICON) + 24.0) / 380.0;
        p.gap = 10.0;
    }
    if dock_apps > 0 && !look.dock.autohide {
        // a dock that hides itself gives the room back to the tiles
        let d = look.dock.size as f32;
        match look.dock.position {
            DockPos::Bottom => p.inset_b = d + 32.0,
            DockPos::Left => p.inset_l = d + 48.0,
            DockPos::Right => p.inset_r = d + 48.0,
            DockPos::Off => {}
        }
    }
    p
}

/// The colours a Look file wrote of its own (anything that differs from its base theme), as a theme the drawing code can hold; `None` when the
/// Look has only its base theme (or when too many different themes were loaded in this run: the base theme is used then).
pub(crate) fn custom_colours(look: &Look) -> Option<&'static iyakki_uikit::theme::Theme> {
    if format!("{:?}", look.theme) == format!("{:?}", look.theme.id.theme()) {
        return None;
    }
    iyakki_uikit::usertheme::leak(&iyakki_uikit::usertheme::UserTheme { name: look.name.clone(), theme: look.theme })
}

impl Shell {
    /// Read this person's Look at sign-in: the chosen one when it is still there and readable, else the default. A chosen Look also brings
    /// its wallpaper and its theme base, unless the person uses High contrast (accessibility wins).
    pub(super) fn load_look(&mut self, user: &str) {
        let dir = self.env.data_dir().to_path_buf();
        let chosen = look_registry::load_choice(&dir, user).is_some();
        self.look = look_registry::current(&dir, user).look;
        // the dock position this person kept for the main screen (through the core, F-SDK-30) wins over the Look's own
        if let Some(p) = self.core.desktop().screen_dock(user, 0) {
            self.look.dock.position = match p.as_str() {
                "left" => DockPos::Left,
                "right" => DockPos::Right,
                "off" => DockPos::Off,
                _ => DockPos::Bottom,
            };
        }
        self.pinned = look_registry::load_dock(&dir, user);
        if chosen && crate::theme::load(&dir, user) != crate::theme::ThemeId::HighContrast {
            iyakki_uikit::wallpaper::set_current(self.look.wallpaper);
            // back to what the person saved (base theme, or their theme file), then the Look's own colours on top when it carries any
            crate::theme::apply_for(&dir, user);
            match custom_colours(&self.look) {
                Some(t) => crate::theme::set_custom(Some(t)),
                None if !crate::theme::is_custom() => crate::theme::set_current(self.look.theme.id),
                None => {}
            }
        }
        self.load_board(user);
        self.sync_look();
        self.load_wallpaper_picture(user);
    }

    /// Hand the picture wallpaper (`wallpaper.png`, `.jpg` or `.jpeg` in the person's theme folder, else the shared one) to the toolkit when
    /// the wallpaper in force is `picture`; any other wallpaper lets go of it. A file that is too big, not readable or not a picture is
    /// ignored (the theme's own background is drawn) and the access log says so.
    pub(super) fn load_wallpaper_picture(&mut self, user: &str) {
        use iyakki_uikit::wallpaper;
        if wallpaper::current() != wallpaper::Id::Picture {
            if wallpaper::has_picture() {
                wallpaper::set_picture(None);
            }
            return;
        }
        let dir = self.env.data_dir().to_path_buf();
        // a file of the person's own wins; only when there is none the shipped picture is drawn (a file that is there but bad is not hidden
        // by it: the theme background is drawn and the log says why)
        let found = match read_wallpaper_picture(&dir, user) {
            Err(e) if e.starts_with("there is no wallpaper") => shipped_wallpaper_picture(),
            other => other,
        };
        match found {
            Ok(p) => {
                wallpaper::set_picture(Some(p));
            }
            Err(e) => {
                wallpaper::set_picture(None);
                self.env.audit.record(user, "settings.display.wallpaper_picture", &format!("not used: {e}"));
            }
        }
    }

    /// Back to the default Look (sign-out: the sign-in screen is always the default Dark look).
    pub(super) fn reset_look(&mut self) {
        self.look = Look::default();
        self.pinned = None;
        self.bar_peek = false;
        self.dock_focus = None;
        self.reset_board();
        launcher::reset_params();
    }

    /// Hand the launcher geometry what the Look asks for. Called whenever the Look or the pinned apps change, and before a frame.
    pub(super) fn sync_look(&self) {
        let n = self.dock_apps().len();
        launcher::set_params(params_for(&self.look, n));
    }

    /// Whether the top bar's row of buttons is drawn and live: the Look has it on, or the person asked to see it once (`b`, or the corner
    /// handle) while the Look has it off. The essentials stay reachable either way: `o` signs out, `p` is power, `s` Settings, and the handle
    /// and `b` bring the bar back for the bell and the rest.
    pub(super) fn bar_visible(&self) -> bool {
        self.look.topbar.on || self.bar_peek
    }

    /// The corner handle that shows a hidden bar: a small round button at the top right, a region for a click and the key `b`.
    pub(super) fn draw_bar_handle(&self, g: &mut Gfx, s: f32) {
        let r = bar_handle(g.width(), s);
        let hover = g.hovered(r);
        crate::fx::chrome_button(g, r, s * 2.5, hover, crate::fx::CYAN);
        for dx in [-6.0f32, 0.0, 6.0] {
            g.circle(r.x + r.w / 2.0 + dx * s, r.y + r.h / 2.0, 1.8 * s, MUTED);
        }
        g.hit(r, Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32), "show the bar");
    }

    /// `b` shows a hidden bar (or hides it again) and Esc puts it away; only while the Look has the bar off, on the home screen.
    pub(super) fn bar_key(&mut self, k: &Key) -> bool {
        if self.look.topbar.on || self.session.is_none() || !matches!(self.screen, Screen::Launcher) || self.panes.compact {
            return false;
        }
        match k {
            Key::Char('b') | Key::Char('B') => {
                self.bar_peek = !self.bar_peek;
                mark_stale!(self);
                true
            }
            Key::Esc if self.bar_peek => {
                self.bar_peek = false;
                mark_stale!(self);
                true
            }
            _ => false,
        }
    }

    /// Whether the screen has room for the dock beside the tiles: a small window or the largest text size gives the room back to the
    /// launcher (the dock is a convenience; the home screen must stay usable).
    pub(super) fn dock_fits(&self) -> bool {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let d = launcher::dock_scale(s);
        let size = self.look.dock.size as f32;
        match self.look.dock.position {
            DockPos::Bottom => h as f32 >= 560.0 * s.max(1.0) + (size + 32.0) * d,
            DockPos::Left | DockPos::Right => w as f32 >= 760.0 * d.max(1.0),
            DockPos::Off => false,
        }
    }

    /// The apps in the dock, in order: the person's pinned list, else the Look's, else the default set; only apps this person may use and
    /// that are on the home screen (role gating and Cloud mode apply), no repeats, at most 10.
    pub(super) fn dock_apps(&self) -> Vec<PApp> {
        if self.look.dock.position == DockPos::Off || !self.dock_fits() {
            return vec![];
        }
        let Some(sess) = self.session.as_ref() else { return vec![] };
        let saved = self.pinned.clone();
        let ids: Vec<String> = match saved {
            Some(v) => v,
            None if !self.look.dock.items.is_empty() => self.look.dock.items.clone(),
            None => DEFAULT_DOCK.iter().map(|a| a.id().to_string()).collect(),
        };
        let shown = self.visible_tiles();
        let mut out: Vec<PApp> = Vec::new();
        for id in ids {
            if let Some(a) = PApp::ALL.into_iter().find(|a| a.id() == id) {
                if !out.contains(&a) && sess.policy.allows_app(a) && shown.iter().any(|t| t.app == a) {
                    out.push(a);
                }
            }
        }
        out.truncate(10);
        out
    }

    /// Keep a new pinned list for this person (the order is the dock's order) and lay the launcher out again.
    pub(super) fn set_dock(&mut self, apps: &[PApp]) {
        if let Some(sess) = self.session.clone() {
            let ids: Vec<String> = apps.iter().map(|a| a.id().to_string()).collect();
            look_registry::save_dock(self.env.data_dir(), &sess.user, &ids);
            self.pinned = Some(ids.clone());
            self.env.audit.record(&sess.user, "settings.display.dock", &ids.join(","));
        }
        self.sync_look();
        mark_stale!(self);
    }

    /// Where the dock and its icons are at rest (no magnify), whether or not an autohiding dock is out (None: no dock).
    pub(super) fn dock_resting(&self, w: u32, h: u32, s: f32) -> Option<(R, Vec<R>)> {
        let n = self.dock_apps().len();
        (n > 0).then(|| launcher::dock_rects(w, h, s, n, self.look.dock.size, self.look.dock.position))
    }

    /// Whether the dock is on screen now: always, unless the Look hides it (autohide) and the pointer and the keyboard are elsewhere.
    pub(super) fn dock_out(&self, w: u32, h: u32, s: f32) -> bool {
        let Some((plate, _)) = self.dock_resting(w, h, s) else { return false };
        let strip = launcher::dock_hot_strip(w, h, s, self.look.dock.position);
        launcher::dock_revealed(self.look.dock.autohide, strip, plate, self.mouse, self.dock_focus.is_some(), self.dock_press.is_some())
    }

    /// Where the dock and its icons are on this screen (None: no dock, or an autohiding one that is tucked away). With magnify on and the
    /// pointer over the dock the icons nearest it are bigger.
    pub(super) fn dock_layout(&self, w: u32, h: u32, s: f32) -> Option<(R, Vec<R>)> {
        let (plate, items) = self.dock_resting(w, h, s)?;
        if !self.dock_out(w, h, s) {
            return None;
        }
        let pos = self.look.dock.position;
        let long = if matches!(pos, DockPos::Left | DockPos::Right) { h } else { w } as f32;
        let items = launcher::magnify_rects(&items, plate, self.mouse, self.look.dock.magnify && !crate::fx::reduced_motion(), pos, long);
        // the plate grows with the row, so the big icons never hang over its edge
        let plate = launcher::dock_plate_around(plate, &items, pos, 12.0 * launcher::dock_scale(s));
        Some((plate, items))
    }

    /// True when a pointer move from `a` to `b` changes the dock's picture: it magnifies under the pointer, or a tucked-away dock is woken.
    pub(super) fn dock_pointer_changes(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        if !self.look.dock.magnify && !self.look.dock.autohide {
            return false;
        }
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let Some((plate, _)) = self.dock_resting(w, h, s) else { return false };
        let strip = launcher::dock_hot_strip(w, h, s, self.look.dock.position);
        let in_zone = |p: (i32, i32)| plate.contains(p.0, p.1) || (self.look.dock.autohide && strip.contains(p.0, p.1));
        (in_zone(a) || in_zone(b)) && a != b
    }

    pub(super) fn app_running(&self, a: PApp) -> bool {
        matches!(&self.screen, Screen::App(_, x) if *x == a) || self.stash.as_ref().is_some_and(|(_, x)| *x == a)
    }

    /// Draws the dock over the launcher: a plate of the theme's surface with the pinned apps' icons, a dot under the running ones, and the
    /// regions for a click.
    pub(super) fn draw_dock(&self, g: &mut Gfx, s: f32) {
        let _p = iyakki_uikit::prof::scope("shell.draw_dock");
        let (w, h) = (g.width(), g.height());
        let Some((plate, items)) = self.dock_layout(w, h, s) else { return };
        let apps = self.dock_apps();
        let il = self.icon_look();
        g.rrect(plate.x, plate.y + 3.0 * s, plate.w, plate.h, 22.0 * s, [0, 0, 0, 40]);
        g.rrect(plate.x, plate.y, plate.w, plate.h, 22.0 * s, with_alpha(CARD, appicon::island_alpha()));
        g.rrect_gradient(plate.x + 1.0, plate.y + 1.0, plate.w - 2.0, plate.h * 0.45, 21.0 * s, &[(0.0, [255, 255, 255, 30]), (1.0, [255, 255, 255, 0])]);
        g.rrect_outline(plate.x, plate.y, plate.w, plate.h, 22.0 * s, 1.2 * s, if crate::theme::is_high_contrast() { BORDER } else { [255, 255, 255, 90] });
        let reduced = crate::fx::reduced_motion();
        for (i, (a, r)) in apps.iter().zip(items.iter()).enumerate() {
            let Some(t) = TILES.iter().find(|t| t.app == *a) else { continue };
            let lift = if self.dock_focus == Some(i) || g.hovered(*r) { 4.0 * s } else { 0.0 };
            let sq = R::new(r.x, r.y - lift, r.w, r.w);
            if self.dock_focus == Some(i) {
                crate::access::note_selected(*r);
                g.rrect_outline(r.x - 5.0 * s, r.y - 5.0 * s - lift, r.w + 10.0 * s, r.w + 10.0 * s, 14.0 * s, 2.4 * s * crate::theme::ring().max(1.0), crate::ui::ACCENT_MAIN);
            }
            let drag = self.dock_press.is_some_and(|(p, _)| p == i) && !reduced && self.dock_moved;
            let body = appicon::body_with(g, sq, t.accent, true, if drag { 1.08 } else { 1.0 }, s, il);
            appicon::glyph_with(g, t.icon, body, s, il.effective().style);
            if self.app_running(*a) && self.look.dock.running {
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.w + 8.0 * s);
                g.circle(cx, cy, 2.6 * s, TEXT);
            }
            g.hit(*r, Event::MouseDown((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32), &format!("dock {}", t.label));
        }
    }

    /// The icon shape and style the launcher draws with: the Look's, with accessibility overriding it.
    pub(super) fn icon_look(&self) -> IconLook {
        IconLook { shape: self.look.icons.shape, style: self.look.icons.style }.effective()
    }

    /// The dock item under a point.
    pub(super) fn dock_hit(&self, x: i32, y: i32) -> Option<usize> {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let (_, items) = self.dock_layout(w, h, s)?;
        items.iter().position(|r| R::new(r.x - 6.0 * s, r.y - 6.0 * s, r.w + 12.0 * s, r.h + 12.0 * s).contains(x, y))
    }

    /// Dock clicks and drags on the home screen: a press and release on one icon opens the app; dragging an icon over another moves it
    /// there. Returns true when the event was the dock's.
    pub(super) fn dock_pointer(&mut self, ev: &Event) -> bool {
        match ev {
            Event::MouseDown(x, y) => match self.dock_hit(*x, *y) {
                Some(i) => {
                    self.dock_press = Some((i, (*x, *y)));
                    self.dock_moved = false;
                    true
                }
                None => false,
            },
            Event::MouseMove(x, y) => {
                if let Some((_, (px, py))) = self.dock_press {
                    if (x - px).abs() + (y - py).abs() > 8 && !self.dock_moved {
                        self.dock_moved = true;
                        mark_stale!(self);
                    }
                    return true;
                }
                false
            }
            Event::MouseUp(x, y) => {
                let Some((from, _)) = self.dock_press.take() else { return false };
                let moved = std::mem::take(&mut self.dock_moved);
                let mut apps = self.dock_apps();
                let to = self.dock_hit(*x, *y);
                match to {
                    Some(t) if t != from && moved && from < apps.len() => {
                        let a = apps.remove(from);
                        apps.insert(t.min(apps.len()), a);
                        self.set_dock(&apps);
                    }
                    Some(t) if t == from && from < apps.len() => self.open(apps[from]),
                    _ => {}
                }
                mark_stale!(self);
                true
            }
            _ => false,
        }
    }

    /// Keyboard on the dock: `d` puts the focus on it (or takes it off); then Left and Right move along, `,` and `.` move the icon, `x` unpins,
    /// Enter opens, Esc leaves. Returns true when the key was the dock's.
    pub(super) fn dock_key(&mut self, k: &Key) -> bool {
        let n = self.dock_apps().len();
        if n == 0 {
            self.dock_focus = None;
            return false;
        }
        let Some(f) = self.dock_focus else {
            if matches!(k, Key::Char('d')) {
                self.dock_focus = Some(0);
                mark_stale!(self);
                return true;
            }
            return false;
        };
        let mut apps = self.dock_apps();
        match k {
            Key::Left => self.dock_focus = Some(f.saturating_sub(1)),
            Key::Right => self.dock_focus = Some((f + 1).min(n - 1)),
            Key::Char(',') if f > 0 => {
                apps.swap(f, f - 1);
                self.dock_focus = Some(f - 1);
                self.set_dock(&apps);
            }
            Key::Char('.') if f + 1 < n => {
                apps.swap(f, f + 1);
                self.dock_focus = Some(f + 1);
                self.set_dock(&apps);
            }
            Key::Char('x') => {
                apps.remove(f);
                self.dock_focus = if apps.is_empty() { None } else { Some(f.min(apps.len() - 1)) };
                self.set_dock(&apps);
            }
            Key::Enter => {
                let a = apps[f];
                self.dock_focus = None;
                self.open(a);
            }
            Key::Esc | Key::Char('d') | Key::Up => self.dock_focus = None,
            _ => return true, // the dock has the keyboard: other keys do nothing
        }
        mark_stale!(self);
        true
    }
}
