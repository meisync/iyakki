//! Named, coloured tab groups for the Browser tile. A group is a name, a colour and a collapsed flag; every tab belongs to at most
//! one group, and the tabs of a group always sit next to each other in the strip (so one chip in front of them stands for the group).
//! This file is the pure model (no drawing): the browser keeps one [`Groups`] next to its tab list, `of_tab[i]` being the group of tab `i`.
//! Groups are kept per person (`browser-groups-<user>.json`) with the addresses of their tabs, and come back as tabs that load when
//! they are first opened.

use serde::{Deserialize, Serialize};

use super::AppCx;
use crate::gfx::rgb;

/// The colours a group can wear (the tab underline and the chip): six that differ for colour-blind eyes too (blue, orange, green,
/// pink, yellow, purple), each readable on the dark strip.
pub const COLORS: [[u8; 4]; 6] = [rgb(88, 190, 235), rgb(255, 150, 60), rgb(110, 224, 160), rgb(255, 120, 190), rgb(240, 210, 90), rgb(170, 140, 255)];
pub const COLOR_NAMES: [&str; 6] = ["blue", "orange", "green", "pink", "yellow", "purple"];
pub const NAME_MAX: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: u32,
    pub name: String,
    pub color: usize,
    pub collapsed: bool,
}

/// What the strip draws, left to right: a group's chip (once, where its first tab is) or a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripItem {
    Chip(u32),
    Tab(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Groups {
    pub list: Vec<Group>,
    pub of_tab: Vec<Option<u32>>,
    next_id: u32,
}

impl Default for Groups {
    fn default() -> Self {
        Groups { list: Vec::new(), of_tab: vec![None], next_id: 1 }
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    name: String,
    color: usize,
    collapsed: bool,
    urls: Vec<String>,
}

impl Groups {
    pub fn get(&self, id: u32) -> Option<&Group> {
        self.list.iter().find(|g| g.id == id)
    }

    pub fn of(&self, tab: usize) -> Option<&Group> {
        self.of_tab.get(tab).copied().flatten().and_then(|id| self.get(id))
    }

    pub fn color_of(&self, tab: usize) -> Option<[u8; 4]> {
        self.of(tab).map(|g| COLORS[g.color % COLORS.len()])
    }

    pub fn members(&self, id: u32) -> Vec<usize> {
        self.of_tab.iter().enumerate().filter(|(_, g)| **g == Some(id)).map(|(i, _)| i).collect()
    }

    pub fn push_tab(&mut self) {
        self.of_tab.push(None);
    }

    /// Tab `i` is gone; a group left without tabs goes with it.
    pub fn remove_tab(&mut self, i: usize) {
        if i < self.of_tab.len() {
            let old = self.of_tab.remove(i);
            self.prune(old);
        }
    }

    /// The only tab was replaced by a blank one: no groups.
    pub fn reset(&mut self) {
        self.list.clear();
        self.of_tab = vec![None];
    }

    fn prune(&mut self, id: Option<u32>) {
        if let Some(id) = id {
            if !self.of_tab.contains(&Some(id)) {
                self.list.retain(|g| g.id != id);
            }
        }
    }

    /// Put tab `tab` in a new group (named "Group N", the next colour). Returns the id; `None` when the tab does not exist.
    pub fn new_group(&mut self, tab: usize) -> Option<u32> {
        if tab >= self.of_tab.len() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let n = self.list.len() + 1;
        self.list.push(Group { id, name: format!("Group {n}"), color: (id as usize - 1) % COLORS.len(), collapsed: false });
        let old = self.of_tab[tab].replace(id);
        self.prune(old);
        Some(id)
    }

    /// Tab `tab` goes into group `id` (or out of any group for `None`).
    pub fn assign(&mut self, tab: usize, id: Option<u32>) {
        if tab >= self.of_tab.len() || id.is_some_and(|g| self.get(g).is_none()) {
            return;
        }
        let old = std::mem::replace(&mut self.of_tab[tab], id);
        self.prune(old);
    }

    /// The group after the one of `tab` in the list, then none, then the first again (what "Group..." on a card steps through).
    pub fn next_for(&self, tab: usize) -> Option<u32> {
        let cur = self.of_tab.get(tab).copied().flatten();
        match cur {
            None => self.list.first().map(|g| g.id),
            Some(c) => {
                let i = self.list.iter().position(|g| g.id == c)?;
                self.list.get(i + 1).map(|g| g.id)
            }
        }
    }

    pub fn toggle_collapse(&mut self, id: u32) {
        if let Some(g) = self.list.iter_mut().find(|g| g.id == id) {
            g.collapsed = !g.collapsed;
        }
    }

    pub fn rename(&mut self, id: u32, name: &str) {
        let clean: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(NAME_MAX).collect();
        if let Some(g) = self.list.iter_mut().find(|g| g.id == id) {
            if !clean.is_empty() {
                g.name = clean;
            }
        }
    }

    pub fn set_color(&mut self, id: u32, color: usize) {
        if let Some(g) = self.list.iter_mut().find(|g| g.id == id) {
            g.color = color % COLORS.len();
        }
    }

    /// The order that makes every group's tabs contiguous: a group sits where its first tab was, ungrouped tabs keep their places.
    /// Returns the old index of each position.
    pub fn contiguous_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.of_tab.len());
        let mut done: Vec<u32> = Vec::new();
        for (i, g) in self.of_tab.iter().enumerate() {
            match g {
                None => out.push(i),
                Some(id) if !done.contains(id) => {
                    done.push(*id);
                    out.extend(self.members(*id));
                }
                Some(_) => {}
            }
        }
        out
    }

    /// Reorder the group table with a permutation from [`Groups::contiguous_order`].
    pub fn apply_order(&mut self, order: &[usize]) {
        self.of_tab = order.iter().map(|&i| self.of_tab[i]).collect();
    }

    /// What the strip shows. The tab in use stays visible when its group is collapsed.
    pub fn items(&self, active: usize) -> Vec<StripItem> {
        let mut out = Vec::new();
        let mut seen: Vec<u32> = Vec::new();
        for (i, g) in self.of_tab.iter().enumerate() {
            match g.and_then(|id| self.get(id)) {
                None => out.push(StripItem::Tab(i)),
                Some(grp) => {
                    if !seen.contains(&grp.id) {
                        seen.push(grp.id);
                        out.push(StripItem::Chip(grp.id));
                    }
                    if !grp.collapsed || i == active {
                        out.push(StripItem::Tab(i));
                    }
                }
            }
        }
        out
    }

    pub fn saved_json(&self, urls: &[String]) -> String {
        let saved: Vec<Saved> = self
            .list
            .iter()
            .map(|g| Saved { name: g.name.clone(), color: g.color, collapsed: g.collapsed, urls: self.members(g.id).into_iter().filter_map(|i| urls.get(i).filter(|u| !u.is_empty()).cloned()).collect() })
            .collect();
        serde_json::to_string(&saved).unwrap_or_default()
    }
}

/// The groups kept for this person: (name, colour, collapsed, addresses).
pub fn load(cx: &AppCx) -> Vec<(String, usize, bool, Vec<String>)> {
    let saved: Vec<Saved> = iyakki_schema::read_plain("browser tab groups", &path(cx), iyakki_schema::now_secs()).unwrap_or_default();
    saved.into_iter().filter(|g| !g.urls.is_empty()).map(|g| (g.name, g.color, g.collapsed, g.urls)).collect()
}

pub fn save(cx: &AppCx, json: &str) {
    super::browser_config::write_checked("browser tab groups", &path(cx), json);
}

fn path(cx: &AppCx) -> std::path::PathBuf {
    cx.services.dir().join(format!("browser-groups-{}.json", super::browser_config::who(cx)))
}

/// The pages open in tabs that belong to no group when the browser was last used, so they come back the next time it opens
/// (the groups keep their own tabs, see above). Only web addresses are kept, at most `max`.
pub fn load_open(cx: &AppCx, max: usize) -> Vec<String> {
    let urls: Vec<String> = iyakki_schema::read_plain("browser open pages", &open_path(cx), iyakki_schema::now_secs()).unwrap_or_default();
    urls.into_iter().filter(|u| keep_open(u)).take(max).collect()
}

pub fn save_open(cx: &AppCx, json: &str) {
    super::browser_config::write_checked("browser open pages", &open_path(cx), json);
}

/// A page worth bringing back: a web address (not a blank tab, a file, a data page or an internal page).
pub fn keep_open(u: &str) -> bool {
    u.len() < 2048 && (u.starts_with("https://") || u.starts_with("http://"))
}

fn open_path(cx: &AppCx) -> std::path::PathBuf {
    cx.services.dir().join(format!("browser-open-{}.json", super::browser_config::who(cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tabs(n: usize) -> Groups {
        let mut g = Groups::default();
        for _ in 1..n {
            g.push_tab();
        }
        g
    }

    #[test]
    fn a_new_group_takes_the_tab_and_its_chip_shows_once() {
        let mut g = with_tabs(3);
        let id = g.new_group(1).unwrap();
        assert_eq!(g.items(0), vec![StripItem::Tab(0), StripItem::Chip(id), StripItem::Tab(1), StripItem::Tab(2)]);
        g.assign(2, Some(id));
        assert_eq!(g.items(0), vec![StripItem::Tab(0), StripItem::Chip(id), StripItem::Tab(1), StripItem::Tab(2)]);
        assert_eq!(g.members(id), vec![1, 2]);
    }

    #[test]
    fn collapsing_hides_the_tabs_but_not_the_one_in_use() {
        let mut g = with_tabs(3);
        let id = g.new_group(0).unwrap();
        g.assign(1, Some(id));
        g.toggle_collapse(id);
        assert_eq!(g.items(2), vec![StripItem::Chip(id), StripItem::Tab(2)]);
        assert_eq!(g.items(1), vec![StripItem::Chip(id), StripItem::Tab(1), StripItem::Tab(2)]);
        g.toggle_collapse(id);
        assert_eq!(g.items(2).len(), 4);
    }

    #[test]
    fn closing_the_last_tab_of_a_group_removes_the_group() {
        let mut g = with_tabs(3);
        let id = g.new_group(1).unwrap();
        g.assign(2, Some(id));
        g.remove_tab(1);
        assert!(g.get(id).is_some(), "one tab left");
        g.remove_tab(1);
        assert!(g.get(id).is_none() && g.list.is_empty(), "the group went with its last tab");
        assert_eq!(g.of_tab, vec![None]);
    }

    #[test]
    fn ungrouping_the_last_member_removes_the_group_and_groups_become_contiguous() {
        let mut g = with_tabs(4);
        let a = g.new_group(0).unwrap();
        g.assign(2, Some(a));
        assert_eq!(g.contiguous_order(), vec![0, 2, 1, 3]);
        g.apply_order(&[0, 2, 1, 3]);
        assert_eq!(g.of_tab, vec![Some(a), Some(a), None, None]);
        g.assign(0, None);
        g.assign(1, None);
        assert!(g.list.is_empty());
    }

    #[test]
    fn names_are_cleaned_and_colours_wrap() {
        let mut g = with_tabs(1);
        let id = g.new_group(0).unwrap();
        g.rename(id, "  Work\n things that are far too long to fit in a chip  ");
        let n = g.get(id).unwrap().name.clone();
        assert!(n.chars().count() <= NAME_MAX && n.starts_with("Work"), "{n}");
        g.rename(id, "   ");
        assert_eq!(g.get(id).unwrap().name, n, "an empty name keeps the old one");
        g.set_color(id, 13);
        assert_eq!(g.get(id).unwrap().color, 1);
    }

    #[test]
    fn the_card_button_steps_through_the_groups_and_back_to_none() {
        let mut g = with_tabs(3);
        let a = g.new_group(0).unwrap();
        let b = g.new_group(1).unwrap();
        assert_eq!(g.next_for(2), Some(a));
        g.assign(2, Some(a));
        assert_eq!(g.next_for(2), Some(b));
        g.assign(2, Some(b));
        assert_eq!(g.next_for(2), None);
    }

    #[test]
    fn saved_groups_keep_their_addresses() {
        let mut g = with_tabs(3);
        let id = g.new_group(1).unwrap();
        g.assign(2, Some(id));
        let json = g.saved_json(&["a".into(), "https://x.org/".into(), "".into()]);
        let back: Vec<Saved> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].urls, vec!["https://x.org/".to_string()], "a tab with no page has no address to keep");
    }
}
