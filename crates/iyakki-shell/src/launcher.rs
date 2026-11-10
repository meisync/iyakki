//! The launcher's structure: named groups of tiles, and the geometry of the group chips, the tile grid and the page dots.
//! Pure functions (no drawing), so the shell, the tests and every front end (window, browser, framebuffer) agree.

use iyakki_sec::policy::App;

use crate::ui::R;

/// A built-in section of the home screen, shown as a tab at the top. Every tile is in exactly one: the section is the `group` field of
/// its entry in the shell's tile table (the inbox has none: it is the bell; Settings has none either: it is an icon in the top bar).
/// The ids are those of the first four-group layout (`home`, `web-media`, `work`, `system`) so saved layouts keep working.
/// Custom and Store apps go where their app.json's `group` says, else Apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Ask & Create: the assistant and what it makes (CHAT, RAG, SKILLS, CALENDAR, TALES, VOICE, LAB).
    Home,
    /// Build: AI IDE, GIT, FLOWS, K8S, EDIT, CLI, MCP, RUNS.
    Work,
    /// Media: BROWSER, RADIO, MEDIA, CAMERA.
    WebMedia,
    /// Data: DATA, DASH, USAGE.
    Data,
    /// Devices & System: SYSTEM, DISKS, NET SCAN, MESH, USERS, STORE, LINUX WINDOW.
    System,
    /// Tiles installed later (custom tile apps, Store apps) until the person moves them.
    Apps,
}

impl Group {
    /// Tab order on the home screen.
    pub const CHIPS: [Group; 6] = [Group::Home, Group::Work, Group::WebMedia, Group::Data, Group::System, Group::Apps];

    pub fn id(self) -> &'static str {
        match self {
            Group::Home => "home",
            Group::WebMedia => "web-media",
            Group::Work => "work",
            Group::Data => "data",
            Group::System => "system",
            Group::Apps => "apps",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Home => "Ask & Create",
            Group::Work => "Build",
            Group::WebMedia => "Media",
            Group::Data => "Data",
            Group::System => "Devices & System",
            Group::Apps => "Apps",
        }
    }

    /// A remembered section by its id (`assistant`, the old name of Home, still works; `all` and `my-apps` are gone).
    pub fn from_id(id: &str) -> Option<Group> {
        if id == "assistant" {
            return Some(Group::Home);
        }
        Group::CHIPS.into_iter().find(|g| g.id() == id)
    }

    /// The section an app's tile is in (`None` for apps without a tile on the grid: the inbox has the bell, Settings the top bar).
    pub fn of(app: App) -> Option<Group> {
        crate::shell::tile_group(app)
    }

    pub fn holds(self, app: App) -> bool {
        Group::of(app) == Some(self)
    }
}

/// The section a custom app's `group` (from its app.json) puts it in: a built-in one when the name is one of theirs (or an old
/// name: "Assistant" and "Home" mean Ask & Create, "Web & Media" Media), else Apps.
pub fn group_of_custom(name: &str) -> Group {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "assistant" => return Group::Home,
        "web & media" => return Group::WebMedia,
        _ => {}
    }
    Group::CHIPS.into_iter().find(|g| g.label().to_lowercase() == n || g.id() == n).unwrap_or(Group::Apps)
}

/// The tabs to show for these apps: every section that has at least one of them (empty sections are hidden).
pub fn groups_of(apps: &[App]) -> Vec<Group> {
    groups_with(apps, &[])
}

/// Like [`groups_of`], with the groups that custom apps are in.
pub fn groups_with(apps: &[App], custom: &[Group]) -> Vec<Group> {
    Group::CHIPS.into_iter().filter(|g| apps.iter().any(|a| g.holds(*a)) || custom.contains(g)).collect()
}

const TOP: f32 = 140.0;
const BOTTOM: f32 = 84.0;
const GAP: f32 = 28.0;
const MAX_TILE: f32 = 250.0;
/// The narrowest a tile gets before the next one goes to another page (tile height is 1.32 times the width).
const MIN_TILE: f32 = 140.0;
const RATIO: f32 = 1.32;

/// How many tile columns and rows fit the screen at least `MIN_TILE` wide.
fn grid(w: u32, h: u32, s: f32) -> (usize, usize) {
    let avail = w as f32 - 80.0 * s;
    let room = (h as f32 - (TOP + BOTTOM) * s).max(120.0 * s);
    let cols = (((avail + GAP * s) / (MIN_TILE * s + GAP * s)).floor() as usize).max(1);
    let rows = (((room + GAP * s) / (MIN_TILE * RATIO * s + GAP * s)).floor() as usize).max(1);
    (cols, rows)
}

/// Tiles per page at this size.
pub fn per_page(w: u32, h: u32, s: f32) -> usize {
    let (c, r) = grid(w, h, s);
    c * r
}

pub fn page_count(n: usize, cap: usize) -> usize {
    n.max(1).div_ceil(cap.max(1))
}

/// The rectangles of the tiles on page `page` of `n` tiles: every page uses the same tile size and rows, the rows
/// are centred, and a short last page keeps the top-left positions' size.
pub fn page_rects(w: u32, h: u32, s: f32, n: usize, page: usize) -> Vec<R> {
    let gap = GAP * s;
    let (max_cols, max_rows) = grid(w, h, s);
    let cap = max_cols * max_rows;
    let n = n.max(1);
    let on_first = n.min(cap);
    let rows = on_first.div_ceil(max_cols);
    let cols = on_first.div_ceil(rows);
    let avail = w as f32 - 80.0 * s;
    let room = (h as f32 - (TOP + BOTTOM) * s).max(120.0 * s);
    let mut tile_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).min(MAX_TILE * s);
    tile_w = tile_w.min(((room - gap * (rows as f32 - 1.0)) / rows as f32 / RATIO).max(60.0 * s));
    let tile_h = tile_w * RATIO;
    let total_h = tile_h * rows as f32 + gap * (rows as f32 - 1.0);
    let y0 = TOP * s + ((room - total_h) / 2.0).max(0.0);
    let first = page * cap;
    let m = n.saturating_sub(first).min(cap);
    let mut out = Vec::with_capacity(m);
    for r in 0..m.div_ceil(cols) {
        let in_row = (m - r * cols).min(cols);
        let row_w = tile_w * in_row as f32 + gap * (in_row as f32 - 1.0);
        let x0 = (w as f32 - row_w) / 2.0;
        for c in 0..in_row {
            out.push(R::new(x0 + c as f32 * (tile_w + gap), y0 + r as f32 * (tile_h + gap), tile_w, tile_h));
        }
    }
    out
}

/// The tiles of the first page (what fits at once).
pub fn tile_rects(w: u32, h: u32, s: f32, n: usize) -> Vec<R> {
    page_rects(w, h, s, n, 0)
}

/// Width of a chip for its label (the drawing shrinks text that would not fit, so this only has to be consistent).
fn chip_width(label: &str, s: f32) -> f32 {
    (label.chars().count() as f32 * 9.5 + 34.0) * s
}

/// The group chips, left to right under the title.
pub fn chip_rects(s: f32, labels: &[&str]) -> Vec<R> {
    let mut x = 40.0 * s;
    labels
        .iter()
        .map(|l| {
            let w = chip_width(l, s);
            let r = R::new(x, 84.0 * s, w, 36.0 * s);
            x += w + 10.0 * s;
            r
        })
        .collect()
}

/// The chip row with its two extra chips: `+` (a new group) at the end and, when the groups do not all fit the width, `…`
/// (the rest, in a menu) before it. The shown group's chip is always visible.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipRow {
    /// (index into the labels, rectangle) of the chips shown, left to right.
    pub chips: Vec<(usize, R)>,
    /// The `…` chip, when some groups are hidden behind it.
    pub more: Option<R>,
    /// The indexes of the hidden groups.
    pub hidden: Vec<usize>,
    pub plus: R,
}

/// Lay out the chips for a screen `w` wide. With room for everything the chips sit where [`chip_rects`] puts them.
pub fn chip_row(w: u32, s: f32, labels: &[&str], current: usize) -> ChipRow {
    let (x0, limit, gap) = (40.0 * s, w as f32 - 40.0 * s, 10.0 * s);
    let ws: Vec<f32> = labels.iter().map(|l| chip_width(l, s)).collect();
    let extra = chip_width("+", s);
    let used = |idx: &[usize]| idx.iter().map(|i| ws[*i]).sum::<f32>() + gap * idx.len().saturating_sub(1) as f32;
    let all: Vec<usize> = (0..labels.len()).collect();
    let (shown, more) = if x0 + used(&all) + gap + extra <= limit || labels.len() <= 1 {
        (all, false)
    } else {
        // as many from the left as fit beside `…` and `+`, the shown group's chip taking the last place when it is further right
        let mut pick = Vec::new();
        for k in (1..labels.len()).rev() {
            let mut v: Vec<usize> = (0..k).collect();
            if current >= k {
                v[k - 1] = current;
            }
            if x0 + used(&v) + 2.0 * (gap + extra) <= limit {
                pick = v;
                break;
            }
        }
        if pick.is_empty() {
            pick = vec![current.min(labels.len() - 1)];
        }
        (pick, true)
    };
    let mut x = x0;
    let mut chips = Vec::new();
    for i in &shown {
        chips.push((*i, R::new(x, 84.0 * s, ws[*i], 36.0 * s)));
        x += ws[*i] + gap;
    }
    let hidden: Vec<usize> = (0..labels.len()).filter(|i| !shown.contains(i)).collect();
    let more_r = more.then(|| {
        let r = R::new(x, 84.0 * s, extra, 36.0 * s);
        x += extra + gap;
        r
    });
    ChipRow { chips, more: more_r, hidden, plus: R::new(x, 84.0 * s, extra, 36.0 * s) }
}

/// The page controls: previous arrow, one dot per page, next arrow, centred above the hint line.
pub struct PageNav {
    pub prev: R,
    pub next: R,
    pub dots: Vec<R>,
}

pub fn page_nav(w: u32, h: u32, s: f32, pages: usize) -> PageNav {
    let (d, gap) = (26.0 * s, 8.0 * s);
    let total = d * (pages as f32 + 2.0) + gap * (pages as f32 + 1.0);
    let x0 = (w as f32 - total) / 2.0;
    let y = h as f32 - 78.0 * s;
    let at = |i: usize| R::new(x0 + i as f32 * (d + gap), y, d, d);
    PageNav { prev: at(0), dots: (0..pages).map(|p| at(p + 1)).collect(), next: at(pages + 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tile_app_is_in_exactly_one_section_and_the_inbox_and_settings_in_none() {
        for a in App::ALL {
            let n = Group::CHIPS.iter().filter(|g| g.holds(a)).count();
            assert_eq!(n, if matches!(a, App::Inbox | App::Settings) { 0 } else { 1 }, "{a:?}");
        }
        for g in Group::CHIPS {
            assert_eq!(Group::from_id(g.id()), Some(g));
            assert!(iyakki_agent::layout::is_builtin(g.id()) && iyakki_agent::layout::BUILTIN.iter().any(|(i, l)| *i == g.id() && *l == g.label()), "{g:?}: the layout and the launcher agree on the built-in groups");
        }
        assert_eq!(Group::CHIPS.len(), iyakki_agent::layout::BUILTIN.len());
        assert_eq!(Group::from_id("assistant"), Some(Group::Home), "the old name of Home");
        assert_eq!(Group::from_id("all"), None);
        assert_eq!(Group::from_id("nope"), None);
        assert_eq!(group_of_custom("My apps"), Group::Apps, "no declared section: Apps");
        assert_eq!(group_of_custom("web & media"), Group::WebMedia);
        assert_eq!(group_of_custom("Media"), Group::WebMedia);
        assert_eq!(group_of_custom("Work"), Group::Work);
        assert_eq!(group_of_custom("Assistant"), Group::Home);
        assert_eq!(group_of_custom(""), Group::Apps);
    }

    #[test]
    fn the_default_groups_hold_the_tiles_the_owner_listed() {
        let of = |g: Group| -> Vec<App> { App::ALL.into_iter().filter(|a| g.holds(*a)).collect() };
        let mut home = of(Group::Home);
        home.sort_by_key(|a| a.id());
        assert_eq!(home, [App::Calendar, App::Chat, App::Lab, App::Learn, App::Rag, App::Skills, App::Tales, App::Voice]);
        for a in [App::Chat, App::Tales, App::Voice] {
            assert!(Group::Home.holds(a), "Ask & Create: {a:?}");
        }
        for a in [App::Ide, App::Git, App::Flows, App::K8s, App::Edit] {
            assert!(Group::Work.holds(a), "Build: {a:?}");
        }
        for a in [App::Browser, App::Radio, App::Media, App::Camera] {
            assert!(Group::WebMedia.holds(a), "Media: {a:?}");
        }
        for a in [App::Data, App::Dash] {
            assert!(Group::Data.holds(a), "Data: {a:?}");
        }
        for a in [App::System, App::Store, App::Users, App::Scan, App::Mesh, App::Disks, App::Doctor] {
            assert!(Group::System.holds(a), "Devices & System: {a:?}");
        }
        assert!(of(Group::Apps).is_empty(), "Apps holds nothing built in: installed tiles land there");
    }

    #[test]
    fn the_section_tabs_fit_a_phone_wide_browser_beside_plus_and_more() {
        for (w, h) in [(360u32, 740u32), (390, 844), (800, 480), (480, 600)] {
            let s = crate::shell::scale_for(w, h);
            let labels: Vec<&str> = Group::CHIPS.iter().map(|g| g.label()).collect();
            for cur in 0..labels.len() {
                let row = chip_row(w, s, &labels, cur);
                let mut rects: Vec<R> = row.chips.iter().map(|(_, r)| *r).collect();
                rects.extend(row.more);
                rects.push(row.plus);
                assert!(rects.last().unwrap().x + rects.last().unwrap().w <= w as f32 - 8.0, "{w}x{h}: {rects:?}");
                assert!(rects.iter().all(|c| c.h >= 20.0), "{w}x{h}: tappable");
                assert!(row.chips.iter().any(|(i, _)| *i == cur), "{w}x{h}: the shown group is visible");
                assert_eq!(row.chips.len() + row.hidden.len(), labels.len(), "every group is a chip or in the more menu");
            }
        }
        // the six groups and `+` fit a laptop screen with no `…`
        let labels: Vec<&str> = Group::CHIPS.iter().map(|g| g.label()).collect();
        for (w, h) in [(1280u32, 720u32), (1024, 600)] {
            assert!(chip_row(w, crate::shell::scale_for(w, h), &labels, 0).more.is_none(), "{w}x{h}");
        }
    }

    #[test]
    fn empty_groups_are_hidden() {
        assert_eq!(groups_of(&[App::Chat, App::Rag]), vec![Group::Home]);
        assert_eq!(groups_of(&[App::Radio, App::Edit]), vec![Group::Work, Group::WebMedia]);
        assert_eq!(groups_of(&[]), Vec::<Group>::new());
        assert_eq!(groups_of(&App::ALL).len(), 5, "Apps has no built-in tile: it shows with the first installed one");
        assert_eq!(groups_with(&App::ALL, &[Group::Apps]).len(), 6);
    }

    #[test]
    fn paging_follows_the_screen_and_the_text_size() {
        let (small, big) = (per_page(1024, 600, 0.8), per_page(1920, 1080, 1.5));
        assert!(small >= 6 && big >= 6, "{small} {big}");
        assert!(page_count(17, small) > 1, "17 tiles page on a small screen");
        assert_eq!(page_count(3, small), 1);
        // bigger text, fewer tiles per page
        assert!(per_page(1280, 720, 2.0) < per_page(1280, 720, 1.0));
        for (w, h) in [(1024u32, 600u32), (1280, 720), (1920, 1080), (1024, 768), (800, 480), (480, 600)] {
            let s = crate::shell::scale_for(w, h);
            let cap = per_page(w, h, s);
            let n = cap * 2 + 1;
            let mut seen = 0;
            for p in 0..page_count(n, cap) {
                let rs = page_rects(w, h, s, n, p);
                seen += rs.len();
                for r in &rs {
                    assert!(r.w >= 120.0 * s - 0.5 || cap == 1, "tile stays readable {r:?} at {w}x{h}");
                    assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y >= 100.0 * s && r.y + r.h <= h as f32 - 60.0 * s, "{r:?} at {w}x{h}");
                }
                for (i, a) in rs.iter().enumerate() {
                    assert!(rs[i + 1..].iter().all(|b| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y || b.y + b.h <= a.y), "overlap at {w}x{h}");
                }
            }
            assert_eq!(seen, n);
        }
    }

    #[test]
    fn chips_and_dots_do_not_overlap_each_other_or_the_tiles() {
        let labels: Vec<&str> = Group::CHIPS.iter().map(|g| g.label()).collect();
        for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5)] {
            let chips = chip_rects(s, &labels);
            assert!(chips.last().unwrap().x + chips.last().unwrap().w < w as f32 - 40.0 * s);
            let nav = page_nav(w, h, s, 3);
            let tiles = page_rects(w, h, s, per_page(w, h, s) * 3, 0);
            assert!(chips.iter().all(|c| tiles.iter().all(|t| c.y + c.h <= t.y)));
            assert!(tiles.iter().all(|t| t.y + t.h <= nav.dots[0].y));
            assert_eq!(nav.dots.len(), 3);
        }
    }

    #[test]
    fn the_chip_row_scrolls_into_a_more_chip_and_always_fits_and_shows_the_current_group() {
        let long: Vec<String> = (0..12).map(|i| format!("Group number {i:02}")).collect();
        let sets: Vec<Vec<&str>> = vec![vec!["Ask & Create", "Build", "Media", "Data", "Devices & System", "Apps"], long.iter().map(|s| s.as_str()).collect(), vec!["Ask & Create", "Build", "Media", "Data", "Devices & System", "Apps", "Games", "Reading", "A 16 chars label"]];
        for (w, h) in [(360u32, 740u32), (390, 844), (800, 480), (1280, 720), (1920, 1080)] {
            let s = crate::shell::scale_for(w, h);
            for labels in &sets {
                for cur in 0..labels.len() {
                    let row = chip_row(w, s, labels, cur);
                    let mut rects: Vec<R> = row.chips.iter().map(|(_, r)| *r).collect();
                    rects.extend(row.more);
                    rects.push(row.plus);
                    for r in &rects {
                        assert!(r.x >= 0.0 && r.x + r.w <= w as f32, "{w}x{h} {cur}: {r:?} outside the screen");
                    }
                    for pair in rects.windows(2) {
                        assert!(pair[0].x + pair[0].w <= pair[1].x, "{w}x{h}: overlap {pair:?}");
                    }
                    assert!(row.chips.iter().any(|(i, _)| *i == cur), "{w}x{h}: the shown group {cur} is visible");
                    assert_eq!(row.chips.len() + row.hidden.len(), labels.len(), "every group is shown or in the more menu");
                    assert_eq!(row.more.is_some(), !row.hidden.is_empty());
                    assert!(rects.iter().all(|r| r.h >= 20.0), "tappable");
                }
            }
        }
        // with room the chips sit where chip_rects puts them
        let six = ["Ask & Create", "Build", "Media", "Data", "Devices & System", "Apps"];
        let s = crate::shell::scale_for(1280, 720);
        let row = chip_row(1280, s, &six, 2);
        assert_eq!(row.chips.iter().map(|(_, r)| *r).collect::<Vec<_>>(), chip_rects(s, &six));
        // many groups on a phone: a `…` chip appears
        assert!(chip_row(360, crate::shell::scale_for(360, 740), &sets[1], 0).more.is_some());
    }
}
