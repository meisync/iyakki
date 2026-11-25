//! The study store: plans and quizzes of a person (docs/STUDY.md). The data types and the pure rules live in the leaf crate
//! `iyakki-study-model` (alpha.8 E3, D-S2-01 option (a)) and are re-exported here under the old path; this file keeps the store itself
//! (the person's file, its lock, the calendar link).

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::calendar::{parse_hm, CalendarStore, Repeat};
use iyakki_clock::{civil, days_from_civil, Zone};
pub use iyakki_study_model::study::*;

#[derive(Default, Serialize, Deserialize)]
struct Data {
    #[serde(default)]
    plans: Vec<Plan>,
    #[serde(default)]
    quizzes: Vec<Quiz>,
}

pub struct StudyStore {
    pub dir: PathBuf,
    lock: Mutex<()>,
}

impl StudyStore {
    pub fn new(dir: &std::path::Path) -> StudyStore {
        StudyStore { dir: dir.into(), lock: Mutex::new(()) }
    }

    fn path(&self, user: &str) -> Result<PathBuf, String> {
        if iyakki_sec::policy::valid_username(user) { Ok(self.dir.join(format!("{user}.json"))) } else { Err("invalid user".into()) }
    }

    fn load(&self, user: &str) -> Result<Data, String> {
        match std::fs::read(self.path(user)?) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("the study file is damaged: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Data::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, user: &str, d: &Data) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(user)?;
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&serde_json::to_vec_pretty(d).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ----- plans -----

    /// Check the request and the syllabus, work out the dates, and keep the plan.
    pub fn create_plan(&self, user: &str, spec: &PlanSpec, now: i64, zone: &Zone) -> Result<Plan, String> {
        let mut problems = Vec::new();
        let topic = clean(&spec.topic, MAX_TOPIC, "the topic", false, &mut problems);
        if topic.is_empty() {
            problems.push("the topic is empty".into());
        }
        let unit = Unit::parse(&spec.unit).map_err(|e| e.to_string())?;
        let (max_count, what) = match unit {
            Unit::Days => (MAX_SESSIONS as u64, "366 days"),
            Unit::Hours => (1000, "1000 hours"),
            Unit::Weeks => (52, "52 weeks"),
            Unit::Months => (12, "12 months"),
        };
        if spec.count == 0 || spec.count > max_count {
            return Err(format!("count must be from 1 to {what} for unit {}", unit.name()));
        }
        let count = spec.count as u32;
        let minutes = spec.minutes.unwrap_or(30);
        if !(5..=480).contains(&minutes) {
            return Err("minutes per session must be from 5 to 480".into());
        }
        let minutes = minutes as u32;
        let every = match (&spec.every, unit) {
            (None, Unit::Days | Unit::Hours) => "daily",
            (None, _) => "weekly",
            (Some(e), _) => parse_every(e)?,
        };
        if matches!(unit, Unit::Weeks | Unit::Months) && every != "weekly" {
            return Err(format!("a plan in {} has one session a week: leave `every` out or use weekly", unit.name()));
        }
        let time = spec.time.as_deref().unwrap_or("18:00");
        let (h, mi) = parse_hm(time).ok_or_else(|| format!("time `{time}` must look like 18:00 or 6:30pm"))?;
        let start = match spec.start.as_deref() {
            Some(s) => parse_date(s)?,
            None => {
                let l = zone.local(now);
                days_from_civil(l.year, l.month, l.day) + 1
            }
        };
        let want = expected_sessions(unit, count, minutes, start);
        if want > MAX_SESSIONS {
            return Err(format!("that makes {want} sessions; a plan may have at most {MAX_SESSIONS}. Use fewer {} or longer sessions.", unit.name()));
        }
        if spec.sessions.len() != want {
            return Err(format!(
                "this plan needs exactly {want} sessions ({} {} {}), but you sent {}. Write exactly {want} sessions, one {{title, notes}} per session in order, and call study_plan_create again.",
                count,
                unit.name(),
                match unit {
                    Unit::Days => "= one a day".to_string(),
                    Unit::Weeks => "= one a week".to_string(),
                    Unit::Hours => format!("in sessions of {minutes} minutes"),
                    Unit::Months => format!("from {} = one a week", date_text(start)),
                },
                spec.sessions.len()
            ));
        }
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for (i, (title, notes)) in spec.sessions.iter().enumerate() {
            let t = clean(title, MAX_TITLE, &format!("the title of session {}", i + 1), false, &mut problems);
            if t.is_empty() {
                problems.push(format!("session {} has an empty title", i + 1));
            } else if !seen.insert(norm(&t)) {
                problems.push(format!("session {} repeats the title “{t}”; every session needs its own title", i + 1));
            }
            rows.push((t, clean(notes, MAX_NOTES, &format!("the notes of session {}", i + 1), true, &mut problems)));
        }
        if !problems.is_empty() {
            return Err(format!("the plan was not saved; fix these and call study_plan_create again:\n- {}", problems.join("\n- ")));
        }
        let days = dates(start, want, every);
        let total = count * 60;
        let sessions = rows
            .into_iter()
            .zip(days)
            .enumerate()
            .map(|(i, ((title, notes), d))| {
                // hours: every session is `minutes` long, the last one only what is left of the total
                let m = if unit == Unit::Hours { minutes.min(total - minutes * i as u32) } else { minutes };
                Session { date: date_text(d), time: format!("{h:02}:{mi:02}"), minutes: m, title, notes, done: false }
            })
            .collect();
        let _g = self.guard();
        let mut data = self.load(user)?;
        if data.plans.len() >= MAX_PLANS {
            return Err(format!("there are already {MAX_PLANS} plans; delete one first"));
        }
        let plan = Plan { id: new_id(), topic, created: now, unit: unit.name().into(), count, every: every.into(), sessions, events: vec![] };
        data.plans.push(plan.clone());
        self.save(user, &data)?;
        Ok(plan)
    }

    pub fn plans(&self, user: &str) -> Vec<Plan> {
        self.load(user).map(|d| d.plans).unwrap_or_default()
    }

    pub fn plan(&self, user: &str, id: &str) -> Result<Plan, String> {
        self.load(user)?.plans.into_iter().find(|p| p.id == id).ok_or_else(|| format!("no plan with id {id}"))
    }

    /// Tick a session (1-based) done or not done.
    pub fn set_done(&self, user: &str, id: &str, session: usize, done: bool) -> Result<Plan, String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let p = data.plans.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("no plan with id {id}"))?;
        if session == 0 || session > p.sessions.len() {
            return Err(format!("session must be from 1 to {}", p.sessions.len()));
        }
        p.sessions[session - 1].done = done;
        let out = p.clone();
        self.save(user, &data)?;
        Ok(out)
    }

    /// Delete a plan together with the calendar events it added.
    pub fn delete_plan(&self, user: &str, id: &str, cal: &CalendarStore) -> Result<usize, String> {
        let removed = self.calendar_remove(user, id, cal).unwrap_or(0);
        let _g = self.guard();
        let mut data = self.load(user)?;
        let n = data.plans.len();
        data.plans.retain(|p| p.id != id);
        if data.plans.len() == n {
            return Err(format!("no plan with id {id}"));
        }
        self.save(user, &data)?;
        drop(_g);
        // the written lessons of the plan go with it
        self.delete_lessons_of(user, id);
        Ok(removed)
    }

    /// One calendar event per session, tagged `assistant` with the plan id in the notes. Only once per plan; if any event
    /// cannot be made, the ones already made are removed again so nothing is left half done.
    pub fn to_calendar(&self, user: &str, id: &str, cal: &CalendarStore, zone: &Zone) -> Result<usize, String> {
        self.to_calendar_with(user, id, cal, zone, None)
    }

    /// As `to_calendar`, with every event ringing `remind` minutes before it starts (0 = when it starts) so no separate alarm is needed.
    pub fn to_calendar_with(&self, user: &str, id: &str, cal: &CalendarStore, zone: &Zone, remind: Option<u32>) -> Result<usize, String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let p = data.plans.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("no plan with id {id}"))?;
        if !p.events.is_empty() {
            return Err(format!("plan {id} is already in the calendar ({} events); remove them first with study_plan_calendar_remove", p.events.len()));
        }
        let mut made: Vec<String> = Vec::new();
        for s in &p.sessions {
            let ev = (|| -> Result<String, String> {
                let (y, m, d) = civil(parse_date(&s.date)?);
                let (h, mi) = parse_hm(&s.time).ok_or("bad time")?;
                let notes = if s.notes.is_empty() { format!("[study:{id}]") } else { format!("{}\n[study:{id}]", s.notes) };
                let title = format!("{}: {}", p.topic, s.title);
                cal.add_event(user, &title.chars().take(120).collect::<String>(), zone.local_to_unix(y, m, d, h, mi, 0), s.minutes, false, Repeat::None, &notes, "assistant", remind).map(|e| e.id)
            })();
            match ev {
                Ok(e) => made.push(e),
                Err(e) => {
                    for m in &made {
                        let _ = cal.remove_event(user, m);
                    }
                    return Err(format!("the calendar refused session “{}”: {e}. Nothing was added.", s.title));
                }
            }
        }
        p.events = made;
        let n = p.events.len();
        self.save(user, &data)?;
        Ok(n)
    }

    /// Remove the calendar events of a plan: those it recorded, and any assistant event that carries its tag.
    pub fn calendar_remove(&self, user: &str, id: &str, cal: &CalendarStore) -> Result<usize, String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let p = data.plans.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("no plan with id {id}"))?;
        let tag = format!("[study:{id}]");
        let mut ids: HashSet<String> = p.events.iter().cloned().collect();
        ids.extend(cal.events(user).into_iter().filter(|e| e.by == "assistant" && e.notes.contains(&tag)).map(|e| e.id));
        let mut removed = 0;
        for e in ids {
            if cal.remove_event(user, &e).is_ok() {
                removed += 1;
            }
        }
        p.events.clear();
        self.save(user, &data)?;
        Ok(removed)
    }

    // ----- quizzes -----

    /// Validate what the model sent and keep the quiz. Every problem found is listed in the error.
    pub fn create_quiz(&self, user: &str, topic: &str, count: u64, questions: &Value, now: i64) -> Result<Quiz, String> {
        let (topic, qs) = validate_quiz(topic, count, questions)?;
        let _g = self.guard();
        let mut data = self.load(user)?;
        if data.quizzes.len() >= MAX_QUIZZES {
            return Err(format!("there are already {MAX_QUIZZES} quizzes; delete one first"));
        }
        let quiz = Quiz { id: new_id(), topic, created: now, questions: qs, attempts: vec![], progress: vec![] };
        data.quizzes.push(quiz.clone());
        self.save(user, &data)?;
        Ok(quiz)
    }

    pub fn quizzes(&self, user: &str) -> Vec<Quiz> {
        self.load(user).map(|d| d.quizzes).unwrap_or_default()
    }

    pub fn quiz(&self, user: &str, id: &str) -> Result<Quiz, String> {
        self.load(user)?.quizzes.into_iter().find(|q| q.id == id).ok_or_else(|| format!("no quiz with id {id}"))
    }

    pub fn delete_quiz(&self, user: &str, id: &str) -> Result<(), String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let n = data.quizzes.len();
        data.quizzes.retain(|q| q.id != id);
        if data.quizzes.len() == n {
            return Err(format!("no quiz with id {id}"));
        }
        self.save(user, &data)
    }

    /// Keep one finished attempt. Returns its score.
    pub fn record_attempt(&self, user: &str, id: &str, answers: &[Option<usize>], now: i64) -> Result<Scored, String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let q = data.quizzes.iter_mut().find(|q| q.id == id).ok_or_else(|| format!("no quiz with id {id}"))?;
        if answers.len() != q.questions.len() {
            return Err(format!("expected {} answers, got {}", q.questions.len(), answers.len()));
        }
        if answers.iter().zip(&q.questions).any(|(a, qu)| a.is_some_and(|i| i >= qu.options.len())) {
            return Err("an answer points at an option that does not exist".into());
        }
        q.attempts.push(Attempt { taken: now, answers: answers.to_vec() });
        if q.attempts.len() > MAX_ATTEMPTS {
            q.attempts.remove(0);
        }
        let s = score(q, answers);
        self.save(user, &data)?;
        Ok(s)
    }

    /// The next question of a quiz taken in the chat: (its 0-based index, the question). The answer key stays inside; only
    /// the text and options are for the person. None when every question has been answered.
    pub fn chat_next(&self, user: &str, id: &str) -> Result<Option<(usize, Question)>, String> {
        let q = self.quiz(user, id)?;
        let done = q.progress.iter().position(|a| a.is_none()).unwrap_or(q.progress.len());
        Ok(q.questions.get(done).cloned().map(|x| (done, x)))
    }

    /// Record the person's answer to the next question (`question` is 1-based, `option` an option letter such as `B`).
    /// Returns whether it was right, the key and why, and the final score once the last question is answered (the attempt
    /// is then kept and the chat session cleared).
    pub fn chat_answer(&self, user: &str, id: &str, question: usize, option: &str, now: i64) -> Result<ChatAnswer, String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let q = data.quizzes.iter_mut().find(|q| q.id == id).ok_or_else(|| format!("no quiz with id {id}"))?;
        if q.progress.len() != q.questions.len() {
            q.progress = vec![None; q.questions.len()];
        }
        let next = q.progress.iter().position(Option::is_none).unwrap_or(q.questions.len());
        if question == 0 || question - 1 != next {
            return Err(format!("the next question to answer is {}, not {question}. Ask one question at a time, in order.", next + 1));
        }
        let qu = &q.questions[next];
        let t = option.trim();
        let pick = if t.chars().count() == 1 && t.as_bytes()[0].is_ascii_alphabetic() {
            (t.as_bytes()[0].to_ascii_uppercase() - b'A') as usize
        } else {
            qu.options.iter().position(|o| o.eq_ignore_ascii_case(t)).unwrap_or(usize::MAX)
        };
        if pick >= qu.options.len() {
            return Err(format!("`option` must be a letter A to {} (or the option's exact text)", (b'A' + qu.options.len() as u8 - 1) as char));
        }
        q.progress[next] = Some(pick);
        let (right, key, why, flagged) = (pick == qu.answer, qu.options[qu.answer].clone(), qu.explanation.clone(), qu.flagged);
        let finished = if next + 1 == q.questions.len() {
            let answers = std::mem::take(&mut q.progress);
            let s = score(q, &answers);
            q.attempts.push(Attempt { taken: now, answers });
            if q.attempts.len() > MAX_ATTEMPTS {
                q.attempts.remove(0);
            }
            Some(s)
        } else {
            None
        };
        let total = q.questions.len();
        self.save(user, &data)?;
        Ok(ChatAnswer { right, key, why, flagged, answered: next + 1, total, finished })
    }

    /// Start the chat quiz over (forget the answers so far).
    pub fn chat_restart(&self, user: &str, id: &str) -> Result<(), String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        data.quizzes.iter_mut().find(|q| q.id == id).ok_or_else(|| format!("no quiz with id {id}"))?.progress.clear();
        self.save(user, &data)
    }

    /// Report a question as wrong (or take the report back). It stops counting in every score, past attempts included.
    pub fn flag_question(&self, user: &str, id: &str, question: usize, flagged: bool) -> Result<(), String> {
        let _g = self.guard();
        let mut data = self.load(user)?;
        let q = data.quizzes.iter_mut().find(|q| q.id == id).ok_or_else(|| format!("no quiz with id {id}"))?;
        q.questions.get_mut(question).ok_or_else(|| format!("no question {}", question + 1))?.flagged = flagged;
        self.save(user, &data)
    }
}
