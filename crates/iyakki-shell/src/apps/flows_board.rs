//! FLOWS, the BOARD tab: every run in lanes (Scheduled, Running, Needs you, Failed, Done) with a flow filter and a search, the next
//! runs the schedules will start, and the history as a table. Click a run to open it in RUN; Shift+click selects several for the
//! bulk actions (run again, cancel).

use iyakki_flow::model::*;

use super::flows::*;
use super::flows_view::GfxFit;
use super::flows_run::status_color;
use super::flows_view::{ago_text, dur_text, when_text};
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) const LANES: [&str; 5] = ["Scheduled", "Running", "Needs you", "Failed", "Done"];
/// `Event::Row(HIST_BASE + i)` is a click on the i-th row of the history table.
pub(super) const HIST_BASE: usize = 10_000;

pub(super) fn lane_of(st: RunStatus) -> usize {
    match st {
        RunStatus::Scheduled | RunStatus::Queued => 0,
        RunStatus::Running | RunStatus::Retrying | RunStatus::Healing => 1,
        RunStatus::Blocked => 2,
        RunStatus::Failed => 3,
        RunStatus::Succeeded | RunStatus::Cancelled => 4,
    }
}

fn lane_color(i: usize) -> [u8; 4] {
    [crate::gfx::rgb(120, 170, 255), CYAN, MAGENTA, ERR, OK][i.min(4)]
}

impl FlowsApp {
    fn run_matches(&self, r: &RunSummary) -> bool {
        if r.parent.is_some() {
            return false;
        }
        if self.board.flow_filter.as_ref().is_some_and(|f| *f != r.flow_id) {
            return false;
        }
        let q = self.board.search.value.trim().to_lowercase();
        q.is_empty() || [&r.id, &r.flow_id, &r.flow_name, &r.trigger, &r.error, &r.blocked].iter().any(|t| t.to_lowercase().contains(&q)) || r.status.label().contains(&q)
    }

    /// The runs on the board, lane by lane, newest first in each.
    pub(super) fn board_lanes(&self) -> Vec<Vec<RunSummary>> {
        let mut lanes: Vec<Vec<RunSummary>> = vec![vec![]; 5];
        for r in self.runs.iter().filter(|r| self.run_matches(r)) {
            lanes[lane_of(r.status)].push(r.clone());
        }
        lanes
    }

    /// The run ids in card order (what `Event::Row(n)` counts over).
    pub(super) fn board_rows(&self) -> Vec<String> {
        self.board_lanes().into_iter().flatten().map(|r| r.id).collect()
    }

    pub(super) fn history_rows(&self) -> Vec<RunSummary> {
        self.runs.iter().filter(|r| self.run_matches(r)).cloned().collect()
    }

    fn draw_card_for(&mut self, g: &mut Gfx, r: R, s: f32, x: &RunSummary, row: usize, now: i64) {
        let hot = g.hovered(r);
        let sel = self.board.sel.contains(&x.id);
        let cur = self.run_id.as_deref() == Some(x.id.as_str());
        let c = status_color(x.status);
        g.rrect(r.x, r.y, r.w, r.h, 9.0 * s, if hot { crate::ui::CARD_HOVER } else { crate::ui::CARD });
        g.rect(r.x, r.y + 5.0 * s, 3.0 * s, r.h - 10.0 * s, c);
        g.rrect_outline(r.x, r.y, r.w, r.h, 9.0 * s, if sel { 2.2 * s } else { 1.0 }, if sel { ACCENT } else if cur { with_alpha(ACCENT, 140) } else { with_alpha(c, 70) });
        let pad = 10.0 * s;
        let t1 = g.fit(12.5 * s, &format!("{} · {}", x.id, x.flow_name), Face::Bold, r.w - 2.0 * pad);
        g.text(r.x + pad, r.y + 5.0 * s, 12.5 * s, TEXT, &t1, Face::Bold);
        let when = if let Some(at) = x.scheduled_ms.filter(|_| lane_of(x.status) == 0) { format!("starts {}", when_text(at, now)) } else if x.ended_ms > 0 { format!("{} · took {}", ago_text(now - x.ended_ms), dur_text(x.ended_ms - x.started_ms)) } else if x.started_ms > 0 { format!("started {}", ago_text(now - x.started_ms)) } else { "waiting".to_string() };
        let t2 = g.fit(10.5 * s, &format!("v{} · {} · {}", x.version, x.trigger, when), Face::Sans, r.w - 2.0 * pad);
        g.text(r.x + pad, r.y + 22.0 * s, 10.5 * s, DIM, &t2, Face::Sans);
        let detail = if !x.blocked.is_empty() { x.blocked.clone() } else if !x.error.is_empty() { x.error.clone() } else { format!("{}/{} steps{}", x.nodes_done, x.nodes_total, if x.cards > 0 { format!(" · {} fix card(s)", x.cards) } else { String::new() }) };
        let dc = if !x.blocked.is_empty() { MAGENTA } else if !x.error.is_empty() { ERR } else { MUTED };
        let t3 = g.fit(10.5 * s, &detail, Face::Sans, r.w - 2.0 * pad);
        g.text(r.x + pad, r.y + 37.0 * s, 10.5 * s, dc, &t3, Face::Sans);
        if x.nodes_total > 0 && !x.status.is_terminal() {
            fx::progress_bar(g, R::new(r.x + pad, r.y + r.h - 8.0 * s, r.w - 2.0 * pad, 3.0 * s), x.nodes_done as f32 / x.nodes_total as f32, c, s);
        }
        g.hit(r, Event::Row(row), &x.id);
    }

    pub(super) fn draw_board(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let _ = tick;
        let pad = 10.0 * s;
        let now = self.now();
        let status_h = 24.0 * s;
        let inner = R::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - status_h - 4.0 * s);
        // the search and, in a narrow window, the lane chips
        let wide = inner.w >= 900.0 * s;
        let blink = super::blink(self.tick);
        let on = self.focus == Focus::BoardSearch;
        let sw = (inner.w * 0.34).clamp(160.0 * s, 300.0 * s);
        let field = R::new(inner.x, inner.y, sw, 32.0 * s);
        super::with_caret(on, || self.board.search.render(g, field, on, s, blink, Face::Sans, fid::BOARD_SEARCH));
        let lanes = self.board_lanes();
        let total: usize = lanes.iter().map(Vec::len).sum();
        let mut x = field.x + field.w + 8.0 * s;
        let mut y = field.y;
        if !wide {
            for (i, name) in LANES.iter().enumerate() {
                let label = format!("{name} {}", lanes[i].len());
                let size = 11.5 * s;
                let w = g.text_width(size, &label, Face::Bold) + size * 1.7;
                if x + w > inner.x + inner.w && x > field.x + field.w + 8.0 * s {
                    x = inner.x;
                    y += 30.0 * s;
                }
                if y < field.y + field.h {
                    g.button(x, y, size, &label, if self.board.lane == i { lane_color(i) } else { MUTED }, Event::Tab(id::B_LANE + i));
                } else {
                    g.button(x, y, size, &label, if self.board.lane == i { lane_color(i) } else { MUTED }, Event::Tab(id::B_LANE + i));
                }
                x += w + 5.0 * s;
            }
        } else {
            g.text(x, y + 8.0 * s, 12.0 * s, DIM, &format!("{total} run{}", if total == 1 { "" } else { "s" }), Face::Sans);
        }
        let top = y + 40.0 * s;
        let avail = R::new(inner.x, top, inner.w, (inner.y + inner.h - top).max(0.0));
        if total == 0 && self.runs.is_empty() {
            fx::empty_state(g, avail, s, Icon::Runs, "Nothing has run yet", "Runs from schedules, from the Run button and from the assistant appear here, in lanes: Scheduled, Running, Needs you, Failed, Done.", ACCENT);
            self.draw_next(g, R::new(avail.x, avail.y + avail.h - 90.0 * s, avail.w.min(420.0 * s), 90.0 * s), s, now);
            self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
            return;
        }
        let bottom_h = if wide && avail.h >= 430.0 * s { (avail.h * 0.40).min(260.0 * s) } else { 0.0 };
        let lanes_r = R::new(avail.x, avail.y, avail.w, avail.h - if bottom_h > 0.0 { bottom_h + 8.0 * s } else { 0.0 });
        let card_h = 58.0 * s;
        let gap = 8.0 * s;
        let skip = self.board.hist_scroll;
        let show: Vec<usize> = if wide { (0..5).collect() } else { vec![self.board.lane.min(4)] };
        let lane_w = (lanes_r.w - gap * (show.len() as f32 - 1.0)) / show.len() as f32;
        for (col, &li) in show.iter().enumerate() {
            let lr = R::new(lanes_r.x + col as f32 * (lane_w + gap), lanes_r.y, lane_w, lanes_r.h);
            fx::panel(g, lr, s, lane_color(li));
            let c = lane_color(li);
            g.text(lr.x + 12.0 * s, lr.y + 8.0 * s, 12.5 * s, c, &LANES[li].to_uppercase(), Face::Bold);
            let n = format!("{}", lanes[li].len());
            let nw = g.text_width(12.0 * s, &n, Face::Bold);
            g.text(lr.x + lr.w - nw - 12.0 * s, lr.y + 8.0 * s, 12.0 * s, c, &n, Face::Bold);
            let list = R::new(lr.x + 8.0 * s, lr.y + 30.0 * s, lr.w - 16.0 * s, lr.h - 38.0 * s);
            let vis = ((list.h / (card_h + 6.0 * s)).floor() as usize).max(1);
            // the card number is its place over all lanes, so `Row(n)` finds the run
            let row_base: usize = lanes.iter().take(li).map(Vec::len).sum();
            for (k, x) in lanes[li].iter().enumerate().skip(skip.min(lanes[li].len().saturating_sub(1))).take(vis) {
                let rr = R::new(list.x, list.y + (k - skip.min(lanes[li].len().saturating_sub(1))) as f32 * (card_h + 6.0 * s), list.w, card_h);
                if rr.y + rr.h > list.y + list.h + 1.0 {
                    break;
                }
                self.draw_card_for(g, rr, s, x, row_base + k, now);
            }
            if lanes[li].is_empty() {
                g.text(list.x + 4.0 * s, list.y + 6.0 * s, 11.5 * s, DIM, "empty", Face::Sans);
            } else if lanes[li].len() > skip + vis {
                g.text(list.x + 4.0 * s, list.y + list.h - 14.0 * s, 10.5 * s, DIM, &format!("+{} more (wheel)", lanes[li].len() - skip - vis), Face::Sans);
            }
        }
        if bottom_h > 0.0 {
            let br = R::new(avail.x, avail.y + avail.h - bottom_h, avail.w, bottom_h);
            let nw = (br.w * 0.34).clamp(240.0 * s, 380.0 * s);
            self.draw_next(g, R::new(br.x, br.y, nw, br.h), s, now);
            self.draw_history(g, R::new(br.x + nw + 8.0 * s, br.y, br.w - nw - 8.0 * s, br.h), s, now);
        } else if !wide && self.board.lane == 0 && !self.board.next.is_empty() {
            // under the scheduled cards in a narrow window
        }
        self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
    }

    fn draw_next(&mut self, g: &mut Gfx, r: R, s: f32, now: i64) {
        fx::panel(g, r, s, ACCENT);
        g.text(r.x + 12.0 * s, r.y + 8.0 * s, 12.0 * s, ACCENT, "NEXT RUNS", Face::Bold);
        let mut y = r.y + 28.0 * s;
        if self.board.next.is_empty() {
            g.text(r.x + 12.0 * s, y, 11.5 * s, DIM, "Nothing is scheduled. A published flow with a schedule shows its next times here.", Face::Sans);
            return;
        }
        for (flow, trig, at) in self.board.next.iter() {
            if y + 18.0 * s > r.y + r.h {
                break;
            }
            let name = self.flows.iter().find(|f| f.id == *flow).map(|f| f.name.clone()).unwrap_or_else(|| flow.clone());
            let w1 = g.text_width(11.5 * s, &when_text(*at, now), Face::Bold) + 8.0 * s;
            g.text(r.x + 12.0 * s, y, 11.5 * s, OK, &when_text(*at, now), Face::Bold);
            g.text_fit(r.x + 12.0 * s + w1.max(70.0 * s), y, 11.5 * s, TEXT, &format!("{name} · {trig}"), Face::Sans, r.w - w1.max(70.0 * s) - 24.0 * s);
            y += 18.0 * s;
        }
    }

    fn draw_history(&mut self, g: &mut Gfx, r: R, s: f32, now: i64) {
        fx::panel(g, r, s, ACCENT);
        g.text(r.x + 12.0 * s, r.y + 8.0 * s, 12.0 * s, ACCENT, "HISTORY", Face::Bold);
        let cols: [(&str, f32); 7] = [("run", 0.0), ("flow", 70.0), ("v", 230.0), ("trigger", 262.0), ("status", 350.0), ("started", 440.0), ("took", 530.0)];
        let w = r.w - 20.0 * s;
        let scale = (w / (600.0 * s)).clamp(0.6, 1.4);
        fx::table_header(g, r.x + 12.0 * s, r.y + 26.0 * s, w, s, &cols.iter().map(|(t, o)| (*t, o * scale)).collect::<Vec<_>>(), ACCENT);
        let rows = self.history_rows();
        let rh = 19.0 * s;
        let list = R::new(r.x + 8.0 * s, r.y + 50.0 * s, r.w - 16.0 * s, r.h - 56.0 * s);
        let vis = ((list.h / rh).floor() as usize).max(1);
        let skip = self.board.hist_scroll.min(rows.len().saturating_sub(1));
        for (i, x) in rows.iter().enumerate().skip(skip).take(vis) {
            let rr = R::new(list.x, list.y + (i - skip) as f32 * rh, list.w, rh);
            let hot = g.hovered(rr);
            if hot || self.run_id.as_deref() == Some(x.id.as_str()) {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 5.0 * s, with_alpha(ACCENT, if hot { 40 } else { 24 }));
            }
            let cells = [x.id.clone(), x.flow_name.clone(), format!("v{}", x.version), x.trigger.clone(), x.status.label().to_string(), if x.started_ms > 0 { ago_text(now - x.started_ms) } else { "-".into() }, if x.started_ms > 0 { dur_text(if x.ended_ms > 0 { x.ended_ms - x.started_ms } else { now - x.started_ms }) } else { "-".into() }];
            for (j, (t, (_, off))) in cells.iter().zip(cols.iter()).enumerate() {
                let maxw = cols.get(j + 1).map_or(r.x + r.w - (rr.x + 4.0 * s + off * scale), |n| (n.1 - off) * scale - 6.0 * s);
                let c = if j == 4 { status_color(x.status) } else if j == 0 { TEXT } else { MUTED };
                g.text_fit(rr.x + 4.0 * s + off * scale, rr.y + 3.0 * s, 11.0 * s, c, t, Face::Sans, maxw.max(20.0));
            }
            g.hit(rr, Event::Row(HIST_BASE + i), &x.id);
        }
        if rows.is_empty() {
            g.text(list.x + 4.0 * s, list.y + 4.0 * s, 11.5 * s, DIM, "no runs match", Face::Sans);
        }
        let _ = AMBER;
    }
}
