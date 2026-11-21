//! Shrinking an exFAT volume with our own code (F-INST-27), no external tool. Files that lie beyond the new end are first moved below it
//! (`exfatreloc`, crash-safe per chain); without such files the volume may shrink down to just after its last allocated cluster (the
//! highest set bit of the allocation bitmap, which also covers the bitmap itself, the up-case table and the root directory). The boot sector says the new volume length and cluster
//! count; the boot region's checksum sector is recomputed.
//!
//! Order of writes: the BACKUP boot region (sectors 12 to 23) first, then the main region with sector 0 and the checksum sector 11 last, in
//! one flush. A power cut before that leaves the old volume whole (the main region is untouched), and one in the last two sector writes
//! leaves a main region whose checksum does not match: the other system's tools restore it from the backup region, which is already new.
//! Everything is read back and compared. The FAT is not shortened (a longer FAT than needed is valid) and the bitmap keeps its length.
// SPDX-License-Identifier: Apache-2.0

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::posix::FileExt;

pub(crate) struct Geometry {
    pub(crate) bps: u64,
    pub(crate) spc: u64,
    pub(crate) total: u64,
    pub(crate) fat_off: u64,
    pub(crate) fat_len: u64,
    pub(crate) heap_off: u64,
    pub(crate) clusters: u64,
    pub(crate) root: u64,
}

impl Geometry {
    pub(crate) fn parse(b: &[u8]) -> Result<Geometry, String> {
        if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
            return Err("no boot sector signature".into());
        }
        if &b[3..11] != b"EXFAT   " {
            return Err("this is not an exFAT volume".into());
        }
        let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64;
        let (bs, cs) = (b[108] as u32, b[109] as u32);
        if !(9..=12).contains(&bs) || cs > 25 - bs.min(25) {
            return Err("the boot sector's numbers are not those of an exFAT volume".into());
        }
        let mut total8 = [0u8; 8];
        total8.copy_from_slice(&b[72..80]);
        let g = Geometry {
            bps: 1u64 << bs,
            spc: 1u64 << cs,
            total: u64::from_le_bytes(total8),
            fat_off: u32at(80),
            fat_len: u32at(84),
            heap_off: u32at(88),
            clusters: u32at(92),
            root: u32at(96),
        };
        if g.total == 0 || g.clusters == 0 || g.root < 2 || g.heap_off == 0 || g.heap_off >= g.total || b[110] == 0 || b[110] > 2 {
            return Err("the boot sector's numbers are not those of an exFAT volume".into());
        }
        if g.heap_off + g.clusters * g.spc > g.total {
            return Err("the cluster heap reaches past the end of the volume; run the other system's disk check first".into());
        }
        Ok(g)
    }
    pub(crate) fn cluster_byte(&self, c: u64) -> u64 {
        (self.heap_off + (c - 2) * self.spc) * self.bps
    }
}

pub(crate) fn open(dev: &Path, write: bool) -> Result<File, String> {
    OpenOptions::new().read(true).write(write).open(dev).map_err(|e| format!("{}: {e}", dev.display()))
}

pub(crate) fn read_boot(f: &File) -> Result<(Geometry, Vec<u8>), String> {
    let mut b = vec![0u8; 512];
    f.read_exact_at(&mut b, 0).map_err(|e| format!("reading the boot sector: {e}"))?;
    Ok((Geometry::parse(&b)?, b))
}

/// The boot region checksum over sectors 0 to 10 (the volume flags and the percent in use are left out, as the specification says).
pub fn region_checksum(region: &[u8]) -> u32 {
    let mut sum = 0u32;
    for (i, b) in region.iter().enumerate() {
        if i == 106 || i == 107 || i == 112 {
            continue;
        }
        sum = (if sum & 1 != 0 { 0x8000_0000u32 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(*b as u32);
    }
    sum
}

pub(crate) fn read_region(f: &File, g: &Geometry, first_sector: u64) -> Result<Vec<u8>, String> {
    let mut r = vec![0u8; (12 * g.bps) as usize];
    f.read_exact_at(&mut r, first_sector * g.bps).map_err(|e| format!("reading the boot region: {e}"))?;
    Ok(r)
}

pub(crate) fn region_ok(r: &[u8], bps: usize) -> bool {
    let want = region_checksum(&r[..11 * bps]);
    r[11 * bps..12 * bps].chunks_exact(4).all(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) == want)
}

pub(crate) fn make_region(mut r: Vec<u8>, bps: usize) -> Vec<u8> {
    let sum = region_checksum(&r[..11 * bps]).to_le_bytes();
    for c in r[11 * bps..12 * bps].chunks_exact_mut(4) {
        c.copy_from_slice(&sum);
    }
    r
}

pub(crate) fn fat_next(f: &File, g: &Geometry, c: u64) -> Result<u64, String> {
    let mut e = [0u8; 4];
    f.read_exact_at(&mut e, g.fat_off * g.bps + c * 4).map_err(|e| format!("reading the FAT: {e}"))?;
    Ok(u32::from_le_bytes(e) as u64)
}

/// The allocation bitmap: its first cluster and length in bytes, found in the root directory (which is followed along the FAT).
pub(crate) fn find_bitmap(f: &File, g: &Geometry) -> Result<(u64, u64), String> {
    let mut c = g.root;
    for _ in 0..64 {
        let mut dir = vec![0u8; (g.spc * g.bps) as usize];
        f.read_exact_at(&mut dir, g.cluster_byte(c)).map_err(|e| format!("reading the root directory: {e}"))?;
        for e in dir.chunks_exact(32) {
            if e[0] == 0x00 {
                return Err("the root directory has no allocation bitmap entry".into());
            }
            if e[0] == 0x81 {
                let first = u32::from_le_bytes([e[20], e[21], e[22], e[23]]) as u64;
                let mut l = [0u8; 8];
                l.copy_from_slice(&e[24..32]);
                let len = u64::from_le_bytes(l);
                if first < 2 || first >= g.clusters + 2 || len * 8 < g.clusters || len > (1 << 30) {
                    return Err("the allocation bitmap entry is not sane; run the other system's disk check first".into());
                }
                return Ok((first, len));
            }
        }
        c = fat_next(f, g, c)?;
        if !(2..g.clusters + 2).contains(&c) {
            break;
        }
    }
    Err("the root directory has no allocation bitmap entry".into())
}

pub(crate) fn read_bitmap(f: &File, g: &Geometry) -> Result<Vec<u8>, String> {
    let (first, len) = find_bitmap(f, g)?;
    let mut b = vec![0u8; len as usize];
    f.read_exact_at(&mut b, g.cluster_byte(first)).map_err(|e| format!("reading the allocation bitmap: {e}"))?;
    Ok(b)
}

/// The highest cluster number whose bit is set (at least the root directory's cluster).
pub(crate) fn highest_used(bitmap: &[u8], g: &Geometry) -> u64 {
    let mut hi = g.root;
    for i in (0..g.clusters).rev() {
        if bitmap[(i / 8) as usize] & (1 << (i % 8)) != 0 {
            hi = hi.max(i + 2);
            break;
        }
    }
    hi
}

/// The smallest size in bytes this volume can be shrunk to: the files that lie further out are moved below it first (`exfatreloc`), so
/// it is what the files need, never below the root directory, the bitmap and the up-case table.
pub fn minimum(dev: &Path) -> Result<u64, String> {
    crate::exfatreloc::minimum(dev)
}

/// The smallest size in bytes this volume can be shrunk to without moving a file (just after the last allocated cluster).
pub fn minimum_without_moving(dev: &Path) -> Result<u64, String> {
    let f = open(dev, false)?;
    let (g, _) = read_boot(&f)?;
    let hi = highest_used(&read_bitmap(&f, &g)?, &g);
    Ok((g.heap_off + (hi - 1) * g.spc) * g.bps)
}

/// Check the structure without changing anything: the main boot region's checksum is right and the backup region is the same, the volume
/// fits in the partition, and the allocation bitmap is found.
pub fn check(dev: &Path) -> Result<(), String> {
    let f = open(dev, false)?;
    let (g, _) = read_boot(&f)?;
    if g.total * g.bps > f.metadata().map_err(|e| e.to_string())?.len() {
        return Err("the volume claims more sectors than the partition holds".into());
    }
    let main = read_region(&f, &g, 0)?;
    if !region_ok(&main, g.bps as usize) {
        return Err("the boot region's checksum is wrong; run the other system's disk check first".into());
    }
    if read_region(&f, &g, 12)? != main {
        return Err("the backup boot region differs from the main one; run the other system's disk check first".into());
    }
    if g.fat_off + g.fat_len > g.heap_off || g.fat_len * g.bps < (g.clusters + 2) * 4 {
        return Err("the FAT does not cover the clusters; run the other system's disk check first".into());
    }
    read_bitmap(&f, &g)?;
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
        return Err(format!("the files need {min} bytes even when moved together, so it cannot shrink below that"));
    }
    let clusters = (total - g.heap_off) / g.spc;
    if clusters == 0 || clusters > g.clusters {
        return Err("the new size leaves no room for a cluster heap".into());
    }
    // anything above the new end moves below it first (refused before a byte is written when it does not fit)
    if highest_used(&read_bitmap(&f, &g)?, &g) >= clusters + 2 {
        crate::exfatreloc::compact(dev, (clusters + 2) as u32)?;
    }
    let bps = g.bps as usize;
    let main = read_region(&f, &g, 0)?;
    if !region_ok(&main, bps) {
        return Err("the boot region's checksum is wrong; run the other system's disk check first".into());
    }
    let mut new = main.clone();
    new[72..80].copy_from_slice(&total.to_le_bytes());
    new[92..96].copy_from_slice(&(clusters as u32).to_le_bytes());
    let new = make_region(new, bps);
    let w = |off: u64, data: &[u8]| f.write_all_at(data, off).map_err(|e| format!("writing the volume: {e}"));
    w(12 * g.bps, &new)?;
    f.sync_data().map_err(|e| e.to_string())?;
    // the main region: sectors 1 to 11 are unchanged except the checksum sector; sector 0 and then 11 last
    w(0, &new[..bps])?;
    w(11 * g.bps, &new[11 * bps..])?;
    f.sync_data().map_err(|e| e.to_string())?;
    // read back
    let (g2, _) = read_boot(&f)?;
    if g2.total != total || g2.clusters != clusters || read_region(&f, &g2, 0)? != new || read_region(&f, &g2, 12)? != new {
        return Err("the boot region did not take the new size".into());
    }
    Ok(())
}
