//! In-place conversion of a plain partition into an IYAKVOL1 volume (F-STORE-05, design section 12 item 6; the whole-partition form).
//!
//! The plain file system has been shrunk first (the installer's shrink step) so that the last `gap + journal` bytes of the partition are
//! free. The data is then encrypted where it lies and moved UP by `gap` bytes, from the last sector to the first, so that the header
//! area (`gap` bytes at the start) is free at the end:
//!
//! ```text
//! before:  [ plain sectors 0 .. n ........................... ][ free: gap + journal ]
//! after:   [ header A, B (gap bytes) ][ encrypted sectors 0 .. n ][ free ][ journal (zeroed) ]
//! ```
//!
//! Crash safety. A batch moves `gap / sector` sectors `[a, b)` to the places `[a + h, b + h)` (h = gap in sectors) and then the
//! journal says "everything at or above `a` is moved". A batch never overwrites a sector that the journal does not already list as
//! moved (`a + h >= b`) and never overwrites its own source (the batch is at most `h` sectors), so a cut at ANY write leaves every
//! plain sector readable from its old place or its new one, and starting the same batch again gives the same bytes. The journal
//! lives in the last 12 KiB of the partition: a copy of the finished header (so a resume needs only the password), and two progress
//! blocks written alternately (each with its own checksum), so a torn write damages at most one of them. Nothing of the header area is
//! written until the last sector is moved; the volume appears in one step (header B, then A), and the journal is wiped last.

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::container::Store;
use crate::header::{Header, KdfParams, NewVolume, SlotKind, HEADER_LEN, KEY_LEN};
use crate::xts::Xts;
use crate::{Random, VolError};

/// The journal at the end of the partition: header copy, progress block A, progress block B (4096 bytes each).
pub const JOURNAL_LEN: u64 = 3 * 4096;
const PROGRESS_MAGIC: &[u8; 8] = b"IYAKPRG1";
/// The smallest gap: the two header copies.
pub const MIN_GAP: u64 = 2 * HEADER_LEN as u64;
/// The gap the installer leaves by default (one MiB: big batches, few flushes).
pub const DEFAULT_GAP: u64 = 1 << 20;

/// What to convert.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub sector_size: u32,
    /// Bytes of plain data to keep (the shrunk file system's size, a multiple of the sector size).
    pub plain_bytes: u64,
    /// Bytes reserved at the start for the header copies (a multiple of 4096 and of the sector size, at least [`MIN_GAP`]).
    pub gap_bytes: u64,
}

impl Plan {
    fn check(&self, store_len: u64) -> Result<(), VolError> {
        let (ss, g) = (self.sector_size as u64, self.gap_bytes);
        if self.sector_size != 512 && self.sector_size != 4096 {
            return Err(VolError::BadRequest("the sector size must be 512 or 4096"));
        }
        if self.plain_bytes == 0 || self.plain_bytes % ss != 0 {
            return Err(VolError::BadRequest("the data size must be a whole number of sectors"));
        }
        if g < MIN_GAP || g % 4096 != 0 || g % ss != 0 {
            return Err(VolError::BadRequest("the header gap must be at least 8 KiB and a multiple of 4096 and of the sector size"));
        }
        if store_len < g + self.plain_bytes + JOURNAL_LEN {
            return Err(VolError::BadRequest("the partition has no room left for the header gap and the journal: shrink the file system further first"));
        }
        Ok(())
    }
    fn sectors(&self) -> u64 {
        self.plain_bytes / self.sector_size as u64
    }
    fn gap_sectors(&self) -> u64 {
        self.gap_bytes / self.sector_size as u64
    }
}

/// Where a store stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Neither a volume nor a conversion: a plain partition (or anything else).
    NotStarted,
    /// A conversion was cut: `moved` of `total` sectors are done; resume with the password.
    InProgress { moved: u64, total: u64 },
    /// The volume is complete but the journal was not wiped yet (a cut at the very end): resuming only cleans up.
    Finished,
    /// A volume with nothing left to do.
    Converted,
}

fn journal_at<S: Store>(s: &S) -> Result<u64, VolError> {
    s.len()?.checked_sub(JOURNAL_LEN).ok_or(VolError::BadRequest("the partition is too small"))
}

fn progress_bytes(generation: u64, k: u64, n: u64, uuid: &[u8; 16]) -> Vec<u8> {
    let mut b = vec![0u8; 4096];
    b[..8].copy_from_slice(PROGRESS_MAGIC);
    b[8..16].copy_from_slice(&generation.to_le_bytes());
    b[16..24].copy_from_slice(&k.to_le_bytes());
    b[24..32].copy_from_slice(&n.to_le_bytes());
    b[32..48].copy_from_slice(uuid);
    let sum = Sha256::digest(&b[..48]);
    b[48..80].copy_from_slice(&sum);
    b
}

fn parse_progress(b: &[u8], uuid: &[u8; 16]) -> Option<(u64, u64, u64)> {
    if b.len() < 80 || &b[..8] != PROGRESS_MAGIC || Sha256::digest(&b[..48]).as_slice() != &b[48..80] || &b[32..48] != uuid {
        return None;
    }
    let u = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    Some((u(8), u(16), u(24)))
}

/// The journal's header copy and the best progress block, if the store has a conversion in it.
struct Journal {
    header: Header,
    generation: u64,
    k: u64,
    n: u64,
}

fn read_journal<S: Store>(s: &S) -> Result<Option<Journal>, VolError> {
    let at = journal_at(s)?;
    let mut hb = vec![0u8; HEADER_LEN];
    s.read_at(at, &mut hb)?;
    let Ok(header) = Header::parse(&hb) else { return Ok(None) };
    let mut best: Option<(u64, u64, u64)> = None;
    for slot in 0..2u64 {
        let mut b = vec![0u8; 4096];
        s.read_at(at + 4096 * (1 + slot), &mut b)?;
        if let Some(p) = parse_progress(&b, &header.uuid) {
            if best.is_none_or(|x| p.0 > x.0) {
                best = Some(p);
            }
        }
    }
    let (generation, k, n) = best.ok_or(VolError::Damaged)?;
    Ok(Some(Journal { header, generation, k, n }))
}

fn volume_header_at_start<S: Store>(s: &S) -> Option<Header> {
    let mut b = vec![0u8; HEADER_LEN];
    s.read_at(0, &mut b).ok()?;
    Header::parse(&b).ok()
}

/// What the store holds.
pub fn state<S: Store>(s: &S) -> Result<State, VolError> {
    let journal = read_journal(s).ok().flatten();
    let start = volume_header_at_start(s);
    Ok(match (journal, start) {
        (Some(j), Some(h)) if h.uuid == j.header.uuid => State::Finished,
        (Some(j), _) => State::InProgress { moved: j.n - j.k, total: j.n },
        (None, Some(_)) => State::Converted,
        (None, None) => State::NotStarted,
    })
}

/// Something the caller wants to see while a conversion runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub moved: u64,
    pub total: u64,
}

/// Begin: make the header (one password slot, random volume key), write the journal, and then move the data. Refuses a store that already
/// holds a volume or a conversion. The first destructive write happens only after the journal is safely on the drive.
pub fn convert<S: Store>(store: &mut S, rng: &mut dyn Random, plan: &Plan, password: &[u8], kind: SlotKind, tag: u32, kdf: KdfParams, on_progress: &mut dyn FnMut(Progress)) -> Result<(), VolError> {
    plan.check(store.len()?)?;
    if state(store)? != State::NotStarted {
        return Err(VolError::BadRequest("this partition already holds an encrypted volume or an unfinished conversion"));
    }
    let (header, key) = Header::create(rng, &NewVolume { sector_size: plan.sector_size, payload_bytes: plan.plain_bytes, payload_offset: plan.gap_bytes }, password, kind, tag, kdf)?;
    let at = journal_at(store)?;
    store.write_at(at, &header.to_bytes())?;
    let n = plan.sectors();
    store.write_at(at + 4096, &progress_bytes(1, n, n, &header.uuid))?;
    store.write_at(at + 8192, &progress_bytes(1, n, n, &header.uuid))?;
    store.flush()?;
    run(store, plan, &header, &key, n, 1, on_progress)
}

/// Continue a cut conversion with the password (or finish the clean-up of a complete one).
pub fn resume<S: Store>(store: &mut S, password: &[u8], on_progress: &mut dyn FnMut(Progress)) -> Result<(), VolError> {
    let j = read_journal(store)?.ok_or(VolError::BadRequest("there is no conversion to resume on this partition"))?;
    let unlocked = j.header.unlock_password(password)?;
    let h = &j.header;
    let plan = Plan { sector_size: h.sector_size, plain_bytes: h.payload_bytes, gap_bytes: h.payload_offset };
    plan.check(store.len()?)?;
    if j.n != plan.sectors() {
        return Err(VolError::Invalid("the journal and the header disagree"));
    }
    run(store, &plan, &j.header, &unlocked.key, j.k, j.generation, on_progress)
}

fn run<S: Store>(store: &mut S, plan: &Plan, header: &Header, key: &Zeroizing<[u8; KEY_LEN]>, mut k: u64, mut generation: u64, on_progress: &mut dyn FnMut(Progress)) -> Result<(), VolError> {
    let at = journal_at(store)?;
    let (ss, n, h) = (plan.sector_size as u64, plan.sectors(), plan.gap_sectors());
    let xts = Xts::new(key);
    let mut buf = Zeroizing::new(vec![0u8; (h * ss) as usize]);
    while k > 0 {
        let a = k.saturating_sub(h);
        let len = ((k - a) * ss) as usize;
        store.read_at(a * ss, &mut buf[..len])?;
        for (i, sec) in buf[..len].chunks_exact_mut(ss as usize).enumerate() {
            xts.encrypt_sector((a + i as u64) * (ss / 512), sec);
        }
        store.write_at((a + h) * ss, &buf[..len])?;
        store.flush()?;
        generation += 1;
        k = a;
        store.write_at(at + 4096 * (1 + generation % 2), &progress_bytes(generation, k, n, &header.uuid))?;
        store.flush()?;
        on_progress(Progress { moved: n - k, total: n });
    }
    // every sector is moved: the volume appears (B first, then A, as `Volume::format` does), then the journal goes
    let hb = header.to_bytes();
    // the part of the gap beyond the two header copies still holds the first plain sectors: wipe it before the volume appears
    if plan.gap_bytes > MIN_GAP {
        store.write_at(MIN_GAP, &vec![0u8; (plan.gap_bytes - MIN_GAP) as usize])?;
    }
    store.write_at(HEADER_LEN as u64, &hb)?;
    store.write_at(0, &hb)?;
    store.flush()?;
    wipe_journal(store)
}

fn wipe_journal<S: Store>(store: &mut S) -> Result<(), VolError> {
    let at = journal_at(store)?;
    // the progress blocks first: a cut here leaves a header copy without progress, which reads as "no conversion"
    store.write_at(at + 4096, &vec![0u8; 4096])?;
    store.write_at(at + 8192, &vec![0u8; 4096])?;
    store.write_at(at, &vec![0u8; 4096])?;
    store.flush()
}

/// Clean up a finished volume whose journal was left behind (see [`State::Finished`]).
pub fn finish_cleanup<S: Store>(store: &mut S) -> Result<(), VolError> {
    if state(store)? != State::Finished {
        return Err(VolError::BadRequest("there is nothing to clean up"));
    }
    wipe_journal(store)
}
