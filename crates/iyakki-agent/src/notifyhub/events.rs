//! The live event stream's hub: a bounded queue of already-serialised events per connection, filled by the hub and drained by the
//! WebSocket loop (`webservice/events.rs`). A slow reader never slows the hub: when its queue is full the OLDEST event is dropped and
//! counted. Connections are capped per person and per tenant. Events carry only what the person's own bell shows.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub const QUEUE_CAP: usize = 100;
pub const MAX_PER_PERSON: usize = 5;
pub const MAX_PER_TENANT: usize = 200;

struct Sub {
    user: String,
    q: VecDeque<String>,
    dropped: u64,
}

#[derive(Default)]
struct Inner {
    subs: BTreeMap<u64, Sub>,
    next: u64,
}

#[derive(Default)]
pub struct Bus {
    inner: Mutex<Inner>,
    cv: Condvar,
}

/// One event as the stream carries it: `{"type":..,"data":..}`.
pub fn event(kind: &str, data: serde_json::Value) -> String {
    serde_json::json!({"type": kind, "data": data}).to_string()
}

impl Bus {
    pub fn new() -> Arc<Bus> {
        Arc::new(Bus::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A connection for `user`, or why not.
    pub fn subscribe(&self, user: &str) -> Result<u64, &'static str> {
        let mut g = self.lock();
        if g.subs.values().filter(|s| s.user == user).count() >= MAX_PER_PERSON {
            return Err("too many open event connections for this person");
        }
        if g.subs.len() >= MAX_PER_TENANT {
            return Err("too many open event connections for this tenant");
        }
        g.next += 1;
        let id = g.next;
        g.subs.insert(id, Sub { user: user.into(), q: VecDeque::new(), dropped: 0 });
        Ok(id)
    }

    pub fn unsubscribe(&self, id: u64) {
        self.lock().subs.remove(&id);
    }

    /// Queue an event for every connection of `user` (and only them).
    pub fn publish(&self, user: &str, ev: &str) -> usize {
        let mut g = self.lock();
        let mut n = 0;
        for s in g.subs.values_mut().filter(|s| s.user == user) {
            if s.q.len() >= QUEUE_CAP {
                s.q.pop_front();
                s.dropped += 1;
            }
            s.q.push_back(ev.to_string());
            n += 1;
        }
        drop(g);
        self.cv.notify_all();
        n
    }

    /// Everything queued for the connection, waiting up to `wait` for the first. Empty on timeout; `None` when it is gone.
    pub fn drain(&self, id: u64, wait: Duration) -> Option<Vec<String>> {
        let mut g = self.lock();
        if !g.subs.contains_key(&id) {
            return None;
        }
        if g.subs.get(&id).is_some_and(|s| s.q.is_empty()) && !wait.is_zero() {
            g = self.cv.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner()).0;
        }
        g.subs.get_mut(&id).map(|s| s.q.drain(..).collect())
    }

    pub fn dropped(&self, id: u64) -> u64 {
        self.lock().subs.get(&id).map_or(0, |s| s.dropped)
    }

    pub fn open(&self) -> usize {
        self.lock().subs.len()
    }
}

/// The bus of a tenant's volume (one per data directory, shared by the hub and the sockets).
pub fn bus_for(dir: &std::path::Path) -> Arc<Bus> {
    static R: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Arc<Bus>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new())).lock().unwrap_or_else(|e| e.into_inner()).entry(dir.to_path_buf()).or_insert_with(Bus::new).clone()
}
