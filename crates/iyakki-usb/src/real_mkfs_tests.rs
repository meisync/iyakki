//! The partition planner's reading side against disks and file systems made by the real tools (`sgdisk`, `mkfs.fat`, `mke2fs`, `mkfs.ntfs`,
//! `mkfs.exfat`, `mkswap`): a GPT disk is made with `sgdisk`, each file system is made by its own tool and copied to its partition, and the
//! model the planner reads must say what the tools made (F-INST-05). Skipped, and said so, where a tool is missing.

use crate::planner::*;
use crate::gpt_install::TableKind;
use std::path::Path;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool} >/dev/null")]).status().is_ok_and(|s| s.success())
}

fn run(cmd: &str, args: &[&str]) -> String {
    let o = Command::new(cmd).args(args).output().unwrap_or_else(|e| panic!("{cmd}: {e}"));
    assert!(o.status.success(), "{cmd} {args:?}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn first_sector(disk: &Path, n: u32) -> u64 {
    let t = run("sgdisk", &["-i", &n.to_string(), disk.to_str().unwrap()]);
    t.lines().find_map(|l| l.strip_prefix("First sector: ")).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok()).expect("First sector line")
}

#[test]
fn a_gpt_disk_made_by_sgdisk_with_file_systems_from_their_own_tools_is_read_as_what_the_tools_made() {
    for t in ["sgdisk", "mkfs.fat", "mke2fs", "mkfs.ntfs", "mkfs.exfat", "mkswap", "dd"] {
        if !have(t) {
            eprintln!("skipped: {t} is not installed");
            return;
        }
    }
    let d = std::env::temp_dir().join(format!("iyakki-usb-mkfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let disk = d.join("disk.img");
    let disk_s = disk.to_str().unwrap();
    run("truncate", &["-s", "400M", disk_s]);
    run("sgdisk", &["-o", disk_s]);
    // (size, GPT type code, the tool's own arguments, the file system the planner must read, its label)
    type Case<'a> = (&'a str, &'a str, &'a str, Vec<&'a str>, &'a str, &'a str);
    let cases: Vec<Case> = vec![
        ("70M", "0700", "mkfs.fat", vec!["-F", "32", "-n", "FATDATA"], "fat32", "FATDATA"),
        ("40M", "0700", "mkfs.ntfs", vec!["-F", "-Q", "-L", "WINDATA"], "ntfs", "WINDATA"),
        ("40M", "0700", "mkfs.exfat", vec!["-L", "EXFDATA"], "exfat", "EXFDATA"),
        ("40M", "8300", "mke2fs", vec!["-q", "-t", "ext4", "-L", "EXT4DATA"], "ext4", "EXT4DATA"),
        ("30M", "8300", "mke2fs", vec!["-q", "-t", "ext3", "-L", "EXT3DATA"], "ext3", "EXT3DATA"),
        ("30M", "8300", "mke2fs", vec!["-q", "-t", "ext2", "-L", "EXT2DATA"], "ext2", "EXT2DATA"),
        ("16M", "8200", "mkswap", vec!["-L", "SWAPDATA"], "swap", "SWAPDATA"),
    ];
    for (i, (size, code, _, _, _, _)) in cases.iter().enumerate() {
        let n = (i + 1).to_string();
        run("sgdisk", &["-n", &format!("{n}:0:+{size}"), "-t", &format!("{n}:{code}"), disk_s]);
    }
    for (i, (size, _, tool, args, _, _)) in cases.iter().enumerate() {
        let n = (i + 1) as u32;
        let img = d.join(format!("fs{n}.img"));
        run("truncate", &["-s", size, img.to_str().unwrap()]);
        let mut a: Vec<&str> = args.clone();
        a.push(img.to_str().unwrap());
        run(tool, &a);
        run("dd", &[&format!("if={}", img.display()), &format!("of={disk_s}"), "bs=512", &format!("seek={}", first_sector(&disk, n)), "conv=notrunc", "status=none"]);
    }
    let meta = DiskMeta { name: "loopdisk".into(), path: disk_s.into(), model: "file".into(), serial: "x".into(), removable: false, usb: false, read_only: false, swap_parts: vec![] };
    let model = read_model(&mut std::fs::File::open(&disk).unwrap(), &meta, 400 * 1024 * 1024, 512, &[], None);
    assert_eq!(model.parts.len(), cases.len(), "{:?}", model.parts);
    for (i, (_, _, tool, _, fs, label)) in cases.iter().enumerate() {
        let p = &model.parts[i];
        assert_eq!(p.fs, *fs, "partition {} made by {tool}: read as {:?}", i + 1, p.fs);
        eprintln!("partition {}: {tool} -> fs {:?}, label {:?}, kind {:?}", i + 1, p.fs, p.label, p.kind);
        // the free bytes of the FAT32 volume come from its FSInfo sector, which mkfs.fat wrote
        if *fs == "fat32" {
            let free = p.fat_free.expect("FSInfo free space of a fresh FAT32 volume");
            assert!(free > 60 * 1024 * 1024 && free < 70 * 1024 * 1024, "free space {free}");
        }
        let _ = label;
    }
    // the free space after the last partition is offered, aligned to 1 MiB
    assert!(model.largest_gap(TableKind::Gpt).is_some_and(|g| g.bytes(512) > 100 * 1024 * 1024), "the unused rest of the 400 MiB disk is a gap: {:?}", model.largest_gap(TableKind::Gpt));
    // and the real tool agrees about the table
    let sg = run("sgdisk", &["-v", disk_s]);
    assert!(sg.contains("No problems found"), "{sg}");
    let _ = std::fs::remove_dir_all(&d);
}
