//! The image readers against archives and images made by tools that are not ours: `cpio` (newc), `hdiutil` (an ISO 9660 with Joliet names) and,
//! as a second reader of the same bytes, `bsdtar` (libarchive). A test whose tool is not on the machine says so and passes without checking.

use crate::cpio::{self, Kind};
use crate::fsimage::iso_list;
use std::path::{Path, PathBuf};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool} >/dev/null 2>&1")).status().is_ok_and(|s| s.success())
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-relcheck-real-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

/// A small tree: folders, files of 0, 5 and 1500 bytes, and a link.
fn tree(root: &Path) {
    std::fs::create_dir_all(root.join("usr/bin")).unwrap();
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(root.join("usr/bin/tool"), b"abcde").unwrap();
    std::fs::write(root.join("etc/empty"), b"").unwrap();
    std::fs::write(root.join("usr/big.bin"), vec![7u8; 1500]).unwrap();
    std::os::unix::fs::symlink("big.bin", root.join("usr/link")).unwrap();
}

fn names_by_bsdtar(file: &Path) -> Option<Vec<String>> {
    if !have("bsdtar") {
        return None;
    }
    let o = Command::new("bsdtar").arg("-tf").arg(file).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let mut v: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().map(cpio::norm).filter(|p| !p.is_empty() && p != ".").map(|p| p.trim_end_matches('/').to_string()).collect();
    v.sort();
    Some(v)
}

#[test]
fn a_newc_archive_made_by_the_real_cpio_is_listed_as_the_tree_and_as_a_second_reader_lists_it() {
    if !have("cpio") {
        eprintln!("skipped: no cpio on this machine");
        return;
    }
    let d = tmp("cpio");
    let root = d.join("root");
    tree(&root);
    let out = d.join("x.cpio");
    let script = |fmt: &str| format!("cd '{}' && find . | cpio -o {fmt} > '{}' 2>/dev/null", root.display(), out.display());
    let ok = ["-H newc", "--format newc"].iter().any(|f| Command::new("sh").arg("-c").arg(script(f)).status().is_ok_and(|s| s.success()) && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false));
    if !ok {
        eprintln!("skipped: this cpio does not write newc");
        return;
    }
    let ours = cpio::list(&std::fs::read(&out).unwrap()[..]).unwrap();
    let find = |p: &str| ours.iter().find(|e| e.path == p).unwrap_or_else(|| panic!("{p} in {ours:?}"));
    assert_eq!((find("usr/bin/tool").kind.clone(), find("usr/bin/tool").size), (Kind::File, 5));
    assert_eq!((find("etc/empty").kind.clone(), find("etc/empty").size), (Kind::File, 0));
    assert_eq!((find("usr/big.bin").kind.clone(), find("usr/big.bin").size), (Kind::File, 1500));
    assert_eq!(find("usr").kind, Kind::Dir);
    let l = find("usr/link");
    assert_eq!((l.kind.clone(), l.target.as_str()), (Kind::Symlink, "big.bin"));
    let mut mine: Vec<String> = ours.iter().map(|e| e.path.clone()).collect();
    mine.sort();
    assert_eq!(mine, ["etc", "etc/empty", "usr", "usr/big.bin", "usr/bin", "usr/bin/tool", "usr/link"]);
    if let Some(theirs) = names_by_bsdtar(&out) {
        assert_eq!(mine, theirs, "libarchive reads the same names from the same bytes");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_iso_made_by_hdiutil_is_listed_with_its_long_names_and_as_a_second_reader_lists_it() {
    if !have("hdiutil") {
        eprintln!("skipped: no hdiutil on this machine");
        return;
    }
    let d = tmp("iso");
    let root = d.join("root");
    std::fs::create_dir_all(root.join("boot")).unwrap();
    std::fs::write(root.join("boot/vmlinuz-long-name"), vec![1u8; 3000]).unwrap();
    std::fs::write(root.join("README.txt"), b"hello").unwrap();
    std::fs::create_dir_all(root.join("docs/deeper")).unwrap();
    std::fs::write(root.join("docs/deeper/Licence Text.txt"), b"x").unwrap();
    let out = d.join("image");
    let made = Command::new("hdiutil").args(["makehybrid", "-iso", "-joliet", "-o"]).arg(&out).arg(&root).output();
    let iso = d.join("image.iso");
    if !made.is_ok_and(|o| o.status.success()) || !iso.is_file() {
        eprintln!("skipped: hdiutil could not make an ISO here");
        return;
    }
    let ours = iso_list(&std::fs::read(&iso).unwrap()).unwrap();
    let mut mine: Vec<String> = ours.iter().map(|e| e.path.clone()).collect();
    mine.sort();
    assert_eq!(mine, ["README.txt", "boot", "boot/vmlinuz-long-name", "docs", "docs/deeper", "docs/deeper/Licence Text.txt"]);
    assert_eq!(ours.iter().find(|e| e.path == "boot/vmlinuz-long-name").map(|e| (e.size, e.kind.clone())), Some((3000, Kind::File)));
    assert_eq!(ours.iter().find(|e| e.path == "boot").map(|e| e.kind.clone()), Some(Kind::Dir));
    if let Some(theirs) = names_by_bsdtar(&iso) {
        assert_eq!(mine, theirs, "libarchive reads the same names from the same bytes");
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---- whole disks and ISOs made by the real tools of a Linux machine (sgdisk, sfdisk, mkfs.fat, mke2fs, mtools, xorriso), read by the scan's own readers, and read again by a second reader

fn run(cmd: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

fn out(cmd: &str) -> String {
    let o = Command::new("sh").arg("-c").arg(format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")).output().unwrap();
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// The names mtools lists on the FAT volume at `offset` bytes of `img` (every file and folder, no leading `::/`), sorted and lower-cased.
fn names_by_mtools(img: &Path, offset: u64) -> Vec<String> {
    let mut v: Vec<String> = out(&format!("mdir -i '{}@@{offset}' -/ -b -a ::", img.display())).lines().map(|l| l.trim().trim_start_matches("::").trim_matches('/').to_string()).filter(|l| !l.is_empty()).map(|l| l.to_lowercase()).collect();
    v.sort();
    v.dedup();
    v
}

/// A little tree of the kind a boot volume holds, written into the FAT volume at `offset` with mtools.
fn fill_fat(img: &Path, offset: u64, scratch: &Path) -> bool {
    let src = scratch.join("fatsrc");
    std::fs::create_dir_all(src.join("EFI/iyakki-boot")).unwrap();
    std::fs::create_dir_all(src.join("boot/grub/themes")).unwrap();
    std::fs::write(src.join("boot/bzImage"), vec![9u8; 40_000]).unwrap();
    std::fs::write(src.join("boot/initrd"), vec![8u8; 90_000]).unwrap();
    std::fs::write(src.join("boot/grub/grub.cfg"), b"set timeout=3\n").unwrap();
    std::fs::write(src.join("boot/grub/themes/a-long-theme-file-name.txt"), b"x").unwrap();
    std::fs::write(src.join("EFI/iyakki-boot/loader.efi"), vec![1u8; 500]).unwrap();
    run(&format!("mcopy -s -m -i '{}@@{offset}' '{}'/* ::", img.display(), src.display()))
}

fn disk_names(img: &Path) -> (Vec<String>, Vec<String>) {
    let (list, skipped) = crate::fsimage::disk_list(&std::fs::read(img).unwrap()).unwrap();
    let mut names: Vec<String> = list.iter().map(|e| e.path.clone()).collect();
    names.sort();
    (names, skipped)
}

fn without_prefix(names: &[String], p: &str) -> Vec<String> {
    let pre = format!("{p}/");
    let mut v: Vec<String> = names.iter().filter_map(|n| n.strip_prefix(&pre)).map(|n| n.to_lowercase()).collect();
    v.sort();
    v
}

#[test]
fn a_gpt_disk_made_by_sgdisk_mkfs_fat_and_mtools_is_walked_and_the_real_tools_list_the_same_files() {
    if !["sgdisk", "mkfs.fat", "mcopy", "mdir", "mke2fs"].iter().all(|t| have(t) || std::path::Path::new(&format!("/usr/sbin/{t}")).exists()) {
        eprintln!("skipped: needs sgdisk, mkfs.fat, mtools and mke2fs");
        return;
    }
    let d = tmp("gpt");
    let img = d.join("disk.img");
    assert!(run(&format!("truncate -s 80M '{}' && sgdisk -o -n 1:2048:+24M -t 1:ef00 -c 1:IYAKKI-BOOT -n 2:0:+24M -t 2:8300 -c 2:IYAKKI-DATA -n 3:0:+8M -t 3:8200 '{}'", img.display(), img.display())), "sgdisk made the table");
    // the first partition starts at sector 2048 (1 MiB), 24 MiB long
    assert!(run(&format!("mkfs.fat -F 32 -n IYAKKIBOOT --offset 2048 '{}' 24576", img.display())), "mkfs.fat made the FAT32 volume");
    assert!(fill_fat(&img, 2048 * 512, &d), "mtools wrote the files");
    let p2_start = 2048u64 + 24 * 2048; // sectors
    assert!(run(&format!("mke2fs -q -t ext4 -E offset={} '{}' 24576k", p2_start * 512, img.display())), "mke2fs made the ext4 volume");
    let (names, skipped) = disk_names(&img);
    let ours = without_prefix(&names, "p1");
    let theirs = names_by_mtools(&img, 2048 * 512);
    assert_eq!(ours, theirs, "the scan and mtools list the same files of the FAT volume");
    assert!(ours.contains(&"boot/bzimage".to_string()) && ours.contains(&"boot/grub/themes/a-long-theme-file-name.txt".to_string()), "{ours:?}");
    assert_eq!(skipped.len(), 2, "the ext4 and the swap partitions are said, not read: {skipped:?}");
    // the sizes the scan reads are the sizes the files have
    let (list, _) = crate::fsimage::disk_list(&std::fs::read(&img).unwrap()).unwrap();
    let sz = |p: &str| list.iter().find(|e| e.path.eq_ignore_ascii_case(p)).map(|e| e.size);
    assert_eq!((sz("p1/boot/bzImage"), sz("p1/boot/initrd"), sz("p1/boot/grub/grub.cfg")), (Some(40_000), Some(90_000), Some(14)));
    // sgdisk itself lists the three partitions the scan walked
    let table = out(&format!("sgdisk -p '{}'", img.display()));
    assert!(table.matches("8300").count() == 1 && table.contains("EF00") && table.contains("8200"), "{table}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_mbr_disk_made_by_sfdisk_and_mkfs_fat_is_walked_and_a_cut_copy_is_refused() {
    if !["sfdisk", "mkfs.fat", "mcopy", "mdir"].iter().all(|t| have(t) || std::path::Path::new(&format!("/usr/sbin/{t}")).exists()) {
        eprintln!("skipped: needs sfdisk, mkfs.fat and mtools");
        return;
    }
    let d = tmp("mbr");
    let img = d.join("disk.img");
    assert!(run(&format!("truncate -s 64M '{}' && printf 'label: dos\\n,24M,c,*\\n,,83\\n' | sfdisk -q '{}'", img.display(), img.display())), "sfdisk made the MBR");
    let start = out(&format!("sfdisk -d '{}'", img.display())).lines().find_map(|l| l.split("start=").nth(1).and_then(|r| r.split(',').next()).map(|s| s.trim().parse::<u64>().unwrap())).expect("the first partition's start");
    assert!(run(&format!("mkfs.fat -F 32 -n BOOT --offset {start} '{}' 24576", img.display())), "mkfs.fat");
    assert!(fill_fat(&img, start * 512, &d));
    let (names, skipped) = disk_names(&img);
    assert_eq!(without_prefix(&names, "p1"), names_by_mtools(&img, start * 512), "the scan and mtools list the same files");
    assert_eq!(skipped.len(), 1, "the second partition (type 83, blank) is said, not read: {skipped:?}");
    // a copy cut short in the middle of the first volume is an error, never a passing scan
    let cut = d.join("cut.img");
    assert!(run(&format!("head -c 6000000 '{}' > '{}'", img.display(), cut.display())));
    assert!(crate::fsimage::disk_list(&std::fs::read(&cut).unwrap()).is_err(), "a cut image must not pass");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_iso_made_by_xorriso_with_rock_ridge_and_joliet_is_listed_like_xorriso_lists_it() {
    if !have("xorriso") {
        eprintln!("skipped: no xorriso");
        return;
    }
    let d = tmp("xorriso");
    let root = d.join("root");
    std::fs::create_dir_all(root.join("boot/grub")).unwrap();
    std::fs::create_dir_all(root.join("docs/deeper")).unwrap();
    std::fs::write(root.join("boot/vmlinuz-with-a-long-name"), vec![1u8; 5000]).unwrap();
    std::fs::write(root.join("boot/grub/grub.cfg"), b"menuentry x {}\n").unwrap();
    std::fs::write(root.join("docs/deeper/Licence Text.txt"), b"x").unwrap();
    std::fs::write(root.join("README.txt"), b"hello").unwrap();
    let iso = d.join("x.iso");
    assert!(run(&format!("xorriso -as mkisofs -quiet -J -R -V IYAKKI -o '{}' '{}'", iso.display(), root.display())), "xorriso made the ISO");
    let ours = iso_list(&std::fs::read(&iso).unwrap()).unwrap();
    let mut mine: Vec<String> = ours.iter().map(|e| e.path.clone()).collect();
    mine.sort();
    assert_eq!(mine, ["README.txt", "boot", "boot/grub", "boot/grub/grub.cfg", "boot/vmlinuz-with-a-long-name", "docs", "docs/deeper", "docs/deeper/Licence Text.txt"]);
    assert_eq!(ours.iter().find(|e| e.path == "boot/vmlinuz-with-a-long-name").map(|e| (e.size, e.kind.clone())), Some((5000, Kind::File)));
    // the second reader: xorriso itself lists the same paths
    let mut theirs: Vec<String> = out(&format!("xorriso -indev '{}' -find 2>/dev/null", iso.display())).lines().map(|l| l.trim().trim_matches('\'').trim_start_matches("./").trim_start_matches('/').to_string()).filter(|l| !l.is_empty() && l != "." && !l.starts_with("xorriso")).collect();
    theirs.sort();
    theirs.dedup();
    assert_eq!(mine, theirs, "xorriso reads the same names from the same bytes");
    if let Some(b) = names_by_bsdtar(&iso) {
        assert_eq!(mine, b, "libarchive too");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A real image of IYAKKI (`IYAKKI_REAL_INSTALL_IMAGE=/path/iyakki-install-x86_64.img`, made by the build's genimage and mkfs): the scan walks it and lists what mtools lists.
#[test]
fn a_real_iyakki_install_image_is_walked_and_mtools_lists_the_same_files() {
    let Ok(p) = std::env::var("IYAKKI_REAL_INSTALL_IMAGE") else {
        eprintln!("skipped: IYAKKI_REAL_INSTALL_IMAGE is not set");
        return;
    };
    let img = Path::new(&p);
    assert!(img.is_file(), "{p}");
    if !have("mdir") {
        eprintln!("skipped: no mtools");
        return;
    }
    let (list, skipped) = crate::fsimage::disk_list(&std::fs::read(img).unwrap()).unwrap();
    let mut names: Vec<String> = list.iter().map(|e| e.path.clone()).collect();
    names.sort();
    let sfd = out(&format!("sfdisk -d '{}'", img.display()));
    let start: u64 = sfd.lines().find_map(|l| l.split("start=").nth(1).and_then(|r| r.split(',').next()).map(|s| s.trim().parse().unwrap())).expect("a first partition");
    let theirs = names_by_mtools(img, start * 512);
    assert_eq!(without_prefix(&names, "p1"), theirs, "the scan and mtools list the same files of the boot volume");
    assert!(theirs.iter().any(|n| n == "boot/bzimage") && theirs.iter().any(|n| n.starts_with("efi/")), "{theirs:?}");
    eprintln!("{} files on the boot volume; not opened: {skipped:?}", theirs.len());
}
