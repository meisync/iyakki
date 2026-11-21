//! Shrinking a btrfs file system (F-INST-27). btrfs can only be resized while it is MOUNTED, so the steps are: check the unmounted
//! volume (`btrfs check --readonly`), mount it on a private folder (read-only to ask for the smallest size, read-write to shrink), run
//! `btrfs filesystem resize`, unmount, and check again. The computer's own `btrfs`, `mount` and `umount` do the work (never shipped);
//! this module only orders the steps and undoes the mount on every path out. The commands go through [`Sys`], so the order, the
//! unmount after a failure and the refusals are tested with a recording fake; a loop-file run of the real tools is the Manual part.
//!
//! xfs cannot be shrunk at all (the file system has no such operation); [`xfs_refusal`] is the one sentence the plan gives for it.

use std::path::{Path, PathBuf};

/// The commands and the folder this module needs from the system.
pub trait Sys {
    /// Run a program; the text is its output, an error says what failed.
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, String>;
    /// A new empty private folder to mount on.
    fn make_dir(&self) -> Result<PathBuf, String>;
    fn remove_dir(&self, dir: &Path);
}

/// The real system: `std::process::Command` and a folder under the temporary folder.
pub struct HostSys;

impl Sys for HostSys {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, String> {
        let o = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e} (is it installed?)"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        if o.status.success() {
            Ok(text)
        } else {
            Err(format!("{cmd} failed: {}", text.lines().last().unwrap_or("").trim()))
        }
    }
    fn make_dir(&self) -> Result<PathBuf, String> {
        let d = std::env::temp_dir().join(format!("iyakki-btrfs-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.subsec_nanos())));
        std::fs::create_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        Ok(d)
    }
    fn remove_dir(&self, dir: &Path) {
        let _ = std::fs::remove_dir(dir);
    }
}

/// The sentence for xfs (and the reason it is not an option).
pub fn xfs_refusal(part: &str) -> String {
    format!("{part} holds an xfs file system, and xfs cannot be made smaller by any tool (it can only grow). Copy what you need off it, then use the other system's own disk tool to delete and recreate the partition smaller")
}

/// A mounted btrfs volume; unmounted and its folder removed when dropped, whatever happened.
struct Mounted<'a> {
    sys: &'a dyn Sys,
    dir: PathBuf,
    mounted: bool,
}

impl<'a> Mounted<'a> {
    fn new(sys: &'a dyn Sys, dev: &Path, read_only: bool) -> Result<Mounted<'a>, String> {
        let dir = sys.make_dir()?;
        let (d, m) = (dev.to_str().ok_or("path")?, dir.to_str().ok_or("path")?);
        let opts = if read_only { "loop,ro,noatime" } else { "loop,noatime" };
        let mut me = Mounted { sys, dir: dir.clone(), mounted: false };
        match sys.run("mount", &["-t", "btrfs", "-o", opts, d, m]) {
            Ok(_) => {
                me.mounted = true;
                Ok(me)
            }
            Err(e) => Err(format!("{} could not be mounted to be resized ({e}); nothing was changed", dev.display())),
        }
    }
    fn path(&self) -> String {
        self.dir.display().to_string()
    }
    /// Unmount now and say so if it failed (the drop would hide it).
    fn unmount(mut self) -> Result<(), String> {
        self.mounted = false;
        let m = self.path();
        let r = self.sys.run("umount", &[&m]);
        self.sys.remove_dir(&self.dir);
        r.map(|_| ()).map_err(|e| format!("the volume could not be unmounted ({e}); it is still mounted at {m}"))
    }
}

impl Drop for Mounted<'_> {
    fn drop(&mut self) {
        if self.mounted {
            let m = self.path();
            let _ = self.sys.run("umount", &[&m]);
            self.sys.remove_dir(&self.dir);
        }
    }
}

/// The text `btrfs inspect-internal min-dev-size` prints: `123456 bytes (117.7MiB)`.
pub fn parse_min_dev_size(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// The smallest size, in bytes, the volume can be shrunk to (asked of the mounted volume, read-only).
pub fn minimum(sys: &dyn Sys, dev: &Path) -> Result<u64, String> {
    let m = Mounted::new(sys, dev, true)?;
    let p = m.path();
    let t = sys.run("btrfs", &["inspect-internal", "min-dev-size", &p]);
    m.unmount()?;
    parse_min_dev_size(&t?).ok_or_else(|| "btrfs did not say a minimum size".to_string())
}

/// Check without changing anything: `btrfs check --readonly` on the unmounted volume.
pub fn check(sys: &dyn Sys, dev: &Path) -> Result<(), String> {
    let d = dev.to_str().ok_or("path")?;
    sys.run("btrfs", &["check", "--readonly", d]).map(|_| ())
}

/// Shrink the mounted volume to `bytes`, then unmount. Never leaves it mounted.
pub fn shrink(sys: &dyn Sys, dev: &Path, bytes: u64) -> Result<(), String> {
    let m = Mounted::new(sys, dev, false)?;
    let p = m.path();
    let r = sys.run("btrfs", &["filesystem", "resize", &bytes.to_string(), &p]);
    m.unmount()?;
    r.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        log: RefCell<Vec<String>>,
        fail: Option<&'static str>,
        min_text: &'static str,
    }
    impl Sys for Fake {
        fn run(&self, cmd: &str, args: &[&str]) -> Result<String, String> {
            let what = if cmd == "btrfs" { format!("btrfs {}", args[0]) } else { cmd.to_string() };
            self.log.borrow_mut().push(format!("{cmd} {}", args.join(" ")).replace("/tmp/fakemnt", "MNT"));
            if self.fail == Some(what.as_str()) {
                return Err(format!("{cmd} failed: boom"));
            }
            if args.first() == Some(&"inspect-internal") {
                return Ok(self.min_text.to_string());
            }
            Ok(String::new())
        }
        fn make_dir(&self) -> Result<PathBuf, String> {
            self.log.borrow_mut().push("mkdir".into());
            Ok(PathBuf::from("/tmp/fakemnt"))
        }
        fn remove_dir(&self, _d: &Path) {
            self.log.borrow_mut().push("rmdir".into());
        }
    }
    fn fake(fail: Option<&'static str>) -> Fake {
        Fake { fail, min_text: "52428800 bytes (50.0MiB)\n", ..Default::default() }
    }

    #[test]
    fn the_minimum_is_asked_of_a_read_only_mount_that_is_undone() {
        let f = fake(None);
        assert_eq!(minimum(&f, Path::new("/dev/sdb2")).unwrap(), 52_428_800);
        assert_eq!(*f.log.borrow(), ["mkdir", "mount -t btrfs -o loop,ro,noatime /dev/sdb2 MNT", "btrfs inspect-internal min-dev-size MNT", "umount MNT", "rmdir"]);
    }

    #[test]
    fn a_shrink_mounts_writable_resizes_by_bytes_and_unmounts() {
        let f = fake(None);
        shrink(&f, Path::new("/dev/sdb2"), 200 * 1024 * 1024).unwrap();
        assert_eq!(*f.log.borrow(), ["mkdir", "mount -t btrfs -o loop,noatime /dev/sdb2 MNT", "btrfs filesystem resize 209715200 MNT", "umount MNT", "rmdir"]);
    }

    #[test]
    fn a_failed_resize_still_unmounts_and_says_so() {
        let f = fake(Some("btrfs filesystem"));
        let e = shrink(&f, Path::new("/dev/sdb2"), 1 << 28).unwrap_err();
        assert!(e.contains("btrfs failed"), "{e}");
        assert!(f.log.borrow().iter().any(|l| l == "umount MNT"));
        assert_eq!(f.log.borrow().last().unwrap(), "rmdir");
    }

    #[test]
    fn a_mount_that_fails_changes_nothing_and_leaves_no_folder_unmounted_twice() {
        let f = fake(Some("mount"));
        let e = minimum(&f, Path::new("/dev/sdb2")).unwrap_err();
        assert!(e.contains("nothing was changed"), "{e}");
        assert!(!f.log.borrow().iter().any(|l| l.starts_with("umount")), "{:?}", f.log.borrow());
    }

    #[test]
    fn a_volume_that_will_not_unmount_is_reported_and_an_unparsable_minimum_is_refused() {
        let f = fake(Some("umount"));
        assert!(shrink(&f, Path::new("/dev/sdb2"), 1 << 28).unwrap_err().contains("still mounted"));
        let g = Fake { min_text: "garbage", ..Default::default() };
        assert!(minimum(&g, Path::new("/dev/sdb2")).unwrap_err().contains("minimum"));
        assert_eq!(parse_min_dev_size("1048576 bytes (1.00MiB)"), Some(1_048_576));
    }

    #[test]
    fn the_check_is_read_only_and_xfs_is_refused_in_words() {
        let f = fake(None);
        check(&f, Path::new("/dev/sdb2")).unwrap();
        assert_eq!(*f.log.borrow(), ["btrfs check --readonly /dev/sdb2"]);
        let s = xfs_refusal("sdb3");
        assert!(s.contains("xfs") && s.contains("cannot be made smaller"));
    }
}
