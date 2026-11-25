//! LEARN: the study plans and quizzes the chat makes (`docs/STUDY.md`). Two tabs, Plans and Quizzes, over the same store the chat's
//! `study_*` and `quiz_*` tools use (`iyakki_shared::study`): a plan made in the chat is here, and a session ticked here is seen by the chat.
//!
//! * Plans: pick a plan, tick sessions done, optionally put the plan in the calendar (and take it out again), delete it.
//! * Quizzes: one question at a time, pick an option, see right or wrong with the explanation, Next, and at the end the score. **The answer
//!   key is never drawn before the person has answered that question.** The code scores (`study::score`); a question the person reports as
//!   wrong stops counting, and the percent is worked out over the rest.
//!
//! Keys: Left / Right change tab, Up / Down choose, Enter opens or picks, Space ticks a session, a-f or 1-6 pick an option, n or Enter goes
//! on, r reports a question, c calendar, x delete (twice), Esc goes back.
//!
//! * A lesson: Enter (or a click) on a day of a plan opens the lesson of that day at once (`learn_lesson.rs`): the goals of the day, then the
//!   teaching streaming in with pictures. Left / Right move between its sections, Space marks the day done, q asks a question, s simpler,
//!   d deeper, g writes it again (asks first), r reads it aloud, c cancels the writer, Esc goes back to the plan.

use iyakki_study_model::study::{score, Plan, Quiz};
use iyakki_uikit::gfx::{rgb, Icon};
use iyakki_uikit::scrolllist::ScrollList;
use iyakki_uikit::ui::{Event, Key};

use crate::lesson::*;
use crate::port::{LearnPort, Pictures};

/// What a screen of the tile answers to an event: stay, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(255, 236, 70);

pub const T_PLANS: usize = 1;
pub const T_QUIZZES: usize = 2;
pub const T_BACK: usize = 3;
pub const T_CAL: usize = 4;
pub const T_UNCAL: usize = 5;
pub const T_DELETE: usize = 6;
pub const T_YES: usize = 7;
pub const T_NO: usize = 8;
pub const T_START: usize = 9;
pub const T_NEXT: usize = 10;
pub const T_REPORT: usize = 11;
pub const T_AGAIN: usize = 12;
pub const T_STOP: usize = 13;
pub const T_DISMISS: usize = 14;
/// Export the open (or chosen) plan, quiz or lesson as a PDF, and the switch for the quiz's answer key page.
pub const T_PDF: usize = 40;
pub const T_PDF_KEY: usize = 41;
/// Option `i` of the open question is `T_OPTION + i`.
pub const T_OPTION: usize = 100;
pub const A_LIST: usize = 61_330;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Plans,
    Quizzes,
}

/// A quiz being taken.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    pub idx: usize,
    pub picks: Vec<Option<usize>>,
    /// The highlighted option (keys Up / Down, Enter picks it).
    pub cursor: usize,
    /// The current question has been answered: the key and the explanation are shown.
    pub answered: bool,
}

pub enum View {
    List,
    Plan(String),
    Quiz(Run),
    /// The result of the latest attempt at a quiz, with the row chosen for reporting.
    Result(String, usize),
    /// The lesson of one day of a plan.
    Lesson(Box<LessonState>),
}

pub struct LearnApp {
    pub user: String,
    pub tab: Tab,
    pub view: View,
    pub plans: Vec<Plan>,
    pub quizzes: Vec<Quiz>,
    pub sel: [usize; 2],
    pub sel_session: usize,
    pub confirm_delete: bool,
    pub msg: Option<(String, bool)>,
    pub list: ScrollList,
    pub loaded_at: u64,
    pub request: Option<(String, String)>,
    pub dirty: bool,
    pub pictures: Box<dyn Pictures>,
    /// The picture of a lesson, scaled to the slot it is drawn in: (id, width, height, scale, pixels).
    pub pic_scaled: Vec<(u64, i32, i32, i32, tiny_skia::Pixmap)>,
    /// The voice must stop at the next call that holds the port (set where there is no port at hand).
    pub stop_reading: bool,
    /// The voice was reading at the last frame (the port is not at hand while drawing).
    pub voice_on: bool,
    /// Put the answer key on its own page in a quiz PDF (off until switched on).
    pub pdf_key: bool,
}

impl LearnApp {
    pub fn new(user: &str, cx: &dyn LearnPort, pictures: Box<dyn Pictures>) -> LearnApp {
        let mut a = LearnApp { user: user.to_string(), tab: Tab::Plans, view: View::List, plans: vec![], quizzes: vec![], sel: [0; 2], sel_session: 0, confirm_delete: false, msg: None, list: ScrollList::default(), loaded_at: 0, request: None, dirty: true, pictures, pic_scaled: Vec::new(), stop_reading: false, voice_on: false, pdf_key: false };
        a.reload(cx);
        a
    }

    /// Read the store again: the chat may have made or changed things since.
    pub fn reload(&mut self, cx: &dyn LearnPort) {
        self.plans = cx.plans(&self.user);
        self.quizzes = cx.quizzes(&self.user);
        self.sel[0] = self.sel[0].min(self.plans.len().saturating_sub(1));
        self.sel[1] = self.sel[1].min(self.quizzes.len().saturating_sub(1));
        // what the open view shows may be gone (deleted from the chat)
        let gone = match &self.view {
            View::Plan(id) => !self.plans.iter().any(|p| &p.id == id),
            View::Quiz(r) => !self.quizzes.iter().any(|q| q.id == r.id),
            View::Result(id, _) => !self.quizzes.iter().any(|q| &q.id == id),
            View::List => false,
            View::Lesson(l) => !self.plans.iter().any(|p| p.id == l.plan),
        };
        if gone {
            self.stop_lesson();
            self.view = View::List;
            self.say("That item was deleted.", false);
        }
    }

    pub fn say(&mut self, m: &str, ok: bool) {
        self.msg = Some((m.to_string(), ok));
    }

    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }

    pub fn quiz(&self, id: &str) -> Option<&Quiz> {
        self.quizzes.iter().find(|q| q.id == id)
    }

    fn tab_ix(&self) -> usize {
        usize::from(self.tab == Tab::Quizzes)
    }

    fn set_tab(&mut self, t: Tab) {
        self.tab = t;
        self.view = View::List;
        self.confirm_delete = false;
        self.list.to_top();
    }

    fn open_selected(&mut self) {
        self.list.to_top();
        match self.tab {
            Tab::Plans => {
                if let Some(p) = self.plans.get(self.sel[0]) {
                    self.sel_session = p.next_index().unwrap_or(0);
                    self.view = View::Plan(p.id.clone());
                }
            }
            Tab::Quizzes => self.start_quiz(),
        }
    }

    fn start_quiz(&mut self) {
        if let Some(q) = self.quizzes.get(self.sel[1]) {
            self.list.to_top();
            self.view = View::Quiz(Run { id: q.id.clone(), idx: 0, picks: vec![None; q.questions.len()], cursor: 0, answered: false });
        }
    }

    fn pick(&mut self, opt: usize) {
        let View::Quiz(r) = &mut self.view else { return };
        let Some(q) = self.quizzes.iter().find(|q| q.id == r.id) else { return };
        if r.answered || opt >= q.questions[r.idx].options.len() {
            return;
        }
        r.picks[r.idx] = Some(opt);
        r.cursor = opt;
        r.answered = true;
        self.list.to_top();
    }

    fn next(&mut self, cx: &dyn LearnPort) {
        let View::Quiz(r) = &mut self.view else { return };
        if !r.answered {
            return;
        }
        let Some(total) = self.quizzes.iter().find(|q| q.id == r.id).map(|q| q.questions.len()) else { return };
        self.list.to_top();
        if r.idx + 1 < total {
            r.idx += 1;
            r.cursor = 0;
            r.answered = false;
            return;
        }
        let (id, picks) = (r.id.clone(), r.picks.clone());
        match cx.record_attempt(&self.user, &id, &picks) {
            Ok(_) => {
                self.reload(cx);
                self.view = View::Result(id, 0);
            }
            Err(e) => self.say(&e, false),
        }
    }

    /// Report (or take back the report of) question `q` of quiz `id`.
    fn report(&mut self, cx: &dyn LearnPort, id: &str, q: usize) {
        let now = self.quiz(id).and_then(|x| x.questions.get(q)).map(|x| x.flagged);
        let Some(was) = now else { return };
        match cx.flag_question(&self.user, id, q, !was) {
            Ok(()) => {
                self.reload(cx);
                self.say(if was { "Report taken back: the question counts again." } else { "Reported. This question no longer counts in the score." }, true);
            }
            Err(e) => self.say(&e, false),
        }
    }

    /// Write the open plan, quiz or lesson (or the chosen one in a list) as a PDF in the person's Downloads, with the same code the chat's
    /// `study_export_pdf` uses: nothing is replaced, other scripts are refused in words.
    pub fn export_pdf(&mut self, cx: &dyn LearnPort) {
        use iyakki_study_model::studypdf::Item;
        let item = match &self.view {
            View::Plan(id) => Some(Item::Plan(id.clone())),
            View::Lesson(l) if l.saved.is_some() => Some(Item::Lesson { plan: l.plan.clone(), n: l.n }),
            View::Lesson(_) => None,
            View::Result(id, _) => Some(Item::Quiz { id: id.clone(), key: self.pdf_key }),
            // while a quiz is being taken the key must not leave the page
            View::Quiz(_) => None,
            View::List if self.tab == Tab::Plans => self.plans.get(self.sel[0]).map(|p| Item::Plan(p.id.clone())),
            View::List => self.quizzes.get(self.sel[1]).map(|q| Item::Quiz { id: q.id.clone(), key: self.pdf_key }),
        };
        let Some(item) = item else {
            self.say("There is nothing to put in a PDF here yet (a lesson must be written first).", false);
            return;
        };
        match cx.export_pdf(&self.user, &item) {
            Ok(m) => self.say(&m, true),
            Err(e) => self.say(&e, false),
        }
    }

    fn toggle_session(&mut self, cx: &dyn LearnPort, i: usize) {
        let View::Plan(id) = &self.view else { return };
        let id = id.clone();
        let Some(done) = self.plan(&id).and_then(|p| p.sessions.get(i)).map(|s| s.done) else { return };
        self.sel_session = i;
        if let Err(e) = cx.set_done(&self.user, &id, i + 1, !done) {
            self.say(&e, false);
        }
        self.reload(cx);
    }

    fn calendar(&mut self, cx: &dyn LearnPort, add: bool) {
        let View::Plan(id) = &self.view else { return };
        let id = id.clone();
        let r = if add { cx.to_calendar(&self.user, &id).map(|n| format!("Added {n} sessions to the calendar.")) } else { cx.calendar_remove(&self.user, &id).map(|n| format!("Removed {n} events from the calendar.")) };
        match r {
            Ok(m) => self.say(&m, true),
            Err(e) => self.say(&e, false),
        }
        self.reload(cx);
    }

    fn delete_open(&mut self, cx: &dyn LearnPort) {
        self.confirm_delete = false;
        let r = match (&self.view, self.tab) {
            (View::Plan(id), _) => cx.delete_plan(&self.user, id).map(|_| "Plan deleted."),
            (_, Tab::Quizzes) => match self.quizzes.get(self.sel[1]) {
                Some(q) => cx.delete_quiz(&self.user, &q.id).map(|_| "Quiz deleted."),
                None => return,
            },
            (_, Tab::Plans) => match self.plans.get(self.sel[0]) {
                Some(p) => cx.delete_plan(&self.user, &p.id).map(|_| "Plan deleted."),
                None => return,
            },
        };
        match r {
            Ok(m) => {
                self.view = View::List;
                self.reload(cx);
                self.say(m, true);
            }
            Err(e) => self.say(&e, false),
        }
    }

    fn back(&mut self) -> Flow {
        self.confirm_delete = false;
        if let View::Lesson(l) = &mut self.view {
            // one Esc undoes the smallest thing open on the page, the last one leaves the lesson for the plan
            if l.input.is_some() {
                l.input = None;
                return Flow::Stay;
            }
            if l.confirm_regen {
                l.confirm_regen = false;
                return Flow::Stay;
            }
            self.leave_lesson();
            return Flow::Stay;
        }
        self.list.to_top();
        match self.view {
            View::List => Flow::Back,
            _ => {
                self.view = View::List;
                Flow::Stay
            }
        }
    }

    fn key(&mut self, k: &Key, cx: &dyn LearnPort) {
        let t = self.tab_ix();
        let n_list = if self.tab == Tab::Plans { self.plans.len() } else { self.quizzes.len() };
        // what the open view holds, copied out so the keys can change the app freely
        enum V {
            List,
            Plan(String, usize),
            Quiz(String, usize, usize, bool, usize),
            Result(String, usize, usize),
            Lesson,
        }
        let v = match &self.view {
            View::List => V::List,
            View::Plan(id) => V::Plan(id.clone(), self.plan(id).map_or(0, |p| p.sessions.len())),
            View::Quiz(r) => V::Quiz(r.id.clone(), r.idx, r.cursor, r.answered, self.quiz(&r.id).map_or(0, |q| q.questions[r.idx].options.len())),
            View::Result(id, sel) => V::Result(id.clone(), *sel, self.quiz(id).map_or(0, |q| q.questions.len())),
            View::Lesson(_) => V::Lesson,
        };
        match v {
            V::List => match k {
                Key::Left => self.set_tab(Tab::Plans),
                Key::Right => self.set_tab(Tab::Quizzes),
                Key::Up => self.sel[t] = self.sel[t].saturating_sub(1),
                Key::Down => self.sel[t] = (self.sel[t] + 1).min(n_list.saturating_sub(1)),
                Key::Enter => self.open_selected(),
                Key::Char('p') if n_list > 0 => self.export_pdf(cx),
                Key::Char('k') if self.tab == Tab::Quizzes => self.handle_pdf_key(),
                Key::Char('x') if n_list > 0 => {
                    if self.confirm_delete {
                        self.delete_open(cx)
                    } else {
                        self.confirm_delete = true
                    }
                }
                other => {
                    self.list.key(other);
                }
            },
            V::Lesson => self.lesson_key(k, cx),
            V::Plan(id, n) => match k {
                Key::Up => self.sel_session = self.sel_session.saturating_sub(1),
                Key::Down => self.sel_session = (self.sel_session + 1).min(n.saturating_sub(1)),
                Key::Char(' ') => self.toggle_session(cx, self.sel_session),
                Key::Char('p') => self.export_pdf(cx),
                Key::Enter => self.open_lesson(cx, &id, self.sel_session + 1),
                Key::Char('c') => {
                    let has = self.plan(&id).is_some_and(|p| !p.events.is_empty());
                    self.calendar(cx, !has);
                }
                Key::Char('x') => {
                    if self.confirm_delete {
                        self.delete_open(cx)
                    } else {
                        self.confirm_delete = true
                    }
                }
                other => {
                    self.list.key(other);
                }
            },
            V::Quiz(id, idx, cursor, answered, n_opts) => match k {
                Key::Up if !answered => self.set_cursor(cursor.saturating_sub(1)),
                Key::Down if !answered => self.set_cursor((cursor + 1).min(n_opts.saturating_sub(1))),
                Key::Enter if !answered => self.pick(cursor),
                Key::Enter | Key::Right | Key::Char('n') if answered => self.next(cx),
                Key::Char('r') if answered => self.report(cx, &id, idx),
                Key::Char(c) if !answered => {
                    let o = match c.to_ascii_lowercase() {
                        d @ '1'..='6' => d as usize - '1' as usize,
                        l @ 'a'..='f' => l as usize - 'a' as usize,
                        _ => return,
                    };
                    self.pick(o);
                }
                _ => {}
            },
            V::Result(id, sel, n) => match k {
                Key::Up => self.set_result_sel(sel.saturating_sub(1)),
                Key::Down => self.set_result_sel((sel + 1).min(n.saturating_sub(1))),
                Key::Char('r') => self.report(cx, &id, sel),
                Key::Char('p') => self.export_pdf(cx),
                Key::Char('k') => self.handle_pdf_key(),
                Key::Char('a') => {
                    if let Some(i) = self.quizzes.iter().position(|q| q.id == id) {
                        self.sel[1] = i;
                    }
                    self.start_quiz();
                }
                Key::Enter => self.view = View::List,
                other => {
                    self.list.key(other);
                }
            },
        }
    }

    fn handle_pdf_key(&mut self) {
        self.pdf_key = !self.pdf_key;
        self.say(if self.pdf_key { "Quiz PDFs will have the answer key on a page of their own." } else { "Quiz PDFs will not show the answers." }, true);
    }

    fn set_cursor(&mut self, c: usize) {
        if let View::Quiz(r) = &mut self.view {
            r.cursor = c;
        }
    }

    fn set_result_sel(&mut self, c: usize) {
        if let View::Result(_, sel) = &mut self.view {
            *sel = c;
        }
    }

    /// The percent line of a quiz's latest attempt, for the lists.
    pub fn last_line(q: &Quiz) -> String {
        match q.attempts.last() {
            Some(a) => {
                let s = score(q, &a.answers);
                format!("last score {} of {} = {}% ({} attempt{})", s.correct, s.counted, s.percent, q.attempts.len(), if q.attempts.len() == 1 { "" } else { "s" })
            }
            None => "not taken yet".into(),
        }
    }
}

impl LearnApp {
    pub fn title(&self) -> &'static str {
        "Learn"
    }
    pub fn page(&self) -> String {
        match &self.view {
            View::List => if self.tab == Tab::Plans { "plans" } else { "quizzes" }.into(),
            View::Plan(_) => "plan".into(),
            View::Quiz(_) => "quiz".into(),
            View::Result(..) => "result".into(),
            View::Lesson(l) if l.input.is_some() => "lesson-ask".into(),
            View::Lesson(l) if l.confirm_regen => "lesson-confirm".into(),
            View::Lesson(l) if l.fail.is_some() => "lesson-failed".into(),
            View::Lesson(l) if l.writing() => "lesson-writing".into(),
            View::Lesson(_) => "lesson".into(),
        }
    }
    pub fn icon(&self) -> Icon {
        Icon::Learn
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match &self.view {
            View::List => "←→ tab · ↑↓ choose · Enter open · x delete · Esc back".into(),
            View::Plan(_) => "↑↓ choose · Enter or click: lesson · Space tick · c calendar · x delete · Esc back".into(),
            View::Lesson(l) if l.input.is_some() => "type your question · Enter send · Esc cancel".into(),
            View::Lesson(l) if l.generating() => "←→ sections · Space day done · c cancel · Esc back".into(),
            View::Lesson(_) => "←→ sections · Space done · q ask · s simpler · d deeper · g again · r read · Esc back".into(),
            View::Quiz(r) if r.answered => "Enter next · r report a wrong question · Esc stop".into(),
            View::Quiz(_) => "a-d or 1-4 pick · ↑↓ Enter · Esc stop".into(),
            View::Result(..) => "↑↓ choose · r report · a again · Enter done".into(),
        }
    }
    pub fn help(&self) -> Vec<String> {
        [
            "LEARN holds your study plans and quizzes. Ask the assistant in the chat to make them (\"plan 21 days to learn programming basics\", \"quiz me with 10 questions on the solar system\"); they appear here, and what you tick or answer here the chat knows too.",
            "Plans: Enter opens a plan. p saves the plan (syllabus and day list) as a PDF in Downloads. In a plan, Enter or a click on a day opens its lesson; Space ticks a session done. c adds the whole plan to the calendar, or takes it out again. x twice deletes it and its calendar events.",
            "Quizzes: Enter starts. Pick an option with a to f, 1 to 6, the arrows and Enter, or a click. You see right or wrong and why, then Next. The answers are not shown before you answer.",
            "A lesson: the assistant writes the day's lesson as soon as you open it (what you will learn, an explanation, a worked example, what to do, a practice task, a recap) with pictures, and keeps it, so it opens at once next time. Left and Right move between sections, Space marks the day done, q asks a question, s makes it simpler, d goes deeper, g writes it again (it asks first), r reads it aloud, c cancels the writing, Esc goes back to the plan.",
            "The score is worked out by the program, not by the assistant. If a question or its answer looks wrong, press r (Report a wrong question): it stops counting, and the percent is worked out over the rest.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn handle(&mut self, ev: &Event, cx: &dyn LearnPort) -> Flow {
        let flow = self.handle_event(ev, cx);
        self.settle_voice(cx);
        flow
    }

    /// Stop the voice when something asked for it where the port was not at hand.
    fn settle_voice(&mut self, cx: &dyn LearnPort) {
        if std::mem::take(&mut self.stop_reading) {
            cx.stop_reading();
        }
    }

    fn handle_event(&mut self, ev: &Event, cx: &dyn LearnPort) -> Flow {
        self.dirty = true;
        match ev {
            Event::Key(Key::Esc) => return self.back(),
            ev if self.lesson_state().is_some() && self.lesson_event(ev, cx) => {}
            Event::Tab(T_TICK) => {
                if matches!(self.view, View::Plan(_)) {
                    self.toggle_session(cx, self.sel_session);
                }
            }
            Event::Tab(T_OPEN_LESSON) => {
                if let View::Plan(id) = &self.view {
                    let id = id.clone();
                    self.open_lesson(cx, &id, self.sel_session + 1);
                }
            }
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_PLANS) => self.set_tab(Tab::Plans),
            Event::Tab(T_QUIZZES) => self.set_tab(Tab::Quizzes),
            Event::Tab(T_BACK) | Event::Tab(T_STOP) | Event::Tab(T_NO) => {
                self.confirm_delete = false;
                if !matches!(ev, Event::Tab(T_NO)) {
                    return self.back();
                }
            }
            Event::Tab(T_PDF) => self.export_pdf(cx),
            Event::Tab(T_PDF_KEY) => {
                self.pdf_key = !self.pdf_key;
                self.say(if self.pdf_key { "Quiz PDFs will have the answer key on a page of their own." } else { "Quiz PDFs will not show the answers." }, true);
            }
            Event::Tab(T_CAL) => self.calendar(cx, true),
            Event::Tab(T_UNCAL) => self.calendar(cx, false),
            Event::Tab(T_DELETE) => self.confirm_delete = true,
            Event::Tab(T_YES) => self.delete_open(cx),
            Event::Tab(T_START) | Event::Tab(T_AGAIN) => {
                if let View::Result(id, _) = &self.view {
                    if let Some(i) = self.quizzes.iter().position(|q| &q.id == id) {
                        self.sel[1] = i;
                    }
                }
                self.start_quiz();
            }
            Event::Tab(T_NEXT) => self.next(cx),
            Event::Tab(T_REPORT) => match &self.view {
                View::Quiz(r) => {
                    let (id, idx) = (r.id.clone(), r.idx);
                    self.report(cx, &id, idx);
                }
                View::Result(id, sel) => {
                    let (id, sel) = (id.clone(), *sel);
                    self.report(cx, &id, sel);
                }
                _ => {}
            },
            Event::Tab(n) if *n >= T_OPTION => self.pick(n - T_OPTION),
            Event::Row(i) => match &mut self.view {
                View::List => {
                    let t = self.tab_ix();
                    self.sel[t] = *i;
                    self.open_selected();
                }
                View::Plan(id) => {
                    let (id, i) = (id.clone(), *i);
                    self.open_lesson(cx, &id, i + 1);
                }
                View::Result(_, sel) => *sel = *i,
                View::Quiz(_) | View::Lesson(_) => {}
            },
            Event::Key(k) => self.key(k, cx),
            other => {
                self.list.event(other, A_LIST);
            }
        }
        Flow::Stay
    }

    pub fn open_item(&mut self, item: &str, cx: &dyn LearnPort) {
        self.reload(cx);
        self.confirm_delete = false;
        self.list.to_top();
        if let Some(rest) = item.strip_prefix("lesson:") {
            // lesson:<plan>:<day>
            self.tab = Tab::Plans;
            self.view = View::List;
            if let Some((id, n)) = rest.rsplit_once(':') {
                if let (Some(i), Ok(n)) = (self.plans.iter().position(|p| p.id == id), n.parse::<usize>()) {
                    self.sel[0] = i;
                    self.open_lesson(cx, id, n);
                }
            }
        } else if let Some(id) = item.strip_prefix("quiz:") {
            self.tab = Tab::Quizzes;
            self.view = View::List;
            if let Some(i) = self.quizzes.iter().position(|q| q.id == id) {
                self.sel[1] = i;
            }
        } else if let Some(id) = item.strip_prefix("plan:") {
            self.tab = Tab::Plans;
            self.view = View::List;
            if let Some(i) = self.plans.iter().position(|p| p.id == id) {
                self.sel[0] = i;
                self.open_selected();
            }
        }
        self.dirty = true;
    }

    pub fn scroll_px(&mut self, px: i32, _cx: &dyn LearnPort) -> bool {
        self.list.wheel_px(px)
    }
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    pub fn wants_redraw(&mut self) -> bool {
        // a lesson being written, or read aloud (as of the last frame), moves by itself
        let busy = self.lesson_state().is_some_and(|l| l.generating()) || self.voice_on;
        std::mem::take(&mut self.dirty) || busy
    }
    pub fn idle_redraw_ms(&self) -> u64 {
        500
    }
    pub fn render(&mut self, g: &mut iyakki_uikit::gfx::Gfx, area: iyakki_uikit::ui::R, s: f32, cx: &dyn LearnPort, tick: u64) {
        // the chat may have changed the store; look again about every two seconds (a question in progress keeps its own picks)
        self.pump_lesson(cx);
        self.settle_voice(cx);
        self.voice_on = cx.reading();
        if tick.saturating_sub(self.loaded_at) >= 60 {
            self.loaded_at = tick;
            self.reload(cx);
        }
        self.draw(g, area, s);
    }
}
