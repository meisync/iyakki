//! The chat's study tools (`study_plan_*`, `quiz_*`): thin wrappers over [`crate::study`] that give the model the tool
//! descriptions, the approval texts and the answers it reads. All the checking lives in `study.rs`; the LEARN tile reads
//! the same store, so what is made here appears there and the other way round.

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::calendar::CalendarStore;
use crate::chat::ToolSpec;
use crate::clock::{now_unix, Zone};
use crate::services::Services;
use crate::study::{plan_line, plan_text, quiz_line, results_text, PlanSpec};

pub const NAMES: [&str; 15] = [
    "study_plan_create", "study_plan_list", "study_plan_progress", "study_plan_to_calendar", "study_plan_calendar_remove", "study_plan_delete",
    "quiz_create", "quiz_list", "quiz_results", "quiz_next", "quiz_answer", "quiz_report", "quiz_open", "quiz_delete",
    "study_lesson_open",
];

fn def(name: &str, desc: &str, props: Value, required: &[&str], risk: Risk) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema: json!({"type": "object", "properties": props, "required": required}) }, risk, untrusted: false }
}

/// The tools, when the account may use LEARN; the two calendar ones also need the calendar app.
pub fn defs(calendar: bool) -> Vec<ToolDef> {
    let s = json!({"type": "string"});
    let i = json!({"type": "integer"});
    let mut d = vec![
        def("study_plan_create", "Make a study or practice schedule (a lesson plan) from a topic. `unit`: days, hours, weeks or months; `count`: how many. The code works out the dates and checks your list, so write `sessions` as [{title, notes}] in order: days = one session per day (count sessions); weeks = one per week (count); hours = the total hours cut into sessions of `minutes` (ceil(count*60/minutes) sessions, default 30 minutes); months = one per week over the real months (usually 4 or 5 per month). Optional `start` (date, default tomorrow), `time` (HH:MM local, default 18:00), `minutes` per session (default 30), `every`: daily, weekdays or weekly. Write a real progressive syllabus: one concept per session, a practice task in `notes`, a review session, a final project or quiz. If the number of sessions is wrong you get the exact number back: fix it and call again. This does NOT touch the calendar: afterwards ASK the person whether to add it, and only then call study_plan_to_calendar.",
            json!({"topic": s, "unit": s, "count": i, "start": s, "time": s, "minutes": i, "every": s, "sessions": {"type": "array", "items": {"type": "object", "properties": {"title": s, "notes": s}, "required": ["title"]}}}), &["topic", "unit", "count", "sessions"], Risk::Safe),
        def("study_plan_list", "List the person's study plans with progress, or with `id` show one plan's sessions in full.", json!({"id": s}), &[], Risk::Safe),
        def("study_plan_progress", "Tick a session of a plan (number 1, 2, ... as listed) done or not done: `done` true or false.", json!({"id": s, "session": i, "done": {"type": "boolean"}}), &["id", "session", "done"], Risk::Safe),
        def("study_plan_delete", "Delete a study plan and the calendar events it added. Asks the user first.", json!({"id": s}), &["id"], Risk::Ask),
        def("quiz_create", "Save a multiple-choice quiz you wrote. `count` is the number of questions the person asked for and `questions` must hold exactly that many: [{question, options (2 to 6, usually 4, all different, none like \"all of the above\"), answer (the 0-based number of the ONE correct option), explanation (why it is right)}]. Before sending, check every answer number against its option: a wrong key gives the person a wrong grade. The code rejects a bad quiz with the exact problems: fix them and call again. The person does not see the answers until they have answered; the code scores.",
            json!({"topic": s, "count": i, "questions": {"type": "array", "items": {"type": "object", "properties": {"question": s, "options": {"type": "array", "items": s}, "answer": i, "explanation": s}, "required": ["question", "options", "answer", "explanation"]}}}), &["topic", "count", "questions"], Risk::Safe),
        def("quiz_list", "List the person's quizzes with their last score (never the answers).", json!({}), &[], Risk::Safe),
        def("quiz_results", "The score of the person's latest attempt at a quiz and the questions they got wrong (with what they picked, the right answer and why), so you can coach them. Without an attempt there is nothing to read.", json!({"id": s}), &["id"], Risk::Safe),
        def("quiz_next", "Take a quiz inside the chat: the next unanswered question with lettered options and NO answer. Ask it to the person exactly as given, one at a time. `restart` true begins again. Returns that the quiz is finished when all are answered.", json!({"id": s, "restart": {"type": "boolean"}}), &["id"], Risk::Safe),
        def("quiz_answer", "Record the person's answer to the question quiz_next gave (`question` is its number, `option` the letter they chose, A, B, C ...). The code marks it and returns right or wrong, the correct option and the explanation; after the last question it returns the score. Do not judge the answer yourself.", json!({"id": s, "question": i, "option": s}), &["id", "question", "option"], Risk::Safe),
        def("quiz_report", "The person says a question is wrong (a wrong answer key or a bad question): leave it out of the score from now on (`wrong` false takes the report back). `question` is its number.", json!({"id": s, "question": i, "wrong": {"type": "boolean"}}), &["id", "question"], Risk::Safe),
        def("quiz_open", "Bring the person to the LEARN tile: leaves a note in their inbox that opens a quiz (`id` of a quiz) or a plan (`plan` with a plan id). Say you did.", json!({"id": s, "plan": s}), &[], Risk::Safe),
        def("study_lesson_open", "Bring the person to the lesson of one day of a study plan in the LEARN tile: leaves a note in their inbox that opens it. `plan` is the plan id, `session` the day's number (1, 2, ...). The tile writes the lesson (text with pictures) itself the first time and keeps it, so you do not write it; the person can also click any day of a plan there. Say you did.", json!({"plan": s, "session": i}), &["plan", "session"], Risk::Safe),
        def("quiz_delete", "Delete a quiz and its results. Asks the user first.", json!({"id": s}), &["id"], Risk::Ask),
    ];
    if calendar {
        d.push(def("study_plan_to_calendar", "Put every session of a study plan in the person's calendar, one event each (tagged as yours, so they can be removed together). Only after the person said yes. `remind_minutes` (optional): ring a reminder that many minutes before each session, 0 = at the start; this is the way to remind for a plan, so do NOT call alarm_set for it. Asks the user first.", json!({"id": s, "remind_minutes": i}), &["id"], Risk::Ask));
        d.push(def("study_plan_calendar_remove", "Take a study plan's events out of the calendar again, and only those; the plan stays. Asks the user first.", json!({"id": s}), &["id"], Risk::Ask));
    }
    d
}

pub fn describe(name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
    Some(match name {
        "study_plan_to_calendar" => format!("add every session of the study plan {} to the calendar", s("id")),
        "study_plan_calendar_remove" => format!("remove the calendar events of the study plan {}", s("id")),
        "study_plan_delete" => format!("delete the study plan {} and its calendar events", s("id")),
        "quiz_delete" => format!("delete the quiz {} and its results", s("id")),
        "study_plan_create" => format!("make a study plan “{}”", s("topic")),
        "quiz_create" => format!("save a quiz “{}”", s("topic")),
        _ => return None,
    })
}

fn arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| format!("`{k}` is required"))
}

fn num(a: &Value, k: &str) -> Result<usize, String> {
    a.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).map(|n| n as usize).ok_or_else(|| format!("`{k}` must be a whole number"))
}

fn options_text(q: &crate::study::Question) -> String {
    q.options.iter().enumerate().map(|(i, o)| format!("{}) {o}", (b'A' + i as u8) as char)).collect::<Vec<_>>().join("\n")
}

fn question_text(i: usize, total: usize, q: &crate::study::Question) -> String {
    format!("Question {} of {}: {}\n{}", i + 1, total, q.text, options_text(q))
}

pub fn call(svc: &Services, user: &str, name: &str, args: &Value) -> Result<String, String> {
    let st = &svc.study;
    let zone = Zone::system();
    let now = now_unix();
    let cal: &CalendarStore = &svc.calendar;
    match name {
        "study_plan_create" => {
            let p = st.create_plan(user, &PlanSpec::from_args(args)?, now, &zone)?;
            Ok(format!(
                "Saved the study plan (it is in the LEARN tile):\n{}\nNOT in the calendar yet. Now ASK the person whether to add these {} sessions to their calendar (use ask_user with Yes / No); call study_plan_to_calendar with id {} only if they say yes, and ask whether they want a reminder before each session (remind_minutes) instead of using alarm_set. Also tell the person they can click a day of the plan in the LEARN tile to get a written lesson for it with pictures (you can open one for them with study_lesson_open).",
                plan_text(&p),
                p.sessions.len(),
                p.id
            ))
        }
        "study_plan_list" => match args.get("id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Ok(plan_text(&st.plan(user, id)?)),
            None => {
                let all = st.plans(user);
                Ok(if all.is_empty() { "No study plans yet.".into() } else { all.iter().map(plan_line).collect::<Vec<_>>().join("\n") })
            }
        },
        "study_plan_progress" => {
            let done = args.get("done").and_then(Value::as_bool).ok_or("`done` must be true or false")?;
            let p = st.set_done(user, arg(args, "id")?, num(args, "session")?, done)?;
            Ok(format!("{}\n{}", plan_line(&p), p.next_index().map_or("Every session is done.".to_string(), |i| format!("Next: session {} on {}: {}", i + 1, p.sessions[i].date, p.sessions[i].title))))
        }
        "study_plan_to_calendar" => {
            let remind = args.get("remind_minutes").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).map(|m| m.min(1440) as u32);
            let n = st.to_calendar_with(user, arg(args, "id")?, cal, &zone, remind)?;
            let rem = remind.map_or(String::new(), |m| if m == 0 { " Each rings when it starts.".into() } else { format!(" Each rings {m} minutes before.") });
            Ok(format!("Added {n} events to the calendar.{rem} Nothing else is needed: do not set an alarm for the plan (alarm_set `days` takes weekdays, never a number of days); if the person wants reminders and none were set, offer to take the events out (study_plan_calendar_remove) and add them again with remind_minutes."))
        }
        "study_plan_calendar_remove" => Ok(format!("Removed {} calendar events of the plan.", st.calendar_remove(user, arg(args, "id")?, cal)?)),
        "study_plan_delete" => {
            let n = st.delete_plan(user, arg(args, "id")?, cal)?;
            Ok(format!("Deleted the plan and {n} calendar events."))
        }
        "quiz_create" => {
            let count = args.get("count").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).ok_or("`count` must be the number of questions asked for")?;
            let q = st.create_quiz(user, arg(args, "topic")?, count, args.get("questions").unwrap_or(&Value::Null), now)?;
            Ok(format!("Saved the quiz [{}] “{}” with {} questions (it is in the LEARN tile). The answers are hidden from the person until they answer. Offer to quiz them here (quiz_next) or open the tile (quiz_open).", q.id, q.topic, q.questions.len()))
        }
        "quiz_list" => {
            let all = st.quizzes(user);
            Ok(if all.is_empty() { "No quizzes yet.".into() } else { all.iter().map(quiz_line).collect::<Vec<_>>().join("\n") })
        }
        "quiz_results" => Ok(results_text(&st.quiz(user, arg(args, "id")?)?)),
        "quiz_next" => {
            let id = arg(args, "id")?;
            if args.get("restart").and_then(Value::as_bool).unwrap_or(false) {
                st.chat_restart(user, id)?;
            }
            let quiz = st.quiz(user, id)?;
            match st.chat_next(user, id)? {
                Some((i, q)) => Ok(format!("{}\nShow exactly this to the person and wait for their letter. Do not reveal or hint at the answer.", question_text(i, quiz.questions.len(), &q))),
                None => Ok("This quiz is finished; read the score with quiz_results.".into()),
            }
        }
        "quiz_answer" => {
            let id = arg(args, "id")?;
            let a = st.chat_answer(user, id, num(args, "question")?, arg(args, "option")?, now)?;
            let mut out = format!("{} (question {} of {}). Correct answer: {}. Why: {}", if a.right { "Right" } else { "Wrong" }, a.answered, a.total, a.key, a.why);
            if a.flagged {
                out.push_str(" (this question was reported as wrong, so it does not count)");
            }
            match a.finished {
                Some(s) => out.push_str(&format!("\nFINISHED. Score {}/{} = {}%{}. Tell the person, and offer to go over the wrong ones (quiz_results).", s.correct, s.counted, s.percent, if s.flagged > 0 { format!(", {} reported question(s) not counted", s.flagged) } else { String::new() })),
                None => out.push_str("\nTell the person this, then ask the next question with quiz_next."),
            }
            Ok(out)
        }
        "quiz_report" => {
            let q = num(args, "question")?;
            let wrong = args.get("wrong").and_then(Value::as_bool).unwrap_or(true);
            st.flag_question(user, arg(args, "id")?, q.checked_sub(1).ok_or("question numbers start at 1")?, wrong)?;
            Ok(format!("Question {q} is {} the score.", if wrong { "left out of" } else { "counted in" }))
        }
        "quiz_open" => {
            let (what, item) = match (args.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()), args.get("plan").and_then(Value::as_str).filter(|s| !s.is_empty())) {
                (Some(q), _) => {
                    let quiz = st.quiz(user, q)?;
                    (format!("Quiz: {}", quiz.topic), format!("quiz:{q}"))
                }
                (None, Some(p)) => {
                    let plan = st.plan(user, p)?;
                    (format!("Study plan: {}", plan.topic), format!("plan:{p}"))
                }
                _ => return Err("give the quiz `id` or a `plan` id".into()),
            };
            svc.notify(user, crate::inbox::Kind::Info, &what, "Open it in the LEARN tile.", Some(crate::inbox::Link::new("learn", &item)))?;
            Ok(format!("Left a note in the inbox that opens {what} in the LEARN tile. Tell the person to tap it in the inbox bell, or open LEARN from the home screen."))
        }
        "study_lesson_open" => {
            let (plan_id, n) = (arg(args, "plan")?, num(args, "session")?);
            let plan = st.plan(user, plan_id)?;
            let s = n.checked_sub(1).and_then(|i| plan.sessions.get(i)).ok_or_else(|| format!("session must be from 1 to {}", plan.sessions.len()))?;
            let what = format!("Lesson, day {n}: {}", s.title);
            let had = st.lesson(user, plan_id, n).is_some();
            svc.notify(user, crate::inbox::Kind::Info, &what, "Open it in the LEARN tile.", Some(crate::inbox::Link::new("learn", &format!("lesson:{plan_id}:{n}"))))?;
            Ok(format!("Left a note in the inbox that opens the lesson of day {n} ({}) in the LEARN tile{}. Tell the person to tap it in the inbox bell, or open LEARN, open the plan and click the day.", s.title, if had { " (it is already written, so it opens at once)" } else { ": the tile writes the lesson when it opens, with pictures" }))
        }
        "quiz_delete" => {
            st.delete_quiz(user, arg(args, "id")?)?;
            Ok("Deleted the quiz.".into())
        }
        _ => Err(format!("unknown study tool {name}")),
    }
}
