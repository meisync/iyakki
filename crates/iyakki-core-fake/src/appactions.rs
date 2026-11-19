//! The fake app actions: the real waiting list ([`ActionBook`]) and a record of the notes and flows (nothing reaches an inbox or an
//! engine). A test grants the capabilities the app holds, adds the flows that exist and reads what was done.

use iyakki_core_api::appactions::{clean_note, ActionBook, AppActions, PendingAction};
use std::collections::BTreeSet;
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeAppActions {
    book: ActionBook,
    held: Mutex<BTreeSet<(String, String)>>,
    flows: Mutex<BTreeSet<String>>,
    notes: Mutex<Vec<(String, String, String)>>,
    started: Mutex<Vec<(String, String, String)>>,
    audit: Mutex<Vec<String>>,
    now: std::sync::atomic::AtomicU64,
}

impl FakeAppActions {
    pub fn grant(&self, app: &str, cap: &str) {
        self.held.lock().unwrap().insert((app.into(), cap.into()));
    }
    pub fn add_flow(&self, name: &str) {
        self.flows.lock().unwrap().insert(name.into());
    }
    pub fn set_time(&self, ms: u64) {
        self.now.store(ms, std::sync::atomic::Ordering::SeqCst);
    }
    /// `(person, app, text)` of every note left.
    pub fn notes(&self) -> Vec<(String, String, String)> {
        self.notes.lock().unwrap().clone()
    }
    /// `(person, app, flow)` of every flow started.
    pub fn started(&self) -> Vec<(String, String, String)> {
        self.started.lock().unwrap().clone()
    }
    fn check(&self, app: &str, cap: &str) -> Result<(), String> {
        if self.held.lock().unwrap().contains(&(app.to_string(), cap.to_string())) {
            Ok(())
        } else {
            self.audit.lock().unwrap().push(format!("appactions.refuse {app}: {cap}"));
            Err(format!("{app} does not hold {cap}: the person grants it first"))
        }
    }
}

impl AppActions for FakeAppActions {
    fn notify(&self, user: &str, app: &str, text: &str) -> Result<(), String> {
        self.check(app, "notify")?;
        let t = clean_note(text)?;
        self.notes.lock().unwrap().push((user.into(), app.into(), t));
        self.audit.lock().unwrap().push(format!("appactions.notify {app}"));
        Ok(())
    }
    fn start_flow(&self, user: &str, app: &str, name: &str) -> Result<String, String> {
        self.check(app, "flows.start")?;
        if !self.flows.lock().unwrap().contains(name) {
            return Err(format!("there is no published flow called {name}"));
        }
        self.started.lock().unwrap().push((user.into(), app.into(), name.into()));
        self.audit.lock().unwrap().push(format!("appactions.flow {app}: {name}"));
        Ok(format!("started {name}"))
    }
    fn request_tool(&self, user: &str, app: &str, tool: &str, args: &str) -> Result<String, String> {
        self.check(app, "tools.request")?;
        let id = self.book.ask(user, app, tool, args, self.now.load(std::sync::atomic::Ordering::SeqCst))?;
        self.audit.lock().unwrap().push(format!("appactions.request {app}: {tool} {id}"));
        Ok(id)
    }
    fn pending(&self, user: &str) -> Vec<PendingAction> {
        self.book.pending(user, self.now.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn take(&self, user: &str, id: &str) -> Option<PendingAction> {
        let p = self.book.take(user, id)?;
        self.audit.lock().unwrap().push(format!("appactions.answer {} {}", p.app, p.tool));
        Some(p)
    }
    fn audit(&self) -> Vec<String> {
        self.audit.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_checks_the_capability_cleans_notes_and_keeps_tool_requests_for_the_person() {
        let f = FakeAppActions::default();
        assert!(f.notify("pat", "a", "hello").unwrap_err().contains("notify"));
        f.grant("a", "notify");
        f.notify("pat", "a", "hello\nthere").unwrap();
        assert_eq!(f.notes(), [("pat".to_string(), "a".to_string(), "hellothere".to_string())]);
        assert!(f.start_flow("pat", "a", "Backup").is_err());
        f.grant("a", "flows.start");
        assert!(f.start_flow("pat", "a", "Backup").unwrap_err().contains("no published flow"));
        f.add_flow("Backup");
        f.start_flow("pat", "a", "Backup").unwrap();
        assert_eq!(f.started().len(), 1);
        f.grant("a", "tools.request");
        let id = f.request_tool("pat", "a", "wifi_status", "{}").unwrap();
        assert_eq!(f.pending("pat")[0].tool, "wifi_status");
        assert!(f.take("sam", &id).is_none());
        assert!(f.take("pat", &id).is_some());
        assert!(f.audit().iter().all(|l| !l.contains("{}")), "no arguments in the log");
    }
}
