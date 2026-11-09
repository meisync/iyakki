//! The volume header: one 4096-byte block with up to eight key slots. Every slot wraps the same random 512-bit volume key (the
//! XTS key) under its own key-encryption key, from a password through Argon2id or from the recovery key. Changing a password
//! therefore rewrites one slot and never touches the data.
//!
//! Layout (all numbers little-endian):
//! ```text
//!    0  magic "IYAKVOL1"                      8  format version (1)        12  header length (4096)
//!   16  generation u64                       24  volume uuid (16)          40  sector size u32 (512 or 4096)
//!   44  cipher u32 (1 = aes-xts-plain64, 64-byte key)                      48  payload offset u64   56  payload length u64
//!   64..128 zero
//!  128  8 slots of 128 bytes: kind u32 (0 empty, 1 person, 2 recovery, 3 admin), m KiB u32, t u32, p u32, tag u32, 0 u32,
//!       salt 32, wrapped key 72
//! 1152  header MAC (HMAC-SHA256 under a key made from the volume key)       1184  SHA-256 of bytes 0..1184      rest zero
//! ```
//! The SHA-256 at the end tells a damaged header from a wrong password. The MAC tells, once the key is known, that nobody edited the
//! slot table (for instance removed a slot or weakened the cost) without it. Every unused or reserved byte must be zero, so nothing
//! can be hidden in the header.

use std::cell::Cell;

use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::kw;
use crate::mac::{ct_eq, hmac_sha256};
use crate::recovery::RecoveryKey;
use crate::{Random, VolError};

pub const HEADER_LEN: usize = 4096;
pub const SLOTS: usize = 8;
pub const KEY_LEN: usize = 64;
pub const WRAPPED_LEN: usize = KEY_LEN + 8;
const MAGIC: &[u8; 8] = b"IYAKVOL1";
const SLOT_AT: usize = 128;
const SLOT_LEN: usize = 128;
const MAC_AT: usize = SLOT_AT + SLOTS * SLOT_LEN;
const SUM_AT: usize = MAC_AT + 32;
const BODY_END: usize = SUM_AT + 32;
/// Most memory a header may ask the checker to use (1 GiB); a hostile header cannot make unlocking exhaust the computer.
pub const MAX_M_KIB: u32 = 1 << 20;
pub const MAX_T: u32 = 16;
pub const MAX_P: u32 = 8;
/// What new volumes must at least use (the design's floor: 64 MiB, 3 passes, 2 lanes). Tests build with smaller costs on purpose.
pub const FLOOR: KdfParams = KdfParams { m_kib: 64 * 1024, t: 3, p: 2 };

thread_local! {
    static KDF_RUNS: Cell<u32> = const { Cell::new(0) };
}

/// How many Argon2 runs this thread did so far (a test hook: unlocking must do the same work for a right and a wrong password).
pub fn kdf_runs() -> u32 {
    KDF_RUNS.with(|c| c.get())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    fn valid(&self) -> bool {
        (1..=MAX_P).contains(&self.p) && (1..=MAX_T).contains(&self.t) && self.m_kib >= 8 * self.p && self.m_kib <= MAX_M_KIB
    }

    /// Meets the design floor (used when a person makes a real volume; the format itself accepts less so tests stay fast).
    pub fn meets_floor(&self) -> bool {
        self.m_kib >= FLOOR.m_kib && self.t >= FLOOR.t && self.p >= 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Person = 1,
    Recovery = 2,
    Admin = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub kind: SlotKind,
    /// Zero for a recovery slot.
    pub kdf: Option<KdfParams>,
    /// Which account the slot belongs to (0 = none); only a label for Settings, never a secret.
    pub tag: u32,
    salt: [u8; 32],
    wrapped: [u8; WRAPPED_LEN],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Rises by one with every change; the newer of the two header copies wins.
    pub generation: u64,
    pub uuid: [u8; 16],
    pub sector_size: u32,
    pub payload_offset: u64,
    pub payload_bytes: u64,
    pub slots: [Option<Slot>; SLOTS],
    mac: [u8; 32],
}

/// The volume key and the slot that opened it.
pub struct Unlocked {
    pub key: Zeroizing<[u8; KEY_LEN]>,
    pub slot: usize,
}

fn u32at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Argon2id of `password` into a 256-bit key-encryption key. Refuses costs outside the limits.
pub fn derive_kek(password: &[u8], salt: &[u8; 32], k: &KdfParams) -> Result<Zeroizing<[u8; 32]>, VolError> {
    if k.m_kib > MAX_M_KIB {
        return Err(VolError::KdfTooBig(k.m_kib / 1024));
    }
    if !k.valid() {
        return Err(VolError::Invalid("password cost out of range"));
    }
    let params = Params::new(k.m_kib, k.t, k.p, Some(32)).map_err(|_| VolError::Invalid("password cost out of range"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    KDF_RUNS.with(|c| c.set(c.get() + 1));
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, &mut *out)
        .map_err(|_| VolError::Invalid("password cost out of range"))?;
    Ok(out)
}

/// What a new volume looks like.
pub struct NewVolume {
    pub sector_size: u32,
    /// Bytes of encrypted data after the header area; a multiple of the sector size.
    pub payload_bytes: u64,
    /// Where the data starts (the container puts two header copies before it: 8192).
    pub payload_offset: u64,
}

impl Header {
    fn check_geometry(sector_size: u32, payload_offset: u64, payload_bytes: u64) -> Result<(), VolError> {
        if sector_size != 512 && sector_size != 4096 {
            return Err(VolError::Invalid("sector size"));
        }
        let s = sector_size as u64;
        if payload_offset < HEADER_LEN as u64 || payload_offset % s != 0 || payload_bytes == 0 || payload_bytes % s != 0 {
            return Err(VolError::Invalid("payload position or size"));
        }
        if payload_offset.checked_add(payload_bytes).is_none() {
            return Err(VolError::Invalid("payload position or size"));
        }
        Ok(())
    }

    /// A fresh volume with one slot (`kind` Person or Admin, opened by `password`) and a random volume key.
    pub fn create(rng: &mut dyn Random, v: &NewVolume, password: &[u8], kind: SlotKind, tag: u32, kdf: KdfParams) -> Result<(Header, Zeroizing<[u8; KEY_LEN]>), VolError> {
        Self::check_geometry(v.sector_size, v.payload_offset, v.payload_bytes)?;
        if kind == SlotKind::Recovery {
            return Err(VolError::WrongSlotKind);
        }
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        rng.fill(&mut *key)?;
        let mut uuid = [0u8; 16];
        rng.fill(&mut uuid)?;
        let mut h = Header { generation: 1, uuid, sector_size: v.sector_size, payload_offset: v.payload_offset, payload_bytes: v.payload_bytes, slots: Default::default(), mac: [0; 32] };
        let slot = Self::make_password_slot(rng, &key, password, kind, tag, kdf)?;
        h.slots[0] = Some(slot);
        h.seal(&key);
        Ok((h, key))
    }

    fn make_password_slot(rng: &mut dyn Random, key: &[u8; KEY_LEN], password: &[u8], kind: SlotKind, tag: u32, kdf: KdfParams) -> Result<Slot, VolError> {
        let mut salt = [0u8; 32];
        rng.fill(&mut salt)?;
        let kek = derive_kek(password, &salt, &kdf)?;
        let w = kw::wrap256(&kek, key).ok_or(VolError::Invalid("key wrap"))?;
        Ok(Slot { kind, kdf: Some(kdf), tag, salt, wrapped: w.try_into().unwrap() })
    }

    fn mac_key(key: &[u8; KEY_LEN]) -> [u8; 32] {
        hmac_sha256(key, &[b"iyakki-volume-header-mac-key-v1"])
    }

    fn body(&self) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        b[..8].copy_from_slice(MAGIC);
        b[8..12].copy_from_slice(&1u32.to_le_bytes());
        b[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        b[16..24].copy_from_slice(&self.generation.to_le_bytes());
        b[24..40].copy_from_slice(&self.uuid);
        b[40..44].copy_from_slice(&self.sector_size.to_le_bytes());
        b[44..48].copy_from_slice(&1u32.to_le_bytes());
        b[48..56].copy_from_slice(&self.payload_offset.to_le_bytes());
        b[56..64].copy_from_slice(&self.payload_bytes.to_le_bytes());
        for (i, s) in self.slots.iter().enumerate() {
            if let Some(s) = s {
                let o = SLOT_AT + i * SLOT_LEN;
                b[o..o + 4].copy_from_slice(&(s.kind as u32).to_le_bytes());
                let k = s.kdf.unwrap_or(KdfParams { m_kib: 0, t: 0, p: 0 });
                b[o + 4..o + 8].copy_from_slice(&k.m_kib.to_le_bytes());
                b[o + 8..o + 12].copy_from_slice(&k.t.to_le_bytes());
                b[o + 12..o + 16].copy_from_slice(&k.p.to_le_bytes());
                b[o + 16..o + 20].copy_from_slice(&s.tag.to_le_bytes());
                b[o + 24..o + 56].copy_from_slice(&s.salt);
                b[o + 56..o + 128].copy_from_slice(&s.wrapped);
            }
        }
        b
    }

    /// The 4096 bytes to store.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = self.body();
        b[MAC_AT..MAC_AT + 32].copy_from_slice(&self.mac);
        let sum = Sha256::digest(&b[..SUM_AT]);
        b[SUM_AT..SUM_AT + 32].copy_from_slice(&sum);
        b
    }

    fn compute_mac(&self, key: &[u8; KEY_LEN]) -> [u8; 32] {
        let b = self.body();
        hmac_sha256(&Self::mac_key(key), &[&b[..MAC_AT]])
    }

    /// Raise the generation and re-sign the slot table; every change ends here.
    fn seal(&mut self, key: &[u8; KEY_LEN]) {
        self.mac = self.compute_mac(key);
    }

    /// Read a header. Hostile or damaged input gives an error and never a panic or a huge allocation.
    pub fn parse(bytes: &[u8]) -> Result<Header, VolError> {
        if bytes.len() < 8 {
            return Err(VolError::Truncated);
        }
        if &bytes[..8] != MAGIC {
            return Err(VolError::NotOurs);
        }
        if bytes.len() < HEADER_LEN {
            return Err(VolError::Truncated);
        }
        let b = &bytes[..HEADER_LEN];
        let version = u32at(b, 8);
        if version > 1 {
            return Err(VolError::Newer(version));
        }
        if version != 1 {
            return Err(VolError::Invalid("format version"));
        }
        if !ct_eq(&Sha256::digest(&b[..SUM_AT]), &b[SUM_AT..SUM_AT + 32]) {
            return Err(VolError::Damaged);
        }
        if u32at(b, 12) as usize != HEADER_LEN || u32at(b, 44) != 1 {
            return Err(VolError::Invalid("header length or cipher"));
        }
        if b[64..SLOT_AT].iter().any(|x| *x != 0) || b[BODY_END..].iter().any(|x| *x != 0) {
            return Err(VolError::Invalid("reserved bytes are not zero"));
        }
        let sector_size = u32at(b, 40);
        let (payload_offset, payload_bytes) = (u64at(b, 48), u64at(b, 56));
        Self::check_geometry(sector_size, payload_offset, payload_bytes)?;
        let mut slots: [Option<Slot>; SLOTS] = Default::default();
        let mut any = false;
        for (i, slot) in slots.iter_mut().enumerate() {
            let o = SLOT_AT + i * SLOT_LEN;
            let s = &b[o..o + SLOT_LEN];
            let kind = u32at(s, 0);
            if kind == 0 {
                if s.iter().any(|x| *x != 0) {
                    return Err(VolError::Invalid("an empty key slot holds data"));
                }
                continue;
            }
            if u32at(s, 20) != 0 {
                return Err(VolError::Invalid("reserved bytes are not zero"));
            }
            let k = KdfParams { m_kib: u32at(s, 4), t: u32at(s, 8), p: u32at(s, 12) };
            let (kind, kdf) = match kind {
                1 | 3 => {
                    if k.m_kib > MAX_M_KIB {
                        return Err(VolError::KdfTooBig(k.m_kib / 1024));
                    }
                    if !k.valid() {
                        return Err(VolError::Invalid("password cost out of range"));
                    }
                    (if kind == 1 { SlotKind::Person } else { SlotKind::Admin }, Some(k))
                }
                2 => {
                    if k.m_kib != 0 || k.t != 0 || k.p != 0 {
                        return Err(VolError::Invalid("a recovery slot has a password cost"));
                    }
                    (SlotKind::Recovery, None)
                }
                _ => return Err(VolError::Invalid("unknown key slot kind")),
            };
            *slot = Some(Slot { kind, kdf, tag: u32at(s, 16), salt: s[24..56].try_into().unwrap(), wrapped: s[56..128].try_into().unwrap() });
            any = true;
        }
        if !any {
            return Err(VolError::Invalid("no key slot"));
        }
        Ok(Header { generation: u64at(b, 16), uuid: b[24..40].try_into().unwrap(), sector_size, payload_offset, payload_bytes, slots, mac: b[MAC_AT..MAC_AT + 32].try_into().unwrap() })
    }

    /// Is the slot table signed by this key (nobody changed it without the key)?
    pub fn mac_ok(&self, key: &[u8; KEY_LEN]) -> bool {
        ct_eq(&self.compute_mac(key), &self.mac)
    }

    fn unlock_with(&self, want: fn(SlotKind) -> bool, kek_for: &dyn Fn(&Slot) -> Result<Zeroizing<[u8; 32]>, VolError>) -> Result<Unlocked, VolError> {
        // Every matching slot is tried, even after one opens, so a right and a wrong secret do the same work.
        let mut found: Option<Unlocked> = None;
        for (i, s) in self.slots.iter().enumerate() {
            let Some(s) = s else { continue };
            if !want(s.kind) {
                continue;
            }
            let kek = kek_for(s)?;
            if let Some(k) = kw::unwrap256(&kek, &s.wrapped) {
                if found.is_none() && k.len() == KEY_LEN {
                    let mut key = Zeroizing::new([0u8; KEY_LEN]);
                    key.copy_from_slice(&k);
                    found = Some(Unlocked { key, slot: i });
                }
            }
        }
        let u = found.ok_or(VolError::WrongSecret)?;
        if !self.mac_ok(&u.key) {
            return Err(VolError::Tampered);
        }
        Ok(u)
    }

    /// Open with a password (person and admin slots).
    pub fn unlock_password(&self, password: &[u8]) -> Result<Unlocked, VolError> {
        self.unlock_with(|k| k != SlotKind::Recovery, &|s| derive_kek(password, &s.salt, &s.kdf.ok_or(VolError::Invalid("slot"))?))
    }

    /// Open with the recovery key.
    pub fn unlock_recovery(&self, rk: &RecoveryKey) -> Result<Unlocked, VolError> {
        self.unlock_with(|k| k == SlotKind::Recovery, &|_| Ok(Zeroizing::new(rk.kek())))
    }

    fn check_key(&self, key: &[u8; KEY_LEN]) -> Result<(), VolError> {
        if self.mac_ok(key) {
            Ok(())
        } else {
            Err(VolError::WrongSecret)
        }
    }

    fn free_slot(&self) -> Result<usize, VolError> {
        self.slots.iter().position(|s| s.is_none()).ok_or(VolError::SlotsFull)
    }

    fn done(&mut self, key: &[u8; KEY_LEN]) {
        self.generation += 1;
        self.seal(key);
    }

    /// Add a password slot (Person or Admin). Returns its index.
    pub fn add_password(&mut self, rng: &mut dyn Random, key: &[u8; KEY_LEN], password: &[u8], kind: SlotKind, tag: u32, kdf: KdfParams) -> Result<usize, VolError> {
        self.check_key(key)?;
        if kind == SlotKind::Recovery {
            return Err(VolError::WrongSlotKind);
        }
        let i = self.free_slot()?;
        self.slots[i] = Some(Self::make_password_slot(rng, key, password, kind, tag, kdf)?);
        self.done(key);
        Ok(i)
    }

    /// Make a recovery slot and return the key to print once. An older recovery slot is replaced, so there is only ever one.
    pub fn set_recovery(&mut self, rng: &mut dyn Random, key: &[u8; KEY_LEN]) -> Result<(usize, RecoveryKey), VolError> {
        self.check_key(key)?;
        let rk = RecoveryKey::generate(rng)?;
        let i = match self.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.kind == SlotKind::Recovery)) {
            Some(i) => i,
            None => self.free_slot()?,
        };
        let w = kw::wrap256(&rk.kek(), key).ok_or(VolError::Invalid("key wrap"))?;
        self.slots[i] = Some(Slot { kind: SlotKind::Recovery, kdf: None, tag: 0, salt: [0; 32], wrapped: w.try_into().unwrap() });
        self.done(key);
        Ok((i, rk))
    }

    /// Give slot `index` a new password (new salt, same kind and tag). The data is untouched.
    pub fn change_password(&mut self, rng: &mut dyn Random, key: &[u8; KEY_LEN], index: usize, new_password: &[u8], kdf: KdfParams) -> Result<(), VolError> {
        self.check_key(key)?;
        let old = self.slots.get(index).and_then(|s| s.clone()).ok_or(VolError::NoSuchSlot)?;
        if old.kind == SlotKind::Recovery {
            return Err(VolError::WrongSlotKind);
        }
        self.slots[index] = Some(Self::make_password_slot(rng, key, new_password, old.kind, old.tag, kdf)?);
        self.done(key);
        Ok(())
    }

    /// Remove a slot. The last slot cannot be removed.
    pub fn remove_slot(&mut self, key: &[u8; KEY_LEN], index: usize) -> Result<(), VolError> {
        self.check_key(key)?;
        if self.slots.get(index).is_none_or(|s| s.is_none()) {
            return Err(VolError::NoSuchSlot);
        }
        if self.slots.iter().flatten().count() == 1 {
            return Err(VolError::LastSlot);
        }
        self.slots[index] = None;
        self.done(key);
        Ok(())
    }

    /// Slot index, kind and tag of every used slot, for Settings.
    pub fn list(&self) -> Vec<(usize, SlotKind, u32)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|s| (i, s.kind, s.tag))).collect()
    }
}
