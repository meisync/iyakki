//! LEARN as PDF files (F-STUDY-07): a study plan (syllabus and day list), a quiz (the questions, and on a page of its own the answer key,
//! which is off unless asked for) and the lesson of one day, written with the in-house PDF writer (`iyakki-pdf`) from what the store
//! holds: the model is not asked to make the file. The chat tool `study_export_pdf` and the LEARN tile's Export PDF buttons both come
//! through [`export`], so they write the same file under the same rules: the person's file rules, a free name, never a replaced file,
//! and text the standard fonts cannot write (Tamil, Hindi, ...) refused in plain words (F-PDF-11 will lift that).

use serde_json::Value;

use crate::pdftools::{decode_picture, downloads_for, finish_pdf, pdf_date, target_in, write_new};
use crate::study::{Plan, Quiz, StudyStore};
use crate::studylesson::Lesson;
use iyakki_pdf::md::{fold, markdown_blocks};
use iyakki_pdf::{Block, PageSize};

/// What to export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Plan(String),
    /// `key`: put the answer key on a page of its own after the questions.
    Quiz { id: String, key: bool },
    /// Day `n` (1-based) of a plan.
    Lesson { plan: String, n: usize },
}

pub struct Made {
    pub title: String,
    pub bytes: Vec<u8>,
    pub pages: usize,
}

fn letter(i: usize) -> char {
    (b'A' + (i as u8).min(25)) as char
}

pub fn plan_blocks(p: &Plan) -> Vec<Block> {
    let mut v = vec![Block::Heading(1, format!("Study plan: {}", p.topic)), Block::Quote(format!("{} {}, {} - {} sessions, {} done", p.count, p.unit, p.every, p.sessions.len(), p.done_count()))];
    v.push(Block::Heading(2, "Syllabus".into()));
    for (i, s) in p.sessions.iter().enumerate() {
        v.push(Block::Bullet(0, format!("{}.", i + 1), s.title.clone()));
        let notes = s.notes.split_whitespace().collect::<Vec<_>>().join(" ");
        if !notes.is_empty() {
            v.push(Block::Bullet(1, "\u{2022}".into(), notes));
        }
    }
    v.push(Block::Heading(2, "Day by day".into()));
    let mut rows = vec![format!("{:<4} {:<11} {:<6} {:<5} {}", "Day", "Date", "Time", "Min", "Done")];
    for (i, s) in p.sessions.iter().enumerate() {
        rows.push(format!("{:<4} {:<11} {:<6} {:<5} {}", i + 1, s.date, s.time, s.minutes, if s.done { "yes" } else { "" }));
    }
    v.push(Block::Code(rows.join("\n")));
    v
}

/// The questions that count (a question the person reported as wrong is left out), as (original index, question).
fn counted(q: &Quiz) -> Vec<(usize, &crate::study::Question)> {
    q.questions.iter().enumerate().filter(|(_, x)| !x.flagged).collect()
}

pub fn quiz_blocks(q: &Quiz, key: bool) -> Result<Vec<Block>, String> {
    let qs = counted(q);
    if qs.is_empty() {
        return Err("every question of this quiz was reported as wrong, so there is nothing to print".into());
    }
    let mut v = vec![Block::Heading(1, format!("Quiz: {}", q.topic)), Block::Quote(format!("{} questions. Choose the best answer for each one.", qs.len())), Block::Para("Name: ______________________    Date: ______________".into())];
    for (k, (_, x)) in qs.iter().enumerate() {
        v.push(Block::Para(format!("{}. {}", k + 1, x.text)));
        for (i, o) in x.options.iter().enumerate() {
            v.push(Block::Bullet(1, format!("{})", letter(i)), o.clone()));
        }
    }
    if key {
        v.push(Block::PageBreak);
        v.push(Block::Heading(1, format!("Answer key: {}", q.topic)));
        for (k, (_, x)) in qs.iter().enumerate() {
            let why = x.explanation.split_whitespace().collect::<Vec<_>>().join(" ");
            v.push(Block::Bullet(0, format!("{}.", k + 1), format!("{}) {}{}", letter(x.answer), x.options[x.answer], if why.is_empty() { String::new() } else { format!(" - {why}") })));
        }
    }
    Ok(v)
}

/// A lesson with its pictures (`image(file)` gives the bytes of a picture file of the lesson).
pub fn lesson_blocks(p: &Plan, n: usize, l: &Lesson, image: &dyn Fn(&str) -> Option<Vec<u8>>) -> Vec<Block> {
    let s = n.checked_sub(1).and_then(|i| p.sessions.get(i));
    let mut v = vec![Block::Heading(1, format!("Day {n}: {}", s.map_or(p.topic.as_str(), |s| s.title.as_str())))];
    v.push(Block::Quote(format!("{} - {}{}", p.topic, s.map_or(String::new(), |s| format!("{} {}, {} min", s.date, s.time, s.minutes)), match l.level {
        -1 => ", simpler version",
        1 => ", deeper version",
        _ => "",
    })));
    for sec in &l.sections {
        v.push(Block::Heading(2, sec.title.clone()));
        // a section is plain text with the odd list or code; it is read the way the chat's text is, and no picture address in it is followed
        match markdown_blocks(&sec.text, &mut |_| Err("pictures in the text are not followed".into())) {
            Ok(b) if !b.is_empty() => v.extend(b),
            _ => v.push(Block::Para(sec.text.split_whitespace().collect::<Vec<_>>().join(" "))),
        }
        for &pi in &sec.pics {
            let Some(pic) = l.pictures.get(pi) else { continue };
            if pic.file.is_empty() {
                continue;
            }
            if let Some(img) = image(&pic.file).and_then(|b| decode_picture(&b)) {
                v.push(Block::Image(img));
                if !pic.caption.trim().is_empty() {
                    v.push(Block::Quote(pic.caption.trim().to_string()));
                }
            }
        }
    }
    if !l.qa.is_empty() {
        v.push(Block::Heading(2, "Questions and answers".into()));
        for qa in &l.qa {
            v.push(Block::Heading(3, qa.q.clone()));
            v.push(Block::Para(qa.a.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
    }
    v
}

/// Build the file for `item` from the store.
pub fn make(study: &StudyStore, user: &str, item: &Item, size: PageSize, date: Option<String>) -> Result<Made, String> {
    let (title, mut blocks) = match item {
        Item::Plan(id) => {
            let p = study.plan(user, id)?;
            (format!("Study plan: {}", p.topic), plan_blocks(&p))
        }
        Item::Quiz { id, key } => {
            let q = study.quiz(user, id)?;
            (format!("Quiz: {}", q.topic), quiz_blocks(&q, *key)?)
        }
        Item::Lesson { plan, n } => {
            let p = study.plan(user, plan)?;
            let l = study.lesson(user, plan, *n).ok_or_else(|| format!("day {n} of this plan has no lesson written yet: open it in LEARN (or study_lesson_open) so it is written first"))?;
            let (u, pl) = (user.to_string(), plan.clone());
            let n = *n;
            let title = format!("{}: day {n}", p.topic);
            (title, lesson_blocks(&p, n, &l, &|f| study.lesson_image(&u, &pl, n, f)))
        }
    };
    for b in blocks.iter_mut() {
        match b {
            Block::Heading(_, t) | Block::Para(t) | Block::Quote(t) | Block::Code(t) => *t = fold(t),
            Block::Bullet(_, m, t) => {
                *m = fold(m);
                *t = fold(t);
            }
            _ => {}
        }
    }
    let title = fold(&title);
    let (bytes, pages) = finish_pdf(&title, "IYAKKI LEARN", size, blocks, date)?;
    Ok(Made { title, bytes, pages })
}

/// Make the file and save it under the person's file rules. `path`: a file to make (it must not exist), else a free name in Downloads.
pub fn export(study: &StudyStore, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, item: &Item, size: PageSize, path: Option<&str>) -> Result<String, String> {
    let made = make(study, user, item, size, Some(pdf_date(crate::clock::now_unix())))?;
    let target = target_in(policy, user, &downloads_for(policy, user, data_dir), path.map(str::trim).filter(|p| !p.is_empty()), &made.title)?;
    write_new(&target, &made.bytes)?;
    Ok(format!("Saved {} ({} page{}, {}, {} KB).", target.display(), made.pages, if made.pages == 1 { "" } else { "s" }, if size == PageSize::A4 { "A4" } else { "Letter" }, made.bytes.len() / 1024 + 1))
}

/// The `study_export_pdf` tool's arguments as an item.
pub fn item_from_args(a: &Value) -> Result<Item, String> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    match s("kind").map(str::to_lowercase).as_deref() {
        Some("plan") => Ok(Item::Plan(s("id").ok_or("`id` (the plan) is required")?.to_string())),
        Some("quiz") => Ok(Item::Quiz { id: s("id").ok_or("`id` (the quiz) is required")?.to_string(), key: a.get("answer_key").and_then(Value::as_bool).unwrap_or(false) }),
        Some("lesson") => {
            let n = a.get("session").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))).filter(|n| *n >= 1).ok_or("`session` (the day, 1, 2, ...) is required for a lesson")?;
            Ok(Item::Lesson { plan: s("id").ok_or("`id` (the plan) is required")?.to_string(), n: n as usize })
        }
        _ => Err("`kind` must be plan, quiz or lesson".into()),
    }
}

pub fn size_from_args(a: &Value) -> Result<PageSize, String> {
    match a.get("size").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(PageSize::A4),
        Some(s) => PageSize::from_name(s).ok_or_else(|| format!("`{s}` is not a page size here: use A4 or Letter")),
    }
}
