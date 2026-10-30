use super::*;
use crate::calendar::CalendarStore;
use crate::clock::Zone;
use crate::study::PlanSpec;

fn store(tag: &str) -> (StudyStore, CalendarStore, PathBuf) {
    let d = crate::testdir::unique("iyakki-lesson", tag);
    (StudyStore::new(&d.join("study")), CalendarStore::new(&d.join("calendar")), d)
}

fn plan(st: &StudyStore, n: usize) -> Plan {
    let spec = PlanSpec { topic: "Python basics".into(), unit: "days".into(), count: n as u64, start: Some("2030-05-06".into()), sessions: (1..=n).map(|i| (format!("Lesson {i}"), format!("loops; practice {i}. second point"))).collect(), ..PlanSpec::default() };
    st.create_plan("amy", &spec, 1_792_000_000, &Zone::utc()).unwrap()
}

const REPLY: &str = "## Explanation\nA **loop** repeats work.\n[PICTURE: A loop | a circle of arrows]\n## Worked example\nCount to 3.\n## What to do\n1. Open the editor\n* then run it\n## Practice task\nPrint 1 to 5.\n## Recap\nLoops repeat.";

#[test]
fn a_reply_becomes_sections_and_pictures_with_the_markup_gone() {
    let p = parse(REPLY, true);
    let titles: Vec<&str> = p.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, HEADINGS);
    assert_eq!(p.sections[0].text, "A loop repeats work.");
    assert_eq!(p.sections[0].pics, vec![0]);
    assert_eq!(p.pictures[0].caption, "A loop");
    assert!(p.sections[2].text.contains("- then run it"));
    let dirty = parse("## Explanation\n<script>alert(1)</script>Hello <b>there</b> ![x](http://a/b.png) see [docs](http://x) now\n<!-- c -->", true);
    let t = &dirty.sections[0].text;
    assert!(!t.contains('<') && !t.contains("http") && t.contains("Hello there") && t.contains("see docs now"), "{t}");
}

#[test]
fn a_half_written_reply_shows_what_is_there_and_hides_an_unfinished_picture_line() {
    let p = parse("## Explanation\nA loop repeats.\n[PICTURE: A lo", false);
    assert_eq!(p.sections.len(), 1);
    assert!(p.pictures.is_empty() && !p.sections[0].text.contains("PICTURE"));
    let p = parse("<think>hmm ## Explanation</think>## Explanation\nx", false);
    assert_eq!(p.sections[0].text, "x");
    let p = parse("<think>still thinking", false);
    assert!(p.sections.is_empty());
}

#[test]
fn the_code_caps_sections_pictures_and_length() {
    let mut raw = String::new();
    for i in 0..20 {
        raw.push_str(&format!("## Part {i}\n{}\n[PICTURE: c{i} | b]\n", "word ".repeat(1000)));
    }
    let p = parse(&raw, true);
    assert!(p.sections.len() <= MAX_SECTIONS && p.pictures.len() <= MAX_PICTURES);
    assert!(p.sections.iter().all(|s| s.text.chars().count() <= MAX_SECTION_CHARS));
    let cap = parse(&format!("## Explanation\n[PICTURE: {} | b]", "c".repeat(500)), true);
    assert!(cap.pictures[0].caption.chars().count() <= MAX_CAPTION);
}

#[test]
fn the_goals_come_from_the_title_the_notes_and_the_neighbours() {
    let (st, _, _) = store("goals");
    let p = plan(&st, 3);
    let g = goals(&p, 2);
    assert!(g[0].contains("day 2 of 3") && g[0].contains("Lesson 2"), "{g:?}");
    assert!(g.iter().any(|l| l == "loops") && g.iter().any(|l| l == "second point"), "{g:?}");
    assert!(g.iter().any(|l| l.contains("Before this: Lesson 1")) && g.iter().any(|l| l.contains("Next: Lesson 3")));
    assert!(goals(&p, 9).is_empty());
    let m = lesson_messages(&p, 2, 1);
    assert!(m[1].content.contains("Yesterday's lesson was: Lesson 1") && m[1].content.contains("Tomorrow's lesson will be: Lesson 3") && m[0].content.contains("DEEPER"));
}

#[test]
fn a_lesson_is_saved_with_its_pictures_loaded_again_and_goes_with_its_plan() {
    let (st, cal, d) = store("save");
    let p = plan(&st, 2);
    assert!(st.lesson("amy", &p.id, 1).is_none());
    let parsed = parse(REPLY, true);
    let mut l = Lesson { plan: p.id.clone(), session: 1, created: 5, sections: parsed.sections, pictures: parsed.pictures, ..Default::default() };
    l.pictures[0].file = "pic1.svg".into();
    st.save_lesson("amy", &p.id, 1, &l, &[("pic1.svg".into(), placeholder_svg("a loop").into_bytes())]).unwrap();
    assert_eq!(st.lesson("amy", &p.id, 1).unwrap(), l);
    assert!(st.lesson_image("amy", &p.id, 1, "pic1.svg").unwrap().starts_with(b"<svg"));
    assert!(st.lesson_image("amy", &p.id, 1, "../lesson.json").is_none() && st.lesson_image("amy", &p.id, 1, "").is_none());
    st.add_qa("amy", &p.id, 1, "why?", "because").unwrap();
    assert_eq!(st.lesson("amy", &p.id, 1).unwrap().qa, vec![Qa { q: "why?".into(), a: "because".into() }]);
    use std::os::unix::fs::PermissionsExt;
    let dir = d.join("study").join("amy.lessons").join(format!("{}-1", p.id));
    assert_eq!(std::fs::metadata(dir.join("lesson.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    // a plan id that is a path is refused
    assert!(st.save_lesson("amy", "../x", 1, &l, &[]).is_err());
    st.delete_plan("amy", &p.id, &cal).unwrap();
    assert!(st.lesson("amy", &p.id, 1).is_none() && !dir.exists());
}

#[test]
fn the_placeholder_passes_the_same_sanitiser_as_every_other_drawing() {
    let s = placeholder_svg("anything at all");
    assert!(crate::story::sanitize_svg(&s).is_ok(), "{s}");
    assert_ne!(placeholder_svg("a"), placeholder_svg("b"));
}
