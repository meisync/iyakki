//! The tooltip card of a node on the FLOWS canvas (Design and Run views): its full name, kind, id and settings summary, and in a
//! run its state, time, attempts and the first lines of its error and output; the problems of the step in full. It appears after the
//! pointer has rested on a node for about 350 ms, for the selected node on the `i` key, or on a long press. The look and the timing
//! are `crate::tooltip::Tooltip`'s; this file only says what the card contains and where the nodes are.

use std::collections::BTreeSet;

use iyakki_flow::kinds::Registry;
use iyakki_flow::model::*;

use super::flows::FlowsApp;
use super::flows_canvas::*;
use super::flows_icons::state_words;
use crate::gfx::Gfx;
use crate::ui::R;

/// Heartbeats of a resting pointer before a node's tooltip shows: 11 of 30 a second, about 350 ms.
pub const NODE_TIP_DELAY: u32 = 11;

fn lines_of(text: &str, n: usize, w: usize) -> Vec<String> {
    text.lines().filter(|l| !l.trim().is_empty()).take(n).map(|l| l.trim().chars().take(w).collect()).collect()
}

fn dur_words(ms: i64) -> String {
    if ms < 1000 { format!("{ms} ms") } else { format!("{:.1} s", ms as f64 / 1000.0) }
}

/// What the tooltip of one node says.
pub fn node_tip(reg: &Registry, issues: &[iyakki_flow::kinds::Issue], n: &Node, run: Option<(&Run, i64)>) -> String {
    let k = reg.get(&n.kind);
    let label = k.map(|k| k.label().to_string()).unwrap_or_else(|| n.kind.clone());
    let desc = k.map(|k| k.describe(n)).unwrap_or_default();
    let mut t = format!("{}\n{label} · id {}", n.title(), n.id);
    if !desc.is_empty() && !desc.starts_with(&label) {
        t.push_str(&format!("\n{desc}"));
    }
    if let Some((run, now)) = run {
        if let Some(nr) = run.nodes.get(&n.id) {
            let mut st = state_words(nr.state).to_string();
            if nr.started_ms > 0 {
                let end = if nr.ended_ms > 0 { nr.ended_ms } else { now };
                st.push_str(&format!(" · {}", dur_words((end - nr.started_ms).max(0))));
            }
            if nr.max_attempts > 1 || nr.attempts.len() > 1 {
                st.push_str(&format!(" · attempt {} of {}", nr.attempts.len().max(1), nr.max_attempts.max(1)));
            }
            t.push_str(&format!("\n{st}"));
            for l in lines_of(&nr.error, 3, 160) {
                t.push_str(&format!("\nError: {l}"));
            }
            if !nr.output.is_null() && nr.error.is_empty() {
                t.push_str(&format!("\nOutput: {}", iyakki_flow::kinds::preview(&nr.output, 160)));
            }
            if !nr.note.is_empty() {
                t.push_str(&format!("\n{}", nr.note.chars().take(160).collect::<String>()));
            }
        }
    }
    for i in issues.iter().filter(|i| i.node.as_deref() == Some(n.id.as_str())).take(6) {
        t.push_str(&format!("\n{} {}", if i.level == iyakki_flow::kinds::Level::Error { "Problem:" } else { "Warning:" }, i.msg));
    }
    t
}

impl FlowsApp {
    /// While the graph is drawn: offer the tooltip of the node under the pointer (or the selected one, when the keyboard was used last)
    /// and of the run-state badge on its corner. Nothing is offered under a dialog.
    pub(super) fn offer_node_tips(&mut self, g: &Gfx, canvas: R, view: View, flow: &Flow, run: Option<(&Run, i64)>, selected: &BTreeSet<String>, _s: f32) {
        if self.modal.is_some() {
            return;
        }
        let z = view.zoom;
        let issues = if run.is_some() { Vec::new() } else { self.issues.clone() };
        for n in &flow.nodes {
            let (sx, sy) = view.to_screen(canvas, (n.pos.x, n.pos.y));
            let rect = R::new(sx, sy, NODE_W * z, NODE_H * z);
            let sel = selected.contains(&n.id);
            if !(sel || g.hovered(rect)) || rect.x > canvas.x + canvas.w || rect.y > canvas.y + canvas.h || rect.x + rect.w < canvas.x || rect.y + rect.h < canvas.y {
                continue;
            }
            if let Some((r, _)) = run {
                if let Some(nr) = r.nodes.get(&n.id) {
                    let b = (15.0 * z).max(10.0);
                    let badge = R::new(sx + rect.w - 3.0 * z - b / 2.0, sy + 3.0 * z - b / 2.0, b, b);
                    self.tip.offer(g, badge, false, &format!("{}: {}", n.title(), state_words(nr.state)));
                }
            }
            let text = node_tip(&self.reg, &issues, n, run);
            self.tip.offer(g, rect, sel, &text);
        }
    }
}
