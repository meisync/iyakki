//! The kept kernel logs on the fake machine: a note, files made of lines, and a record of what was marked or copied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_core_api::crashlog::{CrashLog, Unclean};

#[derive(Default)]
pub struct State {
    pub unclean: Option<Unclean>,
    /// File name -> its lines.
    pub files: BTreeMap<String, Vec<String>>,
    pub latest: Option<String>,
    pub current: Option<String>,
    pub calls: Vec<String>,
}

#[derive(Default)]
pub struct FakeCrashLog {
    state: Mutex<State>,
}

impl FakeCrashLog {
    pub fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl CrashLog for FakeCrashLog {
    fn read_unclean(&self, _: &Path) -> Option<Unclean> {
        self.state.lock().unwrap().unclean.clone()
    }
    fn latest_crash(&self, _: &Path) -> Option<String> {
        self.state.lock().unwrap().latest.clone()
    }
    fn current_file(&self, _: &Path) -> Option<String> {
        self.state.lock().unwrap().current.clone()
    }
    fn has_file(&self, _: &Path, name: &str) -> bool {
        self.state.lock().unwrap().files.contains_key(name)
    }
    fn last_lines(&self, _: &Path, name: &str, n: usize) -> Vec<String> {
        let s = self.state.lock().unwrap();
        let l = s.files.get(name).cloned().unwrap_or_default();
        l[l.len().saturating_sub(n)..].to_vec()
    }
    fn mark_seen(&self, _: &Path) {
        let mut s = self.state.lock().unwrap();
        s.calls.push("mark_seen".into());
        if let Some(u) = s.unclean.as_mut() {
            u.seen = true;
        }
    }
    fn copy_to_folder(&self, _: &Path, name: &str, dest_dir: &Path, _: i64) -> Result<PathBuf, String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("copy_to_folder({name})"));
        if !s.files.contains_key(name) {
            return Err(format!("there is no kept log {name}"));
        }
        Ok(dest_dir.join(format!("kernel-log-{name}")))
    }
}
