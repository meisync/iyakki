//! The geometry of the FLOWS canvas, with no drawing: the world and the screen, node cards and their ports, edges as curves,
//! hit-testing, snapping to the grid, the rules for connecting, adding, deleting, copying and pasting nodes, and the minimap.
//! `flows_paint.rs` draws it; `flows.rs` drives it. Everything here is plain data in and out, so it is tested without a window.

use std::collections::{BTreeMap, BTreeSet};

use iyakki_flow::kinds::Registry;
use iyakki_flow::model::*;
use serde_json::{json, Value};

use crate::ui::R;

pub const NODE_W: f32 = 176.0;
pub const NODE_H: f32 = 64.0;
pub const GRID: f32 = 20.0;
pub const PORT_R: f32 = 6.0;
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 3.0;
/// How close (screen pixels) a press must be to a port or an edge to take it.
pub const PORT_HIT: f32 = 10.0;
pub const EDGE_HIT: f32 = 6.0;

/// Where the canvas is looking: `screen = rect.origin + (world + pan) * zoom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub pan: (f32, f32),
    pub zoom: f32,
}

impl Default for View {
    fn default() -> View {
        View { pan: (0.0, 0.0), zoom: 1.0 }
    }
}

impl View {
    pub fn to_screen(&self, rect: R, w: (f32, f32)) -> (f32, f32) {
        (rect.x + (w.0 + self.pan.0) * self.zoom, rect.y + (w.1 + self.pan.1) * self.zoom)
    }

    pub fn to_world(&self, rect: R, p: (f32, f32)) -> (f32, f32) {
        ((p.0 - rect.x) / self.zoom - self.pan.0, (p.1 - rect.y) / self.zoom - self.pan.1)
    }

    /// Move the picture by a distance on the screen.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan.0 += dx / self.zoom;
        self.pan.1 += dy / self.zoom;
    }

    /// Zoom by `factor` keeping the world point under (px, py) where it is.
    pub fn zoom_at(&mut self, rect: R, px: f32, py: f32, factor: f32) {
        let before = self.to_world(rect, (px, py));
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.to_world(rect, (px, py));
        self.pan.0 += after.0 - before.0;
        self.pan.1 += after.1 - before.1;
    }

    /// A view that shows all of the flow in `rect` (never zoomed in past 1).
    pub fn fit(flow: &Flow, rect: R) -> View {
        let Some((x0, y0, x1, y1)) = bounds(flow) else { return View::default() };
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let pad = 40.0;
        let zoom = ((rect.w - 2.0 * pad) / w).min((rect.h - 2.0 * pad) / h).clamp(MIN_ZOOM, 1.0);
        let (vw, vh) = (rect.w / zoom, rect.h / zoom);
        View { pan: (-(x0 - (vw - w) / 2.0), -(y0 - (vh - h) / 2.0)), zoom }
    }
}

pub fn snap(v: f32) -> f32 {
    (v / GRID).round() * GRID
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PortRef {
    pub node: String,
    pub side: Side,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Port(PortRef),
    Node(String),
    Edge(usize),
    Empty,
}

/// The ports of a node as the canvas draws them: input names, output names, and whether it also has the `error` output.
pub fn node_ports(reg: &Registry, n: &Node) -> (Vec<String>, Vec<String>, bool) {
    match reg.get(&n.kind) {
        Some(k) => {
            let p = k.ports(n);
            (p.inputs, p.outputs, k.can_fail())
        }
        None => (vec!["in".into()], vec!["out".into()], true),
    }
}

pub fn node_rect(n: &Node) -> (f32, f32, f32, f32) {
    (n.pos.x, n.pos.y, NODE_W, NODE_H)
}

/// The place of a port in the world. Inputs sit on the left edge, outputs on the right, spread evenly; the `error` output is a small
/// port on the bottom edge near the right.
pub fn port_world(n: &Node, side: Side, name: &str, inputs: &[String], outputs: &[String]) -> (f32, f32) {
    match side {
        Side::In => {
            let i = inputs.iter().position(|p| p == name).unwrap_or(0);
            (n.pos.x, n.pos.y + NODE_H * (i as f32 + 1.0) / (inputs.len() as f32 + 1.0))
        }
        Side::Out if name == "error" && !outputs.iter().any(|o| o == "error") => (n.pos.x + NODE_W - 22.0, n.pos.y + NODE_H),
        Side::Out => {
            let i = outputs.iter().position(|p| p == name).unwrap_or(0);
            (n.pos.x + NODE_W, n.pos.y + NODE_H * (i as f32 + 1.0) / (outputs.len() as f32 + 1.0))
        }
    }
}

pub fn bounds(flow: &Flow) -> Option<(f32, f32, f32, f32)> {
    let mut it = flow.nodes.iter();
    let f = it.next()?;
    let mut b = (f.pos.x, f.pos.y, f.pos.x + NODE_W, f.pos.y + NODE_H);
    for n in it {
        b = (b.0.min(n.pos.x), b.1.min(n.pos.y), b.2.max(n.pos.x + NODE_W), b.3.max(n.pos.y + NODE_H));
    }
    Some(b)
}

/// The curve of an edge as a polyline in the world (a cubic from the output to the input).
pub fn edge_points(flow: &Flow, reg: &Registry, e: &Edge) -> Vec<(f32, f32)> {
    let (Some(a), Some(b)) = (flow.node(&e.from), flow.node(&e.to)) else { return vec![] };
    let (_, aouts, _) = node_ports(reg, a);
    let (bins, _, _) = node_ports(reg, b);
    let p0 = port_world(a, Side::Out, &e.port, &[], &aouts);
    let p3 = port_world(b, Side::In, "in", &bins, &[]);
    bezier(p0, p3, e.port == "error")
}

pub fn bezier(p0: (f32, f32), p3: (f32, f32), from_bottom: bool) -> Vec<(f32, f32)> {
    let dx = ((p3.0 - p0.0).abs() * 0.5).max(40.0);
    let (p1, p2) = if from_bottom { ((p0.0, p0.1 + dx.min(80.0)), (p3.0 - dx, p3.1)) } else { ((p0.0 + dx, p0.1), (p3.0 - dx, p3.1)) };
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1)
        })
        .collect()
}

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// What is under a press at (px, py): a port first, then a node (the one drawn last wins), then an edge, else nothing.
pub fn hit_test(flow: &Flow, reg: &Registry, view: &View, rect: R, px: f32, py: f32) -> Hit {
    if !rect.contains(px as i32, py as i32) {
        return Hit::Empty;
    }
    let to_s = |w: (f32, f32)| view.to_screen(rect, w);
    // ports, nodes in drawing order from the top
    for n in flow.nodes.iter().rev() {
        let (ins, outs, can_fail) = node_ports(reg, n);
        let mut candidates: Vec<PortRef> = outs.iter().map(|o| PortRef { node: n.id.clone(), side: Side::Out, name: o.clone() }).collect();
        if can_fail && !outs.iter().any(|o| o == "error") {
            candidates.push(PortRef { node: n.id.clone(), side: Side::Out, name: "error".into() });
        }
        candidates.extend(ins.iter().map(|i| PortRef { node: n.id.clone(), side: Side::In, name: i.clone() }));
        let mut best: Option<(f32, PortRef)> = None;
        for c in candidates {
            let w = port_world(n, c.side, &c.name, &ins, &outs);
            let s = to_s(w);
            let d = ((s.0 - px).powi(2) + (s.1 - py).powi(2)).sqrt();
            if d <= PORT_HIT * view.zoom.clamp(0.8, 1.4) && best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, c));
            }
        }
        if let Some((_, p)) = best {
            return Hit::Port(p);
        }
    }
    for n in flow.nodes.iter().rev() {
        let (x, y, w, h) = node_rect(n);
        let (sx, sy) = to_s((x, y));
        if px >= sx && px <= sx + w * view.zoom && py >= sy && py <= sy + h * view.zoom {
            return Hit::Node(n.id.clone());
        }
    }
    for (i, e) in flow.edges.iter().enumerate().rev() {
        let pts: Vec<(f32, f32)> = edge_points(flow, reg, e).into_iter().map(to_s).collect();
        if pts.windows(2).any(|w| dist_to_segment((px, py), w[0], w[1]) <= EDGE_HIT) {
            return Hit::Edge(i);
        }
    }
    Hit::Empty
}

/// The nodes whose card touches a rectangle on the screen (a rubber band).
pub fn nodes_in(flow: &Flow, view: &View, rect: R, band: R) -> Vec<String> {
    flow.nodes
        .iter()
        .filter(|n| {
            let (sx, sy) = view.to_screen(rect, (n.pos.x, n.pos.y));
            let (w, h) = (NODE_W * view.zoom, NODE_H * view.zoom);
            sx < band.x + band.w && sx + w > band.x && sy < band.y + band.h && sy + h > band.y
        })
        .map(|n| n.id.clone())
        .collect()
}

/// Does a path of edges lead from `a` to `b`?
pub fn reaches(flow: &Flow, a: &str, b: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![a.to_string()];
    while let Some(n) = stack.pop() {
        if n == b {
            return true;
        }
        if seen.insert(n.clone()) {
            stack.extend(flow.edges.iter().filter(|e| e.from == n).map(|e| e.to.clone()));
        }
    }
    false
}

/// May an output port be joined to a node's input: not to itself, not twice, not into a start node, and never so that a loop is made.
pub fn can_connect(flow: &Flow, reg: &Registry, from: &str, port: &str, to: &str) -> Result<(), String> {
    let (a, b) = (flow.node(from).ok_or("no such step")?, flow.node(to).ok_or("no such step")?);
    if from == to {
        return Err("a step cannot be joined to itself".into());
    }
    let (_, outs, can_fail) = node_ports(reg, a);
    if !outs.iter().any(|o| o == port) && !(port == "error" && can_fail) {
        return Err(format!("`{from}` has no `{port}` output"));
    }
    let (ins, _, _) = node_ports(reg, b);
    if ins.is_empty() {
        return Err(format!("`{to}` is a start: nothing joins into it"));
    }
    if flow.edges.iter().any(|e| e.from == from && e.port == port && e.to == to) {
        return Err("they are joined already".into());
    }
    if reaches(flow, to, from) {
        return Err("that would make a loop (use a for-each step to repeat)".into());
    }
    Ok(())
}

pub fn connect(flow: &mut Flow, reg: &Registry, from: &str, port: &str, to: &str) -> Result<(), String> {
    can_connect(flow, reg, from, port, to)?;
    flow.edges.push(Edge { from: from.into(), port: port.into(), to: to.into(), when: String::new() });
    Ok(())
}

/// Move nodes by a distance in the world; with `snapped` the nodes land on the grid.
pub fn move_nodes(flow: &mut Flow, ids: &[String], dx: f32, dy: f32, snapped: bool) {
    for n in flow.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
        n.pos.x += dx;
        n.pos.y += dy;
        if snapped {
            n.pos.x = snap(n.pos.x);
            n.pos.y = snap(n.pos.y);
        }
    }
}

pub fn unique_node_id(flow: &Flow, base: &str) -> String {
    unique_among(base, &|id| flow.node(id).is_some())
}

/// `base`, or `base2`, `base3`... the first that `taken` does not know.
pub fn unique_among(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let base = if base.is_empty() { "node".to_string() } else { base };
    if !taken(&base) {
        return base;
    }
    let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('-');
    let stem = if stem.is_empty() { "node" } else { stem };
    (2..).map(|n| format!("{stem}{n}")).find(|id| !taken(id)).unwrap_or(base)
}

/// A starting configuration for a new node of a kind (what the palette gives).
pub fn skeleton(kind: &str) -> Value {
    match kind {
        "http" => json!({"method": "GET", "url": "https://"}),
        "remote" => json!({"method": "POST", "url": "https://", "auth": {"kind": "bearer", "secret": ""}}),
        "cli" => json!({"command": ["echo", "hello"]}),
        "grpc" => json!({"address": "localhost:50051", "method": "pkg.Service/Method", "plaintext": true, "data": {}}),
        "thrift" => json!({"endpoint": "", "service": "", "method": "", "protocol": "json-http"}),
        "browser" => json!({"steps": [{"open": "https://example.com"}], "allow_domains": ["example.com"], "level": "read_only"}),
        "agent" => json!({"prompt": "", "max_cents": 20}),
        "script" => json!({"lang": "js", "source": "return args;"}),
        "condition" => json!({"expr": "true"}),
        "switch" => json!({"value": "input.status", "cases": [{"equals": 200, "port": "ok"}]}),
        "delay" => json!({"seconds": 5}),
        "approval" => json!({"message": "Continue?"}),
        "notify" => json!({"title": "Hello", "body": ""}),
        "subflow" => json!({"flow": ""}),
        "foreach" => json!({"items": "{{ input.items }}", "flow": "", "max_items": 100}),
        "join" => json!({"mode": "any"}),
        "end" => json!({"outputs": {}}),
        "cam_photo" => json!({"keep": false}),
        "br_open" => json!({"url": "http://localhost:8080/", "wait": "idle"}),
        "br_screenshot" => json!({"name": "screenshot"}),
        "br_dom" => json!({"mode": "summary", "console": true}),
        "br_assert" => json!({"check": "exists", "selector": "h1"}),
        "br_click" => json!({"selector": "button"}),
        "br_type" => json!({"selector": "input", "text": "hello"}),
        "br_fill" => json!({"fields": {"#email": "qa@example.com"}}),
        "br_submit" => json!({"selector": "form"}),
        "br_key" => json!({"key": "Enter"}),
        "br_hover" => json!({"selector": "a"}),
        "br_scroll" => json!({"to": "bottom"}),
        "br_select" => json!({"selector": "select", "option": "1", "by": "index"}),
        "br_wait" => json!({"for": "selector", "value": "h1"}),
        "br_extract" => json!({"fields": {"title": "h1"}}),
        "br_js" => json!({"expression": "document.title"}),
        "br_frame" => json!({"action": "main"}),
        "br_viewport" => json!({"preset": "phone 375x667"}),
        "br_storage" => json!({"kind": "local_storage", "name": "theme", "value": "dark"}),
        "br_download" => json!({}),
        "br_compare" => json!({"name": "page", "threshold": "0.5"}),
        _ => json!({}),
    }
}

/// Add a node of a kind at a place (snapped); returns its id.
pub fn add_node(flow: &mut Flow, kind: &str, at: (f32, f32)) -> String {
    let id = unique_node_id(flow, kind);
    let mut n = Node::new(&id, kind, skeleton(kind));
    n.pos = Pos { x: snap(at.0), y: snap(at.1) };
    flow.nodes.push(n);
    id
}

pub fn delete_nodes(flow: &mut Flow, ids: &[String]) {
    flow.nodes.retain(|n| !ids.contains(&n.id));
    flow.edges.retain(|e| !ids.contains(&e.from) && !ids.contains(&e.to));
}

/// Copied nodes and the edges between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

pub fn copy(flow: &Flow, ids: &[String]) -> Clip {
    Clip { nodes: flow.nodes.iter().filter(|n| ids.contains(&n.id)).cloned().collect(), edges: flow.edges.iter().filter(|e| ids.contains(&e.from) && ids.contains(&e.to)).cloned().collect() }
}

fn rename_refs(v: &mut Value, map: &BTreeMap<String, String>) {
    match v {
        Value::String(s) => {
            for (old, new) in map {
                if s.contains(&format!("nodes.{old}.")) {
                    *s = s.replace(&format!("nodes.{old}."), &format!("nodes.{new}."));
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rename_refs(x, map)),
        Value::Object(o) => o.values_mut().for_each(|x| rename_refs(x, map)),
        _ => {}
    }
}

/// Paste a clip with new ids, moved by `offset`, with the references between the pasted nodes renamed; returns the new ids.
pub fn paste(flow: &mut Flow, clip: &Clip, offset: (f32, f32)) -> Vec<String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut taken: BTreeSet<String> = flow.nodes.iter().map(|n| n.id.clone()).collect();
    for n in &clip.nodes {
        let id = unique_among(&n.id, &|x| taken.contains(x));
        taken.insert(id.clone());
        map.insert(n.id.clone(), id);
    }
    let mut made = Vec::new();
    for n in &clip.nodes {
        let mut c = n.clone();
        c.id = map[&n.id].clone();
        c.pos = Pos { x: snap(n.pos.x + offset.0), y: snap(n.pos.y + offset.1) };
        c.modified_by = None;
        rename_refs(&mut c.config, &map);
        made.push(c.id.clone());
        flow.nodes.push(c);
    }
    for e in &clip.edges {
        flow.edges.push(Edge { from: map[&e.from].clone(), port: e.port.clone(), to: map[&e.to].clone(), when: e.when.clone() });
    }
    made
}

/// The minimap: the flow scaled into a small rectangle, and the part the canvas shows.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniMap {
    pub scale: f32,
    pub origin: (f32, f32),
    pub nodes: Vec<(String, R)>,
    pub viewport: R,
}

pub fn minimap(flow: &Flow, view: &View, canvas: R, mini: R) -> MiniMap {
    let (x0, y0, x1, y1) = bounds(flow).unwrap_or((0.0, 0.0, NODE_W, NODE_H));
    // the shown part of the world is part of the extent, so the viewport box is always inside the minimap
    let (vx0, vy0) = view.to_world(canvas, (canvas.x, canvas.y));
    let (vx1, vy1) = view.to_world(canvas, (canvas.x + canvas.w, canvas.y + canvas.h));
    let (ex0, ey0, ex1, ey1) = (x0.min(vx0), y0.min(vy0), x1.max(vx1), y1.max(vy1));
    let scale = ((mini.w - 6.0) / (ex1 - ex0).max(1.0)).min((mini.h - 6.0) / (ey1 - ey0).max(1.0));
    let origin = (mini.x + 3.0 - ex0 * scale + ((mini.w - 6.0) - (ex1 - ex0) * scale) / 2.0, mini.y + 3.0 - ey0 * scale + ((mini.h - 6.0) - (ey1 - ey0) * scale) / 2.0);
    let at = |wx: f32, wy: f32| (origin.0 + wx * scale, origin.1 + wy * scale);
    let nodes = flow.nodes.iter().map(|n| { let p = at(n.pos.x, n.pos.y); (n.id.clone(), R::new(p.0, p.1, NODE_W * scale, NODE_H * scale)) }).collect();
    let (a, b) = (at(vx0, vy0), at(vx1, vy1));
    MiniMap { scale, origin, nodes, viewport: R::new(a.0, a.1, b.0 - a.0, b.1 - a.1) }
}

/// A click in the minimap, as the world point it stands for.
pub fn minimap_to_world(m: &MiniMap, px: f32, py: f32) -> (f32, f32) {
    ((px - m.origin.0) / m.scale, (py - m.origin.1) / m.scale)
}

/// Centre the canvas on a world point.
pub fn center_on(view: &mut View, canvas: R, w: (f32, f32)) {
    view.pan = (-w.0 + canvas.w / (2.0 * view.zoom), -w.1 + canvas.h / (2.0 * view.zoom));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow() -> Flow {
        let mut f = Flow::new("f", "F");
        for (i, (id, k)) in [("s", "start"), ("a", "http"), ("c", "condition"), ("e", "end")].iter().enumerate() {
            f.nodes.push(Node::new(id, k, skeleton(k)).at(40.0 + i as f32 * 240.0, 60.0));
        }
        f.edges = vec![Edge::new("s", "a"), Edge::new("a", "c"), Edge::new("c", "e").port("true")];
        f
    }

    const RECT: R = R { x: 100.0, y: 80.0, w: 900.0, h: 500.0 };

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (self.next() % 10_000) as f32 / 10_000.0 * (hi - lo)
        }
    }

    #[test]
    fn world_and_screen_are_inverses_under_any_pan_and_zoom() {
        let mut r = Rng(5);
        for _ in 0..500 {
            let v = View { pan: (r.f(-500.0, 500.0), r.f(-500.0, 500.0)), zoom: r.f(MIN_ZOOM, MAX_ZOOM) };
            let w = (r.f(-800.0, 1500.0), r.f(-800.0, 1500.0));
            let s = v.to_screen(RECT, w);
            let back = v.to_world(RECT, s);
            assert!((back.0 - w.0).abs() < 0.05 && (back.1 - w.1).abs() < 0.05, "{w:?} {back:?}");
        }
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_and_stays_in_range() {
        let mut v = View::default();
        let (px, py) = (500.0, 300.0);
        let before = v.to_world(RECT, (px, py));
        for f in [1.1, 1.1, 0.8, 1.5, 1.5, 1.5, 2.0] {
            v.zoom_at(RECT, px, py, f);
            let after = v.to_world(RECT, (px, py));
            assert!((after.0 - before.0).abs() < 0.01 && (after.1 - before.1).abs() < 0.01);
            assert!((MIN_ZOOM..=MAX_ZOOM).contains(&v.zoom));
        }
        for _ in 0..30 {
            v.zoom_at(RECT, px, py, 0.5);
        }
        assert_eq!(v.zoom, MIN_ZOOM);
        let mut p = View::default();
        p.pan_by(30.0, -10.0);
        assert_eq!(p.pan, (30.0, -10.0));
        let mut z = View { pan: (0.0, 0.0), zoom: 2.0 };
        z.pan_by(30.0, 0.0);
        assert_eq!(z.pan.0, 15.0, "a screen distance is half as far in the world at zoom 2");
    }

    #[test]
    fn a_press_finds_the_port_then_the_node_then_the_edge() {
        let f = flow();
        let reg = Registry::builtin();
        let v = View::default();
        let at = |wx: f32, wy: f32| v.to_screen(RECT, (wx, wy));
        // the centre of the card is the node
        let (cx, cy) = at(40.0 + NODE_W / 2.0 + 240.0, 60.0 + NODE_H / 2.0);
        assert_eq!(hit_test(&f, &reg, &v, RECT, cx, cy), Hit::Node("a".into()));
        // the right edge is its output port, the left its input port
        let (ox, oy) = at(40.0 + 240.0 + NODE_W, 60.0 + NODE_H / 2.0);
        assert_eq!(hit_test(&f, &reg, &v, RECT, ox, oy), Hit::Port(PortRef { node: "a".into(), side: Side::Out, name: "out".into() }));
        let (ix, iy) = at(40.0 + 240.0, 60.0 + NODE_H / 2.0);
        assert_eq!(hit_test(&f, &reg, &v, RECT, ix, iy), Hit::Port(PortRef { node: "a".into(), side: Side::In, name: "in".into() }));
        // the condition has two outputs: true above false
        let (tx, ty) = at(40.0 + 480.0 + NODE_W, 60.0 + NODE_H / 3.0);
        assert_eq!(hit_test(&f, &reg, &v, RECT, tx, ty), Hit::Port(PortRef { node: "c".into(), side: Side::Out, name: "true".into() }));
        let (fx2, fy2) = at(40.0 + 480.0 + NODE_W, 60.0 + NODE_H * 2.0 / 3.0);
        assert_eq!(hit_test(&f, &reg, &v, RECT, fx2, fy2), Hit::Port(PortRef { node: "c".into(), side: Side::Out, name: "false".into() }));
        // the error output of a node that can fail
        let (ex, ey) = at(40.0 + 240.0 + NODE_W - 22.0, 60.0 + NODE_H);
        assert_eq!(hit_test(&f, &reg, &v, RECT, ex, ey), Hit::Port(PortRef { node: "a".into(), side: Side::Out, name: "error".into() }));
        // on the curve between two nodes: the edge
        let pts = edge_points(&f, &reg, &f.edges[0]);
        let mid = pts[pts.len() / 2];
        let (mx, my) = at(mid.0, mid.1);
        assert_eq!(hit_test(&f, &reg, &v, RECT, mx, my), Hit::Edge(0));
        // empty space, and outside the canvas
        assert_eq!(hit_test(&f, &reg, &v, RECT, RECT.x + 5.0, RECT.y + 450.0), Hit::Empty);
        assert_eq!(hit_test(&f, &reg, &v, RECT, 5.0, 5.0), Hit::Empty);
    }

    #[test]
    fn hit_testing_agrees_with_the_geometry_under_random_views() {
        let mut r = Rng(11);
        let reg = Registry::builtin();
        for _ in 0..300 {
            let mut f = flow();
            // scatter the nodes
            for n in f.nodes.iter_mut() {
                n.pos = Pos { x: snap(r.f(-200.0, 900.0)), y: snap(r.f(-100.0, 500.0)) };
            }
            let v = View { pan: (r.f(-200.0, 200.0), r.f(-200.0, 200.0)), zoom: r.f(0.5, 2.0) };
            for n in &f.nodes {
                // the centre of a card, if it is on the canvas and no other card covers it, is that card
                let (cx, cy) = v.to_screen(RECT, (n.pos.x + NODE_W / 2.0, n.pos.y + NODE_H / 2.0));
                if !RECT.contains(cx as i32, cy as i32) {
                    continue;
                }
                let covered = f.nodes.iter().any(|o| o.id != n.id && cx >= v.to_screen(RECT, (o.pos.x, o.pos.y)).0 && cx <= v.to_screen(RECT, (o.pos.x + NODE_W, o.pos.y)).0 && cy >= v.to_screen(RECT, (o.pos.x, o.pos.y)).1 && cy <= v.to_screen(RECT, (o.pos.x, o.pos.y + NODE_H)).1);
                if covered {
                    continue;
                }
                match hit_test(&f, &reg, &v, RECT, cx, cy) {
                    Hit::Node(id) => assert_eq!(id, n.id),
                    other => panic!("centre of {} gave {other:?}", n.id),
                }
            }
            // a point far from everything is empty
            let far = RECT.x + RECT.w - 2.0;
            let hit = hit_test(&f, &reg, &v, RECT, far, RECT.y + RECT.h - 2.0);
            if let Hit::Node(id) = &hit {
                let n = f.node(id).unwrap();
                let (sx, sy) = v.to_screen(RECT, (n.pos.x, n.pos.y));
                assert!(far >= sx && far <= sx + NODE_W * v.zoom && RECT.y + RECT.h - 2.0 >= sy);
            }
        }
    }

    #[test]
    fn dragging_moves_and_snaps_and_undo_is_the_old_flow() {
        let mut f = flow();
        let before = f.clone();
        let ids = vec!["a".to_string(), "c".to_string()];
        move_nodes(&mut f, &ids, 33.0, -17.0, false);
        assert_eq!(f.node("a").unwrap().pos, Pos { x: 280.0 + 33.0, y: 60.0 - 17.0 });
        assert_eq!(f.node("s").unwrap().pos, before.node("s").unwrap().pos, "the others stay");
        let mut g = before.clone();
        move_nodes(&mut g, &ids, 33.0, -17.0, true);
        for n in &g.nodes {
            assert!(n.pos.x % GRID == 0.0 || !ids.contains(&n.id) || (n.pos.x - snap(n.pos.x)).abs() < 1e-3, "{:?}", n.pos);
        }
        assert_eq!(g.node("a").unwrap().pos.x % GRID, 0.0);
        let mut r = Rng(3);
        for _ in 0..200 {
            let mut h = before.clone();
            let (dx, dy) = (r.f(-300.0, 300.0), r.f(-300.0, 300.0));
            move_nodes(&mut h, &ids, dx, dy, true);
            assert!(h.nodes.iter().filter(|n| ids.contains(&n.id)).all(|n| n.pos.x % GRID == 0.0 && n.pos.y % GRID == 0.0));
            move_nodes(&mut h, &ids, -dx, -dy, false);
            // moving back by the same distance lands within one grid step of where it began
            for n in &h.nodes {
                let o = before.node(&n.id).unwrap();
                assert!((n.pos.x - o.pos.x).abs() <= GRID && (n.pos.y - o.pos.y).abs() <= GRID);
            }
            assert_eq!(h.edges, before.edges, "dragging never changes the edges");
        }
    }

    #[test]
    fn connecting_obeys_the_rules_and_never_makes_a_loop() {
        let reg = Registry::builtin();
        let mut f = flow();
        assert!(can_connect(&f, &reg, "a", "out", "a").is_err());
        assert!(can_connect(&f, &reg, "a", "out", "c").unwrap_err().contains("already"));
        assert!(can_connect(&f, &reg, "c", "out", "e").unwrap_err().contains("no `out` output"), "a condition has true and false");
        assert!(can_connect(&f, &reg, "e", "out", "s").is_err(), "an end has no output; nothing joins into a start");
        assert!(can_connect(&f, &reg, "a", "out", "s").unwrap_err().contains("start"));
        assert!(can_connect(&f, &reg, "c", "false", "a").unwrap_err().contains("loop"));
        assert!(can_connect(&f, &reg, "ghost", "out", "a").is_err());
        connect(&mut f, &reg, "c", "false", "e").unwrap();
        assert_eq!(f.edges.len(), 4);
        connect(&mut f, &reg, "a", "error", "e").unwrap();
        // a random walk of connect attempts never produces a cycle, a duplicate or a self edge
        let mut r = Rng(17);
        let mut g = Flow::new("g", "G");
        for i in 0..8 {
            g.nodes.push(Node::new(&format!("n{i}"), if i == 0 { "start" } else { "notify" }, json!({})));
        }
        for _ in 0..400 {
            let (a, b) = (r.next() % 8, r.next() % 8);
            let _ = connect(&mut g, &reg, &format!("n{a}"), "out", &format!("n{b}"));
        }
        assert!(iyakki_flow::validate::topo_order(&g).is_some(), "still acyclic");
        let keys: BTreeSet<String> = g.edges.iter().map(|e| e.key()).collect();
        assert_eq!(keys.len(), g.edges.len());
        assert!(g.edges.iter().all(|e| e.from != e.to && e.to != "n0"));
    }

    #[test]
    fn adding_deleting_copying_and_pasting() {
        let mut f = flow();
        let id = add_node(&mut f, "http", (333.0, 151.0));
        assert_eq!(id, "http");
        assert_eq!(f.node(&id).unwrap().pos, Pos { x: 340.0, y: 160.0 }, "snapped");
        let id2 = add_node(&mut f, "http", (0.0, 0.0));
        assert_eq!(id2, "http2");
        assert_eq!(f.node("http").unwrap().config["method"], "GET");
        let n = f.nodes.len();
        delete_nodes(&mut f, &["a".into()]);
        assert!(f.node("a").is_none() && f.edges.iter().all(|e| e.from != "a" && e.to != "a"));
        assert_eq!(f.nodes.len(), n - 1);
        // copy two joined nodes, paste with an offset: new ids, the edge between them, references renamed
        let mut g = flow();
        g.node_mut("c").unwrap().config = json!({"expr": "nodes.a.output.n > 1"});
        let clip = copy(&g, &["a".into(), "c".into()]);
        assert_eq!((clip.nodes.len(), clip.edges.len()), (2, 1));
        let made = paste(&mut g, &clip, (40.0, 100.0));
        assert_eq!(made.len(), 2);
        assert!(made.iter().all(|m| g.node(m).is_some()) && made[0] != "a" && made[1] != "c");
        assert_eq!(g.nodes.len(), 6);
        let (na, nc) = (&made[0], &made[1]);
        assert!(g.edges.iter().any(|e| &e.from == na && &e.to == nc));
        assert_eq!(g.node(nc).unwrap().config["expr"], format!("nodes.{na}.output.n > 1"));
        assert_eq!(g.node(na).unwrap().pos, Pos { x: 320.0, y: 160.0 });
        assert_eq!(g.node("c").unwrap().config["expr"], "nodes.a.output.n > 1", "the original is untouched");
        // pasting again gives still other ids
        let again = paste(&mut g, &clip, (80.0, 200.0));
        assert!(again.iter().all(|m| !made.contains(m)));
        let ids: BTreeSet<&String> = g.nodes.iter().map(|n| &n.id).collect();
        assert_eq!(ids.len(), g.nodes.len(), "ids stay unique");
        assert!(g.node(&unique_node_id(&g, "a")).is_none());
    }

    #[test]
    fn fit_and_the_minimap_hold_the_whole_flow() {
        let f = flow();
        let v = View::fit(&f, RECT);
        let (x0, y0, x1, y1) = bounds(&f).unwrap();
        for (wx, wy) in [(x0, y0), (x1, y1)] {
            let s = v.to_screen(RECT, (wx, wy));
            assert!(RECT.contains(s.0 as i32, s.1 as i32), "{s:?}");
        }
        assert!(v.zoom <= 1.0);
        let mini = R::new(0.0, 0.0, 160.0, 100.0);
        let m = minimap(&f, &v, RECT, mini);
        for (_, r) in &m.nodes {
            assert!(r.x >= mini.x - 0.5 && r.x + r.w <= mini.x + mini.w + 0.5 && r.y >= mini.y - 0.5 && r.y + r.h <= mini.y + mini.h + 0.5, "{r:?}");
        }
        assert!(m.viewport.x >= mini.x - 0.5 && m.viewport.x + m.viewport.w <= mini.x + mini.w + 0.5);
        // a click in the minimap maps back to a world point inside the extent; centring on it puts it mid-canvas
        let click = (m.nodes[2].1.x + 2.0, m.nodes[2].1.y + 2.0);
        let w = minimap_to_world(&m, click.0, click.1);
        assert!((w.0 - f.nodes[2].pos.x).abs() < 4.0 / m.scale);
        let mut v2 = v;
        center_on(&mut v2, RECT, w);
        let s = v2.to_screen(RECT, w);
        assert!((s.0 - (RECT.x + RECT.w / 2.0)).abs() < 1.0 && (s.1 - (RECT.y + RECT.h / 2.0)).abs() < 1.0);
        assert_eq!(View::fit(&Flow::new("e", "E"), RECT), View::default());
    }

    #[test]
    fn a_rubber_band_takes_the_cards_it_touches() {
        let f = flow();
        let v = View::default();
        let band = R::new(RECT.x + 0.0, RECT.y + 40.0, 300.0, 100.0);
        assert_eq!(nodes_in(&f, &v, RECT, band), vec!["s", "a"]);
        assert!(nodes_in(&f, &v, RECT, R::new(RECT.x + 700.0, RECT.y + 400.0, 50.0, 50.0)).is_empty());
    }
}
