//! Settings -> Appearance (docs/development/LOOK-DESIGN.md): the person's Look with a live preview, the wallpaper as buttons, the icon style
//! and shape, the launcher layout, the dock and the pinned apps, and which optional bar items show. Everything here is the person's own.
//!
//! Every change edits the Look in force and keeps it as the person's own file (`my-look.look`, `look_registry::save_custom`), checked first;
//! the shell reads it back the moment it is told (`look_registry::notify_changed`). The essential controls (the bell, "something needs
//! you", Settings, the account and power) cannot be switched off here or in a file: the validator puts them back. Accessibility
//! (larger text, reduced motion, High contrast) overrules a Look, and the sign-in screen is always the default Dark look.

use super::*;
use iyakki_uikit::look::{self, BarItem, Columns, Density, DockPos, IconShape, IconSize, IconStyle, LauncherLayout, Look};
use iyakki_uikit::look_registry;
use iyakki_uikit::wallpaper as wp;

/// The name of the Look this page keeps the person's own edits in.
const MINE: &str = "My look";
/// The optional bar items this page switches (the others are essential or have no switch yet), with their keys.
const BAR_KEYS: [(char, BarItem, &str); 5] = [('k', BarItem::Clock, "Clock"), ('g', BarItem::Battery, "Battery"), ('n', BarItem::Network, "Network"), ('v', BarItem::Volume, "Volume"), ('j', BarItem::Apps, "Apps button")];

fn next<T: Copy + PartialEq>(all: &[T], cur: T, back: bool) -> T {
    let at = all.iter().position(|x| *x == cur).unwrap_or(0);
    all[if back { (at + all.len() - 1) % all.len() } else { (at + 1) % all.len() }]
}

/// The columns the page cycles through: the screen decides, then 3 to 8.
fn column_steps() -> Vec<Columns> {
    std::iter::once(Columns::Auto).chain(Columns::RANGE.map(Columns::Fixed)).collect()
}

impl SettingsApp {
    /// The Look in force for this person.
    pub(super) fn ap_look(cx: &AppCx) -> Look {
        look_registry::current(cx.data_dir(), &cx.session.user).look
    }

    /// Change the Look in force with `f`, keep it as the person's own Look and tell the shell. `what` goes to the access log.
    pub(super) fn ap_edit(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut Look)) -> Option<Look> {
        let mut l = Self::ap_look(cx);
        f(&mut l);
        l.name = MINE.into();
        match look_registry::save_custom(cx.data_dir(), &cx.session.user, &l) {
            Ok(a) => {
                cx.audit.record(&cx.session.user, "settings.display.look", what);
                look_registry::notify_changed();
                Some(a.look)
            }
            Err(e) => {
                self.fail(look::problems_text(&e));
                None
            }
        }
    }

    /// The Looks this person can choose that pass the check, in list order.
    fn ap_choices(cx: &AppCx) -> Vec<(String, String)> {
        look_registry::available(cx.data_dir(), &cx.session.user).into_iter().filter_map(|e| e.result.ok().map(|a| (e.key, a.look.name))).collect()
    }

    /// Line height (`l`), letter spacing (`m`) or tabular figures (`u`): kept for this person, applied at once, logged.
    fn ap_type(&mut self, cx: &AppCx, c: char) {
        use iyakki_uikit::typeset;
        let back = c.is_uppercase();
        let mut p = typeset::current();
        match c.to_ascii_lowercase() {
            'l' => p.line = next(&typeset::LINE_ALL, p.line, back),
            'm' => p.tracking = next(&typeset::TRACKING_ALL, p.tracking, back),
            _ => p.tabular = !p.tabular,
        }
        typeset::save(cx.data_dir(), &cx.session.user, p);
        typeset::set_current(p);
        cx.audit.record(&cx.session.user, "settings.display.type", &typeset::describe(p));
        self.ok(format!("Type: {}.", typeset::describe(p)));
    }

    /// Choose the next (or, with a capital, the previous) text font (`f`) or code font (`p`): kept for this person, applied at once, logged.
    fn ap_font(&mut self, cx: &AppCx, c: char) {
        use iyakki_uikit::fonts;
        let back = c.is_uppercase();
        let (mut ui, mut code) = (fonts::current_ui(), fonts::current_code());
        if c.eq_ignore_ascii_case(&'f') {
            ui = next(&fonts::UI_ALL, ui, back);
        } else {
            code = next(&fonts::CODE_ALL, code, back);
        }
        fonts::save_choice(cx.data_dir(), &cx.session.user, ui, code);
        fonts::set_ui(ui);
        fonts::set_code(code);
        cx.audit.record(&cx.session.user, "settings.display.font", &format!("{} {}", ui.key(), code.key()));
        self.ok(format!("Fonts: {}.", fonts::describe()));
    }

    pub(super) fn handle_appearance(&mut self, k: &Key, cx: &AppCx) {
        let Key::Char(c) = k else { return };
        let c = *c;
        match c {
            '[' | ']' => {
                let list = Self::ap_choices(cx);
                let cur = look_registry::load_choice(cx.data_dir(), &cx.session.user).unwrap_or_else(|| "default".into());
                let at = list.iter().position(|(k, _)| *k == cur).unwrap_or(0);
                if list.is_empty() {
                    return;
                }
                let (key, name) = list[if c == ']' { (at + 1) % list.len() } else { (at + list.len() - 1) % list.len() }].clone();
                match look_registry::choose(cx.data_dir(), &cx.session.user, &key) {
                    Ok(a) => {
                        wp::save(cx.data_dir(), &cx.session.user, a.look.wallpaper);
                        wp::set_current(a.look.wallpaper);
                        cx.audit.record(&cx.session.user, "settings.display.look", &format!("choose {key}"));
                        look_registry::notify_changed();
                        self.ok(format!("Look: {name}."));
                    }
                    Err(e) => self.fail(look::problems_text(&e)),
                }
            }
            '1'..='5' | 'w' | 'W' => {
                let all = wp::ALL;
                let id = match c {
                    'w' => next(&all, wp::current(), false),
                    'W' => next(&all, wp::current(), true),
                    d => all[d as usize - '1' as usize],
                };
                wp::save(cx.data_dir(), &cx.session.user, id);
                wp::set_current(id);
                if self.ap_edit(cx, &format!("wallpaper {}", id.key()), |l| l.wallpaper = id).is_some() {
                    self.ok(format!("Wallpaper: {}.", id.name()));
                }
            }
            'f' | 'F' | 'p' | 'P' => self.ap_font(cx, c),
            'l' | 'L' | 'm' | 'M' | 'u' => self.ap_type(cx, c),
            's' | 'S' => {
                let st = next(IconStyle::ALL, Self::ap_look(cx).icons.style, c == 'S');
                if self.ap_edit(cx, &format!("icon style {}", st.key()), |l| l.icons.style = st).is_some() {
                    self.ok(format!("Icon style: {}.", st.key()));
                }
            }
            'h' | 'H' => {
                let sh = next(IconShape::ALL, Self::ap_look(cx).icons.shape, c == 'H');
                if self.ap_edit(cx, &format!("icon shape {}", sh.key()), |l| l.icons.shape = sh).is_some() {
                    self.ok(format!("Icon shape: {}.", sh.key()));
                }
            }
            'z' | 'Z' => {
                let sz = next(&[IconSize::S, IconSize::M, IconSize::L], match Self::ap_look(cx).icons.size { IconSize::Px(_) => IconSize::M, o => o }, c == 'Z');
                if self.ap_edit(cx, &format!("icon size {}", sz.text()), |l| l.icons.size = sz).is_some() {
                    self.ok(format!("Icon size: {}.", sz.text()));
                }
            }
            'i' => {
                if let Some(l) = self.ap_edit(cx, "icon labels", |l| l.icons.labels = !l.icons.labels) {
                    self.ok(format!("Icon names {}.", if l.icons.labels { "shown" } else { "hidden" }));
                }
            }
            'y' | 'Y' => {
                let ly = next(LauncherLayout::ALL, Self::ap_look(cx).launcher.layout, c == 'Y');
                if self.ap_edit(cx, &format!("launcher {}", ly.key()), |l| l.launcher.layout = ly).is_some() {
                    self.ok(format!("Launcher layout: {}.", ly.key()));
                }
            }
            'c' | 'C' => {
                let steps = column_steps();
                let cl = next(&steps, Self::ap_look(cx).launcher.columns, c == 'C');
                if self.ap_edit(cx, "columns", |l| l.launcher.columns = cl).is_some() {
                    self.ok(format!("Columns: {}.", Self::ap_columns_text(cl)));
                }
            }
            'e' | 'E' => {
                let d = next(Density::ALL, Self::ap_look(cx).launcher.density, c == 'E');
                if self.ap_edit(cx, &format!("density {}", d.key()), |l| l.launcher.density = d).is_some() {
                    self.ok(format!("Spacing: {}.", d.key()));
                }
            }
            'd' | 'D' => {
                let p = next(DockPos::ALL, Self::ap_look(cx).dock.position, c == 'D');
                if self.ap_edit(cx, &format!("dock {}", p.key()), |l| l.dock.position = p).is_some() {
                    self.ok(format!("Dock: {}.", p.key()));
                }
            }
            'r' => {
                if let Some(l) = self.ap_edit(cx, "dock running dots", |l| l.dock.running = !l.dock.running) {
                    self.ok(format!("A dot under running apps: {}.", if l.dock.running { "on" } else { "off" }));
                }
            }
            'a' => self.ap_pin_next(cx),
            'x' => self.ap_unpin_last(cx),
            't' | 'T' => {
                // EDIT (t) and CLI (T): the version written on the UI SDK or the usual screen, the person's own choice (experimental)
                let app = if c == 't' { "edit" } else { "cli" };
                let on = !crate::apps::sdk_apps::chosen(cx, app);
                match crate::apps::sdk_apps::set_chosen(&*cx.core, &cx.session.user, app, on) {
                    Ok(said) => {
                        cx.audit.record(&cx.session.user, "settings.display.sdk_apps", &format!("{app} {}", if on { "on" } else { "off" }));
                        self.ok(format!("{said} It applies the next time the app is opened."));
                    }
                    Err(e) => self.fail(e),
                }
            }
            'o' => {
                iyakki_uikit::look_registry::save_dock(cx.data_dir(), &cx.session.user, &[]);
                iyakki_uikit::look_registry::clear_dock(cx.data_dir(), &cx.session.user);
                cx.audit.record(&cx.session.user, "settings.display.dock", "default");
                look_registry::notify_changed();
                self.ok("Dock: the usual apps again.");
            }
            c if BAR_KEYS.iter().any(|(k, _, _)| *k == c) => {
                let (_, item, name) = BAR_KEYS.iter().find(|(k, _, _)| *k == c).copied().unwrap_or(BAR_KEYS[0]);
                if let Some(l) = self.ap_edit(cx, &format!("top bar {}", item.key()), |l| {
                    if l.topbar.items.contains(&item) {
                        l.topbar.items.retain(|i| *i != item);
                    } else {
                        // back where the default order has it
                        let order = Look::default().topbar.items;
                        let at = order.iter().position(|i| *i == item).unwrap_or(0);
                        let pos = l.topbar.items.iter().position(|i| order.iter().position(|o| o == i).unwrap_or(usize::MAX) > at).unwrap_or(l.topbar.items.len());
                        l.topbar.items.insert(pos, item);
                    }
                }) {
                    self.ok(format!("{name} in the top bar: {}.", if l.topbar.items.contains(&item) { "shown" } else { "hidden" }));
                }
            }
            _ => {}
        }
    }

    fn ap_columns_text(c: Columns) -> String {
        match c {
            Columns::Auto => "automatic".into(),
            Columns::Fixed(n) => n.to_string(),
        }
    }

    /// The apps this person may pin (on the home screen for them), in tile order: (id, label).
    pub(super) fn ap_pinnable(cx: &AppCx) -> Vec<(String, String)> {
        crate::apps::pinnable(&cx.session.policy).into_iter().map(|(a, l)| (a.to_string(), l.to_string())).collect()
    }

    /// The ids in the dock now: the person's own list, else the Look's, else the shell's default set.
    pub(super) fn ap_pinned(cx: &AppCx) -> Vec<String> {
        look_registry::load_dock(cx.data_dir(), &cx.session.user).or_else(|| Some(Self::ap_look(cx).dock.items).filter(|v| !v.is_empty())).unwrap_or_else(|| ["chat", "browser", "ide", "edit", "cli", "system"].iter().map(|s| s.to_string()).collect())
    }

    fn ap_pin_next(&mut self, cx: &AppCx) {
        let pinned = Self::ap_pinned(cx);
        match Self::ap_pinnable(cx).into_iter().find(|(id, _)| !pinned.contains(id)) {
            Some((id, label)) if pinned.len() < 10 => {
                let mut v = pinned;
                v.push(id);
                look_registry::save_dock(cx.data_dir(), &cx.session.user, &v);
                cx.audit.record(&cx.session.user, "settings.display.dock", &v.join(","));
                look_registry::notify_changed();
                self.ok(format!("{label} is in the dock."));
            }
            _ => self.fail("The dock is full (10 apps) or every app you may use is already in it."),
        }
    }

    fn ap_unpin_last(&mut self, cx: &AppCx) {
        let mut v = Self::ap_pinned(cx);
        if v.pop().is_some() {
            look_registry::save_dock(cx.data_dir(), &cx.session.user, &v);
            cx.audit.record(&cx.session.user, "settings.display.dock", &v.join(","));
            look_registry::notify_changed();
            self.ok("The last app is out of the dock.");
        }
    }

    pub(super) fn appearance_buttons(&self) -> Vec<Btn> {
        vec![("Next look", ACCENT, ']'), ("Wallpaper", TEXT, 'w'), ("Icon style", TEXT, 's'), ("Icon shape", TEXT, 'h'), ("Layout", TEXT, 'y'), ("Dock", TEXT, 'd'), ("Pin an app", TEXT, 'a')]
    }

    /// One row of the page: a title with its current value and the key that changes it. A click is the key.
    fn ap_row(g: &mut Gfx, r: R, s: f32, title: &str, value: &str, key: char, help: &str) {
        let hot = g.hovered(r);
        crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, false, hot, ACCENT);
        let vw = g.text_width(14.0 * s, value, Face::Bold).min(r.w * 0.5);
        let value = g.fit(14.0 * s, value, Face::Bold, r.w * 0.5);
        g.text(r.x + r.w - 16.0 * s - vw, r.y + 10.0 * s, 14.0 * s, ACCENT, &value, Face::Bold);
        let title = g.fit(15.0 * s, title, Face::Bold, (r.w - vw - 48.0 * s).max(40.0 * s));
        g.text(r.x + 16.0 * s, r.y + 9.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
        let help = g.fit(12.0 * s, &format!("{help}  ({key})"), Face::Sans, r.w - 32.0 * s);
        g.text(r.x + 16.0 * s, r.y + 31.0 * s, 12.0 * s, DIM, &help, Face::Sans);
        g.hit(r, Event::Key(Key::Char(key)), title.as_str());
    }

    /// The preview: the launcher's island with six sample icons in the chosen shape and style, and the dock where the Look puts it.
    fn ap_preview(g: &mut Gfx, r: R, s: f32, l: &Look) {
        use iyakki_uikit::appicon;
        g.clipped(r, |g| {
            g.rrect(r.x, r.y, r.w, r.h, 16.0 * s, with_alpha_c(crate::ui::CARD_HOVER, 255));
            let side = matches!(l.dock.position, DockPos::Left | DockPos::Right);
            let dock_h = if l.dock.position == DockPos::Bottom { 34.0 * s } else { 0.0 };
            let dock_w = if side { 34.0 * s } else { 0.0 };
            let (ix0, ix1) = (r.x + 10.0 * s + if l.dock.position == DockPos::Left { dock_w } else { 0.0 }, r.x + r.w - 10.0 * s - if l.dock.position == DockPos::Right { dock_w } else { 0.0 });
            let isl = R::new(ix0, r.y + 10.0 * s, (ix1 - ix0).max(10.0), (r.h - 20.0 * s - dock_h).max(10.0));
            if l.launcher.island {
                appicon::island(g, isl, s * 0.5, 1.0);
            }
            let sample: [(Icon, [u8; 4]); 6] = [(Icon::Chat, rgb(79, 140, 255)), (Icon::Browser, rgb(88, 190, 235)), (Icon::Ide, rgb(100, 230, 255)), (Icon::Edit, rgb(110, 224, 160)), (Icon::Cli, rgb(255, 184, 77)), (Icon::System, rgb(176, 226, 92))];
            let il = appicon::IconLook { shape: l.icons.shape, style: l.icons.style };
            let cols = if l.launcher.layout == LauncherLayout::List { 1 } else { 3 };
            let n = if cols == 1 { 3 } else { 6 };
            let cell = ((isl.w - 16.0 * s) / cols as f32).min((isl.h - 12.0 * s) / (n as f32 / cols as f32).ceil()).max(8.0);
            let ic = (cell * 0.62).min(46.0 * s);
            for (i, (icon, hue)) in sample.iter().take(n).enumerate() {
                let (cx0, cy0) = (isl.x + 8.0 * s + (i % cols) as f32 * ((isl.w - 16.0 * s) / cols as f32), isl.y + 6.0 * s + (i / cols) as f32 * cell);
                let sq = R::new(cx0 + (((isl.w - 16.0 * s) / cols as f32) - ic) / 2.0, cy0 + 2.0 * s, ic, ic);
                let b = appicon::body_with(g, sq, *hue, true, 1.0, s * 0.5, il);
                appicon::glyph_with(g, *icon, b, s * 0.5, il.effective().style);
                if l.icons.labels {
                    g.text(sq.x, sq.y + ic + 2.0 * s, 9.0 * s, MUTED, "App", Face::Sans);
                }
            }
            if l.dock.position != DockPos::Off {
                let (dx, dy, dw, dh) = match l.dock.position {
                    DockPos::Bottom => (r.x + r.w * 0.25, r.y + r.h - 30.0 * s, r.w * 0.5, 22.0 * s),
                    DockPos::Left => (r.x + 6.0 * s, r.y + r.h * 0.2, 24.0 * s, r.h * 0.6),
                    _ => (r.x + r.w - 30.0 * s, r.y + r.h * 0.2, 24.0 * s, r.h * 0.6),
                };
                g.rrect(dx, dy, dw, dh, 8.0 * s, with_alpha_c(crate::ui::CARD, 235));
                g.rrect_outline(dx, dy, dw, dh, 8.0 * s, 1.0, with_alpha_c(crate::ui::BORDER, 200));
                for i in 0..4 {
                    let (px, py) = if side { (dx + 4.0 * s, dy + 4.0 * s + i as f32 * 18.0 * s) } else { (dx + 6.0 * s + i as f32 * 22.0 * s, dy + 3.0 * s) };
                    let (c, ic) = sample[i];
                    appicon::body_with(g, R::new(px, py, 16.0 * s, 16.0 * s), ic, true, 1.0, s * 0.3, il);
                    let _ = c;
                }
            }
        });
    }

    pub(super) fn render_appearance(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let l = Self::ap_look(cx);
        let chosen = look_registry::load_choice(cx.data_dir(), &cx.session.user);
        let wall = wp::load(cx.data_dir(), &cx.session.user);
        let pinned = Self::ap_pinned(cx);
        let labels = Self::ap_pinnable(cx);
        let pinned_names: Vec<String> = pinned.iter().filter_map(|id| labels.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone())).collect();
        let wide = a.w >= 700.0 * s;
        // the preview: beside the rows on a wide page, above them on a narrow one
        let ph = if wide { 170.0 * s } else { 150.0 * s };
        let pw = if wide { (a.w * 0.36).min(380.0 * s) } else { a.w };
        let preview = R::new(if wide { a.x + a.w - pw } else { a.x }, a.y, pw, ph);
        Self::ap_preview(g, preview, s, &l);
        let who = chosen.as_deref().map_or("Default".to_string(), |k| look_registry::available(cx.data_dir(), &cx.session.user).into_iter().find(|e| e.key == k).and_then(|e| e.result.ok().map(|a| a.look.name)).unwrap_or_else(|| k.to_string()));
        let rows_w = if wide { a.w - pw - 12.0 * s } else { a.w };
        let mut y = a.y;
        if !wide {
            y += ph + 10.0 * s;
        }
        let rh = 52.0 * s;
        let gap = 7.0 * s;
        let at = |g: &mut Gfx, y: &mut f32, title: &str, value: &str, key: char, help: &str| {
            Self::ap_row(g, R::new(a.x, *y, rows_w, rh), s, title, value, key, help);
            *y += rh + gap;
        };
        at(g, &mut y, "Look", &who, ']', "[ and ] go through the Looks: Default, Compact, Classic and your own files");
        // the wallpaper as buttons
        let wr = R::new(a.x, y, rows_w, 62.0 * s);
        crate::fx::row_at(g, wr.x, wr.y, wr.w, wr.h, 12.0 * s, false, false, ACCENT);
        g.text(wr.x + 16.0 * s, wr.y + 7.0 * s, 15.0 * s, TEXT, "Wallpaper", Face::Bold);
        let (mut bx, mut by) = (wr.x + 16.0 * s, wr.y + 30.0 * s);
        let mut extra = 0.0;
        for (i, w) in wp::ALL.iter().enumerate() {
            let label = if *w == wall { format!("\u{2713} {}", w.name()) } else { w.name().to_string() };
            let bw = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.7;
            if bx + bw > wr.x + wr.w - 12.0 * s && bx > wr.x + 20.0 * s {
                bx = wr.x + 16.0 * s;
                by += 32.0 * s;
                extra += 32.0 * s;
            }
            g.button(bx, by, 13.0 * s, &label, if *w == wall { ACCENT } else { MUTED }, Event::Key(Key::Char((b'1' + i as u8) as char)));
            bx += bw + 6.0 * s;
        }
        let _ = extra;
        y += wr.h + gap + extra;
        at(g, &mut y, "Text font", &format!("{} ({})", iyakki_uikit::fonts::current_ui().name(), iyakki_uikit::fonts::current_ui().licence()), 'f', "DejaVu Sans or Liberation Sans for the interface; the size stays as you chose it");
        at(g, &mut y, "Code font", &format!("{} ({})", iyakki_uikit::fonts::current_code().name(), iyakki_uikit::fonts::current_code().licence()), 'p', "DejaVu Sans Mono or Liberation Mono for code and fixed-width text");
        let tp = iyakki_uikit::typeset::current();
        at(g, &mut y, "Line height", tp.line.key(), 'l', "tight, normal or loose (L for the other way); apps made with the UI SDK follow it");
        at(g, &mut y, "Letter spacing", tp.tracking.key(), 'm', "normal, wide or wider (M for the other way)");
        at(g, &mut y, "Tabular figures", if tp.tabular { "on" } else { "off" }, 'u', "digits of one width, for columns of numbers");
        at(g, &mut y, "Icon style", l.icons.style.key(), 's', "gradient, flat, outline, glass or 3d (S for the other way)");
        at(g, &mut y, "Icon shape", l.icons.shape.key(), 'h', "squircle, circle, rounded or square");
        at(g, &mut y, "Icon size", &l.icons.size.text(), 'z', "S, M or L");
        at(g, &mut y, "Names under icons", if l.icons.labels { "shown" } else { "hidden" }, 'i', "the app name and its line");
        at(g, &mut y, "Launcher", l.launcher.layout.key(), 'y', "paged, grid or list");
        at(g, &mut y, "Columns", &Self::ap_columns_text(l.launcher.columns), 'c', "automatic, or 3 to 8 across");
        at(g, &mut y, "Spacing", l.launcher.density.key(), 'e', "compact, comfortable or roomy");
        at(g, &mut y, "Dock", l.dock.position.key(), 'd', "bottom, left, right or off");
        at(g, &mut y, "Dot under running apps", if l.dock.running { "on" } else { "off" }, 'r', "in the dock");
        at(g, &mut y, "EDIT version (experimental)", if crate::apps::sdk_apps::chosen(cx, "edit") { "the SDK version" } else { "the usual screen" }, 't', "the editor written on the UI SDK: pinch changes the text size; the usual one stays");
        at(g, &mut y, "CLI version (experimental)", if crate::apps::sdk_apps::chosen(cx, "cli") { "the SDK version" } else { "the usual screen" }, 'T', "the command line written on the UI SDK, with the whole-session terminal; the usual one stays");
        at(g, &mut y, "Pinned apps", &format!("{} in the dock", pinned_names.len()), 'a', "A pins the next app, X takes the last out, O puts the usual ones back");
        let names = pinned_names.join(", ");
        for line in g.wrap(13.0 * s, &names, Face::Sans, rows_w).into_iter().take(3) {
            g.text(a.x + 6.0 * s, y, 13.0 * s, MUTED, &line, Face::Sans);
            y += 17.0 * s;
        }
        y += 4.0 * s;
        for (key, item, name) in BAR_KEYS {
            at(g, &mut y, &format!("{name} in the top bar"), if l.topbar.items.contains(&item) { "shown" } else { "hidden" }, key, "the bell, what needs you, Settings, your account and power always stay");
        }
        let notes = "On the dock: d puts the keys on it, comma and period move an app, x unpins; or drag an icon. Larger text, reduced motion and High contrast overrule a Look, and the sign-in screen is always the default Dark look.";
        for line in g.wrap(12.5 * s, notes, Face::Sans, rows_w) {
            g.text(a.x + 6.0 * s, y, 12.5 * s, DIM, &line, Face::Sans);
            y += 17.0 * s;
        }
        g.ink_bottom = g.ink_bottom.max(y);
    }
}

fn with_alpha_c(c: [u8; 4], a: u8) -> [u8; 4] {
    crate::fx::with_alpha(c, a)
}
