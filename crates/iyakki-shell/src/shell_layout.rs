//! The shell's side of the home-screen layout: moving tiles between groups (the `m` key, the right-click menu, picking a tile up and
//! dropping it on a group chip), making, renaming and deleting the person's own groups, the card that asks about an app naming a
//! group that does not exist, and one-step undo. The rules and the stored data are in `iyakki_agent::layout`; this file only
//! draws and asks. Moving a tile changes where it is listed, never what the app may do.

use super::*;
use crate::ctxmenu::{Item, Menu, Outcome};
use iyakki_agent::layout::{app_key, sys_key, Layout};

/// How long the "Moved … Undo" notice stays, in ticks (about ten seconds).
const NOTICE_TICKS: u64 = 300;
/// Events of the buttons this file draws (`Event::Tab(BASE + n)`).
const BASE: usize = 0x4C00;
const BTN_A: usize = BASE + 1;
const BTN_B: usize = BASE + 2;
const UNDO: usize = BASE + 90;
const PLUS: usize = BASE + 91;
const MORE: usize = BASE + 92;

/// A group on the chip row: a built-in one or one the person made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRef {
    pub id: String,
    pub name: String,
}

impl GroupRef {
    pub fn label(&self) -> &str {
        &self.name
    }
}

impl From<Group> for GroupRef {
    fn from(g: Group) -> GroupRef {
        GroupRef { id: g.id().to_string(), name: g.label().to_string() }
    }
}

impl PartialEq<Group> for GroupRef {
    fn eq(&self, g: &Group) -> bool {
        self.id == g.id()
    }
}

/// The key a tile has in the layout (`sys:chat`, `app:calculator`); empty for the hint tile.
pub(super) fn item_key(it: &LItem) -> String {
    match it {
        LItem::Sys(t) => sys_key(t.label),
        LItem::App(e) => app_key(&e.name),
        LItem::Empty => String::new(),
    }
}

#[derive(Clone, Debug)]
pub(super) enum MenuKind {
    /// Pick a group for this tile.
    Move(String),
    /// The tile's own menu.
    Tile(String),
    /// A group chip's menu.
    Chip(String),
    /// The groups that did not fit the chip row.
    More,
}

#[derive(Clone, Debug)]
pub(super) enum NameFor {
    /// A new group, and the tile to put in it when the card came from a move.
    New(Option<String>),
    Rename(String),
}

#[derive(Clone, Debug)]
pub(super) enum Card {
    Name { target: NameFor, text: String, err: String },
    Delete { id: String, label: String, n: usize, focus: usize },
    Ask { app: String, title: String, asked: String, focus: usize },
}

#[derive(Default)]
pub(super) struct LayoutUi {
    pub menu: Option<(MenuKind, Menu)>,
    pub card: Option<Card>,
    /// A tile picked up, waiting to be dropped on a group chip (key).
    pub carry: Option<String>,
    /// The layout before the last change (one step of undo) and the notice that offers it.
    pub undo: Option<Layout>,
    pub notice: Option<(String, u64)>,
}

impl LayoutUi {
    pub fn modal(&self) -> bool {
        self.menu.is_some() || self.card.is_some()
    }
}

/// What a group of the person's own shows while it has no tiles.
pub(super) fn draw_empty_hint(g: &mut Gfx, r: R, s: f32, group: &str) {
    g.rrect_outline(r.x, r.y, r.w, r.h, 18.0 * s, 1.5 * s, rgb(70, 82, 120));
    let size = 14.0 * s;
    let text = format!("Nothing in {group} yet. Select a tile in another group and press m (or right-click it) to move it here.");
    for (i, line) in g.wrap(size, &text, Face::Sans, r.w - 28.0 * s).into_iter().take(7).enumerate() {
        g.text(r.x + 14.0 * s, r.y + 24.0 * s + i as f32 * size * 1.35, size, MUTED, &line, Face::Sans);
    }
}

impl Shell {
    pub(super) fn lay_sign_in(&mut self, remembered: Option<&str>) {
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        self.layout = Layout::load(self.env.services.dir(), &user);
        self.lay = LayoutUi::default();
        self.cgroup = remembered.filter(|id| Group::from_id(id).is_none() && self.layout.groups.iter().any(|g| g.id == *id)).map(str::to_string);
    }

    /// Read the layout file again (Settings → Home layout writes it).
    pub(super) fn lay_reload(&mut self) {
        if let Some(sess) = &self.session {
            self.layout = Layout::load(self.env.services.dir(), &sess.user);
            if self.cgroup.as_ref().is_some_and(|c| !self.layout.exists(c)) {
                self.cgroup = None;
            }
        }
    }

    pub(super) fn lay_sign_out(&mut self) {
        self.layout = Layout::default();
        self.lay = LayoutUi::default();
        self.cgroup = None;
    }

    /// The text of the notice that offers Undo, while it is up (for tests and the drawing).
    pub fn layout_notice(&self) -> Option<&str> {
        self.lay.notice.as_ref().map(|n| n.0.as_str())
    }

    pub(super) fn chip_row(&self) -> launcher::ChipRow {
        let (w, h) = self.size;
        let gs = self.groups();
        let labels: Vec<&str> = gs.iter().map(|g| g.label()).collect();
        let cur = self.current_group();
        let i = gs.iter().position(|g| *g == cur).unwrap_or(0);
        launcher::chip_row(w, scale_for(w, h), &labels, i)
    }

    fn save_layout(&self) {
        if let Some(s) = &self.session {
            self.layout.save(self.env.services.dir(), &s.user);
        }
    }

    /// Keep the change, remember how it was before (one step of undo) and say what happened.
    fn lay_commit(&mut self, old: Layout, msg: String) {
        self.save_layout();
        self.lay.undo = Some(old);
        self.lay.notice = Some((msg, self.tick + NOTICE_TICKS));
        self.stale = true;
        let n = self.group_items().len();
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    fn undo_layout(&mut self) {
        if let Some(old) = self.lay.undo.take() {
            self.layout = old;
            self.save_layout();
            self.lay.notice = None;
            self.say_info("Undone");
            // a shown group of the person's own may be gone now
            if self.cgroup.as_ref().is_some_and(|c| !self.layout.exists(c)) {
                self.cgroup = None;
            }
            let n = self.group_items().len();
            self.sel = self.sel.min(n.saturating_sub(1));
        }
    }

    /// (title, the group it is in when left alone) of a tile by its layout key.
    fn tile_info(&self, key: &str) -> Option<(String, String)> {
        if let Some(t) = TILES.iter().find(|t| t.group.is_some() && sys_key(t.label) == key) {
            return Some((t.label.to_string(), t.group.map_or(String::new(), |g| g.id().to_string())));
        }
        let e = self.custom.iter().find(|e| app_key(&e.name) == key)?;
        Some((e.title.clone(), self.layout.app_default(&e.name, &e.group).0))
    }

    fn selected_key(&self) -> Option<String> {
        let items = self.group_items();
        let k = item_key(items.get(self.sel)?);
        (!k.is_empty()).then_some(k)
    }

    /// Where a tile is now for this person (its group id).
    fn tile_now(&self, key: &str) -> Option<String> {
        let (_, own) = self.tile_info(key)?;
        Some(self.layout.group_of(key, &own))
    }

    pub(super) fn move_tile(&mut self, key: &str, gid: &str) {
        let Some((title, own)) = self.tile_info(key) else { return };
        if self.tile_now(key).as_deref() == Some(gid) {
            self.say_info(format!("{title} is already in {}", self.layout.label_of(gid).unwrap_or_default()));
            return;
        }
        let old = self.layout.clone();
        match self.layout.place(key, gid, &own) {
            Ok(()) => {
                let name = self.layout.label_of(gid).unwrap_or_default();
                self.lay_commit(old, format!("Moved {title} to {name}"));
            }
            Err(e) => self.say(e),
        }
    }

    fn reset_tile(&mut self, key: &str) {
        let Some((title, _)) = self.tile_info(key) else { return };
        let old = self.layout.clone();
        self.layout.reset_tile(key);
        self.lay_commit(old, format!("{title} is back in its own group"));
    }

    fn reset_all(&mut self) {
        let old = self.layout.clone();
        self.layout.reset();
        self.lay_commit(old, "Every tile is back in its own group".to_string());
    }

    fn all_groups(&self) -> Vec<GroupRef> {
        let mut v: Vec<GroupRef> = Group::CHIPS.into_iter().map(GroupRef::from).collect();
        v.extend(self.layout.groups.iter().map(|g| GroupRef { id: g.id.clone(), name: g.label.clone() }));
        v
    }

    fn open_menu(&mut self, kind: MenuKind, items: Vec<Item>, at: (i32, i32)) {
        let (w, h) = self.size;
        self.lay.menu = Some((kind, Menu::open(items, at, (w, h), scale_for(w, h), false)));
        self.stale = true;
    }

    /// The centre of the selected tile (where a menu opened with the keyboard appears).
    fn selected_at(&self) -> (i32, i32) {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let n = self.group_items().len();
        let cap = launcher::per_page(w, h, s);
        let rects = launcher::page_rects(w, h, s, n, self.sel / cap);
        rects.get(self.sel % cap).map_or((w as i32 / 2, h as i32 / 2), |r| ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32))
    }

    fn open_move_menu(&mut self, key: &str, at: (i32, i32)) {
        let now = self.tile_now(key).unwrap_or_default();
        let mut items: Vec<Item> = self.all_groups().into_iter().filter(|g| g.id != now).map(|g| Item::new(&format!("to:{}", g.id), &format!("Move to {}", g.name), "")).collect();
        items.push(Item::separator());
        items.push(Item::new("new", "New group…", ""));
        self.open_menu(MenuKind::Move(key.to_string()), items, at);
    }

    fn open_tile_menu(&mut self, key: &str, at: (i32, i32)) {
        let placed = self.layout.placed.contains_key(key) || self.layout.order.values().any(|v| v.iter().any(|k| k == key));
        let items = vec![
            Item::new("move", "Move to group…", "m"),
            Item::new("pickup", "Pick up, then click a group", ""),
            Item::separator(),
            Item::new("reset-tile", "Reset position", "").enabled(placed),
            Item::new("reset-all", "Reset to default layout", "").enabled(!self.layout.placed.is_empty() || !self.layout.order.is_empty()),
        ];
        self.open_menu(MenuKind::Tile(key.to_string()), items, at);
    }

    fn open_chip_menu(&mut self, gid: &str, at: (i32, i32)) {
        let mut items = Vec::new();
        // only the groups the person made can be renamed, moved or deleted: a built-in group's menu has no such entries
        if !iyakki_agent::layout::is_builtin(gid) {
            items.push(Item::new("rename", "Rename group…", ""));
            items.push(Item::new("delete", "Delete group…", ""));
            items.push(Item::new("left", "Move group left", ""));
            items.push(Item::new("right", "Move group right", ""));
            items.push(Item::separator());
        }
        items.push(Item::new("new", "New group…", ""));
        items.push(Item::new("reset-all", "Reset to default layout", "").enabled(!self.layout.placed.is_empty() || !self.layout.order.is_empty()));
        self.open_menu(MenuKind::Chip(gid.to_string()), items, at);
    }

    fn open_more_menu(&mut self) {
        let row = self.chip_row();
        let gs = self.groups();
        let items: Vec<Item> = row.hidden.iter().map(|i| Item::new(&format!("show:{}", gs[*i].id), &gs[*i].name, "")).collect();
        let at = row.more.map_or((40, 130), |r| (r.x as i32, (r.y + r.h) as i32 + 4));
        self.open_menu(MenuKind::More, items, at);
    }

    fn open_name_card(&mut self, target: NameFor) {
        let text = match &target {
            NameFor::Rename(id) => self.layout.label_of(id).unwrap_or_default(),
            NameFor::New(_) => String::new(),
        };
        self.lay.card = Some(Card::Name { target, text, err: String::new() });
        self.stale = true;
    }

    fn open_delete_card(&mut self, id: &str) {
        let Some(label) = self.layout.label_of(id) else { return };
        let n = self.layout.count_in(id);
        self.lay.card = Some(Card::Delete { id: id.to_string(), label, n, focus: 1 });
        self.stale = true;
    }

    /// A Store app names a group that does not exist (and the person has not been asked about it yet): ask, once.
    pub(super) fn lay_poll(&mut self) {
        if self.lay.modal() || self.session.is_none() || !matches!(self.screen, Screen::Launcher) {
            return;
        }
        let found = self.custom.iter().find_map(|e| {
            let asked = self.layout.app_default(&e.name, &e.group).1?;
            (!self.layout.placed.contains_key(&app_key(&e.name))).then(|| (e.name.clone(), e.title.clone(), asked))
        });
        if let Some((app, title, asked)) = found {
            self.lay.card = Some(Card::Ask { app, title, asked, focus: 0 });
            self.stale = true;
        }
    }

    fn run_pick(&mut self, kind: MenuKind, id: &str) {
        match kind {
            MenuKind::Move(key) => match id {
                "new" => self.open_name_card(NameFor::New(Some(key))),
                _ => {
                    if let Some(gid) = id.strip_prefix("to:") {
                        self.move_tile(&key, gid);
                    }
                }
            },
            MenuKind::Tile(key) => match id {
                "move" => {
                    let at = self.selected_at();
                    self.open_move_menu(&key, at);
                }
                "pickup" => {
                    self.lay.carry = Some(key);
                    self.stale = true;
                }
                "reset-tile" => self.reset_tile(&key),
                "reset-all" => self.reset_all(),
                _ => {}
            },
            MenuKind::Chip(gid) => match id {
                "new" => self.open_name_card(NameFor::New(None)),
                "rename" => self.open_name_card(NameFor::Rename(gid)),
                "delete" => self.open_delete_card(&gid),
                "left" | "right" => {
                    let old = self.layout.clone();
                    if self.layout.shift_group(&gid, id == "left") {
                        self.lay_commit(old, "Moved the group".to_string());
                    }
                }
                "reset-all" => self.reset_all(),
                _ => {}
            },
            MenuKind::More => {
                if let Some(gid) = id.strip_prefix("show:") {
                    let g = self.groups().into_iter().find(|g| g.id == gid);
                    if let Some(g) = g {
                        self.set_group(g);
                    }
                }
            }
        }
    }

    fn submit_name(&mut self) {
        let Some(Card::Name { target, text, .. }) = self.lay.card.clone() else { return };
        let old = self.layout.clone();
        let result: Result<String, String> = match &target {
            NameFor::New(tile) => self.layout.create_group(&text).map(|id| {
                let name = self.layout.label_of(&id).unwrap_or_default();
                match tile.as_ref().and_then(|t| self.tile_info(t).map(|(title, own)| (t.clone(), title, own))) {
                    Some((t, title, own)) => {
                        let _ = self.layout.place(&t, &id, &own);
                        format!("Made the group {name} and moved {title} there|{id}")
                    }
                    None => format!("Made the group {name}|{id}"),
                }
            }),
            NameFor::Rename(id) => self.layout.rename_group(id, &text).map(|_| format!("Renamed the group to {}|{id}", text.trim())),
        };
        match result {
            Ok(m) => {
                let (msg, id) = m.split_once('|').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
                self.lay.card = None;
                self.lay_commit(old, msg);
                if matches!(target, NameFor::New(_)) {
                    if let Some(g) = self.groups().into_iter().find(|g| g.id == id) {
                        self.set_group(g);
                    }
                }
            }
            Err(e) => {
                self.layout = old;
                if let Some(Card::Name { err, .. }) = self.lay.card.as_mut() {
                    *err = e;
                }
                self.stale = true;
            }
        }
    }

    fn confirm_delete(&mut self, id: &str) {
        let old = self.layout.clone();
        let label = self.layout.label_of(id).unwrap_or_default();
        match self.layout.delete_group(id) {
            Ok(n) => {
                self.lay.card = None;
                if self.cgroup.as_deref() == Some(id) {
                    self.cgroup = None;
                    self.sel = 0;
                }
                let msg = match n {
                    0 => format!("Deleted the group {label}"),
                    1 => format!("Deleted the group {label}. 1 tile went back to its own group"),
                    n => format!("Deleted the group {label}. {n} tiles went back to their own group"),
                };
                self.lay_commit(old, msg);
            }
            Err(e) => {
                self.lay.card = None;
                self.say(e);
            }
        }
    }

    fn answer_ask(&mut self, create: bool) {
        let Some(Card::Ask { app, title, asked, .. }) = self.lay.card.take() else { return };
        let (target, note) = self.layout.answer_app_group(&app, &asked, create);
        self.save_layout();
        self.stale = true;
        let name = self.layout.label_of(&target).unwrap_or_else(|| "Apps".into());
        match note {
            Some(why) => self.say(format!("{title} is in {name}: {why}")),
            None => self.say_info(format!("{title} is in {name}")),
        }
    }

    /// A right click on the launcher: over a tile or a chip it opens that one's menu. False: not ours (the usual menu opens).
    pub(super) fn layout_right_click(&mut self, x: i32, y: i32) -> bool {
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher) || self.help || self.brand.open || self.devpop.open || self.installs.open || self.startup.open || self.lay.modal() {
            return false;
        }
        if self.lay.carry.take().is_some() {
            self.stale = true;
        }
        let row = self.chip_row();
        let gs = self.groups();
        if let Some(i) = row.chips.iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| *i) {
            let gid = gs[i].id.clone();
            self.open_chip_menu(&gid, (x, y));
            return true;
        }
        let items = self.group_items();
        let (w, h) = self.size;
        let s = scale_for(w, h);
        let cap = launcher::per_page(w, h, s);
        let page = self.sel.min(items.len() - 1) / cap;
        let rects = launcher::page_rects(w, h, s, items.len(), page);
        if let Some(k) = rects.iter().position(|r| r.contains(x, y)) {
            let i = page * cap + k;
            if let Some(key) = items.get(i).map(item_key).filter(|k| !k.is_empty()) {
                self.sel = i;
                self.open_tile_menu(&key, (x, y));
                return true;
            }
        }
        false
    }

    /// Keys and clicks for the layout menus and cards, and the keys that open them. True: the event was handled here.
    pub(super) fn layout_intercept(&mut self, ev: &Event) -> bool {
        if let Some((_, until)) = &self.lay.notice {
            if *until <= self.tick {
                self.lay.notice = None;
                self.lay.undo = None;
                self.stale = true;
            }
        }
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher) || self.clip.is_some() || self.help || self.ring.is_some() {
            return false;
        }
        if self.brand.open || self.devpop.open || self.installs.open || self.startup.open || self.vol.open || self.login.as_ref().is_some_and(|m| !m.hidden()) {
            return false;
        }
        if matches!(ev, Event::Tick | Event::MouseUp(..) | Event::MouseMove(..)) {
            if let (Event::MouseMove(x, y), Some((_, m))) = (ev, self.lay.menu.as_mut()) {
                let before = m.sel;
                m.hover(*x, *y);
                if m.sel != before {
                    self.stale = true;
                }
            }
            return false;
        }
        if let Some(card) = self.lay.card.clone() {
            self.card_event(card, ev);
            self.stale = true;
            return true;
        }
        if self.lay.menu.is_some() {
            self.menu_event(ev);
            self.stale = true;
            return true;
        }
        // an offered Undo
        if self.lay.notice.is_some() && self.lay.undo.is_some() && matches!(ev, Event::Key(Key::Char('u')) | Event::Tab(UNDO)) {
            self.undo_layout();
            return true;
        }
        // a tile picked up: a group chip takes it; Enter drops it in the shown group; Esc or any other click puts it back
        if let Some(key) = self.lay.carry.clone() {
            match ev {
                Event::Key(Key::Esc) => {
                    self.lay.carry = None;
                    self.stale = true;
                    return true;
                }
                Event::Key(Key::Enter) => {
                    self.lay.carry = None;
                    let gid = self.current_group().id;
                    self.move_tile(&key, &gid);
                    return true;
                }
                Event::MouseDown(x, y) => {
                    self.lay.carry = None;
                    self.stale = true;
                    let row = self.chip_row();
                    let gs = self.groups();
                    if let Some(i) = row.chips.iter().find(|(_, r)| r.contains(*x, *y)).map(|(i, _)| *i) {
                        let gid = gs[i].id.clone();
                        self.move_tile(&key, &gid);
                    } else if row.plus.contains(*x, *y) {
                        self.open_name_card(NameFor::New(Some(key)));
                    }
                    return true;
                }
                Event::Tab(PLUS) => {
                    self.lay.carry = None;
                    self.open_name_card(NameFor::New(Some(key)));
                    return true;
                }
                _ => {}
            }
        }
        match ev {
            Event::Tab(PLUS) => {
                self.open_name_card(NameFor::New(None));
                true
            }
            Event::Tab(MORE) => {
                self.open_more_menu();
                true
            }
            Event::Key(Key::Char('m')) => match self.selected_key() {
                Some(k) => {
                    let at = self.selected_at();
                    self.open_move_menu(&k, at);
                    true
                }
                None => false,
            },
            Event::Key(Key::Char('g')) => {
                let gid = self.current_group().id;
                let row = self.chip_row();
                let at = row.chips.iter().find(|(i, _)| self.groups().get(*i).is_some_and(|g| g.id == gid)).map_or((40, 130), |(_, r)| (r.x as i32, (r.y + r.h) as i32 + 4));
                self.open_chip_menu(&gid, at);
                true
            }
            Event::Key(Key::Char(c @ ('[' | ']'))) => {
                let Some(key) = self.selected_key() else { return false };
                let gid = self.current_group().id;
                let keys: Vec<String> = self.group_items().iter().map(item_key).collect();
                let old = self.layout.clone();
                let earlier = *c == '[';
                if self.layout.reorder(&gid, &key, earlier, keys) {
                    let title = self.tile_info(&key).map(|t| t.0).unwrap_or_default();
                    self.sel = if earlier { self.sel.saturating_sub(1) } else { self.sel + 1 };
                    self.lay_commit(old, format!("Moved {title} {}", if earlier { "earlier" } else { "later" }));
                }
                true
            }
            _ => false,
        }
    }

    fn menu_event(&mut self, ev: &Event) {
        let Some((kind, m)) = self.lay.menu.as_mut() else { return };
        let out = match ev {
            Event::Key(k) => m.key(k),
            Event::MouseDown(x, y) => m.click(*x, *y),
            Event::MouseRight(..) | Event::Scroll(_) => Outcome::Close,
            _ => Outcome::Stay,
        };
        let kind = kind.clone();
        match out {
            Outcome::Stay => {}
            Outcome::Close => self.lay.menu = None,
            Outcome::Pick(id) => {
                self.lay.menu = None;
                self.run_pick(kind, &id);
            }
        }
    }

    fn card_event(&mut self, card: Card, ev: &Event) {
        match card {
            Card::Name { .. } => {
                let Some(Card::Name { text, err, .. }) = self.lay.card.as_mut() else { return };
                match ev {
                    Event::Key(Key::Esc) | Event::Tab(BTN_B) => self.lay.card = None,
                    Event::Key(Key::Enter) | Event::Tab(BTN_A) => self.submit_name(),
                    Event::Key(Key::Backspace) => {
                        text.pop();
                        err.clear();
                    }
                    Event::Key(Key::Char(c)) if !c.is_control() && text.chars().count() < 30 => {
                        text.push(*c);
                        err.clear();
                    }
                    Event::Paste(t) => {
                        let line: String = t.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
                        text.push_str(&line);
                        *text = text.chars().take(30).collect();
                        err.clear();
                    }
                    _ => {}
                }
            }
            Card::Delete { id, .. } | Card::Ask { app: id, .. } => {
                let is_delete = matches!(self.lay.card, Some(Card::Delete { .. }));
                let focus = match self.lay.card.as_mut() {
                    Some(Card::Delete { focus, .. } | Card::Ask { focus, .. }) => focus,
                    _ => return,
                };
                let activate = match ev {
                    Event::Key(Key::Left | Key::Right | Key::Tab | Key::BackTab | Key::Up | Key::Down) => {
                        *focus = 1 - *focus;
                        None
                    }
                    Event::Key(Key::Enter) => Some(*focus),
                    Event::Tab(BTN_A) => Some(0),
                    Event::Tab(BTN_B) => Some(1),
                    // Esc: leave the group alone / put the app in Work
                    Event::Key(Key::Esc) => Some(1),
                    _ => None,
                };
                match (activate, is_delete) {
                    (Some(0), true) => self.confirm_delete(&id),
                    (Some(_), true) => self.lay.card = None,
                    (Some(0), false) => self.answer_ask(true),
                    (Some(_), false) => self.answer_ask(false),
                    (None, _) => {}
                }
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    /// The `…` and `+` chips at the end of the chip row.
    pub(super) fn draw_extra_chips(&mut self, g: &mut Gfx, row: &launcher::ChipRow, s: f32) {
        let draw = |g: &mut Gfx, r: R, glyph: &str, id: usize, label: &str| {
            let hover = g.hovered(r);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, r.h / 2.0, false, hover, crate::fx::CYAN);
            let lw = g.text_width(18.0 * s, glyph, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 18.0 * s) / 2.0 - s, 18.0 * s, MUTED, glyph, Face::Bold);
            g.hit(r, Event::Tab(id), label);
        };
        if let Some(r) = row.more {
            draw(g, r, "…", MORE, "more groups");
        }
        draw(g, row.plus, "+", PLUS, "new group");
    }

    pub(super) fn draw_layout_ui(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher) {
            return;
        }
        let (wf, hf) = (w as f32, h as f32);
        if let Some(key) = &self.lay.carry {
            let title = self.tile_info(key).map(|t| t.0).unwrap_or_default();
            let text = format!("Moving {title}: click a group above (or + for a new one), Enter drops it in this group, Esc cancels");
            let text = g.fit(15.0 * s, &text, Face::Bold, wf - 60.0 * s);
            let tw = g.text_width(15.0 * s, &text, Face::Bold) + 36.0 * s;
            let r = R::new((wf - tw) / 2.0, hf - 150.0 * s, tw, 36.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(20, 40, 56));
            g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, crate::fx::CYAN);
            g.text(r.x + 18.0 * s, r.y + 9.0 * s, 15.0 * s, TEXT, &text, Face::Bold);
        }
        if let (Some((msg, _)), true) = (&self.lay.notice, self.lay.undo.is_some()) {
            let msg = g.fit(16.0 * s, msg, Face::Sans, wf - 200.0 * s);
            let mw = g.text_width(16.0 * s, &msg, Face::Sans);
            let bw = 78.0 * s;
            let tw = mw + bw + 56.0 * s;
            let r = R::new((wf - tw) / 2.0, hf - 100.0 * s, tw, 44.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(14, 40, 36));
            g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, crate::ui::OK);
            g.text(r.x + 20.0 * s, r.y + 12.0 * s, 16.0 * s, TEXT, &msg, Face::Sans);
            let b = R::new(r.x + r.w - bw - 6.0 * s, r.y + 6.0 * s, bw, r.h - 12.0 * s);
            let hot = g.hovered(b);
            let c = crate::fx::pill(g, b, s, crate::fx::CYAN, hot, true);
            let lw = g.text_width(15.0 * s, "Undo", Face::Bold);
            g.text(b.x + (b.w - lw) / 2.0, b.y + (b.h - 15.0 * s) / 2.0 - s, 15.0 * s, c, "Undo", Face::Bold);
            g.hit(b, Event::Tab(UNDO), "Undo");
        }
        if let Some((_, m)) = &self.lay.menu {
            m.draw(g, s, crate::fx::CYAN);
        }
        let Some(card) = self.lay.card.clone() else { return };
        let area = R::new(0.0, 0.0, wf, hf);
        match card {
            Card::Name { target, text, err } => {
                let title = if matches!(target, NameFor::Rename(_)) { "Rename group" } else { "New group" };
                let p = crate::fx::modal(g, area, s, 520.0 * s, 250.0 * s, crate::fx::CYAN, false);
                g.text(p.x + 24.0 * s, p.y + 20.0 * s, 22.0 * s, TEXT, title, Face::Bold);
                g.text(p.x + 24.0 * s, p.y + 54.0 * s, 14.0 * s, MUTED, "Up to 16 characters. Enter saves, Esc cancels.", Face::Sans);
                let f = R::new(p.x + 24.0 * s, p.y + 84.0 * s, p.w - 48.0 * s, 44.0 * s);
                g.rrect(f.x, f.y, f.w, f.h, 10.0 * s, rgb(12, 16, 32));
                g.rrect_outline(f.x, f.y, f.w, f.h, 10.0 * s, 1.5 * s, crate::fx::CYAN);
                let shown = g.fit(18.0 * s, &text, Face::Sans, f.w - 40.0 * s);
                let tw = g.text_width(18.0 * s, &shown, Face::Sans);
                g.text(f.x + 14.0 * s, f.y + 11.0 * s, 18.0 * s, TEXT, &shown, Face::Sans);
                if self.tick % 30 < 20 {
                    g.rect(f.x + 16.0 * s + tw, f.y + 9.0 * s, 2.0 * s, f.h - 18.0 * s, crate::fx::CYAN);
                }
                let n = text.chars().count();
                let count = format!("{n}/16");
                let cw = g.text_width(12.0 * s, &count, Face::Sans);
                g.text(f.x + f.w - cw, f.y + f.h + 4.0 * s, 12.0 * s, if n > 16 { crate::ui::ERR } else { DIM }, &count, Face::Sans);
                if !err.is_empty() {
                    let e = g.fit(14.0 * s, &err, Face::Sans, f.w - 60.0 * s);
                    g.text(f.x, f.y + f.h + 4.0 * s, 14.0 * s, crate::ui::ERR, &e, Face::Sans);
                }
                self.card_buttons(g, p, s, if matches!(target, NameFor::Rename(_)) { "Rename" } else { "Create" }, "Cancel", 0, false);
            }
            Card::Delete { label, n, focus, .. } => {
                let p = crate::fx::modal(g, area, s, 540.0 * s, 280.0 * s, crate::ui::ERR, true);
                g.text(p.x + 24.0 * s, p.y + 20.0 * s, 22.0 * s, TEXT, &format!("Delete the group “{label}”?"), Face::Bold);
                let body = match n {
                    0 => "No tile is listed in it. No app is uninstalled.".to_string(),
                    1 => "1 tile goes back to its own default group. No app is uninstalled and nothing it may do changes.".to_string(),
                    n => format!("{n} tiles go back to their own default group. No app is uninstalled and nothing they may do changes."),
                };
                for (i, line) in g.wrap(15.0 * s, &body, Face::Sans, p.w - 48.0 * s).into_iter().enumerate() {
                    g.text(p.x + 24.0 * s, p.y + 62.0 * s + i as f32 * 22.0 * s, 15.0 * s, MUTED, &line, Face::Sans);
                }
                self.card_buttons(g, p, s, "Delete group", "Keep it", focus, true);
            }
            Card::Ask { title, asked, focus, .. } => {
                let p = crate::fx::modal(g, area, s, 560.0 * s, 280.0 * s, crate::fx::CYAN, false);
                let t = g.fit(22.0 * s, &title, Face::Bold, p.w - 48.0 * s);
                g.text(p.x + 24.0 * s, p.y + 20.0 * s, 22.0 * s, TEXT, &t, Face::Bold);
                let body = format!("This app wants to be in the group “{asked}”. Create it? You are asked once; you can move the app any time.");
                for (i, line) in g.wrap(15.0 * s, &body, Face::Sans, p.w - 48.0 * s).into_iter().enumerate() {
                    g.text(p.x + 24.0 * s, p.y + 62.0 * s + i as f32 * 22.0 * s, 15.0 * s, MUTED, &line, Face::Sans);
                }
                self.card_buttons(g, p, s, "Create", "Put it in Apps", focus, false);
            }
        }
    }

    /// The two buttons at the bottom of a card (`BTN_A` is the left one). `focus` picks the one Enter presses.
    fn card_buttons(&mut self, g: &mut Gfx, p: R, s: f32, a: &str, b: &str, focus: usize, danger: bool) {
        let bw = ((p.w - 24.0 * s * 2.0 - 12.0 * s) / 2.0).min(220.0 * s);
        let y = p.y + p.h - 64.0 * s;
        for (i, (label, id)) in [(a, BTN_A), (b, BTN_B)].into_iter().enumerate() {
            let r = R::new(p.x + p.w - 24.0 * s - bw * (2 - i) as f32 - 12.0 * s * (1 - i) as f32, y, bw, 44.0 * s);
            let hot = g.hovered(r) || focus == i;
            let accent = if danger && i == 0 { crate::ui::ERR } else { crate::fx::CYAN };
            let c = crate::fx::pill(g, r, s, accent, hot, true);
            let lw = g.text_width(16.0 * s, label, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, r.y + (r.h - 16.0 * s) / 2.0 - s, 16.0 * s, c, label, Face::Bold);
            g.hit(r, Event::Tab(id), label);
        }
    }
}
