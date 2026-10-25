//! Adding files to a FAT32 volume that belongs to someone else (the other system's EFI System Partition), without mounting it.
//!
//! This is the "keep the existing ESP" half of the dual-boot install. It is deliberately small and careful:
//!
//! * it never changes an existing directory entry, file or directory; it only fills **free** directory slots (extending a
//!   directory by one cluster when it is full) and claims **free** clusters;
//! * the order is data first, then the FAT (every copy), then the directory entry last, so a power loss leaves at worst some
//!   unreferenced clusters (a file system check reclaims them), never an entry that points at nothing;
//! * names are 8.3 (upper case); matching of existing names is case-insensitive and understands long names;
//! * FAT32 only (FAT12/16 ESPs are refused by the planner).
//!
//! The kernel's FAT driver must not have the volume mounted: the planner refuses a mounted ESP.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::FileExt;

const EOC: u32 = 0x0FFF_FFFF;

fn le16(b: &[u8], o: usize) -> u64 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u64
}
fn le32(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64
}

pub struct Fat32Rw<'a> {
    f: &'a File,
    base: u64,
    bps: u64,
    spc: u64,
    reserved: u64,
    nfats: u64,
    fatsz: u64,
    data_start: u64,
    clusters: u32,
    root: u32,
    fsinfo: Option<u64>,
    fat: Vec<u32>,
    dirty: BTreeSet<u64>,
    next_free: u32,
    allocated: u64,
}

/// The 11-byte short name for `NAME.EXT` (upper case, 8.3).
pub fn short_name(name: &str) -> Result<[u8; 11], String> {
    let (b, x) = name.split_once('.').unwrap_or((name, ""));
    let ok = |s: &str, max: usize| s.len() <= max && s.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || b"_-$~!#%&'()@^{}".contains(&c));
    if b.is_empty() || !ok(b, 8) || !ok(x, 3) {
        return Err(format!("\"{name}\" is not an upper-case 8.3 name"));
    }
    let mut n = [b' '; 11];
    n[..b.len()].copy_from_slice(b.as_bytes());
    n[8..8 + x.len()].copy_from_slice(x.as_bytes());
    Ok(n)
}

fn civil(days: i64) -> (i64, u32, u32) {
    // days since 1970-01-01 -> (year, month, day)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (FAT date, FAT time) of now; 1980-01-01 when the clock is before that.
fn fat_now() -> (u16, u16) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (y, m, d) = civil(secs.div_euclid(86400));
    if y < 1980 {
        return (0x21, 0);
    }
    let t = secs.rem_euclid(86400);
    (((((y - 1980) as u16).min(127)) << 9) | ((m as u16) << 5) | d as u16, (((t / 3600) as u16) << 11) | ((((t / 60) % 60) as u16) << 5) | ((t % 60 / 2) as u16))
}

impl<'a> Fat32Rw<'a> {
    pub fn open(f: &'a File, base: u64, len: u64) -> Result<Fat32Rw<'a>, String> {
        let mut b = vec![0u8; 512];
        f.read_exact_at(&mut b, base).map_err(|e| format!("reading the EFI System Partition: {e}"))?;
        let (bps, spc, reserved, nfats) = (le16(&b, 11), b[13] as u64, le16(&b, 14), b[16] as u64);
        let (fatsz, total) = (le32(&b, 36), le32(&b, 32));
        if b[510..512] != [0x55, 0xAA] || &b[82..87] != b"FAT32" || le16(&b, 22) != 0 || fatsz == 0 || !matches!(bps, 512 | 1024 | 2048 | 4096) || spc == 0 || !spc.is_power_of_two() || nfats == 0 || nfats > 2 {
            return Err("the EFI System Partition is not a FAT32 volume".into());
        }
        if total * bps > len {
            return Err("the FAT32 volume claims to be larger than its partition; refused".into());
        }
        let data_start = reserved + nfats * fatsz;
        if total <= data_start {
            return Err("the EFI System Partition is damaged (no data area)".into());
        }
        let clusters = ((total - data_start) / spc) as u32;
        let mut raw = vec![0u8; (fatsz * bps) as usize];
        f.read_exact_at(&mut raw, base + reserved * bps).map_err(|e| format!("reading the FAT: {e}"))?;
        let n = (clusters as usize + 2).min(raw.len() / 4);
        let fat: Vec<u32> = (0..n).map(|i| le32(&raw, 4 * i) as u32).collect();
        let fs = le16(&b, 48);
        Ok(Fat32Rw { f, base, bps, spc, reserved, nfats, fatsz, data_start, clusters, root: le32(&b, 44) as u32, fsinfo: (fs != 0 && fs != 0xFFFF).then_some(fs), fat, dirty: BTreeSet::new(), next_free: 2, allocated: 0 })
    }

    pub fn cluster_bytes(&self) -> u64 {
        self.bps * self.spc
    }

    /// Free bytes counted from the FAT itself (not from FSInfo).
    pub fn free_bytes(&self) -> u64 {
        self.fat.iter().skip(2).take(self.clusters as usize).filter(|e| **e & 0x0FFF_FFFF == 0).count() as u64 * self.cluster_bytes()
    }

    fn offset(&self, c: u32) -> u64 {
        self.base + (self.data_start + (c as u64 - 2) * self.spc) * self.bps
    }

    fn entry(&self, c: u32) -> u32 {
        self.fat.get(c as usize).map_or(EOC, |e| e & 0x0FFF_FFFF)
    }

    fn set_entry(&mut self, c: u32, v: u32) {
        let keep = self.fat[c as usize] & 0xF000_0000;
        self.fat[c as usize] = keep | (v & 0x0FFF_FFFF);
        self.dirty.insert(c as u64 * 4 / self.bps);
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>, String> {
        let mut v = Vec::new();
        let mut c = first;
        while (2..0x0FFF_FFF0).contains(&c) {
            if c as usize >= self.fat.len() || v.len() > self.clusters as usize {
                return Err("the EFI System Partition has a broken cluster chain; refused".into());
            }
            v.push(c);
            c = self.entry(c);
        }
        Ok(v)
    }

    fn alloc(&mut self, n: u64) -> Result<Vec<u32>, String> {
        let mut out = Vec::with_capacity(n as usize);
        let top = self.fat.len() as u32;
        let mut c = self.next_free.max(2);
        let mut scanned = 0u32;
        while (out.len() as u64) < n {
            if scanned > top {
                return Err("the EFI System Partition is full".into());
            }
            if c >= top {
                c = 2;
            }
            if self.entry(c) == 0 && !out.contains(&c) {
                out.push(c);
            }
            c += 1;
            scanned += 1;
        }
        for (i, &x) in out.iter().enumerate() {
            let next = out.get(i + 1).copied().unwrap_or(EOC);
            self.set_entry(x, next);
        }
        self.next_free = out.last().map_or(self.next_free, |l| l + 1);
        self.allocated += n;
        Ok(out)
    }

    /// Write the changed FAT sectors (every copy) and the FSInfo counters.
    pub fn flush(&mut self) -> Result<(), String> {
        let ss = self.bps as usize;
        for &s in &self.dirty {
            let mut buf = vec![0u8; ss];
            for k in 0..ss / 4 {
                let i = s as usize * (ss / 4) + k;
                let v = self.fat.get(i).copied().unwrap_or(0);
                buf[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
            }
            // entries beyond what we parsed (a FAT larger than the cluster count) keep their bytes
            let mut old = vec![0u8; ss];
            let first = self.base + (self.reserved) * self.bps + s * self.bps;
            self.f.read_exact_at(&mut old, first).map_err(|e| format!("reading the FAT: {e}"))?;
            let have = self.fat.len().saturating_sub(s as usize * (ss / 4)).min(ss / 4);
            buf[have * 4..].copy_from_slice(&old[have * 4..]);
            for k in 0..self.nfats {
                let at = self.base + (self.reserved + k * self.fatsz) * self.bps + s * self.bps;
                self.f.write_all_at(&buf, at).map_err(|e| format!("writing the FAT: {e}"))?;
            }
        }
        self.dirty.clear();
        self.f.sync_data().map_err(|e| format!("flushing the EFI System Partition: {e}"))?;
        Ok(())
    }

    /// Update the FSInfo free-cluster count and next-free hint (cosmetic for readers; done last).
    pub fn write_fsinfo(&mut self) -> Result<(), String> {
        let Some(fs) = self.fsinfo else { return Ok(()) };
        let at = self.base + fs * self.bps;
        let mut b = vec![0u8; 512];
        self.f.read_exact_at(&mut b, at).map_err(|e| e.to_string())?;
        if le32(&b, 0) != 0x4161_5252 || le32(&b, 484) != 0x6141_7272 {
            return Ok(());
        }
        let free = le32(&b, 488);
        if free != 0xFFFF_FFFF {
            let nf = free.saturating_sub(self.allocated) as u32;
            b[488..492].copy_from_slice(&nf.to_le_bytes());
        }
        b[492..496].copy_from_slice(&self.next_free.to_le_bytes());
        self.f.write_all_at(&b, at).map_err(|e| format!("writing FSInfo: {e}"))?;
        Ok(())
    }

    /// Look up `name` in the directory starting at cluster `dir`: (first cluster, is a directory).
    pub fn find(&self, dir: u32, name: &str) -> Result<Option<(u32, bool)>, String> {
        let want = name.to_ascii_uppercase();
        let mut lfn: Vec<u16> = Vec::new();
        for c in self.chain(dir)? {
            let mut buf = vec![0u8; self.cluster_bytes() as usize];
            self.f.read_exact_at(&mut buf, self.offset(c)).map_err(|e| e.to_string())?;
            for e in buf.chunks_exact(32) {
                if e[0] == 0 {
                    return Ok(None);
                }
                if e[0] == 0xE5 {
                    lfn.clear();
                    continue;
                }
                if e[11] == 0x0F {
                    let ord = (e[0] & 0x1F) as usize;
                    if e[0] & 0x40 != 0 {
                        lfn = vec![0xFFFF; ord * 13];
                    }
                    if ord >= 1 && lfn.len() >= ord * 13 {
                        for (k, o) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
                            lfn[(ord - 1) * 13 + k] = u16::from_le_bytes([e[*o], e[*o + 1]]);
                        }
                    }
                    continue;
                }
                if e[11] & 0x08 != 0 {
                    lfn.clear();
                    continue;
                }
                let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                let sfn = if ext.is_empty() { base } else { format!("{base}.{ext}") };
                let long: String = String::from_utf16_lossy(&lfn.iter().copied().take_while(|u| *u != 0 && *u != 0xFFFF).collect::<Vec<_>>());
                lfn.clear();
                if sfn.to_ascii_uppercase() == want || (!long.is_empty() && long.to_ascii_uppercase() == want) {
                    let first = ((le16(e, 20) << 16) | le16(e, 26)) as u32;
                    return Ok(Some((first, e[11] & 0x10 != 0)));
                }
            }
        }
        Ok(None)
    }

    /// Write a 32-byte entry into the first free slot of the directory at `dir`, extending the directory when it is full.
    fn add_entry(&mut self, dir: u32, entry: &[u8; 32]) -> Result<(), String> {
        let chain = self.chain(dir)?;
        let cb = self.cluster_bytes() as usize;
        for &c in &chain {
            let mut buf = vec![0u8; cb];
            self.f.read_exact_at(&mut buf, self.offset(c)).map_err(|e| e.to_string())?;
            for (i, e) in buf.chunks_exact(32).enumerate() {
                if e[0] == 0 || e[0] == 0xE5 {
                    self.f.write_all_at(entry, self.offset(c) + 32 * i as u64).map_err(|e| format!("writing the directory: {e}"))?;
                    return Ok(());
                }
            }
        }
        // full: one more cluster, zeroed, linked after the last
        let new = self.alloc(1)?[0];
        self.f.write_all_at(&vec![0u8; cb], self.offset(new)).map_err(|e| e.to_string())?;
        let last = *chain.last().ok_or("empty directory chain")?;
        self.set_entry(last, new);
        self.flush()?;
        self.f.write_all_at(entry, self.offset(new)).map_err(|e| format!("writing the directory: {e}"))?;
        Ok(())
    }

    fn make_entry(name: &[u8; 11], attr: u8, first: u32, size: u32) -> [u8; 32] {
        let (date, time) = fat_now();
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = attr;
        e[14..16].copy_from_slice(&time.to_le_bytes());
        e[16..18].copy_from_slice(&date.to_le_bytes());
        e[18..20].copy_from_slice(&date.to_le_bytes());
        e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
        e[22..24].copy_from_slice(&time.to_le_bytes());
        e[24..26].copy_from_slice(&date.to_le_bytes());
        e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    /// The directory `parts` below the root, created where missing. Returns its first cluster and whether it already existed
    /// completely (every component).
    pub fn mkdir_p(&mut self, parts: &[&str]) -> Result<(u32, bool), String> {
        let mut dir = self.root;
        let mut all_existed = true;
        for p in parts {
            match self.find(dir, p)? {
                Some((c, true)) => dir = c,
                Some((_, false)) => return Err(format!("\"{p}\" exists on the EFI System Partition and is a file, not a folder")),
                None => {
                    all_existed = false;
                    let name = short_name(&p.to_ascii_uppercase())?;
                    let c = self.alloc(1)?[0];
                    let cb = self.cluster_bytes() as usize;
                    let mut data = vec![0u8; cb];
                    let mut dot = Self::make_entry(b".          ", 0x10, c, 0);
                    dot[11] = 0x10;
                    let parent = if dir == self.root { 0 } else { dir };
                    let dotdot = Self::make_entry(b"..         ", 0x10, parent, 0);
                    data[..32].copy_from_slice(&dot);
                    data[32..64].copy_from_slice(&dotdot);
                    self.f.write_all_at(&data, self.offset(c)).map_err(|e| e.to_string())?;
                    self.flush()?;
                    self.add_entry(dir, &Self::make_entry(&name, 0x10, c, 0))?;
                    dir = c;
                }
            }
        }
        Ok((dir, all_existed))
    }

    /// Create the file `name` (8.3, upper case) in `dir` with `size` bytes read from `src`. Refuses an existing name.
    pub fn create_file(&mut self, dir: u32, name: &str, size: u64, src: &mut dyn Read, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
        if self.find(dir, name)?.is_some() {
            return Err(format!("{name} already exists in that folder of the EFI System Partition; it is not overwritten"));
        }
        let sfn = short_name(name)?;
        if size > u32::MAX as u64 {
            return Err("a file over 4 GiB does not fit FAT32".into());
        }
        let cb = self.cluster_bytes();
        let first = if size == 0 {
            0
        } else {
            let chain = self.alloc(size.div_ceil(cb))?;
            let mut left = size;
            let mut i = 0usize;
            let mut buf = vec![0u8; 1 << 20];
            while i < chain.len() {
                let mut j = i + 1;
                while j < chain.len() && chain[j] == chain[j - 1] + 1 && ((j - i) as u64) * cb < (1 << 20) {
                    j += 1;
                }
                let run_bytes = ((j - i) as u64 * cb) as usize;
                let want = (run_bytes as u64).min(left) as usize;
                buf[..run_bytes].fill(0);
                src.read_exact(&mut buf[..want]).map_err(|e| format!("reading the IYAKKI payload: {e}"))?;
                self.f.write_all_at(&buf[..run_bytes], self.offset(chain[i])).map_err(|e| format!("writing the EFI System Partition: {e}"))?;
                left -= want as u64;
                progress(want as u64);
                i = j;
            }
            chain[0]
        };
        self.f.sync_data().map_err(|e| e.to_string())?;
        self.flush()?;
        self.add_entry(dir, &Self::make_entry(&sfn, 0x20, first, size as u32))?;
        self.f.sync_data().map_err(|e| e.to_string())?;
        Ok(())
    }
}
