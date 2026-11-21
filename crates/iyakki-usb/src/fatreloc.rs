//! Moving the files of a FAT32 volume below a cluster limit, so that the volume can shrink further than the last used cluster (F-INST-27).
//! Our own code, no external tool. Used by [`crate::fatshrink`] before it writes the new size.
//!
//! What moves: every cluster of every file and directory chain whose number is at or above `limit` goes to a free cluster below it.
//! The directory tree is read from the root (long-name entries are skipped as the 32-byte records they are, deleted entries and the
//! volume label are ignored); each chain is found from its directory entry's first-cluster field.
//!
//! Order for ONE chain, so that a power cut never loses or damages a file: (1) the data of each cluster to move is copied to its new
//! free cluster (the old chain is still whole and in use); (2) the new clusters are linked in both FAT copies; (3) ONE field switches the
//! chain over (the previous cluster's FAT entry, or the directory entry's first-cluster field); (4) the old clusters are freed. A cut
//! before (3) leaves the old chain and some unreachable but marked-used clusters (the other system's disk check frees them); a cut after
//! it leaves the new chain and some lost clusters of the same kind. After the last chain the `.` and `..` entries of every directory are
//! made to say where their directory now is, and a root directory that moved is named in the boot sector (and its backup).
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::posix::FileExt;

const EOC: u32 = 0x0FFF_FFFF;
const ENTRY: usize = 32;

struct Vol {
    f: File,
    bps: u64,
    spc: u64,
    reserved: u64,
    nfats: u64,
    fatsz: u64,
    total: u64,
    root: u32,
    backup: u64,
    fat: Vec<u32>,
}

/// Where the first cluster number of one chain is written.
#[derive(Clone, Copy, Debug)]
struct Link {
    /// The directory cluster that holds the entry and the byte offset of the 32-byte entry in that cluster.
    dir_cluster: u32,
    offset: usize,
}

/// One chain found in the tree.
struct Chain {
    first: u32,
    /// `None` for the root directory (it is named by the boot sector).
    link: Option<Link>,
    is_dir: bool,
    /// For a directory: the first cluster of the directory it is in (0 for the root's children's `..`).
    parent_first: u32,
}

impl Vol {
    fn open(dev: &Path, write: bool) -> Result<Vol, String> {
        let f = OpenOptions::new().read(true).write(write).open(dev).map_err(|e| format!("{}: {e}", dev.display()))?;
        let mut b = vec![0u8; 512];
        f.read_exact_at(&mut b, 0).map_err(|e| format!("reading the boot sector: {e}"))?;
        if b[510] != 0x55 || b[511] != 0xAA {
            return Err("no boot sector signature".into());
        }
        let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u64;
        let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let v = Vol { bps: u16at(11), spc: b[13] as u64, reserved: u16at(14), nfats: b[16] as u64, fatsz: u32at(36) as u64, total: u32at(32) as u64, root: u32at(44), backup: u16at(50), fat: vec![], f };
        if !matches!(v.bps, 512 | 1024 | 2048 | 4096) || v.spc == 0 || !v.spc.is_power_of_two() || v.reserved < 2 || !(1..=2).contains(&v.nfats) || v.fatsz == 0 || v.total == 0 || u16at(17) != 0 || u16at(19) != 0 || u16at(22) != 0 {
            return Err("the boot sector's numbers are not those of a FAT32 volume".into());
        }
        let n = (v.clusters() + 2).min(v.fatsz * v.bps / 4) as usize;
        let mut raw = vec![0u8; n * 4];
        v.f.read_exact_at(&mut raw, v.reserved * v.bps).map_err(|e| format!("reading the FAT: {e}"))?;
        let fat = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x0FFF_FFFF).collect();
        Ok(Vol { fat, ..v })
    }

    fn data_start(&self) -> u64 {
        self.reserved + self.nfats * self.fatsz
    }
    fn clusters(&self) -> u64 {
        (self.total - self.data_start()) / self.spc
    }
    fn cluster_bytes(&self) -> usize {
        (self.spc * self.bps) as usize
    }
    fn cluster_off(&self, c: u32) -> u64 {
        (self.data_start() + (c as u64 - 2) * self.spc) * self.bps
    }
    fn is_eoc(v: u32) -> bool {
        v >= 0x0FFF_FFF8
    }

    fn chain(&self, first: u32) -> Result<Vec<u32>, String> {
        let mut out = vec![];
        let mut c = first;
        let mut seen = HashSet::new();
        while c >= 2 && !Self::is_eoc(c) {
            if c as usize >= self.fat.len() || !seen.insert(c) {
                return Err(format!("the chain starting at cluster {first} is broken (a loop or a cluster outside the volume)"));
            }
            out.push(c);
            c = self.fat[c as usize];
            if c == 0 {
                return Err(format!("the chain starting at cluster {first} runs into a free cluster"));
            }
        }
        Ok(out)
    }

    fn read_cluster(&self, c: u32) -> Result<Vec<u8>, String> {
        let mut b = vec![0u8; self.cluster_bytes()];
        self.f.read_exact_at(&mut b, self.cluster_off(c)).map_err(|e| format!("reading cluster {c}: {e}"))?;
        Ok(b)
    }

    fn write_fat(&mut self, c: u32, v: u32) -> Result<(), String> {
        self.fat[c as usize] = v;
        for copy in 0..self.nfats {
            let at = (self.reserved + copy * self.fatsz) * self.bps + c as u64 * 4;
            // the top four bits of an entry are reserved and kept as they are
            let mut old = [0u8; 4];
            self.f.read_exact_at(&mut old, at).map_err(|e| format!("reading the FAT: {e}"))?;
            let keep = u32::from_le_bytes(old) & 0xF000_0000;
            self.f.write_all_at(&(keep | v).to_le_bytes(), at).map_err(|e| format!("writing the FAT: {e}"))?;
        }
        Ok(())
    }

    /// Every chain of the tree: the root, then each file and directory reached from it.
    fn walk(&self) -> Result<Vec<Chain>, String> {
        let mut out = vec![Chain { first: self.root, link: None, is_dir: true, parent_first: 0 }];
        let mut queue = vec![(self.root, 0u32)];
        let mut visited = HashSet::new();
        while let Some((dir_first, _)) = queue.pop() {
            if !visited.insert(dir_first) {
                return Err("a directory is reached twice (a loop in the tree); run the other system's disk check first".into());
            }
            for dc in self.chain(dir_first)? {
                let data = self.read_cluster(dc)?;
                for (i, e) in data.chunks_exact(ENTRY).enumerate() {
                    if e[0] == 0 {
                        break;
                    }
                    let attr = e[11];
                    if e[0] == 0xE5 || attr == 0x0F || (attr & 0x08 != 0 && attr & 0x10 == 0) {
                        continue;
                    }
                    let is_dir = attr & 0x10 != 0;
                    if is_dir && (e[0] == b'.') {
                        continue;
                    }
                    let first = ((u16::from_le_bytes([e[20], e[21]]) as u32) << 16) | u16::from_le_bytes([e[26], e[27]]) as u32;
                    if first < 2 {
                        continue; // an empty file has no cluster
                    }
                    if first as usize >= self.fat.len() {
                        return Err(format!("an entry points at cluster {first}, outside the volume; run the other system's disk check first"));
                    }
                    out.push(Chain { first, link: Some(Link { dir_cluster: dc, offset: i * ENTRY }), is_dir, parent_first: dir_first });
                    if is_dir {
                        queue.push((first, dir_first));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The number of clusters that hold anything (every chain's length): the volume cannot be smaller than this many clusters.
pub fn used_clusters(dev: &Path) -> Result<u64, String> {
    let v = Vol::open(dev, false)?;
    let mut n = 0u64;
    for c in v.walk()? {
        n += v.chain(c.first)?.len() as u64;
    }
    Ok(n)
}

/// What a compaction did.
#[derive(Debug, PartialEq, Eq)]
pub struct Moved {
    pub clusters: u64,
    pub chains: u64,
}

/// Move everything at or above cluster number `limit` (that is: the volume will keep clusters `2..limit`) to free clusters below it.
/// Refused, before a byte is written, when the used clusters would not fit.
pub fn compact(dev: &Path, limit: u32) -> Result<Moved, String> {
    let mut v = Vol::open(dev, true)?;
    let chains = v.walk()?;
    let mut lens = HashMap::new();
    let mut used = 0u64;
    for c in &chains {
        let ch = v.chain(c.first)?;
        used += ch.len() as u64;
        lens.insert(c.first, ch);
    }
    if used > (limit as u64).saturating_sub(2) {
        return Err(format!("the files need {used} clusters and only {} would be left; nothing was moved", (limit as u64).saturating_sub(2)));
    }
    // free clusters below the limit, lowest first
    let mut free: Vec<u32> = (2..limit.min(v.fat.len() as u32)).filter(|c| v.fat[*c as usize] == 0).collect();
    free.reverse();
    let mut moved = Moved { clusters: 0, chains: 0 };
    // old first cluster -> new first cluster, for the entries that live in directories that moved
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for c in &chains {
        let old = lens.get(&c.first).cloned().unwrap_or_default();
        if old.iter().all(|x| *x < limit) {
            continue;
        }
        // (1) the data of every cluster that moves, to its new place
        let mut new_chain = Vec::with_capacity(old.len());
        for &o in &old {
            if o < limit {
                new_chain.push(o);
            } else {
                let n = free.pop().ok_or("no free cluster left to move into (the disk changed under us)")?;
                let data = v.read_cluster(o)?;
                v.f.write_all_at(&data, v.cluster_off(n)).map_err(|e| format!("writing cluster {n}: {e}"))?;
                new_chain.push(n);
                remap.insert(o, n);
                moved.clusters += 1;
            }
        }
        v.f.sync_data().map_err(|e| e.to_string())?;
        // (2) link the new clusters (both FAT copies), so they are in use and chained
        for (i, &n) in new_chain.iter().enumerate() {
            if old[i] != n {
                let next = new_chain.get(i + 1).copied().unwrap_or(EOC);
                v.write_fat(n, next)?;
            }
        }
        v.f.sync_data().map_err(|e| e.to_string())?;
        // (3) the one field that switches the chain over: the entry before the first moved cluster
        let first_moved = old.iter().position(|x| *x >= limit).unwrap();
        if first_moved > 0 {
            v.write_fat(old[first_moved - 1], new_chain[first_moved])?;
        } else {
            match c.link {
                Some(l) => {
                    // the directory cluster that holds the entry may itself have moved already
                    let at_cluster = remap.get(&l.dir_cluster).copied().unwrap_or(l.dir_cluster);
                    let off = v.cluster_off(at_cluster) + l.offset as u64;
                    let mut e = vec![0u8; ENTRY];
                    v.f.read_exact_at(&mut e, off).map_err(|x| format!("reading a directory entry: {x}"))?;
                    let nf = new_chain[0];
                    e[20..22].copy_from_slice(&((nf >> 16) as u16).to_le_bytes());
                    e[26..28].copy_from_slice(&((nf & 0xFFFF) as u16).to_le_bytes());
                    v.f.write_all_at(&e, off).map_err(|x| format!("writing a directory entry: {x}"))?;
                }
                None => {
                    // the root directory moved: the boot sector (and its backup) name it
                    let nf = new_chain[0];
                    let mut sectors = vec![0u64];
                    if v.backup != 0 {
                        sectors.push(v.backup);
                    }
                    for s in sectors {
                        v.f.write_all_at(&nf.to_le_bytes(), s * v.bps + 44).map_err(|x| format!("writing the boot sector: {x}"))?;
                    }
                    v.root = nf;
                }
            }
        }
        v.f.sync_data().map_err(|e| e.to_string())?;
        // (4) free the old clusters that moved
        for (i, &o) in old.iter().enumerate() {
            if o != new_chain[i] {
                v.write_fat(o, 0)?;
            }
        }
        v.f.sync_data().map_err(|e| e.to_string())?;
        moved.chains += 1;
    }
    // the `.` and `..` entries of every directory now name the directories where they are (the root has neither)
    let root_old = chains[0].first;
    let new_first = |old: u32| remap.get(&old).copied().unwrap_or(old);
    for c in chains.iter().filter(|c| c.is_dir && c.link.is_some()) {
        let first = new_first(c.first);
        // a child of the root names 0 as its parent
        let parent = if c.parent_first == root_old { 0 } else { new_first(c.parent_first) };
        let mut b = vec![0u8; 2 * ENTRY];
        v.f.read_exact_at(&mut b, v.cluster_off(first)).map_err(|e| format!("reading a directory: {e}"))?;
        let mut changed = false;
        for (i, target) in [(0usize, first), (1usize, parent)] {
            let e = &mut b[i * ENTRY..(i + 1) * ENTRY];
            if e[0] != b'.' {
                continue;
            }
            let (hi, lo) = ((target >> 16) as u16, (target & 0xFFFF) as u16);
            if u16::from_le_bytes([e[20], e[21]]) != hi || u16::from_le_bytes([e[26], e[27]]) != lo {
                e[20..22].copy_from_slice(&hi.to_le_bytes());
                e[26..28].copy_from_slice(&lo.to_le_bytes());
                changed = true;
            }
        }
        if changed {
            v.f.write_all_at(&b, v.cluster_off(first)).map_err(|e| format!("writing a directory: {e}"))?;
        }
    }
    v.f.sync_all().map_err(|e| e.to_string())?;
    Ok(moved)
}
