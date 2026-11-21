//! One file for the compiled form of many small scripts (F-BROW-99, step 3 of docs/JS-PERFORMANCE.md).
//!
//! A page has dozens of scripts of a few kilobytes. Keeping each one in a file of its own costs a directory lookup, an ownership check and a
//! read per script, as much as parsing it; so the compiled forms of the small ones are kept together in one per-user pack file, read once
//! per process, and written again (merged with what another process stored meanwhile) when an engine that added something is dropped. Like the
//! single files, the pack is trusted only if we own it, nobody else can write it and its checksum matches (see [`crate::cache`]); anything
//! doubtful is an empty pack. Keys are SHA-256 digests of the engine build, the script's name and its text, so a changed script or engine is
//! simply another key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Scripts shorter than this are not worth a compiled form (they parse in well under a tenth of a millisecond).
pub const PACK_MIN: usize = 512;
/// The pack never takes more than this; when it would, the entries stored earliest go.
const PACK_MAX_BYTES: usize = 24 << 20;
/// One entry never takes more than this (a script near the single-file limit has its own file).
const ENTRY_MAX: usize = 256 << 10;

#[derive(Default)]
struct Pack {
    /// key -> compiled form, with the order they were stored in (older first)
    entries: HashMap<[u8; 32], (u64, Arc<Vec<u8>>)>,
    next: u64,
    bytes: usize,
    /// Keys added since the file was read.
    added: Vec<[u8; 32]>,
}

impl Pack {
    fn insert(&mut self, key: [u8; 32], code: Arc<Vec<u8>>) {
        self.bytes += code.len();
        if let Some((_, old)) = self.entries.insert(key, (self.next, code)) {
            self.bytes -= old.len();
        }
        self.next += 1;
    }

    fn encode(&self) -> Vec<u8> {
        let mut order: Vec<(&[u8; 32], &(u64, Arc<Vec<u8>>))> = self.entries.iter().collect();
        order.sort_by_key(|(_, (n, _))| *n);
        let mut out = Vec::with_capacity(self.bytes + order.len() * 40 + 8);
        out.extend_from_slice(b"IYPK");
        out.extend_from_slice(&(order.len() as u32).to_le_bytes());
        for (k, (_, code)) in order {
            out.extend_from_slice(k);
            out.extend_from_slice(&(code.len() as u32).to_le_bytes());
            out.extend_from_slice(code);
        }
        out
    }

    /// Entries from a stored pack; a malformed one gives what could be read before the damage (the checksum already caught torn files).
    fn decode(&mut self, data: &[u8]) {
        if data.len() < 8 || &data[..4] != b"IYPK" {
            return;
        }
        let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let mut at = 8usize;
        for _ in 0..count {
            let Some(head) = data.get(at..at + 36) else { return };
            let mut key = [0u8; 32];
            key.copy_from_slice(&head[..32]);
            let len = u32::from_le_bytes([head[32], head[33], head[34], head[35]]) as usize;
            at += 36;
            let Some(code) = data.get(at..at.saturating_add(len)) else { return };
            at += len;
            if len > 0 && len <= ENTRY_MAX && !self.entries.contains_key(&key) {
                self.insert(key, Arc::new(code.to_vec()));
            }
        }
    }

    fn trim(&mut self) {
        if self.bytes <= PACK_MAX_BYTES {
            return;
        }
        let mut order: Vec<([u8; 32], u64, usize)> = self.entries.iter().map(|(k, (n, c))| (*k, *n, c.len())).collect();
        order.sort_by_key(|e| e.1);
        for (k, _, len) in order {
            if self.bytes <= PACK_MAX_BYTES / 4 * 3 {
                break;
            }
            self.entries.remove(&k);
            self.bytes -= len;
        }
    }
}

type Packs = Mutex<HashMap<PathBuf, Pack>>;

fn packs() -> &'static Packs {
    static P: std::sync::OnceLock<Packs> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_of(dir: &Path, digest: &str) -> PathBuf {
    dir.join(format!("smallpack-{digest}.pack"))
}

fn load(file: &Path) -> Pack {
    let mut p = Pack::default();
    if let Some(data) = crate::cache::read(file) {
        p.decode(&data);
    }
    p
}

/// The compiled form stored under `key`, if the pack of this engine build in `dir` has one.
pub fn get(dir: &Path, digest: &str, key: &[u8; 32]) -> Option<Arc<Vec<u8>>> {
    let file = file_of(dir, digest);
    let mut all = packs().lock().ok()?;
    let pack = all.entry(file.clone()).or_insert_with(|| load(&file));
    pack.entries.get(key).map(|(_, c)| c.clone())
}

/// Remember a compiled form; it reaches the disk with the next [`flush`].
pub fn put(dir: &Path, digest: &str, key: [u8; 32], code: &[u8]) {
    if code.is_empty() || code.len() > ENTRY_MAX {
        return;
    }
    let file = file_of(dir, digest);
    let Ok(mut all) = packs().lock() else { return };
    let pack = all.entry(file.clone()).or_insert_with(|| load(&file));
    if !pack.entries.contains_key(&key) {
        pack.insert(key, Arc::new(code.to_vec()));
        pack.added.push(key);
    }
}

/// Write the packs that gained entries: merged with what another process stored since this one read it, then renamed into place.
pub fn flush() {
    let Ok(mut all) = packs().lock() else { return };
    for (file, pack) in all.iter_mut() {
        if pack.added.is_empty() {
            continue;
        }
        let mut merged = load(file);
        // the entries of this process come after those another process stored
        for key in std::mem::take(&mut pack.added) {
            if let Some((_, code)) = pack.entries.get(&key) {
                merged.insert(key, code.clone());
            }
        }
        merged.trim();
        crate::cache::write(file, &merged.encode());
        // what the other process stored is ours to serve now too
        for (k, (_, c)) in &merged.entries {
            if !pack.entries.contains_key(k) {
                pack.insert(*k, c.clone());
            }
        }
        pack.entries.retain(|k, _| merged.entries.contains_key(k));
        pack.bytes = pack.entries.values().map(|(_, c)| c.len()).sum();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn entries_survive_a_flush_and_a_new_process_reads_them() {
        let dir = crate::cache::use_private_dir_for_this_test();
        put(&dir, "d1", key(1), b"first compiled form");
        put(&dir, "d1", key(2), b"second");
        assert_eq!(get(&dir, "d1", &key(1)).as_deref().map(|v| v.as_slice()), Some(&b"first compiled form"[..]));
        flush();
        // a new process: forget the memory, read the file
        packs().lock().unwrap().remove(&file_of(&dir, "d1"));
        assert_eq!(get(&dir, "d1", &key(2)).as_deref().map(|v| v.as_slice()), Some(&b"second"[..]));
        assert!(get(&dir, "d1", &key(3)).is_none());
        // another engine build has its own pack
        assert!(get(&dir, "d2", &key(1)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_processes_that_add_different_entries_keep_both() {
        let dir = crate::cache::use_private_dir_for_this_test();
        put(&dir, "d", key(1), b"mine");
        // another process stored an entry meanwhile
        let mut other = Pack::default();
        other.insert(key(9), Arc::new(b"theirs".to_vec()));
        crate::cache::write(&file_of(&dir, "d"), &other.encode());
        flush();
        packs().lock().unwrap().remove(&file_of(&dir, "d"));
        assert!(get(&dir, "d", &key(1)).is_some() && get(&dir, "d", &key(9)).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damage_is_a_miss_and_never_a_panic() {
        let dir = crate::cache::use_private_dir_for_this_test();
        put(&dir, "d", key(1), b"payload");
        flush();
        let file = file_of(&dir, "d");
        let mut raw = std::fs::read(&file).unwrap();
        // every truncation and every flipped byte is an empty or shorter pack, never a crash
        for cut in 0..raw.len() {
            packs().lock().unwrap().remove(&file);
            std::fs::write(&file, &raw[..cut]).unwrap();
            let _ = get(&dir, "d", &key(1));
        }
        for i in 0..raw.len() {
            raw[i] ^= 0xff;
            packs().lock().unwrap().remove(&file);
            std::fs::write(&file, &raw).unwrap();
            let _ = get(&dir, "d", &key(1));
            raw[i] ^= 0xff;
        }
        // lying lengths inside a well-checksummed file
        let mut p = Vec::from(&b"IYPK"[..]);
        p.extend_from_slice(&u32::MAX.to_le_bytes());
        p.extend_from_slice(&[7; 32]);
        p.extend_from_slice(&u32::MAX.to_le_bytes());
        crate::cache::write(&file, &p);
        packs().lock().unwrap().remove(&file);
        assert!(get(&dir, "d", &key(1)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_entries_go_when_the_pack_is_over_its_size() {
        let mut p = Pack::default();
        for n in 0..8u8 {
            p.insert(key(n), Arc::new(vec![n; PACK_MAX_BYTES / 6]));
        }
        p.trim();
        assert!(p.bytes <= PACK_MAX_BYTES && !p.entries.contains_key(&key(0)) && p.entries.contains_key(&key(7)));
    }
}
