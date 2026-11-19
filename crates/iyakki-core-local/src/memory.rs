//! `LocalMemory`: the core's [`Memory`] on this machine. It holds where the kernel's views are (`Env`), the swap seam and the path of
//! `system.conf`, so a test points it at a made-up root with a swapper that records instead of calling `swapon(2)`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_core_api::memory::{self, config::Config, DataPlace, Memory, MemStats, Step, Swapper};
use iyakki_zram::Env;

pub struct LocalMemory {
    env: Env,
    swapper: Arc<dyn Swapper + Send + Sync>,
    conf: PathBuf,
    roots: iyakki_storage::Roots,
}

/// The swapper of a made-up machine: it refuses, so a test that forgot to give its own never changes the real swap.
struct NoSwap;

impl Swapper for NoSwap {
    fn swapon(&self, _: &Path, _: i32) -> Result<(), String> {
        Err("this is a made-up machine: swap is not changed".into())
    }
    fn swapoff(&self, _: &Path) -> Result<(), String> {
        Err("this is a made-up machine: swap is not changed".into())
    }
}

fn made_up_roots(proc: &Path, sys: &Path, dev: &Path) -> iyakki_storage::Roots {
    iyakki_storage::Roots { sys_block: sys.join("block"), dev: dev.into(), mounts: proc.join("mounts"), require_block: false }
}

impl LocalMemory {
    /// This machine.
    pub fn system() -> LocalMemory {
        LocalMemory { env: Env::real(), swapper: Arc::new(iyakki_zram::apply::KernelSwap), conf: memory::CONF_PATH.into(), roots: iyakki_storage::Roots::real() }
    }
    /// A made-up machine under `root`; `system.conf` is `root/system.conf` and swap is not changed.
    pub fn under(root: &Path) -> LocalMemory {
        LocalMemory { env: Env::under(root), swapper: Arc::new(NoSwap), conf: root.join("system.conf"), roots: made_up_roots(&root.join("proc"), &root.join("sys"), &root.join("dev")) }
    }
    /// Made-up `/proc` and `/sys`; `system.conf` sits beside the `/sys` folder and swap is not changed.
    pub fn with_roots(proc: PathBuf, sys: PathBuf) -> LocalMemory {
        let conf = sys.with_file_name("system.conf");
        LocalMemory { env: Env::with_roots(proc.clone(), sys.clone()), swapper: Arc::new(NoSwap), conf, roots: made_up_roots(&proc, &sys, Path::new("/dev")) }
    }
    /// Everything given (tests).
    pub fn new(env: Env, swapper: Arc<dyn Swapper + Send + Sync>, conf: PathBuf) -> LocalMemory {
        let roots = made_up_roots(&env.proc, &env.sys, &env.dev);
        LocalMemory { env, swapper, conf, roots }
    }
}

impl Memory for LocalMemory {
    fn stats(&self) -> MemStats {
        MemStats::read(&self.env)
    }
    fn conf_path(&self) -> PathBuf {
        self.conf.clone()
    }
    fn read_conf(&self) -> String {
        std::fs::read_to_string(&self.conf).unwrap_or_default()
    }
    fn save_conf(&self, cfg: &Config) -> Result<(), String> {
        iyakki_zram::save_conf(&self.conf, cfg)
    }
    fn set_conf_key(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        iyakki_zram::set_conf_key(&self.conf, key, value)
    }
    fn reconfigure(&self, cfg: &Config, ram: u64) -> Result<Vec<Step>, String> {
        iyakki_zram::apply::reconfigure(&self.env, self.swapper.as_ref(), cfg, ram)
    }
    fn apply_swapfile(&self, bytes: Option<u64>, on_disk: bool) -> Vec<Step> {
        iyakki_zram::apply::apply_swapfile(self.swapper.as_ref(), &self.env, Path::new(memory::SWAPFILE_PATH), bytes, on_disk)
    }
    fn data_place(&self, dir: &Path) -> DataPlace {
        match iyakki_storage::data_location(&self.roots, dir) {
            iyakki_storage::DataLocation::Disk(_) => DataPlace::Disk,
            iyakki_storage::DataLocation::Memory => DataPlace::Memory,
            iyakki_storage::DataLocation::Other(_) => DataPlace::Other,
        }
    }
    fn data_free(&self, dir: &Path) -> Option<u64> {
        iyakki_zram::compressed::data_free(dir)
    }
}
