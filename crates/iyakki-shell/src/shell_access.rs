//! Accessibility in the shell (docs/ACCESSIBILITY.md): the screen reader, the keyboard walk over the controls of a screen, and the
//! options person and computer keep (`iyakki_agent::accessibility`).
//!
//! * **Keys** (taken before any screen sees them, also at the sign-in and the lock screens): Super+Alt+S the screen reader on or off,
//!   Super+Alt+Right / Left the next / previous control, Super+Alt+Enter press it, Super+Alt+R say the last thing again. `input.rs`
//!   turns the chords into `Event::Fn` keys; sticky keys let them be typed one key at a time.
//! * **The walk** stands on a control of the last frame ([`crate::access::entries`]): a ring is drawn round it and, with the reader on,
//!   it is spoken. Pressing it sends the click the control would get (a pointer move, press and release at its middle), so everything
//!   a mouse can do is reachable from the keyboard on every screen, whatever the tile's own keys are.
//! * **The reader** speaks the control the walk moved to, a text field that took the focus, the chosen row of a list, a new screen or
//!   dialog, toasts, errors and notices. A password field says how many characters were typed, never which. A new focus interrupts
//!   what is being said. The voice is the one of Settings -> Voice; without one the reader says so on screen.

use super::*;
use crate::access::{self, Cursor, Node, Reader, Role, SpeechSink};
use iyakki_agent::accessibility as accfile;
use iyakki_agent::voiceassist::Speaker;

/// The system reading voice behind the reader's [`SpeechSink`].
struct VoiceSink(iyakki_agent::voiceassist_ports::SystemSpeaker);

impl SpeechSink for VoiceSink {
    fn say(&mut self, text: &str) -> Result<(), String> {
        self.0.say(text)
    }
    fn busy(&mut self) -> bool {
        self.0.busy()
    }
    fn stop(&mut self) {
        self.0.stop()
    }
}

#[derive(Default)]
pub(super) struct AccessUi {
    pub(super) cfg: accfile::Access,
    pub(super) reader: Reader,
    cursor: Cursor,
    sink: Option<Box<dyn SpeechSink>>,
    last_widget: Option<Node>,
    last_screen: String,
    last_toast: Option<String>,
    last_osd: String,
    last_err: Option<String>,
    stamp: Option<([Option<std::time::SystemTime>; 3], String)>,
}

/// What is said when the reader has no voice to speak with.
pub(crate) const NO_VOICE: &str = "The screen reader is on, but no reading voice is installed, so it cannot speak. A sighted helper can open Settings, Voice and choose a reading voice. Super+Alt+S turns the reader off.";

impl Shell {
    fn access_user(&self) -> String {
        self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default()
    }

    /// Read the saved choices of whoever is here (the computer's alone at the sign-in screen) and apply them.
    pub(super) fn access_reload(&mut self) {
        let dir = self.env.services.dir().to_path_buf();
        let user = self.access_user();
        let cfg = if user.is_empty() { accfile::Access { screen_reader: accfile::machine_reader(&dir), ..Default::default() } } else { accfile::effective(&dir, &user) };
        self.acc.stamp = Some((accfile::stamp(&dir, &user), user.clone()));
        if !user.is_empty() {
            crate::theme::apply(Some(crate::theme::load(&dir, &user)));
        }
        self.access_apply(cfg);
    }

    fn access_apply(&mut self, cfg: accfile::Access) {
        let was = self.acc.cfg.screen_reader;
        access::set_prefs(access::Prefs { screen_reader: cfg.screen_reader, large_pointer: cfg.large_pointer, big_ring: cfg.big_ring, reduce_motion: cfg.reduce_motion });
        access::set_key_options(cfg.sticky_keys, cfg.slow_keys_ms);
        let changed = self.acc.cfg != cfg;
        self.acc.cfg = cfg;
        if was && !self.acc.cfg.screen_reader {
            if let Some(sink) = self.acc.sink.as_mut() {
                self.acc.reader.silence(sink.as_mut());
            }
            self.acc.cursor.clear();
        }
        if !was && self.acc.cfg.screen_reader {
            // the screen as it is now is read once the reader starts
            self.acc.last_screen.clear();
            self.acc.last_widget = None;
        }
        if changed {
            self.stale = true;
        }
    }

    /// A speech sink of the tests (the real one is made when first needed).
    #[cfg(test)]
    pub(crate) fn access_sink_for_test(&mut self, sink: Box<dyn SpeechSink>) {
        self.acc.sink = Some(sink);
    }

    #[cfg(test)]
    pub(crate) fn access_apply_for_test(&mut self, cfg: accfile::Access) {
        self.access_apply(cfg);
    }

    #[cfg(test)]
    pub(crate) fn access_config(&self) -> &accfile::Access {
        &self.acc.cfg
    }

    #[cfg(test)]
    pub(crate) fn access_problem(&self) -> Option<String> {
        self.acc.reader.problem.clone()
    }

    /// Where the keyboard walk stands: the control's name, if it stands anywhere.
    #[cfg(test)]
    pub(crate) fn access_walk_at(&self) -> Option<String> {
        let list = access::entries(&self.hits, (self.size.0 as f32, self.size.1 as f32));
        self.acc.cursor.find(&list, &self.hits).map(|i| list[i].node.name.clone())
    }

    /// The controls the walk can stand on in the last frame, in order (tests and the reachability report).
    pub(crate) fn access_controls(&self) -> Vec<access::Entry> {
        access::entries(&self.hits, (self.size.0 as f32, self.size.1 as f32))
    }

    // ------------------------------------------------------------------------------------------------------ keys

    /// The accessibility keys. True when the event was one of them.
    pub(super) fn access_intercept(&mut self, ev: &Event) -> bool {
        use crate::fnkeys::FnKey;
        let Event::Fn(key, down) = ev else { return false };
        if !matches!(key, FnKey::ScreenReader | FnKey::AccessNext | FnKey::AccessPrev | FnKey::AccessActivate | FnKey::AccessRepeat) {
            return false;
        }
        if !*down {
            return true;
        }
        match key {
            FnKey::ScreenReader => self.access_toggle_reader(),
            FnKey::AccessNext => self.access_walk(false),
            FnKey::AccessPrev => self.access_walk(true),
            FnKey::AccessActivate => self.access_press(),
            _ => {
                self.access_make_sink();
                self.acc.reader.repeat();
            }
        }
        self.stale = true;
        true
    }

    fn access_make_sink(&mut self) {
        if self.acc.sink.is_none() {
            self.acc.sink = Some(Box::new(VoiceSink(iyakki_agent::voiceassist_ports::SystemSpeaker::new(self.env.services.clone()))));
        }
    }

    fn access_toggle_reader(&mut self) {
        let mut cfg = self.acc.cfg.clone();
        cfg.screen_reader = !cfg.screen_reader;
        let dir = self.env.services.dir().to_path_buf();
        let user = self.access_user();
        let saved = if user.is_empty() { accfile::set_machine_reader(&dir, cfg.screen_reader) } else { accfile::save(&dir, &user, &accfile::Access { screen_reader: cfg.screen_reader, ..accfile::load(&dir, &user) }) };
        if let Err(e) = saved {
            self.say(format!("The screen reader choice could not be kept: {e}"));
        }
        let on = cfg.screen_reader;
        self.access_apply(cfg);
        self.access_make_sink();
        if on {
            self.fn_notice("Screen reader on");
            self.acc.reader.say_now("Screen reader on. Super Alt Right Arrow moves to the next control, Super Alt Enter presses it.");
        } else {
            self.fn_notice("Screen reader off");
            // said once, directly: the reader itself is off now
            if let Some(sink) = self.acc.sink.as_mut() {
                sink.stop();
                let _ = sink.say("Screen reader off");
            }
        }
        if let Some(e) = self.access_pump() {
            self.say(e);
        }
    }

    fn access_walk(&mut self, back: bool) {
        let list = self.access_controls();
        let at = self.acc.cursor.step(&list, &self.hits, back);
        match at {
            Some(i) => {
                if self.acc.cfg.screen_reader {
                    self.access_make_sink();
                    self.acc.reader.focus(&list[i].node, true);
                }
            }
            None => {
                if self.acc.cfg.screen_reader {
                    self.access_make_sink();
                    self.acc.reader.say_now("Nothing to select on this screen");
                }
            }
        }
    }

    /// Super+Alt+Enter: the click the control under the walk would get.
    fn access_press(&mut self) {
        let list = self.access_controls();
        let Some(i) = self.acc.cursor.find(&list, &self.hits) else {
            if self.acc.cfg.screen_reader {
                self.access_make_sink();
                self.acc.reader.say_now("No control is selected. Super Alt Right Arrow selects the first.");
            }
            return;
        };
        let r = list[i].rect;
        let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        self.handle(&Event::MouseMove(x, y));
        self.handle(&Event::MouseDown(x, y));
        self.handle(&Event::MouseUp(x, y));
    }

    // ------------------------------------------------------------------------------------------------- each beat

    /// Once per heartbeat: new choices written by Settings or the assistant, then the speech queue.
    pub(super) fn access_tick(&mut self) {
        let user_changed = self.acc.stamp.as_ref().is_none_or(|(_, u)| *u != self.access_user());
        if self.tick % 15 == 0 || user_changed {
            let dir = self.env.services.dir().to_path_buf();
            let user = self.access_user();
            let now = accfile::stamp(&dir, &user);
            if self.acc.stamp.as_ref().is_none_or(|(s, u)| *s != now || *u != user) {
                self.access_reload();
            }
        }
        if self.acc.cfg.screen_reader {
            self.access_observe(false);
            if let Some(e) = self.access_pump() {
                self.say(e);
            }
        }
    }

    fn access_pump(&mut self) -> Option<String> {
        let AccessUi { reader, sink, .. } = &mut self.acc;
        let sink = sink.as_mut()?;
        let problem = reader.pump(sink.as_mut())?;
        Some(if problem.contains("No reading voice") { NO_VOICE.to_string() } else { format!("The screen reader could not speak: {problem}") })
    }

    fn access_screen_title(&self) -> String {
        match self.screen_name().as_str() {
            "login" => "Sign in".into(),
            "setup" => "First start. Create the first account".into(),
            "locked" => "Screen locked. Type your password to unlock".into(),
            "launcher" => "Home".into(),
            "power" => "Power menu".into(),
            "hwcheck" => "Hardware check".into(),
            _ => match &self.screen {
                Screen::App(app, _) => {
                    let page = app.page();
                    if page.is_empty() { app.title().to_string() } else { format!("{}, {}", app.title(), page) }
                }
                _ => String::new(),
            },
        }
    }

    /// What a frame (`frame`) or a beat tells the reader: a new screen, the control with the focus, messages. Called at the end of every
    /// frame and on every beat; it only speaks what changed.
    pub(super) fn access_observe(&mut self, frame: bool) {
        let widget = access::take_focus();
        let selected = access::take_selected();
        let notices = access::take_announcements();
        if !self.acc.cfg.screen_reader {
            self.acc.last_widget = None;
            return;
        }
        self.access_make_sink();
        if !frame {
            // a beat sees no controls: it only passes on what was said
            if self.acc.cfg.speak_notices {
                self.access_notices(notices);
            }
            return;
        }
        // the control with the focus: a text field with the caret, else the list row the arrow keys chose
        let widget = widget.or_else(|| selected.last().and_then(|r| self.access_row_under(*r)));
        let title_key = format!("{}|{}", self.screen_name(), self.access_screen_title());
        let new_screen = title_key != self.acc.last_screen;
        if new_screen {
            self.acc.last_screen = title_key;
            self.acc.cursor.clear();
            self.acc.reader.forget_focus();
            self.acc.last_widget = None;
        }
        // typing into the field that already has the focus is not read back (the whole text again at every key); another control is
        let same = |a: &Node, b: &Node| if a.role == Role::Field && b.role == Role::Field { a.name == b.name } else { a == b };
        let changed = widget.as_ref().filter(|w| !self.acc.last_widget.as_ref().is_some_and(|l| same(l, w))).cloned();
        if widget.is_some() || new_screen {
            self.acc.last_widget = widget;
        }
        match (new_screen, changed) {
            (true, Some(n)) => {
                let t = self.access_screen_title();
                self.acc.reader.focus_after(&t, &n);
            }
            (true, None) => {
                let t = self.access_screen_title();
                if !t.is_empty() {
                    self.acc.reader.say_now(&t);
                }
            }
            (false, Some(n)) => {
                self.acc.reader.focus(&n, false);
            }
            (false, None) => {}
        }
        if self.acc.cfg.speak_notices {
            self.access_notices(notices);
        }
    }

    fn access_notices(&mut self, notices: Vec<String>) {
        let toast = self.toast.as_ref().map(|t| t.0.clone());
        if toast != self.acc.last_toast {
            if let Some(t) = &toast {
                self.acc.reader.notice(t);
            }
            self.acc.last_toast = toast;
        }
        let osd = self.fnk.osd.as_ref().filter(|o| o.fraction.is_none()).map(|o| o.text.clone()).unwrap_or_default();
        if osd != self.acc.last_osd {
            if !osd.is_empty() {
                self.acc.reader.notice(&osd);
            }
            self.acc.last_osd = osd;
        }
        let err = match &self.screen {
            Screen::Login(f) | Screen::Setup(f) => f.error.clone(),
            _ => None,
        };
        let err = err.or_else(|| if self.lockui.active { self.lockui.form.error.clone() } else { None });
        if err != self.acc.last_err {
            if let Some(e) = &err {
                self.acc.reader.notice(&format!("Error. {e}"));
            }
            self.acc.last_err = err;
        }
        if self.help {
            if !self.acc.last_screen.ends_with("|Help") {
                self.acc.reader.notice("Help. The keys of this screen are listed. Escape closes it.");
                self.acc.last_screen.push_str("|Help");
            }
        } else if self.acc.last_screen.ends_with("|Help") {
            let n = self.acc.last_screen.len() - "|Help".len();
            self.acc.last_screen.truncate(n);
        }
        for n in notices {
            self.acc.reader.notice(&n);
        }
    }

    /// The control drawn under a chosen row, as a list item that is selected.
    fn access_row_under(&self, row: R) -> Option<Node> {
        let (cx, cy) = (row.x + row.w / 2.0, row.y + row.h / 2.0);
        let h = self.hits.iter().rev().find(|h| h.rect.contains(cx as i32, cy as i32) && h.rect.w < row.w * 1.05 + 4.0)?;
        let mut n = access::node_of(h)?;
        if n.role == Role::Button || n.role == Role::ListItem {
            n.role = Role::ListItem;
            n.state = n.state.selected(true);
        }
        Some(n)
    }

    /// Drawn over the finished frame: the ring round the control the walk stands on, and the sign that the reader has no voice.
    pub(super) fn access_draw(&mut self, g: &mut Gfx) {
        let (w, h) = self.size;
        let s = scale_for(w, h);
        if self.acc.cursor.is_set() {
            let list = self.access_controls();
            if let Some(i) = self.acc.cursor.find(&list, &self.hits) {
                let r = list[i].rect.inset(-3.0 * s);
                let k = access::focus_ring_factor();
                g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 5.0 * s * k, crate::ui::BG_BOTTOM);
                g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 3.0 * s * k, crate::ui::TEXT);
                g.rrect_outline(r.x + 1.5 * s, r.y + 1.5 * s, r.w - 3.0 * s, r.h - 3.0 * s, 7.0 * s, 1.5 * s * k, crate::ui::ACCENT_MAIN);
            }
        }
        if self.acc.cfg.screen_reader && self.acc.reader.problem.is_some() {
            let t = "Screen reader: no voice. Settings, Voice";
            let tw = g.text_width(14.0 * s, t, Face::Bold) + 28.0 * s;
            let r = R::new(w as f32 - tw - 16.0 * s, h as f32 - 48.0 * s, tw, 32.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, rgb(70, 50, 0));
            g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, crate::fx::AMBER);
            g.text(r.x + 14.0 * s, r.y + 7.0 * s, 14.0 * s, crate::ui::TEXT, t, Face::Bold);
        }
    }
}
