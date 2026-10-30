//! Drawing the FLOWS graph: the grid, edges as curves with arrowheads, node cards with ports and badges, the connecting wire, the
//! rubber band and the minimap. The same painter serves the designer and the run view (a run adds colours, attempt counters and
//! moving dashes). The geometry is `flows_canvas`; nothing here decides anything.

use std::collections::{BTreeMap, BTreeSet};

use iyakki_flow::kinds::Registry;
use iyakki_flow::model::*;

use super::flows_canvas::*;
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

pub const GRID_AREA: usize = 71_000;
pub const MINIMAP_AREA: usize = 71_001;

/// What a run adds to the picture.
#[derive(Clone, Debug, Default)]
pub struct RunPaint {
    pub node_state: BTreeMap<String, NodeState>,
    /// (attempts made, attempts allowed, when the next one starts)
    pub attempts: BTreeMap<String, (u32, u32, Option<i64>)>,
    pub edges: BTreeMap<String, EdgeState>,
    pub now_ms: i64,
}

pub struct Paint<'a> {
    pub flow: &'a Flow,
    pub reg: &'a Registry,
    pub view: View,
    pub rect: R,
    pub selected: &'a BTreeSet<String>,
    pub sel_edge: Option<usize>,
    pub hover: &'a Hit,
    /// Per node: (errors, warnings) from validation.
    pub issues: &'a BTreeMap<String, (usize, usize)>,
    pub run: Option<&'a RunPaint>,
    pub connecting: Option<(&'a PortRef, (f32, f32))>,
    pub rubber: Option<R>,
    pub tick: u64,
    pub s: f32,
}

pub fn category_color(cat: &str) -> [u8; 4] {
    match cat {
        "Control" => rgb(140, 160, 210),
        "Call" => rgb(70, 225, 255),
        "Run" => rgb(255, 196, 94),
        "Human" => rgb(255, 120, 200),
        "Flow" => rgb(120, 230, 150),
        "Browser" => rgb(110, 200, 190),
        _ => rgb(170, 175, 200),
    }
}

pub fn port_color(name: &str) -> [u8; 4] {
    match name {
        "out" | "in" | "approved" => CYAN,
        "true" | "ok" => OK,
        "false" | "default" => AMBER,
        "error" | "rejected" => ERR,
        _ => rgb(190, 150, 255),
    }
}

pub fn state_color(st: NodeState) -> [u8; 4] {
    match st {
        NodeState::Pending => rgb(110, 120, 150),
        NodeState::Running => rgb(90, 170, 255),
        NodeState::Retrying => AMBER,
        NodeState::Waiting => rgb(190, 130, 255),
        NodeState::Succeeded => OK,
        NodeState::Failed => ERR,
        NodeState::Skipped => rgb(80, 88, 112),
        NodeState::Interrupted => rgb(255, 150, 80),
        NodeState::Cancelled => rgb(150, 100, 110),
    }
}

fn kind_category(reg: &Registry, kind: &str) -> String {
    reg.get(kind).map(|k| k.category().to_string()).unwrap_or_default()
}

/// Draw dashes along a polyline that move with `tick`.
fn dashed(g: &mut Gfx, pts: &[(f32, f32)], w: f32, c: [u8; 4], phase: f32, dash: f32, gap: f32) {
    let mut carry = -phase.rem_euclid(dash + gap);
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len < 0.01 {
            continue;
        }
        let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let mut t = carry;
        while t < len {
            let (s0, s1) = (t.max(0.0), (t + dash).min(len));
            if s1 > s0 {
                g.line(a.0 + ux * s0, a.1 + uy * s0, a.0 + ux * s1, a.1 + uy * s1, w, c);
            }
            t += dash + gap;
        }
        carry = t - len;
    }
}

fn arrow(g: &mut Gfx, tip: (f32, f32), from: (f32, f32), size: f32, c: [u8; 4]) {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    g.polygon(&[tip, (tip.0 - ux * size + px * size * 0.55, tip.1 - uy * size + py * size * 0.55), (tip.0 - ux * size - px * size * 0.55, tip.1 - uy * size - py * size * 0.55)], c);
}

/// The grid, the edges, the nodes, the wire being drawn and the rubber band, inside the canvas rectangle.
pub fn draw_graph(g: &mut Gfx, p: &Paint) {
    let (r, s, z) = (p.rect, p.s, p.view.zoom);
    g.hit(r, Event::Area(GRID_AREA), "canvas");
    fx::well(g, r, s);
    g.clipped(r, |g| {
        // dots on the grid
        if z >= 0.5 {
            let step = GRID * z * 2.0;
            let (ox, oy) = p.view.to_screen(r, (0.0, 0.0));
            let (mut x, x1) = (r.x + (ox - r.x).rem_euclid(step), r.x + r.w);
            while x < x1 {
                let mut y = r.y + (oy - r.y).rem_euclid(step);
                while y < r.y + r.h {
                    g.rect(x, y, 1.4, 1.4, with_alpha(rgb(120, 140, 200), 60));
                    y += step;
                }
                x += step;
            }
        }
        // edges under the nodes
        for (i, e) in p.flow.edges.iter().enumerate() {
            let pts: Vec<(f32, f32)> = edge_points(p.flow, p.reg, e).into_iter().map(|w| p.view.to_screen(r, w)).collect();
            if pts.len() < 2 {
                continue;
            }
            let selected = p.sel_edge == Some(i);
            let hover = *p.hover == Hit::Edge(i);
            let base = port_color(&e.port);
            let (color, w, moving) = match p.run.and_then(|rp| rp.edges.get(&e.key())) {
                Some(EdgeState::Taken) => {
                    let from_running = p.run.and_then(|rp| rp.node_state.get(&e.to)).is_some_and(|st| matches!(st, NodeState::Running | NodeState::Retrying));
                    (OK, 2.4, from_running)
                }
                Some(EdgeState::Dead) => (rgb(70, 78, 100), 1.2, false),
                Some(EdgeState::Open) => (with_alpha(base, 150), 1.6, p.run.and_then(|rp| rp.node_state.get(&e.from)).is_some_and(|st| *st == NodeState::Running)),
                None => (if selected { rgb(255, 255, 255) } else { with_alpha(base, if hover { 255 } else { 190 }) }, if selected || hover { 3.0 } else { 2.0 }, false),
            };
            let lw = (w * s * z.clamp(0.6, 1.6)).max(1.0);
            if moving && !fx::reduced_motion() {
                dashed(g, &pts, lw, color, p.tick as f32 * 0.7, 9.0 * s * z.max(0.6), 6.0 * s * z.max(0.6));
            } else {
                for seg in pts.windows(2) {
                    g.line(seg[0].0, seg[0].1, seg[1].0, seg[1].1, lw, color);
                }
            }
            let n = pts.len();
            arrow(g, pts[n - 1], pts[n - 3], 8.0 * s * z.clamp(0.7, 1.4), color);
            if !e.when.is_empty() && z >= 0.6 {
                let mid = pts[n / 2];
                let label = g.fit(10.0 * s, &e.when, Face::Mono, 120.0 * s);
                let tw = g.text_width(10.0 * s, &label, Face::Mono);
                g.rrect(mid.0 - tw / 2.0 - 4.0 * s, mid.1 - 8.0 * s, tw + 8.0 * s, 16.0 * s, 6.0 * s, fx::PANEL);
                g.text(mid.0 - tw / 2.0, mid.1 - 6.0 * s, 10.0 * s, AMBER, &label, Face::Mono);
            }
        }
        for n in &p.flow.nodes {
            draw_node(g, p, n);
        }
        if let Some((from, to)) = &p.connecting {
            if let Some(n) = p.flow.node(&from.node) {
                let (_, outs, _) = node_ports(p.reg, n);
                let w = port_world(n, from.side, &from.name, &[], &outs);
                let a = p.view.to_screen(r, w);
                let pts = bezier((a.0, a.1), (to.0, to.1), from.name == "error");
                for seg in pts.windows(2) {
                    g.line(seg[0].0, seg[0].1, seg[1].0, seg[1].1, 2.2 * s, with_alpha(MAGENTA, 220));
                }
                g.circle(to.0, to.1, 4.0 * s, MAGENTA);
            }
        }
        if let Some(b) = p.rubber {
            g.rrect(b.x, b.y, b.w, b.h, 3.0, with_alpha(CYAN, 36));
            g.rrect_outline(b.x, b.y, b.w, b.h, 3.0, 1.0, with_alpha(CYAN, 200));
        }
    });
}

/// The name shown on a node's card at scale `s` and zoom `z`: the whole name, cut with an ellipsis only when it does not fit the one line.
pub fn card_title(g: &mut Gfx, n: &Node, _s: f32, z: f32) -> String {
    let (pad, ic) = (9.0 * z, 15.0 * z);
    let ts = (11.5 * z).max(10.0);
    let room = NODE_W * z - pad - ic - 5.0 * z - 6.0 * z;
    g.fit(ts, n.title(), Face::Bold, room)
}

fn draw_node(g: &mut Gfx, p: &Paint, n: &Node) {
    let (r, s, z) = (p.rect, p.s, p.view.zoom);
    let (sx, sy) = p.view.to_screen(r, (n.pos.x, n.pos.y));
    let (w, h) = (NODE_W * z, NODE_H * z);
    if sx > r.x + r.w || sy > r.y + r.h || sx + w < r.x || sy + h < r.y {
        return;
    }
    let cat = kind_category(p.reg, &n.kind);
    let accent = category_color(&cat);
    let selected = p.selected.contains(&n.id);
    let hover = matches!(p.hover, Hit::Node(id) if *id == n.id);
    let run_state = p.run.and_then(|rp| rp.node_state.get(&n.id).copied());
    let border = match run_state {
        Some(st) => state_color(st),
        None if selected => rgb(255, 255, 255),
        None => accent,
    };
    let rad = 10.0 * s * z;
    if selected || matches!(run_state, Some(NodeState::Running | NodeState::Retrying | NodeState::Waiting | NodeState::Failed)) {
        let pulse = if matches!(run_state, Some(NodeState::Running)) && !fx::reduced_motion() { 60 + ((p.tick as f32 * 0.15).sin().abs() * 90.0) as u8 } else { 90 };
        g.rrect_glow(sx, sy, w, h, rad, 6.0 * s * z, with_alpha(border, pulse));
    }
    let dim = matches!(run_state, Some(NodeState::Skipped | NodeState::Cancelled)) || (p.run.is_some() && run_state == Some(NodeState::Pending));
    g.rrect(sx, sy, w, h, rad, if hover { fx::mix(fx::WELL, accent, 0.18) } else { fx::mix(fx::PANEL, accent, 0.10) });
    g.rrect(sx, sy, 5.0 * s * z, h, rad.min(5.0 * s * z), with_alpha(accent, if dim { 90 } else { 255 }));
    g.rrect_outline(sx, sy, w, h, rad, if selected { 2.2 } else { 1.4 }, with_alpha(border, if dim { 120 } else { 255 }));
    let icon_c = if dim { with_alpha(accent, 130) } else { accent };
    if z < 0.5 {
        // zoomed far out: the icon alone, as large as the card allows
        super::flows_icons::draw_kind(g, &n.kind, sx + w / 2.0, sy + h / 2.0, h * 0.55, icon_c);
    } else {
        let pad = 9.0 * z;
        let ic = 15.0 * z;
        super::flows_icons::draw_kind(g, &n.kind, sx + pad + ic / 2.0, sy + 8.0 * z + ic / 2.0, ic, icon_c);
        // one line: the step's name, cut with an ellipsis only when it really does not fit; the kind and the settings are in the
        // tooltip and the step's panel, not on the card
        let ts = (11.5 * z).max(10.0);
        let tx = sx + pad + ic + 5.0 * z;
        let title = card_title(g, n, s, z);
        g.text(tx, sy + 8.0 * z + (ic - ts * 1.2) / 2.0, ts, if dim { MUTED } else { TEXT }, &title, Face::Bold);
        // badges on the bottom edge: retry, sandbox, the agent's change, problems
        let mut bx = sx + pad;
        let by = sy + h - 19.0 * z;
        let bs = (9.5 * z).max(10.0);
        let badge = |g: &mut Gfx, text: &str, c: [u8; 4], x: &mut f32| {
            let tw = g.text_width(bs, text, Face::Bold);
            g.rrect(*x, by, tw + 8.0 * z, 14.0 * z, 5.0 * z, with_alpha(c, 50));
            g.text(*x + 4.0 * z, by + 1.0 * z, bs, c, text, Face::Bold);
            *x += tw + 12.0 * z;
        };
        if let Some(rp) = &n.retry {
            if rp.total_attempts() > 1 {
                badge(g, &format!("x{}", rp.total_attempts()), AMBER, &mut bx);
            }
        }
        if n.runner == Some(RunnerKind::K3sJob) || (n.runner.is_none() && p.flow.defaults.runner == RunnerKind::K3sJob) {
            badge(g, "k3s", rgb(120, 230, 150), &mut bx);
        }
        if n.modified_by.as_ref().is_some_and(Change::by_agent) {
            badge(g, "agent changed", MAGENTA, &mut bx);
        }
        if let Some((e, wn)) = p.issues.get(&n.id) {
            if *e + *wn > 0 {
                // the problem or warning as an icon with its count; the words are in the tooltip
                let isz = 14.0 * z;
                super::flows_icons::draw_alert(g, bx + isz / 2.0, by + 7.0 * z, isz, *e > 0);
                let txt = (e + wn).to_string();
                g.text(bx + isz + 3.0 * z, by + 1.0 * z, bs, if *e > 0 { ERR } else { AMBER }, &txt, Face::Bold);
            }
        }
    }
    // the run state as a round icon on the card's top right corner (the words are in the tooltip and the chip below the card)
    if let (Some(st), true) = (run_state, z >= 0.3) {
        let bsz = (15.0 * z).max(10.0);
        super::flows_icons::draw_state(g, st, sx + w - 3.0 * z, sy + 3.0 * z, bsz, p.tick);
    }
    // ports
    let (ins, outs, can_fail) = node_ports(p.reg, n);
    let pr = PORT_R * s * z.clamp(0.7, 1.3);
    for name in &ins {
        let (px, py) = p.view.to_screen(r, port_world(n, Side::In, name, &ins, &outs));
        let hot = matches!(p.hover, Hit::Port(pp) if pp.node == n.id && pp.side == Side::In);
        g.circle(px, py, pr + if hot { 2.0 } else { 0.0 }, fx::PANEL);
        g.ring(px, py, pr, 1.8, port_color(name));
    }
    let mut out_names = outs.clone();
    if can_fail && !outs.iter().any(|o| o == "error") {
        out_names.push("error".into());
    }
    for name in &out_names {
        let (px, py) = p.view.to_screen(r, port_world(n, Side::Out, name, &ins, &outs));
        let hot = matches!(p.hover, Hit::Port(pp) if pp.node == n.id && pp.side == Side::Out && pp.name == *name);
        let c = port_color(name);
        let small = name == "error" && !outs.iter().any(|o| o == "error");
        let rr = if small { pr * 0.75 } else { pr };
        g.circle(px, py, rr + if hot { 2.0 } else { 0.0 }, with_alpha(c, if hot { 255 } else { 210 }));
        if (outs.len() > 1 || hot) && z >= 0.7 && !small {
            let ls = (9.5 * s * z).max(10.0);
            let tw = g.text_width(ls, name, Face::Sans);
            g.text(px - tw - 9.0 * s * z, py - ls / 2.0 - 1.0, ls, with_alpha(c, 230), name, Face::Sans);
        }
    }
    if let (Some(rp), true) = (p.run, z >= 0.6) {
        if let Some(st) = run_state {
            let mut chip = st.label().to_string();
            if let Some((made, max, next)) = rp.attempts.get(&n.id) {
                if *max > 1 && matches!(st, NodeState::Running | NodeState::Retrying | NodeState::Failed) {
                    chip = format!("{} {}/{}", st.label(), made.max(&(if st == NodeState::Retrying { *made } else { 1 })), max);
                    if let (NodeState::Retrying, Some(t)) = (st, next) {
                        chip.push_str(&format!(" · next in {} ms", (t - rp.now_ms).max(0)));
                    }
                }
            }
            let cs = (10.5 * s * z).max(10.0);
            let tw = g.text_width(cs, &chip, Face::Bold);
            let (cx, cy) = (sx, sy + h + 4.0 * s * z);
            g.rrect(cx, cy, tw + 12.0 * s * z, 15.0 * s * z, 6.0 * s * z, with_alpha(state_color(st), 60));
            g.text(cx + 6.0 * s * z, cy + 1.5 * s * z, cs, state_color(st), &chip, Face::Bold);
        }
    }
}

/// The minimap in a corner of the canvas; registers a press area. Returns its geometry for hit-testing.
pub fn draw_minimap(g: &mut Gfx, flow: &Flow, view: &View, canvas: R, mini: R, s: f32, states: Option<&BTreeMap<String, NodeState>>) -> MiniMap {
    g.rrect(mini.x, mini.y, mini.w, mini.h, 6.0 * s, with_alpha(fx::PANEL, 230));
    g.rrect_outline(mini.x, mini.y, mini.w, mini.h, 6.0 * s, 1.0, with_alpha(CYAN, 120));
    let m = minimap(flow, view, canvas, mini);
    for (id, r) in &m.nodes {
        let c = states.and_then(|st| st.get(id)).map(|st| state_color(*st)).unwrap_or_else(|| flow.node(id).map(|n| category_color(&n.kind)).map(|_| rgb(110, 150, 220)).unwrap_or(DIM));
        g.rect(r.x, r.y, r.w.max(2.0), r.h.max(2.0), c);
    }
    let v = m.viewport;
    let vx0 = v.x.max(mini.x + 1.0);
    let vy0 = v.y.max(mini.y + 1.0);
    let vx1 = (v.x + v.w).min(mini.x + mini.w - 1.0);
    let vy1 = (v.y + v.h).min(mini.y + mini.h - 1.0);
    if vx1 > vx0 && vy1 > vy0 {
        g.rrect_outline(vx0, vy0, vx1 - vx0, vy1 - vy0, 2.0, 1.2, with_alpha(MAGENTA, 230));
    }
    g.hit(mini, Event::Area(MINIMAP_AREA), "minimap");
    m
}
