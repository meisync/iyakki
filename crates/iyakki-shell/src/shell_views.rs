//! Desktop views: several named views on one screen, each with its own open apps, last tile and window arrangement (panes). The
//! model (names, order, lock flag, templates, key set) is `iyakki_shared::views`; this file keeps the live apps of every view and
//! switches between them.
//!
//! How a switch works: the shell has one live "screen" (the open app or the launcher), a workspace of panes when the person split the
//! display, and one app kept aside for Back. Those three are the *content* of a view. Switching moves them into the view being
//! left, and installs the target view's (or a fresh launcher the first time, with the apps of its template). Nothing is closed: the
//! apps of the other views stay as they are and come back as they were. Apps of a hidden view are not drawn and get no heartbeat;
//! work they started (a download, a run) goes on in the background.
//!
//! Keys: Ctrl+Alt+Super+1..9 jump, Ctrl+Alt+PageUp / PageDown (browser-style) and Control+Command+Left / Right (Mac-style) step. The
//! person picks which step set is on in Settings, Desktop views. None of them is the lock and sign-out chords (Control+Command+L and Q) or a pane key.
//!
//! A locked view asks for the password again: switching to it locks the screen (the existing lock screen, same password check and
//! throttle) and the switch happens when the right password is typed. Whether the lock returns to the first view is the person's
//! Settings choice (default: stay on the active view).

use crate::stale_stats::mark_stale;
use super::*;
use crate::panes::LayoutKey;
use iyakki_shared::views::Views;
use std::collections::HashMap;

/// Click regions of the strip: `Event::Tab(TAB + index in the person's order)`.
const TAB: usize = 0x5E00;

/// The content of a view that is not showing.
struct Slot {
    ws: Option<panes_impl::Workspace>,
    screen: Screen,
    custom_open: Option<String>,
    custom_at: u64,
    stash: Option<(Box<dyn App>, PApp)>,
}

impl Shell {
    /// Every app held by a view that is not showing (its screen, its panes and its stash).
    pub(super) fn views_each_hidden_app(&mut self, f: &mut dyn FnMut(&mut Box<dyn App>)) {
        for slot in self.views.slots.values_mut() {
            if let Screen::App(app, _) = &mut slot.screen {
                f(app);
            }
            if let Some((app, _)) = &mut slot.stash {
                f(app);
            }
            if let Some(ws) = &mut slot.ws {
                ws.each_app(f);
            }
        }
    }
}

pub(super) struct ViewsUi {
    pub(super) model: Views,
    /// The id of the view whose content is live on screen.
    pub(super) cur: u32,
    slots: HashMap<u32, Slot>,
    /// A locked view waiting for the password.
    pub(super) pending: Option<u32>,
    /// A view's apps are being put back: the one-window rule does not apply to that.
    restoring: bool,
}

impl Default for ViewsUi {
    fn default() -> ViewsUi {
        ViewsUi { model: Views::default(), cur: 1, slots: HashMap::new(), pending: None, restoring: false }
    }
}

impl Shell {
    // ------------------------------------------------------------------ sign-in, sign-out, Settings changes

    /// After sign-in: read the person's views and show the one that was active. A view made from a template opens its apps.
    pub(super) fn views_sign_in(&mut self) {
        let Some(s) = self.session.as_ref() else { return };
        let model = Views::load(self.env.data_dir(), &s.user);
        self.views = ViewsUi { cur: model.active, model, slots: HashMap::new(), pending: None, restoring: false };
        self.views_show_content(self.views.cur);
    }

    /// Forget the live apps of every view (they are dropped, the saved views stay).
    pub(super) fn views_sign_out(&mut self) {
        self.views = ViewsUi::default();
    }

    /// Read the file again (Settings -> Desktop views writes it). A view that was deleted loses its apps; when the live view is the
    /// one deleted, what is on screen becomes the first view's.
    pub(super) fn views_reload(&mut self) {
        let Some(s) = self.session.as_ref() else { return };
        let model = Views::load(self.env.data_dir(), &s.user);
        if model == self.views.model {
            return;
        }
        self.views.slots.retain(|id, _| model.get(*id).is_some());
        if model.get(self.views.cur).is_none() {
            self.views.cur = model.views[0].id;
            let mut m = model.clone();
            m.active = self.views.cur;
            m.save(self.env.data_dir(), &s.user);
            self.views.model = m;
        } else {
            self.views.model = model;
            self.views.model.active = self.views.cur;
        }
        if self.views.pending.is_some_and(|p| self.views.model.get(p).is_none()) {
            self.views.pending = None;
        }
        mark_stale!(self);
    }

    /// Look for what the assistant's view tools left in the data folder: a changed views file is read again, and a request to go to a view is
    /// followed (a locked view still shows the lock screen). Called a few times a second; two small file reads.
    pub(super) fn views_poll(&mut self) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        let dir = self.env.data_dir().to_path_buf();
        let to = iyakki_shared::views::take_switch(&dir, &user);
        self.views_reload();
        if let Some(id) = to {
            self.views_goto(id);
        }
    }

    pub(super) fn views_save(&self) {
        if let Some(s) = &self.session {
            self.views.model.save(self.env.data_dir(), &s.user);
        }
    }

    // ------------------------------------------------------------------ keys and clicks

    /// A view key from `panes_key`: jump to view N, or step to the previous or next one.
    pub(super) fn views_key(&mut self, k: LayoutKey) {
        let target = match k {
            LayoutKey::View(n) => match self.views.model.nth(n) {
                Some(id) => id,
                None => return self.say_info(format!("There is no view {n}")),
            },
            LayoutKey::ViewStep { back, mac } => {
                let set = self.views.model.key_set;
                if (mac && !set.mac()) || (!mac && !set.browser()) {
                    return self.say_info("That key is off (Settings, Desktop views, Keys)");
                }
                match self.views.model.step(back) {
                    Some(id) => id,
                    None => return self.say_info("Only one view"),
                }
            }
            _ => return,
        };
        self.views_goto(target);
    }

    pub(super) fn views_owns_tab(&self, n: usize) -> bool {
        self.session.is_some() && n >= TAB && n < TAB + self.views.model.views.len()
    }

    pub(super) fn views_tab(&mut self, n: usize) {
        if let Some(v) = self.views.model.views.get(n - TAB) {
            let id = v.id;
            self.views_goto(id);
        }
    }

    // ------------------------------------------------------------------ switching

    /// The name of the view that is showing.
    pub fn view_name(&self) -> String {
        self.views.model.get(self.views.cur).map(|v| v.name.clone()).unwrap_or_default()
    }

    /// Names of the person's views in order, with whether each is locked.
    pub fn view_names(&self) -> Vec<(String, bool)> {
        self.views.model.views.iter().map(|v| (v.name.clone(), v.locked)).collect()
    }

    /// Go to a view. A locked one locks the screen first and the switch is made when the password is right.
    pub(super) fn views_goto(&mut self, to: u32) {
        if self.session.is_none() || self.lockui.active || !matches!(self.screen, Screen::Launcher | Screen::App(..)) {
            return;
        }
        if to == self.views.cur {
            return;
        }
        let Some(v) = self.views.model.get(to).cloned() else { return };
        if v.locked {
            self.views.pending = Some(to);
            if !self.lock_screen("desktop view locked") {
                self.views.pending = None;
                self.say("A guest has no password, so a locked view cannot be opened");
            }
            return;
        }
        self.views_switch(to);
    }

    /// Make view `to` the live one. Its content is put on screen; the content that was live goes into its slot.
    fn views_switch(&mut self, to: u32) {
        if to == self.views.cur || self.views.model.get(to).is_none() {
            return;
        }
        let from = self.views.cur;
        let ws = self.panes_take_for_view();
        let screen = std::mem::replace(&mut self.screen, Screen::Launcher);
        if let Some(l) = Self::panes_describe(ws.as_ref(), &screen) {
            self.views.model.remember(from, l);
        }
        let leaving = Slot { ws, screen, custom_open: self.custom_open.take(), custom_at: std::mem::take(&mut self.custom_at), stash: self.stash.take() };
        self.views.slots.insert(from, leaving);
        self.ctx = None;
        self.help = false;
        self.views.cur = to;
        self.views.model.active = to;
        self.views_save();
        let name = self.view_name();
        match self.views.slots.remove(&to) {
            Some(slot) => {
                self.screen = slot.screen;
                self.custom_open = slot.custom_open;
                self.custom_at = slot.custom_at;
                self.stash = slot.stash;
                self.panes_put_for_view(slot.ws);
            }
            None => {
                self.panes_put_for_view(None);
                self.views_show_content(to);
            }
        }
        if let Some(s) = &self.session {
            self.env.audit.record(&s.user, "view.switch", &name);
        }
        self.say_info(format!("View: {name}"));
        mark_stale!(self);
    }

    /// Put the apps of view `id` on screen for the first time in this session: the window arrangement it had when it was last left, else the apps of its template.
    fn views_show_content(&mut self, id: u32) {
        let Some(v) = self.views.model.get(id).cloned() else { return };
        self.views.restoring = true;
        match v.layout.filter(|_| self.views.model.remember_layout) {
            Some(l) => self.views_open_saved(&l),
            None => self.views_open_tiles(&v.tiles),
        }
        self.views.restoring = false;
    }

    /// Open the arrangement a view had: its layout, and in each pane the app that was there (an app the role may not open, or that is hidden, leaves
    /// that pane on the launcher). A layout that is gone or does not fit leaves the first app that can open, alone.
    fn views_open_saved(&mut self, l: &iyakki_shared::views::SavedLayout) {
        let Some(cx) = self.cx() else { return };
        let apps: Vec<Option<PApp>> = l
            .apps
            .iter()
            .map(|t| PApp::ALL.into_iter().find(|a| a.id() == t).filter(|a| *a != PApp::Inbox && cx.session.policy.allows_app(*a) && !self.cloud.hides_app(*a)))
            .collect();
        let alone = |sh: &mut Shell| {
            if let Some(a) = apps.iter().flatten().next().copied() {
                sh.open(a);
            }
        };
        if apps.len() < 2 || !self.panes_switch_on() || self.ws_apply(&l.layout, Some(0), true).is_err() {
            return alone(self);
        }
        for (i, a) in apps.iter().enumerate() {
            if let Some(a) = a {
                self.ws_focus(i);
                self.open(*a);
            }
        }
        self.ws_focus(l.focus.min(apps.len() - 1));
    }

    /// Keep the arrangement that is showing now (sign-out): written to the person's file so the next sign-in puts it back.
    pub(super) fn views_remember_live(&mut self) {
        if self.session.is_none() {
            return;
        }
        if let Some(l) = Self::panes_describe(self.panes.ws.as_ref(), &self.screen) {
            let cur = self.views.cur;
            if self.views.model.remember(cur, l) {
                self.views_save();
            }
        }
    }

    /// The one-window rule: `app` is open in another pane of this view, or in another view. Goes there and says so (true); false when the rule
    /// is off for this app, or it is not open anywhere else (or the same window is already showing it), and the caller opens it as usual.
    pub(super) fn views_single_hit(&mut self, app: PApp) -> bool {
        if self.views.restoring || self.session.is_none() || !self.views.model.is_single(app.id()) || matches!(&self.screen, Screen::App(_, p) if *p == app) {
            return false;
        }
        if let Some(i) = self.panes.ws.as_ref().and_then(|ws| Self::panes_pane_of(ws, app)) {
            self.ws_focus(i);
            self.say_info(format!("{} is already open: went to it", app.id().to_uppercase()));
            return true;
        }
        let found = self.views.slots.iter().find_map(|(id, s)| match &s.screen {
            Screen::App(_, p) if *p == app => Some((*id, None)),
            _ => s.ws.as_ref().and_then(|ws| Self::panes_pane_of(ws, app)).map(|i| (*id, Some(i))),
        });
        let Some((id, pane)) = found else { return false };
        self.views_goto(id);
        if self.views.cur == id {
            if let Some(i) = pane {
                self.ws_focus(i);
            }
            self.say_info(format!("{} is already open in the view {}: went to it", app.id().to_uppercase(), self.view_name()));
        }
        true
    }

    /// The apps of a template, in pane order: the first fills the screen; with more, a layout is made and each opens in its pane.
    /// Apps the role may not open, or that are hidden in cloud mode, are left out; a layout that does not fit leaves the first app only.
    fn views_open_tiles(&mut self, tiles: &[String]) {
        let Some(cx) = self.cx() else { return };
        let apps: Vec<PApp> = tiles
            .iter()
            .filter_map(|t| PApp::ALL.into_iter().find(|a| a.id() == t))
            .filter(|a| *a != PApp::Inbox && cx.session.policy.allows_app(*a) && !self.cloud.hides_app(*a))
            .take(4)
            .collect();
        let Some(first) = apps.first().copied() else { return };
        self.open(first);
        if apps.len() < 2 || !self.panes_switch_on() {
            return;
        }
        let layout = match apps.len() {
            2 => "cols-50-50",
            3 => "thirds",
            _ => "grid",
        };
        if self.ws_apply(layout, Some(0), true).is_ok() {
            for (i, a) in apps.iter().enumerate().skip(1) {
                self.ws_focus(i);
                self.open(*a);
            }
            self.ws_focus(0);
        }
    }

    /// The screen was unlocked: go on to the view that asked for the password, or back to the first view when the person chose that.
    pub(super) fn views_after_unlock(&mut self) {
        if let Some(id) = self.views.pending.take() {
            self.views_switch(id);
        } else if self.views.model.reset_on_lock {
            let first = self.views.model.views[0].id;
            self.views_switch(first);
        }
    }

    /// The screen saver (or the black screen) ended without a lock: back to the first view when the person chose that. A locked screen
    /// is left alone here: its unlock does this ([`Shell::views_after_unlock`]).
    pub(super) fn views_after_saver(&mut self) {
        if self.session.is_some() && !self.is_locked() && self.views.model.reset_on_lock && self.views.pending.is_none() {
            let first = self.views.model.views[0].id;
            if self.views.cur != first {
                self.views_switch(first);
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    /// A small strip at the bottom right with one chip per view (only when there is more than one). A click on a chip switches.
    pub(super) fn draw_views_strip(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if self.session.is_none() || self.views.model.views.len() < 2 || !matches!(self.screen, Screen::Launcher | Screen::App(..)) {
            return;
        }
        let size = 14.0 * s;
        let chip_h = 30.0 * s;
        let pad = 12.0 * s;
        let gap = 6.0 * s;
        let bar_pad = 6.0 * s;
        let items: Vec<(String, bool, bool)> = self.views.model.views.iter().enumerate().map(|(i, v)| (format!("{} {}{}", i + 1, v.name, if v.locked { " (locked)" } else { "" }), v.id == self.views.cur, v.locked)).collect();
        let widths: Vec<f32> = items.iter().map(|(t, ..)| g.text_width(size, t, Face::Sans) + 2.0 * pad).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (items.len() as f32 - 1.0);
        let max_w = w as f32 - 24.0 * s - 2.0 * bar_pad;
        if total > max_w {
            return; // no room: the keys and Settings still work
        }
        // a visible bar behind the chips (light edge, darker than the wall) so the strip is never lost on a dark picture
        let bar = R::new(w as f32 - 12.0 * s - total - 2.0 * bar_pad, h as f32 - 12.0 * s - chip_h - 2.0 * bar_pad, total + 2.0 * bar_pad, chip_h + 2.0 * bar_pad);
        g.rrect(bar.x - s, bar.y - s, bar.w + 2.0 * s, bar.h + 2.0 * s, 13.0 * s, rgb(86, 104, 170));
        g.rrect(bar.x, bar.y, bar.w, bar.h, 12.0 * s, rgb(12, 15, 28));
        let mut x = bar.x + bar_pad;
        let y = bar.y + bar_pad;
        for (i, ((text, on, _), cw)) in items.iter().zip(widths.iter()).enumerate() {
            let r = R::new(x, y, *cw, chip_h);
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, if *on { rgb(64, 92, 176) } else if hot { rgb(54, 66, 110) } else { rgb(34, 42, 72) });
            g.text(r.x + pad, r.y + (chip_h - size) / 2.0 - s, size, if *on || hot { TEXT } else { rgb(196, 204, 226) }, text, if *on { Face::Bold } else { Face::Sans });
            g.hit(r, Event::Tab(TAB + i), &format!("view {text}"));
            x += cw + gap;
        }
    }
}
