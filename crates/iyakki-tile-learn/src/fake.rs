//! A fake [`LearnPort`] for the tests of this crate: the plans, quizzes and lessons live in memory, every call is logged, and the lesson
//! writer, the voice and the picture drawer are scripted. No chat backend, no file, no sound, no thread that waits on a clock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use iyakki_study_model::study::{Attempt, Plan, Question, Quiz, Session};
use iyakki_study_model::studylesson::{Lesson, Parsed, Picture, Qa, Section};
use iyakki_study_model::studypdf::Item;

use crate::port::{LearnPort, LessonWriter, Pictures, WriteEvent};

#[derive(Default)]
pub struct State {
    pub plans: Vec<Plan>,
    pub quizzes: Vec<Quiz>,
    pub lessons: HashMap<(String, usize), Lesson>,
    pub images: HashMap<(String, usize, String), Vec<u8>>,
    pub log: Vec<String>,
    /// When set, every call that changes something answers this sentence.
    pub refuse: Option<String>,
    pub reading: bool,
    /// When set, reading aloud answers this sentence (no voice is set up).
    pub no_voice: Option<String>,
    pub pdf_answer: Option<Result<String, String>>,
}

pub struct FakeWriter {
    pub st: Arc<Mutex<State>>,
    /// What `generate` answers (a lesson, or the sentence of a failure).
    pub result: Mutex<Result<Lesson, String>>,
    /// When set, `generate` waits here for a message after its first event (the page is then mid-run).
    pub gate: Mutex<Option<Receiver<()>>>,
    pub answer: Mutex<Result<String, String>>,
    pub stop_seen: AtomicBool,
}

impl LessonWriter for FakeWriter {
    fn generate(&self, user: &str, plan: &Plan, n: usize, level: i32, on: &mut dyn FnMut(WriteEvent), stop: &AtomicBool) -> Result<Lesson, String> {
        self.st.lock().unwrap().log.push(format!("generate {user} {} {n} {level}", plan.id));
        on(WriteEvent::Phase("Writing the lesson".into()));
        let r = self.result.lock().unwrap().clone();
        if let Ok(l) = &r {
            on(WriteEvent::Text(Parsed { sections: l.sections.iter().take(1).cloned().collect(), pictures: vec![] }));
        }
        let gate = self.gate.lock().unwrap().take();
        if let Some(g) = gate {
            let _ = g.recv();
        }
        if stop.load(Ordering::SeqCst) {
            self.stop_seen.store(true, Ordering::SeqCst);
            return Err("Cancelled.".into());
        }
        if let Ok(l) = &r {
            for (i, p) in l.pictures.iter().enumerate() {
                on(WriteEvent::Pic(i, p.file.clone().into_bytes()));
            }
            let mut st = self.st.lock().unwrap();
            st.lessons.insert((plan.id.clone(), n), l.clone());
            for p in &l.pictures {
                st.images.insert((plan.id.clone(), n, p.file.clone()), p.file.clone().into_bytes());
            }
        }
        r
    }

    fn ask(&self, user: &str, plan: &Plan, lesson: &Lesson, question: &str, stop: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
        self.st.lock().unwrap().log.push(format!("ask {user} {} {question}", plan.id));
        let a = self.answer.lock().unwrap().clone();
        if stop.load(Ordering::SeqCst) {
            return Err("Cancelled.".into());
        }
        if let Ok(a) = &a {
            on_text(a);
            let mut st = self.st.lock().unwrap();
            if let Some(l) = st.lessons.get_mut(&(plan.id.clone(), lesson.session)) {
                l.qa.push(Qa { q: question.to_string(), a: a.clone() });
            }
        }
        a
    }
}

pub struct FakePort {
    pub st: Arc<Mutex<State>>,
    pub writer: Arc<FakeWriter>,
}

impl FakePort {
    pub fn new() -> FakePort {
        let st = Arc::new(Mutex::new(State::default()));
        let writer = Arc::new(FakeWriter { st: st.clone(), result: Mutex::new(Ok(lesson("p1", 1))), gate: Mutex::new(None), answer: Mutex::new(Ok("Because the loop counts for you.".into())), stop_seen: AtomicBool::new(false) });
        FakePort { st, writer }
    }
    pub fn calls(&self) -> Vec<String> {
        self.st.lock().unwrap().log.clone()
    }
    pub fn with_plan(self, sessions: usize) -> FakePort {
        self.st.lock().unwrap().plans.push(plan("p1", sessions));
        self
    }
    pub fn with_quiz(self) -> FakePort {
        self.st.lock().unwrap().quizzes.push(quiz("q1"));
        self
    }
    fn change(&self, what: String) -> Result<(), String> {
        let mut st = self.st.lock().unwrap();
        if let Some(e) = &st.refuse {
            return Err(e.clone());
        }
        st.log.push(what);
        Ok(())
    }
}

pub fn plan(id: &str, n: usize) -> Plan {
    Plan {
        id: id.into(),
        topic: "Programming basics".into(),
        created: 0,
        unit: "days".into(),
        count: n as u32,
        every: "daily".into(),
        sessions: (1..=n).map(|i| Session { date: format!("2030-05-{:02}", 5 + i), time: "18:00".into(), minutes: 30, title: format!("Day {i} concept"), notes: format!("practice {i}"), done: false }).collect(),
        events: vec![],
    }
}

pub fn quiz(id: &str) -> Quiz {
    let q = |t: &str, o: [&str; 3], a: usize, why: &str| Question { text: t.into(), options: o.iter().map(|s| s.to_string()).collect(), answer: a, explanation: why.into(), flagged: false };
    Quiz { id: id.into(), topic: "Solar system".into(), created: 0, questions: vec![q("Which planet is closest to the Sun?", ["Mercury", "Venus", "Earth"], 0, "Mercury orbits closest."), q("Which is the largest planet?", ["Earth", "Saturn", "Jupiter"], 2, "Jupiter is the largest."), q("How many moons does Earth have?", ["Two", "None", "One"], 2, "Earth has one moon.")], attempts: vec![], progress: vec![] }
}

pub fn lesson(plan: &str, n: usize) -> Lesson {
    let sec = |t: &str, x: &str| Section { title: t.into(), text: x.into(), pics: vec![] };
    Lesson {
        plan: plan.into(),
        session: n,
        level: 0,
        created: 0,
        sections: vec![Section { pics: vec![0], ..sec("Explanation", "A loop repeats the same work many times.") }, sec("Worked example", "Count from one to three with one loop."), sec("What to do", "1. Open the editor."), sec("Practice task", "Print the numbers one to five."), sec("Recap", "A loop repeats.")],
        pictures: vec![Picture { caption: "A loop of arrows".into(), brief: "arrows going round".into(), file: "pic1.svg".into(), how: "placeholder".into() }],
        qa: vec![],
    }
}

impl LearnPort for FakePort {
    fn plans(&self, _user: &str) -> Vec<Plan> {
        self.st.lock().unwrap().plans.clone()
    }
    fn quizzes(&self, _user: &str) -> Vec<Quiz> {
        self.st.lock().unwrap().quizzes.clone()
    }
    fn record_attempt(&self, _user: &str, quiz: &str, picks: &[Option<usize>]) -> Result<(), String> {
        self.change(format!("attempt {quiz} {picks:?}"))?;
        let mut st = self.st.lock().unwrap();
        let taken = st.quizzes.iter().map(|q| q.attempts.len() as i64).sum::<i64>() + 1;
        if let Some(q) = st.quizzes.iter_mut().find(|q| q.id == quiz) {
            q.attempts.push(Attempt { taken, answers: picks.to_vec() });
        }
        Ok(())
    }
    fn flag_question(&self, _user: &str, quiz: &str, q: usize, flagged: bool) -> Result<(), String> {
        self.change(format!("flag {quiz} {q} {flagged}"))?;
        if let Some(x) = self.st.lock().unwrap().quizzes.iter_mut().find(|x| x.id == quiz).and_then(|x| x.questions.get_mut(q)) {
            x.flagged = flagged;
        }
        Ok(())
    }
    fn set_done(&self, _user: &str, plan: &str, n: usize, done: bool) -> Result<(), String> {
        self.change(format!("done {plan} {n} {done}"))?;
        if let Some(s) = self.st.lock().unwrap().plans.iter_mut().find(|p| p.id == plan).and_then(|p| p.sessions.get_mut(n - 1)) {
            s.done = done;
        }
        Ok(())
    }
    fn delete_quiz(&self, _user: &str, id: &str) -> Result<(), String> {
        self.change(format!("delete-quiz {id}"))?;
        self.st.lock().unwrap().quizzes.retain(|q| q.id != id);
        Ok(())
    }
    fn delete_plan(&self, _user: &str, id: &str) -> Result<(), String> {
        self.change(format!("delete-plan {id}"))?;
        self.st.lock().unwrap().plans.retain(|p| p.id != id);
        Ok(())
    }
    fn to_calendar(&self, _user: &str, id: &str) -> Result<usize, String> {
        self.change(format!("calendar-add {id}"))?;
        let mut st = self.st.lock().unwrap();
        let p = st.plans.iter_mut().find(|p| p.id == id).ok_or("no such plan")?;
        p.events = (0..p.sessions.len()).map(|i| format!("e{i}")).collect();
        Ok(p.events.len())
    }
    fn calendar_remove(&self, _user: &str, id: &str) -> Result<usize, String> {
        self.change(format!("calendar-remove {id}"))?;
        let mut st = self.st.lock().unwrap();
        let p = st.plans.iter_mut().find(|p| p.id == id).ok_or("no such plan")?;
        Ok(std::mem::take(&mut p.events).len())
    }
    fn lesson(&self, _user: &str, plan: &str, n: usize) -> Option<Lesson> {
        self.st.lock().unwrap().lessons.get(&(plan.to_string(), n)).cloned()
    }
    fn lesson_image(&self, _user: &str, plan: &str, n: usize, file: &str) -> Option<Vec<u8>> {
        self.st.lock().unwrap().images.get(&(plan.to_string(), n, file.to_string())).cloned()
    }
    fn export_pdf(&self, _user: &str, item: &Item) -> Result<String, String> {
        let mut st = self.st.lock().unwrap();
        st.log.push(format!("pdf {item:?}"));
        st.pdf_answer.clone().unwrap_or_else(|| Ok("Saved the PDF in Downloads.".into()))
    }
    fn writer(&self) -> Arc<dyn LessonWriter> {
        self.writer.clone()
    }
    fn audit(&self, user: &str, action: &str, detail: &str) {
        self.st.lock().unwrap().log.push(format!("audit {user} {action} {detail}"));
    }
    fn read_aloud(&self, text: &str) -> Result<usize, String> {
        let mut st = self.st.lock().unwrap();
        if let Some(why) = &st.no_voice {
            return Err(why.clone());
        }
        st.reading = true;
        st.log.push(format!("read {} chars", text.chars().count()));
        Ok(2)
    }
    fn reading(&self) -> bool {
        self.st.lock().unwrap().reading
    }
    fn stop_reading(&self) {
        let mut st = self.st.lock().unwrap();
        st.reading = false;
        st.log.push("stop-reading".into());
    }
}

/// Pictures that draw a 4 x 4 square, counting the calls.
#[derive(Default)]
pub struct FakePictures {
    pub drawn: Arc<Mutex<usize>>,
    pub cleared: Arc<Mutex<usize>>,
}

impl Pictures for FakePictures {
    fn clear(&mut self) {
        *self.cleared.lock().unwrap() += 1;
    }
    fn draw(&mut self, _g: &mut iyakki_uikit::gfx::Gfx, _id: u64, bytes: &[u8], _max_w: f32, _s: f32) -> Result<Arc<tiny_skia::Pixmap>, String> {
        if bytes.is_empty() {
            return Err("this is not a picture".into());
        }
        *self.drawn.lock().unwrap() += 1;
        tiny_skia::Pixmap::new(4, 4).map(Arc::new).ok_or_else(|| "too small".to_string())
    }
}
