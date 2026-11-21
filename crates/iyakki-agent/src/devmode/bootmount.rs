//! The real way to reach the boot partition from the running system for a system update: find the FAT32 volume labelled
//! IYAKKI-BOOT **on the same disk as the data volume** (never a partition of another drive), mount it (or remount it read-write when
//! it is mounted read-only already), and put the mount back afterwards. Plain asynchronous mounts: no `sync` option.
//!
//! Finding the volume is tested against a made-up `/proc`, `/sys` and `/dev`. The mount calls themselves need the real kernel and
//! root, so they are NOT exercised by any test (see docs/DEVELOPER-MODE.md, "what was not tried").

use std::path::{Path, PathBuf};

use super::update::{BootMount, BootVol};

pub use iyakki_core_local::bootvolume::{is_boot_volume, parent_disk, parse_mounts, Located, MountLine};

pub struct SysBoot {
    pub data_dir: PathBuf,
    pub proc: PathBuf,
    pub sys: PathBuf,
    pub dev: PathBuf,
    pub mnt: PathBuf,
}

impl SysBoot {
    pub fn real(data_dir: &Path) -> SysBoot {
        SysBoot { data_dir: data_dir.into(), proc: iyakki_core_local::kernelfacts::PROC.into(), sys: iyakki_core_local::kernelfacts::SYS.into(), dev: iyakki_core_local::kernelfacts::DEV.into(), mnt: "/run/iyakki-boot".into() }
    }

    pub fn locate(&self) -> Result<Located, String> {
        iyakki_core_local::bootvolume::locate_in(&self.data_dir, &self.proc, &self.sys, &self.dev)
    }
}

fn cstr(p: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| "bad path".to_string())
}

fn do_mount(src: Option<&Path>, dir: &Path, fs: Option<&str>, flags: libc::c_ulong) -> Result<(), String> {
    let s = src.map(cstr).transpose()?;
    let d = cstr(dir)?;
    let f = fs.map(|f| std::ffi::CString::new(f).unwrap_or_default());
    // SAFETY: every pointer is a valid NUL-terminated string that outlives the call (or null where the kernel allows it).
    let rc = unsafe { crate::oscompat::mount(s.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), d.as_ptr(), f.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), flags, std::ptr::null()) };
    if rc == 0 { Ok(()) } else { Err(format!("mount {}: {}", dir.display(), std::io::Error::last_os_error())) }
}

fn do_umount(dir: &Path) {
    if let Ok(d) = cstr(dir) {
        // SAFETY: a valid NUL-terminated path.
        unsafe {
            libc::sync();
            crate::oscompat::umount(d.as_ptr());
        }
    }
}

impl BootMount for SysBoot {
    fn open(&self, write: bool) -> Result<BootVol, String> {
        let loc = self.locate()?;
        let base = crate::oscompat::MS_NOSUID | crate::oscompat::MS_NODEV | crate::oscompat::MS_NOEXEC;
        let (dir, restore): (PathBuf, Option<Box<dyn FnOnce() + Send>>) = match loc.mounted {
            Some((dir, was_ro)) => {
                if write && was_ro {
                    do_mount(None, &dir, None, crate::oscompat::MS_REMOUNT | base)?;
                    let d2 = dir.clone();
                    (dir, Some(Box::new(move || {
                        let _ = do_mount(None, &d2, None, crate::oscompat::MS_REMOUNT | crate::oscompat::MS_RDONLY | base);
                    })))
                } else {
                    (dir, None)
                }
            }
            None => {
                std::fs::create_dir_all(&self.mnt).map_err(|e| format!("{}: {e}", self.mnt.display()))?;
                do_mount(Some(&loc.node), &self.mnt, Some("vfat"), base | if write { 0 } else { crate::oscompat::MS_RDONLY })?;
                let d2 = self.mnt.clone();
                (self.mnt.clone(), Some(Box::new(move || do_umount(&d2))))
            }
        };
        let (total, free) = super::debug::statvfs(&dir);
        let cluster = cluster_of(&dir);
        let v = BootVol::new(dir, free, total, cluster);
        Ok(match restore {
            Some(r) => v.on_drop(r),
            None => v,
        })
    }

    fn free(&self, vol: &BootVol) -> u64 {
        super::debug::statvfs(&vol.root).1
    }
}

fn cluster_of(p: &Path) -> u64 {
    let Ok(c) = cstr(p) else { return 4096 };
    // SAFETY: a valid path and a zeroed out-parameter.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) == 0 { s.f_frsize as u64 } else { 4096 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_roots_are_the_kernels_and_a_missing_data_volume_is_an_error_not_a_guess() {
        let b = SysBoot::real(Path::new("/nonexistent-data-folder"));
        assert_eq!((b.proc.as_path(), b.sys.as_path(), b.dev.as_path()), (Path::new("/proc"), Path::new("/sys"), Path::new("/dev")));
        assert!(b.locate().unwrap_err().contains("not a mounted volume"));
    }
}
