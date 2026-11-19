//! The fake [`Memory`]: figures and `system.conf` text in memory, every change recorded, nothing touches the machine.

use iyakki_core_api::memory::{config::Config, DataPlace, Memory, MemStats, Step};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeMemory {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    stats: MemStats,
    conf: String,
    free: Option<u64>,
    place: Option<DataPlace>,
    calls: Vec<String>,
    /// The answer of `reconfigure` (default: one successful step).
    refuse: Option<String>,
}

impl FakeMemory {
    pub fn new() -> FakeMemory {
        FakeMemory::default()
    }
    pub fn set_stats(&self, s: MemStats) {
        self.state.lock().unwrap().stats = s;
    }
    pub fn set_conf(&self, text: &str) {
        self.state.lock().unwrap().conf = text.into();
    }
    pub fn conf(&self) -> String {
        self.state.lock().unwrap().conf.clone()
    }
    pub fn set_data_place(&self, place: DataPlace) {
        self.state.lock().unwrap().place = Some(place);
    }
    pub fn set_data_free(&self, free: Option<u64>) {
        self.state.lock().unwrap().free = free;
    }
    /// Make `reconfigure` fail with this sentence.
    pub fn refuse_reconfigure(&self, why: &str) {
        self.state.lock().unwrap().refuse = Some(why.into());
    }
    /// Every call so far, as `method` or `method(argument)`.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn rec(&self, what: impl Into<String>) {
        self.state.lock().unwrap().calls.push(what.into());
    }
}

impl Memory for FakeMemory {
    fn stats(&self) -> MemStats {
        self.rec("stats");
        self.state.lock().unwrap().stats.clone()
    }
    fn conf_path(&self) -> PathBuf {
        PathBuf::from("/fake/system.conf")
    }
    fn read_conf(&self) -> String {
        self.state.lock().unwrap().conf.clone()
    }
    fn save_conf(&self, cfg: &Config) -> Result<(), String> {
        self.rec(format!("save_conf(zram={})", cfg.zram_value()));
        let mut s = self.state.lock().unwrap();
        s.conf = format!("zram={}\nswappiness={}\n", cfg.zram_value(), cfg.swappiness);
        Ok(())
    }
    fn set_conf_key(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        self.rec(format!("set_conf_key({key},{value:?})"));
        let mut s = self.state.lock().unwrap();
        let mut lines: Vec<String> = s.conf.lines().filter(|l| l.split('=').next() != Some(key)).map(String::from).collect();
        if let Some(v) = value {
            lines.push(format!("{key}={v}"));
        }
        s.conf = lines.join("\n") + "\n";
        Ok(())
    }
    fn reconfigure(&self, cfg: &Config, ram: u64) -> Result<Vec<Step>, String> {
        self.rec(format!("reconfigure(zram={},ram={ram})", cfg.zram_value()));
        match &self.state.lock().unwrap().refuse {
            Some(why) => Err(why.clone()),
            None => Ok(vec![Step { ok: true, msg: "zram set up (fake)".into() }]),
        }
    }
    fn apply_swapfile(&self, bytes: Option<u64>, on_disk: bool) -> Vec<Step> {
        self.rec(format!("apply_swapfile({bytes:?},{on_disk})"));
        vec![Step { ok: true, msg: "swap file left alone (fake)".into() }]
    }
    fn data_place(&self, _: &Path) -> DataPlace {
        self.state.lock().unwrap().place.unwrap_or(DataPlace::Other)
    }
    fn data_free(&self, _: &Path) -> Option<u64> {
        self.state.lock().unwrap().free
    }
}
