//! The layout engine for panes: pure geometry, no drawing and no apps.
//!
//! A [`Layout`] is a tree: a leaf is a pane, a split cuts its rectangle in two with a ratio (the first part's share of the
//! space without the gap) and may be *linked* to other splits so they move together (the 2x2 grid keeps its columns aligned).
//! Panes are numbered in reading order (the first part before the second, depth first). See `docs/MULTI-DISPLAY.md`.

use crate::safeclamp::SafeClamp;
use crate::ui::R;


/// The chord for a key named by its DOM `code` ("ArrowLeft", "Digit3", "Tab", "KeyM") with Ctrl and Alt held.
pub fn chord_from_name(code: &str, shift: bool) -> Option<LayoutKey> {
    let side = match code {
        "ArrowLeft" => Some(Side::Left),
        "ArrowRight" => Some(Side::Right),
        "ArrowUp" => Some(Side::Up),
        "ArrowDown" => Some(Side::Down),
        _ => None,
    };
    if let Some(sd) = side {
        return Some(if shift { LayoutKey::Nudge(sd) } else { LayoutKey::Snap(sd) });
    }
    match code {
        "Tab" => Some(LayoutKey::Cycle { back: shift }),
        "KeyM" => Some(LayoutKey::NextDisplay),
        "Digit0" => Some(LayoutKey::Picker),
        c => {
            let d = c.strip_prefix("Digit")?.parse::<u8>().ok()?;
            (1..=9).contains(&d).then_some(LayoutKey::Apply(d))
        }
    }
}

/// The chord for a Linux evdev key code with Ctrl and Alt held.
pub fn chord_from_evdev(code: u16, shift: bool) -> Option<LayoutKey> {
    let name = match code {
        2..=10 => return chord_from_name(&format!("Digit{}", code - 1), shift),
        11 => "Digit0",
        15 => "Tab",
        50 => "KeyM",
        105 => "ArrowLeft",
        106 => "ArrowRight",
        103 => "ArrowUp",
        108 => "ArrowDown",
        _ => return None,
    };
    chord_from_name(name, shift)
}

/// How a split cuts its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Two parts side by side (left, right).
    Side,
    /// Two parts one above the other (top, bottom).
    Stack,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf,
    /// `link` 0 means unlinked.
    Split { dir: Dir, ratio: f32, link: u8, a: Box<Node>, b: Box<Node> },
}

fn split(dir: Dir, ratio: f32, a: Node, b: Node) -> Node {
    Node::Split { dir, ratio, link: 0, a: Box::new(a), b: Box::new(b) }
}
fn linked(dir: Dir, ratio: f32, link: u8, a: Node, b: Node) -> Node {
    Node::Split { dir, ratio, link, a: Box::new(a), b: Box::new(b) }
}

/// One row of the template table.
pub struct TemplateDef {
    pub id: &'static str,
    pub name: &'static str,
    /// The digit of Ctrl+Alt+N, when it has one.
    pub key: Option<u8>,
    pub build: fn() -> Node,
}

fn t_single() -> Node {
    Node::Leaf
}
fn t_cols5050() -> Node {
    split(Dir::Side, 0.5, Node::Leaf, Node::Leaf)
}
fn t_rows5050() -> Node {
    split(Dir::Stack, 0.5, Node::Leaf, Node::Leaf)
}
fn t_cols3070() -> Node {
    split(Dir::Side, 0.3, Node::Leaf, Node::Leaf)
}
fn t_cols7030() -> Node {
    split(Dir::Side, 0.7, Node::Leaf, Node::Leaf)
}
fn t_cols2575() -> Node {
    split(Dir::Side, 0.25, Node::Leaf, Node::Leaf)
}
fn t_cols7525() -> Node {
    split(Dir::Side, 0.75, Node::Leaf, Node::Leaf)
}
fn t_cols6040() -> Node {
    split(Dir::Side, 0.6, Node::Leaf, Node::Leaf)
}
fn t_cols4060() -> Node {
    split(Dir::Side, 0.4, Node::Leaf, Node::Leaf)
}
fn t_rows3070() -> Node {
    split(Dir::Stack, 0.3, Node::Leaf, Node::Leaf)
}
fn t_rows7030() -> Node {
    split(Dir::Stack, 0.7, Node::Leaf, Node::Leaf)
}
fn t_thirds() -> Node {
    split(Dir::Side, 1.0 / 3.0, Node::Leaf, split(Dir::Side, 0.5, Node::Leaf, Node::Leaf))
}
fn t_thirds_rows() -> Node {
    split(Dir::Stack, 1.0 / 3.0, Node::Leaf, split(Dir::Stack, 0.5, Node::Leaf, Node::Leaf))
}
fn t_grid() -> Node {
    linked(Dir::Stack, 0.5, 0, linked(Dir::Side, 0.5, 1, Node::Leaf, Node::Leaf), linked(Dir::Side, 0.5, 1, Node::Leaf, Node::Leaf))
}
fn t_main_stack() -> Node {
    split(Dir::Side, 0.7, Node::Leaf, split(Dir::Stack, 0.5, Node::Leaf, Node::Leaf))
}

/// Every template, the numbered ones first (Ctrl+Alt+1 to 9).
pub const TEMPLATES: &[TemplateDef] = &[
    TemplateDef { id: "single", name: "Single", key: Some(1), build: t_single },
    TemplateDef { id: "cols-50-50", name: "50:50 side by side", key: Some(2), build: t_cols5050 },
    TemplateDef { id: "rows-50-50", name: "50:50 stacked", key: Some(3), build: t_rows5050 },
    TemplateDef { id: "cols-30-70", name: "30:70", key: Some(4), build: t_cols3070 },
    TemplateDef { id: "cols-70-30", name: "70:30", key: Some(5), build: t_cols7030 },
    TemplateDef { id: "thirds", name: "33:33:33", key: Some(6), build: t_thirds },
    TemplateDef { id: "grid", name: "2x2 grid", key: Some(7), build: t_grid },
    TemplateDef { id: "main-stack", name: "70:30, right side stacked", key: Some(8), build: t_main_stack },
    TemplateDef { id: "cols-60-40", name: "60:40", key: Some(9), build: t_cols6040 },
    TemplateDef { id: "cols-25-75", name: "25:75", key: None, build: t_cols2575 },
    TemplateDef { id: "cols-75-25", name: "75:25", key: None, build: t_cols7525 },
    TemplateDef { id: "cols-40-60", name: "40:60", key: None, build: t_cols4060 },
    TemplateDef { id: "rows-30-70", name: "30:70 stacked", key: None, build: t_rows3070 },
    TemplateDef { id: "rows-70-30", name: "70:30 stacked", key: None, build: t_rows7030 },
    TemplateDef { id: "thirds-rows", name: "33:33:33 stacked", key: None, build: t_thirds_rows },
];

/// The ratios dividers snap to.
pub const SNAPS: [f32; 9] = [0.25, 0.30, 1.0 / 3.0, 0.40, 0.50, 0.60, 2.0 / 3.0, 0.70, 0.75];
/// How close (as a share of the split's length) a divider has to be to a snap ratio to jump to it.
pub const SNAP_NEAR: f32 = 0.02;

pub fn template(id: &str) -> Option<&'static TemplateDef> {
    TEMPLATES.iter().find(|t| t.id == id)
}

pub fn template_for_key(n: u8) -> Option<&'static TemplateDef> {
    TEMPLATES.iter().find(|t| t.key == Some(n))
}

/// A divider of the layout: which split it belongs to (preorder number), where the pointer can grab it and how it cuts.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub split: usize,
    pub rect: R,
    pub dir: Dir,
}

pub use iyakki_uikit::keys::{LayoutKey, Side};

/// What Ctrl+Alt+arrow does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snap {
    /// Swap the focused pane's content with this pane's; focus follows.
    Swap(usize),
    /// Single pane: make this template and put the focused content in pane `focus_after` of it.
    Make { template: &'static str, focus_after: usize },
    /// Nothing on that side.
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub id: String,
    pub root: Node,
}

impl Layout {
    pub fn from_template(id: &str) -> Layout {
        let t = template(id).unwrap_or(&TEMPLATES[0]);
        Layout { id: t.id.to_string(), root: (t.build)() }
    }

    pub fn single() -> Layout {
        Layout::from_template("single")
    }

    pub fn leaf_count(&self) -> usize {
        count(&self.root)
    }

    /// The rectangle of every pane in reading order, with `gap` between neighbours.
    pub fn rects(&self, area: R, gap: f32) -> Vec<R> {
        let mut out = Vec::new();
        walk(&self.root, area, gap, &mut out, &mut Vec::new(), &mut 0);
        out
    }

    /// The dividers, with a grab zone at least `grab` wide.
    pub fn dividers(&self, area: R, gap: f32, grab: f32) -> Vec<Divider> {
        let mut out = Vec::new();
        let mut leaves = Vec::new();
        walk(&self.root, area, gap, &mut leaves, &mut out, &mut 0);
        out.into_iter()
            .map(|mut d| {
                let extra = (grab - gap).max(0.0) / 2.0;
                match d.dir {
                    Dir::Side => {
                        d.rect.x -= extra;
                        d.rect.w += 2.0 * extra;
                    }
                    Dir::Stack => {
                        d.rect.y -= extra;
                        d.rect.h += 2.0 * extra;
                    }
                }
                d
            })
            .collect()
    }

    /// The pane at a point.
    pub fn pane_at(&self, area: R, gap: f32, x: i32, y: i32) -> Option<usize> {
        self.rects(area, gap).iter().position(|r| r.contains(x, y))
    }

    /// The divider at a point (the grab zone), if any.
    pub fn divider_at(&self, area: R, gap: f32, grab: f32, x: i32, y: i32) -> Option<Divider> {
        self.dividers(area, gap, grab).into_iter().find(|d| d.rect.contains(x, y))
    }

    /// Panes in focus order: reading order.
    pub fn focus_order(&self) -> Vec<usize> {
        (0..self.leaf_count()).collect()
    }

    pub fn next_focus(&self, from: usize, back: bool) -> usize {
        let n = self.leaf_count().max(1);
        if back {
            (from + n - 1) % n
        } else {
            (from + 1) % n
        }
    }

    /// The pane next to `from` on `side` (the nearest one that overlaps it along the other axis).
    pub fn neighbour(&self, area: R, gap: f32, from: usize, side: Side) -> Option<usize> {
        let rs = self.rects(area, gap);
        let f = *rs.get(from)?;
        let eps = 0.5 + gap;
        let mut best: Option<(f32, f32, usize)> = None;
        for (i, r) in rs.iter().enumerate() {
            if i == from {
                continue;
            }
            let (dist, overlap) = match side {
                Side::Left => (f.x - (r.x + r.w), overlap(f.y, f.h, r.y, r.h)),
                Side::Right => (r.x - (f.x + f.w), overlap(f.y, f.h, r.y, r.h)),
                Side::Up => (f.y - (r.y + r.h), overlap(f.x, f.w, r.x, r.w)),
                Side::Down => (r.y - (f.y + f.h), overlap(f.x, f.w, r.x, r.w)),
            };
            if dist >= -eps && dist <= gap + eps + 1.0 && overlap > 1.0 {
                let cand = (dist.max(0.0), -overlap, i);
                if best.map_or(true, |b| (cand.0, cand.1) < (b.0, b.1)) {
                    best = Some(cand);
                }
            }
        }
        best.map(|b| b.2)
    }

    /// What Ctrl+Alt+`side` does for the pane `focus`.
    pub fn snap(&self, area: R, gap: f32, focus: usize, side: Side) -> Snap {
        if self.leaf_count() == 1 {
            return match side {
                Side::Left => Snap::Make { template: "cols-50-50", focus_after: 0 },
                Side::Right => Snap::Make { template: "cols-50-50", focus_after: 1 },
                Side::Up => Snap::Make { template: "rows-50-50", focus_after: 0 },
                Side::Down => Snap::Make { template: "rows-50-50", focus_after: 1 },
            };
        }
        match self.neighbour(area, gap, focus, side) {
            Some(n) => Snap::Swap(n),
            None => Snap::Nothing,
        }
    }

    /// Whether every pane is at least `min` (w, h) big in this area.
    pub fn fits(&self, area: R, gap: f32, min: (f32, f32)) -> bool {
        self.rects(area, gap).iter().all(|r| r.w + 0.5 >= min.0 && r.h + 0.5 >= min.1)
    }

    /// The template to use after one pane is closed: the first one with one pane fewer.
    pub fn after_close(&self) -> &'static str {
        match self.leaf_count().saturating_sub(1) {
            0 | 1 => "single",
            2 => "cols-50-50",
            _ => "thirds",
        }
    }

    /// The divider (split number) that borders pane `leaf` on `side`.
    pub fn divider_beside(&self, leaf: usize, side: Side) -> Option<usize> {
        let want = if matches!(side, Side::Left | Side::Right) { Dir::Side } else { Dir::Stack };
        let toward_second = matches!(side, Side::Right | Side::Down);
        let mut found = None;
        find_beside(&self.root, leaf, &mut 0, &mut 0, want, toward_second, &mut found);
        found
    }

    pub fn ratio(&self, split_no: usize) -> Option<f32> {
        let mut out = None;
        visit_splits(&self.root, &mut 0, &mut |i, n| {
            if i == split_no {
                if let Node::Split { ratio, .. } = n {
                    out = Some(*ratio);
                }
            }
        });
        out
    }

    /// Move a divider to `pos` (a coordinate on the split's axis, in the display's space), clamped so that no pane under it
    /// gets smaller than `mins[pane]` (w, h), and snapped to the table ratios. Splits linked to it follow. Returns the ratio set.
    pub fn drag(&mut self, area: R, gap: f32, split_no: usize, pos: f32, mins: &[(f32, f32)]) -> Option<f32> {
        let sp = self.split_rect(area, gap, split_no)?;
        let (rect, dir, _) = sp;
        let (origin, len) = match dir {
            Dir::Side => (rect.x, rect.w),
            Dir::Stack => (rect.y, rect.h),
        };
        let span = (len - gap).max(1.0);
        let mut ratio = ((pos - origin - gap / 2.0) / span).clamp(0.02, 0.98);
        // allowed range from the minimum sizes of everything on either side
        let (lo, hi) = self.bounds_for(area, gap, split_no, mins);
        for s in SNAPS {
            if (ratio - s).abs() <= SNAP_NEAR {
                ratio = s;
                break;
            }
        }
        if lo <= hi {
            ratio = ratio.clamp_safe(lo, hi);
        } else {
            ratio = (lo + hi) / 2.0;
        }
        self.set_ratio(split_no, ratio);
        Some(ratio)
    }

    /// Set a split's ratio (and the splits linked to it).
    pub fn set_ratio(&mut self, split_no: usize, ratio: f32) {
        let mut link = 0u8;
        visit_splits(&self.root, &mut 0, &mut |i, n| {
            if i == split_no {
                if let Node::Split { link: l, .. } = n {
                    link = *l;
                }
            }
        });
        visit_splits_mut(&mut self.root, &mut 0, &mut |i, n| {
            if let Node::Split { ratio: r, link: l, .. } = n {
                if i == split_no || (link != 0 && *l == link) {
                    *r = ratio;
                }
            }
        });
    }

    /// Move the divider beside `leaf` on `side` by `step` of its length in the direction of `side`; snaps and respects minimums.
    pub fn nudge(&mut self, area: R, gap: f32, leaf: usize, side: Side, step: f32, mins: &[(f32, f32)]) -> bool {
        let Some(sp) = self.divider_beside(leaf, side) else { return false };
        let Some(old) = self.ratio(sp) else { return false };
        let Some((rect, dir, _)) = self.split_rect(area, gap, sp) else { return false };
        let (origin, len) = match dir {
            Dir::Side => (rect.x, rect.w),
            Dir::Stack => (rect.y, rect.h),
        };
        let delta = if matches!(side, Side::Left | Side::Up) { -step } else { step };
        let pos = origin + gap / 2.0 + (old + delta) * (len - gap).max(1.0);
        let before = old;
        let after = self.drag(area, gap, sp, pos, mins).unwrap_or(old);
        (after - before).abs() > 1e-4
    }

    /// (rectangle, direction, number of the first leaf under it) of split number `n`.
    fn split_rect(&self, area: R, gap: f32, n: usize) -> Option<(R, Dir, usize)> {
        let mut out = None;
        find_split_rect(&self.root, area, gap, &mut 0, &mut 0, n, &mut out);
        out
    }

    /// The smallest and the largest ratio of split `n` (and its linked splits) that keep every pane at its minimum.
    fn bounds_for(&self, area: R, gap: f32, n: usize, mins: &[(f32, f32)]) -> (f32, f32) {
        let mut link = 0u8;
        visit_splits(&self.root, &mut 0, &mut |i, nd| {
            if i == n {
                if let Node::Split { link: l, .. } = nd {
                    link = *l;
                }
            }
        });
        let mut targets = vec![n];
        if link != 0 {
            visit_splits(&self.root, &mut 0, &mut |i, nd| {
                if let Node::Split { link: l, .. } = nd {
                    if *l == link && i != n {
                        targets.push(i);
                    }
                }
            });
        }
        let (mut lo, mut hi) = (0.02f32, 0.98f32);
        for t in targets {
            let Some((rect, dir, _)) = self.split_rect(area, gap, t) else { continue };
            let mut parts = None;
            find_parts(&self.root, &mut 0, &mut 0, t, &mut parts);
            if let Some((a_first, a, b_first, b)) = parts {
                let axis = |m: (f32, f32)| if dir == Dir::Side { m.0 } else { m.1 };
                let need_a = min_extent(a, a_first, dir, gap, mins, &axis);
                let need_b = min_extent(b, b_first, dir, gap, mins, &axis);
                let span = (if dir == Dir::Side { rect.w } else { rect.h } - gap).max(1.0);
                lo = lo.max(need_a / span);
                hi = hi.min(1.0 - need_b / span);
            }
        }
        (lo, hi)
    }
}

fn overlap(a: f32, al: f32, b: f32, bl: f32) -> f32 {
    (a + al).min(b + bl) - a.max(b)
}

fn count(n: &Node) -> usize {
    match n {
        Node::Leaf => 1,
        Node::Split { a, b, .. } => count(a) + count(b),
    }
}

/// Cut `r` for a split: (first part, second part, divider strip).
fn cut(r: R, dir: Dir, ratio: f32, gap: f32) -> (R, R, R) {
    match dir {
        Dir::Side => {
            let span = (r.w - gap).max(0.0);
            let w1 = (span * ratio).round();
            let w2 = (span - w1).max(0.0);
            (R::new(r.x, r.y, w1, r.h), R::new(r.x + w1 + gap, r.y, w2, r.h), R::new(r.x + w1, r.y, gap, r.h))
        }
        Dir::Stack => {
            let span = (r.h - gap).max(0.0);
            let h1 = (span * ratio).round();
            let h2 = (span - h1).max(0.0);
            (R::new(r.x, r.y, r.w, h1), R::new(r.x, r.y + h1 + gap, r.w, h2), R::new(r.x, r.y + h1, r.w, gap))
        }
    }
}

fn walk(n: &Node, r: R, gap: f32, leaves: &mut Vec<R>, divs: &mut Vec<Divider>, split_no: &mut usize) {
    match n {
        Node::Leaf => leaves.push(r),
        Node::Split { dir, ratio, a, b, .. } => {
            let me = *split_no;
            *split_no += 1;
            let (ra, rb, d) = cut(r, *dir, *ratio, gap);
            divs.push(Divider { split: me, rect: d, dir: *dir });
            walk(a, ra, gap, leaves, divs, split_no);
            walk(b, rb, gap, leaves, divs, split_no);
        }
    }
}

fn visit_splits(n: &Node, no: &mut usize, f: &mut impl FnMut(usize, &Node)) {
    if let Node::Split { a, b, .. } = n {
        let me = *no;
        *no += 1;
        f(me, n);
        visit_splits(a, no, f);
        visit_splits(b, no, f);
    }
}

fn visit_splits_mut(n: &mut Node, no: &mut usize, f: &mut impl FnMut(usize, &mut Node)) {
    if let Node::Split { .. } = n {
        let me = *no;
        *no += 1;
        f(me, n);
        if let Node::Split { a, b, .. } = n {
            visit_splits_mut(a, no, f);
            visit_splits_mut(b, no, f);
        }
    }
}

fn find_split_rect(n: &Node, r: R, gap: f32, split_no: &mut usize, leaf_no: &mut usize, want: usize, out: &mut Option<(R, Dir, usize)>) {
    match n {
        Node::Leaf => *leaf_no += 1,
        Node::Split { dir, ratio, a, b, .. } => {
            let me = *split_no;
            *split_no += 1;
            if me == want {
                *out = Some((r, *dir, *leaf_no));
            }
            let (ra, rb, _) = cut(r, *dir, *ratio, gap);
            find_split_rect(a, ra, gap, split_no, leaf_no, want, out);
            find_split_rect(b, rb, gap, split_no, leaf_no, want, out);
        }
    }
}

/// (first leaf of part a, part a, first leaf of part b, part b) of split `want`.
fn find_parts<'a>(n: &'a Node, split_no: &mut usize, leaf_no: &mut usize, want: usize, out: &mut Option<(usize, &'a Node, usize, &'a Node)>) {
    match n {
        Node::Leaf => *leaf_no += 1,
        Node::Split { a, b, .. } => {
            let me = *split_no;
            *split_no += 1;
            if me == want {
                *out = Some((*leaf_no, a, *leaf_no + count(a), b));
            }
            find_parts(a, split_no, leaf_no, want, out);
            find_parts(b, split_no, leaf_no, want, out);
        }
    }
}

/// The least extent the node needs along `dir`'s axis. `axis` picks width or height out of a pane's minimum.
fn min_extent(n: &Node, first: usize, dir: Dir, gap: f32, mins: &[(f32, f32)], axis: &dyn Fn((f32, f32)) -> f32) -> f32 {
    match n {
        Node::Leaf => mins.get(first).map_or(0.0, |m| axis(*m)),
        Node::Split { dir: d, a, b, .. } => {
            let ea = min_extent(a, first, dir, gap, mins, axis);
            let eb = min_extent(b, first + count(a), dir, gap, mins, axis);
            if *d == dir {
                ea + eb + gap
            } else {
                ea.max(eb)
            }
        }
    }
}

fn find_beside(n: &Node, leaf: usize, split_no: &mut usize, leaf_no: &mut usize, want: Dir, toward_second: bool, found: &mut Option<usize>) -> bool {
    match n {
        Node::Leaf => {
            let here = *leaf_no == leaf;
            *leaf_no += 1;
            here
        }
        Node::Split { dir, a, b, .. } => {
            let me = *split_no;
            *split_no += 1;
            let in_a = find_beside(a, leaf, split_no, leaf_no, want, toward_second, found);
            let in_b = !in_a && find_beside(b, leaf, split_no, leaf_no, want, toward_second, found);
            // the nearest ancestor (deepest) whose axis matches and where the leaf is on the side facing the divider
            if found.is_none() && *dir == want && ((in_a && toward_second) || (in_b && !toward_second)) {
                *found = Some(me);
            }
            in_a || in_b
        }
    }
}

#[cfg(test)]
#[path = "panes_tests.rs"]
mod tests;
