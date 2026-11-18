//! File lists of an ISO 9660 image (Joliet names when the image has them) and of a FAT12/16/32 volume image, read with no dependency
//! and nothing extracted (F-QA-05). The release image scan takes the same [`Entry`] list from them as from a `rootfs.cpio`.
//! Reading only. Every length and pointer comes from the image, so each read is bounded and loops (directory cycles, cluster chains
//! that point back) are cut. Rock Ridge links and attributes are not read; they are not needed to name the files.

use std::collections::HashSet;

use crate::cpio::{norm, Entry, Kind};

const MAX_ENTRIES: usize = 1_000_000;
const MAX_DEPTH: usize = 40;

fn le16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// What kind of image these bytes start with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniff {
    Cpio,
    Iso,
    Fat,
    /// A whole disk image: a partition table (MBR or GPT) with volumes in it.
    Disk,
    Zstd,
    Unknown,
}

/// Look at the start of a file (the first 64 KiB is enough; an ISO's marker is at byte 32769).
pub fn sniff(head: &[u8]) -> Sniff {
    if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Sniff::Zstd
    } else if head.starts_with(b"070701") || head.starts_with(b"070702") {
        Sniff::Cpio
    } else if head.get(0x8001..0x8006) == Some(b"CD001") {
        Sniff::Iso
    } else if is_fat_boot(head) {
        Sniff::Fat
    } else if partitions(head).is_some_and(|p| !p.is_empty()) || (head.len() >= 520 && head.get(512..520) == Some(b"EFI PART")) {
        Sniff::Disk
    } else {
        Sniff::Unknown
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Partition tables (a whole disk image)
// ---------------------------------------------------------------------------------------------------------------------

/// The most partitions looked at in one disk image.
const MAX_PARTITIONS: usize = 128;

/// The volumes of a disk image's first sectors: `(first byte, length in bytes)` of each non-empty partition, from the GPT when there is one
/// (a protective MBR in front of it is not a volume), else from the four MBR entries. None when there is no table at all. Needs at least the
/// first 34 sectors for a GPT; the entries are read from where the header says, which may be later, so pass the whole image when it is known.
pub fn partitions(img: &[u8]) -> Option<Vec<(usize, usize)>> {
    if img.len() < 512 || img[510] != 0x55 || img[511] != 0xAA {
        return None;
    }
    let mbr: Vec<(u8, u32, u32)> = (0..4).filter_map(|i| {
        let e = img.get(446 + i * 16..446 + i * 16 + 16)?;
        let (ty, start, len) = (e[4], le32(e, 8)?, le32(e, 12)?);
        (ty != 0 && len != 0).then_some((ty, start, len))
    }).collect();
    if mbr.iter().any(|m| m.0 == 0xEE) {
        // GPT: the header at sector 1, then a table of entries
        let h = img.get(512..512 + 92)?;
        if h.get(0..8)? != b"EFI PART" {
            return None;
        }
        let (table_lba, n, size) = (le64(h, 72)? as usize, le32(h, 80)? as usize, le32(h, 84)? as usize);
        if !(128..=1024).contains(&size) || n > MAX_PARTITIONS * 8 {
            return None;
        }
        let mut out = Vec::new();
        for i in 0..n {
            let o = table_lba.checked_mul(512)?.checked_add(i.checked_mul(size)?)?;
            let Some(e) = img.get(o..o + 48) else { break };
            if e[..16].iter().all(|b| *b == 0) {
                continue;
            }
            let (first, last) = (le64(e, 32)? as usize, le64(e, 40)? as usize);
            if last < first {
                continue;
            }
            out.push((first.checked_mul(512)?, (last - first + 1).checked_mul(512)?));
            if out.len() >= MAX_PARTITIONS {
                break;
            }
        }
        return Some(out);
    }
    Some(mbr.into_iter().filter(|m| !matches!(m.0, 0x05 | 0x0F | 0x85)).map(|(_, s, l)| (s as usize * 512, l as usize * 512)).collect())
}

fn le64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Every file and folder of every volume of a whole disk image that this module can read (FAT and ISO), named `p1/...`, `p2/...` by the
/// partition's number. A partition of another kind (ext4, swap, a blank one) is skipped and said in the second answer; a volume that is cut
/// short or damaged is an error for the whole image: a scan must not pass what it could not read.
pub fn disk_list(img: &[u8]) -> Result<(Vec<Entry>, Vec<String>), String> {
    let parts = partitions(img).ok_or("no partition table in this image")?;
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for (n, (start, len)) in parts.into_iter().enumerate() {
        let n = n + 1;
        let end = start.checked_add(len).ok_or("a partition lies outside the image")?;
        let vol = img.get(start..end).ok_or_else(|| format!("partition {n} lies outside the image (cut short?)"))?;
        let list = match sniff(vol) {
            Sniff::Fat => fat_list(vol).map_err(|e| format!("partition {n}: {e}"))?,
            Sniff::Iso => iso_list(vol).map_err(|e| format!("partition {n}: {e}"))?,
            other => {
                skipped.push(format!("partition {n} ({:?}, {} MiB) is not a volume this scan can open", other, len >> 20));
                continue;
            }
        };
        for mut e in list {
            e.path = format!("p{n}/{}", e.path);
            if out.len() >= MAX_ENTRIES {
                return Err("more entries than any real image holds".into());
            }
            out.push(e);
        }
    }
    Ok((out, skipped))
}

// ---------------------------------------------------------------------------------------------------------------------
// ISO 9660
// ---------------------------------------------------------------------------------------------------------------------

struct Iso<'a> {
    img: &'a [u8],
    joliet: bool,
    out: Vec<Entry>,
    seen: HashSet<u32>,
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&u)
}

impl Iso<'_> {
    fn dir(&mut self, extent: u32, len: u32, prefix: &str, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the directories are nested deeper than any real image".into());
        }
        if !self.seen.insert(extent) {
            return Err("a directory points back to one above it".into());
        }
        let start = (extent as usize).checked_mul(2048).ok_or("directory outside the image")?;
        let end = start.checked_add(len as usize).ok_or("directory outside the image")?;
        let d = self.img.get(start..end).ok_or("a directory lies outside the image (cut short?)")?;
        let mut at = 0usize;
        while at < d.len() {
            let rl = d[at] as usize;
            if rl == 0 {
                at = (at / 2048 + 1) * 2048; // records never straddle a sector; zero pads the rest
                continue;
            }
            let r = d.get(at..at + rl).filter(|r| r.len() >= 34).ok_or("a directory record is cut short")?;
            at += rl;
            let nl = r[32] as usize;
            let raw = r.get(33..33 + nl).ok_or("a name is cut short")?;
            if raw == [0] || raw == [1] {
                continue;
            }
            let mut name = if self.joliet { utf16be(raw) } else { String::from_utf8_lossy(raw).into_owned() };
            if let Some(i) = name.rfind(';') {
                name.truncate(i);
            }
            if name.ends_with('.') {
                name.pop();
            }
            let name: String = name.chars().filter(|c| !c.is_control() && *c != '/').collect();
            if name.is_empty() {
                continue;
            }
            let path = if prefix.is_empty() { name } else { format!("{prefix}/{}", name) };
            let (ext, size) = (le32(r, 2).ok_or("record cut short")?, le32(r, 10).ok_or("record cut short")?);
            let is_dir = r[25] & 2 != 0;
            if self.out.len() >= MAX_ENTRIES {
                return Err("more entries than any real image holds".into());
            }
            self.out.push(Entry { path: norm(&path), kind: if is_dir { Kind::Dir } else { Kind::File }, size: u64::from(size), mode: 0, target: String::new() });
            if is_dir {
                self.dir(ext, size, &path, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// Every file and folder of an ISO 9660 image held in memory (the Joliet tree when there is one, else the primary one).
pub fn iso_list(img: &[u8]) -> Result<Vec<Entry>, String> {
    let mut primary = None;
    let mut joliet = None;
    for n in 16..64usize {
        let Some(d) = img.get(n * 2048..n * 2048 + 2048) else { break };
        if &d[1..6] != b"CD001" {
            return Err("not an ISO 9660 image".into());
        }
        match d[0] {
            1 => primary = primary.or(Some(d)),
            2 if matches!(&d[88..91], b"%/@" | b"%/C" | b"%/E") => joliet = joliet.or(Some(d)),
            255 => break,
            _ => {}
        }
    }
    let (pvd, is_j) = joliet.map(|d| (d, true)).or(primary.map(|d| (d, false))).ok_or("not an ISO 9660 image (no volume descriptor)")?;
    let root = &pvd[156..190];
    let (ext, len) = (le32(root, 2).ok_or("bad root")?, le32(root, 10).ok_or("bad root")?);
    let mut it = Iso { img, joliet: is_j, out: Vec::new(), seen: HashSet::new() };
    it.dir(ext, len, "", 0)?;
    Ok(it.out)
}

// ---------------------------------------------------------------------------------------------------------------------
// FAT
// ---------------------------------------------------------------------------------------------------------------------

fn is_fat_boot(b: &[u8]) -> bool {
    if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
        return false;
    }
    let (bps, spc) = (le16(b, 11).unwrap_or(0), b[13]);
    matches!(bps, 512 | 1024 | 2048 | 4096) && spc.is_power_of_two() && b[16] >= 1 && le16(b, 14).unwrap_or(0) >= 1 && (b.get(54..57) == Some(b"FAT") || b.get(82..85) == Some(b"FAT"))
}

struct Fat<'a> {
    img: &'a [u8],
    bps: usize,
    spc: usize,
    fat_start: usize,
    data_start: usize,
    clusters: u32,
    kind: u8, // 12, 16, 32
    out: Vec<Entry>,
}

impl Fat<'_> {
    fn next(&self, c: u32) -> Option<u32> {
        let v = match self.kind {
            12 => {
                let o = self.fat_start + (c as usize) * 3 / 2;
                let w = u32::from(le16(self.img, o)?);
                if c & 1 == 0 { w & 0xFFF } else { w >> 4 }
            }
            16 => u32::from(le16(self.img, self.fat_start + c as usize * 2)?),
            _ => le32(self.img, self.fat_start + c as usize * 4)? & 0x0FFF_FFFF,
        };
        let eoc = match self.kind { 12 => 0xFF8, 16 => 0xFFF8, _ => 0x0FFF_FFF8 };
        if v >= eoc || v < 2 || v >= self.clusters + 2 { None } else { Some(v) }
    }

    fn chain(&self, first: u32) -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        let mut c = first;
        let mut seen = HashSet::new();
        loop {
            if c < 2 || c >= self.clusters + 2 || !seen.insert(c) {
                return Err("a directory's cluster chain loops or leaves the volume".into());
            }
            let o = self.data_start + (c as usize - 2) * self.spc * self.bps;
            data.extend_from_slice(self.img.get(o..o + self.spc * self.bps).ok_or("a directory lies outside the image (cut short?)")?);
            if data.len() > 64 << 20 {
                return Err("a directory larger than 64 MiB".into());
            }
            match self.next(c) {
                Some(n) => c = n,
                None => return Ok(data),
            }
        }
    }

    fn entries(&mut self, dir: &[u8], prefix: &str, depth: usize, seen: &mut HashSet<u32>) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the folders are nested deeper than any real volume".into());
        }
        let mut lfn: Vec<(u8, Vec<u16>)> = Vec::new();
        for e in dir.chunks_exact(32) {
            if e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 {
                lfn.clear();
                continue;
            }
            let attr = e[11];
            if attr == 0x0F {
                let mut u: Vec<u16> = Vec::with_capacity(13);
                for r in [1..11, 14..26, 28..32] {
                    u.extend(e[r].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
                }
                lfn.push((e[0] & 0x1F, u));
                continue;
            }
            if attr & 0x08 != 0 {
                lfn.clear();
                continue;
            }
            let name = if lfn.is_empty() {
                let base = String::from_utf8_lossy(&e[..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                let mut s = if ext.is_empty() { base } else { format!("{base}.{ext}") };
                if e[0] == 0x05 {
                    s.replace_range(..1, "\u{e5}");
                }
                s
            } else {
                lfn.sort_by_key(|p| p.0);
                let u: Vec<u16> = lfn.iter().flat_map(|p| p.1.iter().copied()).take_while(|c| *c != 0 && *c != 0xFFFF).collect();
                String::from_utf16_lossy(&u)
            };
            lfn.clear();
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let name: String = name.chars().filter(|c| !c.is_control() && *c != '/').collect();
            let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let first = (u32::from(le16(e, 20).unwrap_or(0)) << 16) | u32::from(le16(e, 26).unwrap_or(0));
            let size = le32(e, 28).unwrap_or(0);
            let is_dir = attr & 0x10 != 0;
            if self.out.len() >= MAX_ENTRIES {
                return Err("more entries than any real volume holds".into());
            }
            self.out.push(Entry { path: norm(&path), kind: if is_dir { Kind::Dir } else { Kind::File }, size: u64::from(size), mode: 0, target: String::new() });
            if is_dir && first >= 2 {
                if !seen.insert(first) {
                    return Err("a folder points back to one above it".into());
                }
                let d = self.chain(first)?;
                self.entries(&d, &path, depth + 1, seen)?;
            }
        }
        Ok(())
    }
}

/// Every file and folder of a FAT12/16/32 volume image (a bare file system, not a whole disk with a partition table).
pub fn fat_list(img: &[u8]) -> Result<Vec<Entry>, String> {
    if !is_fat_boot(img) {
        return Err("not a FAT volume image (a whole disk image with a partition table must be cut at the partition first)".into());
    }
    let bps = le16(img, 11).ok_or("short")? as usize;
    let spc = img[13] as usize;
    let reserved = le16(img, 14).ok_or("short")? as usize;
    let nfats = img[16] as usize;
    let root_entries = le16(img, 17).ok_or("short")? as usize;
    let total = match le16(img, 19).ok_or("short")? { 0 => le32(img, 32).ok_or("short")? as usize, n => n as usize };
    let fat_size = match le16(img, 22).ok_or("short")? { 0 => le32(img, 36).ok_or("short")? as usize, n => n as usize };
    if nfats == 0 || fat_size == 0 || total == 0 {
        return Err("the FAT boot sector holds values that make no sense".into());
    }
    let root_sectors = (root_entries * 32).div_ceil(bps);
    let data_sector = reserved + nfats * fat_size + root_sectors;
    let clusters = (total.saturating_sub(data_sector) / spc) as u32;
    // FAT32 is told by the boot sector (no root folder entries and a 32-bit FAT size), as the kernel does, not by the number of clusters: `mkfs.fat -F 32` makes volumes under 32 MiB with fewer than 65525 clusters
    let fat32 = root_entries == 0 && le16(img, 22) == Some(0);
    let kind = if fat32 { 32 } else if clusters < 4085 { 12 } else { 16 };
    let mut f = Fat { img, bps, spc, fat_start: reserved * bps, data_start: data_sector * bps, clusters, kind, out: Vec::new() };
    let mut seen = HashSet::new();
    let root = if kind == 32 {
        let rc = le32(img, 44).ok_or("short")?;
        seen.insert(rc);
        f.chain(rc)?
    } else {
        let o = (reserved + nfats * fat_size) * bps;
        img.get(o..o + root_entries * 32).ok_or("the root folder lies outside the image (cut short?)")?.to_vec()
    };
    f.entries(&root, "", 0, &mut seen)?;
    Ok(f.out)
}

/// Read the file list of an image file of any kind this module and `cpio` know, by looking at its first bytes. The bool says whether
/// it is a root file system image (the notice-file check applies) or not. A `.zst` is refused with the way to unpack it.
pub fn list_file(path: &std::path::Path) -> Result<(Vec<Entry>, bool), String> {
    use std::io::Read;
    let mut head = vec![0u8; 65536];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| format!("{}: {e}", path.display()))?;
    head.truncate(n);
    let read = || std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    match sniff(&head) {
        Sniff::Cpio => crate::cpio::list(std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?)).map(|e| (e, true)).map_err(|e| e.to_string()),
        Sniff::Iso => iso_list(&read()?).map(|e| (e, false)),
        Sniff::Fat => fat_list(&read()?).map(|e| (e, false)),
        Sniff::Disk => {
            let (list, skipped) = disk_list(&read()?)?;
            for s in skipped {
                eprintln!("note: {s}");
            }
            Ok((list, false))
        }
        Sniff::Zstd => Err("this is zstd-compressed; use dist/release/image-gate.sh, or `zstd -dc FILE | iyakki-relcheck image-scan --cpio -`".into()),
        Sniff::Unknown => Err("not a cpio archive, ISO 9660 image or FAT volume image".into()),
    }
}
