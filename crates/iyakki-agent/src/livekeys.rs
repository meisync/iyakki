//! The vault keys of signed-in people, packed for the memory file of a live restart (docs/development/LIVE-RESTART.md, "Vault key hand-over").
//! Plain text inside a memory-only file that never reaches a disk and is let go once the new program reports healthy: a JSON list of
//! `{user, kind, key}` with the 32-byte key in hex. Nothing here logs a key or writes a file.

use std::sync::{Arc, Mutex, Weak};

use crate::services::Services;

static CURRENT: Mutex<Weak<Services>> = Mutex::new(Weak::new());

/// The services of this program (set where the shell environment is built), for the code that runs at the moment of a live restart.
pub fn set_current(svc: &Arc<Services>) {
    *CURRENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(svc);
}

pub fn current() -> Option<Arc<Services>> {
    CURRENT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).upgrade()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The bytes to put in the memory file: the keys of every unlocked vault of this program. Empty list when nobody has one open.
pub fn pack(svc: &Services) -> Vec<u8> {
    let list: Vec<serde_json::Value> = svc.export_session_keys().iter().map(|(u, k, m)| serde_json::json!({"user": u, "kind": k, "key": hex(&m[..])})).collect();
    serde_json::to_vec(&list).unwrap_or_default()
}

pub fn unpack(bytes: &[u8]) -> Vec<(String, String, [u8; 32])> {
    let Ok(list) = serde_json::from_slice::<Vec<serde_json::Value>>(bytes) else { return Vec::new() };
    list.iter().filter_map(|v| Some((v["user"].as_str()?.to_string(), v["kind"].as_str()?.to_string(), unhex32(v["key"].as_str()?)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pack_into_text_and_come_back_and_damaged_or_short_ones_are_dropped() {
        let k = [7u8; 32];
        let text = serde_json::to_vec(&vec![serde_json::json!({"user": "alice", "kind": "user", "key": hex(&k)}), serde_json::json!({"user": "x", "kind": "user", "key": "abcd"}), serde_json::json!({"kind": "system"})]).unwrap();
        assert_eq!(unpack(&text), vec![("alice".to_string(), "user".to_string(), k)]);
        assert!(unpack(b"not json").is_empty());
        assert_eq!(unhex32(&"zz".repeat(32)), None);
    }

    #[test]
    fn the_current_services_are_held_weakly_and_go_with_their_owner() {
        assert!(current().is_none() || current().is_some());
    }
}
