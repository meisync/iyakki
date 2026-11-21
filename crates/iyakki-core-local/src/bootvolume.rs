//! Finding the boot partition on the disk the data volume is on (F-ARCH-07, moved from the agent's `devmode/bootmount.rs`): the data
//! folder's mount line in `<proc>/mounts`, the disk it is on in `<sys>/class/block`, and the FAT32 volume labelled IYAKKI-BOOT among that
//! disk's partitions in `<dev>`. The agent mounts what is found here; every root is an argument so a test uses a made-up tree.

use std::path::{Path, PathBuf};

/// `sda2` -> `sda`, `nvme0n1p2` -> `nvme0n1`, `mmcblk0p2` -> `mmcblk0`.
pub fn parent_disk(part: &str) -> String {
    let digits_end = part.trim_end_matches(|c: char| c.is_ascii_digit());
    if digits_end.len() == part.len() {
        return part.to_string();
    }
    let base = if (digits_end.ends_with('p') && digits_end[..digits_end.len() - 1].ends_with(|c: char| c.is_ascii_digit())) || digits_end.ends_with("pp") { &digits_end[..digits_end.len() - 1] } else { digits_end };
    base.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountLine {
    pub dev: String,
    pub dir: PathBuf,
    pub fs: String,
    pub ro: bool,
}

pub fn parse_mounts(text: &str) -> Vec<MountLine> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 4).then(|| MountLine { dev: f[0].to_string(), dir: PathBuf::from(f[1].replace("\\040", " ")), fs: f[2].to_string(), ro: f[3].split(',').any(|o| o == "ro") })
        })
        .collect()
}

/// Does the first sector of this device node say FAT32 with the label IYAKKI-BOOT?
pub fn is_boot_volume(dev: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 512];
    std::fs::File::open(dev).and_then(|mut f| f.read_exact(&mut b)).is_ok() && &b[71..82] == b"IYAKKI-BOOT" && &b[82..90] == b"FAT32   " && b[510..512] == [0x55, 0xAA]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub node: PathBuf,
    /// Where it is mounted already, and whether read-only.
    pub mounted: Option<(PathBuf, bool)>,
}

/// The partitions of the data volume's own disk that hold the boot volume. `data_dir` is where the data volume is mounted; the three
/// roots are `/proc`, `/sys` and `/dev` on the machine. Never a partition of another drive; two labelled volumes are an error, not a guess.
pub fn locate_in(data_dir: &Path, proc_: &Path, sys: &Path, dev: &Path) -> Result<Located, String> {
    let mounts = parse_mounts(&std::fs::read_to_string(proc_.join("mounts")).unwrap_or_default());
    let data = mounts.iter().filter(|m| m.dir == data_dir).last().ok_or("the data folder is not a mounted volume: there is no IYAKKI-DATA disk to find the boot partition on")?;
    let name = data.dev.rsplit('/').next().unwrap_or("").to_string();
    let disk = parent_disk(&name);
    if disk.is_empty() || !sys.join("class/block").join(&disk).is_dir() {
        return Err(format!("the data volume ({}) is not on a disk this device can look at", data.dev));
    }
    let mut found = Vec::new();
    for e in std::fs::read_dir(sys.join("class/block").join(&disk)).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(&disk) && n != disk && is_boot_volume(&dev.join(&n)) {
            found.push(n);
        }
    }
    found.sort();
    match found.len() {
        0 => Err(format!("no IYAKKI-BOOT partition on {disk}, the disk the data volume is on")),
        1 => {
            let node = dev.join(&found[0]);
            let mounted = mounts.iter().find(|m| m.fs == "vfat" && Path::new(&m.dev).file_name().is_some_and(|f| f.to_string_lossy() == found[0])).map(|m| (m.dir.clone(), m.ro));
            Ok(Located { node, mounted })
        }
        _ => Err(format!("{} partitions of {disk} are labelled IYAKKI-BOOT: not guessing which one to write", found.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fat_node(label: &[u8; 11], sig: &[u8; 8]) -> Vec<u8> {
        let mut b = vec![0u8; 1024];
        b[71..82].copy_from_slice(label);
        b[82..90].copy_from_slice(sig);
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    }

    struct M {
        data: PathBuf,
        proc: PathBuf,
        sys: PathBuf,
        dev: PathBuf,
    }

    impl M {
        fn locate(&self) -> Result<Located, String> {
            locate_in(&self.data, &self.proc, &self.sys, &self.dev)
        }
    }

    fn machine(tag: &str, mounted_boot: Option<&str>, two_boots: bool, other_disk_boot: bool) -> M {
        let d = std::env::temp_dir().join(format!("iyakki-bootvol-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let w = |p: PathBuf, c: &[u8]| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        };
        for part in ["sda1", "sda2", "sdb1"] {
            w(d.join("sys/class/block").join(part.trim_end_matches(char::is_numeric)).join(part).join("x"), b"");
        }
        w(d.join("dev/sda1"), &fat_node(b"IYAKKI-BOOT", b"FAT32   "));
        w(d.join("dev/sda2"), &vec![0u8; 1024]);
        if two_boots {
            w(d.join("sys/class/block/sda/sda3/x"), b"");
            w(d.join("dev/sda3"), &fat_node(b"IYAKKI-BOOT", b"FAT32   "));
        }
        // a boot partition on another drive must never be picked
        w(d.join("dev/sdb1"), &fat_node(b"IYAKKI-BOOT", b"FAT32   "));
        let _ = other_disk_boot;
        let data = d.join("var/lib/iyakki");
        std::fs::create_dir_all(&data).unwrap();
        let mut m = format!("proc /proc proc rw 0 0\n/dev/sda2 {} ext4 rw 0 0\n", data.display());
        if let Some(opts) = mounted_boot {
            m.push_str(&format!("/dev/sda1 /boot vfat {opts} 0 0\n"));
        }
        w(d.join("proc/mounts"), m.as_bytes());
        M { data, proc: d.join("proc"), sys: d.join("sys"), dev: d.join("dev") }
    }

    #[test]
    fn the_boot_partition_is_the_labelled_fat32_volume_on_the_disk_of_the_data_volume_only() {
        assert_eq!(parent_disk("sda2"), "sda");
        assert_eq!(parent_disk("nvme0n1p2"), "nvme0n1");
        assert_eq!(parent_disk("mmcblk0p3"), "mmcblk0");
        assert_eq!(parent_disk("vdb1"), "vdb");
        assert_eq!(parent_disk("sda"), "sda");
        let m = machine("plain", None, false, true);
        let l = m.locate().unwrap();
        assert!(l.node.ends_with("dev/sda1"), "sdb1 is on another drive and is not picked: {l:?}");
        assert_eq!(l.mounted, None);
        let m = machine("ro", Some("ro,relatime"), false, false);
        assert_eq!(m.locate().unwrap().mounted, Some((PathBuf::from("/boot"), true)));
        let m = machine("rw", Some("rw,relatime"), false, false);
        assert_eq!(m.locate().unwrap().mounted, Some((PathBuf::from("/boot"), false)));
        let e = machine("two", None, true, false).locate().unwrap_err();
        assert!(e.contains("not guessing"), "{e}");
    }

    #[test]
    fn a_missing_data_volume_or_a_wrong_label_or_a_wrong_filesystem_finds_nothing() {
        let m = machine("nodata", None, false, false);
        std::fs::write(m.proc.join("mounts"), "proc /proc proc rw 0 0\n").unwrap();
        assert!(m.locate().unwrap_err().contains("not a mounted volume"));
        let m = machine("label", None, false, false);
        std::fs::write(m.dev.join("sda1"), fat_node(b"SOMETHING-E", b"FAT32   ")).unwrap();
        assert!(m.locate().unwrap_err().contains("no IYAKKI-BOOT"));
        std::fs::write(m.dev.join("sda1"), fat_node(b"IYAKKI-BOOT", b"FAT16   ")).unwrap();
        assert!(m.locate().is_err());
        assert!(!is_boot_volume(&m.dev.join("nothing")));
        let lines = parse_mounts("/dev/sda1 /run/a\\040b vfat ro,noatime 0 0\nbad\n");
        assert_eq!(lines, vec![MountLine { dev: "/dev/sda1".into(), dir: "/run/a b".into(), fs: "vfat".into(), ro: true }]);
    }
}
