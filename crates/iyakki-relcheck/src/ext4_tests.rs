use crate::cpio::Kind;
use crate::ext4::*;
use crate::fsimage::*;
use std::process::Command;

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t} >/dev/null 2>&1 || test -x /usr/sbin/{t} || test -x /sbin/{t}")]).status().is_ok_and(|s| s.success())
}

fn tool(t: &str) -> String {
    for d in ["/usr/sbin", "/sbin", "/usr/bin", "/opt/homebrew/opt/e2fsprogs/sbin", "/usr/local/opt/e2fsprogs/sbin"] {
        let p = format!("{d}/{t}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    t.to_string()
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-ext4-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A folder with known content: nested folders, a long name, a big file, a fast and a long symlink, an executable.
fn populate(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("usr/lib/x")).unwrap();
    std::fs::create_dir_all(dir.join("etc")).unwrap();
    std::fs::write(dir.join("etc/hostname"), b"iyakki\n").unwrap();
    std::fs::write(dir.join("usr/lib/x/big.bin"), vec![7u8; 700_000]).unwrap();
    std::fs::write(dir.join(format!("etc/{}", "n".repeat(200))), b"long").unwrap();
    std::fs::write(dir.join("usr/lib/x/prog"), b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        std::fs::set_permissions(dir.join("usr/lib/x/prog"), std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("hostname", dir.join("etc/host")).unwrap();
        symlink(format!("/usr/lib/{}", "t".repeat(90)), dir.join("etc/long-link")).unwrap();
    }
}

fn make(img: &std::path::Path, src: &std::path::Path, mib: u64, opts: &[&str]) -> bool {
    let f = std::fs::File::create(img).unwrap();
    f.set_len(mib << 20).unwrap();
    let o = Command::new(tool("mke2fs")).args(["-q", "-F"]).args(opts).arg("-d").arg(src).arg(img).output().unwrap();
    o.status.success()
}

fn check(list: &[crate::cpio::Entry]) {
    let get = |p: &str| list.iter().find(|e| e.path == p).unwrap_or_else(|| panic!("{p} is not listed: {:?}", list.iter().map(|e| &e.path).collect::<Vec<_>>()));
    assert!(get("etc").kind == Kind::Dir);
    assert!(get("etc/hostname").kind == Kind::File && get("etc/hostname").size == 7);
    assert_eq!(get("usr/lib/x/big.bin").size, 700_000);
    assert_eq!(get(&format!("etc/{}", "n".repeat(200))).size, 4);
    assert_eq!(get("usr/lib/x/prog").mode & 0o777, 0o755);
    assert!(get("etc/host").kind == Kind::Symlink && get("etc/host").target == "hostname");
    assert_eq!(get("etc/long-link").target, format!("/usr/lib/{}", "t".repeat(90)));
    assert!(list.iter().any(|e| e.path == "lost+found"));
}

#[test]
fn an_ext4_volume_made_by_the_real_mke2fs_is_listed_with_names_kinds_sizes_modes_and_links() {
    if !have("mke2fs") {
        eprintln!("skipped: mke2fs is not installed");
        return;
    }
    let d = scratch("ext4");
    populate(&d.join("src"));
    let img = d.join("v.ext4");
    if !make(&img, &d.join("src"), 8, &["-t", "ext4", "-b", "1024", "-I", "256"]) {
        eprintln!("skipped: this mke2fs cannot fill a volume from a folder (-d)");
        return;
    }
    let bytes = std::fs::read(&img).unwrap();
    assert_eq!(sniff(&bytes[..65536]), Sniff::Ext);
    let list = ext_list(&bytes).unwrap();
    check(&list);
    // the same list through the file entry point
    let (l2, rootfs) = list_file(&img).unwrap();
    assert!(!rootfs && l2 == list);
    // a copy cut in the middle of the inode tables is an error, not a pass
    assert!(ext_list(&bytes[..bytes.len() / 8]).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_ext2_volume_with_block_maps_and_indirect_blocks_is_listed_the_same_way() {
    if !have("mke2fs") {
        eprintln!("skipped: mke2fs is not installed");
        return;
    }
    let d = scratch("ext2");
    populate(&d.join("src"));
    let img = d.join("v.ext2");
    if !make(&img, &d.join("src"), 8, &["-t", "ext2", "-b", "1024", "-O", "^resize_inode"]) {
        eprintln!("skipped: this mke2fs cannot fill a volume from a folder (-d)");
        return;
    }
    let list = ext_list(&std::fs::read(&img).unwrap()).unwrap();
    check(&list);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn damage_in_the_superblock_or_a_directory_loop_is_an_error() {
    assert!(ext_list(&[0u8; 2048]).unwrap_err().contains("not an ext"));
    assert!(ext_list(&[0u8; 100]).unwrap_err().contains("cut short"));
    let mut sb = vec![0u8; 4096];
    sb[1024 + 56..1024 + 58].copy_from_slice(&0xEF53u16.to_le_bytes());
    sb[1024 + 24..1024 + 28].copy_from_slice(&40u32.to_le_bytes());
    assert!(ext_list(&sb).unwrap_err().contains("not believable"));
    assert!(is_ext(&sb) && !is_ext(&[0u8; 2000]));
}

// ---- partition tables with extended partitions, hybrid tables, and a .zst

fn put(img: &mut [u8], at: usize, bytes: &[u8]) {
    img[at..at + bytes.len()].copy_from_slice(bytes);
}
fn entry(img: &mut [u8], slot: usize, ty: u8, start: u32, len: u32) {
    let o = 446 + slot * 16;
    img[o + 4] = ty;
    put(img, o + 8, &start.to_le_bytes());
    put(img, o + 12, &len.to_le_bytes());
}

/// An MBR disk: primary 1 (sectors 8..24), an extended partition at sector 100 (to 400) whose first EBR holds a logical partition and
/// a link to a second EBR at 100+150, which holds another logical partition.
fn extended_disk() -> Vec<u8> {
    let mut d = vec![0u8; 400 * 512];
    d[510] = 0x55;
    d[511] = 0xAA;
    entry(&mut d, 0, 0x83, 8, 16);
    entry(&mut d, 1, 0x0F, 100, 300);
    // EBR 1 at sector 100: logical at +2 (len 20), link to +150
    let e1 = 100 * 512;
    d[e1 + 510] = 0x55;
    d[e1 + 511] = 0xAA;
    entry(&mut d[e1..e1 + 512], 0, 0x83, 2, 20);
    entry(&mut d[e1..e1 + 512], 1, 0x05, 150, 100);
    let e2 = 250 * 512;
    d[e2 + 510] = 0x55;
    d[e2 + 511] = 0xAA;
    entry(&mut d[e2..e2 + 512], 0, 0x83, 2, 30);
    d
}

#[test]
fn logical_partitions_of_an_extended_partition_are_walked_through_the_ebr_chain() {
    let d = extended_disk();
    let p = partitions_checked(&d).unwrap().unwrap();
    assert_eq!(p, [(8 * 512, 16 * 512), (102 * 512, 20 * 512), (252 * 512, 30 * 512)]);
    assert_eq!(sniff(&d), Sniff::Disk);
    // a chain that points back to itself, and one that points outside the image, are errors
    let mut looped = d.clone();
    entry(&mut looped[250 * 512..251 * 512], 1, 0x05, 0, 100);
    assert!(partitions_checked(&looped).unwrap_err().contains("points back"));
    let mut cut = d.clone();
    cut.truncate(200 * 512);
    assert!(partitions_checked(&cut).unwrap_err().contains("cut short"));
    let mut nosig = d.clone();
    nosig[100 * 512 + 511] = 0;
    assert!(partitions_checked(&nosig).unwrap_err().contains("signature"));
    assert!(disk_list(&cut).is_err());
}

#[test]
fn a_hybrid_table_reads_the_gpt_once_and_says_so() {
    // a GPT with one entry, and a protective entry plus a real one in the MBR
    let mut d = vec![0u8; 80 * 512];
    d[510] = 0x55;
    d[511] = 0xAA;
    entry(&mut d, 0, 0xEE, 1, 79);
    entry(&mut d, 1, 0x0B, 40, 30);
    put(&mut d, 512, b"EFI PART");
    put(&mut d, 512 + 72, &2u64.to_le_bytes());
    put(&mut d, 512 + 80, &1u32.to_le_bytes());
    put(&mut d, 512 + 84, &128u32.to_le_bytes());
    d[2 * 512] = 0xAF;
    put(&mut d, 2 * 512 + 32, &40u64.to_le_bytes());
    put(&mut d, 2 * 512 + 40, &69u64.to_le_bytes());
    assert!(is_hybrid(&d));
    assert_eq!(partitions_checked(&d).unwrap().unwrap(), [(40 * 512, 30 * 512)]);
    let (_, notes) = disk_list(&d).unwrap();
    assert!(notes.iter().any(|n| n.starts_with("hybrid MBR")), "{notes:?}");
    // a plain protective MBR is not hybrid
    entry(&mut d, 1, 0, 0, 0);
    assert!(!is_hybrid(&d));
}

#[test]
fn a_zst_root_file_system_is_decoded_by_our_own_path_and_listed() {
    let d = scratch("zst");
    // a tiny newc archive
    let mut cpio = Vec::new();
    for (name, body) in [("etc", None), ("etc/hostname", Some(&b"iyakki\n"[..])), ("TRAILER!!!", None)] {
        let mode = if body.is_some() { 0o100644 } else { 0o040755 };
        let size = body.map_or(0, |b| b.len());
        cpio.extend_from_slice(format!("070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}", 1, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, name.len() + 1, 0).as_bytes());
        cpio.extend_from_slice(name.as_bytes());
        cpio.push(0);
        while cpio.len() % 4 != 0 {
            cpio.push(0);
        }
        if let Some(b) = body {
            cpio.extend_from_slice(b);
            while cpio.len() % 4 != 0 {
                cpio.push(0);
            }
        }
    }
    let z = ruzstd::encoding::compress_to_vec(&cpio[..], ruzstd::encoding::CompressionLevel::Fastest);
    let path = d.join("rootfs.cpio.zst");
    std::fs::write(&path, &z).unwrap();
    let (list, rootfs) = list_file(&path).unwrap();
    assert!(rootfs);
    assert!(list.iter().any(|e| e.path == "etc/hostname" && e.size == 7), "{list:?}");
    // a zst that holds something else, and a damaged one, are errors in words
    let other = ruzstd::encoding::compress_to_vec(&b"plain text, not an archive"[..], ruzstd::encoding::CompressionLevel::Fastest);
    std::fs::write(d.join("x.zst"), &other).unwrap();
    assert!(list_file(&d.join("x.zst")).is_err());
    let mut bad = z.clone();
    let n = bad.len();
    bad.truncate(n - 6);
    std::fs::write(d.join("y.zst"), &bad).unwrap();
    assert!(list_file(&d.join("y.zst")).is_err());
    let _ = std::fs::remove_dir_all(&d);
}
