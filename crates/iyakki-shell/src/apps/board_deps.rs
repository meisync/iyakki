//! The board's two newer layers, kept out of `board.rs`: **dependencies between rows** (a chain mark and "waits: F-APP-01 · unblocks 2" on the
//! cards, a Depends and an Agent column in the table, the Graph view with the critical path) and **agents** (the Agents strip, who is working on
//! which row, the lane filter, the drill-down panel with the timeline, files held, notes added and the controls).
//!
//! What a row waits for is `iyakki_agent::boarddeps`; who works on what is `iyakki_agent::assign` (`.iyakki/board-assignments.json`, never
//! FEATURES.md). This module only draws them and turns clicks and keys into [`BoardAct`]s; the IDE owns the task manager that acts on them.

use std::time::{Instant, SystemTime};

use iyakki_agent::assign::{self, AState, AgentRun, Choice, StepKind};
use iyakki_agent::boarddeps::{DepState, Graph};
use iyakki_agent::featureboard::Feature;
use iyakki_agent::tasks::AutoAssign;

use super::*;

/// `Event::Tab(TAB_BASE + BASE + n)` ranges of this module (the board's other chips use 0..2100).
/// Added to `TAB_BASE` for the events of this module: above the Mark done buttons (2000..12000) of the cards.
pub const BASE: usize = 10_000;
pub const AGENT_CHIP: usize = 3000;
pub const DRILL_BTN: usize = 3100;
pub const FILTER_CHIP: usize = 3200;
pub const NODE_BASE: usize = 4000;
pub const MISC: usize = 3300;
pub const CONFLICT_BTN: usize = 3400;
pub const DRILL_ROW: usize = 3500;
const NODE_W: f32 = 170.0;
const NODE_H: f32 = 54.0;
const GAP_X: f32 = 70.0;
const GAP_Y: f32 = 14.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentFilter {
    All,
    Agent(String),
    Unassigned,
}

impl AgentFilter {
    pub fn label(&self) -> String {
        match self {
            AgentFilter::All => "agent: all".into(),
            AgentFilter::Agent(n) => format!("agent: {n}"),
            AgentFilter::Unassigned => "agent: unassigned".into(),
        }
    }
}

/// What the agent-aware part of the board remembers between frames.
pub struct Ext {
    pub graph: Graph,
    pub doc: assign::Doc,
    doc_stamp: Option<SystemTime>,
    doc_polled: Instant,
    pub agent_filter: AgentFilter,
    /// The agent whose progress panel is open (its registry key).
    pub drill: Option<String>,
    pub drill_scroll: usize,
    /// The log tail of the open agent (filled by the IDE from the task manager).
    pub log: Vec<String>,
    pub auto: AutoAssign,
    pub zoom: f32,
    /// Fit the whole graph in the pane (until the person zooms or pans).
    pub fit: bool,
    pub pan: (f32, f32),
    pub critical: bool,
    pub dim_others: bool,
    pub tick: u64,
    node_rects: Vec<(String, R)>,
    /// Rows nobody works on and that can start now.
    pub ready: Vec<String>,
    agent_chips: Vec<String>,
    /// How the open panel was last laid out (for tests): (timeline rows shown, files shown).
    pub panel_shape: (usize, usize),
    /// Wait this long between looks at the registry file (0 in tests).
    pub poll_every: std::time::Duration,
}

impl Ext {
    pub fn new() -> Ext {
        Ext {
            graph: Graph::default(),
            doc: assign::Doc::default(),
            doc_stamp: None,
            doc_polled: Instant::now() - std::time::Duration::from_secs(10),
            agent_filter: AgentFilter::All,
            drill: None,
            drill_scroll: 0,
            log: vec![],
            auto: AutoAssign::Ask,
            zoom: 1.0,
            fit: true,
            pan: (0.0, 0.0),
            critical: true,
            dim_others: true,
            tick: 0,
            node_rects: vec![],
            ready: vec![],
            agent_chips: vec![],
            panel_shape: (0, 0),
            poll_every: std::time::Duration::from_millis(700),
        }
    }

    /// Does the agent filter let this row through?
    pub fn agent_ok(&self, row: &str) -> bool {
        match &self.agent_filter {
            AgentFilter::All => true,
            AgentFilter::Unassigned => self.doc.assignee(row).is_none(),
            AgentFilter::Agent(n) => self.doc.assignee(row).is_some_and(|(a, _)| a == n),
        }
    }

    /// The agents to show in the strip: at work, or finished within the last ten minutes (so the result can still be opened).
    pub fn strip_agents(&self, now: i64) -> Vec<&AgentRun> {
        self.doc.agents.iter().filter(|a| a.state.active() || (now - a.ended.max(a.updated)) < 600).collect()
    }

    pub fn agents_working(&self) -> usize {
        self.doc.agents.iter().filter(|a| a.state == AState::Working).count()
    }

    pub fn drilled(&self) -> Option<&AgentRun> {
        self.drill.as_ref().and_then(|k| self.doc.get(k))
    }
}

impl Default for Ext {
    fn default() -> Ext {
        Ext::new()
    }
}

pub fn state_color(st: AState) -> [u8; 4] {
    match st {
        AState::Working => CYAN,
        AState::WaitingApproval => AMBER,
        AState::Blocked => ERR,
        AState::Paused => rgb(140, 150, 185),
        AState::Idle => MUTED,
        AState::Failed => ERR,
        AState::Done => OK,
        AState::Interrupted => AMBER,
    }
}

fn clock(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

/// A small chain: two linked rounded links.
pub fn chain_icon(g: &mut Gfx, x: f32, y: f32, size: f32, c: [u8; 4]) {
    let lw = (size * 0.14).max(1.2);
    let w = size * 0.62;
    let h = size * 0.38;
    g.rrect_outline(x, y + size * 0.1, w, h, h / 2.0, lw, c);
    g.rrect_outline(x + size * 0.38, y + size * 0.5 - h * 0.1, w, h, h / 2.0, lw, c);
}

fn dep_state_color(st: &DepState) -> [u8; 4] {
    match st {
        DepState::Done => OK,
        DepState::Ready => CYAN,
        DepState::Waiting(_) => AMBER,
        DepState::Blocked(_) => ERR,
    }
}

impl BoardView {
    /// Read the agent registry again when it changed and rebuild the dependency graph. Cheap; called every frame.
    pub fn refresh_deps(&mut self, force: bool) {
        if force || self.ext.doc_polled.elapsed() >= self.ext.poll_every {
            self.ext.doc_polled = Instant::now();
            let stamp = std::fs::metadata(assign::path_of(&self.root)).and_then(|m| m.modified()).ok();
            if force || stamp != self.ext.doc_stamp || stamp.is_none() && !self.ext.doc.agents.is_empty() {
                self.ext.doc = assign::load(&self.root);
                self.ext.doc_stamp = stamp;
            }
        }
        let feats = self.file.as_ref().map(|f| f.board.features()).unwrap_or_default();
        self.ext.graph = Graph::build(&feats);
        let owned: Vec<String> = self.ext.doc.agents.iter().filter(|a| a.state.active()).flat_map(|a| a.rows.clone()).collect();
        self.ext.ready = iyakki_agent::boarddeps::ready_rows(&feats).into_iter().filter(|r| !owned.contains(r) && feats.iter().any(|f| &f.id == r && f.status == "planned")).collect();
    }

    /// What the card of a row says about its dependencies: "waits: F-A-1 · unblocks 2" (None when it has none and nothing waits for it).
    pub fn dep_line(&self, f: &Feature) -> Option<(String, [u8; 4])> {
        let g = &self.ext.graph;
        let has_deps = g.deps.contains_key(&f.id);
        let unblocks = g.unblocks(&f.id);
        if !has_deps && unblocks == 0 {
            return None;
        }
        let st = g.state(&f.id)?;
        let mut parts: Vec<String> = Vec::new();
        let color = match st {
            DepState::Waiting(v) => {
                parts.push(format!("waits: {}", v.join(", ")));
                AMBER
            }
            DepState::Blocked(v) if !v.is_empty() => {
                parts.push(format!("blocked by: {}", v.join(", ")));
                ERR
            }
            DepState::Ready if has_deps => {
                parts.push("ready".into());
                CYAN
            }
            DepState::Done => MUTED,
            _ => MUTED,
        };
        if unblocks > 0 {
            parts.push(format!("unblocks {unblocks}"));
        }
        Some((parts.join(" · "), color))
    }

    /// Extra height the card needs for its dependency and assignee lines.
    pub fn deps_card_extra(&self, f: &Feature, s: f32) -> f32 {
        let mut h = 0.0;
        if self.dep_line(f).is_some() {
            h += 15.0 * s;
        }
        if self.ext.doc.assignee(&f.id).is_some() {
            h += 15.0 * s;
        }
        h
    }

    /// The lines at the end of a card; returns the height used.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_card_deps(&self, g: &mut Gfx, f: &Feature, x: f32, y: f32, w: f32, s: f32, bg: [u8; 4]) -> f32 {
        let mut used = 0.0;
        if let Some((line, col)) = self.dep_line(f) {
            chain_icon(g, x, y + 1.0 * s, 12.0 * s, fx::readable(col, bg, 4.5));
            let t = g.fit(11.0 * s, &line, Face::Sans, w - 18.0 * s);
            g.text(x + 17.0 * s, y, 11.0 * s, fx::readable(col, bg, 4.5), &t, Face::Sans);
            used += 15.0 * s;
        }
        if let Some((name, st)) = self.ext.doc.assignee(&f.id) {
            let col = state_color(st);
            // the dot breathes while the agent works
            let phase = ((self.ext.tick % 48) as f32 / 48.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let r = if st == AState::Working { 2.6 * s + phase * 1.6 * s } else { 3.0 * s };
            g.circle(x + 4.0 * s, y + 6.5 * s, r, col);
            let label = format!("{name} {}", match st {
                AState::Working => "working",
                AState::WaitingApproval => "waiting for approval",
                other => other.label(),
            });
            let t = g.fit(11.0 * s, &label, Face::Bold, w - 18.0 * s);
            g.text(x + 17.0 * s, y, 11.0 * s, fx::readable(col, bg, 4.5), &t, Face::Bold);
            used += 15.0 * s;
        }
        used
    }

    /// The text of the table's Depends and Agent cells.
    pub fn depends_cell(&self, f: &Feature) -> String {
        match self.ext.graph.deps.get(&f.id) {
            Some(d) => d.join(", "),
            None => f.depends(),
        }
    }

    pub fn agent_cell(&self, f: &Feature) -> String {
        self.ext.doc.assignee(&f.id).map(|(n, st)| format!("{n} · {}", st.label())).unwrap_or_default()
    }

    pub fn show_depends_column(&self) -> bool {
        !self.ext.graph.deps.is_empty() || self.file.as_ref().and_then(|f| f.board.cols().map(|c| c.depends.is_some())).unwrap_or(false)
    }

    pub fn show_agent_column(&self) -> bool {
        self.ext.doc.agents.iter().any(|a| a.state.active())
    }

    /// The drawer's DEPENDS part: what the row waits for, what waits for it, and who works on it. Returns the height used.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_drawer_deps(&mut self, g: &mut Gfx, f: &Feature, x: f32, y0: f32, w: f32, limit: f32, s: f32, accent: [u8; 4]) -> f32 {
        let g_ = &self.ext.graph;
        let deps: Vec<String> = g_.deps.get(&f.id).cloned().unwrap_or_default();
        let unblocks: Vec<String> = g_.dependants.get(&f.id).cloned().unwrap_or_default();
        let assignee = self.ext.doc.assignee(&f.id).map(|(n, st)| (n.to_string(), st));
        let unknown: Vec<String> = g_.unknown.get(&f.id).cloned().unwrap_or_default();
        if deps.is_empty() && unblocks.is_empty() && assignee.is_none() && unknown.is_empty() {
            return 0.0;
        }
        let status_of: std::collections::BTreeMap<String, String> = self.file.as_ref().map(|b| b.board.features().into_iter().map(|x| (x.id, x.status)).collect()).unwrap_or_default();
        let mut y = y0;
        g.text(x, y, 10.0 * s, DIM, "DEPENDS", Face::Bold);
        y += 15.0 * s;
        let size = 11.0 * s;
        let chips = |g: &mut Gfx, y: &mut f32, label: &str, ids: &[String], base: usize| {
            if ids.is_empty() || *y + 22.0 * s > limit {
                return;
            }
            g.text(x, *y + 3.0 * s, size, MUTED, label, Face::Bold);
            let mut cx = x + g.text_width(size, label, Face::Bold) + 8.0 * s;
            for (i, id) in ids.iter().enumerate().take(12) {
                let st = status_of.get(id).cloned().unwrap_or_default();
                let t = format!("{id} {st}");
                let bw = g.text_width(size, &t, Face::Bold) + 14.0 * s;
                if cx + bw > x + w {
                    cx = x;
                    *y += 22.0 * s;
                    if *y + 22.0 * s > limit {
                        return;
                    }
                }
                let b = R::new(cx, *y, bw, 20.0 * s);
                let col = status_color(&st);
                g.rrect(b.x, b.y, b.w, b.h, 10.0 * s, if g.hovered(b) { rgb(40, 50, 86) } else { fx::WELL });
                g.rrect_outline(b.x, b.y, b.w, b.h, 10.0 * s, 1.0, with_alpha(col, 140));
                g.text(b.x + 7.0 * s, b.y + (b.h - g.line_height(size, Face::Bold)) / 2.0, size, fx::readable(accent, fx::WELL, 4.5), &t, Face::Bold);
                g.hit(b, Event::Tab(TAB_BASE + BASE + base + i), &format!("go to {id}"));
                cx += bw + 4.0 * s;
            }
            *y += 24.0 * s;
        };
        chips(g, &mut y, "waits for", &deps, 3600);
        chips(g, &mut y, "unblocks", &unblocks, 3650);
        if !unknown.is_empty() && y + 16.0 * s < limit {
            let t = g.fit(size, &format!("not rows of this board: {}", unknown.join(", ")), Face::Sans, w);
            g.text(x, y, size, AMBER, &t, Face::Sans);
            y += 16.0 * s;
        }
        if let Some((name, st)) = assignee {
            if y + 22.0 * s < limit {
                let label = format!("{name} · {}", st.label());
                let bw = g.text_width(size, &label, Face::Bold) + 22.0 * s;
                let b = R::new(x, y, bw, 20.0 * s);
                let col = state_color(st);
                g.rrect(b.x, b.y, b.w, b.h, 10.0 * s, if g.hovered(b) { rgb(40, 50, 86) } else { fx::WELL });
                g.circle(b.x + 9.0 * s, b.y + b.h / 2.0, 3.0 * s, col);
                g.text(b.x + 16.0 * s, b.y + (b.h - g.line_height(size, Face::Bold)) / 2.0, size, fx::readable(col, fx::WELL, 4.5), &label, Face::Bold);
                g.hit(b, Event::Tab(TAB_BASE + BASE + 3750), &format!("open {name}"));
                y += 24.0 * s;
            }
        }
        y - y0 + 4.0 * s
    }

    // ------------------------------------------------------------------------------------------------ events

    /// A click on one of this module's controls (`n` is the offset from `TAB_BASE`).
    pub fn deps_click(&mut self, n: usize) -> BoardAct {
        let now = iyakki_agent::clock::now_unix();
        match n {
            AGENT_CHIP..=3099 => {
                let key = self.ext.agent_chips.get(n - AGENT_CHIP).cloned();
                if let Some(k) = key {
                    self.ext.drill = if self.ext.drill.as_ref() == Some(&k) { None } else { Some(k) };
                    self.ext.drill_scroll = 0;
                    self.drawer = false;
                    if let Some(a) = self.ext.drilled() {
                        if let Some(r) = a.rows.first().cloned() {
                            self.select_id(&r);
                        }
                    }
                }
                BoardAct::Used
            }
            n3 if n3 == DRILL_BTN + 20 => {
                if let Some(c) = self.ext.doc.live_conflicts().first() {
                    self.ext.drill = Some(c.other.clone());
                    self.ext.drill_scroll = 0;
                    self.drawer = false;
                }
                BoardAct::Used
            }
            DRILL_BTN..=3199 => {
                let Some(a) = self.ext.drilled().cloned() else { return BoardAct::Used };
                let cmd = ['s', 'p', 'r', 'm', 'o', 'x'].get(n - DRILL_BTN).copied();
                match cmd {
                    Some('x') => {
                        self.ext.drill = None;
                        BoardAct::Used
                    }
                    Some(c) => BoardAct::AgentCmd(a.key, c),
                    None => BoardAct::Used,
                }
            }
            FILTER_CHIP => {
                self.cycle_agent_filter();
                BoardAct::Used
            }
            MISC => BoardAct::AutoAssign(self.ext.auto.next()),
            3301 => {
                let rows = self.ext.ready.clone();
                if rows.is_empty() { BoardAct::Used } else { BoardAct::AssignReady(rows) }
            }
            3302 => BoardAct::OpenNotes(None),
            CONFLICT_BTN..=3499 => {
                // pairs: even = queue, odd = let it continue
                let i = (n - CONFLICT_BTN) / 2;
                let choice = if (n - CONFLICT_BTN) % 2 == 0 { Choice::Queue } else { Choice::Continue };
                if let Some(c) = self.ext.doc.live_conflicts().get(i).map(|c| (*c).clone()) {
                    let (path, other) = (c.path.clone(), c.other.clone());
                    assign::update(&self.root, |d| d.resolve_conflict(&path, &other, choice));
                    self.refresh_deps(true);
                    self.say(format!("{}: {}", c.other_name, if choice == Choice::Queue { "waits for the other agent to finish with that file" } else { "goes on with it" }), true);
                }
                BoardAct::Used
            }
            3600..=3649 | 3650..=3699 => {
                let sel = self.selected().map(|f| f.id);
                let list = sel.and_then(|id| if n < 3650 { self.ext.graph.deps.get(&id).cloned() } else { self.ext.graph.dependants.get(&id).cloned() }).unwrap_or_default();
                if let Some(id) = list.get(if n < 3650 { n - 3600 } else { n - 3650 }).cloned() {
                    self.select_id(&id);
                }
                BoardAct::Used
            }
            3750 => {
                let key = self.selected().and_then(|f| self.ext.doc.active().find(|a| a.rows.iter().any(|r| r == &f.id)).map(|a| a.key.clone()));
                if key.is_some() {
                    self.ext.drill = key;
                    self.ext.drill_scroll = 0;
                    self.drawer = false;
                }
                BoardAct::Used
            }
            3800..=3899 => {
                // a note the agent added
                match self.ext.drilled().and_then(|a| a.notes.get(n - 3800).cloned()) {
                    Some(id) => BoardAct::OpenNotes(Some(id)),
                    None => BoardAct::Used,
                }
            }
            DRILL_ROW..=3599 => {
                // a row chip in the panel
                if let Some(r) = self.ext.drilled().and_then(|a| a.rows.get(n - DRILL_ROW).cloned()) {
                    self.select_id(&r);
                }
                BoardAct::Used
            }
            NODE_BASE..=5999 => {
                let i = n - NODE_BASE;
                if let Some((id, _)) = self.ext.node_rects.get(i).cloned() {
                    self.select_id(&id);
                    if crate::ui::click_count() >= 2 {
                        self.drawer = true;
                    }
                }
                BoardAct::Used
            }
            _ => {
                let _ = now;
                BoardAct::Used
            }
        }
    }

    pub fn cycle_agent_filter(&mut self) {
        let names: Vec<String> = self.ext.doc.agents.iter().map(|a| a.name.clone()).collect();
        self.ext.agent_filter = match &self.ext.agent_filter {
            AgentFilter::All => names.first().cloned().map(AgentFilter::Agent).unwrap_or(AgentFilter::Unassigned),
            AgentFilter::Agent(n) => match names.iter().position(|x| x == n).and_then(|i| names.get(i + 1)) {
                Some(next) => AgentFilter::Agent(next.clone()),
                None => AgentFilter::Unassigned,
            },
            AgentFilter::Unassigned => AgentFilter::All,
        };
        self.sel = 0;
    }

    /// Keys of the Graph view and of the open panel; None: not for this module.
    pub fn deps_key(&mut self, k: &Key) -> Option<BoardAct> {
        if self.ext.drill.is_some() {
            let cmd = match k {
                Key::Esc => {
                    self.ext.drill = None;
                    return Some(BoardAct::Used);
                }
                Key::Char('s') => 's',
                Key::Char('p') => 'p',
                Key::Char('r') => 'r',
                Key::Char('m') => 'm',
                Key::Char('o') => 'o',
                Key::PageUp => {
                    self.ext.drill_scroll += 5;
                    return Some(BoardAct::Used);
                }
                Key::PageDown => {
                    self.ext.drill_scroll = self.ext.drill_scroll.saturating_sub(5);
                    return Some(BoardAct::Used);
                }
                _ => '\0',
            };
            if cmd != '\0' {
                let key = self.ext.drill.clone()?;
                return Some(BoardAct::AgentCmd(key, cmd));
            }
        }
        match k {
            Key::Char('G') => {
                self.cycle_agent_filter();
                return Some(BoardAct::Used);
            }
            Key::Char('A') => {
                let rows = self.ext.ready.clone();
                return Some(if rows.is_empty() { BoardAct::Used } else { BoardAct::AssignReady(rows) });
            }
            Key::Char('D') => {
                // open the first working agent's panel, or close it
                self.ext.drill = if self.ext.drill.is_some() { None } else { self.ext.doc.active().next().map(|a| a.key.clone()) };
                return Some(BoardAct::Used);
            }
            _ => {}
        }
        if self.mode != Mode::Graph {
            return None;
        }
        let step = |this: &mut BoardView, dx: i32, dy: i32| this.graph_step(dx, dy);
        match k {
            Key::Char('+') | Key::Char('=') => self.zoom_by(1.2),
            Key::Char('-') => self.zoom_by(1.0 / 1.2),
            Key::Char('0') => {
                self.ext.fit = true;
                self.ext.pan = (0.0, 0.0);
            }
            Key::Char('t') => self.ext.critical = !self.ext.critical,
            Key::Char('f') => self.ext.dim_others = !self.ext.dim_others,
            Key::Nav { nav: Nav::Left, ctrl: true, .. } => self.pan_by(60.0, 0.0),
            Key::Nav { nav: Nav::Right, ctrl: true, .. } => self.pan_by(-60.0, 0.0),
            Key::Nav { nav: Nav::Up, ctrl: true, .. } => self.pan_by(0.0, 60.0),
            Key::Nav { nav: Nav::Down, ctrl: true, .. } => self.pan_by(0.0, -60.0),
            Key::Left | Key::Nav { nav: Nav::Left, ctrl: false, .. } => step(self, -1, 0),
            Key::Right | Key::Nav { nav: Nav::Right, ctrl: false, .. } => step(self, 1, 0),
            Key::Up | Key::Nav { nav: Nav::Up, ctrl: false, .. } => step(self, 0, -1),
            Key::Down | Key::Nav { nav: Nav::Down, ctrl: false, .. } => step(self, 0, 1),
            _ => return None,
        }
        Some(BoardAct::Used)
    }

    fn zoom_by(&mut self, f: f32) {
        if self.ext.fit {
            self.ext.fit = false;
        }
        self.ext.zoom = (self.ext.zoom * f).clamp(0.3, 2.5);
    }

    fn pan_by(&mut self, dx: f32, dy: f32) {
        self.ext.fit = false;
        self.ext.pan.0 += dx;
        self.ext.pan.1 += dy;
    }

    /// Move the selection along the graph: left to a dependency, right to a dependant, up and down within a layer.
    fn graph_step(&mut self, dx: i32, dy: i32) {
        let layers = self.ext.graph.layers();
        let Some(cur) = self.selected().map(|f| f.id) else {
            if let Some(first) = layers.first().and_then(|l| l.first()).cloned() {
                self.select_id(&first);
            }
            return;
        };
        let g = &self.ext.graph;
        let Some((li, pos)) = layers.iter().enumerate().find_map(|(li, l)| l.iter().position(|x| *x == cur).map(|p| (li, p))) else {
            if let Some(first) = layers.first().and_then(|l| l.first()).cloned() {
                self.select_id(&first);
            }
            return;
        };
        let target = if dx < 0 {
            g.deps.get(&cur).and_then(|d| d.first()).cloned()
        } else if dx > 0 {
            g.dependants.get(&cur).and_then(|d| d.iter().find(|x| layers.get(li + 1).is_some_and(|l| l.contains(x))).or(d.first())).cloned()
        } else {
            let l = &layers[li];
            let np = (pos as i32 + dy).clamp(0, l.len() as i32 - 1) as usize;
            l.get(np).cloned()
        };
        if let Some(t) = target {
            self.select_id(&t);
        }
    }

    pub fn graph_scroll(&mut self, d: i32) {
        self.zoom_by(if d > 0 { 1.1 } else { 1.0 / 1.1 });
    }

    // ------------------------------------------------------------------------------------------------ drawing

    /// The Agents strip: a chip per agent, who holds which row, the ready rows and the filters. Returns its height (0 when there is
    /// nothing to show: no agent has worked here and no row depends on another).
    pub fn draw_agents_strip(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) -> f32 {
        let now = iyakki_agent::clock::now_unix();
        let agents: Vec<AgentRun> = self.ext.strip_agents(now).into_iter().cloned().collect();
        let has_graph = !self.ext.graph.deps.is_empty();
        if agents.is_empty() && !has_graph {
            self.ext.agent_chips.clear();
            return 0.0;
        }
        let m = super::super::ide_chrome::cur();
        let size = m.font_small * s;
        let chip_h = (22.0 * s).max(20.0);
        let mut x = r.x;
        let mut y = r.y + 2.0 * s;
        let mut lines = 1;
        g.text(x, y + (chip_h - g.line_height(size, Face::Bold)) / 2.0, size, fx::readable(MUTED, fx::PANEL, 4.5), "Agents", Face::Bold);
        x += g.text_width(size, "Agents", Face::Bold) + 8.0 * s;
        self.ext.agent_chips.clear();
        if agents.is_empty() {
            g.text(x, y + (chip_h - g.line_height(size, Face::Sans)) / 2.0, size, DIM, "none working", Face::Sans);
            x += g.text_width(size, "none working", Face::Sans) + 10.0 * s;
        }
        for (i, a) in agents.iter().enumerate() {
            let col = state_color(a.state);
            let row = a.rows.first().cloned().unwrap_or_default();
            let label = format!("{}{} · {} · {}", a.name, if row.is_empty() { String::new() } else { format!(" · {row}") }, clock(a.elapsed(now)), a.state.label());
            let w = (g.text_width(size, &label, Face::Bold) + 30.0 * s).min(r.w);
            if x + w > r.x + r.w && x > r.x + 60.0 * s {
                if lines >= 2 {
                    g.text(x, y + (chip_h - g.line_height(size, Face::Sans)) / 2.0, size, DIM, &format!("+{} more", agents.len() - i), Face::Sans);
                    break;
                }
                lines += 1;
                x = r.x;
                y += chip_h + 3.0 * s;
            }
            let b = R::new(x, y, w, chip_h);
            let open = self.ext.drill.as_ref() == Some(&a.key);
            let hot = g.hovered(b);
            g.rrect(b.x, b.y, b.w, b.h, b.h / 2.0, if open { fx::over(fx::WELL, with_alpha(col, 70)) } else if hot { rgb(34, 44, 78) } else { fx::WELL });
            g.rrect_outline(b.x, b.y, b.w, b.h, b.h / 2.0, if open { 1.6 * s } else { 1.0 }, with_alpha(col, if open { 255 } else { 130 }));
            let phase = ((self.ext.tick % 48) as f32 / 48.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let dr = if a.state == AState::Working { 3.0 * s + phase * 1.5 * s } else { 3.5 * s };
            g.circle(b.x + 12.0 * s, b.y + b.h / 2.0, dr, col);
            let t = g.fit(size, &label, Face::Bold, b.w - 28.0 * s);
            g.text(b.x + 22.0 * s, b.y + (b.h - g.line_height(size, Face::Bold)) / 2.0, size, if open { TEXT } else { fx::readable(col, fx::WELL, 4.5) }, &t, Face::Bold);
            self.ext.agent_chips.push(a.key.clone());
            g.hit(b, Event::Tab(TAB_BASE + BASE + AGENT_CHIP + i), &format!("agent {}", a.name));
            x += w + 5.0 * s;
        }
        // the second row: the filter, the conflict badge, the ready rows, auto-assign
        y += chip_h + 3.0 * s;
        let mut x = r.x;
        let put = |g: &mut Gfx, label: &str, col: [u8; 4], on: bool, id: usize, tip: &str, x: &mut f32| {
            let w = g.text_width(size, label, Face::Bold) + 16.0 * s;
            if *x + w > r.x + r.w {
                return;
            }
            let b = R::new(*x, y, w, chip_h - 2.0 * s);
            let hot = g.hovered(b);
            if on || hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if on { fx::over(fx::WELL, with_alpha(col, 60)) } else { rgb(34, 44, 78) });
            }
            g.text(b.x + 8.0 * s, b.y + (b.h - g.line_height(size, Face::Bold)) / 2.0, size, if on { TEXT } else { fx::readable(col, fx::PANEL, 4.5) }, label, Face::Bold);
            let _ = tip;
            g.hit(b, Event::Tab(TAB_BASE + BASE + id), label);
            *x += w + 3.0 * s;
        };
        let filter = self.ext.agent_filter.label();
        put(g, &filter, MAGENTA, self.ext.agent_filter != AgentFilter::All, FILTER_CHIP, "filter the lanes by agent", &mut x);
        let conflicts = self.ext.doc.live_conflicts().len();
        if conflicts > 0 {
            let label = format!("{conflicts} file conflict{}", if conflicts == 1 { "" } else { "s" });
            put(g, &label, ERR, true, DRILL_BTN + 20, "two agents changed the same file: open an agent to choose", &mut x);
        }
        if !self.ext.ready.is_empty() {
            let rows = self.ext.ready.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            let label = format!("{} ready: {rows}{} · assign (A)", self.ext.ready.len(), if self.ext.ready.len() > 3 { ", …" } else { "" });
            put(g, &label, CYAN, false, 3301, "start an agent on each row whose dependencies are done", &mut x);
        }
        let auto = format!("auto-assign: {}", self.ext.auto.key());
        put(g, &auto, AMBER, self.ext.auto == AutoAssign::On, MISC, "when a row is done: off, ask (offer the rows that are ready) or on (start them)", &mut x);
        let _ = accent;
        let height = y + chip_h - r.y;
        height + 2.0 * s
    }

    /// The Graph view: rows as boxes in layers from the dependency to what depends on it, arrows between them, the critical path in amber.
    pub fn draw_graph(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        self.ext.node_rects.clear();
        let layers = self.ext.graph.layers();
        let legend_h = 18.0 * s;
        if layers.is_empty() {
            let lines = ["No dependencies between the rows yet.", "A row can wait for others: give it a Depends cell with their ids (F-APP-01, F-DB-02), or ask the assistant (board_update with depends). The Graph then draws the order, the critical path and who is working on what."];
            let mut y = r.y + 16.0 * s;
            for (i, l) in lines.iter().enumerate() {
                for w in g.wrap(if i == 0 { 15.0 } else { 12.5 } * s, l, Face::Sans, r.w - 24.0 * s) {
                    g.text(r.x + 12.0 * s, y, if i == 0 { 15.0 } else { 12.5 } * s, if i == 0 { TEXT } else { MUTED }, &w, if i == 0 { Face::Bold } else { Face::Sans });
                    y += (if i == 0 { 20.0 } else { 17.0 }) * s;
                }
                y += 8.0 * s;
            }
            return;
        }
        let nlayers = layers.len();
        let tallest = layers.iter().map(Vec::len).max().unwrap_or(1);
        let (mw, mh) = (nlayers as f32 * NODE_W + (nlayers as f32 - 1.0) * GAP_X, tallest as f32 * NODE_H + (tallest as f32 - 1.0) * GAP_Y);
        let area = R::new(r.x, r.y, r.w, (r.h - legend_h).max(40.0 * s));
        let fit = ((area.w - 24.0 * s) / (mw * s)).min((area.h - 16.0 * s) / (mh * s)).clamp(0.3, 1.6);
        if self.ext.fit {
            self.ext.zoom = fit;
            self.ext.pan = (0.0, 0.0);
        }
        let u = s * self.ext.zoom;
        let origin = (area.x + (area.w - mw * u) / 2.0 + self.ext.pan.0 * s, area.y + (area.h - mh * u) / 2.0 + self.ext.pan.1 * s);
        let sel = self.selected().map(|f| f.id);
        // what is upstream and downstream of the selection, and the critical path
        let chain: std::collections::BTreeSet<String> = match &sel {
            Some(id) if self.ext.dim_others => {
                let mut set = std::collections::BTreeSet::from([id.clone()]);
                let mut stack = vec![(id.clone(), true), (id.clone(), false)];
                while let Some((n, up)) = stack.pop() {
                    let next = if up { self.ext.graph.deps.get(&n) } else { self.ext.graph.dependants.get(&n) };
                    for m in next.into_iter().flatten() {
                        if set.insert(m.clone()) {
                            stack.push((m.clone(), up));
                        }
                    }
                }
                set
            }
            _ => Default::default(),
        };
        let critical: Vec<String> = if self.ext.critical { self.ext.graph.critical_path() } else { vec![] };
        // positions in model space
        let mut pos: std::collections::BTreeMap<String, (f32, f32)> = Default::default();
        for (li, l) in layers.iter().enumerate() {
            let lh = l.len() as f32 * NODE_H + (l.len() as f32 - 1.0) * GAP_Y;
            for (k, id) in l.iter().enumerate() {
                pos.insert(id.clone(), (li as f32 * (NODE_W + GAP_X), (mh - lh) / 2.0 + k as f32 * (NODE_H + GAP_Y)));
            }
        }
        let to_screen = |p: (f32, f32)| (origin.0 + p.0 * u, origin.1 + p.1 * u);
        let feats: std::collections::BTreeMap<String, Feature> = self.file.as_ref().map(|f| f.board.features().into_iter().map(|x| (x.id.clone(), x)).collect()).unwrap_or_default();
        let dim = !chain.is_empty();
        let ids: Vec<String> = layers.iter().flatten().cloned().collect();
        let graph = self.ext.graph.clone();
        let doc = self.ext.doc.clone();
        let tick = self.ext.tick;
        let mut rects: Vec<(String, R)> = Vec::new();
        g.clipped(area, |g| {
            // edges first, under the boxes
            for id in &ids {
                for d in graph.deps.get(id).into_iter().flatten() {
                    let (Some(a), Some(b)) = (pos.get(d), pos.get(id)) else { continue };
                    let (x0, y0) = to_screen((a.0 + NODE_W, a.1 + NODE_H / 2.0));
                    let (x1, y1) = to_screen((b.0, b.1 + NODE_H / 2.0));
                    let on_path = critical.windows(2).any(|w| w[0] == *d && w[1] == *id);
                    let in_chain = chain.contains(d) && chain.contains(id);
                    let col = if on_path { AMBER } else if in_chain { with_alpha(accent, 230) } else if dim { [90, 100, 135, 70] } else { [100, 112, 150, 170] };
                    let lw = if on_path { 2.6 * s } else if in_chain { 2.0 * s } else { 1.3 * s };
                    let mut prev = (x0, y0);
                    let steps = 14;
                    for i in 1..=steps {
                        let t = i as f32 / steps as f32;
                        // an S curve: smoothstep between the two heights, linear in x
                        let e = t * t * (3.0 - 2.0 * t);
                        let p = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * e);
                        g.line(prev.0, prev.1, p.0, p.1, lw, col);
                        prev = p;
                    }
                    let ah = 6.0 * u.max(0.5 * s);
                    g.polygon(&[(x1, y1), (x1 - ah * 1.4, y1 - ah * 0.7), (x1 - ah * 1.4, y1 + ah * 0.7)], col);
                }
            }
            for id in &ids {
                let Some(p) = pos.get(id) else { continue };
                let (sx, sy) = to_screen(*p);
                let nr = R::new(sx, sy, NODE_W * u, NODE_H * u);
                let f = feats.get(id);
                let st = graph.state(id).cloned().unwrap_or(DepState::Ready);
                let col = dep_state_color(&st);
                let selected = sel.as_deref() == Some(id.as_str());
                let faded = dim && !chain.contains(id);
                let a = if faded { 80 } else { 255 };
                g.rrect(nr.x, nr.y + 2.0 * u, nr.w, nr.h, 8.0 * u, [0, 0, 0, if faded { 20 } else { 60 }]);
                g.rrect(nr.x, nr.y, nr.w, nr.h, 8.0 * u, if selected { fx::over(rgb(24, 32, 60), with_alpha(accent, 50)) } else if faded { rgb(14, 19, 38) } else { rgb(18, 24, 46) });
                g.rrect_outline(nr.x, nr.y, nr.w, nr.h, 8.0 * u, if selected { 2.0 * s } else { 1.2 }, with_alpha(if selected { accent } else { col }, if selected { 255 } else { if faded { 60 } else { 190 } }));
                if critical.contains(id) {
                    g.rrect_outline(nr.x - 2.0 * s, nr.y - 2.0 * s, nr.w + 4.0 * s, nr.h + 4.0 * s, 10.0 * u, 1.2 * s, with_alpha(AMBER, if faded { 60 } else { 200 }));
                }
                g.rect(nr.x + 1.0 * u, nr.y + 8.0 * u, 3.0 * u, nr.h - 16.0 * u, with_alpha(col, a));
                if u >= 0.45 * s {
                    let size = (11.0 * u).max(8.0);
                    g.text(nr.x + 10.0 * u, nr.y + 5.0 * u, size, with_alpha(if faded { MUTED } else { accent }, a), id, Face::Bold);
                    if let Some(f) = f {
                        let title = g.fit(size, &f.short_title(), Face::Sans, nr.w - 16.0 * u);
                        g.text(nr.x + 10.0 * u, nr.y + 5.0 * u + g.line_height(size, Face::Bold) + 1.0 * u, size, with_alpha(TEXT, if faded { 90 } else { 255 }), &title, Face::Sans);
                        if let Some(pr) = f.progress {
                            let bar = R::new(nr.x + 10.0 * u, nr.y + nr.h - 9.0 * u, nr.w - 20.0 * u, 4.0 * u);
                            fx::progress_bar(g, bar, pr as f32 / 100.0, with_alpha(col, a), s);
                        }
                    }
                    if let Some((name, ast)) = doc.assignee(id) {
                        let ph = ((tick % 48) as f32 / 48.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                        let dc = state_color(ast);
                        let rr = if ast == AState::Working { 2.4 * u + ph * 1.4 * u } else { 2.6 * u };
                        let label = g.fit(size * 0.9, name, Face::Bold, 60.0 * u);
                        let lw = g.text_width(size * 0.9, &label, Face::Bold);
                        g.circle(nr.x + nr.w - lw - 14.0 * u, nr.y + 10.0 * u, rr, with_alpha(dc, a));
                        g.text(nr.x + nr.w - lw - 8.0 * u, nr.y + 5.0 * u, size * 0.9, with_alpha(fx::readable(dc, rgb(18, 24, 46), 4.5), a), &label, Face::Bold);
                    }
                } else {
                    g.circle(nr.x + nr.w / 2.0, nr.y + nr.h / 2.0, (nr.h / 4.0).max(3.0), with_alpha(col, a));
                }
                rects.push((id.clone(), nr));
            }
        });
        // click areas: only the part of a box that is inside the pane
        for (i, (id, nr)) in rects.iter().enumerate() {
            let vis = R::new(nr.x.max(area.x), nr.y.max(area.y), (nr.x + nr.w).min(area.x + area.w) - nr.x.max(area.x), (nr.y + nr.h).min(area.y + area.h) - nr.y.max(area.y));
            if vis.w > 4.0 && vis.h > 4.0 {
                g.hit(vis, Event::Tab(TAB_BASE + BASE + NODE_BASE + i), &format!("graph node {id}"));
            }
        }
        self.ext.node_rects = rects;
        // legend and keys
        let ly = r.y + r.h - legend_h + 2.0 * s;
        let mut lx = r.x + 4.0 * s;
        for (label, c) in [("done", OK), ("ready", CYAN), ("waiting", AMBER), ("blocked", ERR)] {
            g.circle(lx + 4.0 * s, ly + 7.0 * s, 3.5 * s, c);
            g.text(lx + 12.0 * s, ly, 11.0 * s, MUTED, label, Face::Sans);
            lx += g.text_width(11.0 * s, label, Face::Sans) + 24.0 * s;
        }
        let cp = if self.ext.critical && !critical.is_empty() { format!("critical path {}", critical.join(" → ")) } else { String::new() };
        let help = format!("{cp}{}+ − zoom {}% · Ctrl+arrows pan · 0 fit · t critical path", if cp.is_empty() { "" } else { "   " }, (self.ext.zoom * 100.0) as i32);
        let t = g.fit(11.0 * s, &help, Face::Sans, (r.x + r.w - lx - 8.0 * s).max(0.0));
        g.text(lx, ly, 11.0 * s, if focused { fx::readable(AMBER, fx::PANEL, 4.5) } else { DIM }, &t, Face::Sans);
    }

    /// The panel of one agent: its state and controls, its timeline, the files it holds, the notes it added and the tail of its log.
    pub fn draw_agent_panel(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4], narrow: bool) {
        fx::panel(g, r, s, accent);
        let Some(a) = self.ext.drilled().cloned() else {
            g.text(r.x + 14.0 * s, r.y + 14.0 * s, 13.0 * s, MUTED, "That agent is gone.", Face::Sans);
            return;
        };
        let now = iyakki_agent::clock::now_unix();
        let m = super::super::ide_chrome::cur();
        let pad = 12.0 * s;
        let x = r.x + pad;
        let w = r.w - 2.0 * pad;
        let col = state_color(a.state);
        let size = m.font_ui * s;
        let small = m.font_small * s;
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        let mut y = r.y + 8.0 * s;
        // title and the state
        let title = g.fit(size * 1.15, &a.title(), Face::Bold, w - 90.0 * s);
        g.text(x, y, size * 1.15, TEXT, &title, Face::Bold);
        let cl = if narrow { "‹ Board" } else { "Esc close" };
        let cw = g.text_width(small, cl, Face::Bold) + 14.0 * s;
        let cb = R::new(r.x + r.w - pad - cw, y - 1.0 * s, cw, 22.0 * s);
        g.rrect(cb.x, cb.y, cb.w, cb.h, 11.0 * s, fx::WELL);
        g.text(cb.x + 7.0 * s, cb.y + (cb.h - g.line_height(small, Face::Bold)) / 2.0, small, MUTED, cl, Face::Bold);
        g.hit(cb, Event::Tab(TAB_BASE + BASE + DRILL_BTN + 5), "close the agent panel");
        y += g.line_height(size * 1.15, Face::Bold) + 4.0 * s;
        let mut cx = x;
        cx += fx::chip(g, cx, y, s * 0.85, a.state.label(), col, a.state == AState::Working) + 5.0 * s;
        let usage = format!("{} · {}{}", clock(a.elapsed(now)), if a.tokens >= 1000 { format!("{}k tokens", a.tokens / 1000) } else { format!("{} tokens", a.tokens) }, if a.cost_cents > 0.0 { format!(" · ${:.2}", a.cost_cents / 100.0) } else { String::new() });
        let ut = g.fit(small, &usage, Face::Sans, (r.x + r.w - cx - pad).max(0.0));
        g.text(cx, y + 3.0 * s, small, chrome_muted(), &ut, Face::Sans);
        y += 24.0 * s;
        // controls
        let mut bx = x - 8.0 * s;
        let task_like = a.kind == "task";
        for (i, (label, tip, c, on)) in [
            ("Stop", "Stop the agent (s)", ERR, a.state.active() && task_like),
            ("Pause", "Pause at the end of the step (p)", AMBER, matches!(a.state, AState::Working | AState::WaitingApproval) && task_like),
            ("Resume", "Continue from the saved place (r)", OK, matches!(a.state, AState::Paused | AState::Failed | AState::Interrupted) && task_like),
            ("Message", "Send guidance: the agent pauses after its step and continues with it (m)", CYAN, task_like),
            ("Open run", "Open the run in the Tasks tab (o)", MUTED, task_like),
        ]
        .into_iter()
        .enumerate()
        {
            let bw = g.text_width(small + 1.0 * s, label, Face::Bold) + 16.0 * s;
            if bx + bw > r.x + r.w - 4.0 * s && bx > r.x + 40.0 * s {
                bx = x - 8.0 * s;
                y += (m.tool_h * s).max(20.0) + 2.0 * s;
            }
            bx += super::super::ide_chrome::text_btn(g, bx, y, s, label, c, on, Event::Tab(TAB_BASE + BASE + DRILL_BTN + i), &format!("agent {label}"), Some(tip), bounds) + 2.0 * s;
        }
        y += (m.tool_h * s).max(20.0) + 6.0 * s;
        // its rows with progress
        if !a.rows.is_empty() {
            let mut rx = x;
            for (i, row) in a.rows.iter().enumerate().take(6) {
                let p = self.file.as_ref().and_then(|f| f.board.get(row)).and_then(|f| f.progress);
                let label = format!("{row}{}", p.map(|p| format!(" {p}%")).unwrap_or_default());
                let bw = g.text_width(small, &label, Face::Bold) + 16.0 * s;
                if rx + bw > x + w {
                    break;
                }
                let b = R::new(rx, y, bw, 20.0 * s);
                g.rrect(b.x, b.y, b.w, b.h, 10.0 * s, if g.hovered(b) { rgb(40, 50, 86) } else { fx::WELL });
                g.text(b.x + 8.0 * s, b.y + (b.h - g.line_height(small, Face::Bold)) / 2.0, small, fx::readable(accent, fx::WELL, 4.5), &label, Face::Bold);
                g.hit(b, Event::Tab(TAB_BASE + BASE + DRILL_ROW + i), &format!("row {row}"));
                rx += bw + 4.0 * s;
            }
            y += 26.0 * s;
        }
        let bottom = r.y + r.h - 6.0 * s;
        let lh = g.line_height(small, Face::Sans) + 2.0 * s;
        let head = |g: &mut Gfx, y: &mut f32, t: &str| {
            g.text(x, *y, 10.0 * s, DIM, t, Face::Bold);
            *y += 14.0 * s;
        };
        // how much room each part may take: the timeline is the main thing, the others give way on a small screen
        let room = (bottom - y).max(0.0);
        let files: Vec<assign::Touched> = a.files.clone();
        let conflicts: Vec<assign::Conflict> = self.ext.doc.live_conflicts().into_iter().filter(|c| c.holder == a.key || c.other == a.key).cloned().collect();
        let want_files = if files.is_empty() { 0.0 } else { 14.0 * s + files.len().min(4) as f32 * lh + conflicts.len().min(2) as f32 * 24.0 * s };
        let want_notes = if a.notes.is_empty() { 0.0 } else { 14.0 * s + lh };
        let want_log = if self.ext.log.is_empty() { 0.0 } else { 14.0 * s + 4.0 * lh };
        let min_timeline = 14.0 * s + 3.0 * lh;
        let mut extra = room - min_timeline;
        let (mut show_files, mut show_notes, mut show_log) = (false, false, false);
        if want_files > 0.0 && extra >= want_files {
            show_files = true;
            extra -= want_files;
        }
        if want_notes > 0.0 && extra >= want_notes {
            show_notes = true;
            extra -= want_notes;
        }
        if want_log > 0.0 && extra >= want_log {
            show_log = true;
            extra -= want_log;
        }
        let _ = extra;
        let used_below = (if show_files { want_files } else { 0.0 }) + (if show_notes { want_notes } else { 0.0 }) + (if show_log { want_log } else { 0.0 });
        // the timeline
        head(g, &mut y, &format!("TIMELINE ({} steps, newest first)", a.steps.len()));
        let rows_fit = (((bottom - used_below - y) / lh) as usize).max(1);
        let total = a.steps.len();
        self.ext.drill_scroll = self.ext.drill_scroll.min(total.saturating_sub(rows_fit));
        let mut shown = 0;
        for st in a.steps.iter().rev().skip(self.ext.drill_scroll).take(rows_fit) {
            let c = match st.kind {
                StepKind::Plan => fx::readable(accent, fx::PANEL, 4.5),
                StepKind::Tool => rgb(205, 212, 232),
                StepKind::File => fx::readable(CYAN, fx::PANEL, 4.5),
                StepKind::Test => fx::readable(OK, fx::PANEL, 4.5),
                StepKind::Board => fx::readable(MAGENTA, fx::PANEL, 4.5),
                StepKind::Note => fx::readable(AMBER, fx::PANEL, 4.5),
                StepKind::Message => TEXT,
                StepKind::State => chrome_muted(),
            };
            let when = iyakki_agent::clock::format(&iyakki_agent::clock::Zone::system(), st.at, "%H:%M:%S");
            let tag = format!("{when} {:<5}", st.kind.key());
            let tw = g.text_width(small, &tag, Face::Mono);
            g.text(x, y, small, DIM, &tag, Face::Mono);
            let t = g.fit(small, &st.text, Face::Sans, (w - tw - 6.0 * s).max(10.0));
            g.text(x + tw + 6.0 * s, y, small, c, &t, Face::Sans);
            y += lh;
            shown += 1;
        }
        if a.steps.is_empty() {
            g.text(x, y, small, DIM, "nothing yet", Face::Sans);
        }
        if total > shown + self.ext.drill_scroll {
            let t = format!("▼ {} older (PageUp)", total - shown - self.ext.drill_scroll);
            let tw = g.text_width(10.0 * s, &t, Face::Bold);
            g.text(x + w - tw, r.y + r.h - used_below - 14.0 * s, 10.0 * s, DIM, &t, Face::Bold);
        }
        let mut y = (bottom - used_below).max(y);
        self.ext.panel_shape = (shown, if show_files { files.len().min(4) } else { 0 });
        if show_files {
            head(g, &mut y, &format!("FILES HELD ({})", files.len()));
            for f in files.iter().take(4) {
                let counts = format!("+{} -{}", f.adds, f.dels);
                let cwid = g.text_width(small, &counts, Face::Mono);
                let t = g.fit(small, &f.path, Face::Sans, w - cwid - 8.0 * s);
                let clash = conflicts.iter().any(|c| c.path == f.path);
                g.text(x, y, small, if clash { fx::readable(ERR, fx::PANEL, 4.5) } else { rgb(205, 212, 232) }, &t, Face::Sans);
                g.text(x + w - cwid, y, small, chrome_muted(), &counts, Face::Mono);
                y += lh;
            }
            for c in conflicts.iter().take(2) {
                let live = self.ext.doc.live_conflicts();
                let idx = live.iter().position(|l| l.path == c.path && l.other == c.other).unwrap_or(0);
                let msg = format!("{} is also changed by {}", c.path, if c.holder == a.key { &c.other_name } else { &c.holder_name });
                let t = g.fit(small, &msg, Face::Bold, w - 150.0 * s);
                g.text(x, y + 2.0 * s, small, fx::readable(ERR, fx::PANEL, 4.5), &t, Face::Bold);
                if c.other == a.key || true {
                    let q = super::super::ide_chrome::text_btn(g, x + w - 150.0 * s, y - 1.0 * s, s, if c.choice == Choice::Queue { "Queued" } else { "Queue it" }, AMBER, c.choice != Choice::Queue, Event::Tab(TAB_BASE + BASE + CONFLICT_BTN + idx * 2), "queue the second agent", Some("The second agent waits until the first has finished with the file"), bounds);
                    super::super::ide_chrome::text_btn(g, x + w - 150.0 * s + q + 2.0 * s, y - 1.0 * s, s, "Go on", OK, c.choice != Choice::Continue, Event::Tab(TAB_BASE + BASE + CONFLICT_BTN + idx * 2 + 1), "let it continue", Some("Let the second agent continue with the file"), bounds);
                }
                y += 24.0 * s;
            }
        }
        if show_notes {
            head(g, &mut y, "NOTES IT ADDED");
            let mut nx = x;
            for (i, id) in a.notes.iter().enumerate().take(8) {
                let bw = g.text_width(small, id, Face::Bold) + 14.0 * s;
                if nx + bw > x + w {
                    break;
                }
                let b = R::new(nx, y, bw, lh);
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if g.hovered(b) { rgb(40, 50, 86) } else { fx::WELL });
                g.text(b.x + 7.0 * s, b.y + 1.0 * s, small, fx::readable(AMBER, fx::WELL, 4.5), id, Face::Bold);
                g.hit(b, Event::Tab(TAB_BASE + BASE + 3800 + i), &format!("note {id}"));
                nx += bw + 4.0 * s;
            }
            y += lh;
        }
        if show_log {
            head(g, &mut y, "LOG");
            for l in self.ext.log.iter().rev().take(4).rev() {
                let t = g.fit(small, l, Face::Mono, w);
                g.text(x, y, small, DIM, &t, Face::Mono);
                y += lh;
            }
        }
    }
}

fn chrome_muted() -> [u8; 4] {
    super::super::ide_chrome::muted()
}
