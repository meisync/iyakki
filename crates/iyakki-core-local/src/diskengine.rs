//! `LocalDiskEngine`: the core's [`DiskEngine`] on this machine (F-ARCH-05, F-ARCH-07). One handle over the disk engine of `disks.rs`; the engine
//! reads `/sys` and `/proc` and mounts through the `DiskEnv` seam, so a test gives it a made-up machine ([`LocalDiskEngine::over`]).

use std::sync::{Arc, Mutex, OnceLock};

use iyakki_core_api::diskmodel::{Actor, AuditFn, Confirm, Disk, DiskEngine, MountOpts, MountPlan, Mounted, PartOp, Plan, Snapshot};
use iyakki_diskcrypt::{CryptKind, Secret};

use crate::disks::Disks;
use crate::disks_helper::{CryptRunner, FakeSys, ProcRunner};

pub struct LocalDiskEngine {
    disks: Arc<Disks>,
    /// Opens and closes LUKS and BitLocker volumes: "no helper installed" until [`DiskEngine::use_programs`] (or a test) sets one.
    crypt: Mutex<Arc<dyn CryptRunner>>,
}

/// The runner of an engine that was not told where the programs are: nothing is installed, nothing is started.
fn no_helpers() -> Arc<dyn CryptRunner> {
    Arc::new(ProcRunner::new(Arc::new(FakeSys::default()), "/none", "/none"))
}

impl LocalDiskEngine {
    /// This computer: one engine for the whole program (it keeps the wrong-key wait and the partition cache).
    pub fn system() -> LocalDiskEngine {
        static REAL: OnceLock<Arc<Disks>> = OnceLock::new();
        LocalDiskEngine { disks: REAL.get_or_init(|| Arc::new(Disks::real())).clone(), crypt: Mutex::new(no_helpers()) }
    }

    /// The same engine as a tile that holds `disks` already.
    pub fn over(disks: Arc<Disks>) -> LocalDiskEngine {
        LocalDiskEngine { disks, crypt: Mutex::new(no_helpers()) }
    }

    /// Tests: the runner that opens and closes encrypted volumes.
    pub fn set_crypt(&self, runner: Arc<dyn CryptRunner>) {
        *self.crypt.lock().unwrap_or_else(|e| e.into_inner()) = runner;
    }

    /// Tests: the wrong-key wait uses a clock that stands still at `secs`.
    pub fn hold_clock(&self, secs: u64) {
        self.disks.hold_clock(secs)
    }

    /// Tests: the person-installed file system programs (exFAT, NTFS, resizing).
    pub fn set_tools(&self, t: Arc<dyn crate::disks_fmt::FsTools>) {
        self.disks.set_tools(t)
    }

    fn runner(&self) -> Arc<dyn CryptRunner> {
        self.crypt.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl DiskEngine for LocalDiskEngine {
    fn scan(&self) -> Snapshot {
        self.disks.scan()
    }
    fn fingerprint(&self) -> u64 {
        self.disks.fingerprint()
    }
    fn forget(&self) {
        self.disks.forget()
    }
    fn users_of(&self, mountpoint: &str) -> Vec<(u32, String)> {
        self.disks.users_of(mountpoint)
    }
    fn mounted_in(&self, home: &str) -> Vec<(String, String)> {
        self.disks.mounted_in(home)
    }
    fn plan_mount(&self, snap: &Snapshot, who: &Actor, part: &str, opts: &MountOpts) -> Result<MountPlan, String> {
        self.disks.plan_mount(snap, who, part, opts)
    }
    fn mount(&self, who: &Actor, part: &str, opts: &MountOpts, audit: AuditFn) -> Result<Mounted, String> {
        self.disks.mount(who, part, opts, audit)
    }
    fn unmount(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        self.disks.unmount(who, part, audit)
    }
    fn eject(&self, who: &Actor, disk: &str, audit: AuditFn) -> Result<String, String> {
        self.disks.eject(who, disk, audit)
    }
    fn at_sign_out(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        self.disks.at_sign_out(who, audit)
    }
    fn after_unplug(&self, who: &Actor, audit: AuditFn) -> Option<String> {
        self.disks.after_unplug(who, audit)
    }
    fn refusal(&self, who: &Actor, d: &Disk) -> Option<String> {
        self.disks.refusal(who, d)
    }
    fn plan_partition(&self, who: &Actor, disk: &str, op: &PartOp) -> Result<Plan, String> {
        self.disks.plan_partition(who, disk, op)
    }
    fn apply_partition(&self, who: &Actor, disk: &str, op: &PartOp, confirm: &Confirm, audit: AuditFn) -> Result<String, String> {
        self.disks.apply_partition(who, disk, op, confirm, audit)
    }
    fn plan_unlock(&self, snap: &Snapshot, who: &Actor, part: &str) -> Result<CryptKind, String> {
        self.disks.plan_unlock(snap, who, part, &*self.runner())
    }
    fn unlock(&self, who: &Actor, part: &str, secret: &Secret, audit: AuditFn) -> Result<Mounted, String> {
        self.disks.unlock(who, part, secret, &*self.runner(), audit)
    }
    fn allow_writing(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<Mounted, String> {
        self.disks.allow_writing(who, part, audit)
    }
    fn lock(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<String, String> {
        self.disks.lock(who, part, &*self.runner(), audit)
    }
    fn take_format_request(&self, dir: &std::path::Path, user: &str) -> Option<iyakki_core_api::diskmodel::FormatRequest> {
        crate::disksformattools::take_request(dir, user)
    }
    fn use_programs(&self, bin_dir: &str) {
        let dir = std::path::Path::new(bin_dir);
        self.disks.set_tools(Arc::new(crate::disks_fmt::ProcTools::real(dir)));
        self.set_crypt(Arc::new(ProcRunner::real(dir)));
    }
}
