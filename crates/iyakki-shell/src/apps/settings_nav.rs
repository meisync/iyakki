//! The left side of Settings: a search box over a scrollable list of pages, grouped. Typing narrows the list to the pages
//! whose name, group or keywords contain every word typed ("wifi" finds Network, "volume" finds Sound).

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, TextField, CARD_HOVER, CARD_SEL, DIM, MUTED, R, TEXT};

/// One page in the list.
pub struct Item {
    pub tab: usize,
    pub label: &'static str,
    pub group: &'static str,
    /// Words that should find this page.
    pub keywords: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Group(&'static str),
    Item { tab: usize, label: &'static str },
}

/// The hit id of the search box (`Event::Focus` → `FocusAt`).
pub const SEARCH_ID: usize = 900;

const ROW_H: f32 = 34.0;
const GROUP_H: f32 = 30.0;
const SEARCH_H: f32 = 48.0;

/// Plain words people type that no page uses itself: what to look for instead. Each right-hand word is in some page's keywords.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("dark", &["brightness"]),
    ("darker", &["brightness"]),
    ("night", &["brightness"]),
    ("theme", &["brightness", "text size"]),
    ("sleep", &["screen", "brightness"]),
    ("screensaver", &["screen", "brightness"]),
    ("mouse", &["bluetooth", "keyboard"]),
    ("touchpad", &["bluetooth", "keyboard"]),
    ("trackpad", &["bluetooth", "keyboard"]),
    ("backup", &["storage"]),
    ("restore", &["storage"]),
    ("restart", &["storage"]),
    ("reboot", &["storage"]),
    ("shutdown", &["storage"]),
    ("name", &["host name", "account"]),
    ("language", &["voice", "keyboard"]),
    ("security", &["firewall", "vault", "account"]),
    ("privacy", &["firewall", "vault", "browser"]),
    ("safe", &["firewall", "vault"]),
    ("protect", &["firewall", "vault"]),
    ("assistant", &["ai", "model"]),
    ("chatgpt", &["ai", "model"]),
    ("gpt", &["ai", "model"]),
    ("online", &["network", "internet"]),
    ("offline", &["network", "internet"]),
    ("lan", &["network"]),
    ("speakers", &["sound"]),
    ("loud", &["sound", "volume"]),
    ("zoom", &["text size"]),
    ("battery", &["time", "bluetooth"]),
    ("slow", &["memory"]),
    ("update", &["install"]),
];

/// Does the page answer to what was typed? Every word must appear in its name, group or keywords, or one of its plain-word
/// synonyms must (so "dark", "sleep", "mouse" and "backup" lead somewhere sensible).
pub fn matches(query: &str, item: &Item) -> bool {
    let hay = format!("{} {} {}", item.label, item.group, item.keywords).to_lowercase();
    query.split_whitespace().all(|w| {
        let w = w.to_lowercase();
        hay.contains(&w) || SYNONYMS.iter().any(|(k, alts)| *k == w && alts.iter().any(|a| hay.contains(a)))
    })
}

/// The list to show: pages that match, each group only if one of its pages does.
pub fn entries(items: &[Item], query: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut groups: Vec<&'static str> = Vec::new();
    for it in items {
        if !groups.contains(&it.group) {
            groups.push(it.group);
        }
    }
    for g in groups {
        let found: Vec<&Item> = items.iter().filter(|i| i.group == g && matches(query, i)).collect();
        if found.is_empty() {
            continue;
        }
        out.push(Entry::Group(g));
        out.extend(found.into_iter().map(|i| Entry::Item { tab: i.tab, label: i.label }));
    }
    out
}

fn entry_height(e: &Entry, s: f32) -> f32 {
    match e {
        Entry::Group(_) => GROUP_H * s,
        Entry::Item { .. } => ROW_H * s,
    }
}

pub fn content_height(entries: &[Entry], s: f32) -> f32 {
    entries.iter().map(|e| entry_height(e, s)).sum()
}

/// The list's view height inside a nav rectangle of height `h` (below the search box).
pub fn view_height(h: f32, s: f32) -> f32 {
    (h - (SEARCH_H + 10.0) * s).max(0.0)
}

pub fn clamp_scroll(scroll: f32, entries: &[Entry], h: f32, s: f32) -> f32 {
    scroll.clamp_safe(0.0, (content_height(entries, s) - view_height(h, s)).max(0.0))
}

/// The smallest scroll that shows the row of `tab` (a group header above it too when it is the first of its group).
pub fn scroll_to_show(scroll: f32, entries: &[Entry], tab: usize, h: f32, s: f32) -> f32 {
    let view = view_height(h, s);
    let mut y = 0.0;
    for (i, e) in entries.iter().enumerate() {
        let eh = entry_height(e, s);
        if matches!(e, Entry::Item { tab: t, .. } if *t == tab) {
            let top = if i > 0 && matches!(entries[i - 1], Entry::Group(_)) { y - entry_height(&entries[i - 1], s) } else { y };
            let new = if top < scroll { top } else if y + eh > scroll + view { y + eh - view } else { scroll };
            return clamp_scroll(new, entries, h, s);
        }
        y += eh;
    }
    clamp_scroll(scroll, entries, h, s)
}

/// The first page that matches (what Enter in the search box opens).
pub fn first_tab(entries: &[Entry]) -> Option<usize> {
    entries.iter().find_map(|e| if let Entry::Item { tab, .. } = e { Some(*tab) } else { None })
}

/// The page before (`-1`) or after (`1`) `tab` among the matches.
pub fn step_tab(entries: &[Entry], tab: usize, d: i32) -> Option<usize> {
    let tabs: Vec<usize> = entries.iter().filter_map(|e| if let Entry::Item { tab, .. } = e { Some(*tab) } else { None }).collect();
    let pos = tabs.iter().position(|t| *t == tab);
    match pos {
        Some(p) => tabs.get((p as i32 + d).clamp_safe(0, tabs.len() as i32 - 1) as usize).copied(),
        None => tabs.first().copied(),
    }
}

/// Draw the whole left side into `r`.
#[allow(clippy::too_many_arguments)]
pub fn draw(g: &mut Gfx, r: R, s: f32, entries: &[Entry], selected: usize, scroll: f32, query: &TextField, focused: bool, blink_on: bool, accent: [u8; 4]) {
    g.rrect(r.x, r.y, r.w, r.h, 16.0 * s, rgb(16, 21, 40));
    let list_top = r.y + (SEARCH_H + 10.0) * s;
    let view = view_height(r.h, s);
    // a row half out of the list is cut at its edge, not drawn over the page's border or off the window
    g.clipped(R::new(r.x, list_top, r.w, view), |g| {
        let mut y = 0.0;
        for e in entries {
            let eh = entry_height(e, s);
            let (top, bottom) = (y - scroll, y - scroll + eh);
            y += eh;
            if bottom < 0.0 || top > view {
                continue;
            }
            let ry = list_top + top;
            match e {
                Entry::Group(name) => {
                    let head = g.fit(12.0 * s, &name.to_uppercase(), Face::Bold, r.w - 36.0 * s);
                    g.text(r.x + 18.0 * s, ry + 10.0 * s, 12.0 * s, DIM, &head, Face::Bold);
                }
                Entry::Item { tab, label } => {
                    let row = R::new(r.x + 8.0 * s, ry + 2.0 * s, r.w - 16.0 * s, eh - 4.0 * s);
                    let on = *tab == selected;
                    let hot = g.hovered(row);
                    if on || hot {
                        g.rrect(row.x, row.y, row.w, row.h, 10.0 * s, if on { CARD_SEL } else { CARD_HOVER });
                    }
                    if on {
                        g.rrect(row.x, row.y + 7.0 * s, 4.0 * s, row.h - 14.0 * s, 2.0 * s, accent);
                    }
                    let lh = g.line_height(15.0 * s, Face::Bold);
                    let face = if on { Face::Bold } else { Face::Sans };
                    // a page name longer than the list is wide is cut with an ellipsis (the hit region below still carries the whole name)
                    let shown = g.fit(15.0 * s, label, face, row.w - 26.0 * s);
                    g.text(row.x + 18.0 * s, row.y + (row.h - lh) / 2.0, 15.0 * s, if on { TEXT } else { MUTED }, &shown, face);
                    // only the part of the row inside the list is clickable (a half-visible row must not reach off the screen)
                    let (hy0, hy1) = (row.y.max(list_top), (row.y + row.h).min(list_top + view));
                    if hy1 > hy0 {
                        g.hit(R::new(row.x, hy0, row.w, hy1 - hy0), Event::Tab(*tab), label);
                    }
                }
            }
        }
    });
    if entries.is_empty() {
        let msg = g.fit(14.0 * s, &format!("Nothing matches “{}”.", query.value), Face::Sans, r.w - 36.0 * s);
        g.text(r.x + 18.0 * s, list_top + 12.0 * s, 14.0 * s, DIM, &msg, Face::Sans);
        for (i, l) in g.wrap(13.0 * s, "Try one simple word, such as wifi, sound, password or time. Esc clears the search.", Face::Sans, r.w - 36.0 * s).iter().take(4).enumerate() {
            g.text(r.x + 18.0 * s, list_top + (38.0 + i as f32 * 18.0) * s, 13.0 * s, DIM, l, Face::Sans);
        }
    }
    // a scroll bar when the list is longer than the box
    let total = content_height(entries, s);
    if total > view && view > 0.0 {
        let bh = (view * view / total).max(28.0 * s);
        let by = list_top + (view - bh) * (scroll / (total - view).max(1.0));
        g.rrect(r.x + r.w - 7.0 * s, by, 4.0 * s, bh, 2.0 * s, rgb(60, 72, 110));
    }
    // the search box covers whatever scrolled under it
    g.rrect(r.x, r.y, r.w, (SEARCH_H + 10.0) * s, 16.0 * s, rgb(16, 21, 40));
    query.render(g, R::new(r.x + 10.0 * s, r.y + 8.0 * s, r.w - 20.0 * s, SEARCH_H * s - 6.0 * s), focused, s, blink_on, Face::Sans, SEARCH_ID);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        vec![
            Item { tab: 0, label: "Models", group: "Assistant", keywords: "ai chat llm openai claude ollama api" },
            Item { tab: 1, label: "RAG", group: "Assistant", keywords: "search documents knowledge chunk embedding" },
            Item { tab: 2, label: "Network", group: "Devices", keywords: "wifi ethernet internet ip" },
            Item { tab: 3, label: "Sound", group: "Devices", keywords: "volume audio speaker mute" },
            Item { tab: 4, label: "Account", group: "You", keywords: "password sign out" },
        ]
    }

    fn tabs(e: &[Entry]) -> Vec<usize> {
        e.iter().filter_map(|x| if let Entry::Item { tab, .. } = x { Some(*tab) } else { None }).collect()
    }

    #[test]
    fn typing_narrows_the_list_by_name_group_or_keyword_and_hides_empty_groups() {
        let it = items();
        assert_eq!(entries(&it, "").len(), 5 + 3, "everything, with three group headings");
        assert_eq!(tabs(&entries(&it, "wifi")), vec![2]);
        assert_eq!(entries(&it, "wifi")[0], Entry::Group("Devices"));
        assert_eq!(entries(&it, "wifi").len(), 2, "the Assistant and You groups are gone");
        assert_eq!(tabs(&entries(&it, "VOLUME")), vec![3], "case does not matter");
        assert_eq!(tabs(&entries(&it, "devices")), vec![2, 3], "a group name finds its pages");
        assert_eq!(tabs(&entries(&it, "search docu")), vec![1], "every word must match, in any order");
        assert_eq!(tabs(&entries(&it, "docu search")), vec![1]);
        assert!(entries(&it, "zzz").is_empty());
        assert_eq!(tabs(&entries(&it, "  ")), vec![0, 1, 2, 3, 4], "blanks match everything");
    }

    #[test]
    fn enter_and_arrows_move_among_the_matches() {
        let e = entries(&items(), "a");
        assert_eq!(first_tab(&e), Some(0));
        assert_eq!(step_tab(&e, 0, 1), Some(1));
        assert_eq!(step_tab(&e, 4, 1), Some(4), "no further than the last");
        assert_eq!(step_tab(&e, 0, -1), Some(0));
        assert_eq!(step_tab(&entries(&items(), "wifi"), 4, 1), Some(2), "from a page that is not listed, go to the first match");
        assert_eq!(first_tab(&[]), None);
        assert_eq!(step_tab(&[], 0, 1), None);
    }

    #[test]
    fn scrolling_keeps_the_chosen_page_in_view_and_never_past_the_ends() {
        let it: Vec<Item> = (0..30).map(|i| Item { tab: i, label: "Page", group: "G", keywords: "" }).collect();
        let e = entries(&it, "");
        let (h, s) = (400.0, 1.0);
        let view = view_height(h, s);
        assert!(content_height(&e, s) > view);
        assert_eq!(clamp_scroll(-50.0, &e, h, s), 0.0);
        let max = content_height(&e, s) - view;
        assert_eq!(clamp_scroll(1e9, &e, h, s), max);
        // choosing a page far down scrolls just enough to show it
        let sc = scroll_to_show(0.0, &e, 25, h, s);
        assert!(sc > 0.0 && sc <= max);
        let row_bottom = GROUP_H + 26.0 * ROW_H;
        assert!(sc + view >= row_bottom - 0.01 && sc + view < row_bottom + ROW_H, "the row is the last thing in view: {sc}");
        // going back up shows the group heading too
        assert_eq!(scroll_to_show(sc, &e, 0, h, s), 0.0);
        // a page already in view does not move the list
        assert_eq!(scroll_to_show(0.0, &e, 2, h, s), 0.0);
        // a short list never scrolls
        assert_eq!(clamp_scroll(30.0, &entries(&items(), ""), 800.0, 1.0), 0.0);
    }

    #[test]
    fn the_list_draws_registers_a_click_per_visible_page_and_the_search_box() {
        let it = items();
        let e = entries(&it, "");
        let mut g = Gfx::new(600, 700);
        g.clear(rgb(0, 0, 0));
        let q = TextField::new("Search settings", false);
        draw(&mut g, R::new(10.0, 10.0, 260.0, 600.0), 1.0, &e, 1, 0.0, &q, false, true, rgb(120, 170, 255));
        for t in 0..5 {
            assert!(g.hits.iter().any(|h| h.ev == Event::Tab(t)), "page {t}");
        }
        assert!(g.hits.iter().any(|h| h.ev == Event::Focus(SEARCH_ID)), "the search box takes clicks");
        // a narrow, short box shows only some pages (the rest are reached by scrolling)
        g.hits.clear();
        draw(&mut g, R::new(10.0, 10.0, 200.0, 180.0), 1.0, &e, 0, 0.0, &q, false, true, rgb(120, 170, 255));
        let shown = (0..5).filter(|t| g.hits.iter().any(|h| h.ev == Event::Tab(*t))).count();
        assert!(shown >= 1 && shown < 5, "{shown}");
        // nothing found says so
        g.hits.clear();
        draw(&mut g, R::new(10.0, 10.0, 260.0, 600.0), 1.0, &[], 0, 0.0, &q, false, true, rgb(120, 170, 255));
        assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(_))));
    }
}
