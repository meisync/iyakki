//! Ports that carry no pixels: what an app keeps and how it asks for it. An SDK app never opens a file or a socket (the core rule): it is
//! handed a port, and the port is the core's. [`MemStorage`] is the in-memory one for tests and the browser preview; the core-backed one is
//! `iyakki-sdk-tile`'s `CoreStorage`, which asks the core's `storage` namespace (get, set, remove, list) through the tile ABI's `call`.

use std::collections::BTreeMap;

/// The most one app may keep, in bytes of keys and values together (the core's limit for `storage`).
pub const STORAGE_LIMIT: usize = 256 * 1024;
/// The longest key and the longest value a port takes.
pub const MAX_KEY: usize = 128;
pub const MAX_VALUE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortError {
    /// The key or value is not acceptable (empty or too long key, too long value).
    BadKey,
    /// The app would keep more than [`STORAGE_LIMIT`].
    TooBig,
    /// The core did not allow it or did not answer.
    Denied,
}

/// An app's own saved data: text under short keys, readable by that app only.
pub trait StoragePort {
    fn get(&mut self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError>;
    fn remove(&mut self, key: &str) -> Result<(), PortError>;
    fn keys(&mut self) -> Vec<String>;
}

pub fn check_key(key: &str) -> Result<(), PortError> {
    if key.is_empty() || key.len() > MAX_KEY || key.chars().any(char::is_control) {
        Err(PortError::BadKey)
    } else {
        Ok(())
    }
}

#[derive(Default)]
pub struct MemStorage {
    pub map: BTreeMap<String, String>,
}

impl StoragePort for MemStorage {
    fn get(&mut self, key: &str) -> Option<String> {
        self.map.get(key).cloned()
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError> {
        check_key(key)?;
        if value.len() > MAX_VALUE {
            return Err(PortError::BadKey);
        }
        let others: usize = self.map.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.len()).sum();
        if others + key.len() + value.len() > STORAGE_LIMIT {
            return Err(PortError::TooBig);
        }
        self.map.insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn remove(&mut self, key: &str) -> Result<(), PortError> {
        self.map.remove(key);
        Ok(())
    }
    fn keys(&mut self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }
}
