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
    /// Build: AI IDE, GIT, FLOWS, K8S, EDIT, CLI, MCP, RUNS, SAMPLE (the UI SDK example).
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
/// Room under the tiles for the island's edge, the page dots (`page_dots`) and the paging arrows of a group with several screens.
const BOTTOM: f32 = 116.0;
/// The same room in a short window (see [`bottom`]): the page dots move down towards the hint line so a tile still fits above them.
const BOTTOM_SHORT: f32 = 80.0;

/// The room kept under the tiles (unscaled): the usual one, or the smaller one when the window is so short that the usual one would leave
/// less than a tile's height (800x480 at the largest text size).
fn bottom(h: u32, s: f32) -> f32 {
    if (h as f32) - (TOP + BOTTOM) * s < 120.0 * s {
        BOTTOM_SHORT
    } else {
        BOTTOM
    }
}

/// What the person's Look changes about the geometry: how many icons across, how big a cell is and how it is spaced, and the room a dock
/// takes at the bottom or a side (in unscaled pixels). The defaults are the launcher as it always was, so no Look means no change. The
/// shell sets it from the Look in force (`Shell::sync_look`); the geometry functions stay pure functions of their arguments and these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Most icons across one screen of the island (a group with more goes on to another screen).
    pub max_cols: usize,
    /// A fixed number of columns (the Look's `columns = 3..8`); `None`: as many as fit.
    pub fixed_cols: Option<usize>,
    /// The narrowest a cell gets before the next one goes to another page.
    pub min_tile: f32,
    pub max_tile: f32,
    /// Cell height over cell width (1.32 for a stacked icon and label; a list row is much flatter).
    pub ratio: f32,
    pub gap: f32,
    pub inset_l: f32,
    pub inset_r: f32,
    pub inset_b: f32,
}

impl Default for Params {
    fn default() -> Params {
        Params { max_cols: 7, fixed_cols: None, min_tile: 140.0, max_tile: 210.0, ratio: 1.32, gap: 28.0, inset_l: 0.0, inset_r: 0.0, inset_b: 0.0 }
    }
}

thread_local! {
    static PARAMS: std::cell::Cell<Params> = std::cell::Cell::new(Params::default());
}

pub fn params() -> Params {
    PARAMS.with(|p| p.get())
}

pub fn set_params(p: Params) {
    PARAMS.with(|c| c.set(p));
}

/// Back to the launcher as it always was (the pure geometry tests start here).
pub fn reset_params() {
    set_params(Params::default());
}

/// The dock does not grow with the text size past this: a big text size must not push the dock over the tiles.
pub fn dock_scale(s: f32) -> f32 {
    s.min(1.25)
}

/// The horizontal room the tiles have: from the left edge of the area to its right edge.
fn area_x(w: u32, s: f32) -> (f32, f32) {
    let p = params();
    let d = dock_scale(s);
    (p.inset_l * d, (w as f32 - p.inset_r * d).max(p.inset_l * d + 1.0))
}

fn room_h(h: u32, s: f32) -> f32 {
    (h as f32 - (TOP + bottom(h, s)) * s - params().inset_b * dock_scale(s)).max(if bottom(h, s) == BOTTOM_SHORT { 60.0 } else { 120.0 } * s)
}

/// How many tile columns and rows fit the screen at least `MIN_TILE` wide.
fn grid(w: u32, h: u32, s: f32) -> (usize, usize) {
    let p = params();
    let (xl, xr) = area_x(w, s);
    let avail = xr - xl - 80.0 * s;
    let room = room_h(h, s);
    let fit = (((avail + p.gap * s) / (p.min_tile * s + p.gap * s)).floor() as usize).max(1).min(p.max_cols);
    // a fixed column count is honoured while the screen can hold it, and never more than a tile can be narrow
    let cols = p.fixed_cols.map_or(fit, |c| c.min(fit.max(1)).max(1));
    let rows = (((room + p.gap * s) / (p.min_tile * p.ratio * s + p.gap * s)).floor() as usize).max(1);
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
    let p = params();
    let gap = p.gap * s;
    let (max_cols, max_rows) = grid(w, h, s);
    let cap = max_cols * max_rows;
    let n = n.max(1);
    let on_first = n.min(cap);
    let rows = on_first.div_ceil(max_cols);
    let cols = on_first.div_ceil(rows);
    let (xl, xr) = area_x(w, s);
    let avail = xr - xl - 80.0 * s;
    let room = room_h(h, s);
    let mut tile_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).min(p.max_tile * s);
    tile_w = tile_w.min(((room - gap * (rows as f32 - 1.0)) / rows as f32 / p.ratio).max(60.0 * s));
    let tile_h = tile_w * p.ratio;
    let total_h = tile_h * rows as f32 + gap * (rows as f32 - 1.0);
    let y0 = TOP * s + ((room - total_h) / 2.0).max(0.0);
    let first = page * cap;
    let m = n.saturating_sub(first).min(cap);
    let mut out = Vec::with_capacity(m);
    for r in 0..m.div_ceil(cols) {
        let in_row = (m - r * cols).min(cols);
        let row_w = tile_w * in_row as f32 + gap * (in_row as f32 - 1.0);
        let x0 = (xl + xr - row_w) / 2.0;
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

/// The frosted island behind the icons of a group: the tiles' bounding box with room around it, clear of the chip row above and of the
/// page dots below, and inside the screen. The same on every screen of a group (it is cut from the first screen's tiles).
pub fn island_rect(w: u32, h: u32, s: f32, n: usize) -> R {
    let tiles = page_rects(w, h, s, n, 0);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0.0f32, 0.0f32);
    for t in &tiles {
        x0 = x0.min(t.x);
        y0 = y0.min(t.y);
        x1 = x1.max(t.x + t.w);
        y1 = y1.max(t.y + t.h);
    }
    if tiles.is_empty() {
        let (xl, xr) = area_x(w, s);
        return R::new(xl + 40.0 * s, TOP * s, (xr - xl - 80.0 * s).max(1.0), room_h(h, s));
    }
    let pad = 36.0 * s;
    let (axl, axr) = area_x(w, s);
    let left = (x0 - pad).max(axl + 12.0 * s);
    let top = (y0 - 14.0 * s).max(126.0 * s);
    let right = (x1 + pad).min(axr - 12.0 * s);
    let bottom = (y1 + 18.0 * s).min(h as f32 - (bottom(h, s) - 4.0) * s - params().inset_b * dock_scale(s));
    R::new(left, top, (right - left).max(1.0), (bottom - top).max(1.0))
}

/// The island of the machine page: the whole area between the chip row and the page dots.
pub fn machine_rect(w: u32, h: u32, s: f32) -> R {
    let (xl, xr) = area_x(w, s);
    let (x, y) = (xl + 40.0 * s, 126.0 * s);
    R::new(x, y, (xr - 40.0 * s - x).max(1.0), (h as f32 - (bottom(h, s) - 4.0) * s - params().inset_b * dock_scale(s) - y).max(1.0))
}

/// Centre of the dot row's vertical band.
fn dots_y(h: u32, s: f32) -> f32 {
    h as f32 - (bottom(h, s) - 20.0) * s - params().inset_b * dock_scale(s)
}

/// One hit rectangle per page (the machine page first, then the groups), centred under the island. Each is a comfortable touch target
/// that stays clear of its neighbours; the drawn dot is smaller.
pub fn page_dots(w: u32, h: u32, s: f32, pages: usize) -> Vec<R> {
    let pages = pages.max(1);
    let short = bottom(h, s) == BOTTOM_SHORT;
    // in a short window the touch targets are smaller and the rows sit tight under the tiles, so the dots, the group's own page buttons and
    // the hint line do not run into each other (a person can still hit a 20 px target at that size)
    let d = if short { 20.0 } else { 26.0 } * s;
    let k = ((w as f32 - 16.0 * s) / (d * pages as f32)).min(1.0);
    let d = d * k;
    let x0 = (w as f32 - d * pages as f32) / 2.0;
    let cy = if short { h as f32 - 69.0 * s - params().inset_b * dock_scale(s) } else { dots_y(h, s) };
    (0..pages).map(|i| R::new(x0 + i as f32 * d, cy - d / 2.0, d, d)).collect()
}

/// The paging arrows at the screen's left and right edges, level with the middle of the island: (previous, next).
pub fn edge_chevrons(w: u32, h: u32, s: f32) -> (R, R) {
    let (cw, ch) = (34.0 * s, 64.0 * s);
    let y = (TOP * s + room_h(h, s) / 2.0 - ch / 2.0).max(0.0);
    (R::new(2.0 * s, y, cw, ch), R::new(w as f32 - 2.0 * s - cw, y, cw, ch))
}

/// The dock: its plate and one square per pinned app (the icon's box, `size` px at the base scale), for a dock of `n` apps at `pos`. At the
/// bottom it sits above the hint line and is centred; at a side it is centred in the height. `Off` gives an empty plate.
pub fn dock_rects(w: u32, h: u32, s: f32, n: usize, size: u16, pos: iyakki_uikit::look::DockPos) -> (R, Vec<R>) {
    use iyakki_uikit::look::DockPos;
    let s = dock_scale(s);
    let (item, gap, pad) = (size as f32 * s, 12.0 * s, 12.0 * s);
    let long = n as f32 * item + (n.max(1) as f32 - 1.0) * gap + 2.0 * pad;
    let short = item + 28.0 * s;
    match pos {
        DockPos::Left | DockPos::Right => {
            let x = if pos == DockPos::Left { 14.0 * s } else { w as f32 - 14.0 * s - short };
            let y = ((h as f32 - long) / 2.0).max(126.0 * s);
            let plate = R::new(x, y, short, long);
            (plate, (0..n).map(|i| R::new(x + (short - item) / 2.0, y + pad + i as f32 * (item + gap), item, item)).collect())
        }
        _ => {
            let x = ((w as f32 - long) / 2.0).max(4.0 * s);
            let y = h as f32 - 52.0 * s - short;
            let plate = R::new(x, y, long.min(w as f32 - 8.0 * s), short);
            (plate, (0..n).map(|i| R::new(x + pad + i as f32 * (item + gap), y + pad, item, item)).collect())
        }
    }
}

/// The strip along the screen edge where the pointer wakes a hidden dock (autohide): 10 px at the base scale, the whole length of that edge.
pub fn dock_hot_strip(w: u32, h: u32, s: f32, pos: iyakki_uikit::look::DockPos) -> R {
    use iyakki_uikit::look::DockPos;
    let t = 10.0 * dock_scale(s);
    match pos {
        DockPos::Left => R::new(0.0, 0.0, t, h as f32),
        DockPos::Right => R::new(w as f32 - t, 0.0, t, h as f32),
        _ => R::new(0.0, h as f32 - t, w as f32, t),
    }
}

/// Whether an autohiding dock is on screen: the pointer is in the hot strip or over the plate (a little bigger, so it does not flicker at
/// the edge), or the dock has the keyboard, or an icon is being pressed. A dock that does not autohide is always shown.
pub fn dock_revealed(autohide: bool, strip: R, plate: R, mouse: (i32, i32), focused: bool, pressed: bool) -> bool {
    if !autohide || focused || pressed {
        return true;
    }
    let pad = 6.0;
    let near = R::new(plate.x - pad, plate.y - pad, plate.w + 2.0 * pad, plate.h + 2.0 * pad);
    strip.contains(mouse.0, mouse.1) || near.contains(mouse.0, mouse.1)
}

/// Magnify (the dock Look key): each icon grows with the pointer's nearness along the dock, up to `MAGNIFY_MAX` times its size, growing
/// away from the screen edge; the icons are laid out again side by side so they never overlap, centred where the row was. With the
/// pointer away from the dock (or `on` false) the rects come back unchanged. Pure; the caller widens the plate with [`dock_plate_around`].
pub const MAGNIFY_MAX: f32 = 1.45;
pub fn magnify_rects(items: &[R], plate: R, mouse: (i32, i32), on: bool, pos: iyakki_uikit::look::DockPos, screen_long: f32) -> Vec<R> {
    use iyakki_uikit::look::DockPos;
    let vertical = matches!(pos, DockPos::Left | DockPos::Right);
    if !on || items.is_empty() || !plate.contains(mouse.0, mouse.1) {
        return items.to_vec();
    }
    let along = |r: &R| if vertical { (r.y, r.h) } else { (r.x, r.w) };
    let m = if vertical { mouse.1 as f32 } else { mouse.0 as f32 };
    let (first, _) = along(&items[0]);
    let (last_at, last_len) = along(&items[items.len() - 1]);
    let (centre, span) = ((first + last_at + last_len) / 2.0, last_at + last_len - first);
    let reach = items[0].w.max(items[0].h) * 2.2;
    let sizes: Vec<f32> = items
        .iter()
        .map(|r| {
            let (a, l) = along(r);
            let d = ((a + l / 2.0) - m).abs();
            l * (1.0 + (MAGNIFY_MAX - 1.0) * (1.0 - (d / reach).min(1.0)).powi(2))
        })
        .collect();
    let gap = if items.len() > 1 { (along(&items[1]).0 - (along(&items[0]).0 + along(&items[0]).1)).max(0.0) } else { 0.0 };
    let total: f32 = sizes.iter().sum::<f32>() + gap * (items.len() as f32 - 1.0);
    // never longer than the screen (less the plate's own padding either side): a very long row stays as it was
    if total > screen_long - 2.0 * 24.0 || span <= 0.0 {
        return items.to_vec();
    }
    let mut at = centre - total / 2.0;
    items
        .iter()
        .zip(&sizes)
        .map(|(r, &sz)| {
            let out = if vertical {
                // grow away from the edge: a left dock grows to the right, a right dock to the left
                let x = if pos == DockPos::Left { r.x } else { r.x + r.w - sz };
                R::new(x, at, sz, sz)
            } else {
                R::new(at, r.y + r.h - sz, sz, sz)
            };
            at += sz + gap;
            out
        })
        .collect()
}

/// The plate grown along its long side to hold `items` with the plate's own padding either side (what magnify needs; at rest it is `plate`).
pub fn dock_plate_around(plate: R, items: &[R], pos: iyakki_uikit::look::DockPos, pad: f32) -> R {
    use iyakki_uikit::look::DockPos;
    let (Some(a), Some(b)) = (items.first(), items.last()) else { return plate };
    if matches!(pos, DockPos::Left | DockPos::Right) {
        let (lo, hi) = ((a.y - pad).min(plate.y), (b.y + b.h + pad).max(plate.y + plate.h));
        R::new(plate.x, lo, plate.w, hi - lo)
    } else {
        let (lo, hi) = ((a.x - pad).min(plate.x), (b.x + b.w + pad).max(plate.x + plate.w));
        R::new(lo, plate.y, hi - lo, plate.h)
    }
}

/// The three points of an edge arrow in the box `r`: top end, tip, bottom end. The tip points the way the page goes: left on the
/// left-edge arrow (`back`), right on the right-edge one.
pub fn chevron_points(r: R, back: bool) -> [(f32, f32); 3] {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let k = r.w.min(r.h) * 0.62 * 0.2;
    let dir = if back { 1.0 } else { -1.0 };
    [(cx + dir * k * 0.5, cy - k * 1.3), (cx - dir * k * 0.6, cy), (cx + dir * k * 0.5, cy + k * 1.3)]
}

/// The page that a key or a step from page `from` (0 = the machine, then the groups) leads to, `None` when there is none (no wrapping).
pub fn page_step(from: usize, back: bool, pages: usize) -> Option<usize> {
    if back {
        from.checked_sub(1)
    } else {
        Some(from + 1).filter(|p| *p < pages)
    }
}

/// The page a number key leads to: `1` is the machine, `2` the first group, up to `6`; `None` for a key with no such page.
pub fn page_for_digit(c: char, pages: usize) -> Option<usize> {
    let d = c.to_digit(10)? as usize;
    (1..=6).contains(&d).then(|| d - 1).filter(|p| *p < pages)
}

/// The page controls: previous arrow, one dot per page, next arrow, centred above the hint line.
pub struct PageNav {
    pub prev: R,
    pub next: R,
    pub dots: Vec<R>,
}

pub fn page_nav(w: u32, h: u32, s: f32, pages: usize) -> PageNav {
    let short = bottom(h, s) == BOTTOM_SHORT;
    let (d, gap) = (if short { 20.0 } else { 26.0 } * s, 8.0 * s);
    let total = d * (pages as f32 + 2.0) + gap * (pages as f32 + 1.0);
    // many pages on a narrow screen: the buttons and dots shrink together to stay inside the window
    let k = ((w as f32 - 16.0 * s) / total).min(1.0);
    let (d, gap, total) = (d * k, gap * k, total * k);
    let x0 = (w as f32 - total) / 2.0;
    let y = h as f32 - if short { 57.0 } else { 78.0 } * s - params().inset_b * dock_scale(s);
    let at = |i: usize| R::new(x0 + i as f32 * (d + gap), y, d, d);
    PageNav { prev: at(0), dots: (0..pages).map(|p| at(p + 1)).collect(), next: at(pages + 1) }
}

#[cfg(test)]
mod tests {
    use iyakki_uikit::look::DockPos;

    #[test]
    fn the_dock_reveal_and_magnify_geometry() {
        let (w, h, s) = (1280u32, 720u32, 1.0f32);
        let strip = dock_hot_strip(w, h, s, DockPos::Bottom);
        assert!(strip.y + strip.h >= h as f32 - 0.01 && strip.w >= w as f32);
        let l = dock_hot_strip(w, h, s, DockPos::Left);
        let r = dock_hot_strip(w, h, s, DockPos::Right);
        assert!(l.x == 0.0 && r.x + r.w >= w as f32 - 0.01 && l.h == h as f32);
        let (plate, items) = dock_rects(w, h, s, 5, 56, DockPos::Bottom);
        assert!(dock_revealed(false, strip, plate, (0, 0), false, false), "no autohide: always out");
        assert!(!dock_revealed(true, strip, plate, (5, 5), false, false));
        assert!(dock_revealed(true, strip, plate, (600, 719), false, false));
        assert!(dock_revealed(true, strip, plate, ((plate.x + 3.0) as i32, (plate.y + 3.0) as i32), false, false), "over the plate");
        assert!(dock_revealed(true, strip, plate, (5, 5), true, false) && dock_revealed(true, strip, plate, (5, 5), false, true));
        // magnify: away or off changes nothing; over the middle icon it is the biggest and the row stays inside the plate, centred
        assert_eq!(magnify_rects(&items, plate, (0, 0), true, DockPos::Bottom, w as f32), items);
        let m = (items[2].x + items[2].w / 2.0) as i32;
        let y = (items[2].y + 5.0) as i32;
        assert_eq!(magnify_rects(&items, plate, (m, y), false, DockPos::Bottom, w as f32), items);
        let big = magnify_rects(&items, plate, (m, y), true, DockPos::Bottom, w as f32);
        assert!(big[2].w > items[2].w && big[2].w <= items[2].w * MAGNIFY_MAX + 0.01);
        assert!(big[2].w >= big[1].w && big[1].w >= big[0].w);
        let (first, last) = (big[0], big[4]);
        let grown = dock_plate_around(plate, &big, DockPos::Bottom, 12.0);
        assert!(grown.x <= first.x - 12.0 + 0.01 && grown.x + grown.w >= last.x + last.w + 12.0 - 0.01 && grown.w > plate.w, "the plate grows with the row");
        assert_eq!(dock_plate_around(plate, &items, DockPos::Bottom, 12.0), plate, "at rest the plate is as it was");
        let (a, b) = ((items[0].x + items[4].x + items[4].w) / 2.0, (first.x + last.x + last.w) / 2.0);
        assert!((a - b).abs() < 0.5, "centred where the row was");
        // a side dock grows away from its edge
        let (sp, si) = dock_rects(w, h, s, 4, 56, DockPos::Left);
        let sm = magnify_rects(&si, sp, ((si[1].x + 4.0) as i32, (si[1].y + si[1].h / 2.0) as i32), true, DockPos::Left, h as f32);
        assert!(sm[1].x == si[1].x && sm[1].w > si[1].w);
        let (rp, ri) = dock_rects(w, h, s, 4, 56, DockPos::Right);
        let rm = magnify_rects(&ri, rp, ((ri[1].x + 4.0) as i32, (ri[1].y + ri[1].h / 2.0) as i32), true, DockPos::Right, h as f32);
        assert!((rm[1].x + rm[1].w - (ri[1].x + ri[1].w)).abs() < 0.01 && rm[1].w > ri[1].w);
    }

    use super::*;

    #[test]
    fn every_tile_app_is_in_exactly_one_section_and_the_inbox_and_settings_in_none() {
        reset_params();
        for a in App::ALL {
            let n = Group::CHIPS.iter().filter(|g| g.holds(a)).count();
            assert_eq!(n, if matches!(a, App::Inbox | App::Settings) { 0 } else { 1 }, "{a:?}");
        }
        for g in Group::CHIPS {
            assert_eq!(Group::from_id(g.id()), Some(g));
            assert!(iyakki_shared::layout::is_builtin(g.id()) && iyakki_shared::layout::BUILTIN.iter().any(|(i, l)| *i == g.id() && *l == g.label()), "{g:?}: the layout and the launcher agree on the built-in groups");
        }
        assert_eq!(Group::CHIPS.len(), iyakki_shared::layout::BUILTIN.len());
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
        reset_params();
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
        reset_params();
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
        reset_params();
        assert_eq!(groups_of(&[App::Chat, App::Rag]), vec![Group::Home]);
        assert_eq!(groups_of(&[App::Radio, App::Edit]), vec![Group::Work, Group::WebMedia]);
        assert_eq!(groups_of(&[]), Vec::<Group>::new());
        assert_eq!(groups_of(&App::ALL).len(), 5, "Apps has no built-in tile: it shows with the first installed one");
        assert_eq!(groups_with(&App::ALL, &[Group::Apps]).len(), 6);
    }

    #[test]
    fn paging_follows_the_screen_and_the_text_size() {
        reset_params();
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
    fn the_island_the_dots_and_the_arrows_fit_every_screen_without_touching_the_chips_or_each_other() {
        reset_params();
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (2560, 1440), (3840, 2160), (1024, 600), (800, 480), (1024, 768)] {
            let s = crate::shell::scale_for(w, h);
            let cap = per_page(w, h, s);
            for n in [1usize, 3, 8, 11, cap, cap * 2 + 1] {
                let isl = island_rect(w, h, s, n);
                assert!(isl.x >= 0.0 && isl.x + isl.w <= w as f32 && isl.y >= 120.0 * s && isl.y + isl.h <= h as f32 - 100.0 * s, "{w}x{h} n={n} {isl:?}");
                for t in page_rects(w, h, s, n, 0) {
                    assert!(t.x >= isl.x && t.x + t.w <= isl.x + isl.w + 0.5 && t.y >= isl.y && t.y + t.h <= isl.y + isl.h + 0.5, "{w}x{h}: a tile sticks out of the island");
                }
                let dots = page_dots(w, h, s, 6);
                assert!(dots.iter().all(|d| d.y >= isl.y + isl.h && d.x >= 0.0 && d.x + d.w <= w as f32), "{w}x{h}: dots below the island");
                let nav = page_nav(w, h, s, 3);
                assert!(dots.iter().all(|d| d.y + d.h <= nav.dots[0].y + 0.5), "{w}x{h}: dots above the screen-paging row");
                for pair in dots.windows(2) {
                    assert!(pair[0].x + pair[0].w <= pair[1].x + 0.01);
                }
            }
            let (a, b) = edge_chevrons(w, h, s);
            assert!(a.x >= 0.0 && b.x + b.w <= w as f32 && a.x + a.w < b.x);
            assert!(a.w >= 24.0 * s.min(1.0) && a.h >= 40.0 * s.min(1.0), "tappable");
        }
    }

    #[test]
    fn the_dock_sits_clear_of_the_tiles_the_dots_and_the_hint_line_at_every_size_and_side() {
        reset_params();
        use iyakki_uikit::look::DockPos;
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (2560, 1440), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            for pos in [DockPos::Bottom, DockPos::Left, DockPos::Right] {
                for (n, size) in [(1usize, 56u16), (6, 56), (10, 56), (6, 24), (6, 128)] {
                    let look = iyakki_uikit::look::Look::default();
                    let mut look = look;
                    look.dock.position = pos;
                    look.dock.size = size;
                    set_params(crate::shell::look_impl::params_for(&look, n));
                    let (plate, items) = dock_rects(w, h, s, n, size, pos);
                    assert_eq!(items.len(), n);
                    assert!(plate.y >= 0.0 && plate.y + plate.h <= h as f32 - 30.0 * s || pos != DockPos::Bottom, "{w}x{h} {pos:?}: above the hint line");
                    for r in &items {
                        assert!(r.x >= plate.x - 0.1 && r.x + r.w <= plate.x + plate.w + 0.1 + (if pos == DockPos::Bottom { 0.0 } else { 0.0 }) || plate.w < n as f32 * r.w, "{w}x{h} {pos:?}: an item inside the plate");
                    }
                    // the tiles, the island and the page dots never reach the dock
                    let nt = 20usize;
                    let isl = island_rect(w, h, s, nt);
                    let dots = page_dots(w, h, s, 6);
                    if size as f32 * s * n as f32 + 24.0 * s < w as f32 - 8.0 * s || pos == DockPos::Bottom && n < 7 {
                        let apart = |a: &R, b: &R| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
                        assert!(apart(&isl, &plate), "{w}x{h} {pos:?} n={n} size={size}: island {isl:?} meets dock {plate:?}");
                        assert!(dots.iter().all(|d| apart(d, &plate)), "{w}x{h} {pos:?}: dots meet the dock");
                    }
                }
            }
            reset_params();
        }
    }

    #[test]
    fn the_left_arrow_points_left_and_the_right_arrow_points_right() {
        reset_params();
        let (a, b) = edge_chevrons(1920, 1080, 1.0);
        let l = chevron_points(a, true);
        assert!(l[1].0 < l[0].0 && l[1].0 < l[2].0, "the tip of the left-edge arrow is at its left: {l:?}");
        let r = chevron_points(b, false);
        assert!(r[1].0 > r[0].0 && r[1].0 > r[2].0, "the tip of the right-edge arrow is at its right: {r:?}");
    }

    #[test]
    fn paging_steps_stop_at_the_ends_and_digits_one_to_six_pick_the_pages() {
        reset_params();
        assert_eq!(page_step(0, true, 6), None);
        assert_eq!(page_step(0, false, 6), Some(1));
        assert_eq!(page_step(5, false, 6), None);
        assert_eq!(page_step(5, true, 6), Some(4));
        assert_eq!(page_for_digit('1', 6), Some(0));
        assert_eq!(page_for_digit('6', 6), Some(5));
        assert_eq!(page_for_digit('6', 4), None, "a person with fewer groups has fewer pages");
        assert_eq!(page_for_digit('0', 6), None);
        assert_eq!(page_for_digit('7', 9), None);
        assert_eq!(page_for_digit('x', 6), None);
    }

    #[test]
    fn chips_and_dots_do_not_overlap_each_other_or_the_tiles() {
        reset_params();
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
        reset_params();
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
