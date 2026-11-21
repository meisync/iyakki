//! The made-up sound facts (`iyakki_core_api::AudioRoute`): a tree of `/proc/asound` files in memory, the holders of the devices, the
//! mixer's answers and the product name, and a record of what was asked.

use iyakki_core_api::audioroute::rel_is_safe;
use iyakki_core_api::{AudioRoute, PcmHolder};
use std::collections::BTreeMap;
use std::sync::Mutex;

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
pub struct FakeAudioRoute {
    files: Mutex<BTreeMap<String, String>>,
    holders: Mutex<BTreeMap<(u32, u32), Vec<PcmHolder>>>,
    mixer: Mutex<BTreeMap<u32, String>>,
    product: Mutex<Option<String>>,
    procs: Mutex<BTreeMap<u32, (u32, String)>>,
    calls: Mutex<Vec<String>>,
}

impl FakeAudioRoute {
    /// Put the file `/proc/asound/<rel>` in the tree.
    pub fn put(&self, rel: &str, text: &str) {
        lk(&self.files).insert(rel.to_string(), text.to_string());
    }
    pub fn set_holders(&self, card: u32, device: u32, who: Vec<PcmHolder>) {
        lk(&self.holders).insert((card, device), who);
    }
    pub fn set_mixer(&self, card: u32, contents: &str) {
        lk(&self.mixer).insert(card, contents.to_string());
    }
    /// A process of the made-up machine: its parent and its name.
    pub fn set_process(&self, pid: u32, parent: u32, name: &str) {
        lk(&self.procs).insert(pid, (parent, name.to_string()));
    }
    pub fn set_product(&self, name: Option<&str>) {
        *lk(&self.product) = name.map(str::to_string);
    }
    /// Every question asked so far, in order (`text:cards`, `dir:card0`, `holders:0:0`, `mixer:0`, `product`).
    pub fn calls(&self) -> Vec<String> {
        lk(&self.calls).clone()
    }
    fn note(&self, c: String) {
        lk(&self.calls).push(c);
    }
}

impl AudioRoute for FakeAudioRoute {
    fn process_parent(&self, pid: u32) -> Option<u32> {
        self.note(format!("parent:{pid}"));
        lk(&self.procs).get(&pid).map(|p| p.0)
    }
    fn process_name(&self, pid: u32) -> Option<String> {
        self.note(format!("name:{pid}"));
        lk(&self.procs).get(&pid).map(|p| p.1.clone())
    }
    fn asound_text(&self, rel: &str) -> Option<String> {
        self.note(format!("text:{rel}"));
        if !rel_is_safe(rel) {
            return None;
        }
        lk(&self.files).get(rel).cloned()
    }
    fn asound_dir(&self, rel: &str) -> Vec<String> {
        self.note(format!("dir:{rel}"));
        if !rel_is_safe(rel) {
            return Vec::new();
        }
        let prefix = if rel.is_empty() { String::new() } else { format!("{}/", rel.trim_end_matches('/')) };
        let mut names: Vec<String> = lk(&self.files).keys().filter_map(|k| k.strip_prefix(&prefix)).filter_map(|r| r.split('/').next()).filter(|n| !n.is_empty()).map(str::to_string).collect();
        names.sort();
        names.dedup();
        names
    }
    fn asound_has(&self, rel: &str) -> bool {
        self.note(format!("has:{rel}"));
        if !rel_is_safe(rel) {
            return false;
        }
        let dir = format!("{}/", rel.trim_end_matches('/'));
        lk(&self.files).keys().any(|k| k == rel || k.starts_with(&dir))
    }
    fn product_name(&self) -> Option<String> {
        self.note("product".into());
        lk(&self.product).clone()
    }
    fn pcm_holders(&self, card: u32, device: u32) -> Vec<PcmHolder> {
        self.note(format!("holders:{card}:{device}"));
        lk(&self.holders).get(&(card, device)).cloned().unwrap_or_default()
    }
    fn mixer_contents(&self, card: u32) -> Option<String> {
        self.note(format!("mixer:{card}"));
        lk(&self.mixer).get(&card).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_lists_its_directories_and_refuses_paths_that_leave_it() {
        let f = FakeAudioRoute::default();
        f.put("cards", " 0 [PCH]: HDA-Intel - HDA Intel PCH\n");
        f.put("card0/pcm0p/info", "id: x\n");
        f.put("card0/eld#0.0", "monitor_present 1\n");
        assert_eq!(f.asound_dir(""), vec!["card0", "cards"]);
        assert_eq!(f.asound_dir("card0"), vec!["eld#0.0", "pcm0p"]);
        assert!(f.asound_has("card0/pcm0p") && !f.asound_has("card1"));
        assert_eq!(f.card_count(), 1);
        assert!(f.asound_text("../cmdline").is_none() && f.asound_dir("../x").is_empty());
    }
}
