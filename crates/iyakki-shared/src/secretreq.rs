//! Asking the person for a secret without the secret ever passing through the assistant. A tool call (`vault_request`)
//! registers a request here and waits; the INBOX (Approvals page) and the open chat show it, the person types the value into
//! a masked form, and it goes straight into their own vault under the name they confirm. The assistant only ever learns
//! `stored as uvault:<name>` or `cancelled`. Requests hold names and purposes, never values.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub const MAX_PENDING: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRequest {
    pub id: String,
    pub user: String,
    /// The vault name suggested to the person (they may change it).
    pub name: String,
    /// What the secret is for, as the assistant described it.
    pub purpose: String,
    /// `password`, `api_key`, `token` or `other`.
    pub kind: String,
    pub since: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Stored in the person's vault under this name.
    Stored(String),
    Cancelled,
}

#[derive(Default)]
struct State {
    open: Vec<SecretRequest>,
    done: HashMap<String, Outcome>,
    next: u64,
}

#[derive(Default)]
pub struct SecretRequests {
    st: Mutex<State>,
    cv: Condvar,
}

/// The name to suggest: lowercase letters, digits, `.`, `_`, `-`, under the `svc.` prefix.
pub fn suggest_name(wanted: &str) -> String {
    let mut n: String = wanted.trim().to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    n = n.trim_matches(|c| c == '-' || c == '.').to_string();
    if n.is_empty() {
        n = "secret".into();
    }
    if !n.contains('.') {
        n = format!("svc.{n}");
    }
    n.chars().take(64).collect()
}

impl SecretRequests {
    pub fn new() -> SecretRequests {
        SecretRequests::default()
    }

    /// Register a request; returns its id, or an error when the person already has too many waiting.
    pub fn create(&self, user: &str, name: &str, purpose: &str, kind: &str, now: i64) -> Result<String, String> {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        if g.open.iter().filter(|r| r.user == user).count() >= MAX_PENDING {
            return Err("too many secret requests are waiting; ask the person to answer them first".into());
        }
        g.next += 1;
        let id = format!("sr{}", g.next);
        let purpose: String = purpose.chars().filter(|c| !c.is_control()).take(200).collect();
        g.open.push(SecretRequest { id: id.clone(), user: user.into(), name: suggest_name(name), purpose, kind: kind.into(), since: now });
        Ok(id)
    }

    pub fn pending(&self, user: &str) -> Vec<SecretRequest> {
        self.st.lock().unwrap_or_else(|e| e.into_inner()).open.iter().filter(|r| r.user == user).cloned().collect()
    }

    pub fn get(&self, user: &str, id: &str) -> Option<SecretRequest> {
        self.pending(user).into_iter().find(|r| r.id == id)
    }

    /// Close a request with its outcome (only its owner may).
    pub fn resolve(&self, user: &str, id: &str, outcome: Outcome) -> Result<(), String> {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pos) = g.open.iter().position(|r| r.id == id && r.user == user) else { return Err("that request is not waiting any more".into()) };
        g.open.remove(pos);
        g.done.insert(id.to_string(), outcome);
        self.cv.notify_all();
        Ok(())
    }

    /// Wait for the person's answer; `None` when the time ran out (the request is then withdrawn).
    pub fn wait(&self, id: &str, timeout: Duration) -> Option<Outcome> {
        let end = Instant::now() + timeout;
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(o) = g.done.remove(id) {
                return Some(o);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                g.open.retain(|r| r.id != id);
                return None;
            }
            g = self.cv.wait_timeout(g, left.min(Duration::from_millis(200))).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn names_are_suggested_under_svc_and_always_valid() {
        assert_eq!(suggest_name("Plex Server"), "svc.plex-server");
        assert_eq!(suggest_name("svc.nas"), "svc.nas");
        assert_eq!(suggest_name("  "), "svc.secret");
        for n in ["Plex Server", "ünï", "a/b", &"x".repeat(100)] {
            assert!(crate::vault::valid_secret_name(&suggest_name(n)), "{n}");
        }
    }

    #[test]
    fn a_waiting_request_is_answered_from_another_thread_by_its_owner_only() {
        let r = Arc::new(SecretRequests::new());
        let id = r.create("ann", "nas", "SFTP password for the NAS", "password", 5).unwrap();
        assert_eq!(r.pending("ann").len(), 1);
        assert!(r.pending("bob").is_empty());
        assert!(r.resolve("bob", &id, Outcome::Cancelled).is_err(), "someone else cannot answer");
        let r2 = r.clone();
        let id2 = id.clone();
        let h = std::thread::spawn(move || r2.wait(&id2, Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        r.resolve("ann", &id, Outcome::Stored("svc.nas".into())).unwrap();
        assert_eq!(h.join().unwrap(), Some(Outcome::Stored("svc.nas".into())));
        assert!(r.pending("ann").is_empty());
        // no answer in time: withdrawn
        let id = r.create("ann", "x", "y", "token", 6).unwrap();
        assert_eq!(r.wait(&id, Duration::from_millis(30)), None);
        assert!(r.pending("ann").is_empty());
        for _ in 0..MAX_PENDING {
            r.create("ann", "x", "y", "token", 7).unwrap();
        }
        assert!(r.create("ann", "x", "y", "token", 7).is_err());
    }
}
