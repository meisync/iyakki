//! The small pictures on the FLOWS canvas: one vector icon per node kind and one per run state, drawn with the shell's own
//! shapes (lines, rings, polygons): no font, no image file, no emoji. Every icon is drawn inside a square of side `size` centred on
//! `(cx, cy)`; the colour is the kind's accent.

use iyakki_flow::model::NodeState;

use crate::fx::{with_alpha, AMBER};
use crate::gfx::{rgb, Gfx};
use crate::ui::{ERR, MUTED, OK};

/// Every node kind that has its own icon (the table test checks that the registry has no kind outside it).
pub const KINDS: [&str; 40] = [
    "start", "end", "http", "cli", "browser", "agent", "script", "grpc", "thrift", "remote", "condition", "switch", "parallel", "join", "delay", "approval", "notify", "subflow", "foreach", "cam_photo", "br_open", "br_screenshot", "br_dom", "br_assert", "br_click", "br_type", "br_fill", "br_submit", "br_key", "br_hover", "br_scroll", "br_select", "br_wait", "br_extract", "br_js", "br_frame", "br_viewport", "br_storage", "br_download", "br_compare",
];

pub fn has_icon(kind: &str) -> bool {
    KINDS.contains(&kind)
}

fn arc(g: &mut Gfx, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, w: f32, c: [u8; 4]) {
    let n = 10;
    let mut prev = (cx + r * a0.cos(), cy + r * a0.sin());
    for i in 1..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        let p = (cx + r * a.cos(), cy + r * a.sin());
        g.line(prev.0, prev.1, p.0, p.1, w, c);
        prev = p;
    }
}

fn poly(g: &mut Gfx, pts: &[(f32, f32)], w: f32, c: [u8; 4], close: bool) {
    for s in pts.windows(2) {
        g.line(s[0].0, s[0].1, s[1].0, s[1].1, w, c);
    }
    if close && pts.len() > 2 {
        g.line(pts[pts.len() - 1].0, pts[pts.len() - 1].1, pts[0].0, pts[0].1, w, c);
    }
}

/// The icon of a node kind.
pub fn draw_kind(g: &mut Gfx, kind: &str, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let r = size / 2.0;
    let w = (size / 11.0).max(1.2);
    let p = |dx: f32, dy: f32| (cx + dx * r, cy + dy * r);
    let pi = std::f32::consts::PI;
    match kind {
        "start" => g.polygon(&[p(-0.35, -0.7), p(-0.35, 0.7), p(0.75, 0.0)], c),
        "end" => {
            g.line(p(-0.5, -0.8).0, p(-0.5, -0.8).1, p(-0.5, 0.85).0, p(-0.5, 0.85).1, w, c);
            g.polygon(&[p(-0.5, -0.8), p(0.8, -0.4), p(-0.5, 0.05)], c);
        }
        "http" => {
            g.ring(cx, cy, r * 0.8, w, c);
            g.line(p(-0.8, 0.0).0, cy, p(0.8, 0.0).0, cy, w * 0.8, c);
            g.line(cx, p(0.0, -0.8).1, cx, p(0.0, 0.8).1, w * 0.8, c);
            poly(g, &[p(-0.35, -0.7), p(-0.5, 0.0), p(-0.35, 0.7)], w * 0.8, c, false);
            poly(g, &[p(0.35, -0.7), p(0.5, 0.0), p(0.35, 0.7)], w * 0.8, c, false);
        }
        "cli" => {
            g.rrect_outline(cx - r * 0.9, cy - r * 0.7, r * 1.8, r * 1.4, r * 0.2, w, c);
            poly(g, &[p(-0.5, -0.3), p(-0.1, 0.0), p(-0.5, 0.3)], w, c, false);
            g.line(p(0.05, 0.35).0, p(0.05, 0.35).1, p(0.5, 0.35).0, p(0.5, 0.35).1, w, c);
        }
        "browser" | "br_open" | "br_viewport" | "br_frame" | "br_storage" | "br_download" | "br_wait" | "br_select" | "br_hover" | "br_scroll" | "br_key" | "br_extract" | "br_js" | "br_compare" => {
            g.rrect_outline(cx - r * 0.9, cy - r * 0.75, r * 1.8, r * 1.5, r * 0.2, w, c);
            g.line(p(-0.9, -0.3).0, p(-0.9, -0.3).1, p(0.9, -0.3).0, p(0.9, -0.3).1, w * 0.8, c);
            match kind {
                "br_open" => poly(g, &[p(-0.2, 0.05), p(0.4, 0.05), p(0.15, -0.1)], w * 0.8, c, false),
                "br_wait" => g.ring(cx, cy + r * 0.25, r * 0.28, w * 0.8, c),
                "br_scroll" => poly(g, &[p(0.0, 0.0), p(0.0, 0.6), p(-0.2, 0.4)], w * 0.8, c, false),
                "br_js" => poly(g, &[p(-0.25, 0.0), p(-0.45, 0.25), p(-0.25, 0.5)], w * 0.8, c, false),
                _ => g.circle(cx, cy + r * 0.25, r * 0.15, c),
            }
        }
        "agent" => {
            // a four-point spark and a small one
            g.polygon(&[p(0.0, -0.9), p(0.18, -0.18), p(0.9, 0.0), p(0.18, 0.18), p(0.0, 0.9), p(-0.18, 0.18), p(-0.9, 0.0), p(-0.18, -0.18)], c);
            g.circle(p(0.7, -0.7).0, p(0.7, -0.7).1, r * 0.14, c);
        }
        "script" => {
            poly(g, &[p(-0.2, -0.8), p(-0.45, -0.8), p(-0.55, -0.55), p(-0.55, -0.1), p(-0.8, 0.0), p(-0.55, 0.1), p(-0.55, 0.55), p(-0.45, 0.8), p(-0.2, 0.8)], w, c, false);
            poly(g, &[p(0.2, -0.8), p(0.45, -0.8), p(0.55, -0.55), p(0.55, -0.1), p(0.8, 0.0), p(0.55, 0.1), p(0.55, 0.55), p(0.45, 0.8), p(0.2, 0.8)], w, c, false);
        }
        "grpc" | "thrift" | "remote" => {
            g.rrect(cx - r * 0.45, cy - r * 0.2, r * 0.9, r * 0.8, r * 0.15, c);
            g.line(p(-0.22, -0.2).0, p(-0.22, -0.2).1, p(-0.22, -0.8).0, p(-0.22, -0.8).1, w * 1.1, c);
            g.line(p(0.22, -0.2).0, p(0.22, -0.2).1, p(0.22, -0.8).0, p(0.22, -0.8).1, w * 1.1, c);
            g.line(cx, p(0.0, 0.6).1, cx, p(0.0, 0.95).1, w, c);
        }
        "condition" => poly(g, &[p(0.0, -0.9), p(0.9, 0.0), p(0.0, 0.9), p(-0.9, 0.0)], w, c, true),
        "switch" => {
            g.line(p(-0.9, 0.0).0, cy, p(-0.2, 0.0).0, cy, w, c);
            for dy in [-0.7f32, 0.0, 0.7] {
                g.line(p(-0.2, 0.0).0, cy, p(0.8, dy).0, p(0.8, dy).1, w, c);
                g.circle(p(0.8, dy).0, p(0.8, dy).1, r * 0.14, c);
            }
        }
        "parallel" => {
            g.line(p(-0.8, -0.35).0, p(-0.8, -0.35).1, p(0.8, -0.35).0, p(0.8, -0.35).1, w, c);
            g.line(p(-0.8, 0.35).0, p(-0.8, 0.35).1, p(0.8, 0.35).0, p(0.8, 0.35).1, w, c);
            g.polygon(&[p(0.9, -0.35), p(0.5, -0.65), p(0.5, -0.05)], c);
            g.polygon(&[p(0.9, 0.35), p(0.5, 0.05), p(0.5, 0.65)], c);
        }
        "join" => {
            g.line(p(-0.8, -0.7).0, p(-0.8, -0.7).1, p(0.1, 0.0).0, cy, w, c);
            g.line(p(-0.8, 0.7).0, p(-0.8, 0.7).1, p(0.1, 0.0).0, cy, w, c);
            g.line(p(0.1, 0.0).0, cy, p(0.8, 0.0).0, cy, w, c);
            g.circle(p(0.8, 0.0).0, cy, r * 0.16, c);
        }
        "delay" => {
            g.ring(cx, cy, r * 0.85, w, c);
            g.line(cx, cy, cx, p(0.0, -0.5).1, w, c);
            g.line(cx, cy, p(0.4, 0.0).0, p(0.0, 0.25).1, w, c);
        }
        "approval" | "br_assert" => {
            g.ring(cx, cy, r * 0.85, w, c);
            poly(g, &[p(-0.4, 0.05), p(-0.1, 0.4), p(0.45, -0.3)], w * 1.2, c, false);
        }
        "notify" => {
            g.polygon(&[p(-0.2, -0.75), p(0.2, -0.75), p(0.55, -0.2), p(0.7, 0.45), p(-0.7, 0.45), p(-0.55, -0.2)], c);
            g.circle(cx, p(0.0, 0.7).1, r * 0.16, c);
        }
        "subflow" => {
            g.rrect_outline(cx - r * 0.9, cy - r * 0.9, r * 1.1, r * 1.1, r * 0.15, w, c);
            g.rrect_outline(cx - r * 0.2, cy - r * 0.2, r * 1.1, r * 1.1, r * 0.15, w, c);
        }
        "foreach" => {
            arc(g, cx, cy, r * 0.7, -pi * 0.2, pi * 1.5, w, c);
            g.polygon(&[p(0.9, -0.2), p(0.45, -0.2), p(0.7, -0.65)], c);
        }
        "cam_photo" | "br_screenshot" => {
            g.rrect_outline(cx - r * 0.9, cy - r * 0.5, r * 1.8, r * 1.3, r * 0.2, w, c);
            g.rrect(cx - r * 0.35, cy - r * 0.8, r * 0.7, r * 0.3, r * 0.1, c);
            g.ring(cx, cy + r * 0.15, r * 0.35, w, c);
        }
        "br_dom" => {
            poly(g, &[p(-0.3, -0.5), p(-0.8, 0.0), p(-0.3, 0.5)], w, c, false);
            poly(g, &[p(0.3, -0.5), p(0.8, 0.0), p(0.3, 0.5)], w, c, false);
            g.line(p(0.15, -0.6).0, p(0.15, -0.6).1, p(-0.15, 0.6).0, p(-0.15, 0.6).1, w, c);
        }
        "br_click" => g.polygon(&[p(-0.5, -0.8), p(-0.5, 0.6), p(-0.15, 0.25), p(0.15, 0.85), p(0.35, 0.75), p(0.05, 0.15), p(0.5, 0.1)], c),
        "br_type" => {
            g.line(p(-0.6, -0.7).0, p(-0.6, -0.7).1, p(0.6, -0.7).0, p(0.6, -0.7).1, w, c);
            g.line(cx, p(0.0, -0.7).1, cx, p(0.0, 0.8).1, w, c);
            g.line(p(-0.3, 0.8).0, p(-0.3, 0.8).1, p(0.3, 0.8).0, p(0.3, 0.8).1, w, c);
        }
        "br_fill" => {
            g.rrect_outline(cx - r * 0.85, cy - r * 0.8, r * 1.7, r * 0.6, r * 0.12, w, c);
            g.rrect_outline(cx - r * 0.85, cy + r * 0.1, r * 1.7, r * 0.6, r * 0.12, w, c);
            g.line(p(-0.55, -0.5).0, p(-0.55, -0.5).1, p(0.0, -0.5).0, p(0.0, -0.5).1, w, c);
        }
        "br_submit" => {
            g.rrect_outline(cx - r * 0.9, cy - r * 0.55, r * 1.8, r * 1.1, r * 0.15, w, c);
            poly(g, &[p(0.5, -0.1), p(0.5, 0.2), p(-0.4, 0.2)], w, c, false);
            g.polygon(&[p(-0.6, 0.2), p(-0.3, 0.0), p(-0.3, 0.4)], c);
        }
        _ => {
            // a kind with no picture of its own still gets one: a plain rounded square
            g.rrect_outline(cx - r * 0.75, cy - r * 0.75, r * 1.5, r * 1.5, r * 0.25, w, c);
        }
    }
}

/// The colour of a state's badge.
pub fn state_badge_color(st: NodeState) -> [u8; 4] {
    match st {
        NodeState::Succeeded => OK,
        NodeState::Failed => ERR,
        NodeState::Running => rgb(90, 170, 255),
        NodeState::Retrying => AMBER,
        NodeState::Waiting => rgb(190, 130, 255),
        NodeState::Interrupted => rgb(255, 150, 80),
        NodeState::Pending | NodeState::Skipped | NodeState::Cancelled => MUTED,
    }
}

/// What a state's badge says when hovered.
pub fn state_words(st: NodeState) -> &'static str {
    match st {
        NodeState::Succeeded => "Done",
        NodeState::Failed => "Failed",
        NodeState::Running => "Running now",
        NodeState::Retrying => "Waiting to try again",
        NodeState::Waiting => "Waiting for you",
        NodeState::Interrupted => "Interrupted by a restart",
        NodeState::Pending => "Not started yet",
        NodeState::Skipped => "Skipped",
        NodeState::Cancelled => "Stopped",
    }
}

/// The round badge of a run state on a node's corner: done tick, failed cross, running spinner, pending dot, skipped dash, a raised
/// hand for "waiting for you", a circular arrow for a retry.
pub fn draw_state(g: &mut Gfx, st: NodeState, cx: f32, cy: f32, size: f32, tick: u64) {
    let r = size / 2.0;
    let c = state_badge_color(st);
    let w = (size / 9.0).max(1.2);
    let white = rgb(255, 255, 255);
    let pi = std::f32::consts::PI;
    g.circle(cx, cy, r, rgb(16, 20, 38));
    match st {
        NodeState::Succeeded => {
            g.circle(cx, cy, r * 0.9, c);
            poly(g, &[(cx - r * 0.45, cy + r * 0.0), (cx - r * 0.1, cy + r * 0.38), (cx + r * 0.5, cy - r * 0.35)], w, white, false);
        }
        NodeState::Failed | NodeState::Cancelled => {
            g.circle(cx, cy, r * 0.9, c);
            g.line(cx - r * 0.35, cy - r * 0.35, cx + r * 0.35, cy + r * 0.35, w, white);
            g.line(cx + r * 0.35, cy - r * 0.35, cx - r * 0.35, cy + r * 0.35, w, white);
        }
        NodeState::Running => {
            g.ring(cx, cy, r * 0.8, w * 0.7, with_alpha(c, 70));
            let a0 = (tick as f32 * 0.25) % (2.0 * pi);
            arc(g, cx, cy, r * 0.8, a0, a0 + pi * 1.1, w * 1.3, c);
        }
        NodeState::Retrying => {
            arc(g, cx, cy, r * 0.65, -pi * 0.2, pi * 1.5, w, c);
            g.polygon(&[(cx + r * 0.85, cy - r * 0.1), (cx + r * 0.45, cy - r * 0.1), (cx + r * 0.65, cy - r * 0.5)], c);
        }
        NodeState::Waiting => {
            // a raised hand: palm and four fingers
            g.rrect(cx - r * 0.4, cy - r * 0.05, r * 0.8, r * 0.7, r * 0.2, c);
            for i in 0..4 {
                let x = cx - r * 0.33 + i as f32 * r * 0.22;
                g.line(x, cy, x, cy - r * (0.55 + if i == 1 || i == 2 { 0.15 } else { 0.0 }), w * 1.1, c);
            }
        }
        NodeState::Interrupted => {
            g.line(cx, cy - r * 0.5, cx, cy + r * 0.1, w * 1.2, c);
            g.circle(cx, cy + r * 0.5, r * 0.13, c);
        }
        NodeState::Pending => g.ring(cx, cy, r * 0.45, w, c),
        NodeState::Skipped => g.line(cx - r * 0.5, cy, cx + r * 0.5, cy, w * 1.2, c),
    }
}

/// The problem or warning badge: a triangle with a mark (red for problems, amber for warnings) and the count beside it.
pub fn draw_alert(g: &mut Gfx, cx: f32, cy: f32, size: f32, error: bool) {
    let r = size / 2.0;
    let c = if error { ERR } else { AMBER };
    g.polygon(&[(cx, cy - r), (cx + r, cy + r * 0.8), (cx - r, cy + r * 0.8)], c);
    let dark = rgb(20, 22, 36);
    g.line(cx, cy - r * 0.3, cx, cy + r * 0.25, (size / 9.0).max(1.2), dark);
    g.circle(cx, cy + r * 0.55, size / 18.0 + 0.4, dark);
}
