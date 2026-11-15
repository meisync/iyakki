//! Fuzz tests (deterministic, seeded) for the shared-disk BIOS boot planner: mutated disk images (MBR and GPT) through `read_facts` and
//! `evaluate`, and mutated save files (with the checksum put right again, so the parser is reached) through `Backup::decode` and `restore`.
//! A damaged save file must be refused in words, and an accepted one must only ever write inside the disk it was made for.
//! `IYAKKI_FUZZ_CASES=20000 cargo test -p iyakki-usb fuzz` runs a larger set.

use crate::biosshared::*;
use crate::fuzzkit::{cases, mutate, run, Rng};
use iyakki_storage::gpt;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::time::Duration;

const MIB: usize = 1 << 20;

/// Re-seals a mutated save file: the trailing SHA-256 of the rest, so the parser behind the checksum is exercised.
fn reseal(mut v: Vec<u8>) -> Vec<u8> {
    if v.len() >= 32 {
        v.truncate(v.len() - 32);
    }
    let sum = Sha256::digest(&v);
    v.extend(sum);
    v
}

/// A sink that remembers where it was written and refuses to be bigger than the disk it stands for.
struct Recorder {
    pos: u64,
    writes: Vec<(u64, u64)>,
}
impl Write for Recorder {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.writes.push((self.pos, b.len() as u64));
        self.pos = self.pos.saturating_add(b.len() as u64);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for Recorder {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        if let SeekFrom::Start(n) = p {
            self.pos = n;
        }
        Ok(self.pos)
    }
}

fn good_backup() -> Vec<u8> {
    Backup { disk_bytes: 8 * MIB as u64, sector_size: 512, regions: vec![(0, vec![0xA5; 512]), (1, vec![7; 1024])] }.encode()
}

#[test]
fn fuzz_bios_save_file_decode_refuses_damage_and_restore_stays_inside_the_disk() {
    let good = good_backup();
    assert!(Backup::decode(&good).is_ok());
    let corpus = vec![good.clone(), Backup { disk_bytes: 1 << 40, sector_size: 4096, regions: vec![(2048, vec![1; 4096])] }.encode()];
    let mut accepted = 0usize;
    let mut k = 0usize;
    let n = run("bios save", 0xB10, &corpus, 20000, Duration::from_secs(1), 64 << 20, |input| {
        k += 1;
        // half of the cases get their checksum put right
        let bytes = if k % 2 == 0 { reseal(input.to_vec()) } else { input.to_vec() };
        let Ok(b) = Backup::decode(&bytes) else { return };
        accepted += 1;
        // an accepted save is for a real kind of disk and its areas lie whole sectors inside it
        assert!(matches!(b.sector_size, 512 | 1024 | 2048 | 4096), "sector size {}", b.sector_size);
        assert!(b.regions.len() <= 16, "{} areas", b.regions.len());
        for (lba, data) in &b.regions {
            assert!(!data.is_empty() && data.len() as u64 % b.sector_size as u64 == 0, "area of {} bytes", data.len());
            let end = lba.checked_mul(b.sector_size as u64).and_then(|o| o.checked_add(data.len() as u64));
            assert!(end.is_some_and(|e| e <= b.disk_bytes), "area at sector {lba} of {} bytes lies outside a disk of {} bytes", data.len(), b.disk_bytes);
        }
        let mut rec = Recorder { pos: 0, writes: vec![] };
        b.restore(&mut rec, b.sector_size, b.disk_bytes).expect("restore on the disk it was made for");
        assert!(rec.writes.iter().all(|(o, l)| o.checked_add(*l).is_some_and(|e| e <= b.disk_bytes)), "a restore wrote outside the disk");
        // another disk is refused
        assert!(b.restore(&mut Recorder { pos: 0, writes: vec![] }, b.sector_size, b.disk_bytes.wrapping_add(1)).is_err());
    });
    assert!(n >= cases(20000) && accepted > 0);
}

#[test]
fn fuzz_bios_save_file_regressions() {
    // an area length near usize::MAX used to overflow `at + len` (a panic in debug builds, a wrong check in release builds)
    let mut v = good_backup();
    let at = 16 + 8 + 4 + 4 + 8; // first area: lba then byte count
    v[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let e = Backup::decode(&reseal(v)).unwrap_err();
    assert!(!e.is_empty());
    // an area outside the disk, an odd length, a made-up sector size and a huge area count are refused in words
    let mk = |disk: u64, ss: u32, r: Vec<(u64, Vec<u8>)>| Backup { disk_bytes: disk, sector_size: ss, regions: r }.encode();
    for (bytes, why) in [
        (mk(1 << 20, 512, vec![(5000, vec![0; 512])]), "outside"),
        (mk(1 << 20, 512, vec![(0, vec![0; 100])]), "whole number"),
        (mk(1 << 20, 7, vec![(0, vec![0; 7])]), "sector size"),
        (mk(1 << 20, 512, vec![(u64::MAX / 256, vec![0; 512])]), "outside"),
        (mk(1 << 20, 512, vec![(0, vec![])]), "whole number"),
        (mk(1 << 20, 512, (0..40).map(|i| (i, vec![0u8; 512])).collect()), "too many"),
    ] {
        let e = Backup::decode(&bytes).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
    // restoring a hand-built save with a wrapping offset is refused too
    let b = Backup { disk_bytes: 1 << 20, sector_size: 4096, regions: vec![(u64::MAX / 2, vec![0; 4096])] };
    assert!(b.restore(&mut Recorder { pos: 0, writes: vec![] }, 4096, 1 << 20).is_err());
}

fn mbr_disk(start: u32, boot: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 4 * MIB];
    d[..boot.len()].copy_from_slice(boot);
    d[440..444].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    let e = 446;
    d[e] = 0x80;
    d[e + 4] = 0x07;
    d[e + 8..e + 12].copy_from_slice(&start.to_le_bytes());
    d[e + 12..e + 16].copy_from_slice(&((4 * MIB / 512) as u32 - start).to_le_bytes());
    d[510] = 0x55;
    d[511] = 0xAA;
    d
}

fn gpt_disk() -> Vec<u8> {
    use gpt::*;
    let total = 4 * MIB as u64 / 512;
    let mut t = Table { sector_size: 512, total_sectors: total, disk_guid: guid_from_str("11111111-2222-3333-4444-555555555555").unwrap(), entries: vec![] };
    t.entries.push(Entry { index: 1, type_guid: TYPE_BIOS_BOOT, guid: [1; 16], first_lba: 2048, last_lba: 2048 + 2047, attrs: 0, name: "BIOS".into() });
    t.entries.push(Entry { index: 2, type_guid: TYPE_LINUX_DATA, guid: [2; 16], first_lba: 4096, last_lba: t.last_usable(), attrs: 0, name: "data".into() });
    let mut d = vec![0u8; 4 * MIB];
    d[..512].copy_from_slice(&t.protective_mbr(&[], [1, 2, 3, 4]));
    d[512..1024].copy_from_slice(&t.header_bytes(false));
    let e = t.entries_bytes();
    d[1024..1024 + e.len()].copy_from_slice(&e);
    let bl = t.backup_entries_lba() as usize * 512;
    d[bl..bl + e.len()].copy_from_slice(&e);
    let hl = (total as usize - 1) * 512;
    d[hl..hl + 512].copy_from_slice(&t.header_bytes(true));
    d
}

#[test]
fn fuzz_bios_planner_on_mutated_partition_tables() {
    let bases = vec![mbr_disk(2048, &[]), mbr_disk(63, &[0x33; 440]), gpt_disk()];
    let mut grub = vec![0x90u8; BOOT_CODE_BYTES];
    grub[0x188..0x18D].copy_from_slice(b"GRUB ");
    let mut r = Rng::new(0xB11);
    let mut parsed = 0usize;
    for case in 0..cases(1500) {
        let mut d = r.pick(&bases).clone();
        // the interesting bytes: sector 0, the GPT header, the entries, the backup header and entries; sometimes the gap or the partition's start
        let (from, len) = match r.below(6) {
            0 => (0, 512),
            1 => (512, 512),
            2 => (1024, 4096),
            3 => (d.len() - 512, 512),
            4 => (d.len() - 512 - 16384, 16384),
            _ => (512 + r.below(1 << 20), 4096),
        };
        let (from, len) = (from.min(d.len() - 1), len.min(d.len() - from.min(d.len() - 1)));
        let mut m = mutate(&mut r, &d[from..from + len]);
        m.resize(len, 0);
        d[from..from + len].copy_from_slice(&m);
        let (ss, size) = *r.pick(&[(512u32, d.len() as u64), (512, d.len() as u64), (4096, d.len() as u64), (512, 0), (512, u64::MAX), (1024, 1 << 30), (512, 511)]);
        let t = std::time::Instant::now();
        if let Ok(f) = read_facts(&mut Cursor::new(d), ss, size) {
            parsed += 1;
            assert!(f.gap.len() <= GAP_READ_MAX as usize && f.bios_boot_content.len() <= GAP_READ_MAX as usize, "case {case}: read more than the limit");
            for core in [0, MIN_CORE_BYTES, 140 * 1024, u64::MAX, u64::MAX / 512] {
                for replace in [false, true] {
                    let c = evaluate(&f, core, replace);
                    let _ = describe(&c);
                    if let Some(m) = c.method {
                        // a plan that says go has no refusal, and its core area is inside the disk
                        assert!(c.refusals.is_empty(), "case {case}: a method with refusals");
                        let _ = regions_to_save(m, core.min(1 << 30));
                    }
                }
            }
            let _ = BootCode::classify(&f.sector0).label();
        }
        assert!(t.elapsed() < Duration::from_secs(5), "case {case} took {:?}", t.elapsed());
    }
    assert!(parsed > 100, "the cases must reach the planner ({parsed})");
    // merged sector and verification with short or odd inputs: a plain refusal, never a panic
    for len in [0usize, 1, 511, 512, 513, 1024] {
        let old = vec![0x55u8; len];
        let _ = merged_sector0(&old, &grub, u64::MAX);
        let _ = merged_sector0(&mbr_disk(2048, &[])[..512], &old, 1);
    }
}
