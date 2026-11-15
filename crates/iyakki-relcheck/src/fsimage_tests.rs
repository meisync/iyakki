use crate::cpio::Kind;
use crate::fsimage::*;

fn paths(v: &[crate::cpio::Entry]) -> Vec<String> {
    let mut p: Vec<String> = v.iter().map(|e| e.path.clone()).collect();
    p.sort();
    p
}

// ---- ISO 9660 ---------------------------------------------------------------------------------------------------------

fn rec(name: &[u8], extent: u32, len: u32, dir: bool) -> Vec<u8> {
    let mut r = vec![0u8; 33 + name.len() + (name.len() + 1) % 2];
    r[0] = r.len() as u8;
    r[2..6].copy_from_slice(&extent.to_le_bytes());
    r[10..14].copy_from_slice(&len.to_le_bytes());
    r[25] = if dir { 2 } else { 0 };
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

fn put(img: &mut [u8], sector: usize, recs: &[Vec<u8>]) {
    let mut at = sector * 2048;
    for r in recs {
        img[at..at + r.len()].copy_from_slice(r);
        at += r.len();
    }
}

fn iso(joliet: bool) -> Vec<u8> {
    let mut img = vec![0u8; 24 * 2048];
    let name = |s: &str| -> Vec<u8> { if joliet { s.encode_utf16().flat_map(u16::to_be_bytes).collect() } else { s.as_bytes().to_vec() } };
    let vd = |kind: u8| {
        let mut d = vec![0u8; 2048];
        d[0] = kind;
        d[1..6].copy_from_slice(b"CD001");
        d
    };
    let mut pvd = vd(if joliet { 2 } else { 1 });
    if joliet {
        pvd[88..91].copy_from_slice(b"%/E");
    }
    pvd[156..190].copy_from_slice(&rec(&[0], 20, 2048, true)[..34]);
    img[16 * 2048..17 * 2048].copy_from_slice(&pvd);
    img[17 * 2048..18 * 2048].copy_from_slice(&vd(255));
    let sfx = if joliet { "" } else { ";1" };
    put(&mut img, 20, &[rec(&[0], 20, 2048, true), rec(&[1], 20, 2048, true), rec(&name("boot"), 21, 2048, true), rec(&name(&format!("README.TXT{sfx}")), 22, 5, false)]);
    put(&mut img, 21, &[rec(&[0], 21, 2048, true), rec(&[1], 20, 2048, true), rec(&name(&format!("vmlinuz{}", if joliet { "" } else { ".;1" })), 23, 99, false)]);
    img
}

#[test]
fn an_iso_lists_folders_and_files_without_version_suffixes_and_with_sizes() {
    for j in [false, true] {
        let l = iso_list(&iso(j)).unwrap();
        let want = if j { vec!["README.TXT", "boot", "boot/vmlinuz"] } else { vec!["README.TXT", "boot", "boot/vmlinuz"] };
        assert_eq!(paths(&l), want, "joliet {j}");
        assert_eq!(l.iter().find(|e| e.path == "boot/vmlinuz").map(|e| (e.size, e.kind.clone())), Some((99, Kind::File)));
        assert_eq!(l.iter().find(|e| e.path == "boot").map(|e| e.kind.clone()), Some(Kind::Dir));
    }
}

#[test]
fn a_directory_that_points_back_a_cut_image_and_garbage_are_refused_not_followed() {
    let mut i = iso(false);
    put(&mut i, 21, &[rec(&[0], 21, 2048, true), rec(&[1], 20, 2048, true), rec(b"LOOP", 20, 2048, true)]);
    assert!(iso_list(&i).unwrap_err().contains("points back"));
    let i = iso(false);
    assert!(iso_list(&i[..21 * 2048]).unwrap_err().contains("outside"));
    assert!(iso_list(&i[..5000]).is_err());
    assert!(iso_list(&[0u8; 100_000]).is_err());
    // every cut length and every flipped byte of the directory sectors: an answer, never a panic
    let base = iso(true);
    for n in (0..base.len()).step_by(997) {
        let _ = iso_list(&base[..n]);
    }
    for k in 20 * 2048..22 * 2048 {
        let mut x = base.clone();
        x[k] ^= 0xFF;
        let _ = iso_list(&x);
    }
}

// ---- FAT16 ------------------------------------------------------------------------------------------------------------

const SECTORS: usize = 5000;
const FAT_SECS: usize = 20;
const DATA0: usize = 1 + FAT_SECS + 2;

fn dirent(short: &[u8; 11], attr: u8, cluster: u16, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(short);
    e[11] = attr;
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

fn lfn(seq: u8, name: &str) -> [u8; 32] {
    let mut u: Vec<u16> = name.encode_utf16().collect();
    u.push(0);
    u.resize(13, 0xFFFF);
    let mut e = [0u8; 32];
    e[0] = seq;
    e[11] = 0x0F;
    for (i, off) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].into_iter().enumerate() {
        e[off..off + 2].copy_from_slice(&u[i].to_le_bytes());
    }
    e
}

fn fat16() -> Vec<u8> {
    let mut img = vec![0u8; SECTORS * 512];
    img[11..13].copy_from_slice(&512u16.to_le_bytes());
    img[13] = 1;
    img[14..16].copy_from_slice(&1u16.to_le_bytes());
    img[16] = 1;
    img[17..19].copy_from_slice(&32u16.to_le_bytes());
    img[19..21].copy_from_slice(&(SECTORS as u16).to_le_bytes());
    img[22..24].copy_from_slice(&(FAT_SECS as u16).to_le_bytes());
    img[54..59].copy_from_slice(b"FAT16");
    img[510] = 0x55;
    img[511] = 0xAA;
    let fat = |img: &mut Vec<u8>, c: usize, v: u16| img[512 + c * 2..512 + c * 2 + 2].copy_from_slice(&v.to_le_bytes());
    fat(&mut img, 0, 0xFFF8);
    fat(&mut img, 1, 0xFFFF);
    fat(&mut img, 2, 0xFFFF); // BOOT: one cluster
    fat(&mut img, 3, 4); // EFI: two clusters
    fat(&mut img, 4, 0xFFFF);
    let root = (1 + FAT_SECS) * 512;
    let mut put_e = |at: usize, e: [u8; 32]| img[at..at + 32].copy_from_slice(&e);
    put_e(root, dirent(b"KERNEL  BIN", 0x20, 9, 1234));
    put_e(root + 32, dirent(b"BOOT       ", 0x10, 2, 0));
    put_e(root + 64, dirent(b"EFI        ", 0x10, 3, 0));
    let mut gone = dirent(b"DELETED TXT", 0x20, 9, 5);
    gone[0] = 0xE5;
    put_e(root + 96, gone);
    put_e(root + 128, dirent(b"LABEL      ", 0x08, 0, 0));
    let boot = (DATA0) * 512; // cluster 2
    put_e(boot, dirent(b".          ", 0x10, 2, 0));
    put_e(boot + 32, dirent(b"..         ", 0x10, 0, 0));
    put_e(boot + 64, lfn(0x41, "Long Name.txt"));
    put_e(boot + 96, dirent(b"LONGNA~1TXT", 0x20, 9, 77));
    // EFI spans clusters 3 and 4: fill 16 entries of cluster 3 with files, the last one in cluster 4
    let efi = (DATA0 + 1) * 512;
    for i in 0..16 {
        let mut n = *b"F0         ";
        n[1] = b'A' + i as u8;
        put_e(efi + i * 32, dirent(&n, 0x20, 9, i as u32));
    }
    put_e(efi + 16 * 32, dirent(b"LAST    EFI", 0x20, 9, 4242));
    img
}

#[test]
fn a_fat_volume_lists_short_and_long_names_folders_and_a_folder_spanning_two_clusters() {
    let l = fat_list(&fat16()).unwrap();
    let p = paths(&l);
    assert!(p.contains(&"KERNEL.BIN".to_string()) && p.contains(&"BOOT".to_string()) && p.contains(&"BOOT/Long Name.txt".to_string()), "{p:?}");
    assert!(p.contains(&"EFI/LAST.EFI".to_string()) && p.contains(&"EFI/FA".to_string()) && p.contains(&"EFI/FP".to_string()), "{p:?}");
    assert!(!p.iter().any(|x| x.contains("DELETED") || x.contains("LABEL") || x.contains("LONGNA")), "{p:?}");
    assert_eq!(l.iter().find(|e| e.path == "EFI/LAST.EFI").unwrap().size, 4242);
    assert_eq!(l.iter().find(|e| e.path == "BOOT").unwrap().kind, Kind::Dir);
}

#[test]
fn a_chain_that_loops_a_short_image_and_a_disk_with_a_table_are_refused() {
    let mut i = fat16();
    i[512 + 4 * 2..512 + 4 * 2 + 2].copy_from_slice(&3u16.to_le_bytes()); // 3 -> 4 -> 3
    assert!(fat_list(&i).unwrap_err().contains("loops"));
    assert!(fat_list(&fat16()[..22 * 512]).is_err());
    let mut mbr = vec![0u8; 100_000];
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    assert!(fat_list(&mbr).unwrap_err().contains("not a FAT"));
    let base = fat16();
    for k in (0..600).chain(21 * 512..24 * 512) {
        let mut x = base.clone();
        x[k] ^= 0xFF;
        let _ = fat_list(&x);
    }
    for n in (0..base.len()).step_by(1999) {
        let _ = fat_list(&base[..n]);
    }
}

#[test]
fn the_first_bytes_tell_what_kind_of_image_a_file_is() {
    assert_eq!(sniff(&fat16()), Sniff::Fat);
    assert_eq!(sniff(&iso(false)), Sniff::Iso);
    assert_eq!(sniff(b"070701000000"), Sniff::Cpio);
    assert_eq!(sniff(&[0x28, 0xb5, 0x2f, 0xfd, 0]), Sniff::Zstd);
    assert_eq!(sniff(&[1, 2, 3]), Sniff::Unknown);
}

#[test]
fn a_file_on_disk_is_recognised_listed_and_a_zst_is_sent_to_the_unpacker() {
    let dir = std::env::temp_dir().join(format!("m46-fsimage-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (i, f, z, x) = (dir.join("a.iso"), dir.join("b.vfat"), dir.join("c.zst"), dir.join("d.bin"));
    std::fs::write(&i, iso(true)).unwrap();
    std::fs::write(&f, fat16()).unwrap();
    std::fs::write(&z, [0x28, 0xb5, 0x2f, 0xfd, 1, 2, 3]).unwrap();
    std::fs::write(&x, b"plain text").unwrap();
    assert_eq!(list_file(&i).unwrap().1, false);
    assert!(list_file(&i).unwrap().0.iter().any(|e| e.path == "boot/vmlinuz"));
    assert!(list_file(&f).unwrap().0.iter().any(|e| e.path == "EFI/LAST.EFI"));
    assert!(list_file(&z).unwrap_err().contains("image-gate.sh"));
    assert!(list_file(&x).unwrap_err().contains("not a cpio"));
    assert!(list_file(&dir.join("missing")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- a whole disk with a partition table ------------------------------------------------------------------------------

fn put_le(img: &mut [u8], at: usize, bytes: &[u8]) {
    img[at..at + bytes.len()].copy_from_slice(bytes);
}

/// An MBR disk: the FAT volume at sector 8 and an unreadable (ext) partition after it.
fn mbr_disk() -> Vec<u8> {
    let vol = fat16();
    let vsec = vol.len().div_ceil(512);
    let mut d = vec![0u8; (8 + vsec + 64) * 512];
    d[510] = 0x55;
    d[511] = 0xAA;
    // entry 1: FAT16, entry 2: Linux (0x83)
    d[446 + 4] = 0x06;
    put_le(&mut d, 446 + 8, &8u32.to_le_bytes());
    put_le(&mut d, 446 + 12, &(vsec as u32).to_le_bytes());
    d[462 + 4] = 0x83;
    put_le(&mut d, 462 + 8, &((8 + vsec) as u32).to_le_bytes());
    put_le(&mut d, 462 + 12, &64u32.to_le_bytes());
    d[8 * 512..8 * 512 + vol.len()].copy_from_slice(&vol);
    d
}

/// A GPT disk: a protective MBR, the header at sector 1, two entries at sector 2: the FAT volume and a blank-named other one.
fn gpt_disk() -> Vec<u8> {
    let vol = fat16();
    let vsec = vol.len().div_ceil(512);
    let first = 40usize;
    let mut d = vec![0u8; (first + vsec + 64) * 512];
    d[510] = 0x55;
    d[511] = 0xAA;
    d[446 + 4] = 0xEE;
    put_le(&mut d, 446 + 8, &1u32.to_le_bytes());
    put_le(&mut d, 446 + 12, &0xffffu32.to_le_bytes());
    put_le(&mut d, 512, b"EFI PART");
    put_le(&mut d, 512 + 72, &2u64.to_le_bytes());
    put_le(&mut d, 512 + 80, &4u32.to_le_bytes());
    put_le(&mut d, 512 + 84, &128u32.to_le_bytes());
    for (i, (s, l)) in [(first, vsec), (first + vsec, 64)].into_iter().enumerate() {
        let o = 2 * 512 + i * 128;
        d[o] = 0xAF + i as u8; // a type GUID that is not all zero
        put_le(&mut d, o + 32, &(s as u64).to_le_bytes());
        put_le(&mut d, o + 40, &((s + l - 1) as u64).to_le_bytes());
    }
    d[first * 512..first * 512 + vol.len()].copy_from_slice(&vol);
    d
}

#[test]
fn a_whole_disk_with_an_mbr_or_a_gpt_is_walked_partition_by_partition_and_the_odd_ones_are_said() {
    let want: Vec<String> = paths(&fat_list(&fat16()).unwrap()).into_iter().map(|p| format!("p1/{p}")).collect();
    assert!(!want.is_empty());
    for (name, d) in [("mbr", mbr_disk()), ("gpt", gpt_disk())] {
        assert_eq!(sniff(&d), Sniff::Disk, "{name}");
        let (list, skipped) = disk_list(&d).unwrap();
        assert_eq!(paths(&list), want, "{name}");
        assert_eq!(skipped.len(), 1, "{name}: {skipped:?}");
        assert!(skipped[0].starts_with("partition 2 "), "{skipped:?}");
    }
    // a bare FAT volume is still a volume and not a disk
    assert_eq!(sniff(&fat16()), Sniff::Fat);
}

#[test]
fn a_disk_that_is_cut_short_or_has_a_bad_table_is_an_error_and_never_a_passing_scan() {
    let d = mbr_disk();
    assert!(disk_list(&d[..9 * 512]).unwrap_err().contains("outside the image"));
    assert!(disk_list(&[0u8; 100]).unwrap_err().contains("no partition table"));
    // a volume whose own boot sector is damaged
    let mut bad = d.clone();
    bad[8 * 512 + 13] = 0;
    assert!(disk_list(&bad).is_err() || disk_list(&bad).unwrap().0.is_empty());
    // a GPT header that lies about its entry size or count
    let mut g = gpt_disk();
    g[512 + 84..512 + 88].copy_from_slice(&7u32.to_le_bytes());
    assert!(partitions(&g).is_none());
    let mut g = gpt_disk();
    g[512 + 80..512 + 84].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(partitions(&g).is_none());
    // extended-partition entries are containers and not volumes; hostile numbers never panic
    let mut e = mbr_disk();
    e[446 + 4] = 0x05;
    assert!(partitions(&e).unwrap().iter().all(|p| p.0 != 8 * 512));
    for k in 0..700 {
        let mut x = gpt_disk();
        x[k] ^= 0xFF;
        let _ = disk_list(&x);
        let mut y = mbr_disk();
        y[k] ^= 0xFF;
        let _ = disk_list(&y);
    }
}
