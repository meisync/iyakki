//! The STORE tile's management layer: updating, rolling back, pinning and removing what is installed (the lifecycle rules are
//! `iyakki_agent::store::lifecycle`), and arranging it the way the person likes (tags, groups, display titles, drag and drop;
//! `iyakki_agent::store::organize`). It is a child of `store.rs` so it can use the tile's own state.

use iyakki_agent::store::installed::{Entry as IEntry, Status, Type as IType};
use iyakki_agent::store::lifecycle::{Life, LifeError, RemoveOpts, RemovePlan, Report, UpdatePlan};
use iyakki_agent::store::organize::{self as org, Org, COLOURS};
use iyakki_agent::store::Store;

use super::{blink, Ask, Done, Job, Mode, Purpose, ReviewView, Section, StoreApp, ACCENT, AMBER};
use crate::apps::AppCx;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, Nav, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// How far the pointer must move with the button down before a press becomes a drag (pixels). A plain click never drags.
pub(super) const DRAG_THRESHOLD: i32 = 8;

pub(super) const TAB_UPDATES: usize = 227;
pub(super) const TAB_VIEW: usize = 231;
pub(super) const TAB_TAGS: usize = 232;
pub(super) const TAB_UPDATE_ALL: usize = 310;
pub(super) const TAB_TAG_CHIP: usize = 2600;
pub(super) const TAB_HEAD: usize = 4000;
pub(super) const TAB_MENU: usize = 5000;
pub(super) const TAB_SUGGEST: usize = 5100;
pub(super) const TAB_TAGROW: usize = 5200;
pub(super) const ACT_ROLLBACK: usize = 4;
pub(super) const ACT_PIN: usize = 5;
pub(super) const ACT_RENAME: usize = 6;
pub(super) const ACT_TAGS: usize = 7;
pub(super) const ACT_MOVE: usize = 8;

// ------------------------------------------------------------------------------------------------ the views

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    Flat,
    Groups,
    Category,
    Kind,
    Source,
}

impl View {
    pub(super) const ALL: [View; 5] = [View::Flat, View::Groups, View::Category, View::Kind, View::Source];

    pub(super) fn id(self) -> &'static str {
        match self {
            View::Flat => "flat",
            View::Groups => "groups",
            View::Category => "category",
            View::Kind => "kind",
            View::Source => "source",
        }
    }

    pub(super) fn from_id(s: &str) -> View {
        View::ALL.into_iter().find(|v| v.id() == s).unwrap_or(View::Flat)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            View::Flat => "None",
            View::Groups => "My groups",
            View::Category => "Category",
            View::Kind => "Kind",
            View::Source => "Source",
        }
    }

    pub(super) fn next(self) -> View {
        View::ALL[(View::ALL.iter().position(|v| *v == self).unwrap_or(0) + 1) % View::ALL.len()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HeadKind {
    /// One of the person's groups.
    Group,
    /// The default place of everything not in a group.
    Ungrouped,
    /// A heading of Category, Kind or Source.
    Plain,
}

#[derive(Clone, Debug)]
pub(super) struct Head {
    /// The group id (groups), or the label (plain headings); None for Ungrouped.
    pub id: Option<String>,
    pub label: String,
    pub count: usize,
    pub colour: Option<[u8; 3]>,
    pub collapsed: bool,
    pub kind: HeadKind,
}

/// One thing that can be listed, with what grouping needs to know about it.
pub(super) struct Cand {
    pub id: String,
    pub okey: String,
    pub cat: String,
    pub kind: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(super) enum DRow {
    Head(Head),
    /// An installed item, by its entry key.
    Item(String),
}

// ------------------------------------------------------------------------------------------------ dragging

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ZoneKind {
    Item { key: String, group: Option<String> },
    Head { group: Option<String>, kind: HeadKind },
}

#[derive(Clone, Debug)]
pub(super) struct Zone {
    pub rect: R,
    pub what: ZoneKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Press {
    /// A row's ordinal.
    Item(usize),
    /// A heading's index in the list of headings.
    Head(usize),
}

#[derive(Clone, Debug)]
pub(super) struct Drag {
    pub press: Press,
    pub start: (i32, i32),
    pub active: bool,
    /// What is being carried: an item key, or a group id.
    pub item: Option<String>,
    pub group: Option<String>,
    pub label: String,
}

// ------------------------------------------------------------------------------------------------ small dialogs

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    Tags(String),
    Alias(String),
    /// A new group; `then_move` is an item to put in it at once.
    NewGroup { then_move: Option<String> },
    RenameGroup(String),
    RenameTag(String),
    ImportPath { replace: bool },
}

pub(super) struct EditState {
    pub kind: EditKind,
    pub title: String,
    pub hint: String,
    pub field: TextField,
    pub sugg_sel: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MenuAct {
    MoveTo(Option<String>),
    NewGroupAndMove,
    NewGroup,
    RenameGroup(String),
    GroupUp(String),
    DeleteGroup(String),
    GroupBy(View),
    ManageTags,
    Export,
    Import(bool),
}

pub(super) struct MenuState {
    pub title: String,
    pub rows: Vec<(String, MenuAct)>,
    pub sel: usize,
    /// The item a move applies to.
    pub item: Option<String>,
}

pub(super) struct TagsState {
    pub sel: usize,
}

pub(super) struct RemoveUi {
    pub plan: RemovePlan,
    pub entry: IEntry,
    /// Typing the name to confirm deleting data.
    pub typing: Option<TextField>,
}

pub(super) fn colour(c: [u8; 3]) -> [u8; 4] {
    rgb(c[0], c[1], c[2])
}

// ------------------------------------------------------------------------------------------------ the list: what is shown

impl StoreApp {
    pub(super) fn okey(e: &IEntry) -> String {
        org::item_key(&e.kind, &e.name)
    }

    /// The title to show for an entry: the person's own, or the real one.
    pub(super) fn shown_title<'a>(&'a self, e: &'a IEntry) -> &'a str {
        self.org.display_title(&Self::okey(e), &e.title)
    }

    pub(super) fn life_of(&self, key: &str) -> Option<&Life> {
        self.lives.iter().find(|l| l.entry.key == key)
    }

    /// Read the person's arrangement again, forget what is no longer installed or listed, and take their grouping choice.
    pub(super) fn reload_org(&mut self, cx: &AppCx) {
        self.org = Org::load(self.store.dir(), &cx.session.user);
        let mut existing: std::collections::HashSet<String> = self.inst.iter().map(Self::okey).collect();
        existing.extend(self.all_items.iter().map(|i| org::item_key(&i.entry.kind, &i.entry.name)));
        // only prune when there is something to compare with (a failed read of the stores must not wipe the arrangement)
        if !existing.is_empty() && self.org.prune(&existing) > 0 {
            let _ = self.org.save();
        }
        self.iview = View::from_id(&self.org.view);
        self.org_rev += 1;
    }

    pub(super) fn org_save(&mut self) {
        self.org_rev += 1;
        if let Err(e) = self.org.save() {
            self.say(format!("Could not save your arrangement: {e}"), false);
        }
    }

    fn matches_query(&self, e: &IEntry, words: &[String]) -> bool {
        if words.is_empty() {
            return true;
        }
        let hay = format!("{} {}", iyakki_agent::store::installed::hay(e), self.org.hay(&Self::okey(e)));
        words.iter().all(|w| hay.contains(w))
    }

    /// The entries that pass the type chip, the Updates chip, the tag filters and the search, in the chosen order.
    pub(super) fn ifiltered_flat(&self) -> Vec<&IEntry> {
        let words: Vec<String> = self.iquery.value.to_lowercase().split_whitespace().map(str::to_string).collect();
        iyakki_agent::store::installed::filter(&self.inst, self.ifilter, "", self.isort)
            .into_iter()
            .filter(|e| !self.iupdates || matches!(e.status, Status::Update(_)))
            .filter(|e| self.itags.iter().all(|t| self.org.tags_of(&Self::okey(e)).contains(t)))
            .filter(|e| self.matches_query(e, &words))
            .collect()
    }

    /// Everything the Installed list shows, in order: headings (when grouped) and items. Collapsed groups show only their heading.
    pub(super) fn display(&self) -> Vec<DRow> {
        let cands: Vec<Cand> = self
            .ifiltered_flat()
            .iter()
            .map(|e| Cand { id: e.key.clone(), okey: Self::okey(e), cat: e.categories.first().cloned().unwrap_or_default(), kind: e.ty.one().to_string(), source: e.source.clone() })
            .collect();
        // empty groups stay as places to drop things, unless a search or a tag is narrowing the list
        let narrowing = (!self.iquery.value.trim().is_empty() || !self.itags.is_empty() || self.iupdates) && !self.drag.as_ref().is_some_and(|d| d.active);
        self.build_display(cands, narrowing)
    }

    /// Group candidates the way the person chose: My groups (their order), Category, Kind or Source. Used by both lists.
    pub(super) fn build_display(&self, cands: Vec<Cand>, hide_empty: bool) -> Vec<DRow> {
        let mut out = Vec::new();
        match self.iview {
            View::Flat => out.extend(cands.iter().map(|c| DRow::Item(c.id.clone()))),
            View::Groups => {
                let place = |out: &mut Vec<DRow>, g: Option<&org::Group>| {
                    let gid = g.map(|g| g.id.as_str());
                    let mine: Vec<&Cand> = cands.iter().filter(|c| self.org.group_id_of(&c.okey) == gid).collect();
                    let arranged = self.org.arrange(gid, mine.iter().map(|c| c.okey.clone()).collect());
                    let ordered: Vec<String> = arranged.iter().filter_map(|ok| mine.iter().find(|c| c.okey == *ok).map(|c| c.id.clone())).collect();
                    if ordered.is_empty() && hide_empty {
                        return;
                    }
                    let collapsed = g.map_or(self.collapsed_ungrouped, |g| g.collapsed);
                    out.push(DRow::Head(Head {
                        id: g.map(|g| g.id.clone()),
                        label: g.map_or("Ungrouped".to_string(), |g| g.name.clone()),
                        count: ordered.len(),
                        colour: g.map(|g| COLOURS[g.colour as usize % COLOURS.len()].1),
                        collapsed,
                        kind: if g.is_some() { HeadKind::Group } else { HeadKind::Ungrouped },
                    }));
                    if !collapsed {
                        out.extend(ordered.into_iter().map(DRow::Item));
                    }
                };
                for g in &self.org.groups {
                    place(&mut out, Some(g));
                }
                place(&mut out, None);
            }
            View::Category | View::Kind | View::Source => {
                let label = |c: &Cand| -> String {
                    match self.iview {
                        View::Category => if c.cat.is_empty() { "Uncategorised".into() } else { c.cat.clone() },
                        View::Kind => c.kind.clone(),
                        _ => c.source.clone(),
                    }
                };
                let mut names: Vec<String> = cands.iter().map(label).collect();
                names.sort();
                names.dedup();
                // "Uncategorised" and the system's own tiles go last
                names.sort_by_key(|n| n == "Uncategorised" || n == "System app" || n.starts_with("built into"));
                for n in names {
                    let mine: Vec<String> = cands.iter().filter(|c| label(c) == n).map(|c| c.id.clone()).collect();
                    let collapsed = self.collapsed_heads.contains(&n);
                    out.push(DRow::Head(Head { id: Some(n.clone()), label: n, count: mine.len(), colour: None, collapsed, kind: HeadKind::Plain }));
                    if !collapsed {
                        out.extend(mine.into_iter().map(DRow::Item));
                    }
                }
            }
        }
        out
    }

    /// The items in the order the list shows them (what selection ordinals refer to).
    pub(super) fn ifiltered(&self) -> Vec<&IEntry> {
        self.display()
            .into_iter()
            .filter_map(|r| if let DRow::Item(k) = r { self.inst.iter().find(|e| e.key == k) } else { None })
            .collect()
    }

    /// What the list of the current page shows (Installed or Discover).
    pub(super) fn cur_display(&self) -> Vec<DRow> {
        if self.section == Section::Discover { self.discover_display() } else { self.display() }
    }

    pub(super) fn discover_display(&self) -> Vec<DRow> {
        let cands: Vec<Cand> = self
            .filtered()
            .into_iter()
            .map(|i| {
                let it = &self.items[i];
                let k = org::item_key(&it.entry.kind, &it.entry.name);
                Cand { id: k.clone(), okey: k, cat: it.entry.meta.categories.first().cloned().unwrap_or_default(), kind: super::kind_label(&it.entry.kind).to_string(), source: it.source_name.clone() }
            })
            .collect();
        let narrowing = (!self.filter.query.trim().is_empty() || !self.itags.is_empty()) && !self.drag.as_ref().is_some_and(|d| d.active);
        self.build_display(cands, narrowing)
    }

    pub(super) fn heads(&self) -> Vec<Head> {
        self.cur_display().into_iter().filter_map(|r| if let DRow::Head(h) = r { Some(h) } else { None }).collect()
    }

    /// The row id of a listed thing as its organisation key.
    fn okey_of_id(&self, id: &str) -> String {
        if self.section == Section::Discover { id.to_string() } else { self.inst.iter().find(|e| e.key == id).map(Self::okey).unwrap_or_default() }
    }

    /// What the thing at this ordinal of the current list is: its organisation key and real title.
    fn item_info(&self, ord: usize) -> Option<(String, String)> {
        let id = self.cur_display().into_iter().filter_map(|r| if let DRow::Item(k) = r { Some(k) } else { None }).nth(ord)?;
        if self.section == Section::Discover {
            let (kind, name) = id.split_once(':')?;
            let it = self.items.iter().find(|i| i.entry.kind == kind && i.entry.name == name)?;
            return Some((id.clone(), it.title().to_string()));
        }
        self.inst.iter().find(|e| e.key == id).map(|e| (Self::okey(e), e.title.clone()))
    }

    /// The thing under the cursor on the current page (or open on its page), for arranging.
    pub(super) fn target(&self) -> Option<(String, String)> {
        if self.section == Section::Discover {
            if let Some((k, n)) = &self.detail {
                let it = self.items.iter().find(|i| i.entry.kind == *k && i.entry.name == *n)?;
                return Some((org::item_key(k, n), it.title().to_string()));
            }
            return self.item_info(self.sel[0]);
        }
        if self.on_head.is_some() && self.idetail.is_none() {
            return None;
        }
        if let Some(k) = &self.idetail {
            return self.inst.iter().find(|e| e.key == *k).map(|e| (Self::okey(e), e.title.clone()));
        }
        self.item_info(self.sel[1])
    }

    fn sel_index(&self) -> usize {
        if self.section == Section::Discover { 0 } else { 1 }
    }

    fn open_current(&mut self) {
        if self.section == Section::Discover {
            self.open_detail();
        } else {
            self.open_idetail();
        }
    }

    /// The cursor moves over headings too when the list is grouped by the person's own groups.
    fn nav_list(&self) -> Vec<Nav2> {
        let mut n = 0;
        let mut out = Vec::new();
        for r in self.display() {
            match r {
                DRow::Head(h) if self.iview == View::Groups => out.push(Nav2::Head(h.id.clone())),
                DRow::Head(_) => {}
                DRow::Item(_) => {
                    out.push(Nav2::Item(n));
                    n += 1;
                }
            }
        }
        out
    }

    /// Move the cursor by `delta` places (over headings too in My groups).
    pub(super) fn nav_move(&mut self, delta: i32) {
        let nav = self.nav_list();
        if nav.is_empty() {
            return;
        }
        let here = match &self.on_head {
            Some(h) => nav.iter().position(|n| *n == Nav2::Head(h.clone())),
            None => nav.iter().position(|n| *n == Nav2::Item(self.sel[1])),
        }
        .unwrap_or(0);
        let to = (here as i32 + delta).clamp(0, nav.len() as i32 - 1) as usize;
        match &nav[to] {
            Nav2::Head(h) => self.on_head = Some(h.clone()),
            Nav2::Item(i) => {
                self.on_head = None;
                self.sel[1] = *i;
            }
        }
    }

    // -------------------------------------------------------------------------------------- pointer: press, drag, drop

    pub(super) fn pointer_move(&mut self, x: i32, y: i32) {
        self.pointer = Some((x, y));
        if let Some(d) = &mut self.drag {
            if !d.active && ((x - d.start.0).abs() >= DRAG_THRESHOLD || (y - d.start.1).abs() >= DRAG_THRESHOLD) {
                d.active = true;
            }
            if d.active {
                self.redraw = true;
            }
        }
    }

    /// A press on a row or heading. With a real pointer the click is decided on release (so it can become a drag); without one
    /// (the keyboard, tests) it acts at once.
    pub(super) fn press(&mut self, p: Press, cx: &AppCx) {
        let si = self.sel_index();
        let carried = match &p {
            Press::Item(i) => self.item_info(*i).map(|(k, t)| {
                let shown = self.org.display_title(&k, &t).to_string();
                (Some(k), None, shown)
            }),
            Press::Head(i) => self.heads().get(*i).map(|h| (None, if h.kind == HeadKind::Group { h.id.clone() } else { None }, h.label.clone())),
        };
        let Some((item, group, label)) = carried else { return };
        match (&p, self.pointer) {
            (_, Some(at)) => self.drag = Some(Drag { press: p.clone(), start: at, active: false, item, group, label }),
            (Press::Item(i), None) => {
                self.sel[si] = *i;
                self.on_head = None;
                self.open_current();
            }
            (Press::Head(i), None) => self.head_click(*i, cx),
        }
        if let Press::Item(i) = &p {
            self.sel[si] = *i;
            self.on_head = None;
        }
    }

    fn head_click(&mut self, i: usize, cx: &AppCx) {
        let Some(h) = self.heads().get(i).cloned() else { return };
        self.on_head = (self.iview == View::Groups && self.section == Section::Installed).then(|| h.id.clone());
        self.toggle_head(&h, cx);
    }

    pub(super) fn toggle_head(&mut self, h: &Head, _cx: &AppCx) {
        match (h.kind, &h.id) {
            (HeadKind::Group, Some(id)) => {
                self.org.toggle_collapsed(id);
                self.org_save();
            }
            (HeadKind::Ungrouped, _) => self.collapsed_ungrouped = !self.collapsed_ungrouped,
            (_, id) => {
                let l = id.clone().unwrap_or_default();
                if !self.collapsed_heads.remove(&l) {
                    self.collapsed_heads.insert(l);
                }
            }
        }
    }

    pub(super) fn pointer_up(&mut self, x: i32, y: i32, cx: &AppCx) {
        self.pointer = Some((x, y));
        let Some(d) = self.drag.take() else { return };
        if !d.active {
            // a click
            match d.press {
                Press::Item(i) => {
                    let si = self.sel_index();
                    self.sel[si] = i;
                    self.on_head = None;
                    self.open_current();
                }
                Press::Head(i) => self.head_click(i, cx),
            }
            return;
        }
        if self.iview != View::Groups {
            self.say("Switch Group by to My groups (v) to arrange things by dragging.", false);
            return;
        }
        let zone = self.zones.iter().rev().find(|z| z.rect.contains(x, y)).cloned();
        let Some(z) = zone else { return };
        let mid = z.rect.y + z.rect.h / 2.0;
        let after = y as f32 >= mid;
        if let Some(item) = d.item.clone() {
            let (group, before) = match &z.what {
                ZoneKind::Head { group, .. } => (group.clone(), None),
                ZoneKind::Item { key, group } => {
                    let target = self.display_okeys(group.as_deref());
                    let tk = self.okey_of_id(key);
                    let before = if after { target.iter().position(|k| *k == tk).and_then(|p| target.get(p + 1)).cloned() } else { Some(tk) };
                    (group.clone(), before.filter(|b| *b != item))
                }
            };
            self.move_to(&item, group, before.as_deref());
        } else if let Some(g) = d.group {
            // a group dropped on a heading or an item: it goes before or after that group
            let target = match &z.what {
                ZoneKind::Head { group, .. } | ZoneKind::Item { group, .. } => group.clone(),
            };
            if let Some(t) = target.filter(|t| *t != g) {
                if let Some(at) = self.org.groups.iter().position(|x| x.id == t) {
                    let to = if after { at + 1 } else { at };
                    let from = self.org.groups.iter().position(|x| x.id == g).unwrap_or(0);
                    let to = if from < to { to - 1 } else { to };
                    let _ = self.org.move_group(&g, to);
                    self.org_save();
                    self.say(format!("Moved the group {}.", d.label), true);
                }
            }
        }
    }

    /// The org keys of the items shown in a group, in the order shown.
    fn display_okeys(&self, group: Option<&str>) -> Vec<String> {
        let mut cur: Option<Option<String>> = None;
        let mut out = vec![];
        for r in self.cur_display() {
            match r {
                DRow::Head(h) => cur = Some(if h.kind == HeadKind::Group { h.id } else { None }),
                DRow::Item(k) => {
                    if cur.as_ref().is_some_and(|c| c.as_deref() == group) {
                        out.push(self.okey_of_id(&k));
                    }
                }
            }
        }
        out
    }

    /// Put an item in a group (None: Ungrouped), before another item or at the end.
    pub(super) fn move_to(&mut self, item: &str, group: Option<String>, before: Option<&str>) {
        match self.org.move_item(item, group.as_deref(), before) {
            Ok(()) => {
                self.org_save();
                let name = group.as_deref().and_then(|g| self.org.group(g)).map_or("Ungrouped".to_string(), |g| g.name.clone());
                self.say(format!("Moved to {name}."), true);
                let at = self.cur_display().into_iter().filter_map(|r| if let DRow::Item(k) = r { Some(k) } else { None }).position(|k| self.okey_of_id(&k) == item);
                if let Some(at) = at {
                    let si = self.sel_index();
                    self.sel[si] = at;
                    self.on_head = None;
                }
            }
            Err(e) => self.say(e, false),
        }
    }

    // -------------------------------------------------------------------------------------- keys for arranging

    pub(super) fn open_edit(&mut self, kind: EditKind, title: &str, hint: &str, initial: &str) {
        let mut field = TextField::new("", false);
        field.set(initial);
        self.mode = Mode::Edit(Box::new(EditState { kind, title: title.into(), hint: hint.into(), field, sugg_sel: 0 }));
    }

    pub(super) fn edit_tags(&mut self) {
        let Some((k, real)) = self.target() else { return };
        let cur = self.org.tags_of(&k).join(", ");
        let shown = self.org.display_title(&k, &real).to_string();
        self.open_edit(EditKind::Tags(k), &format!("Tags for “{shown}”"), "Words separated by commas. Tab completes from your existing tags.", &cur);
    }

    pub(super) fn edit_alias(&mut self) {
        let Some((k, real)) = self.target() else { return };
        let cur = self.org.alias_of(&k).unwrap_or("").to_string();
        self.open_edit(EditKind::Alias(k), &format!("Your title for “{real}”"), "Only changes what you see in lists. The real name, id and publisher always stay visible on its page and when it is installed or updated. Leave empty to go back to the real title.", &cur);
    }

    pub(super) fn open_move_menu(&mut self) {
        let Some((k, real)) = self.target() else { return };
        let mut rows: Vec<(String, MenuAct)> = self.org.groups.iter().map(|g| (g.name.clone(), MenuAct::MoveTo(Some(g.id.clone())))).collect();
        rows.push(("Ungrouped".into(), MenuAct::MoveTo(None)));
        rows.push(("New group…".into(), MenuAct::NewGroupAndMove));
        let cur = self.org.group_id_of(&k).map(str::to_string);
        let sel = rows.iter().position(|(_, a)| *a == MenuAct::MoveTo(cur.clone())).unwrap_or(0);
        let shown = self.org.display_title(&k, &real).to_string();
        self.mode = Mode::Menu(Box::new(MenuState { title: format!("Move “{shown}” to"), rows, sel, item: Some(k) }));
    }

    pub(super) fn open_organise_menu(&mut self) {
        let mut rows = vec![("New group…".to_string(), MenuAct::NewGroup)];
        for g in &self.org.groups {
            rows.push((format!("Rename group “{}”", g.name), MenuAct::RenameGroup(g.id.clone())));
            rows.push((format!("Move group “{}” up", g.name), MenuAct::GroupUp(g.id.clone())));
            rows.push((format!("Delete group “{}” (its items stay)", g.name), MenuAct::DeleteGroup(g.id.clone())));
        }
        for v in View::ALL {
            rows.push((format!("Group by: {}{}", v.label(), if v == self.iview { "  (now)" } else { "" }), MenuAct::GroupBy(v)));
        }
        rows.push(("Tags: filter, rename, merge, delete…".into(), MenuAct::ManageTags));
        rows.push(("Export my arrangement to a file…".into(), MenuAct::Export));
        rows.push(("Import an arrangement (merge)…".into(), MenuAct::Import(false)));
        rows.push(("Import an arrangement (replace mine)…".into(), MenuAct::Import(true)));
        self.mode = Mode::Menu(Box::new(MenuState { title: "Organise".into(), rows, sel: 0, item: None }));
    }

    pub(super) fn set_view(&mut self, v: View) {
        self.iview = v;
        self.org.view = v.id().to_string();
        self.on_head = None;
        self.org_save();
        self.say(format!("Grouped by: {}.", v.label()), true);
    }

    fn run_menu(&mut self, act: MenuAct, item: Option<String>, cx: &AppCx) {
        self.mode = Mode::Browse;
        if self.idetail.is_some() {
            self.mode = Mode::Detail;
        }
        match act {
            MenuAct::MoveTo(g) => {
                if let Some(i) = item {
                    self.move_to(&i, g, None);
                }
            }
            MenuAct::NewGroupAndMove => self.open_edit(EditKind::NewGroup { then_move: item }, "New group", "A short name, such as Work, Kids or Dev tools.", ""),
            MenuAct::NewGroup => self.open_edit(EditKind::NewGroup { then_move: None }, "New group", "A short name, such as Work, Kids or Dev tools.", ""),
            MenuAct::RenameGroup(id) => {
                let cur = self.org.group(&id).map(|g| g.name.clone()).unwrap_or_default();
                self.open_edit(EditKind::RenameGroup(id), "Rename the group", "", &cur);
            }
            MenuAct::GroupUp(id) => {
                let _ = self.org.nudge_group(&id, -1);
                self.org_save();
                self.say("Group moved up.", true);
            }
            MenuAct::DeleteGroup(id) => {
                let name = self.org.group(&id).map(|g| g.name.clone()).unwrap_or_default();
                self.mode = Mode::Confirm(Ask::DeleteGroup(id, name));
            }
            MenuAct::GroupBy(v) => self.set_view(v),
            MenuAct::ManageTags => self.mode = Mode::Tags(Box::new(TagsState { sel: 0 })),
            MenuAct::Export => {
                self.host_save = Some(("iyakki-organisation.json".into(), self.org.export().into_bytes()));
                self.say("Choose where to save your arrangement; the file holds only titles, tags and groups.", true);
            }
            MenuAct::Import(replace) => self.open_edit(EditKind::ImportPath { replace }, if replace { "Import and replace my arrangement" } else { "Import and merge into my arrangement" }, "The path of an exported organisation file (a USB stick is under /mnt).", "/mnt/"),
        }
        let _ = cx;
    }

    /// Keys of the Installed list that arrange things. True when handled.
    pub(super) fn org_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        // a heading under the cursor
        if let Some(h) = self.on_head.clone() {
            let head = self.heads().into_iter().find(|x| x.id == h);
            match k {
                Key::Enter | Key::Char(' ') => {
                    if let Some(head) = head {
                        self.toggle_head(&head, cx);
                    }
                    return true;
                }
                Key::Char('e') => {
                    if let Some(g) = h.as_deref().and_then(|g| self.org.group(g)).cloned() {
                        self.open_edit(EditKind::RenameGroup(g.id), "Rename the group", "", &g.name);
                    } else {
                        self.say("Ungrouped is the default place and cannot be renamed.", false);
                    }
                    return true;
                }
                Key::Char('k') => {
                    if let Some(g) = h.as_deref() {
                        self.org.next_colour(g);
                        self.org_save();
                    }
                    return true;
                }
                Key::Char('x') => {
                    if let Some(g) = h.as_deref().and_then(|g| self.org.group(g)).cloned() {
                        self.mode = Mode::Confirm(Ask::DeleteGroup(g.id, g.name));
                    } else {
                        self.say("Ungrouped cannot be deleted.", false);
                    }
                    return true;
                }
                Key::Nav { nav: Nav::Up | Nav::Down, shift: true, .. } => {
                    let d = if matches!(k, Key::Nav { nav: Nav::Up, .. }) { -1 } else { 1 };
                    if let Some(g) = h.as_deref() {
                        let _ = self.org.nudge_group(g, d);
                        self.org_save();
                    }
                    return true;
                }
                Key::Char('m') | Key::Char('t') | Key::Char('o') | Key::Char('d') | Key::Char('r') | Key::Char('u') | Key::Char('b') | Key::Char('p') => {
                    self.say("Choose an item first (Down).", false);
                    return true;
                }
                _ => {}
            }
        }
        // Discover already uses t (trust) and g (category) for its filters: tags are # and the menu is O there
        let (tags_key, menu_key) = if self.section == Section::Discover { ('#', 'O') } else { ('t', 'g') };
        match k {
            Key::Char('m') => self.open_move_menu(),
            Key::Char(c) if *c == tags_key => self.edit_tags(),
            Key::Char('e') => self.edit_alias(),
            Key::Char('n') => self.open_edit(EditKind::NewGroup { then_move: None }, "New group", "A short name, such as Work, Kids or Dev tools.", ""),
            Key::Char('v') => {
                let v = self.iview.next();
                self.set_view(v);
            }
            Key::Char(c) if *c == menu_key => self.open_organise_menu(),
            Key::Char('T') => self.mode = Mode::Tags(Box::new(TagsState { sel: 0 })),
            Key::Nav { nav: Nav::Up | Nav::Down, shift: true, .. } => {
                let d = if matches!(k, Key::Nav { nav: Nav::Up, .. }) { -1 } else { 1 };
                if self.iview != View::Groups {
                    self.say("Switch Group by to My groups (v) to put things in your own order.", false);
                } else if let Some((key, _)) = self.target() {
                    let group = self.org.group_id_of(&key).map(str::to_string);
                    let visible = self.display_okeys(group.as_deref());
                    let _ = self.org.nudge_item(&key, &visible, d);
                    self.org_save();
                    let at = self.cur_display().into_iter().filter_map(|r| if let DRow::Item(k) = r { Some(k) } else { None }).position(|k| self.okey_of_id(&k) == key);
                    if let Some(at) = at {
                        let si = self.sel_index();
                        self.sel[si] = at;
                    }
                }
            }
            _ => return false,
        }
        true
    }

    // -------------------------------------------------------------------------------------- the small dialogs: keys

    pub(super) fn edit_key(&mut self, k: &Key, cx: &AppCx) {
        let Mode::Edit(st) = &mut self.mode else { return };
        let word_start = |v: &str| v.rfind(',').map_or(0, |i| i + 1);
        match k {
            Key::Esc => {
                self.mode = if self.idetail.is_some() { Mode::Detail } else { Mode::Browse };
                return;
            }
            Key::Tab if matches!(st.kind, EditKind::Tags(_)) => {
                let v = st.field.value.clone();
                let typed = v[word_start(&v)..].trim().to_string();
                let suggestions = self.org.suggest(&typed);
                if let Some(s) = suggestions.get(st.sugg_sel.min(suggestions.len().saturating_sub(1))) {
                    let head = v[..word_start(&v)].to_string();
                    let Mode::Edit(st) = &mut self.mode else { return };
                    st.field.set(&format!("{head}{}{s}, ", if head.is_empty() || head.ends_with(' ') { "" } else { " " }));
                    st.sugg_sel = 0;
                }
                return;
            }
            Key::Up | Key::Down if matches!(st.kind, EditKind::Tags(_)) => {
                if matches!(k, Key::Up) { st.sugg_sel = st.sugg_sel.saturating_sub(1) } else { st.sugg_sel += 1 }
                return;
            }
            Key::Enter => {}
            other => {
                st.field.handle(other);
                st.sugg_sel = 0;
                return;
            }
        }
        // Enter: apply
        let Mode::Edit(st) = std::mem::replace(&mut self.mode, if self.idetail.is_some() { Mode::Detail } else { Mode::Browse }) else { return };
        let text = st.field.value.trim().to_string();
        match st.kind {
            EditKind::Tags(key) => {
                let tags: Vec<String> = text.split(',').map(str::to_string).collect();
                match self.org.set_tags(&key, &tags) {
                    Ok(()) => {
                        self.org_save();
                        self.say("Tags saved.", true);
                    }
                    Err(e) => {
                        self.say(e, false);
                    }
                }
            }
            EditKind::Alias(key) => match self.org.set_alias(&key, Some(&text)) {
                Ok(()) => {
                    self.org_save();
                    self.say(if text.is_empty() { "Back to the real title." } else { "Your title is saved; the real name stays in the page." }, true);
                }
                Err(e) => self.say(e, false),
            },
            EditKind::NewGroup { then_move } => match self.org.create_group(&text, (self.org.groups.len() % COLOURS.len()) as u8) {
                Ok(id) => {
                    self.org_save();
                    if let Some(i) = then_move {
                        self.move_to(&i, Some(id), None);
                    } else {
                        self.say(format!("Group “{text}” made. Choose My groups in Group by (v) to see it."), true);
                    }
                }
                Err(e) => self.say(e, false),
            },
            EditKind::RenameGroup(id) => match self.org.rename_group(&id, &text) {
                Ok(()) => {
                    self.org_save();
                    self.say("Group renamed.", true);
                }
                Err(e) => self.say(e, false),
            },
            EditKind::RenameTag(old) => match self.org.rename_tag(&old, &text) {
                Ok(n) => {
                    self.org_save();
                    let new = org::clean_tag(&text);
                    self.itags.iter_mut().for_each(|t| {
                        if *t == old {
                            *t = new.clone();
                        }
                    });
                    self.itags.dedup();
                    self.say(format!("Renamed on {n} item(s); items that already had “{new}” were merged."), true);
                    self.mode = Mode::Tags(Box::new(TagsState { sel: 0 }));
                }
                Err(e) => self.say(e, false),
            },
            EditKind::ImportPath { replace } => {
                let r = std::fs::metadata(&text).map_err(|e| e.to_string()).and_then(|m| if m.len() as usize > org::MAX_FILE { Err("That file is too large.".to_string()) } else { std::fs::read_to_string(&text).map_err(|e| e.to_string()) }).and_then(|t| self.org.import(&t, replace));
                match r {
                    Ok(m) => {
                        self.org_save();
                        self.reload_org(cx);
                        self.say(m, true);
                    }
                    Err(e) => self.say(e, false),
                }
            }
        }
    }

    pub(super) fn menu_key(&mut self, k: &Key, cx: &AppCx) {
        let Mode::Menu(m) = &mut self.mode else { return };
        match k {
            Key::Esc => self.mode = if self.idetail.is_some() { Mode::Detail } else { Mode::Browse },
            Key::Up => m.sel = m.sel.saturating_sub(1),
            Key::Down => m.sel = (m.sel + 1).min(m.rows.len().saturating_sub(1)),
            Key::Char(c) if c.is_ascii_digit() && *c != '0' => {
                let i = *c as usize - '1' as usize;
                if i < m.rows.len() {
                    m.sel = i;
                    self.menu_run(cx);
                }
            }
            Key::Enter => self.menu_run(cx),
            _ => {}
        }
    }

    fn menu_run(&mut self, cx: &AppCx) {
        let Mode::Menu(m) = &self.mode else { return };
        let (act, item) = (m.rows[m.sel].1.clone(), m.item.clone());
        self.run_menu(act, item, cx);
    }

    pub(super) fn tags_key(&mut self, k: &Key) {
        let tags = self.org.all_tags();
        let Mode::Tags(st) = &mut self.mode else { return };
        match k {
            Key::Esc | Key::Enter if matches!(k, Key::Esc) || tags.is_empty() => self.mode = if self.idetail.is_some() { Mode::Detail } else { Mode::Browse },
            Key::Enter => self.mode = if self.idetail.is_some() { Mode::Detail } else { Mode::Browse },
            Key::Up => st.sel = st.sel.saturating_sub(1),
            Key::Down => st.sel = (st.sel + 1).min(tags.len().saturating_sub(1)),
            Key::Char(' ') => {
                if let Some((t, _)) = tags.get(st.sel) {
                    let t = t.clone();
                    self.toggle_tag_filter(&t);
                }
            }
            Key::Char('c') => {
                self.itags.clear();
                self.sel[1] = 0;
            }
            Key::Char('r') => {
                if let Some((t, _)) = tags.get(st.sel) {
                    let t = t.clone();
                    self.open_edit(EditKind::RenameTag(t.clone()), &format!("Rename the tag “{t}”"), "If the new name is a tag that already exists, the two are merged.", &t);
                }
            }
            Key::Char('x') => {
                if let Some((t, n)) = tags.get(st.sel) {
                    self.mode = Mode::Confirm(Ask::DeleteTag(t.clone(), *n));
                }
            }
            _ => {}
        }
    }

    pub(super) fn toggle_tag_filter(&mut self, t: &str) {
        if let Some(i) = self.itags.iter().position(|x| x == t) {
            self.itags.remove(i);
        } else {
            self.itags.push(t.to_string());
        }
        self.sel[1] = 0;
        self.on_head = None;
    }

    // -------------------------------------------------------------------------------------- lifecycle: actions

    /// Run something slow off the screen's thread; its answer arrives as `Done::Applied`.
    pub(super) fn spawn_op(&mut self, label: &str, f: impl FnOnce(&dyn Fn(&str)) -> Done + Send + 'static) {
        if self.job.is_some() {
            self.say("Wait for the current job to finish.", false);
            return;
        }
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let progress = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let (out, pr) = (slot.clone(), progress.clone());
        std::thread::spawn(move || {
            let say = move |m: &str| *pr.lock().unwrap_or_else(|e| e.into_inner()) = m.to_string();
            let d = f(&say);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
        });
        self.job = Some(Job { label: label.into(), slot, progress: Some(progress) });
    }

    /// Download and review the newer version in the background, then show what would change.
    pub(super) fn start_update(&mut self, cx: &AppCx, key: &str) {
        let Some(life) = self.life_of(key).cloned() else {
            self.say("That is not installed any more.", false);
            return;
        };
        if life.entry.ty == IType::Driver {
            self.say("Driver packs change the whole machine: update them from the Drivers chip.", false);
            return;
        }
        if let Some(p) = &life.pinned {
            self.say(format!("{} is pinned at {}. Press p to unpin it first.", life.entry.title, p.version), false);
            return;
        }
        if !life.has_update() && life.entry.ty != IType::Cli {
            self.say(format!("{}: {} {}", life.entry.title, life.check.label(), life.check.note()), life.check == iyakki_agent::store::lifecycle::Check::UpToDate);
            return;
        }
        let (st, svc, actor, loaded, fetch) = (self.store.clone(), cx.services.clone(), self.actor(cx), self.loaded.clone(), self.fetch.clone());
        let title = life.entry.title.clone();
        self.spawn_op(&format!("Downloading and checking {title}…"), move |_| match st.plan_update(&svc, &actor, &life, &loaded, fetch.as_ref()) {
            Ok(p) => Done::Planned(Box::new(p)),
            Err(e) => Done::Failed(e.to_string()),
        });
    }

    /// The plan was shown and accepted: apply it in the background (it checks the result and puts the old version back if needed).
    pub(super) fn apply_plan(&mut self, cx: &AppCx, plan: UpdatePlan) {
        let (st, svc, actor) = (self.store.clone(), cx.services.clone(), self.actor(cx));
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let label = format!("Updating {} to {}…", plan.title, plan.to);
        self.spawn_op(&label, move |_| {
            let au = |a: &str, d: &str| audit.record(&user, a, d);
            match st.apply_update(&svc, &actor, &plan, iyakki_agent::store::install::Confirm::all(), &au) {
                Ok(m) => Done::Applied(Ok(m)),
                Err(e) => Done::Applied(Err(e.to_string())),
            }
        });
    }

    pub(super) fn ask_update_all(&mut self) {
        let rows = Store::update_all_preview(&self.lives);
        if rows.is_empty() {
            self.say("Everything is up to date as far as your catalogs say. Press c to look for news.", true);
            return;
        }
        self.mode = Mode::Confirm(Ask::UpdateAll(rows));
    }

    pub(super) fn run_update_all(&mut self, cx: &AppCx) {
        let (st, svc, policy, user, loaded, fetch) = (self.store.clone(), cx.services.clone(), cx.session.policy.clone(), cx.session.user.clone(), self.loaded.clone(), self.fetch.clone());
        let audit = cx.audit.clone();
        self.spawn_op("Updating everything that can be updated…", move |say| {
            let au = |a: &str, d: &str| audit.record(&user, a, d);
            let rep = st.update_all(&svc, &policy, &user, &loaded, fetch.as_ref(), &au, &mut |m| say(m));
            Done::AllDone(rep)
        });
    }

    pub(super) fn ask_rollback(&mut self) {
        let Some(e) = self.iselected() else { return };
        match self.life_of(&e.key) {
            Some(l) if l.can_rollback() => {
                self.mode = Mode::Confirm(Ask::Rollback { key: e.key.clone(), title: e.title.clone(), from: e.version.clone(), to: l.previous.clone().unwrap_or_default() });
            }
            _ => self.say(format!("{} has no earlier version kept to go back to.", e.title), false),
        }
    }

    pub(super) fn do_rollback(&mut self, cx: &AppCx, key: &str) {
        let Some(life) = self.life_of(key).cloned() else { return };
        let (st, svc, actor) = (self.store.clone(), cx.services.clone(), self.actor(cx));
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let label = format!("Going back to {}…", life.previous.clone().unwrap_or_default());
        self.spawn_op(&label, move |_| {
            let au = |a: &str, d: &str| audit.record(&user, a, d);
            Done::Applied(st.rollback(&svc, &actor, &life, &au).map_err(|e| e.to_string()))
        });
    }

    pub(super) fn toggle_pin(&mut self, cx: &AppCx) {
        let Some(e) = self.iselected() else { return };
        let Some(l) = self.life_of(&e.key).cloned() else { return };
        if l.pinned.is_some() {
            match self.store.unpin(&e.key) {
                Ok(()) => self.say(format!("{} can be updated again.", e.title), true),
                Err(m) => self.say(m, false),
            }
        } else if l.can_pin() {
            match self.store.pin(&e.key, &e.version) {
                Ok(()) => self.say(format!("{} is pinned at {}: updates skip it until you unpin it (p).", e.title, e.version), true),
                Err(m) => self.say(m, false),
            }
        } else {
            self.say(format!("{} has nothing to update, so there is nothing to pin.", e.title), false);
        }
        self.reload(cx);
    }

    pub(super) fn ask_remove_plan(&mut self, cx: &AppCx, e: IEntry) {
        let Some(l) = self.life_of(&e.key).cloned() else {
            self.mode = Mode::Confirm(Ask::RemoveInstalled(Box::new(e)));
            return;
        };
        let plan = self.store.plan_remove(&cx.services, &cx.session.user, &self.lives, &l);
        self.mode = Mode::Confirm(Ask::RemovePlan(Box::new(RemoveUi { plan, entry: e, typing: None })));
    }

    /// Remove for real. `delete_data` was chosen and confirmed by typing the name (or is false).
    pub(super) fn do_remove_plan(&mut self, cx: &AppCx, ui: RemoveUi, delete_data: bool, typed: String) {
        let Some(life) = self.life_of(&ui.entry.key).cloned() else { return };
        let opts = RemoveOpts { delete_data, typed, ack_dependents: true };
        let audit = cx.audit.clone();
        let user = cx.session.user.clone();
        let au = |a: &str, d: &str| audit.record(&user, a, d);
        match self.store.remove_item(&cx.services, &cx.session.policy, &cx.session.user, &self.lives, &life, &opts, &au) {
            Ok(m) => {
                self.say(m, true);
                self.idetail = None;
                self.mode = Mode::Browse;
            }
            Err(LifeError::NeedsConfirm(m)) | Err(LifeError::Refused(m)) | Err(LifeError::Failed(m)) => {
                self.mode = Mode::Browse;
                self.say(m, false);
            }
        }
        self.reload(cx);
        let n = self.ifiltered().len();
        self.sel[1] = self.sel[1].min(n.saturating_sub(1));
    }

    /// Keys while a removal is being confirmed. True when the event was handled here.
    pub(super) fn remove_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        let Mode::Confirm(Ask::RemovePlan(ui)) = &mut self.mode else { return false };
        if let Some(f) = &mut ui.typing {
            match k {
                Key::Esc => ui.typing = None,
                Key::Enter => {
                    let typed = f.value.trim().to_string();
                    if typed == ui.entry.name {
                        let Mode::Confirm(Ask::RemovePlan(ui)) = std::mem::replace(&mut self.mode, Mode::Browse) else { return true };
                        self.do_remove_plan(cx, *ui, true, typed);
                    } else {
                        let name = ui.entry.name.clone();
                        self.say(format!("That is not the name. Type {name} exactly, or Esc to go back."), false);
                    }
                }
                other => {
                    f.handle(other);
                }
            }
            return true;
        }
        match k {
            Key::Char('y') => {
                let Mode::Confirm(Ask::RemovePlan(ui)) = std::mem::replace(&mut self.mode, Mode::Browse) else { return true };
                self.do_remove_plan(cx, *ui, false, String::new());
            }
            Key::Char('x') if ui.plan.has_data() => {
                let mut f = TextField::new("type the name", false);
                f.set("");
                ui.typing = Some(f);
            }
            _ => self.mode = if self.idetail.is_some() { Mode::Detail } else { Mode::Browse },
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Nav2 {
    Head(Option<String>),
    Item(usize),
}

// ------------------------------------------------------------------------------------------------ drawing: dialogs

fn modal_lines(g: &mut Gfx, p: R, s: f32, top: f32, bottom: f32, lines: &[(String, [u8; 4], bool)]) {
    let lh = 18.0 * s;
    let fit = (((bottom - top) / lh).floor() as usize).max(1);
    for (i, (t, c, bold)) in lines.iter().take(fit).enumerate() {
        let t = g.fit(13.0 * s, t, if *bold { Face::Bold } else { Face::Sans }, p.w - 52.0 * s);
        g.text(p.x + 26.0 * s, top + i as f32 * lh, 13.0 * s, *c, &t, if *bold { Face::Bold } else { Face::Sans });
    }
}

/// The removal dialog: what goes, what stays, who uses it, and the data choice.
pub(super) fn render_remove(g: &mut Gfx, area: R, s: f32, ui: &RemoveUi, tick: u64) {
    let p = fx::modal(g, area, s, 700.0 * s, 440.0 * s, ERR, true);
    let top = fx::modal_title(g, p, s, &format!("Remove “{}”?", ui.entry.title), ERR);
    let mut lines: Vec<(String, [u8; 4], bool)> = vec![("THIS REMOVES".into(), ACCENT, true)];
    for r in &ui.plan.removes {
        for l in g.wrap(13.0 * s, &format!("• {r}"), Face::Sans, p.w - 60.0 * s) {
            lines.push((l, TEXT, false));
        }
    }
    if ui.plan.cached > 0 {
        lines.push((format!("• the kept copies of its package ({} KB)", ui.plan.cached.div_ceil(1024)), TEXT, false));
    }
    if !ui.plan.dependents.is_empty() {
        lines.push(("USED BY (they may stop working)".into(), ERR, true));
        for d in &ui.plan.dependents {
            lines.push((format!("• {d}"), AMBER, false));
        }
    }
    lines.push(("ITS DATA".into(), ACCENT, true));
    if ui.plan.has_data() {
        for (n, b) in &ui.plan.data {
            lines.push((format!("• folder {n} ({} KB) — kept unless you delete it", b.div_ceil(1024)), TEXT, false));
        }
        for v in &ui.plan.vault {
            lines.push((format!("• vault entry {v} — kept unless you delete it"), TEXT, false));
        }
    } else {
        lines.push(("It keeps no data of its own: nothing to keep or delete.".into(), MUTED, false));
    }
    if !ui.plan.vault_note.is_empty() {
        lines.push((ui.plan.vault_note.clone(), MUTED, false));
    }
    let bottom = p.y + p.h - 70.0 * s;
    modal_lines(g, p, s, top, bottom, &lines);
    let by = p.y + p.h - 52.0 * s;
    if let Some(f) = &ui.typing {
        g.text(p.x + 26.0 * s, by - 28.0 * s, 13.0 * s, ERR, &format!("Deleting data cannot be undone. Type {} to confirm, then Enter.", ui.entry.name), Face::Bold);
        let r = R::new(p.x + 26.0 * s, by, 260.0 * s, 36.0 * s);
        f.render(g, r, true, s, blink(tick), Face::Sans, 7);
        g.button(r.x + r.w + 12.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
        return;
    }
    let mut bx = p.x + 26.0 * s;
    bx += g.button(bx, by, 14.0 * s, if ui.plan.has_data() { "Remove, keep data (y)" } else { "Remove (y)" }, ERR, Event::Key(Key::Char('y'))) + 10.0 * s;
    if ui.plan.has_data() {
        bx += g.button(bx, by, 14.0 * s, "Remove and delete data (x)", ERR, Event::Key(Key::Char('x'))) + 10.0 * s;
    }
    g.button(bx, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
}

pub(super) fn render_update_all(g: &mut Gfx, area: R, s: f32, rows: &[(String, String, bool)]) {
    let p = fx::modal(g, area, s, 760.0 * s, 440.0 * s, ACCENT, false);
    let top = fx::modal_title(g, p, s, &format!("Update all: {} item(s)", rows.len()), ACCENT);
    let mut lines: Vec<(String, [u8; 4], bool)> = vec![("Items that need a decision are skipped; update those one by one (u) to see what they ask for.".into(), MUTED, false)];
    for (t, d, ok) in rows {
        lines.push((format!("{} {t}: {d}", if *ok { "will update" } else { "will be skipped:" }), if *ok { TEXT } else { AMBER }, false));
    }
    modal_lines(g, p, s, top, p.y + p.h - 70.0 * s, &lines);
    let by = p.y + p.h - 52.0 * s;
    let bw = g.button(p.x + 26.0 * s, by, 14.0 * s, "Update all (y)", OK, Event::Key(Key::Char('y')));
    g.button(p.x + 26.0 * s + bw + 10.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
}

pub(super) fn render_report(g: &mut Gfx, area: R, s: f32, rep: &Report) {
    let p = fx::modal(g, area, s, 760.0 * s, 440.0 * s, ACCENT, false);
    let top = fx::modal_title(g, p, s, "Update all: done", ACCENT);
    let lines: Vec<(String, [u8; 4], bool)> = rep
        .summary()
        .lines()
        .map(|l| {
            let c = if l.starts_with("FAILED") { ERR } else if l.starts_with("skipped") { AMBER } else if l.starts_with("updated") { OK } else { TEXT };
            (l.to_string(), c, !l.contains(':'))
        })
        .collect();
    modal_lines(g, p, s, top, p.y + p.h - 70.0 * s, &lines);
    g.button(p.x + 26.0 * s, p.y + p.h - 52.0 * s, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
}

pub(super) fn render_update_cli(g: &mut Gfx, area: R, s: f32, plan: &UpdatePlan) {
    let p = fx::modal(g, area, s, 700.0 * s, 380.0 * s, ACCENT, false);
    let top = fx::modal_title(g, p, s, &format!("Update “{}” to {}?", plan.title, plan.to), ACCENT);
    let mut lines: Vec<(String, [u8; 4], bool)> = vec![];
    for l in plan.summary().lines() {
        for w in g.wrap(13.0 * s, l, Face::Sans, p.w - 60.0 * s) {
            lines.push((w, TEXT, false));
        }
    }
    for w in &plan.warnings {
        for l in g.wrap(13.0 * s, w, Face::Sans, p.w - 60.0 * s) {
            lines.push((l, MUTED, false));
        }
    }
    modal_lines(g, p, s, top, p.y + p.h - 70.0 * s, &lines);
    let by = p.y + p.h - 52.0 * s;
    let bw = g.button(p.x + 26.0 * s, by, 14.0 * s, "Update (y)", OK, Event::Key(Key::Char('y')));
    g.button(p.x + 26.0 * s + bw + 10.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
}

/// A heading line in the list.
pub(super) fn render_head(g: &mut Gfx, r: R, s: f32, h: &Head, index: usize, cursor: bool, dropping: bool, dragging: bool) {
    let hot = g.hovered(r);
    let base = h.colour.map(colour).unwrap_or(MUTED);
    g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, if cursor || hot { fx::over(fx::WELL, fx::with_alpha(base, 40)) } else { fx::WELL });
    if cursor || dropping {
        g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if dropping { 2.2 * s } else { 1.4 }, if dropping { ACCENT } else { base });
    }
    g.hit(r, Event::Tab(TAB_HEAD + index), &format!("group {}", h.label));
    let arrow = if h.collapsed { ">" } else { "v" };
    g.text(r.x + 12.0 * s, r.y + (r.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, base, arrow, Face::Bold);
    g.circle(r.x + 34.0 * s, r.y + r.h / 2.0, 5.0 * s, base);
    let label = format!("{}  ({})", h.label, h.count);
    let t = g.fit(13.0 * s, &label, Face::Bold, r.w - 60.0 * s);
    g.text(r.x + 48.0 * s, r.y + (r.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, &t, Face::Bold);
    if dragging && dropping {
        let m = "drop here";
        let w = g.text_width(11.0 * s, m, Face::Bold);
        g.text(r.x + r.w - w - 12.0 * s, r.y + (r.h - g.line_height(11.0 * s, Face::Bold)) / 2.0, 11.0 * s, ACCENT, m, Face::Bold);
    }
}

pub(super) fn render_ghost(g: &mut Gfx, s: f32, at: (i32, i32), label: &str) {
    let t = g.fit(13.0 * s, label, Face::Bold, 260.0 * s);
    let w = g.text_width(13.0 * s, &t, Face::Bold) + 24.0 * s;
    let (x, y) = (at.0 as f32 + 12.0 * s, at.1 as f32 + 8.0 * s);
    g.rrect(x, y, w, 28.0 * s, 8.0 * s, fx::over(fx::PANEL, fx::with_alpha(ACCENT, 60)));
    g.rrect_outline(x, y, w, 28.0 * s, 8.0 * s, 1.6, ACCENT);
    g.text(x + 12.0 * s, y + (28.0 * s - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, &t, Face::Bold);
}

impl StoreApp {
    pub(super) fn render_edit(&self, g: &mut Gfx, area: R, s: f32, st: &EditState, tick: u64) {
        let p = fx::modal(g, area, s, 640.0 * s, 330.0 * s, ACCENT, false);
        let top = fx::modal_title(g, p, s, &st.title, ACCENT);
        let mut y = top + 4.0 * s;
        for l in g.wrap(13.0 * s, &st.hint, Face::Sans, p.w - 52.0 * s).into_iter().take(3) {
            g.text(p.x + 26.0 * s, y, 13.0 * s, MUTED, &l, Face::Sans);
            y += 18.0 * s;
        }
        y += 6.0 * s;
        let r = R::new(p.x + 26.0 * s, y, p.w - 52.0 * s, 38.0 * s);
        st.field.render(g, r, true, s, blink(tick), Face::Sans, 7);
        y += 46.0 * s;
        if let EditKind::Tags(_) = &st.kind {
            let v = &st.field.value;
            let typed = v[v.rfind(',').map_or(0, |i| i + 1)..].trim();
            let sugg = self.org.suggest(typed);
            if !sugg.is_empty() {
                g.text(p.x + 26.0 * s, y, 12.0 * s, DIM, "Your tags (Tab completes, Up/Down chooses):", Face::Sans);
                y += 18.0 * s;
                let mut x = p.x + 26.0 * s;
                for (i, t) in sugg.iter().take(8).enumerate() {
                    let w = g.text_width(12.0 * s, t, Face::Bold) + 17.0 * s;
                    if x + w > p.x + p.w - 26.0 * s {
                        break;
                    }
                    fx::chip(g, x, y, s, t, ACCENT, i == st.sugg_sel.min(sugg.len() - 1));
                    g.hit(R::new(x, y, w, 12.0 * s * 1.9), Event::Tab(TAB_SUGGEST + i), &format!("tag {t}"));
                    x += w + 8.0 * s;
                }
            }
        }
        let by = p.y + p.h - 52.0 * s;
        let bw = g.button(p.x + 26.0 * s, by, 14.0 * s, "Save (Enter)", OK, Event::Key(Key::Enter));
        g.button(p.x + 26.0 * s + bw + 10.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
    }

    pub(super) fn render_menu(&self, g: &mut Gfx, area: R, s: f32, m: &MenuState) {
        let rh = 34.0 * s;
        let h = (m.rows.len() as f32 * (rh + 4.0 * s) + 120.0 * s).min(area.h - 16.0 * s);
        let p = fx::modal(g, area, s, 560.0 * s, h, ACCENT, false);
        let top = fx::modal_title(g, p, s, &m.title, ACCENT);
        let fit = (((p.y + p.h - 64.0 * s - top) / (rh + 4.0 * s)).floor() as usize).max(1);
        let first = m.sel.saturating_sub(fit - 1);
        for (n, (label, _)) in m.rows.iter().enumerate().skip(first).take(fit) {
            let r = R::new(p.x + 20.0 * s, top + (n - first) as f32 * (rh + 4.0 * s), p.w - 40.0 * s, rh);
            fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, n == m.sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Tab(TAB_MENU + n), label);
            let t = g.fit(14.0 * s, &format!("{}  {label}", if n < 9 { format!("{}", n + 1) } else { " ".into() }), Face::Bold, r.w - 24.0 * s);
            g.text(r.x + 12.0 * s, r.y + (r.h - g.line_height(14.0 * s, Face::Bold)) / 2.0, 14.0 * s, TEXT, &t, Face::Bold);
        }
        g.button(p.x + 26.0 * s, p.y + p.h - 52.0 * s, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
    }

    pub(super) fn render_tags(&self, g: &mut Gfx, area: R, s: f32, st: &TagsState) {
        let tags = self.org.all_tags();
        let rh = 32.0 * s;
        let p = fx::modal(g, area, s, 600.0 * s, 460.0 * s, ACCENT, false);
        let top = fx::modal_title(g, p, s, "Tags", ACCENT);
        g.text(p.x + 26.0 * s, top, 12.0 * s, MUTED, "Space filters the list by a tag (several narrow it down) · r rename or merge · x delete · c clear filters", Face::Sans);
        let top = top + 24.0 * s;
        if tags.is_empty() {
            g.text(p.x + 26.0 * s, top + 10.0 * s, 14.0 * s, MUTED, "No tags yet. Choose an item and press t to give it some.", Face::Sans);
        }
        let fit = (((p.y + p.h - 70.0 * s - top) / (rh + 3.0 * s)).floor() as usize).max(1);
        let first = st.sel.saturating_sub(fit - 1);
        for (n, (t, count)) in tags.iter().enumerate().skip(first).take(fit) {
            let r = R::new(p.x + 20.0 * s, top + (n - first) as f32 * (rh + 3.0 * s), p.w - 40.0 * s, rh);
            fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, n == st.sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Tab(TAB_TAGROW + n), &format!("tag {t}"));
            let on = self.itags.contains(t);
            g.text(r.x + 12.0 * s, r.y + (r.h - g.line_height(14.0 * s, Face::Bold)) / 2.0, 14.0 * s, if on { OK } else { TEXT }, &format!("{} {t}", if on { "[x]" } else { "[ ]" }), Face::Bold);
            let c = format!("{count} item{}", if *count == 1 { "" } else { "s" });
            let w = g.text_width(12.0 * s, &c, Face::Sans);
            g.text(r.x + r.w - w - 12.0 * s, r.y + (r.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, MUTED, &c, Face::Sans);
        }
        g.button(p.x + 26.0 * s, p.y + p.h - 52.0 * s, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
    }

    /// A press on a menu row, suggestion or tag row from the pointer.
    pub(super) fn dialog_click(&mut self, n: usize, cx: &AppCx) -> bool {
        match &mut self.mode {
            Mode::Menu(m) if (TAB_MENU..TAB_MENU + 200).contains(&n) => {
                m.sel = (n - TAB_MENU).min(m.rows.len().saturating_sub(1));
                self.menu_run(cx);
                true
            }
            Mode::Edit(st) if (TAB_SUGGEST..TAB_SUGGEST + 20).contains(&n) => {
                st.sugg_sel = n - TAB_SUGGEST;
                self.edit_key(&Key::Tab, cx);
                true
            }
            Mode::Tags(st) if (TAB_TAGROW..TAB_TAGROW + 400).contains(&n) => {
                st.sel = n - TAB_TAGROW;
                self.tags_key(&Key::Char(' '));
                true
            }
            _ => false,
        }
    }
}

pub(super) fn review_for(plan: UpdatePlan) -> Option<Mode> {
    let review = plan.review.clone()?;
    Some(Mode::Review(Box::new(ReviewView { review, scroll: 0, purpose: Purpose::Update, plan: Some(plan) })))
}

impl StoreApp {
    /// The second line of an Installed row: the real name when the person's own title is shown, then version, scope and what it is.
    pub(super) fn row_sub(&self, e: &IEntry) -> String {
        let real = if self.shown_title(e) != e.title { format!("“{}” · ", e.title) } else { String::new() };
        format!(
            "{real}{}{}{}",
            if e.version == "-" || e.version.is_empty() { String::new() } else { format!("v{} · ", e.version) },
            if e.ty == IType::App { String::new() } else { format!("for {} · ", if e.scope == iyakki_agent::skills::Scope::Shared { "everyone" } else { "you" }) },
            if e.summary.is_empty() { &e.trust_label } else { &e.summary }
        )
    }

    /// The second line of a Discover row: the real name when a title of the person's own is shown, the publisher, the kind, the summary.
    pub(super) fn item_sub(&self, it: &super::Item, aliased: bool) -> String {
        let who = if it.entry.meta.publisher.name.is_empty() { String::new() } else { format!("{} · ", it.entry.meta.publisher.name) };
        let real = if aliased { format!("“{}” · ", it.title()) } else { String::new() };
        format!("{real}{who}{} · {}", super::kind_label(&it.entry.kind), it.entry.meta.summary)
    }
}

/// The line under an install or update review's title: what it is, whatever the person calls it. The real name and id come from the
/// package itself; a display title of the person's own is never used here.
pub(super) fn review_sub(r: &iyakki_agent::store::install::Review) -> String {
    format!(
        "{} {} {} · {} · {}{}",
        super::kind_label(&r.kind),
        r.name,
        r.version,
        super::size_text(r.size as u64),
        if r.scope == iyakki_agent::skills::Scope::Shared { "for everyone" } else { "for you" },
        if r.installed.is_some() { " · replaces the installed version" } else { "" }
    )
}
