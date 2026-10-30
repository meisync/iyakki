//! The LEARN tile over a real store: what the chat's tools make shows here, what is done here shows to the chat, a whole quiz is taken with
//! keys and with clicks, the key stays hidden until the answer, reports leave the score, and every screen fits every size.

use serde_json::{json, Value};

use iyakki_agent::study_tools;

use super::learn::*;
use super::learn_lesson::*;
use super::tests::cx_with_admin_store;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

const USER: &str = "root";

struct Fx {
    app: LearnApp,
    cx: AppCx,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

impl Fx {
    fn new(tag: &str) -> Fx {
        let cx = cx_with_admin_store(&format!("learn-{tag}"));
        Fx::on(cx, 800, 480, 1.0)
    }
    fn on(cx: AppCx, w: u32, h: u32, s: f32) -> Fx {
        let app = LearnApp::new(&cx);
        let mut f = Fx { app, cx, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn ev(&mut self, e: Event) -> Flow {
        let r = self.app.handle(&e, &self.cx);
        self.draw();
        r
    }
    fn key(&mut self, k: Key) -> Flow {
        self.ev(Event::Key(k))
    }
    fn chr(&mut self, c: char) {
        self.key(Key::Char(c));
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn has_hit(&self, e: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == e)
    }
    /// What the chat would do: a tool call on the same services.
    fn chat(&self, tool: &str, args: Value) -> Result<String, String> {
        study_tools::call(&self.cx.services, USER, tool, &args)
    }
    fn reload(&mut self) {
        // the tile looks at the store again every couple of seconds
        self.tick += 100;
        self.draw();
    }
}

fn questions() -> Value {
    json!([
        {"question": "Which planet is closest to the Sun?", "options": ["Mercury", "Venus", "Earth", "Mars"], "answer": 0, "explanation": "Mercury orbits closest."},
        {"question": "Which is the largest planet?", "options": ["Earth", "Saturn", "Jupiter", "Neptune"], "answer": 2, "explanation": "Jupiter is the largest."},
        {"question": "How many moons does Earth have?", "options": ["Two", "None", "Three", "One"], "answer": 3, "explanation": "Earth has one moon."}
    ])
}

fn make_quiz(f: &Fx) -> String {
    let r = f.chat("quiz_create", json!({"topic": "Solar system", "count": 3, "questions": questions()})).unwrap();
    r.split(['[', ']']).nth(1).unwrap().to_string()
}

fn plan_args(n: usize) -> Value {
    json!({"topic": "Programming basics", "unit": "days", "count": n, "start": "2030-05-06", "sessions": (1..=n).map(|i| json!({"title": format!("Day {i} concept"), "notes": format!("practice {i}")})).collect::<Vec<_>>()})
}

fn make_plan(f: &Fx, n: usize) -> String {
    let r = f.chat("study_plan_create", plan_args(n)).unwrap();
    r.split(['[', ']']).nth(1).unwrap().to_string()
}

fn to_quizzes(f: &mut Fx) {
    f.reload();
    f.key(Key::Right);
}

#[test]
fn a_plan_the_chat_made_is_in_the_tile_and_a_session_ticked_in_the_tile_is_seen_by_the_chat() {
    let mut f = Fx::new("plan-both");
    assert!(f.text().contains("No study plans yet"), "an empty tile says how to make one");
    let id = make_plan(&f, 3);
    f.reload();
    assert_eq!(f.app.plans.len(), 1);
    let t = f.text();
    assert!(t.contains("Programming basics") && t.contains("0 of 3 done"), "{t}");
    f.key(Key::Enter);
    let t = f.text();
    assert!(t.contains("1. 2030-05-06 18:00 · Day 1 concept") && t.contains("Add to calendar"), "{t}");
    f.chr(' '); // tick the first session
    f.key(Key::Down);
    f.chr(' ');
    let listed = f.chat("study_plan_list", json!({"id": id})).unwrap();
    assert!(listed.contains("1. 2030-05-06 18:00 (30 min) [done]") && listed.contains("2. 2030-05-07 18:00 (30 min) [done]") && !listed.contains("3. 2030-05-08 18:00 (30 min) [done]"), "{listed}");
    // and the other way: the chat ticks the third, the tile sees it
    f.chat("study_plan_progress", json!({"id": id, "session": 3, "done": true})).unwrap();
    f.reload();
    assert!(f.text().contains("3 of 3 sessions done"), "{}", f.text());
    f.key(Key::Esc);
    assert!(f.text().contains("3 of 3 done · all done"), "{}", f.text());
}

#[test]
fn ticking_a_session_and_the_calendar_button_adds_and_removes_exactly_the_plans_events() {
    let mut f = Fx::new("plan-cal");
    let id = make_plan(&f, 4);
    f.reload();
    f.ev(Event::Row(0)); // opens the plan
    assert_eq!(f.app.page(), "plan");
    f.key(Key::Down);
    f.ev(Event::Tab(T_TICK));
    assert!(f.app.plan(&id).unwrap().sessions[1].done);
    f.ev(Event::Tab(T_TICK));
    assert!(!f.app.plan(&id).unwrap().sessions[1].done, "a second click takes it back");
    assert!(f.has_hit(&Event::Tab(T_CAL)) && !f.has_hit(&Event::Tab(T_UNCAL)));
    f.cx.services.calendar.add_event(USER, "Dentist", 1_900_000_000, 30, false, iyakki_agent::calendar::Repeat::None, "", "user", None).unwrap();
    f.ev(Event::Tab(T_CAL));
    assert_eq!(f.cx.services.calendar.events(USER).len(), 5);
    assert!(f.has_hit(&Event::Tab(T_UNCAL)) && !f.has_hit(&Event::Tab(T_CAL)), "once added, the button offers to remove");
    f.ev(Event::Tab(T_CAL)); // not offered any more; a stale click must not add twice
    assert_eq!(f.cx.services.calendar.events(USER).len(), 5, "added once");
    f.ev(Event::Tab(T_UNCAL));
    let left = f.cx.services.calendar.events(USER);
    assert_eq!(left.len(), 1, "only the plan's events went");
    assert_eq!(left[0].title, "Dentist");
    // x twice deletes the plan
    f.chr('c');
    assert_eq!(f.cx.services.calendar.events(USER).len(), 5);
    f.chr('x');
    assert!(f.text().contains("Delete it for good") && f.app.plan(&id).is_some(), "the first x only asks");
    f.chr('x');
    assert!(f.app.plan(&id).is_none() && f.cx.services.calendar.events(USER).len() == 1, "the plan and its events are gone");
    assert_eq!(f.app.page(), "plans");
    assert!(f.text().contains("Plan deleted"));
}

#[test]
fn a_whole_quiz_by_keys_hides_the_key_until_each_answer_and_the_code_scores_it() {
    let mut f = Fx::new("quiz-keys");
    let id = make_quiz(&f);
    to_quizzes(&mut f);
    let t = f.text();
    assert!(t.contains("Solar system") && t.contains("3 questions · not taken yet"), "{t}");
    f.key(Key::Enter);
    assert_eq!(f.app.page(), "quiz");
    // question 1: nothing of the answer is on screen
    let t = f.text();
    assert!(t.contains("question 1 of 3") && t.contains("Which planet is closest to the Sun?") && t.contains("A) Mercury") && t.contains("D) Mars"), "{t}");
    for hidden in ["Mercury orbits closest", "Right.", "Not quite", "The answer is"] {
        assert!(!t.contains(hidden), "{hidden} shown before the answer");
    }
    assert!(!f.has_hit(&Event::Tab(T_NEXT)) && !f.has_hit(&Event::Tab(T_REPORT)));
    f.chr('a'); // right
    let t = f.text();
    assert!(t.contains("Right.") && t.contains("Mercury orbits closest") && f.has_hit(&Event::Tab(T_NEXT)) && f.has_hit(&Event::Tab(T_REPORT)), "{t}");
    f.chr('b'); // a second pick on an answered question changes nothing
    assert!(f.text().contains("Right."));
    f.key(Key::Enter);
    // question 2: answered wrongly with the arrows and Enter (cursor to B, Saturn)
    let t = f.text();
    assert!(t.contains("question 2 of 3") && !t.contains("Jupiter is the largest"), "{t}");
    f.key(Key::Down);
    f.key(Key::Enter);
    let t = f.text();
    assert!(t.contains("Not quite. The answer is C) Jupiter") && t.contains("Jupiter is the largest."), "{t}");
    f.chr('n');
    // question 3: digits work too (4 = One, right) and the last button says Finish
    f.chr('4');
    assert!(f.has_hit(&Event::Tab(T_NEXT)) && f.text().contains("Finish"));
    assert!(f.app.quiz(&id).unwrap().attempts.is_empty(), "nothing is stored before the end");
    f.key(Key::Enter);
    assert_eq!(f.app.page(), "result");
    let t = f.text();
    assert!(t.contains("Solar system: 2 of 3 = 67%"), "{t}");
    assert!(t.contains("2. Wrong") && t.contains("You picked: B) Saturn"), "{t}");
    let q = f.cx.services.study.quiz(USER, &id).unwrap();
    assert_eq!(q.attempts.len(), 1);
    assert_eq!(q.attempts[0].answers, [Some(0), Some(1), Some(3)]);
    // the chat reads the same result
    let r = f.chat("quiz_results", json!({"id": id})).unwrap();
    assert!(r.contains("Score 2/3 = 67%") && r.contains("Wrong: Q2") && r.contains("picked: Saturn") && r.contains("correct: Jupiter"), "{r}");
    // Enter returns to the list which shows the score; 'a' from the result starts again from question 1
    f.chr('a');
    assert!(f.text().contains("question 1 of 3"));
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "quizzes");
    assert!(f.text().contains("last score 2 of 3 = 67% (1 attempt)"), "{}", f.text());
    assert!(matches!(f.key(Key::Esc), Flow::Back), "Esc on the list leaves the tile");
}

#[test]
fn a_quiz_by_clicks_and_a_reported_question_leaves_the_score() {
    let mut f = Fx::new("quiz-clicks");
    let id = make_quiz(&f);
    to_quizzes(&mut f);
    assert!(f.has_hit(&Event::Tab(T_START)));
    f.ev(Event::Tab(T_START));
    for k in 0..4 {
        assert!(f.has_hit(&Event::Tab(T_OPTION + k)), "option {k} is clickable");
    }
    assert!(!f.has_hit(&Event::Tab(T_OPTION + 4)));
    f.ev(Event::Tab(T_OPTION)); // right
    f.ev(Event::Tab(T_NEXT));
    f.ev(Event::Tab(T_OPTION)); // Earth: wrong
    assert!(f.text().contains("Report a wrong question"));
    f.ev(Event::Tab(T_REPORT)); // the person says question 2 is wrong
    assert!(f.app.quiz(&id).unwrap().questions[1].flagged && f.text().contains("Reported. This question no longer counts"), "{}", f.text());
    assert!(f.text().contains("Take back the report"));
    f.ev(Event::Tab(T_NEXT));
    f.ev(Event::Tab(T_OPTION + 3)); // right
    f.ev(Event::Tab(T_NEXT));
    // 2 right of the 2 that count: 100%, though the wrong pick of question 2 is stored
    assert!(f.text().contains("Solar system: 2 of 2 = 100%") && f.text().contains("1 reported question is left out"), "{}", f.text());
    assert!(f.text().contains("2. Reported"));
    // from the result: take the report back with the chosen row
    f.key(Key::Down);
    f.chr('r');
    assert!(!f.app.quiz(&id).unwrap().questions[1].flagged);
    assert!(f.text().contains("Solar system: 2 of 3 = 67%"), "{}", f.text());
    let r = f.chat("quiz_results", json!({"id": id})).unwrap();
    assert!(r.contains("Score 2/3 = 67%"), "{r}");
    f.chr('r'); // and report it again
    assert!(f.text().contains("2 of 2 = 100%"));
}

#[test]
fn a_quiz_the_chat_started_and_finished_is_scored_by_code_and_shows_in_the_tile() {
    let mut f = Fx::new("chat-quiz");
    let id = make_quiz(&f);
    let q1 = f.chat("quiz_next", json!({"id": id})).unwrap();
    assert!(q1.contains("Question 1 of 3") && q1.contains("A) Mercury") && !q1.contains("Mercury orbits"), "{q1}");
    assert!(f.chat("quiz_answer", json!({"id": id, "question": 2, "option": "A"})).unwrap_err().contains("next question to answer is 1"));
    let a = f.chat("quiz_answer", json!({"id": id, "question": 1, "option": "A"})).unwrap();
    assert!(a.starts_with("Right") && a.contains("Mercury orbits closest"), "{a}");
    f.chat("quiz_answer", json!({"id": id, "question": 2, "option": "C"})).unwrap();
    let last = f.chat("quiz_answer", json!({"id": id, "question": 3, "option": "A"})).unwrap();
    assert!(last.contains("FINISHED. Score 2/3 = 67%"), "{last}");
    to_quizzes(&mut f);
    assert!(f.text().contains("last score 2 of 3 = 67% (1 attempt)"), "{}", f.text());
    // the person reports a question in the chat; the tile's percent follows
    f.chat("quiz_report", json!({"id": id, "question": 3})).unwrap();
    f.reload();
    assert!(f.text().contains("last score 2 of 2 = 100%"), "{}", f.text());
}

#[test]
fn the_chat_can_open_the_tile_on_a_quiz_or_a_plan() {
    let mut f = Fx::new("open");
    let q = make_quiz(&f);
    let p = make_plan(&f, 2);
    let r = f.chat("quiz_open", json!({"id": q})).unwrap();
    assert!(r.contains("LEARN"), "{r}");
    let notes = f.cx.services.inbox.list(USER);
    let n = notes.iter().find(|n| n.title == "Quiz: Solar system").expect("the note");
    assert_eq!(n.link, Some(iyakki_agent::inbox::Link::new("learn", &format!("quiz:{q}"))));
    // the inbox hands the link's item to the tile
    f.reload();
    f.app.open_item(&format!("quiz:{q}"), &f.cx);
    f.draw();
    assert_eq!((f.app.page().as_str(), f.app.sel[1]), ("quizzes", 0));
    f.app.open_item(&format!("plan:{p}"), &f.cx);
    f.draw();
    assert_eq!(f.app.page(), "plan");
    assert!(f.text().contains("Programming basics"));
    f.app.open_item("quiz:nope", &f.cx); // an unknown item leaves the tile where it is, no panic
    f.draw();
    assert!(f.chat("quiz_open", json!({"id": "nope"})).is_err() && f.chat("quiz_open", json!({})).is_err());
}

#[test]
fn what_is_deleted_in_the_chat_leaves_the_tile_without_a_crash() {
    let mut f = Fx::new("gone");
    let id = make_quiz(&f);
    to_quizzes(&mut f);
    f.key(Key::Enter);
    f.chr('a');
    f.cx.services.study.delete_quiz(USER, &id).unwrap();
    f.chr('n'); // next on a quiz that no longer exists
    f.reload();
    f.reload();
    assert_eq!(f.app.page(), "quizzes");
    assert!(f.text().contains("No quizzes yet"));
}

#[test]
fn a_quiz_is_stopped_with_esc_and_leaves_no_attempt() {
    let mut f = Fx::new("stop");
    let id = make_quiz(&f);
    to_quizzes(&mut f);
    f.key(Key::Enter);
    f.chr('a');
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "quizzes");
    assert!(f.app.quiz(&id).unwrap().attempts.is_empty());
    f.key(Key::Enter);
    f.ev(Event::Tab(T_STOP));
    assert_eq!(f.app.page(), "quizzes");
    f.chr('x');
    assert!(f.text().contains("Delete it for good"));
    f.ev(Event::Tab(T_NO));
    assert!(f.app.quiz(&id).is_some() && !f.text().contains("Delete it for good"));
    f.chr('x');
    f.ev(Event::Tab(T_YES));
    assert!(f.app.quiz(&id).is_none());
}

/// Every screen of the tile at every size the overflow test uses (and the small text and big text scales): all clickable areas and all
/// text stay inside the window, wherever the content is scrolled.
#[test]
fn every_screen_fits_every_size() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1366, 768), (2560, 1600)] {
        for s in [1.0f32, 1.5, 2.0] {
            let cx = cx_with_admin_store(&format!("learn-fit-{w}-{}", (s * 10.0) as u32));
            let mut f = Fx::on(cx, w, h, s);
            let long = "A very long topic name that goes on and on about the unusual and difficult things one can study in a lifetime";
            let plan = json!({"topic": long, "unit": "days", "count": 12, "start": "2030-05-06", "sessions": (1..=12).map(|i| json!({"title": format!("Session {i}: a quite long title that has to wrap onto lines"), "notes": long})).collect::<Vec<_>>()});
            f.chat("study_plan_create", plan).unwrap();
            let qs: Vec<Value> = (0..6).map(|i| json!({"question": format!("{long}? ({i})"), "options": [format!("first {long}"), "second", format!("third {long}"), "fourth"], "answer": 2, "explanation": long})).collect();
            f.chat("quiz_create", json!({"topic": long, "count": 6, "questions": qs})).unwrap();
            f.reload();
            let mut shots: Vec<(&str, Vec<Event>)> = vec![("plans", vec![]), ("plan", vec![Event::Key(Key::Enter)])];
            shots.push(("confirm", vec![Event::Key(Key::Char('x'))]));
            shots.push(("quizzes", vec![Event::Key(Key::Esc), Event::Key(Key::Esc), Event::Tab(T_QUIZZES)]));
            shots.push(("question", vec![Event::Key(Key::Enter)]));
            shots.push(("answered", vec![Event::Key(Key::Char('a'))]));
            let mut seen = Vec::new();
            for (name, evs) in shots {
                for e in evs {
                    f.ev(e);
                }
                seen.push(name);
                check_fits(&mut f, &format!("{name} {w}x{h} s{s}"));
            }
            for _ in 0..5 {
                f.key(Key::Enter);
                f.chr('c');
                f.key(Key::Enter);
            }
            // the result screen
            for k in 0..6 {
                if f.app.page() == "quiz" {
                    f.chr('c');
                    f.key(Key::Enter);
                } else {
                    break;
                }
                let _ = k;
            }
            assert_eq!(f.app.page(), "result", "{w}x{h}");
            check_fits(&mut f, &format!("result {w}x{h} s{s}"));
            f.key(Key::Down);
            f.key(Key::Down);
            check_fits(&mut f, &format!("result scrolled {w}x{h} s{s}"));
            f.app.scroll_px(-2000, &f.cx);
            f.draw();
            check_fits(&mut f, &format!("result bottom {w}x{h} s{s}"));
            assert_eq!(seen.len(), 6);
        }
    }
}

fn check_fits(f: &mut Fx, what: &str) {
    let a = f.area;
    for h in &f.g.hits {
        let r = h.rect;
        assert!(r.w > 0.0 && r.h > 0.0, "{what}: {:?} is empty", h.label);
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: {:?} {:?} leaves {:?}", h.label, r, a);
    }
    for (_, r, _, t) in &f.g.layout_log.texts {
        assert!(r.x >= -0.5 && r.x + r.w <= a.w + 0.5, "{what}: the text {t:?} {r:?} leaves the window width {}", a.w);
    }
}

#[test]
fn an_empty_tile_and_a_tile_with_everything_in_the_first_frame_do_not_panic_at_the_smallest_size() {
    let cx = cx_with_admin_store("learn-empty");
    let mut f = Fx::on(cx, 360, 740, 2.0);
    f.key(Key::Right);
    f.key(Key::Enter);
    f.chr('x');
    f.key(Key::Left);
    f.key(Key::Enter);
    f.chr(' ');
    f.chr('c');
    assert!(matches!(f.app.items().first(), Some(super::learn_ui::Item::Text(t, _)) if t.starts_with("No study plans yet")), "an empty plan list says how to make one");
    assert!(f.app.help().len() >= 4 && !f.app.hint().is_empty());
}

#[test]
fn the_tile_answer_key_never_leaves_the_store_before_the_answer() {
    // the chat's next-question text and the tile's frame for an unanswered question carry no explanation and no marked option
    let mut f = Fx::new("nokey");
    let id = make_quiz(&f);
    to_quizzes(&mut f);
    f.key(Key::Enter);
    let t = f.text();
    for q in questions().as_array().unwrap().iter().take(1) {
        assert!(!t.contains(q["explanation"].as_str().unwrap()));
    }
    let next = f.chat("quiz_next", json!({"id": id})).unwrap();
    assert!(!next.contains("Mercury orbits") && !next.to_lowercase().contains("correct answer"));
}


// ------------------------------------------------------------------------------------------------------------ the lesson page

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, ToolSpec};

const LESSON: &str = "## Explanation\nA loop repeats the same work many times, so you do not write it again and again.\n[PICTURE: A loop of arrows | arrows going round in a circle]\n## Worked example\nCount from one to three: print 1, print 2, print 3 with one loop.\n## What to do\n1. Open the editor.\n2. Type the loop.\n3. Run it and read the output.\n[PICTURE: The editor | a window with three lines]\n## Practice task\nPrint the numbers one to five.\n## Recap\nA loop repeats. Count the rounds. Stop when done.";
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 400 240\"><rect width=\"400\" height=\"240\" fill=\"#ccddee\"/><circle cx=\"200\" cy=\"120\" r=\"50\" fill=\"#2255aa\"/></svg>";

/// A model that teaches: it answers the lesson, the drawing and the question prompts, counts its calls, can fail, and can be held half way.
struct Teacher {
    calls: AtomicUsize,
    lessons: AtomicUsize,
    fail: Mutex<Option<String>>,
    lesson: Mutex<String>,
    /// When set, the lesson stops after its first line and waits here for a message.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    last_system: Mutex<String>,
}

impl Teacher {
    fn new() -> Arc<Teacher> {
        Arc::new(Teacher { calls: AtomicUsize::new(0), lessons: AtomicUsize::new(0), fail: Mutex::new(None), lesson: Mutex::new(LESSON.to_string()), gate: Mutex::new(None), last_system: Mutex::new(String::new()) })
    }
}

struct Shared(Arc<Teacher>);

impl ChatBackend for Shared {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        let t = &self.0;
        t.calls.fetch_add(1, Ordering::SeqCst);
        let sys = h.first().map(|m| m.content.clone()).unwrap_or_default();
        if let Some(e) = t.fail.lock().unwrap().clone() {
            return Err(e);
        }
        if sys.contains("teaching illustration") {
            return Ok(SVG.to_string());
        }
        if sys.contains("answer a learner's question") {
            return Ok("Because the loop counts for you.".into());
        }
        *t.last_system.lock().unwrap() = sys;
        t.lessons.fetch_add(1, Ordering::SeqCst);
        Ok(t.lesson.lock().unwrap().clone())
    }
    fn complete_tools_stream(&self, h: &[Message], _tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let text = self.complete(h)?;
        let is_lesson = h.first().is_some_and(|m| m.content.contains("ONE day's lesson"));
        if is_lesson {
            if let Some(gate) = self.0.gate.lock().unwrap().take() {
                let cut = text.find("## Worked").unwrap_or(text.len());
                if !on_text(&text[..cut]) {
                    return Ok(ModelTurn { text: String::new(), calls: vec![] });
                }
                let _ = gate.recv();
                if !on_text(&text[cut..]) {
                    return Ok(ModelTurn { text: String::new(), calls: vec![] });
                }
                return Ok(ModelTurn { text, calls: vec![] });
            }
        }
        on_text(&text);
        Ok(ModelTurn { text, calls: vec![] })
    }
}

fn teach_fx(tag: &str, w: u32, h: u32, s: f32) -> (Fx, Arc<Teacher>) {
    let t = Teacher::new();
    let mut cx = cx_with_admin_store(&format!("learn-{tag}"));
    cx.chat = Arc::new(Shared(t.clone()));
    (Fx::on(cx, w, h, s), t)
}

/// Draw until the background run is over.
fn settle(f: &mut Fx) {
    for _ in 0..600 {
        f.draw();
        if !f.app.lesson_state().is_some_and(|l| l.generating()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the lesson run did not finish");
}

fn until_text(f: &mut Fx, needle: &str) {
    for _ in 0..600 {
        f.draw();
        if f.text().contains(needle) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("never saw {needle:?} in {}", f.text());
}

fn open_day(f: &mut Fx, n: usize) -> String {
    let id = make_plan(f, 4);
    f.reload();
    f.key(Key::Enter); // open the plan
    f.ev(Event::Row(n - 1)); // click the day
    id
}

#[test]
fn opening_a_session_starts_the_lesson_at_once_with_the_goals_in_the_first_frame_and_the_teaching_streams_in() {
    let (mut f, t) = teach_fx("lesson-stream", 800, 480, 1.0);
    let (tx, rx) = std::sync::mpsc::channel();
    *t.gate.lock().unwrap() = Some(rx);
    open_day(&mut f, 2);
    // no further input: the very first frame after the click already has the goals block and says the writing is under way
    assert_eq!(f.app.page(), "lesson-writing");
    let first = f.text();
    assert!(first.contains("What you will learn today") && first.contains("Day 2 concept") && first.contains("practice 2") && first.contains("Before this: Day 1 concept") && first.contains("Next: Day 3 concept"), "{first}");
    // the first part of the teaching arrives while the rest is still being written
    until_text(&mut f, "A loop repeats the same work");
    assert!(f.text().contains("Explanation") && !f.text().contains("Worked example"), "{}", f.text());
    assert!(f.app.lesson_state().unwrap().generating() && f.has_hit(&Event::Tab(T_L_CANCEL)));
    tx.send(()).unwrap();
    settle(&mut f);
    assert_eq!(f.app.page(), "lesson");
    let titles: Vec<String> = f.app.lesson_state().unwrap().sections().iter().map(|s| s.title.clone()).collect();
    assert_eq!(titles, ["Explanation", "Worked example", "What to do", "Practice task", "Recap"]);
    let pics: Vec<String> = f.app.lesson_state().unwrap().pictures().iter().map(|p| p.caption.clone()).collect();
    assert_eq!(pics, ["A loop of arrows", "The editor"]);
}

#[test]
fn the_lesson_is_written_once_saved_with_its_pictures_and_reopened_without_the_model() {
    let (mut f, t) = teach_fx("lesson-once", 900, 2600, 1.0);
    let id = open_day(&mut f, 1);
    settle(&mut f);
    assert_eq!(t.lessons.load(Ordering::SeqCst), 1);
    let saved = f.cx.services.study.lesson(USER, &id, 1).expect("saved with the plan");
    assert_eq!(saved.pictures.len(), 2);
    for p in &saved.pictures {
        assert_eq!(p.how, "drawn by the assistant");
        assert!(f.cx.services.study.lesson_image(USER, &id, 1, &p.file).unwrap().starts_with(b"<svg"), "{}", p.file);
    }
    assert!(f.app.pic_cache.rasterised >= 2, "both pictures were drawn on the page");
    // leave and come back, and start a brand new tile: nothing is asked of the model, and the lesson is there in the first frame
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "plan");
    let calls = t.calls.load(Ordering::SeqCst);
    f.ev(Event::Row(0));
    assert_eq!(f.app.page(), "lesson");
    assert!(f.text().contains("A loop repeats the same work") && f.text().contains("A loop of arrows"));
    let mut g = LearnApp::new(&f.cx);
    g.open_item(&format!("lesson:{id}:1"), &f.cx);
    assert_eq!(g.page(), "lesson");
    let mut gfx = Gfx::new(800, 480);
    g.render(&mut gfx, f.area, 1.0, &f.cx, 1);
    assert!(g.pic_cache.rasterised >= 2);
    assert_eq!(t.calls.load(Ordering::SeqCst), calls, "a saved lesson needs no model, also offline");
    // with the model gone altogether the saved lesson still opens
    *t.fail.lock().unwrap() = Some("offline".into());
    f.key(Key::Esc);
    f.ev(Event::Row(0));
    assert_eq!(f.app.page(), "lesson");
    assert!(f.text().contains("The editor"));
}

#[test]
fn writing_it_again_asks_first_and_keeps_the_old_lesson_when_the_new_one_fails() {
    let (mut f, t) = teach_fx("lesson-regen", 800, 480, 1.0);
    open_day(&mut f, 1);
    settle(&mut f);
    f.ev(Event::Tab(T_L_REGEN));
    assert_eq!(f.app.page(), "lesson-confirm");
    assert!(f.text().contains("Write this lesson again?"), "{}", f.text());
    assert_eq!(t.lessons.load(Ordering::SeqCst), 1, "nothing is written before the answer");
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "lesson", "Esc closes only the question");
    f.chr('g');
    f.ev(Event::Tab(T_L_REGEN_NO));
    assert_eq!(f.app.page(), "lesson");
    assert_eq!(t.lessons.load(Ordering::SeqCst), 1);
    // yes, with a different lesson from the model
    *t.lesson.lock().unwrap() = "## Explanation\nA brand new explanation.\n## Recap\nShort.".into();
    f.chr('g');
    f.ev(Event::Tab(T_L_REGEN_YES));
    settle(&mut f);
    assert_eq!(t.lessons.load(Ordering::SeqCst), 2);
    assert!(f.text().contains("A brand new explanation") && !f.text().contains("A loop of arrows"));
    // a failing second try leaves the saved one in place and says why
    *t.fail.lock().unwrap() = Some("the model is overloaded".into());
    f.chr('g');
    f.chr('y');
    settle(&mut f);
    assert_eq!(f.app.page(), "lesson-failed");
    assert!(f.text().contains("the model is overloaded") && f.text().contains("A brand new explanation"), "{}", f.text());
}

#[test]
fn the_writer_can_be_cancelled_and_tried_again_and_a_failure_says_why_with_a_retry() {
    let (mut f, t) = teach_fx("lesson-cancel", 800, 480, 1.0);
    let (tx, rx) = std::sync::mpsc::channel();
    *t.gate.lock().unwrap() = Some(rx);
    let id = open_day(&mut f, 1);
    until_text(&mut f, "A loop repeats the same work");
    f.ev(Event::Tab(T_L_CANCEL));
    tx.send(()).unwrap();
    settle(&mut f);
    assert_eq!(f.app.page(), "lesson-failed");
    assert!(f.text().contains("Cancelled.") && f.has_hit(&Event::Tab(T_L_RETRY)), "{}", f.text());
    assert!(f.cx.services.study.lesson(USER, &id, 1).is_none(), "a cancelled lesson is not kept");
    assert!(f.text().contains("What you will learn today"), "the goals stay");
    f.ev(Event::Tab(T_L_RETRY));
    settle(&mut f);
    assert_eq!(f.app.page(), "lesson");
    assert!(f.cx.services.study.lesson(USER, &id, 1).is_some());
    // no model at all: a plain reason and Retry, then it works once a model is there
    let mut g = Fx::new("lesson-nomodel");
    open_day(&mut g, 1);
    settle(&mut g);
    assert_eq!(g.app.page(), "lesson-failed");
    assert!(g.text().contains("No chat model configured") && g.text().contains("Retry"), "{}", g.text());
    // Esc leaves for the plan and stops everything
    g.key(Key::Esc);
    assert_eq!(g.app.page(), "plan");
}

#[test]
fn marking_the_day_done_from_the_lesson_shows_in_the_plan_and_to_the_chat() {
    let (mut f, _t) = teach_fx("lesson-done", 800, 480, 1.0);
    let id = open_day(&mut f, 2);
    settle(&mut f);
    assert!(f.has_hit(&Event::Tab(T_L_DONE)));
    f.ev(Event::Tab(T_L_DONE));
    let listed = f.chat("study_plan_list", json!({"id": id})).unwrap();
    assert!(listed.contains("2. 2030-05-07 18:00 (30 min) [done]"), "{listed}");
    assert!(f.text().contains("Mark not done") && f.text().contains("· done"), "{}", f.text());
    f.chr(' '); // Space takes it back
    assert!(!f.chat("study_plan_list", json!({"id": id})).unwrap().contains("[done]"));
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "plan");
}

#[test]
fn the_chat_opens_a_lesson_through_the_inbox_note() {
    let (mut f, t) = teach_fx("lesson-chat", 800, 480, 1.0);
    let p = make_plan(&f, 3);
    let r = f.chat("study_lesson_open", json!({"plan": p, "session": 2})).unwrap();
    assert!(r.contains("LEARN") && r.contains("Day 2 concept"), "{r}");
    let n = f.cx.services.inbox.list(USER).into_iter().find(|n| n.title == "Lesson, day 2: Day 2 concept").expect("the note");
    assert_eq!(n.link, Some(iyakki_agent::inbox::Link::new("learn", &format!("lesson:{p}:2"))));
    assert!(f.chat("study_lesson_open", json!({"plan": p, "session": 9})).is_err() && f.chat("study_lesson_open", json!({"plan": "nope", "session": 1})).is_err() && f.chat("study_lesson_open", json!({"plan": p})).is_err());
    assert!(f.chat("study_plan_create", plan_args(2)).unwrap().contains("click a day"), "the plan card tells the person about lessons");
    // the inbox hands the link to the tile: the lesson opens and starts
    f.app.open_item(&format!("lesson:{p}:2"), &f.cx);
    f.draw();
    assert!(f.app.page().starts_with("lesson"), "{}", f.app.page());
    assert!(f.text().contains("Day 2: Day 2 concept"));
    settle(&mut f);
    assert_eq!(t.lessons.load(Ordering::SeqCst), 1);
    // bad items leave the tile where it is
    f.app.open_item(&format!("lesson:{p}:99"), &f.cx);
    f.app.open_item("lesson:nope:1", &f.cx);
    f.app.open_item("lesson:garbage", &f.cx);
    f.draw();
    // the plan's lessons go with the plan
    f.cx.services.study.delete_plan(USER, &p, &f.cx.services.calendar).unwrap();
    assert!(f.cx.services.study.lesson(USER, &p, 2).is_none());
}

#[test]
fn a_question_is_answered_into_the_page_and_kept_and_the_level_changes_the_prompt() {
    let (mut f, t) = teach_fx("lesson-ask", 800, 480, 1.0);
    let id = open_day(&mut f, 1);
    settle(&mut f);
    f.chr('q');
    assert_eq!(f.app.page(), "lesson-ask");
    for c in "why loops?".chars() {
        f.chr(c);
    }
    f.key(Key::Enter);
    settle(&mut f);
    assert_eq!(f.app.page(), "lesson");
    f.app.scroll_px(-5000, &f.cx);
    f.draw();
    assert!(f.text().contains("You asked: why loops?") && f.text().contains("Because the loop counts for you."), "{}", f.text());
    assert_eq!(f.cx.services.study.lesson(USER, &id, 1).unwrap().qa.len(), 1);
    // empty question: nothing is sent
    let calls = t.calls.load(Ordering::SeqCst);
    f.chr('q');
    f.key(Key::Enter);
    assert_eq!(t.calls.load(Ordering::SeqCst), calls);
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "lesson", "Esc closes the question box only");
    // simpler and deeper write it again at that level
    f.ev(Event::Tab(T_L_SIMPLER));
    settle(&mut f);
    assert!(t.last_system.lock().unwrap().contains("SIMPLER") && f.cx.services.study.lesson(USER, &id, 1).unwrap().level == -1);
    assert!(f.cx.services.study.lesson(USER, &id, 1).unwrap().qa.is_empty(), "a new lesson starts without the old questions");
    f.chr('d');
    settle(&mut f);
    assert!(t.last_system.lock().unwrap().contains("DEEPER") && f.cx.services.study.lesson(USER, &id, 1).unwrap().level == 1);
}

#[test]
fn the_keys_move_between_sections_and_back_to_the_plan() {
    let (mut f, _t) = teach_fx("lesson-keys", 360, 740, 1.0);
    open_day(&mut f, 1);
    settle(&mut f);
    assert_eq!(f.app.lesson_state().unwrap().cur, 0);
    f.key(Key::Right);
    f.key(Key::Right);
    assert_eq!(f.app.lesson_state().unwrap().cur, 2);
    f.key(Key::Left);
    assert_eq!(f.app.lesson_state().unwrap().cur, 1);
    f.ev(Event::Tab(T_L_NEXT));
    f.ev(Event::Tab(T_L_PREV));
    assert_eq!(f.app.lesson_state().unwrap().cur, 1);
    f.ev(Event::Tab(T_SEC + 4));
    assert_eq!(f.app.lesson_state().unwrap().cur, 4);
    for _ in 0..9 {
        f.key(Key::Right);
    }
    assert_eq!(f.app.lesson_state().unwrap().cur, 4, "the last section stays the last");
    for _ in 0..9 {
        f.key(Key::Left);
    }
    assert_eq!(f.app.lesson_state().unwrap().cur, 0);
    f.key(Key::Esc);
    assert_eq!((f.app.page().as_str(), f.app.sel_session), ("plan", 0));
    // Enter in the plan opens the lesson of the chosen day, and the lesson is already saved
    f.key(Key::Down);
    f.key(Key::Enter);
    assert!(f.app.page().starts_with("lesson") && f.app.lesson_state().unwrap().n == 2);
    f.ev(Event::Tab(T_L_BACK));
    assert_eq!(f.app.page(), "plan");
}

struct Beep;
impl iyakki_agent::story::Synth for Beep {
    fn synth(&self, _t: &str) -> Result<Vec<u8>, String> {
        Ok(vec![0u8; 64])
    }
}
struct Mute(Arc<AtomicUsize>);
struct Gone;
impl iyakki_agent::story::Playing for Gone {
    fn is_playing(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
}
impl iyakki_agent::story::Sink for Mute {
    fn play(&self, _w: &[u8]) -> Result<Box<dyn iyakki_agent::story::Playing>, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Gone))
    }
}

#[test]
fn reading_aloud_uses_the_tales_reader_and_says_plainly_when_no_voice_is_set_up() {
    let (mut f, _t) = teach_fx("lesson-read", 800, 480, 1.0);
    open_day(&mut f, 1);
    settle(&mut f);
    f.chr('r');
    f.draw();
    let (msg, ok) = f.app.msg.clone().expect("a message");
    assert!(!ok && !msg.is_empty() && !f.app.reading(), "{msg}");
    let played = Arc::new(AtomicUsize::new(0));
    f.app.test_voice = Some((Arc::new(Beep), Arc::new(Mute(played.clone()))));
    f.ev(Event::Tab(T_L_READ));
    for _ in 0..300 {
        if played.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(played.load(Ordering::SeqCst) > 0, "the lesson text reached the speaker");
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "plan");
}

#[test]
fn every_lesson_screen_fits_every_size() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1366, 768), (2560, 1600)] {
        for s in [1.0f32, 1.5, 2.0] {
            let (mut f, t) = teach_fx(&format!("lesson-fit-{w}-{}", (s * 10.0) as u32), w, h, s);
            let long = "A very long topic name that goes on and on about the unusual and difficult things one can study in a lifetime ".repeat(3);
            let topic: String = long.chars().take(110).collect();
            *t.lesson.lock().unwrap() = format!("## Explanation\n{long}\n[PICTURE: {long} | a long thing]\n## Worked example\n{long}{long}\nSupercalifragilisticexpialidocious{}\n[PICTURE: second | b]\n## What to do\n1. {long}\n2. {long}\n[PICTURE: third | c]\n## Practice task\n{long}\n## Recap\n{long}", "x".repeat(200));
            let plan = json!({"topic": topic, "unit": "days", "count": 3, "start": "2030-05-06", "sessions": (1..=3).map(|i| json!({"title": format!("Session {i}: a quite long title that has to wrap onto lines"), "notes": long.chars().take(550).collect::<String>()})).collect::<Vec<_>>()});
            f.chat("study_plan_create", plan).unwrap();
            f.reload();
            let (tx, rx) = std::sync::mpsc::channel();
            *t.gate.lock().unwrap() = Some(rx);
            f.key(Key::Enter);
            f.ev(Event::Row(0));
            check_fits(&mut f, &format!("first frame {w}x{h} s{s}"));
            for _ in 0..600 {
                f.draw();
                if f.app.lesson_state().is_some_and(|l| !l.sections().is_empty() && !l.sections()[0].text.is_empty()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            check_fits(&mut f, &format!("writing {w}x{h} s{s}"));
            tx.send(()).unwrap();
            settle(&mut f);
            check_fits(&mut f, &format!("lesson {w}x{h} s{s}"));
            for step in 0..6 {
                f.key(Key::Right);
                check_fits(&mut f, &format!("section {step} {w}x{h} s{s}"));
            }
            f.app.scroll_px(-3000, &f.cx);
            f.draw();
            check_fits(&mut f, &format!("bottom {w}x{h} s{s}"));
            f.chr('g');
            check_fits(&mut f, &format!("confirm {w}x{h} s{s}"));
            f.key(Key::Esc);
            f.chr('q');
            for c in long.chars().take(60) {
                f.chr(c);
            }
            check_fits(&mut f, &format!("asking {w}x{h} s{s}"));
            f.key(Key::Esc);
            *t.fail.lock().unwrap() = Some(format!("the model said no: {long}"));
            f.chr('g');
            f.chr('y');
            settle(&mut f);
            assert_eq!(f.app.page(), "lesson-failed");
            check_fits(&mut f, &format!("failed {w}x{h} s{s}"));
        }
    }
}

#[test]
fn a_lesson_that_was_deleted_with_its_plan_leaves_the_page_without_a_crash() {
    let (mut f, _t) = teach_fx("lesson-gone", 800, 480, 1.0);
    let id = open_day(&mut f, 1);
    settle(&mut f);
    f.cx.services.study.delete_plan(USER, &id, &f.cx.services.calendar).unwrap();
    f.reload();
    f.reload();
    assert_eq!(f.app.page(), "plans");
}
