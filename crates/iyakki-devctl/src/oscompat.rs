//! The few Linux-only libc names the Bluetooth code uses, with stand-ins on other systems. macOS has no Bluetooth sockets (`AF_BLUETOOTH`,
//! `/dev/net/tun`), so the stand-ins are never reached there: they only let the desktop window compile for macOS (`./run.sh check-macos`).

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use libc::{EBADFD, SOCK_CLOEXEC, SOCK_NONBLOCK};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub const SOCK_CLOEXEC: i32 = 0;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub const SOCK_NONBLOCK: i32 = 0;
/// No such error number outside Linux: -1 never equals an errno.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub const EBADFD: i32 = -1;

/// The request type of `ioctl` (it differs between libc targets).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub type Ioctl = libc::Ioctl;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub type Ioctl = libc::c_ulong;

/// `accept4` where it exists; elsewhere `accept` and then the flags set by hand.
///
/// # Safety
/// `addr` and `len` must be valid for the kernel to write, as for `accept`.
pub unsafe fn accept4(fd: i32, addr: *mut libc::sockaddr, len: *mut libc::socklen_t, flags: i32) -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        libc::accept4(fd, addr, len, flags)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let n = libc::accept(fd, addr, len);
        if n >= 0 && flags != 0 {
            libc::fcntl(n, libc::F_SETFD, libc::FD_CLOEXEC);
            let fl = libc::fcntl(n, libc::F_GETFL);
            libc::fcntl(n, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        n
    }
}

/// Fill `b` with random bytes from the system (getrandom on Linux, /dev/urandom elsewhere). False when nothing could be read.
pub fn random_bytes(b: &mut [u8]) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: b is valid for its length
        unsafe { libc::getrandom(b.as_mut_ptr() as *mut libc::c_void, b.len(), 0) == b.len() as isize }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(b)).is_ok()
    }
}
