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
    use super::*;

    const LIST: &str = "/dev/disk4 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:      GUID_partition_scheme                        *31.0 GB    disk4\n   1:                        EFI EFI                     209.7 MB   disk4s1\n   2:       Microsoft Basic Data USB STICK               30.8 GB    disk4s2\n\n/dev/disk5 (external, physical):\n   #:                       TYPE NAME                    SIZE       IDENTIFIER\n   0:     FDisk_partition_scheme                        *1.0 TB     disk5\n   1:               Windows_NTFS Backup                  1.0 TB     disk5s1\n";

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
