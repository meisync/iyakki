//! VOICE, DICTATE: "Correct my wording with the assistant", as the person talks.
//!
//! The raw words go into the box the moment speech-to-text returns, and Keep listening records again at once. The correction runs in the
//! background (one small thread per chunk); when it returns, the corrected words REPLACE that chunk in place, but only if the chunk still
//! reads exactly as it was inserted (the person has not edited it): otherwise the person's edit stays and the correction is dropped
//! quietly. Each chunk has a serial, and the answer is matched to its chunk by that serial, never by position: the chunk's place is looked
//! up again in the text when the answer comes (where it was put, else its one exact match), so chunks that were corrected or edited before
//! it cannot send it to the wrong place. The last [`KEEP`] chunks are remembered for "Show original" / "Undo wording".
//!
//! When the words are kept (timeout, the checks, an error, no model, no consent) the reason stays on the Correct-my-wording row until
//! the next chunk is answered or the box is cleared.

use std::sync::mpsc::channel;

use iyakki_agent::dictfix::{self, Fixed};

use super::*;

/// How many finished chunks are remembered.
pub(crate) const KEEP: usize = 5;

/// A chunk of dictated text in the box.
#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    pub serial: u64,
    /// Where it was put in the box (characters).
    pub start: usize,
    /// What the speech engine heard.
    pub raw: String,
    /// The corrected words (empty until a correction was applied).
    pub text: String,
    /// The box shows the original now.
    pub showing_raw: bool,
    /// The correction has not come back yet.
    pub pending: bool,
    /// A correction was applied to this chunk.
    pub changed: bool,
}

impl Chunk {
    /// What the box holds for this chunk now.
    fn shown(&self) -> &str {
        if self.changed && !self.showing_raw { &self.text } else { &self.raw }
    }
}

pub(crate) type FixChannel = (std::sync::mpsc::Sender<(u64, Fixed)>, std::sync::mpsc::Receiver<(u64, Fixed)>);

pub(crate) fn fix_channel() -> FixChannel {
    channel()
}

impl VoiceApp {
    /// Corrections still on their way.
    pub(crate) fn pending_fixes(&self) -> usize {
        self.fixes.iter().filter(|c| c.pending).count()
    }

    /// The small "tidying" marker next to the Correct-my-wording box (None: nothing is pending). `dots` animates it.
    pub(crate) fn tidy_marker(&self, dots: usize) -> Option<String> {
        let n = self.pending_fixes();
        let d = ".".repeat(1 + dots % 3);
        match n {
            0 => None,
            1 => Some(format!("tidying{d}")),
            n => Some(format!("tidying {n} parts{d}")),
        }
    }

    /// The newest chunk a correction was applied to (the one "Show original" works on).
    pub(crate) fn last_fix(&self) -> Option<&Chunk> {
        self.fixes.iter().rev().find(|c| c.changed)
    }

    /// Forget every chunk (the box was cleared or replaced as a whole). Answers that arrive later are dropped quietly.
    pub(crate) fn fixes_forget(&mut self) {
        self.fixes.clear();
    }

    /// The box was cleared: chunks, the reason shown and the count of empty results start again.
    pub(crate) fn fixes_clear(&mut self) {
        self.fixes_forget();
        self.fix_note = None;
        self.empty_streak = 0;
    }

    fn trim_chunks(&mut self) {
        let done = self.fixes.iter().filter(|c| !c.pending).count();
        let mut drop_n = done.saturating_sub(KEEP);
        self.fixes.retain(|c| {
            if c.pending || drop_n == 0 {
                return true;
            }
            drop_n -= 1;
            false
        });
    }

    /// A transcript arrived: its raw words go into the box now, and (with the option on) the correction starts in the background.
    pub(crate) fn add_chunk(&mut self, cx: &AppCx, raw: String, fix: bool) {
        let ctx = dictfix::Context::after(&self.transcript.text);
        if !self.transcript.text.is_empty() && !self.transcript.text.ends_with('\n') {
            self.transcript.move_to(self.transcript.len(), false);
            self.transcript.insert("\n");
        }
        self.transcript.move_to(self.transcript.len(), false);
        let start = self.transcript.len();
        self.transcript.insert(&raw);
        // what the box really holds (it may have cut the text to its limit)
        let inserted: String = self.transcript.text.chars().skip(start).collect();
        let mut line = format!("Transcript ready ({} characters). You can edit it.", inserted.chars().count());
        if fix && !inserted.trim().is_empty() {
            self.fix_serial += 1;
            let serial = self.fix_serial;
            self.fixes.push(Chunk { serial, start, raw: inserted.clone(), text: String::new(), showing_raw: false, pending: true, changed: false });
            self.trim_chunks();
            let (svc, tx) = (cx.services.clone(), self.fix_tx.clone());
            std::thread::spawn(move || {
                let f = dictfix::fix_wording(&svc, &inserted, &ctx);
                let _ = tx.send((serial, f));
            });
            line.push_str(" The assistant is tidying the wording in the background.");
        }
        self.say(line, true);
    }

    /// Where the chunk reads as `expect` now: where it was put, else its one exact match; None when it was edited (or is ambiguous).
    fn locate(&self, start: usize, expect: &str) -> Option<usize> {
        let hay: Vec<char> = self.transcript.text.chars().collect();
        let need: Vec<char> = expect.chars().collect();
        if need.is_empty() || need.len() > hay.len() {
            return None;
        }
        if start + need.len() <= hay.len() && hay[start..start + need.len()] == need[..] {
            return Some(start);
        }
        let mut found = None;
        for i in 0..=hay.len() - need.len() {
            if hay[i..i + need.len()] == need[..] {
                if found.is_some() {
                    return None;
                }
                found = Some(i);
            }
        }
        found
    }

    /// Put `to` where `from` stands at `at`, keeping the caret and the scroll; later chunks move by the change in length.
    fn replace_at(&mut self, at: usize, from: &str, to: &str, serial: u64) {
        let chars: Vec<char> = self.transcript.text.chars().collect();
        let end = at + from.chars().count();
        let mut out: String = chars[..at].iter().collect();
        out.push_str(to);
        out.extend(chars[end..].iter());
        let (cur, anchor, top) = (self.transcript.cur, self.transcript.anchor, self.transcript.top);
        self.transcript.set(&out);
        let delta = to.chars().count() as isize - from.chars().count() as isize;
        let fix = |p: usize| if p >= end { (p as isize + delta).max(0) as usize } else if p > at { at + to.chars().count() } else { p };
        let len = self.transcript.len();
        self.transcript.cur = fix(cur).min(len);
        self.transcript.anchor = anchor.map(|a| fix(a).min(len));
        self.transcript.top = top;
        for c in self.fixes.iter_mut() {
            if c.serial != serial && c.start >= end {
                c.start = (c.start as isize + delta).max(0) as usize;
            }
        }
        if let Some(c) = self.fixes.iter_mut().find(|c| c.serial == serial) {
            c.start = at;
        }
    }

    /// A correction came back for chunk `serial`.
    pub(crate) fn apply_fix(&mut self, cx: &AppCx, serial: u64, f: Fixed) {
        let Some(i) = self.fixes.iter().position(|c| c.serial == serial) else {
            // the box was cleared meanwhile: nothing to put it in
            cx.audit.record(&cx.session.user, dictfix::AUDIT, "unchanged");
            return;
        };
        self.fixes[i].pending = false;
        let (start, expect) = (self.fixes[i].start, self.fixes[i].raw.clone());
        let mut detail = f.audit_detail();
        if f.changed {
            match self.locate(start, &expect) {
                Some(at) => {
                    self.replace_at(at, &expect, &f.text, serial);
                    if let Some(c) = self.fixes.iter_mut().find(|c| c.serial == serial) {
                        c.text = f.text.clone();
                        c.changed = true;
                        c.showing_raw = false;
                    }
                    self.fix_note = None;
                    self.say("The assistant tidied the wording; \"Show original\" brings back what was heard.", true);
                }
                None => {
                    // the person edited that part: their edit stays, the correction is dropped quietly
                    detail = "unchanged";
                    self.fix_note = None;
                }
            }
        } else if let Some(why) = f.kept_because.clone() {
            let why = why.trim_end_matches('.').to_string();
            self.fix_note = Some(why.clone());
            self.say(format!("{why}."), true);
        } else {
            self.fix_note = None;
        }
        cx.audit.record(&cx.session.user, dictfix::AUDIT, detail);
        self.trim_chunks();
        self.dirty = true;
    }

    /// Collect the corrections that came back.
    pub(crate) fn fix_tick(&mut self, cx: &AppCx) {
        while let Ok((serial, f)) = self.fix_rx.try_recv() {
            self.apply_fix(cx, serial, f);
        }
    }

    /// Swap the newest corrected chunk between the corrected words and what the speech engine heard (only while it has not been edited).
    pub(super) fn toggle_original(&mut self) {
        let Some(c) = self.last_fix().cloned() else { return };
        let (from, to) = if c.showing_raw { (c.raw.clone(), c.text.clone()) } else { (c.text.clone(), c.raw.clone()) };
        debug_assert_eq!(c.shown(), from);
        let Some(at) = self.locate(c.start, &from) else {
            self.say("You have edited that part, so I left it as it is.", false);
            return;
        };
        self.replace_at(at, &from, &to, c.serial);
        if let Some(x) = self.fixes.iter_mut().find(|x| x.serial == c.serial) {
            x.showing_raw = !c.showing_raw;
        }
        self.say(if !c.showing_raw { "Showing what was heard. Press the button again for the assistant's wording." } else { "Showing the assistant's wording." }, true);
        self.focus = Focus::Transcript;
    }
}
