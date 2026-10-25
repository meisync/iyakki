//! Filter lists the person has chosen: which lists exist, where each came from, its licence, a SHA-256 of what was downloaded,
//! when, and whether it is switched on. The lists are third-party data with their own licences and are **never shipped** with
//! the system: nothing here fetches anything until the person clicks a download (or a weekly refresh of lists they already
//! added runs). Downloading goes through a [`ListSource`] so tests can use a fake server.
//!
//! Files live in one folder: `<id>.txt` per list and `lists.meta` (tab separated, one line per list).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::blocker::{self, Blocker, Compiler};
use crate::fetch::Fetch;
use crate::url::Url;

/// Largest list accepted (bytes).
pub const MAX_LIST_BYTES: usize = 24 << 20;
/// Most lists kept.
pub const MAX_LISTS: usize = 12;
/// Lists older than this are refreshed by the weekly check (seconds).
pub const REFRESH_AFTER: u64 = 7 * 24 * 3600;

/// A list the program knows how to find (a name, an address, its licence as the authors publish it).
#[derive(Clone, Copy, Debug)]
pub struct Known {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub licence: &'static str,
    pub what: &'static str,
}

pub const KNOWN: &[Known] = &[
    Known { id: "easylist", name: "EasyList", url: "https://easylist.to/easylist/easylist.txt", licence: "GPL-3.0 or CC BY-SA 3.0 (dual licence)", what: "advertisements on English-language sites" },
    Known { id: "easyprivacy", name: "EasyPrivacy", url: "https://easylist.to/easylist/easyprivacy.txt", licence: "GPL-3.0 or CC BY-SA 3.0 (dual licence)", what: "trackers and analytics" },
    Known { id: "ublock-filters", name: "uBlock filters", url: "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt", licence: "GPL-3.0", what: "ads, trackers and annoyances; some rules need uBlock's own engine and are skipped here" },
];

/// What is known about one list on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListMeta {
    pub id: String,
    pub name: String,
    /// Where it was downloaded from (empty for a file the person picked).
    pub url: String,
    pub licence: String,
    /// SHA-256 of the text, lower-case hex.
    pub hash: String,
    /// Seconds since 1970 when it was last fetched or added.
    pub updated: u64,
    pub bytes: u64,
    /// Network and hiding rules the program could use.
    pub rules: u64,
    pub enabled: bool,
}

/// Where list text comes from.
pub trait ListSource: Send + Sync {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String>;
}

/// A [`ListSource`] over the browser's own network (HTTPS, redirects followed).
pub struct FetchSource(pub Arc<dyn Fetch>);

impl ListSource for FetchSource {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        let u = Url::parse(url).map_err(|_| "not a web address".to_string())?;
        let (_, r) = crate::fetch::fetch_following(self.0.as_ref(), crate::fetch::Request::get(&u), &|_| true, None)?;
        if r.status != 200 {
            return Err(format!("the server answered {}", r.status));
        }
        if r.body.len() > max_bytes {
            return Err("the list is larger than the limit".into());
        }
        Ok(r.body)
    }
}

pub struct ListStore {
    dir: PathBuf,
}

fn clean_field(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

impl ListStore {
    pub fn open(dir: impl Into<PathBuf>) -> ListStore {
        ListStore { dir: dir.into() }
    }

    fn meta_path(&self) -> PathBuf {
        self.dir.join("lists.meta")
    }

    fn text_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.txt"))
    }

    /// The lists, in the order they were added. A damaged meta file gives an empty list, never an error.
    pub fn list(&self) -> Vec<ListMeta> {
        let Ok(text) = std::fs::read_to_string(self.meta_path()) else { return Vec::new() };
        text.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                if f.len() != 9 || !safe_id(f[0]) {
                    return None;
                }
                Some(ListMeta { id: f[0].into(), name: f[1].into(), url: f[2].into(), licence: f[3].into(), hash: f[4].into(), updated: f[5].parse().ok()?, bytes: f[6].parse().ok()?, rules: f[7].parse().ok()?, enabled: f[8] == "1" })
            })
            .take(MAX_LISTS)
            .collect()
    }

    fn save(&self, metas: &[ListMeta]) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create the folder: {e}"))?;
        let mut s = String::new();
        for m in metas {
            s.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", m.id, clean_field(&m.name, 80).replace('\t', " "), clean_field(&m.url, 500).replace('\t', " "), clean_field(&m.licence, 120).replace('\t', " "), m.hash, m.updated, m.bytes, m.rules, u8::from(m.enabled)));
        }
        let tmp = self.dir.join("lists.meta.tmp");
        std::fs::write(&tmp, s).map_err(|e| format!("cannot save: {e}"))?;
        std::fs::rename(&tmp, self.meta_path()).map_err(|e| format!("cannot save: {e}"))
    }

    /// Store `text` as list `id` (replacing the list of that name). Refuses what is not a list: too large, no rule the program
    /// could read in the first lines. Returns what was stored.
    pub fn put(&self, id: &str, name: &str, url: &str, licence: &str, text: &[u8], now: u64) -> Result<ListMeta, String> {
        if !safe_id(id) {
            return Err("bad list name".into());
        }
        if text.len() > MAX_LIST_BYTES {
            return Err("the list is larger than 24 MB".into());
        }
        let s = String::from_utf8_lossy(text);
        let mut c = Compiler::with_limit(blocker::MAX_RULES);
        c.add_list(&s, 0);
        let rules = {
            let b = c.finish();
            let st = b.stats();
            (st.blocking + st.exceptions + st.cosmetic) as u64
        };
        if rules == 0 {
            return Err("this does not look like a filter list: no rule could be read".into());
        }
        let mut metas = self.list();
        let existing = metas.iter().position(|m| m.id == id);
        if existing.is_none() && metas.len() >= MAX_LISTS {
            return Err(format!("at most {MAX_LISTS} lists"));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create the folder: {e}"))?;
        let tmp = self.dir.join(format!("{id}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("cannot save: {e}"))?;
        std::fs::rename(&tmp, self.text_path(id)).map_err(|e| format!("cannot save: {e}"))?;
        let meta = ListMeta { id: id.into(), name: clean_field(name, 80), url: clean_field(url, 500), licence: clean_field(licence, 120), hash: sha256_hex(text), updated: now, bytes: text.len() as u64, rules, enabled: existing.map(|i| metas[i].enabled).unwrap_or(true) };
        match existing {
            Some(i) => metas[i] = meta.clone(),
            None => metas.push(meta.clone()),
        }
        self.save(&metas)?;
        Ok(meta)
    }

    /// Download a list the program knows (the person pressed the button for it).
    pub fn add_known(&self, k: &Known, src: &dyn ListSource, now: u64) -> Result<ListMeta, String> {
        let text = src.get(k.url, MAX_LIST_BYTES)?;
        self.put(k.id, k.name, k.url, k.licence, &text, now)
    }

    /// Download a list from an address the person typed. The licence is unknown to the program and says so.
    pub fn add_url(&self, url: &str, src: &dyn ListSource, now: u64) -> Result<ListMeta, String> {
        let u = Url::parse(url).map_err(|_| "that is not a web address".to_string())?;
        let text = src.get(url, MAX_LIST_BYTES)?;
        let n = self.list().iter().filter(|m| m.id.starts_with("custom-")).count() + 1;
        let existing = self.list().into_iter().find(|m| m.url == url).map(|m| m.id);
        let id = existing.unwrap_or_else(|| format!("custom-{n}-{:08x}", crate::blocker_lists::short_hash(url)));
        self.put(&id, &u.host, url, "unknown: see the list's own site", &text, now)
    }

    /// Add a file the person picked.
    pub fn add_file(&self, path: &Path, now: u64) -> Result<ListMeta, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("cannot read the file: {e}"))?;
        if meta.len() as usize > MAX_LIST_BYTES {
            return Err("the file is larger than 24 MB".into());
        }
        let text = std::fs::read(path).map_err(|e| format!("cannot read the file: {e}"))?;
        let name: String = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "list".into());
        let id = format!("file-{:08x}", short_hash(&name));
        self.put(&id, &name, "", "unknown: a file you chose", &text, now)
    }

    /// Download `id` again. A list with an address only: files the person picked cannot be refreshed.
    pub fn refresh(&self, id: &str, src: &dyn ListSource, now: u64) -> Result<ListMeta, String> {
        let m = self.list().into_iter().find(|m| m.id == id).ok_or("no such list")?;
        if m.url.is_empty() {
            return Err("this list came from a file: add the file again to update it".into());
        }
        let text = src.get(&m.url, MAX_LIST_BYTES)?;
        self.put(&m.id, &m.name, &m.url, &m.licence, &text, now)
    }

    /// The weekly check: refresh every list with an address that has not been fetched for a week. One failure does not stop the
    /// others and never removes the old text.
    pub fn refresh_due(&self, src: &dyn ListSource, now: u64) -> Vec<(String, Result<ListMeta, String>)> {
        self.list().into_iter().filter(|m| !m.url.is_empty() && m.enabled && now.saturating_sub(m.updated) >= REFRESH_AFTER).map(|m| (m.id.clone(), self.refresh(&m.id, src, now))).collect()
    }

    pub fn set_enabled(&self, id: &str, on: bool) -> bool {
        let mut metas = self.list();
        let Some(m) = metas.iter_mut().find(|m| m.id == id) else { return false };
        m.enabled = on;
        self.save(&metas).is_ok()
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut metas = self.list();
        let n = metas.len();
        metas.retain(|m| m.id != id);
        if metas.len() == n || !safe_id(id) {
            return false;
        }
        let _ = std::fs::remove_file(self.text_path(id));
        self.save(&metas).is_ok()
    }

    /// Compile the enabled lists. A list whose file is missing or unreadable is left out; none at all gives an empty blocker
    /// (nothing is blocked: the filter fails open).
    pub fn compile(&self) -> Blocker {
        let mut c = Compiler::new();
        for (i, m) in self.list().iter().enumerate() {
            if !m.enabled {
                continue;
            }
            if let Ok(bytes) = std::fs::read(self.text_path(&m.id)) {
                c.add_list(&String::from_utf8_lossy(&bytes), i.min(255) as u8);
            }
        }
        c.finish()
    }
}

/// A small stable hash for naming (not security).
pub fn short_hash(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// SHA-256 of `data` as lower-case hex.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, c) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
