//! F-INST-27: shrinking a partition, against disk images made by the real tools (`sgdisk`, `sfdisk`, `mke2fs`, `mkfs.ntfs`) and
//! shrunk by the real `e2fsck`/`resize2fs`/`ntfsresize`; the result is judged by the same tools plus `debugfs` reading a file back.
//! Image files only, never a device. A missing tool skips the test and says so.

use crate::gpt_install::TableKind;
use crate::planner::*;
use crate::resize::*;
use std::path::{Path, PathBuf};
use std::process::Command;

const MB: u64 = 1024 * 1024;

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t} >/dev/null")]).status().is_ok_and(|s| s.success())
}
fn need(ts: &[&str]) -> bool {
    for t in ts {
        if !have(t) {
            eprintln!("skipped: {t} is not installed");
            return false;
        }
    }
    true
}
fn run(cmd: &str, args: &[&str]) -> String {
    let o = Command::new(cmd).args(args).output().unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert!(o.status.success(), "{cmd} {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}
fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-resize-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn model(disk: &Path, size: u64) -> DiskModel {
    let meta = DiskMeta { name: "sdz".into(), path: disk.to_str().unwrap().into(), model: "file".into(), serial: "x".into(), removable: true, usb: true, read_only: false, swap_parts: vec![] };
    read_model(&mut std::fs::File::open(disk).unwrap(), &meta, size, 512, &[], None)
}
/// Make a file system with its own tool, then copy it to the partition.
fn put_fs(d: &Path, disk: &Path, m: &DiskModel, idx: u32, tool: &str, args: &[&str], populate: Option<&Path>) {
    let p = m.part(idx).unwrap();
    let img = d.join(format!("fs{idx}.img"));
    run("truncate", &["-s", &p.bytes(512).to_string(), img.to_str().unwrap()]);
    let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if let Some(src) = populate {
        a.extend(["-d".into(), src.to_str().unwrap().into()]);
    }
    a.push(img.to_str().unwrap().into());
    let r: Vec<&str> = a.iter().map(String::as_str).collect();
    run(tool, &r);
    run("dd", &[&format!("if={}", img.display()), &format!("of={}", disk.display()), "bs=512", &format!("seek={}", p.first_lba), "conv=notrunc", "status=none"]);
}
fn region(disk: &Path, p: &Part, to: &Path, bytes: u64) {
    run("dd", &[&format!("if={}", disk.display()), &format!("of={}", to.display()), "bs=512", &format!("skip={}", p.first_lba), &format!("count={}", bytes / 512), "status=none"]);
}
fn confirm(plan: &ShrinkPlan) -> ShrinkConfirmation {
    ShrinkConfirmation { drive: plan.disk_name.clone(), part: plan.part, new_bytes: plan.new_bytes(), ack: true }
}

fn payload(d: &Path) -> PathBuf {
    let src = d.join("payload");
    std::fs::create_dir_all(&src).unwrap();
    // 12 MiB of a known pattern, so that a shrink that damaged a block shows
    let data: Vec<u8> = (0..12 * MB).map(|i| (i.wrapping_mul(31) >> 3) as u8).collect();
    std::fs::write(src.join("keep.bin"), &data).unwrap();
    src
}

fn shrink_ext4_on(table: &str) {
    let d = dir(table);
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    let total = 400 * MB;
    run("truncate", &["-s", &total.to_string(), ds]);
    if table == "gpt" {
        run("sgdisk", &["-o", ds]);
        run("sgdisk", &["-n", "1:0:+40M", "-t", "1:0700", ds]);
        run("sgdisk", &["-n", "2:0:+300M", "-t", "2:8300", ds]);
    } else {
        let mut c = Command::new("sfdisk").arg(ds).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        std::io::Write::write_all(c.stdin.as_mut().unwrap(), b"label: dos\nunit: sectors\n\n2048, 81920, c\n86016, 614400, 83\n").unwrap();
        assert!(c.wait().unwrap().success());
    }
    let m0 = model(&disk, total);
    let pay = payload(&d);
    put_fs(&d, &disk, &m0, 2, "mke2fs", &["-q", "-t", "ext4", "-L", "OTHERDATA"], Some(&pay));
    put_fs(&d, &disk, &m0, 1, "mkfs.fat", &["-F", "32", "-n", "KEEPME"], None);
    let m = model(&disk, total);
    assert_eq!(m.part(2).unwrap().fs, "ext4");
    let before_other = (m.part(1).unwrap().first_lba, m.part(1).unwrap().sectors);
    let p1_hash = run("sha256sum", &[ds]); // whole disk, for the "partition 1 bytes unchanged" check below
    let _ = p1_hash;
    let p1_before = {
        let f = d.join("p1-before.img");
        region(&disk, m.part(1).unwrap(), &f, m.part(1).unwrap().bytes(512));
        run("sha256sum", &[f.to_str().unwrap()]).split_whitespace().next().unwrap().to_string()
    };
    let tool = HostTools;
    let min = find_minimum(&disk, &m, 2, &tool, None, &d).expect("the tool says a minimum");
    assert!(min >= 12 * MB && min < 300 * MB, "{min}");
    // below the minimum: refused in words
    let e = plan_shrink(&m, &ShrinkRequest { part: 2, new_bytes: 5 * MB }, Some(min)).unwrap_err();
    assert!(e.contains("needs at least"), "{e}");
    // 60 MiB is above the minimum and well below 300
    let plan = plan_shrink(&m, &ShrinkRequest { part: 2, new_bytes: 60 * MB }, Some(min)).unwrap();
    assert_eq!(plan.freed_bytes(), 240 * MB);
    eprintln!("{}", plan.text());
    // no ticked box, no change
    let mut no = confirm(&plan);
    no.ack = false;
    let h0 = run("sha256sum", &[ds]);
    assert!(apply_shrink(&disk, &m, &plan, &no, &tool, None, &d, "").is_err());
    assert_eq!(run("sha256sum", &[ds]), h0, "nothing written without the box");
    // the real thing
    let r = apply_shrink(&disk, &m, &plan, &confirm(&plan), &tool, None, &d, "").unwrap();
    assert_eq!(r.freed_bytes, 240 * MB);
    // judged by the tools, not by this code
    let after = model(&disk, total);
    assert_eq!(after.part(2).unwrap().sectors, 60 * MB / 512);
    assert_eq!(after.part(2).unwrap().first_lba, m.part(2).unwrap().first_lba);
    assert_eq!((after.part(1).unwrap().first_lba, after.part(1).unwrap().sectors), before_other);
    assert!(after.free_gaps(if table == "gpt" { TableKind::Gpt } else { TableKind::Mbr }).iter().any(|g| g.bytes(512) >= 230 * MB), "the freed space is now a gap: {:?}", after.free_gaps(TableKind::Gpt));
    let fs2 = d.join("p2-after.img");
    region(&disk, after.part(2).unwrap(), &fs2, 60 * MB);
    let fsck = Command::new("e2fsck").args(["-f", "-n", fs2.to_str().unwrap()]).output().unwrap();
    assert!(fsck.status.success(), "e2fsck after the shrink: {}", String::from_utf8_lossy(&fsck.stdout));
    if have("debugfs") {
        let out = d.join("keep.out");
        run("debugfs", &["-R", &format!("dump /keep.bin {}", out.display()), fs2.to_str().unwrap()]);
        assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(pay.join("keep.bin")).unwrap(), "the file reads back byte for byte");
    }
    let p1_after = {
        let f = d.join("p1-after.img");
        region(&disk, after.part(1).unwrap(), &f, after.part(1).unwrap().bytes(512));
        run("sha256sum", &[f.to_str().unwrap()]).split_whitespace().next().unwrap().to_string()
    };
    assert_eq!(p1_before, p1_after, "the neighbouring partition is byte for byte the same");
    if table == "gpt" {
        let v = run("sgdisk", &["-v", ds]);
        assert!(v.contains("No problems found"), "{v}");
    } else {
        let v = run("sfdisk", &["-V", ds]);
        assert!(v.contains("No errors") || v.to_lowercase().contains("ok"), "{v}");
    }
    // the plan is stale now: the table changed
    assert!(apply_shrink(&disk, &after, &plan, &confirm(&plan), &tool, None, &d, "").unwrap_err().contains("changed"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_ext4_partition_on_a_gpt_disk_is_shrunk_by_the_real_tools_and_everything_else_stays() {
    if need(&["sgdisk", "mke2fs", "mkfs.fat", "e2fsck", "resize2fs", "dd", "truncate", "sha256sum"]) {
        shrink_ext4_on("gpt");
    }
}

#[test]
fn an_ext4_partition_on_an_mbr_disk_is_shrunk_by_the_real_tools_and_everything_else_stays() {
    if need(&["sfdisk", "mke2fs", "mkfs.fat", "e2fsck", "resize2fs", "dd", "truncate", "sha256sum"]) {
        shrink_ext4_on("mbr");
    }
}

#[test]
fn an_ntfs_partition_is_shrunk_by_ntfsresize_and_still_checks_out() {
    if !need(&["sgdisk", "mkfs.ntfs", "ntfsresize", "dd", "truncate"]) {
        return;
    }
    let d = dir("ntfs");
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    let total = 300 * MB;
    run("truncate", &["-s", &total.to_string(), ds]);
    run("sgdisk", &["-o", ds]);
    run("sgdisk", &["-n", "1:0:+200M", "-t", "1:0700", ds]);
    let m0 = model(&disk, total);
    put_fs(&d, &disk, &m0, 1, "mkfs.ntfs", &["-F", "-Q", "-L", "WINDATA"], None);
    let m = model(&disk, total);
    assert_eq!(m.part(1).unwrap().fs, "ntfs");
    let tool = HostTools;
    let min = find_minimum(&disk, &m, 1, &tool, None, &d).expect("ntfsresize says a minimum");
    assert!(min < 200 * MB, "{min}");
    let want = (min.div_ceil(MB) + 20) * MB;
    let plan = plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: want }, Some(min)).unwrap();
    assert!(plan.warnings.iter().any(|w| w.contains("Windows")));
    apply_shrink(&disk, &m, &plan, &confirm(&plan), &tool, None, &d, "").unwrap();
    let after = model(&disk, total);
    assert_eq!(after.part(1).unwrap().sectors, want / 512);
    let f = d.join("after.img");
    region(&disk, after.part(1).unwrap(), &f, want);
    let info = run("ntfsresize", &["--info", "--force", "--no-progress-bar", f.to_str().unwrap()]);
    assert!(info.contains("consistency check") || info.contains("Cluster size"), "{info}");
    let _ = std::fs::remove_dir_all(&d);
}

fn synthetic() -> DiskModel {
    let part = |index: u32, first_lba: u64, sectors: u64, fs: &str| Part { index, name: format!("sdz{index}"), first_lba, sectors, kind: "Linux".into(), type_guid: None, mbr_type: None, label: String::new(), fs: fs.into(), fat_free: None, mounts: vec![], swap: false, extended: false };
    DiskModel {
        name: "sdz".into(), path: "/dev/sdz".into(), model: "x".into(), serial: "x".into(), size_bytes: 1000 * MB, sector: 512, table: Some(TableKind::Gpt), notes: vec![],
        parts: vec![part(1, 2048, 100 * MB / 512, "ext4"), part(2, 300 * MB / 512, 100 * MB / 512, "xfs"), part(3, 500 * MB / 512, 100 * MB / 512, "ntfs")],
        removable: false, usb: false, read_only: false, running_system: false, booted_from: false, raw_data: false, unusual_gpt: false, disk_guid: None, bios: Local(None), fingerprint: "fp".into(),
    }
}

#[test]
fn the_rules_refuse_what_must_not_be_shrunk_in_plain_words() {
    let ok = ShrinkRequest { part: 1, new_bytes: 50 * MB };
    assert!(plan_shrink(&synthetic(), &ok, Some(20 * MB)).is_ok());
    let err = |m: &DiskModel, r: &ShrinkRequest, min: Option<u64>| plan_shrink(m, r, min).unwrap_err();
    // mounted, swap, running system disk's mounted partition
    let mut m = synthetic();
    m.parts[0].mounts = vec!["/mnt/x".into()];
    assert!(err(&m, &ok, Some(20 * MB)).contains("mounted at /mnt/x"));
    let mut m = synthetic();
    m.parts[0].swap = true;
    assert!(err(&m, &ok, Some(20 * MB)).contains("swap"));
    // the stick IYAKKI booted from, read-only, no table
    let mut m = synthetic();
    m.booted_from = true;
    assert!(err(&m, &ok, Some(20 * MB)).contains("started from"));
    let mut m = synthetic();
    m.read_only = true;
    assert!(err(&m, &ok, Some(20 * MB)).contains("read-only"));
    let mut m = synthetic();
    m.table = None;
    assert!(err(&m, &ok, Some(20 * MB)).contains("no partition table"));
    // a file system the installer cannot shrink, a missing minimum, below the minimum, not smaller
    assert!(err(&synthetic(), &ShrinkRequest { part: 2, new_bytes: 50 * MB }, Some(20 * MB)).contains("xfs"));
    assert!(err(&synthetic(), &ok, None).contains("could not be found"));
    assert!(err(&synthetic(), &ok, Some(80 * MB)).contains("Choose at least"));
    assert!(err(&synthetic(), &ShrinkRequest { part: 1, new_bytes: 100 * MB }, Some(20 * MB)).contains("smaller"));
    assert!(err(&synthetic(), &ShrinkRequest { part: 9, new_bytes: 50 * MB }, Some(20 * MB)).contains("no partition number 9"));
    // an EFI System Partition
    let mut m = synthetic();
    m.parts[0].type_guid = Some(iyakki_storage::gpt::TYPE_ESP);
    assert!(err(&m, &ok, Some(20 * MB)).contains("EFI System Partition"));
    // MBR: a logical partition and an extended one
    let mut m = synthetic();
    m.table = Some(TableKind::Mbr);
    m.parts[0].index = 5;
    assert!(err(&m, &ShrinkRequest { part: 5, new_bytes: 50 * MB }, Some(20 * MB)).contains("logical partition"));
    let mut m = synthetic();
    m.table = Some(TableKind::Mbr);
    m.parts[0].extended = true;
    assert!(err(&m, &ok, Some(20 * MB)).contains("extended"));
    // the confirmation needs the box and the same drive, partition and size
    let plan = plan_shrink(&synthetic(), &ok, Some(20 * MB)).unwrap();
    let good = ShrinkConfirmation { drive: "sdz".into(), part: 1, new_bytes: 50 * MB, ack: true };
    assert!(check_shrink_confirmation(&plan, &good).is_ok());
    assert!(check_shrink_confirmation(&plan, &ShrinkConfirmation { ack: false, ..good.clone() }).is_err());
    assert!(check_shrink_confirmation(&plan, &ShrinkConfirmation { drive: "sda".into(), ..good.clone() }).is_err());
    assert!(check_shrink_confirmation(&plan, &ShrinkConfirmation { new_bytes: 40 * MB, ..good }).is_err());
}

#[test]
fn a_tool_that_fails_halfway_leaves_the_table_alone() {
    struct Failing;
    impl FsTool for Failing {
        fn minimum(&self, _: &str, _: &Path) -> Result<u64, String> { Ok(10 * MB) }
        fn check(&self, _: &str, _: &Path) -> Result<(), String> { Ok(()) }
        fn shrink(&self, _: &str, _: &Path, _: u64) -> Result<(), String> { Err("resize2fs failed: disk full".into()) }
    }
    if !need(&["sgdisk", "truncate", "sha256sum"]) {
        return;
    }
    let d = dir("fail");
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    run("truncate", &["-s", &(300 * MB).to_string(), ds]);
    run("sgdisk", &["-o", ds]);
    run("sgdisk", &["-n", "1:0:+200M", "-t", "1:8300", ds]);
    let m0 = model(&disk, 300 * MB);
    put_fs(&d, &disk, &m0, 1, "mke2fs", &["-q", "-t", "ext4"], None);
    let m = model(&disk, 300 * MB);
    let plan = plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 100 * MB }, Some(10 * MB)).unwrap();
    let h = run("sha256sum", &[ds]);
    let e = apply_shrink(&disk, &m, &plan, &confirm(&plan), &Failing, None, &d, "").unwrap_err();
    assert!(e.contains("disk full"), "{e}");
    assert_eq!(run("sha256sum", &[ds]), h, "a failed tool changes nothing on the disk");
    // a mounted partition is refused at write time too
    let mounts = format!("/dev/{} /mnt ext4 rw 0 0\n", plan.part_name);
    assert!(apply_shrink(&disk, &m, &plan, &confirm(&plan), &Failing, None, &d, &mounts).unwrap_err().contains("mounted"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_command_line_prints_a_plan_by_itself_and_shrinks_only_with_yes_and_the_drive_name_again() {
    if !need(&["sgdisk", "mke2fs", "e2fsck", "resize2fs", "dd", "truncate", "sha256sum"]) {
        return;
    }
    let d = dir("cli");
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    run("truncate", &["-s", &(300 * MB).to_string(), ds]);
    run("sgdisk", &["-o", ds]);
    run("sgdisk", &["-n", "1:0:+250M", "-t", "1:8300", ds]);
    let m0 = model(&disk, 300 * MB);
    put_fs(&d, &disk, &m0, 1, "mke2fs", &["-q", "-t", "ext4"], None);
    let machine = crate::plan_cli::Machine { sys: "/nonexistent".into(), dev: "/nonexistent".into(), proc_mounts: String::new(), swaps: String::new() };
    let a = |extra: &[&str]| -> Vec<String> { ["--shrink", "sdz", "--part", "1", "--to-mib", "100", "--target-file", ds].iter().chain(extra.iter()).map(|s| s.to_string()).collect() };
    let h = run("sha256sum", &[ds]);
    assert_eq!(run_cli(&a(&[]), &machine).unwrap(), 0);
    assert_eq!(run("sha256sum", &[ds]), h, "no --yes: only the plan is printed");
    let e = run_cli(&a(&["--yes"]), &machine).unwrap_err();
    assert!(e.contains("not ticked"), "{e}");
    assert_eq!(run("sha256sum", &[ds]), h, "--yes without the drive name changes nothing");
    let e = run_cli(&a(&["--yes", "--confirm", "sdb"]), &machine).unwrap_err();
    assert!(e.contains("not ticked"), "{e}");
    assert_eq!(run_cli(&a(&["--yes", "--confirm", "testfile"]), &machine).unwrap(), 0);
    assert_eq!(model(&disk, 300 * MB).part(1).unwrap().sectors, 100 * MB / 512);
    assert!(run_cli(&["--shrink".into(), "x".into(), "--part".into(), "1".into()], &machine).unwrap_err().contains("--to-mib"));
    let _ = std::fs::remove_dir_all(&d);
}

// ---- FAT32 with our own code (no external tool does the shrinking)

#[test]
fn a_fat32_partition_is_shrunk_by_our_own_code_to_just_after_its_last_file_and_fsck_and_mtools_agree() {
    if !need(&["sgdisk", "mkfs.fat", "fsck.fat", "mcopy", "mtype", "dd", "truncate"]) {
        return;
    }
    let d = dir("fat32");
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    let total = 300 * MB;
    run("truncate", &["-s", &total.to_string(), ds]);
    run("sgdisk", &["-o", ds]);
    run("sgdisk", &["-n", "1:0:+200M", "-t", "1:0700", ds]);
    let m0 = model(&disk, total);
    // a FAT32 volume with one-sector clusters (so 200 MiB has far more than 65,525 clusters), files made by mtools near the front
    let p = m0.part(1).unwrap();
    let img = d.join("fs.img");
    run("truncate", &["-s", &p.bytes(512).to_string(), img.to_str().unwrap()]);
    run("mkfs.fat", &["-F", "32", "-s", "1", "-n", "WINDATA", img.to_str().unwrap()]);
    let big: Vec<u8> = (0..3 * MB).map(|i| (i.wrapping_mul(17) >> 2) as u8).collect();
    std::fs::write(d.join("big.bin"), &big).unwrap();
    std::fs::write(d.join("note.txt"), b"keep me\n").unwrap();
    run("mcopy", &["-i", img.to_str().unwrap(), d.join("big.bin").to_str().unwrap(), d.join("note.txt").to_str().unwrap(), "::/"]);
    run("dd", &[&format!("if={}", img.display()), &format!("of={}", disk.display()), "bs=512", &format!("seek={}", p.first_lba), "conv=notrunc", "status=none"]);
    let m = model(&disk, total);
    assert_eq!(m.part(1).unwrap().fs, "fat32");
    let tool = HostTools;
    let min = find_minimum(&disk, &m, 1, &tool, None, &d).expect("our own code says a minimum");
    // 65,525 one-sector clusters is the floor of a FAT32 volume, the files are 3 MiB at the front
    assert!(min >= 33 * MB && min < 60 * MB, "{min}");
    // below it: refused in words; above it: planned, with the honest warning
    assert!(plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 20 * MB }, Some(min)).unwrap_err().contains("needs at least"));
    let plan = plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 100 * MB }, Some(min)).unwrap();
    assert!(plan.warnings.iter().any(|w| w.contains("moved to free space")), "{:?}", plan.warnings);
    assert!(plan.text().contains("own FAT32 code"));
    apply_shrink(&disk, &m, &plan, &confirm(&plan), &tool, None, &d, "").unwrap();
    // judged by the real tools
    let after = model(&disk, total);
    assert_eq!(after.part(1).unwrap().sectors, 100 * MB / 512);
    let f = d.join("after.img");
    region(&disk, after.part(1).unwrap(), &f, 100 * MB);
    let fsck = Command::new("fsck.fat").args(["-n", f.to_str().unwrap()]).output().unwrap();
    assert!(fsck.status.success(), "fsck.fat after the shrink: {}{}", String::from_utf8_lossy(&fsck.stdout), String::from_utf8_lossy(&fsck.stderr));
    assert_eq!(run("mtype", &["-i", f.to_str().unwrap(), "::/note.txt"]).trim(), "keep me");
    let out = Command::new("mtype").args(["-i", f.to_str().unwrap(), "::/big.bin"]).output().unwrap();
    assert!(out.status.success() && out.stdout == big, "the 3 MiB file reads back byte for byte");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_fat32_code_refuses_what_it_cannot_do_safely() {
    use crate::fatshrink::*;
    let d = dir("fat32-refuse");
    let img = d.join("v.img");
    // not a FAT32 volume at all, and a boot sector with the wrong numbers
    std::fs::write(&img, vec![0u8; 4096]).unwrap();
    assert!(minimum(&img).unwrap_err().contains("signature"));
    let mut b = vec![0u8; 4096];
    b[510] = 0x55;
    b[511] = 0xAA;
    std::fs::write(&img, &b).unwrap();
    assert!(check(&img).unwrap_err().contains("not those of a FAT32"));
    if need(&["mkfs.fat", "truncate"]) {
        // a small FAT32 volume (fewer than 65,525 clusters would not be FAT32 by definition, so the tool makes 2 sectors per cluster here)
        run("truncate", &["-s", &(64 * MB).to_string(), img.to_str().unwrap()]);
        run("mkfs.fat", &["-F", "32", "-s", "1", img.to_str().unwrap()]);
        check(&img).unwrap();
        let min = minimum(&img).unwrap();
        // the floor of 65,525 clusters holds even though the files are tiny
        assert!(min >= 32 * MB);
        assert!(shrink(&img, 20 * MB).unwrap_err().contains("cannot shrink below") || shrink(&img, 20 * MB).unwrap_err().contains("fewer than"));
        assert!(shrink(&img, 64 * MB).unwrap_err().contains("not smaller"));
        assert!(shrink(&img, 40 * MB + 1).unwrap_err().contains("whole number of sectors"));
        // damage: a FAT entry pointing outside the volume is found by the check
        let mut raw = std::fs::read(&img).unwrap();
        let reserved = u16::from_le_bytes([raw[14], raw[15]]) as usize * 512;
        raw[reserved + 100 * 4..reserved + 100 * 4 + 4].copy_from_slice(&0x0FFF_0000u32.to_le_bytes());
        std::fs::write(&img, &raw).unwrap();
        assert!(check(&img).unwrap_err().contains("differ") || check(&img).unwrap_err().contains("outside"));
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---- exFAT with our own code

#[test]
fn an_exfat_partition_is_shrunk_by_our_own_code_to_its_last_allocated_cluster_and_fsck_agrees() {
    if !need(&["sgdisk", "mkfs.exfat", "fsck.exfat", "dd", "truncate"]) {
        return;
    }
    let d = dir("exfat");
    let disk = d.join("disk.img");
    let ds = disk.to_str().unwrap();
    let total = 300 * MB;
    run("truncate", &["-s", &total.to_string(), ds]);
    run("sgdisk", &["-o", ds]);
    run("sgdisk", &["-n", "1:0:+200M", "-t", "1:0700", ds]);
    let m0 = model(&disk, total);
    let p = m0.part(1).unwrap();
    let img = d.join("fs.img");
    run("truncate", &["-s", &p.bytes(512).to_string(), img.to_str().unwrap()]);
    run("mkfs.exfat", &["-L", "WINDATA", img.to_str().unwrap()]);
    run("dd", &[&format!("if={}", img.display()), &format!("of={}", disk.display()), "bs=512", &format!("seek={}", p.first_lba), "conv=notrunc", "status=none"]);
    let m = model(&disk, total);
    assert_eq!(m.part(1).unwrap().fs, "exfat");
    let tool = HostTools;
    let min = find_minimum(&disk, &m, 1, &tool, None, &d).expect("our own code says a minimum");
    assert!(min < 20 * MB, "an empty volume ends just after its bitmap, up-case table and root: {min}");
    assert!(plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 1 * MB }, Some(min)).unwrap_err().contains("needs at least"));
    let plan = plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 100 * MB }, Some(min)).unwrap();
    assert!(plan.warnings.iter().any(|w| w.contains("moved to free space")), "{:?}", plan.warnings);
    assert!(plan.text().contains("own exFAT code"));
    apply_shrink(&disk, &m, &plan, &confirm(&plan), &tool, None, &d, "").unwrap();
    let after = model(&disk, total);
    assert_eq!(after.part(1).unwrap().sectors, 100 * MB / 512);
    let f = d.join("after.img");
    region(&disk, after.part(1).unwrap(), &f, 100 * MB);
    let fsck = Command::new("fsck.exfat").args(["-n", f.to_str().unwrap()]).output().unwrap();
    assert!(fsck.status.success(), "fsck.exfat after the shrink: {}{}", String::from_utf8_lossy(&fsck.stdout), String::from_utf8_lossy(&fsck.stderr));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_exfat_code_moves_its_minimum_with_the_bitmap_and_refuses_what_it_cannot_do_safely() {
    use crate::exfatshrink::*;
    let d = dir("exfat-refuse");
    let img = d.join("v.img");
    std::fs::write(&img, vec![0u8; 4096]).unwrap();
    assert!(minimum(&img).unwrap_err().contains("signature"));
    if need(&["mkfs.exfat", "truncate"]) {
        run("truncate", &["-s", &(64 * MB).to_string(), img.to_str().unwrap()]);
        run("mkfs.exfat", &[img.to_str().unwrap()]);
        check(&img).unwrap();
        let min = minimum(&img).unwrap();
        assert!(shrink(&img, 64 * MB).unwrap_err().contains("not smaller"));
        assert!(shrink(&img, 40 * MB + 1).unwrap_err().contains("whole number of sectors"));
        assert!(shrink(&img, min - 512).unwrap_err().contains("cannot shrink below"));
        // a cluster far out marked used (as a file there would) moves the minimum past it and the check still passes
        let raw = std::fs::read(&img).unwrap();
        let heap = u32::from_le_bytes([raw[88], raw[89], raw[90], raw[91]]) as usize;
        let spc = 1usize << raw[109];
        let bps = 1usize << raw[108];
        // find the bitmap through the root directory: its first cluster is in the 0x81 entry
        let root = u32::from_le_bytes([raw[96], raw[97], raw[98], raw[99]]) as usize;
        let rd = (heap + (root - 2) * spc) * bps;
        let e = (0..spc * bps / 32).map(|i| rd + i * 32).find(|o| raw[*o] == 0x81).expect("bitmap entry");
        let first = u32::from_le_bytes([raw[e + 20], raw[e + 21], raw[e + 22], raw[e + 23]]) as usize;
        let bm = (heap + (first - 2) * spc) * bps;
        let mut raw = raw;
        let far = 5000usize; // cluster index 5000
        raw[bm + far / 8] |= 1 << (far % 8);
        std::fs::write(&img, &raw).unwrap();
        let min2 = minimum(&img).unwrap();
        assert_eq!(min2 as usize, (heap + (far + 2 - 1) * spc) * bps);
        assert!(min2 > min);
        // damage: a changed byte in the boot region is found by the checksum
        raw[20] ^= 0xFF;
        std::fs::write(&img, &raw).unwrap();
        assert!(check(&img).unwrap_err().contains("checksum"));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_btrfs_partition_is_planned_with_the_mounted_resize_and_xfs_is_told_it_cannot_shrink() {
    let mut m = synthetic();
    m.parts[0].fs = "btrfs".into();
    let plan = plan_shrink(&m, &ShrinkRequest { part: 1, new_bytes: 50 * MB }, Some(20 * MB)).unwrap();
    assert!(plan.steps.iter().any(|s| s.contains("btrfs (mounted for the resize)")), "{:?}", plan.steps);
    let x = plan_shrink(&synthetic(), &ShrinkRequest { part: 2, new_bytes: 50 * MB }, Some(20 * MB)).unwrap_err();
    assert!(x.contains("xfs cannot be made smaller by any tool"), "{x}");
}

/// A FAT32 volume of `mib` MiB with one-sector clusters in a file.
fn fat32_image(d: &std::path::Path, name: &str, mib: u64) -> std::path::PathBuf {
    let img = d.join(name);
    run("truncate", &["-s", &(mib * MB).to_string(), img.to_str().unwrap()]);
    run("mkfs.fat", &["-F", "32", "-s", "1", "-n", "MOVEDATA", img.to_str().unwrap()]);
    img
}

fn mt(img: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new(args[0]).arg("-i").arg(img).args(&args[1..]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn files_beyond_the_new_end_are_moved_below_it_and_every_file_reads_back_and_fsck_agrees() {
    if !need(&["mkfs.fat", "fsck.fat", "mcopy", "mdel", "mmd", "mtype", "truncate"]) {
        return;
    }
    use crate::fatshrink::*;
    let d = dir("fat32-move");
    let img = fat32_image(&d, "v.img", 150);
    let i = img.to_str().unwrap();
    // a filler that is deleted again pushes everything written after it far out (one-sector clusters: cluster number ~ MiB * 2048)
    let filler = vec![1u8; 80 * 1024 * 1024];
    std::fs::write(d.join("filler.bin"), &filler).unwrap();
    run("mcopy", &["-i", i, d.join("filler.bin").to_str().unwrap(), "::/"]);
    run("mmd", &["-i", i, "::/docs", "::/docs/deep", "::/a long directory name with spaces"]);
    let big: Vec<u8> = (0..3 * MB).map(|k| (k.wrapping_mul(31) >> 3) as u8).collect();
    std::fs::write(d.join("big.bin"), &big).unwrap();
    std::fs::write(d.join("Long File Name Here.txt"), b"long name text\n").unwrap();
    std::fs::write(d.join("small.txt"), b"small\n").unwrap();
    run("mcopy", &["-i", i, d.join("big.bin").to_str().unwrap(), "::/docs/deep/big.bin"]);
    run("mcopy", &["-i", i, d.join("Long File Name Here.txt").to_str().unwrap(), "::/docs/"]);
    run("mcopy", &["-i", i, d.join("small.txt").to_str().unwrap(), "::/a long directory name with spaces/small.txt", ]);
    run("mcopy", &["-i", i, d.join("small.txt").to_str().unwrap(), "::/top.txt"]);
    run("mdel", &["-i", i, "::/filler.bin"]);
    // the files reach far out, but only a few MiB of them exist: with moving the volume can be much smaller
    let (with_moving, without) = (minimum(&img).unwrap(), minimum_without_moving(&img).unwrap());
    assert!(without > 70 * MB, "without moving it is held up by the far files: {without}");
    assert!(with_moving < 40 * MB, "with moving only what the files need (and the FAT32 floor) counts: {with_moving}");
    check(&img).unwrap();
    shrink(&img, 40 * MB).unwrap();
    // the file system is smaller and judged by the real checker
    let fsck = Command::new("fsck.fat").args(["-n", i]).output().unwrap();
    assert!(fsck.status.success(), "fsck.fat after moving:\n{}{}", String::from_utf8_lossy(&fsck.stdout), String::from_utf8_lossy(&fsck.stderr));
    let out = Command::new("mtype").args(["-i", i, "::/docs/deep/big.bin"]).output().unwrap();
    assert!(out.status.success() && out.stdout == big, "the 3 MiB file in the nested folder reads back byte for byte");
    assert_eq!(mt(&img, &["mtype", "::/docs/Long File Name Here.txt"]), "long name text\n");
    assert_eq!(mt(&img, &["mtype", "::/a long directory name with spaces/small.txt"]), "small\n");
    assert_eq!(mt(&img, &["mtype", "::/top.txt"]), "small\n");
    assert!(mt(&img, &["mdir", "-/", "::/"]).contains("deep"), "the tree is still listed");
    // after the move nothing lies beyond the new end any more
    let (g_hi, used) = (minimum_without_moving(&img).unwrap(), crate::fatreloc::used_clusters(&img).unwrap());
    assert!(g_hi <= 40 * MB + 512 && used > 6000, "{g_hi} {used}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_root_directory_beyond_the_new_end_is_moved_and_named_in_both_boot_sectors() {
    if !need(&["mkfs.fat", "fsck.fat", "mcopy", "mmd", "mtype", "truncate"]) {
        return;
    }
    use std::os::unix::fs::FileExt;
    let d = dir("fat32-root-move");
    let img = fat32_image(&d, "r.img", 120);
    let i = img.to_str().unwrap();
    std::fs::write(d.join("a.txt"), b"alpha\n").unwrap();
    run("mmd", &["-i", i, "::/sub"]);
    run("mcopy", &["-i", i, d.join("a.txt").to_str().unwrap(), "::/sub/a.txt"]);
    run("mcopy", &["-i", i, d.join("a.txt").to_str().unwrap(), "::/root.txt"]);
    // put the root directory far out by hand: copy its cluster to a free cluster near the end of the volume, chain it, free cluster 2,
    // name it in the boot sector and its backup (a valid FAT32 state that mkfs never makes)
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&img).unwrap();
    let mut b = vec![0u8; 512];
    f.read_exact_at(&mut b, 0).unwrap();
    let (reserved, nfats, fatsz, spc) = (u16::from_le_bytes([b[14], b[15]]) as u64, b[16] as u64, u32::from_le_bytes([b[36], b[37], b[38], b[39]]) as u64, b[13] as u64);
    let backup = u16::from_le_bytes([b[50], b[51]]) as u64;
    let data_start = reserved + nfats * fatsz;
    let far: u32 = 200_000;
    let mut cl = vec![0u8; (spc * 512) as usize];
    f.read_exact_at(&mut cl, data_start * 512).unwrap();
    f.write_all_at(&cl, (data_start + (far as u64 - 2) * spc) * 512).unwrap();
    for copy in 0..nfats {
        let fat_at = (reserved + copy * fatsz) * 512;
        f.write_all_at(&0x0FFF_FFFFu32.to_le_bytes(), fat_at + far as u64 * 4).unwrap();
        f.write_all_at(&0u32.to_le_bytes(), fat_at + 2 * 4).unwrap();
    }
    f.write_all_at(&far.to_le_bytes(), 44).unwrap();
    f.write_all_at(&far.to_le_bytes(), backup * 512 + 44).unwrap();
    drop(f);
    assert_eq!(mt(&img, &["mtype", "::/root.txt"]), "alpha\n", "the hand-made layout is a valid volume");
    crate::fatshrink::shrink(&img, 40 * MB).unwrap();
    let f = std::fs::File::open(&img).unwrap();
    for at in [0u64, backup * 512] {
        let mut r = [0u8; 4];
        f.read_exact_at(&mut r, at + 44).unwrap();
        assert!(u32::from_le_bytes(r) < 41 * 2048 + 2, "the boot sector at {at} names a root below the new end: {}", u32::from_le_bytes(r));
    }
    let fsck = Command::new("fsck.fat").args(["-n", i]).output().unwrap();
    assert!(fsck.status.success(), "fsck.fat:\n{}{}", String::from_utf8_lossy(&fsck.stdout), String::from_utf8_lossy(&fsck.stderr));
    assert_eq!(mt(&img, &["mtype", "::/sub/a.txt"]), "alpha\n");
    assert_eq!(mt(&img, &["mtype", "::/root.txt"]), "alpha\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn compacting_refuses_a_limit_the_files_do_not_fit_under_before_writing_anything() {
    if !need(&["mkfs.fat", "mcopy", "truncate"]) {
        return;
    }
    let d = dir("fat32-nofit");
    let img = fat32_image(&d, "n.img", 100);
    let big = vec![5u8; 6 * 1024 * 1024];
    std::fs::write(d.join("big.bin"), &big).unwrap();
    run("mcopy", &["-i", img.to_str().unwrap(), d.join("big.bin").to_str().unwrap(), "::/"]);
    let before = std::fs::read(&img).unwrap();
    let e = crate::fatreloc::compact(&img, 1000).unwrap_err();
    assert!(e.contains("nothing was moved"), "{e}");
    assert_eq!(std::fs::read(&img).unwrap(), before, "refused means untouched");
    assert_eq!(crate::fatreloc::compact(&img, 100_000).unwrap(), crate::fatreloc::Moved { clusters: 0, chains: 0 }, "everything is below the limit: nothing to do");
    let _ = std::fs::remove_dir_all(&d);
}
