//! The fake app events: the real engine (`iyakki_core_api::appevents::SharedHub`) with a clock the test moves, a store in memory and the
//! scopes and folders the test grants. No sleeping, nothing on disk. `restart()` builds a new engine over the same store, which is how a
//! test proves the registrations and the queue survive a restart.

use iyakki_core_api::appevents::{AppEvent, AppEvents, EventInfo, MemStore, Payload, RegInfo, ScopeCheck, SharedHub};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Default)]
struct Grants {
    scopes: BTreeSet<(String, String)>,
    folders: Vec<String>,
}

struct FakeScopes(Arc<RwLock<Grants>>);
impl ScopeCheck for FakeScopes {
    fn holds(&self, _user: &str, app: &str, scope: &str) -> bool {
        self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner).scopes.contains(&(app.to_string(), scope.to_string()))
    }
    fn held(&self, _user: &str, app: &str) -> Vec<String> {
        self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner).scopes.iter().filter(|(a, _)| a == app).map(|(_, s)| s.clone()).collect()
    }
    fn folder_allowed(&self, _user: &str, _app: &str, folder: &str) -> bool {
        self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner).folders.iter().any(|f| folder == f || folder.starts_with(&format!("{f}/")))
    }
}

pub struct FakeAppEvents {
    hub: Mutex<Arc<SharedHub>>,
    store: MemStore,
    grants: Arc<RwLock<Grants>>,
    clock: Arc<AtomicU64>,
}

impl Default for FakeAppEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeAppEvents {
    pub fn new() -> FakeAppEvents {
        let store = MemStore::default();
        let grants = Arc::new(RwLock::new(Grants::default()));
        let clock = Arc::new(AtomicU64::new(1_000));
        let hub = Self::build(&store, &grants, &clock);
        FakeAppEvents { hub: Mutex::new(hub), store, grants, clock }
    }
    fn build(store: &MemStore, grants: &Arc<RwLock<Grants>>, clock: &Arc<AtomicU64>) -> Arc<SharedHub> {
        let c = clock.clone();
        Arc::new(SharedHub::new(Box::new(store.clone()), Box::new(FakeScopes(grants.clone())), Box::new(move || c.load(Ordering::SeqCst))))
    }
    fn h(&self) -> Arc<SharedHub> {
        self.hub.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    /// The app holds this scope (a capability of the catalogue).
    pub fn grant(&self, app: &str, scope: &str) {
        self.grants.write().unwrap_or_else(std::sync::PoisonError::into_inner).scopes.insert((app.into(), scope.into()));
    }
    pub fn revoke(&self, app: &str, scope: &str) {
        self.grants.write().unwrap_or_else(std::sync::PoisonError::into_inner).scopes.remove(&(app.to_string(), scope.to_string()));
    }
    /// A folder the file policy lets every app read.
    pub fn allow_folder(&self, folder: &str) {
        self.grants.write().unwrap_or_else(std::sync::PoisonError::into_inner).folders.push(folder.into());
    }
    pub fn set_time(&self, ms: u64) {
        self.clock.store(ms, Ordering::SeqCst);
    }
    pub fn advance(&self, ms: u64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }
    /// The machine restarts: a new engine over the same store (every app is "not running").
    pub fn restart(&self) {
        let hub = Self::build(&self.store, &self.grants, &self.clock);
        *self.hub.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = hub;
    }
}

impl AppEvents for FakeAppEvents {
    fn catalogue(&self) -> Vec<EventInfo> {
        self.h().catalogue()
    }
    fn list(&self, u: &str, a: &str) -> Vec<RegInfo> {
        self.h().list(u, a)
    }
    fn apps(&self, u: &str) -> Vec<(String, Vec<RegInfo>)> {
        self.h().apps(u)
    }
    fn register(&self, u: &str, a: &str, e: &str, f: &str) -> Result<(), String> {
        self.h().register(u, a, e, f)
    }
    fn unregister(&self, u: &str, a: &str, e: &str, f: &str) -> Result<(), String> {
        self.h().unregister(u, a, e, f)
    }
    fn set_enabled(&self, u: &str, a: &str, e: &str, f: &str, on: bool) -> Result<(), String> {
        self.h().set_enabled(u, a, e, f, on)
    }
    fn start(&self, u: &str, a: &str) -> Vec<AppEvent> {
        self.h().start(u, a)
    }
    fn stop(&self, u: &str, a: &str) {
        self.h().stop(u, a)
    }
    fn poll(&self, u: &str, a: &str, max: usize) -> Vec<AppEvent> {
        self.h().poll(u, a, max)
    }
    fn replay(&self, u: &str, a: &str, after: u64) -> Vec<AppEvent> {
        self.h().replay(u, a, after)
    }
    fn permissions_changed(&self, u: &str, a: &str, g: &[String], r: &[String], by: &str, restart: bool) -> Result<usize, String> {
        self.h().permissions_changed(u, a, g, r, by, restart)
    }
    fn set_permission(&self, u: &str, a: &str, sc: &str, allow: bool, by: &str) -> Result<usize, String> {
        self.h().set_permission(u, a, sc, allow, by)
    }
    fn permissions(&self, u: &str, a: &str) -> (Vec<String>, Option<String>) {
        self.h().permissions(u, a)
    }
    fn known_apps(&self, u: &str) -> Vec<String> {
        self.h().known_apps(u)
    }
    fn requeue(&self, u: &str, a: &str, after: u64) -> Result<usize, String> {
        self.h().requeue(u, a, after)
    }
    fn publish(&self, u: &str, a: &str, n: &str, p: Payload) -> Result<usize, String> {
        self.h().publish(u, a, n, p)
    }
    fn emit(&self, n: &str, u: Option<&str>, p: Payload) -> Result<usize, String> {
        self.h().emit(n, u, p)
    }
    fn wanted(&self, n: &str) -> bool {
        self.h().wanted(n)
    }
    fn listeners(&self, e: &str) -> Vec<(String, String, String)> {
        self.h().listeners(e)
    }
    fn set_flag(&self, f: &str, on: bool) {
        self.h().set_flag(f, on)
    }
    fn audit(&self) -> Vec<String> {
        self.h().audit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::appevents::{payload, Val};

    fn net(s: &str) -> Payload {
        payload(&[("state", Val::Text(s.into()))])
    }

    #[test]
    fn the_fake_follows_the_rules_and_survives_a_restart() {
        let f = FakeAppEvents::new();
        assert!(f.register("p", "a", "connectivity.changed", "").is_err(), "no scope yet");
        f.grant("a", "events.network");
        f.register("p", "a", "connectivity.changed", "").unwrap();
        f.start("p", "a");
        f.emit("connectivity.changed", None, net("online")).unwrap();
        assert_eq!(f.poll("p", "a", 5)[0].seq, 1);
        f.stop("p", "a");
        f.advance(500);
        f.emit("connectivity.changed", None, net("offline")).unwrap();
        f.restart();
        let got = f.start("p", "a");
        assert_eq!((got.len(), got[0].seq, got[0].late, got[0].at_ms), (1, 2, true, 1_500));
    }

    #[test]
    fn the_fake_checks_the_file_policy_and_the_publish_scope() {
        let f = FakeAppEvents::new();
        f.grant("a", "events.files");
        assert!(f.register("p", "a", "files.changed", "Docs").is_err());
        f.allow_folder("Docs");
        f.register("p", "a", "files.changed", "Docs").unwrap();
        assert!(f.publish("p", "a", "custom.a.x", Payload::new()).is_err());
        f.grant("a", "events.publish");
        assert_eq!(f.publish("p", "a", "custom.a.x", Payload::new()).unwrap(), 0);
    }
}
