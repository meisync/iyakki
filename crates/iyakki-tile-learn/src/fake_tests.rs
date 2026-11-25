//! The LEARN tile against a fake port: no store, no chat backend, no sound, no clock, no sleep (the worked example of
//! docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10). The same behaviours are also proved through the real port, with the chat's tools
//! and a real study store, in `crates/iyakki-apps/src/apps/learn_tests.rs`.

use std::sync::mpsc::channel;

use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use crate::fake::{lesson, FakePictures, FakePort};
use crate::lesson::*;
use crate::{Flow, LearnApp, LearnPort};
use crate::app::*;

struct Fx {
    app: LearnApp,
    port: FakePort,
    g: Gfx,
    area: R,
    tick: u64,
}

impl Fx {
    fn new(port: FakePort) -> Fx {
        let app = LearnApp::new("root", &port, Box::new(FakePictures::default()));
        let mut f = Fx { app, port, g: Gfx::new(900, 2400), area: R::new(0.0, 0.0, 900.0, 2400.0), tick: 0 };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, self.area, 1.0, &self.port, self.tick);
    }
    fn ev(&mut self, e: Event) -> Flow {
        let r = self.app.handle(&e, &self.port);
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
    /// The run of the lesson page is over (waits for it, no clock).
    fn settle(&mut self) {
        self.app.settle(&self.port);
        self.draw();
    }
    fn open_day(&mut self, n: usize) {
        self.key(Key::Enter);
        self.ev(Event::Row(n - 1));
    }
}

fn calls_with(f: &Fx, word: &str) -> Vec<String> {
    f.port.calls().into_iter().filter(|c| c.contains(word)).collect()
}

#[test]
fn the_lists_show_what_the_port_holds_and_an_empty_tile_says_how_to_make_a_plan() {
    let f = Fx::new(FakePort::new());
    assert!(f.text().contains("No study plans yet"), "{}", f.text());
    let mut f = Fx::new(FakePort::new().with_plan(3).with_quiz());
    assert!(f.text().contains("Programming basics") && f.text().contains("0 of 3 done"), "{}", f.text());
    f.key(Key::Right);
    assert!(f.text().contains("Solar system") && f.text().contains("3 questions") && f.text().contains("not taken yet"), "{}", f.text());
    assert_eq!(f.app.title(), "Learn");
    assert_eq!(f.app.page(), "quizzes");
}

#[test]
fn ticking_a_session_asks_the_port_and_a_second_tick_takes_it_back() {
    let mut f = Fx::new(FakePort::new().with_plan(4));
    f.ev(Event::Row(0));
    assert_eq!(f.app.page(), "plan");
    f.key(Key::Down);
    f.ev(Event::Tab(T_TICK));
    assert!(f.app.plan("p1").unwrap().sessions[1].done);
    f.ev(Event::Tab(T_TICK));
    assert!(!f.app.plan("p1").unwrap().sessions[1].done);
    assert_eq!(calls_with(&f, "done p1"), ["done p1 2 true", "done p1 2 false"]);
}

#[test]
fn a_refused_change_is_said_in_the_port_words_and_nothing_changes() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.port.st.lock().unwrap().refuse = Some("Your account may not change plans.".into());
    f.ev(Event::Row(0));
    f.chr(' ');
    assert!(f.text().contains("Your account may not change plans."), "{}", f.text());
    assert!(!f.app.plan("p1").unwrap().sessions[0].done);
}

#[test]
fn the_calendar_button_adds_and_removes_the_plans_events_and_the_label_follows() {
    let mut f = Fx::new(FakePort::new().with_plan(3));
    f.ev(Event::Row(0));
    assert!(f.has_hit(&Event::Tab(T_CAL)) && !f.has_hit(&Event::Tab(T_UNCAL)));
    f.ev(Event::Tab(T_CAL));
    assert!(f.text().contains("Added 3 sessions to the calendar."), "{}", f.text());
    assert!(f.has_hit(&Event::Tab(T_UNCAL)) && !f.has_hit(&Event::Tab(T_CAL)));
    f.chr('c');
    assert!(f.text().contains("Removed 3 events from the calendar."), "{}", f.text());
    assert_eq!(calls_with(&f, "calendar"), ["calendar-add p1", "calendar-remove p1"]);
}

#[test]
fn deleting_asks_twice_and_then_the_port_deletes() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.ev(Event::Row(0));
    f.chr('x');
    assert!(f.text().contains("Delete it for good") && calls_with(&f, "delete").is_empty());
    f.ev(Event::Tab(T_NO));
    assert!(!f.text().contains("Delete it for good"));
    f.chr('x');
    f.chr('x');
    assert_eq!(calls_with(&f, "delete"), ["delete-plan p1"]);
    assert_eq!(f.app.page(), "plans");
    assert!(f.text().contains("Plan deleted.") && f.text().contains("No study plans yet"), "{}", f.text());
}

#[test]
fn a_quiz_hides_the_key_until_the_answer_and_the_score_is_worked_out_by_the_code() {
    let mut f = Fx::new(FakePort::new().with_quiz());
    f.key(Key::Right);
    f.key(Key::Enter);
    assert_eq!(f.app.page(), "quiz");
    let t = f.text();
    assert!(t.contains("Which planet is closest to the Sun?") && !t.contains("Mercury orbits closest.") && !t.contains("Not quite") && !t.contains("Right."), "the key is hidden: {t}");
    f.chr('a');
    assert!(f.text().contains("Right.") && f.text().contains("Mercury orbits closest."), "{}", f.text());
    f.chr('b'); // a second pick on an answered question changes nothing
    assert_eq!(f.app.quiz("q1").map(|q| q.attempts.len()), Some(0));
    f.key(Key::Enter);
    f.chr('c');
    f.key(Key::Enter);
    f.chr('1');
    assert!(f.text().contains("Not quite. The answer is C) One"), "{}", f.text());
    f.key(Key::Enter);
    assert_eq!(f.app.page(), "result");
    assert!(f.text().contains("Solar system: 2 of 3 = 67%"), "{}", f.text());
    assert_eq!(calls_with(&f, "attempt"), ["attempt q1 [Some(0), Some(2), Some(0)]"]);
}

#[test]
fn a_reported_question_stops_counting_and_taking_it_back_makes_it_count_again() {
    let mut f = Fx::new(FakePort::new().with_quiz());
    f.key(Key::Right);
    f.key(Key::Enter);
    for c in ['a', 'c', '1'] {
        f.chr(c);
        if c == '1' {
            f.chr('r'); // report the wrong one while it is open
        }
        f.key(Key::Enter);
    }
    assert!(f.text().contains("Solar system: 2 of 2 = 100%") && f.text().contains("1 reported question is left out"), "{}", f.text());
    f.key(Key::Down);
    f.key(Key::Down);
    f.chr('r'); // take it back from the result
    assert!(f.text().contains("Solar system: 2 of 3 = 67%"), "{}", f.text());
    assert_eq!(calls_with(&f, "flag"), ["flag q1 2 true", "flag q1 2 false"]);
}

#[test]
fn a_pdf_names_what_is_open_and_the_answer_key_switch_goes_with_a_quiz() {
    let mut f = Fx::new(FakePort::new().with_plan(2).with_quiz());
    f.chr('p');
    assert_eq!(calls_with(&f, "pdf"), ["pdf Plan(\"p1\")"]);
    assert!(f.text().contains("Saved the PDF in Downloads."));
    f.key(Key::Right);
    f.chr('k');
    assert!(f.text().contains("Quiz PDFs will have the answer key on a page of their own."), "{}", f.text());
    f.chr('p');
    assert_eq!(calls_with(&f, "pdf").last().unwrap(), "pdf Quiz { id: \"q1\", key: true }");
    f.port.st.lock().unwrap().pdf_answer = Some(Err("Other scripts are not supported.".into()));
    f.chr('p');
    assert!(f.text().contains("Other scripts are not supported."), "{}", f.text());
}

#[test]
fn a_quiz_being_taken_has_no_pdf_so_the_key_cannot_leave_the_page() {
    let mut f = Fx::new(FakePort::new().with_quiz());
    f.key(Key::Right);
    f.key(Key::Enter);
    f.ev(Event::Tab(T_PDF));
    assert!(f.text().contains("There is nothing to put in a PDF here yet"), "{}", f.text());
    assert!(calls_with(&f, "pdf").is_empty());
}

#[test]
fn opening_a_day_starts_the_writer_with_the_goals_at_once_and_the_lesson_is_shown_when_it_is_done() {
    let mut f = Fx::new(FakePort::new().with_plan(3));
    f.port.writer.result.lock().unwrap().as_mut().unwrap().plan = "p1".into();
    let (tx, rx) = channel();
    *f.port.writer.gate.lock().unwrap() = Some(rx);
    f.ev(Event::Row(0));
    f.ev(Event::Row(1));
    assert_eq!(f.app.page(), "lesson-writing");
    let t = f.text();
    assert!(t.contains("What you will learn today") && t.contains("Day 2 concept") && t.contains("Before this: Day 1 concept") && t.contains("Next: Day 3 concept"), "{t}");
    assert!(calls_with(&f, "audit root learn.lesson plan p1 day 2 level 0").len() == 1);
    assert!(f.has_hit(&Event::Tab(T_L_CANCEL)));
    tx.send(()).unwrap();
    f.settle();
    assert_eq!(f.app.page(), "lesson");
    let titles: Vec<String> = f.app.lesson_state().unwrap().sections().iter().map(|s| s.title.clone()).collect();
    assert_eq!(titles, ["Explanation", "Worked example", "What to do", "Practice task", "Recap"]);
    assert!(f.text().contains("A loop repeats the same work many times."), "{}", f.text());
    // the saved lesson is on the port now: opening it again writes nothing
    let before = calls_with(&f, "generate").len();
    f.key(Key::Esc);
    f.ev(Event::Row(1));
    assert_eq!(calls_with(&f, "generate").len(), before);
}

#[test]
fn a_failed_run_says_why_and_offers_a_retry_that_writes_again() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    *f.port.writer.result.lock().unwrap() = Err("No model is set up.".into());
    f.ev(Event::Row(0));
    f.ev(Event::Row(0));
    f.settle();
    assert_eq!(f.app.page(), "lesson-failed");
    assert!(f.text().contains("No model is set up.") && f.has_hit(&Event::Tab(T_L_RETRY)));
    *f.port.writer.result.lock().unwrap() = Ok(lesson("p1", 1));
    f.ev(Event::Tab(T_L_RETRY));
    f.settle();
    assert_eq!(f.app.page(), "lesson");
    assert_eq!(calls_with(&f, "generate").len(), 2);
}

#[test]
fn cancelling_sets_the_stop_flag_the_writer_sees_and_nothing_is_saved() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    let (tx, rx) = channel();
    *f.port.writer.gate.lock().unwrap() = Some(rx);
    f.ev(Event::Row(0));
    f.ev(Event::Row(0));
    f.chr('c');
    tx.send(()).unwrap();
    f.settle();
    assert!(f.port.writer.stop_seen.load(std::sync::atomic::Ordering::SeqCst));
    assert!(f.port.st.lock().unwrap().lessons.is_empty());
    assert!(f.text().contains("Cancelled."), "{}", f.text());
}

#[test]
fn a_question_goes_to_the_writer_with_an_audit_line_and_the_answer_is_kept_with_the_lesson() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.open_day(1);
    f.settle();
    f.chr('q');
    assert_eq!(f.app.page(), "lesson-ask");
    for c in "why?".chars() {
        f.chr(c);
    }
    f.key(Key::Enter);
    f.settle();
    assert_eq!(f.app.page(), "lesson");
    assert_eq!(calls_with(&f, "ask root p1 why?").len(), 1);
    assert_eq!(calls_with(&f, "audit root learn.ask").len(), 1);
    assert!(f.text().contains("You asked: why?") && f.text().contains("Because the loop counts for you."), "{}", f.text());
}

#[test]
fn writing_it_again_asks_first() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.open_day(1);
    f.settle();
    f.chr('g');
    assert_eq!(f.app.page(), "lesson-confirm");
    f.ev(Event::Tab(T_L_REGEN_NO));
    assert_eq!(calls_with(&f, "generate").len(), 1);
    f.chr('g');
    f.chr('y');
    f.settle();
    assert_eq!(calls_with(&f, "generate").len(), 2);
}

#[test]
fn reading_aloud_says_plainly_when_no_voice_is_set_up_then_reads_and_stops() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.open_day(1);
    f.settle();
    f.port.st.lock().unwrap().no_voice = Some("No voice is set up. Open Settings, Voice.".into());
    f.chr('r');
    assert!(f.text().contains("No voice is set up."), "{}", f.text());
    assert!(!f.port.reading());
    f.port.st.lock().unwrap().no_voice = None;
    f.ev(Event::Tab(T_L_READ));
    assert!(f.port.reading());
    assert_eq!(calls_with(&f, "audit root learn.read 2 parts").len(), 1);
    assert!(f.text().contains("Stop reading"), "{}", f.text());
    f.chr('r');
    assert!(!f.port.reading());
}

#[test]
fn leaving_the_lesson_stops_the_voice() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.open_day(1);
    f.settle();
    f.chr('r');
    assert!(f.port.reading());
    f.key(Key::Esc);
    assert_eq!(f.app.page(), "plan");
    assert!(!f.port.reading(), "{:?}", f.port.calls());
}

#[test]
fn what_the_chat_deletes_while_a_page_is_open_is_said_and_the_tile_goes_back_to_the_list() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.ev(Event::Row(0));
    assert_eq!(f.app.page(), "plan");
    f.port.st.lock().unwrap().plans.clear();
    f.tick += 100; // the tile looks at the port again every couple of seconds
    f.draw();
    assert_eq!(f.app.page(), "plans");
    assert!(f.text().contains("That item was deleted."), "{}", f.text());
}

#[test]
fn the_chat_can_open_a_lesson_a_plan_or_a_quiz_by_name() {
    let mut f = Fx::new(FakePort::new().with_plan(3).with_quiz());
    let (tx, rx) = channel();
    *f.port.writer.gate.lock().unwrap() = Some(rx);
    f.app.open_item("lesson:p1:2", &f.port);
    f.draw();
    assert_eq!(f.app.page(), "lesson-writing");
    tx.send(()).unwrap();
    f.settle();
    f.app.open_item("plan:p1", &f.port);
    f.draw();
    assert_eq!(f.app.page(), "plan");
    f.app.open_item("quiz:q1", &f.port);
    f.draw();
    assert_eq!(f.app.page(), "quizzes");
}

#[test]
fn escape_goes_one_step_back_and_leaves_the_tile_from_the_list() {
    let mut f = Fx::new(FakePort::new().with_plan(2));
    f.ev(Event::Row(0));
    assert_eq!(f.key(Key::Esc), Flow::Stay);
    assert_eq!(f.app.page(), "plans");
    assert_eq!(f.key(Key::Esc), Flow::Back);
}

#[test]
fn a_picture_is_drawn_through_the_pictures_object_and_a_missing_one_says_so() {
    let pics = FakePictures::default();
    let (drawn, cleared) = (pics.drawn.clone(), pics.cleared.clone());
    let port = FakePort::new().with_plan(2);
    let app = LearnApp::new("root", &port, Box::new(pics));
    let mut f = Fx { app, port, g: Gfx::new(900, 2400), area: R::new(0.0, 0.0, 900.0, 2400.0), tick: 0 };
    f.draw();
    f.open_day(1);
    f.settle();
    f.draw();
    assert!(*drawn.lock().unwrap() >= 1, "the picture of the saved lesson went through the drawer");
    assert!(*cleared.lock().unwrap() >= 1, "a new lesson starts with an empty cache");
    assert!(!f.text().contains("The picture cannot be shown"), "{}", f.text());
}
