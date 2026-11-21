//! The assistant's proposals in the Web Edit tile (F-WEB-59), the same way the EDIT tile shows them: the person asks for a change (Ctrl+K
//! or the Ask button, a line of words), the service starts a run with the open text as DATA (`assist.ask`), the tile asks for the answer
//! every two seconds (`assist.result`), and a reply that holds an edit (SEARCH/REPLACE blocks or a whole file) is shown as hunks to accept
//! or reject one by one (y, n, a, Enter to apply, Esc to throw away). Nothing is written to the buffer until Enter, in one undo step; a
//! file on disk changes only through the Save button, which asks as before.

use super::*;
use iyakki_edit::diff::{apply_hunks, diff_hunks, summary, Hunk};
use iyakki_edit::proposal::parse_reply;

/// How often the answer is asked for, in milliseconds.
const POLL_MS: u32 = 2000;
/// Longest instruction, in characters.
const MAX_ASK: usize = 400;

#[derive(Default)]
pub struct Assist {
    /// The words being typed (the keys go to them while `focus`).
    pub input: String,
    pub focus: bool,
    /// The run the answer is awaited from, and the time since it was last asked.
    pub run: Option<String>,
    pub waited: u32,
    /// Hunks of a reply that holds an edit, being reviewed.
    pub review: Option<Review>,
}

pub struct Review {
    old: Vec<String>,
    hunks: Vec<Hunk>,
    decided: Vec<Option<bool>>,
    cur: usize,
    explanation: String,
    trailing: bool,
}

fn lines_of(t: &str) -> Vec<String> {
    let t = t.replace("\r\n", "\n");
    t.strip_suffix('\n').unwrap_or(&t).split('\n').map(String::from).collect()
}

impl<S: Source> EditTile<S> {
    /// The words the person types for the assistant.
    pub(super) fn assist_text(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.assist.input.chars().count() < MAX_ASK {
                self.assist.input.push(c);
            }
        }
    }

    /// Ask: the open text and the words go to the service; it starts the run.
    pub(super) fn assist_ask(&mut self) -> bool {
        let words = self.assist.input.trim().to_string();
        self.assist.focus = false;
        if words.is_empty() {
            self.note = "Type what you want changed first.".into();
            return true;
        }
        if self.assist.run.is_some() {
            self.note = "The assistant is still answering.".into();
            return true;
        }
        let text = self.buf.text();
        if text.len() > MAX_BYTES {
            self.note = "The text is too long to send to the assistant.".into();
            return true;
        }
        let mut args = json!({"path": self.path.trim(), "text": text, "instruction": words});
        // the selected lines go along as context (F-WEB-59): the first and last line number and the selected text, 4000 characters at most
        if let (Some((a, b)), Some(sel)) = (self.buf.selection(), self.buf.selected_text().filter(|t| !t.is_empty())) {
            args["selection"] = json!({"from": a.0 + 1, "to": b.0 + 1, "text": sel.chars().take(4000).collect::<String>()});
        }
        match call_json(&mut self.src, "assist", "ask", &args.to_string()) {
            Ok(v) => match v["id"].as_str().filter(|s| !s.is_empty()) {
                Some(id) => {
                    self.assist.run = Some(id.to_string());
                    self.assist.waited = 0;
                    self.assist.input.clear();
                    self.note = "Asked the assistant; its suggestion shows here when it is ready.".into();
                }
                None => self.note = v["error"].as_str().map(|e| e.chars().take(160).collect()).unwrap_or_else(|| "the service did not start the assistant".into()),
            },
            Err(e) => self.note = e.chars().take(160).collect(),
        }
        true
    }

    /// Time passes: ask for the answer now and then.
    pub(super) fn assist_tick(&mut self, dt: u32) -> bool {
        if self.assist.run.is_none() {
            return false;
        }
        self.assist.waited = self.assist.waited.saturating_add(dt);
        if self.assist.waited < POLL_MS {
            return false;
        }
        self.assist.waited = 0;
        self.assist_poll()
    }

    fn assist_poll(&mut self) -> bool {
        let Some(id) = self.assist.run.clone() else { return false };
        match call_json(&mut self.src, "assist", "result", &json!({"id": id}).to_string()) {
            Ok(v) if v["done"] == true => {
                self.assist.run = None;
                if let Some(e) = v["error"].as_str().filter(|e| !e.is_empty()) {
                    self.note = format!("The assistant failed: {}", e.chars().take(140).collect::<String>());
                    return true;
                }
                self.assist_reply(v["reply"].as_str().unwrap_or(""));
                true
            }
            Ok(_) => false,
            Err(e) => {
                self.assist.run = None;
                self.note = e.chars().take(160).collect();
                true
            }
        }
    }

    /// A reply: an edit becomes a review, anything else is just an answer to read.
    pub(super) fn assist_reply(&mut self, reply: &str) {
        let current = self.buf.text();
        match parse_reply(reply, &current) {
            Ok(Some(p)) => {
                let (old, new) = (lines_of(&current), lines_of(&p.new_text));
                let hunks = diff_hunks(&old, &new);
                if hunks.is_empty() {
                    self.note = "The suggested edit changes nothing in the file.".into();
                    return;
                }
                let n = hunks.len();
                let (a, r) = summary(&hunks);
                self.assist.review = Some(Review { old, decided: vec![None; n], hunks, cur: 0, explanation: p.explanation, trailing: p.new_text.ends_with('\n') || current.ends_with('\n') });
                self.note = format!("The assistant suggests {n} change{} (+{a} -{r}). y accept, n reject, a accept all, Enter apply, Esc discard.", if n == 1 { "" } else { "s" });
            }
            Ok(None) => self.note = format!("assistant: {}", reply.chars().filter(|c| !c.is_control() || *c == ' ').take(160).collect::<String>()),
            Err(e) => self.note = e.chars().take(160).collect(),
        }
    }

    /// A button of the review pressed: the same as its key.
    pub(super) fn assist_review_hit(&mut self, id: u32) -> bool {
        let code = match id {
            9 => 'y' as u32,
            10 => 'n' as u32,
            11 => 'a' as u32,
            12 => 13,
            13 => 27,
            14 => 0x101,
            _ => return false,
        };
        self.assist.review.is_some() && self.assist_review_key(code)
    }

    /// Keys while a suggestion is under review. True when the key was taken.
    pub(super) fn assist_review_key(&mut self, code: u32) -> bool {
        let Some(r) = self.assist.review.as_mut() else { return false };
        let n = r.hunks.len();
        let next = |r: &Review| (1..=n).map(|d| (r.cur + d) % n).find(|i| r.decided[*i].is_none());
        match code {
            c if c == 'y' as u32 || c == 'Y' as u32 => {
                r.decided[r.cur] = Some(true);
                if let Some(i) = next(r) {
                    r.cur = i;
                }
            }
            c if c == 'n' as u32 || c == 'N' as u32 => {
                r.decided[r.cur] = Some(false);
                if let Some(i) = next(r) {
                    r.cur = i;
                }
            }
            c if c == 'a' as u32 || c == 'A' as u32 => {
                for d in r.decided.iter_mut() {
                    *d = Some(true);
                }
            }
            0x101 | 0x103 => r.cur = (r.cur + 1) % n,
            0x100 | 0x102 => r.cur = (r.cur + n - 1) % n,
            13 => {
                let accept: Vec<bool> = r.decided.iter().map(|d| *d == Some(true)).collect();
                let lines = apply_hunks(&r.old, &r.hunks, &accept);
                let mut text = lines.join("\n");
                if r.trailing {
                    text.push('\n');
                }
                let taken = accept.iter().filter(|a| **a).count();
                self.assist.review = None;
                match self.buf.set_text_undoable(&text) {
                    Ok(()) => self.note = format!("Applied {taken} change{} to the text (Ctrl+Z undoes them in one step). Save writes the file.", if taken == 1 { "" } else { "s" }),
                    Err(_) => self.note = "That text does not fit.".into(),
                }
            }
            27 => {
                self.assist.review = None;
                self.note = "Suggestion discarded; the text is as it was.".into();
            }
            _ => {}
        }
        true
    }

    /// The review drawn over the text area: the chosen change as `-` and `+` lines, and what the others are.
    pub(super) fn assist_draw(&self, l: &mut DisplayList, p: &Palette) {
        let Some(r) = &self.assist.review else { return };
        let (w, h) = self.size;
        l.push(Op::Rect { r: Rect::new(0, TOP, w, h - TOP - 20), color: p.bg });
        let lh = line_height(12) + 2;
        let n = r.hunks.len();
        let (a, d) = summary(&r.hunks);
        let kept = r.decided.iter().filter(|d| **d == Some(true)).count();
        let mut y = TOP + 2;
        l.push(text(8, y, 12, p.accent, &clip(&format!("Assistant: change {} of {n} (+{a} -{d}), {kept} accepted. y accept, n reject, a all, Enter apply, Esc discard", r.cur + 1), w - 16, 12)));
        y += lh;
        if !r.explanation.is_empty() {
            l.push(text(8, y, 12, p.mute, &clip(&r.explanation.replace('\n', " "), w - 16, 12)));
            y += lh;
        }
        let h0 = &r.hunks[r.cur];
        let state = match r.decided[r.cur] {
            Some(true) => "accepted",
            Some(false) => "rejected",
            None => "not decided",
        };
        l.push(text(8, y, 12, p.mute, &format!("at line {} ({state})", h0.old_start + 1)));
        y += lh;
        let room = ((h - 20 - y) / lh).max(1) as usize;
        let rows: Vec<(char, &String)> = h0.old.iter().map(|s| ('-', s)).chain(h0.new.iter().map(|s| ('+', s))).collect();
        for (sign, line) in rows.iter().take(room) {
            let c = if *sign == '-' { p.bad } else { p.good };
            l.push(text(8, y, 12, c, &clip(&format!("{sign} {line}"), w - 16, 12)));
            y += lh;
        }
        // the same answers with the mouse or a finger: one button each, along the bottom
        let by = h - 46;
        let labels: [(u32, &str); 6] = [(9, "Accept"), (10, "Reject"), (11, "All"), (12, "Apply"), (13, "Discard"), (14, "Next")];
        let bw = ((w - 12) / labels.len() as i32).clamp(40, 80);
        for (i, (id, label)) in labels.iter().enumerate() {
            iyakki_tile_kit::button(l, *id, Rect::new(6 + i as i32 * (bw + 2), by, bw, 22), label, p);
        }
        if rows.len() > room {
            l.push(text(8, y.min(h - 36), 12, p.mute, &format!("... {} more lines", rows.len() - room)));
        }
    }
}
