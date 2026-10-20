//! SRE PILOT (demo): an animated picture of how an assistant for site reliability would work, redrawn from a whiteboard
//! sketch. Sources (logs, metrics, traces, tickets) feed a sampler and aggregator, which fills an optimized store of service
//! health; a map, a pilot assist (recommendations, a human decides) and an auto pilot (guarded actions) read the store, and
//! a front end shows it all; the pilots act on the site infrastructure (which is itself a source) and tell people on chat,
//! Teams and the phone.
//!
//! **Everything on this page is sample data.** There are no integrations: nothing is read from, or sent to, a real system,
//! and nothing is stored or sent over the network. "Click To Schedule a Demo" is a dummy button ([`schedule_demo`] is the one
//! place a real flow would go).
//!
//! Drawing is cheap: the nodes and edges are drawn once per size into a cached copy of the canvas; each frame copies that and
//! draws only the particles, pulses and small texts. The page asks for a redraw about 15 times a second, only while it is on
//! screen and not paused. With `IYAKKI_REDUCED_MOTION=1` the picture is still (a few particles, no movement).
//! `W` swaps the drawing for the original whiteboard photo with the same particles flowing along the lines of the sketch.

use std::cell::Cell;

use iyakki_web::image::Bitmap;

use super::{App, AppCx, Flow};
use crate::fx::{self, mix, size, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, MUTED, R, TEXT};

const GREEN: [u8; 4] = rgb(88, 236, 150);
const RED: [u8; 4] = rgb(255, 96, 108);
const INK: [u8; 4] = rgb(6, 14, 30);

/// The whiteboard photo (1280 x 960 JPEG, orientation already applied).
pub static PHOTO_JPEG: &[u8] = include_bytes!("../../assets/sre-whiteboard.jpg");

/// The text shown by the dummy button.
pub const THANKS: &str = "Thanks for your interest! This is a demo button — the scheduling flow is coming soon.";
pub const BUTTON_LABEL: &str = "Click To Schedule a Demo";
pub const FOUNDER_LINE: &str = "One of our founders (Ramesh Veeramalla) will assist you.";
pub const FOOTER: &str = "Demo with sample data — no real systems are touched";

/// What the "Click To Schedule a Demo" button does. For now it only says thanks (no data is collected, nothing is stored,
/// nothing goes over the network); the real scheduling flow replaces this function later.
pub fn schedule_demo() -> &'static str {
    THANKS
}

// ---------------------------------------------------------------------------------------------------- the model

pub const S1: usize = 0;
pub const S2: usize = 1;
pub const SN: usize = 2;
pub const AGG: usize = 3;
pub const STORE: usize = 4;
pub const MAP: usize = 5;
pub const PILOT: usize = 6;
pub const AUTO: usize = 7;
pub const FE: usize = 8;
pub const INFRA: usize = 9;
pub const NOTIF: usize = 10;
pub const NODES: usize = 11;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Data,
    Action,
    Notify,
    Feedback,
}

impl Tone {
    fn color(self) -> [u8; 4] {
        match self {
            Tone::Data => CYAN,
            Tone::Action => AMBER,
            Tone::Notify => MAGENTA,
            Tone::Feedback => GREEN,
        }
    }
}

/// (from, to, kind, speed in line lengths per second)
pub const EDGES: [(usize, usize, Tone, f32); 15] = [
    (S1, AGG, Tone::Data, 0.22),
    (S2, AGG, Tone::Data, 0.30),
    (SN, AGG, Tone::Data, 0.18),
    (AGG, STORE, Tone::Data, 0.26),
    (STORE, MAP, Tone::Data, 0.20),
    (STORE, PILOT, Tone::Data, 0.24),
    (STORE, AUTO, Tone::Data, 0.17),
    (STORE, FE, Tone::Data, 0.15),
    (MAP, FE, Tone::Data, 0.28),
    (PILOT, FE, Tone::Data, 0.21),
    (PILOT, INFRA, Tone::Action, 0.19),
    (AUTO, INFRA, Tone::Action, 0.25),
    (PILOT, NOTIF, Tone::Notify, 0.16),
    (AUTO, NOTIF, Tone::Notify, 0.22),
    (INFRA, SN, Tone::Feedback, 0.14),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Glyph {
    Logs,
    Metrics,
    Traces,
    Funnel,
    Cylinder,
    Pin,
    Radar,
    Loop,
    Monitor,
    Racks,
    Bell,
}

struct Info {
    title: &'static str,
    sub: &'static str,
    kind: &'static str,
    glyph: Glyph,
    accent: [u8; 4],
    role: &'static str,
    logs: [&'static str; 3],
}

const INFO: [Info; NODES] = [
    Info { title: "Source 1", sub: "Logs", kind: "SOURCE", glyph: Glyph::Logs, accent: CYAN, role: "Application and system logs from every service. One of many pluggable sources; each one is sampled, never copied in full.", logs: ["INFO  checkout order=8123 ok 112ms", "WARN  cart cache miss ratio 0.41", "INFO  gateway 200 GET /api/items"] },
    Info { title: "Source 2", sub: "Metrics", kind: "SOURCE", glyph: Glyph::Metrics, accent: CYAN, role: "Time series: latency, error rate, saturation, queue depth. The cheapest early warning there is.", logs: ["p95 checkout 310ms", "errors 0.4% (limit 1%)", "cpu cart-7 63%"] },
    Info { title: "Source N", sub: "Traces · tickets", kind: "SOURCE", glyph: Glyph::Traces, accent: CYAN, role: "Traces, tickets and anything else that says how the site is doing. The monitored infrastructure is a source too: what the pilots change comes back here.", logs: ["trace 9f2c checkout>payment 480ms", "ticket OPS-412 opened by support", "deploy web-14 finished"] },
    Info { title: "Aggregator", sub: "Sampler & aggregator", kind: "PIPELINE", glyph: Glyph::Funnel, accent: rgb(120, 200, 255), role: "Samples the raw streams, drops duplicates and rolls them up, so the store keeps what matters instead of everything.", logs: ["sampled 1 in 20 (info)", "kept 100% of errors", "rolled up 5s -> 1m windows"] },
    Info { title: "Optimized store", sub: "health", kind: "STATE", glyph: Glyph::Cylinder, accent: rgb(140, 160, 255), role: "A compact store of service health: current state, short history and dependencies. Everything downstream reads from it, so each question is cheap.", logs: ["checkout green 0.93", "cart green 0.97", "payments amber 0.71"] },
    Info { title: "Map", sub: "Service topology", kind: "VIEW", glyph: Glyph::Pin, accent: rgb(110, 224, 160), role: "The service map: which service calls which, coloured by health, so a failure is seen where it starts and where it spreads.", logs: ["checkout -> payments", "checkout -> cart", "payments -> bank-gw"] },
    Info { title: "Pilot Assist", sub: "Recommendations", kind: "ASSIST", glyph: Glyph::Radar, accent: AMBER, role: "Reads the health and recommends what to do, with the evidence. A person decides; the pilot only advises (human in the loop).", logs: ["recommend: restart pod checkout-7f9c", "reason: p95 > 2s for 3 min", "confidence 0.82 (needs approval)"] },
    Info { title: "Auto Pilot", sub: "Guarded actions", kind: "AUTOPILOT", glyph: Glyph::Loop, accent: rgb(255, 150, 110), role: "Carries out pre-approved runbooks by itself, inside guard rails: limits, dry runs, rollback and a full audit trail.", logs: ["runbook scale-out allowed (max 8)", "scaled replicas 3 -> 5", "rollback armed for 10 min"] },
    Info { title: "Front end", sub: "Dashboard", kind: "VIEW", glyph: Glyph::Monitor, accent: rgb(110, 190, 255), role: "The dashboard people look at: health, the map, the pilots' advice and what the auto pilot did.", logs: ["12 viewers online", "dashboard refresh 1s", "alert banner: none"] },
    Info { title: "Site Infra", sub: "Monitored infrastructure", kind: "TARGET", glyph: Glyph::Racks, accent: rgb(176, 226, 92), role: "The servers, pods and services being looked after. The pilots act on it, and it feeds the sources again (feedback).", logs: ["pods checkout x3 ready", "node-4 cpu 58%", "deploy web-14 ok"] },
    Info { title: "Notifications", sub: "Chat · Teams · Phone", kind: "PEOPLE", glyph: Glyph::Bell, accent: MAGENTA, role: "Tells the right people where they already are: Google Chat, Teams, ..., and a phone call for the serious ones.", logs: ["Chat #sre-oncall: sent", "Teams: sent", "Phone: on-call paged"] },
];

fn metrics(i: usize, t: f32) -> Vec<(&'static str, String)> {
    let w = |a: f32, b: f32| a + (t * 0.7 + i as f32 * 1.9).sin() * b;
    match i {
        S1 => vec![("lines / s", format!("{:.0}", w(1800.0, 120.0))), ("errors", format!("{:.1}%", w(0.4, 0.1))), ("hosts", "42".into())],
        S2 => vec![("series", format!("{:.0}", w(9400.0, 80.0))), ("p95 latency", format!("{:.0} ms", w(310.0, 25.0))), ("scrape", "15 s".into())],
        SN => vec![("spans / s", format!("{:.0}", w(620.0, 60.0))), ("open tickets", "7".into()), ("feedback", "from infra".into())],
        AGG => vec![("in", format!("{:.1}k/s", w(12.4, 0.6))), ("kept", format!("{:.0}%", w(6.0, 0.5))), ("lag", format!("{:.0} ms", w(240.0, 30.0)))],
        STORE => vec![("services", "38".into()), ("health", format!("{:.0}%", health(t) * 100.0)), ("history", "6 h".into())],
        MAP => vec![("services", "38".into()), ("edges", "112".into()), ("degraded", if health(t) < 0.7 { "1" } else { "0" }.into())],
        PILOT => vec![("advice today", "5".into()), ("accepted", "4".into()), ("waiting", if cycle(t) > 4.2 && cycle(t) < 10.5 { "1" } else { "0" }.into())],
        AUTO => vec![("runs today", "3".into()), ("rolled back", "0".into()), ("guard rails", "on".into())],
        FE => vec![("viewers", "12".into()), ("refresh", "1 s".into()), ("alerts", if health(t) < 0.7 { "1" } else { "0" }.into())],
        INFRA => vec![("pods", if cycle(t) > 6.8 && cycle(t) < 12.0 { "5/5" } else { "3/3" }.into()), ("cpu", format!("{:.0}%", w(58.0, 6.0))), ("nodes", "6".into())],
        _ => vec![("sent today", "9".into()), ("channels", "3".into()), ("on-call", "ready".into())],
    }
}

// ---------------------------------------------------------------------------------------------------- the story

pub const CYCLE: f32 = 13.0;

fn cycle(t: f32) -> f32 {
    t.rem_euclid(CYCLE)
}

/// Health of the checkout service, 0 to 1: a quiet wobble, and in the middle of each cycle the incident dips it into red
/// and back through amber.
pub fn health(t: f32) -> f32 {
    let u = cycle(t);
    let dip = if u < 2.0 {
        0.0
    } else if u < 4.0 {
        (u - 2.0) / 2.0
    } else if u < 6.0 {
        1.0
    } else if u < 9.8 {
        1.0 - (u - 6.0) / 3.8
    } else {
        0.0
    };
    (0.90 + 0.04 * (t * 1.3).sin() + 0.025 * (t * 3.1).sin() - 0.64 * dip).clamp(0.05, 1.0)
}

fn health_color(h: f32) -> [u8; 4] {
    if h >= 0.75 {
        GREEN
    } else if h >= 0.5 {
        AMBER
    } else {
        RED
    }
}

/// The moving "events": (edge, from, to, colour) in cycle seconds.
const RUNS: [(usize, f32, f32, [u8; 4]); 8] = [
    (1, 0.5, 1.8, RED),
    (3, 1.8, 3.0, RED),
    (5, 3.0, 4.2, RED),
    (6, 3.0, 4.2, RED),
    (12, 4.8, 6.0, MAGENTA),
    (11, 5.4, 6.8, AMBER),
    (13, 6.8, 8.2, MAGENTA),
    (14, 8.2, 9.6, GREEN),
];

/// A node lights up when something arrives: (node, cycle second, colour).
const ARRIVALS: [(usize, f32, [u8; 4]); 9] = [(S2, 0.3, RED), (AGG, 1.8, RED), (STORE, 3.0, RED), (PILOT, 4.2, RED), (AUTO, 4.2, RED), (INFRA, 6.8, AMBER), (NOTIF, 8.2, MAGENTA), (SN, 9.6, GREEN), (FE, 3.4, RED)];

fn flash(node: usize, u: f32) -> Option<(f32, [u8; 4])> {
    ARRIVALS.iter().filter(|a| a.0 == node && u >= a.1 && u < a.1 + 1.0).map(|a| (1.0 - (u - a.1), a.2)).next()
}

fn pilot_text(u: f32) -> Option<&'static str> {
    (4.2..10.5).contains(&u).then_some("Recommend: restart pod checkout-7f9c")
}
fn auto_text(u: f32) -> Option<&'static str> {
    (5.4..11.0).contains(&u).then_some("scaled replicas 3 → 5")
}
fn notify_text(u: f32) -> Option<&'static str> {
    (8.2..12.0).contains(&u).then_some("#sre-oncall: checkout slow, scaled 3 → 5")
}

/// The lines of the live event log at time `t`, oldest first, with their colours.
pub fn event_lines(t: f32) -> Vec<(String, [u8; 4])> {
    let u = cycle(t);
    let mut v = Vec::new();
    if u >= 0.5 {
        v.push(("Source 2: checkout p95 latency 2.4 s".to_string(), RED));
    }
    if u >= 3.0 {
        v.push(("Store: checkout health fell to 31%".to_string(), RED));
    }
    if let Some(p) = pilot_text(u) {
        v.push((format!("Pilot Assist: {p}"), AMBER));
    }
    if let Some(a) = auto_text(u) {
        v.push((format!("Auto Pilot: {a}"), GREEN));
    }
    if let Some(n) = notify_text(u) {
        v.push((format!("Notify: {n}"), MAGENTA));
    }
    if v.is_empty() || (u >= 12.0 && v.len() < 2) {
        v = vec![(format!("All quiet: checkout health {:.0}%", health(t) * 100.0), MUTED)];
    }
    let n = v.len();
    v.split_off(n.saturating_sub(4))
}

// ---------------------------------------------------------------------------------------------------- geometry

#[derive(Clone, Debug)]
pub struct Path {
    pub pts: Vec<(f32, f32)>,
    cum: Vec<f32>,
}

impl Path {
    pub fn new(pts: Vec<(f32, f32)>) -> Path {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
            cum.push(cum.last().unwrap() + d);
        }
        Path { pts, cum }
    }

    pub fn len(&self) -> f32 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// The point at fraction `f` (0..1) of the length, and the direction there.
    pub fn at(&self, f: f32) -> (f32, f32, (f32, f32)) {
        let total = self.len().max(0.001);
        let d = f.clamp(0.0, 1.0) * total;
        let i = self.cum.partition_point(|c| *c < d).clamp(1, self.pts.len().saturating_sub(1).max(1));
        if self.pts.len() < 2 {
            let p = self.pts.first().copied().unwrap_or((0.0, 0.0));
            return (p.0, p.1, (1.0, 0.0));
        }
        let (a, b) = (self.pts[i - 1], self.pts[i]);
        let seg = (self.cum[i] - self.cum[i - 1]).max(0.001);
        let k = ((d - self.cum[i - 1]) / seg).clamp(0.0, 1.0);
        let dir = ((b.0 - a.0) / seg, (b.1 - a.1) / seg);
        (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k, dir)
    }
}

fn bezier(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), n: usize) -> Vec<(f32, f32)> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let m = 1.0 - t;
            let f = |a: f32, b: f32, c: f32, d: f32| m * m * m * a + 3.0 * m * m * t * b + 3.0 * m * t * t * c + t * t * t * d;
            (f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1))
        })
        .collect()
}

/// A curve leaving `a` to the right and arriving at `b` from the left.
fn curve(a: (f32, f32), b: (f32, f32)) -> Vec<(f32, f32)> {
    let dx = ((b.0 - a.0) * 0.5).max(10.0);
    bezier(a, (a.0 + dx, a.1), (b.0 - dx, b.1), b, 24)
}

/// Where everything is drawn for one size.
pub struct Layout {
    pub canvas: R,
    pub nodes: [R; NODES],
    pub paths: Vec<Path>,
    pub vertical: bool,
    /// The photo's rectangle in whiteboard mode.
    pub photo: Option<R>,
}

/// Canvases narrower than this (pixels) stack the nodes in one column.
pub const MIN_WIDE: f32 = 480.0;

pub fn layout_clean(c: R, s: f32) -> Layout {
    if c.w < MIN_WIDE {
        return layout_stack(c, s);
    }
    let mut gap = (c.w * 0.045).max(14.0);
    let mut cw = (c.w - 4.0 * gap) / 5.0;
    let mut x0 = c.x;
    if cw > 230.0 * s {
        cw = 230.0 * s;
        gap = (c.w - 5.0 * cw) / 4.0;
    }
    if gap < 0.0 {
        gap = 0.0;
    }
    x0 += 0.0;
    let lt = (c.h * 0.05).max(8.0);
    let lb = (c.h * 0.07).max(10.0);
    let rp = (c.h - lt - lb) / 3.0;
    let nh = (rp * 0.76).min(132.0 * s);
    let colx = |i: usize| x0 + i as f32 * (cw + gap);
    let rowc = |r: usize| c.y + lt + rp * (r as f32 + 0.5);
    let node = |col: usize, row: usize, h: f32| R::new(colx(col), rowc(row) - h / 2.0, cw, h);
    let mut n = [R::new(0.0, 0.0, 0.0, 0.0); NODES];
    n[S1] = node(0, 0, nh);
    n[S2] = node(0, 1, nh);
    n[SN] = node(0, 2, nh);
    n[AGG] = node(1, 1, nh);
    n[STORE] = node(2, 1, rp * 2.2);
    n[MAP] = node(3, 0, nh);
    n[PILOT] = node(3, 1, nh);
    n[AUTO] = node(3, 2, nh);
    n[FE] = node(4, 0, nh);
    n[NOTIF] = node(4, 1, (rp * 0.95).min(nh * 1.25));
    n[INFRA] = node(4, 2, nh);
    let right = |r: R, f: f32| (r.x + r.w, r.y + r.h * f);
    let left = |r: R, f: f32| (r.x, r.y + r.h * f);
    let mut p: Vec<Vec<(f32, f32)>> = Vec::new();
    let st = |f: f32| (n[STORE].x + n[STORE].w, n[STORE].y + n[STORE].h * f);
    // sources to the aggregator
    p.push(curve(right(n[S1], 0.5), left(n[AGG], 0.22)));
    p.push(curve(right(n[S2], 0.5), left(n[AGG], 0.5)));
    p.push(curve(right(n[SN], 0.5), left(n[AGG], 0.78)));
    p.push(curve(right(n[AGG], 0.5), left(n[STORE], 0.5)));
    // the store to its consumers: ports on its right edge at the height of each target
    let at = |r: R| ((r.y + r.h * 0.5 - n[STORE].y) / n[STORE].h).clamp(0.05, 0.95);
    p.push(curve(st(at(n[MAP])), left(n[MAP], 0.5)));
    p.push(curve(st(at(n[PILOT])), left(n[PILOT], 0.5)));
    p.push(curve(st(at(n[AUTO])), left(n[AUTO], 0.5)));
    // the store to the front end over the top lane
    let lane = c.y + lt * 0.45;
    let (sx, fx) = (n[STORE].x + n[STORE].w * 0.5, n[FE].x + n[FE].w * 0.5);
    p.push(vec![(sx, n[STORE].y), (sx, lane), (fx, lane), (fx, n[FE].y)]);
    p.push(curve(right(n[MAP], 0.5), left(n[FE], 0.5)));
    p.push(curve(right(n[PILOT], 0.25), left(n[FE], 0.8)));
    // actions and notifications
    p.push(curve(right(n[PILOT], 0.75), left(n[INFRA], 0.3)));
    p.push(curve(right(n[AUTO], 0.35), left(n[INFRA], 0.7)));
    p.push(curve(right(n[PILOT], 0.5), left(n[NOTIF], 0.5)));
    p.push(curve(right(n[AUTO], 0.2), left(n[NOTIF], 0.8)));
    // feedback along the bottom lane
    let lane = c.y + c.h - lb * 0.45;
    let (ix, sx) = (n[INFRA].x + n[INFRA].w * 0.5, n[SN].x + n[SN].w * 0.5);
    p.push(vec![(ix, n[INFRA].y + n[INFRA].h), (ix, lane), (sx, lane), (sx, n[SN].y + n[SN].h)]);
    Layout { canvas: c, nodes: n, paths: p.into_iter().map(Path::new).collect(), vertical: false, photo: None }
}

fn layout_stack(c: R, s: f32) -> Layout {
    let vg = 4.0 * s;
    let nh = ((c.h - vg * (NODES as f32 - 1.0)) / NODES as f32).max(10.0);
    let bus = (c.w * 0.22).min(60.0);
    let mut n = [R::new(0.0, 0.0, 0.0, 0.0); NODES];
    for (i, r) in n.iter_mut().enumerate() {
        *r = R::new(c.x + bus, c.y + i as f32 * (nh + vg), c.w - bus, nh);
    }
    let paths = EDGES
        .iter()
        .enumerate()
        .map(|(k, e)| {
            let lane = c.x + 3.0 + (bus - 8.0) * k as f32 / (EDGES.len() - 1) as f32;
            let (a, b) = (n[e.0], n[e.1]);
            Path::new(vec![(a.x, a.y + a.h * 0.5), (lane, a.y + a.h * 0.5), (lane, b.y + b.h * 0.5), (b.x - 1.0, b.y + b.h * 0.5)])
        })
        .collect();
    Layout { canvas: c, nodes: n, paths, vertical: true, photo: None }
}

/// The whiteboard photo is 2000 x 1500 in the coordinates below (the picture the sketch was traced on).
pub const PHOTO_W: f32 = 2000.0;
pub const PHOTO_H: f32 = 1500.0;

/// Boxes round each part of the sketch (x0, y0, x1, y1).
const PHOTO_BOXES: [(f32, f32, f32, f32); NODES] = [
    (1310.0, 170.0, 1660.0, 290.0),
    (1330.0, 325.0, 1610.0, 415.0),
    (1320.0, 585.0, 1665.0, 680.0),
    (900.0, 350.0, 1090.0, 480.0),
    (565.0, 470.0, 805.0, 730.0),
    (650.0, 860.0, 780.0, 925.0),
    (720.0, 935.0, 990.0, 1000.0),
    (850.0, 1015.0, 1140.0, 1110.0),
    (265.0, 810.0, 445.0, 950.0),
    (1225.0, 725.0, 1475.0, 830.0),
    (1250.0, 880.0, 1525.0, 1285.0),
];

type Pts = &'static [(f32, f32)];
/// The lines of the sketch, in the same order as [`EDGES`], pointing the way the data flows.
const PHOTO_PATHS: [Pts; 15] = [
    &[(1310.0, 255.0), (1100.0, 400.0)],
    &[(1335.0, 395.0), (1100.0, 405.0)],
    &[(1340.0, 605.0), (1100.0, 420.0)],
    &[(900.0, 425.0), (725.0, 550.0)],
    &[(655.0, 760.0), (670.0, 800.0), (675.0, 880.0)],
    &[(735.0, 760.0), (750.0, 850.0), (755.0, 940.0)],
    &[(780.0, 760.0), (835.0, 880.0), (875.0, 1025.0)],
    &[(560.0, 740.0), (455.0, 845.0)],
    &[(662.0, 912.0), (640.0, 917.0), (455.0, 845.0)],
    &[(727.0, 975.0), (690.0, 970.0), (645.0, 925.0), (455.0, 845.0)],
    &[(782.0, 962.0), (835.0, 880.0), (1230.0, 775.0)],
    &[(875.0, 1025.0), (835.0, 880.0), (1230.0, 775.0)],
    &[(782.0, 962.0), (835.0, 880.0), (1252.0, 915.0)],
    &[(875.0, 1025.0), (835.0, 880.0), (1252.0, 915.0)],
    &[(1380.0, 722.0), (1385.0, 688.0)],
];

pub fn layout_photo(c: R) -> Layout {
    let k = (c.w / PHOTO_W).min(c.h / PHOTO_H);
    let (pw, ph) = (PHOTO_W * k, PHOTO_H * k);
    let photo = R::new(c.x + (c.w - pw) / 2.0, c.y + (c.h - ph) / 2.0, pw, ph);
    let map = |x: f32, y: f32| (photo.x + x * k, photo.y + y * k);
    let mut n = [R::new(0.0, 0.0, 0.0, 0.0); NODES];
    for (i, b) in PHOTO_BOXES.iter().enumerate() {
        let (x, y) = map(b.0, b.1);
        n[i] = R::new(x, y, (b.2 - b.0) * k, (b.3 - b.1) * k);
    }
    let paths = PHOTO_PATHS.iter().map(|p| Path::new(p.iter().map(|q| map(q.0, q.1)).collect())).collect();
    Layout { canvas: c, nodes: n, paths, vertical: false, photo: Some(photo) }
}

/// The page: where the strips, the diagram, the side panel and the button go.
pub struct Geom {
    pub top: R,
    pub canvas: R,
    pub panel: Option<R>,
    /// The one-line info row under the diagram (used when there is no side panel).
    pub info: R,
    pub legend: R,
    pub foot: R,
    pub button: R,
    pub founder: R,
}

pub fn geom(area: R, s: f32, label_w: f32) -> Geom {
    let m = (24.0 * s).max(10.0);
    let x = area.x + m;
    let w = area.w - 2.0 * m;
    let top = R::new(x, area.y + 8.0 * s, w, 26.0 * s);
    let founder = R::new(x, area.y + area.h - 8.0 * s - 16.0 * s, w, 16.0 * s);
    let bh = 34.0 * s;
    let bw = (label_w + 64.0 * s).min(w);
    let button = R::new(area.x + (area.w - bw) / 2.0, founder.y - 4.0 * s - bh, bw, bh);
    let foot = R::new(x, button.y - 6.0 * s - 15.0 * s, w, 15.0 * s);
    let legend = R::new(x, foot.y - 4.0 * s - 16.0 * s, w, 16.0 * s);
    let wide = area.w >= 1000.0;
    let info = if wide { R::new(x, legend.y, 0.0, 0.0) } else { R::new(x, legend.y - 3.0 * s - 16.0 * s, w, 16.0 * s) };
    let ctop = top.y + top.h + 8.0 * s;
    let cbottom = if wide { legend.y - 6.0 * s } else { info.y - 6.0 * s };
    let ch = (cbottom - ctop).max(40.0);
    if wide {
        let pw = 300.0 * s;
        let panel = R::new(x + w - pw, ctop, pw, ch);
        Geom { top, canvas: R::new(x, ctop, w - pw - 14.0 * s, ch), panel: Some(panel), info, legend, foot, button, founder }
    } else {
        Geom { top, canvas: R::new(x, ctop, w, ch), panel: None, info, legend, foot, button, founder }
    }
}

// ---------------------------------------------------------------------------------------------------- the app

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    None,
    Node(usize),
    Button,
}

#[derive(PartialEq, Eq)]
struct CacheKey {
    canvas: (i32, i32, i32, i32),
    scale: i32,
    whiteboard: bool,
    vertical: bool,
}

struct Cache {
    key: CacheKey,
    rows: Vec<u8>,
}

pub struct SrePilotApp {
    /// The animation clock in seconds (frozen while paused or with reduced motion).
    t: f32,
    paused: bool,
    speed: f32,
    whiteboard: bool,
    focus: Focus,
    hover: Cell<Option<usize>>,
    /// Ticks left that the dummy button's thanks stays on screen.
    toast: u32,
    dirty: bool,
    tick_n: u64,
    photo: Option<Bitmap>,
    cache: Option<Cache>,
    last: Option<(Layout, Geom)>,
    cache_builds: u32,
}

impl Default for SrePilotApp {
    fn default() -> Self {
        SrePilotApp::new()
    }
}

/// The still moment shown with reduced motion: in the middle of the incident, so every part has something to say.
const STILL_T: f32 = 7.4;

impl SrePilotApp {
    pub fn new() -> SrePilotApp {
        SrePilotApp { t: 1.0, paused: false, speed: 1.0, whiteboard: false, focus: Focus::None, hover: Cell::new(None), toast: 0, dirty: true, tick_n: 0, photo: None, cache: None, last: None, cache_builds: 0 }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn is_whiteboard(&self) -> bool {
        self.whiteboard
    }
    pub fn selected(&self) -> Option<usize> {
        if let Focus::Node(i) = self.focus {
            Some(i)
        } else {
            None
        }
    }
    pub fn button_focused(&self) -> bool {
        self.focus == Focus::Button
    }
    pub fn toast_text(&self) -> Option<&'static str> {
        (self.toast > 0).then_some(THANKS)
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
    pub fn clock(&self) -> f32 {
        self.t
    }
    /// How many times the static layer has been drawn (tests: it is cached per size).
    pub fn cache_builds(&self) -> u32 {
        self.cache_builds
    }
    /// The canvas rectangle of the last frame (tests compare only this part between frames).
    pub fn canvas_rect(&self) -> Option<R> {
        self.last.as_ref().map(|l| l.1.canvas)
    }
    pub fn layout(&self) -> Option<&Layout> {
        self.last.as_ref().map(|l| &l.0)
    }
    pub fn geom(&self) -> Option<&Geom> {
        self.last.as_ref().map(|l| &l.1)
    }

    fn animating(&self) -> bool {
        !self.paused && !fx::reduced_motion()
    }

    fn now(&self) -> f32 {
        if fx::reduced_motion() {
            STILL_T
        } else {
            self.t
        }
    }

    fn step_focus(&mut self, back: bool) {
        // nodes 0..10, then the button, then nothing
        let order = |f: Focus| match f {
            Focus::None => 0usize,
            Focus::Node(i) => i + 1,
            Focus::Button => NODES + 1,
        };
        let total = NODES + 2;
        let cur = order(self.focus);
        let next = if back { (cur + total - 1) % total } else { (cur + 1) % total };
        self.focus = match next {
            0 => Focus::None,
            n if n <= NODES => Focus::Node(n - 1),
            _ => Focus::Button,
        };
    }

    fn press_button(&mut self) {
        let _ = schedule_demo();
        self.toast = 150;
        self.dirty = true;
    }

    fn load_photo(&mut self) {
        if self.photo.is_none() {
            self.photo = iyakki_web::image::decode(PHOTO_JPEG).ok();
        }
    }

    pub fn photo_size(&mut self) -> Option<(u32, u32)> {
        self.load_photo();
        self.photo.as_ref().map(|p| (p.width, p.height))
    }

    fn set_whiteboard(&mut self, on: bool) {
        self.whiteboard = on;
        if on {
            self.load_photo();
        }
        self.cache = None;
    }
}

impl App for SrePilotApp {
    fn title(&self) -> &'static str {
        "SRE PILOT"
    }

    fn icon(&self) -> Icon {
        Icon::SrePilot
    }

    fn accent(&self) -> [u8; 4] {
        rgb(60, 235, 200)
    }

    fn hint(&self) -> String {
        "Space pause · 1-3 speed · W whiteboard · Tab select · S schedule a demo · Esc back".into()
    }

    fn help(&self) -> Vec<String> {
        vec![
            "A demo with sample data: nothing here is connected to a real system.".into(),
            "Click a part of the picture, or Tab / arrow keys · see what it does and some sample figures.".into(),
            "Space · pause or resume the animation.   1 2 3 · slow, normal, fast.".into(),
            "W · swap the drawing for the original whiteboard photo, with the same flows over it.".into(),
            "S, or Tab to the button and Enter · \"Click To Schedule a Demo\" (a dummy button for now).".into(),
            "Esc · clear the selection, then go back.   IYAKKI_REDUCED_MOTION=1 · a still picture.".into(),
        ]
    }

    fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        match ev {
            Event::Tick => {
                self.tick_n += 1;
                if self.toast > 0 {
                    self.toast -= 1;
                    if self.toast == 0 {
                        self.dirty = true;
                    }
                }
                if self.animating() {
                    self.t += self.speed / 30.0;
                    if self.tick_n % 2 == 0 {
                        self.dirty = true;
                    }
                } else if self.toast > 0 && self.tick_n % 2 == 0 {
                    self.dirty = true;
                }
            }
            Event::Tab(1) => self.set_whiteboard(false),
            Event::Tab(2) => self.set_whiteboard(true),
            Event::Tab(3) => self.paused = !self.paused,
            Event::Tab(n) if (100..100 + NODES).contains(n) => self.focus = Focus::Node(n - 100),
            Event::Tab(200) => self.press_button(),
            Event::Key(k) => match k {
                Key::Esc => {
                    if self.toast > 0 {
                        self.toast = 0;
                    } else if self.focus != Focus::None {
                        self.focus = Focus::None;
                    } else {
                        return Flow::Back;
                    }
                }
                Key::Char(' ') => self.paused = !self.paused,
                Key::Char('1') => self.speed = 0.5,
                Key::Char('2') => self.speed = 1.0,
                Key::Char('3') => self.speed = 2.0,
                Key::Char('w') | Key::Char('W') => self.set_whiteboard(!self.whiteboard),
                Key::Char('s') | Key::Char('S') => self.press_button(),
                Key::Tab | Key::Right | Key::Down => self.step_focus(false),
                Key::BackTab | Key::Left | Key::Up => self.step_focus(true),
                Key::Enter if self.focus == Focus::Button => self.press_button(),
                _ => {}
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        let _ = tick;
        let label_w = g.text_width(15.0 * s, BUTTON_LABEL, Face::Bold);
        let gm = geom(area, s, label_w);
        let lay = if self.whiteboard { layout_photo(gm.canvas) } else { layout_clean(gm.canvas, s) };
        self.draw_top(g, &gm, s);
        self.draw_canvas(g, &lay, s);
        self.draw_nodes_hits(g, &lay);
        let shown = self.hover.get().or(self.selected());
        if let Some(p) = gm.panel {
            self.draw_panel(g, p, s, shown);
        } else {
            self.draw_info(g, gm.info, s, shown);
        }
        self.draw_legend(g, &gm, s);
        let fw = g.text_width(12.0 * s, FOOTER, Face::Sans);
        g.text(gm.foot.x + (gm.foot.w - fw) / 2.0, gm.foot.y, 12.0 * s, rgb(236, 190, 110), FOOTER, Face::Sans);
        self.draw_button(g, &gm, s);
        let fw = g.text_width(13.0 * s, FOUNDER_LINE, Face::Sans);
        g.text(gm.founder.x + (gm.founder.w - fw) / 2.0, gm.founder.y, 13.0 * s, MUTED, FOUNDER_LINE, Face::Sans);
        if self.toast > 0 {
            fx::toast(g, area.x + area.w, area.y + area.h, s, THANKS, true);
        }
        self.last = Some((lay, gm));
    }
}

// ---------------------------------------------------------------------------------------------------- drawing

fn poly(g: &mut Gfx, pts: &[(f32, f32)], w: f32, c: [u8; 4]) {
    for p in pts.windows(2) {
        g.line(p[0].0, p[0].1, p[1].0, p[1].1, w, c);
    }
}

fn arrow_head(g: &mut Gfx, tip: (f32, f32), dir: (f32, f32), len: f32, c: [u8; 4]) {
    let n = (-dir.1, dir.0);
    let b = (tip.0 - dir.0 * len, tip.1 - dir.1 * len);
    g.polygon(&[tip, (b.0 + n.0 * len * 0.5, b.1 + n.1 * len * 0.5), (b.0 - n.0 * len * 0.5, b.1 - n.1 * len * 0.5)], c);
}

fn particle(g: &mut Gfx, x: f32, y: f32, c: [u8; 4], r: f32, strong: bool) {
    g.circle(x, y, r * 3.2, with_alpha(c, if strong { 40 } else { 26 }));
    g.circle(x, y, r * 1.8, with_alpha(c, if strong { 100 } else { 70 }));
    g.circle(x, y, r, with_alpha(c, 255));
    g.circle(x, y, r * 0.45, [255, 255, 255, 230]);
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    (0..=20).map(|i| {
        let a = (from + (to - from) * i as f32 / 20.0).to_radians();
        (cx + rx * a.cos(), cy + ry * a.sin())
    }).collect()
}

fn glyph(g: &mut Gfx, k: Glyph, cx: f32, cy: f32, sz: f32, c: [u8; 4]) {
    let w = (sz * 0.085).max(1.3);
    let q = sz * 0.5;
    match k {
        Glyph::Logs => {
            for (i, f) in [0.9f32, 0.6, 0.8, 0.45].iter().enumerate() {
                let y = cy - q * 0.75 + i as f32 * q * 0.5;
                g.line(cx - q * 0.8, y, cx - q * 0.8 + 2.0 * q * 0.8 * f, y, w, c);
            }
        }
        Glyph::Metrics => {
            for (i, h) in [0.45f32, 0.9, 0.65, 1.2].iter().enumerate() {
                let x = cx - q * 0.75 + i as f32 * q * 0.5;
                g.rect(x - w, cy + q * 0.7 - q * h, 2.0 * w, q * h, c);
            }
            g.line(cx - q * 0.9, cy + q * 0.75, cx + q * 0.9, cy + q * 0.75, w, c);
        }
        Glyph::Traces => {
            let pts = [(cx - q * 0.7, cy + q * 0.5), (cx, cy - q * 0.5), (cx + q * 0.7, cy + q * 0.3)];
            poly(g, &pts, w, c);
            for p in pts {
                g.circle(p.0, p.1, w * 1.7, c);
            }
        }
        Glyph::Funnel => {
            g.polygon(&[(cx - q * 0.85, cy - q * 0.7), (cx + q * 0.85, cy - q * 0.7), (cx + q * 0.15, cy + q * 0.1), (cx + q * 0.15, cy + q * 0.75), (cx - q * 0.15, cy + q * 0.55), (cx - q * 0.15, cy + q * 0.1)], c);
        }
        Glyph::Cylinder => {
            let (rx, ry, h) = (q * 0.7, q * 0.28, q * 1.1);
            let top = cy - h / 2.0;
            poly(g, &ellipse(cx, top, rx, ry, 0.0, 360.0), w, c);
            poly(g, &ellipse(cx, top + h * 0.5, rx, ry, 0.0, 180.0), w, c);
            poly(g, &ellipse(cx, top + h, rx, ry, 0.0, 180.0), w, c);
            g.line(cx - rx, top, cx - rx, top + h, w, c);
            g.line(cx + rx, top, cx + rx, top + h, w, c);
        }
        Glyph::Pin => {
            g.ring(cx, cy - q * 0.2, q * 0.5, w, c);
            g.polygon(&[(cx - q * 0.45, cy + q * 0.05), (cx + q * 0.45, cy + q * 0.05), (cx, cy + q * 0.85)], c);
            g.circle(cx, cy - q * 0.2, q * 0.17, c);
        }
        Glyph::Radar => g.icon(Icon::SrePilot, cx, cy, sz * 1.05, c),
        Glyph::Loop => {
            let pts = ellipse(cx, cy, q * 0.7, q * 0.7, 40.0, 330.0);
            poly(g, &pts, w, c);
            let e = pts[pts.len() - 1];
            arrow_head(g, (e.0 + q * 0.1, e.1 - q * 0.3), (0.5, -0.85), q * 0.55, c);
            g.circle(cx, cy, q * 0.16, c);
        }
        Glyph::Monitor => {
            g.rrect_outline(cx - q * 0.9, cy - q * 0.7, q * 1.8, q * 1.15, q * 0.12, w, c);
            poly(g, &[(cx - q * 0.6, cy + q * 0.2), (cx - q * 0.2, cy - q * 0.15), (cx + q * 0.1, cy + q * 0.05), (cx + q * 0.6, cy - q * 0.35)], w, c);
            g.line(cx - q * 0.35, cy + q * 0.8, cx + q * 0.35, cy + q * 0.8, w, c);
            g.line(cx, cy + q * 0.45, cx, cy + q * 0.8, w, c);
        }
        Glyph::Racks => {
            for i in 0..3 {
                let y = cy - q * 0.85 + i as f32 * q * 0.62;
                g.rrect_outline(cx - q * 0.85, y, q * 1.7, q * 0.5, q * 0.1, w, c);
                g.circle(cx - q * 0.55, y + q * 0.25, w * 0.9, c);
                g.line(cx - q * 0.2, y + q * 0.25, cx + q * 0.6, y + q * 0.25, w, c);
            }
        }
        Glyph::Bell => g.icon(Icon::Bell, cx, cy, sz, c),
    }
}

impl SrePilotApp {
    fn draw_top(&mut self, g: &mut Gfx, gm: &Geom, s: f32) {
        let t = gm.top;
        // right side: DEMO chip, the two modes and the pause chip, from the right
        let mut x = t.x + t.w;
        let mut put = |g: &mut Gfx, label: &str, accent: [u8; 4], on: bool, ev: Option<Event>| {
            let sz = size::CAPTION * s;
            let w = g.text_width(sz, label, Face::Bold) + sz * 1.4;
            x -= w;
            fx::chip(g, x, t.y + 2.0 * s, s, label, accent, on);
            if let Some(ev) = ev {
                g.hit(R::new(x, t.y + 2.0 * s, w, sz * 1.9), ev, label);
            }
            x -= 8.0 * s;
        };
        put(g, "DEMO", AMBER, true, None);
        put(g, if self.paused { "Resume (Space)" } else { "Pause (Space)" }, MUTED, self.paused, Some(Event::Tab(3)));
        put(g, "Whiteboard (W)", CYAN, self.whiteboard, Some(Event::Tab(2)));
        put(g, "Diagram", CYAN, !self.whiteboard, Some(Event::Tab(1)));
        let sub = g.fit(size::BODY * s, "Demo · assist and autopilot for site reliability", Face::Sans, (x - t.x).max(40.0));
        g.text(t.x, t.y + 4.0 * s, size::BODY * s, MUTED, &sub, Face::Sans);
    }

    fn draw_nodes_hits(&mut self, g: &mut Gfx, lay: &Layout) {
        let mut hov = None;
        for (i, r) in lay.nodes.iter().enumerate() {
            if g.hovered(*r) {
                hov = Some(i);
            }
            g.hit(*r, Event::Tab(100 + i), INFO[i].title);
        }
        self.hover.set(hov);
    }

    /// The cached picture of the canvas (nodes and edges, or the photo), then the moving parts.
    fn draw_canvas(&mut self, g: &mut Gfx, lay: &Layout, s: f32) {
        let c = lay.canvas;
        let key = CacheKey { canvas: (c.x as i32, c.y as i32, c.w as i32, c.h as i32), scale: (s * 100.0) as i32, whiteboard: self.whiteboard, vertical: lay.vertical };
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        let (x0, y0, x1, y1) = ((c.x as i32).clamp(0, gw), (c.y as i32).clamp(0, gh), ((c.x + c.w) as i32 + 1).clamp(0, gw), ((c.y + c.h) as i32 + 1).clamp(0, gh));
        let stride = gw as usize * 4;
        let fresh = self.cache.as_ref().is_some_and(|k| k.key == key && k.rows.len() == (y1 - y0).max(0) as usize * (x1 - x0).max(0) as usize * 4);
        if fresh {
            let k = self.cache.as_ref().unwrap();
            let rw = (x1 - x0) as usize * 4;
            let dst = g.pm.data_mut();
            for (i, row) in k.rows.chunks(rw.max(1)).enumerate() {
                let o = (y0 as usize + i) * stride + x0 as usize * 4;
                dst[o..o + rw].copy_from_slice(row);
            }
        } else {
            if self.whiteboard {
                self.draw_photo_layer(g, lay);
            } else {
                draw_static(g, lay, s);
            }
            let rw = (x1 - x0).max(0) as usize * 4;
            let mut rows = Vec::with_capacity(rw * (y1 - y0).max(0) as usize);
            let data = g.pm.data();
            for y in y0..y1 {
                let o = y as usize * stride + x0 as usize * 4;
                rows.extend_from_slice(&data[o..o + rw]);
            }
            self.cache = Some(Cache { key, rows });
            self.cache_builds += 1;
        }
        self.draw_motion(g, lay, s);
    }

    fn draw_photo_layer(&mut self, g: &mut Gfx, lay: &Layout) {
        let Some(p) = lay.photo else { return };
        let Some(bmp) = self.photo.as_ref() else { return };
        let (dx, dy, dw, dh) = (p.x.round() as i32, p.y.round() as i32, p.w.round() as i32, p.h.round() as i32);
        let (gw, gh) = (g.width() as i32, g.height() as i32);
        let (bw, bh) = (bmp.width as f32, bmp.height as f32);
        let dst = g.pm.data_mut();
        for y in 0..dh {
            let py = dy + y;
            if py < 0 || py >= gh {
                continue;
            }
            for x in 0..dw {
                let px = dx + x;
                if px < 0 || px >= gw {
                    continue;
                }
                let mut acc = [0u32; 3];
                for (ox, oy) in [(0.25f32, 0.25f32), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let sx = (((x as f32 + ox) / dw as f32) * bw) as usize;
                    let sy = (((y as f32 + oy) / dh as f32) * bh) as usize;
                    let o = (sy.min(bmp.height as usize - 1) * bmp.width as usize + sx.min(bmp.width as usize - 1)) * 4;
                    for k in 0..3 {
                        acc[k] += bmp.rgba[o + k] as u32;
                    }
                }
                // a little darker and bluer so the glowing particles stand out on the white board
                let o = (py as usize * gw as usize + px as usize) * 4;
                for (k, tint) in [18u32, 26, 52].iter().enumerate() {
                    let v = acc[k] / 4;
                    dst[o + k] = ((v * 80 + tint * 20) / 100) as u8;
                }
                dst[o + 3] = 255;
            }
        }
        g.rrect_outline(p.x, p.y, p.w, p.h, 6.0, 1.5, with_alpha(CYAN, 150));
    }

    fn draw_motion(&mut self, g: &mut Gfx, lay: &Layout, s: f32) {
        let t = self.now();
        let u = cycle(t);
        let reduced = fx::reduced_motion();
        let pr = (3.2 * s).max(1.8) * if lay.photo.is_some() { 1.5 } else { 1.0 };
        let mut pulse = [0.0f32; NODES];
        // the steady flow: a few particles on every edge
        for (e, edge) in EDGES.iter().enumerate() {
            let per = if reduced { 2 } else { 3 };
            for k in 0..per {
                let f = if reduced { (0.25 + 0.5 * k as f32 + 0.13 * e as f32).fract() } else { (t * edge.3 + k as f32 / per as f32 + e as f32 * 0.137).fract() };
                let (x, y, _) = lay.paths[e].at(f);
                particle(g, x, y, edge.2.color(), pr, false);
                if f > 0.9 {
                    pulse[edge.1] = pulse[edge.1].max((f - 0.9) * 10.0);
                }
            }
        }
        // the incident running through the pipeline
        for (e, a, b, col) in RUNS {
            if u >= a && u <= b {
                let f = ease((u - a) / (b - a));
                for k in (1..5).rev() {
                    let (x, y, _) = lay.paths[e].at((f - k as f32 * 0.035).max(0.0));
                    g.circle(x, y, pr * (1.4 - k as f32 * 0.2), with_alpha(col, 160 - k as u8 * 28));
                }
                let (x, y, _) = lay.paths[e].at(f);
                particle(g, x, y, col, pr * 1.5, true);
            }
        }
        // nodes: soft pulses on arrival, flashes during the incident, hover and selection
        let sel = self.selected();
        for i in 0..NODES {
            let r = lay.nodes[i];
            let acc = INFO[i].accent;
            let rad = if lay.photo.is_some() { 10.0 * s } else { 14.0 * s };
            if pulse[i] > 0.05 {
                g.rrect_glow(r.x, r.y, r.w, r.h, rad, 6.0 * s, with_alpha(acc, (110.0 * pulse[i]) as u8));
            }
            if let Some((f, col)) = flash(i, u) {
                g.rrect_glow(r.x, r.y, r.w, r.h, rad, 10.0 * s, with_alpha(col, (170.0 * f) as u8));
                g.rrect_outline(r.x, r.y, r.w, r.h, rad, 2.0 * s, with_alpha(col, (120.0 + 135.0 * f) as u8));
            }
            if sel == Some(i) || self.hover.get() == Some(i) {
                let a = if sel == Some(i) { 255 } else { 160 };
                g.rrect_glow(r.x, r.y, r.w, r.h, rad, 8.0 * s, with_alpha(acc, a / 2));
                g.rrect_outline(r.x, r.y, r.w, r.h, rad, 2.2 * s, with_alpha(acc, a));
            }
        }
        if lay.photo.is_some() {
            self.draw_photo_texts(g, lay, s, t);
        } else {
            draw_node_status(g, lay, s, t);
        }
    }

    /// Whiteboard mode: the live event log and a small health gauge on the empty part of the board.
    fn draw_photo_texts(&self, g: &mut Gfx, lay: &Layout, s: f32, t: f32) {
        let Some(p) = lay.photo else { return };
        let k = p.w / PHOTO_W;
        let h = health(t);
        let (cx, cy, r) = (p.x + 380.0 * k, p.y + 520.0 * k, (60.0 * k).max(12.0 * s));
        g.circle(cx, cy, r * 1.5, rgb(8, 12, 26));
        fx::ring_gauge(g, cx, cy, r, (r * 0.2).max(2.0), h, health_color(h));
        let txt = format!("{:.0}%", h * 100.0);
        let sz = (r * 0.62).max(8.0);
        let tw = g.text_width(sz, &txt, Face::Bold);
        g.text(cx - tw / 2.0, cy - sz * 0.55, sz, TEXT, &txt, Face::Bold);
        let lines = event_lines(t);
        let (bx, by) = (p.x + 90.0 * k, p.y + 1130.0 * k);
        let bw = 560.0 * k;
        let lh = (24.0 * k).max(11.0 * s);
        let bh = lh * 4.4 + 8.0 * s;
        fx::glass(g, bx, by, bw, bh, 10.0 * s, CYAN);
        g.text(bx + 10.0 * s, by + 4.0 * s, lh * 0.62, DIM, "LIVE · DEMO DATA", Face::Bold);
        for (i, (l, c)) in lines.iter().enumerate() {
            let l = g.fit(lh * 0.72, l, Face::Sans, bw - 20.0 * s);
            g.text(bx + 10.0 * s, by + 4.0 * s + lh * (0.9 + i as f32), lh * 0.72, *c, &l, Face::Sans);
        }
    }

    fn draw_panel(&self, g: &mut Gfx, p: R, s: f32, shown: Option<usize>) {
        fx::panel(g, p, s, shown.map_or(CYAN, |i| INFO[i].accent));
        let (x, w) = (p.x + 16.0 * s, p.w - 32.0 * s);
        let mut y = p.y + 14.0 * s;
        let t = self.now();
        let Some(i) = shown else {
            g.text(x, y, 17.0 * s, TEXT, "How it works", Face::Bold);
            y += 28.0 * s;
            for l in g.wrap(13.0 * s, "Sources send logs, metrics and traces to a sampler, which fills a store of service health. A map, a pilot assist and an auto pilot read it; the pilots act on the site and notify people.", Face::Sans, w) {
                g.text(x, y, 13.0 * s, MUTED, &l, Face::Sans);
                y += 18.0 * s;
            }
            y += 8.0 * s;
            for l in g.wrap(13.0 * s, "Click a part, or press Tab, to see what it does and some sample figures.", Face::Sans, w) {
                g.text(x, y, 13.0 * s, DIM, &l, Face::Sans);
                y += 18.0 * s;
            }
            self.draw_live(g, R::new(x, y + 10.0 * s, w, p.y + p.h - y - 22.0 * s), s, t);
            return;
        };
        let inf = &INFO[i];
        g.circle(x + 16.0 * s, y + 16.0 * s, 17.0 * s, with_alpha(inf.accent, 36));
        glyph(g, inf.glyph, x + 16.0 * s, y + 16.0 * s, 20.0 * s, inf.accent);
        g.text(x + 42.0 * s, y, 17.0 * s, TEXT, inf.title, Face::Bold);
        g.text(x + 42.0 * s, y + 21.0 * s, 12.0 * s, MUTED, inf.sub, Face::Sans);
        let kw = g.text_width(size::CAPTION * s, inf.kind, Face::Bold) + size::CAPTION * s * 1.4;
        fx::chip(g, x + w - kw, y + 2.0 * s, s, inf.kind, inf.accent, false);
        y += 46.0 * s;
        for l in g.wrap(13.0 * s, inf.role, Face::Sans, w) {
            g.text(x, y, 13.0 * s, TEXT, &l, Face::Sans);
            y += 18.0 * s;
        }
        y += 8.0 * s;
        y += fx::section_title(g, x, y, w, s, "Sample figures", inf.accent);
        for (k, v) in metrics(i, t) {
            fx::kv_row(g, R::new(x, y, w, 22.0 * s), s, k, &v, TEXT);
            y += 22.0 * s;
        }
        y += 6.0 * s;
        y += fx::section_title(g, x, y, w, s, "Sample log lines", inf.accent);
        for (n, l) in inf.logs.iter().enumerate() {
            let col = if l.contains("WARN") || l.contains("amber") { AMBER } else if n == 1 && i == AUTO { GREEN } else { MUTED };
            let l = g.fit(11.5 * s, l, Face::Mono, w);
            g.text(x, y, 11.5 * s, col, &l, Face::Mono);
            y += 16.0 * s;
        }
        y += 8.0 * s;
        let sh = (p.y + p.h - y - 14.0 * s).min(54.0 * s);
        if sh > 16.0 * s {
            let r = R::new(x, y, w, sh);
            fx::well(g, r, s);
            let pts: Vec<(f32, f32)> = (0..40)
                .map(|k| {
                    let tt = t - (39 - k) as f32 * 0.25;
                    let v = if i == STORE { health(tt) } else { 0.5 + 0.28 * (tt * 0.9 + i as f32).sin() + 0.12 * (tt * 2.3 + i as f32 * 2.0).sin() };
                    (r.x + 4.0 * s + (r.w - 8.0 * s) * k as f32 / 39.0, r.y + r.h - 4.0 * s - (r.h - 8.0 * s) * v.clamp(0.0, 1.0))
                })
                .collect();
            poly(g, &pts, 4.0 * s, with_alpha(inf.accent, 40));
            poly(g, &pts, 1.5 * s, inf.accent);
            if let Some(l) = pts.last() {
                g.circle(l.0, l.1, 3.0 * s, TEXT);
            }
        }
    }

    fn draw_live(&self, g: &mut Gfx, r: R, s: f32, t: f32) {
        if r.h < 40.0 * s {
            return;
        }
        let mut y = r.y;
        y += fx::section_title(g, r.x, y, r.w, s, "Live event · demo data", CYAN);
        for (l, c) in event_lines(t) {
            let lines = g.wrap(12.5 * s, &l, Face::Sans, r.w);
            for ln in lines {
                if y + 16.0 * s > r.y + r.h {
                    return;
                }
                g.text(r.x, y, 12.5 * s, c, &ln, Face::Sans);
                y += 16.0 * s;
            }
        }
    }

    /// With no side panel: one line under the diagram, the chosen part or the latest event.
    fn draw_info(&self, g: &mut Gfx, r: R, s: f32, shown: Option<usize>) {
        let t = self.now();
        let (txt, col) = match shown {
            Some(i) => (format!("{}: {}", INFO[i].title, INFO[i].role), INFO[i].accent),
            None => event_lines(t).last().cloned().unwrap_or_default(),
        };
        let txt = g.fit(12.0 * s, &txt, Face::Sans, r.w);
        g.text(r.x, r.y, 12.0 * s, if shown.is_some() { TEXT } else { col }, &txt, Face::Sans);
    }

    fn draw_legend(&self, g: &mut Gfx, gm: &Geom, s: f32) {
        let r = gm.legend;
        let mut x = r.x;
        let sz = 12.0 * s;
        for (label, c) in [("Data flow", CYAN), ("Action", AMBER), ("Notification", MAGENTA), ("Feedback", GREEN), ("Incident", RED)] {
            let w = g.text_width(sz, label, Face::Sans) + 30.0 * s;
            if x + w > r.x + r.w {
                break;
            }
            g.line(x, r.y + sz * 0.62, x + 14.0 * s, r.y + sz * 0.62, 2.0 * s, with_alpha(c, 200));
            g.circle(x + 7.0 * s, r.y + sz * 0.62, 3.0 * s, c);
            g.text(x + 20.0 * s, r.y, sz, MUTED, label, Face::Sans);
            x += w + 6.0 * s;
        }
        let hint = "Space pause · 1-3 speed · W whiteboard · Tab select · S demo";
        let hw = g.text_width(11.0 * s, hint, Face::Sans);
        if r.x + r.w - hw > x + 16.0 * s {
            g.text(r.x + r.w - hw, r.y + s, 11.0 * s, DIM, hint, Face::Sans);
        }
    }

    fn draw_button(&self, g: &mut Gfx, gm: &Geom, s: f32) {
        let b = gm.button;
        let hot = g.hovered(b);
        let focused = self.focus == Focus::Button;
        let pulse = if self.animating() { 0.5 + 0.5 * (self.t * 2.4).sin() } else { 0.5 };
        let glow = if hot || focused { 1.0 } else { 0.45 + 0.35 * pulse };
        let rad = b.h / 2.0;
        g.rrect_glow(b.x, b.y, b.w, b.h, rad, 12.0 * s, with_alpha(CYAN, (150.0 * glow) as u8));
        g.rrect(b.x, b.y, b.w, b.h, rad, if hot { mix(CYAN, [255, 255, 255, 255], 0.25) } else { CYAN });
        g.rect(b.x + rad, b.y + 2.0, b.w - 2.0 * rad, (b.h * 0.18).max(1.0), [255, 255, 255, 60]);
        g.rrect_outline(b.x, b.y, b.w, b.h, rad, if focused { 3.0 * s } else { 1.2 * s }, if focused { [255, 255, 255, 255] } else { with_alpha(mix(CYAN, MAGENTA, 0.5), 230) });
        let sz = 15.0 * s;
        let tw = g.text_width(sz, BUTTON_LABEL, Face::Bold);
        g.text(b.x + (b.w - tw) / 2.0, b.y + (b.h - g.line_height(sz, Face::Bold)) / 2.0, sz, INK, BUTTON_LABEL, Face::Bold);
        g.hit(b, Event::Tab(200), BUTTON_LABEL);
    }
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The nodes and edges of the clean diagram (cached).
fn draw_static(g: &mut Gfx, lay: &Layout, s: f32) {
    // edges first, nodes on top
    for (e, edge) in EDGES.iter().enumerate() {
        let c = edge.2.color();
        let p = &lay.paths[e];
        poly(g, &p.pts, 5.0 * s, with_alpha(c, 18));
        poly(g, &p.pts, (1.6 * s).max(1.0), with_alpha(c, 120));
        let (x, y, d) = p.at(1.0);
        arrow_head(g, (x, y), d, 8.0 * s, with_alpha(c, 230));
    }
    if !lay.vertical {
        let cap = (10.0 * s).max(7.0);
        let c = lay.canvas;
        if c.h > 260.0 {
            g.text(c.x + c.w * 0.5, c.y, cap, DIM, "health to the dashboard", Face::Sans);
            let tw = g.text_width(cap, "feedback: the infrastructure is a source too", Face::Sans);
            g.text(c.x + (c.w - tw) / 2.0, c.y + c.h - cap * 1.25, cap, DIM, "feedback: the infrastructure is a source too", Face::Sans);
        }
    }
    for i in 0..NODES {
        draw_node(g, lay, i, s);
    }
}

fn node_k(r: R, s: f32) -> f32 {
    s.min(r.w / 140.0).max(0.45)
}

fn draw_node(g: &mut Gfx, lay: &Layout, i: usize, s: f32) {
    let r = lay.nodes[i];
    let inf = &INFO[i];
    let k = node_k(r, s);
    let rad = (14.0 * k).min(r.h / 2.0);
    g.rrect(r.x + 2.0 * s, r.y + 5.0 * s, r.w, r.h, rad, [0, 0, 0, 100]);
    g.rrect_glow(r.x, r.y, r.w, r.h, rad, 5.0 * s, with_alpha(inf.accent, 36));
    g.rrect(r.x, r.y, r.w, r.h, rad, rgb(12, 17, 36));
    g.gradient_rect(r.x + rad * 0.5, r.y + 1.0, r.w - rad, (r.h * 0.4).max(2.0), (0.0, r.y), (0.0, r.y + r.h * 0.4), &[(0.0, with_alpha(inf.accent, 26)), (1.0, with_alpha(inf.accent, 0))]);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, (1.4 * s).max(1.0), with_alpha(inf.accent, 170));
    let pad = 8.0 * k;
    let bd = (r.h * 0.5).min(30.0 * k).max(12.0);
    let stacked = lay.vertical && r.h < 34.0;
    let (bx, by) = (r.x + pad + bd / 2.0, if i == STORE || r.h < 50.0 * k { r.y + pad + bd / 2.0 } else { r.y + pad + bd / 2.0 });
    let by = if r.h < bd + 2.0 * pad { r.y + r.h / 2.0 } else { by };
    g.circle(bx, by, bd / 2.0 + 1.0, with_alpha(inf.accent, 40));
    glyph(g, inf.glyph, bx, by, bd * 0.8, inf.accent);
    let tx = r.x + pad + bd + 6.0 * k;
    let tw = r.x + r.w - tx - pad * 0.6;
    let mut ts = 13.5 * k;
    while ts > 10.0 * k && g.text_width(ts, inf.title, Face::Bold) > tw {
        ts -= 0.5 * k;
    }
    let title = g.fit(ts, inf.title, Face::Bold, tw);
    let two_lines = r.h >= 40.0 * k && !stacked;
    let ty = if two_lines { by - bd / 2.0 } else { by - ts * 0.6 };
    g.text(tx, ty, ts, TEXT, &title, Face::Bold);
    if two_lines {
        let mut ss = 10.5 * k;
        while ss > 8.5 * k && g.text_width(ss, inf.sub, Face::Sans) > tw {
            ss -= 0.5 * k;
        }
        let sub = g.fit(ss, inf.sub, Face::Sans, tw);
        g.text(tx, ty + ts * 1.25, ss, readable_muted(), &sub, Face::Sans);
    }
    if i == NOTIF && r.h >= 80.0 * k {
        let rows = ["Google Chat", "Teams", "…", "Phone"];
        let top = r.y + pad + bd + 6.0 * k;
        let rh = ((r.y + r.h - pad - top) / 4.0).min(20.0 * k);
        for (n, l) in rows.iter().enumerate() {
            let y = top + n as f32 * rh;
            g.rrect(r.x + pad, y + 1.0, r.w - 2.0 * pad, rh - 2.0, rh / 3.0, rgb(24, 20, 44));
            g.rrect_outline(r.x + pad, y + 1.0, r.w - 2.0 * pad, rh - 2.0, rh / 3.0, 1.0, with_alpha(MAGENTA, 90));
            let ls = (rh * 0.55).min(11.5 * k);
            let l = g.fit(ls, l, Face::Sans, r.w - 4.0 * pad);
            let lw = g.text_width(ls, &l, Face::Sans);
            g.text(r.x + (r.w - lw) / 2.0, y + (rh - ls * 1.2) / 2.0, ls, MUTED, &l, Face::Sans);
        }
    }
    if i == STORE && r.h >= 120.0 * k {
        let cy = r.y + r.h * 0.62;
        let rr = (r.w * 0.26).min(r.h * 0.2);
        g.ring(r.x + r.w / 2.0, cy, rr, 5.0 * k, rgb(26, 34, 62));
        let ls = 10.0 * k;
        let lw = g.text_width(ls, "HEALTH", Face::Bold);
        g.text(r.x + (r.w - lw) / 2.0, cy + rr + 8.0 * k, ls, DIM, "HEALTH", Face::Bold);
    }
}

fn readable_muted() -> [u8; 4] {
    MUTED
}

/// The small moving texts inside the nodes: status lines, the store's gauge, the notification toast.
fn draw_node_status(g: &mut Gfx, lay: &Layout, s: f32, t: f32) {
    let u = cycle(t);
    for (i, r) in lay.nodes.iter().enumerate() {
        let k = node_k(*r, s);
        let pad = 8.0 * k;
        let wh = 20.0 * k;
        match i {
            STORE if r.h >= 120.0 * k => {
                let h = health(t);
                let cy = r.y + r.h * 0.62;
                let rr = (r.w * 0.26).min(r.h * 0.2);
                fx::ring_gauge(g, r.x + r.w / 2.0, cy, rr, 5.0 * k, h, health_color(h));
                let txt = format!("{:.0}%", h * 100.0);
                let sz = rr * 0.62;
                let tw = g.text_width(sz, &txt, Face::Bold);
                g.text(r.x + (r.w - tw) / 2.0, cy - sz * 0.6, sz, TEXT, &txt, Face::Bold);
                // the last minutes as a small line
                let sp = R::new(r.x + pad, r.y + r.h - pad - 22.0 * k, r.w - 2.0 * pad, 22.0 * k);
                if sp.h > 8.0 {
                    let pts: Vec<(f32, f32)> = (0..30).map(|n| (sp.x + sp.w * n as f32 / 29.0, sp.y + sp.h * (1.0 - health(t - (29 - n) as f32 * 0.3)))).collect();
                    poly(g, &pts, 1.4 * k, with_alpha(health_color(h), 200));
                }
            }
            STORE => {
                let h = health(t);
                let (cx, cy) = (r.x + r.w - pad - 6.0 * k, r.y + r.h - pad - 4.0 * k);
                g.circle(cx, cy, 4.0 * k, health_color(h));
            }
            _ if r.h >= 70.0 * k => {
                let sample = INFO[i].logs[((t / 2.5) as usize + i) % 3];
                let (txt, col, mono) = match i {
                    PILOT => pilot_text(u).map_or(("Watching, no advice needed", DIM, false), |x| (x, AMBER, false)),
                    AUTO => auto_text(u).map_or(("Idle, guard rails on", DIM, false), |x| (x, GREEN, false)),
                    INFRA => if (6.8..12.0).contains(&u) { ("checkout x5 ready", GREEN, false) } else { ("checkout x3 ready", DIM, false) },
                    NOTIF => match notify_text(u) {
                        Some(x) => (x, MAGENTA, false),
                        None => continue,
                    },
                    _ => (sample, DIM, true),
                };
                let lines = if i == NOTIF { 2.0 } else { 1.0 };
                let w = R::new(r.x + pad, r.y + r.h - pad - wh * lines, r.w - 2.0 * pad, wh * lines);
                g.rrect(w.x, w.y, w.w, w.h, wh / 3.0, rgb(8, 11, 26));
                g.rrect_outline(w.x, w.y, w.w, w.h, wh / 3.0, 1.0, with_alpha(col, if col == DIM { 70 } else { 200 }));
                let ss = 10.0 * k;
                let face = if mono { Face::Mono } else { Face::Sans };
                if i == NOTIF {
                    for (n, l) in g.wrap(ss, txt, face, w.w - 8.0 * k).iter().take(2).enumerate() {
                        g.text(w.x + 5.0 * k, w.y + 3.0 * k + n as f32 * ss * 1.25, ss, col, l, face);
                    }
                } else {
                    let txt = g.fit(ss, txt, face, w.w - 8.0 * k);
                    g.text(w.x + 5.0 * k, w.y + (w.h - ss * 1.2) / 2.0, ss, col, &txt, face);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    fn frame(app: &mut SrePilotApp, w: u32, h: u32, cx: &AppCx) -> Gfx {
        let mut g = Gfx::new(w, h);
        let s = ((w as f32 / 1280.0).min(h as f32 / 720.0)).clamp(0.6, 3.0);
        crate::fx::background(&mut g);
        app.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
        g
    }

    fn crop(g: &Gfx, r: R) -> Vec<u8> {
        let (gw, gh) = (g.width() as usize, g.height() as usize);
        let mut v = Vec::new();
        for y in (r.y as usize)..((r.y + r.h) as usize).min(gh) {
            for x in (r.x as usize)..((r.x + r.w) as usize).min(gw) {
                v.extend_from_slice(&g.rgba()[(y * gw + x) * 4..(y * gw + x) * 4 + 4]);
            }
        }
        v
    }

    fn lit(g: &Gfx, r: R) -> usize {
        crop(g, r).chunks(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 150).count()
    }

    fn tick(app: &mut SrePilotApp, cx: &AppCx, n: usize) {
        for _ in 0..n {
            app.handle(&Event::Tick, cx);
        }
    }

    const SIZES: [(u32, u32); 3] = [(1280, 720), (1920, 1080), (640, 360)];

    #[test]
    fn the_page_renders_at_three_sizes_in_both_modes() {
        let cx = cx_with_admin_store("sre-render");
        for (w, h) in SIZES {
            for wb in [false, true] {
                let mut app = SrePilotApp::new();
                if wb {
                    app.handle(&Event::Key(Key::Char('w')), &cx);
                }
                let g = frame(&mut app, w, h, &cx);
                let c = app.canvas_rect().unwrap();
                assert!(lit(&g, c) > 300, "{w}x{h} whiteboard={wb} looks blank");
                assert!(g.hits.iter().filter(|x| INFO.iter().any(|i| i.title == x.label)).count() == NODES, "every part is clickable");
            }
        }
    }

    #[test]
    fn particles_move_between_ticks_and_freeze_when_paused() {
        let cx = cx_with_admin_store("sre-move");
        let mut app = SrePilotApp::new();
        let a = frame(&mut app, 1280, 720, &cx);
        let c = app.canvas_rect().unwrap();
        tick(&mut app, &cx, 12);
        let b = frame(&mut app, 1280, 720, &cx);
        assert!(crop(&a, c) != crop(&b, c), "the flow moves");
        app.handle(&Event::Key(Key::Char(' ')), &cx);
        assert!(app.is_paused());
        let p1 = frame(&mut app, 1280, 720, &cx);
        tick(&mut app, &cx, 30);
        assert!(!app.wants_redraw() || true);
        let p2 = frame(&mut app, 1280, 720, &cx);
        assert!(crop(&p1, c) == crop(&p2, c), "paused means frozen");
        app.handle(&Event::Key(Key::Char(' ')), &cx);
        tick(&mut app, &cx, 10);
        let p3 = frame(&mut app, 1280, 720, &cx);
        assert!(crop(&p2, c) != crop(&p3, c), "resumes");
    }

    #[test]
    fn redraws_only_while_animating_and_the_static_layer_is_cached() {
        let cx = cx_with_admin_store("sre-cache");
        let mut app = SrePilotApp::new();
        let mut g = Gfx::new(1280, 720);
        for _ in 0..5 {
            app.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 0);
            tick(&mut app, &cx, 2);
        }
        assert_eq!(app.cache_builds(), 1, "drawn once for one size");
        assert!(app.wants_redraw());
        assert!(!app.wants_redraw(), "once per change");
        app.handle(&Event::Key(Key::Char(' ')), &cx);
        app.wants_redraw();
        tick(&mut app, &cx, 10);
        assert!(!app.wants_redraw(), "no redraw while paused");
        app.render(&mut g, R::new(0.0, 46.0, 640.0, 300.0), 0.6, &cx, 0);
        assert_eq!(app.cache_builds(), 2, "a new size is a new picture");
    }

    #[test]
    fn speed_keys_change_how_fast_the_clock_runs() {
        let cx = cx_with_admin_store("sre-speed");
        let run = |key: char| {
            let mut app = SrePilotApp::new();
            app.handle(&Event::Key(Key::Char(key)), &cx);
            let t0 = app.clock();
            tick(&mut app, &cx, 30);
            app.clock() - t0
        };
        let (slow, normal, fast) = (run('1'), run('2'), run('3'));
        assert!(slow < normal && normal < fast, "{slow} {normal} {fast}");
    }

    #[test]
    fn nodes_are_chosen_by_key_and_by_mouse_and_the_panel_says_what_they_do() {
        let cx = cx_with_admin_store("sre-select");
        let mut app = SrePilotApp::new();
        let g = frame(&mut app, 1280, 720, &cx);
        assert!(app.geom().unwrap().panel.is_some(), "a side panel at 1280 wide");
        let panel = app.geom().unwrap().panel.unwrap();
        let before = crop(&g, panel);
        for _ in 0..4 {
            app.handle(&Event::Key(Key::Tab), &cx);
        }
        assert_eq!(app.selected(), Some(AGG));
        let g2 = frame(&mut app, 1280, 720, &cx);
        assert!(before != crop(&g2, panel));
        app.handle(&Event::Key(Key::BackTab), &cx);
        assert_eq!(app.selected(), Some(SN));
        // mouse: the shell turns a click on a hit region into its event
        let hit = g2.hits.iter().find(|h| h.label == "Pilot Assist").unwrap().clone();
        app.handle(&hit.ev, &cx);
        assert_eq!(app.selected(), Some(PILOT));
        // every node has role text and sample lines
        for i in INFO.iter() {
            assert!(i.role.len() > 40 && i.logs.iter().all(|l| !l.is_empty()));
        }
        assert_eq!(metrics(STORE, 3.0).len(), 3);
        // Esc clears the selection first, then goes back
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Stay));
        assert_eq!(app.selected(), None);
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn hovering_a_node_with_the_pointer_shows_it_in_the_panel() {
        let cx = cx_with_admin_store("sre-hover");
        let mut app = SrePilotApp::new();
        let mut g = frame(&mut app, 1280, 720, &cx);
        let panel = app.geom().unwrap().panel.unwrap();
        let idle = crop(&g, panel);
        let r = app.layout().unwrap().nodes[MAP];
        g.mouse = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        let mut g2 = Gfx::new(1280, 720);
        g2.mouse = g.mouse;
        crate::fx::background(&mut g2);
        app.render(&mut g2, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 0);
        assert!(idle != crop(&g2, panel));
    }

    #[test]
    fn the_whiteboard_photo_decodes_and_the_particles_are_drawn_over_it() {
        let cx = cx_with_admin_store("sre-photo");
        let mut app = SrePilotApp::new();
        assert_eq!(app.photo_size(), Some((1280, 960)), "the photo asset decodes");
        let a = frame(&mut app, 1280, 720, &cx);
        app.handle(&Event::Key(Key::Char('w')), &cx);
        assert!(app.is_whiteboard());
        let b = frame(&mut app, 1280, 720, &cx);
        let c = app.canvas_rect().unwrap();
        assert!(crop(&a, c) != crop(&b, c), "the photo replaces the drawing");
        let lay = app.layout().unwrap();
        let photo = lay.photo.unwrap();
        assert!((photo.w / photo.h - 4.0 / 3.0).abs() < 0.02, "the photo keeps its shape");
        tick(&mut app, &cx, 20);
        let b2 = frame(&mut app, 1280, 720, &cx);
        assert!(crop(&b, c) != crop(&b2, c), "particles move over the photo");
        app.handle(&Event::Key(Key::Char('W')), &cx);
        assert!(!app.is_whiteboard());
        // the traced lines start and end inside the photo, in the order of the edges
        for (i, p) in PHOTO_PATHS.iter().enumerate() {
            assert!(p.len() >= 2 && p.iter().all(|q| q.0 > 0.0 && q.0 < PHOTO_W && q.1 > 0.0 && q.1 < PHOTO_H), "path {i}");
        }
    }

    #[test]
    fn nodes_never_overlap_at_several_sizes() {
        let overlap = |a: &R, b: &R| a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5;
        for (w, h, s) in [(640.0f32, 360.0f32, 0.6f32), (900.0, 500.0, 0.8), (1280.0, 720.0, 1.0), (1920.0, 1080.0, 1.5), (3840.0, 2160.0, 3.0), (400.0, 600.0, 0.6), (300.0, 500.0, 0.6)] {
            let gm = geom(R::new(0.0, 46.0 * s, w, h - 46.0 * s), s, 200.0 * s);
            for lay in [layout_clean(gm.canvas, s), layout_photo(gm.canvas)] {
                for (i, a) in lay.nodes.iter().enumerate() {
                    assert!(a.w > 4.0 && a.h > 4.0, "node {i} has a size at {w}x{h}");
                    let c = lay.canvas;
                    assert!(a.x >= c.x - 0.5 && a.y >= c.y - 0.5 && a.x + a.w <= c.x + c.w + 0.5 && a.y + a.h <= c.y + c.h + 0.5, "node {i} inside the canvas at {w}x{h}: {a:?} in {c:?}");
                    for (j, b) in lay.nodes.iter().enumerate().skip(i + 1) {
                        assert!(!overlap(a, b), "nodes {i} and {j} overlap at {w}x{h} (stack={})", lay.vertical);
                    }
                }
                assert_eq!(lay.paths.len(), EDGES.len());
                assert!(lay.paths.iter().all(|p| p.len() > 1.0));
            }
        }
        assert!(layout_clean(R::new(0.0, 0.0, 400.0, 600.0), 0.6).vertical, "a narrow canvas stacks");
        assert!(!layout_clean(R::new(0.0, 0.0, 900.0, 500.0), 1.0).vertical);
    }

    #[test]
    fn the_page_parts_do_not_overlap_each_other() {
        for (w, h, s) in [(640.0f32, 360.0f32, 0.6f32), (1280.0, 720.0, 1.0), (1920.0, 1080.0, 1.5), (360.0, 640.0, 0.6)] {
            let gm = geom(R::new(0.0, 46.0 * s, w, h - 46.0 * s), s, 200.0 * s);
            assert!(gm.canvas.y >= gm.top.y + gm.top.h && gm.canvas.y + gm.canvas.h <= gm.legend.y + 0.5 || gm.canvas.y + gm.canvas.h <= gm.info.y + 0.5, "{w}x{h}");
            assert!(gm.legend.y + gm.legend.h <= gm.foot.y + 0.5 && gm.foot.y + gm.foot.h <= gm.button.y + 0.5 && gm.button.y + gm.button.h <= gm.founder.y + 0.5, "{w}x{h}");
            assert!(gm.founder.y + gm.founder.h <= h, "{w}x{h}");
            assert!(gm.canvas.h >= 100.0, "{w}x{h} leaves room for the diagram: {}", gm.canvas.h);
        }
    }

    #[test]
    fn reduced_motion_shows_a_still_picture_with_a_few_particles() {
        let cx = cx_with_admin_store("sre-reduced");
        fx::set_reduced_motion(true);
        let mut app = SrePilotApp::new();
        let a = frame(&mut app, 1280, 720, &cx);
        let c = app.canvas_rect().unwrap();
        app.wants_redraw();
        let t0 = app.clock();
        tick(&mut app, &cx, 60);
        let wants = app.wants_redraw();
        let b = frame(&mut app, 1280, 720, &cx);
        fx::set_reduced_motion(false);
        assert_eq!(app.clock(), t0, "the clock does not run");
        assert!(!wants, "no redraws are asked for");
        assert!(crop(&a, c) == crop(&b, c), "a still picture");
        assert!(lit(&a, c) > 300, "and not an empty one");
    }

    #[test]
    fn the_story_is_consistent() {
        // health stays in range, dips to red in the incident and is green when quiet
        let mut lowest: f32 = 1.0;
        for i in 0..1300 {
            let h = health(i as f32 * 0.01);
            assert!((0.0..=1.0).contains(&h));
            lowest = lowest.min(h);
        }
        assert!(lowest < 0.5 && health(0.0) > 0.75 && health(12.5) > 0.75);
        assert!(event_lines(5.0).iter().any(|l| l.0.contains("restart pod")));
        assert!(event_lines(7.0).iter().any(|l| l.0.contains("replicas 3 → 5")));
        assert!(event_lines(9.0).iter().any(|l| l.0.starts_with("Notify")));
        assert!(event_lines(0.1).iter().all(|l| l.0.starts_with("All quiet")));
        // every edge is a real pair, and the run steps point at real edges
        assert!(EDGES.iter().all(|e| e.0 != e.1 && e.0 < NODES && e.1 < NODES));
        assert!(RUNS.iter().all(|r| r.0 < EDGES.len() && r.1 < r.2 && r.2 <= CYCLE));
    }

    #[test]
    fn the_schedule_button_is_drawn_and_only_says_thanks() {
        let cx = cx_with_admin_store("sre-button");
        for (w, h) in SIZES {
            for wb in [false, true] {
                let mut app = SrePilotApp::new();
                if wb {
                    app.handle(&Event::Key(Key::Char('w')), &cx);
                }
                let g = frame(&mut app, w, h, &cx);
                let hit = g.hits.iter().find(|x| x.label == "Click To Schedule a Demo").unwrap_or_else(|| panic!("no button at {w}x{h}"));
                assert_eq!(hit.ev, Event::Tab(200));
                let gm = app.geom().unwrap();
                assert!(hit.rect.x >= 0.0 && hit.rect.x + hit.rect.w <= w as f32 && hit.rect.y + hit.rect.h <= h as f32);
                // the button is bright cyan with dark text, and the founders' line is drawn under it
                let cyan = crop(&g, gm.button).chunks(4).filter(|p| p[1] > 180 && p[2] > 200).count();
                assert!(cyan as f32 > gm.button.w * gm.button.h * 0.4, "the button is filled at {w}x{h}");
                assert!(lit(&g, gm.founder) > 20, "the founders' line has text at {w}x{h}");
                assert!(gm.founder.y > gm.button.y + gm.button.h - 1.0, "text under the button");
                assert!(gm.button.x + gm.button.w / 2.0 > w as f32 * 0.45 && gm.button.x + gm.button.w / 2.0 < w as f32 * 0.55, "centred");
            }
        }
        assert_eq!(FOUNDER_LINE, "One of our founders (Ramesh Veeramalla) will assist you.");
    }

    #[test]
    fn clicking_or_pressing_the_button_shows_the_thanks_and_touches_nothing() {
        let cx = cx_with_admin_store("sre-click");
        let before = std::fs::read_dir(cx.services.dir()).map(|d| d.count()).unwrap_or(0);
        let mut app = SrePilotApp::new();
        let g = frame(&mut app, 1280, 720, &cx);
        assert!(app.toast_text().is_none());
        let hit = g.hits.iter().find(|x| x.label == BUTTON_LABEL).unwrap().clone();
        app.handle(&hit.ev, &cx);
        assert_eq!(app.toast_text(), Some("Thanks for your interest! This is a demo button — the scheduling flow is coming soon."));
        let g2 = frame(&mut app, 1280, 720, &cx);
        assert!(lit(&g2, R::new(300.0, 600.0, 700.0, 80.0)) > 100, "the toast is drawn");
        // it goes away by itself, and by Esc
        tick(&mut app, &cx, 160);
        assert!(app.toast_text().is_none());
        // keyboard: S, and Tab to the button then Enter
        app.handle(&Event::Key(Key::Char('S')), &cx);
        assert!(app.toast_text().is_some());
        app.handle(&Event::Key(Key::Esc), &cx);
        assert!(app.toast_text().is_none());
        for _ in 0..NODES + 1 {
            app.handle(&Event::Key(Key::Tab), &cx);
        }
        assert!(app.button_focused());
        let g3 = frame(&mut app, 1280, 720, &cx);
        assert!(lit(&g3, app.geom().unwrap().button) > 100);
        app.handle(&Event::Key(Key::Enter), &cx);
        assert!(app.toast_text().is_some());
        // nothing was written and no network was needed (the host has none)
        assert_eq!(std::fs::read_dir(cx.services.dir()).map(|d| d.count()).unwrap_or(0), before, "nothing is stored");
        assert_eq!(schedule_demo(), THANKS);
    }

    #[test]
    #[ignore = "writes PNGs to /tmp/iyakki-sre for a look"]
    fn render_pictures_for_a_look() {
        let cx = cx_with_admin_store("sre-look");
        std::fs::create_dir_all("/tmp/iyakki-sre").unwrap();
        for (w, h) in SIZES {
            for (wb, name) in [(false, "diagram"), (true, "board")] {
                let mut app = SrePilotApp::new();
                if wb {
                    app.handle(&Event::Key(Key::Char('w')), &cx);
                }
                for (n, ticks) in [(1, 0usize), (2, 90), (3, 150)] {
                    tick(&mut app, &cx, ticks);
                    if n == 3 && !wb && w == 1280 {
                        app.handle(&Event::Key(Key::Tab), &cx);
                        app.handle(&Event::Key(Key::Tab), &cx);
                    }
                    let g = frame(&mut app, w, h, &cx);
                    g.save_png(std::path::Path::new(&format!("/tmp/iyakki-sre/{name}-{w}x{h}-{n}.png"))).unwrap();
                }
            }
        }
    }
}
