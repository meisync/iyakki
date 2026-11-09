//! A volume on a byte store (a file or memory): `[header copy A, 4096][header copy B, 4096][encrypted payload]`.
//!
//! A change (new password, new slot) is written to the copy that is NOT in use, read back and compared, and only then counts; the
//! newer copy wins when the volume is opened. A power cut at any moment therefore leaves one whole, older or newer header, never a
//! half-written one. Sectors are encrypted with AES-256-XTS, tweak in 512-byte units, so dm-crypt can later open the same payload
//! with the same key (`aes-xts-plain64`, offset = payload offset).

use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::header::{Header, KdfParams, NewVolume, SlotKind, Unlocked, HEADER_LEN, KEY_LEN};
use crate::recovery::RecoveryKey;
use crate::xts::Xts;
use crate::{Random, VolError};

/// Where the payload starts in a container: after the two header copies.
pub const PAYLOAD_OFFSET: u64 = 2 * HEADER_LEN as u64;

/// Anything that can be read and written at byte positions.
pub trait Store {
    fn len(&self) -> Result<u64, VolError>;
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), VolError>;
    fn write_at(&mut self, pos: u64, buf: &[u8]) -> Result<(), VolError>;
    fn flush(&mut self) -> Result<(), VolError>;
}

/// A file (Unix positioned reads and writes).
pub struct FileStore(pub std::fs::File);

impl Store for FileStore {
    fn len(&self) -> Result<u64, VolError> {
        self.0.metadata().map(|m| m.len()).map_err(|e| VolError::Io(e.to_string()))
    }
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), VolError> {
        use std::os::unix::fs::FileExt;
        self.0.read_exact_at(buf, pos).map_err(|e| VolError::Io(e.to_string()))
    }
    fn write_at(&mut self, pos: u64, buf: &[u8]) -> Result<(), VolError> {
        use std::os::unix::fs::FileExt;
        self.0.write_all_at(buf, pos).map_err(|e| VolError::Io(e.to_string()))
    }
    fn flush(&mut self) -> Result<(), VolError> {
        self.0.sync_all().map_err(|e| VolError::Io(e.to_string()))
    }
}

/// Memory, with an optional fault: after `fail_after` more bytes written, every write fails (a power cut in the middle of a write
/// leaves the bytes written so far).
#[derive(Clone, Default)]
pub struct MemStore {
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub fail_after: Option<usize>,
}

impl MemStore {
    pub fn new(len: usize) -> MemStore {
        MemStore { bytes: Arc::new(Mutex::new(vec![0; len])), fail_after: None }
    }
}

impl Store for MemStore {
    fn len(&self) -> Result<u64, VolError> {
        Ok(self.bytes.lock().unwrap().len() as u64)
    }
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), VolError> {
        let b = self.bytes.lock().unwrap();
        let end = pos.checked_add(buf.len() as u64).ok_or_else(|| VolError::Io("read past the end".into()))?;
        if end > b.len() as u64 {
            return Err(VolError::Io("read past the end".into()));
        }
        buf.copy_from_slice(&b[pos as usize..end as usize]);
        Ok(())
    }
    fn write_at(&mut self, pos: u64, buf: &[u8]) -> Result<(), VolError> {
        let mut b = self.bytes.lock().unwrap();
        let end = pos.checked_add(buf.len() as u64).ok_or_else(|| VolError::Io("write past the end".into()))?;
        if end > b.len() as u64 {
            return Err(VolError::Io("write past the end".into()));
        }
        let mut n = buf.len();
        if let Some(left) = self.fail_after {
            n = n.min(left);
            self.fail_after = Some(left - n);
        }
        b[pos as usize..pos as usize + n].copy_from_slice(&buf[..n]);
        if n < buf.len() {
            return Err(VolError::Io("the drive stopped answering".into()));
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), VolError> {
        Ok(())
    }
}

/// An open (unlocked) volume.
pub struct Volume<S: Store> {
    store: S,
    header: Header,
    xts: Xts,
    key: Zeroizing<[u8; KEY_LEN]>,
    /// Which copy holds the current header: 0 = A, 1 = B.
    active: u8,
    /// True when the other copy was unreadable or older than it should be when opened; the next change writes it.
    pub other_copy_bad: bool,
}

/// Total bytes a container with `payload_bytes` of data needs.
pub fn container_len(payload_bytes: u64) -> u64 {
    PAYLOAD_OFFSET + payload_bytes
}

fn read_copy<S: Store>(s: &S, which: u8) -> Result<Header, VolError> {
    let mut b = vec![0u8; HEADER_LEN];
    s.read_at(which as u64 * HEADER_LEN as u64, &mut b)?;
    Header::parse(&b)
}

impl<S: Store> Volume<S> {
    /// Make a new volume in `store` (which must be at least `container_len(payload_bytes)` long). Writes both header copies. The
    /// payload is not touched (it reads as noise until written, which is normal).
    pub fn format(mut store: S, rng: &mut dyn Random, sector_size: u32, payload_bytes: u64, password: &[u8], kind: SlotKind, tag: u32, kdf: KdfParams) -> Result<Volume<S>, VolError> {
        if store.len()? < container_len(payload_bytes) {
            return Err(VolError::BadRequest("the drive is smaller than the volume"));
        }
        let (header, key) = Header::create(rng, &NewVolume { sector_size, payload_bytes, payload_offset: PAYLOAD_OFFSET }, password, kind, tag, kdf)?;
        let b = header.to_bytes();
        // B first, then A: a cut after the first write leaves the old (empty) store unreadable, which is a format that never finished
        store.write_at(HEADER_LEN as u64, &b)?;
        store.write_at(0, &b)?;
        store.flush()?;
        let xts = Xts::new(&key);
        Ok(Volume { store, header, xts, key, active: 0, other_copy_bad: false })
    }

    fn pick(store: &S) -> Result<(Header, u8, bool), VolError> {
        let (a, b) = (read_copy(store, 0), read_copy(store, 1));
        match (a, b) {
            (Ok(a), Ok(b)) => {
                if a.uuid != b.uuid {
                    return Err(VolError::Invalid("the two header copies belong to different volumes"));
                }
                if b.generation > a.generation {
                    Ok((b, 1, false))
                } else {
                    Ok((a, 0, false))
                }
            }
            (Ok(a), Err(_)) => Ok((a, 0, true)),
            (Err(_), Ok(b)) => Ok((b, 1, true)),
            // both bad: report the more useful of the two reasons (not-ours beats damaged)
            (Err(ea), Err(eb)) => Err(if ea == VolError::NotOurs || eb == VolError::NotOurs { VolError::NotOurs } else { ea }),
        }
    }

    fn finish_open(store: S, header: Header, u: Unlocked, active: u8, bad: bool) -> Result<Volume<S>, VolError> {
        if store.len()? < container_len(header.payload_bytes) {
            return Err(VolError::Invalid("the drive is smaller than the volume says"));
        }
        let xts = Xts::new(&u.key);
        Ok(Volume { store, header, xts, key: u.key, active, other_copy_bad: bad })
    }

    /// Open with a password.
    pub fn open_password(store: S, password: &[u8]) -> Result<Volume<S>, VolError> {
        let (h, active, bad) = Self::pick(&store)?;
        let u = h.unlock_password(password)?;
        Self::finish_open(store, h, u, active, bad)
    }

    /// Open with the recovery key.
    pub fn open_recovery(store: S, rk: &RecoveryKey) -> Result<Volume<S>, VolError> {
        let (h, active, bad) = Self::pick(&store)?;
        let u = h.unlock_recovery(rk)?;
        Self::finish_open(store, h, u, active, bad)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn sector_size(&self) -> usize {
        self.header.sector_size as usize
    }

    pub fn sectors(&self) -> u64 {
        self.header.payload_bytes / self.header.sector_size as u64
    }

    /// The volume key, for handing to dm-crypt in a later slice. Handle with care; it is wiped when dropped.
    pub fn key(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    pub fn into_store(self) -> S {
        self.store
    }

    fn span(&self, first: u64, len: usize) -> Result<(), VolError> {
        let ss = self.sector_size();
        if len == 0 || len % ss != 0 {
            return Err(VolError::BadRequest("a whole number of sectors is needed"));
        }
        let n = (len / ss) as u64;
        if first.checked_add(n).is_none_or(|e| e > self.sectors()) {
            return Err(VolError::BadRequest("past the end of the volume"));
        }
        Ok(())
    }

    /// Write whole sectors starting at sector `first`.
    pub fn write_sectors(&mut self, first: u64, data: &[u8]) -> Result<(), VolError> {
        self.span(first, data.len())?;
        let ss = self.sector_size();
        let mut buf = Zeroizing::new(data.to_vec());
        for (i, sec) in buf.chunks_exact_mut(ss).enumerate() {
            self.xts.encrypt_sector((first + i as u64) * (ss as u64 / 512), sec);
        }
        self.store.write_at(self.header.payload_offset + first * ss as u64, &buf)
    }

    /// Read whole sectors starting at sector `first`.
    pub fn read_sectors(&self, first: u64, out: &mut [u8]) -> Result<(), VolError> {
        self.span(first, out.len())?;
        let ss = self.sector_size();
        self.store.read_at(self.header.payload_offset + first * ss as u64, out)?;
        for (i, sec) in out.chunks_exact_mut(ss).enumerate() {
            self.xts.decrypt_sector((first + i as u64) * (ss as u64 / 512), sec);
        }
        Ok(())
    }

    /// Change the slot table with `f` (add a slot, change a password, ...) and store the result safely in the unused copy.
    pub fn change<R>(&mut self, f: impl FnOnce(&mut Header, &[u8; KEY_LEN]) -> Result<R, VolError>) -> Result<R, VolError> {
        let mut h = self.header.clone();
        let r = f(&mut h, &self.key)?;
        let bytes = h.to_bytes();
        let target = 1 - self.active;
        let at = target as u64 * HEADER_LEN as u64;
        self.store.write_at(at, &bytes)?;
        self.store.flush()?;
        let mut back = vec![0u8; HEADER_LEN];
        self.store.read_at(at, &mut back)?;
        if back != bytes {
            return Err(VolError::Io("the header read back differently from what was written".into()));
        }
        self.header = h;
        self.active = target;
        self.other_copy_bad = true; // the old copy is one generation behind until the next change overwrites it
        Ok(r)
    }

    /// Bring the second header copy up to date (after a repair-worthy open or a change), so both copies are current.
    pub fn sync_copies(&mut self) -> Result<(), VolError> {
        let bytes = self.header.to_bytes();
        let other = (1 - self.active) as u64 * HEADER_LEN as u64;
        self.store.write_at(other, &bytes)?;
        self.store.flush()?;
        self.other_copy_bad = false;
        Ok(())
    }
}
