//! Consent: questions only the local screen can answer. The queue is in memory (a question that outlives a restart is asked again
//! by whatever raised it). A remote client lists the open questions; answering needs a [`LocalSeat`], which a remote context cannot make.

use std::sync::Mutex;

use iyakki_core_api::contract::*;

use super::now_ms;

/// How long a question waits for a person before it expires.
pub const WAIT_MS: u64 = 5 * 60 * 1000;
const MAX_OPEN: usize = 32;

#[derive(Default)]
pub struct CoreConsents {
    open: Mutex<Vec<ConsentRequest>>,
    answered: Mutex<Vec<(String, ConsentAnswer, String)>>,
    counter: std::sync::atomic::AtomicU64,
}

impl CoreConsents {
    pub fn new() -> CoreConsents {
        CoreConsents::default()
    }

    /// A tile, a flow or the vault raises a question for the person at the machine; the id to answer it with.
    pub fn ask(&self, kind: ConsentKind, subject: &str, text_key: &str, text: &str) -> String {
        let n = self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let id = format!("c{n}");
        let now = now_ms();
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep(&mut open);
        // bounded: a flood of questions (a remote client asking again and again) pushes out the oldest, never grows without end
        if open.len() >= MAX_OPEN {
            open.remove(0);
        }
        open.push(ConsentRequest { id: id.clone(), kind, subject: subject.to_string(), text_key: text_key.to_string(), text: text.to_string(), created_ms: now, expires_ms: now + WAIT_MS });
        id
    }

    /// Like [`CoreConsents::ask`], but a question of the same kind from the same subject that is still open is not asked twice.
    pub fn ask_once(&self, kind: ConsentKind, subject: &str, text_key: &str, text: &str) -> String {
        {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            self.sweep(&mut open);
            if let Some(r) = open.iter().find(|r| r.kind == kind && r.subject == subject) {
                return r.id.clone();
            }
        }
        self.ask(kind, subject, text_key, text)
    }

    /// (question id, answer, who answered), oldest first.
    pub fn answered(&self) -> Vec<(String, ConsentAnswer, String)> {
        self.answered.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Tests only: make a question look as old as its time allows.
    #[cfg(test)]
    pub(crate) fn age_for_test(&self, id: &str) {
        if let Some(r) = self.open.lock().unwrap().iter_mut().find(|r| r.id == id) {
            r.expires_ms = 1;
        }
    }

    fn sweep(&self, open: &mut Vec<ConsentRequest>) {
        let now = now_ms();
        open.retain(|r| r.expires_ms > now);
    }
}

impl Consents for CoreConsents {
    fn pending(&self, ctx: &CallContext) -> CallResult<Vec<ConsentRequest>> {
        guard(ctx, Scope::CONSENT_READ, Risk::Read)?;
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep(&mut open);
        Ok(open.clone())
    }

    fn answer(&self, seat: &LocalSeat, id: &str, answer: ConsentAnswer) -> CallResult<()> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let Some(i) = open.iter().position(|r| r.id == id) else {
            return Err(CallError::new(ErrorCode::NotFound, "consent.unknown", "No such question is waiting."));
        };
        if open[i].expires_ms <= now_ms() {
            open.remove(i);
            return Err(CallError::new(ErrorCode::Expired, "consent.expired", "That question ran out of time.").hint("Ask again."));
        }
        open.remove(i);
        self.answered.lock().unwrap_or_else(|e| e.into_inner()).push((id.to_string(), answer, seat.user().to_string()));
        Ok(())
    }
}
