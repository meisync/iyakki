//! Settings → Displays: the connected displays, their resolution, which is the main one, where the others sit, mirror or extend,
//! "Identify", and the 15-second "Keep these settings?" that goes back by itself. Also the memory line and the Screen features
//! switch (Automatic, Always on, Off) that decide whether layouts and extra displays are used at all.
//!
//! The page asks; the display loop (`backend_kms.rs`) does it and reports through the hub (`display::outputs`). When the shell
//! does not drive the displays itself (one display, a window, too little memory, another program holds the card) the page lists
//! what is connected and says why nothing can be changed here.

use crate::safeclamp::SafeClamp;
use super::*;
use crate::display::outputs::{hub, DisplayConfig, Request};
use crate::memgate::{self, DisplayMode, Switch};

#[derive(Default)]
pub(super) struct DispPanel {
    /// What the page is editing (starts as the arrangement in force).
    pub(super) draft: DisplayConfig,
    /// What was in force when `draft` was taken, to notice a change made elsewhere.
    base: DisplayConfig,
    pub(super) started: bool,
    /// The touch screen the 't' key changes (index into the list the input thread publishes).
    pub(super) touch_sel: usize,
}

/// The distinct resolutions of an output, the biggest first.
fn sizes(modes: &[(u32, u32, u32)]) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = modes.iter().map(|m| (m.0, m.1)).collect();
    v.sort();
    v.dedup();
    v.sort_by_key(|s| std::cmp::Reverse(s.0 as u64 * s.1 as u64));
    v
}

impl SettingsApp {
    fn disp_sync(&mut self) {
        let st = hub().status();
        if !self.disp.started || st.config != self.disp.base {
            self.disp.draft = st.config.clone();
            self.disp.base = st.config;
            self.disp.started = true;
        }
    }

    pub(super) fn disp_len(&self) -> usize {
        hub().status().outputs.iter().filter(|o| o.connected).count()
    }

    fn disp_selected(&self) -> Option<crate::display::outputs::OutView> {
        let st = hub().status();
        let live: Vec<_> = st.outputs.into_iter().filter(|o| o.connected).collect();
        live.get(self.sel().min(live.len().saturating_sub(1))).cloned()
    }

    fn disp_dirty(&self) -> bool {
        self.disp.draft != self.disp.base
    }

    /// The Memory line, the switch and what the gate allows now.
    pub(super) fn disp_gate(&self, cx: &AppCx) -> (String, Switch) {
        let sw = memgate::read_switch(cx.services.dir());
        let st = hub().status();
        let modes: Vec<DisplayMode> = st.outputs.iter().filter(|o| o.connected).filter_map(|o| o.current.or(o.modes.first().map(|m| (m.0, m.1)))).map(|(w, h)| DisplayMode { w, h }).collect();
        let mem = memgate::read_mem_now();
        let a = memgate::features_with(sw, mem, &modes, &[]);
        (memgate::memory_line(&a, mem), sw)
    }

    pub(super) fn handle_displays(&mut self, k: &Key, cx: &AppCx) {
        self.disp_sync();
        if !self.admin(cx) {
            return;
        }
        match k {
            Key::Up => return self.move_sel(-1, self.disp_len()),
            Key::Down => return self.move_sel(1, self.disp_len()),
            _ => {}
        }
        let st = hub().status();
        let h = hub();
        // the trial: keep or go back
        if st.trial_secs.is_some() {
            match k {
                Key::Char('k') | Key::Enter => {
                    h.request(Request::Keep);
                    return self.ok("Display settings kept.");
                }
                Key::Char('v') | Key::Esc => {
                    h.request(Request::Revert);
                    return self.ok("Going back to the old display settings.");
                }
                _ => {}
            }
        }
        match k {
            Key::Char('s') => {
                let next = memgate::read_switch(cx.services.dir()).next();
                let _ = memgate::write_switch(cx.services.dir(), next);
                self.ok(format!("Screen features: {}.", next.label()));
            }
            Key::Char('i') => {
                h.request(Request::Identify);
                if !st.kms {
                    // without the shell driving the displays there is only one picture; the number shows on it
                    hub().identify(crate::display::outputs::now_ms() + 3000);
                }
                self.ok("A number is shown on each display for three seconds.");
            }
            _ if !st.kms => self.fail(format!("The shell does not drive the displays here: {}.", if st.why_not.is_empty() { "unknown".to_string() } else { st.why_not.clone() })),
            Key::Char('r') | Key::Char('R') => {
                let Some(o) = self.disp_selected() else { return };
                let list = sizes(&o.modes);
                if list.is_empty() {
                    return;
                }
                let cur = self.disp.draft.mode_of(&o.name).or(o.current).and_then(|c| list.iter().position(|s| *s == c)).unwrap_or(0);
                let next = if matches!(k, Key::Char('R')) { (cur + list.len() - 1) % list.len() } else { (cur + 1) % list.len() };
                self.disp.draft.set_mode(&o.name, list[next]);
                self.ok(format!("{}: {}x{} (not applied yet).", o.name, list[next].0, list[next].1));
            }
            Key::Char('p') => {
                if let Some(o) = self.disp_selected() {
                    self.disp.draft.primary = Some(o.name.clone());
                    self.ok(format!("{} will be the main display (not applied yet).", o.name));
                }
            }
            Key::Char('a') => {
                if let Some(o) = self.disp_selected() {
                    let next = self.disp.draft.placement_of(&o.name).next();
                    self.disp.draft.set_placement(&o.name, next);
                    self.ok(format!("{} will be {} the main display (not applied yet).", o.name, next.label()));
                }
            }
            Key::Char('m') => {
                self.disp.draft.mirror = !self.disp.draft.mirror;
                self.ok(if self.disp.draft.mirror { "Mirror: every display shows the same picture (not applied yet)." } else { "Extend: every display has its own picture (not applied yet)." });
            }
            Key::Char('d') => {
                let n = crate::touchmap::shared().lines().len();
                self.disp.touch_sel = if n == 0 { 0 } else { (self.disp.touch_sel + 1) % n };
                self.ok("Next touch screen selected.");
            }
            Key::Char('t') => {
                // Match automatically -> display 1 -> display 2 -> ... -> Match automatically (kept at once, per device)
                let names: Vec<String> = st.outputs.iter().filter(|o| o.connected).map(|o| o.name.clone()).collect();
                let mut m = crate::touchmap::shared();
                let Some(line) = m.lines().get(self.disp.touch_sel.min(m.lines().len().saturating_sub(1))).cloned() else { return self.fail("No touch screen was found.") };
                let kept = m.saved().iter().find(|s| s.0 == line.key).map(|s| s.1.clone()); // the file's word, not the published line (it lags a beat)
                let next = match kept.as_ref().and_then(|s| names.iter().position(|n| n == s)) {
                    None => names.first().cloned(),
                    Some(i) => names.get(i + 1).cloned(),
                };
                m.set_choice(&line.key, next.as_deref());
                match next {
                    Some(d) => self.ok(format!("Touch screen {}: shows on {d}.", line.name)),
                    None => self.ok(format!("Touch screen {}: matched automatically.", line.name)),
                }
            }
            Key::Char('c') => {
                let names: Vec<String> = st.outputs.iter().filter(|o| o.connected).map(|o| o.name.clone()).collect();
                let mut m = crate::touchmap::shared();
                if m.calibrating().is_some() {
                    m.cancel_calibration();
                    return self.ok("Touch calibration stopped.");
                }
                m.start_calibration(names, crate::touchmap::now_ms());
                self.ok("Touch the highlighted screen with the touch screen that belongs to it (Esc skips a display).");
            }
            Key::Char('y') | Key::Enter => {
                if !self.disp_dirty() {
                    return self.ok("Nothing has changed.");
                }
                h.request(Request::Apply(self.disp.draft.clone()));
                self.ok("Trying the new settings: keep them within 15 seconds or they are undone.");
            }
            _ => {}
        }
    }

    pub(super) fn displays_buttons(&self) -> Vec<Btn> {
        let (acc, mut_, err) = (ACCENT, TEXT, ERR);
        let mut v = vec![("Resolution", mut_, 'r'), ("Main display", mut_, 'p'), ("Side", mut_, 'a'), ("Mirror / Extend", mut_, 'm'), ("Identify", acc, 'i'), ("Touch screen", mut_, 't'), ("Next touch screen", mut_, 'd'), ("Touch calibration", acc, 'c'), ("Apply", acc, 'y')];
        if hub().status().trial_secs.is_some() {
            v.push(("Keep", acc, 'k'));
            v.push(("Go back", err, 'v'));
        }
        v.push(("Screen features", mut_, 's'));
        v
    }

    pub(super) fn render_displays(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        self.disp_sync();
        let st = hub().status();
        let (line, sw) = self.disp_gate(cx);
        let mut y = a.y;
        let wrap = |g: &mut Gfx, text: &str, size: f32, color: [u8; 4], y: &mut f32| {
            for l in g.wrap(size, text, Face::Sans, a.w) {
                g.text(a.x, *y, size, color, &l, Face::Sans);
                *y += size * 1.4;
            }
        };
        if st.kms {
            wrap(g, "The shell drives each display itself (extended or mirrored).", 14.0 * s, OK, &mut y);
        } else {
            let why = if st.why_not.is_empty() { "not decided yet".to_string() } else { st.why_not.clone() };
            wrap(g, &format!("One picture on every screen (the framebuffer): {why}."), 14.0 * s, crate::fx::AMBER, &mut y);
        }
        wrap(g, &line, 13.0 * s, MUTED, &mut y);
        wrap(g, &format!("Screen features: {}  (s changes it)", sw.label()), 13.0 * s, TEXT, &mut y);
        y += 6.0 * s;
        // the trial
        if let Some(secs) = st.trial_secs {
            let r = R::new(a.x, y, a.w, 34.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(70, 52, 14));
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.5 * s, crate::fx::AMBER);
            g.text(r.x + 12.0 * s, r.y + 8.0 * s, 15.0 * s, TEXT, &format!("Keep these settings?  They are undone in {secs} s.   k keep · v go back"), Face::Bold);
            y += 44.0 * s;
        }
        // the outputs
        let live: Vec<_> = st.outputs.iter().filter(|o| o.connected).collect();
        let sel = self.sel().min(live.len().saturating_sub(1));
        if live.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "No connected display was found.", Face::Sans);
            y += 24.0 * s;
        }
        for (i, o) in live.iter().enumerate() {
            let row = R::new(a.x, y, a.w, 30.0 * s);
            if i == sel {
                g.rrect(row.x, row.y, row.w, row.h, 8.0 * s, rgb(28, 36, 66));
            }
            let want = self.disp.draft.mode_of(&o.name);
            let size = want.or(o.current).map_or("?".to_string(), |m| format!("{}x{}", m.0, m.1));
            let changed = want.is_some() && want != o.current;
            let primary = self.disp.draft.primary.as_deref().map_or(o.primary, |p| p == o.name);
            let place = if primary { "main display".to_string() } else if self.disp.draft.mirror { "mirror".to_string() } else { self.disp.draft.placement_of(&o.name).label().to_string() + " the main display" };
            let text = format!("{}   {}{}   {}", o.name, size, if changed { "  (new)" } else { "" }, place);
            let t = g.fit(14.0 * s, &text, Face::Bold, row.w - 16.0 * s);
            g.text(row.x + 8.0 * s, row.y + 7.0 * s, 14.0 * s, if i == sel { TEXT } else { MUTED }, &t, Face::Bold);
            g.hit(row, Event::Row(i), &format!("display {}", o.name));
            y += 32.0 * s;
        }
        // a small picture of the arrangement
        if live.len() > 1 {
            let (mut x1, mut y1) = (1, 1);
            for o in &live {
                let (w, h) = o.current.unwrap_or((1920, 1080));
                x1 = x1.max(o.pos.0 + w as i32);
                y1 = y1.max(o.pos.1 + h as i32);
            }
            let box_h = (a.y + a.h - y - 10.0 * s).clamp_safe(0.0, 120.0 * s);
            if box_h > 30.0 * s {
                let sc = (box_h / y1 as f32).min((a.w * 0.5) / x1 as f32);
                for (i, o) in live.iter().enumerate() {
                    let (w, h) = o.current.unwrap_or((1920, 1080));
                    let r = R::new(a.x + 4.0 * s + o.pos.0 as f32 * sc, y + 4.0 * s + o.pos.1 as f32 * sc, w as f32 * sc - 2.0, h as f32 * sc - 2.0);
                    g.rrect(r.x, r.y, r.w, r.h, 3.0 * s, if i == sel { crate::fx::with_alpha(ACCENT, 90) } else { rgb(34, 42, 72) });
                    g.rrect_outline(r.x, r.y, r.w, r.h, 3.0 * s, 1.0, ACCENT);
                    let n = (i + 1).to_string();
                    let nw = g.text_width(14.0 * s, &n, Face::Bold);
                    g.text(r.x + (r.w - nw) / 2.0, r.y + (r.h - 14.0 * s) / 2.0, 14.0 * s, TEXT, &n, Face::Bold);
                }
                y += box_h + 6.0 * s;
            }
        }
        // the touch screens and the display each one belongs to
        let lines = crate::touchmap::shared().lines().to_vec();
        if !lines.is_empty() {
            y += 4.0 * s;
            g.text(a.x, y, 14.0 * s, TEXT, "Touch screens  (d next, t changes its display, c calibrates)", Face::Bold);
            y += 22.0 * s;
            let at = self.disp.touch_sel.min(lines.len() - 1);
            for (i, l) in lines.iter().enumerate() {
                let to = match (&l.saved, &l.display) {
                    (Some(_), Some(d)) => format!("{d} (chosen)"),
                    (None, Some(d)) => format!("{d} (Match automatically)"),
                    (_, None) => "no display".to_string(),
                };
                let t = g.fit(13.0 * s, &format!("Touch screen: {} {} -> {}", l.id, l.name, to), Face::Sans, a.w - 16.0 * s);
                g.text(a.x + 8.0 * s, y, 13.0 * s, if i == at { TEXT } else { MUTED }, &t, Face::Sans);
                y += 18.0 * s;
            }
        }
        if let Some(d) = crate::touchmap::shared().calibrating() {
            wrap(g, &format!("Touch calibration: touch the highlighted screen ({d}). Esc on the keyboard skips it."), 13.0 * s, crate::fx::AMBER, &mut y);
        }
        for n in st.notes.iter() {
            wrap(g, n, 13.0 * s, crate::fx::AMBER, &mut y);
        }
        if self.disp_dirty() {
            wrap(g, "Changed here, not applied yet: Apply tries it for 15 seconds.", 13.0 * s, crate::fx::AMBER, &mut y);
        }
        let _ = y;
    }
}
