use super::*;
use serde_json::json;

fn store(tag: &str) -> (StudyStore, CalendarStore, PathBuf) {
    let d = crate::testdir::unique("iyakki-study", tag);
    (StudyStore::new(&d.join("study")), CalendarStore::new(&d.join("calendar")), d)
}

fn spec(unit: &str, count: u64, start: &str, n: usize) -> PlanSpec {
    PlanSpec { topic: "Python basics".into(), unit: unit.into(), count, start: Some(start.into()), sessions: (1..=n).map(|i| (format!("Lesson {i}"), format!("notes {i}"))).collect(), ..PlanSpec::default() }
}

fn dates_of(p: &Plan) -> Vec<&str> {
    p.sessions.iter().map(|s| s.date.as_str()).collect()
}

const NOW: i64 = 1_792_000_000; // 2026-10-14 in every zone

#[test]
fn dates_run_across_a_month_end_a_year_end_and_a_leap_day() {
    let (st, _, _) = store("dates");
    let z = Zone::utc();
    let p = st.create_plan("amy", &spec("days", 5, "2026-01-30", 5), NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-01-30", "2026-01-31", "2026-02-01", "2026-02-02", "2026-02-03"]);
    let p = st.create_plan("amy", &spec("days", 4, "2028-02-28", 4), NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2028-02-28", "2028-02-29", "2028-03-01", "2028-03-02"], "2028 is a leap year");
    let p = st.create_plan("amy", &spec("days", 3, "2026-12-30", 3), NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-12-30", "2026-12-31", "2027-01-01"]);
    let p = st.create_plan("amy", &spec("days", 3, "2027-02-27", 3), NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2027-02-27", "2027-02-28", "2027-03-01"], "2027 is not");
}

#[test]
fn the_defaults_are_tomorrow_at_six_in_the_evening_thirty_minutes_daily() {
    let (st, _, _) = store("defaults");
    let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
    // 2026-10-14 23:30 local is still the 14th there (it is the 15th in UTC): tomorrow is the 15th
    let late = z.local_to_unix(2026, 10, 14, 23, 30, 0);
    let mut s = spec("days", 2, "", 2);
    s.start = None;
    let p = st.create_plan("amy", &s, late, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-10-15", "2026-10-16"]);
    assert_eq!((p.sessions[0].time.as_str(), p.sessions[0].minutes, p.every.as_str()), ("18:00", 30, "daily"));
    s.time = Some("6:45am".into());
    s.minutes = Some(45);
    let p = st.create_plan("amy", &s, late, &z).unwrap();
    assert_eq!((p.sessions[1].time.as_str(), p.sessions[1].minutes), ("06:45", 45));
}

#[test]
fn weekdays_skip_the_weekend_and_weekly_steps_seven_days() {
    let (st, _, _) = store("every");
    let z = Zone::utc();
    let mut s = spec("days", 4, "2026-10-31", 4); // a Saturday
    s.every = Some("weekdays".into());
    let p = st.create_plan("amy", &s, NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05"]);
    s.start = Some("2026-11-04".into());
    s.count = 4;
    let p = st.create_plan("amy", &s, NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-11-04", "2026-11-05", "2026-11-06", "2026-11-09"], "Friday is followed by Monday");
    s.every = Some("weekly".into());
    let p = st.create_plan("amy", &s, NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2026-11-04", "2026-11-11", "2026-11-18", "2026-11-25"]);
}

#[test]
fn hours_are_cut_into_sessions_and_the_last_one_is_what_is_left() {
    let (st, _, _) = store("hours");
    let z = Zone::utc();
    let mut s = spec("hours", 5, "2026-10-20", 7);
    s.minutes = Some(45);
    let p = st.create_plan("amy", &s, NOW, &z).unwrap();
    assert_eq!(p.sessions.len(), 7, "300 minutes in sessions of 45 is 6 and a half");
    assert_eq!(p.sessions.iter().map(|x| x.minutes).collect::<Vec<_>>(), [45, 45, 45, 45, 45, 45, 30]);
    assert_eq!(p.sessions.iter().map(|x| x.minutes).sum::<u32>(), 300);
    // the default is 30 minutes: 2 hours = 4 sessions
    assert_eq!(st.create_plan("amy", &spec("hours", 2, "2026-10-20", 4), NOW, &z).unwrap().sessions.len(), 4);
    let e = st.create_plan("amy", &spec("hours", 2, "2026-10-20", 3), NOW, &z).unwrap_err();
    assert!(e.contains("exactly 4 sessions") && e.contains("you sent 3"), "{e}");
}

#[test]
fn months_make_one_session_a_week_over_the_real_calendar() {
    assert_eq!(add_months(days_from_civil(2026, 1, 31), 1), days_from_civil(2026, 2, 28));
    assert_eq!(add_months(days_from_civil(2028, 1, 31), 1), days_from_civil(2028, 2, 29));
    assert_eq!(add_months(days_from_civil(2026, 12, 15), 2), days_from_civil(2027, 2, 15));
    let (st, _, _) = store("months");
    let z = Zone::utc();
    // 1 Feb to 1 Mar 2027 is 28 days: 4 weekly sessions
    let p = st.create_plan("amy", &spec("months", 1, "2027-02-01", 4), NOW, &z).unwrap();
    assert_eq!(dates_of(&p), ["2027-02-01", "2027-02-08", "2027-02-15", "2027-02-22"]);
    // 30 Oct to 30 Nov is 31 days: 5 sessions
    let p = st.create_plan("amy", &spec("months", 1, "2026-10-30", 5), NOW, &z).unwrap();
    assert_eq!((p.sessions.len(), p.every.as_str(), p.sessions[4].date.as_str()), (5, "weekly", "2026-11-27"));
    // three months from 1 Jan: 90 days, 13 weeks
    assert_eq!(st.create_plan("amy", &spec("months", 3, "2027-01-01", 13), NOW, &z).unwrap().sessions.len(), 13);
    let e = st.create_plan("amy", &spec("months", 1, "2027-02-01", 5), NOW, &z).unwrap_err();
    assert!(e.contains("exactly 4 sessions"), "{e}");
    let mut s = spec("months", 1, "2027-02-01", 4);
    s.every = Some("daily".into());
    assert!(st.create_plan("amy", &s, NOW, &z).unwrap_err().contains("one session a week"));
    // weeks
    assert_eq!(dates_of(&st.create_plan("amy", &spec("weeks", 2, "2027-02-01", 2), NOW, &z).unwrap()), ["2027-02-01", "2027-02-08"]);
}

#[test]
fn a_plan_that_is_wrong_is_refused_with_every_problem_and_nothing_is_saved() {
    let (st, _, _) = store("reject");
    let z = Zone::utc();
    let e = st.create_plan("amy", &spec("days", 21, "2026-10-20", 20), NOW, &z).unwrap_err();
    assert!(e.contains("exactly 21 sessions") && e.contains("you sent 20") && e.contains("call study_plan_create again"), "{e}");
    let mut s = spec("days", 3, "2026-10-20", 3);
    s.sessions[1].0 = "  ".into();
    s.sessions[2].0 = "lesson 1!".into();
    let e = st.create_plan("amy", &s, NOW, &z).unwrap_err();
    assert!(e.contains("session 2 has an empty title") && e.contains("session 3 repeats the title"), "{e}");
    assert!(st.plans("amy").is_empty(), "a refused plan leaves nothing behind");
    for (unit, count, want) in [("days", 367u64, "from 1 to 366"), ("days", 0, "from 1"), ("months", 13, "12 months"), ("fortnights", 2, "unit")] {
        let e = st.create_plan("amy", &spec(unit, count, "2026-10-20", 1), NOW, &z).unwrap_err();
        assert!(e.contains(want), "{unit} {count}: {e}");
    }
    for (field, bad) in [("start", "next friday"), ("start", "2026-02-30"), ("time", "25:00"), ("every", "sometimes"), ("minutes", "2")] {
        let mut s = spec("days", 1, "2026-10-20", 1);
        match field {
            "start" => s.start = Some(bad.into()),
            "time" => s.time = Some(bad.into()),
            "every" => s.every = Some(bad.into()),
            _ => s.minutes = Some(2),
        }
        assert!(st.create_plan("amy", &s, NOW, &z).is_err(), "{field} {bad}");
    }
    assert!(st.create_plan("amy", &spec("days", 1, "2026-10-20", 1), NOW, &z).is_ok());
    assert!(st.create_plan("../x", &spec("days", 1, "2026-10-20", 1), NOW, &z).is_err());
}

#[test]
fn a_plan_may_have_366_sessions_but_not_more_and_hours_that_make_too_many_are_refused() {
    let (st, _, _) = store("cap");
    let z = Zone::utc();
    let p = st.create_plan("amy", &spec("days", 366, "2026-10-20", 366), NOW, &z).unwrap();
    assert_eq!(p.sessions.last().unwrap().date, "2027-10-20");
    let mut s = spec("hours", 200, "2026-10-20", 1);
    s.minutes = Some(30);
    let e = st.create_plan("amy", &s, NOW, &z).unwrap_err();
    assert!(e.contains("400 sessions") && e.contains("at most 366"), "{e}");
}

#[test]
fn the_plan_arguments_are_read_from_the_models_json() {
    let s = PlanSpec::from_args(&json!({"topic": "Yoga", "unit": "days", "count": "3", "sessions": [{"title": "a"}, {"title": "b", "notes": "n"}]})).unwrap();
    assert_eq!((s.count, s.sessions.len(), s.sessions[1].1.as_str()), (3, 2, "n"));
    assert!(PlanSpec::from_args(&json!({"topic": "Yoga", "unit": "days", "count": 3})).unwrap_err().contains("sessions"));
    assert!(PlanSpec::from_args(&json!({"unit": "days", "count": 3, "sessions": []})).unwrap_err().contains("topic"));
    assert!(PlanSpec::from_args(&json!({"topic": "x", "unit": "days", "count": 3, "sessions": [{"notes": "no title"}]})).unwrap_err().contains("no `title`"));
}

#[test]
fn plans_survive_a_restart_and_progress_is_checked() {
    let (st, _, d) = store("persist");
    let z = Zone::utc();
    let p = st.create_plan("amy", &spec("days", 3, "2026-10-20", 3), NOW, &z).unwrap();
    assert_eq!(p.next_index(), Some(0));
    let q = st.set_done("amy", &p.id, 1, true).unwrap();
    assert_eq!((q.done_count(), q.next_index()), (1, Some(1)));
    let again = StudyStore::new(&d.join("study"));
    assert_eq!(again.plan("amy", &p.id).unwrap(), q, "what was saved is what comes back");
    assert!(again.plans("bob").is_empty());
    assert!(again.set_done("amy", &p.id, 0, true).is_err() && again.set_done("amy", &p.id, 4, true).is_err());
    assert!(again.set_done("amy", "nope", 1, true).is_err());
    again.set_done("amy", &p.id, 1, false).unwrap();
    assert_eq!(again.plan("amy", &p.id).unwrap().done_count(), 0);
    use std::os::unix::fs::PermissionsExt;
    let f = d.join("study/amy.json");
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!f.with_extension("tmp").exists(), "the atomic write leaves no temporary file");
    std::fs::write(&f, b"{ not json").unwrap();
    assert!(again.set_done("amy", &p.id, 1, true).unwrap_err().contains("damaged"));
}

#[test]
fn the_calendar_gets_one_event_per_session_once_at_the_same_wall_clock_time_across_daylight_saving() {
    let (st, cal, _) = store("cal");
    let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
    let p = st.create_plan("amy", &spec("days", 4, "2026-03-06", 4), NOW, &z).unwrap();
    assert!(cal.events("amy").is_empty(), "making a plan never touches the calendar");
    assert_eq!(st.to_calendar("amy", &p.id, &cal, &z).unwrap(), 4);
    let evs = cal.events("amy");
    assert_eq!(evs.len(), 4);
    assert!(evs.iter().all(|e| e.by == "assistant" && e.notes.contains(&format!("[study:{}]", p.id)) && e.minutes == 30 && e.repeat == Repeat::None));
    let mut starts: Vec<i64> = evs.iter().map(|e| e.start).collect();
    starts.sort();
    assert!(starts.iter().all(|t| z.local(*t).hour == 18 && z.local(*t).minute == 0), "18:00 every day");
    assert_eq!(starts[2] - starts[1], 23 * 3600, "the clocks went forward on 8 March: that day is 23 hours long");
    assert!(evs.iter().any(|e| e.title == "Python basics: Lesson 1" && e.notes.starts_with("notes 1\n")));
    // asking again must not double the events
    assert!(st.to_calendar("amy", &p.id, &cal, &z).unwrap_err().contains("already in the calendar"));
    assert_eq!(cal.events("amy").len(), 4);
    assert!(st.plan("amy", &p.id).unwrap().events.len() == 4);
}

#[test]
fn removing_a_plans_events_takes_exactly_those_and_deleting_the_plan_does_too() {
    let (st, cal, _) = store("calrm");
    let z = Zone::utc();
    let a = st.create_plan("amy", &spec("days", 3, "2026-10-20", 3), NOW, &z).unwrap();
    let b = st.create_plan("amy", &spec("days", 2, "2026-10-20", 2), NOW, &z).unwrap();
    st.to_calendar("amy", &a.id, &cal, &z).unwrap();
    st.to_calendar("amy", &b.id, &cal, &z).unwrap();
    let t = z.local_to_unix(2026, 10, 20, 9, 0, 0);
    cal.add_event("amy", "Dentist", t, 30, false, Repeat::None, "", "user", None).unwrap();
    // a person's own event that happens to mention the tag is not the assistant's and stays
    cal.add_event("amy", "Mine", t, 30, false, Repeat::None, &format!("[study:{}]", a.id), "user", None).unwrap();
    assert_eq!(cal.events("amy").len(), 7);
    assert_eq!(st.calendar_remove("amy", &a.id, &cal).unwrap(), 3);
    let left: Vec<String> = cal.events("amy").into_iter().map(|e| e.title).collect();
    assert_eq!(left.len(), 4);
    assert!(left.contains(&"Dentist".to_string()) && left.contains(&"Mine".to_string()) && left.iter().filter(|t| t.contains("Lesson")).count() == 2);
    assert_eq!(st.calendar_remove("amy", &a.id, &cal).unwrap(), 0, "a second removal finds nothing");
    assert!(st.plan("amy", &a.id).unwrap().events.is_empty());
    // the plan can be added again after its events were removed
    assert_eq!(st.to_calendar("amy", &a.id, &cal, &z).unwrap(), 3);
    // deleting a plan clears its events too, and only its own
    assert_eq!(st.delete_plan("amy", &b.id, &cal).unwrap(), 2);
    assert_eq!(cal.events("amy").len(), 2 + 3);
    assert!(st.plan("amy", &b.id).is_err() && st.delete_plan("amy", &b.id, &cal).is_err());
}

#[test]
fn a_calendar_that_refuses_leaves_nothing_half_added() {
    let (st, cal, _) = store("calfull");
    let z = Zone::utc();
    let p = st.create_plan("amy", &spec("days", 3, "2026-10-20", 3), NOW, &z).unwrap();
    for i in 0..(crate::calendar::MAX_EVENTS - 2) {
        cal.add_event("amy", &format!("e{i}"), 1_800_000_000, 0, false, Repeat::None, "", "user", None).unwrap();
    }
    let e = st.to_calendar("amy", &p.id, &cal, &z).unwrap_err();
    assert!(e.contains("Nothing was added"), "{e}");
    assert_eq!(cal.events("amy").len(), crate::calendar::MAX_EVENTS - 2, "the two that fitted were taken back");
    assert!(st.plan("amy", &p.id).unwrap().events.is_empty());
}

fn q(text: &str, options: &[&str], answer: Value) -> Value {
    json!({"question": text, "options": options, "answer": answer, "explanation": "because"})
}

fn good(n: usize) -> Value {
    Value::Array((0..n).map(|i| q(&format!("What is {i} plus one?"), &["zero", &format!("{}", i + 1), "ten", "twenty"], json!(1))).collect())
}

#[test]
fn a_good_quiz_is_kept_and_the_answer_may_be_a_number_a_numeral_string_or_a_letter() {
    let (st, _, _) = store("quizok");
    let quiz = st.create_quiz("amy", " Maths ", 3, &good(3), NOW).unwrap();
    assert_eq!((quiz.topic.as_str(), quiz.questions.len(), quiz.questions[2].answer, quiz.questions[2].options[1].as_str()), ("Maths", 3, 1, "3"));
    for a in [json!("B"), json!("b"), json!("1"), json!(1)] {
        let (_, qs) = validate_quiz("t", 1, &json!([q("Q?", &["a", "b", "c"], a.clone())])).unwrap();
        assert_eq!(qs[0].answer, 1, "{a}");
    }
    assert_eq!(st.quiz("amy", &quiz.id).unwrap(), quiz);
    assert_eq!(st.quizzes("amy").len(), 1);
}

#[test]
fn a_quiz_with_problems_is_refused_with_each_exact_problem() {
    let cases: Vec<(&str, u64, Value, Vec<&str>)> = vec![
        ("wrong count", 5, good(3), vec!["asked for 5 questions but you sent 3"]),
        ("zero", 0, json!([]), vec!["count must be from 1 to 50"]),
        ("too many", 51, good(51), vec!["count must be from 1 to 50", "at most 50"]),
        ("not a list", 1, json!({"a": 1}), vec!["must be a list"]),
        ("empty text", 1, json!([q("  ", &["a", "b"], json!(0))]), vec!["question 1 has no text"]),
        ("one option", 1, json!([q("Q?", &["a"], json!(0))]), vec!["has 1 options; give 2 to 6"]),
        ("seven options", 1, json!([q("Q?", &["a", "b", "c", "d", "e", "f", "g"], json!(0))]), vec!["has 7 options"]),
        ("same options", 1, json!([q("Q?", &["Paris", "paris", "Rome"], json!(0))]), vec!["two options that are the same"]),
        ("empty option", 1, json!([q("Q?", &["a", " ", "c"], json!(0))]), vec!["empty option"]),
        ("range", 1, json!([q("Q?", &["a", "b", "c", "d"], json!(4))]), vec!["`answer` 4 is out of range: the options are numbered 0 to 3"]),
        ("negative", 1, json!([q("Q?", &["a", "b"], json!(-1))]), vec!["whole number"]),
        ("two answers", 1, json!([q("Q?", &["a", "b", "c"], json!([0, 1]))]), vec!["exactly ONE correct option"]),
        ("no answer", 1, json!([{"question": "Q?", "options": ["a", "b"], "explanation": "e"}]), vec!["has no `answer`"]),
        ("all of the above", 1, json!([q("Q?", &["a", "b", "All of the above"], json!(2))]), vec!["all of the above"]),
        ("no explanation", 1, json!([{"question": "Q?", "options": ["a", "b"], "answer": 0}]), vec!["no explanation"]),
        ("duplicate question", 2, json!([q("What is Rust?", &["a", "b"], json!(0)), q("what is rust", &["c", "d"], json!(1))]), vec!["question 2 repeats an earlier question"]),
        ("long text", 1, json!([q(&"x".repeat(401), &["a", "b"], json!(0))]), vec!["longer than 400"]),
        ("long option", 1, json!([q("Q?", &["a", &"y".repeat(201)], json!(0))]), vec!["option 1 is longer than 200"]),
    ];
    for (name, count, qs, want) in cases {
        let e = validate_quiz("Topic", count, &qs).unwrap_err();
        for w in want {
            assert!(e.contains(w), "{name}: wanted {w:?} in {e}");
        }
        assert!(e.contains("not saved"), "{name}");
    }
    let e = validate_quiz("", 1, &json!([q("", &["a"], json!(9))])).unwrap_err();
    assert!(e.matches("\n- ").count() >= 4, "all the problems come together so one retry can fix them: {e}");
    let (st, _, _) = store("quizbad");
    assert!(st.create_quiz("amy", "t", 2, &good(1), NOW).is_err());
    assert!(st.quizzes("amy").is_empty());
}

fn three() -> Quiz {
    validate_quiz("Trivia", 3, &json!([q("Q one?", &["a", "b", "c", "d"], json!(0)), q("Q two?", &["a", "b", "c", "d"], json!(2)), q("Q three?", &["a", "b", "c", "d"], json!(3))])).map(|(topic, questions)| Quiz { id: "q1".into(), topic, created: 0, questions, attempts: vec![], progress: vec![] }).unwrap()
}

#[test]
fn scoring_counts_right_answers_skips_are_wrong_and_percent_is_rounded() {
    let quiz = three();
    let s = score(&quiz, &[Some(0), Some(2), Some(3)]);
    assert_eq!((s.correct, s.counted, s.percent, s.flagged), (3, 3, 100, 0));
    let s = score(&quiz, &[Some(0), Some(1), None]);
    assert_eq!((s.correct, s.counted, s.percent), (1, 3, 33));
    assert_eq!(s.items.iter().map(|i| i.right).collect::<Vec<_>>(), [true, false, false]);
    assert_eq!(s.items[2].picked, None);
    assert_eq!(score(&quiz, &[Some(0), Some(2), None]).percent, 67, "2 of 3 rounds up");
    assert_eq!(score(&quiz, &[]).correct, 0, "no answers at all");
}

#[test]
fn a_reported_question_leaves_the_score_and_the_percent_is_over_the_rest() {
    let mut quiz = three();
    let answers = [Some(0), Some(1), Some(3)]; // the middle one is wrong
    assert_eq!(score(&quiz, &answers).percent, 67);
    quiz.questions[1].flagged = true;
    let s = score(&quiz, &answers);
    assert_eq!((s.correct, s.counted, s.flagged, s.percent), (2, 2, 1, 100));
    assert!(s.items[1].flagged);
    quiz.questions[0].flagged = true;
    quiz.questions[2].flagged = true;
    let s = score(&quiz, &answers);
    assert_eq!((s.counted, s.percent), (0, 0), "nothing left to count is 0, never a division by zero");
}

#[test]
fn attempts_are_saved_checked_and_rescored_when_a_question_is_reported_later() {
    let (st, _, d) = store("attempt");
    let quiz = st.create_quiz("amy", "Trivia", 3, &json!([q("Q one?", &["a", "b", "c", "d"], json!(0)), q("Q two?", &["a", "b", "c", "d"], json!(2)), q("Q three?", &["a", "b", "c", "d"], json!(3))]), NOW).unwrap();
    let s = st.record_attempt("amy", &quiz.id, &[Some(0), Some(1), Some(3)], NOW + 5).unwrap();
    assert_eq!((s.correct, s.percent), (2, 67));
    assert!(st.record_attempt("amy", &quiz.id, &[Some(0)], NOW).unwrap_err().contains("expected 3 answers"));
    assert!(st.record_attempt("amy", &quiz.id, &[Some(0), Some(9), None], NOW).unwrap_err().contains("does not exist"));
    assert!(st.record_attempt("amy", "nope", &[], NOW).is_err());
    assert_eq!(st.quiz("amy", &quiz.id).unwrap().attempts.len(), 1, "refused attempts are not kept");
    st.flag_question("amy", &quiz.id, 1, true).unwrap();
    let back = StudyStore::new(&d.join("study")).quiz("amy", &quiz.id).unwrap();
    assert!(back.questions[1].flagged);
    let s = score(&back, &back.attempts[0].answers);
    assert_eq!((s.correct, s.counted, s.percent), (2, 2, 100), "the earlier attempt is marked again without the reported question");
    assert!(st.flag_question("amy", &quiz.id, 3, true).is_err());
    st.flag_question("amy", &quiz.id, 1, false).unwrap();
    assert!(!st.quiz("amy", &quiz.id).unwrap().questions[1].flagged);
    for i in 0..MAX_ATTEMPTS + 5 {
        st.record_attempt("amy", &quiz.id, &[Some(0), Some(2), Some(3)], NOW + i as i64).unwrap();
    }
    assert_eq!(st.quiz("amy", &quiz.id).unwrap().attempts.len(), MAX_ATTEMPTS);
    st.delete_quiz("amy", &quiz.id).unwrap();
    assert!(st.delete_quiz("amy", &quiz.id).is_err() && st.quizzes("amy").is_empty());
}

#[test]
fn the_coach_text_hides_the_key_until_the_person_has_answered_then_lists_the_wrong_ones() {
    let mut quiz = three();
    let before = results_text(&quiz);
    assert!(before.contains("not taken this quiz yet") && !before.contains("why:") && !before.contains("because"), "{before}");
    quiz.attempts.push(Attempt { taken: 1, answers: vec![Some(0), Some(1), None] });
    let text = results_text(&quiz);
    assert!(text.contains("Score 1/3 = 33%"), "{text}");
    assert!(text.contains("Wrong: Q2 Q two?") && text.contains("picked: b") && text.contains("correct: c") && text.contains("why: because"), "{text}");
    assert!(text.contains("Wrong: Q3") && text.contains("(skipped)"), "{text}");
    assert!(!text.contains("Wrong: Q1"));
    quiz.questions[1].flagged = true;
    let text = results_text(&quiz);
    assert!(text.contains("Score 1/2 = 50%") && text.contains("reported as wrong") && !text.contains("Wrong: Q2") && text.contains("Reported wrong by the person (not counted): Q2"), "{text}");
}

#[test]
fn the_text_lines_name_the_id_and_the_progress() {
    let (st, _, _) = store("lines");
    let z = Zone::utc();
    let p = st.create_plan("amy", &spec("days", 2, "2026-10-20", 2), NOW, &z).unwrap();
    st.set_done("amy", &p.id, 2, true).unwrap();
    let p = st.plan("amy", &p.id).unwrap();
    assert!(plan_line(&p).contains(&p.id) && plan_line(&p).contains("2 sessions, 1 done"));
    let t = plan_text(&p);
    assert!(t.contains("1. 2026-10-20 18:00 (30 min) Lesson 1 — notes 1") && t.contains("2. 2026-10-21 18:00 (30 min) [done] Lesson 2"), "{t}");
}

#[test]
fn a_quiz_taken_in_the_chat_is_asked_in_order_and_scored_by_code() {
    let (st, _, d) = store("chatquiz");
    let quiz = st.create_quiz("amy", "Trivia", 3, &json!([q("Q one?", &["a", "b", "c", "d"], json!(0)), q("Q two?", &["a", "b", "c", "d"], json!(2)), q("Q three?", &["a", "b", "c", "d"], json!(3))]), NOW).unwrap();
    let (i, first) = st.chat_next("amy", &quiz.id).unwrap().unwrap();
    assert_eq!((i, first.text.as_str()), (0, "Q one?"));
    assert!(st.chat_answer("amy", &quiz.id, 2, "A", NOW).unwrap_err().contains("next question to answer is 1"), "no jumping ahead");
    assert!(st.chat_answer("amy", &quiz.id, 1, "Z", NOW).unwrap_err().contains("letter A to D"));
    let a = st.chat_answer("amy", &quiz.id, 1, "a", NOW).unwrap();
    assert!(a.right && a.key == "a" && a.why == "because" && a.answered == 1 && a.finished.is_none());
    // the session survives a restart
    let again = StudyStore::new(&d.join("study"));
    assert_eq!(again.chat_next("amy", &quiz.id).unwrap().unwrap().0, 1);
    assert!(st.chat_answer("amy", &quiz.id, 1, "A", NOW).unwrap_err().contains("not 1"), "a question is answered once");
    let b = again.chat_answer("amy", &quiz.id, 2, "b", NOW).unwrap();
    assert!(!b.right && b.key == "c");
    let c = again.chat_answer("amy", &quiz.id, 3, "d", NOW + 9).unwrap();
    let s = c.finished.unwrap();
    assert_eq!((s.correct, s.counted, s.percent), (2, 3, 67));
    let kept = again.quiz("amy", &quiz.id).unwrap();
    assert_eq!((kept.attempts.len(), kept.progress.len()), (1, 0));
    assert_eq!(kept.attempts[0].answers, [Some(0), Some(1), Some(3)]);
    // a second run starts again from the first question
    assert_eq!(again.chat_next("amy", &quiz.id).unwrap().unwrap().0, 0);
    st.chat_answer("amy", &quiz.id, 1, "a", NOW).unwrap();
    st.chat_restart("amy", &quiz.id).unwrap();
    assert_eq!(st.chat_next("amy", &quiz.id).unwrap().unwrap().0, 0);
}

#[test]
fn a_plan_can_ring_a_reminder_before_each_session_so_no_alarm_is_needed() {
    let (st, cal, _) = store("remind");
    let z = Zone::from_posix("UTC0").unwrap();
    let p = st.create_plan("amy", &spec("days", 3, "2026-03-06", 3), NOW, &z).unwrap();
    assert_eq!(st.to_calendar_with("amy", &p.id, &cal, &z, Some(10)).unwrap(), 3);
    assert!(cal.events("amy").iter().all(|e| e.remind == Some(10)), "every event rings ten minutes before");
    let q = st.create_plan("amy", &spec("days", 2, "2026-04-06", 2), NOW, &z).unwrap();
    st.to_calendar("amy", &q.id, &cal, &z).unwrap();
    assert_eq!(cal.events("amy").iter().filter(|e| e.remind.is_none()).count(), 2, "without the option nothing rings");
}
