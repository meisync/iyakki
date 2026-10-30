//! The shell's side of panes: several apps side by side on one display (`docs/MULTI-DISPLAY.md`).
//!
//! The focused pane's content is the shell's own `screen`, so every path that works on "the open app" (F-keys, help, the context
//! menu, the inbox stash, Back, jump-to-app) keeps working on the focused pane. The other panes keep their content in the
//! workspace and swap with the focused one when focus changes. Each pane draws into its own surface only when it is dirty, and the
//! surfaces are copied into the frame; click regions are translated from the pane's surface to the display.

use crate::safeclamp::SafeClamp;
use super::*;
use crate::apps::card::{Action, Card, Tone};
use crate::memgate::{self, Allowed, AppClass, DisplayMode, MemFacts, Switch};
use crate::panes::{Layout, LayoutKey, Snap, TEMPLATES};
use crate::ctxmenu::FieldNote;

/// Click regions of this file: `Event::Tab(BASE + pane * 8 + button)`.
const BASE: usize = 0x5C00;
const K_CLOSE: usize = 0;
const K_BACK: usize = 1;
const K_LAYOUT: usize = 2;
const K_MOVE: usize = 3;
const K_PUT: usize = 4;
/// The picker's template cells: `PICK + index in TEMPLATES`.
const PICK: usize = 0x5D00;
const PICK_SWITCH: usize = 0x5DF0;
/// The LAYOUT button in the top bars.
pub(super) const OPEN: usize = 0x5DF1;
const PICK_CLOSE: usize = 0x5DF2;
const BAR_SIGNOUT: usize = 0x5DF3;
const BAR_SINGLE: usize = 0x5DF4;

/// Heights, before scaling.
const BAR_H: f32 = 38.0;
const STRIP_H: f32 = 30.0;
const GAP: f32 = 6.0;
const GRAB: f32 = 14.0;
/// How much of the launcher's own top (title and buttons) is cut off in a pane, before scaling.
const CROP: f32 = 76.0;
/// The smallest pane the picker offers a template for (pixels at the display's size).
const MIN_PANE: (f32, f32) = (300.0, 200.0);
/// Ticks between two looks at free memory (10 seconds).
const GATE_EVERY: u64 = 300;

thread_local! {
    static SLOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the single-screen top bars have a LAYOUT button (the clock makes room for it).
pub(super) fn layout_slot() -> bool {
    SLOT.with(|c| c.get())
}

/// The LAYOUT button of the single-screen top bar: left of the bell.
pub(super) fn layout_btn_rect(w: u32, s: f32) -> R {
    let b = bell_rect(w, s);
    R::new(b.x - 40.0 * s, b.y, b.w, b.h)
}

pub(super) struct Pane {
    /// The pane's app (never the focused pane's: that one is the shell's `screen`).
    content: Option<(Box<dyn App>, PApp)>,
    custom: Option<String>,
    gfx: Option<Gfx>,
    /// Click regions in the pane's surface space.
    hits: Vec<Hit>,
    probes: Vec<R>,
    fields: Vec<FieldNote>,
    dirty: bool,
    /// Where the content's (0, 0) is in the pane's surface (the launcher's cut-off top moves it up).
    shift: (f32, f32),
    /// The size the content believes the screen is.
    content_size: (u32, u32),
}

impl Pane {
    fn new() -> Pane {
        Pane { content: None, custom: None, gfx: None, hits: Vec::new(), probes: Vec::new(), fields: Vec::new(), dirty: true, shift: (0.0, 0.0), content_size: (1, 1) }
    }
}

pub(super) struct Workspace {
    layout: Layout,
    panes: Vec<Pane>,
    focus: usize,
    /// Where the last frame put each pane on the display.
    rects: Vec<R>,
    /// The divider being dragged (its split number).
    drag: Option<usize>,
    scratch: Option<Gfx>,
    /// How many pane surfaces were drawn so far (a test reads it).
    redraws: u64,
    /// The display's size at the last frame.
    disp: (u32, u32),
}

#[derive(Default)]
pub(super) struct PanesUi {
    pub(super) ws: Option<Workspace>,
    /// The layout picker is open on this template.
    pub(super) picker: Option<usize>,
    pub(super) card: Option<Card>,
    /// What free memory is, when a test (or a tool) says so; the real machine's otherwise.
    pub(super) mem: Option<MemFacts>,
    allowed: Option<Allowed>,
    switch: Switch,
    checked: u64,
    /// The template the person wanted before memory made the layout smaller.
    wish: Option<String>,
    /// The launcher is being drawn in a pane (no big top bar).
    pub(super) compact: bool,
    /// Several displays (`shell_displays.rs`).
    d: disp::DispState,
}

fn side_of_key(n: u8) -> Option<&'static str> {
    crate::panes::template_for_key(n).map(|t| t.id)
}

impl Shell {
    // ------------------------------------------------------------------ the gate

    pub(super) fn panes_mem(&self) -> MemFacts {
        if cfg!(test) {
            self.panes.mem.unwrap_or_default()
        } else {
            self.panes.mem.unwrap_or_else(memgate::read_mem_now)
        }
    }

    /// The classes of the apps in the panes (the first pane's app first), padded for the panes a layout would add.
    pub(super) fn pane_classes(&self) -> Vec<AppClass> {
        let mut v = Vec::new();
        let one = |c: &Option<(Box<dyn App>, PApp)>| c.as_ref().map_or(AppClass::Light, |(_, p)| memgate::class_of(*p));
        match &self.panes.ws {
            Some(ws) => {
                for (i, p) in ws.panes.iter().enumerate() {
                    if i == ws.focus {
                        v.push(if let Screen::App(_, pa) = &self.screen { memgate::class_of(*pa) } else { AppClass::Light });
                    } else {
                        v.push(one(&p.content));
                    }
                }
            }
            None => v.push(if let Screen::App(_, pa) = &self.screen { memgate::class_of(*pa) } else { AppClass::Light }),
        }
        while v.len() < memgate::MAX_PANES {
            v.push(AppClass::Medium);
        }
        v
    }

    /// Look at memory again and decide what panes may do now; downgrade (with a card) or restore the person's layout.
    pub(super) fn refresh_gate(&mut self) {
        if self.session.is_none() {
            self.panes.allowed = None;
            return;
        }
        self.panes.switch = memgate::read_switch(self.env.services.dir());
        let (w, h) = if let Some(ws) = &self.panes.ws { ws.disp } else { self.size };
        let mem = self.panes_mem();
        let a = memgate::features_with(self.panes.switch, mem, &[DisplayMode { w, h }], &self.pane_classes());
        self.panes.allowed = Some(a.clone());
        SLOT.with(|c| c.set(a.panes()));
        self.panes.checked = self.tick;
        let now = self.panes.ws.as_ref().map_or(1, |ws| ws.layout.leaf_count());
        if now > a.max_panes {
            // not enough memory for this layout: keep the person's wish, show a smaller one, say so
            let to = smaller_for(a.max_panes);
            let wish = self.panes.ws.as_ref().map(|ws| ws.layout.id.clone());
            let name = crate::panes::template(to).map_or(to, |t| t.name);
            if self.panes.wish.is_none() {
                self.panes.wish = wish;
            }
            let _ = self.ws_apply(to, None, true);
            self.panes.card = Some(
                Card::new("Not enough free memory for this layout", rgb(255, 196, 94))
                    .line(&format!("Switching to {name}."), Tone::Bold)
                    .line(&a.reason, Tone::Dim)
                    .line("Your layout comes back when memory is free again.", Tone::Normal)
                    .action(Action::new("OK", Key::Enter, crate::fx::CYAN)),
            );
        } else if let Some(wish) = self.panes.wish.clone() {
            let want = Layout::from_template(&wish).leaf_count();
            let room = a.max_panes >= want && self.layout_fits(&wish);
            if room {
                self.panes.wish = None;
                if self.ws_apply(&wish, None, true).is_ok() {
                    self.say_info("Memory is free again: your layout is back");
                }
            }
        }
    }

    fn layout_fits(&self, id: &str) -> bool {
        let (w, h) = self.panes.ws.as_ref().map_or(self.size, |ws| ws.disp);
        let (area, gap) = ws_area(w, h);
        Layout::from_template(id).fits(area, gap, MIN_PANE)
    }

    /// Whether there is memory to start `app` in a pane now.
    pub(super) fn panes_room_for(&self, app: PApp) -> bool {
        if self.panes.switch != Switch::Auto {
            return true;
        }
        memgate::room_for_app(self.panes_mem(), memgate::class_of(app))
    }

    /// Whether the top bars show the LAYOUT button now.
    pub(super) fn panes_button(&self) -> bool {
        self.session.is_some() && self.panes.allowed.as_ref().is_some_and(|a| a.panes())
    }

    /// The Memory line of the picker and of Settings -> Displays.
    pub fn panes_memory_line(&self) -> String {
        let mem = self.panes_mem();
        let a = self.panes.allowed.clone().unwrap_or_else(|| memgate::features_with(self.panes.switch, mem, &[DisplayMode { w: self.size.0, h: self.size.1 }], &[]));
        memgate::memory_line(&a, mem)
    }

    // ------------------------------------------------------------------ the workspace

    pub(super) fn ws_active(&self) -> bool {
        self.panes.ws.is_some() && matches!(self.screen, Screen::Launcher | Screen::App(..))
    }

    fn ws_take_screen(&mut self) -> (Option<(Box<dyn App>, PApp)>, Option<String>) {
        let custom = self.custom_open.take();
        match std::mem::replace(&mut self.screen, Screen::Launcher) {
            Screen::App(a, p) => (Some((a, p)), custom),
            _ => (None, custom),
        }
    }

    fn ws_put_screen(&mut self, c: (Option<(Box<dyn App>, PApp)>, Option<String>)) {
        self.screen = match c.0 {
            Some((a, p)) => Screen::App(a, p),
            None => Screen::Launcher,
        };
        self.custom_open = c.1;
    }

    /// Give focus to pane `i`: its content becomes the shell's `screen`, the old one goes back to its pane.
    pub(super) fn ws_focus(&mut self, i: usize) {
        let Some(mut ws) = self.panes.ws.take() else { return };
        if i < ws.panes.len() && i != ws.focus {
            let out = self.ws_take_screen();
            let old = ws.focus;
            ws.panes[old].content = out.0;
            ws.panes[old].custom = out.1;
            ws.panes[old].dirty = true;
            let inn = (ws.panes[i].content.take(), ws.panes[i].custom.take());
            self.ws_put_screen(inn);
            ws.panes[i].dirty = true;
            ws.focus = i;
            self.ctx = None;
        }
        self.panes.ws = Some(ws);
        self.stale = true;
    }

    fn ws_mark_all_dirty(&mut self) {
        if let Some(ws) = self.panes.ws.as_mut() {
            for p in ws.panes.iter_mut() {
                p.dirty = true;
            }
        }
    }

    pub(super) fn ws_mark_dirty(&mut self, i: usize) {
        if let Some(p) = self.panes.ws.as_mut().and_then(|ws| ws.panes.get_mut(i)) {
            p.dirty = true;
        }
    }

    /// Apply a template. `place_at`: with a single pane open, the pane the open content goes to. `quiet`: no notice.
    pub(super) fn ws_apply(&mut self, id: &str, place_at: Option<usize>, quiet: bool) -> Result<(), String> {
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher | Screen::App(..)) {
            return Err("Sign in first".into());
        }
        let Some(def) = crate::panes::template(id) else { return Err("No such layout".into()) };
        let layout = Layout::from_template(def.id);
        let n = layout.leaf_count();
        if self.panes.allowed.is_none() {
            self.refresh_gate();
        }
        let a = self.panes.allowed.clone().expect("gate");
        let cur = self.panes.ws.as_ref().map_or(1, |ws| ws.layout.leaf_count());
        if n > cur && n > a.max_panes {
            return Err(if a.reason.is_empty() { "Not enough memory for that layout".to_string() } else { format!("Not available: {}", a.reason) });
        }
        let (w, h) = self.panes.ws.as_ref().map_or(self.size, |ws| ws.disp);
        let (area, gap) = ws_area(w, h);
        if n > 1 && !layout.fits(area, gap, MIN_PANE) {
            return Err("This screen is too small for that layout".into());
        }
        // every pane's content in one list, in pane order
        let (old_focus, mut old): (usize, Vec<(Option<(Box<dyn App>, PApp)>, Option<String>)>) = match self.panes.ws.take() {
            Some(ws) => {
                let focus = ws.focus;
                let mine = self.ws_take_screen();
                let mut v: Vec<_> = ws.panes.into_iter().map(|p| (p.content, p.custom)).collect();
                v[focus] = mine;
                (focus, v)
            }
            None => (0, vec![self.ws_take_screen()]),
        };
        let old_n = old.len();
        let mut new: Vec<(Option<(Box<dyn App>, PApp)>, Option<String>)> = Vec::new();
        let mut closed: Vec<String> = Vec::new();
        let mut focus = old_focus.min(n - 1);
        if old_n == 1 {
            let at = place_at.unwrap_or(0).min(n - 1);
            for i in 0..n {
                new.push(if i == at { old.pop().unwrap_or((None, None)) } else { (None, None) });
            }
            focus = at;
        } else if n >= old_n {
            focus = old_focus;
            for (i, c) in old.drain(..).enumerate() {
                let _ = i;
                new.push(c);
            }
            while new.len() < n {
                new.push((None, None));
            }
        } else {
            // fewer panes: the first ones stay in order; the focused one always stays
            if old_focus >= n {
                old.swap(old_focus, n - 1);
                focus = n - 1;
            }
            for (i, c) in old.into_iter().enumerate() {
                if i < n {
                    new.push(c);
                } else if let Some((app, _)) = c.0 {
                    closed.push(app.title().to_string());
                }
            }
            // the focused content moved to the last slot if it came from beyond; its place there is `focus`
        }
        if n == 1 && !self.panes.d.multi {
            let c = new.pop().unwrap_or((None, None));
            self.panes.ws = None;
            self.ws_put_screen(c);
        } else {
            let mut ws = Workspace { layout, panes: Vec::new(), focus, rects: Vec::new(), drag: None, scratch: None, redraws: 0, disp: (w, h) };
            for c in new {
                let mut p = Pane::new();
                p.content = c.0;
                p.custom = c.1;
                ws.panes.push(p);
            }
            let inn = (ws.panes[focus].content.take(), ws.panes[focus].custom.take());
            self.ws_put_screen(inn);
            self.panes.ws = Some(ws);
        }
        self.ctx = None;
        self.stale = true;
        if !closed.is_empty() && !quiet {
            self.say_info(format!("Closed for the smaller layout: {}", closed.join(", ")));
        } else if !quiet {
            self.say_info(format!("Layout: {}", def.name));
        }
        self.refresh_gate_light();
        Ok(())
    }

    /// The numbers of the gate after the layout changed (no downgrade logic).
    fn refresh_gate_light(&mut self) {
        let (w, h) = self.panes.ws.as_ref().map_or(self.size, |ws| ws.disp);
        let mem = self.panes_mem();
        self.panes.allowed = Some(memgate::features_with(self.panes.switch, mem, &[DisplayMode { w, h }], &self.pane_classes()));
    }

    /// Swap the contents of two panes; focus goes with the focused content.
    fn ws_swap(&mut self, a: usize, b: usize) {
        let Some(mut ws) = self.panes.ws.take() else { return };
        if a == b || a >= ws.panes.len() || b >= ws.panes.len() {
            self.panes.ws = Some(ws);
            return;
        }
        // everything in the panes, then swap, then bring the focused one back
        let mine = self.ws_take_screen();
        let f = ws.focus;
        ws.panes[f].content = mine.0;
        ws.panes[f].custom = mine.1;
        let (ca, cb) = (ws.panes[a].content.take(), ws.panes[b].content.take());
        ws.panes[a].content = cb;
        ws.panes[b].content = ca;
        let (xa, xb) = (ws.panes[a].custom.take(), ws.panes[b].custom.take());
        ws.panes[a].custom = xb;
        ws.panes[b].custom = xa;
        ws.panes[a].dirty = true;
        ws.panes[b].dirty = true;
        // the focus follows the content it had
        let nf = if f == a {
            b
        } else if f == b {
            a
        } else {
            f
        };
        ws.focus = nf;
        let inn = (ws.panes[nf].content.take(), ws.panes[nf].custom.take());
        self.ws_put_screen(inn);
        ws.panes[nf].dirty = true;
        self.panes.ws = Some(ws);
        self.ctx = None;
        self.stale = true;
    }

    fn ws_close(&mut self, i: usize) {
        let Some(ws) = self.panes.ws.as_ref() else { return };
        let n = ws.layout.leaf_count();
        if n <= 1 {
            return;
        }
        let to = ws.layout.after_close();
        // the closing pane's content goes to the last place and focus moves to a pane that stays; fewer panes drops the last
        if i != n - 1 {
            self.ws_swap(i, n - 1);
        }
        if self.panes.ws.as_ref().is_some_and(|ws| ws.focus == n - 1) {
            self.ws_focus(n - 2);
        }
        let _ = self.ws_apply(to, None, true);
        self.panes.wish = None;
    }

    // ------------------------------------------------------------------ keys and clicks

    pub(super) fn panes_key(&mut self, k: LayoutKey) {
        let sw = self.panes.switch;
        if matches!(k, LayoutKey::Picker) {
            self.open_picker();
            return;
        }
        if sw == Switch::Off {
            self.say("Screen features are off (an administrator can switch them on from the layout picker, Ctrl+Alt+0)");
            return;
        }
        if self.panes.allowed.is_none() {
            self.refresh_gate();
        }
        match k {
            LayoutKey::Apply(n) => match side_of_key(n) {
                Some(id) => self.apply_by_person(id, None),
                None => {}
            },
            LayoutKey::Snap(side) => {
                let (w, h) = self.panes.ws.as_ref().map_or(self.size, |ws| ws.disp);
                let (area, gap) = ws_area(w, h);
                let (layout, focus) = match &self.panes.ws {
                    Some(ws) => (ws.layout.clone(), ws.focus),
                    None => (Layout::single(), 0),
                };
                // with several displays, an arrow toward a display goes there when this display has no pane that way
                if self.panes.d.multi && (layout.leaf_count() == 1 || layout.neighbour(area, gap, focus, side).is_none()) {
                    if let Some(k) = self.display_on_side(side) {
                        self.move_focused_to_display(k);
                        return;
                    }
                }
                match layout.snap(area, gap, focus, side) {
                    Snap::Make { template, focus_after } => self.apply_by_person(template, Some(focus_after)),
                    Snap::Swap(n) => self.ws_swap(focus, n),
                    Snap::Nothing => self.say_info("No pane on that side"),
                }
            }
            LayoutKey::Nudge(side) => {
                let (w, h) = self.panes.ws.as_ref().map_or(self.size, |ws| ws.disp);
                let (area, gap) = ws_area(w, h);
                let mins = self.pane_mins(w, h);
                if let Some(ws) = self.panes.ws.as_mut() {
                    if ws.layout.nudge(area, gap, ws.focus, side, 0.05, &mins) {
                        self.ws_mark_all_dirty();
                        self.stale = true;
                    }
                }
            }
            LayoutKey::Cycle { back } => {
                if let Some(ws) = &self.panes.ws {
                    let to = ws.layout.next_focus(ws.focus, back);
                    self.ws_focus(to);
                }
            }
            LayoutKey::NextDisplay => {
                let n = self.panes.d.list.len();
                if self.panes.d.multi && n > 1 {
                    let to = (self.panes.d.active + 1) % n;
                    self.move_focused_to_display(to);
                } else {
                    self.say_info("Only one display is connected");
                }
            }
            LayoutKey::Picker => {}
        }
    }

    fn apply_by_person(&mut self, id: &str, place_at: Option<usize>) {
        if self.panes.card.is_some() {
            return;
        }
        match self.ws_apply(id, place_at, false) {
            Ok(()) => {
                self.panes.wish = None;
                self.save_display_templates();
            }
            Err(e) => self.say(e),
        }
    }

    /// Each pane's minimum size on this display, in pixels (by the class of the app in it).
    fn pane_mins(&self, w: u32, h: u32) -> Vec<(f32, f32)> {
        let s = scale_for(w, h);
        let classes = self.pane_classes();
        let n = self.panes.ws.as_ref().map_or(1, |ws| ws.panes.len());
        (0..n).map(|i| {
            let m = classes.get(i).copied().unwrap_or(AppClass::Light).min_size();
            // a divider may always go as far as the picker's smallest pane; the app's own minimum is checked when drawing
            (m.0.min(MIN_PANE.0 * 1.2) * s.min(1.0), m.1.min(MIN_PANE.1 * 1.2) * s.min(1.0))
        }).collect()
    }

    fn open_picker(&mut self) {
        if self.session.is_none() || !matches!(self.screen, Screen::Launcher | Screen::App(..)) {
            return;
        }
        self.refresh_gate();
        let cur = self.panes.ws.as_ref().map_or("single", |ws| ws.layout.id.as_str()).to_string();
        let idx = TEMPLATES.iter().position(|t| t.id == cur).unwrap_or(0);
        self.panes.picker = Some(idx);
        self.stale = true;
    }

    fn picker_choose(&mut self, idx: usize) {
        let Some(t) = TEMPLATES.get(idx) else { return };
        self.panes.picker = None;
        if self.panes.switch == Switch::Off {
            self.say("Screen features are off");
            return;
        }
        self.apply_by_person(t.id, None);
    }

    /// Why a template cannot be used now (None when it can).
    fn template_blocked(&self, idx: usize) -> Option<String> {
        let t = TEMPLATES.get(idx)?;
        let l = Layout::from_template(t.id);
        let n = l.leaf_count();
        let a = self.panes.allowed.as_ref()?;
        if n > a.max_panes {
            return Some(if a.reason.is_empty() { "needs more memory".into() } else { a.reason.clone() });
        }
        if !self.layout_fits(t.id) {
            return Some("too small for this screen".into());
        }
        None
    }

    pub(super) fn panes_picker_key(&mut self, ev: &Event) {
        let Some(sel) = self.panes.picker else { return };
        let cols = 5usize;
        let n = TEMPLATES.len();
        match ev {
            Event::Key(Key::Esc) => self.panes.picker = None,
            Event::Key(Key::Enter) => self.picker_choose(sel),
            Event::Key(Key::Left) => self.panes.picker = Some((sel + n - 1) % n),
            Event::Key(Key::Right) => self.panes.picker = Some((sel + 1) % n),
            Event::Key(Key::Up) => self.panes.picker = Some(if sel >= cols { sel - cols } else { sel }),
            Event::Key(Key::Down) => self.panes.picker = Some(if sel + cols < n { sel + cols } else { sel }),
            Event::Key(Key::Char(c)) if c.is_ascii_digit() && *c != '0' => {
                if let Some(i) = TEMPLATES.iter().position(|t| t.key == Some(*c as u8 - b'0')) {
                    self.picker_choose(i);
                }
            }
            Event::Key(Key::Char('s')) => self.picker_switch(),
            Event::Tab(PICK_SWITCH) => self.picker_switch(),
            Event::Tab(PICK_CLOSE) => self.panes.picker = None,
            Event::Tab(t) if (PICK..PICK + n).contains(t) => self.picker_choose(t - PICK),
            _ => {}
        }
        self.stale = true;
    }

    fn picker_switch(&mut self) {
        if !self.session.as_ref().is_some_and(|s| s.policy.manage_settings) {
            self.say("Only an administrator can change this");
            return;
        }
        let next = self.panes.switch.next();
        let _ = memgate::write_switch(self.env.services.dir(), next);
        self.panes.switch = next;
        self.refresh_gate();
    }

    pub(super) fn panes_owns_tab(&self, n: usize) -> bool {
        self.session.is_some() && ((BASE..BASE + 8 * memgate::MAX_PANES).contains(&n) || (PICK..PICK + TEMPLATES.len()).contains(&n) || [PICK_SWITCH, OPEN, PICK_CLOSE, BAR_SIGNOUT, BAR_SINGLE].contains(&n))
    }

    /// The memory card takes keys and clicks until it is answered.
    pub(super) fn panes_card_event(&mut self, ev: &Event) {
        if let Some(c) = self.panes.card.as_mut() {
            if c.handle(ev).is_some() || matches!(ev, Event::Key(Key::Esc)) {
                self.panes.card = None;
            }
        }
        self.stale = true;
    }

    pub(super) fn panes_tab(&mut self, n: usize) {
        match n {
            OPEN => self.open_picker(),
            BAR_SIGNOUT => self.sign_out(),
            BAR_SINGLE => self.apply_by_person("single", None),
            PICK_SWITCH | PICK_CLOSE => self.panes_picker_key(&Event::Tab(n)),
            n if (PICK..PICK + TEMPLATES.len()).contains(&n) => self.panes_picker_key(&Event::Tab(n)),
            n => {
                let (i, k) = ((n - BASE) / 8, (n - BASE) % 8);
                let Some(count) = self.panes.ws.as_ref().map(|ws| ws.panes.len()) else { return };
                if i >= count {
                    return;
                }
                match k {
                    K_CLOSE => self.ws_close(i),
                    K_BACK => {
                        if self.panes.ws.as_ref().is_some_and(|ws| ws.focus != i) {
                            self.ws_focus(i);
                        }
                        self.leave_app();
                        self.ws_mark_dirty(i);
                    }
                    K_LAYOUT => {
                        if self.panes.ws.as_ref().is_some_and(|ws| ws.focus != i) {
                            self.ws_focus(i);
                        }
                        self.open_picker();
                    }
                    K_MOVE => {
                        self.panes.d.move_src = Some((self.panes.d.active, i));
                        self.say_info(if self.panes.d.multi { "Click the pane to put it in, on any display (Esc cancels)" } else { "Click the pane to put it in (Esc cancels)" });
                    }
                    K_PUT => {
                        if let Some(f) = self.panes.d.move_src.take() {
                            let to = (self.panes.d.active, i);
                            self.move_content(f, to);
                        }
                    }
                    _ => {}
                }
                self.ws_mark_all_dirty_if_needed();
            }
        }
        self.stale = true;
    }

    fn ws_mark_all_dirty_if_needed(&mut self) {
        // the strips show which pane is focused: both panes involved were marked when focus changed
    }

    /// Mouse and wheel events that belong to the workspace itself. Returns true when the event is used up.
    pub(super) fn panes_intercept(&mut self, ev: &Event) -> bool {
        if !self.ws_active() {
            return false;
        }
        let overlay = self.overlay_open();
        let (w, h) = self.size;
        let (area, gap) = ws_area(w, h);
        let s = scale_for(w, h);
        match ev {
            Event::Key(Key::Esc) if self.panes.d.move_src.is_some() && !overlay => {
                self.panes.d.move_src = None;
                true
            }
            // the pointer is on another display: nothing under it moves or highlights on this one
            Event::MouseMove(..) if self.panes.d.multi && self.panes.d.pointer != self.panes.d.active => true,
            Event::MouseDown(x, y) if !overlay && self.panes.d.move_src.is_some() => {
                // pick up, then click the target (any pane of any display)
                let (pd, from) = (self.panes.d.pointer, self.panes.d.move_src.take());
                if let (Some(from), Some(j)) = (from, self.pane_under(pd, *x, *y)) {
                    if from != (pd, j) {
                        self.move_content(from, (pd, j));
                    }
                }
                self.ws_mark_all_dirty();
                true
            }
            Event::MouseDown(..) if !overlay && self.panes.d.multi && self.panes.d.pointer != self.panes.d.active => {
                // a click on another display makes it the working display, and goes on to what is under the pointer
                let k = self.panes.d.pointer;
                self.focus_display(k);
                self.stale = true;
                // the display is the working one now: the press is handled like any press on it (a divider grab, and the pane
                // under the pointer takes the focus), otherwise the click would reach the pane that had the focus last time
                // on that display, at coordinates that are not its own
                if self.panes.d.active == k {
                    return self.panes_intercept(ev);
                }
                false
            }
            Event::Scroll(_) if !overlay && self.panes.d.multi && self.panes.d.pointer != self.panes.d.active => {
                // the wheel goes to the pane under the pointer on the other display, without changing the working display
                let (old, k) = (self.panes.d.active, self.panes.d.pointer);
                self.focus_display(k);
                let used = self.panes_intercept(ev);
                if !used {
                    self.route(ev);
                }
                self.focus_display(old);
                true
            }
            Event::MouseMove(x, y) => {
                let dragging = self.panes.ws.as_ref().and_then(|ws| ws.drag);
                if let Some(split) = dragging {
                    let mins = self.pane_mins(w, h);
                    if let Some(ws) = self.panes.ws.as_mut() {
                        let dir = split_dir(&ws.layout, split);
                        let pos = if dir == crate::panes::Dir::Side { *x as f32 } else { *y as f32 };
                        ws.layout.drag(area, gap * 1.0, split, pos, &mins);
                    }
                    self.ws_mark_all_dirty();
                    return true;
                }
                false
            }
            Event::MouseDown(x, y) if !overlay => {
                let (x, y) = (*x, *y);
                let hit_div = self.panes.ws.as_ref().and_then(|ws| ws.layout.divider_at(area, gap, GRAB * s, x, y));
                if let (Some(d), true) = (hit_div, self.panes.d.move_src.is_none()) {
                    if let Some(ws) = self.panes.ws.as_mut() {
                        ws.drag = Some(d.split);
                    }
                    return true;
                }
                let under = self.panes.ws.as_ref().and_then(|ws| ws.rects.iter().position(|r| r.contains(x, y)));
                if let Some(i) = under {
                    if self.panes.ws.as_ref().is_some_and(|ws| ws.focus != i) {
                        self.ws_focus(i);
                    }
                }
                false
            }
            Event::MouseUp(..) => self.panes.ws.as_mut().and_then(|ws| ws.drag.take()).is_some(),
            Event::Scroll(_) if !overlay => {
                // the wheel goes to the pane under the pointer
                let (mx, my) = self.mouse;
                let under = self.panes.ws.as_ref().and_then(|ws| ws.rects.iter().position(|r| r.contains(mx, my)));
                let focus = self.panes.ws.as_ref().map_or(0, |ws| ws.focus);
                match under {
                    Some(i) if i != focus => {
                        self.ws_focus(i);
                        self.route(ev);
                        self.ws_focus(focus);
                        self.ws_mark_dirty(i);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub(super) fn overlay_open(&self) -> bool {
        self.ctx.is_some()
            || self.clip.is_some()
            || self.hostdlg.is_some()
            || self.login.as_ref().is_some_and(|m| !m.hidden())
            || !matches!(self.shot, Shot::Off)
            || self.help
            || self.devpop.open
            || self.brand.open
            || self.installs.open
            || self.startup.open
            || self.battery.open
            || self.panes.picker.is_some()
            || self.panes.card.is_some()
            || self.ring.is_some()
    }

    /// The route wrapper: coordinates and size are those of the focused pane's content.
    pub(super) fn route(&mut self, ev: &Event) {
        if !self.ws_active() {
            return self.route_inner(ev);
        }
        // Esc in a pane's launcher leaves the pane alone: signing out is `o`, the button in the bar or Ctrl+Alt+... nothing by accident
        if matches!(self.screen, Screen::Launcher) && matches!(ev, Event::Key(Key::Esc)) {
            return;
        }
        let (origin, size) = match self.panes.ws.as_ref() {
            Some(ws) => {
                let r = ws.rects.get(ws.focus).copied().unwrap_or(R::new(0.0, 0.0, 0.0, 0.0));
                let p = &ws.panes[ws.focus];
                ((r.x + p.shift.0, r.y + p.shift.1), p.content_size)
            }
            None => return self.route_inner(ev),
        };
        let at = |x: i32, y: i32| ((x as f32 - origin.0).round() as i32, (y as f32 - origin.1).round() as i32);
        let local = match ev {
            Event::MouseMove(x, y) => {
                let (a, b) = at(*x, *y);
                Event::MouseMove(a, b)
            }
            Event::MouseDown(x, y) => {
                let (a, b) = at(*x, *y);
                Event::MouseDown(a, b)
            }
            Event::MouseUp(x, y) => {
                let (a, b) = at(*x, *y);
                Event::MouseUp(a, b)
            }
            Event::MouseRight(x, y) => {
                let (a, b) = at(*x, *y);
                Event::MouseRight(a, b)
            }
            other => other.clone(),
        };
        let saved = self.size;
        let mouse = self.mouse;
        self.size = size;
        self.mouse = at(mouse.0, mouse.1);
        self.route_inner(&local);
        self.size = saved;
        self.mouse = mouse;
        let f = self.panes.ws.as_ref().map_or(0, |ws| ws.focus);
        if !matches!(ev, Event::MouseMove(..) | Event::Tick) {
            self.ws_mark_dirty(f);
        }
    }

    // ------------------------------------------------------------------ heartbeat and dirty tracking

    /// Called with every heartbeat: the apps in the other panes get theirs, and say whether they want a new picture.
    pub(super) fn panes_tick(&mut self) {
        if self.session.is_none() {
            return;
        }
        if self.tick.saturating_sub(self.panes.checked) >= GATE_EVERY || (self.panes.allowed.is_none() && self.tick % 30 == 1) {
            self.refresh_gate();
        }
        let Some(cx) = self.cx() else { return };
        let Some(mut ws) = self.panes.ws.take() else { return };
        let tick = self.tick;
        let mut crashed = Vec::new();
        let focus = ws.focus;
        for (i, p) in ws.panes.iter_mut().enumerate() {
            if i == focus {
                continue;
            }
            let Some((app, pa)) = p.content.as_mut() else { continue };
            let pa = *pa;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                app.handle(&Event::Tick, &cx);
                app.wants_redraw()
            })) {
                Ok(want) => {
                    if want || self.bg_stale(pa, tick) {
                        p.dirty = true;
                    }
                }
                Err(_) => crashed.push((i, app.title())),
            }
        }
        for (i, name) in crashed {
            ws.panes[i].content = None;
            ws.panes[i].dirty = true;
            self.app_crashed(name);
        }
        self.panes.ws = Some(ws);
        self.tick_other_displays(&cx);
    }

    /// The same per-app reasons to draw again that the shell checks for the open app, for an app in another pane.
    fn bg_stale(&self, pa: PApp, tick: u64) -> bool {
        let svc = &self.env.services;
        match pa {
            PApp::Radio => svc.radio.is_active(),
            PApp::Cli => tick % 6 == 0 && (svc.clis.running() || svc.cluster.state() == iyakki_agent::cluster::ClusterState::Starting),
            PApp::Calendar => tick % 10 == 0,
            PApp::Media => tick % 10 == 0 || (tick % 3 == 0 && iyakki_agent::media::shared().now_playing().is_some()),
            _ => false,
        }
    }

    /// After an event: what the picture needs again.
    pub(super) fn panes_after(&mut self, ev: &Event, prev_mouse: (i32, i32)) {
        if self.panes.ws.is_none() {
            return;
        }
        if !self.stale {
            return;
        }
        match ev {
            Event::MouseMove(x, y) => {
                // the pointer crossed the edge of something: the panes it left and entered draw their hover state
                let (cur, prev): (Vec<usize>, Vec<usize>) = match self.panes.ws.as_ref() {
                    Some(ws) => (
                        ws.rects.iter().enumerate().filter(|(_, r)| r.contains(*x, *y)).map(|(i, _)| i).collect(),
                        ws.rects.iter().enumerate().filter(|(_, r)| r.contains(prev_mouse.0, prev_mouse.1)).map(|(i, _)| i).collect(),
                    ),
                    None => return,
                };
                for i in cur.into_iter().chain(prev) {
                    self.ws_mark_dirty(i);
                }
            }
            Event::Tick => {
                let f = self.panes.ws.as_ref().map_or(0, |ws| ws.focus);
                self.ws_mark_dirty(f);
            }
            _ => {
                let f = self.panes.ws.as_ref().map_or(0, |ws| ws.focus);
                self.ws_mark_dirty(f);
            }
        }
    }

    pub(super) fn panes_reset(&mut self) {
        // which displays there are does not depend on who is signed in
        let list = std::mem::take(&mut self.panes.d.list);
        self.panes = PanesUi::default();
        let n = list.len();
        self.panes.d.list = list;
        if n >= 2 {
            // the next person signed in gets a workspace on every display (made when the first frame is drawn)
            self.panes.d.multi = true;
            self.panes.d.others = (0..n).map(|_| None).collect();
            self.panes.d.other_hits = vec![Vec::new(); n];
        }
        SLOT.with(|c| c.set(false));
    }

    // ------------------------------------------------------------------ drawing

    /// Draw the bar, the panes, the dividers and the move targets in place of the single screen.
    pub(super) fn draw_panes(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let Some(mut ws) = self.panes.ws.take() else { return };
        let (area, gap) = ws_area(w, h);
        if ws.disp != (w, h) {
            ws.disp = (w, h);
            for p in ws.panes.iter_mut() {
                p.dirty = true;
            }
        }
        self.draw_bar(g, w, s);
        ws.rects = ws.layout.rects(area, gap);
        let cx = self.cx();
        let tick = self.tick;
        let mut all_fields: Vec<FieldNote> = Vec::new();
        let rects = ws.rects.clone();
        let mut crashed: Vec<usize> = Vec::new();
        for (i, r) in rects.iter().enumerate() {
            if self.render_one_pane(&mut ws, i, *r, s, tick, cx.as_ref(), g, &mut all_fields) {
                crashed.push(i);
            }
        }
        // dividers
        let dragging = ws.drag;
        for d in ws.layout.dividers(area, gap, GRAB * s) {
            let hot = dragging == Some(d.split) || g.hovered(d.rect);
            let c = if hot { crate::fx::CYAN } else { rgb(60, 72, 110) };
            let (cx0, cy0) = (d.rect.x + d.rect.w / 2.0, d.rect.y + d.rect.h / 2.0);
            match d.dir {
                crate::panes::Dir::Side => g.rrect(cx0 - 1.5 * s, d.rect.y + 6.0 * s, 3.0 * s, (d.rect.h - 12.0 * s).max(1.0), 1.5 * s, c),
                crate::panes::Dir::Stack => g.rrect(d.rect.x + 6.0 * s, cy0 - 1.5 * s, (d.rect.w - 12.0 * s).max(1.0), 3.0 * s, 1.5 * s, c),
            }
        }
        // the focus frame, and the move targets
        if let Some(r) = ws.rects.get(ws.focus) {
            g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 2.0 * s, crate::fx::CYAN);
        }
        if let Some(from) = self.panes.d.move_src {
            let me = self.panes.d.drawing;
            for (i, r) in ws.rects.iter().enumerate() {
                if from == (me, i) {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 3.0 * s, rgb(255, 196, 94));
                    continue;
                }
                g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, [20, 60, 70, 150]);
                let label = "Put here";
                let tw = g.text_width(20.0 * s, label, Face::Bold);
                g.text(r.x + (r.w - tw) / 2.0, r.y + r.h / 2.0 - 10.0 * s, 20.0 * s, TEXT, label, Face::Bold);
                g.hit(*r, Event::Tab(BASE + i * 8 + K_PUT), &format!("put here {}", i + 1));
            }
        }
        if !self.panes.d.inactive {
            crate::ctxmenu::start_frame();
            for f in all_fields {
                crate::ctxmenu::note_field(f);
            }
        }
        self.panes.ws = Some(ws);
        self.draw_bar_overlay(g, s);
        for i in crashed {
            if self.panes.ws.as_ref().is_some_and(|ws| ws.focus == i) {
                self.screen = Screen::Launcher;
            }
            self.ws_mark_dirty(i);
        }
    }

    fn draw_bar(&mut self, g: &mut Gfx, w: u32, s: f32) {
        let bh = BAR_H * s;
        g.rect(0.0, 0.0, w as f32, bh, [8, 11, 24, 235]);
        g.rect(0.0, bh - 1.0, w as f32, 1.0, [120, 150, 255, 30]);
        let name_w = g.text_width(18.0 * s, "IYAKKI", Face::Bold);
        let nb = R::new(10.0 * s, 4.0 * s, name_w + 20.0 * s, bh - 8.0 * s);
        if g.hovered(nb) {
            g.rrect(nb.x, nb.y, nb.w, nb.h, 8.0 * s, rgb(28, 36, 66));
        }
        g.text(20.0 * s, (bh - 18.0 * s) / 2.0 - s, 18.0 * s, TEXT, "IYAKKI", Face::Bold);
        g.hit(nb, Event::Key(Key::F(10)), "IYAKKI menu");
        // buttons from the right: sign out, single, layout, bell, devices, camera, clipboard, help
        let d = 28.0 * s;
        let y = (bh - d) / 2.0;
        let mut x = w as f32 - 10.0 * s - d;
        let btn = |g: &mut Gfx, x: f32, icon: Option<Icon>, label: &str, ev: Event, name: &str, wide: f32| -> f32 {
            let r = R::new(x - (wide - d), y, wide, d);
            let hot = g.hovered(r);
            crate::fx::chrome_button(g, r, s * 0.8, hot, crate::fx::CYAN);
            match icon {
                Some(ic) => g.icon(ic, r.x + r.w / 2.0, r.y + r.h / 2.0, 18.0 * s, MUTED),
                None => {
                    let tw = g.text_width(13.0 * s, label, Face::Bold);
                    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - 13.0 * s) / 2.0 - s * 0.5, 13.0 * s, TEXT, label, Face::Bold);
                }
            }
            g.hit(r, ev, name);
            r.x - 6.0 * s - d
        };
        x = btn(g, x, Some(Icon::Logout), "", Event::Tab(BAR_SIGNOUT), "sign out", d);
        let single_w = g.text_width(13.0 * s, "Single", Face::Bold) + 22.0 * s;
        x = btn(g, x, None, "Single", Event::Tab(BAR_SINGLE), "single layout", single_w);
        let lay_w = g.text_width(13.0 * s, "LAYOUT", Face::Bold) + 22.0 * s;
        x = btn(g, x, None, "LAYOUT", Event::Tab(OPEN), "layout", lay_w);
        if self.session.as_ref().is_some_and(|x| x.policy.allows_app(PApp::Inbox)) {
            let r = R::new(x, y, d, d);
            draw_bell(g, r, 8.0 * s, s, self.badge, false);
            x = r.x - 6.0 * s - d;
        }
        let help_w = g.text_width(13.0 * s, "F1  Help", Face::Bold) + 22.0 * s;
        // Home, the speaker and Settings (the same icons as on every other bar), as many as fit before the buttons still to come
        {
            let floor = 20.0 * s + name_w + 40.0 * s;
            let reserve = 3.0 * (d + 6.0 * s) + help_w + 6.0 * s;
            let home = self.panes.d.inactive || self.home_applies();
            let placed = self.place_extras(x + d, floor + reserve, floor + reserve, d, 6.0 * s, home);
            for (e, ex) in &placed {
                self.draw_extra(g, *e, R::new(*ex, y, d, d), s * 0.8, crate::fx::CYAN, 18.0 * s);
            }
            if let Some((_, lx)) = placed.first() {
                x = lx - 6.0 * s - d;
            }
        }
        x = btn(g, x, Some(Icon::Devices), "", Event::Key(Key::F(8)), "devices", d);
        x = btn(g, x, Some(Icon::Camera), "", Event::Key(Key::F(12)), "screenshot", d);
        x = btn(g, x, Some(Icon::Clipboard), "", Event::Key(Key::Ctrl('V')), "clipboard history", d);
        x = btn(g, x, None, "F1  Help", Event::Key(Key::F(1)), "help", help_w);
        // the clock, left of the buttons
        let (time, date) = (clock_time_text(&self.clock), self.clock.date_text());
        let tw = g.text_width(15.0 * s, &time, Face::Bold);
        let dw = g.text_width(13.0 * s, &date, Face::Sans);
        let total = tw + 10.0 * s + dw;
        let left_limit = 20.0 * s + name_w + 40.0 * s;
        let (show_date, wide) = if x - total - 8.0 * s > left_limit { (true, total) } else if x - tw - 8.0 * s > left_limit { (false, tw) } else { (false, 0.0) };
        // the battery chip (laptops only), left of the clock (or of the buttons when the clock has no room)
        let bat_right = if wide > 0.0 { x - wide - 8.0 * s - 6.0 * s } else { x - 4.0 * s };
        let mut chip_left = if wide > 0.0 { x - wide - 8.0 * s } else { x };
        match self.battery.plan(g, s, bat_right, left_limit, 4.0 * s, bh - 8.0 * s, 13.0 * s) {
            Some(p) => {
                chip_left = p.rect.x;
                self.battery.draw_chip(g, &p, s);
            }
            None => self.battery.anchor = None,
        }
        // developer mode's DEV sign: the same on every bar and every display
        if let Some(sign) = self.dev_sign.clone() {
            draw_dev_sign(g, chip_left - 4.0 * s, bh / 2.0, s, &sign, left_limit, self.devask.pulse(self.tick));
        }
        if wide > 0.0 {
            let b = R::new(x - wide - 8.0 * s, 4.0 * s, wide + 16.0 * s, bh - 8.0 * s);
            let hot = g.hovered(b);
            if hot {
                g.rrect(b.x, b.y, b.w, b.h, 8.0 * s, rgb(28, 36, 66));
            }
            let ty = b.y + (b.h - g.line_height(15.0 * s, Face::Bold)) / 2.0;
            g.text(b.x + 8.0 * s, ty, 15.0 * s, TEXT, &time, Face::Bold);
            if show_date {
                g.text(b.x + 8.0 * s + tw + 10.0 * s, ty + s, 13.0 * s, MUTED, &date, Face::Sans);
            }
            g.hit(b, Event::Key(Key::F(7)), "clock");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_one_pane(&mut self, ws: &mut Workspace, i: usize, r: R, s_disp: f32, tick: u64, cx: Option<&AppCx>, g: &mut Gfx, fields_out: &mut Vec<FieldNote>) -> bool {
        let (pw, ph) = ((r.w.round() as u32).max(1), (r.h.round() as u32).max(1));
        let focused = ws.focus == i && !self.panes.d.inactive;
        let mut crashed = false;
        let resized = ws.panes[i].gfx.as_ref().map_or(true, |p| (p.width(), p.height()) != (pw, ph));
        if resized {
            ws.panes[i].gfx = Some(Gfx::new(pw, ph));
            ws.panes[i].dirty = true;
        }
        let dm = self.disp_mouse(self.panes.d.drawing);
        let local_mouse = if r.contains(dm.0, dm.1) { (dm.0 - r.x as i32, dm.1 - r.y as i32) } else { (-1, -1) };
        if ws.panes[i].dirty {
            ws.panes[i].dirty = false;
            ws.redraws += 1;
            let mut pg = ws.panes[i].gfx.take().expect("pane surface");
            pg.hits.clear();
            pg.clear_probes();
            pg.mouse = local_mouse;
            crate::ctxmenu::start_frame();
            pg.gradient(BG_TOP, BG_BOTTOM);
            let ps = scale_for(pw, ph);
            let strip = (STRIP_H * ps).min(ph as f32 / 3.0);
            let mut shift = (0.0, 0.0);
            let mut content_size = (pw, ph);
            // what the pane holds
            let (title, icon, accent, is_app, class) = {
                let app = if focused {
                    match &self.screen {
                        Screen::App(a, p) => Some((a.title(), a.icon(), a.accent(), *p)),
                        _ => None,
                    }
                } else {
                    ws.panes[i].content.as_ref().map(|(a, p)| (a.title(), a.icon(), a.accent(), *p))
                };
                match app {
                    Some((t, ic, ac, p)) => (t, Some(ic), ac, true, memgate::class_of(p)),
                    None => ("Home", None, crate::fx::CYAN, false, AppClass::Light),
                }
            };
            let mut content_hits: Vec<Hit> = Vec::new();
            if is_app {
                let area = R::new(0.0, strip, pw as f32, ph as f32 - strip);
                let min = class.min_size();
                if (pw as f32) < min.0 * s_disp.min(1.0) || (area.h) < min.1 * s_disp.min(1.0) {
                    let msg = format!("{} needs at least {}x{}; this pane is {}x{}. Pick a bigger pane or a different layout.", title, (min.0 * s_disp.min(1.0)) as u32, (min.1 * s_disp.min(1.0)) as u32, pw, (area.h) as u32);
                    let lines = pg.wrap(14.0 * ps, &msg, Face::Sans, area.w - 24.0 * ps);
                    for (k, l) in lines.iter().enumerate() {
                        pg.text(area.x + 12.0 * ps, area.y + 14.0 * ps + k as f32 * 20.0 * ps, 14.0 * ps, MUTED, l, Face::Sans);
                    }
                } else if let Some(cx) = cx {
                    let app: Option<&mut Box<dyn App>> = if focused {
                        match &mut self.screen {
                            Screen::App(a, _) => Some(a),
                            _ => None,
                        }
                    } else {
                        ws.panes[i].content.as_mut().map(|(a, _)| a)
                    };
                    if let Some(app) = app {
                        let ok = crate::ui::with_input_focus(focused, || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.render(&mut pg, area, ps, cx, tick))).is_ok());
                        if !ok {
                            crashed = true;
                        }
                    }
                }
                content_hits = std::mem::take(&mut pg.hits);
            } else if cx.is_some() {
                // the launcher at the pane's size, without its own top bar
                let crop = (CROP * ps).min(ph as f32 / 3.0);
                let (sw, sh) = (pw, ((ph as f32 - strip + crop).round() as u32).max(1));
                let mut sc = ws.scratch.take().filter(|c| (c.width(), c.height()) == (sw, sh)).unwrap_or_else(|| Gfx::new(sw, sh));
                sc.hits.clear();
                sc.clear_probes();
                sc.mouse = (local_mouse.0, local_mouse.1 - (strip - crop) as i32);
                sc.gradient(BG_TOP, BG_BOTTOM);
                self.panes.compact = true;
                if focused {
                    crate::ui::with_input_focus(true, || self.draw_launcher(&mut sc, sw, sh, ps));
                } else {
                    crate::ui::with_input_focus(false, || self.draw_launcher(&mut sc, sw, sh, ps));
                }
                self.panes.compact = false;
                shift = (0.0, strip - crop);
                content_size = (sw, sh);
                // copy the visible part under the strip
                let rows = (ph as f32 - strip).round() as usize;
                let (dw, sw_) = (pw as usize * 4, sw as usize * 4);
                let src = sc.pm.data();
                let dst = pg.pm.data_mut();
                for row in 0..rows {
                    let (sy, dy) = (crop.round() as usize + row, strip.round() as usize + row);
                    if sy < sh as usize && dy < ph as usize {
                        dst[dy * dw..dy * dw + dw.min(sw_)].copy_from_slice(&src[sy * sw_..sy * sw_ + dw.min(sw_)]);
                    }
                }
                content_hits = std::mem::take(&mut sc.hits);
                for p in sc.probes() {
                    pg.hovered(R::new(p.x, p.y + shift.1, p.w, p.h));
                }
                ws.scratch = Some(sc);
            }
            // content hits into the pane's surface space (events that carry a position move with them)
            let mut hits: Vec<Hit> = Vec::new();
            for mut h in content_hits {
                if shift != (0.0, 0.0) {
                    h.rect.x += shift.0;
                    h.rect.y += shift.1;
                    h.ev = shift_event(&h.ev, shift);
                    if h.rect.y + h.rect.h <= strip {
                        continue;
                    }
                }
                hits.push(h);
            }
            // the strip is drawn over the top of the pane
            pg.hits.clear();
            let can_back = is_app;
            self.draw_strip(&mut pg, i, pw as f32, strip, ps, title, icon, accent, focused, can_back, self.panes.d.move_src == Some((self.panes.d.drawing, i)));
            hits.extend(std::mem::take(&mut pg.hits));
            let mut fields = crate::ctxmenu::fields();
            for f in fields.iter_mut() {
                f.rect.x += shift.0;
                f.rect.y += shift.1;
            }
            let pane = &mut ws.panes[i];
            pane.hits = hits;
            pane.probes = pg.probes();
            pane.fields = fields;
            pane.shift = shift;
            pane.content_size = content_size;
            pane.gfx = Some(pg);
        }
        // copy the surface into the frame
        let pane = &ws.panes[i];
        if let Some(pg) = &pane.gfx {
            blit(g, pg, r.x.round() as i32, r.y.round() as i32);
        }
        for h in &pane.hits {
            let mut h = h.clone();
            h.rect.x += r.x;
            h.rect.y += r.y;
            h.ev = shift_event(&h.ev, (r.x, r.y));
            // a region never reaches outside its pane
            let (x0, y0) = (h.rect.x.max(r.x), h.rect.y.max(r.y));
            let (x1, y1) = ((h.rect.x + h.rect.w).min(r.x + r.w), (h.rect.y + h.rect.h).min(r.y + r.h));
            if x1 - x0 < 0.5 || y1 - y0 < 0.5 {
                continue;
            }
            h.rect = R::new(x0, y0, x1 - x0, y1 - y0);
            g.hits.push(h);
        }
        for p in &pane.probes {
            g.hovered(R::new(p.x + r.x, p.y + r.y, p.w, p.h));
        }
        for f in &pane.fields {
            let mut f = *f;
            f.rect.x += r.x;
            f.rect.y += r.y;
            fields_out.push(f);
        }
        crashed
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_strip(&mut self, pg: &mut Gfx, i: usize, pw: f32, strip: f32, ps: f32, title: &str, icon: Option<Icon>, accent: [u8; 4], focused: bool, can_back: bool, moving: bool) {
        let lit = if focused { crate::fx::with_alpha(accent, 70) } else { [255, 255, 255, 14] };
        pg.rect(0.0, 0.0, pw, strip, rgb(12, 16, 32));
        pg.rect(0.0, 0.0, pw, strip, lit);
        pg.rect(0.0, strip - 1.0, pw, 1.0, crate::fx::with_alpha(accent, if focused { 160 } else { 50 }));
        let wide = pw >= 420.0 * ps;
        let bh = strip - 8.0 * ps;
        let mut x = pw - 6.0 * ps;
        let specs: Vec<(usize, &str, &str, &str)> = {
            let mut v = vec![(K_CLOSE, "Close", "\u{00d7}", "close")];
            if can_back {
                v.push((K_BACK, "Back", "\u{2190}", "back"));
            }
            v.push((K_LAYOUT, "Layout", "\u{25a6}", "layout"));
            v.push((K_MOVE, "Move", "\u{21c4}", "move"));
            v
        };
        let mut left_limit_used = 0.0f32;
        for (k, long, short, name) in specs {
            let label = if wide { long } else { short };
            let tw = pg.text_width(12.5 * ps, label, Face::Bold);
            let bw = tw + 14.0 * ps;
            let r = R::new(x - bw, 4.0 * ps, bw, bh);
            if r.x < 90.0 * ps {
                break;
            }
            let hot = pg.hovered(r);
            let on = k == K_MOVE && moving;
            pg.rrect(r.x, r.y, r.w, r.h, 5.0 * ps, if on { rgb(255, 196, 94) } else if hot { rgb(48, 60, 100) } else { rgb(26, 33, 58) });
            pg.text(r.x + 7.0 * ps, r.y + (r.h - 12.5 * ps) / 2.0 - ps * 0.5, 12.5 * ps, if on { rgb(20, 20, 20) } else { TEXT }, label, Face::Bold);
            pg.hit(r, Event::Tab(BASE + i * 8 + k), &format!("pane {} {}", i + 1, name));
            x = r.x - 4.0 * ps;
            left_limit_used = x;
        }
        let _ = left_limit_used;
        let mut tx = 10.0 * ps;
        if let Some(ic) = icon {
            pg.icon(ic, tx + 8.0 * ps, strip / 2.0, 18.0 * ps, accent);
            tx += 24.0 * ps;
        }
        let t = pg.fit(14.0 * ps, &format!("{}{}", if moving { "Moving: " } else { "" }, title), Face::Bold, (x - tx - 4.0 * ps).max(10.0));
        pg.text(tx, (strip - 14.0 * ps) / 2.0 - ps * 0.5, 14.0 * ps, if focused { TEXT } else { MUTED }, &t, Face::Bold);
    }

    /// The picker and the card, over the whole display.
    pub(super) fn draw_panes_overlays(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if let Some(sel) = self.panes.picker {
            self.draw_picker(g, w, h, s, sel);
        }
        if let Some(c) = self.panes.card.as_mut() {
            c.render(g, R::new(0.0, 0.0, w as f32, h as f32), s);
        }
    }

    fn draw_picker(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32, sel: usize) {
        let area = R::new(0.0, 0.0, w as f32, h as f32);
        let cols = 5usize;
        let rows = TEMPLATES.len().div_ceil(cols);
        let cell_w = ((w as f32 - 80.0 * s) / cols as f32).clamp_safe(90.0 * s, 150.0 * s);
        let cell_h = cell_w * 0.78;
        let pw = cell_w * cols as f32 + 52.0 * s;
        let ph = 60.0 * s + rows as f32 * (cell_h + 10.0 * s) + 120.0 * s;
        let p = crate::fx::modal(g, area, s, pw, ph, crate::fx::CYAN, false);
        crate::fx::modal_title(g, p, s, "Layout", crate::fx::CYAN);
        let top = p.y + 62.0 * s;
        let (area_r, gap_r) = (R::new(0.0, 0.0, cell_w - 18.0 * s, cell_h - 38.0 * s), 3.0 * s);
        let cur = self.panes.ws.as_ref().map_or("single", |ws| ws.layout.id.as_str()).to_string();
        for (i, t) in TEMPLATES.iter().enumerate() {
            let (cx0, cy0) = (p.x + 26.0 * s + (i % cols) as f32 * cell_w, top + (i / cols) as f32 * (cell_h + 10.0 * s));
            let cell = R::new(cx0, cy0, cell_w - 8.0 * s, cell_h);
            let blocked = self.template_blocked(i);
            let hot = g.hovered(cell);
            crate::fx::row_at(g, cell.x, cell.y, cell.w, cell.h, 10.0 * s, i == sel || t.id == cur, hot && blocked.is_none(), crate::fx::CYAN);
            let l = Layout::from_template(t.id);
            let pv = R::new(cell.x + 9.0 * s, cell.y + 8.0 * s, area_r.w, area_r.h);
            let ink = if blocked.is_some() { rgb(70, 78, 104) } else { crate::fx::CYAN };
            for (k, r) in l.rects(pv, gap_r).iter().enumerate() {
                g.rrect(r.x, r.y, r.w, r.h, 3.0 * s, crate::fx::with_alpha(ink, 60));
                g.rrect_outline(r.x, r.y, r.w, r.h, 3.0 * s, 1.0, ink);
                let num = (k + 1).to_string();
                let nw = g.text_width(11.0 * s, &num, Face::Bold);
                g.text(r.x + (r.w - nw) / 2.0, r.y + (r.h - 11.0 * s) / 2.0, 11.0 * s, ink, &num, Face::Bold);
            }
            let name = match t.key {
                Some(k) => format!("{k}  {}", t.name),
                None => t.name.to_string(),
            };
            let name = g.fit(12.0 * s, &name, Face::Bold, cell.w - 12.0 * s);
            g.text(cell.x + 6.0 * s, cell.y + cell.h - 24.0 * s, 12.0 * s, if blocked.is_some() { DIM } else { TEXT }, &name, Face::Bold);
            g.hit(cell, Event::Tab(PICK + i), &format!("layout {}", t.name));
        }
        // the memory line, the switch, close
        let y = top + rows as f32 * (cell_h + 10.0 * s) + 6.0 * s;
        let line = self.panes_memory_line();
        let lines = g.wrap(13.0 * s, &line, Face::Sans, p.w - 52.0 * s);
        for (k, l) in lines.iter().take(2).enumerate() {
            g.text(p.x + 26.0 * s, y + k as f32 * 18.0 * s, 13.0 * s, MUTED, l, Face::Sans);
        }
        let admin = self.session.as_ref().is_some_and(|s| s.policy.manage_settings);
        let by = p.y + p.h - 46.0 * s;
        if admin {
            let label = format!("Screen features: {}  (S)", self.panes.switch.label());
            let bw = g.button(p.x + 26.0 * s, by, 13.0 * s, &label, crate::fx::CYAN, Event::Tab(PICK_SWITCH));
            let _ = bw;
        } else {
            g.text(p.x + 26.0 * s, by + 8.0 * s, 12.0 * s, DIM, "An administrator can change when screen features are used.", Face::Sans);
        }
        let cw = g.text_width(13.0 * s, "Close", Face::Bold) + 13.0 * s * 1.7;
        g.button(p.x + p.w - 26.0 * s - cw, by, 13.0 * s, "Close", MUTED, Event::Tab(PICK_CLOSE));
    }

    // ------------------------------------------------------------------ tests and tools


    /// Put an app into pane `i` (tests).
    #[cfg(test)]
    pub(crate) fn panes_put_for_test(&mut self, i: usize, app: Box<dyn App>, pa: PApp) {
        match self.panes.ws.as_mut() {
            Some(ws) if ws.focus != i => {
                ws.panes[i].content = Some((app, pa));
                ws.panes[i].dirty = true;
            }
            _ => self.show_app_for_test(app, pa),
        }
        self.ws_mark_dirty(i);
    }

    /// Where the content of pane `i` starts on the display, and the size the content believes the screen is (tests).
    #[cfg(test)]
    pub(crate) fn panes_content_for_test(&self, i: usize) -> Option<(R, (u32, u32))> {
        let ws = self.panes.ws.as_ref()?;
        let r = *ws.rects.get(i)?;
        let p = ws.panes.get(i)?;
        Some((R::new(r.x + p.shift.0, r.y + p.shift.1, 0.0, 0.0), p.content_size))
    }

    #[cfg(test)]
    pub(crate) fn panes_focus_for_test(&self) -> Option<usize> {
        self.panes.ws.as_ref().map(|ws| ws.focus)
    }

    #[cfg(test)]
    pub(crate) fn panes_layout_for_test(&self) -> String {
        self.panes.ws.as_ref().map_or("single".to_string(), |ws| ws.layout.id.clone())
    }

    #[cfg(test)]
    pub(crate) fn panes_rects_for_test(&self) -> Vec<R> {
        self.panes.ws.as_ref().map(|ws| ws.rects.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn panes_redraws_for_test(&self) -> u64 {
        self.panes.ws.as_ref().map_or(0, |ws| ws.redraws)
    }

    /// The names of what each pane holds ("Home" for a launcher), in pane order.
    #[cfg(test)]
    pub(crate) fn panes_titles_for_test(&self) -> Vec<String> {
        let Some(ws) = &self.panes.ws else { return vec![self.screen_name()] };
        ws.panes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == ws.focus {
                    match &self.screen {
                        Screen::App(a, _) => a.title().to_string(),
                        _ => "Home".into(),
                    }
                } else {
                    p.content.as_ref().map_or("Home".to_string(), |(a, _)| a.title().to_string())
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn set_mem_for_test(&mut self, m: Option<MemFacts>) {
        self.panes.mem = m;
        self.refresh_gate();
    }

    #[cfg(test)]
    pub(crate) fn panes_card_for_test(&self) -> Option<String> {
        self.panes.card.as_ref().map(|c| format!("{} {}", c.title, c.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join(" ")))
    }

    #[cfg(test)]
    pub(crate) fn panes_picker_open_for_test(&self) -> bool {
        self.panes.picker.is_some()
    }

}

/// The template to fall back to with at most `n` panes.
fn smaller_for(n: usize) -> &'static str {
    match n {
        0 | 1 => "single",
        2 => "cols-50-50",
        3 => "thirds",
        _ => "grid",
    }
}

/// The area the panes share on a display of this size, and the gap between them.
pub(super) fn ws_area(w: u32, h: u32) -> (R, f32) {
    let s = scale_for(w, h);
    let m = 6.0 * s;
    let bar = BAR_H * s;
    (R::new(m, bar + m, (w as f32 - 2.0 * m).max(1.0), (h as f32 - bar - 2.0 * m).max(1.0)), GAP * s)
}

fn split_dir(l: &Layout, split: usize) -> crate::panes::Dir {
    let (area, gap) = (R::new(0.0, 0.0, 1000.0, 1000.0), 0.0);
    l.dividers(area, gap, 0.0).into_iter().find(|d| d.split == split).map_or(crate::panes::Dir::Side, |d| d.dir)
}

/// Events that carry a position move with the region they came from.
fn shift_event(ev: &Event, by: (f32, f32)) -> Event {
    let m = |x: i32, y: i32| ((x as f32 + by.0).round() as i32, (y as f32 + by.1).round() as i32);
    match ev {
        Event::MouseDown(x, y) => {
            let (a, b) = m(*x, *y);
            Event::MouseDown(a, b)
        }
        Event::MouseUp(x, y) => {
            let (a, b) = m(*x, *y);
            Event::MouseUp(a, b)
        }
        Event::MouseMove(x, y) => {
            let (a, b) = m(*x, *y);
            Event::MouseMove(a, b)
        }
        Event::MouseRight(x, y) => {
            let (a, b) = m(*x, *y);
            Event::MouseRight(a, b)
        }
        other => other.clone(),
    }
}

/// Copy a pane's surface into the frame (both are premultiplied RGBA, the pane is opaque).
fn blit(g: &mut Gfx, src: &Gfx, x: i32, y: i32) {
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (dw, dh) = (g.width() as i32, g.height() as i32);
    let sdata = src.pm.data();
    let ddata = g.pm.data_mut();
    for row in 0..sh {
        let dy = y + row;
        if dy < 0 || dy >= dh {
            continue;
        }
        let (x0, x1) = (x.max(0), (x + sw).min(dw));
        if x1 <= x0 {
            continue;
        }
        let n = ((x1 - x0) * 4) as usize;
        let so = ((row * sw + (x0 - x)) * 4) as usize;
        let d_o = ((dy * dw + x0) * 4) as usize;
        ddata[d_o..d_o + n].copy_from_slice(&sdata[so..so + n]);
    }
}

/// The grid sign on the LAYOUT button.
pub(super) fn draw_layout_button(g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
    let hot = g.hovered(r);
    crate::fx::chrome_button(g, r, s * if r.w > 40.0 * s { 2.5 } else { 1.0 }, hot, accent);
    let d = r.h.min(r.w) * 0.5;
    let (x0, y0) = (r.x + (r.w - d) / 2.0, r.y + (r.h - d) / 2.0);
    let c = d * 0.44;
    for (i, j) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        g.rrect(x0 + i * (d - c), y0 + j * (d - c), c, c, 2.0 * s, MUTED);
    }
    g.hit(r, Event::Tab(OPEN), "layout");
}

#[path = "shell_displays.rs"]
mod disp;
