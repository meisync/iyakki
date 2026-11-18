//! Settings -> Desktop views: the person's named views (make, rename, delete, put in order, lock), the views from a template (Work,
//! Media, Monitoring), which step keys are on (browser-style, Mac-style or both) and whether the lock returns to the first view.
//! It reads and writes the person's own file (`iyakki_shared::views`); the shell reads it again when Settings is left
//! (`shell_views.rs`). Everyone's own choice, not an administrator's setting.

use super::*;
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
}

impl SettingsApp {
    fn vw_load(cx: &AppCx) -> Views {
        Views::load(cx.data_dir(), &cx.session.user)
    }

    fn vw_save(cx: &AppCx, v: &Views) {
        v.save(cx.data_dir(), &cx.session.user);
    }

    pub(super) fn vw_rows(cx: &AppCx) -> Vec<VwRow> {
        let v = Self::vw_load(cx);
        let mut rows: Vec<VwRow> = v.views.iter().enumerate().map(|(i, w)| VwRow::View { id: w.id, name: w.name.clone(), locked: w.locked, tiles: w.tiles.len(), number: i + 1 }).collect();
        rows.push(VwRow::Keys);
        rows.push(VwRow::ResetOnLock);
        rows.push(VwRow::RememberLayout);
        rows.push(VwRow::OneWindow);
        rows
    }

    pub(super) fn vw_len(cx: &AppCx) -> usize {
        Self::vw_rows(cx).len()
    }

    pub(super) fn handle_views(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::vw_rows(cx);
        let cur = rows.get(self.sel()).cloned();
        match (k, cur) {
            (Key::Up, _) => self.move_sel(-1, rows.len()),
            (Key::Down, _) => self.move_sel(1, rows.len()),
            (Key::Char('n'), _) => self.open_form(FormKind::View(None), Form::new("New view").text("Name (up to 16 characters)", "", "for example Docs")),
            (Key::Char('t'), _) => {
                let mut items: Vec<String> = TEMPLATES.iter().map(|t| format!("{} ({})", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", "))).collect();
                // the person's own templates (saved by chat) come after the built-in ones
                items.extend(UserTemplates::load(cx.data_dir(), &cx.session.user).list.iter().map(|t| format!("{} ({}) - yours", t.name, t.tiles.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join(", "))));
                self.modal = Some(Modal::Pick(PickKind::ViewTemplate, Picker::new("New view from a template", items)));
            }
            (Key::Char('r'), Some(VwRow::View { id, name, .. })) => self.open_form(FormKind::View(Some(id)), Form::new("Rename view").text("Name (up to 16 characters)", &name, "")),
            (Key::Char('x'), Some(VwRow::View { id, name, .. })) => {
                let text = format!("Delete the view “{name}”? The apps open in it are closed. Nothing is uninstalled.");
                self.modal = Some(Modal::Confirm(ConfirmKind::ViewDelete(id), text));
            }
            (Key::Char(c @ ('[' | ']')), Some(VwRow::View { id, .. })) => {
                let mut v = Self::vw_load(cx);
                if v.shift(id, *c == '[') {
                    Self::vw_save(cx, &v);
                    self.move_sel(if *c == '[' { -1 } else { 1 }, rows.len());
                    self.ok("View moved.");
                }
            }
            (Key::Char('l') | Key::Enter, Some(VwRow::View { id, name, locked, .. })) => {
                let mut v = Self::vw_load(cx);
                v.set_locked(id, !locked);
                Self::vw_save(cx, &v);
                cx.audit.record(&cx.session.user, "view.lock", &format!("{name}:{}", !locked));
                self.ok(if locked { format!("{name} opens without the password again.") } else { format!("{name} asks for your password each time you switch to it.") });
            }
            (Key::Char('k') | Key::Enter, Some(VwRow::Keys)) | (Key::Char('k'), _) => {
                let mut v = Self::vw_load(cx);
                v.key_set = v.key_set.next();
                Self::vw_save(cx, &v);
                self.ok(format!("Step keys: {}.", v.key_set.label()));
            }
            (Key::Char('o') | Key::Enter, Some(VwRow::ResetOnLock)) | (Key::Char('o'), _) => {
                let mut v = Self::vw_load(cx);
                v.reset_on_lock = !v.reset_on_lock;
                Self::vw_save(cx, &v);
                self.ok(if v.reset_on_lock { "After the lock or the screen saver you return to the first view." } else { "After the lock or the screen saver you stay on the view you were in." });
            }
            (Key::Char('y') | Key::Enter, Some(VwRow::RememberLayout)) | (Key::Char('y'), _) => {
                let mut v = Self::vw_load(cx);
                let on = !v.remember_layout;
                v.set_remember_layout(on);
                Self::vw_save(cx, &v);
                cx.audit.record(&cx.session.user, "view.layout", if on { "remember on" } else { "remember off" });
                self.ok(if on { "Each view keeps its window arrangement, also after you sign out." } else { "Window arrangements are no longer kept; the saved ones are forgotten." });
            }
            (Key::Char('i') | Key::Enter, Some(VwRow::OneWindow)) | (Key::Char('i'), _) => {
                let mut v = Self::vw_load(cx);
                let on = v.single_apps.is_empty();
                let _ = v.set_single(&if on { vec!["*".to_string()] } else { Vec::new() });
                Self::vw_save(cx, &v);
                cx.audit.record(&cx.session.user, "view.single", if on { "all apps" } else { "off" });
                self.ok(if on { "Every app keeps one window: opening an app that is already open goes to it. (Chat can choose single apps.)" } else { "An app can open more than once again." });
            }
            _ => {}
        }
    }

    pub(super) fn vw_submit(&mut self, cx: &AppCx, editing: &Option<u32>, form: &Form) -> Result<String, String> {
        let mut v = Self::vw_load(cx);
        let name = form.value(0);
        let r = match editing {
            Some(id) => v.rename(*id, &name).map(|_| format!("Renamed to {}.", name.trim())),
            None => v.add(&name, None).map(|_| format!("Made the view {}.", name.trim())),
        };
        if r.is_ok() {
            Self::vw_save(cx, &v);
            cx.audit.record(&cx.session.user, "view.edit", &name);
        }
        r
    }

    /// A template was picked: a view named after it (a number is added when the name is taken).
    pub(super) fn vw_pick(&mut self, cx: &AppCx, item: &str) {
        let mut v = Self::vw_load(cx);
        let own = UserTemplates::load(cx.data_dir(), &cx.session.user).list.into_iter().find(|t| item.starts_with(&format!("{} (", t.name)) && item.ends_with(" - yours"));
        let made = match (&own, TEMPLATES.iter().find(|t| item.starts_with(&format!("{} (", t.name)) && !item.ends_with(" - yours"))) {
            (Some(t), _) => v.add_from_user_template(t),
            (None, Some(t)) => v.add_from_template(t.id),
            (None, None) => return,
        };
        match made {
            Ok((_, name)) => {
                Self::vw_save(cx, &v);
                cx.audit.record(&cx.session.user, "view.edit", &name);
                self.ok(format!("Made the view {name}. Its apps open the first time you switch to it."));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn vw_confirm(&mut self, cx: &AppCx, id: u32) {
        let mut v = Self::vw_load(cx);
        match v.delete(id) {
            Ok(()) => {
                Self::vw_save(cx, &v);
                cx.audit.record(&cx.session.user, "view.delete", &id.to_string());
                self.ok("View deleted.");
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_views(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::vw_rows(cx);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 30.0 * s;
        let mut y = a.y;
        let head = g.fit(13.0 * s, &format!("Views (up to {MAX_VIEWS}). Jump: Ctrl+Alt+Super+number. Step: see Keys below."), Face::Bold, a.w);
        g.text(a.x, y + 4.0 * s, 13.0 * s, DIM, &head, Face::Bold);
        y += 26.0 * s;
        let v = Self::vw_load(cx);
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
        vec![("New view", acc, 'n'), ("From a template", acc, 't'), ("Rename", mut_, 'r'), ("Delete view", ERR, 'x'), ("Earlier", mut_, '['), ("Later", mut_, ']'), ("Lock on/off", mut_, 'l'), ("Keys", mut_, 'k'), ("After the lock", mut_, 'o'), ("Keep arrangement", mut_, 'y'), ("One window per app", mut_, 'i')]
    }
}
