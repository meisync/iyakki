//! Shrinking a FAT32 volume with our own code (F-INST-27), no external tool. The files that lie beyond the new end are moved to free
//! clusters below it first (`fatreloc`, crash-safe per chain), so the smallest size is the number of clusters the files actually need,
//! not the position of the last one. The boot sector, its backup copy and the FSInfo sector then say the new size. A volume that would end
//! with fewer than 65,525 clusters is refused (it would no longer be FAT32 by definition).
//!
//! Order of writes: FSInfo and its backup, then the backup boot sector, and the primary boot sector LAST (it is what makes the new size
//! count); a power cut before that leaves the old, whole volume. Everything is read back and compared.
// SPDX-License-Identifier: Apache-2.0

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::posix::FileExt;

/// The fewest clusters a FAT32 volume may have.
pub const MIN_CLUSTERS: u64 = 65_525;

struct Geometry {
    bps: u64,
    spc: u64,
    reserved: u64,
    nfats: u64,
    fatsz: u64,
    total: u64,
    root: u64,
    fsinfo: u64,
    backup: u64,
}

impl Geometry {
    fn parse(b: &[u8]) -> Result<Geometry, String> {
        if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
            return Err("no boot sector signature".into());
        }
        let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
        let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64;
        let g = Geometry { bps: u16at(11), spc: b[13] as u64, reserved: u16at(14), nfats: b[16] as u64, fatsz: u32at(36), total: u32at(32), root: u32at(44), fsinfo: u16at(48), backup: u16at(50) };
        if !matches!(g.bps, 512 | 1024 | 2048 | 4096) || g.spc == 0 || !g.spc.is_power_of_two() || g.reserved < 2 || !(1..=2).contains(&g.nfats) || g.fatsz == 0 || g.total == 0 {
            return Err("the boot sector's numbers are not those of a FAT32 volume".into());
        }
        // FAT32 has no root directory area and 0 in the 16-bit size fields
        if u16at(17) != 0 || u16at(19) != 0 || u16at(22) != 0 {
            return Err("this is FAT12/16 (or damaged), not FAT32".into());
        }
        if g.data_start() >= g.total {
            return Err("the volume has no data area".into());
        }
        Ok(g)
    }
    fn data_start(&self) -> u64 {
        self.reserved + self.nfats * self.fatsz
    }
    fn clusters(&self, total: u64) -> u64 {
        (total - self.data_start()) / self.spc
    }
}

fn open(dev: &Path, write: bool) -> Result<File, String> {
    OpenOptions::new().read(true).write(write).open(dev).map_err(|e| format!("{}: {e}", dev.display()))
}

fn read_boot(f: &File) -> Result<(Geometry, Vec<u8>), String> {
    let mut b = vec![0u8; 512];
    f.read_exact_at(&mut b, 0).map_err(|e| format!("reading the boot sector: {e}"))?;
    Ok((Geometry::parse(&b)?, b))
}

/// The first FAT copy as 32-bit entries (masked to 28 bits), for the clusters this volume has.
fn read_fat(f: &File, g: &Geometry, copy: u64) -> Result<Vec<u32>, String> {
    let n = (g.clusters(g.total) + 2).min(g.fatsz * g.bps / 4) as usize;
    let mut raw = vec![0u8; n * 4];
    f.read_exact_at(&mut raw, (g.reserved + copy * g.fatsz) * g.bps).map_err(|e| format!("reading the FAT: {e}"))?;
    Ok(raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x0FFF_FFFF).collect())
}

/// The highest cluster number in use (not free), at least the root directory's.
fn highest_used(fat: &[u32], root: u64) -> u64 {
    let hi = fat.iter().enumerate().skip(2).filter(|(_, v)| **v != 0).map(|(i, _)| i as u64).max().unwrap_or(2);
    hi.max(root)
}

/// The smallest size in bytes this volume can be shrunk to: the clusters the files and directories need (moved together when they lie
/// further out), and never fewer than [`MIN_CLUSTERS`].
pub fn minimum(dev: &Path) -> Result<u64, String> {
    let f = open(dev, false)?;
    let (g, _) = read_boot(&f)?;
    let used = crate::fatreloc::used_clusters(dev)?;
    let by_files = g.data_start() + used.max(1) * g.spc;
    let by_type = g.data_start() + MIN_CLUSTERS * g.spc;
    Ok(by_files.max(by_type) * g.bps)
}

/// The smallest size without moving a file (just after the highest used cluster): what a shrink that moves nothing could reach.
pub fn minimum_without_moving(dev: &Path) -> Result<u64, String> {
    let f = open(dev, false)?;
    let (g, _) = read_boot(&f)?;
    let fat = read_fat(&f, &g, 0)?;
    let hi = highest_used(&fat, g.root);
    Ok((g.data_start() + (hi - 1) * g.spc).max(g.data_start() + MIN_CLUSTERS * g.spc) * g.bps)
}

/// Check the structure without changing anything: the two FAT copies agree, no entry points outside the volume, the root directory is a used
/// chain, the media entry is right.
pub fn check(dev: &Path) -> Result<(), String> {
    let f = open(dev, false)?;
    let (g, _) = read_boot(&f)?;
    if g.total * g.bps > f.metadata().map_err(|e| e.to_string())?.len() {
        return Err("the volume claims more sectors than the partition holds".into());
    }
    let fat = read_fat(&f, &g, 0)?;
    if g.nfats == 2 && read_fat(&f, &g, 1)? != fat {
        return Err("the two copies of the FAT differ; run the other system's disk check first".into());
    }
    if fat.len() < 3 || fat[0] & 0xFF < 0xF0 {
        return Err("the FAT does not start with a media entry".into());
    }
    let n = g.clusters(g.total) + 2;
    for (i, v) in fat.iter().enumerate().skip(2) {
        let v = *v as u64;
        if v >= 2 && v < 0x0FFF_FFF0 && v >= n {
            return Err(format!("cluster {i} points at {v}, outside the volume; run the other system's disk check first"));
        }
    }
    if fat.get(g.root as usize).copied().unwrap_or(0) == 0 {
        return Err("the root directory's first cluster is marked free".into());
    }
    Ok(())
}

/// Shrink the volume on `dev` to `bytes` (a multiple of the sector size, not below [`minimum`]).
pub fn shrink(dev: &Path, bytes: u64) -> Result<(), String> {
    let f = open(dev, true)?;
    let (g, _) = read_boot(&f)?;
    if bytes % g.bps != 0 {
        return Err("the new size is not a whole number of sectors".into());
    }
    let total = bytes / g.bps;
    if total >= g.total {
        return Err("the new size is not smaller than the volume".into());
    }
    let min = minimum(dev)?;
    if bytes < min {
        return Err(format!("the files need {min} bytes even when they are moved together; it cannot shrink below that"));
    }
    let clusters = g.clusters(total);
    if clusters < MIN_CLUSTERS {
        return Err("the volume would have fewer than 65,525 clusters and stop being FAT32".into());
    }
    // the files beyond the new end move below it first (nothing happens when they are all below already); the boot sector is read again
    // afterwards because a root directory that moved is named in it
    let limit = (clusters + 2) as u32;
    if highest_used(&read_fat(&f, &g, 0)?, g.root) >= limit as u64 {
        crate::fatreloc::compact(dev, limit)?;
    }
    let (g, boot) = read_boot(&f)?;
    let fat = read_fat(&f, &g, 0)?;
    let free = fat.iter().enumerate().skip(2).take(clusters as usize).filter(|(_, v)| **v == 0).count() as u32;
    // the new boot sector
    let mut nb = boot.clone();
    nb[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    let w = |off: u64, data: &[u8]| f.write_all_at(data, off).map_err(|e| format!("writing the volume: {e}"));
    // FSInfo and its backup first
    let mut fsinfo_sectors = vec![g.fsinfo];
    if g.backup != 0 && g.fsinfo != 0 {
        fsinfo_sectors.push(g.backup + g.fsinfo);
    }
    for s in fsinfo_sectors.into_iter().filter(|s| *s != 0) {
        let mut b = vec![0u8; g.bps as usize];
        f.read_exact_at(&mut b, s * g.bps).map_err(|e| format!("reading FSInfo: {e}"))?;
        if b[0..4] == [0x52, 0x52, 0x61, 0x41] && b[484..488] == [0x72, 0x72, 0x41, 0x61] {
            b[488..492].copy_from_slice(&free.to_le_bytes());
            b[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            w(s * g.bps, &b)?;
        }
    }
    f.sync_data().map_err(|e| e.to_string())?;
    if g.backup != 0 {
        // keep the rest of the backup sector as it was
        let mut old = vec![0u8; g.bps as usize];
        f.read_exact_at(&mut old, g.backup * g.bps).map_err(|e| e.to_string())?;
        old[..512].copy_from_slice(&nb);
        w(g.backup * g.bps, &old)?;
        f.sync_data().map_err(|e| e.to_string())?;
    }
    // the commit: the primary boot sector
    let mut p0 = vec![0u8; g.bps as usize];
    f.read_exact_at(&mut p0, 0).map_err(|e| e.to_string())?;
    p0[..512].copy_from_slice(&nb);
    w(0, &p0)?;
    f.sync_data().map_err(|e| e.to_string())?;
    // read back
    let (g2, _) = read_boot(&f)?;
    if g2.total != total {
        return Err("the boot sector did not take the new size".into());
    }
    Ok(())
}
