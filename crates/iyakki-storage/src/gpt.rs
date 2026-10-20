//! GUID partition tables (GPT), written and read by our own code: protective MBR, primary and backup headers and entry
//! arrays with their CRC32s, any logical sector size (512 or 4096 bytes; every address here is in sectors of that size).
//!
//! The installer builds the table with [`Table`], writes the pieces, and checks both copies again with [`read_gpt`].
//! [`read_disk_table`] describes whatever table a disk carries (GPT or MBR) for the installer's confirmation screen.
//! Nothing here opens a device: callers pass any `Read + Seek` (a file in tests).

use std::io::{Read, Seek, SeekFrom};

/// A GUID in its on-disk form (the first three fields little-endian).
pub type Guid = [u8; 16];

pub const SIGNATURE: &[u8; 8] = b"EFI PART";
pub const HEADER_SIZE: u32 = 92;
pub const ENTRY_SIZE: u32 = 128;
pub const ENTRY_COUNT: u32 = 128;
/// Bytes in the standard entry array (128 entries of 128 bytes).
pub const ENTRY_ARRAY_BYTES: u64 = (ENTRY_SIZE * ENTRY_COUNT) as u64;

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

/// EFI System Partition.
pub const TYPE_ESP: Guid = guid(0xC12A7328, 0xF81F, 0x11D2, [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
/// BIOS boot partition: where GRUB embeds its core image on a GPT disk under legacy BIOS.
pub const TYPE_BIOS_BOOT: Guid = guid(0x21686148, 0x6449, 0x6E6F, [0x74, 0x4E, 0x65, 0x65, 0x64, 0x45, 0x46, 0x49]);
/// Linux filesystem data.
pub const TYPE_LINUX_DATA: Guid = guid(0x0FC63DAF, 0x8483, 0x4772, [0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4]);

/// "C12A7328-F81F-11D2-BA4B-00A0C93EC93B".
pub fn guid_to_string(g: &Guid) -> String {
    let (a, b, c) = (u32::from_le_bytes([g[0], g[1], g[2], g[3]]), u16::from_le_bytes([g[4], g[5]]), u16::from_le_bytes([g[6], g[7]]));
    format!("{a:08X}-{b:04X}-{c:04X}-{:02X}{:02X}-{}", g[8], g[9], g[10..].iter().map(|x| format!("{x:02X}")).collect::<String>())
}

pub fn guid_from_str(s: &str) -> Option<Guid> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    if parts.len() != 5 || [8, 4, 4, 4, 12].iter().zip(&parts).any(|(n, p)| p.len() != *n || !p.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    let (a, b, c) = (u32::from_str_radix(parts[0], 16).ok()?, u16::from_str_radix(parts[1], 16).ok()?, u16::from_str_radix(parts[2], 16).ok()?);
    let tail = format!("{}{}", parts[3], parts[4]);
    let mut d = [0u8; 8];
    for (i, x) in d.iter_mut().enumerate() {
        *x = u8::from_str_radix(&tail[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(guid(a, b, c, d))
}

/// A random (version 4) GUID from the kernel's random source; the clock only if that is unavailable.
pub fn random_guid() -> Guid {
    let mut g = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut g)).is_ok();
    if !ok || g == [0; 16] {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos());
        let mut s = (t as u64) ^ ((t >> 64) as u64) ^ 0x9e37_79b9_7f4a_7c15;
        for b in g.iter_mut() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *b = (s >> 24) as u8;
        }
    }
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

/// CRC-32 (IEEE 802.3, as GPT uses it).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, b| t[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// What a partition type is, for people.
pub fn type_name(g: &Guid) -> String {
    let s = guid_to_string(g);
    match s.as_str() {
        "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" => "EFI System",
        "21686148-6449-6E6F-744E-656564454649" => "BIOS boot",
        "0FC63DAF-8483-4772-8E79-3D69D8477DE4" => "Linux filesystem",
        "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7" => "Microsoft basic data",
        "E3C9E316-0B5C-4DB8-817D-F92DF00215AE" => "Microsoft reserved",
        "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC" => "Windows recovery",
        "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F" => "Linux swap",
        "E6D6D379-F507-44C2-A23C-238F2A3DF928" => "Linux LVM",
        "A19D880F-05FC-4D3B-A006-743F0F84911E" => "Linux RAID",
        "933AC7E1-2EB4-4F13-B844-0E14E2AEF915" => "Linux home",
        "48465300-0000-11AA-AA11-00306543ECAC" => "Apple HFS+",
        "7C3457EF-0000-11AA-AA11-00306543ECAC" => "Apple APFS",
        "00000000-0000-0000-0000-000000000000" => "unused",
        _ => return s,
    }
    .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 1-based partition number (slot + 1).
    pub index: u32,
    pub type_guid: Guid,
    pub guid: Guid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attrs: u64,
    pub name: String,
}

impl Entry {
    pub fn sectors(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
}

/// A GPT as it is on the disk: everything the primary and backup copies are made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub sector_size: u32,
    pub total_sectors: u64,
    pub disk_guid: Guid,
    pub entries: Vec<Entry>,
}

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl Table {
    /// Sectors the entry array takes with this sector size (32 for 512-byte, 4 for 4096-byte sectors).
    pub fn array_sectors(&self) -> u64 {
        ENTRY_ARRAY_BYTES.div_ceil(self.sector_size as u64)
    }
    pub fn last_lba(&self) -> u64 {
        self.total_sectors - 1
    }
    pub fn primary_entries_lba(&self) -> u64 {
        2
    }
    pub fn backup_entries_lba(&self) -> u64 {
        self.total_sectors - 1 - self.array_sectors()
    }
    pub fn first_usable(&self) -> u64 {
        2 + self.array_sectors()
    }
    pub fn last_usable(&self) -> u64 {
        self.backup_entries_lba() - 1
    }

    /// The 16 KiB entry array (padded to whole sectors).
    pub fn entries_bytes(&self) -> Vec<u8> {
        let mut v = vec![0u8; (self.array_sectors() * self.sector_size as u64) as usize];
        for e in &self.entries {
            let o = (e.index as usize - 1) * ENTRY_SIZE as usize;
            let s = &mut v[o..o + ENTRY_SIZE as usize];
            s[0..16].copy_from_slice(&e.type_guid);
            s[16..32].copy_from_slice(&e.guid);
            s[32..40].copy_from_slice(&e.first_lba.to_le_bytes());
            s[40..48].copy_from_slice(&e.last_lba.to_le_bytes());
            s[48..56].copy_from_slice(&e.attrs.to_le_bytes());
            for (i, u) in e.name.encode_utf16().take(36).enumerate() {
                s[56 + 2 * i..58 + 2 * i].copy_from_slice(&u.to_le_bytes());
            }
        }
        v
    }

    /// One sector holding the header of the primary (`backup == false`) or the backup copy.
    pub fn header_bytes(&self, backup: bool) -> Vec<u8> {
        let mut h = vec![0u8; self.sector_size as usize];
        h[0..8].copy_from_slice(SIGNATURE);
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&HEADER_SIZE.to_le_bytes());
        let (me, other, ents) = if backup { (self.last_lba(), 1, self.backup_entries_lba()) } else { (1, self.last_lba(), 2) };
        h[24..32].copy_from_slice(&me.to_le_bytes());
        h[32..40].copy_from_slice(&other.to_le_bytes());
        h[40..48].copy_from_slice(&self.first_usable().to_le_bytes());
        h[48..56].copy_from_slice(&self.last_usable().to_le_bytes());
        h[56..72].copy_from_slice(&self.disk_guid);
        h[72..80].copy_from_slice(&ents.to_le_bytes());
        h[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        h[88..92].copy_from_slice(&crc32(&self.entries_bytes()[..ENTRY_ARRAY_BYTES as usize]).to_le_bytes());
        let c = crc32(&h[..HEADER_SIZE as usize]);
        h[16..20].copy_from_slice(&c.to_le_bytes());
        h
    }

    /// The protective MBR for this disk: boot code (up to 440 bytes) as given, `signature`, and one type-0xEE partition
    /// covering the whole disk (or as much as an MBR can say). One sector long.
    pub fn protective_mbr(&self, boot_code: &[u8], signature: [u8; 4]) -> Vec<u8> {
        let mut m = vec![0u8; self.sector_size as usize];
        let n = boot_code.len().min(440);
        m[..n].copy_from_slice(&boot_code[..n]);
        m[440..444].copy_from_slice(&signature);
        let size = (self.total_sectors - 1).min(0xFFFF_FFFF) as u32;
        m[446..450].copy_from_slice(&[0x00, 0x00, 0x02, 0x00]); // not active; CHS of LBA 1
        m[450] = 0xEE;
        m[451..454].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
        m[454..458].copy_from_slice(&1u32.to_le_bytes());
        m[458..462].copy_from_slice(&size.to_le_bytes());
        m[510] = 0x55;
        m[511] = 0xAA;
        m
    }

    /// Check the table's own consistency (what a reader checks): inside the usable area, not overlapping.
    pub fn check(&self) -> Result<(), String> {
        let (lo, hi) = (self.first_usable(), self.last_usable());
        if lo > hi {
            return Err("the disk is too small for a GPT".into());
        }
        let mut spans: Vec<(u64, u64, u32)> = Vec::new();
        for e in &self.entries {
            if e.index == 0 || e.index > ENTRY_COUNT {
                return Err(format!("partition number {} is outside the table", e.index));
            }
            if e.first_lba > e.last_lba || e.first_lba < lo || e.last_lba > hi {
                return Err(format!("partition {} ({}-{}) is outside the usable area ({lo}-{hi})", e.index, e.first_lba, e.last_lba));
            }
            spans.push((e.first_lba, e.last_lba, e.index));
        }
        spans.sort();
        for w in spans.windows(2) {
            if w[1].0 <= w[0].1 {
                return Err(format!("partitions {} and {} overlap", w[0].2, w[1].2));
            }
        }
        Ok(())
    }
}

/// Both copies as found on a disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub table: Table,
    pub primary_ok: bool,
    pub backup_ok: bool,
    /// What was wrong with a copy that failed, or differed.
    pub problems: Vec<String>,
}

fn read_at<R: Read + Seek>(r: &mut R, off: u64, len: usize) -> Result<Vec<u8>, String> {
    let mut v = vec![0u8; len];
    r.seek(SeekFrom::Start(off)).and_then(|_| r.read_exact(&mut v)).map_err(|e| format!("cannot read at byte {off}: {e}"))?;
    Ok(v)
}

fn parse_copy<R: Read + Seek>(r: &mut R, ss: u32, total: u64, backup: bool) -> Result<Table, String> {
    let which = if backup { "backup" } else { "primary" };
    let hdr_lba = if backup { total - 1 } else { 1 };
    let h = read_at(r, hdr_lba * ss as u64, ss as usize).map_err(|e| format!("{which} header: {e}"))?;
    if &h[0..8] != SIGNATURE {
        return Err(format!("{which} header: no \"EFI PART\" signature"));
    }
    let hs = le32(&h, 12) as usize;
    if !(HEADER_SIZE as usize..=ss as usize).contains(&hs) {
        return Err(format!("{which} header: bad header size {hs}"));
    }
    let mut z = h[..hs].to_vec();
    z[16..20].fill(0);
    if crc32(&z) != le32(&h, 16) {
        return Err(format!("{which} header: checksum (CRC32) does not match"));
    }
    if le32(&h, 8) >> 16 != 1 {
        return Err(format!("{which} header: unknown revision {:#x}", le32(&h, 8)));
    }
    let (me, alt) = (le64(&h, 24), le64(&h, 32));
    let (want_me, want_alt) = if backup { (total - 1, 1) } else { (1, total - 1) };
    if me != want_me {
        return Err(format!("{which} header: says it is at sector {me}, found at {want_me}"));
    }
    if alt != want_alt {
        return Err(format!("{which} header: the other copy should be at sector {want_alt}, it says {alt} (backup table not at the end of the disk)"));
    }
    let (first, last) = (le64(&h, 40), le64(&h, 48));
    let ents_lba = le64(&h, 72);
    let (n, esz) = (le32(&h, 80), le32(&h, 84));
    if esz < ENTRY_SIZE || esz % 8 != 0 || esz > 1024 || n == 0 || n > 4096 {
        return Err(format!("{which} header: unsupported entry array ({n} entries of {esz} bytes)"));
    }
    let bytes = n as u64 * esz as u64;
    let sectors = bytes.div_ceil(ss as u64);
    if backup && ents_lba + sectors != total - 1 || !backup && (ents_lba < 2 || ents_lba + sectors > first) {
        return Err(format!("{which} header: the entry array (sector {ents_lba}) is in the wrong place"));
    }
    if first > last || last >= total - 1 {
        return Err(format!("{which} header: usable area {first}-{last} does not fit the disk"));
    }
    let arr = read_at(r, ents_lba * ss as u64, (sectors * ss as u64) as usize).map_err(|e| format!("{which} entries: {e}"))?;
    if crc32(&arr[..bytes as usize]) != le32(&h, 88) {
        return Err(format!("{which} entries: checksum (CRC32) does not match"));
    }
    let mut entries = Vec::new();
    for i in 0..n as usize {
        let e = &arr[i * esz as usize..i * esz as usize + ENTRY_SIZE as usize];
        if e[0..16] == [0u8; 16] {
            continue;
        }
        let units: Vec<u16> = (0..36).map(|k| u16::from_le_bytes([e[56 + 2 * k], e[57 + 2 * k]])).take_while(|u| *u != 0).collect();
        entries.push(Entry {
            index: i as u32 + 1,
            type_guid: e[0..16].try_into().unwrap(),
            guid: e[16..32].try_into().unwrap(),
            first_lba: le64(e, 32),
            last_lba: le64(e, 40),
            attrs: le64(e, 48),
            name: String::from_utf16_lossy(&units),
        });
    }
    let t = Table { sector_size: ss, total_sectors: total, disk_guid: h[56..72].try_into().unwrap(), entries };
    // the entries are checked against the usable area the header states, not ours
    let (lo, hi) = (first, last);
    let mut spans: Vec<(u64, u64, u32)> = Vec::new();
    for e in &t.entries {
        if e.first_lba > e.last_lba || e.first_lba < lo || e.last_lba > hi {
            return Err(format!("{which} entries: partition {} ({}-{}) is outside the usable area ({lo}-{hi})", e.index, e.first_lba, e.last_lba));
        }
        spans.push((e.first_lba, e.last_lba, e.index));
    }
    spans.sort();
    if let Some(w) = spans.windows(2).find(|w| w[1].0 <= w[0].1) {
        return Err(format!("{which} entries: partitions {} and {} overlap", w[0].2, w[1].2));
    }
    Ok(t)
}

/// Read and check both copies of the GPT of a disk of `total_sectors` sectors of `ss` bytes. Succeeds when at least one copy
/// is good (like the kernel does); `problems` says what was wrong with the other, and with differences between the two.
pub fn read_gpt<R: Read + Seek>(r: &mut R, ss: u32, total_sectors: u64) -> Result<Parsed, String> {
    if total_sectors < 4 + 2 * ENTRY_ARRAY_BYTES.div_ceil(ss as u64) {
        return Err("the disk is too small for a GPT".into());
    }
    let p = parse_copy(r, ss, total_sectors, false);
    let b = parse_copy(r, ss, total_sectors, true);
    let mut problems = Vec::new();
    match (p, b) {
        (Ok(a), Ok(c)) => {
            if a != c {
                problems.push("the primary and the backup table differ".into());
            }
            Ok(Parsed { table: a, primary_ok: true, backup_ok: true, problems })
        }
        (Ok(a), Err(e)) => Ok(Parsed { table: a, primary_ok: true, backup_ok: false, problems: vec![e] }),
        (Err(e), Ok(c)) => Ok(Parsed { table: c, primary_ok: false, backup_ok: true, problems: vec![e] }),
        (Err(e1), Err(e2)) => Err(format!("{e1}; {e2}")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No partition table found (blank, or something else).
    None,
    Mbr,
    Gpt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartInfo {
    pub index: u32,
    /// In sectors of the disk's logical size.
    pub first_lba: u64,
    pub sectors: u64,
    pub type_name: String,
    pub label: String,
}

/// What a disk's partition table says, for showing to a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub kind: Kind,
    pub sector_size: u32,
    pub parts: Vec<PartInfo>,
    /// Damage and oddities: "backup table missing", "protective MBR without a valid GPT", ...
    pub notes: Vec<String>,
}

fn mbr_type_name(t: u8) -> &'static str {
    match t {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x05 | 0x0F | 0x85 => "extended",
        0x07 => "NTFS/exFAT",
        0x0B | 0x0C => "FAT32",
        0x11 | 0x1B | 0x1C => "hidden FAT",
        0x27 => "Windows recovery",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0x8E => "Linux LVM",
        0xA5 | 0xA6 | 0xA9 => "BSD",
        0xAB | 0xAF => "Apple",
        0xEE => "GPT protective",
        0xEF => "EFI System",
        0xFD => "Linux RAID",
        _ => "other",
    }
}

/// Read the table of a disk: GPT when there is a good one, else MBR, else none. Never fails: problems become `notes`.
pub fn read_disk_table<R: Read + Seek>(r: &mut R, ss: u32, size_bytes: u64) -> TableInfo {
    let total = size_bytes / ss as u64;
    let mut info = TableInfo { kind: Kind::None, sector_size: ss, parts: vec![], notes: vec![] };
    let Ok(s0) = read_at(r, 0, ss.max(512) as usize) else { return info };
    let mbr_ok = s0[510..512] == [0x55, 0xAA];
    let protective = mbr_ok && (0..4).any(|i| s0[446 + 16 * i + 4] == 0xEE);
    let gpt_header_at_1 = read_at(r, ss as u64, 8).is_ok_and(|b| &b[..] == SIGNATURE);
    if gpt_header_at_1 || protective || read_at(r, total.saturating_sub(1) * ss as u64, 8).is_ok_and(|b| &b[..] == SIGNATURE) {
        match read_gpt(r, ss, total) {
            Ok(p) => {
                info.kind = Kind::Gpt;
                info.parts = p.table.entries.iter().map(|e| PartInfo { index: e.index, first_lba: e.first_lba, sectors: e.sectors(), type_name: type_name(&e.type_guid), label: e.name.clone() }).collect();
                if !p.primary_ok {
                    info.notes.push("the primary GPT is damaged; the backup copy was read".into());
                } else if !p.backup_ok {
                    info.notes.push("the backup GPT is missing or damaged".into());
                }
                return info;
            }
            Err(e) => {
                info.notes.push(format!("a GPT is present but unreadable ({e})"));
                if !mbr_ok {
                    info.kind = Kind::Gpt;
                    return info;
                }
            }
        }
    }
    if mbr_ok {
        info.kind = Kind::Mbr;
        for i in 0..4 {
            let e = 446 + 16 * i;
            let (t, start, n) = (s0[e + 4], le32(&s0, e + 8) as u64, le32(&s0, e + 12) as u64);
            if t != 0 && n != 0 && t != 0xEE {
                info.parts.push(PartInfo { index: i as u32 + 1, first_lba: start, sectors: n, type_name: mbr_type_name(t).into(), label: String::new() });
            }
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample(ss: u32, total: u64) -> Table {
        let al = 1024 * 1024 / ss as u64;
        let mut t = Table { sector_size: ss, total_sectors: total, disk_guid: guid_from_str("11111111-2222-3333-4444-555555555555").unwrap(), entries: vec![] };
        let mut at = al;
        for (i, (ty, len, name)) in [(TYPE_BIOS_BOOT, al, "BIOS"), (TYPE_ESP, 4 * al, "IYAKKI-BOOT"), (TYPE_LINUX_DATA, 0, "IYAKKI-DATA")].into_iter().enumerate() {
            let last = if len == 0 { (t.last_usable() + 1) / al * al - 1 } else { at + len - 1 };
            t.entries.push(Entry { index: i as u32 + 1, type_guid: ty, guid: [i as u8 + 1; 16], first_lba: at, last_lba: last, attrs: 0, name: name.into() });
            at = last + 1;
        }
        t
    }

    /// The whole disk image as a vector (small disks only).
    fn image(t: &Table) -> Vec<u8> {
        let ss = t.sector_size as usize;
        let mut d = vec![0u8; t.total_sectors as usize * ss];
        d[..ss].copy_from_slice(&t.protective_mbr(&[], [1, 2, 3, 4]));
        d[ss..2 * ss].copy_from_slice(&t.header_bytes(false));
        let e = t.entries_bytes();
        d[2 * ss..2 * ss + e.len()].copy_from_slice(&e);
        let b = t.backup_entries_lba() as usize * ss;
        d[b..b + e.len()].copy_from_slice(&e);
        let l = t.last_lba() as usize * ss;
        d[l..l + ss].copy_from_slice(&t.header_bytes(true));
        d
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn guids_have_the_on_disk_byte_order() {
        assert_eq!(&TYPE_ESP[..8], &[0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11]);
        assert_eq!(guid_to_string(&TYPE_ESP), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
        assert_eq!(guid_to_string(&TYPE_BIOS_BOOT), "21686148-6449-6E6F-744E-656564454649");
        assert_eq!(guid_to_string(&TYPE_LINUX_DATA), "0FC63DAF-8483-4772-8E79-3D69D8477DE4");
        assert_eq!(guid_from_str("0fc63daf-8483-4772-8e79-3d69d8477de4"), Some(TYPE_LINUX_DATA));
        assert_eq!(guid_from_str("nope"), None);
        let (a, b) = (random_guid(), random_guid());
        assert_ne!(a, b);
        assert_eq!((a[7] >> 4, a[8] >> 6), (4, 2), "version 4, RFC 4122 variant");
    }

    #[test]
    fn the_header_has_the_documented_fields() {
        let t = sample(512, 1 << 20);
        let h = t.header_bytes(false);
        assert_eq!(&h[0..8], b"EFI PART");
        assert_eq!((le32(&h, 8), le32(&h, 12), le32(&h, 20)), (0x0001_0000, 92, 0));
        assert_eq!((le64(&h, 24), le64(&h, 32)), (1, (1 << 20) - 1), "primary at LBA 1, backup at the very last sector");
        assert_eq!((le64(&h, 40), le64(&h, 48)), (34, (1 << 20) - 34));
        assert_eq!(le64(&h, 72), 2);
        assert_eq!((le32(&h, 80), le32(&h, 84)), (128, 128));
        assert_eq!(le32(&h, 88), crc32(&t.entries_bytes()), "entry array CRC");
        let mut z = h[..92].to_vec();
        z[16..20].fill(0);
        assert_eq!(le32(&h, 16), crc32(&z), "header CRC computed with its own field zero");
        assert!(h[92..].iter().all(|b| *b == 0));
        let b = t.header_bytes(true);
        assert_eq!((le64(&b, 24), le64(&b, 32), le64(&b, 72)), ((1 << 20) - 1, 1, (1 << 20) - 1 - 32), "the backup points back, its array sits just before it");
        assert_eq!(t.entries_bytes().len(), 16384);
        let e = &t.entries_bytes()[128..256];
        assert_eq!(&e[0..16], &TYPE_ESP);
        assert_eq!(&e[56..58], &[b'I', 0], "names are UTF-16LE");
    }

    #[test]
    fn the_protective_mbr_covers_the_disk() {
        let t = sample(512, 1 << 20);
        let m = t.protective_mbr(&[0xAB; 440], [9, 8, 7, 6]);
        assert_eq!((&m[..440], &m[440..444], m[450], m[510], m[511]), (&[0xAB; 440][..], &[9, 8, 7, 6][..], 0xEE, 0x55, 0xAA));
        assert_eq!((le32(&m, 454), le32(&m, 458)), (1, (1 << 20) - 1));
        let big = Table { total_sectors: 8 << 30, ..t.clone() }; // 4 TiB of 512-byte sectors
        assert_eq!(le32(&big.protective_mbr(&[], [0; 4]), 458), 0xFFFF_FFFF, "capped at what an MBR can say");
        assert_eq!(Table { sector_size: 4096, total_sectors: 1 << 18, ..t }.protective_mbr(&[], [0; 4]).len(), 4096);
    }

    #[test]
    fn what_we_write_is_read_back_for_both_sector_sizes() {
        for (ss, total) in [(512u32, 64 * 2048u64 + 7), (4096, 64 * 256)] {
            let t = sample(ss, total);
            t.check().unwrap();
            let img = image(&t);
            let p = read_gpt(&mut Cursor::new(&img), ss, total).unwrap();
            assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
            assert_eq!(p.table, t, "ss={ss}");
            let info = read_disk_table(&mut Cursor::new(&img), ss, total * ss as u64);
            assert_eq!((info.kind, info.parts.len()), (Kind::Gpt, 3));
            assert_eq!((info.parts[1].type_name.as_str(), info.parts[1].label.as_str()), ("EFI System", "IYAKKI-BOOT"));
            assert_eq!(info.parts[0].type_name, "BIOS boot");
            assert_eq!(info.parts[2].type_name, "Linux filesystem");
        }
    }

    #[test]
    fn damage_is_detected_and_one_good_copy_is_enough() {
        let (ss, total) = (512u32, 64 * 2048u64);
        let t = sample(ss, total);
        let good = image(&t);
        let read = |d: &[u8]| read_gpt(&mut Cursor::new(d.to_vec()), ss, total);
        // a flipped byte in the primary entries
        let mut d = good.clone();
        d[2 * 512 + 40] ^= 1;
        let p = read(&d).unwrap();
        assert!(!p.primary_ok && p.backup_ok && p.problems[0].contains("primary entries") && p.problems[0].contains("CRC32"), "{:?}", p.problems);
        // a flipped byte in the backup header
        let mut d = good.clone();
        d[(total as usize - 1) * 512 + 60] ^= 1;
        let p = read(&d).unwrap();
        assert!(p.primary_ok && !p.backup_ok && p.problems[0].contains("backup header"), "{:?}", p.problems);
        // both broken
        let mut d = good.clone();
        d[512 + 60] ^= 1;
        d[(total as usize - 1) * 512 + 60] ^= 1;
        assert!(read(&d).unwrap_err().contains("CRC32"));
        // the backup is not at the end of the disk (a table made for a smaller disk copied onto a bigger one)
        let small = image(&sample(ss, total / 2));
        let mut moved = vec![0u8; total as usize * 512];
        moved[..small.len()].copy_from_slice(&small);
        let e = read(&moved).unwrap_err();
        assert!(e.contains("end of the disk"), "a stale layout is not accepted: {e}");
        // wrong signature, blank disk
        let mut d = good.clone();
        d[512] = b'X';
        d[(total as usize - 1) * 512] = b'X';
        assert!(read(&d).unwrap_err().contains("signature"));
        assert!(read(&vec![0u8; total as usize * 512]).is_err());
        // a header that lies about the entry size cannot make us read far away
        let mut d = good.clone();
        d[512 + 84..512 + 88].copy_from_slice(&u32::MAX.to_le_bytes());
        let c = crc32(&{ let mut z = d[512..512 + 92].to_vec(); z[16..20].fill(0); z });
        d[512 + 16..512 + 20].copy_from_slice(&c.to_le_bytes());
        assert!(read(&d).unwrap().problems[0].contains("unsupported entry array"));
        // overlapping partitions are refused even with valid checksums
        let mut o = t.clone();
        o.entries[1].first_lba = o.entries[0].first_lba;
        assert!(o.check().unwrap_err().contains("overlap"));
        assert!(read_gpt(&mut Cursor::new(image(&o)), ss, total).unwrap_err().contains("overlap"));
        // too small to hold a GPT
        assert!(read_gpt(&mut Cursor::new(vec![0u8; 4096]), 512, 8).is_err());
    }

    #[test]
    fn an_mbr_disk_is_described_too() {
        let mut d = vec![0u8; 1 << 20];
        d[446 + 4] = 0x07;
        d[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
        d[446 + 12..446 + 16].copy_from_slice(&4096u32.to_le_bytes());
        d[462 + 4] = 0x83;
        d[462 + 8..462 + 12].copy_from_slice(&8192u32.to_le_bytes());
        d[462 + 12..462 + 16].copy_from_slice(&2048u32.to_le_bytes());
        d[510] = 0x55;
        d[511] = 0xAA;
        let i = read_disk_table(&mut Cursor::new(&d), 512, d.len() as u64);
        assert_eq!(i.kind, Kind::Mbr);
        assert_eq!((i.parts[0].type_name.as_str(), i.parts[0].sectors, i.parts[1].type_name.as_str(), i.parts[1].index), ("NTFS/exFAT", 4096, "Linux", 2));
        let blank = read_disk_table(&mut Cursor::new(vec![0u8; 1 << 20]), 512, 1 << 20);
        assert_eq!((blank.kind, blank.parts.len()), (Kind::None, 0));
        // a protective MBR whose GPT is gone is reported, not trusted
        let t = sample(512, 64 * 2048);
        let mut img = image(&t);
        img[512..1024].fill(0);
        img[(t.total_sectors as usize - 1) * 512..].fill(0);
        let i = read_disk_table(&mut Cursor::new(&img), 512, img.len() as u64);
        assert!(i.notes.iter().any(|n| n.contains("unreadable")), "{:?}", i.notes);
        // a backup table gone while the primary is fine
        let mut img = image(&t);
        let n = img.len();
        img[n - 512..].fill(0);
        let i = read_disk_table(&mut Cursor::new(&img), 512, img.len() as u64);
        assert_eq!((i.kind, i.parts.len()), (Kind::Gpt, 3));
        assert!(i.notes[0].contains("backup"));
    }

    #[test]
    fn type_names_are_readable() {
        assert_eq!(type_name(&TYPE_ESP), "EFI System");
        assert_eq!(type_name(&guid_from_str("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7").unwrap()), "Microsoft basic data");
        assert_eq!(type_name(&[7; 16]), guid_to_string(&[7; 16]));
    }
}
