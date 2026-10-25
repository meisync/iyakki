//! Drawing the AI IDE: the workspace rail, the top bar, the panes and their frames, the dividers, the status bar, the
//! welcome screen and the dialogs. The state and the events are in `ide.rs`.

use super::ide::*;
use super::ide_bottom::Tab as BTab;
use super::ide_chrome::{self as chrome, Metrics};
use super::ide_layout::{self as lay, Div, Pane};
use super::AppCx;
use crate::fx::{self, with_alpha, Glyph, AMBER, CYAN, MAGENTA, PANEL};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

fn pane_frame(g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
    let rad = chrome::cur().radius * s;
    if focused {
        g.rrect_glow(r.x, r.y, r.w, r.h, rad, 3.0 * s, with_alpha(accent, 34));
    }
    g.rrect(r.x, r.y, r.w, r.h, rad, PANEL);
    g.rrect_outline(r.x, r.y, r.w, r.h, rad, if focused { 1.2 * s } else { 1.0 }, with_alpha(accent, if focused { 190 } else { 44 }));
}

/// A pill-shaped button; returns its width. `ev` is what a click sends.
fn pill(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, color: [u8; 4], active: bool, ev: Event) -> f32 {
    let sz = 12.0 * s;
    let w = g.text_width(sz, label, Face::Bold) + 20.0 * s;
    let r = R::new(x, y, w, 26.0 * s);
    let hot = g.hovered(r);
    g.rrect(r.x, r.y, r.w, r.h, 13.0 * s, if active { fx::over(fx::WELL, with_alpha(color, 70)) } else if hot { rgb(36, 46, 82) } else { fx::WELL });
    g.rrect_outline(r.x, r.y, r.w, r.h, 13.0 * s, if active { 1.5 * s } else { 1.0 }, if active { color } else if hot { with_alpha(color, 200) } else { rgb(58, 68, 104) });
    g.text(r.x + 10.0 * s, r.y + (r.h - g.line_height(sz, Face::Bold)) / 2.0, sz, if active { TEXT } else { fx::readable(color, fx::WELL, 4.5) }, label, Face::Bold);
    g.hit(r, ev, label);
    w
}

/// The picture of a main tab.
pub fn main_glyph(t: MainTab) -> Glyph {
    match t {
        MainTab::Editor => Glyph::Editor,
        MainTab::Board => Glyph::Board,
        MainTab::Tasks => Glyph::Workloads,
        MainTab::Notes => Glyph::Note,
        MainTab::Browser => Glyph::Globe,
        MainTab::K8s => Glyph::K8s,
    }
}

/// The picture of a bottom panel tab.
pub fn bottom_glyph(t: BTab) -> Glyph {
    match t {
        BTab::Terminal => Glyph::Terminal,
        BTab::K8s => Glyph::K8s,
        BTab::Problems => Glyph::Problems,
        BTab::Search => Glyph::Search,
    }
}

fn pane_head_rect(g: &mut Gfx, r: R, s: f32, title: &str) -> R {
    chrome::pane_head(g, r, s, title)
}

fn activity_color(a: Activity) -> Option<[u8; 4]> {
    match a {
        Activity::Idle => None,
        Activity::Running => Some(CYAN),
        Activity::Waiting => Some(AMBER),
        Activity::Done => Some(OK),
        Activity::Error => Some(ERR),
    }
}

pub fn activity_text(a: Activity) -> &'static str {
    match a {
        Activity::Idle => "idle",
        Activity::Running => "assistant working",
        Activity::Waiting => "waiting for your approval",
        Activity::Done => "finished while you were away",
        Activity::Error => "assistant reported an error",
    }
}

impl IdeApp {
    pub(super) fn render_ide(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // the IDE draws at the window scale times its density; the embedded chat has its own, larger type
        let m = Metrics::of(self.density);
        chrome::set(m);
        self.base_scale = s;
        self.chat_scale = s * m.chat_k;
        let s = s * m.k;
        self.area = area;
        self.scale = s;
        self.mouse = g.mouse;
        g.gradient_rect(area.x, area.y, area.w, area.h, (0.0, area.y), (0.0, area.y + area.h), &[(0.0, rgb(12, 16, 32)), (1.0, rgb(7, 9, 20))]);
        let blink = (tick / 15) % 2 == 0;
        let Some(ai) = self.active else {
            self.layout = None;
            self.draw_welcome(g, area, s);
            self.draw_modal(g, area, s, blink);
            return;
        };
        let (full, shown) = {
            let w = &self.ws[ai];
            (w.board_full, w.compact_pane)
        };
        let prefs = self.ws[ai].prefs;
        let l = lay::compute(area, s, &prefs, shown);
        self.layout = Some(l.clone());
        let accent = self.ws[ai].meta.color();
        self.draw_top(g, &l, s, ai, accent);
        self.draw_rail(g, &l, s, ai, tick);
        let body = R::new(l.rail.x + l.rail.w, l.top.y + l.top.h, area.w - l.rail.w, l.status.y - l.top.y - l.top.h);
        if full {
            let w = &mut self.ws[ai];
            pane_frame(g, R::new(body.x + 4.0 * s, body.y + 2.0 * s, body.w - 8.0 * s, body.h - 4.0 * s), s, true, accent);
            w.board.full = true;
            w.board.draw(g, R::new(body.x + 8.0 * s, body.y + 6.0 * s, body.w - 16.0 * s, body.h - 12.0 * s), s, true, accent, tick);
        } else {
            if l.compact {
                self.draw_pane_tabs(g, &l, s, ai, accent);
            }
            if let Some(sb) = l.sidebar {
                let w = &mut self.ws[ai];
                let focused = w.focus == Pane::Tree;
                pane_frame(g, sb, s, focused, accent);
                let (root, editor) = (w.root(), &w.editor);
                let mark = |rel: &str| editor.tab_state(&root.join(rel));
                w.tree.draw(g, R::new(sb.x + 2.0 * s, sb.y + 4.0 * s, sb.w - 4.0 * s, sb.h - 8.0 * s), s, focused, accent, &mark);
            }
            if !l.compact || shown == Pane::Main {
                self.draw_main(g, l.main, s, cx, tick, ai, accent);
            }
            if let Some(b) = l.bottom {
                self.draw_bottom(g, b, s, ai, accent, tick);
            }
            if let Some(c) = l.chat {
                self.draw_chat(g, c, s, cx, tick, ai, accent);
            }
            // the dividers
            for (i, (d, r)) in l.dividers.iter().enumerate() {
                let idx = match d {
                    Div::Side => 0,
                    Div::Chat => 1,
                    Div::Bottom => 2,
                };
                let hot = g.hovered(*r) || self.drag_div.is_some_and(|x| x.0 == *d);
                let vertical = *d != Div::Bottom;
                if hot {
                    let (cxm, cym) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                    if vertical {
                        g.rrect(cxm - 1.5 * s, r.y + 8.0 * s, 3.0 * s, r.h - 16.0 * s, 1.5 * s, with_alpha(accent, 190));
                    } else {
                        g.rrect(r.x + 8.0 * s, cym - 1.5 * s, r.w - 16.0 * s, 3.0 * s, 1.5 * s, with_alpha(accent, 190));
                    }
                }
                let _ = i;
                g.hit(*r, Event::Area(A_DIV + idx), "divider");
            }
        }
        self.draw_overflow(g, &l, s);
        self.draw_status(g, &l, s, ai, accent);
        self.draw_modal(g, area, s, blink);
    }

    fn draw_top(&mut self, g: &mut Gfx, l: &lay::Layout, s: f32, ai: usize, accent: [u8; 4]) {
        let m = chrome::cur();
        let r = l.top;
        g.gradient_rect(r.x, r.y, r.w, r.h, (0.0, r.y), (0.0, r.y + r.h), &[(0.0, rgb(16, 21, 42)), (1.0, rgb(11, 15, 31))]);
        g.rect(r.x, r.y + r.h - 1.0, r.w, 1.0, with_alpha(accent, 60));
        let hint = self.last_hint.clone();
        let w = &self.ws[ai];
        let bh = super::ide_chrome::tool(s);
        let y = r.y + (r.h - bh) / 2.0;
        let bounds = R::new(r.x, r.y, r.w, g.height() as f32 - r.y);
        let sz = m.font_ui * s;
        // the project chooser: a dot, the name, a caret
        let name = g.fit(sz, &w.meta.name, Face::Bold, (r.w * 0.2).max(80.0 * s));
        let nw = g.text_width(sz, &name, Face::Bold);
        let chooser = R::new(r.x + 8.0 * s, y, nw + 44.0 * s, bh);
        let hot = g.hovered(chooser);
        g.rrect(chooser.x, chooser.y, chooser.w, chooser.h, 5.0 * s, if hot { rgb(34, 44, 78) } else { fx::WELL });
        g.circle(chooser.x + 12.0 * s, chooser.y + chooser.h / 2.0, 4.0 * s, accent);
        g.text(chooser.x + 22.0 * s, chooser.y + (bh - g.line_height(sz, Face::Bold)) / 2.0, sz, TEXT, &name, Face::Bold);
        g.text(chooser.x + chooser.w - 16.0 * s, chooser.y + (bh - g.line_height(sz, Face::Bold)) / 2.0, sz, chrome::muted(), "▾", Face::Bold);
        g.hit(chooser, Event::Tab(T_TOP), "project chooser");
        let mut x = chooser.x + chooser.w + 8.0 * s;
        let mut right = r.x + r.w - 8.0 * s;
        // the controls from the right: (label, button number, colour, lit). What does not fit goes under "⋯", which also holds
        // the settings, the density and "send to chat"
        let ro = w.ro.is_some();
        let target = iyakki_agent::target::Target::from_id(&w.cfg.target);
        let mut controls: Vec<(&str, usize, [u8; 4], bool, Glyph)> = vec![
            ("Chat", 4, MAGENTA, w.prefs.chat_open, Glyph::Chat),
            ("Panel", 3, CYAN, w.prefs.bottom_open, Glyph::Panel),
            ("Files", 2, CYAN, w.prefs.sidebar_open, Glyph::Files),
            ("Open…", 7, CYAN, false, Glyph::Open),
            ("Tests", 6, AMBER, false, Glyph::Tests),
            ("Run", 5, OK, false, Glyph::Run),
        ];
        // an app for IYAKKI: put it on my launcher, and publish it (these are the first to fold into the ⋯ menu)
        if target.is_some_and(|t| t.is_package()) {
            if target == Some(iyakki_agent::target::Target::IyakkiTile) {
                controls.push(("Add tile", 13, OK, false, Glyph::Services));
            }
            controls.push(("Publish", 14, MAGENTA, false, Glyph::Deploy));
        }
        let min_left = x + 150.0 * s;
        // the names go with the pictures while all of them fit; narrower, the buttons are pictures with a hover hint
        let (more_w, text_total) = {
            let mw = chrome::icon_btn_width(g, s, "⋯", false).max(bh) + 4.0 * s;
            let tt: f32 = controls.iter().map(|c| chrome::icon_btn_width(g, s, c.0, true) + 2.0 * s).sum();
            (mw, tt)
        };
        let show_text = right - more_w - text_total >= min_left;
        let btn_w = |g: &mut Gfx, label: &str| chrome::icon_btn_width(g, s, label, show_text);
        let mut shown = 0;
        let mut probe = right - more_w;
        for (label, ..) in &controls {
            let wd = btn_w(g, label) + 2.0 * s;
            if probe - wd >= min_left {
                probe -= wd;
                shown += 1;
            } else {
                break;
            }
        }
        let mut hidden: Vec<(String, usize, [u8; 4])> = controls.iter().skip(shown).map(|(l, n, c, ..)| (l.to_string(), *n, *c)).collect();
        hidden.push(("⚙ Settings…".to_string(), 1, MUTED));
        hidden.push((format!("Density: {}", self.density.name()), 10, MUTED));
        hidden.push(("→ Chat".to_string(), 8, MAGENTA));
        hidden.push(("New app, MCP server or skill…".to_string(), 12, CYAN));
        hidden.push(("Change target…".to_string(), 16, CYAN));
        hidden.push(("Clone repositories…".to_string(), 19, CYAN));
        hidden.push(("Connect Git / Gitea…".to_string(), 21, CYAN));
        if !w.cfg.repos.is_empty() {
            hidden.push(("Repositories…".to_string(), 20, CYAN));
        }
        if target.is_some_and(|t| t.is_package()) {
            hidden.push(("Validate and test app…".to_string(), 15, AMBER));
        }
        let more = more_w - 4.0 * s;
        right -= more;
        chrome::icon_btn(g, right, y, s, "⋯", CYAN, self.top_overflow_open, Event::Tab(T_TOP + 9), "⋯", bounds);
        right -= 4.0 * s;
        for (label, n, c, on, gl) in controls.into_iter().take(shown) {
            let wd = btn_w(g, label);
            right -= wd + 2.0 * s;
            let tip = match n {
                2 => "Show or hide the file tree (Ctrl+B)",
                3 => "Show or hide the bottom panel (Ctrl+J)",
                4 => "Show or hide the assistant",
                5 => if target == Some(iyakki_agent::target::Target::IyakkiTile) { "Run the app live (F5): opens it as an unsigned dev app; Back returns here" } else { "Run the open file (F5)" },
                13 => "Add this app to my launcher as an unsigned dev app (administrators)",
                14 => "Publish: checks, version, store, signing key, review, publish",
                6 => "Run the project's tests",
                _ => "Open a file (Ctrl+P)",
            };
            chrome::icon_btn_glyph(g, right, y, s, gl, label, show_text, c, on, true, Event::Tab(T_TOP + n), label, Some(tip), bounds);
        }
        let avail = right - x;
        // the chips that say something is happening (tasks, context, undo) come before the project path and the lesser chips: the
        // room they need is kept for them, the path and the namespace chip take what is left
        let chip_need = |g: &mut Gfx, label: &str| -> f32 {
            let sz = fx::size::CAPTION * s * 0.85;
            g.text_width(sz, label, Face::Bold) + sz * 1.4 + 6.0 * s
        };
        let mut prio = 0.0f32;
        if let Some((text, _)) = w.tasks.chip() {
            prio += chip_need(g, &format!("tasks: {text}"));
        }
        if let Some(text) = w.notes.chip() {
            let fresh = w.notes.counts().2;
            prio += chip_need(g, &if fresh > 0 { format!("{text} · {fresh} new") } else { text });
        }
        if w.ctx_undo.is_some() {
            prio += chip_need(g, "Undo context change");
        }
        let target_need = { let label = match target { Some(t) => format!("target: {}", t.label()), None => "target: not chosen".to_string() }; chip_need(g, &label) };
        let path_room = (avail * 0.5).min(avail - prio - target_need - 12.0 * s);
        if avail > 100.0 * s && path_room >= 80.0 * s {
            let root = w.root().display().to_string();
            let psz = m.font_small * s;
            let path = g.fit(psz, &root, Face::Mono, path_room);
            let pw = g.text_width(psz, &path, Face::Mono);
            g.text(x, y + (bh - g.line_height(psz, Face::Mono)) / 2.0, psz, chrome::dim(), &path, Face::Mono);
            x += pw + 10.0 * s;
        }
        // what the workspace is for (click to change it)
        {
            let label = match target {
                Some(t) => format!("target: {}", t.label()),
                None => "target: not chosen".to_string(),
            };
            let need = { let sz = fx::size::CAPTION * s * 0.85; g.text_width(sz, &label, Face::Bold) + sz * 1.4 + 6.0 * s };
            if right - x > need {
                let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, &label, if target.is_some() { CYAN } else { AMBER }, target.is_none());
                g.hit(R::new(x, y, cw, bh), Event::Tab(T_TOP + 16), "change the target");
                x += cw + 6.0 * s;
                self.last_target_chip = Some(R::new(x - cw - 6.0 * s, y, cw, bh));
            } else {
                self.last_target_chip = None;
            }
        }
        // the project's git: who it is connected to, or the prompt to connect it (click: the panel, or the connect flow)
        if let Some(head) = self.git_head() {
            let (label, n, col, loud) = match &head.origin {
                Some(_) => (format!("git: {}", head.conn.clone().unwrap_or_else(|| head.host.clone())), 20, CYAN, false),
                None => ("Connect Git / Gitea".to_string(), 21, AMBER, true),
            };
            let need = { let sz = fx::size::CAPTION * s * 0.85; g.text_width(sz, &label, Face::Bold) + sz * 1.4 + 6.0 * s };
            if right - x > need + prio + 60.0 * s {
                let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, &label, col, loud);
                g.hit(R::new(x, y, cw, bh), Event::Tab(T_TOP + n), if loud { "Connect Git / Gitea" } else { "Git: the repositories panel" });
                x += cw + 6.0 * s;
            }
        }
        if let Some(h) = hint.as_ref().filter(|h| { let sz = fx::size::CAPTION * s * 0.85; right - x > g.text_width(sz, h, Face::Bold) + sz * 1.4 + 130.0 * s + prio }) {
            let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, h, if h.contains("outdated") || h.contains("update available") { AMBER } else { OK }, false);
            x += cw + 6.0 * s;
        }
        if right - x > 90.0 * s + prio && target.is_none_or(|t| t.deploys_to_k3s()) {
            let ns = format!("ns {}", w.target().namespace);
            let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, &ns, if w.ns_warning.is_some() { AMBER } else { MAGENTA }, w.ns_warning.is_some());
            x += cw + 6.0 * s;
        }
        if let Some((text, needs)) = w.tasks.chip() {
            let label = format!("tasks: {text}");
            if right - x > chip_need(g, &label).max(60.0 * s) {
                let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, &label, if needs { AMBER } else { CYAN }, true);
                g.hit(R::new(x, y, cw, bh), Event::Tab(T_TOP + 11), "open the Tasks tab");
                x += cw + 6.0 * s;
            }
        }
        if let Some(text) = w.notes.chip() {
            let fresh0 = w.notes.counts().2;
            if right - x > chip_need(g, &if fresh0 > 0 { format!("{text} · {fresh0} new") } else { text.clone() }).max(60.0 * s) {
                let fresh = w.notes.counts().2;
                let label = if fresh > 0 { format!("{text} · {fresh} new") } else { text };
                let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, &label, if fresh > 0 { AMBER } else { CYAN }, false);
                g.hit(R::new(x, y, cw, bh), Event::Tab(T_TOP + 17), "open the Context tab");
                x += cw + 6.0 * s;
            }
        }
        if w.ctx_undo.is_some() && right - x > chip_need(g, "Undo context change") {
            let cw = fx::chip(g, x, y + 1.0 * s, s * 0.85, "Undo context change", MAGENTA, true);
            g.hit(R::new(x, y, cw, bh), Event::Tab(T_TOP + 18), "Undo context change");
            x += cw + 6.0 * s;
        }
        if ro && right - x > 120.0 * s {
            let who = w.ro.as_ref().map_or(String::new(), |l| if l.same_account_here(&self.user) { format!("window {}", l.pid) } else { l.user.clone() });
            fx::chip(g, x, y + 1.0 * s, s * 0.85, &format!("read-only · held by {who}"), AMBER, true);
        }
        self.top_overflow = hidden;
    }

    /// The dropdown of the top bar's "⋯" button: the buttons that did not fit, the settings and the density.
    fn draw_overflow(&mut self, g: &mut Gfx, l: &lay::Layout, s: f32) {
        if !self.top_overflow_open || self.top_overflow.is_empty() {
            return;
        }
        let m = chrome::cur();
        let (rh, pw) = (26.0 * s, 190.0 * s);
        let r = R::new(l.top.x + l.top.w - pw - 8.0 * s, l.top.y + l.top.h + 2.0 * s, pw, rh * self.top_overflow.len() as f32 + 8.0 * s);
        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, rgb(20, 26, 50));
        g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.0, rgb(70, 82, 120));
        for (i, (label, n, c)) in self.top_overflow.iter().enumerate() {
            let row = R::new(r.x + 4.0 * s, r.y + 4.0 * s + i as f32 * rh, r.w - 8.0 * s, rh);
            if g.hovered(row) {
                g.rrect(row.x, row.y, row.w, row.h - 1.0 * s, 4.0 * s, rgb(36, 46, 82));
            }
            let sz = m.font_ui * s;
            g.text(row.x + 8.0 * s, row.y + (rh - g.line_height(sz, Face::Bold)) / 2.0, sz, fx::readable(*c, rgb(20, 26, 50), chrome::CONTRAST), label, Face::Bold);
            g.hit(row, Event::Tab(T_TOP + n), label);
        }
    }

    fn draw_rail(&mut self, g: &mut Gfx, l: &lay::Layout, s: f32, ai: usize, tick: u64) {
        let r = l.rail;
        g.rect(r.x, r.y, r.w, r.h, rgb(9, 12, 26));
        g.rect(r.x + r.w - 1.0, r.y, 1.0, r.h, [255, 255, 255, 14]);
        let order = self.reg.ordered();
        // a slim activity bar: round letters that fit the rail with a margin on both sides
        let d = (r.w - 8.0 * s).min(26.0 * s).max(18.0);
        let gap = 6.0 * s;
        let mut y = r.y + 6.0 * s;
        let active_id = self.ws[ai].meta.id.clone();
        let sz = chrome::cur().font_ui * s;
        for (i, wsmeta) in order.iter().enumerate() {
            if y + d > r.y + r.h - d - 12.0 * s {
                break;
            }
            let cx = r.x + r.w / 2.0;
            let c = wsmeta.color();
            let rect = R::new(cx - d / 2.0, y, d, d);
            let hot = g.hovered(rect);
            let on = wsmeta.id == active_id;
            if on {
                g.rrect(r.x, y + 4.0 * s, 2.0 * s.max(1.0), d - 8.0 * s, 1.0 * s, c);
            }
            g.circle(cx, y + d / 2.0, d / 2.0, if on { fx::over(rgb(14, 18, 36), with_alpha(c, 70)) } else if hot { rgb(30, 38, 70) } else { rgb(20, 26, 50) });
            g.ring(cx, y + d / 2.0, d / 2.0, if on { 1.6 * s } else { 1.0 }, with_alpha(c, if on { 255 } else if hot { 200 } else { 110 }));
            let ini: String = wsmeta.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let iw = g.text_width(sz, &ini, Face::Bold);
            g.text(cx - iw / 2.0, y + (d - g.line_height(sz, Face::Bold)) / 2.0, sz, if on { TEXT } else { fx::readable(c, rgb(20, 26, 50), chrome::CONTRAST) }, &ini, Face::Bold);
            // the activity dot
            if let Some(w) = self.ws.iter().find(|w| w.meta.id == wsmeta.id) {
                if let Some(ac) = activity_color(w.activity()) {
                    let pulse = if w.activity() == Activity::Running || w.activity() == Activity::Waiting { 0.6 + 0.4 * ((tick as f32 / 6.0).sin().abs()) } else { 1.0 };
                    let (dx, dy) = (rect.x + rect.w - 3.0 * s, rect.y + 3.0 * s);
                    g.circle(dx, dy, 5.5 * s * pulse, with_alpha(ac, 60));
                    g.circle(dx, dy, 3.5 * s, ac);
                    g.ring(dx, dy, 3.5 * s, 1.0, rgb(9, 12, 26));
                }
            }
            g.hit(rect, Event::Tab(T_RAIL + i), &wsmeta.name);
            if hot {
                let act = self.ws.iter().find(|w| w.meta.id == wsmeta.id).map(|w| activity_text(w.activity())).unwrap_or("not open");
                let tip = format!("{} · {}{}", wsmeta.name, act, if wsmeta.pinned { " · pinned" } else { "" });
                chrome::tooltip(g, R::new(r.x + r.w + 2.0 * s, rect.y, 0.0, rect.h), s, &tip, R::new(r.x, r.y, g.width() as f32 - r.x, r.h));
            }
            y += d + gap;
        }
        // the plus button
        let cx = r.x + r.w / 2.0;
        let pr = R::new(cx - d / 2.0, r.y + r.h - d - 8.0 * s, d, d);
        let hot = g.hovered(pr);
        g.circle(cx, pr.y + d / 2.0, d / 2.0, if hot { rgb(36, 46, 82) } else { rgb(16, 21, 42) });
        g.ring(cx, pr.y + d / 2.0, d / 2.0, 1.0, if hot { CYAN } else { rgb(70, 82, 120) });
        let pw = g.text_width(sz * 1.3, "+", Face::Bold);
        g.text(cx - pw / 2.0, pr.y + (d - g.line_height(sz * 1.3, Face::Bold)) / 2.0, sz * 1.3, if hot { CYAN } else { chrome::muted() }, "+", Face::Bold);
        g.hit(pr, Event::Tab(T_RAIL_ADD), "workspaces");
    }

    fn draw_pane_tabs(&mut self, g: &mut Gfx, l: &lay::Layout, s: f32, ai: usize, accent: [u8; 4]) {
        let Some(r) = l.pane_tabs else { return };
        let w = &self.ws[ai];
        let mut x = r.x + 4.0 * s;
        let bounds = R::new(0.0, 0.0, g.width() as f32, g.height() as f32);
        let items = |p: &Pane| match p {
            Pane::Tree => ("Files", Glyph::Files),
            Pane::Main => ("Editor", Glyph::Editor),
            Pane::Chat => ("Chat", Glyph::Chat),
            Pane::Bottom => ("Panel", Glyph::Panel),
        };
        let total: f32 = Pane::ALL.iter().map(|p| chrome::icon_tab_width(g, s, items(p).0, true, true, None)).sum();
        let show_text = total <= r.w - 8.0 * s;
        for (i, p) in Pane::ALL.iter().enumerate() {
            let (label, gl) = items(p);
            let act = *p == Pane::Chat && w.activity() != Activity::Idle;
            let col = if act { activity_color(w.activity()).unwrap_or(accent) } else { accent };
            x += chrome::icon_tab(g, x, r.y, s, gl, label, show_text, None, w.compact_pane == *p, col, Event::Tab(T_PANE + i), label, bounds);
        }
        chrome::strip_line(g, r.x, r.y + r.h - 1.0, r.w);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_main(&mut self, g: &mut Gfx, r: R, s: f32, cx: &AppCx, tick: u64, ai: usize, accent: [u8; 4]) {
        let m = chrome::cur();
        let no_dialog = self.modal.is_none();
        let w = &mut self.ws[ai];
        let focused = w.focus == Pane::Main;
        pane_frame(g, r, s, focused, accent);
        // the tab strip: Editor | Board | Browser | K8s, flat text with an underline under the open one
        let mut x = r.x + 4.0 * s;
        let y = r.y + 2.0 * s;
        let counts = w.board.file.as_ref().map(|f| f.board.len());
        let bounds = R::new(0.0, 0.0, g.width() as f32, g.height() as f32);
        let items: Vec<(MainTab, String, Glyph)> = MainTab::ALL
            .iter()
            .map(|t| {
                let label = match (t, counts) {
                    (MainTab::Board, Some(n)) => format!("Board {n}"),
                    (MainTab::Editor, _) if w.editor.any_unsaved() => "Editor ●".to_string(),
                    (MainTab::Notes, _) if w.notes.counts().0 > 0 => format!("Context {}", w.notes.counts().0),
                    _ => t.label().to_string(),
                };
                (*t, label, main_glyph(*t))
            })
            .collect();
        // the names go with the pictures while they fit (the "board unsaved" note needs room too); narrower: pictures only
        let need: f32 = items.iter().map(|(t, l, _)| chrome::icon_tab_width(g, s, l, true, w.main_tab == *t, None)).sum::<f32>() + if w.board.is_dirty() { 110.0 * s } else { 0.0 };
        let show_text = need <= r.w - 8.0 * s;
        for (i, (t, label, gl)) in items.iter().enumerate() {
            x += chrome::icon_tab(g, x, y, s, *gl, label, show_text, None, w.main_tab == *t, accent, Event::Tab(T_MAIN + i), label, bounds);
        }
        if w.board.is_dirty() {
            g.text(x + 6.0 * s, y + (m.tab_h * s - g.line_height(m.font_small * s, Face::Bold)) / 2.0, m.font_small * s, AMBER, "board unsaved", Face::Bold);
        }
        chrome::focus_line(g, r.x + 1.0, y + m.tab_h * s, r.w - 2.0, s, focused && no_dialog, accent);
        let top = m.tab_h * s + 3.0 * s;
        let content = R::new(r.x + 2.0 * s, r.y + top, r.w - 4.0 * s, (r.h - top - 2.0 * s).max(0.0));
        match w.main_tab {
            MainTab::Editor => {
                let editor = &mut w.editor;
                // only the pane that owns the keyboard may show a caret
                super::with_caret(focused && no_dialog, || render_child(g, Pane::Main, |g| super::App::render(editor, g, content, s, cx, tick)));
            }
            MainTab::Board => w.board.draw(g, R::new(content.x + 4.0 * s, content.y + 2.0 * s, content.w - 8.0 * s, content.h - 4.0 * s), s, focused && no_dialog, accent, tick),
            MainTab::Browser => {
                if !cx.session.policy.apps.contains(&iyakki_sec::policy::App::Browser) {
                    let msg = "Your account may not use the browser.";
                    g.text(content.x + 14.0 * s, content.y + 14.0 * s, 14.0 * s, MUTED, msg, Face::Sans);
                } else {
                    IdeApp::ensure_browser(w, cx);
                    let view = super::component::View { scale: s, tick, focused: focused && no_dialog };
                    if let Some(b) = w.browser.as_mut() {
                        super::component::route::render_slot(g, super::component::route::BROWSER, |g| super::component::Component::render(b, g, content, &view, cx));
                    }
                }
            }
            MainTab::Notes => w.notes.draw(g, R::new(content.x + 4.0 * s, content.y + 2.0 * s, content.w - 8.0 * s, content.h - 4.0 * s), s, focused && no_dialog, accent, tick),
            MainTab::Tasks => {
                w.tasks.set_root(&w.meta.root_path().display().to_string());
                w.tasks.draw(g, R::new(content.x + 4.0 * s, content.y + 2.0 * s, content.w - 8.0 * s, content.h - 4.0 * s), s, focused && no_dialog, accent, tick)
            }
            MainTab::K8s => {
                let (name, warn) = (w.meta.name.clone(), w.ns_warning.clone());
                w.k8s_view().draw(g, content, s, focused, accent, tick, &name, warn.as_deref());
            }
        }
    }

    fn draw_bottom(&mut self, g: &mut Gfx, r: R, s: f32, ai: usize, accent: [u8; 4], tick: u64) {
        let m = chrome::cur();
        let no_dialog = self.modal.is_none();
        let w = &mut self.ws[ai];
        let focused = w.focus == Pane::Bottom;
        pane_frame(g, r, s, focused, accent);
        let mut x = r.x + 4.0 * s;
        let y = r.y + 1.0 * s;
        let bounds = R::new(0.0, 0.0, g.width() as f32, g.height() as f32);
        let errors = w.problems.lines.iter().filter(|l| l.color == ERR).count();
        let items: Vec<(BTab, String, Glyph, Option<usize>)> = BTab::ALL
            .iter()
            .map(|t| {
                let busy = match t {
                    BTab::Terminal => w.term.runner.busy(),
                    BTab::Problems => w.problems.runner.busy(),
                    BTab::Search => w.search.busy(),
                    BTab::K8s => false,
                };
                let label = if busy { format!("{} …", t.label()) } else { t.label().to_string() };
                (*t, label, bottom_glyph(*t), (*t == BTab::Problems && errors > 0).then_some(errors))
            })
            .collect();
        let need: f32 = items.iter().map(|(t, l, _, b)| chrome::icon_tab_width(g, s, l, true, w.bottom_tab == *t, *b)).sum();
        let show_text = need <= r.w - 8.0 * s;
        for (i, (t, label, gl, badge)) in items.iter().enumerate() {
            x += chrome::icon_tab(g, x, y, s, *gl, label, show_text, *badge, w.bottom_tab == *t, accent, Event::Tab(T_BOTTOM + i), t.label(), bounds);
        }
        chrome::focus_line(g, r.x + 1.0, y + m.tab_h * s, r.w - 2.0, s, focused && no_dialog, accent);
        let top = m.tab_h * s + 2.0 * s;
        let content = R::new(r.x, r.y + top, r.w, (r.h - top - 2.0 * s).max(0.0));
        let typing = focused && no_dialog;
        let blink = typing && (tick / 15) % 2 == 0;
        match w.bottom_tab {
            BTab::Terminal => w.term.draw(g, content, s, typing, accent, blink),
            // the cluster view is drawn once, as the K8s tab of the main area; here is only its one-line status
            BTab::K8s => {
                let center = w.main_tab == MainTab::K8s;
                w.k8s_view().strip(g, content, s, center);
            }
            BTab::Problems => w.problems.draw(g, content, s, focused, accent),
            BTab::Search => w.search.draw(g, content, s, typing, accent, blink),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_chat(&mut self, g: &mut Gfx, r: R, s: f32, cx: &AppCx, tick: u64, ai: usize, accent: [u8; 4]) {
        let m = chrome::cur();
        let cs = self.chat_scale;
        let no_dialog = self.modal.is_none();
        let w = &mut self.ws[ai];
        let focused = w.focus == Pane::Chat;
        pane_frame(g, r, s, focused, accent);
        let head = pane_head_rect(g, r, s, "Assistant");
        let act = w.activity();
        let label = format!("{} · {}", w.meta.name, activity_text(act));
        let sz = m.font_small * s;
        let maxw = (r.w - 120.0 * s).max(40.0 * s);
        let label = g.fit(sz, &label, Face::Sans, maxw);
        let lw = g.text_width(sz, &label, Face::Sans);
        let ty = head.y + (head.h - g.line_height(sz, Face::Sans)) / 2.0;
        let col = activity_color(act).map_or(chrome::dim(), |c| fx::readable(c, fx::PANEL, chrome::CONTRAST));
        g.text(r.x + r.w - 10.0 * s - lw, ty, sz, col, &label, Face::Sans);
        if let Some(c) = activity_color(act) {
            g.circle(r.x + r.w - 18.0 * s - lw, head.y + head.h / 2.0, 3.0 * s, c);
        }
        chrome::focus_line(g, r.x + 1.0, r.y + head.h - 1.0, r.w - 2.0, s, focused && no_dialog, accent);
        let content = R::new(r.x + 2.0 * s, r.y + head.h, r.w - 4.0 * s, (r.h - head.h - 2.0 * s).max(0.0));
        let chat = &mut w.chat;
        super::with_caret(focused && no_dialog, || render_child(g, Pane::Chat, |g| super::App::render(chat, g, content, cs, cx, tick)));
    }

    fn draw_status(&mut self, g: &mut Gfx, l: &lay::Layout, s: f32, ai: usize, accent: [u8; 4]) {
        let m = chrome::cur();
        let r = l.status;
        g.rect(r.x, r.y, r.w, r.h, rgb(9, 12, 26));
        g.rect(r.x, r.y, r.w, 1.0, with_alpha(accent, 44));
        let w = &self.ws[ai];
        let (msg, ok) = if let Some(n) = &self.notice {
            (n.0.clone(), n.1)
        } else if let Some(m) = &w.status {
            m.clone()
        } else if let Some(m) = w.editor.status() {
            m.clone()
        } else {
            ("Ctrl+P open file · Ctrl+Shift+W workspaces · Ctrl+Shift+B board · F1 keys".to_string(), true)
        };
        let col = if msg.starts_with("Ctrl+P") { chrome::dim() } else if ok { fx::readable(OK, rgb(9, 12, 26), chrome::CONTRAST) } else { fx::readable(ERR, rgb(9, 12, 26), chrome::CONTRAST) };
        let sz = m.font_small * s;
        let right = format!("{} · {} · {}", w.focus.name(), activity_text(w.activity()), self.density.name());
        let right = g.fit(sz, &right, Face::Sans, r.w * 0.4);
        let rw = g.text_width(sz, &right, Face::Sans);
        let ty = r.y + (r.h - g.line_height(sz, Face::Sans)) / 2.0;
        let t = g.fit(sz, &msg, Face::Sans, r.w - rw - 36.0 * s);
        g.text(r.x + 10.0 * s, ty, sz, col, &t, Face::Sans);
        g.text(r.x + r.w - rw - 10.0 * s, ty, sz, chrome::dim(), &right, Face::Sans);
        if t != msg {
            let b = R::new(r.x, r.y, r.w - rw - 20.0 * s, r.h);
            if g.hovered(b) {
                chrome::tooltip(g, R::new(r.x + 10.0 * s, r.y, 0.0, r.h), s, &msg, R::new(r.x, 0.0, r.w, r.y));
            }
        }
    }

    fn draw_welcome(&mut self, g: &mut Gfx, area: R, s: f32) {
        let accent = ACCENT;
        let card_w = (680.0 * s).min(area.w - 40.0 * s);
        let card = R::new(area.x + (area.w - card_w) / 2.0, area.y + (area.h * 0.12).max(16.0 * s), card_w, (area.h * 0.76).min(480.0 * s));
        fx::card(g, card, s, accent, 0.3);
        let cx = card.x + card.w / 2.0;
        g.radial_glow(cx, card.y + 70.0 * s, 90.0 * s, with_alpha(accent, 36));
        g.ring(cx, card.y + 70.0 * s, 34.0 * s, 1.5 * s, with_alpha(accent, 160));
        g.icon(Icon::Ide, cx, card.y + 70.0 * s, 36.0 * s, accent);
        let title = "AI IDE: one workspace per product";
        let tw = g.text_width(24.0 * s, title, Face::Bold);
        g.text(cx - tw / 2.0, card.y + 118.0 * s, 24.0 * s, TEXT, title, Face::Bold);
        let text = "A workspace is a folder with its own files, editor tabs, terminal, cluster view, feature board and chats. Products never mix: the assistant in a workspace can only reach that workspace's folder.";
        let mut y = card.y + 160.0 * s;
        for l in g.wrap(14.0 * s, text, Face::Sans, card.w - 80.0 * s).iter().take(4) {
            let lw = g.text_width(14.0 * s, l, Face::Sans);
            g.text(cx - lw / 2.0, y, 14.0 * s, MUTED, l, Face::Sans);
            y += 21.0 * s;
        }
        y += 14.0 * s;
        let labels = [("New workspace  n", 'n', accent), ("Open existing folder  o", 'o', CYAN), ("All workspaces  w", 'w', MAGENTA)];
        let total: f32 = labels.iter().map(|(l, _, _)| g.text_width(14.0 * s, l, Face::Bold) + 14.0 * s * 1.7 + 12.0 * s).sum();
        let mut x = (cx - total / 2.0).max(card.x + 14.0 * s);
        for (l, k, c) in labels {
            if x + 100.0 * s > card.x + card.w {
                x = card.x + 20.0 * s;
                y += 46.0 * s;
            }
            x += g.button(x, y, 14.0 * s, l, c, Event::Sub(SUB, Key::Char(k))) + 12.0 * s;
        }
        y += 60.0 * s;
        if let Some((m, ok)) = &self.notice {
            let t = g.fit(13.0 * s, m, Face::Sans, card.w - 60.0 * s);
            g.text(card.x + 30.0 * s, y, 13.0 * s, if *ok { OK } else { ERR }, &t, Face::Sans);
            y += 24.0 * s;
        }
        let n = self.reg.items.len();
        let t = format!("{n} workspace{} in your list · new ones go to ~/Workspaces/<name>, with a .iyakki/project.json inside", if n == 1 { "" } else { "s" });
        let t = g.fit(12.0 * s, &t, Face::Sans, card.w - 60.0 * s);
        g.text(card.x + 30.0 * s, y, 12.0 * s, DIM, &t, Face::Sans);
    }

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, blink: bool) {
        let accent = self.active_ws().map_or(ACCENT, |w| w.meta.color());
        if let Some(Modal::Card(cm)) = &mut self.modal {
            cm.card.render(g, area, s);
            return;
        }
        if matches!(self.modal, Some(Modal::Git(_))) {
            let (rows, _) = self.repo_rows();
            let note = if self.git.note.is_empty() {
                self.active_ws().map(|w| w.cfg.git_keys.iter().map(|(h, k)| format!("{h} → {k}")).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty()).map(|s| format!("Keys for this workspace: {s}")).unwrap_or_default()
            } else {
                self.git.note.clone()
            };
            let head = self.git_head();
            if let Some(Modal::Git(gm)) = &mut self.modal {
                if let super::ide_git::GitModal::Repos(p) = &mut **gm {
                    p.head = head;
                }
                gm.render(g, area, s, accent, blink, &rows, &note);
            }
            return;
        }
        let Some(m) = &self.modal else { return };
        match m {
            Modal::Git(_) => {}
            Modal::Form(_, f) => f.render(g, area, s, accent, blink),
            Modal::Browse(_, _, p) => p.render(g, area, s, accent),
            Modal::Card(_) => {}
            Modal::Info(t, msg) => {
                let lines = g.wrap(14.0 * s, msg, Face::Sans, 540.0 * s - 52.0 * s);
                let h = (130.0 + lines.len() as f32 * 20.0) * s;
                let p = fx::modal(g, area, s, 560.0 * s, h, accent, false);
                fx::modal_title(g, p, s, t, accent);
                for (i, l) in lines.iter().enumerate() {
                    g.text(p.x + 26.0 * s, p.y + 66.0 * s + i as f32 * 20.0 * s, 14.0 * s, MUTED, l, Face::Sans);
                }
                g.button(p.x + p.w - 26.0 * s - 70.0 * s, p.y + p.h - 52.0 * s, 14.0 * s, "OK", accent, Event::Sub(SUB, Key::Enter));
            }
            Modal::Confirm(c) => {
                let (title, detail, yes) = match c {
                    Confirm::DeleteFile(rel) => (format!("Delete {rel}?"), "The file or folder (with everything in it) is deleted from the disk. This cannot be undone.".to_string(), "Delete"),
                    Confirm::MergeTask(id, what) => {
                        let t = self.active_ws().and_then(|w| w.tasks.mgr.get(id)).map_or(id.clone(), |t| t.title);
                        (format!("Merge \"{t}\"?"), format!("{what}. The task's branch is merged into the branch you have checked out (git merge --no-ff), or its patch is applied. Nothing is deployed."), "Merge")
                    }
                    Confirm::DiscardTask(id) => {
                        let t = self.active_ws().and_then(|w| w.tasks.mgr.get(id)).map_or(id.clone(), |t| t.title);
                        (format!("Discard \"{t}\"?"), "The task's folder and branch are deleted with everything it changed. Your workspace is not touched. This cannot be undone.".to_string(), "Discard")
                    }
                    Confirm::RemoveWs(id) => {
                        let n = self.reg.get(id).map_or(id.clone(), |w| w.name.clone());
                        (format!("Remove \"{n}\" from the list?"), "Only the entry is removed. The folder and its files are not touched.".to_string(), "Remove")
                    }
                };
                let p = fx::modal(g, area, s, 560.0 * s, 180.0 * s, ERR, true);
                let t = g.fit(19.0 * s, &title, Face::Bold, p.w - 48.0 * s);
                g.text(p.x + 24.0 * s, p.y + 24.0 * s, 19.0 * s, TEXT, &t, Face::Bold);
                for (i, l) in g.wrap(14.0 * s, &detail, Face::Sans, p.w - 48.0 * s).iter().take(3).enumerate() {
                    g.text(p.x + 24.0 * s, p.y + 62.0 * s + i as f32 * 19.0 * s, 14.0 * s, MUTED, l, Face::Sans);
                }
                g.button(p.x + 24.0 * s, p.y + p.h - 56.0 * s, 14.0 * s, yes, ERR, Event::Sub(SUB, Key::Char('y')));
                g.button(p.x + 150.0 * s, p.y + p.h - 56.0 * s, 14.0 * s, "Cancel", MUTED, Event::Sub(SUB, Key::Esc));
            }
            Modal::Lock(i) => {
                let who = self.ws.get(*i).and_then(|w| w.ro.clone());
                let p = fx::modal(g, area, s, 640.0 * s, 230.0 * s, AMBER, false);
                g.text(p.x + 26.0 * s, p.y + 24.0 * s, 20.0 * s, TEXT, "This workspace is open elsewhere", Face::Bold);
                let msg = match who {
                    Some(l) => l.describe(&self.user),
                    None => "Another session has this workspace open.".to_string(),
                };
                for (j, l) in g.wrap(14.0 * s, &msg, Face::Sans, p.w - 52.0 * s).iter().take(5).enumerate() {
                    g.text(p.x + 26.0 * s, p.y + 62.0 * s + j as f32 * 20.0 * s, 14.0 * s, MUTED, l, Face::Sans);
                }
                let by = p.y + p.h - 58.0 * s;
                let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Open read-only  r", CYAN, Event::Sub(SUB, Key::Char('r')));
                let w2 = g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Take over  t", AMBER, Event::Sub(SUB, Key::Char('t')));
                g.button(p.x + 26.0 * s + w1 + w2 + 24.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Sub(SUB, Key::Esc));
            }
            Modal::Quick(q) => {
                let p = fx::modal(g, area, s, 640.0 * s, 460.0 * s, accent, false);
                fx::modal_title(g, p, s, "Open file", accent);
                let fr = R::new(p.x + 26.0 * s, p.y + 62.0 * s, p.w - 52.0 * s, 38.0 * s);
                q.query.render(g, fr, true, s, blink, Face::Sans, 77);
                let rh = 28.0 * s;
                let rows = (((p.h - 130.0 * s) / rh) as usize).max(1);
                let start = q.sel.saturating_sub(rows - 1);
                for (i, f) in q.results.iter().enumerate().skip(start).take(rows) {
                    let y = p.y + 110.0 * s + (i - start) as f32 * rh;
                    let rect = R::new(p.x + 16.0 * s, y, p.w - 32.0 * s, rh - 3.0 * s);
                    let hot = g.hovered(rect);
                    if i == q.sel || hot {
                        fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 8.0 * s, i == q.sel, hot, accent);
                    }
                    let name = f.rsplit('/').next().unwrap_or(f);
                    super::ide_tree::file_icon(g, rect.x + 22.0 * s, rect.y + rect.h / 2.0, s, name);
                    let t = g.fit(14.0 * s, name, Face::Bold, rect.w * 0.4);
                    g.text(rect.x + 40.0 * s, rect.y + 5.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
                    let tw = g.text_width(14.0 * s, &t, Face::Bold);
                    let dir = f.rsplit_once('/').map_or(String::new(), |x| x.0.to_string());
                    let d = g.fit(12.0 * s, &dir, Face::Mono, rect.w - 60.0 * s - tw);
                    g.text(rect.x + 50.0 * s + tw, rect.y + 7.0 * s, 12.0 * s, DIM, &d, Face::Mono);
                    g.hit(rect, Event::Row(ROW_QUICK + i), f);
                }
                if q.results.is_empty() {
                    g.text(p.x + 28.0 * s, p.y + 118.0 * s, 14.0 * s, MUTED, "No file matches.", Face::Sans);
                }
                g.text(p.x + 26.0 * s, p.y + p.h - 28.0 * s, 12.0 * s, DIM, &format!("{} of {} files · ↑↓ choose · Enter opens · Esc closes", q.results.len(), q.all.len()), Face::Sans);
            }
            Modal::Switcher { sel } => {
                let order = self.reg.ordered();
                let p = fx::modal(g, area, s, 700.0 * s, 520.0 * s, accent, false);
                fx::modal_title(g, p, s, "Workspaces", accent);
                let rh = 52.0 * s;
                let rows = (((p.h - 150.0 * s) / rh) as usize).max(1);
                let start = sel.saturating_sub(rows - 1);
                if order.is_empty() {
                    g.text(p.x + 28.0 * s, p.y + 76.0 * s, 15.0 * s, MUTED, "No workspaces yet. Press n to create one or o to open a folder.", Face::Sans);
                }
                for (i, wm) in order.iter().enumerate().skip(start).take(rows) {
                    let y = p.y + 70.0 * s + (i - start) as f32 * rh;
                    let rect = R::new(p.x + 16.0 * s, y, p.w - 32.0 * s, rh - 5.0 * s);
                    let hot = g.hovered(rect);
                    fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 10.0 * s, i == *sel, hot, wm.color());
                    g.circle(rect.x + 24.0 * s, rect.y + rect.h / 2.0, 11.0 * s, with_alpha(wm.color(), 60));
                    g.ring(rect.x + 24.0 * s, rect.y + rect.h / 2.0, 11.0 * s, 1.5 * s, wm.color());
                    let ini: String = wm.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    g.text(rect.x + 19.0 * s, rect.y + rect.h / 2.0 - 8.0 * s, 15.0 * s, TEXT, &ini, Face::Bold);
                    let open = self.ws.iter().find(|w| w.meta.id == wm.id);
                    let name = format!("{}{}", if wm.pinned { "★ " } else { "" }, wm.name);
                    g.text(rect.x + 48.0 * s, rect.y + 7.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
                    let st = open.map(|w| activity_text(w.activity())).unwrap_or("not open");
                    let sw = g.text_width(12.0 * s, st, Face::Sans);
                    g.text(rect.x + rect.w - sw - 14.0 * s, rect.y + 9.0 * s, 12.0 * s, open.and_then(|w| activity_color(w.activity())).unwrap_or(DIM), st, Face::Sans);
                    let path = g.fit(12.0 * s, &wm.root, Face::Mono, rect.w - 70.0 * s);
                    g.text(rect.x + 48.0 * s, rect.y + 28.0 * s, 12.0 * s, DIM, &path, Face::Mono);
                    g.hit(rect, Event::Row(ROW_SWITCH + i), &wm.name);
                }
                let hy = p.y + p.h - 64.0 * s;
                let mut bx = p.x + 26.0 * s;
                for (label, c, k) in [("New  n", accent, 'n'), ("Open folder  o", CYAN, 'o'), ("Clone  c", MUTED, 'c'), ("Rename  r", MUTED, 'r'), ("Pin  p", MUTED, 'p'), ("Remove  x", ERR, 'x')] {
                    let w = g.text_width(12.0 * s, label, Face::Bold) + 20.0 * s;
                    if bx + w > p.x + p.w - 20.0 * s {
                        break;
                    }
                    pill(g, bx, hy, s, label, c, false, Event::Sub(SUB, Key::Char(k)));
                    bx += w + 6.0 * s;
                }
                g.text(p.x + 26.0 * s, p.y + p.h - 28.0 * s, 12.0 * s, DIM, "Enter or click opens · each workspace keeps its own tabs, chats, board and cluster view", Face::Sans);
            }
        }
    }
}

#[allow(dead_code)]
fn _unused(_: Gfx) {}
