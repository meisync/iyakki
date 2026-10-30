//! FLOWS, drawing the shared parts and the DESIGN tab: the top bar, the flows list, the designer (palette, canvas, properties,
//! minimap), the list editor for small windows, the text view, the status line, the dialogs and the assistant pane.

use crate::safeclamp::SafeClamp;
use iyakki_flow::kinds::{KindStatus, Level};
use iyakki_flow::model::*;

use super::component::route;
use super::flows::*;
use super::flows_canvas::*;
use super::flows_paint::{self as paint, Paint};
use super::{App, AppCx};
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx, Icon};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

/// Text drawn cut to a width (the cut text is measured before it is drawn).
pub(super) trait GfxFit {
    #[allow(clippy::too_many_arguments)]
    fn text_fit(&mut self, x: f32, y: f32, size: f32, color: [u8; 4], text: &str, face: Face, max_w: f32) -> f32;
}

impl GfxFit for Gfx {
    fn text_fit(&mut self, x: f32, y: f32, size: f32, color: [u8; 4], text: &str, face: Face, max_w: f32) -> f32 {
        let t = self.fit(size, text, face, max_w);
        self.text(x, y, size, color, &t, face)
    }
}

/// Places items left to right and wraps to a new line when one would pass the right edge, so nothing overlaps.
pub(super) struct Flowing {
    pub x0: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub gap: f32,
    pub h: f32,
}

impl Flowing {
    pub(super) fn new(r: R, h: f32, gap: f32) -> Flowing {
        Flowing { x0: r.x, x: r.x, y: r.y, w: r.w, gap, h }
    }

    pub(super) fn place(&mut self, w: f32) -> (f32, f32) {
        if self.x + w > self.x0 + self.w && self.x > self.x0 {
            self.x = self.x0;
            self.y += self.h + self.gap;
        }
        let p = (self.x, self.y);
        self.x += w + self.gap;
        p
    }

    pub(super) fn button(&mut self, g: &mut Gfx, size: f32, label: &str, accent: [u8; 4], ev: Event) {
        let w = g.text_width(size, label, Face::Bold) + size * 1.7;
        let (x, y) = self.place(w);
        g.button(x, y, size, label, accent, ev);
    }

    pub(super) fn chip(&mut self, g: &mut Gfx, s: f32, label: &str, accent: [u8; 4], filled: bool) {
        let w = g.text_width(fx::size::CAPTION * s, label, Face::Bold) + fx::size::CAPTION * s * 1.4;
        let (x, y) = self.place(w);
        fx::chip(g, x, y + (self.h - fx::size::CAPTION * s * 1.9) / 2.0, s, label, accent, filled);
    }

    pub(super) fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

pub(super) fn ago_text(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=1 => "just now".into(),
        2..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        86_400.. => format!("{}d ago", s / 86_400),
        _ => format!("{}h ago", s / 3600),
    }
}

pub(super) fn dur_text(ms: i64) -> String {
    let ms = ms.max(0);
    match ms {
        0..=999 => format!("{ms} ms"),
        1000..=59_999 => format!("{:.1} s", ms as f64 / 1000.0),
        60_000..=3_599_999 => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1000),
        _ => format!("{}h {}m", ms / 3_600_000, ms % 3_600_000 / 60_000),
    }
}

pub(super) fn when_text(ms: i64, now: i64) -> String {
    if ms > now {
        let d = (ms - now) / 1000;
        match d {
            0..=59 => format!("in {d}s"),
            60..=3599 => format!("in {}m", d / 60),
            _ => format!("in {}h {}m", d / 3600, d % 3600 / 60),
        }
    } else {
        ago_text(now - ms)
    }
}

impl FlowsApp {
    /// (main area, chat pane, whether the chat covers everything): beside the flow in a wide window, a toggle that takes the whole
    /// area in a small one (800x480).
    pub(super) fn lay(&self, area: R, s: f32) -> (R, Option<R>, bool) {
        if !self.chat_open {
            return (area, None, false);
        }
        if area.w >= 1000.0 && area.w / s >= 980.0 {
            let cw = (area.w * 0.32).clamp_safe(340.0 * s, 520.0 * s);
            (R::new(area.x, area.y, area.w - cw, area.h), Some(R::new(area.x + area.w - cw, area.y, cw, area.h)), false)
        } else {
            (area, Some(area), true)
        }
    }

    pub(super) fn render_impl(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.poll();
        self.geo.area = Some(area);
        self.geo.rows.clear();
        self.geo.props = None;
        self.geo.palette = None;
        self.geo.minimap = None;
        self.geo.waterfall = None;
        let (main, chat, overlay) = self.lay(area, s);
        self.geo.chat = chat;
        self.geo.main = main;
        if !overlay {
            let top = self.draw_top(g, main, s);
            let body = R::new(main.x, main.y + top, main.w, (main.h - top).max(0.0));
            match self.tab {
                Tab::Design => {
                    if self.cur.is_none() {
                        self.draw_flow_list(g, body, s);
                    } else if Self::is_small(body, s) {
                        self.draw_list_editor(g, body, s, tick);
                    } else {
                        self.draw_designer(g, body, s, tick);
                    }
                }
                Tab::Run => self.draw_run(g, body, s, tick),
                Tab::Board => self.draw_board(g, body, s, tick),
            }
        }
        if let Some(c) = chat {
            self.draw_chat(g, c, s, cx, tick);
        }
        self.draw_modal(g, area, s, tick);
        self.tip_rect = self.tip.draw(g, s);
    }

    fn draw_chat(&mut self, g: &mut Gfx, r: R, s: f32, cx: &AppCx, tick: u64) {
        let focused = self.focus == Focus::Chat && self.modal.is_none();
        fx::panel(g, r, s, if focused { MAGENTA } else { ACCENT });
        let head = 28.0 * s;
        g.text(r.x + 12.0 * s, r.y + 7.0 * s, 13.0 * s, TEXT, "Assistant", Face::Bold);
        let aw = g.text_width(13.0 * s, "Assistant ", Face::Bold);
        g.text(r.x + 12.0 * s + aw, r.y + 9.0 * s, 11.0 * s, DIM, "builds and fixes flows", Face::Sans);
        g.button(r.x + r.w - 60.0 * s, r.y + 2.0 * s, 11.0 * s, "Hide", MUTED, Event::Tab(id::CHAT_HIDE));
        fx::accent_line(g, r.x + 12.0 * s, r.y + head - 2.0, r.w - 24.0 * s, 1.5, if focused { MAGENTA } else { ACCENT }, tick, s);
        g.hit(R::new(r.x, r.y + head, r.w, r.h - head), Event::Tab(id::CHAT_PANE), "chat pane");
        let strip = if self.fail.is_some() { super::flows_fail::draw_offer_strip(g, R::new(r.x + 4.0 * s, r.y + head + 2.0 * s, r.w - 8.0 * s, 0.0), s, self.fail.as_ref().is_some_and(|f| f.needs_person)) + 4.0 * s } else { 0.0 };
        let content = R::new(r.x + 2.0 * s, r.y + head + strip, r.w - 4.0 * s, (r.h - head - strip - 2.0 * s).max(0.0));
        let chat = &mut self.chat;
        super::with_caret(focused, || route::render_slot(g, CHAT_SLOT, |g| App::render(chat, g, content, s, cx, tick)));
    }

    /// The tabs, the flow's name, its version, the counts, then the tab's toolbar. Returns the height used.
    fn draw_top(&mut self, g: &mut Gfx, main: R, s: f32) -> f32 {
        let pad = 10.0 * s;
        let r = R::new(main.x + pad, main.y + 6.0 * s, main.w - 2.0 * pad, main.h);
        let size = 12.5 * s;
        let h = size * 2.1;
        let mut fl = Flowing::new(r, h, 6.0 * s);
        // the tabs are drawn as buttons so they wrap with everything else
        for (i, (label, tab)) in [("DESIGN", Tab::Design), ("RUN", Tab::Run), ("BOARD", Tab::Board)].iter().enumerate() {
            let on = self.tab == *tab;
            let w = g.text_width(size, label, Face::Bold) + size * 1.7 + 14.0 * s;
            let (x, y) = fl.place(w);
            let rr = R::new(x, y, w, h);
            let hot = g.hovered(rr);
            let fill = if on { fx::over(fx::WELL, with_alpha(ACCENT, 60)) } else if hot { crate::ui::CARD_HOVER } else { fx::WELL };
            g.rrect(rr.x, rr.y, rr.w, rr.h, h / 2.0, fill);
            g.rrect_outline(rr.x, rr.y, rr.w, rr.h, h / 2.0, if on { 1.6 * s } else { 1.0 }, if on { ACCENT } else { rgb_dim() });
            let icon = [Icon::Edit, Icon::Runs, Icon::Dash][i];
            g.icon(icon, rr.x + 14.0 * s, rr.y + h / 2.0, 13.0 * s, if on { ACCENT } else { MUTED });
            g.text(rr.x + 24.0 * s, rr.y + (h - g.line_height(size, Face::Bold)) / 2.0, size, if on { TEXT } else { MUTED }, label, Face::Bold);
            g.hit(rr, Event::Tab(id::TAB + i), label);
        }
        // which flow, and in what state
        let name = match &self.cur {
            Some(_) => g.fit(size, &self.flow.name, Face::Bold, 220.0 * s),
            None => "Flows".to_string(),
        };
        fl.button(g, size, &format!("{name}  \u{25be}"), CYAN, Event::Tab(id::FLOWS));
        if self.env.engine.ticking() {
            fl.chip(g, s, "scheduler running", OK, true);
        }
        if self.cur.is_some() {
            let published = self.flows.iter().find(|f| Some(&f.id) == self.cur.as_ref()).map_or(0, |f| f.published);
            let changed = self.cur_changed;
            let agent = self.flow.meta.modified_by.iter().any(Change::by_agent);
            let label = match (published, changed) {
                (0, _) => "draft · not published".to_string(),
                (v, true) => format!("v{v} · draft changed"),
                (v, false) => format!("v{v} · published"),
            };
            fl.chip(g, s, &label, if published == 0 || changed { AMBER } else { OK }, true);
            if agent {
                fl.chip(g, s, "agent changed", MAGENTA, true);
            }
            let (errs, warns) = (self.issues.iter().filter(|i| i.level == Level::Error).count(), self.issues.iter().filter(|i| i.level == Level::Warn).count());
            if errs > 0 {
                fl.chip(g, s, &format!("{errs} problem{}", if errs == 1 { "" } else { "s" }), ERR, true);
            } else if warns > 0 {
                fl.chip(g, s, &format!("{warns} warning{}", if warns == 1 { "" } else { "s" }), AMBER, false);
            }
        }
        let c = self.counts;
        fl.chip(g, s, &format!("running {}", c.running + c.queued), if c.running + c.queued > 0 { CYAN } else { MUTED }, c.running + c.queued > 0);
        fl.chip(g, s, &format!("needs you {}", c.needs_you), if c.needs_you > 0 { MAGENTA } else { MUTED }, c.needs_you > 0);
        fl.chip(g, s, &format!("scheduled {}", c.scheduled), if c.scheduled > 0 { OK } else { MUTED }, false);
        fl.button(g, size, if self.chat_open { "Assistant \u{2713}" } else { "Assistant" }, MAGENTA, Event::Tab(id::CHAT));
        // the tab's own toolbar
        let mut bottom = fl.bottom();
        let mut fl2 = Flowing::new(R::new(r.x, bottom + 6.0 * s, r.w, h), h, 6.0 * s);
        let mut any = false;
        let mut btn = |g: &mut Gfx, fl2: &mut Flowing, label: &str, accent: [u8; 4], idn: usize, on: bool| {
            any = true;
            if on {
                fl2.button(g, size, label, accent, Event::Tab(idn));
            } else {
                let w = g.text_width(size, label, Face::Bold) + size * 1.7;
                let (x, y) = fl2.place(w);
                g.button_off(x, y, size, label);
            }
        };
        let has = self.cur.is_some();
        match self.tab {
            Tab::Design if has => {
                let sel = !self.sel.is_empty() || self.sel_edge.is_some();
                btn(g, &mut fl2, "Close (Esc)", CYAN, id::CLOSE, true);
                btn(g, &mut fl2, "Add step (a)", ACCENT, id::ADD, true);
                btn(g, &mut fl2, "Undo", MUTED, id::UNDO, !self.undo.is_empty());
                btn(g, &mut fl2, "Redo", MUTED, id::REDO, !self.redo.is_empty());
                btn(g, &mut fl2, "Copy", MUTED, id::COPY, !self.sel.is_empty());
                btn(g, &mut fl2, "Paste", MUTED, id::PASTE, !self.clip.nodes.is_empty());
                btn(g, &mut fl2, "Delete (Del)", ERR, id::DELETE, sel);
                btn(g, &mut fl2, "Arrange", MUTED, id::ARRANGE, true);
                btn(g, &mut fl2, "Flow settings (s)", MUTED, id::CARD_SETTINGS, true);
                btn(g, &mut fl2, "Fit", MUTED, id::FIT, true);
                btn(g, &mut fl2, if self.text.is_some() { "Canvas" } else { "Text" }, MUTED, id::TEXT, true);
                btn(g, &mut fl2, "Test step", CYAN, id::TEST, self.sel.len() == 1 && self.testing.is_none());
                btn(g, &mut fl2, "Run (r)", OK, id::RUN, true);
                btn(g, &mut fl2, "Publish (p)", ACCENT, id::PUBLISH, true);
                btn(g, &mut fl2, "History (h)", MUTED, id::HISTORY, true);
                btn(g, &mut fl2, "Export", MUTED, id::EXPORT, true);
                btn(g, &mut fl2, "Import", MUTED, id::IMPORT, true);
            }
            Tab::Design => {
                btn(g, &mut fl2, "New flow (n)", ACCENT, id::NEW, true);
                btn(g, &mut fl2, "From template (t)", CYAN, id::TEMPLATE, true);
                btn(g, &mut fl2, "Import (i)", MUTED, id::IMPORT, true);
                btn(g, &mut fl2, if self.book.offer() { "Agent help on failure: on (o)" } else { "Agent help on failure: off (o)" }, MUTED, id::OFFER, true);
            }
            Tab::Run => {
                let (run, node) = (self.run.as_ref(), self.run_node.as_ref());
                let st = run.map(|r| r.status);
                let nstate = run.zip(node).and_then(|(r, n)| r.nodes.get(n)).map(|n| n.state);
                btn(g, &mut fl2, "Close (Esc)", CYAN, id::R_BACK, true);
                btn(g, &mut fl2, "Stop run (c)", ERR, id::R_CANCEL, st.is_some_and(|s| !s.is_terminal()));
                btn(g, &mut fl2, "Retry step", CYAN, id::R_RETRY, matches!(nstate, Some(NodeState::Failed | NodeState::Interrupted | NodeState::Cancelled)));
                if self.refused_program().is_some() {
                    btn(g, &mut fl2, "Allow this program", AMBER, id::R_ALLOW, true);
                }
                btn(g, &mut fl2, "Skip step", MUTED, id::R_SKIP, matches!(nstate, Some(NodeState::Failed | NodeState::Interrupted | NodeState::Waiting)));
                btn(g, &mut fl2, "Rerun from here", MUTED, id::R_RERUN, node.is_some() && st.is_some_and(|s| !matches!(s, RunStatus::Scheduled | RunStatus::Queued)));
                if nstate == Some(NodeState::Waiting) && run.zip(node).and_then(|(r, n)| r.nodes.get(n)).is_some_and(|n| n.waiting.starts_with("approval")) {
                    btn(g, &mut fl2, "Approve", OK, id::R_APPROVE, true);
                    btn(g, &mut fl2, "Reject", ERR, id::R_REJECT, true);
                }
                btn(g, &mut fl2, "Newer run", MUTED, id::R_NEWER, true);
                btn(g, &mut fl2, "Older run", MUTED, id::R_OLDER, true);
            }
            Tab::Board => {
                btn(g, &mut fl2, &format!("Flow: {}", self.board.flow_filter.clone().unwrap_or_else(|| "all".into())), CYAN, id::B_FLOW, true);
                btn(g, &mut fl2, "Run again", OK, id::B_RUN_AGAIN, !self.board.sel.is_empty());
                btn(g, &mut fl2, "Cancel selected", ERR, id::B_CANCEL_SEL, !self.board.sel.is_empty());
                btn(g, &mut fl2, "Clear selection", MUTED, id::B_CLEAR, !self.board.sel.is_empty());
                btn(g, &mut fl2, "New flow", ACCENT, id::B_NEW, true);
            }
        }
        if any {
            bottom = fl2.bottom();
        }
        bottom + 6.0 * s - main.y
    }

    // ------------------------------------------------------------------------------------------ the flows list

    fn draw_flow_list(&mut self, g: &mut Gfx, body: R, s: f32) {
        let pad = 12.0 * s;
        let r = R::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - 8.0 * s);
        if self.flows.is_empty() {
            fx::empty_state(g, r, s, Icon::Flows, "No workflows yet - press N or ask the chat to make one", "N makes a new flow. T starts from a template (health check, backup, build and deploy, website monitor, gRPC call, daily report). Or open the assistant with Ctrl+I and say: build a flow that checks my site every 5 minutes.", ACCENT);
            self.draw_status(g, R::new(r.x, r.y + r.h - 22.0 * s, r.w, 22.0 * s), s);
            return;
        }
        let row_h = 52.0 * s;
        let mut failed: std::collections::BTreeSet<String> = Default::default();
        let mut seen: std::collections::BTreeSet<&str> = Default::default();
        for x in self.runs.iter().filter(|x| x.parent.is_none() && x.status != RunStatus::Scheduled) {
            if seen.insert(x.flow_id.as_str()) && x.status == RunStatus::Failed {
                failed.insert(x.flow_id.clone());
            }
        }
        let list = R::new(r.x, r.y + 4.0 * s, r.w, r.h - 30.0 * s);
        let visible = ((list.h / row_h).floor() as usize).max(1);
        let first = self.list_sel.saturating_sub(visible - 1).min(self.flows.len().saturating_sub(visible));
        for (i, f) in self.flows.iter().enumerate().skip(first).take(visible) {
            let rr = R::new(list.x, list.y + (i - first) as f32 * row_h, list.w, row_h - 6.0 * s);
            let hot = g.hovered(rr);
            fx::list_row(g, rr, s, i == self.list_sel, hot, ACCENT);
            g.text_fit(rr.x + 14.0 * s, rr.y + 7.0 * s, 15.0 * s, TEXT, &f.name, Face::Bold, rr.w * 0.5);
            let sub = format!("{} · {} steps · {} trigger{}{}", f.id, f.nodes, f.triggers, if f.triggers == 1 { "" } else { "s" }, if f.description.is_empty() { String::new() } else { format!(" · {}", f.description) });
            g.text_fit(rr.x + 14.0 * s, rr.y + 26.0 * s, 11.5 * s, DIM, &sub, Face::Sans, rr.w - 200.0 * s);
            let mut x = rr.x + rr.w - 12.0 * s;
            let mut tag = |g: &mut Gfx, t: &str, c: [u8; 4], filled: bool| {
                let w = g.text_width(fx::size::CAPTION * s, t, Face::Bold) + fx::size::CAPTION * s * 1.4;
                x -= w;
                fx::chip(g, x, rr.y + (rr.h - fx::size::CAPTION * s * 1.9) / 2.0, s, t, c, filled);
                x -= 6.0 * s;
            };
            if f.production {
                tag(g, "production", ERR, false);
            }
            if failed.contains(&f.id) {
                tag(g, "last run failed", ERR, true);
            }
            if f.agent_changed {
                tag(g, "agent changed", MAGENTA, true);
            }
            if f.published == 0 {
                tag(g, "draft", AMBER, true);
            } else {
                tag(g, &format!("v{}{}", f.published, if f.draft_changed { " · draft changed" } else { "" }), if f.draft_changed { AMBER } else { OK }, f.draft_changed);
            }
            g.hit(rr, Event::Row(i), &f.name);
            self.geo.rows.push((i, rr));
        }
        self.draw_status(g, R::new(r.x, r.y + r.h - 22.0 * s, r.w, 22.0 * s), s);
    }

    // ------------------------------------------------------------------------------------------ the designer

    fn draw_designer(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        self.sync_props();
        let pad = 10.0 * s;
        let mut top = body.y;
        // a step's newer version that running or scheduled runs have not taken yet
        if let Some(u) = self.updates.first().cloned() {
            let h = 30.0 * s;
            let r = R::new(body.x + pad, top, body.w - 2.0 * pad, h);
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, with_alpha(AMBER, 36));
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.0, with_alpha(AMBER, 160));
            let line = format!("{} scheduled and {} running run(s) use an older version of step {}{}.", u.scheduled.len(), u.running.len(), u.node, if u.compatible { "" } else { " (the change is not compatible)" });
            let mut bw = 0.0;
            for (label, ev) in [("Leave", id::LEAVE_UPDATE), ("At next step", id::APPLY_RUNNING), ("Apply to scheduled", id::APPLY_SCHEDULED)] {
                bw += g.text_width(11.0 * s, label, Face::Bold) + 11.0 * s * 1.7 + 6.0 * s;
                let _ = ev;
            }
            g.text_fit(r.x + 10.0 * s, r.y + 8.0 * s, 12.0 * s, AMBER, &line, Face::Sans, r.w - bw - 24.0 * s);
            let mut x = r.x + r.w - 6.0 * s;
            for (label, ev) in [("Leave", id::LEAVE_UPDATE), ("At next step", id::APPLY_RUNNING), ("Apply to scheduled", id::APPLY_SCHEDULED)] {
                let w = g.text_width(11.0 * s, label, Face::Bold) + 11.0 * s * 1.7;
                x -= w;
                g.button(x, r.y + 2.0 * s, 11.0 * s, label, AMBER, Event::Tab(ev));
                x -= 6.0 * s;
            }
            top += h + 6.0 * s;
        }
        for b in self.banners() {
            top += super::flows_fail::draw_banner(g, R::new(body.x + pad, top, body.w - 2.0 * pad, 0.0), s, &b) + 6.0 * s;
        }
        let status_h = 24.0 * s;
        let inner = R::new(body.x + pad, top, body.w - 2.0 * pad, (body.y + body.h - top - status_h - 4.0 * s).max(40.0));
        if self.text.is_some() {
            self.draw_text_view(g, inner, s, tick);
            self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
            return;
        }
        let pal_w = if inner.w >= 960.0 * s { 176.0 * s } else { 0.0 };
        // the flow's own settings are drawn when nothing is selected, so they can always be reached
        let want_props = inner.w >= 640.0 * s;
        let prop_w = if want_props { (inner.w * 0.27).clamp_safe(250.0 * s, 330.0 * s) } else { 0.0 };
        let gap = 8.0 * s;
        let canvas = R::new(inner.x + if pal_w > 0.0 { pal_w + gap } else { 0.0 }, inner.y, inner.w - pal_w - prop_w - if pal_w > 0.0 { gap } else { 0.0 } - if prop_w > 0.0 { gap } else { 0.0 }, inner.h);
        if pal_w > 0.0 {
            self.draw_palette(g, R::new(inner.x, inner.y, pal_w, inner.h), s);
        }
        self.geo.canvas = canvas;
        if !self.fitted && canvas.w > 10.0 {
            self.view = View::fit(&self.flow, canvas);
            self.fitted = true;
        }
        let selected = self.sel.clone();
        let hover = self.hover.clone();
        let conn = match &self.drag {
            Some(Drag::Connect { from, pos }) => Some((from.clone(), *pos)),
            _ => None,
        };
        let rubber = match &self.drag {
            Some(Drag::Band { from, to }) => Some(R::new(from.0.min(to.0), from.1.min(to.1), (from.0 - to.0).abs(), (from.1 - to.1).abs())),
            _ => None,
        };
        {
            let p = Paint { flow: &self.flow, reg: &self.reg, view: self.view, rect: canvas, selected: &selected, sel_edge: self.sel_edge, hover: &hover, issues: &self.issue_map, run: None, connecting: conn.as_ref().map(|(f, p)| (f, *p)), rubber, tick, s };
            paint::draw_graph(g, &p);
        }
        let (fl, vw, sl) = (self.flow.clone(), self.view, self.sel.clone());
        self.offer_node_tips(g, canvas, vw, &fl, None, &sl, s);
        if self.flow.nodes.is_empty() {
            fx::empty_state(g, canvas, s, Icon::Flows, "An empty flow - press A to add a step", "Add step (a) puts a step on the canvas. Or open the assistant with Ctrl+I and tell it what the flow should do. Esc closes the flow; it is saved.", ACCENT);
        }
        // zoom buttons and the minimap
        let zb = 26.0 * s;
        let zx = canvas.x + canvas.w - zb - 8.0 * s;
        let mut zy = canvas.y + 8.0 * s;
        for (label, ev) in [("+", id::ZOOM_IN), ("-", id::ZOOM_OUT), ("fit", id::FIT)] {
            let rr = R::new(zx, zy, zb, zb);
            let hot = g.hovered(rr);
            g.rrect(rr.x, rr.y, rr.w, rr.h, zb / 2.0, if hot { fx::mix(fx::WELL, CYAN, 0.25) } else { with_alpha(fx::PANEL, 230) });
            g.rrect_outline(rr.x, rr.y, rr.w, rr.h, zb / 2.0, 1.0, with_alpha(CYAN, 130));
            let ts = if label == "fit" { 10.5 * s } else { 15.0 * s };
            let tw = g.text_width(ts, label, Face::Bold);
            g.text(rr.x + (zb - tw) / 2.0, rr.y + (zb - g.line_height(ts, Face::Bold)) / 2.0, ts, TEXT, label, Face::Bold);
            g.hit(rr, Event::Tab(ev), label);
            zy += zb + 6.0 * s;
        }
        let pct = format!("{}%", (self.view.zoom * 100.0).round());
        let pw = g.text_width(10.5 * s, &pct, Face::Sans);
        g.text(zx + (zb - pw) / 2.0, zy, 10.5 * s, DIM, &pct, Face::Sans);
        if canvas.w >= 420.0 * s && canvas.h >= 260.0 * s {
            let mini = R::new(canvas.x + canvas.w - 156.0 * s - 8.0 * s, canvas.y + canvas.h - 100.0 * s - 8.0 * s, 156.0 * s, 100.0 * s);
            self.geo.minimap = Some(mini);
            paint::draw_minimap(g, &self.flow, &self.view, canvas, mini, s, None);
        }
        if prop_w > 0.0 {
            let pr = R::new(canvas.x + canvas.w + gap, inner.y, prop_w, inner.h);
            self.geo.props = Some(pr);
            self.draw_props(g, pr, s, tick);
        }
        self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
    }

    fn draw_palette(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let pad = 10.0 * s;
        let blink = super::blink(self.tick);
        let on = self.focus == Focus::Search;
        let field = R::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, 30.0 * s);
        super::with_caret(on, || self.palette.render(g, field, on, s, blink, Face::Sans, fid::SEARCH));
        let q = self.palette.value.to_lowercase();
        let mut y = field.y + field.h + 8.0 * s;
        let mut last_cat = "";
        let kinds: Vec<(String, String, String, bool, String)> = self.reg.all().iter().map(|k| (k.id().to_string(), k.label().to_string(), k.category().to_string(), matches!(k.status(), KindStatus::Planned(_)), k.blurb().to_string())).filter(|(id, l, c, _, b)| q.is_empty() || id.contains(&q) || l.to_lowercase().contains(&q) || c.to_lowercase().contains(&q) || b.to_lowercase().contains(&q)).collect();
        let body = R::new(r.x, y, r.w, r.y + r.h - y - 6.0 * s);
        self.geo.palette = Some(r);
        // the whole list is taller than the panel in a small window: it scrolls (wheel), and the search narrows it
        let (head_h, row_h) = (15.0 * s, 30.0 * s);
        let total: f32 = kinds.iter().enumerate().map(|(i, k)| row_h + if i == 0 || kinds[i - 1].2 != k.2 { head_h } else { 0.0 }).sum();
        self.pal_scroll = self.pal_scroll.clamp_safe(0.0, (total - body.h).max(0.0));
        y -= self.pal_scroll;
        let scroll = self.pal_scroll;
        g.clipped(body, |g| {
            for (i, (id_, label, cat, planned, blurb)) in kinds.iter().enumerate() {
                if cat != last_cat {
                    if y + head_h > body.y && y < body.y + body.h {
                        g.text(r.x + pad, y, 10.0 * s, paint::category_color(cat), &cat.to_uppercase(), Face::Bold);
                    }
                    y += head_h;
                    last_cat = cat;
                }
                let rr = R::new(r.x + pad, y, r.w - 2.0 * pad, 26.0 * s);
                y += rr.h + 4.0 * s;
                if rr.y + rr.h <= body.y || rr.y >= body.y + body.h {
                    continue;
                }
                let hot = g.hovered(rr);
                g.rrect(rr.x, rr.y, rr.w, rr.h, 8.0 * s, if hot { fx::mix(fx::WELL, paint::category_color(cat), 0.25) } else { fx::WELL });
                g.rect(rr.x, rr.y + 4.0 * s, 3.0 * s, rr.h - 8.0 * s, paint::category_color(cat));
                g.text_fit(rr.x + 10.0 * s, rr.y + 5.0 * s, 12.0 * s, if *planned { DIM } else { TEXT }, &format!("{label}{}", if *planned { " (planned)" } else { "" }), Face::Bold, rr.w - 14.0 * s);
                // only the visible part of a row that is cut by the panel's edge answers a click
                let (top, bottom) = (rr.y.max(body.y), (rr.y + rr.h).min(body.y + body.h));
                g.hit(R::new(rr.x, top, rr.w, (bottom - top).max(1.0)), Event::Tab(id::PAL + i), &format!("{id_}: {blurb}"));
            }
        });
        if total > body.h {
            let th = (body.h * body.h / total).max(24.0 * s);
            let ty = body.y + (body.h - th) * (scroll / (total - body.h).max(1.0));
            g.rrect(r.x + r.w - 6.0 * s, ty, 3.0 * s, th, 1.5 * s, with_alpha(CYAN, 120));
        }
        self.palette_ids = kinds.iter().map(|k| k.0.clone()).collect();
    }

    fn draw_text_view(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, ACCENT);
        let pad = 10.0 * s;
        g.text(r.x + pad, r.y + 8.0 * s, 13.0 * s, TEXT, "The flow as text (JSON). Edits apply as soon as the text is a valid flow.", Face::Bold);
        let focused = self.focus == Focus::Text;
        let qb = R::new(r.x + pad, r.y + 30.0 * s, r.w - 2.0 * pad, r.h - 30.0 * s - 28.0 * s);
        if let Some(q) = self.text.as_mut() {
            super::with_caret(focused, || q.draw(g, qb, s, focused, tick, ""));
        }
        let (msg, ok) = if self.text_err.is_empty() { ("Valid. Esc goes back to the canvas.".to_string(), true) } else { (self.text_err.clone(), false) };
        fx::status_line(g, r.x + pad, r.y + r.h - 22.0 * s, r.w - 2.0 * pad, s, &msg, ok);
    }

    /// The list editor: what small windows get instead of a canvas (steps as rows, their connections as text, the form beside).
    fn draw_list_editor(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        self.sync_props();
        let mut body = body;
        for b in self.banners() {
            let h = super::flows_fail::draw_banner(g, R::new(body.x + 8.0 * s, body.y, body.w - 16.0 * s, 0.0), s, &b) + 6.0 * s;
            body = R::new(body.x, body.y + h, body.w, (body.h - h).max(40.0));
        }
        let pad = 8.0 * s;
        let status_h = 22.0 * s;
        let inner = R::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - status_h - 4.0 * s);
        g.text(inner.x, inner.y + 2.0 * s, 11.0 * s, DIM, "A small window edits as a list. Design on the canvas in a larger window (1000 px wide).", Face::Sans);
        let top = inner.y + 18.0 * s;
        let two = inner.w >= 560.0 * s;
        let list_w = if two { (inner.w * 0.46).max(240.0 * s) } else { inner.w };
        let list = R::new(inner.x, top, list_w, inner.y + inner.h - top);
        fx::panel(g, list, s, ACCENT);
        let row_h = 38.0 * s;
        let visible = (((list.h - 8.0 * s) / row_h).floor() as usize).max(1);
        let cur = self.sel.iter().next().and_then(|id| self.flow.nodes.iter().position(|n| n.id == *id)).unwrap_or(0);
        let first = cur.saturating_sub(visible - 1).min(self.flow.nodes.len().saturating_sub(visible));
        for (i, n) in self.flow.nodes.iter().enumerate().skip(first).take(visible) {
            let rr = R::new(list.x + 6.0 * s, list.y + 4.0 * s + (i - first) as f32 * row_h, list.w - 12.0 * s, row_h - 4.0 * s);
            let hot = g.hovered(rr);
            fx::list_row(g, rr, s, self.sel.contains(&n.id), hot, ACCENT);
            let kind = self.reg.get(&n.kind).map(|k| k.label().to_string()).unwrap_or_else(|| n.kind.clone());
            g.text_fit(rr.x + 10.0 * s, rr.y + 4.0 * s, 12.5 * s, TEXT, &format!("{}. {} · {}", i + 1, n.title(), kind), Face::Bold, rr.w - 20.0 * s);
            let outs: Vec<String> = self.flow.outgoing(&n.id).iter().map(|e| if e.port == "out" { e.to.clone() } else { format!("{}: {}", e.port, e.to) }).collect();
            g.text_fit(rr.x + 10.0 * s, rr.y + 20.0 * s, 10.5 * s, DIM, &if outs.is_empty() { "goes nowhere".into() } else { format!("then {}", outs.join(", ")) }, Face::Sans, rr.w - 20.0 * s);
            if let Some((e, w)) = self.issue_map.get(&n.id) {
                if *e + *w > 0 {
                    g.circle(rr.x + rr.w - 12.0 * s, rr.y + 12.0 * s, 4.0 * s, if *e > 0 { ERR } else { AMBER });
                }
            }
            g.hit(rr, Event::Row(i), n.title());
            self.geo.rows.push((i, rr));
        }
        if two {
            let pr = R::new(list.x + list.w + 8.0 * s, top, inner.w - list.w - 8.0 * s, list.h);
            self.geo.props = Some(pr);
            self.draw_props(g, pr, s, tick);
        }
        self.draw_status(g, R::new(inner.x, inner.y + inner.h + 4.0 * s, inner.w, status_h), s);
    }

    /// The footer: the keys of this screen and focus, always; a message (saved, started, refused) takes the left half when there is room.
    pub(super) fn draw_status(&mut self, g: &mut Gfx, r: R, s: f32) {
        let hint = self.footer_hint();
        match &self.status {
            Some((t, ok)) if r.w >= 760.0 * s => {
                let half = r.w * 0.5;
                fx::status_line(g, r.x, r.y, half - 8.0 * s, s, t, *ok);
                g.text_fit(r.x + half, r.y + 3.0 * s, 11.5 * s, DIM, &hint, Face::Sans, r.w - half - 4.0 * s);
            }
            Some((t, ok)) => fx::status_line(g, r.x, r.y, r.w, s, t, *ok),
            None => {
                g.text_fit(r.x + 4.0 * s, r.y + 3.0 * s, 12.0 * s, DIM, &hint, Face::Sans, r.w - 8.0 * s);
            }
        }
    }

    // ------------------------------------------------------------------------------------------ dialogs

    pub(super) fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let blink = super::blink(tick);
        if matches!(self.modal, Some(Modal::Suggestions(_))) {
            self.draw_suggestions(g, area, s);
            return;
        }
        let Some(m) = self.modal.as_mut() else { return };
        match m {
            Modal::Name(form, _) | Modal::Import(form) | Modal::RunInputs(form, _, _) => form.render(g, area, s, ACCENT, blink),
            Modal::Templates(p, _) | Modal::AddNode(p, _) => p.render(g, area, s, ACCENT),
            Modal::Confirm(text, _) => super::widgets::render_confirm(g, area, s, text, "Yes"),
            Modal::Publish(f) => {
                let p = fx::modal(g, area, s, 640.0 * s, 460.0 * s, ACCENT, false);
                let mut y = fx::modal_title(g, p, s, "Publish this flow", ACCENT);
                let w = p.w - 52.0 * s;
                let x = p.x + 26.0 * s;
                if f.agent {
                    for l in g.wrap(12.0 * s, "The assistant changed this draft. Publishing approves its changes; they stay marked as the assistant's in the history.", Face::Sans, w).into_iter().take(2) {
                        g.text(x, y, 12.0 * s, MAGENTA, &l, Face::Sans);
                        y += 16.0 * s;
                    }
                }
                for l in f.summary.iter().take(8) {
                    g.text_fit(x, y, 11.5 * s, MUTED, l, Face::Mono, w);
                    y += 15.0 * s;
                }
                y += 6.0 * s;
                let blockers = f.blockers.clone();
                for b in blockers.iter().take(4) {
                    g.text_fit(x, y, 11.5 * s, ERR, &format!("cannot publish: {b}"), Face::Sans, w);
                    y += 15.0 * s;
                }
                for (i, (label, tf)) in [("Label (optional)", &f.label), ("What changed", &f.changelog)].into_iter().enumerate() {
                    g.text(x, y, 11.5 * s, if f.focus == i { CYAN } else { MUTED }, label, Face::Bold);
                    let fr = R::new(x, y + 16.0 * s, w, 32.0 * s);
                    tf.render(g, fr, f.focus == i, s, blink, Face::Sans, 900 + i);
                    y += 56.0 * s;
                }
                let by = p.y + p.h - 46.0 * s;
                let ok = blockers.is_empty();
                if ok {
                    g.button(x, by, 12.5 * s, "Publish", ACCENT, Event::Key(crate::ui::Key::Ctrl('s')));
                } else {
                    g.button_off(x, by, 12.5 * s, "Publish");
                }
                g.button(x + 110.0 * s, by, 12.5 * s, "Cancel", MUTED, Event::Key(crate::ui::Key::Esc));
            }
            Modal::Suggestions(_) => {}
            Modal::History(h) => {
                let p = fx::modal(g, area, s, 760.0 * s, 520.0 * s, ACCENT, false);
                let y0 = fx::modal_title(g, p, s, "Versions", ACCENT);
                let x = p.x + 22.0 * s;
                let list_w = 250.0 * s;
                let row = 34.0 * s;
                let vis = (((p.h - (y0 - p.y) - 56.0 * s) / row).floor() as usize).max(1);
                let first = h.sel.saturating_sub(vis - 1);
                for (i, e) in h.entries.iter().enumerate().skip(first).take(vis) {
                    let rr = R::new(x, y0 + (i - first) as f32 * row, list_w, row - 4.0 * s);
                    fx::list_row(g, rr, s, i == h.sel, g.hovered(rr), ACCENT);
                    let t = format!("v{}{} · {}", e.version, if e.label.is_empty() { String::new() } else { format!(" {}", e.label) }, e.author);
                    g.text_fit(rr.x + 8.0 * s, rr.y + 3.0 * s, 12.0 * s, TEXT, &t, Face::Bold, rr.w - 16.0 * s);
                    let sub = format!("{}{}", e.changelog, if e.changes.iter().any(Change::by_agent) { " · agent changed" } else { "" });
                    g.text_fit(rr.x + 8.0 * s, rr.y + 18.0 * s, 10.5 * s, if e.changes.iter().any(Change::by_agent) { MAGENTA } else { DIM }, &sub, Face::Sans, rr.w - 16.0 * s);
                    g.hit(rr, Event::Row(i), &t);
                }
                let dx = x + list_w + 14.0 * s;
                let dw = p.x + p.w - 22.0 * s - dx;
                let mut y = y0;
                for l in h.diff.iter().take(((p.h - (y0 - p.y) - 70.0 * s) / (14.0 * s)) as usize) {
                    let c = if l.starts_with('+') { OK } else if l.starts_with('-') { ERR } else if l.starts_with('~') { AMBER } else { MUTED };
                    g.text_fit(dx, y, 11.0 * s, c, l, Face::Mono, dw);
                    y += 14.0 * s;
                }
                let by = p.y + p.h - 44.0 * s;
                g.button(x, by, 12.0 * s, "Roll back to this", AMBER, Event::Key(crate::ui::Key::Char('r')));
                g.button(x + 190.0 * s, by, 12.0 * s, "Drop this version", ERR, Event::Key(crate::ui::Key::Char('x')));
                g.button(x + 380.0 * s, by, 12.0 * s, "Close", MUTED, Event::Key(crate::ui::Key::Esc));
            }
            Modal::Result(title, log) => {
                let p = fx::modal(g, area, s, 640.0 * s, 440.0 * s, ACCENT, false);
                let y0 = fx::modal_title(g, p, s, title, ACCENT);
                let r = R::new(p.x + 22.0 * s, y0, p.w - 44.0 * s, p.h - (y0 - p.y) - 56.0 * s);
                log.render(g, r, 12.0 * s, Face::Mono);
                g.button(p.x + 22.0 * s, p.y + p.h - 44.0 * s, 12.0 * s, "Close", ACCENT, Event::Key(crate::ui::Key::Esc));
            }
            Modal::EditPatch(title, qb) => {
                let p = fx::modal(g, area, s, 680.0 * s, 460.0 * s, ACCENT, false);
                let y0 = fx::modal_title(g, p, s, title, ACCENT);
                let r = R::new(p.x + 22.0 * s, y0, p.w - 44.0 * s, p.h - (y0 - p.y) - 60.0 * s);
                super::with_caret(true, || qb.draw(g, r, s, true, tick, ""));
                g.button(p.x + 22.0 * s, p.y + p.h - 46.0 * s, 12.0 * s, "Approve my version", ACCENT, Event::Key(crate::ui::Key::Ctrl('s')));
                g.button(p.x + 220.0 * s, p.y + p.h - 46.0 * s, 12.0 * s, "Cancel", MUTED, Event::Key(crate::ui::Key::Esc));
            }
        }
    }
}

fn rgb_dim() -> [u8; 4] {
    crate::gfx::rgb(50, 60, 92)
}
