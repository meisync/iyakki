//! The real kernel for [`super::DmKernel`]: `/dev/mapper/control`, `/dev/loop-control`, `mount(2)`. Linux only; needs root.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{DmCmd, DmKernel, DM_IOCTL_LEN};

const LOOP_SET_FD: u64 = 0x4C00;
const LOOP_CLR_FD: u64 = 0x4C01;
const LOOP_SET_STATUS64: u64 = 0x4C04;
const LOOP_CTL_GET_FREE: u64 = 0x4C82;
const LO_FLAGS_AUTOCLEAR: u32 = 4;
const LOOP_INFO64_LEN: usize = 232;
const LOOP_INFO64_FLAGS_AT: usize = 52;

pub struct SysKernel;

fn open(path: &str, write: bool) -> io::Result<OwnedFd> {
    let c = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: valid NUL-terminated path.
    let fd = unsafe { libc::open(c.as_ptr(), (if write { libc::O_RDWR } else { libc::O_RDONLY }) | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor we own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn ioctl_ptr(fd: &OwnedFd, req: u64, arg: *mut libc::c_void) -> io::Result<i32> {
    // SAFETY: the caller passes a pointer to a buffer the request expects; `fd` is open.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, arg) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

impl DmKernel for SysKernel {
    fn dm_ioctl(&self, cmd: DmCmd, buf: &mut Vec<u8>) -> io::Result<()> {
        if buf.len() < DM_IOCTL_LEN {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let fd = open("/dev/mapper/control", true)?;
        ioctl_ptr(&fd, cmd.request(), buf.as_mut_ptr().cast()).map(|_| ())
    }

    fn loop_attach(&self, file: &Path) -> io::Result<String> {
        let ctl = open("/dev/loop-control", true)?;
        let backing = {
            let c = CString::new(file.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            // SAFETY: valid NUL-terminated path.
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fresh descriptor.
            unsafe { OwnedFd::from_raw_fd(fd) }
        };
        let mut last = io::Error::from(io::ErrorKind::Other);
        for _ in 0..8 {
            // another process may take the free number between the two calls: ask again
            // SAFETY: LOOP_CTL_GET_FREE takes no argument and returns the number.
            let n = unsafe { libc::ioctl(ctl.as_raw_fd(), LOOP_CTL_GET_FREE as _) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let path = format!("/dev/loop{n}");
            let dev = open(&path, true)?;
            // SAFETY: LOOP_SET_FD takes the backing descriptor as its argument.
            let r = unsafe { libc::ioctl(dev.as_raw_fd(), LOOP_SET_FD as _, backing.as_raw_fd()) };
            if r < 0 {
                last = io::Error::last_os_error();
                if last.raw_os_error() == Some(libc::EBUSY) {
                    continue;
                }
                return Err(last);
            }
            let mut info = [0u8; LOOP_INFO64_LEN];
            info[LOOP_INFO64_FLAGS_AT..LOOP_INFO64_FLAGS_AT + 4].copy_from_slice(&LO_FLAGS_AUTOCLEAR.to_ne_bytes());
            if let Err(e) = ioctl_ptr(&dev, LOOP_SET_STATUS64, info.as_mut_ptr().cast()) {
                // SAFETY: detach what we just attached; no argument.
                unsafe { libc::ioctl(dev.as_raw_fd(), LOOP_CLR_FD as _) };
                return Err(e);
            }
            return Ok(path);
        }
        Err(last)
    }

    fn loop_detach(&self, dev: &str) -> io::Result<()> {
        let fd = open(dev, true)?;
        // SAFETY: LOOP_CLR_FD takes no argument.
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), LOOP_CLR_FD as _) };
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    fn make_fs(&self, dev: &str, label: &str) -> io::Result<()> {
        let prog = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().find(|p| Path::new(p).exists()).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "mke2fs is not on this system"))?;
        let out = std::process::Command::new(prog)
            .args(["-t", "ext4", "-F", "-q", "-m", "0", "-L", label, "-E", "nodiscard,lazy_itable_init=1,lazy_journal_init=1", dev])
            .stdin(std::process::Stdio::null())
            .output()?;
        if out.status.success() { Ok(()) } else { Err(io::Error::other(format!("mke2fs failed: {}", String::from_utf8_lossy(&out.stderr).trim()))) }
    }

    fn mount(&self, dev: &str, target: &Path) -> io::Result<()> {
        let (s, t) = (CString::new(dev).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?, CString::new(target.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?);
        let (f, d) = (CString::new("ext4").unwrap(), CString::new("").unwrap());
        // SAFETY: all pointers are valid NUL-terminated strings for the duration of the call.
        let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV, d.as_ptr().cast()) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    fn umount(&self, target: &Path) -> io::Result<()> {
        let t = CString::new(target.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: valid NUL-terminated path.
        let rc = unsafe { libc::umount(t.as_ptr()) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    fn is_mounted(&self, target: &Path) -> bool {
        let want = target.display().to_string();
        crate::mounts_text().lines().any(|l| l.split_whitespace().nth(1).map(|m| m.replace("\\040", " ")).as_deref() == Some(want.as_str()))
    }
}
