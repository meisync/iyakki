//! The launcher's structure: named groups of tiles, and the geometry of the group chips, the tile grid and the page dots.
//! Pure functions (no drawing), so the shell, the tests and every front end (window, browser, framebuffer) agree.

use iyakki_sec::policy::App;

use crate::ui::R;

/// A section of the home screen, shown as a tab at the top. Every tile is in exactly one: the section is the `group` field of its
/// entry in the shell's tile table (Settings has none: it is in the IYAKKI menu, not on the grid). Custom apps go where their
/// app.json's `group` says, else Work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// The assistant: CHAT, RAG, TALES, CALENDAR, MCP, SKILLS, CLI.
    Home,
    WebMedia,
    Work,
    System,
}

impl Group {
    /// Tab order on the home screen.
    pub const CHIPS: [Group; 4] = [Group::Home, Group::WebMedia, Group::Work, Group::System];

    pub fn id(self) -> &'static str {
        match self {
            Group::Home => "home",
            Group::WebMedia => "web-media",
            Group::Work => "work",
            Group::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Home => "Home",
            Group::WebMedia => "Web & Media",
            Group::Work => "Work",
            Group::System => "System",
        }
    }

    /// A remembered section by its id (`assistant`, the old name of Home, still works; `all` and `my-apps` are gone).
    pub fn from_id(id: &str) -> Option<Group> {
        if id == "assistant" {
            return Some(Group::Home);
        }
        Group::CHIPS.into_iter().find(|g| g.id() == id)
    }

    /// The section an app's tile is in (`None` for apps without a tile on the grid: the inbox has the bell, Settings the menu).
    pub fn of(app: App) -> Option<Group> {
        crate::shell::tile_group(app)
    }

    pub fn holds(self, app: App) -> bool {
        Group::of(app) == Some(self)
    }
}

/// The section a custom app's `group` (from its app.json) puts it in: a built-in one when the name is one of theirs
/// ("Assistant" means Home), else Work.
pub fn group_of_custom(name: &str) -> Group {
    let n = name.trim().to_lowercase();
    if n == "assistant" {
        return Group::Home;
    }
    Group::CHIPS.into_iter().find(|g| g.label().to_lowercase() == n || g.id() == n).unwrap_or(Group::Work)
}

/// Apps whose tile wears the DEMO ribbon: they show sample data only and touch no real system.
pub fn is_demo(app: App) -> bool {
    app == App::SrePilot
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
        }
        assert_eq!(Group::from_id("assistant"), Some(Group::Home), "the old name of Home");
        assert_eq!(Group::from_id("all"), None);
        assert_eq!(Group::from_id("nope"), None);
        assert_eq!(group_of_custom("My apps"), Group::Work, "no declared section: Work");
        assert_eq!(group_of_custom("web & media"), Group::WebMedia);
        assert_eq!(group_of_custom("Assistant"), Group::Home);
    }

    #[test]
    fn the_sections_hold_the_tiles_the_owner_listed() {
        let of = |g: Group| -> Vec<App> { App::ALL.into_iter().filter(|a| g.holds(*a)).collect() };
        let mut home = of(Group::Home);
        home.sort_by_key(|a| a.id());
        assert_eq!(home, [App::Calendar, App::Chat, App::Cli, App::Mcp, App::Rag, App::Skills, App::Tales]);
        for a in [App::Browser, App::Radio, App::Media, App::Camera] {
            assert!(Group::WebMedia.holds(a), "{a:?}");
        }
        for a in [App::Ide, App::Edit, App::Data, App::Dash, App::Flows, App::Runs, App::SrePilot] {
            assert!(Group::Work.holds(a), "{a:?}");
        }
        for a in [App::System, App::Store, App::Users, App::Usage, App::Scan, App::Mesh] {
            assert!(Group::System.holds(a), "{a:?}");
        }
    }

    #[test]
    fn the_four_section_tabs_fit_a_phone_wide_browser() {
        for (w, h) in [(360u32, 740u32), (390, 844), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            let labels: Vec<&str> = Group::CHIPS.iter().map(|g| g.label()).collect();
            let r = chip_rects(s, &labels);
            let last = r.last().unwrap();
            assert!(last.x + last.w <= w as f32 - 8.0, "{w}x{h}: {r:?}");
            assert!(r.iter().all(|c| c.h >= 20.0), "{w}x{h}: tappable");
        }
    }

    #[test]
    fn empty_groups_are_hidden() {
        assert_eq!(groups_of(&[App::Chat, App::Rag]), vec![Group::Home]);
        assert_eq!(groups_of(&[App::Radio, App::Edit]), vec![Group::WebMedia, Group::Work]);
        assert_eq!(groups_of(&[]), Vec::<Group>::new());
        assert_eq!(groups_of(&App::ALL).len(), 4);
    }

    #[test]
    fn paging_follows_the_screen_and_the_text_size() {
        let (small, big) = (per_page(1024, 600, 0.8), per_page(1920, 1080, 1.5));
        assert!(small >= 6 && big >= 6, "{small} {big}");
        assert!(page_count(17, small) > 1, "17 tiles page on a small screen");
        assert_eq!(page_count(3, small), 1);
        // bigger text, fewer tiles per page
        assert!(per_page(1280, 720, 2.0) < per_page(1280, 720, 1.0));
        for (w, h, s) in [(1024u32, 600u32, 0.8f32), (1280, 720, 1.0), (1920, 1080, 1.5), (1024, 768, 0.8)] {
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
        for (w, h, s) in [(1024u32, 600u32, 0.8f32), (1920, 1080, 1.5)] {
            let chips = chip_rects(s, &labels);
            assert!(chips.last().unwrap().x + chips.last().unwrap().w < w as f32 - 40.0 * s);
            let nav = page_nav(w, h, s, 3);
            let tiles = page_rects(w, h, s, per_page(w, h, s) * 3, 0);
            assert!(chips.iter().all(|c| tiles.iter().all(|t| c.y + c.h <= t.y)));
            assert!(tiles.iter().all(|t| t.y + t.h <= nav.dots[0].y));
            assert_eq!(nav.dots.len(), 3);
        }
    }
}
