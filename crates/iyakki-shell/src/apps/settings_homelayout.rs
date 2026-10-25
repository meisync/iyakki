//! Settings -> Home layout: the groups on the home screen (the four built-in ones and the person's own: make, rename, delete,
//! put in order) and every tile with the group it is in (move it, or put it back), and a Reset layout. It reads and writes the
//! person's own layout file (`iyakki_agent::layout`); the launcher reads it again when Settings is left. Moving a tile changes
//! where it is listed, never what the app may do.
//!
//! Planned: a default layout an administrator sets for new accounts.

use super::*;
use iyakki_agent::layout::{app_key, Layout, BUILTIN};
use iyakki_agent::store::tilehub;

/// One line of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HlRow {
    /// A group: id, name, how many tiles the person put in it, whether it is a built-in one.
    Group { id: String, name: String, moved: usize, builtin: bool },
    /// A tile: its layout key, its title, the group it is in now, and whether the person moved it.
    Tile { key: String, title: String, group: String, moved: bool },
}

impl SettingsApp {
    fn hl_layout(cx: &AppCx) -> Layout {
        Layout::load(Self::svc(cx).dir(), &cx.session.user)
    }

    fn hl_save(cx: &AppCx, l: &Layout) {
        l.save(Self::svc(cx).dir(), &cx.session.user);
    }

    /// The groups, then the tiles this person sees.
    pub(super) fn hl_rows(cx: &AppCx) -> Vec<HlRow> {
        let l = Self::hl_layout(cx);
        let mut rows: Vec<HlRow> = BUILTIN.iter().map(|(id, name)| HlRow::Group { id: id.to_string(), name: name.to_string(), moved: l.count_in(id), builtin: true }).collect();
        rows.extend(l.groups.iter().map(|g| HlRow::Group { id: g.id.clone(), name: g.label.clone(), moved: l.count_in(&g.id), builtin: false }));
        for (key, label, own) in crate::shell::layout_tiles(cx.session.policy.manage_users, &iyakki_agent::cloudmode::CloudMode::desktop()) {
            let now = l.group_of(&key, own);
            rows.push(HlRow::Tile { moved: l.placed.contains_key(&key), group: l.label_of(&now).unwrap_or_default(), title: label.to_string(), key });
        }
        for e in tilehub::installed(Self::svc(cx), &cx.session.policy, &cx.session.user) {
            let key = app_key(&e.name);
            let own = l.app_default(&e.name, &e.group).0;
            let now = l.group_of(&key, &own);
            rows.push(HlRow::Tile { moved: l.placed.contains_key(&key), group: l.label_of(&now).unwrap_or_default(), title: e.title.clone(), key });
        }
        rows
    }

    pub(super) fn hl_len(cx: &AppCx) -> usize {
        Self::hl_rows(cx).len()
    }

    fn hl_default_of(cx: &AppCx, l: &Layout, key: &str) -> String {
        if let Some((_, _, own)) = crate::shell::layout_tiles(cx.session.policy.manage_users, &iyakki_agent::cloudmode::CloudMode::desktop()).into_iter().find(|t| t.0 == key) {
            return own.to_string();
        }
        tilehub::installed(Self::svc(cx), &cx.session.policy, &cx.session.user).into_iter().find(|e| app_key(&e.name) == key).map(|e| l.app_default(&e.name, &e.group).0).unwrap_or_else(|| "work".into())
    }

    pub(super) fn handle_homelayout(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::hl_rows(cx);
        let cur = rows.get(self.sel()).cloned();
        match (k, cur) {
            (Key::Up, _) => self.move_sel(-1, rows.len()),
            (Key::Down, _) => self.move_sel(1, rows.len()),
            (Key::Char('n'), _) => self.open_form(FormKind::HomeGroup(None), Form::new("New group").text("Name (up to 16 characters)", "", "for example Games")),
            (Key::Char('r'), Some(HlRow::Group { id, name, builtin, .. })) => {
                if builtin {
                    self.fail("The built-in groups keep their names.");
                } else {
                    self.open_form(FormKind::HomeGroup(Some(id)), Form::new("Rename group").text("Name (up to 16 characters)", &name, ""));
                }
            }
            (Key::Char('x'), Some(HlRow::Group { id, name, moved, builtin })) => {
                if builtin {
                    self.fail("The built-in groups cannot be deleted.");
                } else {
                    let text = format!("Delete the group “{name}”? {moved} tile{} go back to their own default group. No app is uninstalled.", if moved == 1 { "" } else { "s" });
                    self.modal = Some(Modal::Confirm(ConfirmKind::HomeDelete(id), text));
                }
            }
            (Key::Char(c @ ('[' | ']')), Some(HlRow::Group { id, builtin, .. })) => {
                if builtin {
                    self.fail("The built-in groups stay in their places; your own groups can be put in order.");
                } else {
                    let mut l = Self::hl_layout(cx);
                    if l.shift_group(&id, *c == '[') {
                        Self::hl_save(cx, &l);
                        // the cursor follows the group
                        self.move_sel(if *c == '[' { -1 } else { 1 }, rows.len());
                        self.ok("Group moved.");
                    }
                }
            }
            (Key::Char('m') | Key::Enter, Some(HlRow::Tile { key, title, .. })) => {
                let l = Self::hl_layout(cx);
                let now = l.group_of(&key, &Self::hl_default_of(cx, &l, &key));
                let mut items: Vec<String> = BUILTIN.iter().map(|(_, n)| n.to_string()).chain(l.groups.iter().map(|g| g.label.clone())).collect();
                items.retain(|n| l.find(n).is_some_and(|id| id != now));
                self.modal = Some(Modal::Pick(PickKind::HomeMove(key), Picker::new(&format!("Move {title} to"), items)));
            }
            (Key::Char('p'), Some(HlRow::Tile { key, title, .. })) => {
                let mut l = Self::hl_layout(cx);
                l.reset_tile(&key);
                Self::hl_save(cx, &l);
                self.ok(format!("{title} is back in its own group."));
            }
            (Key::Char('z'), _) => self.modal = Some(Modal::Confirm(ConfirmKind::HomeReset, "Put every tile back in its own group? Your own groups stay.".into())),
            _ => {}
        }
    }

    pub(super) fn hl_submit(&mut self, cx: &AppCx, editing: &Option<String>, form: &Form) -> Result<String, String> {
        let mut l = Self::hl_layout(cx);
        let name = form.value(0);
        let r = match editing {
            Some(id) => l.rename_group(id, &name).map(|_| format!("Renamed to {}.", name.trim())),
            None => l.create_group(&name).map(|_| format!("Made the group {}.", name.trim())),
        };
        if r.is_ok() {
            Self::hl_save(cx, &l);
            cx.audit.record(&cx.session.user, "layout.group", &name);
        }
        r
    }

    pub(super) fn hl_pick(&mut self, cx: &AppCx, key: &str, item: &str) {
        let mut l = Self::hl_layout(cx);
        let Some(gid) = l.find(item) else { return };
        let own = Self::hl_default_of(cx, &l, key);
        match l.place(key, &gid, &own) {
            Ok(()) => {
                Self::hl_save(cx, &l);
                self.ok(format!("Moved to {item}."));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn hl_confirm(&mut self, cx: &AppCx, kind: &ConfirmKind) {
        let mut l = Self::hl_layout(cx);
        match kind {
            ConfirmKind::HomeDelete(id) => match l.delete_group(id) {
                Ok(n) => {
                    Self::hl_save(cx, &l);
                    cx.audit.record(&cx.session.user, "layout.group.delete", id);
                    self.ok(format!("Group deleted. {n} tile{} went back to {}own group.", if n == 1 { "" } else { "s" }, if n == 1 { "its " } else { "their " }));
                }
                Err(e) => self.fail(e),
            },
            ConfirmKind::HomeReset => {
                l.reset();
                Self::hl_save(cx, &l);
                self.ok("Every tile is back in its own group.");
            }
            _ => {}
        }
    }

    pub(super) fn render_homelayout(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::hl_rows(cx);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 30.0 * s;
        let head_h = 26.0 * s;
        let groups = rows.iter().filter(|r| matches!(r, HlRow::Group { .. })).count();
        // enough rows above the selection are dropped to keep it in view
        let room = ((a.h - 2.0 * head_h) / row_h).floor().max(3.0) as usize;
        let start = sel.saturating_sub(room.saturating_sub(2)).min(rows.len().saturating_sub(1));
        let mut y = a.y;
        let mut last_kind = None;
        for (i, row) in rows.iter().enumerate().skip(start) {
            let is_group = matches!(row, HlRow::Group { .. });
            if last_kind != Some(is_group) {
                let title = if is_group { "Groups (your own can be renamed, deleted and put in order)" } else { "Tiles (the group each one is listed in)" };
                g.text(a.x, y + 4.0 * s, 13.0 * s, DIM, title, Face::Bold);
                y += head_h;
                last_kind = Some(is_group);
            }
            if y + row_h > a.y + a.h {
                break;
            }
            let r = R::new(a.x, y, a.w, row_h - 2.0 * s);
            let on = i == sel;
            if on {
                g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(30, 38, 68));
            }
            g.hit(r, Event::Row(i), "row");
            let (left, right, hot) = match row {
                HlRow::Group { name, moved, builtin, .. } => (name.clone(), if *builtin { format!("built-in · {moved} moved here") } else { format!("your group · {moved} moved here") }, false),
                HlRow::Tile { title, group, moved, .. } => (title.clone(), format!("{group}{}", if *moved { " · moved" } else { "" }), *moved),
            };
            let ty = r.y + (r.h - 14.0 * s) / 2.0 - s;
            let left = g.fit(14.0 * s, &left, Face::Bold, r.w * 0.45);
            g.text(r.x + 10.0 * s, ty, 14.0 * s, if on { TEXT } else { MUTED }, &left, Face::Bold);
            let right = g.fit(13.0 * s, &right, Face::Sans, r.w * 0.5);
            let rw = g.text_width(13.0 * s, &right, Face::Sans);
            g.text(r.x + r.w - rw - 10.0 * s, ty, 13.0 * s, if hot { ACCENT } else { DIM }, &right, Face::Sans);
            y += row_h;
        }
        if groups == 0 {
            g.text(a.x, a.y, 14.0 * s, DIM, "Nothing to arrange.", Face::Sans);
        }
    }

    pub(super) fn homelayout_buttons(&self) -> Vec<Btn> {
        let (acc, mut_) = (ACCENT, TEXT);
        vec![("New group", acc, 'n'), ("Rename", mut_, 'r'), ("Delete group", ERR, 'x'), ("Group earlier", mut_, '['), ("Group later", mut_, ']'), ("Move tile", acc, 'm'), ("Put tile back", mut_, 'p'), ("Reset layout", mut_, 'z')]
    }
}
