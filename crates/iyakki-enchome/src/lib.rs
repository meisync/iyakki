//! Encrypted per-person home folders (F-STORE-05, queue item Q5; design: docs/development/DATA-VOLUME-ENCRYPTION.md, option B).
//!
//! A person's private folder is a container file `<data>/home-enc/<account>.ivol` in the IYAKVOL1 format (`iyakki-volcrypt`). At sign-in the
//! sign-in password opens it: the file is attached to a loop device, mapped through dm-crypt (`iyakki_storage::dm`, no `cryptsetup`) and
//! mounted over `/home/<account>`. Signing out (or locking the screen, if the person chose that) unmounts and closes it again.
//!
//! What this crate decides: how a home is made (with a recovery key that counts as saved only after the person typed it back), how it is
//! opened without telling which part of a wrong secret was wrong, what a password change, an administrator's reset and a recovery key do,
//! the optional administrator slot, and (module [`migrate`]) the opt-in move of an existing plain home. Everything that touches the kernel
//! goes through `DmKernel`, so every path here is tested with the recording fake and temp folders; nothing in the tests needs root.
//!
//! Honest limit: nothing here has run on a real kernel yet (the kernel needs `DM_CRYPT`, `XTS`, `BLK_DEV_DM` and loop first).

pub mod form;
pub mod migrate;
pub mod testkit;

#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod migrate_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_diskcrypt::Limiter;
use iyakki_storage::dm::{self, Crypt, DmKernel};
use iyakki_volcrypt::container::{container_len, FileStore, Volume, PAYLOAD_OFFSET};
use iyakki_volcrypt::header::{Header, KdfParams, SlotKind, FLOOR};
use iyakki_volcrypt::recovery::RecoveryKey;
use iyakki_volcrypt::{OsRandom, Random, VolError};
use zeroize::Zeroizing;

/// The folder (inside the data folder) holding the containers.
pub const DIR: &str = "home-enc";
/// The plain home is renamed to `<account>` + this by the migration and kept until the person agrees to remove it.
pub const PLAIN_SUFFIX: &str = ".plain-before-encrypt";
/// The data unit of new containers (matches `sector_size:4096` in the dm-crypt table).
pub const SECTOR: u32 = 4096;
/// Smallest and default-largest private folder (the file is sparse, so the size is a limit, not a cost).
pub const MIN_PAYLOAD: u64 = 64 << 20;
pub const MAX_DEFAULT_PAYLOAD: u64 = 32 << 30;
/// Free space always left on the data volume after making a container.
pub const FREE_MARGIN: u64 = 256 << 20;

/// What went wrong, in a sentence a person can read. Never holds a password, a key or a recovery key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeError {
    /// The one answer for every wrong secret: it never says which part failed or whether a recovery key exists.
    WrongSecret,
    Throttled(u64),
    NoHome,
    AlreadyExists,
    BadAccount,
    PlainHomeHasFiles,
    NotEnoughSpace { need: u64, free: u64 },
    InUse,
    NotOpen,
    NotConfirmed(String),
    Volume(String),
    Kernel(String),
    Io(String),
}

impl std::fmt::Display for HomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HomeError::WrongSecret => write!(f, "The password or recovery key is wrong."),
            HomeError::Throttled(s) => write!(f, "Too many wrong tries. Wait {s} seconds and try again."),
            HomeError::NoHome => write!(f, "There is no encrypted home for this account."),
            HomeError::AlreadyExists => write!(f, "This account already has an encrypted home."),
            HomeError::BadAccount => write!(f, "That account name cannot have an encrypted home."),
            HomeError::PlainHomeHasFiles => write!(f, "Your home folder already holds files. Making an empty encrypted home on top would hide them; use \"Move my existing files\" instead, which copies, checks every file and keeps the old copy until you agree."),
            HomeError::NotEnoughSpace { need, free } => write!(f, "Not enough free space: {} needed, {} free.", human(*need), human(*free)),
            HomeError::InUse => write!(f, "Something still uses your private folder. Close the apps and files that are open in it and try again."),
            HomeError::NotOpen => write!(f, "The encrypted home is locked."),
            HomeError::NotConfirmed(w) => write!(f, "{w}"),
            HomeError::Volume(w) => write!(f, "{w}"),
            HomeError::Kernel(w) => write!(f, "The encrypted home could not be opened or closed: {w}"),
            HomeError::Io(w) => write!(f, "Reading or writing the encrypted home failed: {w}"),
        }
    }
}

impl std::error::Error for HomeError {}

/// Bytes in words ("1.5 GiB").
pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

impl From<VolError> for HomeError {
    fn from(e: VolError) -> HomeError {
        match e {
            // the one message for a wrong password or recovery key; a malformed recovery key keeps its own words (it never reached a slot)
            VolError::WrongSecret => HomeError::WrongSecret,
            other => HomeError::Volume(other.to_string()),
        }
    }
}

fn io_err(e: std::io::Error) -> HomeError {
    HomeError::Io(e.to_string())
}

fn kernel_err(e: impl std::fmt::Display) -> HomeError {
    HomeError::Kernel(e.to_string())
}

/// What a person typed to open the home. Wiped when dropped.
pub enum Secret {
    Password(Zeroizing<String>),
    Recovery(Zeroizing<String>),
}

impl Secret {
    pub fn password(p: &str) -> Secret {
        Secret::Password(Zeroizing::new(p.to_string()))
    }
    pub fn recovery(t: &str) -> Secret {
        Secret::Recovery(Zeroizing::new(t.to_string()))
    }
}

/// The recovery key as text, shown once. Wiped when dropped.
pub struct RecoveryText(pub Zeroizing<String>);

impl std::fmt::Debug for RecoveryText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RecoveryText(hidden)")
    }
}

/// The small plain file next to a container: choices and flags, never a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// The person retyped the recovery key and it opened the volume.
    pub recovery_confirmed: bool,
    /// Lock the home when the screen locks (and open it with the password at the lock screen).
    pub lock_with_screen: bool,
    /// An administrator reset the sign-in password: the new password does not open the home.
    pub sealed_by_reset: bool,
}

impl Default for Meta {
    fn default() -> Meta {
        Meta { recovery_confirmed: false, lock_with_screen: false, sealed_by_reset: false }
    }
}

/// One way to open the home, for Settings. Only kinds, never secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotInfo {
    Password,
    Recovery,
    Admin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No encrypted home for this account.
    NotSetUp,
    /// Made and closed.
    Locked,
    /// Made and open (mounted).
    Open,
    /// The container file is there but its header cannot be read.
    Damaged,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    pub damaged_reason: Option<String>,
    pub container: PathBuf,
    pub size_bytes: Option<u64>,
    pub slots: Vec<SlotInfo>,
    pub meta: Meta,
    /// One of the two header copies was damaged (the next change repairs it).
    pub one_copy_bad: bool,
    /// The old plain home from a migration is still on disk, waiting for the person to agree to remove it.
    pub plain_copy: Option<PathBuf>,
}

impl Status {
    pub fn has_admin_slot(&self) -> bool {
        self.slots.contains(&SlotInfo::Admin)
    }
    pub fn has_recovery_slot(&self) -> bool {
        self.slots.contains(&SlotInfo::Recovery)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the environment
// ---------------------------------------------------------------------------------------------------------------------

type FreeFn = dyn Fn(&Path) -> Option<u64> + Send + Sync;
type ClockFn = dyn Fn() -> u64 + Send + Sync;

/// Everything outside this crate the lifecycle depends on (the kernel, the random source, the clock, the free space). Tests replace all of it.
pub struct EncHome {
    pub data_dir: PathBuf,
    pub home_parent: PathBuf,
    pub kernel: Arc<dyn DmKernel>,
    /// The cost for new password slots (calibrated on the real machine; tests use a small one).
    kdf: Option<KdfParams>,
    rng: Mutex<Box<dyn Random + Send>>,
    free: Box<FreeFn>,
    clock: Box<ClockFn>,
    /// How long to wait between tries to unmount a busy home at sign-out.
    pub retry_wait: Duration,
    pub retries: u32,
    /// Where the account's `passwd` entry is found (for the owner of a new home).
    pub passwd: PathBuf,
}

/// Wrong tries per account, kept for the whole process: a new `EncHome` for each sign-in must not reset the wait.
fn limiter() -> &'static Mutex<Limiter> {
    static L: OnceLock<Mutex<Limiter>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Limiter::default()))
}

/// The loop device each open home sits on (to detach it after closing).
fn loops() -> &'static Mutex<HashMap<String, String>> {
    static L: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs()
}

/// The Argon2id cost for new password slots on this machine: memory floor 256 MiB, passes raised until one derivation takes about
/// `target_ms` (never lowered below the format's floor to hide a slow machine). `time_ms` measures one derivation with the given cost.
pub fn calibrate(target_ms: u64, time_ms: &mut dyn FnMut(&KdfParams) -> u64) -> KdfParams {
    let mut k = KdfParams { m_kib: 256 * 1024, t: FLOOR.t, p: FLOOR.p };
    let ms = time_ms(&k).max(1);
    let per_pass = (ms as f64 / k.t as f64).max(0.001);
    let t = (target_ms as f64 / per_pass).round() as u32;
    k.t = t.clamp(FLOOR.t, iyakki_volcrypt::header::MAX_T);
    k
}

/// The real calibration (about a second of work, once per process).
pub fn calibrated_kdf() -> KdfParams {
    static K: OnceLock<KdfParams> = OnceLock::new();
    *K.get_or_init(|| {
        calibrate(1000, &mut |k| {
            let t0 = Instant::now();
            let _ = iyakki_volcrypt::header::derive_kek(b"calibration", &[7u8; 32], k);
            t0.elapsed().as_millis() as u64
        })
    })
}

/// The mapping name of an account's home.
pub fn map_name(user: &str) -> String {
    format!("iyakki-home-{user}")
}

fn valid_account(user: &str) -> bool {
    iyakki_sec::policy::valid_username(user) && dm::valid_name(&map_name(user))
}

fn tag_of(user: &str) -> u32 {
    // a small label for Settings, not a secret: FNV-1a of the account name
    user.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

impl EncHome {
    /// The real thing: the kernel, `/home`, the operating system's random numbers. The cost is calibrated on first use that needs it.
    pub fn real(data_dir: &Path) -> EncHome {
        EncHome {
            data_dir: data_dir.to_path_buf(),
            home_parent: PathBuf::from("/home"),
            kernel: Arc::new(dm::SysKernel),
            kdf: None,
            rng: Mutex::new(Box::new(OsRandom)),
            free: Box::new(|p| iyakki_storage::space(p).map(|(free, _)| free)),
            clock: Box::new(now_secs),
            retry_wait: Duration::from_millis(300),
            retries: 5,
            passwd: PathBuf::from("/etc/passwd"),
        }
    }

    /// For tests and tools that only read the status: nothing is calibrated, nothing touches the kernel.
    pub fn with(data_dir: &Path, home_parent: &Path, kernel: Arc<dyn DmKernel>, kdf: KdfParams, rng: Box<dyn Random + Send>, free: Box<FreeFn>, clock: Box<ClockFn>) -> EncHome {
        EncHome { data_dir: data_dir.to_path_buf(), home_parent: home_parent.to_path_buf(), kernel, kdf: Some(kdf), rng: Mutex::new(rng), free, clock, retry_wait: Duration::ZERO, retries: 2, passwd: PathBuf::from("/nonexistent-passwd") }
    }

    /// A cheap read-only handle (status only): no calibration, no kernel work happens through it.
    pub fn for_status(data_dir: &Path) -> EncHome {
        EncHome {
            data_dir: data_dir.to_path_buf(),
            home_parent: PathBuf::from("/home"),
            kernel: Arc::new(dm::SysKernel),
            kdf: None,
            rng: Mutex::new(Box::new(OsRandom)),
            free: Box::new(|p| iyakki_storage::space(p).map(|(free, _)| free)),
            clock: Box::new(now_secs),
            retry_wait: Duration::from_millis(300),
            retries: 5,
            passwd: PathBuf::from("/etc/passwd"),
        }
    }

    // ---- paths ------------------------------------------------------------------------------------------------------

    /// The cost for new password slots: the one given, or (the real thing) calibrated on this machine at first use.
    pub fn kdf(&self) -> KdfParams {
        self.kdf.unwrap_or_else(calibrated_kdf)
    }

    pub fn dir(&self) -> PathBuf {
        self.data_dir.join(DIR)
    }
    pub fn container_path(&self, user: &str) -> PathBuf {
        self.dir().join(format!("{user}.ivol"))
    }
    fn meta_path(&self, user: &str) -> PathBuf {
        self.dir().join(format!("{user}.meta"))
    }
    pub fn mount_point(&self, user: &str) -> PathBuf {
        self.home_parent.join(user)
    }
    pub fn plain_copy_path(&self, user: &str) -> PathBuf {
        self.home_parent.join(format!("{user}{PLAIN_SUFFIX}"))
    }
    fn limiter_key(&self, user: &str) -> String {
        format!("{}|{user}", self.data_dir.display())
    }

    /// Whether the account has a container (cheap: one `stat`).
    pub fn exists(&self, user: &str) -> bool {
        valid_account(user) && self.container_path(user).is_file()
    }

    pub fn is_open(&self, user: &str) -> bool {
        valid_account(user) && self.kernel.is_mounted(&self.mount_point(user))
    }

    fn rng<R>(&self, f: impl FnOnce(&mut dyn Random) -> R) -> R {
        let mut g = self.rng.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut **g)
    }

    // ---- the small meta file ----------------------------------------------------------------------------------------

    pub fn meta(&self, user: &str) -> Meta {
        let Ok(t) = std::fs::read_to_string(self.meta_path(user)) else { return Meta::default() };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { return Meta::default() };
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Meta { recovery_confirmed: b("recovery_confirmed"), lock_with_screen: b("lock_with_screen"), sealed_by_reset: b("sealed_by_reset") }
    }

    pub fn save_meta(&self, user: &str, m: &Meta) -> Result<(), HomeError> {
        let v = serde_json::json!({"v": 1, "recovery_confirmed": m.recovery_confirmed, "lock_with_screen": m.lock_with_screen, "sealed_by_reset": m.sealed_by_reset});
        let p = self.meta_path(user);
        let tmp = p.with_extension("meta.new");
        std::fs::write(&tmp, v.to_string()).map_err(io_err)?;
        std::fs::rename(&tmp, &p).map_err(io_err)
    }

    fn update_meta(&self, user: &str, f: impl FnOnce(&mut Meta)) -> Result<(), HomeError> {
        let mut m = self.meta(user);
        f(&mut m);
        self.save_meta(user, &m)
    }

    // ---- status -----------------------------------------------------------------------------------------------------

    /// Read the header (no password needed): which ways to open exist, whether a copy is damaged.
    fn read_header(&self, path: &Path) -> Result<(Header, bool), VolError> {
        use std::os::unix::fs::FileExt;
        let f = std::fs::File::open(path).map_err(|e| VolError::Io(e.to_string()))?;
        let read = |at: u64| -> Result<Header, VolError> {
            let mut b = vec![0u8; iyakki_volcrypt::header::HEADER_LEN];
            f.read_exact_at(&mut b, at).map_err(|e| VolError::Io(e.to_string()))?;
            Header::parse(&b)
        };
        match (read(0), read(iyakki_volcrypt::header::HEADER_LEN as u64)) {
            (Ok(a), Ok(b)) => Ok(if b.generation > a.generation { (b, false) } else { (a, false) }),
            (Ok(a), Err(_)) => Ok((a, true)),
            (Err(_), Ok(b)) => Ok((b, true)),
            (Err(ea), Err(eb)) => Err(if ea == VolError::NotOurs || eb == VolError::NotOurs { VolError::NotOurs } else { ea }),
        }
    }

    pub fn status(&self, user: &str) -> Status {
        let container = if valid_account(user) { self.container_path(user) } else { self.dir().join("invalid.ivol") };
        let plain = self.plain_copy_path(user);
        let plain_copy = (valid_account(user) && plain.is_dir()).then_some(plain);
        let mut st = Status { state: State::NotSetUp, damaged_reason: None, container: container.clone(), size_bytes: None, slots: vec![], meta: self.meta(user), one_copy_bad: false, plain_copy };
        if !valid_account(user) || !container.is_file() {
            return st;
        }
        st.size_bytes = std::fs::metadata(&container).ok().map(|m| m.len());
        match self.read_header(&container) {
            Ok((h, bad)) => {
                st.one_copy_bad = bad;
                st.slots = h
                    .list()
                    .into_iter()
                    .map(|(_, k, _)| match k {
                        SlotKind::Person => SlotInfo::Password,
                        SlotKind::Recovery => SlotInfo::Recovery,
                        SlotKind::Admin => SlotInfo::Admin,
                    })
                    .collect();
                st.state = if self.is_open(user) { State::Open } else { State::Locked };
            }
            Err(e) => {
                st.state = State::Damaged;
                st.damaged_reason = Some(e.to_string());
            }
        }
        st
    }

    // ---- throttle ---------------------------------------------------------------------------------------------------

    fn throttle_check(&self, user: &str) -> Result<(), HomeError> {
        limiter().lock().unwrap_or_else(|e| e.into_inner()).check(&self.limiter_key(user), (self.clock)()).map_err(HomeError::Throttled)
    }
    fn throttle_failed(&self, user: &str) {
        limiter().lock().unwrap_or_else(|e| e.into_inner()).failed(&self.limiter_key(user), (self.clock)());
    }
    fn throttle_ok(&self, user: &str) {
        limiter().lock().unwrap_or_else(|e| e.into_inner()).ok(&self.limiter_key(user));
    }

    // ---- opening a container ----------------------------------------------------------------------------------------

    fn open_file(&self, user: &str) -> Result<FileStore, HomeError> {
        OpenOptions::new().read(true).write(true).open(self.container_path(user)).map(FileStore).map_err(io_err)
    }

    /// Unlock the container with a secret (throttled; a wrong secret always gets the same answer). Used for every route that needs the key.
    fn volume(&self, user: &str, secret: &Secret) -> Result<Volume<FileStore>, HomeError> {
        self.volume_r(user, secret).map(|(v, _)| v)
    }

    /// Like [`Self::volume`], and says whether a damaged header copy was repaired by this open.
    fn volume_r(&self, user: &str, secret: &Secret) -> Result<(Volume<FileStore>, bool), HomeError> {
        if !self.exists(user) {
            return Err(HomeError::NoHome);
        }
        self.throttle_check(user)?;
        let store = self.open_file(user)?;
        let r = match secret {
            Secret::Password(p) => Volume::open_password(store, p.as_bytes()),
            Secret::Recovery(t) => match RecoveryKey::parse(t) {
                Ok(rk) => Volume::open_recovery(store, &rk),
                // a mistyped key is told apart on purpose (the check characters say so before any slot is tried); it is no guess at a secret
                Err(e) => return Err(HomeError::Volume(e.to_string())),
            },
        };
        match r {
            Ok(mut v) => {
                self.throttle_ok(user);
                let repaired = v.other_copy_bad;
                if repaired {
                    let _ = v.sync_copies();
                }
                Ok((v, repaired))
            }
            Err(VolError::WrongSecret) => {
                self.throttle_failed(user);
                Err(HomeError::WrongSecret)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Attach the file and map it. Leaves nothing behind on failure.
    fn activate(&self, user: &str, vol: &Volume<FileStore>) -> Result<(), HomeError> {
        let h = vol.header();
        let dev = self.kernel.loop_attach(&self.container_path(user)).map_err(kernel_err)?;
        let r = Crypt::new(&*self.kernel).open(&map_name(user), vol.key(), &dev, h.payload_offset / 512, h.payload_bytes / 512, h.sector_size);
        if let Err(e) = r {
            let _ = self.kernel.loop_detach(&dev);
            return Err(kernel_err(e));
        }
        loops().lock().unwrap_or_else(|e| e.into_inner()).insert(self.container_path(user).display().to_string(), dev);
        Ok(())
    }

    /// Undo `activate` (mapping removed, loop detached). Errors are returned for the caller to decide.
    fn deactivate(&self, user: &str) -> Result<(), HomeError> {
        let key = self.container_path(user).display().to_string();
        match Crypt::new(&*self.kernel).status(&map_name(user)).map_err(kernel_err)? {
            Some(_) => Crypt::new(&*self.kernel).close(&map_name(user)).map_err(|e| if self.is_busy(&e.to_string()) { HomeError::InUse } else { kernel_err(e) })?,
            None => {}
        }
        if let Some(dev) = loops().lock().unwrap_or_else(|e| e.into_inner()).remove(&key) {
            let _ = self.kernel.loop_detach(&dev); // the loop device clears itself once nothing holds it, so a failure here is harmless
        }
        Ok(())
    }

    fn is_busy(&self, text: &str) -> bool {
        text.contains("Device or resource busy") || text.contains("busy")
    }

    fn mount_over_home(&self, user: &str) -> Result<(), HomeError> {
        let mp = self.mount_point(user);
        std::fs::create_dir_all(&mp).map_err(io_err)?;
        self.kernel.mount(&dm::map_path(&map_name(user)), &mp).map_err(kernel_err)
    }

    /// Open the home for `user` with `secret` and mount it. Already open: nothing happens.
    pub fn unlock(&self, user: &str, secret: &Secret) -> Result<Unlock, HomeError> {
        if !valid_account(user) {
            return Err(HomeError::BadAccount);
        }
        if self.is_open(user) {
            return Ok(Unlock::AlreadyOpen);
        }
        let (vol, repaired) = self.volume_r(user, secret)?;
        self.activate(user, &vol)?;
        drop(vol);
        if let Err(e) = self.mount_over_home(user) {
            let _ = self.deactivate(user);
            return Err(e);
        }
        Ok(Unlock::Opened { repaired_header: repaired })
    }

    /// Unmount and close the home. A busy home is retried a few times (apps that are just closing), then reported as in use; nothing is forced.
    pub fn lock(&self, user: &str) -> Result<(), HomeError> {
        if !valid_account(user) {
            return Err(HomeError::BadAccount);
        }
        if !self.exists(user) {
            return Err(HomeError::NoHome);
        }
        let mp = self.mount_point(user);
        let mut tries = 0;
        while self.kernel.is_mounted(&mp) {
            match self.kernel.umount(&mp) {
                Ok(()) => break,
                Err(e) if tries < self.retries => {
                    tries += 1;
                    let _ = e;
                    std::thread::sleep(self.retry_wait);
                }
                Err(e) => return Err(if e.raw_os_error() == Some(EBUSY) { HomeError::InUse } else { kernel_err(e) }),
            }
        }
        self.deactivate(user)
    }

    // ---- making a home ----------------------------------------------------------------------------------------------

    /// The size a new private folder gets when the person does not choose: half the free space, between the minimum and 32 GiB.
    pub fn default_payload(&self) -> Result<u64, HomeError> {
        let free = (self.free)(&self.data_dir).unwrap_or(0);
        let usable = free.saturating_sub(FREE_MARGIN);
        let want = (usable / 2).min(MAX_DEFAULT_PAYLOAD) / SECTOR as u64 * SECTOR as u64;
        if want < MIN_PAYLOAD {
            return Err(HomeError::NotEnoughSpace { need: MIN_PAYLOAD + FREE_MARGIN, free });
        }
        Ok(want)
    }

    /// Make an encrypted home, open it and mount it. `password` is the sign-in password. The home folder must be empty or missing (a
    /// folder with files is the migration tool's job). The returned recovery key is shown once; it counts as saved only after
    /// [`EncHome::confirm_recovery`] succeeded.
    pub fn create(&self, user: &str, password: &str, payload_bytes: Option<u64>) -> Result<RecoveryText, HomeError> {
        if !valid_account(user) {
            return Err(HomeError::BadAccount);
        }
        if self.exists(user) {
            return Err(HomeError::AlreadyExists);
        }
        let mp = self.mount_point(user);
        // empty folders and the few files the system itself puts in a new home at sign-in do not count as "files" (found on the
        // Mac mini 2018: every new account failed here, because sign-in had made .claude/.claude.json)
        if mp.is_dir() && holds_person_files(&mp).map_err(io_err)? {
            return Err(HomeError::PlainHomeHasFiles);
        }
        let payload = match payload_bytes {
            Some(p) => p / SECTOR as u64 * SECTOR as u64,
            None => self.default_payload()?,
        };
        let free = (self.free)(&self.data_dir).unwrap_or(0);
        if payload < MIN_PAYLOAD || payload + FREE_MARGIN > free {
            return Err(HomeError::NotEnoughSpace { need: payload.max(MIN_PAYLOAD) + FREE_MARGIN, free });
        }
        if password.is_empty() {
            return Err(HomeError::Volume("a password is needed to make the encrypted home".into()));
        }
        // what the system wrote there at sign-in goes before the mount hides it (it makes it again, in the encrypted home)
        remove_sign_in_stubs(&mp);
        self.make_container(user, password, payload, true)
    }

    /// Container, file system, mapping and (for `mount`) the mount. Everything is undone on any failure.
    pub(crate) fn make_container(&self, user: &str, password: &str, payload: u64, mount: bool) -> Result<RecoveryText, HomeError> {
        std::fs::create_dir_all(self.dir()).map_err(io_err)?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(self.dir(), std::fs::Permissions::from_mode(0o700));
        }
        let path = self.container_path(user);
        let result = (|| -> Result<RecoveryText, HomeError> {
            let f = {
                use std::os::unix::fs::OpenOptionsExt;
                OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path).map_err(io_err)?
            };
            f.set_len(container_len(payload)).map_err(io_err)?;
            let mut vol = self.rng(|r| Volume::format(FileStore(f), r, SECTOR, payload, password.as_bytes(), SlotKind::Person, tag_of(user), self.kdf()))?;
            let (_, rk) = self.rng(|r| vol.change(|h, k| h.set_recovery(r, k)))?;
            let text = RecoveryText(Zeroizing::new(rk.to_text()));
            self.save_meta(user, &Meta::default())?;
            self.activate(user, &vol)?;
            drop(vol);
            let dev = dm::map_path(&map_name(user));
            if let Err(e) = self.kernel.make_fs(&dev, "IYAKKI-HOME") {
                let _ = self.deactivate(user);
                return Err(kernel_err(e));
            }
            if mount {
                let before = std::fs::metadata(self.mount_point(user)).ok();
                if let Err(e) = self.mount_over_home(user) {
                    let _ = self.deactivate(user);
                    return Err(e);
                }
                self.own_mount_root(user, before.as_ref());
            }
            Ok(text)
        })();
        if result.is_err() {
            // nothing half-made stays: the container, its meta file, any mapping
            let _ = self.deactivate(user);
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(self.meta_path(user));
        }
        result
    }

    /// The root of a fresh file system belongs to root with mode 0755; the home is the account's, mode 0700.
    fn own_mount_root(&self, user: &str, before: Option<&std::fs::Metadata>) {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mp = self.mount_point(user);
        let _ = std::fs::set_permissions(&mp, std::fs::Permissions::from_mode(0o700));
        let owner = before.map(|m| (m.uid(), m.gid())).filter(|(u, _)| *u != 0).or_else(|| {
            let t = std::fs::read_to_string(&self.passwd).ok()?;
            t.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                if f.len() >= 4 && f[0] == user { Some((f[2].parse::<u32>().ok()?, f[3].parse::<u32>().ok()?)).filter(|(u, _)| *u != 0) } else { None }
            })
        });
        if let Some((u, g)) = owner {
            let _ = std::os::unix::fs::chown(&mp, Some(u), Some(g));
        }
    }

    // ---- recovery key -----------------------------------------------------------------------------------------------

    /// The person typed the recovery key back: if it opens the volume, it counts as saved.
    pub fn confirm_recovery(&self, user: &str, typed: &str) -> Result<(), HomeError> {
        if !self.exists(user) {
            return Err(HomeError::NoHome);
        }
        let rk = RecoveryKey::parse(typed).map_err(|e| HomeError::NotConfirmed(format!("The recovery key you typed is not written correctly: {e}.")))?;
        let (h, _) = self.read_header(&self.container_path(user)).map_err(HomeError::from)?;
        match h.unlock_recovery(&rk) {
            Ok(_) => self.update_meta(user, |m| m.recovery_confirmed = true),
            Err(_) => Err(HomeError::NotConfirmed("That is not the recovery key that was shown. Check it against what you wrote down, or make a new one.".into())),
        }
    }

    /// Make a new recovery key (the old one stops working). Needs the password; counts as saved only after it was typed back.
    pub fn change_recovery(&self, user: &str, password: &str) -> Result<RecoveryText, HomeError> {
        let mut vol = self.volume(user, &Secret::password(password))?;
        let (_, rk) = self.rng(|r| vol.change(|h, k| h.set_recovery(r, k)))?;
        self.update_meta(user, |m| m.recovery_confirmed = false)?;
        Ok(RecoveryText(Zeroizing::new(rk.to_text())))
    }

    // ---- passwords and slots ----------------------------------------------------------------------------------------

    fn person_slot(vol: &Volume<FileStore>) -> Result<usize, HomeError> {
        vol.header().list().into_iter().find(|(_, k, _)| *k == SlotKind::Person).map(|(i, _, _)| i).ok_or(HomeError::Volume("this home has no password slot".into()))
    }

    /// The sign-in password changed (the old one is known): the person's slot is re-wrapped, the data is not touched.
    pub fn change_password(&self, user: &str, old: &str, new: &str) -> Result<(), HomeError> {
        if new.is_empty() {
            return Err(HomeError::Volume("the new password is empty".into()));
        }
        let mut vol = self.volume(user, &Secret::password(old))?;
        let i = Self::person_slot(&vol)?;
        self.rng(|r| vol.change(|h, k| h.change_password(r, k, i, new.as_bytes(), self.kdf())))?;
        self.update_meta(user, |m| m.sealed_by_reset = false)
    }

    /// After a forgotten password: open with the recovery key (or the administrator slot's password) and set the new sign-in password.
    pub fn reset_password_with(&self, user: &str, secret: &Secret, new_password: &str) -> Result<(), HomeError> {
        if new_password.is_empty() {
            return Err(HomeError::Volume("the new password is empty".into()));
        }
        let mut vol = self.volume(user, secret)?;
        let i = Self::person_slot(&vol)?;
        self.rng(|r| vol.change(|h, k| h.change_password(r, k, i, new_password.as_bytes(), self.kdf())))?;
        self.update_meta(user, |m| m.sealed_by_reset = false)
    }

    /// Allow (`Some(administrator's password)`) or forbid (`None`) an administrator slot. Needs the person's password.
    pub fn set_admin_slot(&self, user: &str, password: &str, admin_password: Option<&str>) -> Result<(), HomeError> {
        let mut vol = self.volume(user, &Secret::password(password))?;
        let existing = vol.header().list().into_iter().find(|(_, k, _)| *k == SlotKind::Admin).map(|(i, _, _)| i);
        match (admin_password, existing) {
            (Some(ap), _) if ap.is_empty() => Err(HomeError::Volume("the administrator's password is empty".into())),
            (Some(ap), Some(i)) => self.rng(|r| vol.change(|h, k| h.change_password(r, k, i, ap.as_bytes(), self.kdf()))).map_err(Into::into),
            (Some(ap), None) => self.rng(|r| vol.change(|h, k| h.add_password(r, k, ap.as_bytes(), SlotKind::Admin, 0, self.kdf()))).map(|_| ()).map_err(Into::into),
            (None, Some(i)) => vol.change(|h, k| h.remove_slot(k, i)).map_err(Into::into),
            (None, None) => Ok(()),
        }
    }

    /// What the reset dialog must say before an administrator resets this person's password. `None` when there is nothing to warn about.
    pub fn reset_warning(&self, user: &str) -> Option<String> {
        let st = self.status(user);
        if st.state == State::NotSetUp {
            return None;
        }
        let how = match (st.has_admin_slot(), st.has_recovery_slot() && st.meta.recovery_confirmed) {
            (true, true) => "An administrator slot and the recovery key exist, so the folder can be opened again.",
            (true, false) => "An administrator slot exists, so an administrator can open it again.",
            (false, true) => "Only the person's recovery key can open it again.",
            (false, false) => "No recovery key was confirmed and there is no administrator slot: the folder would be lost for good.",
        };
        Some(format!("{user} has an encrypted home folder. Resetting the sign-in password does NOT change the key to that folder: it stays sealed until the recovery key or an administrator slot is used. {how}"))
    }

    // ---- sign-in, sign-out, reset hooks ----------------------------------------------------------------------------

    pub fn set_lock_with_screen(&self, user: &str, on: bool) -> Result<(), HomeError> {
        if !self.exists(user) {
            return Err(HomeError::NoHome);
        }
        self.update_meta(user, |m| m.lock_with_screen = on)
    }

    /// An administrator reset the sign-in password: remember that the new password does not open the home.
    pub fn note_admin_reset(&self, user: &str) {
        if self.exists(user) {
            let _ = self.update_meta(user, |m| m.sealed_by_reset = true);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unlock {
    Opened { repaired_header: bool },
    AlreadyOpen,
}

/// `EBUSY` on Linux (the kernel's answer for a mount or mapping that is still in use).
const EBUSY: i32 = 16;

// ---------------------------------------------------------------------------------------------------------------------
// the hooks the sign-in code calls (no-ops for an account without an encrypted home)
// ---------------------------------------------------------------------------------------------------------------------

/// Whether `<data>/home-enc/<user>.ivol` exists (one `stat`; the hooks below do nothing at all for an account without one).
pub fn has_container(data_dir: &Path, user: &str) -> bool {
    valid_account(user) && data_dir.join(DIR).join(format!("{user}.ivol")).is_file()
}

/// What sign-in learned about the private folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginHome {
    /// No encrypted home for this account.
    None,
    Opened,
    /// Signed in, but the private folder is still sealed; the words are for a banner.
    Sealed(String),
}

fn banners() -> &'static Mutex<HashMap<String, String>> {
    static B: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The banner text sign-in left for this account (taken once).
pub fn take_banner(user: &str) -> Option<String> {
    banners().lock().unwrap_or_else(|e| e.into_inner()).remove(user)
}

fn set_banner(user: &str, text: &str) {
    banners().lock().unwrap_or_else(|e| e.into_inner()).insert(user.to_string(), text.to_string());
}

impl EncHome {
    /// Sign-in: the password was just verified by the account code; open the private folder with it. A failure never stops the sign-in:
    /// the person gets a minimal session and a banner, and the plain `/home/<name>` is left unmounted-over (the folder stays sealed,
    /// an empty temporary home is not offered as if it were the real one: the banner says so).
    pub fn on_login(&self, user: &str, password: &str) -> LoginHome {
        if !self.exists(user) {
            return LoginHome::None;
        }
        match self.unlock(user, &Secret::password(password)) {
            Ok(_) => LoginHome::Opened,
            Err(e) => {
                let why = match e {
                    HomeError::WrongSecret => "Your private folder is sealed: your sign-in password no longer opens it (it was reset by an administrator?). Use your recovery key in Settings, Encrypted home.".to_string(),
                    other => format!("Your private folder could not be opened: {other}. You are signed in with a temporary, empty home; do not save anything important in it."),
                };
                set_banner(user, &why);
                LoginHome::Sealed(why)
            }
        }
    }

    /// Sign-out: close the private folder. Returns the words for a message when it could not be closed.
    pub fn on_logout(&self, user: &str) -> Option<String> {
        if !self.exists(user) || !self.is_open(user) {
            return None;
        }
        self.lock(user).err().map(|e| e.to_string())
    }

    /// The sign-in password was changed by the person (the old one is known).
    pub fn on_password_change(&self, user: &str, old: &str, new: &str) -> Option<String> {
        if !self.exists(user) {
            return None;
        }
        self.change_password(user, old, new).err().map(|e| format!("Your private folder's password could not be changed to the new one: {e}. It still opens with the old password or the recovery key."))
    }

    /// The screen locks: close the private folder when the person chose that.
    pub fn on_screen_lock(&self, user: &str) -> Option<String> {
        if !self.exists(user) || !self.meta(user).lock_with_screen || !self.is_open(user) {
            return None;
        }
        self.lock(user).err().map(|e| e.to_string())
    }

    /// The lock screen was unlocked with the password: open the folder again if the screen lock had closed it.
    pub fn on_screen_unlock(&self, user: &str, password: &str) -> Option<String> {
        if !self.exists(user) || !self.meta(user).lock_with_screen || self.is_open(user) {
            return None;
        }
        self.unlock(user, &Secret::password(password)).err().map(|e| e.to_string())
    }
}

/// `PAYLOAD_OFFSET` re-exported for the doc and the dm table in tests.
pub const CONTAINER_HEADER_BYTES: u64 = PAYLOAD_OFFSET;

// the entry points the agent's sign-in code calls: a real handle is made only when the account has a container
impl EncHome {
    pub fn on_login_if_any(data_dir: &Path, user: &str, password: &str) -> LoginHome {
        if !has_container(data_dir, user) && !data_dir.join(DIR).join(format!("{user}.building")).exists() {
            return LoginHome::None;
        }
        let env = EncHome::real(data_dir);
        let _ = migrate::recover_interrupted(&env, user);
        env.on_login(user, password)
    }

    pub fn on_logout_if_any(data_dir: &Path, user: &str) -> Option<String> {
        if !has_container(data_dir, user) {
            return None;
        }
        EncHome::real(data_dir).on_logout(user)
    }

    pub fn on_password_change_if_any(data_dir: &Path, user: &str, old: &str, new: &str) -> Option<String> {
        if !has_container(data_dir, user) {
            return None;
        }
        EncHome::real(data_dir).on_password_change(user, old, new)
    }

    /// The screen locks: closes the home when the person chose that (a busy home stays open; the words come back for a notice).
    pub fn on_screen_lock_if_any(data_dir: &Path, user: &str) -> Option<String> {
        if !has_container(data_dir, user) || !EncHome::for_status(data_dir).meta(user).lock_with_screen {
            return None;
        }
        EncHome::real(data_dir).on_screen_lock(user)
    }

    /// The lock screen opened with the password: opens the home again when the screen lock had closed it.
    pub fn on_screen_unlock_if_any(data_dir: &Path, user: &str, password: &str) -> Option<String> {
        if !has_container(data_dir, user) || !EncHome::for_status(data_dir).meta(user).lock_with_screen {
            return None;
        }
        EncHome::real(data_dir).on_screen_unlock(user, password)
    }

    pub fn note_admin_reset_if_any(data_dir: &Path, user: &str) {
        if has_container(data_dir, user) {
            EncHome::real(data_dir).note_admin_reset(user);
        }
    }
}

/// Files the system itself puts in a new home folder at sign-in (paths relative to the home): the tool settings file and its timestamped
/// backup. Nothing a person made. (Found on the Mac mini 2018: the first fix knew only the settings file, and a fresh account still
/// held `.claude/backups/.claude.json.backup.<number>`.)
fn is_sign_in_stub(rel: &str) -> bool {
    rel == ".claude/.claude.json"
        || rel.strip_prefix(".claude/backups/.claude.json.backup.").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Does this folder hold anything a person could have made? Empty folders (at any depth) and the sign-in stubs do not count.
fn holds_person_files(dir: &Path) -> std::io::Result<bool> {
    fn walk(root: &Path, dir: &Path) -> std::io::Result<bool> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            let t = e.file_type()?;
            if t.is_dir() {
                if walk(root, &path)? {
                    return Ok(true);
                }
            } else {
                let rel = path.strip_prefix(root).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
                if !t.is_file() || !is_sign_in_stub(&rel) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    walk(dir, dir)
}

/// Remove the sign-in stubs (only those, only plain files).
fn remove_sign_in_stubs(home: &Path) {
    fn walk(root: &Path, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                walk(root, &path);
            } else if t.is_file() {
                let rel = path.strip_prefix(root).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
                if is_sign_in_stub(&rel) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    walk(home, home);
}
