//! LEARN as PDF: a plan, a quiz with an optional answer key page, and a lesson with a picture, written from the store and read back with
//! the repo's own reader; refusals (other scripts, no lesson yet, existing file, outside the person's folders).

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::clock::Zone;
use crate::study::{Plan, PlanSpec, StudyStore};
use crate::studylesson::{Lesson, Picture, Qa, Section};
use crate::studypdf::*;
use iyakki_pdf::{BoxGlyphs, Document, PageSize};

struct Rig {
    st: StudyStore,
    dir: PathBuf,
    policy: iyakki_sec::policy::Policy,
}

fn rig(tag: &str) -> Rig {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-studypdf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut policy = iyakki_sec::policy::Role::User.template();
    policy.fs_read = vec![dir.display().to_string()];
    policy.fs_write = vec![dir.display().to_string()];
    Rig { st: StudyStore::new(&dir.join("study")), dir, policy }
}

fn plan(st: &StudyStore) -> Plan {
    let spec = PlanSpec { topic: "Python basics".into(), unit: "days".into(), count: 3, start: Some("2030-05-06".into()), sessions: (1..=3).map(|i| (format!("Lesson {i} loops"), format!("practice set {i}"))).collect(), ..PlanSpec::default() };
    st.create_plan("amy", &spec, 1_792_000_000, &Zone::utc()).unwrap()
}

fn quiz(st: &StudyStore) -> crate::study::Quiz {
    let qs = json!([
        {"question": "Which planet is the largest?", "options": ["Mars", "Jupiter", "Venus", "Earth"], "answer": 1, "explanation": "Jupiter is bigger than all the others together."},
        {"question": "How many moons does Earth have?", "options": ["None", "One", "Two"], "answer": "B", "explanation": "Just the Moon."},
        {"question": "Is the Sun a star?", "options": ["Yes", "No"], "answer": 0, "explanation": "It is a star."}
    ]);
    st.create_quiz("amy", "Solar system", 3, &qs, 1_792_000_000).unwrap()
}

fn all_text(d: &Document) -> String {
    (0..d.page_count()).map(|i| d.page_text(i).unwrap()).collect::<Vec<_>>().join("\n")
}

fn svg() -> Vec<u8> {
    b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\"><rect width=\"100\" height=\"100\" fill=\"#00aa00\"/></svg>".to_vec()
}

#[test]
fn a_plan_pdf_has_the_syllabus_and_the_day_list() {
    let r = rig("plan");
    let p = plan(&r.st);
    r.st.set_done("amy", &p.id, 1, true).unwrap();
    let made = make(&r.st, "amy", &Item::Plan(p.id.clone()), PageSize::A4, None).unwrap();
    let d = Document::open(made.bytes).unwrap();
    assert_eq!(d.page_count(), made.pages);
    let t = all_text(&d);
    for want in ["Study plan: Python basics", "Syllabus", "Lesson 2 loops", "practice set 3", "Day by day", "2030-05-06", "2030-05-08", "18:00"] {
        assert!(t.contains(want), "{want}: {t}");
    }
    assert_eq!(d.info().title, "Study plan: Python basics");
    assert!(make(&r.st, "amy", &Item::Plan("nope".into()), PageSize::A4, None).is_err());
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_quiz_pdf_hides_the_key_unless_asked_and_then_puts_it_on_a_page_of_its_own() {
    let r = rig("quiz");
    let q = quiz(&r.st);
    let off = Document::open(make(&r.st, "amy", &Item::Quiz { id: q.id.clone(), key: false }, PageSize::Letter, None).unwrap().bytes).unwrap();
    let t = all_text(&off);
    assert!(t.contains("1. Which planet is the largest?") && t.contains("B) Jupiter") && t.contains("3. Is the Sun a star?"), "{t}");
    assert!(!t.contains("Answer key") && !t.contains("bigger than all the others"), "no key without the switch: {t}");
    assert_eq!(off.page_count(), 1);
    assert_eq!(off.page_size(0).unwrap().0, 612.0);
    let on = Document::open(make(&r.st, "amy", &Item::Quiz { id: q.id.clone(), key: true }, PageSize::A4, None).unwrap().bytes).unwrap();
    assert_eq!(on.page_count(), 2, "the key is a separate page");
    assert!(!on.page_text(0).unwrap().contains("Answer key") && on.page_text(0).unwrap().contains("Which planet"));
    let key = on.page_text(1).unwrap();
    assert!(key.contains("Answer key: Solar system") && key.contains("1. B) Jupiter") && key.contains("2. B) One") && key.contains("3. A) Yes"), "{key}");
    assert!(key.contains("Jupiter is bigger"), "{key}");
    // a question the person reported as wrong is not printed and the numbers close up
    r.st.flag_question("amy", &q.id, 1, true).unwrap();
    let flagged = Document::open(make(&r.st, "amy", &Item::Quiz { id: q.id.clone(), key: true }, PageSize::A4, None).unwrap().bytes).unwrap();
    let t = all_text(&flagged);
    assert!(!t.contains("moons does Earth have") && t.contains("2. Is the Sun a star?"), "{t}");
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_lesson_pdf_has_its_sections_picture_and_questions_and_needs_the_lesson_to_exist() {
    let r = rig("lesson");
    let p = plan(&r.st);
    let item = Item::Lesson { plan: p.id.clone(), n: 2 };
    let e = make(&r.st, "amy", &item, PageSize::A4, None).err().unwrap();
    assert!(e.contains("no lesson written yet"), "{e}");
    let l = Lesson {
        plan: p.id.clone(),
        session: 2,
        level: 0,
        created: 1,
        sections: vec![
            Section { title: "Explanation".into(), text: "A **loop** repeats work.\n\n- for loops\n- while loops".into(), pics: vec![0] },
            Section { title: "Recap".into(), text: "Loops repeat.".into(), pics: vec![] },
        ],
        pictures: vec![Picture { caption: "A loop".into(), brief: "arrows".into(), file: "pic1.svg".into(), how: "placeholder".into() }],
        qa: vec![Qa { q: "What is a loop?".into(), a: "Something that repeats.".into() }],
    };
    r.st.save_lesson("amy", &p.id, 2, &l, &[("pic1.svg".into(), svg())]).unwrap();
    let made = make(&r.st, "amy", &item, PageSize::A4, None).unwrap();
    let d = Document::open(made.bytes).unwrap();
    let t = all_text(&d);
    for want in ["Day 2: Lesson 2 loops", "Python basics", "Explanation", "A loop repeats work.", "for loops", "A loop", "Recap", "Questions and answers", "What is a loop?", "Something that repeats."] {
        assert!(t.contains(want), "{want}: {t}");
    }
    assert!(!t.contains("**"), "marks are dropped: {t}");
    let green = (0..d.page_count()).any(|i| d.render(i, 1.0, &BoxGlyphs).unwrap().rgba.chunks(4).any(|p| p[1] > 150 && p[0] < 20 && p[2] < 20));
    assert!(green, "the lesson's picture is in the file");
    assert!(make(&r.st, "amy", &Item::Lesson { plan: p.id.clone(), n: 3 }, PageSize::A4, None).is_err());
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn export_saves_under_the_file_rules_and_never_replaces_or_writes_other_scripts() {
    let r = rig("export");
    let p = plan(&r.st);
    let q = quiz(&r.st);
    let target = r.dir.join("plan.pdf");
    let msg = export(&r.st, &r.policy, "amy", &r.dir, &Item::Plan(p.id.clone()), PageSize::A4, Some(&target.display().to_string())).unwrap();
    assert!(msg.starts_with("Saved ") && msg.contains("plan.pdf") && target.is_file(), "{msg}");
    let before = std::fs::read(&target).unwrap();
    let e = export(&r.st, &r.policy, "amy", &r.dir, &Item::Plan(p.id.clone()), PageSize::A4, Some(&target.display().to_string())).unwrap_err();
    assert!(e.contains("already exists") && e.contains("nothing is replaced silently"), "{e}");
    assert_eq!(std::fs::read(&target).unwrap(), before);
    // no name: a free one in the person's downloads (here the fallback under the data directory)
    let a = export(&r.st, &r.policy, "amy", &r.dir, &Item::Quiz { id: q.id.clone(), key: true }, PageSize::A4, None).unwrap();
    let b = export(&r.st, &r.policy, "amy", &r.dir, &Item::Quiz { id: q.id.clone(), key: true }, PageSize::A4, None).unwrap();
    assert!(a.contains("Quiz Solar system.pdf") && b.contains("Quiz Solar system (1).pdf"), "{a} / {b}");
    // outside the person's folders
    let outside = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-studypdf-out-{}.pdf", std::process::id()));
    let e = export(&r.st, &r.policy, "amy", &r.dir, &Item::Plan(p.id.clone()), PageSize::A4, Some(&outside.display().to_string())).unwrap_err();
    assert!(e.contains("permission denied") || e.contains("not allowed") || e.contains("may not"), "{e}");
    assert!(!outside.exists());
    // another script: refused in words, nothing written
    let tamil = PlanSpec { topic: "\u{4e2d}\u{6587}".into(), unit: "days".into(), count: 1, start: Some("2030-05-06".into()), sessions: vec![("\u{4e2d}\u{6587}".into(), String::new())], ..PlanSpec::default() };
    let tp = r.st.create_plan("amy", &tamil, 1_792_000_000, &Zone::utc()).unwrap();
    let t2 = r.dir.join("tamil.pdf");
    let e = export(&r.st, &r.policy, "amy", &r.dir, &Item::Plan(tp.id), PageSize::A4, Some(&t2.display().to_string())).unwrap_err();
    assert!(e.contains("cannot be written in a PDF yet") && e.contains("Nothing was saved") && !t2.exists(), "{e}");
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn the_tool_arguments_are_checked() {
    let a = item_from_args(&json!({"kind": "quiz", "id": "q1", "answer_key": true})).unwrap();
    assert_eq!(a, Item::Quiz { id: "q1".into(), key: true });
    assert_eq!(item_from_args(&json!({"kind": "quiz", "id": "q1"})).unwrap(), Item::Quiz { id: "q1".into(), key: false }, "the key is off by default");
    assert_eq!(item_from_args(&json!({"kind": "lesson", "id": "p1", "session": "2"})).unwrap(), Item::Lesson { plan: "p1".into(), n: 2 });
    assert_eq!(item_from_args(&json!({"kind": "PLAN", "id": "p1"})).unwrap(), Item::Plan("p1".into()));
    for bad in [json!({}), json!({"kind": "poem", "id": "x"}), json!({"kind": "plan"}), json!({"kind": "lesson", "id": "p"}), json!({"kind": "lesson", "id": "p", "session": 0})] {
        assert!(item_from_args(&bad).is_err(), "{bad}");
    }
    assert_eq!(size_from_args(&json!({"size": "Letter"})).unwrap(), PageSize::Letter);
    assert!(size_from_args(&json!({"size": "A3"})).unwrap_err().contains("A4 or Letter"));
    // the tool is in the study group, asks first, and the card says what it makes
    let d = crate::study_tools::defs(false);
    let t = d.iter().find(|d| d.spec.name == "study_export_pdf").expect("listed");
    assert_eq!(t.risk, crate::agent::Risk::Ask);
    assert!(crate::study_tools::NAMES.contains(&"study_export_pdf") && t.spec.description.len() < 420);
    assert_eq!(crate::toolblock::group_of("study_export_pdf"), crate::toolblock::group_of("study_plan_list"));
    let card = crate::study_tools::describe("study_export_pdf", &json!({"kind": "quiz", "id": "q1", "answer_key": true})).unwrap();
    assert!(card.contains("quiz q1") && card.contains("answer key"), "{card}");
    let _ = Path::new("");
}

#[test]
fn a_cyrillic_plan_and_quiz_are_exported_with_the_embedded_font() {
    let r = rig("cyr");
    let spec = PlanSpec { topic: "\u{418}\u{441}\u{442}\u{43e}\u{440}\u{438}\u{44f}".into(), unit: "days".into(), count: 1, start: Some("2030-05-06".into()), sessions: vec![("\u{423}\u{440}\u{43e}\u{43a} \u{43f}\u{435}\u{440}\u{432}\u{44b}\u{439}".into(), "\u{43f}\u{440}\u{430}\u{43a}\u{442}\u{438}\u{43a}\u{430}".into())], ..PlanSpec::default() };
    let p = r.st.create_plan("amy", &spec, 1_792_000_000, &Zone::utc()).unwrap();
    let d = Document::open(make(&r.st, "amy", &Item::Plan(p.id), PageSize::A4, None).unwrap().bytes).unwrap();
    let t = all_text(&d);
    assert!(t.contains("\u{423}\u{440}\u{43e}\u{43a} \u{43f}\u{435}\u{440}\u{432}\u{44b}\u{439}") && t.contains("2030-05-06"), "{t}");
    let qs = json!([{"question": "\u{421}\u{442}\u{43e}\u{43b}\u{438}\u{446}\u{430} \u{424}\u{440}\u{430}\u{43d}\u{446}\u{438}\u{438}?", "options": ["\u{41f}\u{430}\u{440}\u{438}\u{436}", "\u{41b}\u{438}\u{43e}\u{43d}"], "answer": 0, "explanation": "\u{41f}\u{430}\u{440}\u{438}\u{436}."}]);
    let q = r.st.create_quiz("amy", "\u{413}\u{435}\u{43e}\u{433}\u{440}\u{430}\u{444}\u{438}\u{44f}", 1, &qs, 1_792_000_000).unwrap();
    let d = Document::open(make(&r.st, "amy", &Item::Quiz { id: q.id, key: true }, PageSize::A4, None).unwrap().bytes).unwrap();
    assert_eq!(d.page_count(), 2);
    assert!(d.page_text(1).unwrap().contains("A) \u{41f}\u{430}\u{440}\u{438}\u{436}"), "{}", d.page_text(1).unwrap());
    let _ = std::fs::remove_dir_all(&r.dir);
}
