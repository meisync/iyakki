//! FLOWS, the RUN tab: the graph of the run lit by the state of each step (moving dashes on the edges that carry work, attempt
//! counters like "2/4 · next in 400 ms"), a timing waterfall, the selected step's output, input, log and attempts as selectable
//! text, and the fix card the assistant made when a step failed for good.

use std::collections::BTreeMap;
use std::sync::Arc;

use iyakki_flow::host::Host;
use iyakki_flow::model::*;

use super::flows::*;
use super::flows_view::GfxFit;
use super::flows_canvas::*;
use super::flows_paint::{self as paint, Paint, RunPaint};
use super::flows_view::{ago_text, dur_text};
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) fn run_paint(run: &Run, now_ms: i64) -> RunPaint {
    let mut rp = RunPaint { now_ms, ..Default::default() };
    for (id, nr) in &run.nodes {
        rp.node_state.insert(id.clone(), nr.state);
        rp.attempts.insert(id.clone(), (nr.attempts.len() as u32, nr.max_attempts, nr.next_retry_ms));
    }
    rp.edges = run.edges.clone();
    rp
}

fn pretty(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Null => vec!["(nothing)".into()],
        other => serde_json::to_string_pretty(other).unwrap_or_default().lines().take(400).map(str::to_string).collect(),
    }
}

impl FlowsApp {
    /// What the detail text shows for the selected step (or the whole run), as coloured lines.
    fn run_lines(&self) -> Vec<(String, [u8; 4])> {
        let Some(run) = &self.run else { return vec![] };
        if self.rv.tab == RunTab::Report {
            return report_lines(&iyakki_flow::report::build(run));
        }
        if self.rv.tab == RunTab::Pictures {
            return vec![];
        }
        let mut out: Vec<(String, [u8; 4])> = Vec::new();
        let put = |out: &mut Vec<(String, [u8; 4])>, lines: Vec<String>, c: [u8; 4]| out.extend(lines.into_iter().map(|l| (l, c)));
        match (&self.run_node, self.rv.tab) {
            (Some(n), tab) => {
                let Some(nr) = run.nodes.get(n) else { return out };
                match tab {
                    RunTab::Output => {
                        if !nr.error.is_empty() {
                            put(&mut out, vec![format!("error: {}", nr.error)], ERR);
                        }
                        if !nr.note.is_empty() {
                            put(&mut out, vec![format!("note: {}", nr.note)], MAGENTA);
                        }
                        put(&mut out, pretty(&nr.output), TEXT);
                    }
                    RunTab::Input => put(&mut out, pretty(&nr.input), TEXT),
                    RunTab::Pictures | RunTab::Report => {}
                    RunTab::Log => {
                        let lines = if nr.log.is_empty() { vec!["(no log lines for this step)".to_string()] } else { nr.log.clone() };
                        put(&mut out, lines, MUTED);
                    }
                    RunTab::Attempts => {
                        if nr.attempts.is_empty() {
                            put(&mut out, vec!["(no attempts yet)".into()], DIM);
                        }
                        for a in &nr.attempts {
                            let head = format!("attempt {} of {} · limit {} ms{} · took {}", a.n, nr.max_attempts.max(a.n), a.timeout_ms, if a.delay_before_ms > 0 { format!(" · waited {} ms before", a.delay_before_ms) } else { String::new() }, dur_text(a.ended_ms - a.started_ms));
                            put(&mut out, vec![head], if a.ok { OK } else { AMBER });
                            if !a.ok {
                                put(&mut out, vec![format!("   {} ({}): {}", "failed", a.class, a.error)], ERR);
                            }
                            for l in a.logs.iter().take(12) {
                                put(&mut out, vec![format!("   | {l}")], DIM);
                            }
                        }
                    }
                }
            }
            (None, RunTab::Output) => {
                if !run.error.is_empty() {
                    put(&mut out, vec![format!("error: {}", run.error)], ERR);
                }
                put(&mut out, pretty(&run.outputs), TEXT);
            }
            (None, RunTab::Input) => put(&mut out, pretty(&run.inputs), TEXT),
            (None, RunTab::Log) => {
                let lines: Vec<String> = run.log.iter().map(|l| format!("{}{}", if l.node.is_empty() { String::new() } else { format!("[{}] ", l.node) }, l.text)).collect();
                put(&mut out, if lines.is_empty() { vec!["(no log lines)".into()] } else { lines }, MUTED);
            }
            (None, RunTab::Pictures | RunTab::Report) => {}
            (None, RunTab::Attempts) => {
                for p in &run.patches {
                    put(&mut out, vec![format!("{} fixed {}{}: {}", p.by, p.node, if p.simple { " (simple)" } else { "" }, p.summary)], MAGENTA);
                }
                if run.patches.is_empty() {
                    put(&mut out, vec!["No fixes were applied in this run.".into()], DIM);
                }
            }
        }
        out
    }

    fn fill_run_log(&mut self) {
        let Some(run) = &self.run else { return };
        let key = (run.id.clone(), self.run_node.clone().unwrap_or_default(), self.rv.tab as u8, run.rev);
        if self.rv.shown == key {
            return;
        }
        let lines = self.run_lines();
        let was_end = self.rv.log.scroll == 0;
        self.rv.log.lines = lines;
        if !was_end {
            self.rv.log.scroll = self.rv.log.scroll.min(self.rv.log.lines.len());
        }
        self.rv.shown = key;
    }

    /// The card that needs the person (or the latest one) for the run on screen.
    pub(super) fn open_card(&self) -> Option<HealCard> {
        let run = self.run.as_ref()?;
        run.heal.iter().find(|c| c.state == CardState::Proposed).or_else(|| run.heal.last()).cloned()
    }

    pub(super) fn draw_run(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let pad = 10.0 * s;
        let status_h = 24.0 * s;
        let inner = R::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - status_h - 4.0 * s);
        let Some(run) = self.run.clone() else {
            fx::empty_state(g, inner, s, Icon::Runs, "No run to show", "Start the flow with Run in DESIGN, wait for its schedule, or ask the assistant to run it. The newest run appears here by itself.", ACCENT);
            self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
            return;
        };
        let now = self.now();
        // the run's header line
        let st_color = match run.status {
            RunStatus::Succeeded => OK,
            RunStatus::Failed => ERR,
            RunStatus::Blocked => MAGENTA,
            RunStatus::Retrying | RunStatus::Healing => AMBER,
            RunStatus::Cancelled | RunStatus::Scheduled | RunStatus::Queued => MUTED,
            RunStatus::Running => CYAN,
        };
        let head_h = 24.0 * s;
        let mut hx = inner.x;
        hx += fx::chip(g, hx, inner.y, s, run.status.label(), st_color, true) + 8.0 * s;
        if let Some(b) = self.env.host.browser_session(&run.id) {
            // a browser session is open for this run: say so, with what it has done
            let w = fx::chip(g, hx, inner.y, s, &format!("Browser session · {} actions · {}", b.actions, if b.profile == "person" { "your profile" } else { "private" }), CYAN, true);
            g.hit(R::new(hx, inner.y, w, 20.0 * s), Event::Tab(id::R_TAB_PICS), "Browser session");
            hx += w + 8.0 * s;
        }
        let line = format!("{} · {} v{} · {} · started {} · took {}{}", run.id, run.flow.name, run.version(), run.trigger.kind, if run.started_ms == 0 { "not yet".to_string() } else { ago_text(now - run.started_ms) }, dur_text(run.duration_ms(now)), if run.blocked.is_empty() { String::new() } else { format!(" · {}", run.blocked) });
        let t = g.fit(12.0 * s, &line, Face::Sans, inner.w - (hx - inner.x));
        g.text(hx, inner.y + 3.0 * s, 12.0 * s, if run.blocked.is_empty() { MUTED } else { MAGENTA }, &t, Face::Sans);
        let top = inner.y + head_h + 4.0 * s;
        let avail = R::new(inner.x, top, inner.w, inner.y + inner.h - top);
        // columns: runs (wide windows), graph + waterfall, detail
        let wide = avail.w >= 1000.0 * s;
        let stacked = avail.w < 760.0 * s;
        let list_w = if wide { 190.0 * s } else { 0.0 };
        let detail_w = if stacked { 0.0 } else { (avail.w * 0.34).clamp(300.0 * s, 460.0 * s) };
        let gap = 8.0 * s;
        let mid_x = avail.x + if list_w > 0.0 { list_w + gap } else { 0.0 };
        let mid_w = avail.w - if list_w > 0.0 { list_w + gap } else { 0.0 } - if detail_w > 0.0 { detail_w + gap } else { 0.0 };
        if list_w > 0.0 {
            self.draw_run_list(g, R::new(avail.x, avail.y, list_w, avail.h), s);
        }
        let (mid, detail) = if stacked {
            let gh = (avail.h * 0.5).max(120.0 * s);
            (R::new(avail.x, avail.y, avail.w, gh), R::new(avail.x, avail.y + gh + gap, avail.w, avail.h - gh - gap))
        } else {
            (R::new(mid_x, avail.y, mid_w, avail.h), R::new(mid_x + mid_w + gap, avail.y, detail_w, avail.h))
        };
        let wf_h = if mid.h >= 320.0 * s { (run.nodes.len() as f32 * 15.0 * s + 26.0 * s).min(mid.h * 0.34).max(60.0 * s) } else { 0.0 };
        let canvas = R::new(mid.x, mid.y, mid.w, mid.h - if wf_h > 0.0 { wf_h + gap } else { 0.0 });
        self.geo.canvas = canvas;
        if self.rv.fitted_for != run.id && canvas.w > 10.0 {
            self.rv.view = View::fit(&run.flow, canvas);
            self.rv.fitted_for = run.id.clone();
        }
        let rp = run_paint(&run, now);
        let sel: std::collections::BTreeSet<String> = self.run_node.iter().cloned().collect();
        let hover = self.hover.clone();
        let issues = BTreeMap::new();
        {
            let p = Paint { flow: &run.flow, reg: &self.reg, view: self.rv.view, rect: canvas, selected: &sel, sel_edge: None, hover: &hover, issues: &issues, run: Some(&rp), connecting: None, rubber: None, tick, s };
            paint::draw_graph(g, &p);
        }
        // the zoom buttons
        let zb = 24.0 * s;
        let zx = canvas.x + canvas.w - zb - 8.0 * s;
        let mut zy = canvas.y + 8.0 * s;
        for (label, ev) in [("+", id::ZOOM_IN), ("-", id::ZOOM_OUT), ("fit", id::FIT)] {
            let rr = R::new(zx, zy, zb, zb);
            let hot = g.hovered(rr);
            g.rrect(rr.x, rr.y, rr.w, rr.h, zb / 2.0, if hot { fx::mix(fx::WELL, CYAN, 0.25) } else { with_alpha(fx::PANEL, 230) });
            g.rrect_outline(rr.x, rr.y, rr.w, rr.h, zb / 2.0, 1.0, with_alpha(CYAN, 130));
            let ts = if label == "fit" { 10.0 * s } else { 14.0 * s };
            let tw = g.text_width(ts, label, Face::Bold);
            g.text(rr.x + (zb - tw) / 2.0, rr.y + (zb - g.line_height(ts, Face::Bold)) / 2.0, ts, TEXT, label, Face::Bold);
            g.hit(rr, Event::Tab(ev), label);
            zy += zb + 6.0 * s;
        }
        if canvas.w >= 420.0 * s && canvas.h >= 220.0 * s {
            let mini = R::new(canvas.x + 8.0 * s, canvas.y + canvas.h - 80.0 * s - 8.0 * s, 124.0 * s, 80.0 * s);
            self.geo.minimap = Some(mini);
            paint::draw_minimap(g, &run.flow, &self.rv.view, canvas, mini, s, Some(&rp.node_state));
        }
        if wf_h > 0.0 {
            self.draw_waterfall(g, R::new(mid.x, canvas.y + canvas.h + gap, mid.w, wf_h), s, &run, now);
        }
        self.geo.props = Some(detail);
        self.draw_run_detail(g, detail, s, &run, tick);
        self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
    }

    fn draw_run_list(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let now = self.now();
        let pad = 6.0 * s;
        let row_h = 44.0 * s;
        let runs: Vec<RunSummary> = self.runs.iter().filter(|x| self.cur.as_ref().is_none_or(|c| *c == x.flow_id)).cloned().collect();
        let vis = (((r.h - 2.0 * pad) / row_h).floor() as usize).max(1);
        let cur = runs.iter().position(|x| Some(&x.id) == self.run_id.as_ref()).unwrap_or(0);
        let first = cur.saturating_sub(vis - 1).min(runs.len().saturating_sub(vis));
        if runs.is_empty() {
            g.text(r.x + 10.0 * s, r.y + 10.0 * s, 11.5 * s, DIM, "no runs yet", Face::Sans);
        }
        for (i, x) in runs.iter().enumerate().skip(first).take(vis) {
            let rr = R::new(r.x + pad, r.y + pad + (i - first) as f32 * row_h, r.w - 2.0 * pad, row_h - 4.0 * s);
            let hot = g.hovered(rr);
            fx::list_row(g, rr, s, Some(&x.id) == self.run_id.as_ref(), hot, ACCENT);
            let c = status_color(x.status);
            g.circle(rr.x + 10.0 * s, rr.y + 12.0 * s, 4.0 * s, c);
            g.text_fit(rr.x + 20.0 * s, rr.y + 4.0 * s, 12.0 * s, TEXT, &format!("{} · {}", x.id, x.status.label()), Face::Bold, rr.w - 26.0 * s);
            g.text_fit(rr.x + 20.0 * s, rr.y + 21.0 * s, 10.5 * s, DIM, &format!("{} v{} · {}", if self.cur.is_some() { x.trigger.clone() } else { x.flow_name.clone() }, x.version, ago_text(now - x.created_ms)), Face::Sans, rr.w - 26.0 * s);
            g.hit(rr, Event::Row(i), &x.id);
        }
    }

    fn draw_waterfall(&mut self, g: &mut Gfx, r: R, s: f32, run: &Run, now: i64) {
        fx::panel(g, r, s, ACCENT);
        let t0 = if run.started_ms > 0 { run.started_ms } else { now };
        let t1 = if run.ended_ms > 0 { run.ended_ms } else { now }.max(t0 + 1);
        let label_w = 96.0 * s;
        let pad = 8.0 * s;
        let inner = R::new(r.x + pad, r.y + 6.0 * s, r.w - 2.0 * pad, r.h - 12.0 * s);
        let bar = R::new(inner.x + label_w, inner.y + 14.0 * s, inner.w - label_w, inner.h - 14.0 * s);
        g.text(inner.x, inner.y, 10.0 * s, DIM, &format!("timing · {}", dur_text(t1 - t0)), Face::Bold);
        let started: Vec<(&String, &NodeRun)> = run.flow.nodes.iter().filter_map(|n| run.nodes.get(&n.id).map(|nr| (&n.id, nr))).filter(|(_, nr)| nr.started_ms > 0).collect();
        let rowh = (bar.h / started.len().max(1) as f32).clamp(8.0 * s, 16.0 * s);
        let mut rows = Vec::new();
        g.clipped(inner, |g| {
            for (i, (id_, nr)) in started.iter().enumerate() {
                let y = bar.y + i as f32 * rowh;
                if y + rowh > inner.y + inner.h + 1.0 {
                    break;
                }
                let sel = self.run_node.as_deref() == Some(id_.as_str());
                if sel {
                    g.rect(inner.x, y, inner.w, rowh, with_alpha(ACCENT, 40));
                }
                g.text_fit(inner.x, y, (11.0 * s).max(10.0).min((rowh - 2.0).max(10.0)), if sel { TEXT } else { MUTED }, id_, Face::Sans, label_w - 6.0 * s);
                let end = if nr.ended_ms > 0 { nr.ended_ms } else { now };
                let span = (t1 - t0) as f32;
                let (x0, x1) = (bar.x + (nr.started_ms - t0) as f32 / span * bar.w, bar.x + (end - t0) as f32 / span * bar.w);
                let c = paint::state_color(nr.state);
                g.rrect(x0, y + 2.0, (x1 - x0).max(3.0), rowh - 4.0, 3.0, with_alpha(c, 90));
                // attempts inside the bar: a gap is the pause before a retry
                for a in &nr.attempts {
                    let (a0, a1) = (bar.x + (a.started_ms - t0) as f32 / span * bar.w, bar.x + ((if a.ended_ms > 0 { a.ended_ms } else { now }) - t0) as f32 / span * bar.w);
                    g.rrect(a0, y + 2.0, (a1 - a0).max(2.0), rowh - 4.0, 3.0, if a.ok || a.ended_ms == 0 { c } else { with_alpha(AMBER, 230) });
                }
                rows.push(((*id_).clone(), nr.started_ms, end));
            }
        });
        g.hit(R::new(inner.x, bar.y, inner.w, inner.h - 14.0 * s), Event::Area(area::WATERFALL), "timing");
        self.geo.waterfall = Some((R::new(inner.x, bar.y, inner.w, inner.h - 14.0 * s), t0, t1, rows));
    }

    fn draw_run_detail(&mut self, g: &mut Gfx, r: R, s: f32, run: &Run, tick: u64) {
        fx::panel(g, r, s, ACCENT);
        let pad = 10.0 * s;
        let inner = R::new(r.x + pad, r.y + 8.0 * s, r.w - 2.0 * pad, r.h - 16.0 * s);
        let mut y = inner.y;
        // the step
        let title = match &self.run_node {
            Some(n) => {
                let nr = run.nodes.get(n);
                format!("{}{}", run.flow.node(n).map(|x| x.title().to_string()).unwrap_or_else(|| n.clone()), nr.map(|x| format!(" · {}{}", x.state.label(), if x.max_attempts > 1 { format!(" · {}/{}", x.attempts.len(), x.max_attempts) } else { String::new() })).unwrap_or_default())
            }
            None => "Whole run (click a step)".to_string(),
        };
        g.text_fit(inner.x, y, 14.0 * s, TEXT, &title, Face::Bold, inner.w);
        y += 22.0 * s;
        // the fix card
        if let Some(card) = self.open_card() {
            let open = card.state == CardState::Proposed;
            let h = self.draw_card(g, R::new(inner.x, y, inner.w, inner.h - (y - inner.y)), s, &card, open);
            y += h + 8.0 * s;
        }
        // tabs
        let mut x = inner.x;
        let mut tabs: Vec<(&str, RunTab, usize)> = vec![("Output", RunTab::Output, id::R_TAB), ("Input", RunTab::Input, id::R_TAB + 1), ("Log", RunTab::Log, id::R_TAB + 2), ("Attempts", RunTab::Attempts, id::R_TAB + 3)];
        let browser = self.run_has_browser();
        if browser {
            tabs.push(("Pictures", RunTab::Pictures, id::R_TAB_PICS));
            tabs.push(("Report", RunTab::Report, id::R_TAB_REPORT));
        } else if matches!(self.rv.tab, RunTab::Pictures | RunTab::Report) {
            self.rv.tab = RunTab::Output;
        }
        let size = 11.5 * s;
        let mut tab_y = y;
        for (label, t, ev) in tabs.iter() {
            let w = g.text_width(size, label, Face::Bold) + size * 1.6;
            if x + w > inner.x + inner.w && x > inner.x {
                x = inner.x;
                tab_y += size * 2.1 + 4.0 * s;
            }
            let on = self.rv.tab == *t;
            g.button(x, tab_y, size, label, if on { ACCENT } else { MUTED }, Event::Tab(*ev));
            x += w + 5.0 * s;
        }
        y = tab_y + size * 2.1 + 8.0 * s;
        let mut text_r = R::new(inner.x, y, inner.w, (inner.y + inner.h - y).max(20.0));
        if self.rv.tab == RunTab::Pictures {
            self.draw_pictures(g, text_r, s, run);
            return;
        }
        if self.rv.tab == RunTab::Report {
            // the report can be saved as one web page (pictures inside) or as data
            let mut bx = text_r.x;
            bx += g.button(bx, text_r.y, 11.5 * s, "Export HTML", CYAN, Event::Tab(id::R_EXPORT_HTML)) + 6.0 * s;
            g.button(bx, text_r.y, 11.5 * s, "Export JSON", CYAN, Event::Tab(id::R_EXPORT_JSON));
            let used = 11.5 * s * 2.1 + 8.0 * s;
            text_r = R::new(text_r.x, text_r.y + used, text_r.w, (text_r.h - used).max(20.0));
        }
        self.fill_run_log();
        self.rv.log.nowrap = false;
        self.rv.log.render(g, text_r, 11.5 * s, Face::Mono);
        let _ = tick;
    }

    pub(super) fn run_has_browser(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.flow.nodes.iter().any(|n| n.kind.starts_with("br_")))
    }

    /// The pictures the run's steps made, in step order: (step, file).
    fn run_pictures(&self) -> Vec<(String, String)> {
        let Some(run) = &self.run else { return vec![] };
        let mut v = Vec::new();
        for n in &run.flow.nodes {
            if let Some(nr) = run.nodes.get(&n.id) {
                for a in nr.artifacts.iter().filter(|a| a.ends_with(".png")) {
                    v.push((n.id.clone(), a.clone()));
                }
            }
        }
        v
    }

    /// Only files in the program's own picture folder are read for the screen and the report.
    pub(super) fn own_picture(&self, path: &str) -> bool {
        let root = std::fs::canonicalize(self.env.data.join("artifacts")).ok();
        let real = std::fs::canonicalize(path).ok();
        matches!((root, real), (Some(r), Some(p)) if p.starts_with(&r))
    }

    fn load_pic(&mut self, path: &str) -> Option<Arc<tiny_skia::Pixmap>> {
        if let Some(p) = self.rv.pics.get(path) {
            return p.clone();
        }
        if self.rv.pics.len() > 24 {
            self.rv.pics.clear();
        }
        let pm = if self.own_picture(path) { std::fs::read(path).ok().filter(|b| b.len() < 16 << 20).and_then(|b| super::browser_images::to_pixmap(&b).ok()).map(Arc::new) } else { None };
        self.rv.pics.insert(path.to_string(), pm.clone());
        pm
    }

    /// The live picture of the run's browser session, read again about once a second while the session is open.
    fn live_preview(&mut self, run_id: &str) -> Option<(Arc<tiny_skia::Pixmap>, String)> {
        let info = self.env.host.browser_session(run_id)?;
        let path = info.preview?;
        let now = self.now();
        let stale = self.rv.preview.as_ref().is_none_or(|(t, p, _)| *p != path || now - *t > 900);
        if stale {
            let pm = if self.own_picture(&path) { std::fs::read(&path).ok().and_then(|b| super::browser_images::to_pixmap(&b).ok()).map(Arc::new) } else { None };
            self.rv.preview = Some((now, path.clone(), pm));
        }
        let pm = self.rv.preview.as_ref().and_then(|p| p.2.clone())?;
        Some((pm, info.url))
    }

    fn draw_pictures(&mut self, g: &mut Gfx, r: R, s: f32, run: &Run) {
        let pics = self.run_pictures();
        let mut y = r.y;
        let live = self.live_preview(&run.id);
        let big_h = (r.h * 0.5).max(60.0 * s);
        if let Some((pm, url)) = &live {
            g.text_fit(r.x, y, 11.0 * s, CYAN, &format!("Browser session · live · {url}"), Face::Bold, r.w);
            y += 16.0 * s;
            let used = blit(g, pm, R::new(r.x, y, r.w, big_h - 16.0 * s));
            g.rrect_outline(used.x, used.y, used.w, used.h, 2.0, 1.0, with_alpha(CYAN, 150));
            y += used.h + 8.0 * s;
        } else if let Some((node, path)) = pics.get(self.rv.pic_sel.min(pics.len().saturating_sub(1))).cloned() {
            self.rv.pic_sel = self.rv.pic_sel.min(pics.len().saturating_sub(1));
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            g.text_fit(r.x, y, 11.0 * s, MUTED, &format!("{node} · {name}"), Face::Bold, r.w);
            y += 16.0 * s;
            match self.load_pic(&path) {
                Some(pm) => {
                    let used = blit(g, &pm, R::new(r.x, y, r.w, big_h - 16.0 * s));
                    g.rrect_outline(used.x, used.y, used.w, used.h, 2.0, 1.0, with_alpha(ACCENT, 150));
                    y += used.h + 8.0 * s;
                }
                None => {
                    g.text_fit(r.x, y, 11.5 * s, ERR, "this picture cannot be shown (the file is gone, or it is not in the run's folder)", Face::Sans, r.w);
                    y += 20.0 * s;
                }
            }
        } else {
            for l in g.wrap(11.5 * s, "No pictures yet. A Screenshot or Compare to baseline step makes one; a running browser session shows live here.", Face::Sans, r.w).into_iter().take(4) {
                g.text(r.x, y, 11.5 * s, DIM, &l, Face::Sans);
                y += 15.0 * s;
            }
            return;
        }
        // the thumbnails
        let cell_w = 104.0 * s;
        let cell_h = 72.0 * s;
        let per_row = ((r.w / (cell_w + 6.0 * s)).floor() as usize).max(1);
        g.clipped(R::new(r.x, y, r.w, (r.y + r.h - y).max(0.0)), |g| {
            for (i, (node, path)) in pics.iter().enumerate() {
                let (col, row) = (i % per_row, i / per_row);
                let cell = R::new(r.x + col as f32 * (cell_w + 6.0 * s), y + row as f32 * (cell_h + 18.0 * s), cell_w, cell_h);
                if cell.y + cell.h > r.y + r.h {
                    break;
                }
                let on = i == self.rv.pic_sel && live.is_none();
                g.rrect(cell.x, cell.y, cell.w, cell.h, 4.0 * s, fx::WELL);
                if let Some(pm) = self.rv.pics.get(path).and_then(|p| p.clone()) {
                    blit(g, &pm, R::new(cell.x + 2.0, cell.y + 2.0, cell.w - 4.0, cell.h - 4.0));
                }
                g.rrect_outline(cell.x, cell.y, cell.w, cell.h, 4.0 * s, if on { 2.0 } else { 1.0 }, if on { ACCENT } else { with_alpha(MUTED, 90) });
                g.text_fit(cell.x, cell.y + cell.h + 2.0 * s, 10.0 * s, if on { TEXT } else { DIM }, node, Face::Sans, cell.w);
                g.hit(cell, Event::Tab(id::R_PIC + i), &format!("{node}: {path}"));
            }
        });
        // load the thumbnails that are not loaded yet (a few per frame, so a big run does not stall the screen)
        let todo: Vec<String> = pics.iter().map(|p| p.1.clone()).filter(|p| !self.rv.pics.contains_key(p)).take(3).collect();
        for p in todo {
            self.load_pic(&p);
        }
    }

    /// Save the QA report: one web page with the pictures inside, or the data (with the picture files named).
    pub(super) fn export_report(&mut self, html: bool) {
        let Some(run) = self.run.clone() else { return };
        let rep = iyakki_flow::report::build(&run);
        let stem = format!("{}-{}-report", run.flow_id, run.id);
        if html {
            let root = std::fs::canonicalize(self.env.data.join("artifacts")).ok();
            let page = iyakki_flow::report::to_html(&rep, &|p| {
                let real = std::fs::canonicalize(p).ok()?;
                real.starts_with(root.as_ref()?).then(|| std::fs::read(real).ok()).flatten()
            });
            self.host_save = Some((format!("{stem}.html"), page.into_bytes()));
        } else {
            self.host_save = Some((format!("{stem}.json"), iyakki_flow::report::to_json(&rep).into_bytes()));
        }
        self.say("Choose where to save the report.", true);
    }

    /// The fix card; returns its height.
    fn draw_card(&mut self, g: &mut Gfx, r: R, s: f32, card: &HealCard, open: bool) -> f32 {
        let w = r.w - 20.0 * s;
        let mut lines: Vec<(String, [u8; 4], f32)> = Vec::new();
        let state = match card.state {
            CardState::Proposed if card.patch.is_none() && card.note == "investigate" => "The assistant has not looked yet".to_string(),
            CardState::Proposed if card.patch.is_none() => "The assistant could not fix this by itself".to_string(),
            CardState::Proposed => "The assistant proposes a fix".to_string(),
            CardState::Investigating => "The assistant is looking at it...".to_string(),
            CardState::Applied => "Fix applied, trying it".to_string(),
            CardState::Verified => "Fix verified".to_string(),
            CardState::Rejected => "Fix rejected".to_string(),
            CardState::Failed => "The fix did not work".to_string(),
        };
        lines.push((format!("{state} · {}", card.node), MAGENTA, 12.5));
        for l in g.wrap(11.5 * s, &card.diagnosis, Face::Sans, w).into_iter().take(4) {
            lines.push((l, TEXT, 11.5));
        }
        if let Some(p) = &card.patch {
            lines.push((format!("change: {}", p.summary()), CYAN, 11.5));
            if let Some(c) = &card.class {
                let tag = match c.class {
                    iyakki_flow::heal::Class::Simple => "simple fix".to_string(),
                    iyakki_flow::heal::Class::NeedsApproval => format!("needs you: {}", c.reasons.join("; ")),
                    iyakki_flow::heal::Class::Refused => format!("refused: {}", c.reasons.join("; ")),
                };
                for l in g.wrap(11.0 * s, &tag, Face::Sans, w).into_iter().take(2) {
                    lines.push((l, if c.is_simple() { OK } else { AMBER }, 11.0));
                }
            }
        }
        if !card.decided_by.is_empty() {
            lines.push((format!("decided by {}{}", card.decided_by, if card.draft_rev > 0 { format!(" · draft r{}", card.draft_rev) } else { String::new() }), DIM, 10.5));
        }
        let btn_h = if open { 34.0 * s } else { 0.0 };
        let h = lines.iter().map(|l| l.2 * 1.3 * s).sum::<f32>() + btn_h + 14.0 * s;
        let h = h.min(r.h.max(60.0 * s) * 0.7);
        g.rrect(r.x, r.y, r.w, h, 10.0 * s, with_alpha(MAGENTA, 26));
        g.rrect_outline(r.x, r.y, r.w, h, 10.0 * s, 1.2, with_alpha(MAGENTA, 160));
        let mut y = r.y + 6.0 * s;
        for (t, c, sz) in &lines {
            if y + sz * 1.3 * s > r.y + h - btn_h {
                break;
            }
            g.text_fit(r.x + 10.0 * s, y, sz * s, *c, t, if *sz > 12.0 { Face::Bold } else { Face::Sans }, w);
            y += sz * 1.3 * s;
        }
        if open {
            let by = r.y + h - btn_h + 2.0 * s;
            if card.patch.is_some() {
                let mut x = r.x + 10.0 * s;
                x += g.button(x, by, 11.5 * s, "Approve", OK, Event::Tab(id::CARD_APPROVE)) + 6.0 * s;
                x += g.button(x, by, 11.5 * s, "Edit", CYAN, Event::Tab(id::CARD_EDIT)) + 6.0 * s;
                g.button(x, by, 11.5 * s, "Reject", ERR, Event::Tab(id::CARD_REJECT));
            } else {
                let mut x = r.x + 10.0 * s;
                if card.note == "investigate" {
                    x += g.button(x, by, 11.5 * s, "Ask the assistant", MAGENTA, Event::Tab(id::CARD_ASK)) + 6.0 * s;
                }
                g.button(x, by, 11.5 * s, "Dismiss", MUTED, Event::Tab(id::CARD_REJECT));
            }
        }
        h
    }
}

pub(super) fn status_color(st: RunStatus) -> [u8; 4] {
    match st {
        RunStatus::Succeeded => OK,
        RunStatus::Failed => ERR,
        RunStatus::Blocked => MAGENTA,
        RunStatus::Retrying | RunStatus::Healing => AMBER,
        RunStatus::Running => CYAN,
        RunStatus::Scheduled | RunStatus::Queued => rgb_blue(),
        RunStatus::Cancelled => MUTED,
    }
}

fn rgb_blue() -> [u8; 4] {
    crate::gfx::rgb(120, 170, 255)
}

/// Draw `pm` as large as fits in `r` (keeping its shape), centred across the width; returns where it went.
fn blit(g: &mut Gfx, pm: &tiny_skia::Pixmap, r: R) -> R {
    let k = (r.w / pm.width() as f32).min(r.h / pm.height() as f32).max(0.01);
    let (w, h) = (pm.width() as f32 * k, pm.height() as f32 * k);
    let (x, y) = (r.x + (r.w - w) / 2.0, r.y);
    let paint = tiny_skia::PixmapPaint { opacity: 1.0, quality: tiny_skia::FilterQuality::Bilinear, ..tiny_skia::PixmapPaint::default() };
    g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_translate(x, y).pre_scale(k, k), None);
    R::new(x, y, w, h)
}

/// The report as coloured lines for the text view.
fn report_lines(rep: &iyakki_flow::report::QaReport) -> Vec<(String, [u8; 4])> {
    let mut out: Vec<(String, [u8; 4])> = Vec::new();
    let c = if rep.passed { OK } else if rep.status == "failed" || rep.summary.failed > 0 || rep.summary.checks_failed > 0 { ERR } else { AMBER };
    out.push((format!("QA report: {} · run {} · {}", if rep.passed { "PASSED" } else if c == ERR { "FAILED" } else { "NOT FINISHED" }, rep.run_id, dur_text(rep.duration_ms)), c));
    let s = &rep.summary;
    out.push((format!("{} steps: {} done, {} failed, {} skipped · {} checks, {} failed ({} soft) · {} pictures · {} console errors", s.nodes, s.succeeded, s.failed, s.skipped, s.checks, s.checks_failed, s.soft_failed, s.screenshots, s.console_errors), MUTED));
    out.push((String::new(), MUTED));
    for n in &rep.nodes {
        let col = match n.state.as_str() {
            "succeeded" => OK,
            "failed" => ERR,
            "skipped" | "cancelled" => DIM,
            _ => AMBER,
        };
        out.push((format!("[{}] {} ({}) {}", n.state, n.title, n.kind, dur_text(n.duration_ms)), col));
        if let Some(a) = &n.assertion {
            let ok = a["passed"].as_bool() == Some(true);
            out.push((format!("    check {}: {}{}", if ok { "passed" } else { "FAILED" }, a["message"].as_str().unwrap_or(""), if a["soft"].as_bool() == Some(true) { " (soft)" } else { "" }), if ok { OK } else { ERR }));
        }
        if !n.error.is_empty() {
            out.push((format!("    {}", n.error), ERR));
        }
        if let Some(d) = &n.dom {
            out.push((format!("    page {} · {}", d["url"].as_str().unwrap_or(""), d["title"].as_str().unwrap_or("")), MUTED));
            for l in d["snippet"].as_str().unwrap_or("").lines().take(6) {
                out.push((format!("    | {l}"), DIM));
            }
        }
        for e in &n.console_errors {
            out.push((format!("    console {e}"), ERR));
        }
        for p in &n.screenshots {
            out.push((format!("    picture {}", p.rsplit('/').next().unwrap_or(p)), CYAN));
        }
    }
    out.push((String::new(), MUTED));
    out.push((rep.note.clone(), DIM));
    out
}
