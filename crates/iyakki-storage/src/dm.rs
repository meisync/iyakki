//! Device-mapper encryption for container files (F-STORE-05, docs/development/DATA-VOLUME-ENCRYPTION.md).
//!
//! The one place that talks to `/dev/mapper/control`, the loop devices and `mount(2)` for an encrypted home (the kernel-access rule:
//! one shared module, no copies elsewhere). A container file is attached to a loop device, the volume key is handed to dm-crypt as
//! `aes-xts-plain64` with 4096-byte sectors, and the mapping is mounted. There is no `cryptsetup`: the table is loaded straight through
//! the `DM_TABLE_LOAD` ioctl, so the key lives only in this process's memory and in the kernel. It never appears on a command line,
//! in an environment variable, in a log line or in an error message, and the ioctl buffer is wiped after use (and flagged
//! `DM_SECURE_DATA_FLAG` so the kernel wipes its copy too).
//!
//! Everything goes through [`DmKernel`]: [`SysKernel`] is the real one (Linux only, needs root), [`fake::FakeKernel`] records every
//! ioctl payload and checks it, so the layout of the table and the order of the steps are tested without a kernel.
//! The real-kernel test (`dm_real_tests.rs`) runs only when `/dev/mapper/control` is usable and `IYAKKI_TEST_DM` is set.

use std::io;
use std::path::Path;

pub mod fake;
mod sys;

pub use sys::SysKernel;

/// Size of the kernel's `struct dm_ioctl` header.
pub const DM_IOCTL_LEN: usize = 312;
/// Size of `struct dm_target_spec`.
pub const DM_SPEC_LEN: usize = 40;
/// The interface version we speak (the kernel accepts any 4.x it knows).
pub const DM_VERSION: [u32; 3] = [4, 0, 0];

pub const DM_READONLY_FLAG: u32 = 1 << 0;
pub const DM_SUSPEND_FLAG: u32 = 1 << 1;
pub const DM_ACTIVE_PRESENT_FLAG: u32 = 1 << 5;
/// The kernel wipes the buffer it copied (the table holds the key).
pub const DM_SECURE_DATA_FLAG: u32 = 1 << 15;

/// Longest mapping name we use (the kernel allows 127).
pub const MAX_NAME: usize = 64;
/// Where the kernel makes the device node of mapping `name`.
pub fn map_path(name: &str) -> String {
    format!("/dev/mapper/{name}")
}

/// The device-mapper commands we use, with the kernel's command numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmCmd {
    Version = 0,
    DevCreate = 3,
    DevRemove = 4,
    /// With `DM_SUSPEND_FLAG` suspends; without it resumes (which also makes a freshly loaded table live).
    DevSuspend = 6,
    DevStatus = 7,
    TableLoad = 9,
}

impl DmCmd {
    /// `_IOWR(0xfd, nr, struct dm_ioctl)`.
    pub fn request(self) -> u64 {
        (3u64 << 30) | ((DM_IOCTL_LEN as u64) << 16) | (0xfdu64 << 8) | self as u64
    }

    pub fn from_request(r: u64) -> Option<DmCmd> {
        [DmCmd::Version, DmCmd::DevCreate, DmCmd::DevRemove, DmCmd::DevSuspend, DmCmd::DevStatus, DmCmd::TableLoad].into_iter().find(|c| c.request() == r)
    }
}

/// Everything the encryption layer needs from the kernel. One implementation talks to the kernel, one is a recording fake.
pub trait DmKernel: Send + Sync {
    /// One device-mapper ioctl. `buf` is the whole buffer (header first); the kernel's answer is written back into it.
    fn dm_ioctl(&self, cmd: DmCmd, buf: &mut Vec<u8>) -> io::Result<()>;
    /// Attach `file` to a free loop device (auto-clearing when the last user closes it) and return its path, for example `/dev/loop3`.
    fn loop_attach(&self, file: &Path) -> io::Result<String>;
    /// Detach a loop device.
    fn loop_detach(&self, dev: &str) -> io::Result<()>;
    /// Make an ext4 file system on a (mapped) device.
    fn make_fs(&self, dev: &str, label: &str) -> io::Result<()>;
    /// Mount an ext4 device on `target` (nosuid, nodev).
    fn mount(&self, dev: &str, target: &Path) -> io::Result<()>;
    fn umount(&self, target: &Path) -> io::Result<()>;
    /// Whether something is mounted exactly at `target`.
    fn is_mounted(&self, target: &Path) -> bool;
}

/// What can go wrong, in words a person can read. Never holds a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmError {
    BadName,
    BadRequest(&'static str),
    Kernel(String),
}

impl std::fmt::Display for DmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DmError::BadName => write!(f, "the mapping name is not allowed (letters, digits, - and _ only)"),
            DmError::BadRequest(w) => write!(f, "the request does not fit: {w}"),
            DmError::Kernel(e) => write!(f, "the kernel refused: {e}"),
        }
    }
}

impl std::error::Error for DmError {}

fn kerr(what: &str, e: io::Error) -> DmError {
    DmError::Kernel(format!("{what}: {e}"))
}

/// A mapping name is safe to put in a path and a table: lower-case letters, digits, `-`, `_`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

// ---------------------------------------------------------------------------------------------------------------------
// the ioctl buffers
// ---------------------------------------------------------------------------------------------------------------------

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_ne_bytes());
}
fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_ne_bytes());
}
pub fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}
pub fn get_u64(b: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(b[at..at + 8].try_into().unwrap())
}

/// The header alone (312 bytes), for commands that carry no table.
pub fn header(name: &str, flags: u32) -> Vec<u8> {
    let mut b = vec![0u8; DM_IOCTL_LEN];
    put_u32(&mut b, 0, DM_VERSION[0]);
    put_u32(&mut b, 4, DM_VERSION[1]);
    put_u32(&mut b, 8, DM_VERSION[2]);
    put_u32(&mut b, 12, DM_IOCTL_LEN as u32); // data_size
    put_u32(&mut b, 16, DM_IOCTL_LEN as u32); // data_start
    put_u32(&mut b, 28, flags);
    let n = name.as_bytes();
    b[48..48 + n.len()].copy_from_slice(n); // name[128], NUL padded by the zero fill
    b
}

/// The `TABLE_LOAD` buffer: the header, one target spec and its parameter string (NUL-terminated, padded to 8 bytes).
pub fn table_load_buf(name: &str, sectors: u64, target_type: &str, params: &str, flags: u32) -> Vec<u8> {
    let mut b = header(name, flags);
    let plen = (params.len() + 1).div_ceil(8) * 8;
    let spec_total = DM_SPEC_LEN + plen;
    let at = b.len();
    b.resize(at + spec_total, 0);
    put_u64(&mut b, at, 0); // sector_start
    put_u64(&mut b, at + 8, sectors); // length
    // status (i32) stays 0
    put_u32(&mut b, at + 20, spec_total as u32); // next
    let t = target_type.as_bytes();
    b[at + 24..at + 24 + t.len()].copy_from_slice(t);
    b[at + DM_SPEC_LEN..at + DM_SPEC_LEN + params.len()].copy_from_slice(params.as_bytes());
    let total = b.len() as u32;
    put_u32(&mut b, 12, total);
    put_u32(&mut b, 20, 1); // target_count
    b
}

/// A decoded `TABLE_LOAD` buffer (the fake kernel and the tests use this to check what would reach the kernel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLoad {
    pub name: String,
    pub flags: u32,
    pub start: u64,
    pub sectors: u64,
    pub target_type: String,
    pub params: String,
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|x| *x == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The name in a header.
pub fn header_name(b: &[u8]) -> Option<String> {
    (b.len() >= DM_IOCTL_LEN).then(|| cstr(&b[48..176]))
}

pub fn header_flags(b: &[u8]) -> u32 {
    if b.len() >= DM_IOCTL_LEN { get_u32(b, 28) } else { 0 }
}

/// Check and decode a `TABLE_LOAD` buffer the way the kernel would (sizes, counts, termination).
pub fn decode_table_load(b: &[u8]) -> Result<TableLoad, &'static str> {
    if b.len() < DM_IOCTL_LEN + DM_SPEC_LEN {
        return Err("buffer too short");
    }
    if get_u32(b, 0) != DM_VERSION[0] {
        return Err("wrong interface version");
    }
    if get_u32(b, 12) as usize != b.len() {
        return Err("data_size does not match the buffer");
    }
    if get_u32(b, 16) as usize != DM_IOCTL_LEN {
        return Err("data_start is wrong");
    }
    if get_u32(b, 20) != 1 {
        return Err("exactly one target is expected");
    }
    let at = DM_IOCTL_LEN;
    let next = get_u32(b, at + 20) as usize;
    if next < DM_SPEC_LEN + 1 || next % 8 != 0 || at + next != b.len() {
        return Err("the target spec does not end where the buffer ends");
    }
    let params = &b[at + DM_SPEC_LEN..];
    let end = params.iter().position(|x| *x == 0).ok_or("the parameters are not terminated")?;
    if params[end..].iter().any(|x| *x != 0) {
        return Err("bytes after the parameter string");
    }
    Ok(TableLoad {
        name: cstr(&b[48..176]),
        flags: get_u32(b, 28),
        start: get_u64(b, at),
        sectors: get_u64(b, at + 8),
        target_type: cstr(&b[at + 24..at + 40]),
        params: String::from_utf8_lossy(&params[..end]).into_owned(),
    })
}

/// Overwrite a buffer that held a key so the compiler cannot drop the writes.
pub fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: `x` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(x, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Hex text of a key, wiped when dropped.
struct HexKey(Vec<u8>);

impl HexKey {
    fn new(key: &[u8]) -> HexKey {
        const D: &[u8; 16] = b"0123456789abcdef";
        let mut v = Vec::with_capacity(key.len() * 2);
        for b in key {
            v.push(D[(b >> 4) as usize]);
            v.push(D[(b & 15) as usize]);
        }
        HexKey(v)
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl Drop for HexKey {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// What `DEV_STATUS` tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmStatus {
    /// How many users hold the device open (a mounted file system counts as one).
    pub open_count: i32,
    pub active: bool,
    pub suspended: bool,
}

// ---------------------------------------------------------------------------------------------------------------------
// the encryption layer
// ---------------------------------------------------------------------------------------------------------------------

pub const CIPHER: &str = "aes-xts-plain64";
/// The key length in bytes (two AES-256 keys for XTS).
pub const KEY_LEN: usize = 64;

/// The table line for one dm-crypt target: `<cipher> <hexkey> <iv_offset> <device> <offset> [1 sector_size:4096]`. The caller passes
/// the start/length separately (the kernel takes them from the target spec, but `dmsetup` prints them first, so the full line is
/// available as [`full_table_line`] for tests and for the doc).
pub fn crypt_params(key_hex: &str, dev: &str, offset_sectors: u64, sector_size: u32) -> Result<String, DmError> {
    if dev.is_empty() || dev.len() > 200 || dev.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(DmError::BadRequest("the device path is not usable"));
    }
    let tail = match sector_size {
        512 => String::new(),
        4096 => " 1 sector_size:4096".to_string(),
        _ => return Err(DmError::BadRequest("the sector size must be 512 or 4096")),
    };
    Ok(format!("{CIPHER} {key_hex} 0 {dev} {offset_sectors}{tail}"))
}

/// `0 <sectors> crypt aes-xts-plain64 <hexkey> 0 <dev> <offset> 1 sector_size:4096`
pub fn full_table_line(sectors: u64, params: &str) -> String {
    format!("0 {sectors} crypt {params}")
}

pub struct Crypt<'a> {
    k: &'a dyn DmKernel,
}

impl<'a> Crypt<'a> {
    pub fn new(k: &'a dyn DmKernel) -> Crypt<'a> {
        Crypt { k }
    }

    /// Create the mapping `name` over `dev` (a loop device or a partition): `sectors` is the length in 512-byte units, `offset_sectors`
    /// where the encrypted payload starts on `dev`, `sector_size` the data unit the volume was formatted with. On any failure after the
    /// device was created it is removed again, so a failed open leaves nothing behind.
    pub fn open(&self, name: &str, key: &[u8; KEY_LEN], dev: &str, offset_sectors: u64, sectors: u64, sector_size: u32) -> Result<(), DmError> {
        if !valid_name(name) {
            return Err(DmError::BadName);
        }
        let unit = (sector_size / 512) as u64;
        if sectors == 0 || (sector_size != 512 && sector_size != 4096) || offset_sectors % unit.max(1) != 0 || sectors % unit.max(1) != 0 {
            return Err(DmError::BadRequest("size and offset must be whole sectors"));
        }
        let hex = HexKey::new(key);
        let params = crypt_params(hex.as_str(), dev, offset_sectors, sector_size)?;
        let mut create = header(name, 0);
        self.k.dm_ioctl(DmCmd::DevCreate, &mut create).map_err(|e| kerr("create the mapping", e))?;
        let mut load = table_load_buf(name, sectors, "crypt", &params, DM_SECURE_DATA_FLAG);
        drop(params);
        let r = self.k.dm_ioctl(DmCmd::TableLoad, &mut load);
        wipe(&mut load);
        if let Err(e) = r {
            let _ = self.remove(name);
            return Err(kerr("load the encryption table", e));
        }
        let mut resume = header(name, 0);
        if let Err(e) = self.k.dm_ioctl(DmCmd::DevSuspend, &mut resume) {
            let _ = self.remove(name);
            return Err(kerr("start the mapping", e));
        }
        Ok(())
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        let mut b = header(name, 0);
        self.k.dm_ioctl(DmCmd::DevRemove, &mut b)
    }

    /// Remove the mapping (it must not be mounted or held open).
    pub fn close(&self, name: &str) -> Result<(), DmError> {
        if !valid_name(name) {
            return Err(DmError::BadName);
        }
        self.remove(name).map_err(|e| kerr("close the mapping", e))
    }

    /// `None` when there is no such mapping.
    pub fn status(&self, name: &str) -> Result<Option<DmStatus>, DmError> {
        if !valid_name(name) {
            return Err(DmError::BadName);
        }
        let mut b = header(name, 0);
        match self.k.dm_ioctl(DmCmd::DevStatus, &mut b) {
            Ok(()) => {
                let flags = header_flags(&b);
                Ok(Some(DmStatus { open_count: get_u32(&b, 24) as i32, active: flags & DM_ACTIVE_PRESENT_FLAG != 0, suspended: flags & DM_SUSPEND_FLAG != 0 }))
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENXIO) | Some(libc::ENODEV)) || e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(kerr("ask about the mapping", e)),
        }
    }
}

#[cfg(test)]
#[path = "dm_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "dm_real_tests.rs"]
mod real_tests;
