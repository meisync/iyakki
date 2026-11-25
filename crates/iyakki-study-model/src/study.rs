//! Types moved out of `iyakki_agent::study` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Local date, `2026-10-30`.
    pub date: String,
    /// Local clock time, `18:00`.
    pub time: String,
    pub minutes: u32,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub topic: String,
    pub created: i64,
    /// `days`, `hours`, `weeks` or `months`, and how many, as the person asked.
    pub unit: String,
    pub count: u32,
    /// `daily`, `weekdays` or `weekly`.
    pub every: String,
    pub sessions: Vec<Session>,
    /// The calendar events this plan added (empty until the person agrees to put it in the calendar).
    #[serde(default)]
    pub events: Vec<String>,
}

impl Plan {
    pub fn done_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.done).count()
    }
    /// The first session not done yet (index).
    pub fn next_index(&self) -> Option<usize> {
        self.sessions.iter().position(|s| !s.done)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    /// Index into `options`.
    pub answer: usize,
    pub explanation: String,
    /// The person reported this question as wrong: it no longer counts in any score.
    #[serde(default)]
    pub flagged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub taken: i64,
    /// The option picked for each question (None = skipped).
    pub answers: Vec<Option<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quiz {
    pub id: String,
    pub topic: String,
    pub created: i64,
    pub questions: Vec<Question>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// A quiz being taken one question at a time in the chat: the answers so far (empty = none under way).
    #[serde(default)]
    pub progress: Vec<Option<usize>>,
}

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// Study plans and quizzes (the LEARN tile and the chat's `study_*` / `quiz_*` tools, `docs/STUDY.md`).
//
// The model writes the *content* (the syllabus lines, the questions); this code does everything that must be exact:
// the date arithmetic of a plan, the check that the model gave the right number of sessions or questions, the validation
// of every question, the scoring, and the removal of exactly the calendar events a plan added. A wrong call gets a list of
// the exact problems so the model can fix it and try again.
//
// One JSON file per person (`<dir>/<user>.json`), written atomically with owner-only permissions. A plan's sessions keep
// their date and clock time as plain text (`2026-10-30`, `18:00`); the instant is worked out when an event is made, so a
// daily 18:00 stays 18:00 across daylight saving.

use std::collections::HashSet;

use serde_json::Value;

use iyakki_clock::{civil, days_from_civil};

/// The most sessions one plan may have.
pub const MAX_SESSIONS: usize = 366;
pub const MAX_PLANS: usize = 50;
pub const MAX_QUIZZES: usize = 100;
pub const MAX_QUESTIONS: usize = 50;
/// Attempts kept per quiz (the oldest go first).
pub const MAX_ATTEMPTS: usize = 30;
pub const MAX_TOPIC: usize = 120;
pub const MAX_TITLE: usize = 120;
pub const MAX_NOTES: usize = 600;
pub const MAX_QUESTION: usize = 400;
pub const MAX_OPTION: usize = 200;
pub const MAX_EXPLAIN: usize = 600;







#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemResult {
    pub index: usize,
    pub picked: Option<usize>,
    pub right: bool,
    pub flagged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Scored {
    pub correct: usize,
    /// Questions that count (not reported as wrong).
    pub counted: usize,
    pub flagged: usize,
    /// Rounded to a whole number; 0 when nothing counts.
    pub percent: u32,
    pub items: Vec<ItemResult>,
}

/// Mark an attempt: unanswered questions are wrong, reported questions are left out of both numbers.
pub fn score(q: &Quiz, answers: &[Option<usize>]) -> Scored {
    let mut items = Vec::new();
    let (mut correct, mut counted, mut flagged) = (0, 0, 0);
    for (i, qu) in q.questions.iter().enumerate() {
        let picked = answers.get(i).copied().flatten();
        let right = picked == Some(qu.answer);
        if qu.flagged {
            flagged += 1;
        } else {
            counted += 1;
            if right {
                correct += 1;
            }
        }
        items.push(ItemResult { index: i, picked, right, flagged: qu.flagged });
    }
    let percent = if counted == 0 { 0 } else { ((correct * 100 + counted / 2) / counted) as u32 };
    Scored { correct, counted, flagged, percent, items }
}

/// What recording one answer from the chat produced.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatAnswer {
    pub right: bool,
    pub key: String,
    pub why: String,
    pub flagged: bool,
    pub answered: usize,
    pub total: usize,
    pub finished: Option<Scored>,
}


pub fn new_id() -> String {
    let mut r = [0u8; 4];
    let _ = iyakki_sec::random_fill(&mut r);
    r.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn clean(s: &str, max: usize, what: &str, multiline: bool, problems: &mut Vec<String>) -> String {
    let t: String = s.chars().map(|c| if c.is_control() && !(multiline && c == '\n') { ' ' } else { c }).collect::<String>().trim().to_string();
    if t.chars().count() > max {
        problems.push(format!("{what} is longer than {max} characters"));
    }
    t
}

pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn date_text(days: i64) -> String {
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn parse_date(s: &str) -> Result<i64, String> {
    let bad = || format!("`{s}` is not a date: write 2026-10-30");
    let mut p = s.trim().split('-');
    let (y, m, d) = (p.next().and_then(|v| v.parse::<i64>().ok()).ok_or_else(bad)?, p.next().and_then(|v| v.parse::<u32>().ok()).ok_or_else(bad)?, p.next().and_then(|v| v.parse::<u32>().ok()).ok_or_else(bad)?);
    if p.next().is_some() || !(1970..=2100).contains(&y) || !(1..=12).contains(&m) || d == 0 || d > 31 {
        return Err(bad());
    }
    let days = days_from_civil(y, m, d);
    if civil(days) != (y, m, d) {
        return Err(bad());
    }
    Ok(days)
}

/// 0 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

/// `days` plus whole months, the day of the month kept or cut back to the last day of a shorter month.
pub fn add_months(days: i64, months: u32) -> i64 {
    let (y, m, d) = civil(days);
    let t = (y * 12 + m as i64 - 1) + months as i64;
    let (ny, nm) = (t.div_euclid(12), (t.rem_euclid(12) + 1) as u32);
    let first = days_from_civil(ny, nm, 1);
    let next = if nm == 12 { days_from_civil(ny + 1, 1, 1) } else { days_from_civil(ny, nm + 1, 1) };
    first + (d as i64 - 1).min(next - first - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Days,
    Hours,
    Weeks,
    Months,
}

impl Unit {
    pub fn parse(s: &str) -> Result<Unit, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "day" | "days" => Ok(Unit::Days),
            "hour" | "hours" => Ok(Unit::Hours),
            "week" | "weeks" => Ok(Unit::Weeks),
            "month" | "months" => Ok(Unit::Months),
            o => Err(format!("unit `{o}` is not one of: days, hours, weeks, months")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Hours => "hours",
            Unit::Weeks => "weeks",
            Unit::Months => "months",
        }
    }
}

/// How the sessions are spaced.
pub fn parse_every(s: &str) -> Result<&'static str, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "daily" | "day" | "every day" | "everyday" => Ok("daily"),
        "weekdays" | "weekday" => Ok("weekdays"),
        "weekly" | "week" => Ok("weekly"),
        o => Err(format!("every `{o}` is not one of: daily, weekdays, weekly")),
    }
}

/// What the model asked for (the syllabus lines are its own; everything else is checked here).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PlanSpec {
    pub topic: String,
    pub unit: String,
    pub count: u64,
    pub start: Option<String>,
    pub time: Option<String>,
    pub minutes: Option<u64>,
    pub every: Option<String>,
    pub sessions: Vec<(String, String)>,
}

impl PlanSpec {
    pub fn from_args(a: &Value) -> Result<PlanSpec, String> {
        let text = |k: &str| a.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let num = |k: &str| a.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())));
        let mut sessions = Vec::new();
        let arr = a.get("sessions").and_then(Value::as_array).ok_or("`sessions` must be a list of {title, notes}, one entry per session")?;
        for (i, s) in arr.iter().enumerate() {
            let title = s.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            let notes = s.get("notes").and_then(Value::as_str).unwrap_or("").to_string();
            if s.get("title").is_none() {
                return Err(format!("session {} has no `title`", i + 1));
            }
            sessions.push((title, notes));
        }
        Ok(PlanSpec {
            topic: text("topic").ok_or("`topic` is required")?,
            unit: text("unit").ok_or("`unit` is required: days, hours, weeks or months")?,
            count: num("count").ok_or("`count` is required: a whole number of days, hours, weeks or months")?,
            start: text("start"),
            time: text("time"),
            minutes: num("minutes"),
            every: text("every"),
            sessions,
        })
    }
}

/// How many sessions a request makes. Days and weeks: one per day or week. Hours: the hours cut into sessions of `minutes`
/// (the last may be shorter). Months: one per week, counted over the real calendar months from `start`.
pub fn expected_sessions(unit: Unit, count: u32, minutes: u32, start: i64) -> usize {
    match unit {
        Unit::Days | Unit::Weeks => count as usize,
        Unit::Hours => (count as usize * 60).div_ceil(minutes as usize),
        Unit::Months => ((add_months(start, count) - start) as usize).div_ceil(7),
    }
}

/// The dates of `n` sessions from `start`.
pub fn dates(start: i64, n: usize, every: &str) -> Vec<i64> {
    let mut out = Vec::with_capacity(n);
    match every {
        "weekly" => out.extend((0..n as i64).map(|i| start + 7 * i)),
        "weekdays" => {
            let mut d = start;
            while out.len() < n {
                if !matches!(weekday(d), 0 | 6) {
                    out.push(d);
                }
                d += 1;
            }
        }
        _ => out.extend((0..n as i64).map(|i| start + i)),
    }
    out
}

/// The phrases that make "exactly one correct answer" impossible to check.
const AMBIGUOUS: [&str; 6] = ["all of the above", "none of the above", "all the above", "both of the above", "all of these", "none of these"];

pub fn parse_answer(v: Option<&Value>, n_options: usize) -> Result<usize, String> {
    let v = v.ok_or("has no `answer`")?;
    let idx = match v {
        Value::Number(n) => n.as_u64().map(|x| x as usize).ok_or("`answer` must be a whole number")?,
        Value::String(s) => {
            let t = s.trim();
            match t.parse::<usize>() {
                Ok(i) => i,
                Err(_) if t.len() == 1 && t.as_bytes()[0].is_ascii_alphabetic() => (t.as_bytes()[0].to_ascii_uppercase() - b'A') as usize,
                Err(_) => return Err(format!("`answer` “{t}” is not an option number")),
            }
        }
        Value::Array(_) => return Err("has several answers; there must be exactly ONE correct option: give a single index".into()),
        _ => return Err("`answer` must be the 0-based index of the correct option".into()),
    };
    if idx >= n_options {
        return Err(format!("`answer` {idx} is out of range: the options are numbered 0 to {}", n_options.saturating_sub(1)));
    }
    Ok(idx)
}

/// All the rules for a quiz from the model, checked together so one retry can fix everything.
pub fn validate_quiz(topic: &str, count: u64, questions: &Value) -> Result<(String, Vec<Question>), String> {
    let mut problems = Vec::new();
    let topic = clean(topic, MAX_TOPIC, "the topic", false, &mut problems);
    if topic.is_empty() {
        problems.push("the topic is empty".into());
    }
    if count == 0 || count > MAX_QUESTIONS as u64 {
        problems.push(format!("count must be from 1 to {MAX_QUESTIONS}"));
    }
    let Some(arr) = questions.as_array() else {
        return Err("the quiz was not saved: `questions` must be a list of {question, options, answer, explanation}".into());
    };
    if arr.len() as u64 != count {
        problems.push(format!("the person asked for {count} questions but you sent {}; send exactly {count}", arr.len()));
    }
    if arr.len() > MAX_QUESTIONS {
        problems.push(format!("a quiz has at most {MAX_QUESTIONS} questions"));
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (i, q) in arr.iter().take(MAX_QUESTIONS).enumerate() {
        let at = format!("question {}", i + 1);
        let text = clean(q.get("question").and_then(Value::as_str).unwrap_or(""), MAX_QUESTION, &format!("{at}'s text"), true, &mut problems);
        if text.is_empty() {
            problems.push(format!("{at} has no text"));
        } else if !seen.insert(norm(&text)) {
            problems.push(format!("{at} repeats an earlier question"));
        }
        let raw: Vec<&str> = q.get("options").and_then(Value::as_array).map(|a| a.iter().map(|o| o.as_str().unwrap_or("")).collect()).unwrap_or_default();
        if !(2..=6).contains(&raw.len()) {
            problems.push(format!("{at} has {} options; give 2 to 6 (usually 4)", raw.len()));
        }
        let options: Vec<String> = raw.iter().enumerate().map(|(k, o)| clean(o, MAX_OPTION, &format!("{at}, option {k}"), false, &mut problems)).collect();
        if options.iter().any(String::is_empty) {
            problems.push(format!("{at} has an empty option"));
        }
        let mut distinct = HashSet::new();
        if options.iter().any(|o| !distinct.insert(o.to_lowercase())) {
            problems.push(format!("{at} has two options that are the same (ignoring capitals)"));
        }
        if options.iter().any(|o| AMBIGUOUS.iter().any(|a| o.to_lowercase().contains(a))) {
            problems.push(format!("{at} has an option like “all of the above”; rewrite it so exactly one option is correct on its own"));
        }
        let answer = match parse_answer(q.get("answer"), options.len()) {
            Ok(a) => a,
            Err(e) => {
                problems.push(format!("{at} {e}"));
                0
            }
        };
        let explanation = clean(q.get("explanation").and_then(Value::as_str).unwrap_or(""), MAX_EXPLAIN, &format!("{at}'s explanation"), true, &mut problems);
        if explanation.is_empty() {
            problems.push(format!("{at} has no explanation of why the answer is right"));
        }
        out.push(Question { text, options, answer, explanation, flagged: false });
    }
    if !problems.is_empty() {
        return Err(format!("the quiz was not saved; fix these and call quiz_create again (check every answer index against its option before sending):\n- {}", problems.join("\n- ")));
    }
    Ok((topic, out))
}

// ----- text for the tools -----

pub fn plan_line(p: &Plan) -> String {
    format!("[{}] {} — {} {} ({} sessions, {} done){}", p.id, p.topic, p.count, p.unit, p.sessions.len(), p.done_count(), if p.events.is_empty() { "" } else { ", in the calendar" })
}

pub fn plan_text(p: &Plan) -> String {
    let mut out = plan_line(p);
    for (i, s) in p.sessions.iter().enumerate() {
        out.push_str(&format!("\n{:>3}. {} {} ({} min){} {}{}", i + 1, s.date, s.time, s.minutes, if s.done { " [done]" } else { "" }, s.title, if s.notes.is_empty() { String::new() } else { format!(" — {}", s.notes.replace('\n', " ")) }));
    }
    out
}

pub fn quiz_line(q: &Quiz) -> String {
    let last = q.attempts.last().map(|a| score(q, &a.answers));
    format!("[{}] {} — {} questions, {}", q.id, q.topic, q.questions.len(), last.map_or("not taken yet".to_string(), |s| format!("last score {}/{} ({}%), {} attempt(s)", s.correct, s.counted, s.percent, q.attempts.len())))
}

/// The last attempt for the coach: the score and the wrong questions (never before the person has answered).
pub fn results_text(q: &Quiz) -> String {
    let Some(a) = q.attempts.last() else {
        return format!("{}\nThe person has not taken this quiz yet, so there is no score. Do not guess; offer quiz_open to bring them to it.", quiz_line(q));
    };
    let s = score(q, &a.answers);
    let mut out = format!("{}\nScore {}/{} = {}%{}.", quiz_line(q), s.correct, s.counted, s.percent, if s.flagged > 0 { format!(" ({} question(s) reported as wrong are not counted)", s.flagged) } else { String::new() });
    for it in s.items.iter().filter(|i| !i.right && !i.flagged) {
        let qu = &q.questions[it.index];
        out.push_str(&format!(
            "\nWrong: Q{} {}\n  picked: {}\n  correct: {}\n  why: {}",
            it.index + 1,
            qu.text,
            it.picked.map_or("(skipped)".to_string(), |p| qu.options[p].clone()),
            qu.options[qu.answer],
            qu.explanation
        ));
    }
    for (i, qu) in q.questions.iter().enumerate().filter(|(_, q)| q.flagged) {
        out.push_str(&format!("\nReported wrong by the person (not counted): Q{} {}", i + 1, qu.text));
    }
    out
}
