//! F-INST-27: moving files on an exFAT volume (`exfatreloc`) against images made by the real `mkfs.exfat` and judged by the real
//! `fsck.exfat`. The files are written by a small writer of this file (the computer has no tool or driver that writes exFAT files without
//! mounting), so the volumes hold contiguous files, fragmented FAT chains and a directory with files in it. Image files only.
//! A missing tool skips the test and says so.

use crate::exfatreloc::{compact, set_checksum, tree};
use crate::exfatshrink::{find_bitmap, highest_used, minimum, read_bitmap, read_boot, shrink, Geometry};
use crate::posix::FileExt;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

const MB: u64 = 1024 * 1024;

fn need(ts: &[&str]) -> bool {
    for t in ts {
        if !Command::new("sh").args(["-c", &format!("command -v {t} >/dev/null")]).status().is_ok_and(|s| s.success()) {
            eprintln!("skipped: {t} is not installed");
            return false;
        }
    }
    true
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-exreloc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &str, args: &[&str]) {
    let o = Command::new(cmd).args(args).output().unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert!(o.status.success(), "{cmd} {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}

fn fsck_ok(img: &Path) {
    let o = Command::new("fsck.exfat").args(["-n", img.to_str().unwrap()]).output().unwrap();
    assert!(o.status.success(), "fsck.exfat: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed as usize) % 251) as u8).collect()
}

fn name_hash(name: &str) -> u16 {
    let mut h = 0u16;
    for u in name.to_ascii_uppercase().encode_utf16() {
        for b in u.to_le_bytes() {
            h = (if h & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(h >> 1).wrapping_add(b as u16);
        }
    }
    h
}

/// A directory the writer fills: its clusters (FAT chain) and where the next entry set goes.
#[derive(Clone)]
struct Dir {
    clusters: Vec<u32>,
    next: usize,
}

struct W {
    f: File,
    g: Geometry,
    bitmap: Vec<u8>,
    bm_first: u64,
    root: Dir,
    /// name -> expected bytes
    files: BTreeMap<String, Vec<u8>>,
}

impl W {
    fn new(img: &Path) -> W {
        let f = OpenOptions::new().read(true).write(true).open(img).unwrap();
        let (g, _) = read_boot(&f).unwrap();
        let (bm_first, _) = find_bitmap(&f, &g).unwrap();
        let bitmap = read_bitmap(&f, &g).unwrap();
        // the root: how many records it holds already (bitmap, up-case, label...)
        let mut d = vec![0u8; (g.spc * g.bps) as usize];
        f.read_exact_at(&mut d, g.cluster_byte(g.root)).unwrap();
        let used = d.chunks_exact(32).position(|e| e[0] == 0).unwrap();
        W { f, bitmap, bm_first, root: Dir { clusters: vec![g.root as u32], next: used * 32 }, files: BTreeMap::new(), g }
    }
    fn cs(&self) -> usize {
        (self.g.spc * self.g.bps) as usize
    }
    fn set_bit(&mut self, c: u32, on: bool) {
        let i = (c - 2) as usize;
        if on {
            self.bitmap[i / 8] |= 1 << (i % 8);
        } else {
            self.bitmap[i / 8] &= !(1 << (i % 8));
        }
        self.f.write_all_at(&[self.bitmap[i / 8]], self.g.cluster_byte(self.bm_first) + (i / 8) as u64).unwrap();
    }
    fn free(&self, c: u32) -> bool {
        let i = (c - 2) as usize;
        self.bitmap[i / 8] & (1 << (i % 8)) == 0
    }
    /// The first run of `n` free clusters at or after `from`.
    fn run(&self, n: usize, from: u32) -> u32 {
        let (mut start, mut len) = (0u32, 0usize);
        for c in from..self.g.clusters as u32 + 2 {
            if self.free(c) {
                if len == 0 {
                    start = c;
                }
                len += 1;
                if len == n {
                    return start;
                }
            } else {
                len = 0;
            }
        }
        panic!("no run of {n} clusters");
    }
    fn set_fat(&self, c: u32, v: u32) {
        self.f.write_all_at(&v.to_le_bytes(), self.g.fat_off * self.g.bps + c as u64 * 4).unwrap();
    }
    fn write_cluster(&self, c: u32, data: &[u8]) {
        let mut b = vec![0u8; self.cs()];
        b[..data.len()].copy_from_slice(data);
        self.f.write_all_at(&b, self.g.cluster_byte(c as u64)).unwrap();
    }
    /// The records of one entry set, with the checksum made.
    fn entry_set(name: &str, attr: u16, flags: u8, first: u32, len: u64) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let names = units.len().div_ceil(15);
        let mut set = vec![0u8; 32 * (2 + names)];
        set[0] = 0x85;
        set[1] = (1 + names) as u8;
        set[4..6].copy_from_slice(&attr.to_le_bytes());
        for o in [8, 12, 16] {
            set[o..o + 4].copy_from_slice(&0x5021_0000u32.to_le_bytes());
        }
        set[32] = 0xC0;
        set[33] = flags;
        set[35] = units.len() as u8;
        set[36..38].copy_from_slice(&name_hash(name).to_le_bytes());
        set[40..48].copy_from_slice(&len.to_le_bytes());
        set[52..56].copy_from_slice(&first.to_le_bytes());
        set[56..64].copy_from_slice(&len.to_le_bytes());
        for (i, ch) in units.chunks(15).enumerate() {
            set[64 + i * 32] = 0xC1;
            for (j, u) in ch.iter().enumerate() {
                set[64 + i * 32 + 2 + j * 2..64 + i * 32 + 4 + j * 2].copy_from_slice(&u.to_le_bytes());
            }
        }
        let sum = set_checksum(&set).to_le_bytes();
        set[2..4].copy_from_slice(&sum);
        set
    }
    fn put_set(&mut self, d: &mut Dir, set: &[u8]) -> usize {
        let at = d.next;
        assert!(at + set.len() <= self.cs() * d.clusters.len(), "the test directory is full");
        for (i, rec) in set.chunks(32).enumerate() {
            let off = at + i * 32;
            let c = d.clusters[off / self.cs()];
            self.f.write_all_at(rec, self.g.cluster_byte(c as u64) + (off % self.cs()) as u64).unwrap();
        }
        d.next += set.len();
        at
    }
    /// A file in `d` written in one piece (NoFatChain).
    fn contiguous(&mut self, d: &mut Dir, name: &str, data: Vec<u8>, from: u32) -> (u32, usize) {
        let n = data.len().div_ceil(self.cs()).max(1);
        let first = self.run(n, from);
        for i in 0..n {
            self.set_bit(first + i as u32, true);
            let part = &data[(i * self.cs()).min(data.len())..((i + 1) * self.cs()).min(data.len())];
            self.write_cluster(first + i as u32, part);
        }
        let set = Self::entry_set(name, 0x20, 0x03, first, data.len() as u64);
        let at = self.put_set(d, &set);
        self.files.insert(name.to_string(), data);
        (first, at)
    }
    /// A file in `d` on every second free cluster (a FAT chain with no two clusters next to each other).
    fn fragmented(&mut self, d: &mut Dir, name: &str, data: Vec<u8>, from: u32) -> u32 {
        let n = data.len().div_ceil(self.cs()).max(1);
        let mut cl = Vec::new();
        let mut c = from;
        while cl.len() < n {
            if self.free(c) {
                cl.push(c);
                c += 2;
            } else {
                c += 1;
            }
        }
        for (i, c) in cl.iter().enumerate() {
            self.set_bit(*c, true);
            let part = &data[(i * self.cs()).min(data.len())..((i + 1) * self.cs()).min(data.len())];
            self.write_cluster(*c, part);
            self.set_fat(*c, cl.get(i + 1).copied().unwrap_or(0xFFFF_FFFF));
        }
        let set = Self::entry_set(name, 0x20, 0x01, cl[0], data.len() as u64);
        self.put_set(d, &set);
        self.files.insert(name.to_string(), data);
        cl[0]
    }
    fn mkdir(&mut self, d: &mut Dir, name: &str, from: u32) -> Dir {
        let c = self.run(1, from);
        self.set_bit(c, true);
        self.write_cluster(c, &[]);
        self.set_fat(c, 0xFFFF_FFFF);
        let set = Self::entry_set(name, 0x10, 0x01, c, self.cs() as u64);
        self.put_set(d, &set);
        Dir { clusters: vec![c], next: 0 }
    }
    /// Delete the file whose set starts at `at` in `d`: the records lose their in-use bit and the clusters are freed.
    fn delete(&mut self, d: &Dir, at: usize, first: u32, n: usize) {
        let sets = 3;
        for i in 0..sets {
            let off = at + i * 32;
            let c = d.clusters[off / self.cs()];
            let pos = self.g.cluster_byte(c as u64) + (off % self.cs()) as u64;
            let mut b = [0u8; 1];
            self.f.read_exact_at(&mut b, pos).unwrap();
            self.f.write_all_at(&[b[0] & 0x7F], pos).unwrap();
        }
        for i in 0..n {
            self.set_bit(first + i as u32, false);
        }
    }
}

fn mkfs(d: &Path, mb: u64) -> PathBuf {
    let img = d.join("v.img");
    run("truncate", &["-s", &(mb * MB).to_string(), img.to_str().unwrap()]);
    run("mkfs.exfat", &["-L", "TEST", img.to_str().unwrap()]);
    img
}

/// Every file of the tree read back by its name.
fn read_all(img: &Path) -> BTreeMap<String, Vec<u8>> {
    let f = File::open(img).unwrap();
    let (g, _) = read_boot(&f).unwrap();
    let cs = (g.spc * g.bps) as usize;
    let mut out = BTreeMap::new();
    for it in tree(img).unwrap().items.iter().filter(|i| !i.is_dir && !i.name.is_empty()) {
        let mut all = Vec::new();
        for c in &it.clusters {
            let mut b = vec![0u8; cs];
            f.read_exact_at(&mut b, g.cluster_byte(*c as u64)).unwrap();
            all.extend(b);
        }
        out.insert(it.name.clone(), all);
    }
    out
}

fn same_content(img: &Path, want: &BTreeMap<String, Vec<u8>>) {
    let got = read_all(img);
    assert_eq!(got.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "the same files");
    for (name, bytes) in want {
        assert_eq!(&got[name][..bytes.len()], &bytes[..], "{name} reads back byte for byte");
    }
}

#[test]
fn files_beyond_the_new_end_are_moved_below_it_contiguous_chained_and_in_a_directory_and_fsck_agrees() {
    if !need(&["mkfs.exfat", "fsck.exfat", "truncate"]) {
        return;
    }
    let d = dir("move");
    let img = mkfs(&d, 64);
    let mut w = W::new(&img);
    let mut root = w.root.clone();
    // a big file at the start, deleted later; the files that matter lie after it
    let (filler, at) = w.contiguous(&mut root, "FILLER.BIN", pattern(20 * MB as usize, 1), 2);
    let filler_n = (20 * MB as usize).div_ceil(w.cs());
    let after = filler + filler_n as u32;
    w.contiguous(&mut root, "A.BIN", pattern(MB as usize + 17, 2), after);
    w.fragmented(&mut root, "FRAG.BIN", pattern(300 * 1024, 3), after + 400);
    let mut docs = w.mkdir(&mut root, "DOCS", after + 300);
    w.contiguous(&mut docs, "B.TXT", pattern(5000, 4), after + 600);
    w.fragmented(&mut docs, "C.BIN", pattern(70_000, 5), after + 700);
    w.delete(&root, at, filler, filler_n);
    w.files.remove("FILLER.BIN");
    let want = w.files.clone();
    drop(w);
    fsck_ok(&img);
    let before_hi = { let f = File::open(&img).unwrap(); let (g, _) = read_boot(&f).unwrap(); highest_used(&read_bitmap(&f, &g).unwrap(), &g) };
    let min = minimum(&img).unwrap();
    let (g, _) = read_boot(&File::open(&img).unwrap()).unwrap();
    let min_clusters = (min / g.bps - g.heap_off) / g.spc;
    assert!(min_clusters + 2 < before_hi, "the minimum {min_clusters} is well below the last used cluster {before_hi}");
    assert!(min < crate::exfatshrink::minimum_without_moving(&img).unwrap());
    shrink(&img, min).unwrap();
    same_content(&img, &want);
    // the volume now ends at `min`: cut the file there and let the real checker judge it
    OpenOptions::new().write(true).open(&img).unwrap().set_len(min).unwrap();
    fsck_ok(&img);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_contiguous_file_that_finds_no_free_run_is_refused_before_anything_is_written() {
    if !need(&["mkfs.exfat", "fsck.exfat", "truncate"]) {
        return;
    }
    let d = dir("norun");
    let img = mkfs(&d, 32);
    let mut w = W::new(&img);
    let mut root = w.root.clone();
    let cs = w.cs();
    let start = w.run(1, 2);
    let mut places = Vec::new();
    for i in 0..10 {
        places.push(w.contiguous(&mut root, &format!("S{i}.BIN"), pattern(cs, i as u8), start));
    }
    // every second small file goes: holes of one cluster each
    for i in (0..10).step_by(2) {
        w.delete(&root, places[i].1, places[i].0, 1);
        w.files.remove(&format!("S{i}.BIN"));
    }
    w.contiguous(&mut root, "BIG.BIN", pattern(8 * cs, 9), start + 10);
    drop(w);
    fsck_ok(&img);
    let before = std::fs::read(&img).unwrap();
    // only five holes and then five free clusters lie below this limit: ten free clusters but no run of eight
    let e = compact(&img, start + 15).unwrap_err();
    assert!(e.contains("one piece") && e.contains("nothing was moved"), "{e}");
    assert_eq!(std::fs::read(&img).unwrap(), before, "refused means untouched");
    // a limit above everything used asks for nothing to be moved
    let moved = compact(&img, start + 40).unwrap();
    assert_eq!(moved.chains, 0, "everything is below that limit already");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn what_cannot_move_sets_the_minimum_and_a_damaged_entry_is_refused_in_words() {
    if !need(&["mkfs.exfat", "truncate"]) {
        return;
    }
    let d = dir("fixed");
    let img = mkfs(&d, 32);
    // a cluster far out marked used that no entry names cannot move: the limit may not pass below it
    let (g, _) = read_boot(&File::open(&img).unwrap()).unwrap();
    let w = W::new(&img);
    let mut w = w;
    w.set_bit(5000, true);
    drop(w);
    let min = minimum(&img).unwrap();
    assert_eq!(min, (g.heap_off + (5000 - 1) * g.spc) * g.bps);
    let e = compact(&img, 4000).unwrap_err();
    assert!(e.contains("never moved"), "{e}");
    // a wrong set checksum is found by the walk
    let mut w = W::new(&img);
    let mut root = w.root.clone();
    let (_, at) = w.contiguous(&mut root, "X.BIN", pattern(100, 7), 2);
    drop(w);
    let raw_at = g.cluster_byte(g.root) + at as u64;
    let f = OpenOptions::new().read(true).write(true).open(&img).unwrap();
    f.write_all_at(&[0xFF, 0xFF], raw_at + 2).unwrap();
    assert!(tree(&img).err().unwrap().contains("checksum"));
    let _ = std::fs::remove_dir_all(&d);
}
