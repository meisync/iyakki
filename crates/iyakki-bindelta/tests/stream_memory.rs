//! A whole-file run of the streaming delta apply with a counting allocator: the heap the apply itself asks for stays a few hundred KiB
//! however large the files are. The size comes from `IYAKKI_BDS_MIB` (default 24 MiB; 400 reproduces a base image).

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};

use iyakki_bindelta::bindelta::{apply_delta_files, hex, make_delta, sha256};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::SeqCst) + l.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Ordering::SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = System.realloc(p, l, new);
        if !q.is_null() {
            if new >= l.size() {
                let now = LIVE.fetch_add(new - l.size(), Ordering::SeqCst) + (new - l.size());
                PEAK.fetch_max(now, Ordering::SeqCst);
            } else {
                LIVE.fetch_sub(l.size() - new, Ordering::SeqCst);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// A program-like file: 32-byte "functions" with an address field that depends on where they sit.
fn program(n: usize, seed: u64, shift: u32) -> Vec<u8> {
    let mut s = seed | 1;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let templates: Vec<[u8; 32]> = (0..512).map(|_| std::array::from_fn(|_| (next() >> 24) as u8)).collect();
    let mut out = Vec::with_capacity(n + 64);
    while out.len() < n {
        let mut f = templates[(next() % 512) as usize];
        f[10..14].copy_from_slice(&(out.len() as u32).wrapping_add(shift).to_le_bytes());
        out.extend(f);
    }
    out.truncate(n);
    out
}

fn file_sha(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    hex(&h.finalize())
}

#[test]
fn applying_a_delta_to_whole_files_asks_the_heap_for_a_few_hundred_kib_whatever_their_size() {
    let mib: usize = std::env::var("IYAKKI_BDS_MIB").ok().and_then(|v| v.parse().ok()).unwrap_or(24);
    let dir = std::env::temp_dir().join(format!("iyakki-bds-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (old_p, delta_p, out_p) = (dir.join("old.img"), dir.join("update.delta"), dir.join("new.img"));
    let new_sha;
    let new_len;
    {
        let old = program(mib << 20, 11, 0);
        let new = program((mib << 20) + 250_000, 11, 8192);
        let delta = make_delta(&old, &new);
        assert!(delta.len() < new.len() / 2, "the delta is much smaller than the file: {} of {}", delta.len(), new.len());
        new_sha = hex(&sha256(&new));
        new_len = new.len() as u64;
        std::fs::write(&old_p, &old).unwrap();
        std::fs::write(&delta_p, &delta).unwrap();
    } // the in-memory copies are gone before the streaming apply is measured
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let info = apply_delta_files(&old_p, &delta_p, &out_p).unwrap();
    let extra = PEAK.load(Ordering::SeqCst) - base;
    assert_eq!((info.new_len, info.new_sha256.as_str()), (new_len, new_sha.as_str()));
    assert_eq!(std::fs::metadata(&out_p).unwrap().len(), new_len);
    assert_eq!(file_sha(&out_p), new_sha, "the file on disk is the promised one");
    assert!(!dir.join("new.img.applying").exists(), "no temporary file is left");
    // the bound does not grow with the files: well under 4 MiB for 24 MiB files and for 400 MiB ones
    assert!(extra < 4 << 20, "the apply asked the heap for {extra} bytes for {mib} MiB files");
    println!("{mib} MiB files: delta applied, peak extra heap {extra} bytes");
    let _ = std::fs::remove_dir_all(&dir);
}
