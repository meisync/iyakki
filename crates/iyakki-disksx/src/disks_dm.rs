//! Unlocking a LUKS drive without `cryptsetup` (F-DISKS-02): our own reader of the LUKS1 and LUKS2 headers (`iyakki-volcrypt`) finds the
//! master key from the passphrase, and the key goes to the kernel straight through device-mapper (`iyakki_storage::dm::Crypt`) as a
//! `crypt` target over the partition. Nothing GPL is needed for the volumes it understands (`aes-xts-plain64` with a 128 or 256 bit
//! key, PBKDF2 or Argon2 slots); anything else comes back as [`DmOpenError::Unsupported`] and the caller may still use `cryptsetup`
//! when the person has downloaded it. The key lives in memory that is wiped on drop and in the kernel's table buffer, which is flagged
//! so the kernel wipes its copy; it is never in an argument, a file or a log.

use std::io::{Read, Seek, SeekFrom};

use iyakki_storage::dm::{self, Crypt, DmError, DmKernel};
use iyakki_volcrypt::luks1::{self, LuksError};
use iyakki_volcrypt::luks2;

/// Why an open did not happen.
#[derive(Debug, PartialEq, Eq)]
pub enum DmOpenError {
    /// The passphrase does not open any key slot.
    WrongKey,
    /// This reader does not do this volume (another cipher, a hash or a slot kind it does not know): not a failure of the person.
    Unsupported(String),
    Failed(String),
}

/// Reads bytes of the partition and tells its length.
pub trait Device {
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>>;
    fn len_bytes(&self) -> Option<u64>;
}

/// The real partition (`/dev/sdb1`): read with ordinary file reads, length from the block device ioctl.
pub struct BlockFile {
    path: String,
}

impl BlockFile {
    pub fn new(path: &str) -> BlockFile {
        BlockFile { path: path.to_string() }
    }
}

impl Device for BlockFile {
    fn read_at(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        if len > 64 << 20 {
            return None;
        }
        let mut f = std::fs::File::open(&self.path).ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        let mut v = vec![0u8; len];
        f.read_exact(&mut v).ok()?;
        Some(v)
    }

    fn len_bytes(&self) -> Option<u64> {
        use std::os::fd::AsRawFd;
        let f = std::fs::File::open(&self.path).ok()?;
        let m = f.metadata().ok()?;
        if m.is_file() {
            return Some(m.len());
        }
        let mut n: u64 = 0;
        // BLKGETSIZE64
        let r = unsafe { libc::ioctl(f.as_raw_fd(), 0x8008_1272_u64 as _, &mut n as *mut u64) };
        (r == 0 && n > 0).then_some(n)
    }
}

fn map(e: LuksError) -> DmOpenError {
    match e {
        LuksError::WrongPassword => DmOpenError::WrongKey,
        LuksError::Unsupported(w) => DmOpenError::Unsupported(w),
        other => DmOpenError::Failed(format!("the volume's header could not be read: {other:?}")),
    }
}

/// Open the LUKS volume on `dev` (named `devpath`, for example `/dev/sdb1`) as the mapping `name`.
pub fn open_luks(k: &dyn DmKernel, dev: &dyn Device, devpath: &str, name: &str, password: &[u8], read_only: bool) -> Result<(), DmOpenError> {
    let head = dev.read_at(0, 4096).ok_or_else(|| DmOpenError::Failed("the drive could not be read".into()))?;
    let (key, offset_bytes, size_bytes, sector_size) = match head.get(6..8) {
        Some([0, 1]) => {
            let h = luks1::parse_header(&head).map_err(map)?;
            let key = h.unlock(password, &mut |sector, n| dev.read_at(sector * 512, n)).map_err(map)?;
            let off = h.payload_sector as u64 * 512;
            (key, off, None, 512u32)
        }
        Some([0, 2]) => {
            let size = luks2::header_size(&head).map_err(map)? as usize;
            let all = dev.read_at(0, size).ok_or_else(|| DmOpenError::Failed("the drive could not be read".into()))?;
            let h = luks2::parse_header(&all).map_err(map)?;
            let seg = h.data_segment().ok_or_else(|| DmOpenError::Unsupported("the volume has no plain data segment".into()))?.clone();
            let key = h.unlock(password, &mut |off, n| dev.read_at(off, n)).map_err(map)?;
            (key, seg.offset, seg.size, seg.sector_size)
        }
        _ => return Err(DmOpenError::Unsupported("not a LUKS volume".into())),
    };
    if key.len() != 32 && key.len() != 64 {
        return Err(DmOpenError::Unsupported(format!("a {} bit key", key.len() * 8)));
    }
    let total = dev.len_bytes().ok_or_else(|| DmOpenError::Failed("the size of the drive is not known".into()))?;
    let size = match size_bytes {
        Some(s) => s,
        None => total.checked_sub(offset_bytes).ok_or_else(|| DmOpenError::Failed("the volume is shorter than its header".into()))?,
    };
    if offset_bytes + size > total || size == 0 {
        return Err(DmOpenError::Failed("the volume's data lies outside the drive".into()));
    }
    let unit = sector_size as u64;
    let sectors = size / 512 / (unit / 512) * (unit / 512);
    Crypt::new(k).open_key(name, &key, devpath, offset_bytes / 512, sectors, sector_size, read_only).map_err(|e: DmError| DmOpenError::Failed(e.to_string()))
}

/// Close a mapping this module (or `cryptsetup`) made. Ok(false) when there is no such mapping.
pub fn close(k: &dyn DmKernel, name: &str) -> Result<bool, String> {
    let c = Crypt::new(k);
    match c.status(name).map_err(|e| e.to_string())? {
        None => Ok(false),
        Some(_) => c.close(name).map(|()| true).map_err(|e| e.to_string()),
    }
}

/// Is there a mapping of this name?
pub fn is_open(k: &dyn DmKernel, name: &str) -> bool {
    dm::valid_name(name) && Crypt::new(k).status(name).ok().flatten().is_some()
}

#[cfg(test)]
#[path = "disks_dm_tests.rs"]
mod tests;
