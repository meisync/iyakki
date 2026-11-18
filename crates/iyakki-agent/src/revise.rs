//! The side assistant for a transcript: the person gives an instruction ("shorter", "translate to Tamil", or their own words) and the chat
//! model proposes a revised text. Nothing here ever changes the person's text: [`revise`] returns a [`Proposal`] and the person presses Apply.
//!
//! The call is a one-shot without tools (`ChatBackend::complete`). The TEXT is untrusted data, given only as a JSON-quoted string, and the
//! system instruction says to obey the INSTRUCTION field alone; the INSTRUCTION is the person's own (length capped). A reply is checked
//! ([`accept`]): non-empty, within a length ratio that suits the kind of instruction (a shorter text may shrink a lot, a longer one may grow),
//! with a thinking block, a code fence or wrapping quotes taken off. On any failure the text is kept and the reason is said in plain words.
//!
//! The model is the chat model (`model.chat`). A model on another machine is used only after the person agreed to send text there (the same
//! yes as `dictfix`, `Services::allow_speech_server`); a model on this computer needs no ask. The undo stack ([`History`]) lives in memory only.

pub use iyakki_shared::revise::{DIFF_MAX_WORDS, AUDIT, Context, HISTORY_STEPS, History, MAX_INSTRUCTION_CHARS, MAX_TEXT_CHARS, NO_MODEL, SYSTEM, Seg, TIMEOUT, build_messages, capital, diff_counts, splice, word_diff};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::chat::{ChatBackend, Message};
use crate::dictfix::{self, Target};
use crate::services::Services;
use crate::settings::ModelChoice;
pub use iyakki_shared::revise::{Kind, Op, Proposal};










/// Check a reply against the text that was sent: the revised text, or why it was not used.
pub fn accept(original: &str, reply: &str, kind: &Kind) -> Result<String, String> {
    let text = dictfix::clean_reply(original, reply);
    if text.is_empty() {
        return Err("the assistant answered with nothing".into());
    }
    let (rl, tl) = (original.trim().chars().count(), text.chars().count());
    if tl > MAX_TEXT_CHARS * 5 {
        return Err("the assistant's answer was far too long".into());
    }
    if rl >= 20 {
        let (lo, hi) = kind.ratio();
        let ratio = tl as f32 / rl as f32;
        if ratio < lo {
            return Err("the assistant's answer was much shorter than this kind of change should give".into());
        }
        if ratio > hi {
            return Err("the assistant's answer was much longer than this kind of change should give".into());
        }
    }
    Ok(text)
}

/// One revision through `backend`, waiting at most `timeout`. Never fails: the text comes back with the reason.
pub fn revise_with(backend: Arc<dyn ChatBackend>, text: &str, kind: &Kind, timeout: Duration) -> Proposal {
    if text.trim().is_empty() {
        return Proposal::kept(text, "there is no text to revise yet");
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Proposal::kept(text, format!("the text is longer than {MAX_TEXT_CHARS} characters; select a part of it and ask again"));
    }
    let instruction = kind.instruction();
    if instruction.is_empty() {
        return Proposal::kept(text, "type what you want changed first");
    }
    if instruction.chars().count() > MAX_INSTRUCTION_CHARS {
        return Proposal::kept(text, format!("the instruction is longer than {MAX_INSTRUCTION_CHARS} characters; make it shorter"));
    }
    let msgs = build_messages(text, &instruction);
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("revise".into()).spawn(move || {
        crate::providers::set_thread_reasoning(Some(crate::dictfix::REASONING));
        let mut r = backend.complete(&msgs);
        if r.as_ref().is_err_and(|e| e.contains("400")) {
            crate::providers::set_thread_reasoning(None);
            r = backend.complete(&msgs);
        }
        let _ = tx.send(r);
    });
    if spawned.is_err() {
        return Proposal::kept(text, "kept your text: the revision could not start");
    }
    match rx.recv_timeout(timeout) {
        Err(_) => Proposal::kept(text, "kept your text: the assistant did not answer in time"),
        Ok(Err(_)) => Proposal::kept(text, "kept your text: the assistant could not be reached"),
        Ok(Ok(reply)) => match accept(text, &reply, kind) {
            Ok(revised) => {
                let changed = revised != text.trim();
                if !changed {
                    return Proposal { text: text.to_string(), note: "Nothing needed changing.".into(), changed: false, kept_because: None, needs_consent: None };
                }
                let (add, del) = diff_counts(&word_diff(text, &revised));
                Proposal { text: revised, note: format!("{}: {add} words added, {del} removed.", capital(kind.label())), changed: true, kept_because: None, needs_consent: None }
            }
            Err(why) => Proposal::kept(text, format!("kept your text: {why}")),
        },
    }
}


/// The model to use and whether it may be used now (the question is worded for revising).
pub fn target(svc: &Services, ctx: &Context) -> Target {
    match dictfix::target(svc, ctx.model.as_ref()) {
        Target::NeedsConsent(c, server, q) => {
            let q = q.replace("Correcting your wording sends the dictated text", "Revising your text sends the text you choose");
            Target::NeedsConsent(c, server, q)
        }
        other => other,
    }
}

/// Revise `text` with the person's chat model. Blocks up to [`TIMEOUT`]; call it from a worker thread.
pub fn revise(svc: &Arc<Services>, text: &str, kind: &Kind, ctx: &Context) -> Proposal {
    match target(svc, ctx) {
        Target::NoModel => Proposal::kept(text, NO_MODEL),
        Target::NeedsConsent(_, server, q) => Proposal {
            needs_consent: Some((server, q)),
            ..Proposal::kept(text, "kept your text: the assistant is on another machine and you have not agreed to send text there yet")
        },
        Target::Ready(choice) => {
            let t = TIMEOUT.max(dictfix::timeout_for(svc, &choice));
            revise_with(svc.chat_backend_for(choice), text, kind, t)
        }
    }
}

// ------------------------------------------------------------------------------------------------ the word diff






// ------------------------------------------------------------------------------------------------ the text and its history





#[cfg(test)]
#[path = "revise_tests.rs"]
mod tests;
