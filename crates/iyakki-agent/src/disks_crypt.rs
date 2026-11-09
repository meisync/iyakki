//! DISKS, encrypted volumes: what a LUKS or BitLocker signature says (pure parsers, hostile-input safe), the secret a person types
//! (never stored, never printed, wiped when dropped) and the rate limit on wrong keys. The unlocking itself is `disks_unlock.rs`.
//!
//! Nothing here reads a device: the parsers take the first bytes of a partition that the DISKS probe has already read.

use std::collections::HashMap;

/// What kind of encrypted container a partition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptKind {
    Luks1,
    Luks2,
    BitLocker,
}

impl CryptKind {
    /// "LUKS1", "LUKS2", "BitLocker".
    pub fn name(self) -> &'static str {
        match self {
            CryptKind::Luks1 => "LUKS1",
            CryptKind::Luks2 => "LUKS2",
            CryptKind::BitLocker => "BitLocker",
        }
    }

    /// "LUKS" or "BitLocker": the word the Disks tile shows ("Encrypted: LUKS").
    pub fn family(self) -> &'static str {
        match self {
            CryptKind::Luks1 | CryptKind::Luks2 => "LUKS",
            CryptKind::BitLocker => "BitLocker",
        }
    }

    /// The id of the helper program that opens it (a CLI tools entry).
    pub fn helper(self) -> &'static str {
        match self {
            CryptKind::Luks1 | CryptKind::Luks2 => "cryptsetup",
            CryptKind::BitLocker => "dislocker",
        }
    }

    /// What the person types.
    pub fn secret_word(self) -> &'static str {
        match self {
            CryptKind::Luks1 | CryptKind::Luks2 => "password",
            CryptKind::BitLocker => "password or 48-digit recovery key",
        }
    }
}

/// What the header of an encrypted container says. Everything in it came from the drive, so it is cleaned before it is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CryptInfo {
    pub kind: CryptKind,
    /// LUKS UUID or the BitLocker volume GUID; empty when the header has none that makes sense.
    pub uuid: String,
    /// LUKS2 label (LUKS1 and BitLocker have none).
    pub label: String,
    /// "aes-xts-plain64, sha256" and the like; short.
    pub detail: String,
    /// Why this header cannot be used (damaged, unknown version); `None` when it looks usable.
    pub problem: Option<String>,
}

impl CryptInfo {
    /// "Encrypted: LUKS2" / "Encrypted: BitLocker".
    pub fn headline(&self) -> String {
        format!("Encrypted: {}", if self.kind == CryptKind::BitLocker { "BitLocker" } else { self.kind.name() })
    }
}

/// Printable ASCII only, up to the first NUL, at most `max` characters: a header is hostile input.
fn clean(b: &[u8], off: usize, len: usize, max: usize) -> String {
    let s = b.get(off..off.saturating_add(len)).unwrap_or(&[]);
    s.iter().copied().take_while(|c| *c != 0).filter(|c| (0x20..0x7f).contains(c)).take(max).map(|c| c as char).collect::<String>().trim().to_string()
}

fn be16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn be64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// A UUID as text only when it has the shape of one (36 characters, hex and dashes).
fn uuid_text(b: &[u8], off: usize) -> String {
    let t = clean(b, off, 40, 36);
    let ok = t.len() == 36 && t.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() });
    if ok { t.to_ascii_lowercase() } else { String::new() }
}

/// The smallest header either LUKS version needs for the fields read here.
const LUKS_MIN: usize = 208;

/// `Some` when the bytes start with the LUKS magic (`LUKS\xba\xbe`), whatever else is wrong with them.
pub fn parse_luks(b: &[u8]) -> Option<CryptInfo> {
    if b.get(0..6) != Some(b"LUKS\xba\xbe") {
        return None;
    }
    let mk = |kind, uuid, label, detail, problem: Option<String>| Some(CryptInfo { kind, uuid, label, detail, problem });
    let Some(version) = be16(b, 6) else {
        return mk(CryptKind::Luks1, String::new(), String::new(), String::new(), Some("the LUKS header is cut short (the drive may be damaged)".into()));
    };
    let kind = match version {
        1 => CryptKind::Luks1,
        2 => CryptKind::Luks2,
        v => {
            return mk(CryptKind::Luks2, String::new(), String::new(), String::new(), Some(format!("a LUKS header of version {v}, which IYAKKI does not know (only 1 and 2)")));
        }
    };
    if b.len() < LUKS_MIN {
        return mk(kind, String::new(), String::new(), String::new(), Some("the LUKS header is cut short (the drive may be damaged)".into()));
    }
    let uuid = uuid_text(b, 168);
    let mut problem = None;
    let (label, detail) = if kind == CryptKind::Luks1 {
        let (cipher, mode, hash) = (clean(b, 8, 32, 32), clean(b, 40, 32, 32), clean(b, 72, 32, 32));
        let key_bytes = be32(b, 108).unwrap_or(0);
        if cipher.is_empty() || !matches!(key_bytes, 16 | 24 | 32 | 48 | 64) {
            problem = Some("the LUKS1 header is damaged (its cipher or key size makes no sense)".into());
        }
        (String::new(), format!("{cipher}-{mode}, {hash}"))
    } else {
        // LUKS2: the binary header gives the size of the header area (a power of two from 16 KiB to 4 MiB)
        let hdr = be64(b, 8).unwrap_or(0);
        if !(16 << 10..=4 << 20).contains(&hdr) || !hdr.is_power_of_two() {
            problem = Some("the LUKS2 header is damaged (its size makes no sense)".into());
        }
        (clean(b, 24, 48, 48), format!("{}", clean(b, 72, 32, 16)))
    };
    mk(kind, uuid, label, detail, problem)
}

/// The GUID that marks a BitLocker To Go volume that looks like FAT (offset 424 of its boot sector).
pub const FVE_TO_GO_GUID: [u8; 16] = [0x3B, 0xD6, 0x67, 0x49, 0x29, 0x2E, 0xD8, 0x4A, 0x83, 0x99, 0xF6, 0xA3, 0x39, 0xE3, 0xD0, 0x01];

/// A Windows GUID (first three fields little-endian) as text.
fn guid_text(b: &[u8]) -> String {
    if b.len() != 16 || b.iter().all(|x| *x == 0) {
        return String::new();
    }
    format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u16::from_le_bytes([b[4], b[5]]), u16::from_le_bytes([b[6], b[7]]), b[8], b[9], b[10..16].iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// `Some` when the boot sector is a BitLocker volume's: `-FVE-FS-` at offset 3, or a BitLocker To Go sector (`MSWIN4.1` with the
/// To Go GUID at 424).
pub fn parse_bitlocker(b: &[u8]) -> Option<CryptInfo> {
    let fve = b.get(3..11) == Some(b"-FVE-FS-");
    let to_go = b.get(3..11) == Some(b"MSWIN4.1") && b.get(424..440) == Some(&FVE_TO_GO_GUID[..]);
    if !fve && !to_go {
        return None;
    }
    let mut problem = None;
    let sector = b.get(11..13).map(|s| u16::from_le_bytes([s[0], s[1]]));
    if b.len() < 512 {
        problem = Some("the BitLocker boot sector is cut short (the drive may be damaged)".into());
    } else if !matches!(sector, Some(512 | 1024 | 2048 | 4096)) {
        problem = Some("the BitLocker boot sector is damaged (its sector size makes no sense)".into());
    }
    let guid = if fve { b.get(160..176).map(guid_text).unwrap_or_default() } else { String::new() };
    Some(CryptInfo { kind: CryptKind::BitLocker, uuid: guid, label: String::new(), detail: if to_go { "BitLocker To Go".into() } else { "Windows BitLocker".into() }, problem })
}

/// Either kind, from the first bytes of a partition.
pub fn parse(b: &[u8]) -> Option<CryptInfo> {
    parse_luks(b).or_else(|| parse_bitlocker(b))
}

// ---------------------------------------------------------------------------------------------------------------------
// the secret
// ---------------------------------------------------------------------------------------------------------------------

/// A password or recovery key. It has no `Clone`, no `Display` and a `Debug` that shows nothing; the bytes are overwritten when it
/// is dropped. It is handed to the helper through a pipe, never an argument or the environment.
pub struct Secret(Vec<u8>);

/// Overwrite bytes in a way the optimiser may not remove.
pub fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: `x` is a valid, aligned, exclusive reference to one byte.
        unsafe { std::ptr::write_volatile(x, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

impl Secret {
    /// Take the text of a masked field (the caller wipes its own copy).
    pub fn new(text: &str) -> Secret {
        Secret(text.as_bytes().to_vec())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The bytes, for writing to the helper's pipe and nothing else.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

/// Wipe the text of a field after it was used.
pub fn wipe_string(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8, so the string stays valid.
    wipe(unsafe { s.as_bytes_mut() });
    s.clear();
}

// ---------------------------------------------------------------------------------------------------------------------
// the rate limit
// ---------------------------------------------------------------------------------------------------------------------

/// Seconds to wait after the n-th wrong key in a row: 2, 5, 15, 30, then 60.
pub fn delay_after(fails: u32) -> u64 {
    match fails {
        0 => 0,
        1 => 2,
        2 => 5,
        3 => 15,
        4 => 30,
        _ => 60,
    }
}

/// Wrong keys per partition. Time is given in seconds by the caller, so a test needs no sleeping.
#[derive(Default, Debug)]
pub struct Limiter {
    map: HashMap<String, (u32, u64)>,
}

const LIMITER_KEYS: usize = 64;

impl Limiter {
    /// `Err(seconds to wait)` when a try is too soon after the last wrong key.
    pub fn check(&self, key: &str, now: u64) -> Result<(), u64> {
        match self.map.get(key) {
            Some((n, at)) => {
                let until = at + delay_after(*n);
                if now < until { Err(until - now) } else { Ok(()) }
            }
            None => Ok(()),
        }
    }

    pub fn failed(&mut self, key: &str, now: u64) {
        if !self.map.contains_key(key) && self.map.len() >= LIMITER_KEYS {
            if let Some(old) = self.map.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) {
                self.map.remove(&old);
            }
        }
        let e = self.map.entry(key.to_string()).or_insert((0, now));
        e.0 = e.0.saturating_add(1);
        e.1 = now;
    }

    pub fn ok(&mut self, key: &str) {
        self.map.remove(key);
    }

    pub fn fails(&self, key: &str) -> u32 {
        self.map.get(key).map_or(0, |e| e.0)
    }
}

/// The name of the mapped device of a partition: `iyakki-sdb1`. Only letters, digits, `-` and `_` survive.
pub fn mapper_name(part: &str) -> String {
    let s: String = part.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(40).collect();
    format!("iyakki-{s}")
}

/// Where the opened (decrypted) device appears.
pub fn mapper_path(part: &str) -> String {
    format!("/dev/mapper/{}", mapper_name(part))
}
