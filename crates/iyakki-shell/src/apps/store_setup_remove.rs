//! STORE -> Setup: removing a Quick install item. The popup's Remove button opens a review card: what goes, what stays, how much space
//! the data deletion frees (and that it cannot be undone), the data tick (off by default), the tick to remove what needs the item
//! first, and an explicit Remove. Then progress, then the result; a failure shows Retry. Engine: `iyakki_agent::quickinstall::uninstall`.

use super::*;
use iyakki_agent::quickinstall::uninstall as un;

use crate::ui::ACCENT_MAIN;

/// The popup's Remove button (one per item), and the card's own codes.
pub const RM_OPEN: usize = SB + 1100;
pub const RM_YES: usize = SB + 1200;
pub const RM_NO: usize = SB + 1201;
pub const RM_DATA: usize = SB + 1202;
pub const RM_DEPS: usize = SB + 1203;
pub const RM_RETRY: usize = SB + 1204;
pub const RM_CLOSE: usize = SB + 1205;
pub const RM_PANEL: usize = SB + 1206;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Review,
    Running,
    Done(String),
    Failed(String),
}

pub struct RemoveCard {
    pub item: Item,
    pub plan: un::Plan,
    /// Installed items that need this one (listed; removed first only with the tick).
    pub deps: Vec<Item>,
    pub warn: Vec<String>,
    pub delete_data: bool,
    pub with_deps: bool,
    pub phase: Phase,
    pub scroll: usize,
}

impl RemoveCard {
    /// Why the Remove button cannot be pressed yet (None: it can).
    pub fn blocked(&self) -> Option<String> {
        if !self.deps.is_empty() && !self.with_deps {
            return Some(format!("{} need it: tick the box below to remove them first, or remove them yourself.", self.deps.iter().map(|d| d.title()).collect::<Vec<_>>().join(", ")));
        }
        if self.plan.data_requires && !self.delete_data {
            return Some("This item is its own data: tick the box to confirm deleting it.".into());
        }
        None
    }

    /// The card's sections as (kind, text) lines before wrapping: 'h' heading, 'g' goes, 'd' data (danger), 's' stays, 'n' note.
    pub fn lines(&self) -> Vec<(char, String)> {
        let r = self.plan.review(self.delete_data);
        let mut v: Vec<(char, String)> = Vec::new();
        v.push(('h', "What goes".into()));
        v.extend(r.goes.iter().map(|g| ('g', format!("\u{2022} {g}"))));
        if self.delete_data && !r.data.is_empty() {
            v.push(('h', "Also deleted (this cannot be undone)".into()));
            v.extend(r.data.iter().map(|g| ('d', format!("\u{2022} {g}"))));
            v.push(('d', format!("Frees {}.", r.space_if_ticked)));
        }
        v.push(('h', "What stays".into()));
        v.extend(r.stays.iter().map(|g| ('s', format!("\u{2022} {g}"))));
        for d in &self.deps {
            v.push(('n', format!("{} needs this and is installed.", d.title())));
        }
        v.extend(self.warn.iter().map(|w| ('n', w.clone())));
        v.extend(r.notes.iter().map(|w| ('n', w.clone())));
        v
    }
}

impl Setup {
    /// Can a Remove be offered for this item: IYAKKI's own install (or a leftover), for an administrator.
    pub fn can_remove(&self, item: Item) -> bool {
        if !self.admin || self.running_job(item).is_some() {
            return false;
        }
        match self.presence(item) {
            Some(Presence::Installed(_)) | Some(Presence::Earlier(_)) => true,
            Some(Presence::Elsewhere(_)) => false,
            _ => self.failed.contains(&item),
        }
    }

    /// Open the review card of an item.
    pub fn open_remove(&mut self, item: Item, cx: &AppCx) {
        if !self.admin {
            self.say(format!("Only an administrator can remove software (your role is {}). Ask an administrator.", cx.session.role.name()), false);
            return;
        }
        if !self.can_remove(item) {
            self.say(format!("{} is not something IYAKKI can remove right now.", item.title()), false);
            return;
        }
        let ctx = un::real_ctx(&cx.services, &cx.session.user, item);
        let installed = |i: Item| matches!(self.presence(i), Some(Presence::Installed(_)) | Some(Presence::Earlier(_)));
        let (deps, warn) = un::dependency_lines(item, &installed);
        let warn = warn.into_iter().chain(ctx.cli_warning.clone()).collect::<Vec<_>>();
        self.rm = Some(RemoveCard { item, plan: un::plan(item, &ctx), deps, warn, delete_data: false, with_deps: false, phase: Phase::Review, scroll: 0 });
    }

    /// Start the removal on a thread (tests run it at once).
    fn start_remove(&mut self, cx: &AppCx) {
        let Some(c) = self.rm.as_mut() else { return };
        if c.blocked().is_some() {
            return;
        }
        c.phase = Phase::Running;
        let (item, o) = (c.item, un::Opts { delete_data: c.delete_data, with_dependants: c.with_deps });
        cx.audit.record(&cx.session.user, "quickinstall.uninstall", &format!("{} data={}", item.id(), if o.delete_data { "deleted" } else { "kept" }));
        let (svc, user, slot, prog) = (cx.services.clone(), cx.session.user.clone(), self.rm_slot.clone(), self.rm_progress.clone());
        *prog.lock().unwrap_or_else(|e| e.into_inner()) = "Starting\u{2026}".into();
        let run = move || {
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(un::uninstall_real(&svc, &user, item, o, Some(prog)));
        };
        if cfg!(test) {
            run();
            self.take_remove_result(cx);
        } else if std::thread::Builder::new().name("iyakki-quickinstall-remove".into()).spawn(run).is_err() {
            if let Some(c) = self.rm.as_mut() {
                c.phase = Phase::Failed("Could not start the helper that removes it.".into());
            }
        }
    }

    /// The thread finished: show the result, and read what is installed again so the item shows as not installed.
    pub(super) fn take_remove_result(&mut self, cx: &AppCx) -> bool {
        let Some(r) = self.rm_slot.lock().ok().and_then(|mut x| x.take()) else { return false };
        let ok = r.is_ok();
        if let Some(c) = self.rm.as_mut() {
            c.phase = match r {
                Ok(m) => Phase::Done(m),
                Err(e) => Phase::Failed(e),
            };
        }
        if ok {
            self.fixed = false;
            self.refresh_facts(cx);
        }
        true
    }

    /// Everything while the card is open: it is modal.
    pub(super) fn remove_event(&mut self, ev: &Event, cx: &AppCx) -> Option<Flow> {
        let phase = self.rm.as_ref()?.phase.clone();
        let close = |s: &mut Setup| s.rm = None;
        match (&phase, ev) {
            (_, Event::Tab(RM_PANEL)) => {}
            (Phase::Running, _) => {}
            (Phase::Review, Event::Tab(RM_YES)) | (Phase::Review, Event::Key(Key::Char('y'))) => self.start_remove(cx),
            (Phase::Review, Event::Tab(RM_DATA)) | (Phase::Review, Event::Key(Key::Char('d'))) => {
                if let Some(c) = self.rm.as_mut() {
                    c.delete_data = !c.delete_data;
                    c.scroll = 0;
                }
            }
            (Phase::Review, Event::Tab(RM_DEPS)) | (Phase::Review, Event::Key(Key::Char('p'))) => {
                if let Some(c) = self.rm.as_mut().filter(|c| !c.deps.is_empty()) {
                    c.with_deps = !c.with_deps;
                }
            }
            (Phase::Failed(_), Event::Tab(RM_RETRY)) | (Phase::Failed(_), Event::Key(Key::Char('r'))) => {
                if let Some(c) = self.rm.as_mut() {
                    c.phase = Phase::Review;
                }
                self.start_remove(cx);
            }
            (_, Event::Tab(RM_NO)) | (_, Event::Tab(RM_CLOSE)) | (_, Event::Key(Key::Esc)) | (Phase::Review, Event::Key(Key::Char('n'))) => close(self),
            (Phase::Done(_), Event::Key(Key::Enter)) => close(self),
            (_, Event::Key(Key::Up)) | (_, Event::Key(Key::PageUp)) => {
                if let Some(c) = self.rm.as_mut() {
                    c.scroll = c.scroll.saturating_sub(if matches!(ev, Event::Key(Key::Up)) { 1 } else { 8 });
                }
            }
            (_, Event::Key(Key::Down)) | (_, Event::Key(Key::PageDown)) => {
                if let Some(c) = self.rm.as_mut() {
                    c.scroll += if matches!(ev, Event::Key(Key::Down)) { 1 } else { 8 };
                }
            }
            (_, Event::Scroll(d)) => {
                if let Some(c) = self.rm.as_mut() {
                    c.scroll = if *d < 0 { c.scroll.saturating_sub(3) } else { c.scroll + 3 };
                }
            }
            _ => {}
        }
        Some(Flow::Stay)
    }

    pub fn remove_hint(&self) -> Option<String> {
        let c = self.rm.as_ref()?;
        Some(match c.phase {
            Phase::Review => "y remove · d also delete the data · p remove what needs it first · Up/Down scroll · Esc cancel".into(),
            Phase::Running => "Removing\u{2026}".into(),
            Phase::Done(_) => "Esc or Enter close".into(),
            Phase::Failed(_) => "r retry · Esc close".into(),
        })
    }

    /// Draw the card over the page.
    pub(super) fn draw_remove(&mut self, g: &mut Gfx, area: R, s: f32) {
        let progress = self.rm_progress.lock().map(|p| p.clone()).unwrap_or_default();
        let Some(c) = self.rm.as_mut() else { return };
        let danger = c.delete_data;
        let w = (680.0 * s).min(area.w - 24.0 * s).max(200.0 * s);
        let p = fx::modal(g, area, s, w, area.h - 16.0 * s, if danger { ERR } else { ACCENT_MAIN }, danger);
        // a click on the card does nothing (the dim backdrop is not a way out)
        g.hit(p, Event::Tab(RM_PANEL), "card");
        let pad = 22.0 * s;
        let iw = p.w - 2.0 * pad;
        let title = g.fit(18.0 * s, &format!("Remove {}", c.item.title()), Face::Bold, iw - 30.0 * s);
        g.text(p.x + pad, p.y + 18.0 * s, 18.0 * s, TEXT, &title, Face::Bold);
        g.text(p.x + p.w - pad - 12.0 * s, p.y + 14.0 * s, 20.0 * s, MUTED, "\u{d7}", Face::Bold);
        g.hit(R::new(p.x + p.w - pad - 30.0 * s, p.y + 8.0 * s, 34.0 * s, 34.0 * s), Event::Tab(RM_CLOSE), "Close");
        let body_top = p.y + 52.0 * s;
        let lh = 13.0 * s * 1.35;
        let (btn_sz, brow) = (14.0 * s, 14.0 * s * 2.1);
        match c.phase.clone() {
            Phase::Review => {
                // measure the fixed parts from the bottom: buttons, the reason, the two ticks
                let opt = |g: &mut Gfx, text: &str| g.wrap(12.5 * s, text, Face::Sans, iw - 34.0 * s);
                let data_text = format!("Also delete the data ({}). This cannot be undone.", c.plan.space_text());
                let dl = opt(g, &data_text);
                let deps_text = if c.deps.is_empty() { String::new() } else { format!("Also remove first, with their data kept: {}", c.deps.iter().map(|d| d.title()).collect::<Vec<_>>().join(", ")) };
                let pl = if deps_text.is_empty() { vec![] } else { opt(g, &deps_text) };
                let why = c.blocked().map(|b| g.wrap(12.5 * s, &b, Face::Sans, iw)).unwrap_or_default();
                let row_h = |n: usize| (n.max(1) as f32 * 17.0 * s).max(30.0 * s) + 6.0 * s;
                let foot_h = brow + 10.0 * s + why.len() as f32 * 17.0 * s + row_h(dl.len()) + if pl.is_empty() { 0.0 } else { row_h(pl.len()) } + 8.0 * s;
                let body_h = (p.y + p.h - foot_h - 10.0 * s - body_top).max(lh * 3.0);
                // the lines, wrapped, scrolled
                let mut wrapped: Vec<(char, String)> = vec![];
                for (k, t) in c.lines() {
                    let face = if k == 'h' { Face::Bold } else { Face::Sans };
                    for l in g.wrap(12.5 * s, &t, face, iw) {
                        wrapped.push((k, l));
                    }
                }
                let rows = ((body_h / lh).floor() as usize).max(1);
                let max_scroll = wrapped.len().saturating_sub(rows);
                c.scroll = c.scroll.min(max_scroll);
                let mut y = body_top;
                for (k, t) in wrapped.iter().skip(c.scroll).take(rows) {
                    let (col, face) = match k {
                        'h' => (TEXT, Face::Bold),
                        'd' => (ERR, Face::Sans),
                        'n' => (AMBER, Face::Sans),
                        _ => (MUTED, Face::Sans),
                    };
                    g.text(p.x + pad, y, 12.5 * s, col, t, face);
                    y += lh;
                }
                if max_scroll > 0 {
                    g.text(p.x + p.w - pad - 150.0 * s, body_top + body_h - lh, 11.0 * s, DIM, &format!("more: Up, Down  ({}/{})", c.scroll + 1, max_scroll + 1), Face::Sans);
                }
                let mut fy = p.y + p.h - foot_h;
                let tick = |g: &mut Gfx, y: f32, on: bool, lines: &[String], ev: usize, col: [u8; 4]| {
                    let hh = row_h(lines.len());
                    let b = 18.0 * s;
                    g.rrect(p.x + pad, y + 6.0 * s, b, b, 4.0 * s, if on { col } else { [255, 255, 255, 18] });
                    g.rrect_outline(p.x + pad, y + 6.0 * s, b, b, 4.0 * s, 1.4 * s, col);
                    if on {
                        g.text(p.x + pad + 3.0 * s, y + 6.0 * s, 15.0 * s, rgb(10, 12, 22), "\u{2713}", Face::Bold);
                    }
                    for (i, l) in lines.iter().enumerate() {
                        g.text(p.x + pad + 28.0 * s, y + 7.0 * s + i as f32 * 17.0 * s, 12.5 * s, TEXT, l, Face::Sans);
                    }
                    g.hit(R::new(p.x + pad, y, iw, hh), Event::Tab(ev), "tick");
                    y + hh
                };
                fy = tick(g, fy, c.delete_data, &dl, RM_DATA, ERR);
                if !pl.is_empty() {
                    fy = tick(g, fy, c.with_deps, &pl, RM_DEPS, ACCENT_MAIN);
                }
                for l in &why {
                    g.text(p.x + pad, fy, 12.5 * s, AMBER, l, Face::Sans);
                    fy += 17.0 * s;
                }
                let by = p.y + p.h - brow - 12.0 * s;
                let label = if c.delete_data { "Remove and delete the data (y)" } else { "Remove (y)" };
                let w1 = if c.blocked().is_none() { g.button(p.x + pad, by, btn_sz, label, ERR, Event::Tab(RM_YES)) } else { g.button_off(p.x + pad, by, btn_sz, label) };
                g.button(p.x + pad + w1 + 12.0 * s, by, btn_sz, "Cancel (Esc)", MUTED, Event::Tab(RM_NO));
            }
            Phase::Running => {
                let t = g.wrap(13.0 * s, &progress, Face::Sans, iw);
                let mut y = body_top;
                g.text(p.x + pad, y, 14.0 * s, TEXT, "Removing\u{2026}", Face::Bold);
                y += 26.0 * s;
                for l in t.iter().take(6) {
                    g.text(p.x + pad, y, 13.0 * s, MUTED, l, Face::Sans);
                    y += lh;
                }
            }
            Phase::Done(m) | Phase::Failed(m) => {
                let failed = matches!(c.phase, Phase::Failed(_));
                let mut y = body_top;
                g.text(p.x + pad, y, 14.0 * s, if failed { ERR } else { OK }, if failed { "It did not finish" } else { "Done" }, Face::Bold);
                y += 26.0 * s;
                let rows = (((p.y + p.h - brow - 24.0 * s) - y) / lh).floor().max(1.0) as usize;
                for l in g.wrap(13.0 * s, &m, Face::Sans, iw).into_iter().take(rows) {
                    g.text(p.x + pad, y, 13.0 * s, TEXT, &l, Face::Sans);
                    y += lh;
                }
                let by = p.y + p.h - brow - 12.0 * s;
                let mut x = p.x + pad;
                if failed {
                    x += g.button(x, by, btn_sz, "Retry (r)", ERR, Event::Tab(RM_RETRY)) + 12.0 * s;
                }
                g.button(x, by, btn_sz, "Close (Esc)", MUTED, Event::Tab(RM_CLOSE));
            }
        }
    }
}
