//! A terminal for tests (the `Terminal` facet): lines are echoed as `ran: <line>`; a line that is not read-only waits for a yes, like the real
//! one, because it uses the same classification (`iyakki_core_api::terminal`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use iyakki_core_api::terminal::{self as t, TermApproval, TermOpen, TermReply, Terminal};

#[derive(Default)]
struct Inner {
    sessions: BTreeMap<(String, String), Option<(String, String)>>,
    next: u32,
    /// What was run, in order: `user: line`.
    pub log: Vec<String>,
}

#[derive(Default)]
pub struct FakeTerminal {
    inner: Mutex<Inner>,
    off: bool,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeTerminal {
    pub fn log(&self) -> Vec<String> {
        lk(&self.inner).log.clone()
    }
}

impl Terminal for FakeTerminal {
    fn open(&self, user: &str, _app: &str) -> Result<TermOpen, String> {
        let _ = self.off;
        let mut g = lk(&self.inner);
        if g.sessions.keys().filter(|(u, _)| u == user).count() >= t::MAX_SESSIONS {
            return Err(format!("at most {} terminals can be open at once", t::MAX_SESSIONS));
        }
        g.next += 1;
        let id = format!("t{}", g.next);
        g.sessions.insert((user.into(), id.clone()), None);
        Ok(TermOpen { id, prompt: format!("{user}$"), banner: "fake terminal".into() })
    }

    fn run(&self, user: &str, session: &str, line: &str) -> Result<TermReply, String> {
        t::check_line(line)?;
        let mut g = lk(&self.inner);
        let prompt = format!("{user}$");
        let key = (user.to_string(), session.to_string());
        match g.sessions.get(&key) {
            None => return Err("that terminal is not open".into()),
            Some(Some(_)) => return Err("answer the waiting question first".into()),
            _ => {}
        }
        if t::is_power(line) {
            return Ok(TermReply { text: t::NO_POWER.into(), prompt, ..TermReply::default() });
        }
        if !t::runs_without_asking(line) {
            g.next += 1;
            let call = format!("c{}", g.next);
            g.sessions.insert(key, Some((call.clone(), line.to_string())));
            return Ok(TermReply { prompt, approval: Some(TermApproval { call, summary: format!("Run in your terminal: {line}") }), ..TermReply::default() });
        }
        g.log.push(format!("{user}: {line}"));
        Ok(TermReply { text: format!("ran: {line}"), prompt, clear: line.trim() == "clear", ..TermReply::default() })
    }

    fn decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<TermReply, String> {
        let mut g = lk(&self.inner);
        let key = (user.to_string(), session.to_string());
        let prompt = format!("{user}$");
        let Some(Some((c, line))) = g.sessions.get(&key).cloned() else { return Err("there is no such question waiting".into()) };
        if c != call {
            return Err("there is no such question waiting".into());
        }
        g.sessions.insert(key, None);
        if !approve {
            return Ok(TermReply { text: "Not run: you said no.".into(), prompt, ..TermReply::default() });
        }
        g.log.push(format!("{user}: {line}"));
        Ok(TermReply { text: format!("ran: {line}"), prompt, ..TermReply::default() })
    }

    fn close(&self, user: &str, session: &str) {
        lk(&self.inner).sessions.remove(&(user.to_string(), session.to_string()));
    }
}
