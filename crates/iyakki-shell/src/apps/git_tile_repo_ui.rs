//! GIT: the drawing of the repository view: header (back, name, tabs, branch or tag, actions) and one body per tab. Wide and medium
//! screens show a list and a viewer side by side; a phone-sized one shows one of them at a time (Back returns to the list).

use iyakki_agent::gitbrowse::{EntryKind, LineKind};

use super::git_tile::*;
use super::git_tile_repo::*;
use super::git_tile_ui::*;
use super::widgets;
use crate::fx::{self, with_alpha, AMBER, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

const KEY: [u8; 4] = rgb(130, 190, 255);

impl GitApp {
    // ------------------------------------------------------------------------------------------------ header

    pub(super) fn draw_repo_header(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let size = 13.0 * s;
        let row_h = size * 2.1;
        let mut c = Cursor::new(x, y, w, row_h, 8.0 * s);
        let Some(v) = &self.view else { return 0.0 };
        let (name, private, cloned, tab, rf_label, has_conn, has_clone, web) = (v.item.full_name.clone(), v.item.private, v.clone.is_some(), v.tab, v.ref_label(), v.conn.is_some(), v.clone.is_some(), !v.item.web_url.is_empty());
        let local_only = !has_conn;
        let (bx, by) = c.place(btn_w(g, size, "Back"));
        g.button(bx, by, size, "Back", MUTED, Event::Tab(T_BACK));
        let tw = g.text_width(15.0 * s, &name, Face::Bold).min(w * 0.55);
        let (tx, ty) = c.place(tw);
        let t = g.fit(15.0 * s, &name, Face::Bold, tw);
        g.text(tx, ty + (row_h - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, TEXT, &t, Face::Bold);
        if !local_only {
            let word = if private { "private" } else { "public" };
            let cw = g.text_width(fx::size::CAPTION * s, word, Face::Bold) + fx::size::CAPTION * s * 1.4;
            let (px, py) = c.place(cw);
            fx::chip(g, px, py + (row_h - fx::size::CAPTION * s * 1.9) / 2.0, s, word, if private { AMBER } else { OK }, false);
        }
        if cloned {
            let word = "cloned here";
            let cw = g.text_width(fx::size::CAPTION * s, word, Face::Bold) + fx::size::CAPTION * s * 1.4;
            let (px, py) = c.place(cw);
            fx::chip(g, px, py + (row_h - fx::size::CAPTION * s * 1.9) / 2.0, s, word, OK, true);
        }
        let (rx, ry) = c.place(btn_w(g, size, "Refresh"));
        g.button(rx, ry, size, "Refresh", MUTED, Event::Tab(T_REFRESH));
        let lab = if self.show_log { "Hide log" } else { "Show log" };
        let (lx, ly) = c.place(btn_w(g, size, lab));
        g.button(lx, ly, size, lab, MUTED, Event::Tab(T_LOG));
        // the tabs, then the branch or tag
        let mut c2 = Cursor::new(x, c.bottom() + 6.0 * s, w, row_h, 6.0 * s);
        for (i, t) in Tab::ALL.iter().enumerate() {
            let label = if self.mode == Mode::Phone && *t == Tab::Issues { "Issues" } else { t.label() };
            let (px, py) = c2.place(btn_w(g, size, label));
            if *t == tab {
                g.button(px, py, size, label, ACCENT, Event::Tab(T_TAB + i));
                let bw = btn_w(g, size, label);
                g.rect(px + 6.0 * s, py + row_h + 1.0 * s, bw - 12.0 * s, 2.0 * s, ACCENT);
            } else {
                g.button(px, py, size, label, MUTED, Event::Tab(T_TAB + i));
            }
        }
        let lab = format!("\u{2387} {rf_label}");
        let bw = widgets::select_width(g, s, &lab).min(w * 0.6);
        let (sx, sy) = c2.place(bw);
        widgets::select_box(g, sx, sy - (super::ide_chrome::tool(s) - row_h) / 2.0, bw, s, &lab, self.overlay == Some(Overlay::Refs), ACCENT, Event::Tab(T_REF), "branch or tag");
        // the actions
        let mut c3 = Cursor::new(x, c2.bottom() + 8.0 * s, w, row_h, 6.0 * s);
        if has_conn && !has_clone {
            btn(g, &mut c3, size, "Clone", ACCENT, Event::Tab(T_CLONE), true);
        }
        btn(g, &mut c3, size, "Open in AI IDE", if has_clone { ACCENT } else { MUTED }, Event::Tab(T_IDE), true);
        if has_clone {
            btn(g, &mut c3, size, "Fetch", MUTED, Event::Tab(T_FETCH), true);
            btn(g, &mut c3, size, "Pull", MUTED, Event::Tab(T_PULL), true);
            btn(g, &mut c3, size, "Push", MUTED, Event::Tab(T_PUSH), true);
        }
        btn(g, &mut c3, size, "Copy clone URL", MUTED, Event::Tab(T_COPY_URL), has_conn || has_clone);
        btn(g, &mut c3, size, "Open in browser", MUTED, Event::Tab(T_BROWSER), web || has_clone);
        c3.bottom() - y
    }

    // ------------------------------------------------------------------------------------------------ the bodies

    /// The list and the viewer: (left, right). A phone shows one of them.
    fn split(&self, body: R, s: f32, ratio: f32) -> (R, R) {
        let gap = 8.0 * s;
        let Some(v) = &self.view else { return (body, R::default()) };
        match self.mode {
            Mode::Phone => {
                if v.page_detail { (R::default(), body) } else { (body, R::default()) }
            }
            Mode::Medium => {
                let l = (body.w * (ratio + 0.06)).max(260.0 * s).min(body.w - 200.0 * s);
                (R::new(body.x, body.y, l, body.h), R::new(body.x + l + gap, body.y, body.w - l - gap, body.h))
            }
            Mode::Wide => {
                let l = (body.w * ratio).max(280.0 * s).min(body.w - 300.0 * s);
                (R::new(body.x, body.y, l, body.h), R::new(body.x + l + gap, body.y, body.w - l - gap, body.h))
            }
        }
    }

    pub(super) fn draw_repo_body(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let Some(v) = &mut self.view else { return };
        v.rects.left = R::default();
        v.rects.right = R::default();
        let tab = v.tab;
        match tab {
            Tab::Files => self.draw_files(g, body, s),
            Tab::Commits => self.draw_commits(g, body, s),
            Tab::Branches => self.draw_branches(g, body, s),
            Tab::Issues => self.draw_issues(g, body, s),
            Tab::Info => self.draw_info(g, body, s),
        }
        let _ = tick;
    }

    fn pane(&self, g: &mut Gfx, r: R, s: f32, focused: bool) {
        fx::panel(g, r, s, if focused { ACCENT } else { rgb(60, 72, 112) });
    }

    // ------------------------------------------------------------------------------------------------ files

    fn draw_files(&mut self, g: &mut Gfx, body: R, s: f32) {
        let (l, r) = self.split(body, s, 0.36);
        let focus_left = self.focus != Focus::Right;
        if l.w > 0.0 {
            self.pane(g, l, s, focus_left);
            let Some(v) = &mut self.view else { return };
            v.rects.left = l;
            let (x, w) = (l.x + 8.0 * s, l.w - 16.0 * s);
            let head = format!("FILES \u{b7} {}", v.ref_label());
            let t = g.fit(13.0 * s, &head, Face::Bold, w);
            g.text(x, l.y + 6.0 * s, 13.0 * s, MUTED, &t, Face::Bold);
            let list = R::new(x, l.y + 26.0 * s, w, (l.h - 32.0 * s).max(20.0));
            let row_h = 25.0 * s;
            let n = v.files.rows.len();
            let fit = ((list.h / row_h) as usize).max(1);
            let top = fit_list(&mut v.files.list, n, fit);
            for k in top..(top + fit).min(n) {
                let row = v.files.rows[k].clone();
                let ry = list.y + (k - top) as f32 * row_h;
                let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
                let sel = k == v.files.list.sel && focus_left;
                let hot = g.hovered(rr) && !row.note;
                if (sel || hot) && !row.note {
                    fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, sel, hot, ACCENT);
                }
                let indent = row.depth as f32 * 14.0 * s;
                let ty = ry + (row_h - 2.0 * s - g.line_height(13.0 * s, Face::Sans)) / 2.0;
                if row.note {
                    let t = g.fit(12.5 * s, &row.name, Face::Sans, rr.w - indent - 16.0 * s);
                    g.text(rr.x + 8.0 * s + indent + 14.0 * s, ty, 12.5 * s, DIM, &t, Face::Sans);
                    continue;
                }
                let mark = match row.kind {
                    Some(EntryKind::Dir) => if row.open { "\u{25be}" } else { "\u{25b8}" },
                    Some(EntryKind::Link) => "\u{2192}",
                    Some(EntryKind::Submodule) => "\u{25a3}",
                    _ => " ",
                };
                g.text(rr.x + 6.0 * s + indent, ty, 13.0 * s, if row.kind == Some(EntryKind::Dir) { ACCENT } else { DIM }, mark, Face::Sans);
                let size_t = if row.kind == Some(EntryKind::File) && row.size > 0 { bytes_text(row.size) } else { String::new() };
                let sw = g.text_width(11.5 * s, &size_t, Face::Sans);
                let nm = g.fit(13.0 * s, &row.name, Face::Sans, (rr.w - indent - 26.0 * s - sw - 10.0 * s).max(20.0));
                g.text(rr.x + 20.0 * s + indent, ty, 13.0 * s, if row.kind == Some(EntryKind::Dir) { TEXT } else { MUTED }, &nm, Face::Sans);
                if !size_t.is_empty() {
                    g.text(rr.x + rr.w - sw - 8.0 * s, ty + 1.0 * s, 11.5 * s, DIM, &size_t, Face::Sans);
                }
                g.hit(rr, Event::Tab(T_FROW + k), &row.name);
            }
            sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
        }
        if r.w > 0.0 {
            self.pane(g, r, s, self.focus == Focus::Right);
            self.draw_viewer(g, r, s);
        }
    }

    fn draw_viewer(&mut self, g: &mut Gfx, r: R, s: f32) {
        let mode = self.mode;
        let Some(v) = &mut self.view else { return };
        v.rects.right = R::default();
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let mut y = r.y + 6.0 * s;
        let size = 12.5 * s;
        if mode == Mode::Phone {
            g.button(x, y, size, "Back", MUTED, Event::Tab(T_BACK));
            y += size * 2.1 + 6.0 * s;
        }
        let Some(f) = &mut v.files.file else {
            let msg = if v.files.dirs.get("").is_some_and(|d| d.ready().is_some()) { "Select a file to read it here. This folder has no README." } else { "Select a file to read it here." };
            para(g, x, y + 4.0 * s, w, 14.0 * s, DIM, msg, 4, true);
            return;
        };
        // the title line
        let title = format!("{}{}", f.name, if f.size > 0 { format!("  \u{b7}  {}", bytes_text(f.size)) } else { String::new() });
        let mut bx = r.x + r.w - 8.0 * s;
        if matches!(f.shown, Shown::Text | Shown::Md) {
            let bw = btn_w(g, size, "Copy");
            bx -= bw;
            g.button(bx, y - 2.0 * s, size, "Copy", MUTED, Event::Tab(T_COPY_TEXT));
            bx -= 6.0 * s;
        }
        if matches!(f.shown, Shown::Md) {
            let label = if f.raw { "Rendered" } else { "Source" };
            let bw = btn_w(g, size, label);
            bx -= bw;
            g.button(bx, y - 2.0 * s, size, label, MUTED, Event::Tab(T_VIEW_RAW));
        }
        let t = g.fit(14.0 * s, &title, Face::Bold, (bx - x - 8.0 * s).max(40.0));
        g.text(x, y + 2.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
        y += size * 2.1 + 4.0 * s;
        let view = R::new(x, y, w, (r.y + r.h - y - 6.0 * s).max(20.0));
        v.rects.right = view;
        match &mut f.shown {
            Shown::Loading => {
                g.text(view.x + 2.0 * s, view.y + 4.0 * s, 13.5 * s, DIM, "Reading the file\u{2026}", Face::Sans);
            }
            Shown::Failed(e) => {
                para(g, view.x, view.y + 4.0 * s, view.w, 13.5 * s, ERR, e, 6, true);
            }
            Shown::TooBig => {
                para(g, view.x, view.y + 4.0 * s, view.w, 13.5 * s, AMBER, &format!("{} is {}, more than the {} the viewer opens. Clone the repository to read it.", f.name, bytes_text(f.size), bytes_text(iyakki_agent::gitbrowse::FILE_MAX)), 6, true);
            }
            Shown::Binary(label) => {
                let lh = g.line_height(13.5 * s, Face::Sans) * 1.5;
                g.text(view.x, view.y + 4.0 * s, 13.5 * s, TEXT, &format!("Name: {}", f.name), Face::Sans);
                g.text(view.x, view.y + 4.0 * s + lh, 13.5 * s, TEXT, &format!("Size: {}", bytes_text(f.size.max(f.bytes as u64))), Face::Sans);
                g.text(view.x, view.y + 4.0 * s + 2.0 * lh, 13.5 * s, TEXT, &format!("Type: {label}"), Face::Sans);
                para(g, view.x, view.y + 4.0 * s + 3.5 * lh, view.w, 12.5 * s, DIM, "A binary file is not shown.", 2, true);
            }
            Shown::Image(pm) => {
                let cap = format!("{}\u{d7}{} pixels", pm.width(), pm.height());
                let area = R::new(view.x, view.y, view.w, (view.h - 20.0 * s).max(20.0));
                super::lab_pic::draw_fit(g, pm, area);
                g.text(view.x, view.y + view.h - 16.0 * s, 12.0 * s, DIM, &cap, Face::Sans);
            }
            Shown::Text => draw_text(g, view, s, &mut f.text, true),
            Shown::Md => {
                if f.raw {
                    draw_text(g, view, s, &mut f.text, true);
                } else if let Some(m) = &mut f.md {
                    m.rect = view;
                    let key = view.w as i32;
                    if m.layout.as_ref().map(|l| l.0) != Some(key) {
                        m.layout = Some((key, crate::mdtext::layout(g, &m.src, 13.5 * s, view.w - 10.0 * s)));
                    }
                    let (_, lay) = m.layout.as_ref().unwrap();
                    let max_top = (lay.height - view.h).max(0.0);
                    m.top = m.top.clamp(0.0, max_top);
                    let top = m.top;
                    let hh = lay.height;
                    g.clipped(view, |g| {
                        let (_, lay) = m.layout.as_ref().unwrap();
                        crate::mdtext::draw(g, lay, view.x, view.y - top, TEXT, (view.y, view.y + view.h), view.w - 10.0 * s);
                    });
                    let lines_total = (hh / 18.0) as usize;
                    sbar(g, R::new(view.x + view.w - 5.0 * s, view.y, 4.0 * s, view.h), s, (top / 18.0) as usize, (view.h / 18.0) as usize, lines_total);
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ commits

    fn draw_commits(&mut self, g: &mut Gfx, body: R, s: f32) {
        let (l, r) = self.split(body, s, 0.42);
        let focus_left = self.focus != Focus::Right;
        let now = self.wall();
        let mode = self.mode;
        let problem = self.view.as_ref().and_then(|v| v.commits.slot.problem());
        if l.w > 0.0 {
            self.pane(g, l, s, focus_left);
            let Some(v) = &mut self.view else { return };
            v.rects.left = l;
            let (x, w) = (l.x + 8.0 * s, l.w - 16.0 * s);
            let head = format!("COMMITS \u{b7} {}", v.ref_label());
            let t = g.fit(13.0 * s, &head, Face::Bold, w);
            g.text(x, l.y + 6.0 * s, 13.0 * s, MUTED, &t, Face::Bold);
            let list = R::new(x, l.y + 26.0 * s, w, (l.h - 32.0 * s).max(20.0));
            let items = v.commits.slot.ready().cloned().unwrap_or_default();
            if items.is_empty() {
                let msg = match (&v.commits.slot.st, problem) {
                    (_, Some(p)) => p,
                    (Load::Ready(_), None) => "This branch has no commits.".to_string(),
                    _ => "Reading the commits\u{2026}".to_string(),
                };
                para(g, list.x + 2.0 * s, list.y + 4.0 * s, list.w, 13.5 * s, DIM, &msg, 5, true);
            }
            let row_h = 42.0 * s;
            let n = items.len();
            let fit = ((list.h / row_h) as usize).max(1);
            let top = fit_list(&mut v.commits.list, n, fit);
            for k in top..(top + fit).min(n) {
                let c = &items[k];
                let ry = list.y + (k - top) as f32 * row_h;
                let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
                let chosen = v.commits.open.as_deref() == Some(c.sha.as_str());
                let sel = k == v.commits.list.sel && focus_left;
                let hot = g.hovered(rr);
                if chosen || sel || hot {
                    fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen || sel, hot, ACCENT);
                }
                let subj = g.fit(13.5 * s, &c.subject(), Face::Sans, rr.w - 16.0 * s);
                g.text(rr.x + 8.0 * s, ry + 3.0 * s, 13.5 * s, TEXT, &subj, Face::Sans);
                let ago = if c.when == 0 { String::new() } else { format!(" \u{b7} {}", iyakki_agent::k8s::age_text(now.saturating_sub(c.when))) };
                let meta = g.fit(12.0 * s, &format!("{}  {}{}", c.short(), c.author, ago), Face::Mono, rr.w - 16.0 * s);
                g.text(rr.x + 8.0 * s, ry + 3.0 * s + g.line_height(13.5 * s, Face::Sans) + 1.0 * s, 12.0 * s, DIM, &meta, Face::Mono);
                g.hit(rr, Event::Tab(T_CROW + k), &c.subject());
            }
            sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
        }
        if r.w > 0.0 {
            self.pane(g, r, s, self.focus == Focus::Right);
            self.draw_commit_detail(g, r, s, mode);
        }
    }

    fn draw_commit_detail(&mut self, g: &mut Gfx, r: R, s: f32, mode: Mode) {
        let Some(v) = &mut self.view else { return };
        v.rects.right = R::default();
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let mut y = r.y + 6.0 * s;
        if mode == Mode::Phone {
            g.button(x, y, 12.5 * s, "Back", MUTED, Event::Tab(T_BACK));
            y += 12.5 * s * 2.1 + 6.0 * s;
        }
        let Some(sha) = v.commits.open.clone() else {
            para(g, x, y + 4.0 * s, w, 14.0 * s, DIM, "Choose a commit (Enter or a click) to read its message and its diff.", 4, true);
            return;
        };
        let info = v.commits.slot.ready().and_then(|c| c.iter().find(|c| c.sha == sha)).cloned();
        if let Some(c) = &info {
            let hdr = format!("{}  {}", c.short(), c.author);
            g.text(x, y, 13.0 * s, KEY, &hdr, Face::Mono);
            y += g.line_height(13.0 * s, Face::Mono) + 4.0 * s;
        }
        // the message: the first lines, wrapped
        let lh = g.line_height(13.5 * s, Face::Sans) * 1.15;
        let mut shown = 0;
        'm: for (i, l) in v.commits.msg.iter().enumerate() {
            for p in g.wrap(13.5 * s, l, Face::Sans, w) {
                if shown >= 6 {
                    g.text(x, y, 13.0 * s, DIM, "\u{2026}", Face::Sans);
                    y += lh;
                    break 'm;
                }
                g.text(x, y, 13.5 * s, if i == 0 { TEXT } else { MUTED }, &p, if i == 0 { Face::Bold } else { Face::Sans });
                y += lh;
                shown += 1;
            }
        }
        y += 6.0 * s;
        // the diff: the files on the left (or above, when narrow), the lines of the chosen file
        let area = R::new(x, y, w, (r.y + r.h - y - 6.0 * s).max(40.0));
        let side = area.w >= 520.0 * s;
        let (fl, dv) = match v.commits.diff.ready() {
            _ if matches!(v.commits.diff.st, Load::Loading | Load::None) => {
                g.text(area.x, area.y + 2.0 * s, 13.0 * s, DIM, "Reading the diff\u{2026}", Face::Sans);
                return;
            }
            Some(d) if d.too_large || d.files.is_empty() => (R::default(), area),
            Some(_) => {
                if side {
                    let fw = (area.w * 0.34).max(150.0 * s);
                    (R::new(area.x, area.y, fw, area.h), R::new(area.x + fw + 8.0 * s, area.y, area.w - fw - 8.0 * s, area.h))
                } else {
                    let fh = (area.h * 0.3).max(70.0 * s).min(area.h - 60.0 * s);
                    (R::new(area.x, area.y, area.w, fh), R::new(area.x, area.y + fh + 6.0 * s, area.w, area.h - fh - 6.0 * s))
                }
            }
            None => {
                let p = v.commits.diff.problem().unwrap_or_default();
                para(g, area.x, area.y, area.w, 13.5 * s, ERR, &p, 4, true);
                return;
            }
        };
        if fl.w > 0.0 {
            if let Some(d) = v.commits.diff.ready().cloned() {
                let (a, rm): (u32, u32) = d.files.iter().fold((0, 0), |t, f| (t.0 + f.added, t.1 + f.removed));
                g.text(fl.x, fl.y, 12.0 * s, MUTED, &format!("{} file(s) \u{b7} +{a} \u{2212}{rm}", d.files.len()), Face::Sans);
                let list = R::new(fl.x, fl.y + 18.0 * s, fl.w, fl.h - 18.0 * s);
                let row_h = 22.0 * s;
                let fit = ((list.h / row_h) as usize).max(1);
                let top = fit_list(&mut v.commits.files, d.files.len(), fit);
                for k in top..(top + fit).min(d.files.len()) {
                    let f = &d.files[k];
                    let ry = list.y + (k - top) as f32 * row_h;
                    let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 1.0 * s);
                    let sel = k == v.commits.files.sel;
                    let hot = g.hovered(rr);
                    if sel || hot {
                        fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 5.0 * s, sel, hot, ACCENT);
                    }
                    let col = match f.status {
                        'A' => OK,
                        'D' => ERR,
                        'R' => CYAN,
                        _ => AMBER,
                    };
                    g.text(rr.x + 5.0 * s, ry + 3.0 * s, 12.0 * s, col, &f.status.to_string(), Face::Mono);
                    let stat = if f.binary { "bin".to_string() } else { format!("+{} \u{2212}{}", f.added, f.removed) };
                    let sw = g.text_width(11.0 * s, &stat, Face::Sans);
                    let nm = g.fit(12.0 * s, f.name(), Face::Sans, rr.w - 22.0 * s - sw - 8.0 * s);
                    g.text(rr.x + 18.0 * s, ry + 3.0 * s, 12.0 * s, TEXT, &nm, Face::Sans);
                    g.text(rr.x + rr.w - sw - 5.0 * s, ry + 4.0 * s, 11.0 * s, DIM, &stat, Face::Sans);
                    g.hit(rr, Event::Tab(T_DROW + k), f.name());
                }
                sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, d.files.len());
            }
        }
        v.rects.right = dv;
        draw_text(g, dv, s, &mut v.commits.text, false);
    }

    // ------------------------------------------------------------------------------------------------ branches

    fn draw_branches(&mut self, g: &mut Gfx, body: R, s: f32) {
        self.pane(g, body, s, true);
        let now = self.wall();
        let Some(v) = &mut self.view else { return };
        v.rects.left = body;
        let (x, w) = (body.x + 8.0 * s, body.w - 16.0 * s);
        g.text(x, body.y + 6.0 * s, 13.0 * s, MUTED, "BRANCHES AND TAGS", Face::Bold);
        let list = R::new(x, body.y + 26.0 * s, w, (body.h - 32.0 * s).max(20.0));
        let mut rows: Vec<(String, bool, bool, String, String, u64)> = Vec::new();
        if let Some(b) = v.refs.branches.ready() {
            rows.extend(b.iter().map(|r| (r.name.clone(), false, r.default || r.name == v.item.default_branch, r.sha.chars().take(7).collect(), r.subject.clone(), r.when)));
        }
        if let Some(t) = v.refs.tags.ready() {
            rows.extend(t.iter().map(|r| (r.name.clone(), true, false, r.sha.chars().take(7).collect(), r.subject.clone(), r.when)));
        }
        let problem = v.refs.branches.problem().or_else(|| v.refs.tags.problem());
        if rows.is_empty() {
            let msg = problem.unwrap_or_else(|| if matches!(v.refs.branches.st, Load::Ready(_)) { "No branches or tags.".to_string() } else { "Reading the branches and tags\u{2026}".to_string() });
            para(g, list.x + 2.0 * s, list.y + 4.0 * s, list.w, 13.5 * s, DIM, &msg, 5, true);
        }
        let row_h = 28.0 * s;
        let n = rows.len();
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut v.refs.list, n, fit);
        let wide = list.w >= 520.0 * s;
        for k in top..(top + fit).min(n) {
            let (name, tag, def, sha, subj, when) = &rows[k];
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let sel = k == v.refs.list.sel;
            let hot = g.hovered(rr);
            let cur = *name == v.rf;
            if sel || hot || cur {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, sel || cur, hot, ACCENT);
            }
            let ty = ry + (row_h - 2.0 * s - g.line_height(13.0 * s, Face::Sans)) / 2.0;
            let mut tx = rr.x + 8.0 * s;
            let chip = if *tag { "tag" } else if *def { "default" } else { "" };
            if !chip.is_empty() {
                let cw = fx::chip(g, tx, ry + (row_h - 2.0 * s - fx::size::CAPTION * s * 1.9) / 2.0, s, chip, if *tag { CYAN } else { OK }, *def);
                tx += cw + 8.0 * s;
            }
            let nw = if wide { rr.w * 0.34 } else { rr.w - (tx - rr.x) - 8.0 * s };
            let nm = g.fit(13.0 * s, name, Face::Bold, nw - (tx - rr.x));
            let used = g.text(tx, ty, 13.0 * s, TEXT, &nm, Face::Bold);
            if wide {
                let sx = rr.x + nw + 8.0 * s;
                g.text(sx, ty, 12.0 * s, DIM, sha, Face::Mono);
                let age = if *when == 0 { String::new() } else { iyakki_agent::k8s::age_text(now.saturating_sub(*when)) };
                let aw = g.text_width(12.0 * s, &age, Face::Sans);
                let sj = g.fit(12.5 * s, subj, Face::Sans, (rr.x + rr.w - sx - 70.0 * s - aw - 16.0 * s).max(20.0));
                g.text(sx + 70.0 * s, ty, 12.5 * s, MUTED, &sj, Face::Sans);
                g.text(rr.x + rr.w - aw - 8.0 * s, ty, 12.0 * s, DIM, &age, Face::Sans);
            }
            let _ = used;
            g.hit(rr, Event::Tab(T_BROW + k), name);
        }
        sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
    }

    // ------------------------------------------------------------------------------------------------ issues and pull requests

    fn draw_issues(&mut self, g: &mut Gfx, body: R, s: f32) {
        let (l, r) = self.split(body, s, 0.45);
        let focus_left = self.focus != Focus::Right;
        let now = self.wall();
        let mode = self.mode;
        if l.w > 0.0 {
            self.pane(g, l, s, focus_left);
            let Some(v) = &mut self.view else { return };
            v.rects.left = l;
            let (x, w) = (l.x + 8.0 * s, l.w - 16.0 * s);
            let size = 12.5 * s;
            let mut y = l.y + 6.0 * s;
            let mut cx0 = x;
            for (i, (label, on)) in [("Issues", !v.issues.prs), ("Pull requests", v.issues.prs)].iter().enumerate() {
                let bw = g.button(cx0, y, size, label, if *on { ACCENT } else { MUTED }, Event::Tab(T_SUB + i));
                cx0 += bw + 6.0 * s;
            }
            let ro = "read-only";
            let cw = g.text_width(fx::size::CAPTION * s, ro, Face::Bold) + fx::size::CAPTION * s * 1.4;
            if cx0 + cw < x + w {
                fx::chip(g, x + w - cw, y + (size * 2.1 - fx::size::CAPTION * s * 1.9) / 2.0, s, ro, DIM, false);
            }
            y += size * 2.1 + 8.0 * s;
            let list = R::new(x, y, w, (l.y + l.h - y - 6.0 * s).max(20.0));
            let slot = if v.issues.prs { &v.issues.pulls } else { &v.issues.issues };
            let items = slot.ready().cloned().unwrap_or_default();
            if items.is_empty() {
                let msg = match (&slot.st, slot.problem()) {
                    (_, Some(p)) => p,
                    (Load::Ready(_), None) => format!("No {} on this repository.", if v.issues.prs { "pull requests" } else { "issues" }),
                    _ => "Reading\u{2026}".to_string(),
                };
                para(g, list.x + 2.0 * s, list.y + 4.0 * s, list.w, 13.5 * s, DIM, &msg, 5, true);
            }
            let row_h = 42.0 * s;
            let n = items.len();
            let fit = ((list.h / row_h) as usize).max(1);
            let top = fit_list(&mut v.issues.list, n, fit);
            for k in top..(top + fit).min(n) {
                let it = &items[k];
                let ry = list.y + (k - top) as f32 * row_h;
                let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
                let chosen = v.issues.open == Some(it.number);
                let sel = k == v.issues.list.sel && focus_left;
                let hot = g.hovered(rr);
                if chosen || sel || hot {
                    fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen || sel, hot, ACCENT);
                }
                let col = match it.state.as_str() {
                    "open" => OK,
                    "merged" => CYAN,
                    _ => DIM,
                };
                g.circle(rr.x + 10.0 * s, ry + 12.0 * s, 4.0 * s, col);
                let t = g.fit(13.5 * s, &format!("#{} {}", it.number, it.title), Face::Sans, rr.w - 30.0 * s);
                g.text(rr.x + 20.0 * s, ry + 3.0 * s, 13.5 * s, TEXT, &t, Face::Sans);
                let ago = if it.when == 0 { String::new() } else { format!(" \u{b7} {}", iyakki_agent::k8s::age_text(now.saturating_sub(it.when))) };
                let labels = if it.labels.is_empty() { String::new() } else { format!(" \u{b7} {}", it.labels.join(", ")) };
                let meta = g.fit(12.0 * s, &format!("{} \u{b7} {}{ago}{labels}", it.state, it.author), Face::Sans, rr.w - 30.0 * s);
                g.text(rr.x + 20.0 * s, ry + 3.0 * s + g.line_height(13.5 * s, Face::Sans) + 1.0 * s, 12.0 * s, DIM, &meta, Face::Sans);
                g.hit(rr, Event::Tab(T_IROW + k), &it.title);
            }
            sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n);
        }
        if r.w > 0.0 {
            self.pane(g, r, s, self.focus == Focus::Right);
            let Some(v) = &mut self.view else { return };
            let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
            let mut y = r.y + 6.0 * s;
            if mode == Mode::Phone {
                g.button(x, y, 12.5 * s, "Back", MUTED, Event::Tab(T_BACK));
                y += 12.5 * s * 2.1 + 6.0 * s;
            }
            if v.issues.open.is_none() {
                para(g, x, y + 4.0 * s, w, 14.0 * s, DIM, "Choose one to read it here. This is a read-only view: nothing can be written or commented on.", 4, true);
            } else {
                let view = R::new(x, y, w, (r.y + r.h - y - 6.0 * s).max(20.0));
                v.rects.right = view;
                draw_text(g, view, s, &mut v.issues.body, false);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ info

    fn draw_info(&mut self, g: &mut Gfx, body: R, s: f32) {
        self.pane(g, body, s, true);
        let now = self.wall();
        let Some(v) = &self.view else { return };
        let it = v.item.clone();
        let conn = v.conn.clone();
        let clone = v.clone.clone();
        let (x, w) = (body.x + 14.0 * s, body.w - 28.0 * s);
        let mut y = body.y + 10.0 * s;
        let size = 13.5 * s;
        let lh = g.line_height(size, Face::Sans) * 1.45;
        let key_w = 120.0 * s;
        let mut rows: Vec<(&str, String)> = vec![("Name", it.full_name.clone())];
        if !it.description.is_empty() {
            rows.push(("Description", it.description.clone()));
        }
        if !it.default_branch.is_empty() {
            rows.push(("Default branch", it.default_branch.clone()));
        }
        if conn.is_some() {
            rows.push(("Visibility", if it.private { "private".into() } else { "public".into() }));
        }
        if it.updated > 0 {
            rows.push(("Last push", format!("{} ago", iyakki_agent::k8s::age_text(now.saturating_sub(it.updated)))));
        }
        if let Some(n) = it.stars {
            rows.push(("Stars", n.to_string()));
        }
        if let Some(n) = it.size_kb {
            rows.push(("Size", size_text(n)));
        }
        if !it.license.is_empty() {
            rows.push(("License", it.license.clone()));
        }
        if it.archived {
            rows.push(("Archived", "yes: it can only be read".into()));
        }
        if it.fork {
            rows.push(("Fork", "yes".into()));
        }
        if let Some(c) = &conn {
            rows.push(("From", format!("{} ({}, {}, {})", c.name, c.kind.label(), c.url, place_text(c))));
        }
        if let Some(c) = &clone {
            rows.push(("On this computer", format!("{} (branch {})", c.dir.display(), if c.branch.is_empty() { "none" } else { &c.branch })));
        }
        for (k, val) in &rows {
            g.text(x, y, size, DIM, k, Face::Bold);
            let hh = para(g, x + key_w, y, w - key_w, size, TEXT, val, 4, true);
            y += hh.max(lh) + 2.0 * s;
        }
        y += 6.0 * s;
        // the clone addresses, each with its Copy
        if let Some(c) = &conn {
            let (https, ssh) = c.clone_urls(&it.full_name);
            for (label, url, ev) in [("Clone (HTTPS)", https, T_COPY_HTTPS), ("Clone (SSH)", ssh, T_COPY_SSH)] {
                g.text(x, y + 4.0 * s, size, DIM, label, Face::Bold);
                let bw = btn_w(g, 12.5 * s, "Copy");
                let t = g.fit(size, &url, Face::Mono, (w - key_w - bw - 12.0 * s).max(40.0));
                g.text(x + key_w, y + 4.0 * s, size, TEXT, &t, Face::Mono);
                g.button(x + w - bw, y, 12.5 * s, "Copy", MUTED, Event::Tab(ev));
                y += 12.5 * s * 2.1 + 6.0 * s;
            }
        } else if let Some(r) = clone.as_ref().and_then(|c| c.remote.clone()) {
            g.text(x, y + 4.0 * s, size, DIM, "origin", Face::Bold);
            let bw = btn_w(g, 12.5 * s, "Copy");
            let t = g.fit(size, &r, Face::Mono, (w - key_w - bw - 12.0 * s).max(40.0));
            g.text(x + key_w, y + 4.0 * s, size, TEXT, &t, Face::Mono);
            g.button(x + w - bw, y, 12.5 * s, "Copy", MUTED, Event::Tab(T_COPY_URL));
        }
        let _ = with_alpha(OK, 1);
    }

    // ------------------------------------------------------------------------------------------------ the branch chooser

    pub(super) fn draw_refs_overlay(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(v) = &self.view else { return };
        let rows = v.ref_rows();
        let cur = v.rf.clone();
        let h = 90.0 + 36.0 * rows.len().max(2) as f32;
        let p = fx::modal(g, area, s, 440.0 * s, h * s, ACCENT, false);
        g.hit(area, Event::Tab(T_CLOSE), "close the list");
        g.hit(p, Event::Tick, "the list");
        let y = fx::modal_title(g, p, s, "Choose a branch or tag", ACCENT);
        if rows.is_empty() {
            g.text(p.x + 26.0 * s, y + 8.0 * s, 13.5 * s, DIM, "Reading the branches and tags\u{2026}", Face::Sans);
            return;
        }
        let row_h = 32.0 * s;
        let fit = (((p.y + p.h - y - 12.0 * s) / row_h) as usize).max(1);
        let top = fit_list(&mut self.src_list, rows.len(), fit);
        for k in top..(top + fit).min(rows.len()) {
            let (name, tag, def) = &rows[k];
            let rr = R::new(p.x + 18.0 * s, y + (k - top) as f32 * row_h, p.w - 36.0 * s, row_h - 3.0 * s);
            let hot = g.hovered(rr);
            let chosen = *name == cur;
            if chosen || hot || k == self.src_list.sel {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen, hot || k == self.src_list.sel, ACCENT);
            }
            let note = if *tag { "tag" } else if *def { "default" } else { "" };
            let nw = g.text_width(12.0 * s, note, Face::Sans);
            let t = g.fit(13.5 * s, name, Face::Bold, rr.w - nw - 36.0 * s);
            g.text(rr.x + 12.0 * s, rr.y + (rr.h - g.line_height(13.5 * s, Face::Bold)) / 2.0, 13.5 * s, TEXT, &t, Face::Bold);
            g.text(rr.x + rr.w - nw - 10.0 * s, rr.y + (rr.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, DIM, note, Face::Sans);
            g.hit(rr, Event::Tab(T_REFROW + k), name);
        }
    }
}

/// The lines of a scrolling text, one row each, cut at the edge, with the first line in view kept inside the text.
pub(super) fn draw_text(g: &mut Gfx, rect: R, s: f32, view: &mut super::k8s_tile::TextView, mono: bool) {
    let (size, face) = if mono { (12.5 * s, Face::Mono) } else { (12.5 * s, Face::Mono) };
    let lh = g.line_height(size, face) * 1.12;
    let fit = ((rect.h / lh).floor() as usize).max(1);
    view.rect = rect;
    view.fit = fit;
    let n = view.lines.len();
    if view.stick {
        view.top = n.saturating_sub(fit);
    }
    view.top = view.top.min(n.saturating_sub(fit.min(n)));
    let top = view.top;
    let inner_w = rect.w - 8.0 * s;
    for (i, line) in view.lines.iter().enumerate().skip(top).take(fit) {
        let y = rect.y + (i - top) as f32 * lh;
        let t = g.fit(size, &line.text, face, inner_w);
        // the line number of a file stays dim
        if mono && line.text.len() > 7 && line.text.as_bytes()[5] == b' ' && line.text.as_bytes()[..5].iter().all(|b| *b == b' ' || b.is_ascii_digit()) && line.color == TEXT {
            let (num, rest) = t.split_at(t.char_indices().nth(7).map_or(t.len(), |(b, _)| b));
            let nw = g.text_width(size, num, face);
            g.text(rect.x, y, size, DIM, num, face);
            g.text(rect.x + nw, y, size, line.color, rest, face);
        } else {
            if line.color == rgb(110, 224, 160) {
                g.rect(rect.x - 2.0 * s, y, inner_w, lh, with_alpha(OK, 22));
            } else if line.color == ERR {
                g.rect(rect.x - 2.0 * s, y, inner_w, lh, with_alpha(ERR, 22));
            }
            g.text(rect.x, y, size, line.color, &t, face);
        }
    }
    sbar(g, R::new(rect.x + rect.w - 5.0 * s, rect.y, 4.0 * s, rect.h), s, top, fit, n);
}

#[allow(dead_code)]
fn unused(_: LineKind) {}
