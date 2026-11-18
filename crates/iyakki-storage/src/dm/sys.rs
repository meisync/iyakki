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

/// The loop devices we attached, with the handle we attached them through. A loop device set to auto-clear is cleared the moment its
/// last handle closes, and device-mapper has not opened it yet when `loop_attach` returns: without this the file came off at once
/// and the encrypted device was 0 sectors long ("attempt to access beyond end of device loop1", found on the Mac mini 2018).
static HELD: std::sync::Mutex<Vec<(String, OwnedFd)>> = std::sync::Mutex::new(Vec::new());

fn hold(path: &str, fd: OwnedFd) {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push((path.to_string(), fd));
}

/// Let go of our handle on `path` (the device clears itself once nothing else holds it). Returns how many handles were let go.
fn release(path: &str) -> usize {
    let mut h = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let before = h.len();
    h.retain(|(p, _)| p != path);
    before - h.len()
}

/// What to do with the device node of a mapping after a successful ioctl.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NodeChange {
    /// The mapping was created: make `/dev/mapper/<name>` for the device number the kernel answered with.
    Make { name: String, dev: u64 },
    /// The mapping was removed: take its node away.
    Drop { name: String },
}

/// The node work a finished `cmd` asks for, read from the buffer (`dev` is at byte 40 and `name` at 48, as in `struct dm_ioctl`).
pub(crate) fn node_change(cmd: DmCmd, buf: &[u8]) -> Option<NodeChange> {
    if buf.len() < 176 {
        return None;
    }
    let name = {
        let raw = &buf[48..176];
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).to_string()
    };
    // a name that could be a path is never turned into one
    if name.is_empty() || name.contains('/') || name == "." || name == ".." || name == "control" {
        return None;
    }
    match cmd {
        DmCmd::DevCreate => {
            let dev = u64::from_le_bytes(buf[40..48].try_into().ok()?);
            Some(NodeChange::Make { name, dev })
        }
        DmCmd::DevRemove => Some(NodeChange::Drop { name }),
        _ => None,
    }
}

fn make_node(name: &str, dev: u64) -> io::Result<()> {
    let path = format!("/dev/mapper/{name}");
    let c = CString::new(path.as_str()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // a node left by an earlier run, maybe for another device number, is replaced
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::unlink(c.as_ptr()) };
    // SAFETY: valid path; `dev` is the number the kernel gave for this mapping.
    let r = unsafe { libc::mknod(c.as_ptr(), libc::S_IFBLK | 0o600, dev as libc::dev_t) };
    if r < 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(e.kind(), format!("could not make the device node {path}: {e}")));
    }
    Ok(())
}

fn drop_node(name: &str) {
    if let Ok(c) = CString::new(format!("/dev/mapper/{name}")) {
        // SAFETY: valid NUL-terminated path; a node that is already gone is fine.
        unsafe { libc::unlink(c.as_ptr()) };
    }
}

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
        ioctl_ptr(&fd, cmd.request(), buf.as_mut_ptr().cast())?;
        // this system has no udev: nothing else makes /dev/mapper/<name>, which mke2fs and mount(2) need (found on the Mac mini
        // 2018: "The file /dev/mapper/iyakki-home-scratch does not exist")
        match node_change(cmd, buf) {
            Some(NodeChange::Make { name, dev }) => make_node(&name, dev),
            Some(NodeChange::Drop { name }) => {
                drop_node(&name);
                Ok(())
            }
            None => Ok(()),
        }
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
            hold(&path, dev);
            return Ok(path);
        }
        Err(last)
    }

    fn loop_detach(&self, dev: &str) -> io::Result<()> {
        let fd = open(dev, true)?;
        // SAFETY: LOOP_CLR_FD takes no argument.
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), LOOP_CLR_FD as _) };
        let result = if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) };
        // our own handle goes either way: the device clears itself when the last one closes
        release(dev);
        result
    }

    fn make_fs(&self, dev: &str, label: &str) -> io::Result<()> {
        let prog = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().find(|p| Path::new(p).exists()).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "mke2fs is not on this system"))?;
        let out = std::process::Command::new(prog)
            .args(["-t", "ext4", "-F", "-q", "-m", "0", "-L", label, "-E", "nodiscard,lazy_itable_init=1,lazy_journal_init=1", dev])
            .stdin(std::process::Stdio::null())
            .output()?;
        if out.status.success() { Ok(()) } else { Err(io::Error::other(format!("mke2fs failed: {}", String::from_utf8_lossy(&out.stderr).trim()))) }
    }

    #[cfg(not(target_os = "linux"))]
    fn mount(&self, _dev: &str, _target: &Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "mounting an encrypted volume is Linux only"))
    }

    #[cfg(target_os = "linux")]
    fn mount(&self, dev: &str, target: &Path) -> io::Result<()> {
        let (s, t) = (CString::new(dev).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?, CString::new(target.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?);
        let (f, d) = (CString::new("ext4").unwrap(), CString::new("").unwrap());
        // SAFETY: all pointers are valid NUL-terminated strings for the duration of the call.
        let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV, d.as_ptr().cast()) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    #[cfg(not(target_os = "linux"))]
    fn umount(&self, _target: &Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "unmounting an encrypted volume is Linux only"))
    }

    #[cfg(target_os = "linux")]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer as the kernel answers: `dev` at 40, `name` at 48.
    fn answer(name: &str, dev: u64) -> Vec<u8> {
        let mut b = vec![0u8; DM_IOCTL_LEN];
        b[40..48].copy_from_slice(&dev.to_le_bytes());
        b[48..48 + name.len()].copy_from_slice(name.as_bytes());
        b
    }

    #[test]
    fn an_attached_loop_device_is_held_until_it_is_detached() {
        // /dev/null stands in for a loop device handle
        let fd = open("/dev/null", false).unwrap();
        hold("/dev/loop-test-held", fd);
        assert!(HELD.lock().unwrap().iter().any(|(p, _)| p == "/dev/loop-test-held"), "held after attach");
        assert_eq!(release("/dev/loop-test-held"), 1, "let go at detach");
        assert_eq!(release("/dev/loop-test-held"), 0, "and only once");
    }

    #[test]
    fn a_created_mapping_gets_its_node_and_a_removed_one_loses_it_and_nothing_else_touches_nodes() {
        // found on the Mac mini 2018: no udev, so /dev/mapper/<name> never existed and mke2fs failed
        let dev = (252u64 << 8) | 3;
        assert_eq!(node_change(DmCmd::DevCreate, &answer("iyakki-home-amy", dev)), Some(NodeChange::Make { name: "iyakki-home-amy".into(), dev }));
        assert_eq!(node_change(DmCmd::DevRemove, &answer("iyakki-home-amy", 0)), Some(NodeChange::Drop { name: "iyakki-home-amy".into() }));
        for other in [DmCmd::Version, DmCmd::DevSuspend, DmCmd::DevStatus, DmCmd::TableLoad] {
            assert_eq!(node_change(other, &answer("iyakki-home-amy", dev)), None, "{other:?}");
        }
    }

    #[test]
    fn a_name_that_could_be_a_path_or_the_control_node_never_makes_or_drops_a_node() {
        for bad in ["", "..", ".", "control", "a/b", "../etc"] {
            assert_eq!(node_change(DmCmd::DevCreate, &answer(bad, 1)), None, "{bad:?}");
            assert_eq!(node_change(DmCmd::DevRemove, &answer(bad, 1)), None, "{bad:?}");
        }
        assert_eq!(node_change(DmCmd::DevCreate, &[0u8; 10]), None, "a short buffer");
    }
}
