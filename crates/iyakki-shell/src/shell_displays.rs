//! Several displays in the shell: one workspace of panes per display, one working display (the one with the keyboard focus), the
//! pointer's display, moving an app to another display, and the picture of each display. The device side (which output shows
//! what) is in `display/` and `backend_kms.rs`; here a display is a name, a size and a place.

use super::*;
use crate::display::outputs::{hub, now_ms, DispSpec, DisplayConfig};

#[derive(Default)]
pub(super) struct DispState {
    pub(super) list: Vec<DispSpec>,
    /// The display with the keyboard focus; its workspace is `PanesUi::ws`.
    pub(super) active: usize,
    /// The workspaces of the other displays (the active one's slot is `None`).
    pub(super) others: Vec<Option<Workspace>>,
    pub(super) other_hits: Vec<Vec<Hit>>,
    /// The display the pointer is on (the events' coordinates are on it).
    pub(super) pointer: usize,
    /// A pane picked up with Move: (display, pane).
    pub(super) move_src: Option<(usize, usize)>,
    pub(super) multi: bool,
    /// A display other than the working one is being drawn.
    pub(super) inactive: bool,
    /// The display being drawn.
    pub(super) drawing: usize,
}

fn fresh_ws(w: u32, h: u32) -> Workspace {
    Workspace { layout: Layout::single(), panes: vec![Pane::new()], focus: 0, rects: Vec::new(), drag: None, scratch: None, redraws: 0, disp: (w, h) }
}

type Content = (Option<(Box<dyn App>, PApp)>, Option<String>);

impl Shell {
    pub(in crate::shell) fn disp_mouse(&self, k: usize) -> (i32, i32) {
        if !self.panes.d.multi || self.panes.d.pointer == k {
            self.mouse
        } else {
            (-9999, -9999)
        }
    }

    /// The display being drawn now.
    pub(in crate::shell) fn panes_drawing_display(&self) -> usize {
        self.panes.d.drawing
    }

    /// Pretend display `k` is being drawn (tests of what only the main display shows).
    #[cfg(test)]
    pub(in crate::shell) fn test_set_drawing_display(&mut self, k: usize) {
        self.panes.d.drawing = k;
    }

    /// The pane in focus on the working display, when there is a workspace.
    pub(in crate::shell) fn panes_focus_index(&self) -> Option<usize> {
        self.panes.ws.as_ref().map(|ws| ws.focus)
    }

    pub(in crate::shell) fn panes_active_display(&self) -> usize {
        self.panes.d.active
    }

    /// The signed-in person's name, for the display file.
    pub fn current_user(&self) -> Option<String> {
        self.session.as_ref().map(|s| s.user.clone())
    }

    /// The folder of the per-person display files.
    pub fn display_dir(&self) -> std::path::PathBuf {
        self.env.services.dir().to_path_buf()
    }

    /// What panes and extra displays may do on this machine now (the memory gate), for these display modes (the first is the primary).
    pub fn display_features(&self, modes: &[crate::memgate::DisplayMode]) -> crate::memgate::Allowed {
        let sw = crate::memgate::read_switch(self.env.services.dir());
        crate::memgate::features_with(sw, self.panes_mem(), modes, &self.pane_classes())
    }

    /// The working display (the one with the keyboard focus).
    pub fn displays_active(&self) -> usize {
        self.panes.d.active
    }

    pub fn display_count(&self) -> usize {
        self.panes.d.list.len().max(1)
    }

    /// The display the pointer is on; the coordinates of the pointer events are on that display.
    pub fn set_pointer_display(&mut self, k: usize) {
        if k < self.panes.d.list.len().max(1) {
            self.panes.d.pointer = k;
        }
    }

    pub(super) fn pane_under(&self, disp: usize, x: i32, y: i32) -> Option<usize> {
        let ws = if disp == self.panes.d.active { self.panes.ws.as_ref() } else { self.panes.d.others.get(disp).and_then(|w| w.as_ref()) }?;
        ws.rects.iter().position(|r| r.contains(x, y))
    }

    /// Tell the shell which displays there are (the first is the primary). With one display everything is as before; with more,
    /// every display gets a workspace. `saved` carries the layout template each display had last time.
    pub fn set_displays(&mut self, specs: &[DispSpec], saved: &DisplayConfig) {
        let n = specs.len();
        if n <= 1 {
            if self.panes.d.multi {
                self.leave_multi();
            }
            self.panes.d.list = specs.to_vec();
            self.panes.d.active = 0;
            self.panes.d.pointer = 0;
            return;
        }
        let old = std::mem::replace(&mut self.panes.d.list, specs.to_vec());
        if !self.panes.d.multi || self.panes.ws.is_none() {
            // entering: the working display is the first
            self.panes.d.multi = true;
            self.panes.d.active = 0;
            self.panes.d.others = (0..n).map(|_| None).collect();
            self.panes.d.other_hits = vec![Vec::new(); n];
            self.signed_in_workspaces(saved);
            self.panes.d.pointer = self.panes.d.pointer.min(n - 1);
            return;
        }
        // a change of what is plugged in: keep the workspaces of the displays that are still there (by name)
        let active_name = old.get(self.panes.d.active).map(|o| o.name.clone()).unwrap_or_default();
        let mut by_name: Vec<(String, Workspace)> = Vec::new();
        let mut act = self.panes.ws.take().unwrap_or_else(|| fresh_ws(specs[0].w, specs[0].h));
        let out = self.ws_take_screen();
        act.panes[act.focus].content = out.0;
        act.panes[act.focus].custom = out.1;
        let others = std::mem::take(&mut self.panes.d.others);
        for (i, w) in others.into_iter().enumerate() {
            if let (Some(w), Some(o)) = (w, old.get(i)) {
                by_name.push((o.name.clone(), w));
            }
        }
        by_name.push((active_name.clone(), act));
        let mut new_ws: Vec<Workspace> = Vec::new();
        let mut created = Vec::new();
        for (j, sp) in specs.iter().enumerate() {
            match by_name.iter().position(|(nm, _)| *nm == sp.name) {
                Some(p) => new_ws.push(by_name.remove(p).1),
                None => {
                    new_ws.push(fresh_ws(sp.w, sp.h));
                    created.push(j);
                }
            }
        }
        // the active display stays if it is still there, else the first
        let active = specs.iter().position(|sp| sp.name == active_name).unwrap_or(0);
        // displays that went away give their apps to launcher panes on the displays that are left
        let mut gone: Vec<Content> = Vec::new();
        for (_, mut w) in by_name {
            for p in w.panes.iter_mut() {
                if p.content.is_some() {
                    gone.push((p.content.take(), p.custom.take()));
                }
            }
        }
        let mut cur = new_ws.remove(active);
        let f = cur.focus.min(cur.panes.len() - 1);
        cur.focus = f;
        let inn = (cur.panes[f].content.take(), cur.panes[f].custom.take());
        self.ws_put_screen(inn);
        let mut placed_names = Vec::new();
        let mut closed = Vec::new();
        for c in gone {
            let title = c.0.as_ref().map(|(a, _)| a.title().to_string()).unwrap_or_default();
            // first a launcher pane of the working display, then of the others
            let slot = cur.panes.iter().position(|p| p.content.is_none()).filter(|i| *i != cur.focus);
            if let Some(i) = slot {
                cur.panes[i].content = c.0;
                cur.panes[i].custom = c.1;
                cur.panes[i].dirty = true;
                placed_names.push(title);
                continue;
            }
            let mut put = false;
            for w in new_ws.iter_mut() {
                if let Some(i) = w.panes.iter().position(|p| p.content.is_none()) {
                    w.panes[i].content = c.0;
                    w.panes[i].custom = c.1;
                    put = true;
                    break;
                }
            }
            if put {
                placed_names.push(title);
            } else {
                closed.push(title);
            }
        }
        new_ws.insert(active, fresh_ws(0, 0));
        self.panes.d.others = new_ws.into_iter().map(Some).collect();
        self.panes.d.others[active] = None;
        self.panes.d.other_hits = vec![Vec::new(); n];
        self.panes.d.active = active;
        self.panes.ws = Some(cur);
        self.size = (specs[active].w, specs[active].h);
        self.panes.d.pointer = self.panes.d.pointer.min(n - 1);
        self.apply_saved_templates(saved, &created);
        if !placed_names.is_empty() {
            self.say_info(format!("A display was unplugged: {} moved to the screens that are left", placed_names.join(", ")));
        }
        if !closed.is_empty() {
            self.say(format!("A display was unplugged and there was no room for: {}", closed.join(", ")));
        }
        self.stale = true;
    }

    /// Workspaces for every display once somebody is signed in (before that, other displays show the same sign-in screen).
    fn signed_in_workspaces(&mut self, saved: &DisplayConfig) {
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher | Screen::App(..)) {
            return;
        }
        let list = self.panes.d.list.clone();
        if self.panes.ws.is_none() {
            self.panes.ws = Some(fresh_ws(list[0].w, list[0].h));
        }
        for (k, sp) in list.iter().enumerate() {
            if k != self.panes.d.active && self.panes.d.others[k].is_none() {
                self.panes.d.others[k] = Some(fresh_ws(sp.w, sp.h));
            }
        }
        let all: Vec<usize> = (0..list.len()).collect();
        self.apply_saved_templates(saved, &all);
    }

    /// Give the displays in `which` the layout they had last time (when the memory allows it).
    fn apply_saved_templates(&mut self, saved: &DisplayConfig, which: &[usize]) {
        let keep = self.panes.d.active;
        for &k in which {
            let Some(name) = self.panes.d.list.get(k).map(|d| d.name.clone()) else { continue };
            let Some(t) = saved.template_of(&name).map(str::to_string) else { continue };
            if t == "single" {
                continue;
            }
            self.focus_display(k);
            let _ = self.ws_apply(&t, None, true);
        }
        self.focus_display(keep);
    }

    /// One display left: everything comes to it.
    fn leave_multi(&mut self) {
        let others = std::mem::take(&mut self.panes.d.others);
        self.panes.d.multi = false;
        self.panes.d.move_src = None;
        let mut closed = Vec::new();
        let mut moved = Vec::new();
        let mut gone: Vec<Content> = Vec::new();
        for w in others.into_iter().flatten() {
            for mut p in w.panes {
                if p.content.is_some() {
                    gone.push((p.content.take(), p.custom.take()));
                }
            }
        }
        let _ = self.panes.d.active;
        // the working display keeps its own workspace, whichever display it was; the apps of the others go to launcher panes
        if self.panes.ws.is_none() {
            return;
        }
        for c in gone {
            let title = c.0.as_ref().map(|(a, _)| a.title().to_string()).unwrap_or_default();
            let slot = self.panes.ws.as_ref().and_then(|ws| ws.panes.iter().enumerate().position(|(i, p)| i != ws.focus && p.content.is_none()));
            match (slot, self.panes.ws.as_mut()) {
                (Some(i), Some(ws)) => {
                    ws.panes[i].content = c.0;
                    ws.panes[i].custom = c.1;
                    ws.panes[i].dirty = true;
                    moved.push(title);
                }
                _ => closed.push(title),
            }
        }
        // a single pane needs no workspace any more
        if self.panes.ws.as_ref().is_some_and(|ws| ws.layout.leaf_count() == 1) {
            self.panes.ws = None;
        }
        self.panes.d.active = 0;
        self.panes.d.pointer = 0;
        self.panes.d.other_hits.clear();
        if !moved.is_empty() {
            self.say_info(format!("Back to one display: {} kept in the panes of this screen", moved.join(", ")));
        }
        if !closed.is_empty() {
            self.say(format!("Back to one display; there was no room for: {}", closed.join(", ")));
        }
        self.stale = true;
    }

    /// Give the keyboard focus to another display.
    pub(super) fn focus_display(&mut self, k: usize) {
        let (multi, active, n) = (self.panes.d.multi, self.panes.d.active, self.panes.d.list.len());
        if !multi || k == active || k >= n || self.panes.ws.is_none() {
            return;
        }
        let Some(mut act) = self.panes.ws.take() else { return };
        let out = self.ws_take_screen();
        act.panes[act.focus].content = out.0;
        act.panes[act.focus].custom = out.1;
        act.panes[act.focus].dirty = true;
        let sp = self.panes.d.list[k].clone();
        let mut new = self.panes.d.others[k].take().unwrap_or_else(|| fresh_ws(sp.w, sp.h));
        let f = new.focus.min(new.panes.len() - 1);
        new.focus = f;
        let inn = (new.panes[f].content.take(), new.panes[f].custom.take());
        new.panes[f].dirty = true;
        self.ws_put_screen(inn);
        self.panes.d.others[active] = Some(act);
        self.panes.ws = Some(new);
        self.panes.d.active = k;
        self.size = (sp.w, sp.h);
        let h = std::mem::take(&mut self.panes.d.other_hits[k]);
        let prev = std::mem::replace(&mut self.hits, h);
        self.panes.d.other_hits[active] = prev;
        self.ctx = None;
        self.stale = true;
    }

    fn content_take(&mut self, d: usize, p: usize) -> Option<Content> {
        if d == self.panes.d.active {
            let f = self.panes.ws.as_ref()?.focus;
            if p >= self.panes.ws.as_ref()?.panes.len() {
                return None;
            }
            if p == f {
                return Some(self.ws_take_screen());
            }
            let pane = &mut self.panes.ws.as_mut()?.panes[p];
            pane.dirty = true;
            return Some((pane.content.take(), pane.custom.take()));
        }
        let pane = self.panes.d.others.get_mut(d)?.as_mut()?.panes.get_mut(p)?;
        pane.dirty = true;
        Some((pane.content.take(), pane.custom.take()))
    }

    fn content_put(&mut self, d: usize, p: usize, c: Content) {
        if d == self.panes.d.active {
            if self.panes.ws.as_ref().is_some_and(|ws| ws.focus == p) {
                self.ws_put_screen(c);
                self.ws_mark_dirty(p);
            } else if let Some(pane) = self.panes.ws.as_mut().and_then(|ws| ws.panes.get_mut(p)) {
                pane.content = c.0;
                pane.custom = c.1;
                pane.dirty = true;
            }
        } else if let Some(pane) = self.panes.d.others.get_mut(d).and_then(|w| w.as_mut()).and_then(|w| w.panes.get_mut(p)) {
            pane.content = c.0;
            pane.custom = c.1;
            pane.dirty = true;
        }
        self.stale = true;
    }

    /// Swap what two panes hold (on the same display or not); an app keeps running.
    pub(super) fn move_content(&mut self, a: (usize, usize), b: (usize, usize)) {
        if a == b {
            return;
        }
        let (Some(ca), Some(cb)) = (self.content_take(a.0, a.1), self.content_take(b.0, b.1)) else { return };
        self.content_put(a.0, a.1, cb);
        self.content_put(b.0, b.1, ca);
        self.ctx = None;
    }

    /// Ctrl+Alt+M and the arrow keys at an edge: the working pane's app goes to display `k`, into a pane that holds no app
    /// (else that display's working pane, which comes back here), and the keyboard follows it.
    pub(super) fn move_focused_to_display(&mut self, k: usize) {
        let (active, n) = (self.panes.d.active, self.panes.d.list.len());
        if !self.panes.d.multi || k == active || k >= n || self.panes.ws.is_none() {
            return;
        }
        let src = (active, self.panes.ws.as_ref().map_or(0, |ws| ws.focus));
        let target_ws = self.panes.d.others[k].as_ref();
        let p = target_ws.and_then(|w| w.panes.iter().position(|p| p.content.is_none())).or_else(|| target_ws.map(|w| w.focus)).unwrap_or(0);
        self.move_content(src, (k, p));
        self.focus_display(k);
        self.ws_focus(p);
    }

    /// The display next to the working one on `side` of the arrangement.
    pub(super) fn display_on_side(&self, side: crate::panes::Side) -> Option<usize> {
        use crate::panes::Side;
        let l = &self.panes.d.list;
        let me = l.get(self.panes.d.active)?;
        let (mx0, my0, mx1, my1) = (me.x, me.y, me.x + me.w as i32, me.y + me.h as i32);
        l.iter()
            .enumerate()
            .filter(|(i, _)| *i != self.panes.d.active)
            .filter_map(|(i, d)| {
                let (x0, y0, x1, y1) = (d.x, d.y, d.x + d.w as i32, d.y + d.h as i32);
                let v_overlap = y0.max(my0) < y1.min(my1);
                let h_overlap = x0.max(mx0) < x1.min(mx1);
                let dist = match side {
                    Side::Right if v_overlap && x0 >= mx1 - 1 => x0 - mx1,
                    Side::Left if v_overlap && x1 <= mx0 + 1 => mx0 - x1,
                    Side::Down if h_overlap && y0 >= my1 - 1 => y0 - my1,
                    Side::Up if h_overlap && y1 <= my0 + 1 => my0 - y1,
                    _ => return None,
                };
                Some((dist, i))
            })
            .min()
            .map(|(_, i)| i)
    }

    /// The apps in the workspaces of the displays that are not the working one get their heartbeat.
    pub(super) fn tick_other_displays(&mut self, cx: &AppCx) {
        let tick = self.tick;
        let mut others = std::mem::take(&mut self.panes.d.others);
        let mut crashed = Vec::new();
        for (k, w) in others.iter_mut().enumerate() {
            let Some(w) = w else { continue };
            for (i, p) in w.panes.iter_mut().enumerate() {
                let Some((app, pa)) = p.content.as_mut() else { continue };
                let pa = *pa;
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    app.handle(&Event::Tick, cx);
                    app.wants_redraw()
                })) {
                    Ok(want) => {
                        if want || self.bg_stale(pa, tick) {
                            p.dirty = true;
                        }
                    }
                    Err(_) => crashed.push((k, i, app.title())),
                }
            }
        }
        for (k, i, name) in crashed {
            if let Some(Some(w)) = others.get_mut(k) {
                w.panes[i].content = None;
                w.panes[i].dirty = true;
            }
            self.app_crashed(&name);
        }
        self.panes.d.others = others;
    }

    /// Draw display `k` into `g` (the size of its mode). The working display is the whole shell; the others are their panes.
    /// Draw the other displays first: the working display's frame sets the shell's idea of its own size.
    pub fn render_display(&mut self, k: usize, g: &mut Gfx) {
        if self.saver_render(g) {
            return; // the saver (or black) on every display
        }
        if self.panes.d.multi && self.session.is_some() && self.panes.ws.is_none() {
            let saved = match self.current_user() {
                Some(u) => crate::display::outputs::load(&self.display_dir(), &u),
                None => DisplayConfig::default(),
            };
            self.signed_in_workspaces(&saved);
        }
        if !self.panes.d.multi || k == self.panes.d.active || self.panes.d.others.get(k).is_none_or_none() || !self.ws_active() {
            self.render(g);
        } else {
            self.render_other(k, g);
        }
        self.draw_identify(g, k);
    }

    fn render_other(&mut self, k: usize, g: &mut Gfx) {
        let (w, h) = (g.width(), g.height());
        let s = scale_for(w, h);
        let Some(wk) = self.panes.d.others[k].take() else { return };
        let act = self.panes.ws.take();
        self.panes.ws = Some(wk);
        self.panes.d.inactive = true;
        self.panes.d.drawing = k;
        self.vol.rects.retain(|p| p.disp != k);
        g.hits.clear();
        g.clear_probes();
        g.mouse = self.disp_mouse(k);
        g.gradient(BG_TOP, BG_BOTTOM);
        self.draw_panes(g, w, h, s);
        self.panes.d.inactive = false;
        self.panes.d.drawing = self.panes.d.active;
        self.panes.d.others[k] = self.panes.ws.take();
        self.panes.ws = act;
        self.panes.d.other_hits[k] = std::mem::take(&mut g.hits);
    }

    /// Settings -> Displays "Touch calibration": the display being asked shows a bright prompt, the others a quiet one.
    fn draw_touch_prompt(&mut self, g: &mut Gfx, k: usize) {
        let Some(asked) = crate::touchmap::shared().calibrating().map(str::to_string) else { return };
        let (w, h) = (g.width() as f32, g.height() as f32);
        let s = scale_for(g.width(), g.height());
        let mine = self.panes.d.list.get(k).is_some_and(|d| d.name == asked);
        g.rect(0.0, 0.0, w, h, [3, 5, 14, if mine { 215 } else { 170 }]);
        let (title, color) = if mine { ("Touch this screen".to_string(), crate::fx::CYAN) } else { (format!("Touch the highlighted screen ({asked})"), DIM) };
        let size = if mine { h * 0.09 } else { 30.0 * s };
        let tw = g.text_width(size, &title, Face::Bold);
        g.text((w - tw) / 2.0, h * 0.38, size, color, &title, Face::Bold);
        if mine {
            let sub = "With the touch screen that belongs to this display. Esc skips it.";
            let sw = g.text_width(22.0 * s, sub, Face::Sans);
            g.text((w - sw) / 2.0, h * 0.38 + size * 1.5, 22.0 * s, TEXT, sub, Face::Sans);
            g.rrect_outline(8.0 * s, 8.0 * s, w - 16.0 * s, h - 16.0 * s, 12.0 * s, 4.0 * s, crate::fx::CYAN);
        }
    }

    /// Settings -> Displays "Identify": a big number on each screen for a few seconds.
    fn draw_identify(&mut self, g: &mut Gfx, k: usize) {
        self.draw_touch_prompt(g, k);
        if !hub().identifying(now_ms()) {
            return;
        }
        let (w, h) = (g.width() as f32, g.height() as f32);
        let s = scale_for(g.width(), g.height());
        g.rect(0.0, 0.0, w, h, [3, 5, 14, 170]);
        let num = (k + 1).to_string();
        let size = h * 0.45;
        let tw = g.text_width(size, &num, Face::Bold);
        g.text((w - tw) / 2.0, h * 0.12, size, crate::fx::CYAN, &num, Face::Bold);
        let name = self.panes.d.list.get(k).map(|d| format!("{}  {}x{}", d.name, d.w, d.h)).unwrap_or_default();
        let nw = g.text_width(26.0 * s, &name, Face::Bold);
        g.text((w - nw) / 2.0, h * 0.12 + size * 1.05, 26.0 * s, TEXT, &name, Face::Bold);
    }

    /// Keep each display's layout for next time (the file of the signed-in person).
    pub(super) fn save_display_templates(&mut self) {
        if !self.panes.d.multi {
            return;
        }
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        let dir = self.env.services.dir().to_path_buf();
        let mut cfg = crate::display::outputs::load(&dir, &user);
        let list = self.panes.d.list.clone();
        for (k, sp) in list.iter().enumerate() {
            let id = if k == self.panes.d.active { self.panes.ws.as_ref().map(|w| w.layout.id.clone()) } else { self.panes.d.others.get(k).and_then(|w| w.as_ref()).map(|w| w.layout.id.clone()) };
            if let Some(id) = id {
                cfg.set_template(&sp.name, &id);
            }
        }
        let _ = crate::display::outputs::save(&dir, &user, &cfg);
    }

    // ---- tests

    #[cfg(test)]
    pub(crate) fn displays_active_for_test(&self) -> usize {
        self.panes.d.active
    }

    #[cfg(test)]
    pub(crate) fn display_layouts_for_test(&self) -> Vec<String> {
        (0..self.panes.d.list.len())
            .map(|k| if k == self.panes.d.active { self.panes.ws.as_ref().map(|w| w.layout.id.clone()) } else { self.panes.d.others.get(k).and_then(|w| w.as_ref()).map(|w| w.layout.id.clone()) }.unwrap_or_else(|| "-".into()))
            .collect()
    }

    /// What each display shows: the titles of its panes ("Home" for a launcher).
    #[cfg(test)]
    pub(crate) fn display_titles_for_test(&self, k: usize) -> Vec<String> {
        if k == self.panes.d.active {
            return self.panes_titles_for_test();
        }
        let Some(Some(w)) = self.panes.d.others.get(k) else { return vec![] };
        w.panes.iter().map(|p| p.content.as_ref().map_or("Home".to_string(), |(a, _)| a.title().to_string())).collect()
    }

    #[cfg(test)]
    pub(crate) fn display_rects_for_test(&self, k: usize) -> Vec<R> {
        if k == self.panes.d.active {
            return self.panes_rects_for_test();
        }
        self.panes.d.others.get(k).and_then(|w| w.as_ref()).map(|w| w.rects.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn display_hits_for_test(&self, k: usize) -> Vec<Hit> {
        if k == self.panes.d.active {
            self.hits.clone()
        } else {
            self.panes.d.other_hits.get(k).cloned().unwrap_or_default()
        }
    }

    #[cfg(test)]
    pub(crate) fn display_put_for_test(&mut self, k: usize, p: usize, app: Box<dyn App>, pa: PApp) {
        if k == self.panes.d.active {
            self.panes_put_for_test(p, app, pa);
        } else if let Some(pane) = self.panes.d.others.get_mut(k).and_then(|w| w.as_mut()).and_then(|w| w.panes.get_mut(p)) {
            pane.content = Some((app, pa));
            pane.dirty = true;
        }
    }
}

/// `Option<&Option<T>>` that is `None` or holds `None`.
trait NoneOrNone {
    fn is_none_or_none(&self) -> bool;
}

impl<T> NoneOrNone for Option<&Option<T>> {
    fn is_none_or_none(&self) -> bool {
        !matches!(self, Some(Some(_)))
    }
}
