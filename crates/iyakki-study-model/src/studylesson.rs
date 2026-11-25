//! Types moved out of `iyakki_agent::studylesson` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub text: String,
    /// Indexes into [`Lesson::pictures`] drawn after this section's text.
    #[serde(default)]
    pub pics: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    pub caption: String,
    /// What the picture should show (kept to draw it again).
    pub brief: String,
    /// The file next to the lesson (`pic1.svg`); empty until drawn.
    #[serde(default)]
    pub file: String,
    /// How it was made: `image model`, `drawn by the assistant` or `placeholder`.
    #[serde(default)]
    pub how: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Qa {
    pub q: String,
    pub a: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lesson {
    pub plan: String,
    /// 1-based session number.
    pub session: usize,
    /// -1 simpler, 0 normal, 1 deeper.
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub created: i64,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub pictures: Vec<Picture>,
    #[serde(default)]
    pub qa: Vec<Qa>,
}

impl Lesson {
    /// The lesson as plain text, for reading aloud and for the follow-up questions.
    pub fn plain(&self) -> String {
        self.sections.iter().map(|s| format!("{}.\n{}", s.title, s.text)).collect::<Vec<_>>().join("\n\n")
    }
}

/// What came out of one reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub sections: Vec<Section>,
    pub pictures: Vec<Picture>,
}

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// The lesson of one day of a study plan (the LEARN tile, `docs/STUDY.md`): the assistant writes a short text lesson for the session
// (what to learn, an explanation, a worked example, what to do step by step, a practice task, a recap) with one to three pictures.
//
// It follows how TALES works (`story.rs`): the model gets no tools and only writes; the reply is parsed while it streams; the code
// (never the model) strips markup and caps every size; pictures come from the picture model when there is one, else the writing
// model draws an SVG that goes through the same sanitiser as TALES' drawings (`story::sanitize_svg`), else the code draws a plain
// placeholder, so a lesson never lacks its pictures. A finished lesson is saved next to the plan (one JSON file written
// atomically with owner-only permissions, the pictures as files beside it) so reopening it is instant and needs no model.

use crate::study::Plan;

/// Caps the code applies to whatever the model wrote.
pub const MAX_SECTIONS: usize = 8;
pub const MAX_SECTION_CHARS: usize = 1800;
pub const MAX_LESSON_CHARS: usize = 9000;
pub const MAX_PICTURES: usize = 3;
pub const MAX_CAPTION: usize = 120;
pub const MAX_BRIEF: usize = 300;
pub const MAX_TITLE: usize = 60;
pub const MAX_QA: usize = 12;
pub const MAX_ANSWER: usize = 1500;
pub const MAX_QUESTION: usize = 400;

/// The headings of a lesson, in order.
pub const HEADINGS: [&str; 5] = ["Explanation", "Worked example", "What to do", "Practice task", "Recap"];






// ------------------------------------------------------------------------------------------------------------ what to learn today

/// "What you will learn today": made by the code from the session's title and notes (and its neighbours), so it can be shown at
/// once, before any model has answered.
pub fn goals(plan: &Plan, n: usize) -> Vec<String> {
    let mut v = Vec::new();
    let Some(s) = n.checked_sub(1).and_then(|i| plan.sessions.get(i)) else { return v };
    v.push(format!("Today, day {n} of {}: {}", plan.sessions.len(), s.title));
    for part in s.notes.split(['\n', ';']).flat_map(|l| l.split(". ")) {
        let t = part.trim().trim_end_matches('.').trim();
        if !t.is_empty() {
            v.push(t.chars().take(160).collect());
        }
    }
    v.push(format!("This is part of: {}. About {} minutes.", plan.topic, s.minutes));
    if let Some(p) = n.checked_sub(2).and_then(|i| plan.sessions.get(i)) {
        v.push(format!("Before this: {}", p.title));
    }
    if let Some(x) = plan.sessions.get(n) {
        v.push(format!("Next: {}", x.title));
    }
    v
}

// ------------------------------------------------------------------------------------------------------------ prompts

pub fn level_words(level: i32) -> &'static str {
    match level {
        l if l < 0 => "Make it SIMPLER than usual: short sentences, everyday words, one small idea at a time, no jargon.",
        l if l > 0 => "Go DEEPER than usual: more detail, the reasons behind things, a harder second example, common mistakes.",
        _ => "Pitch it for a beginner who is willing to work.",
    }
}

pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t = t.trim_end().to_string();
        t.push('…');
        t
    }
}

/// Remove `<...>` tags, markdown pictures and links, emphasis marks and control characters from one line.
pub fn clean_line(line: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // a tag: '<' then a letter, '/' or '!' and a '>' soon after
        if c == '<' && chars.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!') {
            if let Some(e) = chars[i..].iter().take(300).position(|x| *x == '>') {
                i += e + 1;
                continue;
            }
        }
        // ![alt](url) is dropped, [text](url) keeps its text
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some(e) = link_end(&chars, i + 1) {
                i = e;
                continue;
            }
        }
        if c == '[' {
            if let Some(e) = link_end(&chars, i) {
                let close = chars[i..].iter().position(|x| *x == ']').unwrap_or(0);
                out.extend(&chars[i + 1..i + close]);
                i = e;
                continue;
            }
        }
        if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) {
            i += 2;
            continue;
        }
        if c == '`' {
            i += 1;
            continue;
        }
        out.push(if c.is_control() && c != '\t' { ' ' } else { c });
        i += 1;
    }
    out.replace('\t', " ")
}

/// The end (exclusive) of a `[text](url)` starting at `start`, when there is one.
pub fn link_end(chars: &[char], start: usize) -> Option<usize> {
    let close = chars[start..].iter().take(200).position(|x| *x == ']')? + start;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().take(500).position(|x| *x == ')')? + close + 2;
    Some(end + 1)
}

pub fn canonical(title: &str) -> String {
    let t = title.trim().trim_matches(|c: char| c == '#' || c == ':' || c == '*' || c == '.').trim();
    let l = t.to_lowercase();
    for h in HEADINGS {
        if l == h.to_lowercase() {
            return h.to_string();
        }
    }
    if l.starts_with("what to do") || l.starts_with("steps") || l.starts_with("step by step") {
        return "What to do".into();
    }
    if l.starts_with("practice") || l.starts_with("exercise") {
        return "Practice task".into();
    }
    if l.starts_with("example") {
        return "Worked example".into();
    }
    if l.starts_with("recap") || l.starts_with("summary") {
        return "Recap".into();
    }
    cut(&clean_line(t), MAX_TITLE)
}


pub fn strip_think(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if let Some(i) = lower.rfind("</think>") {
        return raw[i + 8..].to_string();
    }
    // a thought still being written
    if let Some(i) = lower.find("<think>") {
        return raw[..i].to_string();
    }
    raw.to_string()
}

/// Turn the model's reply (even a half-written one, `done` false) into sections and picture requests. All the caps are applied here.
pub fn parse(raw: &str, done: bool) -> Parsed {
    let text = strip_think(raw);
    let mut lines: Vec<&str> = text.lines().collect();
    // a picture line or heading that is still being typed is not shown yet
    if !done {
        if let Some(last) = lines.last() {
            let t = last.trim_start();
            if (t.starts_with("[PICTURE") || t.starts_with("[picture") || "[PICTURE".starts_with(t) && !t.is_empty()) && !t.contains(']') {
                lines.pop();
            }
        }
    }
    let mut p = Parsed::default();
    let mut total = 0usize;
    for line in lines {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("##") {
            if p.sections.len() >= MAX_SECTIONS {
                break;
            }
            p.sections.push(Section { title: canonical(h), text: String::new(), pics: vec![] });
            continue;
        }
        let up = t.to_ascii_uppercase();
        if up.starts_with("[PICTURE") {
            let inner = t.trim_start_matches('[').trim_end_matches(']');
            let inner = inner.split_once(':').map_or("", |x| x.1);
            let (cap, brief) = inner.split_once('|').unwrap_or((inner, inner));
            let (cap, brief) = (cut(clean_line(cap).trim(), MAX_CAPTION), cut(clean_line(brief).trim(), MAX_BRIEF));
            if p.pictures.len() < MAX_PICTURES && !cap.is_empty() {
                if p.sections.is_empty() {
                    p.sections.push(Section { title: HEADINGS[0].into(), ..Default::default() });
                }
                let idx = p.pictures.len();
                p.pictures.push(Picture { caption: cap, brief: if brief.is_empty() { cut(&clean_line(inner), MAX_BRIEF) } else { brief }, ..Default::default() });
                p.sections.last_mut().unwrap().pics.push(idx);
            }
            continue;
        }
        let mut c = clean_line(line);
        // markdown bullets and heading marks become plain ones
        let trimmed = c.trim_start().to_string();
        if let Some(rest) = trimmed.strip_prefix("* ").or_else(|| trimmed.strip_prefix("• ")) {
            c = format!("- {rest}");
        } else if trimmed.starts_with('#') {
            c = trimmed.trim_start_matches('#').trim().to_string();
        } else {
            c = trimmed;
        }
        if p.sections.is_empty() {
            if c.trim().is_empty() {
                continue;
            }
            p.sections.push(Section { title: HEADINGS[0].into(), ..Default::default() });
        }
        let s = p.sections.last_mut().unwrap();
        if s.text.chars().count() >= MAX_SECTION_CHARS || total >= MAX_LESSON_CHARS {
            continue;
        }
        if c.trim().is_empty() && (s.text.is_empty() || s.text.ends_with("\n\n")) {
            continue;
        }
        if !s.text.is_empty() {
            s.text.push('\n');
        }
        s.text.push_str(c.trim_end());
        total += c.chars().count() + 1;
    }
    for s in &mut p.sections {
        s.text = cut(s.text.trim(), MAX_SECTION_CHARS);
    }
    // a section with no words and no picture is a heading still waiting for its text
    if done {
        p.sections.retain(|s| !s.text.is_empty() || !s.pics.is_empty());
    }
    p
}

/// A model's answer to a question, cleaned and capped.
pub fn clean_answer(raw: &str) -> String {
    let text = strip_think(raw);
    let lines: Vec<String> = text.lines().map(clean_line).collect();
    cut(lines.join("\n").trim(), MAX_ANSWER)
}

// ------------------------------------------------------------------------------------------------------------ pictures

/// A plain picture drawn by the code, so a lesson is never without one: soft colours chosen from the words of the brief.
pub fn placeholder_svg(brief: &str) -> String {
    let h = brief.bytes().fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let hue = |k: u32| -> String {
        let v = h.rotate_left(k * 7);
        let (r, g, b) = (90 + (v & 0x7f), 110 + ((v >> 8) & 0x7f), 130 + ((v >> 16) & 0x6f));
        format!("#{:02x}{:02x}{:02x}", r.min(255), g.min(255), b.min(255))
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 400 240\"><defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{a}\"/><stop offset=\"1\" stop-color=\"{b}\"/></linearGradient></defs>\
<rect width=\"400\" height=\"240\" fill=\"url(#g)\"/><circle cx=\"{x1}\" cy=\"{y1}\" r=\"46\" fill=\"{c}\" opacity=\"0.8\"/><rect x=\"{x2}\" y=\"110\" width=\"120\" height=\"80\" rx=\"12\" fill=\"{d}\" opacity=\"0.85\"/><path d=\"M20 220 L120 150 L200 200 L300 120 L380 220 Z\" fill=\"{e}\" opacity=\"0.7\"/></svg>",
        a = hue(1),
        b = hue(2),
        c = hue(3),
        d = hue(4),
        e = hue(5),
        x1 = 70 + (h % 220),
        y1 = 60 + ((h >> 5) % 40),
        x2 = 40 + ((h >> 9) % 200)
    )
}

pub fn file_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 24 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') && !name.starts_with('.')
}

