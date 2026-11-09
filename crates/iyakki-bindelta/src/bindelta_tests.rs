//! The binary delta (bindelta.rs): round trips, refusals, sizes. Small inputs only; one ignored test makes 80 MB.

use super::bindelta::*;

fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut s = seed | 1;
    move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    }
}

fn random(n: usize, seed: u64) -> Vec<u8> {
    let mut r = rng(seed);
    (0..n).map(|_| (r() >> 24) as u8).collect()
}

/// Something that looks like a program: 32-byte "functions" with a 4-byte address field that depends on where they sit.
pub fn program(n: usize, seed: u64, shift: u32) -> Vec<u8> {
    let mut r = rng(seed);
    let templates: Vec<[u8; 32]> = (0..512).map(|_| std::array::from_fn(|_| (r() >> 24) as u8)).collect();
    let mut out = Vec::with_capacity(n + 64);
    let mut i = 0u32;
    while out.len() < n {
        let mut f = templates[(r() % 512) as usize];
        let addr = (out.len() as u32).wrapping_add(shift).wrapping_add(i.wrapping_mul(0));
        f[10..14].copy_from_slice(&addr.to_le_bytes());
        out.extend(f);
        i += 1;
    }
    out.truncate(n);
    out
}

fn round(old: &[u8], new: &[u8]) -> usize {
    let d = make_delta(old, new);
    assert_eq!(apply_delta(old, &d).unwrap(), new);
    d.len()
}

#[test]
fn a_delta_rebuilds_the_new_file_from_the_old_one_for_edges_and_random_data() {
    round(b"", b"");
    round(b"", b"all new");
    round(b"all gone", b"");
    round(b"same", b"same");
    round(b"tiny", b"tiny but longer now");
    let a = random(50_000, 1);
    round(&a, &a);
    round(&a, &random(50_000, 2));
    round(&a, &a[..30_000]);
    let mut longer = a.clone();
    longer.extend(random(10_000, 3));
    round(&a, &longer);
}

#[test]
fn insertions_deletions_and_moves_are_found_and_stay_small() {
    let old = random(300_000, 7);
    // insert 100 bytes in the middle, delete 70 elsewhere, move a 20 000 byte block
    let mut new = Vec::new();
    new.extend(&old[..100_000]);
    new.extend(random(100, 8));
    new.extend(&old[100_000..150_000]);
    new.extend(&old[150_070..250_000]);
    new.extend(&old[270_000..]);
    new.extend(&old[250_000..270_000]);
    let n = round(&old, &new);
    assert!(n < 2_000, "insert, delete and move cost {n} bytes for a 300 000 byte file");
}

#[test]
fn a_five_percent_change_is_much_smaller_than_the_file() {
    let old = random(400_000, 11);
    let mut new = old.clone();
    let mut r = rng(12);
    // 5 percent of the bytes, in 200 runs of 100
    for _ in 0..200 {
        let at = (r() as usize) % (new.len() - 100);
        for b in &mut new[at..at + 100] {
            *b = (r() >> 20) as u8;
        }
    }
    let n = round(&old, &new);
    assert!(n < 40_000 && n < old.len() / 8, "5% changed gave {n} bytes for {}", old.len());
}

#[test]
fn a_relinked_program_with_moved_addresses_is_a_small_delta() {
    // one small change early in the file moves every address after it, like a relinked binary
    let old = program(1_000_000, 5, 0);
    let mut new = old.clone();
    let patch = random(48, 6);
    new.splice(1_000..1_016, patch);
    // every function after the change gets its address field moved by 32
    for chunk in new[2_000..].chunks_mut(32) {
        if chunk.len() == 32 {
            let a = u32::from_le_bytes(chunk[10..14].try_into().unwrap()).wrapping_add(32);
            chunk[10..14].copy_from_slice(&a.to_le_bytes());
        }
    }
    let n = round(&old, &new);
    assert!(n < old.len() / 6, "a relink cost {n} bytes of {}", old.len());
}

#[test]
fn the_header_names_both_files() {
    let (a, b) = (random(1000, 1), random(1200, 2));
    let d = make_delta(&a, &b);
    let i = info(&d).unwrap();
    assert_eq!((i.old_len, i.new_len), (1000, 1200));
    assert_eq!(i.old_sha256, hex(&sha256(&a)));
    assert_eq!(i.new_sha256, hex(&sha256(&b)));
}

#[test]
fn a_delta_is_refused_for_the_wrong_old_file_a_changed_delta_and_a_cut_delta() {
    let old = random(20_000, 21);
    let mut new = old.clone();
    new[5_000..5_100].fill(7);
    let d = make_delta(&old, &new);
    // the wrong old file: another content, the same content one byte off, and a shorter one
    let other = random(20_000, 22);
    assert!(apply_delta(&other, &d).unwrap_err().contains("not the version this delta was made against"));
    let mut off = old.clone();
    off[19_999] ^= 1;
    assert!(apply_delta(&off, &d).unwrap_err().contains("not the version"));
    assert!(apply_delta(&old[..19_999], &d).unwrap_err().contains("not the version"));
    // any flipped byte is found by the trailer
    for at in [0, 9, 20, 60, HEADERISH, d.len() / 2, d.len() - 33, d.len() - 1] {
        let mut t = d.clone();
        t[at] ^= 1;
        assert!(apply_delta(&old, &t).is_err(), "byte {at}");
    }
    // cut off at several places, and nothing at all
    for keep in [0, 7, 100, d.len() / 2, d.len() - 1] {
        assert!(apply_delta(&old, &d[..keep]).is_err(), "kept {keep}");
    }
    assert!(apply_delta(&old, b"not a delta at all, but long enough to pass the length test, really it is not").unwrap_err().contains("not an IYAKKI binary delta"));
}

const HEADERISH: usize = 90;

#[test]
fn a_forged_delta_with_a_fixed_up_trailer_is_still_refused() {
    let old = random(10_000, 31);
    let mut new = old.clone();
    new[100] ^= 0xff;
    let d = make_delta(&old, &new);
    // change the promised hash of the new file and recompute the trailer: the result no longer matches its promise
    let mut f = d[..d.len() - 32].to_vec();
    f[8 + 4 + 8 + 8 + 32] ^= 1;
    let sum = sha256(&f);
    f.extend(sum);
    assert!(apply_delta(&old, &f).unwrap_err().contains("does not match the SHA-256 the delta promises"));
    // a huge announced size is refused before anything is allocated
    let mut g = d[..d.len() - 32].to_vec();
    g[8 + 4 + 8..8 + 4 + 16].copy_from_slice(&(u64::MAX >> 1).to_be_bytes());
    let sum = sha256(&g);
    g.extend(sum);
    assert!(apply_delta(&old, &g).is_err());
}

#[test]
#[ignore = "makes two 80 MB files; run with --ignored --nocapture to see the time and the size"]
fn eighty_megabytes_with_a_small_change_in_seconds() {
    let t = std::time::Instant::now();
    let old = program(80 << 20, 41, 0);
    let mut new = old.clone();
    new.splice(1 << 20..(1 << 20) + 64, random(4_000, 42));
    for chunk in new[(1 << 20) + 4_000..].chunks_mut(32) {
        if chunk.len() == 32 {
            let a = u32::from_le_bytes(chunk[10..14].try_into().unwrap()).wrapping_add(3_936);
            chunk[10..14].copy_from_slice(&a.to_le_bytes());
        }
    }
    println!("made files in {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let d = make_delta(&old, &new);
    println!("make_delta {:?}: {} bytes", t.elapsed(), d.len());
    let t = std::time::Instant::now();
    assert_eq!(apply_delta(&old, &d).unwrap(), new);
    println!("apply_delta {:?}", t.elapsed());
    assert!(d.len() < 20 << 20);
}
