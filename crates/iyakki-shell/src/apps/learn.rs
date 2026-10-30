//! LEARN: the study plans and quizzes the chat makes (`docs/STUDY.md`). Two tabs, Plans and Quizzes, over the same store the chat's
//! `study_*` and `quiz_*` tools use (`iyakki_agent::study`): a plan made in the chat is here, and a session ticked here is seen by the chat.
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

use iyakki_agent::clock::{now_unix, Zone};
use iyakki_agent::study::{score, Plan, Quiz};

use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Icon};
use super::chatpic::PicCache;
use super::learn_lesson::*;
use iyakki_agent::story::Reader;
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key};

pub const ACCENT: [u8; 4] = rgb(255, 236, 70);

pub(super) const T_PLANS: usize = 1;
pub(super) const T_QUIZZES: usize = 2;
pub(super) const T_BACK: usize = 3;
pub(super) const T_CAL: usize = 4;
pub(super) const T_UNCAL: usize = 5;
pub(super) const T_DELETE: usize = 6;
pub(super) const T_YES: usize = 7;
pub(super) const T_NO: usize = 8;
pub(super) const T_START: usize = 9;
pub(super) const T_NEXT: usize = 10;
pub(super) const T_REPORT: usize = 11;
pub(super) const T_AGAIN: usize = 12;
pub(super) const T_STOP: usize = 13;
pub(super) const T_DISMISS: usize = 14;
/// Option `i` of the open question is `T_OPTION + i`.
pub(super) const T_OPTION: usize = 100;
pub(super) const A_LIST: usize = 61_330;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Plans,
    Quizzes,
}

/// A quiz being taken.
#[derive(Clone, Debug)]
pub(super) struct Run {
    pub id: String,
    pub idx: usize,
    pub picks: Vec<Option<usize>>,
    /// The highlighted option (keys Up / Down, Enter picks it).
    pub cursor: usize,
    /// The current question has been answered: the key and the explanation are shown.
    pub answered: bool,
}

pub(super) enum View {
    List,
    Plan(String),
    Quiz(Run),
    /// The result of the latest attempt at a quiz, with the row chosen for reporting.
    Result(String, usize),
    /// The lesson of one day of a plan.
    Lesson(Box<LessonState>),
}

pub struct LearnApp {
    pub(super) user: String,
    pub(super) tab: Tab,
    pub(super) view: View,
    pub(super) plans: Vec<Plan>,
    pub(super) quizzes: Vec<Quiz>,
    pub(super) sel: [usize; 2],
    pub(super) sel_session: usize,
    pub(super) confirm_delete: bool,
    pub(super) msg: Option<(String, bool)>,
    pub(super) list: ScrollList,
    pub(super) loaded_at: u64,
    pub(super) request: Option<(String, String)>,
    pub(super) dirty: bool,
    pub(super) pic_cache: PicCache,
    /// The picture of a lesson, scaled to the slot it is drawn in: (id, width, height, scale, pixels).
    pub(super) pic_scaled: Vec<(u64, i32, i32, i32, tiny_skia::Pixmap)>,
    pub(super) reader: Option<Reader>,
    #[cfg(test)]
    pub(super) test_voice: Option<(std::sync::Arc<dyn iyakki_agent::story::Synth>, std::sync::Arc<dyn iyakki_agent::story::Sink>)>,
}

impl LearnApp {
    pub fn new(cx: &AppCx) -> LearnApp {
        let mut a = LearnApp { user: cx.session.user.clone(), tab: Tab::Plans, view: View::List, plans: vec![], quizzes: vec![], sel: [0; 2], sel_session: 0, confirm_delete: false, msg: None, list: ScrollList::default(), loaded_at: 0, request: None, dirty: true, pic_cache: PicCache::default(), pic_scaled: Vec::new(), reader: None, #[cfg(test)] test_voice: None };
        a.reload(cx);
        a
    }

    /// Read the store again: the chat may have made or changed things since.
    pub(super) fn reload(&mut self, cx: &AppCx) {
        self.plans = cx.services.study.plans(&self.user);
        self.quizzes = cx.services.study.quizzes(&self.user);
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

    pub(super) fn say(&mut self, m: &str, ok: bool) {
        self.msg = Some((m.to_string(), ok));
    }

    pub(super) fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }

    pub(super) fn quiz(&self, id: &str) -> Option<&Quiz> {
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

    fn next(&mut self, cx: &AppCx) {
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
        match cx.services.study.record_attempt(&self.user, &id, &picks, now_unix()) {
            Ok(_) => {
                self.reload(cx);
                self.view = View::Result(id, 0);
            }
            Err(e) => self.say(&e, false),
        }
    }

    /// Report (or take back the report of) question `q` of quiz `id`.
    fn report(&mut self, cx: &AppCx, id: &str, q: usize) {
        let now = self.quiz(id).and_then(|x| x.questions.get(q)).map(|x| x.flagged);
        let Some(was) = now else { return };
        match cx.services.study.flag_question(&self.user, id, q, !was) {
            Ok(()) => {
                self.reload(cx);
                self.say(if was { "Report taken back: the question counts again." } else { "Reported. This question no longer counts in the score." }, true);
            }
            Err(e) => self.say(&e, false),
        }
    }

    fn toggle_session(&mut self, cx: &AppCx, i: usize) {
        let View::Plan(id) = &self.view else { return };
        let id = id.clone();
        let Some(done) = self.plan(&id).and_then(|p| p.sessions.get(i)).map(|s| s.done) else { return };
        self.sel_session = i;
        if let Err(e) = cx.services.study.set_done(&self.user, &id, i + 1, !done) {
            self.say(&e, false);
        }
        self.reload(cx);
    }

    fn calendar(&mut self, cx: &AppCx, add: bool) {
        let View::Plan(id) = &self.view else { return };
        let id = id.clone();
        let st = &cx.services.study;
        let r = if add { st.to_calendar(&self.user, &id, &cx.services.calendar, &Zone::system()).map(|n| format!("Added {n} sessions to the calendar.")) } else { st.calendar_remove(&self.user, &id, &cx.services.calendar).map(|n| format!("Removed {n} events from the calendar.")) };
        match r {
            Ok(m) => self.say(&m, true),
            Err(e) => self.say(&e, false),
        }
        self.reload(cx);
    }

    fn delete_open(&mut self, cx: &AppCx) {
        self.confirm_delete = false;
        let r = match (&self.view, self.tab) {
            (View::Plan(id), _) => cx.services.study.delete_plan(&self.user, id, &cx.services.calendar).map(|_| "Plan deleted."),
            (_, Tab::Quizzes) => match self.quizzes.get(self.sel[1]) {
                Some(q) => cx.services.study.delete_quiz(&self.user, &q.id).map(|_| "Quiz deleted."),
                None => return,
            },
            (_, Tab::Plans) => match self.plans.get(self.sel[0]) {
                Some(p) => cx.services.study.delete_plan(&self.user, &p.id, &cx.services.calendar).map(|_| "Plan deleted."),
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

    fn key(&mut self, k: &Key, cx: &AppCx) {
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
    pub(super) fn last_line(q: &Quiz) -> String {
        match q.attempts.last() {
            Some(a) => {
                let s = score(q, &a.answers);
                format!("last score {} of {} = {}% ({} attempt{})", s.correct, s.counted, s.percent, q.attempts.len(), if q.attempts.len() == 1 { "" } else { "s" })
            }
            None => "not taken yet".into(),
        }
    }
}

impl App for LearnApp {
    fn title(&self) -> &'static str {
        "Learn"
    }
    fn page(&self) -> String {
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
    fn icon(&self) -> Icon {
        Icon::Learn
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
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
    fn help(&self) -> Vec<String> {
        [
            "LEARN holds your study plans and quizzes. Ask the assistant in the chat to make them (\"plan 21 days to learn programming basics\", \"quiz me with 10 questions on the solar system\"); they appear here, and what you tick or answer here the chat knows too.",
            "Plans: Enter opens a plan. In a plan, Enter or a click on a day opens its lesson; Space ticks a session done. c adds the whole plan to the calendar, or takes it out again. x twice deletes it and its calendar events.",
            "Quizzes: Enter starts. Pick an option with a to f, 1 to 6, the arrows and Enter, or a click. You see right or wrong and why, then Next. The answers are not shown before you answer.",
            "A lesson: the assistant writes the day's lesson as soon as you open it (what you will learn, an explanation, a worked example, what to do, a practice task, a recap) with pictures, and keeps it, so it opens at once next time. Left and Right move between sections, Space marks the day done, q asks a question, s makes it simpler, d goes deeper, g writes it again (it asks first), r reads it aloud, c cancels the writing, Esc goes back to the plan.",
            "The score is worked out by the program, not by the assistant. If a question or its answer looks wrong, press r (Report a wrong question): it stops counting, and the percent is worked out over the rest.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
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

    fn open_item(&mut self, item: &str, cx: &AppCx) {
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

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.list.wheel_px(px)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn wants_redraw(&mut self) -> bool {
        // a lesson being written, or read aloud, moves by itself
        let busy = self.lesson_state().is_some_and(|l| l.generating()) || self.reading();
        std::mem::take(&mut self.dirty) || busy
    }
    fn idle_redraw_ms(&self) -> u64 {
        500
    }
    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, cx: &AppCx, tick: u64) {
        // the chat may have changed the store; look again about every two seconds (a question in progress keeps its own picks)
        self.pump_lesson(cx);
        if tick.saturating_sub(self.loaded_at) >= 60 {
            self.loaded_at = tick;
            self.reload(cx);
        }
        self.draw(g, area, s);
    }
}
