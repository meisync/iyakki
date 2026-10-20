//! Changing the project context by telling the assistant: "remember that we use pnpm", "forget the pnpm note", "update the context about the DB:
//! it is Postgres 16 now", "pin that", "what is in the context?".
//!
//! The harness reads the clear imperative forms itself, *before* any model call, so it works with a weak local model, in the IDE chat, in
//! Autopilot and with an external agent alike (they all start from the person's message). What it did is said plainly and can be undone.
//! Commentary is left alone: "I remember when ...", "note that this is slow", "remember when we used cvs?", "never mind".
//!
//! * An entry written this way is the person's (not "new by agent", an instruction to the agents).
//! * A similar title updates the entry that exists instead of adding a second.
//! * Forget, update and pin find their target by the words of the title and text; when two entries fit about equally the person is asked
//!   to choose (numbered options, which the chat shows as buttons).
//! * A credential is never stored: the message is left for the chat's own credential handling.

use std::path::Path;

use crate::notes::{self, Kind, NewNote, Note, Notes, Status, Who};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Add { text: String },
    Forget { query: String },
    Update { query: String, text: String },
    Pin { query: Option<String>, pin: bool },
    List,
}

/// An intent and what is left of the message after it (a task for the model, or nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub intent: Intent,
    pub rest: String,
}

/// How to take a change back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    /// An entry that was added.
    Remove(String),
    /// An entry as it was before it was changed, archived or pinned.
    Restore(Box<Note>),
}

#[derive(Clone, Debug, Default)]
pub struct Done {
    /// What to say in the chat, exactly what happened.
    pub reply: String,
    pub undo: Option<Undo>,
    /// The entry the change was about (for "pin that" next).
    pub touched: Option<String>,
    /// Several entries fit: the ids offered, in the order of the numbered options in `reply`.
    pub choices: Vec<String>,
    /// Nothing was done and the message should go on to the model as it is (a credential, for the chat to deal with).
    pub refused: bool,
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

const QUESTION_WORDS: [&str; 12] = ["when", "how", "if", "why", "what", "where", "who", "whether", "which", "me", "you", "us"];

fn starts_with_word<'a>(lower: &'a str, w: &str) -> Option<&'a str> {
    let rest = lower.strip_prefix(w)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if !c.is_alphanumeric() && c != '\'' => Some(rest),
        _ => None,
    }
}

/// The rule of an "add" message and the rest: the rule ends at the end of the line or after a full stop that is followed by a new sentence.
fn split_rule(text: &str) -> (String, String) {
    let t = text.trim();
    let first_line_end = t.find('\n').unwrap_or(t.len());
    let line = &t[..first_line_end];
    let mut cut = line.len();
    let b = line.as_bytes();
    for i in 0..b.len().saturating_sub(2) {
        if b[i] == b'.' && b[i + 1] == b' ' && line[i + 2..].chars().next().is_some_and(char::is_uppercase) {
            // "e.g. Foo" and "v1. Next" are not sentence ends: a lower-case letter or digit before the dot, then a capital after, is
            let before = line[..i].chars().last().unwrap_or(' ');
            if before.is_alphanumeric() || before == ')' || before == '`' || before == '"' {
                cut = i + 1;
                break;
            }
        }
    }
    let rule = line[..cut].trim().trim_end_matches('.').trim().to_string();
    let rest = format!("{} {}", line[cut..].trim(), t[first_line_end..].trim()).trim().to_string();
    (rule, rest)
}

const RULE_VERBS: [&str; 24] = ["use", "write", "run", "commit", "edit", "touch", "delete", "keep", "add", "put", "call", "name", "format", "test", "check", "ask", "include", "exclude", "import", "rename", "modify", "push", "deploy", "install"];

/// Does the text of a message ask to change the context? None for anything else.
pub fn detect(text: &str) -> Option<Detected> {
    let t = text.trim();
    if t.is_empty() || t.len() > 4000 {
        return None;
    }
    let mut lower = t.to_lowercase().replace('\u{2019}', "'");
    let mut body = t.to_string();
    // a polite or connecting start
    for p in ["please ", "pls ", "hey ", "ok ", "okay ", "and ", "also ", "ok, ", "okay, ", "so "] {
        if lower.starts_with(p) {
            lower = lower[p.len()..].to_string();
            body = body[p.len()..].to_string();
        }
    }
    let skip = |n: usize| body.char_indices().nth(n).map_or(String::new(), |(i, _)| body[i..].to_string());
    // --- list
    let l = lower.trim_end_matches(['?', '.', '!']).trim();
    for p in ["what is in the context", "what's in the context", "what is in the project context", "what's in the project context", "what is in the notes", "what's in the notes", "what is in context", "what do you remember about this project", "show the context", "show me the context", "show the project context", "show me the project context", "list the context", "list context", "list the project context", "show the notes", "list the notes", "show context", "what is the project context", "what's the project context"] {
        if l == p {
            return Some(Detected { intent: Intent::List, rest: String::new() });
        }
    }
    // --- forget / remove / archive
    for (w, needs_from) in [("forget", false), ("remove", true), ("delete", true), ("archive", false), ("drop", true)] {
        if let Some(rest) = starts_with_word(&lower, w) {
            let off = lower.len() - rest.len();
            let r = body[off..].trim().trim_end_matches(['.', '!']).to_string();
            let rl = r.to_lowercase();
            if r.is_empty() || r.contains('?') || matches!(rl.as_str(), "it" | "about it" | "everything" | "all") {
                return None;
            }
            // "remove the pnpm note from the context", "delete the X entry": the topic is what is named
            let mut q = rl.clone();
            for suffix in [" from the project context", " from the context", " from context", " from the notes", " from notes", " in the context", " from the project notes"] {
                if let Some(x) = q.strip_suffix(suffix) {
                    q = x.to_string();
                }
            }
            let mentions = q != rl || rl.contains(" note") || rl.contains(" entry") || rl.contains("context") || rl.contains(" rule");
            if needs_from && !mentions {
                return None;
            }
            if w == "archive" && !mentions {
                return None;
            }
            let q = clean_query(&q);
            if q.is_empty() && !matches!(rl.as_str(), "that" | "this" | "that one" | "this one") {
                return None;
            }
            // only the first sentence is the request
            let (first, rest_msg) = split_rule(&q);
            let _ = first;
            return Some(Detected { intent: Intent::Forget { query: clean_query(&q) }, rest: rest_msg });
        }
    }
    // --- update / change
    for w in ["update", "change", "correct"] {
        if let Some(rest) = starts_with_word(&lower, w) {
            let off = lower.len() - rest.len();
            let r = body[off..].trim().to_string();
            let rl = r.to_lowercase();
            let rl2 = rl.trim_start_matches("the ").trim_start_matches("our ");
            let kind_word = ["project context", "context", "notes", "note", "entry"].iter().find(|k| rl2.starts_with(*k) || rl2.contains(&format!(" {k}")));
            if kind_word.is_none() {
                return None;
            }
            // "<the> context about the DB: it is Postgres 16 now" / "the DB note: ..." / "the DB note to say ..."
            let (topic_part, new_text) = if let Some(i) = r.find(':') {
                (r[..i].to_string(), r[i + 1..].trim().to_string())
            } else if let Some(i) = rl.find(" to say ").or_else(|| rl.find(" to read ")).or_else(|| rl.find(" to be ")) {
                let skip_len = if rl[i..].starts_with(" to be ") { 7 } else { 8 };
                (r[..i].to_string(), r[(i + skip_len).min(r.len())..].trim().to_string())
            } else if let Some(i) = r.find(" - ") {
                (r[..i].to_string(), r[i + 3..].trim().to_string())
            } else {
                return None;
            };
            let mut topic = topic_part.to_lowercase();
            for junk in ["project context", "the context", "context", "the notes", "notes", "note", "entry", "about", "regarding", "on", "for", "the", "our", "that says", "that"] {
                topic = topic.split_whitespace().filter(|x| *x != junk).collect::<Vec<_>>().join(" ");
            }
            let (new_text, rest_msg) = split_rule(&new_text);
            if topic.trim().is_empty() || new_text.is_empty() {
                return None;
            }
            return Some(Detected { intent: Intent::Update { query: topic.trim().to_string(), text: new_text }, rest: rest_msg });
        }
    }
    // --- pin / unpin
    for (w, pin) in [("unpin", false), ("pin", true)] {
        if let Some(rest) = starts_with_word(&lower, w) {
            let off = lower.len() - rest.len();
            let r = body[off..].trim().trim_end_matches(['.', '!']).trim().to_string();
            let rl = r.to_lowercase();
            if r.contains('?') || r.len() > 80 {
                return None;
            }
            if matches!(rl.as_str(), "that" | "this" | "it" | "that one" | "this one" | "that note" | "this note" | "that entry" | "that rule") {
                return Some(Detected { intent: Intent::Pin { query: None, pin }, rest: String::new() });
            }
            // "pin the pnpm note"; a bare "pin x" outside the context is not ours: it must name a note, entry or the context
            let named = rl.contains(" note") || rl.contains(" entry") || rl.contains("context") || rl.contains(" rule");
            if named {
                let q = clean_query(&rl);
                if !q.is_empty() {
                    return Some(Detected { intent: Intent::Pin { query: Some(q), pin }, rest: String::new() });
                }
            }
            return None;
        }
    }
    // --- add
    // "remember ...", "don't forget ...", "note: ...", "add to the context: ...", "context: ...", "from now on ..."
    let starters: [(&str, bool); 8] = [("remember", false), ("don't forget", false), ("do not forget", false), ("note", true), ("context", true), ("from now on", false), ("keep in mind", false), ("never forget", false)];
    for (w, colon_only) in starters {
        if let Some(rest) = starts_with_word(&lower, w) {
            let off = lower.len() - rest.len();
            let after = body[off..].trim_start();
            let al = after.to_lowercase();
            let has_colon = after.starts_with(':') || after.starts_with('-') && after[1..].starts_with(' ');
            if colon_only && !has_colon {
                return None;
            }
            let after = after.trim_start_matches([':', '-', ',']).trim_start();
            let al2 = after.to_lowercase();
            // "remember when ...", "remember how ...", "remember me?": recollection or a question, not an instruction
            if !colon_only && QUESTION_WORDS.iter().any(|q| starts_with_word(&al2, q).is_some()) {
                return None;
            }
            let _ = al;
            let after = strip_lead(after, &["that ", "to ", "this: ", "this ", "also "]);
            if after.is_empty() {
                return None;
            }
            let (rule, rest) = split_rule(after);
            if rule.is_empty() || rule.contains('?') {
                return None;
            }
            let rule = if w == "from now on" { format!("From now on {}", rule.trim_start_matches([',', ' '])) } else { rule };
            return Some(Detected { intent: Intent::Add { text: norm(&rule) }, rest });
        }
    }
    for p in ["add to the project context", "add to project context", "add to the context", "add to context", "add this to the project context", "add this to the context", "add to the project notes", "add to the notes", "add it to the context", "put this in the context", "put in the context", "save to the context", "save this to the context", "add a context entry", "add a note to the context"] {
        if let Some(rest) = starts_with_word(&lower, p) {
            let off = lower.len() - rest.len();
            let after = body[off..].trim().trim_start_matches([':', '-', ',']).trim();
            if after.is_empty() {
                return None;
            }
            let (rule, rest_msg) = split_rule(after);
            if rule.is_empty() {
                return None;
            }
            return Some(Detected { intent: Intent::Add { text: norm(&rule) }, rest: rest_msg });
        }
    }
    // a bare rule: the whole message is "never X" / "always X" (one short sentence with an action in it)
    let _ = skip;
    for w in ["never", "always"] {
        if let Some(rest) = starts_with_word(&lower, w) {
            if rest.trim_start().starts_with("mind") || rest.trim_start().starts_with("again") || t.contains('?') || t.contains('\n') || t.len() > 160 {
                return None;
            }
            let has_verb = rest.split(|c: char| !c.is_alphanumeric()).any(|x| RULE_VERBS.contains(&x));
            if has_verb {
                return Some(Detected { intent: Intent::Add { text: norm(t.trim_end_matches('.')) }, rest: String::new() });
            }
        }
    }
    None
}

fn strip_lead<'a>(s: &'a str, leads: &[&str]) -> &'a str {
    let l = s.to_lowercase();
    for p in leads {
        if l.starts_with(p) {
            return s[p.len()..].trim_start();
        }
    }
    s
}

/// The words that name an entry: the query without the filler words around it.
fn clean_query(q: &str) -> String {
    const JUNK: [&str; 18] = ["the", "that", "our", "about", "note", "notes", "entry", "entries", "context", "project", "rule", "please", "from", "in", "to", "a", "an", "this"];
    q.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.' && c != '/' && c != '_').filter(|w| !w.is_empty() && !JUNK.contains(w)).collect::<Vec<_>>().join(" ")
}

/// The kind a rule most likely is.
pub fn infer_kind(text: &str) -> Kind {
    let l = format!(" {} ", text.to_lowercase());
    let has = |ws: &[&str]| ws.iter().any(|w| l.contains(w));
    if has(&[" never ", " always ", " must ", " must not ", " do not ", " don't ", " should not ", " shouldn't ", " only ", " not allowed ", " forbidden "]) {
        Kind::Constraint
    } else if has(&[" careful ", " watch out ", " gotcha ", " beware ", " breaks ", " fails ", " slow ", " trap ", " bug ", " flaky ", " crashes "]) {
        Kind::Gotcha
    } else if has(&["http://", "https://", " docs ", " see ", " lives at ", " is at ", " base is ", " base url ", " endpoint ", " is in ", " lives in "]) || l.contains(" /") {
        Kind::Reference
    } else if has(&[" we use ", " we're using ", " we are using ", " decided ", " chose ", " switched to ", " going with ", " migrated to "]) {
        Kind::Decision
    } else if has(&[" format ", " style ", " naming ", " name ", " prefix ", " indent ", " commit ", " lint ", " convention ", " prefer "]) {
        Kind::Convention
    } else {
        Kind::Decision
    }
}

/// A title from the first words of a rule (at most 8 words and 80 characters); the text is kept in full when the title does not say it all.
fn title_and_text(rule: &str) -> (String, String) {
    let words: Vec<&str> = rule.split_whitespace().collect();
    let mut title = words.iter().take(8).cloned().collect::<Vec<_>>().join(" ");
    if title.chars().count() > notes::MAX_TITLE {
        title = title.chars().take(notes::MAX_TITLE - 1).collect::<String>() + "…";
    }
    let mut c = title.chars();
    let title = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => title,
    };
    let text = if words.len() > 8 || rule.chars().count() > notes::MAX_TITLE { rule.to_string() } else { String::new() };
    (title, text)
}

/// The share of words two titles have in common (0..=1).
fn overlap(a: &str, b: &str) -> f32 {
    let (x, y) = (words_of(a), words_of(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let common = x.iter().filter(|w| y.contains(w)).count();
    common as f32 / (x.len() + y.len() - common) as f32
}

fn words_of(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.').filter(|w| w.len() > 1).map(str::to_string).collect();
    v.sort();
    v.dedup();
    v
}

/// How well an entry fits a query (0..=1): the share of the query's words found in its title (counted double) and text.
fn fit(n: &Note, query: &str) -> f32 {
    let q = words_of(query);
    if q.is_empty() {
        return 0.0;
    }
    let (t, x) = (words_of(&n.title), words_of(&n.text));
    let score: f32 = q.iter().map(|w| if t.iter().any(|y| y == w || (w.len() > 3 && y.starts_with(w.as_str()))) { 1.0 } else if x.iter().any(|y| y == w || (w.len() > 3 && y.starts_with(w.as_str()))) { 0.5 } else { 0.0 }).sum();
    score / q.len() as f32
}

/// The entries that fit a query, best first: (id, score).
fn candidates(n: &Notes, query: &str) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = n.active().map(|x| (x.id.clone(), fit(x, query))).filter(|c| c.1 >= 0.5).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

enum Target {
    One(String),
    Ask(Vec<String>),
    None,
}

fn pick(n: &Notes, query: &str) -> Target {
    let c = candidates(n, query);
    match c.as_slice() {
        [] => Target::None,
        [one] => Target::One(one.0.clone()),
        [a, b, ..] if a.1 >= b.1 + 0.34 => Target::One(a.0.clone()),
        many => Target::Ask(many.iter().take(5).map(|c| c.0.clone()).collect()),
    }
}

fn show(n: &Note) -> String {
    format!("{} {}", n.id, n.title)
}

fn ask_reply(n: &Notes, verb: &str, ids: &[String]) -> String {
    let mut s = format!("Several entries of the project context fit. Which one should I {verb}?\n");
    for (i, id) in ids.iter().enumerate() {
        if let Some(x) = n.get(id) {
            s.push_str(&format!("\n{}. {}", i + 1, show(x)));
        }
    }
    s
}

/// Do what the person asked. `last` is the entry touched last in this chat ("pin that"). Never panics, never half-writes.
pub fn apply(root: &Path, person: &str, d: &Detected, last: Option<&str>, now: i64) -> Done {
    let who = Who::Person(person.to_string());
    let (n, problem) = notes::load(root);
    if let Some(p) = problem {
        return Done { reply: format!("I could not read the project context: {p}"), ..Default::default() };
    }
    match &d.intent {
        Intent::List => {
            let list: Vec<&Note> = n.view(&notes::Filter::default());
            if list.is_empty() {
                return Done { reply: "The project context is empty. Say \"remember that ...\" to add to it.".into(), ..Default::default() };
            }
            let (a, p, _) = n.counts();
            let mut s = format!("The project context has {a} active entr{} ({p} pinned):", if a == 1 { "y" } else { "ies" });
            for x in list.iter().take(25) {
                s.push_str(&format!("\n- {}{} [{}] {}", x.id, if x.pinned { " (pinned)" } else { "" }, x.kind.label(), x.title));
            }
            if list.len() > 25 {
                s.push_str(&format!("\n... and {} more (Context tab)", list.len() - 25));
            }
            Done { reply: s, ..Default::default() }
        }
        Intent::Add { text } => {
            if let Err(e) = notes::refuse_secret(text) {
                return Done { reply: format!("Not added to the project context: {e}"), refused: true, ..Default::default() };
            }
            let kind = infer_kind(text);
            let (title, body) = title_and_text(text);
            // a similar title: that entry is updated, not copied
            if let Some(old) = n.active().find(|x| notes::similar(&x.title, &title) || overlap(&x.title, &title) >= 0.5).cloned() {
                let r = notes::edit(root, |n| n.update(&old.id, None, Some(if body.is_empty() { text } else { &body }), Some(kind), &who, now));
                return match r {
                    Ok(()) => Done { reply: format!("Updated in the project context: {} now says \"{}\" ({}).", show(&old), text, kind.label()), undo: Some(Undo::Restore(Box::new(old.clone()))), touched: Some(old.id), ..Default::default() },
                    Err(e) => Done { reply: format!("Not saved: {e}"), refused: e.contains("credential"), ..Default::default() },
                };
            }
            let r = notes::edit(root, |n| n.add(NewNote { title: title.clone(), text: body.clone(), kind, pinned: false, source: "chat".into() }, &who, now));
            match r {
                Ok(a) => Done { reply: format!("Added to the project context: {} {} ({}). The whole team and every agent of this project get it.", a.id, text, kind.label()), undo: Some(Undo::Remove(a.id.clone())), touched: Some(a.id), ..Default::default() },
                Err(e) => Done { reply: format!("Not added to the project context: {e}"), refused: e.contains("credential"), ..Default::default() },
            }
        }
        Intent::Forget { query } => {
            let target = if matches!(query.as_str(), "" | "that" | "this" | "that one" | "this one") { last.map(|l| Target::One(l.to_string())).unwrap_or(Target::None) } else { pick(&n, query) };
            resolve(root, &n, &who, now, target, query, |n, id| n.archive(id, &who, now), "forgot (archived)", "forget")
        }
        Intent::Pin { query, pin } => {
            let target = match query {
                None => last.map(|l| Target::One(l.to_string())).unwrap_or(Target::None),
                Some(q) => pick(&n, q),
            };
            let pin = *pin;
            resolve(root, &n, &who, now, target, query.as_deref().unwrap_or("that"), move |n, id| n.set_pinned(id, pin, now), if pin { "pinned" } else { "unpinned" }, if pin { "pin" } else { "unpin" })
        }
        Intent::Update { query, text } => {
            if let Err(e) = notes::refuse_secret(text) {
                return Done { reply: format!("Not changed in the project context: {e}"), refused: true, ..Default::default() };
            }
            let text = text.clone();
            resolve(root, &n, &who, now, pick(&n, query), query, move |n, id| n.update(id, None, Some(&text), None, &Who::Person("person".into()), now), "updated", "update")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve(root: &Path, n: &Notes, who: &Who, now: i64, target: Target, query: &str, op: impl FnOnce(&mut Notes, &str) -> Result<(), String>, verb: &str, ask_verb: &str) -> Done {
    let _ = (who, now);
    match target {
        Target::None => Done { reply: format!("I found nothing in the project context that matches \"{query}\". Say \"what is in the context?\" to see it."), ..Default::default() },
        Target::Ask(ids) => Done { reply: ask_reply(n, ask_verb, &ids), choices: ids, ..Default::default() },
        Target::One(id) => apply_to(root, n, &id, op, verb),
    }
}

fn apply_to(root: &Path, n: &Notes, id: &str, op: impl FnOnce(&mut Notes, &str) -> Result<(), String>, verb: &str) -> Done {
    let Some(old) = n.get(id).cloned() else { return Done { reply: format!("There is no context entry {id}."), ..Default::default() } };
    match notes::edit(root, |x| op(x, id)) {
        Ok(()) => {
            let after = notes::load(root).0.get(id).cloned();
            let said = match (&after, verb) {
                (Some(a), "updated") => format!("Updated in the project context: {} now says \"{}\".", show(a), if a.text.is_empty() { &a.title } else { &a.text }),
                _ => format!("Project context: {verb} {}.", show(&old)),
            };
            Done { reply: said, undo: Some(Undo::Restore(Box::new(old.clone()))), touched: Some(old.id), ..Default::default() }
        }
        Err(e) => Done { reply: format!("Not changed: {e}"), ..Default::default() },
    }
}

/// The person chose one of the numbered options of an ambiguous forget, update or pin (`pick` is 1-based).
pub fn apply_choice(root: &Path, person: &str, d: &Detected, ids: &[String], pick: usize, now: i64) -> Done {
    let Some(id) = ids.get(pick.wrapping_sub(1)) else { return Done { reply: "Cancelled: nothing was changed.".into(), ..Default::default() } };
    let (n, _) = notes::load(root);
    let who = Who::Person(person.to_string());
    match &d.intent {
        Intent::Forget { .. } => apply_to(root, &n, id, |x, i| x.archive(i, &who, now), "forgot (archived)"),
        Intent::Pin { pin, .. } => {
            let pin = *pin;
            apply_to(root, &n, id, move |x, i| x.set_pinned(i, pin, now), if pin { "pinned" } else { "unpinned" })
        }
        Intent::Update { text, .. } => {
            let text = text.clone();
            apply_to(root, &n, id, move |x, i| x.update(i, None, Some(&text), None, &Who::Person(person_of(&who)), now), "updated")
        }
        _ => Done::default(),
    }
}

fn person_of(w: &Who) -> String {
    w.name().to_string()
}

/// Take a change back.
pub fn undo(root: &Path, u: &Undo) -> Result<String, String> {
    match u {
        Undo::Remove(id) => notes::edit(root, |n| n.delete(id).map(|x| format!("Undone: {} was removed from the project context.", show(&x)))),
        Undo::Restore(old) => notes::edit(root, |n| {
            match n.notes.iter_mut().find(|x| x.id == old.id) {
                Some(x) => *x = (**old).clone(),
                None => n.notes.push((**old).clone()),
            }
            Ok(format!("Undone: {} is as it was ({}).", show(old), if old.status == Status::Active { "active" } else { old.status.key() }))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(text: &str) -> Intent {
        Intent::Add { text: text.into() }
    }
    fn d(text: &str) -> Option<Intent> {
        detect(text).map(|x| x.intent)
    }
    fn dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("iyakki-ctxintent-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_clear_imperative_forms_are_understood_in_their_variety() {
        assert_eq!(d("remember that we use pnpm"), Some(add("we use pnpm")));
        assert_eq!(d("Remember we use pnpm."), Some(add("we use pnpm")));
        assert_eq!(d("please remember to run make test, not cargo test"), Some(add("run make test, not cargo test")));
        assert_eq!(d("Don't forget that the API base is /v1"), Some(add("the API base is /v1")));
        assert_eq!(d("do not forget: never edit generated/ files"), Some(add("never edit generated/ files")));
        assert_eq!(d("note: never edit generated/ files"), Some(add("never edit generated/ files")));
        assert_eq!(d("Note - tests need docker"), Some(add("tests need docker")));
        assert_eq!(d("add to context: API base is /v1"), Some(add("API base is /v1")));
        assert_eq!(d("add to the project context: we deploy on fridays only"), Some(add("we deploy on fridays only")));
        assert_eq!(d("Add this to the context - staging is at https://stg.example.com"), Some(add("staging is at https://stg.example.com")));
        assert_eq!(d("context: we use sqlite"), Some(add("we use sqlite")));
        assert_eq!(d("from now on, always write tests first"), Some(add("From now on always write tests first")));
        assert_eq!(d("keep in mind that the CI is slow"), Some(add("the CI is slow")));
        assert_eq!(d("never commit to main"), Some(add("never commit to main")));
        assert_eq!(d("Always use tabs in Makefiles."), Some(add("Always use tabs in Makefiles")));
        // a rule followed by a task: the rule is stored, the task goes on to the model
        let x = detect("Remember that we use pnpm. Now add a login page to the app.").unwrap();
        assert_eq!((x.intent, x.rest.as_str()), (add("we use pnpm"), "Now add a login page to the app."));
        let x = detect("remember that the DB is Postgres 16\nand then write the migration").unwrap();
        assert_eq!((x.intent, x.rest.as_str()), (add("the DB is Postgres 16"), "and then write the migration"));
        // forget, update, pin, list
        assert_eq!(d("forget the pnpm note"), Some(Intent::Forget { query: "pnpm".into() }));
        assert_eq!(d("Forget about the docker thing"), Some(Intent::Forget { query: "docker thing".into() }));
        assert_eq!(d("remove the pnpm note from the context"), Some(Intent::Forget { query: "pnpm".into() }));
        assert_eq!(d("archive the sqlite entry"), Some(Intent::Forget { query: "sqlite".into() }));
        assert_eq!(d("forget that"), Some(Intent::Forget { query: String::new() }), "no words: the entry touched last");
        assert_eq!(d("update the context about the DB: it is Postgres 16 now"), Some(Intent::Update { query: "db".into(), text: "it is Postgres 16 now".into() }));
        assert_eq!(d("change the pnpm note to say we use yarn now"), Some(Intent::Update { query: "pnpm".into(), text: "we use yarn now".into() }));
        assert_eq!(d("pin that"), Some(Intent::Pin { query: None, pin: true }));
        assert_eq!(d("unpin that note"), Some(Intent::Pin { query: None, pin: false }));
        assert_eq!(d("pin the pnpm note"), Some(Intent::Pin { query: Some("pnpm".into()), pin: true }));
        for q in ["what is in the context?", "What's in the project context", "show me the context", "list the notes"] {
            assert_eq!(d(q), Some(Intent::List), "{q}");
        }
    }

    #[test]
    fn commentary_questions_and_other_uses_of_the_words_do_not_trigger() {
        for t in [
            "I remember when we used cvs",
            "remember when we used cvs?",
            "Do you remember how this works?",
            "remember me?",
            "note that this is slow",
            "I'll note that for later",
            "this note is wrong",
            "forget it",
            "forget about it",
            "never mind",
            "never again will I trust that script",
            "Always happy to help",
            "always on time?",
            "what does the note say about pnpm?",
            "pin the version of left-pad in package.json",
            "pin this commit",
            "update the readme with the new flags",
            "change the colour of the button",
            "remove the unused import from main.rs",
            "delete the old branch",
            "list the files in src",
            "show me the diff",
            "context menus are broken",
            "can you remember that we use pnpm?",
            "remember?",
            "",
        ] {
            assert_eq!(d(t), None, "{t:?} is not an instruction to change the context");
        }
    }

    #[test]
    fn the_kind_is_inferred_and_the_title_comes_from_the_first_words() {
        assert_eq!(infer_kind("never edit generated/ files"), Kind::Constraint);
        assert_eq!(infer_kind("we must always run the linter"), Kind::Constraint);
        assert_eq!(infer_kind("we use pnpm"), Kind::Decision);
        assert_eq!(infer_kind("API base is /v1"), Kind::Reference);
        assert_eq!(infer_kind("staging is at https://stg.example.com"), Kind::Reference);
        assert_eq!(infer_kind("the CI is slow, be careful"), Kind::Gotcha);
        assert_eq!(infer_kind("commit messages start with the area"), Kind::Convention);
        let (t, x) = title_and_text("we use pnpm");
        assert_eq!((t.as_str(), x.as_str()), ("We use pnpm", ""));
        let (t, x) = title_and_text("tests need docker running and the registry on port 5000 before they pass");
        assert_eq!((t.as_str(), x.is_empty()), ("Tests need docker running and the registry on", false));
    }

    #[test]
    fn remember_adds_an_entry_by_the_person_updates_a_similar_one_and_can_be_undone() {
        let r = dir("add");
        let x = detect("remember that we use pnpm. Now fix the build").unwrap();
        let done = apply(&r, "Casey", &x, None, 100);
        assert!(done.reply.starts_with("Added to the project context: N-001 we use pnpm (Decision)"), "{}", done.reply);
        let (n, _) = notes::load(&r);
        let e = n.get("N-001").unwrap();
        assert_eq!((e.author.as_str(), e.by_agent, e.reviewed, e.is_instruction(), e.is_new_by_agent(), e.source.as_str()), ("Casey", false, true, true, false, "chat"), "the person's, trusted, not 'new by agent'");
        assert_eq!(x.rest, "Now fix the build");
        // the same thing again, worded a little differently: the entry is updated, not copied
        let again = apply(&r, "Casey", &detect("remember: we use pnpm for everything").unwrap(), None, 200);
        assert!(again.reply.starts_with("Updated in the project context: N-001"), "{}", again.reply);
        assert_eq!(notes::load(&r).0.notes.len(), 1);
        // undo of the update brings the old text back; undo of the add removes it
        let u = undo(&r, again.undo.as_ref().unwrap()).unwrap();
        assert!(u.contains("as it was"));
        assert_eq!(notes::load(&r).0.get("N-001").unwrap().text, "");
        let u = undo(&r, done.undo.as_ref().unwrap()).unwrap();
        assert!(u.contains("was removed"));
        assert!(notes::load(&r).0.notes.is_empty());
        // a constraint
        let c = apply(&r, "Casey", &detect("note: never edit generated/ files").unwrap(), None, 300);
        assert!(c.reply.contains("(Constraint)"), "{}", c.reply);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_secret_is_refused_with_a_clear_message_and_the_message_goes_on_untouched() {
        let r = dir("secret");
        let x = detect("remember that the token is ghp_abcdefghijklmnopqrstuvwxyz0123456789AB").unwrap();
        let done = apply(&r, "Casey", &x, None, 1);
        assert!(done.refused && done.reply.contains("credential") && done.reply.contains("vault") && done.undo.is_none(), "{}", done.reply);
        assert!(!notes::json_path(&r).exists(), "nothing written");
        let x = detect("remember: password: hunter2xyz99").unwrap();
        assert!(apply(&r, "Casey", &x, None, 1).refused);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn forget_update_and_pin_find_their_entry_ask_when_two_fit_and_can_be_undone() {
        let r = dir("find");
        for t in ["remember that we use pnpm", "remember that the DB is Postgres 15", "remember: never edit generated/ files", "remember: pnpm workspaces live in packages/"] {
            apply(&r, "Casey", &detect(t).unwrap(), None, 10);
        }
        // pin that: the entry touched last
        let p = apply(&r, "Casey", &detect("pin that").unwrap(), Some("N-004"), 20);
        assert!(p.reply.contains("pinned N-004") && notes::load(&r).0.get("N-004").unwrap().pinned, "{}", p.reply);
        undo(&r, p.undo.as_ref().unwrap()).unwrap();
        assert!(!notes::load(&r).0.get("N-004").unwrap().pinned);
        assert!(apply(&r, "Casey", &detect("pin that").unwrap(), None, 20).reply.contains("found nothing"));
        // update the DB entry: one clear match
        let u = apply(&r, "Casey", &detect("update the context about the DB: it is Postgres 16 now").unwrap(), None, 30);
        assert!(u.reply.contains("Updated in the project context: N-002") && u.reply.contains("Postgres 16 now"), "{}", u.reply);
        assert_eq!(notes::load(&r).0.get("N-002").unwrap().text, "it is Postgres 16 now");
        // "pnpm" fits two entries about equally: the person chooses
        let f = detect("forget the pnpm note").unwrap();
        let a = apply(&r, "Casey", &f, None, 40);
        assert_eq!(a.choices, ["N-001", "N-004"], "{}", a.reply);
        assert!(a.reply.contains("Which one should I forget?") && a.reply.contains("\n1. N-001 We use pnpm") && a.reply.contains("\n2. N-004"), "{}", a.reply);
        assert_eq!(notes::load(&r).0.active().count(), 4, "nothing was changed yet");
        let done = apply_choice(&r, "Casey", &f, &a.choices, 2, 41);
        assert!(done.reply.contains("forgot (archived) N-004"), "{}", done.reply);
        assert_eq!(notes::load(&r).0.get("N-004").unwrap().status, Status::Archived);
        undo(&r, done.undo.as_ref().unwrap()).unwrap();
        assert_eq!(notes::load(&r).0.get("N-004").unwrap().status, Status::Active);
        assert!(apply_choice(&r, "Casey", &f, &a.choices, 9, 42).reply.contains("Cancelled"));
        // a clear target is acted on at once
        let g = apply(&r, "Casey", &detect("forget the generated files note").unwrap(), None, 50);
        assert!(g.reply.contains("N-003") && notes::load(&r).0.get("N-003").unwrap().status == Status::Archived, "{}", g.reply);
        // nothing fits
        assert!(apply(&r, "Casey", &detect("forget the kubernetes note").unwrap(), None, 60).reply.contains("found nothing"));
        // listing
        let l = apply(&r, "Casey", &detect("what is in the context?").unwrap(), None, 70);
        assert!(l.reply.contains("3 active entries") && l.reply.contains("N-001") && !l.reply.contains("N-003"), "{}", l.reply);
        assert!(apply(&dir("empty"), "Casey", &detect("show the context").unwrap(), None, 1).reply.contains("empty"));
        let _ = std::fs::remove_dir_all(&r);
    }
}
