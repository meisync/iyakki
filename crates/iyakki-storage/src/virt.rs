//! Virtual disks for the desktop window: instead of the host's real drives, the window shows disks that IYAKKI makes for
//! itself, as sparse image files in a folder of its own. The folder looks like a small machine (`sys/block`, `dev`,
//! `mounts`) to the rest of this crate, so listing, formatting and every safety refusal run unchanged, and nothing can
//! reach a real drive.

use crate::{Mounter, Roots};
use std::path::{Path, PathBuf};

pub const MIN_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;
const MODEL: &str = "IYAKKI virtual disk";

#[derive(Clone, Debug)]
pub struct VirtualDisks {
    root: PathBuf,
}

/// "8G", "512M", "1.5G", "2T" or plain bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase();
    let s = s.strip_suffix("IB").or_else(|| s.strip_suffix('B')).unwrap_or(&s);
    let (num, mult) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        'T' => (&s[..s.len() - 1], 1 << 40),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    let n: f64 = num.trim().parse().ok().filter(|n: &f64| n.is_finite() && *n > 0.0)?;
    let bytes = n * mult as f64;
    (bytes < u64::MAX as f64 / 2.0).then_some(bytes as u64)
}

impl VirtualDisks {
    /// Use (and make, if needed) the folder `root` as the virtual machine's disk area.
    pub fn open(root: &Path) -> Result<VirtualDisks, String> {
        for d in ["sys/block", "dev"] {
            std::fs::create_dir_all(root.join(d)).map_err(|e| format!("{}: {e}", root.join(d).display()))?;
        }
        let mounts = root.join("mounts");
        if !mounts.exists() {
            std::fs::write(&mounts, "").map_err(|e| format!("{}: {e}", mounts.display()))?;
        }
        Ok(VirtualDisks { root: root.to_path_buf() })
    }

    /// Where the rest of the storage code looks. Disks are plain files here, not block devices.
    pub fn roots(&self) -> Roots {
        Roots { sys_block: self.root.join("sys/block"), dev: self.root.join("dev"), mounts: self.root.join("mounts"), require_block: false }
    }

    pub fn mounter(&self) -> VirtualMounter {
        VirtualMounter
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.root.join("sys/block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// Make a new disk of `bytes` (a sparse file: it takes no space until something is written). Returns its name, `vda`, `vdb`, …
    pub fn add(&self, bytes: u64) -> Result<String, String> {
        if !(MIN_BYTES..=MAX_BYTES).contains(&bytes) {
            return Err(format!("a virtual disk is between {} and {}", crate::human(MIN_BYTES), crate::human(MAX_BYTES)));
        }
        let taken = self.names();
        let name = ('a'..='z').map(|c| format!("vd{c}")).find(|n| !taken.contains(n)).ok_or("there are already 26 virtual disks")?;
        let bytes = bytes / 512 * 512;
        let dir = self.root.join("sys/block").join(&name);
        let node = self.root.join("dev").join(&name);
        let made = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir.join("device"))?;
            std::fs::write(dir.join("size"), format!("{}\n", bytes / 512))?;
            std::fs::write(dir.join("removable"), "0\n")?;
            std::fs::write(dir.join("device/model"), format!("{MODEL}\n"))?;
            std::fs::File::create(&node)?.set_len(bytes)
        })();
        if let Err(e) = made {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_file(&node);
            return Err(format!("could not make the disk: {e}"));
        }
        Ok(name)
    }

    /// Delete a virtual disk and its image file.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        if !(name.len() == 3 && name.starts_with("vd") && name.as_bytes()[2].is_ascii_lowercase()) || !self.names().iter().any(|n| n == name) {
            return Err(format!("{name} is not one of the virtual disks"));
        }
        std::fs::remove_dir_all(self.root.join("sys/block").join(name)).map_err(|e| e.to_string())?;
        std::fs::remove_file(self.root.join("dev").join(name)).map_err(|e| e.to_string())
    }
}

/// Mounting needs the administrator's rights and a real kernel, so a virtual disk is never mounted: its "mount" is a
/// plain folder the data is copied into, which keeps every step of making a data disk visible without touching anything.
pub struct VirtualMounter;

impl Mounter for VirtualMounter {
    fn mount(&self, _dev: &Path, target: &Path) -> Result<(), String> {
        std::fs::create_dir_all(target).map_err(|e| e.to_string())
    }
    fn unmount(&self, _target: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-virt-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn sizes_are_understood() {
        assert_eq!(parse_size("8G"), Some(8 << 30));
        assert_eq!(parse_size("512m"), Some(512 << 20));
        assert_eq!(parse_size("1.5G"), Some(3 << 29));
        assert_eq!(parse_size("2 TiB"), Some(2 << 40));
        assert_eq!(parse_size("1048576"), Some(1 << 20));
        for bad in ["", "G", "-1G", "abc", "0", "1X", "NaNG", "infG"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn disks_are_made_listed_formatted_and_removed_without_touching_anything_real() {
        let d = dir("life");
        let v = VirtualDisks::open(&d).unwrap();
        assert!(crate::list(&v.roots()).is_empty());
        assert!(v.add(1024).is_err(), "too small");
        assert_eq!(v.add(128 << 20).unwrap(), "vda");
        assert_eq!(v.add(256 << 20).unwrap(), "vdb");
        let list = crate::list(&v.roots());
        assert_eq!(list.iter().map(|b| (b.name.as_str(), b.size_bytes)).collect::<Vec<_>>(), vec![("vda", 128 << 20), ("vdb", 256 << 20)]);
        assert!(list.iter().all(|b| b.content == crate::Content::Empty && b.model == MODEL));
        // a sparse file takes almost no room
        use std::os::unix::fs::MetadataExt;
        assert!(std::fs::metadata(d.join("dev/vda")).unwrap().blocks() * 512 < 1 << 20);

        if let Some(mke2fs) = ["/usr/sbin/mke2fs", "/sbin/mke2fs"].iter().map(PathBuf::from).find(|p| p.exists()) {
            let seed = d.join("seed");
            std::fs::create_dir_all(&seed).unwrap();
            std::fs::write(seed.join("users.json"), "{}").unwrap();
            let r = crate::persist(&v.roots(), &v.mounter(), &mke2fs, &d, "vda", false, Some(&seed)).expect("persist");
            assert_eq!((r.device.as_str(), r.files_copied), ("vda", 1));
            assert_eq!(crate::probe(&d.join("dev/vda")).unwrap(), crate::Content::Iyakki);
            // the safety refusal is the real one: a disk that already holds data needs confirmation
            assert!(crate::persist(&v.roots(), &v.mounter(), &mke2fs, &d, "vda", false, None).is_err());
        }

        v.remove("vda").unwrap();
        assert_eq!(v.names(), vec!["vdb"]);
        assert!(!d.join("dev/vda").exists());
        assert_eq!(v.add(64 << 20).unwrap(), "vda", "a free name is reused");
        assert!(v.remove("sda").is_err() && v.remove("../x").is_err() && v.remove("vdz").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
