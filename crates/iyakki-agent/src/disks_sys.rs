//! The real machine behind [`crate::disks::DiskEnv`]: `/sys` and `/proc` text, device reads and writes, `mount`, `umount2` (never
//! forced), `sync`, the partition-table re-read ioctl and `mke2fs`. This is the only file of the DISKS engine that touches the
//! base OS (BOUNDARY.md); the tests use a made-up machine and never run any of it.

use std::ffi::CString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::disks::{DiskEnv, MountFlags};

pub struct RealEnv;

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "a path with a NUL byte".to_string())
}

fn dev_path_ok(dev: &str) -> Result<(), String> {
    if dev.starts_with("/dev/") && !dev.contains("..") { Ok(()) } else { Err(format!("{dev} is not a device path")) }
}

impl DiskEnv for RealEnv {
    fn read_text(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn list(&self, path: &str) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn read_link(&self, path: &str) -> Option<String> {
        std::fs::read_link(path).ok().map(|p| p.to_string_lossy().into_owned())
    }

    fn canonical(&self, path: &str) -> Option<String> {
        std::fs::canonicalize(path).ok().map(|p| p.to_string_lossy().into_owned())
    }

    fn dev_read(&self, dev: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        dev_path_ok(dev)?;
        let mut f = std::fs::File::open(dev).map_err(|e| format!("{dev}: {e}"))?;
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; len];
        let mut n = 0;
        while n < len {
            match f.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) => return Err(format!("{dev}: {e}")),
            }
        }
        buf.truncate(n);
        Ok(buf)
    }

    fn dev_write(&self, dev: &str, offset: u64, data: &[u8]) -> Result<(), String> {
        dev_path_ok(dev)?;
        let mut f = std::fs::OpenOptions::new().write(true).open(dev).map_err(|e| format!("{dev}: {e}"))?;
        f.seek(SeekFrom::Start(offset)).and_then(|_| f.write_all(data)).and_then(|_| f.sync_all()).map_err(|e| format!("{dev}: {e}"))
    }

    fn dev_zero(&self, dev: &str, offset: u64, len: u64) -> Result<(), String> {
        dev_path_ok(dev)?;
        let mut f = std::fs::OpenOptions::new().write(true).open(dev).map_err(|e| format!("{dev}: {e}"))?;
        f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let chunk = vec![0u8; 1 << 20];
        let mut left = len;
        while left > 0 {
            let n = left.min(chunk.len() as u64) as usize;
            f.write_all(&chunk[..n]).map_err(|e| format!("{dev}: {e}"))?;
            left -= n as u64;
        }
        f.sync_all().map_err(|e| format!("{dev}: {e}"))
    }

    fn dev_len(&self, dev: &str) -> u64 {
        std::fs::File::open(dev).and_then(|mut f| f.seek(SeekFrom::End(0))).unwrap_or(0)
    }

    fn space(&self, mountpoint: &str) -> Option<(u64, u64)> {
        iyakki_storage::space(Path::new(mountpoint)).map(|(free, total)| (free, total))
    }

    fn mount(&self, dev: &str, target: &str, fstype: &str, flags: MountFlags, data: &str) -> Result<(), String> {
        dev_path_ok(dev)?;
        let (s, t, f, d) = (cstr(dev)?, cstr(target)?, cstr(fstype)?, cstr(data)?);
        let mut fl: libc::c_ulong = 0;
        if flags.rdonly {
            fl |= libc::MS_RDONLY;
        }
        if flags.nosuid {
            fl |= libc::MS_NOSUID;
        }
        if flags.nodev {
            fl |= libc::MS_NODEV;
        }
        if flags.noexec {
            fl |= libc::MS_NOEXEC;
        }
        // SAFETY: all pointers are valid NUL-terminated strings for the duration of the call.
        let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), fl, d.as_ptr().cast()) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
    }

    fn umount(&self, target: &str) -> Result<(), String> {
        let t = cstr(target)?;
        // SAFETY: a valid NUL-terminated path; flags 0 = a plain unmount (no MNT_FORCE, no MNT_DETACH).
        let rc = unsafe { libc::umount2(t.as_ptr(), 0) };
        if rc == 0 { return Ok(()) };
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EBUSY) { Err("device is busy".into()) } else { Err(e.to_string()) }
    }

    fn make_dir(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())
    }

    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), String> {
        std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| e.to_string())
    }

    fn remove_dir(&self, path: &str) -> Result<(), String> {
        std::fs::remove_dir(Path::new(path)).map_err(|e| e.to_string())
    }

    fn sync(&self) {
        // SAFETY: sync(2) takes no arguments.
        unsafe { libc::sync() };
    }

    fn write_sys(&self, path: &str, text: &str) -> Result<(), String> {
        if !path.starts_with("/sys/") || path.contains("..") {
            return Err(format!("{path} is not a sysfs path"));
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    fn reread_table(&self, disk_dev: &str) -> Result<(), String> {
        dev_path_ok(disk_dev)?;
        let f = std::fs::OpenOptions::new().read(true).open(disk_dev).map_err(|e| format!("{disk_dev}: {e}"))?;
        // SAFETY: BLKRRPART (0x125f) takes no argument.
        let rc = unsafe { libc::ioctl(f.as_raw_fd(), 0x125f as _) };
        if rc == 0 { Ok(()) } else { Err(format!("the kernel did not re-read the partition table ({})", std::io::Error::last_os_error())) }
    }

    fn mke2fs(&self, dev: &str, label: &str, uuid: &str) -> Result<(), String> {
        dev_path_ok(dev)?;
        let prog = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().find(|p| Path::new(p).exists()).ok_or("mke2fs is not installed")?;
        let args: Vec<std::ffi::OsString> = ["-t", "ext4", "-L", label, "-U", uuid, "-F", "-q", "-m", "1", "-E", &format!("nodiscard,lazy_itable_init=1,lazy_journal_init=1,hash_seed={uuid}"), dev].iter().map(Into::into).collect();
        let out = std::process::Command::new(prog).args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("could not run mke2fs: {e}"))?;
        if out.status.success() { Ok(()) } else { Err(format!("mke2fs failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
    }

    fn wait_node(&self, dev: &str) -> bool {
        for _ in 0..40 {
            if Path::new(dev).exists() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }
}
