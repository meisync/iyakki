//! The few Linux-only system calls and constants the agent uses, with stand-ins on other systems.
//!
//! macOS (the desktop window, `./run.sh window`) has no `mount(2)` with Linux flags, no pipe-size `fcntl`, no `sched_setscheduler`, no
//! `SO_PEERCRED`. On those systems the stand-ins fail with `ENOSYS` (or the closest honest answer) and [`UNAVAILABLE`] is the words
//! the person sees; real Linux builds call libc directly and behave exactly as before.

/// What the person is told when a Linux-only feature is used on another system.
pub const UNAVAILABLE: &str = "not available on this platform (it needs Linux)";

#[cfg(target_os = "linux")]
pub use libc::{F_GETPIPE_SZ, F_SETPIPE_SZ, MNT_DETACH, MS_BIND, MS_NODEV, MS_NOEXEC, MS_NOSUID, MS_RDONLY, MS_REMOUNT};
#[cfg(not(target_os = "linux"))]
pub const MS_RDONLY: libc::c_ulong = 1;
#[cfg(not(target_os = "linux"))]
pub const MS_NOSUID: libc::c_ulong = 2;
#[cfg(not(target_os = "linux"))]
pub const MS_NODEV: libc::c_ulong = 4;
#[cfg(not(target_os = "linux"))]
pub const MS_NOEXEC: libc::c_ulong = 8;
#[cfg(not(target_os = "linux"))]
pub const MS_REMOUNT: libc::c_ulong = 32;
#[cfg(not(target_os = "linux"))]
pub const MS_BIND: libc::c_ulong = 4096;
#[cfg(not(target_os = "linux"))]
pub const MNT_DETACH: libc::c_int = 2;
/// Not a real `fcntl` command anywhere else: the call fails with `EINVAL`, which callers already treat as "keep the default size".
#[cfg(not(target_os = "linux"))]
pub const F_SETPIPE_SZ: libc::c_int = -1;
#[cfg(not(target_os = "linux"))]
pub const F_GETPIPE_SZ: libc::c_int = -2;

#[cfg(not(target_os = "linux"))]
fn fail(e: libc::c_int) -> libc::c_int {
    // SAFETY: the errno location is thread-local and always valid.
    unsafe { *libc::__error() = e };
    -1
}

/// `mount(2)`; outside Linux it fails with `ENOSYS`.
///
/// # Safety
/// The pointers must be valid NUL-terminated strings (or null where Linux allows it), as for `libc::mount`.
pub unsafe fn mount(src: *const libc::c_char, target: *const libc::c_char, fstype: *const libc::c_char, flags: libc::c_ulong, data: *const libc::c_void) -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        libc::mount(src, target, fstype, flags, data)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (src, target, fstype, flags, data);
        fail(libc::ENOSYS)
    }
}

/// `umount2(2)`; outside Linux it fails with `ENOSYS`.
///
/// # Safety
/// `target` must be a valid NUL-terminated path.
pub unsafe fn umount2(target: *const libc::c_char, flags: libc::c_int) -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        libc::umount2(target, flags)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (target, flags);
        fail(libc::ENOSYS)
    }
}

/// `umount(2)`; outside Linux it fails with `ENOSYS`.
///
/// # Safety
/// `target` must be a valid NUL-terminated path.
pub unsafe fn umount(target: *const libc::c_char) -> libc::c_int {
    umount2(target, 0)
}

/// `sched_setscheduler(2)`; outside Linux it fails with `ENOSYS` (the caller keeps the normal scheduling).
///
/// # Safety
/// `param` must point to a valid `sched_param`.
pub unsafe fn sched_setscheduler(pid: libc::pid_t, policy: libc::c_int, param: *const libc::sched_param) -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        libc::sched_setscheduler(pid, policy, param)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, policy, param);
        fail(libc::ENOSYS)
    }
}

/// The name of the slave side of a pseudo terminal (`ptsname_r` on Linux, `ptsname` elsewhere). Empty on failure.
///
/// # Safety
/// `fd` must be a pseudo-terminal master.
pub unsafe fn pts_name(fd: libc::c_int) -> std::io::Result<std::ffi::CString> {
    #[cfg(target_os = "linux")]
    {
        let mut name = [0 as libc::c_char; 128];
        if libc::ptsname_r(fd, name.as_mut_ptr(), name.len()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::ffi::CStr::from_ptr(name.as_ptr()).to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let p = libc::ptsname(fd);
        if p.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::ffi::CStr::from_ptr(p).to_owned())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn the_stand_ins_fail_with_enosys() {
        let p = std::ffi::CString::new("/nonexistent").unwrap();
        assert_eq!(unsafe { super::umount(p.as_ptr()) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENOSYS));
    }

    #[test]
    fn the_words_for_the_person_are_plain() {
        assert!(super::UNAVAILABLE.contains("not available") && super::UNAVAILABLE.contains("Linux"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_the_names_are_libc_s() {
        assert_eq!(super::MS_RDONLY, libc::MS_RDONLY);
        assert_eq!(super::F_SETPIPE_SZ, libc::F_SETPIPE_SZ);
    }
}
