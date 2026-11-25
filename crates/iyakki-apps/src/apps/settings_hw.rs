//! Settings: the hw page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::*;
use super::super::settings_device_port::{SoundPort, UserPort};

impl SettingsApp {
    pub(super) fn dev_tab(&self) -> Option<DevTab> {
        match self.tab() {
            Tab::Network => Some(DevTab::Network),
            Tab::Display => Some(DevTab::Display),
            Tab::Sound => Some(DevTab::Sound),
            _ => None,
        }
    }

    pub(super) fn dev_rows(&mut self) -> Vec<DevRow> {
        let ticks = self.ticks;
        // a window on another computer: the host operating system owns the MAC addresses
        let host = self.storage.virtual_disks.is_some();
        self.hw.set_host_managed(host);
        self.dev_tab().map(|t| self.hw.rows(t, ticks)).unwrap_or_default()
    }

    pub(super) fn selected_target(&mut self) -> Option<Target> {
        let sel = self.sel();
        self.dev_rows().get(sel).map(|r| r.target.clone())
    }

    /// The adapter the selected Network row belongs to.
    pub(super) fn selected_adapter(&mut self) -> Option<(String, iyakki_core_api::device_types::devctl::LinkKind)> {
        match self.selected_target()? {
            Target::Ethernet(n) => Some((n, iyakki_core_api::device_types::devctl::LinkKind::Ethernet)),
            Target::Wifi(n, _) => Some((n, iyakki_core_api::device_types::devctl::LinkKind::Wifi)),
            Target::Mac(n, k) => Some((n, k)),
            _ => None,
        }
    }

    /// Bigger (`u`) or smaller (`U`) text and buttons, kept for this person.
    pub(super) fn change_ui_size(&mut self, bigger: bool, p: &dyn super::super::settings_device_port::UserPort) {
        use crate::uisize::{apply, load, save, LEVELS};
        let cur = load(&p.data_dir(), &p.user());
        let next = if bigger { (cur + 1).min(LEVELS.len() - 1) } else { cur.saturating_sub(1) };
        if next == cur {
            return self.ok(format!("Already the {} size.", if bigger { "largest" } else { "smallest" }));
        }
        save(&p.data_dir(), &p.user(), next);
        apply(Some(next));
        p.audit("settings.display.text_size", LEVELS[next].0);
        self.ok(format!("Text and buttons: {} ({}%).", LEVELS[next].0, (LEVELS[next].1 * 100.0).round()));
    }

    /// Choose the desktop wallpaper for this person (`w` next, `W` previous): kept beside their theme and UI size and applied at
    /// once, so the page behind this card is the preview. `Theme` is the background the shell has always drawn.
    pub(super) fn change_wallpaper(&mut self, forward: bool, p: &dyn super::super::settings_device_port::UserPort) {
        use iyakki_uikit::wallpaper as wp;
        let all = wp::ALL;
        let at = all.iter().position(|w| *w == wp::current()).unwrap_or(0);
        let next = all[if forward { (at + 1) % all.len() } else { (at + all.len() - 1) % all.len() }];
        wp::save(&p.data_dir(), &p.user(), next);
        wp::set_current(next);
        p.audit("settings.display.wallpaper", next.key());
        self.ok(format!("Wallpaper: {}.", next.name()));
    }

    /// Choose the theme for this person (`1` Dark, `2` Light, `3` High contrast, `t` next, `T` previous): kept with their other
    /// display settings and applied at once, so the page shows it as a live preview.
    pub(super) fn change_theme(&mut self, key: char, p: &dyn super::super::settings_device_port::UserPort) {
        let choices = Self::theme_choices(p);
        let at = choices.iter().position(|c| c.chosen).unwrap_or(0);
        let next = match key {
            't' => (at + 1) % choices.len(),
            'T' => (at + choices.len() - 1) % choices.len(),
            d => (d as usize).saturating_sub('1' as usize),
        };
        let Some(c) = choices.get(next) else {
            return self.fail("There is no theme under that number.".to_string());
        };
        let (dir, user) = (p.data_dir(), p.user());
        let (dir, user) = (dir.as_path(), &user);
        match (&c.builtin, &c.file) {
            (Some(id), _) => {
                crate::theme::save(dir, user, *id);
                crate::theme::apply(Some(*id));
                p.audit("settings.display.theme", id.name());
                self.ok(format!("Theme: {}.", id.name()));
            }
            (None, Some(f)) => match iyakki_uikit::usertheme::choose(dir, user, f) {
                Ok(name) => {
                    p.audit("settings.display.theme", &format!("file {f}"));
                    self.ok(format!("Theme: {name}."));
                }
                Err(e) => self.fail(format!("That theme cannot be used: {e}")),
            },
            _ => {}
        }
    }

    pub(super) fn handle_hw(&mut self, k: &Key, cx: &AppCx) {
        let Some(tab) = self.dev_tab() else { return };
        if tab == DevTab::Network {
            if self.share.open {
                return self.handle_share(k, cx);
            }
            if matches!(k, Key::Char('i')) {
                return self.share_open(cx);
            }
        }
        let len = self.dev_rows().len();
        match k {
            Key::Char('w') if tab == DevTab::Network => self.scan_wifi(cx),
            Key::Char('m') if tab == DevTab::Network => self.choose_mac_mode(cx),
            Key::Char('a') if tab == DevTab::Network => self.ask_mac_apply(cx),
            Key::Char('n') if tab == DevTab::Network => self.toggle_announce(cx),
            Key::Char('o') if tab == DevTab::Network => self.open_sign_in(cx),
            Key::Char('v') if tab == DevTab::Network => self.use_next_adapter(cx),
            Key::Enter | Key::Char(' ') if tab == DevTab::Network && matches!(self.selected_target(), Some(Target::Mac(..))) => self.choose_mac_mode(cx),
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('r') => {
                self.hw.invalidate();
                self.ok("Looked at the hardware again.");
            }
            other => self.dev_act(tab, other, cx),
        }
    }

    /// A key that a device page's row takes (administrators only): the hardware acts and the outcome is audited under the page's name.
    pub(super) fn dev_act(&mut self, tab: DevTab, k: &Key, p: &dyn UserPort) {
        if !self.admin(p) {
            return;
        }
        let (sel, ticks) = (self.sel(), self.ticks);
        if let Some(d) = self.hw.act(tab, sel, k, ticks) {
            match d.result {
                Ok(m) => {
                    if !d.audit.is_empty() {
                        p.audit(&format!("settings.{}", self.tab().label().to_lowercase()), &d.audit);
                    }
                    self.ok(m);
                }
                Err(e) => self.fail(e),
            }
        }
    }

    pub(super) fn render_hw(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if self.tab() == Tab::Network && self.share.open {
            return self.render_share(g, a, s, cx);
        }
        let announce = if self.tab() == Tab::Network {
            match self.link.line() {
                Some(l) => format!("WiFi: {l}  ·  {}", self.announce_line(cx)),
                None => self.announce_line(cx),
            }
        } else {
            String::new()
        };
        let admin = cx.session.policy.manage_settings;
        self.draw_dev_rows(g, a, s, a.y, admin, announce.as_str());
    }

    /// The rows of a device page (Network, Sound, Display) from `y` down, and the help line at the bottom of the room.
    pub(super) fn draw_dev_rows(&mut self, g: &mut Gfx, a: R, s: f32, mut y: f32, admin: bool, note: &str) {
        let rows = self.dev_rows();
        let notes: Vec<String> = Vec::new();
        let low = a.y + a.h - 70.0 * s - if notes.is_empty() { 0.0 } else { (notes.len() as f32 - 1.0) * 17.0 * s };
        // a small screen: the rows above are left out; a click leaves them where they are, a key scrolls to the choice
        let step = |r: &DevRow| if r.slider.is_some() { 90.0 * s } else { 64.0 * s };
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let steps: Vec<f32> = rows.iter().map(step).collect();
        let first = self.win[self.tab].layout(sel, &steps, low - y + 6.0 * s);
        let mut cut: Option<String> = None;
        for (i, r) in rows.iter().enumerate().skip(first) {
            if y + if r.slider.is_some() { 82.0 * s } else { 58.0 * s } > low + 8.0 * s {
                break;
            }
            if let Some(v) = r.slider {
                // a click on the card selects the row; the buttons and the track on top of it change the value
                let card = R::new(a.x, y, a.w, 82.0 * s);
                g.hit(card, Event::Row(i), &r.title);
                let track = controls::slider_row(
                    g,
                    card,
                    s,
                    &Slider { id: SLIDER_HW + i, title: &r.title, help: &r.sub, value: v.value, min: v.min, max: v.max, unit: v.unit, minus: Event::Sub(i as u8, Key::Char('-')), plus: Event::Sub(i as u8, Key::Char('+')), enabled: admin && r.actionable() },
                    ACCENT,
                );
                self.slider_w.push((SLIDER_HW + i, track.w));
                if i == self.sel() {
                    g.rrect_outline(card.x, card.y, card.w, card.h, 14.0 * s, 2.0 * s, ACCENT);
                }
                y += 90.0 * s;
            } else {
                let chips: Vec<(&str, [u8; 4])> = r.chips.iter().map(|c| (c.0.as_str(), c.1)).collect();
                let on = i == self.sel();
                if let Some(f) = row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, on, &r.title, &r.sub, &chips) {
                    if i == self.sel() {
                        cut = Some(f);
                    }
                }
                // a choice row: the click chooses it (select, then Enter), not only selects
                if admin && r.target.activates_on_click() {
                    g.hit(R::new(a.x, y, a.w, 58.0 * s), Event::Sub(i as u8, Key::Enter), &r.title);
                }
                y += 64.0 * s;
            }
        }
        // the help line sits at the bottom of the room; when the cards and rows above it take more than the room (a small window, big
        // text) it follows them instead, and the page scrolls
        let bottom = (a.y + a.h).max(y + 46.0 * s);
        // the keyboard's way to the tooltip: the whole text of the selected row when its line was cut, above the help line
        if let Some(full) = cut.filter(|_| notes.is_empty()) {
            let one = full.replace('\n', " · ");
            let one = crate::tooltip::fit_keep_ip(g, 13.0 * s, &one, Face::Sans, a.w);
            g.text(a.x, bottom - 42.0 * s, 13.0 * s, TEXT, &one, Face::Sans);
        }
        if notes.is_empty() {
            // a page squeezed to almost nothing has no room for the help line: it would land on the text above the page
            if a.h >= 40.0 * s {
                let note = g.fit(13.0 * s, note, Face::Sans, a.w);
                g.text(a.x, bottom - 22.0 * s, 13.0 * s, DIM, &note, Face::Sans);
            }
        } else {
            let top = a.y + a.h - 22.0 * s - (notes.len() as f32 - 1.0) * 17.0 * s;
            for (i, l) in notes.iter().enumerate() {
                g.text(a.x, top + i as f32 * 17.0 * s, 13.0 * s, DIM, l, Face::Sans);
            }
        }
    }

    /// A click on a hardware page's slider track.
    pub(super) fn hw_slider(&mut self, id: usize, x: f32, w: f32, p: &dyn SoundPort) {
        let row = id - SLIDER_HW;
        let Some(tab) = self.dev_tab() else { return };
        if tab == DevTab::Sound {
            if let Some(Target::SoundBoost(card_id)) = self.dev_rows().get(row).map(|r| r.target.clone()) {
                self.sel[self.tab] = row;
                return self.sound_set_boost(p, &card_id, controls::value_at(x, w, 0, iyakki_shared::softvol::MAX_BOOST_DB as i64, 1) as u32);
            }
        }
        if tab == DevTab::Sound && matches!(self.dev_rows().get(row).map(|r| &r.target), Some(Target::SoundHighlightOffset)) {
            self.sel[self.tab] = row;
            return self.sound_set_highlight_offset(p, controls::value_at(x, w, -500, 500, 25) as i32);
        }
        if tab == DevTab::Sound && matches!(self.dev_rows().get(row).map(|r| &r.target), Some(Target::SoftVolume)) {
            self.sel[self.tab] = row;
            return self.sound_set_volume(p, controls::value_at(x, w, 0, 100, 5) as u32);
        }
        if !self.admin(p) {
            return;
        }
        let Some(v) = self.dev_rows().get(row).and_then(|r| r.slider) else { return };
        self.sel[self.tab] = row;
        let value = controls::value_at(x, w, v.min, v.max, v.step);
        let ticks = self.ticks;
        if let Some(d) = self.hw.set_value(tab, row, value, ticks) {
            match d.result {
                Ok(m) => {
                    if !d.audit.is_empty() {
                        p.audit(&format!("settings.{}", self.tab().label().to_lowercase()), &d.audit);
                    }
                    self.ok(m);
                }
                Err(e) => self.fail(e),
            }
        }
    }
}
