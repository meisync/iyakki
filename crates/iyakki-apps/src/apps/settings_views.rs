//! Settings -> Desktop views: the person's named views (make, rename, delete, put in order, lock), the views from a template (Work,
//! Media, Monitoring), which step keys are on (browser-style, Mac-style or both) and whether the lock returns to the first view.
//! It reads and writes the person's own file (`iyakki_shared::views`); the shell reads it again when Settings is left
//! (`shell_views.rs`). Everyone's own choice, not an administrator's setting.

use super::*;
use super::super::settings_device_port::UserPort;
use iyakki_shared::views::{UserTemplates, Views, MAX_VIEWS, TEMPLATES};

/// One line of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VwRow {
    /// A view: id, name, locked, how many apps its template opens, its number (1 to 9).
    View { id: u32, name: String, locked: bool, tiles: usize, number: usize },
    /// Which step keys are on.
    Keys,
    /// Whether the lock returns to the first view.
    ResetOnLock,
    /// Whether each view's window arrangement is remembered.
    RememberLayout,
    /// Whether an app keeps one window (opening it again goes to the open one).
    OneWindow,
    /// The person's own step keys (Ctrl+Alt plus a key each way).
    OwnKeys,
}

impl SettingsApp {
    fn vw_load(p: &dyn UserPort) -> Views {
        Views::load(&p.data_dir(), &p.user())
    }

    fn vw_save(p: &dyn UserPort, v: &Views) {
        v.save(&p.data_dir(), &p.user());
    }

    pub(super) fn vw_rows(p: &dyn UserPort) -> Vec<VwRow> {
        let v = Self::vw_load(p);
        let mut rows: Vec<VwRow> = v.views.iter().enumerate().map(|(i, w)| VwRow::View { id: w.id, name: w.name.clone(), locked: w.locked, tiles: w.tiles.len(), number: i + 1 }).collect();
        rows.push(VwRow::Keys);
        rows.push(VwRow::ResetOnLock);
        rows.push(VwRow::RememberLayout);
        rows.push(VwRow::OneWindow);
        rows.push(VwRow::OwnKeys);
        rows
    }

    pub(super) fn vw_len(p: &dyn UserPort) -> usize {
        Self::vw_rows(p).len()
    }

    pub(super) fn handle_views(&mut self, k: &Key, p: &dyn UserPort) {
        let rows = Self::vw_rows(p);
        let cur = rows.get(self.sel()).cloned();
        match (k, cur) {
            (Key::Up, _) => self.move_sel(-1, rows.len()),
            (Key::Down, _) => self.move_sel(1, rows.len()),
            (Key::Char('n'), _) => self.open_form(FormKind::View(None), Form::new("New view").text("Name (up to 16 characters)", "", "for example Docs")),
            (Key::Char('t'), _) => {
                let mut items: Vec<String> = TEMPLATES.iter().map(|t| format!("{} ({})", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", "))).collect();
                // the person's own templates (saved by chat) come after the built-in ones
                items.extend(UserTemplates::load(&p.data_dir(), &p.user()).list.iter().map(|t| format!("{} ({}) - yours", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", "))));
                // then the templates installed bundles brought (kept members that passed the schema)
                items.extend(iyakki_shared::bundle_members::bundled_view_templates(&p.data_dir(), &p.user()).iter().map(|t| format!("{} ({}) - from a bundle", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", "))));
                self.modal = Some(Modal::Pick(PickKind::ViewTemplate, Picker::new("New view from a template", items)));
            }
            (Key::Char('r'), Some(VwRow::View { id, name, .. })) => self.open_form(FormKind::View(Some(id)), Form::new("Rename view").text("Name (up to 16 characters)", &name, "")),
            (Key::Char('x'), Some(VwRow::View { id, name, .. })) => {
                let text = format!("Delete the view “{name}”? The apps open in it are closed. Nothing is uninstalled.");
                self.modal = Some(Modal::Confirm(ConfirmKind::ViewDelete(id), text));
            }
            (Key::Char(c @ ('[' | ']')), Some(VwRow::View { id, .. })) => {
                let mut v = Self::vw_load(p);
                if v.shift(id, *c == '[') {
                    Self::vw_save(p, &v);
                    self.move_sel(if *c == '[' { -1 } else { 1 }, rows.len());
                    self.ok("View moved.");
                }
            }
            (Key::Char('l') | Key::Enter, Some(VwRow::View { id, name, locked, .. })) => {
                let mut v = Self::vw_load(p);
                v.set_locked(id, !locked);
                Self::vw_save(p, &v);
                p.audit("view.lock", &format!("{name}:{}", !locked));
                self.ok(if locked { format!("{name} opens without the password again.") } else { format!("{name} asks for your password each time you switch to it.") });
            }
            (Key::Char('k') | Key::Enter, Some(VwRow::Keys)) | (Key::Char('k'), _) => {
                let mut v = Self::vw_load(p);
                v.key_set = v.key_set.next();
                Self::vw_save(p, &v);
                self.ok(format!("Step keys: {}.", v.key_set.label()));
            }
            (Key::Char('o') | Key::Enter, Some(VwRow::ResetOnLock)) | (Key::Char('o'), _) => {
                let mut v = Self::vw_load(p);
                v.reset_on_lock = !v.reset_on_lock;
                Self::vw_save(p, &v);
                self.ok(if v.reset_on_lock { "After the lock or the screen saver you return to the first view." } else { "After the lock or the screen saver you stay on the view you were in." });
            }
            (Key::Char('y') | Key::Enter, Some(VwRow::RememberLayout)) | (Key::Char('y'), _) => {
                let mut v = Self::vw_load(p);
                let on = !v.remember_layout;
                v.set_remember_layout(on);
                Self::vw_save(p, &v);
                p.audit("view.layout", if on { "remember on" } else { "remember off" });
                self.ok(if on { "Each view keeps its window arrangement, also after you sign out." } else { "Window arrangements are no longer kept; the saved ones are forgotten." });
            }
            (Key::Char('i') | Key::Enter, Some(VwRow::OneWindow)) | (Key::Char('i'), _) => {
                let mut v = Self::vw_load(p);
                let on = v.single_apps.is_empty();
                let _ = v.set_single(&if on { vec!["*".to_string()] } else { Vec::new() });
                Self::vw_save(p, &v);
                p.audit("view.single", if on { "all apps" } else { "off" });
                self.ok(if on { "Every app keeps one window: opening an app that is already open goes to it. (Chat can choose single apps.)" } else { "An app can open more than once again." });
            }
            (Key::Char('j') | Key::Enter, Some(VwRow::OwnKeys)) | (Key::Char('j'), _) => {
                let v = Self::vw_load(p);
                let (p, n) = v.custom_keys.as_ref().map(|c| (Self::vw_typed(&c.prev), Self::vw_typed(&c.next))).unwrap_or_default();
                self.open_form(FormKind::ViewKeys, Form::new("Your own step keys (Ctrl+Alt plus the key)").text("Previous view key (a letter other than M, Q, L, S, or [ ] , .)", &p, "for example J").text("Next view key", &n, "for example K; both empty turns them off"));
            }
            _ => {}
        }
    }

    fn vw_typed(name: &str) -> String {
        match name {
            "BracketLeft" => "[".into(),
            "BracketRight" => "]".into(),
            "Comma" => ",".into(),
            "Period" => ".".into(),
            n => n.trim_start_matches("Key").to_string(),
        }
    }

    /// The own-keys form was sent.
    pub(super) fn vw_keys_submit(&mut self, p: &dyn UserPort, form: &Form) -> Result<String, String> {
        let keys = iyakki_shared::views::CustomKeys::from_typed(&form.value(0), &form.value(1))?;
        let mut v = Self::vw_load(p);
        let msg = match &keys {
            Some(c) => format!("Own step keys: {}.", c.label()),
            None => "Own step keys are off.".to_string(),
        };
        v.custom_keys = keys;
        Self::vw_save(p, &v);
        p.audit("view.keys", if v.custom_keys.is_some() { "own keys on" } else { "own keys off" });
        Ok(msg)
    }

    pub(super) fn vw_submit(&mut self, p: &dyn UserPort, editing: &Option<u32>, form: &Form) -> Result<String, String> {
        let mut v = Self::vw_load(p);
        let name = form.value(0);
        let r = match editing {
            Some(id) => v.rename(*id, &name).map(|_| format!("Renamed to {}.", name.trim())),
            None => v.add(&name, None).map(|_| format!("Made the view {}.", name.trim())),
        };
        if r.is_ok() {
            Self::vw_save(p, &v);
            p.audit("view.edit", &name);
        }
        r
    }

    /// A template was picked: a view named after it (a number is added when the name is taken).
    pub(super) fn vw_pick(&mut self, p: &dyn UserPort, item: &str) {
        let mut v = Self::vw_load(p);
        let own = UserTemplates::load(&p.data_dir(), &p.user()).list.into_iter().find(|t| item.starts_with(&format!("{} (", t.name)) && item.ends_with(" - yours"));
        let bundled = iyakki_shared::bundle_members::bundled_view_templates(&p.data_dir(), &p.user()).into_iter().find(|t| item == format!("{} ({}) - from a bundle", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", ")));
        let own = own.or(bundled);
        let made = match (&own, TEMPLATES.iter().find(|t| item.starts_with(&format!("{} (", t.name)) && !item.ends_with(" - yours"))) {
            (Some(t), _) => v.add_from_user_template(t),
            (None, Some(t)) => v.add_from_template(t.id),
            (None, None) => return,
        };
        match made {
            Ok((_, name)) => {
                Self::vw_save(p, &v);
                p.audit("view.edit", &name);
                self.ok(format!("Made the view {name}. Its apps open the first time you switch to it."));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn vw_confirm(&mut self, p: &dyn UserPort, id: u32) {
        let mut v = Self::vw_load(p);
        match v.delete(id) {
            Ok(()) => {
                Self::vw_save(p, &v);
                p.audit("view.delete", &id.to_string());
                self.ok("View deleted.");
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_views(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn UserPort) {
        let rows = Self::vw_rows(p);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 30.0 * s;
        let mut y = a.y;
        let head = g.fit(13.0 * s, &format!("Views (up to {MAX_VIEWS}). Jump: Ctrl+Alt+Super+number. Step: see Keys below."), Face::Bold, a.w);
        g.text(a.x, y + 4.0 * s, 13.0 * s, DIM, &head, Face::Bold);
        y += 26.0 * s;
        let v = Self::vw_load(p);
        for (i, row) in rows.iter().enumerate() {
            if y + row_h > a.y + a.h {
                break;
            }
            let r = R::new(a.x, y, a.w, row_h - 2.0 * s);
            let on = i == sel;
            if on {
                g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(30, 38, 68));
            }
            g.hit(r, Event::Row(i), "row");
            let (left, right) = match row {
                VwRow::View { name, locked, tiles, number, .. } => (format!("{number}  {name}"), format!("{}{}", if *locked { "locked, asks for your password" } else { "open" }, if *tiles > 0 { format!(" · opens {tiles} apps first") } else { String::new() })),
                VwRow::Keys => ("Keys".to_string(), v.key_set.label().to_string()),
                VwRow::ResetOnLock => ("After the lock or saver".to_string(), if v.reset_on_lock { "return to the first view".to_string() } else { "stay on the view you were in".to_string() }),
                VwRow::RememberLayout => ("Window arrangement".to_string(), if v.remember_layout { "kept for each view".to_string() } else { "not kept".to_string() }),
                VwRow::OwnKeys => ("Your own step keys".to_string(), v.custom_keys.as_ref().map(|c| c.label()).unwrap_or_else(|| "off".to_string())),
                VwRow::OneWindow => ("One window per app".to_string(), if v.single_apps.is_empty() { "off".to_string() } else if v.is_single("*") { "every app".to_string() } else { v.single_apps.join(", ") }),
            };
            let ty = r.y + (r.h - 14.0 * s) / 2.0 - s;
            let left = g.fit(14.0 * s, &left, Face::Bold, r.w * 0.4);
            g.text(r.x + 10.0 * s, ty, 14.0 * s, if on { TEXT } else { MUTED }, &left, Face::Bold);
            let right = g.fit(13.0 * s, &right, Face::Sans, r.w * 0.55);
            let rw = g.text_width(13.0 * s, &right, Face::Sans);
            let hot = matches!(row, VwRow::View { locked: true, .. });
            g.text(r.x + r.w - rw - 10.0 * s, ty, 13.0 * s, if hot { ACCENT } else { DIM }, &right, Face::Sans);
            y += row_h;
        }
    }

    pub(super) fn views_buttons(&self) -> Vec<Btn> {
        let (acc, mut_) = (ACCENT, TEXT);
        vec![("New view", acc, 'n'), ("From a template", acc, 't'), ("Rename", mut_, 'r'), ("Delete view", ERR, 'x'), ("Earlier", mut_, '['), ("Later", mut_, ']'), ("Lock on/off", mut_, 'l'), ("Keys", mut_, 'k'), ("After the lock", mut_, 'o'), ("Keep arrangement", mut_, 'y'), ("One window per app", mut_, 'i'), ("Own step keys", mut_, 'j')]
    }
}
