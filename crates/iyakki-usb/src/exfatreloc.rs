//! Moving the files of an exFAT volume below a cluster limit, so that the volume can shrink further than its last used cluster (F-INST-27).
//! Our own code, no external tool. Used by [`crate::exfatshrink`] before it writes the new size.
//!
//! The tree is read from the root directory: every in-use file directory entry set (`0x85` + stream `0xC0` + name `0xC1`) names one chain
//! of clusters, either contiguous (the stream's NoFatChain flag, no FAT entries are valid) or followed along the FAT. What moves: a chain
//! with a cluster at or above `limit`. A contiguous chain moves whole into a free run below the limit; a FAT chain keeps its clusters
//! below the limit and only the clusters at or above it go to free clusters below it (the FAT entries are rewritten). Directories are
//! files like any other, and nothing points back at a directory from its children, so a directory moves without touching them.
//!
//! What never moves, so the smallest size never goes below it: the root directory, the allocation bitmap, the up-case table and any
//! allocated cluster that no directory entry names (vendor data, damage). A volume whose root directory is the problem refuses in words.
//!
//! Order for ONE chain, so that a power cut never loses or damages a file: (1) the new clusters are marked used in the bitmap; (2) the
//! data is copied (the old chain is still whole and in use); (3) the FAT entries of the new clusters are written; (4) the switch: the FAT
//! entry of the cluster before the first moved one, or the first-cluster field of the stream entry (with the set checksum made again);
//! (5) the old clusters are freed in the bitmap and their FAT entries zeroed. A cut before (4) leaves the old chain and some marked-used
//! clusters that nothing names; a cut after it leaves the new chain and some lost clusters of the same kind (the other system's disk
//! check frees them). The refusals (nothing fits, an entry that is not sane) all happen before the first write.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;
use std::fs::File;
use std::path::Path;

use crate::exfatshrink::{find_bitmap, open, read_boot, Geometry};
use crate::posix::FileExt;

const ENTRY: usize = 32;
const END_OF_CHAIN: u32 = 0xFFFF_FFFF;
const MAX_ITEMS: usize = 2_000_000;

/// One chain found in the tree.
#[derive(Clone, Debug)]
pub struct Item {
    /// The directory (index into the item list) the entry set lies in, and the byte offset of the set in that directory's data.
    dir: usize,
    off: usize,
    /// Secondary entries of the set (the set is `count + 1` records).
    count: usize,
    pub name: String,
    pub is_dir: bool,
    contiguous: bool,
    pub clusters: Vec<u32>,
}

/// What the walk found.
pub struct Tree {
    pub items: Vec<Item>,
    /// The clusters of the allocation bitmap and the up-case table (never moved).
    pub system: Vec<u32>,
}

struct Vol {
    f: File,
    g: Geometry,
    bitmap_first: u64,
    bitmap: Vec<u8>,
    fat: Vec<u32>,
}

/// The set checksum of the specification over a whole entry set, leaving out the checksum field itself (bytes 2 and 3 of the first record).
pub fn set_checksum(set: &[u8]) -> u16 {
    let mut sum = 0u16;
    for (i, b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        sum = (if sum & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(*b as u16);
    }
    sum
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64at(b: &[u8], o: usize) -> u64 {
    let mut x = [0u8; 8];
    x.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(x)
}

impl Vol {
    fn load(dev: &Path, write: bool) -> Result<Vol, String> {
        let f = open(dev, write)?;
        let (g, _) = read_boot(&f)?;
        let (bitmap_first, len) = find_bitmap(&f, &g)?;
        let mut bitmap = vec![0u8; len as usize];
        f.read_exact_at(&mut bitmap, g.cluster_byte(bitmap_first)).map_err(|e| format!("reading the allocation bitmap: {e}"))?;
        let mut raw = vec![0u8; ((g.clusters + 2) * 4) as usize];
        f.read_exact_at(&mut raw, g.fat_off * g.bps).map_err(|e| format!("reading the FAT: {e}"))?;
        let fat = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        Ok(Vol { f, g, bitmap_first, bitmap, fat })
    }

    fn cs(&self) -> usize {
        (self.g.spc * self.g.bps) as usize
    }

    fn in_range(&self, c: u64) -> bool {
        (2..self.g.clusters + 2).contains(&c)
    }

    fn is_set(&self, c: u32) -> bool {
        let i = (c - 2) as usize;
        self.bitmap[i / 8] & (1 << (i % 8)) != 0
    }

    /// The clusters of a chain of `n` clusters starting at `first`.
    fn chain(&self, first: u64, n: u64, contiguous: bool) -> Result<Vec<u32>, String> {
        if !self.in_range(first) || n == 0 || n > self.g.clusters {
            return Err("a directory entry names clusters outside the volume; run the other system's disk check first".into());
        }
        if contiguous {
            if !self.in_range(first + n - 1) {
                return Err("a file reaches past the end of the volume; run the other system's disk check first".into());
            }
            return Ok((first..first + n).map(|c| c as u32).collect());
        }
        let mut out = Vec::with_capacity(n as usize);
        let mut c = first;
        for _ in 0..n {
            if !self.in_range(c) {
                return Err("a cluster chain leaves the volume; run the other system's disk check first".into());
            }
            out.push(c as u32);
            c = self.fat[c as usize] as u64;
            if c >= 0xFFFF_FFF8 {
                break;
            }
        }
        if out.len() as u64 != n {
            return Err("a cluster chain is not as long as its file says; run the other system's disk check first".into());
        }
        Ok(out)
    }

    fn read_dir(&self, clusters: &[u32]) -> Result<Vec<u8>, String> {
        let cs = self.cs();
        let mut out = vec![0u8; cs * clusters.len()];
        for (i, c) in clusters.iter().enumerate() {
            self.f.read_exact_at(&mut out[i * cs..(i + 1) * cs], self.g.cluster_byte(*c as u64)).map_err(|e| format!("reading a directory: {e}"))?;
        }
        Ok(out)
    }

    fn walk(&self) -> Result<Tree, String> {
        let cs = self.cs() as u64;
        let root = self.chain(self.g.root, self.fat_len(self.g.root)?, false)?;
        let mut items = vec![Item { dir: usize::MAX, off: 0, count: 0, name: String::new(), is_dir: true, contiguous: false, clusters: root }];
        let mut system = Vec::new();
        let mut seen: HashSet<u32> = items[0].clusters.iter().copied().collect();
        let mut d = 0;
        while d < items.len() {
            let data = self.read_dir(&items[d].clusters.clone())?;
            let mut i = 0;
            while i + ENTRY <= data.len() {
                let t = data[i];
                if t == 0x00 {
                    break;
                }
                if d == 0 && (t == 0x81 || t == 0x82) {
                    let (first, len) = (u32at(&data, i + 20) as u64, u64at(&data, i + 24));
                    if len > 0 {
                        let n = len.div_ceil(cs);
                        let cl = self.chain(first, n, false).or_else(|_| self.chain(first, n, true))?;
                        for c in cl {
                            if !seen.insert(c) {
                                return Err("two directory entries claim the same cluster; run the other system's disk check first".into());
                            }
                            system.push(c);
                        }
                    }
                }
                if t != 0x85 {
                    i += ENTRY;
                    continue;
                }
                let count = data[i + 1] as usize;
                if !(2..=18).contains(&count) || i + ENTRY * (count + 1) > data.len() {
                    return Err("a directory entry set is not sane; run the other system's disk check first".into());
                }
                let set = &data[i..i + ENTRY * (count + 1)];
                if set[ENTRY] != 0xC0 {
                    return Err("a file entry is not followed by its stream entry; run the other system's disk check first".into());
                }
                if u16::from_le_bytes([set[2], set[3]]) != set_checksum(set) {
                    return Err("a directory entry set has a wrong checksum; run the other system's disk check first".into());
                }
                let (flags, name_len) = (set[ENTRY + 1], set[ENTRY + 3] as usize);
                let (first, data_len) = (u32at(set, ENTRY + 20) as u64, u64at(set, ENTRY + 24));
                let attr = u16::from_le_bytes([set[4], set[5]]);
                let mut units = Vec::new();
                for e in 2..=count {
                    if set[e * ENTRY] == 0xC1 {
                        units.extend(set[e * ENTRY + 2..(e + 1) * ENTRY].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
                    }
                }
                units.truncate(name_len);
                let name = String::from_utf16_lossy(&units);
                let next = i + ENTRY * (count + 1);
                if first != 0 && data_len != 0 {
                    let (is_dir, contiguous) = (attr & 0x10 != 0, flags & 2 != 0);
                    let clusters = self.chain(first, data_len.div_ceil(cs), contiguous)?;
                    for c in &clusters {
                        if !seen.insert(*c) {
                            return Err("two files claim the same cluster; run the other system's disk check first".into());
                        }
                    }
                    if items.len() >= MAX_ITEMS {
                        return Err("the volume holds too many files for the shrink step".into());
                    }
                    items.push(Item { dir: d, off: i, count, name, is_dir, contiguous, clusters });
                }
                i = next;
            }
            d += 1;
            // directories are read when their turn comes: only the ones that are directories need reading, the others are skipped here
            while d < items.len() && !items[d].is_dir {
                d += 1;
            }
        }
        Ok(Tree { items, system })
    }

    /// The length in clusters of the FAT chain that starts at `first` (the root directory has no length of its own).
    fn fat_len(&self, first: u64) -> Result<u64, String> {
        let mut n = 0;
        let mut c = first;
        loop {
            if !self.in_range(c) || n > self.g.clusters {
                return Err("the root directory's chain leaves the volume; run the other system's disk check first".into());
            }
            n += 1;
            c = self.fat[c as usize] as u64;
            if c >= 0xFFFF_FFF8 {
                return Ok(n);
            }
        }
    }

    /// Every allocated cluster that no entry names.
    fn orphans(&self, tree: &Tree) -> Vec<u32> {
        let mut named: HashSet<u32> = tree.system.iter().copied().collect();
        for it in &tree.items {
            named.extend(it.clusters.iter().copied());
        }
        (2..self.g.clusters as u32 + 2).filter(|c| self.is_set(*c) && !named.contains(c)).collect()
    }
}

/// One planned move: the item and the whole new cluster list.
#[derive(Debug)]
struct Move {
    item: usize,
    new: Vec<u32>,
}

/// Decide where everything at or above `limit` goes. Pure: nothing is written.
fn plan(v: &Vol, tree: &Tree, limit: u32) -> Result<Vec<Move>, String> {
    let top = v.g.clusters as u32 + 2;
    if limit < 3 || limit > top {
        return Err("the limit is outside the volume".into());
    }
    let mut busy = vec![false; top as usize];
    let mut fixed_hi = 0u32;
    for c in tree.system.iter().chain(tree.items[0].clusters.iter()).copied().chain(v.orphans(tree)) {
        busy[c as usize] = true;
        fixed_hi = fixed_hi.max(c);
    }
    if fixed_hi >= limit {
        return Err(format!("the root directory, the allocation bitmap, the up-case table or data that no entry names lies at cluster {fixed_hi}; those are never moved, so the volume cannot shrink below it"));
    }
    let mut movers: Vec<usize> = Vec::new();
    for (idx, it) in tree.items.iter().enumerate().skip(1) {
        if it.clusters.iter().any(|c| *c >= limit) {
            movers.push(idx);
            if !it.contiguous {
                for c in it.clusters.iter().filter(|c| **c < limit) {
                    busy[*c as usize] = true;
                }
            }
        } else {
            for c in &it.clusters {
                busy[*c as usize] = true;
            }
        }
    }
    // the biggest contiguous ones first (they are the ones that need a run), then the chains
    movers.sort_by_key(|i| (!tree.items[*i].contiguous, std::cmp::Reverse(tree.items[*i].clusters.len())));
    let mut out = Vec::new();
    for idx in movers {
        let it = &tree.items[idx];
        let n = it.clusters.len();
        if it.contiguous {
            let mut run = 0usize;
            let mut start = None;
            for c in 2..limit {
                if busy[c as usize] {
                    run = 0;
                } else {
                    run += 1;
                    if run == n {
                        start = Some(c + 1 - n as u32);
                        break;
                    }
                }
            }
            let Some(s) = start else {
                return Err(format!("{} needs {n} clusters in one piece and no such free run is left below the limit; nothing was moved", display(&it.name)));
            };
            let new: Vec<u32> = (s..s + n as u32).collect();
            for c in &new {
                busy[*c as usize] = true;
            }
            out.push(Move { item: idx, new });
        } else {
            let mut free = (2..limit).filter(|c| !busy[*c as usize]);
            let mut new = Vec::with_capacity(n);
            for c in &it.clusters {
                if *c < limit {
                    new.push(*c);
                } else {
                    match free.next() {
                        Some(f) => new.push(f),
                        None => return Err(format!("{} does not fit below the limit; nothing was moved", display(&it.name))),
                    }
                }
            }
            for c in new.iter().filter(|c| !it.clusters.contains(c)) {
                busy[*c as usize] = true;
            }
            out.push(Move { item: idx, new });
        }
    }
    Ok(out)
}

fn display(name: &str) -> String {
    if name.is_empty() { "a file".into() } else { format!("\"{name}\"") }
}

/// The smallest size in bytes this volume can be shrunk to when files may move: found by trying smaller and smaller limits on the plan,
/// never below what cannot move.
pub fn minimum(dev: &Path) -> Result<u64, String> {
    let v = Vol::load(dev, false)?;
    let tree = v.walk()?;
    let top = v.g.clusters as u32 + 2;
    let hi = crate::exfatshrink::highest_used(&v.bitmap, &v.g) as u32;
    let used = (2..top).filter(|c| v.is_set(*c)).count() as u32;
    let mut fixed_hi = tree.items[0].clusters.iter().chain(tree.system.iter()).copied().max().unwrap_or(2);
    fixed_hi = fixed_hi.max(v.orphans(&tree).into_iter().max().unwrap_or(0));
    let mut limit = (used + 2).max(fixed_hi + 1).min(hi + 1);
    let step = ((hi + 1).saturating_sub(limit) / 64).max(1);
    while limit < hi + 1 {
        if plan(&v, &tree, limit).is_ok() {
            break;
        }
        limit = (limit + step).min(hi + 1);
    }
    Ok((v.g.heap_off + (limit as u64 - 2) * v.g.spc) * v.g.bps)
}

/// What a compaction did.
#[derive(Debug, PartialEq, Eq)]
pub struct Moved {
    pub clusters: u64,
    pub chains: u64,
}

/// Where `off` of directory `dir` is on the disk.
fn dir_pos(v: &Vol, items: &[Item], dir: usize, off: usize) -> u64 {
    let cs = v.cs();
    v.g.cluster_byte(items[dir].clusters[off / cs] as u64) + (off % cs) as u64
}

impl Vol {
    fn write(&self, data: &[u8], at: u64) -> Result<(), String> {
        self.f.write_all_at(data, at).map_err(|e| format!("writing the volume: {e}"))
    }
    fn sync(&self) -> Result<(), String> {
        self.f.sync_data().map_err(|e| e.to_string())
    }

    fn set_bits(&mut self, clusters: &[u32], on: bool) -> Result<(), String> {
        for c in clusters {
            let i = (*c - 2) as usize;
            if on {
                self.bitmap[i / 8] |= 1 << (i % 8);
            } else {
                self.bitmap[i / 8] &= !(1 << (i % 8));
            }
        }
        let base = self.g.cluster_byte(self.bitmap_first);
        let bytes: std::collections::BTreeSet<usize> = clusters.iter().map(|c| (*c - 2) as usize / 8).collect();
        for b in bytes {
            self.write(&[self.bitmap[b]], base + b as u64)?;
        }
        self.sync()
    }

    fn set_fat(&mut self, c: u32, v: u32) -> Result<(), String> {
        self.fat[c as usize] = v;
        self.write(&v.to_le_bytes(), self.g.fat_off * self.g.bps + c as u64 * 4)
    }

    fn copy_cluster(&self, from: u32, to: u32) -> Result<(), String> {
        let mut buf = vec![0u8; self.cs()];
        self.f.read_exact_at(&mut buf, self.g.cluster_byte(from as u64)).map_err(|e| format!("reading a cluster: {e}"))?;
        self.write(&buf, self.g.cluster_byte(to as u64))
    }

    /// Write the new first cluster into the entry set of an item and make its checksum again (stream first, then the checksum).
    fn point_to(&self, items: &[Item], idx: usize, first: u32) -> Result<(), String> {
        let it = &items[idx];
        let mut set = vec![0u8; ENTRY * (it.count + 1)];
        for e in 0..=it.count {
            self.f.read_exact_at(&mut set[e * ENTRY..(e + 1) * ENTRY], dir_pos(self, items, it.dir, it.off + e * ENTRY)).map_err(|e| format!("reading an entry: {e}"))?;
        }
        set[ENTRY + 20..ENTRY + 24].copy_from_slice(&first.to_le_bytes());
        self.write(&set[ENTRY..2 * ENTRY], dir_pos(self, items, it.dir, it.off + ENTRY))?;
        self.sync()?;
        let sum = set_checksum(&set).to_le_bytes();
        self.write(&sum, dir_pos(self, items, it.dir, it.off) + 2)?;
        self.sync()
    }

    fn move_item(&mut self, items: &mut [Item], m: &Move) -> Result<u64, String> {
        let old = items[m.item].clusters.clone();
        let new = &m.new;
        let contiguous = items[m.item].contiguous;
        let changed: Vec<usize> = (0..old.len()).filter(|i| old[*i] != new[*i]).collect();
        let fresh: Vec<u32> = changed.iter().map(|i| new[*i]).collect();
        self.set_bits(&fresh, true)?;
        for i in &changed {
            self.copy_cluster(old[*i], new[*i])?;
        }
        self.sync()?;
        if !contiguous {
            for i in &changed {
                let next = if i + 1 < new.len() { new[i + 1] } else { END_OF_CHAIN };
                self.set_fat(new[*i], next)?;
            }
            self.sync()?;
            // the switches inside the chain: an unchanged cluster that is followed by a changed one
            for i in &changed {
                if *i > 0 && old[i - 1] == new[i - 1] {
                    self.set_fat(new[i - 1], new[*i])?;
                }
            }
            self.sync()?;
        }
        if changed.first() == Some(&0) {
            self.point_to(items, m.item, new[0])?;
        }
        let gone: Vec<u32> = changed.iter().map(|i| old[*i]).collect();
        if !contiguous {
            for c in &gone {
                self.set_fat(*c, 0)?;
            }
            self.sync()?;
        }
        self.set_bits(&gone, false)?;
        items[m.item].clusters = new.clone();
        Ok(changed.len() as u64)
    }
}

/// The number of clusters that hold anything (named or not): the volume cannot be smaller than this many clusters.
pub fn used_clusters(dev: &Path) -> Result<u64, String> {
    let v = Vol::load(dev, false)?;
    Ok((2..v.g.clusters as u32 + 2).filter(|c| v.is_set(*c)).count() as u64)
}

/// The files and directories the walk finds (names, whether a directory, their clusters), for the checks of the tests and the plan page.
pub fn tree(dev: &Path) -> Result<Tree, String> {
    Vol::load(dev, false)?.walk()
}

/// Move everything at or above cluster number `limit` (the volume will keep clusters `2..limit`) to free clusters below it.
/// Refused, before a byte is written, when it would not fit.
pub fn compact(dev: &Path, limit: u32) -> Result<Moved, String> {
    let mut v = Vol::load(dev, true)?;
    let mut tree = v.walk()?;
    let moves = plan(&v, &tree, limit)?;
    let mut done = Moved { clusters: 0, chains: 0 };
    for m in &moves {
        done.clusters += v.move_item(&mut tree.items, m)?;
        done.chains += 1;
    }
    Ok(done)
}
