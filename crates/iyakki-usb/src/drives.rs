//! Which disks exist, and which of them this tool is willing to overwrite.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// Where to write (`/dev/sdb`, or `/dev/rdisk4` on macOS for speed).
    pub path: String,
    /// What the user sees and types (`sdb`, `disk4`).
    pub name: String,
    pub model: String,
    pub size_bytes: u64,
    pub removable: bool,
    pub usb: bool,
    pub read_only: bool,
    /// (device, mount point) of this disk and its partitions.
    pub mounts: Vec<(String, String)>,
    /// The disk (or a partition of it) is swap space.
    pub swap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A removable or USB disk with nothing important on it.
    Ok,
    /// A fixed internal disk: only offered with `--all`, with an extra warning.
    Internal,
    /// Never written, with the reason shown to the user.
    Refused(String),
}

/// Mount points that mean "this disk holds the running system".
pub fn critical_mount(mp: &str) -> bool {
    matches!(mp, "/" | "/usr" | "/var" | "/etc" | "/home" | "/opt" | "/srv" | "/nix" | "/System" | "/Users" | "/Applications") || mp == "/boot" || mp.starts_with("/boot/") || mp.starts_with("/System/Volumes/")
}

pub fn parse_proc_mounts(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| {
        let mut it = l.split_whitespace();
        let (dev, mp) = (it.next()?, it.next()?);
        dev.starts_with("/dev/").then(|| (dev.to_string(), mp.replace("\\040", " ")))
    }).collect()
}

/// Is `dev` the disk `name` or one of its partitions? Disk names that end in a digit (`nvme0n1`, `mmcblk0`,
/// `disk4`) take `p<N>` / `s<N>` suffixes (macOS volumes can nest: `disk3s1s1`); names ending in a letter
/// (`sdb`) take a plain number. So `nvme0n10` is a different disk from `nvme0n1`.
pub fn belongs_to(dev: &str, name: &str) -> bool {
    let base = dev.rsplit('/').next().unwrap_or(dev);
    let Some(mut rest) = base.strip_prefix(name) else { return false };
    if rest.is_empty() {
        return true;
    }
    if !name.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        return rest.chars().all(|c| c.is_ascii_digit());
    }
    while !rest.is_empty() {
        let Some(r) = rest.strip_prefix('p').or_else(|| rest.strip_prefix('s')) else { return false };
        let digits = r.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        rest = &r[digits..];
    }
    true
}

pub fn mounts_of(name: &str, all: &[(String, String)]) -> Vec<(String, String)> {
    all.iter().filter(|(d, _)| belongs_to(d, name)).cloned().collect()
}

/// May this drive be written with `image`? The image's own location counts: never erase the stick the image is on.
pub fn verdict(d: &Drive, image: &Path) -> Verdict {
    if d.size_bytes == 0 {
        return Verdict::Refused("no media in the drive".into());
    }
    if d.read_only {
        return Verdict::Refused("write-protected".into());
    }
    if let Some((_, mp)) = d.mounts.iter().find(|(_, mp)| critical_mount(mp)) {
        return Verdict::Refused(format!("holds the running system ({mp} is mounted from it)"));
    }
    if d.swap {
        return Verdict::Refused("in use as swap space".into());
    }
    if let Some((_, mp)) = d.mounts.iter().find(|(_, mp)| mp != "/" && image.starts_with(mp)) {
        return Verdict::Refused(format!("the image file lives on it ({mp})"));
    }
    if d.removable || d.usb { Verdict::Ok } else { Verdict::Internal }
}

/// The logical sector size of a drive: Linux reads it from `/sys/block/<name>/queue/logical_block_size`; elsewhere (and when
/// unknown) 512.
pub fn logical_sector(d: &Drive) -> u32 {
    logical_sector_in(Path::new("/sys"), &d.name)
}

pub fn logical_sector_in(sys: &Path, name: &str) -> u32 {
    std::fs::read_to_string(sys.join("block").join(name).join("queue/logical_block_size")).ok().and_then(|t| t.trim().parse().ok()).filter(|n| *n >= 512).unwrap_or(512)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

/// Linux: disks from `/sys/block`. `sys` is `/sys`, `proc_mounts` the text of `/proc/mounts`, `swaps` of `/proc/swaps`.
pub fn list_linux(sys: &Path, proc_mounts: &str, swaps: &str) -> Vec<Drive> {
    let mounts = parse_proc_mounts(proc_mounts);
    let mut names: Vec<String> = std::fs::read_dir(sys.join("block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut out = Vec::new();
    for n in names {
        if ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| n.starts_with(p)) {
            continue;
        }
        let d = sys.join("block").join(&n);
        let sectors: u64 = read(&d.join("size")).parse().unwrap_or(0);
        let model = [read(&d.join("device/vendor")), read(&d.join("device/model"))].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let usb = std::fs::canonicalize(&d).map(|p| p.to_string_lossy().contains("/usb")).unwrap_or(false);
        out.push(Drive {
            path: format!("/dev/{n}"),
            swap: swaps.lines().skip(1).any(|l| l.split_whitespace().next().is_some_and(|dev| belongs_to(dev, &n))),
            mounts: mounts_of(&n, &mounts),
            name: n,
            model: if model.is_empty() { "disk".into() } else { model },
            size_bytes: sectors * 512,
            removable: read(&d.join("removable")) == "1",
            usb,
            read_only: read(&d.join("ro")) == "1",
        });
    }
    out
}

/// The disks of this computer.
pub fn list_drives() -> Result<Vec<Drive>, String> {
    #[cfg(target_os = "linux")]
    {
        Ok(list_linux(Path::new("/sys"), &std::fs::read_to_string("/proc/mounts").unwrap_or_default(), &std::fs::read_to_string("/proc/swaps").unwrap_or_default()))
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::list_drives()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("this tool supports Linux and macOS; on Windows use Rufus or balenaEtcher in DD mode for now".into())
    }
}

/// Ask the kernel to re-read the partition table (Linux) so the new stick appears without replugging.
pub fn reread_partitions(path: &str) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        if let Ok(f) = std::fs::OpenOptions::new().read(true).open(path) {
            // SAFETY: BLKRRPART (0x125f) takes no argument.
            unsafe { libc::ioctl(f.as_raw_fd(), 0x125f as _) };
        }
    }
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(name: &str) -> Drive {
        Drive { path: format!("/dev/{name}"), name: name.into(), model: "Cruzer".into(), size_bytes: 16_000_000_000, removable: true, usb: true, read_only: false, mounts: vec![], swap: false }
    }

    #[test]
    fn partitions_belong_to_their_disk_and_lookalikes_do_not() {
        for (dev, name, yes) in [("/dev/sdb", "sdb", true), ("/dev/sdb1", "sdb", true), ("/dev/sdb12", "sdb", true), ("/dev/sdba", "sdb", false), ("/dev/sdc1", "sdb", false), ("/dev/nvme0n1p2", "nvme0n1", true), ("/dev/nvme0n10", "nvme0n1", false), ("/dev/mmcblk0p1", "mmcblk0", true), ("/dev/disk4s2", "disk4", true), ("/dev/disk3s1s1", "disk3", true), ("/dev/disk40", "disk4", false), ("/dev/disk4s", "disk4", false), ("/dev/sdb1p", "sdb", false), ("/dev/disk4", "disk4", true), ("tmpfs", "sdb", false)] {
            assert_eq!(belongs_to(dev, name), yes, "{dev} vs {name}");
        }
    }

    #[test]
    fn mount_tables_are_parsed() {
        let t = "/dev/sda2 / ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sdb1 /media/amy/USB\\040STICK vfat rw 0 0\ntmpfs /run tmpfs rw 0 0\n";
        let m = parse_proc_mounts(t);
        assert_eq!(m, vec![("/dev/sda2".to_string(), "/".to_string()), ("/dev/sdb1".to_string(), "/media/amy/USB STICK".to_string())]);
        assert_eq!(mounts_of("sdb", &m).len(), 1);
        assert_eq!(mounts_of("sda", &m).len(), 1);
    }

    #[test]
    fn the_safety_rules() {
        let img = Path::new("/home/amy/iyakki.iso");
        assert_eq!(verdict(&drive("sdb"), img), Verdict::Ok);
        let mut internal = drive("sda");
        internal.removable = false;
        internal.usb = false;
        assert_eq!(verdict(&internal, img), Verdict::Internal);
        let mut sys = drive("sdc");
        sys.mounts = vec![("/dev/sdc2".into(), "/".into())];
        assert!(matches!(verdict(&sys, img), Verdict::Refused(r) if r.contains("running system")));
        for mp in ["/boot", "/boot/efi", "/home", "/usr", "/var", "/System/Volumes/Data"] {
            let mut d = drive("sdd");
            d.mounts = vec![("/dev/sdd1".into(), mp.into())];
            assert!(matches!(verdict(&d, img), Verdict::Refused(_)), "{mp}");
        }
        let mut media = drive("sde");
        media.mounts = vec![("/dev/sde1".into(), "/media/amy/USB".into())];
        assert_eq!(verdict(&media, img), Verdict::Ok, "an ordinary mounted stick is fine (it gets unmounted)");
        // the image on the stick we are about to erase
        let mut holder = drive("sdf");
        holder.mounts = vec![("/dev/sdf1".into(), "/media/amy/USB".into())];
        assert!(matches!(verdict(&holder, Path::new("/media/amy/USB/iyakki.iso")), Verdict::Refused(r) if r.contains("image file")));
        let mut ro = drive("sdg");
        ro.read_only = true;
        assert!(matches!(verdict(&ro, img), Verdict::Refused(r) if r.contains("write-protected")));
        let mut empty = drive("sdh");
        empty.size_bytes = 0;
        assert!(matches!(verdict(&empty, img), Verdict::Refused(r) if r.contains("no media")));
        let mut swap = drive("sdi");
        swap.swap = true;
        assert!(matches!(verdict(&swap, img), Verdict::Refused(r) if r.contains("swap")));
    }

    fn put(root: &Path, rel: &str, v: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, v).unwrap();
    }

    #[test]
    fn linux_disks_come_from_sysfs_and_usb_is_detected_from_the_device_path() {
        let root = std::env::temp_dir().join(format!("iyakki-usb-sys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // real directories under devices/, and symlinks in block/ like the kernel does
        put(&root, "devices/pci0000:00/usb1/1-1/host0/target0/0:0:0:0/block/sdb/size", "31250000\n");
        put(&root, "devices/pci0000:00/usb1/1-1/host0/target0/0:0:0:0/block/sdb/removable", "1\n");
        put(&root, "devices/pci0000:00/usb1/1-1/host0/target0/0:0:0:0/block/sdb/device/vendor", "SanDisk \n");
        put(&root, "devices/pci0000:00/usb1/1-1/host0/target0/0:0:0:0/block/sdb/device/model", "Cruzer Blade\n");
        put(&root, "devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1/size", "1000215216\n");
        put(&root, "devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1/removable", "0\n");
        put(&root, "devices/virtual/block/loop0/size", "100\n");
        std::fs::create_dir_all(root.join("block")).unwrap();
        for (n, t) in [("sdb", "devices/pci0000:00/usb1/1-1/host0/target0/0:0:0:0/block/sdb"), ("nvme0n1", "devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1"), ("loop0", "devices/virtual/block/loop0")] {
            std::os::unix::fs::symlink(root.join(t), root.join("block").join(n)).unwrap();
        }
        let mounts = "/dev/nvme0n1p2 / ext4 rw 0 0\n/dev/nvme0n1p1 /boot/efi vfat rw 0 0\n/dev/sdb1 /media/amy/KEY vfat rw 0 0\n";
        let d = list_linux(&root, mounts, "Filename Type Size Used Priority\n/dev/nvme0n1p3 partition 100 0 -2\n");
        assert_eq!(d.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["nvme0n1", "sdb"], "loop devices are not offered");
        let sdb = d.iter().find(|x| x.name == "sdb").unwrap();
        assert_eq!((sdb.model.as_str(), sdb.size_bytes, sdb.removable, sdb.usb, sdb.path.as_str()), ("SanDisk Cruzer Blade", 16_000_000_000, true, true, "/dev/sdb"));
        assert_eq!(sdb.mounts, vec![("/dev/sdb1".to_string(), "/media/amy/KEY".to_string())]);
        let nv = d.iter().find(|x| x.name == "nvme0n1").unwrap();
        assert!(!nv.removable && !nv.usb && nv.swap && nv.mounts.len() == 2);
        assert!(matches!(verdict(nv, Path::new("/tmp/x.iso")), Verdict::Refused(_)), "the system disk is refused");
        assert_eq!(verdict(sdb, Path::new("/tmp/x.iso")), Verdict::Ok);
    }

    #[test]
    fn the_real_machine_can_be_listed_without_panicking() {
        let _ = list_drives();
    }
}
