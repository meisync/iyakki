//! K8S: the drawing. Three columns when there is room (namespaces and kinds | resources | details), two on a medium screen
//! (resources | details, the namespace and kind in drop-downs), one at a time on a phone-sized one (lists, then details with Back).
//! Lists and the text view draw whole rows only, so nothing is cut at an edge and every click region lies inside its pane.

use crate::safeclamp::SafeClamp;
use iyakki_agent::k8s::{Health, age_text};
use iyakki_agent::k8sexp::{self as kx, Confirm, Kind, Scope};

use super::k8s_tile::*;
use super::widgets;
use super::AppCx;
use crate::fx::{self, with_alpha, AMBER, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

const KEY: [u8; 4] = rgb(130, 190, 255);

fn health_color(h: Option<Health>) -> [u8; 4] {
    match h {
        Some(Health::Good) => OK,
        Some(Health::Warn) => AMBER,
        Some(Health::Bad) => ERR,
        Some(Health::Idle) => DIM,
        None => TEXT,
    }
}

/// Lay controls out left to right and wrap when the row is full.
struct Cursor {
    x0: f32,
    x: f32,
    y: f32,
    w: f32,
    row_h: f32,
    gap: f32,
}

impl Cursor {
    fn new(x: f32, y: f32, w: f32, row_h: f32, gap: f32) -> Cursor {
        Cursor { x0: x, x, y, w, row_h, gap }
    }
    /// Where a control of width `cw` goes.
    fn place(&mut self, cw: f32) -> (f32, f32) {
        if self.x > self.x0 && self.x + cw > self.x0 + self.w {
            self.x = self.x0;
            self.y += self.row_h + self.gap;
        }
        let p = (self.x, self.y);
        self.x += cw + self.gap;
        p
    }
    fn bottom(&self) -> f32 {
        self.y + self.row_h
    }
}

fn btn_w(g: &mut Gfx, size: f32, label: &str) -> f32 {
    g.text_width(size, label, Face::Bold) + size * 1.7
}

/// A button (or a dimmed one) placed by the cursor.
fn btn(g: &mut Gfx, c: &mut Cursor, size: f32, label: &str, accent: [u8; 4], ev: Event, on: bool) {
    let (x, y) = c.place(btn_w(g, size, label));
    if on {
        g.button(x, y, size, label, accent, ev);
    } else {
        g.button_off(x, y, size, label);
    }
}

fn para(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, color: [u8; 4], text: &str, max: usize, draw: bool) -> f32 {
    let lines = g.wrap(size, text, Face::Sans, w);
    let lh = g.line_height(size, Face::Sans) * 1.2;
    let n = lines.len().clamp_safe(1, max);
    if draw {
        for (i, l) in lines.iter().take(max).enumerate() {
            g.text(x, y + i as f32 * lh, size, color, l, Face::Sans);
        }
    }
    n as f32 * lh
}

/// Keep the selected row in view when it moved by key; clamp the first row. Returns the first row.
fn fit_list(l: &mut ListState, n: usize, fit: usize) -> usize {
    l.fit = fit;
    if l.reveal && n > 0 {
        let sel = l.sel.min(n - 1);
        if sel < l.top {
            l.top = sel;
        } else if sel >= l.top + fit {
            l.top = sel + 1 - fit;
        }
        l.reveal = false;
    }
    l.top = l.top.min(n.saturating_sub(fit));
    l.top
}

/// A thin scroll bar on the right edge of a list or text.
fn scrollbar(g: &mut Gfx, r: R, s: f32, top: usize, fit: usize, n: usize) {
    if n <= fit || n == 0 {
        return;
    }
    let th = (r.h * fit as f32 / n as f32).max(14.0 * s);
    let ty = r.y + (r.h - th) * (top as f32 / (n - fit).max(1) as f32);
    g.rrect(r.x + r.w - 4.0 * s, ty, 3.0 * s, th, 1.5 * s, with_alpha(MUTED, 110));
}

impl K8sApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.last_draw = Some(std::time::Instant::now());
        self.mode = Mode::of(area.w, s);
        let m = 10.0 * s;
        let (x, w) = (area.x + m, (area.w - 2.0 * m).max(40.0));
        let mut y = area.y + 8.0 * s;
        y += self.draw_header(g, x, y, w, s) + 8.0 * s;
        // the line at the bottom: the last thing that happened
        let status_h = self.status_height(g, w, s);
        let body = R::new(x, y, w, (area.y + area.h - y - status_h - 6.0 * s).max(60.0));
        self.draw_body(g, body, s, tick);
        if status_h > 0.0 {
            self.draw_status(g, R::new(x, body.y + body.h + 4.0 * s, w, status_h), s);
        }
        self.draw_overlays(g, area, s, cx, tick);
        // the open cluster list goes over everything, unless a card or panel for a change is up
        if self.pending.is_none() && self.overlay.is_none() {
            self.host.draw_popup(g, area, s, ACCENT);
        }
    }

    // ------------------------------------------------------------------------------------------------ header

    fn draw_header(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let row_h = super::ide_chrome::tool(s);
        let mut c = Cursor::new(x, y, w, row_h, 8.0 * s);
        let chip = if self.entry().managed { self.managed.chip().to_string() } else { "saved profile".to_string() };
        let label = format!("{} \u{b7} {}", self.entry().label, chip);
        if self.host.visible() {
            // several clusters: the switcher (name, state dot; its list has kind, GPU and "Manage clusters...") replaces the plain list
            let cw = self.host.width(g, s).min(w);
            let (cx0, cy0) = c.place(cw);
            self.host.draw(g, cx0, cy0, cw, s, ACCENT);
        } else {
            let cw = widgets::select_width(g, s, &label).min(w);
            let (cx0, cy0) = c.place(cw);
            let hw = if self.mode == Mode::Phone { (w - 0.0).min(cw) } else { cw };
            widgets::select_box(g, cx0, cy0, hw, s, &label, self.overlay == Some(Overlay::Clusters), ACCENT, Event::Tab(T_CLUSTER), "cluster");
        }
        // a state dot beside the chip text is part of the label; the colour cue sits right after the box
        if self.mode != Mode::Wide && self.usable() {
            let nl = format!("Namespace: {}", self.scope.label());
            let nw = widgets::select_width(g, s, &nl).min(w);
            let (nx, ny) = c.place(nw);
            widgets::select_box(g, nx, ny, nw, s, &nl, self.overlay == Some(Overlay::Namespaces), ACCENT, Event::Tab(T_NS_BTN), "namespace");
            let kl = format!("Kind: {}", self.kind.label());
            let kw = widgets::select_width(g, s, &kl).min(w);
            let (kx0, ky) = c.place(kw);
            widgets::select_box(g, kx0, ky, kw, s, &kl, self.overlay == Some(Overlay::Kinds), ACCENT, Event::Tab(T_KIND_BTN), "kind");
        }
        let size = 13.0 * s;
        if self.usable() {
            let (rx, ry) = c.place(btn_w(g, size, "Refresh"));
            g.button(rx, ry + (row_h - size * 2.1) / 2.0, size, "Refresh", MUTED, Event::Tab(T_REFRESH));
            let lab = if self.show_log { "Hide log" } else { "Show log" };
            let (lx, ly) = c.place(btn_w(g, size, lab));
            g.button(lx, ly + (row_h - size * 2.1) / 2.0, size, lab, MUTED, Event::Tab(T_LOG));
            // the Allow changes switch
            let on = self.allow_changes();
            let lab = "Allow changes";
            let sw = 34.0 * s;
            let tw = g.text_width(size, lab, Face::Bold);
            let (ax, ay) = c.place(sw + 8.0 * s + tw + 8.0 * s);
            let r = R::new(ax, ay, sw + 8.0 * s + tw + 8.0 * s, row_h);
            let dim = self.change_block().is_some();
            let col = if dim { DIM } else if on { AMBER } else { MUTED };
            let ty = ay + (row_h - 16.0 * s) / 2.0;
            g.rrect(ax, ty, sw, 16.0 * s, 8.0 * s, if on { with_alpha(AMBER, 200) } else { rgb(40, 48, 78) });
            g.rrect_outline(ax, ty, sw, 16.0 * s, 8.0 * s, 1.0, col);
            g.circle(if on { ax + sw - 8.0 * s } else { ax + 8.0 * s }, ty + 8.0 * s, 6.0 * s, if on { rgb(30, 24, 8) } else { col });
            g.text(ax + sw + 8.0 * s, ay + (row_h - g.line_height(size, Face::Bold)) / 2.0, size, col, lab, Face::Bold);
            g.hit(r, Event::Tab(T_ALLOW), "Allow changes");
        }
        c.bottom() - y
    }

    fn status_height(&self, g: &mut Gfx, w: f32, s: f32) -> f32 {
        match &self.status {
            Some((t, _)) if !t.is_empty() => para(&mut *g, 0.0, 0.0, w, 13.0 * s, TEXT, t, 2, false) + 2.0 * s,
            _ => match self.stale_text() {
                Some(t) => para(&mut *g, 0.0, 0.0, w, 12.5 * s, DIM, &t, 2, false) + 2.0 * s,
                None => 0.0,
            },
        }
    }

    fn draw_status(&mut self, g: &mut Gfx, r: R, s: f32) {
        if let Some((t, ok)) = &self.status {
            para(g, r.x, r.y, r.w, 13.0 * s, if *ok { MUTED } else { ERR }, t, 2, true);
        } else if let Some(t) = self.stale_text() {
            para(g, r.x, r.y, r.w, 12.5 * s, DIM, &t, 2, true);
        }
    }

    // ------------------------------------------------------------------------------------------------ body

    fn draw_body(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        self.ns.rect = R::default();
        self.kinds.rect = R::default();
        if self.show_log {
            self.res.rect = R::default();
            self.res.fit = 0;
            self.det.view.rect = R::default();
            self.draw_cluster_log(g, body, s);
            return;
        }
        if !self.usable() || self.listing_problem().is_some() {
            self.res.rect = R::default();
            self.res.fit = 0;
            self.det.view.rect = R::default();
            self.draw_state(g, body, s);
            return;
        }
        let gap = 8.0 * s;
        match self.mode {
            Mode::Wide => {
                let c1 = (body.w * 0.2).clamp_safe(190.0 * s, 300.0 * s);
                let c3 = ((body.w - c1) * 0.42).max(260.0 * s);
                let c2 = body.w - c1 - c3 - 2.0 * gap;
                let ns_h = (body.h * 0.6).max(120.0 * s).min(body.h - 100.0 * s);
                self.draw_ns_pane(g, R::new(body.x, body.y, c1, ns_h), s, tick);
                self.draw_kind_pane(g, R::new(body.x, body.y + ns_h + gap, c1, body.h - ns_h - gap), s);
                self.draw_res_pane(g, R::new(body.x + c1 + gap, body.y, c2, body.h), s, tick);
                self.draw_detail_pane(g, R::new(body.x + c1 + c2 + 2.0 * gap, body.y, c3, body.h), s, tick);
            }
            Mode::Medium => {
                let c1 = (body.w * 0.5).max(300.0 * s);
                self.draw_res_pane(g, R::new(body.x, body.y, c1, body.h), s, tick);
                self.draw_detail_pane(g, R::new(body.x + c1 + gap, body.y, body.w - c1 - gap, body.h), s, tick);
            }
            Mode::Phone => {
                if self.page_detail {
                    self.res.rect = R::default();
                    self.res.fit = 0;
                    self.draw_detail_pane(g, body, s, tick);
                } else {
                    self.det.view.rect = R::default();
                    self.draw_res_pane(g, body, s, tick);
                }
            }
        }
    }

    /// What is wrong with the cluster, from the last answer of the feed (None: all is well or nothing has come yet).
    fn listing_problem(&self) -> Option<iyakki_agent::k8s::Cluster> {
        use iyakki_agent::k8s::Cluster;
        match self.feed.listing().filter(|l| l.kind == self.kind && l.scope == self.scope) {
            Some(l) if l.cluster != Cluster::Ok => Some(l.cluster.clone()),
            Some(_) => None,
            None => self.feed.ns().filter(|n| n.cluster != Cluster::Ok && n.cluster != Cluster::Failed(String::new())).and_then(|n| match n.cluster.clone() {
                c @ (Cluster::KubectlMissing | Cluster::Unreachable(_)) => Some(c),
                _ => None,
            }),
        }
    }

    /// No cluster to look at: say why in plain words, with the button that helps.
    fn draw_state(&mut self, g: &mut Gfx, r: R, s: f32) {
        let (title, text, buttons) = self.state_text();
        fx::panel(g, r, s, ACCENT);
        let (x, w) = (r.x + 16.0 * s, r.w - 32.0 * s);
        let mut y = r.y + 14.0 * s;
        let th = g.line_height(18.0 * s, Face::Bold);
        let t = g.fit(18.0 * s, &title, Face::Bold, w);
        g.text(x, y, 18.0 * s, TEXT, &t, Face::Bold);
        y += th + 8.0 * s;
        y += para(g, x, y, w, 14.0 * s, MUTED, &text, 12, true) + 12.0 * s;
        let mut c = Cursor::new(x, y, w, 13.5 * s * 2.1, 8.0 * s);
        for (label, ev, on) in buttons {
            btn(g, &mut c, 13.5 * s, label, ACCENT, ev, on);
        }
    }

    pub(super) fn state_text(&self) -> (String, String, Vec<(&'static str, Event, bool)>) {
        use iyakki_agent::k8s::Cluster;
        if self.entry().managed && self.managed != Managed::Running {
            return match &self.managed {
                Managed::NotSet => ("No dev cluster is set up on this computer".into(), "The managed k3s cluster is set up in Settings \u{2192} Dev cluster. You can also pick one of your saved kubeconfig profiles from the cluster list above (they are added in Settings \u{2192} Dev cluster, with the kubeconfig kept in the vault).".into(), vec![("Open Settings", Event::Tab(T_SETTINGS), true)]),
                Managed::Stopped => ("The dev cluster is stopped".into(), "Start it to look at it. The first start takes a minute or two; this page fills in by itself when it is up.".into(), vec![("Start", Event::Tab(T_START), true)]),
                Managed::Starting { waiting_net: true } => ("Waiting for the network".into(), "The dev cluster starts as soon as this computer has a network connection with a default route. Nothing needs to be done here.".into(), vec![]),
                Managed::Starting { .. } => ("The dev cluster is starting".into(), "The first start takes a minute or two. This page fills in by itself when the cluster answers.".into(), vec![]),
                Managed::Stopping => ("The dev cluster is stopping".into(), "Wait a moment, then press Start.".into(), vec![("Start", Event::Tab(T_START), true)]),
                Managed::Failed(r) => ("The dev cluster failed".into(), format!("{r}. Settings \u{2192} Dev cluster shows the log; Start tries again once the cause is fixed."), vec![("Start", Event::Tab(T_START), true), ("Open Settings", Event::Tab(T_SETTINGS), true)]),
                Managed::Running => unreachable!(),
            };
        }
        let c = self.listing_problem().unwrap_or(Cluster::Ok);
        match super::ide_k8s::banner_for(&c) {
            Some((t, b)) => {
                // after the stale window: say when it last worked and what happens next
                let st = self.feed.status();
                let b = if st.failing { format!("{b} {}", iyakki_agent::k8sretry::banner_tail(&st)) } else { b };
                (t, b, vec![("Refresh", Event::Tab(T_REFRESH), true), ("Show log", Event::Tab(T_LOG), true), ("Open Settings", Event::Tab(T_SETTINGS), true)])
            }
            None => ("Reading from the cluster\u{2026}".into(), "The first answer takes a moment.".into(), vec![]),
        }
    }

    // ------------------------------------------------------------------------------------------------ panes

    fn draw_ns_pane(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, if self.focus == Focus::Ns || self.focus == Focus::NsFilter { ACCENT } else { rgb(60, 72, 112) });
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        g.text(x, r.y + 6.0 * s, 13.0 * s, MUTED, "NAMESPACES", Face::Bold);
        let fy = r.y + 6.0 * s + 18.0 * s;
        let fh = 28.0 * s;
        self.ns_filter.render(g, R::new(x, fy, w, fh), self.focus == Focus::NsFilter, s, super::blink(tick), Face::Sans, F_NS);
        let list = R::new(x, fy + fh + 6.0 * s, w, (r.y + r.h - fy - fh - 12.0 * s).max(20.0));
        self.ns.rect = list;
        self.draw_ns_list(g, list, s, self.focus == Focus::Ns);
    }

    fn draw_ns_list(&mut self, g: &mut Gfx, list: R, s: f32, focused: bool) {
        let rows = self.ns_rows();
        let row_h = 28.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut self.ns, rows.len(), fit);
        if rows.is_empty() {
            let msg = if self.feed.ns().is_none() { "Reading the namespaces\u{2026}" } else { "No namespace matches." };
            g.text(list.x + 4.0 * s, list.y + 4.0 * s, 13.0 * s, DIM, msg, Face::Sans);
            return;
        }
        for (k, (scope, label, pods, failing)) in rows.iter().enumerate().skip(top).take(fit) {
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let chosen = *scope == self.scope;
            let hot = g.hovered(rr);
            if chosen || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen, hot, ACCENT);
            }
            if focused && k == self.ns.sel {
                g.rrect_outline(rr.x, rr.y, rr.w, rr.h, 6.0 * s, 1.5 * s, ACCENT);
            }
            let size = 13.0 * s;
            let ty = ry + (row_h - 2.0 * s - g.line_height(size, Face::Sans)) / 2.0;
            let mut right = rr.x + rr.w - 8.0 * s;
            if let Some(n) = pods {
                let t = n.to_string();
                let bw = g.text_width(11.5 * s, &t, Face::Bold) + 12.0 * s;
                right -= bw;
                g.rrect(right, ry + 4.0 * s, bw, row_h - 10.0 * s, (row_h - 10.0 * s) / 2.0, rgb(36, 46, 80));
                g.text(right + 6.0 * s, ry + (row_h - 2.0 * s - g.line_height(11.5 * s, Face::Bold)) / 2.0, 11.5 * s, MUTED, &t, Face::Bold);
                right -= 6.0 * s;
            }
            if *failing > 0 {
                g.circle(right - 4.0 * s, ry + (row_h - 2.0 * s) / 2.0, 4.0 * s, ERR);
                right -= 12.0 * s;
            }
            let t = g.fit(size, label, Face::Sans, (right - rr.x - 10.0 * s).max(10.0));
            g.text(rr.x + 8.0 * s, ty, size, if chosen { TEXT } else { MUTED }, &t, Face::Sans);
            g.hit(rr, Event::Tab(T_NS_ROW + k), label);
        }
        scrollbar(g, list, s, top, fit, rows.len());
    }

    fn draw_kind_pane(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, if self.focus == Focus::Kinds { ACCENT } else { rgb(60, 72, 112) });
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        g.text(x, r.y + 6.0 * s, 13.0 * s, MUTED, "KINDS", Face::Bold);
        let list = R::new(x, r.y + 6.0 * s + 20.0 * s, w, (r.h - 30.0 * s).max(20.0));
        self.kinds.rect = list;
        self.draw_kind_list(g, list, s, self.focus == Focus::Kinds);
    }

    fn draw_kind_list(&mut self, g: &mut Gfx, list: R, s: f32, focused: bool) {
        let row_h = 26.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut self.kinds, Kind::ALL.len(), fit);
        for (k, kind) in Kind::ALL.iter().enumerate().skip(top).take(fit) {
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let chosen = *kind == self.kind;
            let hot = g.hovered(rr);
            if chosen || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen, hot, ACCENT);
            }
            if focused && k == self.kinds.sel {
                g.rrect_outline(rr.x, rr.y, rr.w, rr.h, 6.0 * s, 1.5 * s, ACCENT);
            }
            let size = 13.0 * s;
            let t = g.fit(size, kind.label(), Face::Sans, rr.w - 16.0 * s);
            g.text(rr.x + 8.0 * s, ry + (row_h - 2.0 * s - g.line_height(size, Face::Sans)) / 2.0, size, if chosen { TEXT } else { MUTED }, &t, Face::Sans);
            g.hit(rr, Event::Tab(T_KIND_ROW + k), kind.label());
        }
        scrollbar(g, list, s, top, fit, Kind::ALL.len());
    }

    // ------------------------------------------------------------------------------------------------ resources

    fn draw_res_pane(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, if matches!(self.focus, Focus::Res | Focus::Filter) { ACCENT } else { rgb(60, 72, 112) });
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let listing = self.listing();
        let n_all = self.order().len();
        let rows_now = listing.as_ref().map(|l| l.rows.clone()).unwrap_or_default();
        // the count line
        let count = match &listing {
            Some(l) => {
                let shown: Vec<kx::Row> = self.order().iter().map(|&i| l.rows[i].clone()).collect();
                let mut t = kx::count_line(self.kind, &shown, l.total.max(shown.len()));
                if shown.len() != l.rows.len() {
                    t = format!("{} of {}", shown.len(), t);
                }
                format!("{} \u{b7} {}", t, self.scope.label())
            }
            None => format!("{} \u{b7} {}", self.kind.label(), self.scope.label()),
        };
        let mut y = r.y + 6.0 * s;
        if self.mode == Mode::Phone {
            // nothing to add: the drop-downs are in the header
        }
        let t = g.fit(13.0 * s, &count, Face::Bold, w);
        g.text(x, y, 13.0 * s, MUTED, &t, Face::Bold);
        y += 20.0 * s;
        let fh = 28.0 * s;
        let fw = if self.filter.value.is_empty() { w } else { w - 70.0 * s };
        self.filter.render(g, R::new(x, y, fw, fh), self.focus == Focus::Filter, s, super::blink(tick), Face::Sans, F_FILTER);
        if !self.filter.value.is_empty() {
            g.button(x + fw + 8.0 * s, y + (fh - 12.5 * s * 2.1) / 2.0, 12.5 * s, "Clear", MUTED, Event::Tab(T_CLEAR_FILTER));
        }
        y += fh + 6.0 * s;
        let Some(l) = listing else {
            g.text(x + 4.0 * s, y + 6.0 * s, 13.0 * s, DIM, "Reading from the cluster\u{2026}", Face::Sans);
            self.res.rect = R::new(x, y, w, (r.y + r.h - y).max(0.0));
            return;
        };
        // columns: drop the least important ones until the rest fit
        let with_ns = l.scope == Scope::All;
        let cols = kx::columns(l.kind, with_ns);
        let min_w = |g: &mut Gfx, c: &kx::Col| -> f32 {
            let base = match c.title {
                "Name" => 150.0,
                "Message" => 170.0,
                "Status" => 112.0,
                "Namespace" | "Node" | "Hosts" | "Object" | "Ports" => 100.0,
                _ => 62.0,
            };
            (g.text_width(12.0 * s, c.title, Face::Bold) + 22.0 * s).max(base * s)
        };
        let mut shown: Vec<usize> = (0..cols.len()).collect();
        loop {
            let need: f32 = shown.iter().map(|&i| min_w(g, &cols[i])).sum();
            if need <= w - 16.0 * s || shown.len() <= 1 {
                break;
            }
            let worst = shown.iter().copied().filter(|&i| cols[i].prio > 0).max_by_key(|&i| (cols[i].prio, i));
            match worst {
                Some(i) => shown.retain(|&k| k != i),
                None => break,
            }
        }
        // every shown column gets its least width, the rest is shared by weight
        let mins: Vec<f32> = shown.iter().map(|&i| min_w(g, &cols[i])).collect();
        let wsum: f32 = shown.iter().map(|&i| cols[i].weight).sum();
        let avail = w - 16.0 * s;
        let extra = (avail - mins.iter().sum::<f32>()).max(0.0);
        let widths: Vec<f32> = shown.iter().enumerate().map(|(k, &i)| mins[k] + extra * cols[i].weight / wsum).collect();
        // header
        let hh = 22.0 * s;
        let mut cx0 = x + 14.0 * s;
        let sort = self.sort.unwrap_or_else(|| kx::default_sort(l.kind, with_ns));
        for (k, &ci) in shown.iter().enumerate() {
            let arrow = if sort.0 == ci { if sort.1 { " \u{25bc}" } else { " \u{25b2}" } } else { "" };
            let label = format!("{}{}", cols[ci].title, arrow);
            let t = g.fit(12.0 * s, &label, Face::Bold, widths[k] - 6.0 * s);
            let rr = R::new(cx0 - 2.0 * s, y, widths[k] - 2.0 * s, hh);
            if g.hovered(rr) {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 4.0 * s, rgb(34, 44, 78));
            }
            g.text(cx0, y + (hh - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, if sort.0 == ci { ACCENT } else { MUTED }, &t, Face::Bold);
            g.hit(rr, Event::Tab(T_SORT + ci), cols[ci].title);
            cx0 += widths[k];
        }
        y += hh + 2.0 * s;
        // rows
        let more = n_all.saturating_sub(kx::MAX_SHOWN);
        let foot_h = if more > 0 || n_all == 0 { 22.0 * s } else { 0.0 };
        let list = R::new(x, y, w, (r.y + r.h - y - foot_h - 4.0 * s).max(10.0));
        self.res.rect = list;
        let row_h = 26.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let n = n_all.min(kx::MAX_SHOWN);
        let top = fit_list(&mut self.res, n, fit);
        let order: Vec<usize> = self.order().to_vec();
        for k in top..(top + fit).min(n) {
            let row = &rows_now[order[k]];
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let sel = k == self.res.sel;
            let hot = g.hovered(rr);
            if row.warn && !sel {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 5.0 * s, with_alpha(AMBER, 22));
            }
            if sel || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, sel, hot, ACCENT);
            }
            let hc = health_color(row.health);
            if row.health.is_some() {
                g.circle(rr.x + 8.0 * s, ry + (row_h - 2.0 * s) / 2.0, 3.5 * s, hc);
            }
            let mut tx = list.x + 14.0 * s;
            for (j, &ci) in shown.iter().enumerate() {
                let title = cols[ci].title;
                let statusy = matches!(title, "Status" | "Ready" | "Type" | "Restarts") && row.health.is_some();
                let color = if row.warn { AMBER } else if statusy { hc } else if row.health == Some(Health::Idle) { MUTED } else { TEXT };
                let cell = row.cells.get(ci).map(String::as_str).unwrap_or("");
                let t = g.fit(13.0 * s, cell, Face::Sans, widths[j] - 8.0 * s);
                g.text(tx, ry + (row_h - 2.0 * s - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, color, &t, Face::Sans);
                tx += widths[j];
            }
            g.hit(rr, Event::Tab(T_RES_ROW + k), &row.name);
        }
        scrollbar(g, list, s, top, fit, n);
        let fy = list.y + list.h + 4.0 * s;
        if n_all == 0 {
            let msg = if !self.filter.value.is_empty() { format!("Nothing matches \u{201c}{}\u{201d}. Esc clears the filter.", self.filter.value) } else if l.kind.namespaced() { format!("No {} in {}. Try another namespace (n) or kind (k).", l.kind.label().to_lowercase(), self.scope.label()) } else { format!("No {}.", l.kind.label().to_lowercase()) };
            let t = g.fit(13.0 * s, &msg, Face::Sans, list.w - 8.0 * s);
            g.text(list.x + 4.0 * s, list.y + 4.0 * s, 13.0 * s, DIM, &t, Face::Sans);
        } else if more > 0 {
            let msg = format!("{more} more: refine the filter (/)");
            g.text(x + 4.0 * s, fy, 12.5 * s, AMBER, &msg, Face::Sans);
        }
        if l.kind == Kind::Nodes && l.metrics == Some(false) {
            let msg = "Usage shows \u{2014} because metrics-server is not running";
            let t = g.fit(11.5 * s, msg, Face::Sans, w - 8.0 * s);
            g.text(x + 4.0 * s, r.y + r.h - 18.0 * s, 11.5 * s, DIM, &t, Face::Sans);
        }
    }

    // ------------------------------------------------------------------------------------------------ details

    fn draw_detail_pane(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, if self.focus == Focus::Detail || self.focus == Focus::LogSearch { ACCENT } else { rgb(60, 72, 112) });
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let mut y = r.y + 6.0 * s;
        let size = 13.0 * s;
        let Some((kind, ns, name)) = self.det.key.clone() else {
            let mut c = Cursor::new(x, y, w, size * 2.1, 8.0 * s);
            if self.mode == Mode::Phone {
                btn(g, &mut c, size, "Back", MUTED, Event::Tab(T_BACK), true);
            }
            let by = c.bottom() + 8.0 * s;
            para(g, x, by, w, 14.0 * s, DIM, "Select something in the list to see it here: a summary, its YAML, events, related objects and, for a pod, its logs.", 6, true);
            self.det.view.rect = R::default();
            return;
        };
        if self.mode == Mode::Phone {
            g.button(x, y, size, "Back", MUTED, Event::Tab(T_BACK));
            let bw = btn_w(g, size, "Back") + 8.0 * s;
            let title = if ns.is_empty() { format!("{} \u{b7} {}", kind.api_kind(), name) } else { format!("{} \u{b7} {}/{}", kind.api_kind(), ns, name) };
            let t = g.fit(14.0 * s, &title, Face::Bold, w - bw);
            g.text(x + bw, y + (size * 2.1 - g.line_height(14.0 * s, Face::Bold)) / 2.0, 14.0 * s, TEXT, &t, Face::Bold);
            y += size * 2.1 + 6.0 * s;
        } else {
            let title = if ns.is_empty() { format!("{} \u{b7} {}", kind.api_kind(), name) } else { format!("{} \u{b7} {}/{}", kind.api_kind(), ns, name) };
            let t = g.fit(14.0 * s, &title, Face::Bold, w);
            g.text(x, y, 14.0 * s, TEXT, &t, Face::Bold);
            y += g.line_height(14.0 * s, Face::Bold) + 6.0 * s;
        }
        // tabs
        let mut c = Cursor::new(x, y, w, fx::size::CAPTION * s * 1.9, 6.0 * s);
        for (i, t) in DTab::ALL.iter().enumerate() {
            if *t == DTab::Logs && kind != Kind::Pods {
                continue;
            }
            let sz = fx::size::CAPTION * s;
            let cw = g.text_width(sz, t.label(), Face::Bold) + sz * 1.4;
            let (tx, ty) = c.place(cw);
            fx::chip(g, tx, ty, s, t.label(), ACCENT, *t == self.det.tab);
            g.hit(R::new(tx, ty, cw, c.row_h), Event::Tab(T_TAB + i), t.label());
        }
        y = c.bottom() + 6.0 * s;
        // tab-specific controls
        let mut c = Cursor::new(x, y, w, size * 2.1, 6.0 * s);
        btn(g, &mut c, size, "Copy", MUTED, Event::Tab(T_COPY), !self.det.view.lines.is_empty());
        match self.det.tab {
            DTab::Yaml if kind == Kind::Secrets => {
                if self.det.reveal {
                    btn(g, &mut c, size, "Hide values", AMBER, Event::Tab(T_REVEAL), true);
                } else {
                    btn(g, &mut c, size, "Reveal (asks first)", AMBER, Event::Tab(T_REVEAL), true);
                }
            }
            DTab::Yaml => btn(g, &mut c, size, if self.det.view.wrap { "No wrap" } else { "Wrap" }, MUTED, Event::Tab(T_WRAP), true),
            DTab::Logs => {
                let l = &self.det.logs;
                btn(g, &mut c, size, if l.follow { "Follow: on" } else { "Follow: off" }, if l.follow { OK } else { MUTED }, Event::Tab(T_FOLLOW), true);
                btn(g, &mut c, size, if l.paused { "Resume" } else { "Pause" }, MUTED, Event::Tab(T_PAUSE), true);
                btn(g, &mut c, size, if l.previous { "Previous: on" } else { "Previous" }, if l.previous { AMBER } else { MUTED }, Event::Tab(T_PREV), true);
                btn(g, &mut c, size, &format!("Last {}", iyakki_agent::k8sexp::LogReq::TAILS[l.tail_i.min(2)]), MUTED, Event::Tab(T_TAIL), true);
                btn(g, &mut c, size, if self.det.view.wrap { "No wrap" } else { "Wrap" }, MUTED, Event::Tab(T_WRAP), true);
                btn(g, &mut c, size, "Save", MUTED, Event::Tab(T_SAVE), !l.buf.lines.is_empty());
            }
            _ => {}
        }
        if kind == Kind::Pods {
            btn(g, &mut c, size, "Copy exec command", MUTED, Event::Tab(T_EXEC), true);
        }
        if self.ide_ok {
            btn(g, &mut c, size, "Open in AI IDE", MUTED, Event::Tab(T_IDE), true);
        }
        y = c.bottom() + 4.0 * s;
        // container chips and the log search
        if self.det.tab == DTab::Logs {
            if self.det.containers.len() > 1 {
                let mut cc = Cursor::new(x, y, w, fx::size::CAPTION * s * 1.9, 6.0 * s);
                let cur = if self.det.logs.container.is_empty() { self.det.containers.first().cloned().unwrap_or_default() } else { self.det.logs.container.clone() };
                for (i, n) in self.det.containers.iter().enumerate().take(99) {
                    let sz = fx::size::CAPTION * s;
                    let cw = g.text_width(sz, n, Face::Bold) + sz * 1.4;
                    let (tx, ty) = cc.place(cw);
                    fx::chip(g, tx, ty, s, n, ACCENT, *n == cur);
                    g.hit(R::new(tx, ty, cw, cc.row_h), Event::Tab(T_CONT + i), n);
                }
                y = cc.bottom() + 4.0 * s;
            }
            let fh = 26.0 * s;
            self.det.logs.search.render(g, R::new(x, y, w, fh), self.focus == Focus::LogSearch, s, super::blink(tick), Face::Sans, F_LOGS);
            y += fh + 4.0 * s;
        }
        // the change buttons
        if self.det.tab == DTab::Summary || self.det.tab == DTab::Yaml {
            let acts = self.action_buttons();
            if !acts.is_empty() {
                if self.allow_changes() {
                    let mut ac = Cursor::new(x, y, w, size * 2.1, 6.0 * s);
                    for (code, label) in acts {
                        btn(g, &mut ac, size, label, ERR, Event::Tab(T_ACT + code), true);
                    }
                    y = ac.bottom() + 4.0 * s;
                } else {
                    let why = self.change_block().unwrap_or_else(|| "Read-only. Turn on Allow changes in the header to restart, scale or delete.".into());
                    y += para(g, x, y, w, 12.0 * s, DIM, &why, 2, true) + 4.0 * s;
                }
            }
        }
        let view = R::new(x, y, w, (r.y + r.h - y - 6.0 * s).max(20.0));
        self.draw_view(g, view, s);
        if let Some(p) = &self.panel {
            let _ = p;
        }
    }

    /// The scrolling text of the open tab.
    fn draw_view(&mut self, g: &mut Gfx, rect: R, s: f32) {
        let tab = self.det.tab;
        let mono = matches!(tab, DTab::Yaml | DTab::Logs);
        let (size, face) = if mono { (12.5 * s, Face::Mono) } else { (13.5 * s, Face::Sans) };
        let wrap = !mono || self.det.view.wrap;
        let lh = g.line_height(size, face) * 1.12;
        let fit = ((rect.h / lh).floor() as usize).max(1);
        let query = if tab == DTab::Logs { self.det.logs.search.value.to_ascii_lowercase() } else { String::new() };
        let inner = R::new(rect.x, rect.y, rect.w - 8.0 * s, rect.h);
        self.det.view.rect = rect;
        self.det.view.fit = fit;
        let n = self.det.view.lines.len();
        // follow mode keeps the end in view
        if self.det.view.stick {
            self.det.view.top = if wrap {
                let mut rows = 0;
                let mut top = n;
                while top > 0 {
                    let c = g.wrap(size, &self.det.view.lines[top - 1].text, face, inner.w).len().max(1);
                    if rows + c > fit {
                        break;
                    }
                    rows += c;
                    top -= 1;
                }
                top
            } else {
                n.saturating_sub(fit)
            };
        }
        self.det.view.top = self.det.view.top.min(n.saturating_sub(1));
        let top = self.det.view.top;
        let mut y = rect.y;
        let related = tab == DTab::Related;
        for i in top..n {
            let line = self.det.view.lines[i].clone();
            let pieces: Vec<String> = if wrap && mono {
                // keep the indentation of YAML and logs on every wrapped piece
                let body = line.text.trim_start_matches(' ');
                let indent = " ".repeat(line.text.len() - body.len());
                let room = (inner.w - g.text_width(size, &indent, face)).max(40.0);
                g.wrap(size, body, face, room).into_iter().map(|p| format!("{indent}{p}")).collect()
            } else if wrap {
                g.wrap(size, &line.text, face, inner.w)
            } else {
                vec![g.fit(size, &line.text, face, inner.w)]
            };
            let pieces = if pieces.is_empty() { vec![String::new()] } else { pieces };
            if y + lh * pieces.len() as f32 > rect.y + rect.h + 0.5 && i > top {
                break;
            }
            let start_y = y;
            for (pi, piece) in pieces.iter().enumerate() {
                if y + lh > rect.y + rect.h + 0.5 {
                    break;
                }
                if !query.is_empty() {
                    let lower = piece.to_ascii_lowercase();
                    let mut from = 0;
                    while let Some(p) = lower[from..].find(&query) {
                        let at = from + p;
                        if !piece.is_char_boundary(at) || !piece.is_char_boundary(at + query.len()) {
                            break;
                        }
                        let px = g.text_width(size, &piece[..at], face);
                        let pw = g.text_width(size, &piece[at..at + query.len()], face);
                        g.rect(inner.x + px, y, pw, lh, with_alpha(AMBER, 110));
                        from = at + query.len().max(1);
                        if from >= lower.len() {
                            break;
                        }
                    }
                }
                if pi == 0 && pieces.len() == 1 && line.key_len > 0 && line.key_len <= piece.chars().count() {
                    let kl: String = piece.chars().take(line.key_len).collect();
                    let vl: String = piece.chars().skip(line.key_len).collect();
                    let kw = g.text_width(size, &kl, face);
                    g.text(inner.x, y, size, KEY, &kl, face);
                    g.text(inner.x + kw, y, size, line.color, &vl, face);
                } else {
                    g.text(inner.x, y, size, line.color, piece, face);
                }
                y += lh;
            }
            if related {
                g.hit(R::new(rect.x, start_y, rect.w, (y - start_y).max(lh)), Event::Tab(T_REL_ROW + i), &line.text);
            }
        }
        scrollbar(g, rect, s, top, fit, n);
    }

    // ------------------------------------------------------------------------------------------------ overlays

    fn draw_overlays(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let _ = cx;
        if let Some(o) = self.overlay {
            let (title, h) = match o {
                Overlay::Clusters => ("Choose a cluster", 140.0 + 34.0 * self.entries.len() as f32),
                Overlay::Namespaces => ("Choose a namespace", 520.0),
                Overlay::Kinds => ("Choose what to look at", 520.0),
            };
            let p = fx::modal(g, area, s, 440.0 * s, h * s, ACCENT, false);
            let y = fx::modal_title(g, p, s, title, ACCENT);
            let mut ly = y + 2.0 * s;
            if o == Overlay::Namespaces {
                self.ns_filter.render(g, R::new(p.x + 20.0 * s, ly, p.w - 40.0 * s, 28.0 * s), self.focus == Focus::NsFilter, s, super::blink(tick), Face::Sans, F_NS);
                ly += 34.0 * s;
            }
            let list = R::new(p.x + 18.0 * s, ly, p.w - 36.0 * s, (p.y + p.h - ly - 56.0 * s).max(30.0));
            match o {
                Overlay::Clusters => {
                    self.clusters.rect = list;
                    self.draw_cluster_list(g, list, s);
                }
                Overlay::Namespaces => {
                    self.ns.rect = list;
                    self.draw_ns_list(g, list, s, true);
                }
                Overlay::Kinds => {
                    self.kinds.rect = list;
                    self.draw_kind_list(g, list, s, true);
                }
            }
            let bw = btn_w(g, 13.5 * s, "Close");
            g.button(p.x + p.w - 20.0 * s - bw, p.y + p.h - 46.0 * s, 13.5 * s, "Close", MUTED, Event::Tab(T_CLOSE));
            return;
        }
        if self.reveal_ask {
            let name = self.det.key.as_ref().map(|k| k.2.clone()).unwrap_or_default();
            let p = fx::modal(g, area, s, 520.0 * s, 250.0 * s, AMBER, false);
            let y = fx::modal_title(g, p, s, "Show the secret's values?", AMBER);
            let t = format!("The values of {name} will be shown in the YAML tab and can be copied. The reveal is recorded in the audit log by name; the values themselves never are.");
            para(g, p.x + 26.0 * s, y + 4.0 * s, p.w - 52.0 * s, 14.0 * s, TEXT, &t, 6, true);
            let by = p.y + p.h - 58.0 * s;
            let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Reveal", AMBER, Event::Tab(T_REVEAL_YES));
            g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_NO));
            return;
        }
        if self.pending.is_some() {
            self.draw_card(g, area, s, tick);
            return;
        }
        if self.panel.is_some() {
            self.draw_panel(g, area, s);
        }
    }

    fn draw_cluster_list(&mut self, g: &mut Gfx, list: R, s: f32) {
        let row_h = 34.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let n = self.entries.len();
        let top = fit_list(&mut self.clusters, n, fit);
        for k in top..(top + fit).min(n) {
            let e = self.entries[k].clone();
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 3.0 * s);
            let chosen = k == self.cur;
            let hot = g.hovered(rr);
            fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen, hot, ACCENT);
            if k == self.clusters.sel {
                g.rrect_outline(rr.x, rr.y, rr.w, rr.h, 6.0 * s, 1.5 * s, ACCENT);
            }
            let (chip, col) = if e.managed {
                let c = match &self.managed {
                    Managed::Running => OK,
                    Managed::Starting { .. } => AMBER,
                    Managed::Failed(_) => ERR,
                    _ => DIM,
                };
                (self.managed.chip().to_string(), c)
            } else {
                ("saved profile".to_string(), MUTED)
            };
            let cw = g.text_width(12.0 * s, &chip, Face::Bold);
            g.text(rr.x + rr.w - cw - 10.0 * s, ry + (row_h - 3.0 * s - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, col, &chip, Face::Bold);
            let t = g.fit(13.5 * s, &e.label, Face::Sans, rr.w - cw - 30.0 * s);
            g.text(rr.x + 10.0 * s, ry + (row_h - 3.0 * s - g.line_height(13.5 * s, Face::Sans)) / 2.0, 13.5 * s, TEXT, &t, Face::Sans);
            g.hit(rr, Event::Tab(T_CLUSTER_ROW + k), &e.label);
        }
        if n == 1 {
            let y = list.y + row_h + 4.0 * s;
            para(g, list.x + 4.0 * s, y, list.w - 8.0 * s, 12.5 * s, DIM, "Saved kubeconfig profiles appear here once you add them in Settings \u{2192} Dev cluster (the kubeconfig stays in the vault).", 4, true);
        }
    }

    /// The cluster line of the confirmation card: the action and the cluster by name and kind (`clusters::confirm_text*`).
    pub(super) fn card_cluster(&self, action: &kx::Action) -> String {
        let e = &self.entry().reg;
        let t = action.title();
        match action {
            kx::Action::DeletePod { .. } | kx::Action::Delete { .. } => iyakki_agent::clusters::confirm_text_from(&t, e),
            _ => iyakki_agent::clusters::confirm_text_on(&format!("{t} on"), e),
        }
    }

    fn draw_card(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let Some(p) = &self.pending else { return };
        let action = p.action.clone();
        let mut action_shown = action.clone();
        if let kx::Action::Scale { replicas, .. } = &mut action_shown {
            *replicas = p.replicas;
        }
        let danger = matches!(action, kx::Action::DeletePod { .. } | kx::Action::Delete { .. });
        let typed_needed = match action.confirm() {
            Confirm::TypeName(n) => Some(n),
            Confirm::Click => None,
        };
        let scaling = p.scaling.is_some();
        let h = 270.0 + if typed_needed.is_some() { 50.0 } else { 0.0 } + if scaling { 40.0 } else { 0.0 };
        let card = fx::modal(g, area, s, 540.0 * s, h * s, if danger { ERR } else { AMBER }, danger);
        let mut y = fx::modal_title(g, card, s, &action_shown.title(), if danger { ERR } else { AMBER });
        let (x, w) = (card.x + 26.0 * s, card.w - 52.0 * s);
        let where_ = format!("{}{}.", self.card_cluster(&action), match &action { kx::Action::Restart { ns, .. } | kx::Action::Scale { ns, .. } | kx::Action::DeletePod { ns, .. } | kx::Action::Delete { ns, .. } => format!(", namespace {ns}"), _ => String::new() });
        y += para(g, x, y + 2.0 * s, w, 13.0 * s, DIM, &where_, 2, true) + 8.0 * s;
        y += para(g, x, y, w, 14.5 * s, TEXT, &action_shown.warning(p.owned), 6, true) + 8.0 * s;
        if scaling {
            let sz = 14.0 * s;
            g.button(x, y, sz, "\u{2212}", MUTED, Event::Tab(T_SCALE_DN));
            let bw = btn_w(g, sz, "\u{2212}") + 10.0 * s;
            let num = p.replicas.to_string();
            g.text(x + bw, y + 4.0 * s, 20.0 * s, TEXT, &num, Face::Bold);
            let nw = g.text_width(20.0 * s, &num, Face::Bold) + 14.0 * s;
            g.button(x + bw + nw, y, sz, "+", MUTED, Event::Tab(T_SCALE_UP));
            let pw = btn_w(g, sz, "+");
            g.text(x + bw + nw + pw + 12.0 * s, y + 8.0 * s, 12.5 * s, DIM, "replicas (arrows or + and \u{2212} keys)", Face::Sans);
            y += 40.0 * s;
        }
        let mut ok = true;
        if let Some(n) = &typed_needed {
            g.text(x, y, 13.5 * s, MUTED, &format!("Type {n} to confirm:"), Face::Sans);
            p.typed.render(g, R::new(x, y + 20.0 * s, w, 28.0 * s), true, s, super::blink(tick), Face::Sans, F_NAME);
            ok = p.typed.value.trim() == n;
            y += 50.0 * s;
        }
        let _ = y;
        let by = card.y + card.h - 58.0 * s;
        let label = match &action {
            kx::Action::Restart { .. } => "Restart",
            kx::Action::Scale { .. } => "Scale",
            kx::Action::DeletePod { .. } | kx::Action::Delete { .. } => "Delete",
            kx::Action::Cordon { .. } => "Cordon",
            kx::Action::Uncordon { .. } => "Uncordon",
        };
        let w1 = if ok { g.button(x, by, 14.0 * s, label, if danger { ERR } else { AMBER }, Event::Tab(T_YES)) } else { g.button_off(x, by, 14.0 * s, label) };
        g.button(x + w1 + 12.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_NO));
        g.text(card.x + card.w - 170.0 * s, by + 8.0 * s, 12.0 * s, DIM, "Enter confirms \u{b7} Esc cancels", Face::Sans);
    }

    /// The small panel with the live output of a change, bottom right over the details.
    /// The cluster log: the last kubectl problems, newest at the bottom, with Copy.
    fn draw_cluster_log(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let (x, w) = (r.x + 14.0 * s, r.w - 28.0 * s);
        g.text(x, r.y + 12.0 * s, 16.0 * s, TEXT, "Cluster log", Face::Bold);
        let mut c = Cursor::new(x, r.y + 38.0 * s, w, 13.0 * s * 2.1, 8.0 * s);
        btn(g, &mut c, 13.0 * s, "Copy", ACCENT, Event::Tab(T_LOG_COPY), true);
        btn(g, &mut c, 13.0 * s, "Hide log", MUTED, Event::Tab(T_LOG), true);
        let top = r.y + 38.0 * s + 13.0 * s * 2.1 + 10.0 * s;
        let log = self.feed.cluster_log();
        let text = log.text();
        let lh = g.line_height(12.0 * s, Face::Mono) * 1.15;
        let fit = (((r.y + r.h - top - 8.0 * s) / lh) as usize).max(1);
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().rev().take(fit).rev().enumerate() {
            let t = g.fit(12.0 * s, l, Face::Mono, w);
            g.text(x, top + i as f32 * lh, 12.0 * s, if l.contains("persistent") { ERR } else { MUTED }, &t, Face::Mono);
        }
    }

    fn draw_panel(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(p) = &self.panel else { return };
        let lines: Vec<String> = p.lines.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let done = p.finished_at.is_some();
        let w = (area.w - 20.0 * s).min(520.0 * s);
        let h = 150.0 * s;
        let r = R::new(area.x + area.w - w - 10.0 * s, area.y + area.h - h - 10.0 * s, w, h);
        g.rrect(r.x, r.y + 3.0 * s, r.w, r.h, 10.0 * s, [0, 0, 0, 90]);
        g.rrect(r.x, r.y, r.w, r.h, 10.0 * s, rgb(17, 22, 42));
        g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 1.5 * s, if done { OK } else { AMBER });
        let title = if done { format!("{} \u{2014} finished", p.title) } else { format!("{} \u{2014} running\u{2026}", p.title) };
        let t = g.fit(13.0 * s, &title, Face::Bold, r.w - 90.0 * s);
        g.text(r.x + 10.0 * s, r.y + 8.0 * s, 13.0 * s, TEXT, &t, Face::Bold);
        let cb = btn_w(g, 12.0 * s, "Close");
        g.button(r.x + r.w - 10.0 * s - cb, r.y + 5.0 * s, 12.0 * s, "Close", MUTED, Event::Tab(T_PANEL_CLOSE));
        let lh = g.line_height(12.0 * s, Face::Mono) * 1.1;
        let fit = (((r.h - 40.0 * s) / lh) as usize).max(1);
        for (i, l) in lines.iter().rev().take(fit).rev().enumerate() {
            let t = g.fit(12.0 * s, l, Face::Mono, r.w - 20.0 * s);
            g.text(r.x + 10.0 * s, r.y + 34.0 * s + i as f32 * lh, 12.0 * s, if l.starts_with('$') { CYAN } else { MUTED }, &t, Face::Mono);
        }
        let _ = age_text;
    }
}
