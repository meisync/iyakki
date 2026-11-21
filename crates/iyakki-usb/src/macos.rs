//! macOS: `diskutil` output parsers (pure, tested everywhere) and the calls that run it (macOS only).

use crate::drives::{mounts_of, Drive};

/// "31.0 GB" / "512.1 KB" / "1.0 TB" (decimal units, as diskutil prints them) to bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let mut it = s.trim().trim_start_matches('*').split_whitespace();
    let n: f64 = it.next()?.replace(',', ".").parse().ok()?;
    let mult = match it.next()? {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// `diskutil list external physical` -> (identifier like `disk4`, whole-disk size in bytes).
pub fn parse_diskutil_list(text: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("/dev/") {
            cur = rest.split_whitespace().next().map(String::from);
            continue;
        }
        // "   0:      GUID_partition_scheme                        *31.0 GB    disk4"
        let t = l.trim_start();
        if let (Some(id), true) = (&cur, t.starts_with("0:")) {
            let size = t.split_whitespace().position(|w| w.starts_with('*')).and_then(|i| {
                let w: Vec<&str> = t.split_whitespace().collect();
                parse_size(&format!("{} {}", w[i], w.get(i + 1)?))
            });
            if let Some(sz) = size {
                out.push((id.clone(), sz));
            }
            cur = None;
        }
    }
    out
}

/// `diskutil info diskN` -> (media name, removable, writable).
pub fn parse_diskutil_info(text: &str) -> (String, bool, bool) {
    let get = |key: &str| text.lines().find_map(|l| l.trim_start().strip_prefix(key).map(|v| v.trim().trim_start_matches(':').trim().to_string())).unwrap_or_default();
    let model = get("Device / Media Name:");
    let removable = get("Removable Media:").to_lowercase().starts_with("removable") || get("Protocol:").eq_ignore_ascii_case("USB");
    let writable = !get("Media Read-Only:").eq_ignore_ascii_case("yes");
    (if model.is_empty() { "disk".into() } else { model }, removable, writable)
}

/// BSD `mount` output: "/dev/disk4s2 on /Volumes/USB (msdos, local, nodev, ...)" -> (device, mount point).
pub fn parse_bsd_mount(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| {
        let (dev, rest) = l.split_once(" on ")?;
        let mp = rest.rsplit_once(" (").map_or(rest, |(m, _)| m);
        dev.starts_with("/dev/").then(|| (dev.to_string(), mp.to_string()))
    }).collect()
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let o = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).into_owned()) } else { Err(format!("{cmd} failed: {}", String::from_utf8_lossy(&o.stderr).trim())) }
}

/// External physical disks (the internal drive is never listed).
#[cfg(target_os = "macos")]
pub fn list_drives() -> Result<Vec<Drive>, String> {
    let list = run("diskutil", &["list", "external", "physical"])?;
    let mounts = parse_bsd_mount(&run("mount", &[]).unwrap_or_default());
    let mut out = Vec::new();
    for (id, size) in parse_diskutil_list(&list) {
        let (model, removable, writable) = parse_diskutil_info(&run("diskutil", &["info", &id]).unwrap_or_default());
        out.push(Drive { path: format!("/dev/r{id}"), mounts: mounts_of(&id, &mounts), name: id, model, size_bytes: size, removable, usb: true, read_only: !writable, swap: false });
    }
    Ok(out)
}

/// Unmount every volume of the disk so it can be written.
#[cfg(target_os = "macos")]
pub fn unmount_disk(name: &str) -> Result<(), String> {
    run("diskutil", &["unmountDisk", &format!("/dev/{name}")]).map(|_| ())
}


/// The APFS containers that live on partitions of the disk `diskutil list diskN` describes: the synthesized disks (`disk5`) whose volumes
/// are mounted from a physical store on this disk. They must be unmounted before anything under them is written, or the Mac would keep
/// writing into a partition the installer has replaced. A line looks like `2:  Apple_APFS Container disk5   999.8 GB   disk4s2`.
pub fn apfs_containers(list_text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in list_text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if let Some(i) = w.iter().position(|x| *x == "Apple_APFS") {
            if w.get(i + 1) == Some(&"Container") {
                if let Some(id) = w.get(i + 2).filter(|id| id.starts_with("disk") && id[4..].bytes().all(|b| b.is_ascii_digit()) && id.len() > 4) {
                    if !out.iter().any(|o| o == id) {
                        out.push((*id).to_string());
                    }
                }
            }
        }
    }
    out
}

/// The buffered block device of a raw one (`/dev/rdisk4` -> `/dev/disk4`). The raw device takes only whole-sector transfers; the
/// planner's writes (a FAT entry, a directory record) are smaller, so the planner writes through the block device, which takes any
/// offset and length. Anything else is returned unchanged.
pub fn block_path(path: &str) -> String {
    match path.strip_prefix("/dev/rdisk") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit() || b == b's') => format!("/dev/disk{n}"),
        _ => path.to_string(),
    }
}

/// Free the disk for the planner's writer: unmount the volumes of every APFS container that has a physical store on it, then the disk
/// itself (`diskutil unmountDisk`, never forced: a busy volume stops the install with a plain message and nothing is written).
pub fn unmount_for_planner(run: &dyn Fn(&str, &[&str]) -> Result<String, String>, disk: &str) -> Result<(), String> {
    let id = disk.trim_start_matches("/dev/").trim_start_matches('r');
    let listing = run("diskutil", &["list", id]).unwrap_or_default();
    for c in apfs_containers(&listing).iter().filter(|c| c.as_str() != id) {
        run("diskutil", &["unmountDisk", &format!("/dev/{c}")]).map_err(|e| format!("could not unmount the APFS volumes of {c} ({e}): close anything using them and try again"))?;
    }
    run("diskutil", &["unmountDisk", &format!("/dev/{id}")]).map(|_| ()).map_err(|e| format!("could not unmount {id} ({e}): close anything using it and try again"))
}

/// What `diskutil info diskN` says about the geometry: the logical sector size ("Device Block Size: 4096 Bytes") and the exact size in
/// bytes (the number in brackets after "Disk Size:"). Both must be there; a disk that does not say is not guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub sector: u32,
    pub bytes: u64,
}

pub fn parse_geometry(text: &str) -> Option<Geometry> {
    let line = |key: &str| text.lines().find_map(|l| l.trim_start().strip_prefix(key).map(|v| v.trim().to_string()));
    let sector: u32 = line("Device Block Size:")?.split_whitespace().next()?.parse().ok()?;
    let bytes: u64 = line("Disk Size:")?.split('(').nth(1)?.split_whitespace().next()?.parse().ok()?;
    (matches!(sector, 512 | 4096) && bytes >= u64::from(sector) * 64).then_some(Geometry { sector, bytes })
}

/// "Device Location: Internal" (the machine's own disks) or "Virtual: Yes" (a disk image or an APFS container): never a target.
pub fn is_internal_or_virtual(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        (t.starts_with("Device Location:") && t.trim_end().ends_with("Internal")) || (t.starts_with("Virtual:") && t.trim_end().ends_with("Yes"))
    })
}

/// The disks of a Mac as the planner reads them (the disk reader side of F-INST-12): `run` answers a command line (`diskutil`, `mount`),
/// `open` opens the raw device for reading. Only external physical disks are looked at; one whose geometry is not stated, that is
/// internal or virtual, or cannot be read (it needs administrator rights) is skipped with a plain note.
pub fn discover_with(
    run: &dyn Fn(&str, &[&str]) -> Result<String, String>,
    open: &dyn Fn(&str) -> std::io::Result<std::fs::File>,
    boot_media: Option<&str>,
    skipped: &mut Vec<String>,
) -> (Vec<crate::planner::DiskModel>, Vec<crate::install::DiskInfo>) {
    let list = match run("diskutil", &["list", "external", "physical"]) {
        Ok(l) => l,
        Err(e) => {
            skipped.push(format!("diskutil: {e}"));
            return (vec![], vec![]);
        }
    };
    let mounts = parse_bsd_mount(&run("mount", &[]).unwrap_or_default());
    let (mut models, mut infos) = (Vec::new(), Vec::new());
    for (id, _) in parse_diskutil_list(&list) {
        let info = run("diskutil", &["info", &id]).unwrap_or_default();
        if is_internal_or_virtual(&info) {
            skipped.push(format!("{id}: an internal or virtual disk is never a target"));
            continue;
        }
        let Some(g) = parse_geometry(&info) else {
            skipped.push(format!("{id}: diskutil did not state its sector size and exact size"));
            continue;
        };
        let (model, removable, writable) = parse_diskutil_info(&info);
        let path = format!("/dev/r{id}");
        let drive = Drive { path: path.clone(), mounts: mounts_of(&id, &mounts), name: id.clone(), model: model.clone(), size_bytes: g.bytes, removable, usb: true, read_only: !writable, swap: false };
        let meta = crate::planner::DiskMeta { name: id.clone(), path: path.clone(), model, serial: String::new(), removable, usb: true, read_only: !writable, swap_parts: vec![] };
        match open(&path) {
            Ok(mut f) => {
                let bsd: Vec<(String, String)> = mounts.iter().map(|(d, m)| (d.trim_start_matches("/dev/").to_string(), m.clone())).collect();
                models.push(crate::planner::read_model(&mut f, &meta, g.bytes, g.sector, &bsd, boot_media));
                infos.push(crate::install::DiskInfo { drive, serial: String::new(), logical_sector: g.sector, partitions: vec![], gpt: false, table: String::new() });
            }
            Err(e) => skipped.push(format!("{path}: cannot read ({e}); run as administrator")),
        }
    }
    (models, infos)
}

/// The real thing: `diskutil` and `mount` of this Mac, the raw devices opened read-only.
#[cfg(target_os = "macos")]
pub fn discover_real(boot_media: Option<&str>, skipped: &mut Vec<String>) -> (Vec<crate::planner::DiskModel>, Vec<crate::install::DiskInfo>) {
    discover_with(&run, &|p| std::fs::File::open(p), boot_media, skipped)
}

#[cfg(not(target_os = "macos"))]
pub fn list_drives() -> Result<Vec<Drive>, String> {
    let _ = mounts_of;
    Err("macOS only".into())
}

#[cfg(not(target_os = "macos"))]
pub fn unmount_disk(_name: &str) -> Result<(), String> {
    Err("macOS only".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn apfs_containers_of_a_disk_are_found_and_unmounted_before_the_disk() {
        let list = "/dev/disk4 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *1.0 TB     disk4\n   1:                        EFI EFI                     209.7 MB   disk4s1\n   2:                 Apple_APFS Container disk5         999.8 GB   disk4s2\n   3:       Microsoft Basic Data DATA                    100.0 GB   disk4s3\n";
        assert_eq!(super::apfs_containers(list), ["disk5"]);
        assert!(super::apfs_containers("   2: Apple_APFS Container diskX 1 GB x\n   3: Apple_APFS Container disk 1 GB x\n").is_empty());
        let log = std::cell::RefCell::new(Vec::<String>::new());
        let run = |c: &str, a: &[&str]| -> Result<String, String> {
            log.borrow_mut().push(format!("{c} {}", a.join(" ")));
            if a[0] == "list" { Ok(list.to_string()) } else { Ok(String::new()) }
        };
        super::unmount_for_planner(&run, "/dev/rdisk4").unwrap();
        assert_eq!(*log.borrow(), ["diskutil list disk4", "diskutil unmountDisk /dev/disk5", "diskutil unmountDisk /dev/disk4"]);
        // a busy container stops everything before the disk is touched
        let log2 = std::cell::RefCell::new(Vec::<String>::new());
        let busy = |c: &str, a: &[&str]| -> Result<String, String> {
            log2.borrow_mut().push(format!("{c} {}", a.join(" ")));
            match a[0] { "list" => Ok(list.to_string()), _ if a[1] == "/dev/disk5" => Err("busy".into()), _ => Ok(String::new()) }
        };
        let e = super::unmount_for_planner(&busy, "disk4").unwrap_err();
        assert!(e.contains("APFS") && e.contains("busy") && log2.borrow().len() == 2, "{e}");
    }

    #[test]
    fn the_planner_writes_through_the_block_device_not_the_raw_one() {
        assert_eq!(super::block_path("/dev/rdisk4"), "/dev/disk4");
        assert_eq!(super::block_path("/dev/rdisk10s2"), "/dev/disk10s2");
        assert_eq!(super::block_path("/dev/sdb"), "/dev/sdb");
        assert_eq!(super::block_path("/dev/rdiskx"), "/dev/rdiskx");
        assert_eq!(super::block_path("/dev/rdisk"), "/dev/rdisk");
    }

    use super::*;

    const LIST: &str = "/dev/disk4 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *31.0 GB    disk4\n   1:                        EFI EFI                     209.7 MB   disk4s1\n   2:       Microsoft Basic Data USB STICK               30.8 GB    disk4s2\n\n/dev/disk5 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:     FDisk_partition_scheme                        *1.0 TB     disk5\n   1:               Windows_NTFS Backup                  1.0 TB     disk5s1\n";

    const INFO4: &str = "   Device Identifier:         disk4\n   Device Node:               /dev/disk4\n   Whole:                     Yes\n   Part of Whole:             disk4\n   Device / Media Name:       SanDisk Cruzer\n\n   Volume Name:               Not applicable (no file system)\n\n   Disk Size:                 31.0 GB (31004295168 Bytes) (exactly 60555264 512-Byte-Units)\n   Device Block Size:         512 Bytes\n\n   Removable Media:           Removable\n   Media Read-Only:           No\n   Protocol:                  USB\n   Device Location:           External\n";

    #[test]
    fn geometry_is_read_exactly_and_never_guessed() {
        assert_eq!(parse_geometry(INFO4), Some(Geometry { sector: 512, bytes: 31_004_295_168 }));
        let k = INFO4.replace("512 Bytes", "4096 Bytes");
        assert_eq!(parse_geometry(&k).unwrap().sector, 4096);
        assert_eq!(parse_geometry(&INFO4.replace("Device Block Size:         512 Bytes\n", "")), None);
        assert_eq!(parse_geometry(&INFO4.replace("(31004295168 Bytes)", "")), None);
        assert_eq!(parse_geometry(&INFO4.replace("512 Bytes", "1024 Bytes")), None);
        assert_eq!(parse_geometry(""), None);
        assert!(!is_internal_or_virtual(INFO4));
        assert!(is_internal_or_virtual(&INFO4.replace("External", "Internal")));
        assert!(is_internal_or_virtual("   Virtual:                   Yes\n"));
    }

    #[test]
    fn the_planner_reads_the_external_disks_of_a_mac_and_skips_what_it_must() {
        let dir = std::env::temp_dir().join(format!("iyakki-macdisc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("rdisk4");
        std::fs::File::create(&img).unwrap().set_len(31_004_295_168 / 1024).unwrap();
        let list = "/dev/disk4 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *31.0 GB    disk4\n\n/dev/disk6 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *500.1 GB   disk6\n\n/dev/disk7 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *8.0 GB     disk7\n";
        let run = |c: &str, a: &[&str]| -> Result<String, String> {
            Ok(match (c, a) {
                ("diskutil", ["list", ..]) => list.to_string(),
                ("diskutil", ["info", "disk4"]) => INFO4.to_string(),
                ("diskutil", ["info", "disk6"]) => INFO4.replace("External", "Internal"),
                ("diskutil", ["info", "disk7"]) => INFO4.replace("Device Block Size:         512 Bytes\n", ""),
                ("mount", _) => "/dev/disk4s2 on /Volumes/USB (msdos, local)\n".to_string(),
                _ => return Err("unexpected".into()),
            })
        };
        let open = |p: &str| std::fs::File::open(dir.join(p.trim_start_matches("/dev/")));
        let mut skipped = Vec::new();
        let (models, infos) = discover_with(&run, &open, None, &mut skipped);
        // the stand-in file is smaller than the stated size: the planner still reads (an empty disk) and keeps the stated geometry
        assert_eq!((models.len(), infos.len()), (1, 1));
        assert_eq!((models[0].name.as_str(), models[0].sector, models[0].size_bytes), ("disk4", 512, 31_004_295_168));
        assert_eq!(infos[0].drive.path, "/dev/rdisk4");
        assert_eq!(infos[0].drive.mounts.len(), 1);
        assert!(skipped.iter().any(|s| s.starts_with("disk6") && s.contains("never a target")), "{skipped:?}");
        assert!(skipped.iter().any(|s| s.starts_with("disk7") && s.contains("sector size")), "{skipped:?}");
        // diskutil failing is said, not hidden
        let mut sk = Vec::new();
        assert!(discover_with(&|_, _| Err("boom".into()), &open, None, &mut sk).0.is_empty() && sk[0].contains("boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(parse_size("31.0 GB"), Some(31_000_000_000));
        assert_eq!(parse_size("*1.0 TB"), Some(1_000_000_000_000));
        assert_eq!(parse_size("209.7 MB"), Some(209_700_000));
        assert_eq!(parse_size("512 B"), Some(512));
        assert_eq!(parse_size("abc"), None);
        assert_eq!(parse_size("5 PB"), None);
    }

    #[test]
    fn diskutil_list_gives_each_external_disk_and_its_size() {
        assert_eq!(parse_diskutil_list(LIST), vec![("disk4".to_string(), 31_000_000_000), ("disk5".to_string(), 1_000_000_000_000)]);
        assert!(parse_diskutil_list("").is_empty());
        assert!(parse_diskutil_list("nonsense\nmore").is_empty());
    }

    /// The parsers on what this Mac's real `diskutil` prints (read only: `list physical` and `info`, nothing is written or unmounted).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_parsers_read_the_output_of_the_real_diskutil_and_the_internal_drive_is_never_a_target() {
        let out = |args: &[&str]| String::from_utf8_lossy(&std::process::Command::new("diskutil").args(args).output().expect("diskutil").stdout).into_owned();
        let all = out(&["list", "physical"]);
        let disks = parse_diskutil_list(&all);
        assert!(!disks.is_empty(), "the real listing has at least the internal disk: {all}");
        for (id, size) in &disks {
            assert!(id.starts_with("disk") && *size > 1_000_000, "{id} {size}");
            // the size in the listing agrees with the exact byte count that `info` prints in brackets
            let info = out(&["info", id]);
            let exact: u64 = info.lines().find(|l| l.trim_start().starts_with("Disk Size:")).and_then(|l| l.split('(').nth(1)).and_then(|t| t.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap_or(0);
            assert!(exact > 0 && (*size as f64 - exact as f64).abs() / (exact as f64) < 0.06, "{id}: listing {size}, info {exact}\n{info}");
            let (model, removable, writable) = parse_diskutil_info(&info);
            assert!(!model.is_empty() && model != "disk", "a real media name for {id}: {info}");
            let _ = (removable, writable);
        }
        // the machine's own disk is internal and fixed: not removable, and not in the list of drives the writer may offer
        let internal: Vec<&(String, u64)> = disks.iter().filter(|(id, _)| out(&["info", id]).lines().any(|l| l.trim_start().starts_with("Device Location:") && l.trim_end().ends_with("Internal"))).collect();
        assert!(!internal.is_empty(), "the internal drive is among the physical disks");
        for (id, _) in &internal {
            let (_, removable, writable) = parse_diskutil_info(&out(&["info", id]));
            assert!(!removable && writable);
        }
        let offered = list_drives().expect("diskutil answers");
        for d in &offered {
            assert!(d.path.starts_with("/dev/rdisk") && d.usb && d.size_bytes > 0, "{d:?}");
            assert!(!internal.iter().any(|(id, _)| *id == d.name), "the internal drive {} must not be offered: {d:?}", d.name);
        }
        // the mount table is read the same way
        let mounts = parse_bsd_mount(&String::from_utf8_lossy(&std::process::Command::new("mount").output().unwrap().stdout));
        assert!(mounts.iter().any(|(_, mp)| mp == "/"), "the root volume is mounted: {mounts:?}");
    }

    #[test]
    fn diskutil_info_gives_name_removable_and_writable() {
        let info = "   Device Identifier:         disk4\n   Device / Media Name:       SanDisk Cruzer\n   Removable Media:           Removable\n   Media Read-Only:           No\n   Protocol:                  USB\n";
        assert_eq!(parse_diskutil_info(info), ("SanDisk Cruzer".to_string(), true, true));
        let ro = "   Device / Media Name:       Locked\n   Removable Media:           Fixed\n   Media Read-Only:           Yes\n   Protocol:                  USB\n";
        assert_eq!(parse_diskutil_info(ro), ("Locked".to_string(), true, false), "USB protocol counts as removable");
        assert_eq!(parse_diskutil_info(""), ("disk".to_string(), false, true));
    }

    #[test]
    fn bsd_mount_lines_are_parsed() {
        let m = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n/dev/disk4s2 on /Volumes/USB STICK (msdos, local, nodev, nosuid, noowners)\ndevfs on /dev (devfs, local, nobrowse)\n";
        let p = parse_bsd_mount(m);
        assert_eq!(p, vec![("/dev/disk3s1s1".to_string(), "/".to_string()), ("/dev/disk4s2".to_string(), "/Volumes/USB STICK".to_string())]);
        assert_eq!(mounts_of("disk4", &p).len(), 1);
        assert_eq!(mounts_of("disk3", &p).len(), 1);
    }
}
