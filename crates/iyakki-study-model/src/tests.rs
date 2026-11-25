//! The pure rules of the study model: scoring, dates, spacing, the quiz check and the lesson parser. No file, no clock, no chat backend.

use crate::study::*;
use crate::studylesson::{self as sl, Lesson, Picture, Section};
use serde_json::json;

fn quiz() -> Quiz {
    let q = |a: usize, flagged: bool| Question { text: "q".into(), options: vec!["a".into(), "b".into(), "c".into()], answer: a, explanation: "why".into(), flagged };
    Quiz { id: "q1".into(), topic: "t".into(), created: 0, questions: vec![q(0, false), q(1, false), q(2, false), q(0, true)], attempts: vec![], progress: vec![] }
}

#[test]
fn a_score_counts_the_right_ones_skips_are_wrong_and_reported_questions_leave_both_numbers() {
    let s = score(&quiz(), &[Some(0), Some(0), None, Some(2)]);
    assert_eq!((s.correct, s.counted, s.flagged, s.percent), (1, 3, 1, 33));
    assert!(s.items[0].right && !s.items[1].right && !s.items[2].right && s.items[3].flagged);
    let all = score(&quiz(), &[Some(0), Some(1), Some(2)]);
    assert_eq!((all.correct, all.percent), (3, 100));
}

#[test]
fn a_quiz_with_every_question_reported_scores_zero_and_not_a_division_by_zero() {
    let mut q = quiz();
    for x in &mut q.questions {
        x.flagged = true;
    }
    let s = score(&q, &[Some(0), Some(1)]);
    assert_eq!((s.counted, s.percent, s.flagged), (0, 0, 4));
}

#[test]
fn dates_run_through_month_ends_and_leap_days_and_the_text_round_trips() {
    let start = parse_date("2028-02-27").unwrap();
    let days: Vec<String> = dates(start, 4, "daily").into_iter().map(date_text).collect();
    assert_eq!(days, ["2028-02-27", "2028-02-28", "2028-02-29", "2028-03-01"], "2028 is a leap year");
    assert!(parse_date("2027-02-29").is_err() && parse_date("not a date").is_err());
    let wk: Vec<String> = dates(parse_date("2030-05-06").unwrap(), 3, "weekly").into_iter().map(date_text).collect();
    assert_eq!(wk, ["2030-05-06", "2030-05-13", "2030-05-20"]);
}

#[test]
fn weekdays_skip_saturday_and_sunday() {
    let fri = parse_date("2030-05-10").unwrap();
    let v = dates(fri, 3, "weekdays");
    assert_eq!(v.iter().map(|d| weekday(*d)).collect::<Vec<_>>().len(), 3);
    assert!(v.iter().all(|d| !matches!(weekday(*d), 0 | 6)), "no weekend day: {:?}", v.iter().map(|d| (date_text(*d), weekday(*d))).collect::<Vec<_>>());
}

#[test]
fn adding_months_keeps_the_day_or_the_last_day_of_a_short_month() {
    let jan31 = parse_date("2030-01-31").unwrap();
    assert_eq!(date_text(add_months(jan31, 1)), "2030-02-28");
    assert_eq!(date_text(add_months(jan31, 2)), "2030-03-31");
}

#[test]
fn units_and_spacings_are_parsed_in_words_and_refused_in_words() {
    assert_eq!(parse_every(" Every day ").unwrap(), "daily");
    assert_eq!(parse_every("weekday").unwrap(), "weekdays");
    assert!(parse_every("fortnightly").unwrap_err().contains("not one of"));
    assert_eq!(Unit::parse("weeks").unwrap().name(), "weeks");
    assert!(Unit::parse("decades").is_err());
}

#[test]
fn hours_are_cut_into_sessions_with_a_shorter_last_one() {
    assert_eq!(expected_sessions(Unit::Hours, 2, 30, 0), 4);
    assert_eq!(expected_sessions(Unit::Days, 21, 30, 0), 21);
}

#[test]
fn the_quiz_check_lists_every_problem_and_accepts_a_good_quiz() {
    let good = json!([{"question": "Which?", "options": ["a", "b"], "answer": 1, "explanation": "because"}]);
    let (topic, qs) = validate_quiz("Topic", 1, &good).unwrap();
    assert_eq!((topic.as_str(), qs.len(), qs[0].answer), ("Topic", 1, 1));
    let bad = json!([{"question": "Which?", "options": ["a"], "answer": 7, "explanation": ""}]);
    let e = validate_quiz("Topic", 2, &bad).unwrap_err();
    assert!(e.contains("2") || e.contains("option") || e.contains("answer"), "{e}");
}

#[test]
fn text_helpers_cap_clean_and_normalise() {
    let mut problems = vec![];
    assert_eq!(clean("  a\tb\nc ", 20, "title", false, &mut problems), "a b c");
    assert!(problems.is_empty());
    clean(&"x".repeat(30), 10, "title", false, &mut problems);
    assert_eq!(problems.len(), 1);
    assert_eq!(norm("Hello, World 2!"), "helloworld2");
    assert_eq!(new_id().len(), 8);
}

#[test]
fn the_goals_of_a_day_come_from_its_title_notes_and_neighbours() {
    let s = |t: &str, n: &str| Session { date: "2030-05-06".into(), time: "18:00".into(), minutes: 30, title: t.into(), notes: n.into(), done: false };
    let p = Plan { id: "p".into(), topic: "Rust".into(), created: 0, unit: "days".into(), count: 3, every: "daily".into(), sessions: vec![s("One", ""), s("Two", "read a chapter; write a test"), s("Three", "")], events: vec![] };
    let g = sl::goals(&p, 2);
    assert_eq!(g[0], "Today, day 2 of 3: Two");
    assert!(g.contains(&"read a chapter".to_string()) && g.contains(&"write a test".to_string()));
    assert!(g.contains(&"Before this: One".to_string()) && g.contains(&"Next: Three".to_string()));
    assert!(sl::goals(&p, 0).is_empty() && sl::goals(&p, 9).is_empty());
}

#[test]
fn a_lesson_is_parsed_into_sections_and_pictures_and_markup_is_stripped() {
    let raw = "## Explanation\nA <b>loop</b> repeats.\n[PICTURE: Arrows | arrows in a circle]\n## Worked example\n* count to three\n## Recap\nDone.";
    let p = sl::parse(raw, true);
    let titles: Vec<&str> = p.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Explanation", "Worked example", "Recap"]);
    assert!(!p.sections[0].text.contains('<'), "{}", p.sections[0].text);
    assert_eq!(p.pictures.len(), 1);
    assert_eq!((p.pictures[0].caption.as_str(), p.sections[0].pics.clone()), ("Arrows", vec![0]));
    assert!(p.sections[1].text.starts_with("- count"), "bullets become plain ones: {}", p.sections[1].text);
}

#[test]
fn a_picture_line_still_being_typed_is_not_shown_until_the_run_is_done() {
    let streaming = sl::parse("## Explanation\nText.\n[PICTURE: Arro", false);
    assert!(streaming.pictures.is_empty() && !streaming.sections[0].text.contains("PICTURE"));
}

#[test]
fn the_caps_hold_whatever_the_model_wrote() {
    let long = format!("## Explanation\n{}", "word ".repeat(5000));
    let p = sl::parse(&long, true);
    assert!(p.sections[0].text.chars().count() <= sl::MAX_SECTION_CHARS + 10);
    let many = (0..20).map(|i| format!("## S{i}\nx")).collect::<Vec<_>>().join("\n");
    assert_eq!(sl::parse(&many, true).sections.len(), sl::MAX_SECTIONS);
    assert_eq!(sl::cut("abcdef", 3).chars().count() <= 4, true);
}

#[test]
fn only_safe_file_names_are_kept_with_a_lesson() {
    assert!(sl::file_ok("pic1.svg") && sl::file_ok("pic2.png"));
    for bad in ["", ".hidden", "../x", "a/b", "a b", &"x".repeat(40)] {
        assert!(!sl::file_ok(bad), "{bad}");
    }
}

#[test]
fn a_lesson_reads_as_plain_text_for_the_voice() {
    let l = Lesson { plan: "p".into(), session: 1, level: 0, created: 0, sections: vec![Section { title: "Explanation".into(), text: "A loop.".into(), pics: vec![] }], pictures: vec![Picture::default()], qa: vec![] };
    assert_eq!(l.plain(), "Explanation.\nA loop.");
    assert!(sl::placeholder_svg("a brief").starts_with("<svg"));
}
