//! The cluster switcher: a compact drop-down that shows which cluster a screen works against and lets the person change it
//! (docs/K3S-DEV.md, "Clusters"). One component for every place that needs it: the K8S tile, the AI IDE's K8s panel and deploy, the
//! Setup items, the Clusters page. The host owns a [`ClusterSwitch`], builds the rows with [`rows_for`], draws the closed box with
//! [`ClusterSwitch::draw`] (nothing is drawn when only one cluster exists) and the open list with [`ClusterSwitch::draw_popup`] last,
//! and feeds its events to [`ClusterSwitch::handle`].
//!
//! The list shows a status dot (running, stopped, unreachable, stale), the kind (local, remote, tunnel, cloud-cli), a GPU badge when
//! the AI hardware profile knows one, and "Manage clusters..." at the end. Switching never moves or deletes anything.

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

use iyakki_agent::clusters::{ClusterEntry, Dot, Kind, Registry, Status};
use iyakki_sec::policy::Role;

use super::widgets;

const AMBER: [u8; 4] = rgb(255, 184, 77);

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchRow {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub dot: Dot,
    pub gpu: Option<String>,
    pub active: bool,
}

impl SwitchRow {
    pub fn kind_word(&self) -> &'static str {
        self.kind.word()
    }
    /// The words a screen reader or a hit label uses.
    pub fn describe(&self) -> String {
        format!("{} ({}, {})", self.name, self.kind.word(), self.dot.word())
    }
}

/// The rows a person with `role` sees. `workspace` picks the per-workspace override when there is one. Empty for guests.
pub fn rows_for(reg: &Registry, role: Role, workspace: &str, status: &dyn Fn(&ClusterEntry) -> Status) -> Vec<SwitchRow> {
    let eff = reg.effective(workspace).id;
    reg.visible_to(role)
        .into_iter()
        .map(|e| {
            let st = status(e);
            SwitchRow { id: e.id.clone(), name: e.name.clone(), kind: e.kind, dot: st.dot, gpu: st.gpu.clone(), active: e.id == eff }
        })
        .collect()
}

pub fn dot_color(d: Dot) -> [u8; 4] {
    match d {
        Dot::Running => OK,
        Dot::Stopped => MUTED,
        Dot::Unreachable => ERR,
        Dot::Stale => AMBER,
        Dot::Unknown => DIM,
    }
}

/// What a click or key in the switcher asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchOut {
    None,
    /// The person chose this cluster id.
    Switch(String),
    /// "Manage clusters..." (the host opens Settings -> Clusters).
    Manage,
}

pub struct ClusterSwitch {
    /// Event codes `base..base + 8` and `base + 10 + row` belong to this switcher (`Event::Tab`).
    pub base: usize,
    pub open: bool,
    sel: usize,
    anchor: R,
}

impl ClusterSwitch {
    pub fn new(base: usize) -> ClusterSwitch {
        ClusterSwitch { base, open: false, sel: 0, anchor: R::new(0.0, 0.0, 0.0, 0.0) }
    }

    fn code_toggle(&self) -> usize {
        self.base
    }
    fn code_close(&self) -> usize {
        self.base + 1
    }
    fn code_manage(&self) -> usize {
        self.base + 2
    }
    fn code_row(&self, i: usize) -> usize {
        self.base + 10 + i
    }

    /// Whether the switcher shows at all: one cluster is no choice.
    pub fn visible(rows: &[SwitchRow]) -> bool {
        rows.len() > 1
    }

    /// The text of the closed box.
    pub fn label(rows: &[SwitchRow]) -> String {
        rows.iter().find(|r| r.active).map(|r| r.name.clone()).unwrap_or_else(|| "Cluster".into())
    }

    /// The width the closed box wants (0 when hidden).
    pub fn width(g: &mut Gfx, s: f32, rows: &[SwitchRow]) -> f32 {
        if !Self::visible(rows) {
            return 0.0;
        }
        widgets::select_width(g, s, &format!("      {}", Self::label(rows)))
    }

    /// Draw the closed box at (`x`, `y`) at most `max_w` wide. `None` (and nothing drawn or clickable) when there is one cluster.
    pub fn draw(&mut self, g: &mut Gfx, x: f32, y: f32, max_w: f32, s: f32, rows: &[SwitchRow], accent: [u8; 4]) -> Option<R> {
        if !Self::visible(rows) || max_w < 40.0 * s {
            return None;
        }
        let want = Self::width(g, s, rows);
        let w = want.min(max_w).max(1.0);
        let a = rows.iter().find(|r| r.active);
        let r = widgets::select_box(g, x, y, w, s, &format!("      {}", Self::label(rows)), self.open, accent, Event::Tab(self.code_toggle()), &format!("cluster switcher: {}", a.map(|r| r.describe()).unwrap_or_default()));
        // the status dot and a one-letter kind mark sit in the padding at the left of the label
        if let Some(a) = a {
            g.circle(r.x + 13.0 * s, r.y + r.h / 2.0, (3.5 * s).max(2.0), dot_color(a.dot));
        }
        self.anchor = r;
        Some(r)
    }

    /// Draw the open list (call after everything else of the host, inside `bounds`): the later hit wins. Returns its rectangle.
    pub fn draw_popup(&mut self, g: &mut Gfx, bounds: R, s: f32, rows: &[SwitchRow], accent: [u8; 4]) -> Option<R> {
        if !self.open || !Self::visible(rows) {
            return None;
        }
        if self.sel >= rows.len() {
            self.sel = rows.iter().position(|r| r.active).unwrap_or(0);
        }
        let sz = (13.0 * s).max(10.0);
        let small = (11.0 * s).max(9.0);
        let rh = (sz * 2.3).max(30.0 * s);
        // widest row: dot, name, chips
        let mut want = 0.0f32;
        for r in rows {
            let mut w = 30.0 * s + g.text_width(sz, &r.name, Face::Bold) + g.text_width(small, r.kind_word(), Face::Bold) + 28.0 * s;
            if let Some(gpu) = &r.gpu {
                w += g.text_width(small, gpu, Face::Bold) + 28.0 * s;
            }
            want = want.max(w);
        }
        want = want.max(g.text_width(sz, "Manage clusters...", Face::Bold) + 40.0 * s);
        let w = want.clamp_safe(self.anchor.w, bounds.w - 8.0 * s);
        let total = (rows.len() + 1) as f32 * rh + 10.0 * s;
        let h = total.min((bounds.h - 8.0 * s).max(rh + 10.0 * s));
        let x = self.anchor.x.clamp_safe(bounds.x + 4.0 * s, bounds.x + bounds.w - w - 4.0 * s);
        let below = self.anchor.y + self.anchor.h + 4.0 * s;
        let y = if below + h <= bounds.y + bounds.h { below } else { (self.anchor.y - h - 4.0 * s).max(bounds.y + 4.0 * s) };
        g.hit(bounds, Event::Tab(self.code_close()), "close the cluster list");
        let pop = R::new(x, y, w, h);
        g.rrect(pop.x, pop.y, pop.w, pop.h, 8.0 * s, rgb(16, 20, 36));
        g.rrect_outline(pop.x, pop.y, pop.w, pop.h, 8.0 * s, 1.5 * s, accent);
        // (the small epsilon: three rows in exactly their own height must not come out as two through float rounding)
        let fit = ((((h - 10.0 * s) / rh) + 0.01).floor() as usize).max(1);
        // keep the highlighted row in view; "Manage clusters..." is the last row
        let n = rows.len() + 1;
        let first = self.sel.saturating_sub(fit - 1).min(n.saturating_sub(fit));
        for i in first..n.min(first + fit) {
            let rr = R::new(pop.x + 5.0 * s, pop.y + 5.0 * s + (i - first) as f32 * rh, pop.w - 10.0 * s, rh - 2.0 * s);
            let hot = g.hovered(rr) || i == self.sel;
            if hot {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 5.0 * s, rgb(34, 44, 78));
            }
            let ty = rr.y + (rr.h - g.line_height(sz, Face::Bold)) / 2.0;
            if let Some(r) = rows.get(i) {
                g.circle(rr.x + 12.0 * s, rr.y + rr.h / 2.0, (4.0 * s).max(2.0), dot_color(r.dot));
                let mut right = rr.x + rr.w - 8.0 * s;
                let mut chips: Vec<(String, [u8; 4])> = vec![(r.kind_word().to_string(), MUTED)];
                if let Some(gpu) = &r.gpu {
                    chips.push((format!("GPU {gpu}"), accent));
                }
                for (label, col) in chips.iter().rev() {
                    let cw = g.text_width(small, label, Face::Bold) + small * 1.4;
                    if right - cw < rr.x + 60.0 * s {
                        break;
                    }
                    right -= cw;
                    crate::fx::chip(g, right, rr.y + (rr.h - small * 1.9) / 2.0, s, label, *col, false);
                    right -= 4.0 * s;
                }
                let t = g.fit(sz, &r.name, Face::Bold, (right - rr.x - 30.0 * s).max(20.0));
                g.text(rr.x + 24.0 * s, ty, sz, if r.active { accent } else { TEXT }, &t, if r.active { Face::Bold } else { Face::Sans });
                g.hit(rr, Event::Tab(self.code_row(i)), &r.describe());
            } else {
                let t = g.fit(sz, "Manage clusters...", Face::Bold, rr.w - 20.0 * s);
                g.text(rr.x + 10.0 * s, ty, sz, MUTED, &t, Face::Bold);
                g.hit(rr, Event::Tab(self.code_manage()), "Manage clusters...");
            }
        }
        Some(pop)
    }

    /// Feed an event. Keys are taken only while the list is open. `rows` are the rows last drawn.
    pub fn handle(&mut self, ev: &Event, rows: &[SwitchRow]) -> SwitchOut {
        let n = rows.len() + 1;
        match ev {
            Event::Tab(c) if *c == self.code_toggle() => {
                self.open = !self.open && Self::visible(rows);
                self.sel = rows.iter().position(|r| r.active).unwrap_or(0);
                SwitchOut::None
            }
            Event::Tab(c) if *c == self.code_close() => {
                self.open = false;
                SwitchOut::None
            }
            Event::Tab(c) if *c == self.code_manage() => {
                self.open = false;
                SwitchOut::Manage
            }
            Event::Tab(c) if *c >= self.code_row(0) && *c < self.code_row(rows.len()) => {
                self.open = false;
                SwitchOut::Switch(rows[*c - self.code_row(0)].id.clone())
            }
            Event::Key(Key::Esc) if self.open => {
                self.open = false;
                SwitchOut::None
            }
            Event::Key(Key::Up) if self.open => {
                self.sel = (self.sel + n - 1) % n;
                SwitchOut::None
            }
            Event::Key(Key::Down) if self.open => {
                self.sel = (self.sel + 1) % n;
                SwitchOut::None
            }
            Event::Key(Key::Enter) if self.open => {
                self.open = false;
                match rows.get(self.sel) {
                    Some(r) => SwitchOut::Switch(r.id.clone()),
                    None => SwitchOut::Manage,
                }
            }
            _ => SwitchOut::None,
        }
    }

    /// Whether the switcher wants every event (the list is open).
    pub fn owns_keys(&self) -> bool {
        self.open
    }
}

#[cfg(test)]
#[path = "cluster_switch_tests.rs"]
mod tests;
