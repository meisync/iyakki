//! Accessibility in the shell (docs/ACCESSIBILITY.md): the screen reader, the keyboard walk over the controls of a screen, and the
//! options person and computer keep (`iyakki_shared::accessibility`).
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

use crate::stale_stats::mark_stale;
use super::*;
use crate::access::{self, Cursor, Node, Reader, Role, SpeechSink};
use iyakki_shared::accessibility as accfile;
use iyakki_shared::voiceassist::Speaker;

/// The system reading voice behind the reader's [`SpeechSink`].
struct VoiceSink(Box<dyn Speaker>);

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

/// The core's braille display as the toolkit's [`CellDisplay`](iyakki_uikit::braille::CellDisplay): the shell never opens a device itself.
struct CoreCells(Box<dyn iyakki_core_api::BrailleCells>);

impl iyakki_uikit::braille::CellDisplay for CoreCells {
    fn width(&self) -> usize {
        self.0.width()
    }
    fn put(&mut self, cells: &str) -> Result<(), String> {
        self.0.put(cells)
    }
    fn keys(&mut self) -> Vec<iyakki_uikit::braille::Press> {
        use iyakki_core_api::BrailleKey as K;
        use iyakki_uikit::braille::Press as P;
        self.0
            .keys()
            .into_iter()
            .map(|k| match k {
                K::PanLeft => P::PanLeft,
                K::PanRight => P::PanRight,
                K::FocusPrev => P::FocusPrev,
                K::FocusNext => P::FocusNext,
                K::Route(n) => P::Route(n),
            })
            .collect()
    }
}

/// Beats between two tries to find a display that is not there (the heartbeat counts them: no clock).
const BRAILLE_RETRY_BEATS: u32 = 60;

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
    last_dialog: Option<String>,
    stamp: Option<([Option<std::time::SystemTime>; 3], String)>,
    /// The braille display while it is wanted and connected (F-A11Y-07).
    braille: Option<iyakki_uikit::braille::Session<CoreCells>>,
    /// Beats left before the next try to find a display.
    braille_wait: u32,
}

/// What is said when the reader has no voice to speak with.
pub(crate) const NO_VOICE: &str = "The screen reader is on, but no reading voice is installed, so it cannot speak. A sighted helper can open Settings, Voice and choose a reading voice. Super+Alt+S turns the reader off.";

impl Shell {
    fn access_user(&self) -> String {
        self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default()
    }

    /// Read the saved choices of whoever is here (the computer's alone at the sign-in screen) and apply them.
    pub(super) fn access_reload(&mut self) {
        let dir = self.env.data_dir().to_path_buf();
        let user = self.access_user();
        let cfg = if user.is_empty() { accfile::Access { screen_reader: accfile::machine_reader(&dir), ..Default::default() } } else { accfile::effective(&dir, &user) };
        self.acc.stamp = Some((accfile::stamp(&dir, &user), user.clone()));
        if !user.is_empty() {
            crate::theme::apply_for(&dir, &user);
            iyakki_uikit::wallpaper::apply_for(&dir, &user);
            iyakki_uikit::fonts::apply_for(&dir, &user);
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
            mark_stale!(self);
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
    /// The controls of the frame whose spoken name is empty or a bare symbol (fewer than two letters or digits, and not a number): a reader would say nothing useful.
    #[cfg(test)]
    pub(crate) fn thin_names_for_test(&self) -> Vec<String> {
        self.access_controls().iter().filter(|e| e.node.name.chars().filter(|c| c.is_alphanumeric()).count() < 2 && !e.node.name.chars().all(|c| c.is_ascii_digit() && !c.is_whitespace()) || e.node.name.is_empty()).map(|e| format!("{:?} {:?}", e.node.name, self.hits[e.hit].ev)).collect()
    }

    #[cfg(test)]
    pub(crate) fn unnamed_controls_for_test(&self) -> Vec<(usize, String)> {
        access::unnamed(&self.hits, (self.size.0 as f32, self.size.1 as f32))
    }

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
        mark_stale!(self);
        true
    }

    fn access_make_sink(&mut self) {
        if self.acc.sink.is_none() {
            self.acc.sink = Some(Box::new(VoiceSink(self.env.services.system_speaker())));
        }
    }

    fn access_toggle_reader(&mut self) {
        let mut cfg = self.acc.cfg.clone();
        cfg.screen_reader = !cfg.screen_reader;
        let dir = self.env.data_dir().to_path_buf();
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
            let dir = self.env.data_dir().to_path_buf();
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
        } else if self.acc.braille.is_some() {
            self.access_braille_step();
        }
    }

    /// The braille display, once per beat: found when the choice and the reader are on, shown what the reader said, its keys carried out.
    /// A display that fails is dropped and looked for again later; turning the choice off blanks it.
    fn access_braille_step(&mut self) {
        let want = self.acc.cfg.braille && self.acc.cfg.screen_reader;
        if !want {
            if let Some(mut s) = self.acc.braille.take() {
                let _ = s.set_on(false);
            }
            self.acc.reader.take_spoken();
            return;
        }
        if self.acc.braille.is_none() {
            self.acc.braille_wait = self.acc.braille_wait.saturating_sub(1);
            if self.acc.braille_wait == 0 {
                match self.core.braille().open() {
                    Some(c) => self.acc.braille = Some(iyakki_uikit::braille::Session::new(CoreCells(c), true)),
                    None => self.acc.braille_wait = BRAILLE_RETRY_BEATS,
                }
            }
        }
        let said = self.acc.reader.take_spoken();
        let Some(s) = self.acc.braille.as_mut() else { return };
        let grade = if self.acc.cfg.braille_contracted { iyakki_uikit::braille::Grade::Two } else { iyakki_uikit::braille::Grade::One };
        let regraded = s.set_grade(grade).is_err();
        let mut failed = regraded || said.is_some_and(|t| s.announce(&t).is_err());
        let mut focus = Vec::new();
        let mut carets = Vec::new();
        match s.poll() {
            Ok(list) => {
                for o in list {
                    match o {
                        iyakki_uikit::braille::Outcome::Focus { forward } => focus.push(forward),
                        iyakki_uikit::braille::Outcome::Cursor(i) => carets.push((i, s.line().to_string())),
                        _ => {}
                    }
                }
            }
            Err(_) => failed = true,
        }
        if failed {
            self.acc.braille = None;
            self.acc.braille_wait = BRAILLE_RETRY_BEATS;
        }
        for forward in focus {
            self.access_walk(!forward);
        }
        for (i, line) in carets {
            self.access_route_caret(i, &line);
        }
    }

    /// A routing key of the braille display over a text field: the caret goes to the character under the key. The field the walk stands on is
    /// clicked (so it has the keyboard), then Home and as many Right keys as the character's place say, the way a person would. Only when the
    /// line on the display is what that field says now (the display may show something else), never for a secret field.
    fn access_route_caret(&mut self, spoken_index: usize, line: &str) {
        let list = self.access_controls();
        let Some(i) = self.acc.cursor.find(&list, &self.hits) else { return };
        let node = list[i].node.clone();
        if node.speak() != line {
            return;
        }
        let Some(to) = iyakki_uikit::braille::field_caret(&node, spoken_index) else { return };
        self.access_press();
        self.handle(&Event::Key(Key::Home));
        for _ in 0..to.min(4096) {
            self.handle(&Event::Key(Key::Right));
        }
    }

    fn access_pump(&mut self) -> Option<String> {
        let problem = {
            let AccessUi { reader, sink, .. } = &mut self.acc;
            sink.as_mut().and_then(|s| reader.pump(s.as_mut()))
        };
        self.access_braille_step();
        let problem = problem?;
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
        let before = self.acc.last_widget.clone();
        if widget.is_some() || new_screen {
            self.acc.last_widget = widget;
        }
        // typing echo: the same field, its text grew or shrank at the end (never a field that hides its text)
        if !new_screen && self.acc.cfg.typing_echo {
            if let (Some(o), Some(n)) = (&before, &self.acc.last_widget) {
                if let Some(t) = access::typing_echo(o, n) {
                    self.acc.reader.say_now(&t);
                }
            }
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
        let osd = self.fnk.osd.as_ref().map(|o| o.spoken()).unwrap_or_default();
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
        // a dialog the tile opened over itself: its title, once, when it appears (and again when another one replaces it)
        let dialog = match &self.screen {
            Screen::App(app, _) => app.dialog_title().or_else(|| access::generic_dialog(&self.hits, app.title())),
            _ => None,
        };
        if dialog != self.acc.last_dialog {
            if let Some(d) = &dialog {
                self.acc.reader.notice(&format!("Dialog. {d}"));
            }
            self.acc.last_dialog = dialog;
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
