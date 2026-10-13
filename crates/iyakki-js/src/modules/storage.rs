//! `localStorage` and `sessionStorage`, kept wherever the embedder says: memory for a session, the user's vault for a
//! site that is allowed to keep data.

use std::any::Any;
use std::collections::BTreeMap;

use crate::engine::WebModule;

pub trait KeyValueStore: Send {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), String>;
    fn remove(&mut self, key: &str);
    fn clear(&mut self);
    fn keys(&self) -> Vec<String>;
}

#[derive(Default, Clone)]
pub struct MemoryStore(pub BTreeMap<String, String>);

/// Per-site quota, as in browsers.
pub const QUOTA_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_KEYS: usize = 10_000;

impl KeyValueStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let used: usize = self.0.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.len()).sum();
        if used + key.len() + value.len() > QUOTA_BYTES || (!self.0.contains_key(key) && self.0.len() >= MAX_KEYS) {
            return Err("QuotaExceededError: the storage for this site is full".into());
        }
        self.0.insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn clear(&mut self) {
        self.0.clear();
    }
    fn keys(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Local,
    Session,
}

pub struct StorageModule {
    kind: Kind,
    pub store: Box<dyn KeyValueStore>,
}

impl StorageModule {
    pub fn new(kind: Kind, store: Box<dyn KeyValueStore>) -> StorageModule {
        StorageModule { kind, store }
    }
}

macro_rules! storage_prelude {
    ($name:literal) => {
        concat!(
            "(function (g) { 'use strict'; const M = ",
            $name,
            r#";
  const call = (op, a) => JSON.parse(__host(M, op, JSON.stringify(a || {})));
  const api = {
    getItem: (k) => call('get', { k: String(k) }),
    setItem: (k, v) => { call('set', { k: String(k), v: String(v) }); },
    removeItem: (k) => { call('remove', { k: String(k) }); },
    clear: () => { call('clear'); },
    key: (i) => call('key', { i: i | 0 }),
    get length() { return call('len'); },
  };
  g[M] = new Proxy(api, {
    get(t, p) { if (p in t) return t[p]; if (typeof p === 'symbol') return undefined; const v = call('get', { k: p }); return v === null ? undefined : v; },
    set(t, p, v) { if (typeof p === 'symbol') return false; call('set', { k: p, v: String(v) }); return true; },
    deleteProperty(t, p) { call('remove', { k: String(p) }); return true; },
    has(t, p) { return p in t || (typeof p === 'string' && call('get', { k: p }) !== null); },
    ownKeys() { return call('keys'); },
    getOwnPropertyDescriptor(t, p) { if (typeof p !== 'string') return undefined; const v = call('get', { k: p }); return v === null ? undefined : { value: v, enumerable: true, configurable: true, writable: true }; },
  });
})(globalThis);
"#
        )
    };
}

impl WebModule for StorageModule {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Local => "localStorage",
            Kind::Session => "sessionStorage",
        }
    }

    fn prelude(&self) -> &'static str {
        match self.kind {
            Kind::Local => storage_prelude!("'localStorage'"),
            Kind::Session => storage_prelude!("'sessionStorage'"),
        }
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        let key = args["k"].as_str().unwrap_or("");
        Ok(match op {
            "get" => self.store.get(key).map_or(serde_json::Value::Null, serde_json::Value::String),
            "set" => {
                self.store.set(key, args["v"].as_str().unwrap_or(""))?;
                serde_json::Value::Null
            }
            "remove" => {
                self.store.remove(key);
                serde_json::Value::Null
            }
            "clear" => {
                self.store.clear();
                serde_json::Value::Null
            }
            "key" => self.store.keys().get(args["i"].as_u64().unwrap_or(u64::MAX) as usize).cloned().map_or(serde_json::Value::Null, serde_json::Value::String),
            "len" => serde_json::json!(self.store.keys().len()),
            "keys" => serde_json::json!(self.store.keys()),
            other => return Err(format!("storage has no operation {other}")),
        })
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsEngine, Limits};

    fn engine(local: MemoryStore) -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(StorageModule::new(Kind::Local, Box::new(local))), Box::new(StorageModule::new(Kind::Session, Box::new(MemoryStore::default())))]).unwrap()
    }

    #[test]
    fn items_can_be_read_written_listed_and_removed_in_every_way_a_page_does() {
        let mut e = engine(MemoryStore::default());
        assert_eq!(e.eval("t", "localStorage.setItem('a', 1); localStorage.b = 'two'; localStorage['c d'] = {}; [localStorage.getItem('a'), localStorage.b, localStorage['c d'], localStorage.getItem('zz'), localStorage.zz, localStorage.length].join('|')").unwrap(), "1|two|[object Object]|||3");
        assert_eq!(e.eval("t", "Object.keys(localStorage).join(',')").unwrap(), "a,b,c d");
        assert_eq!(e.eval("t", "('b' in localStorage) + ',' + ('nope' in localStorage) + ',' + localStorage.key(1) + ',' + localStorage.key(9)").unwrap(), "true,false,b,null");
        assert_eq!(e.eval("t", "delete localStorage.b; localStorage.removeItem('a'); localStorage.length").unwrap(), "1");
        assert_eq!(e.eval("t", "localStorage.clear(); localStorage.length").unwrap(), "0");
        assert_eq!(e.eval("t", "localStorage.setItem('n', 5); typeof localStorage.getItem('n')").unwrap(), "string", "values are stored as text");
    }

    #[test]
    fn local_and_session_storage_are_separate_and_the_store_holds_the_data() {
        let mut e = engine(MemoryStore::default());
        e.eval("t", "localStorage.x = 'L'; sessionStorage.x = 'S'").unwrap();
        assert_eq!(e.eval("t", "localStorage.x + sessionStorage.x").unwrap(), "LS");
        let m = e.module::<StorageModule>("localStorage").unwrap();
        assert_eq!(m.store.get("x").as_deref(), Some("L"));
        // data that was already there is visible to the page
        let mut seeded = MemoryStore::default();
        seeded.0.insert("saved".into(), "from before".into());
        let mut e = engine(seeded);
        assert_eq!(e.eval("t", "localStorage.getItem('saved')").unwrap(), "from before");
    }

    #[test]
    fn the_quota_is_enforced_and_reported_to_the_page() {
        let mut e = engine(MemoryStore::default());
        let r = e.eval("t", "let n = 0; try { for (;;) { localStorage.setItem('k' + n, 'x'.repeat(100000)); n++; } } catch (err) { String(err.message).includes('QuotaExceeded') + ',' + (n > 40 && n < 60) }").unwrap();
        assert_eq!(r, "true,true");
    }
}
