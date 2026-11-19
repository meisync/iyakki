//! The local socket's two extras (F-SPLIT-03, docs/development/UI-BOUNDARY.md section 14): who is on the other end of the unix socket
//! (`SO_PEERCRED`, read from the kernel, never from what the peer says) and the hand-over of BULK data (display lists, audio blocks,
//! camera frames) as an anonymous memory file passed over the same socket (`memfd_create` plus `SCM_RIGHTS`), so a big block is not copied
//! through the line transport. The design decision is a unix domain socket, not D-Bus.
//!
//! Linux only (`memfd_create`); on other unix systems the peer check works and the memory file calls answer `Unsupported`.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Who the kernel says is on the other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// Read the peer's credentials from the kernel (`SO_PEERCRED` on Linux, `LOCAL_PEERCRED` and `LOCAL_PEERPID` elsewhere are not used: the
/// program is for Linux, the macOS build only compiles it).
#[cfg(target_os = "linux")]
pub fn peer_of(s: &UnixStream) -> io::Result<Peer> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for the call and `len` is its size.
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer { pid: cred.pid, uid: cred.uid, gid: cred.gid })
}

#[cfg(not(target_os = "linux"))]
pub fn peer_of(_s: &UnixStream) -> io::Result<Peer> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "peer credentials are read on Linux only"))
}

/// May this peer talk to the core's UI socket? Root and the one user the UI program runs as (the core says which) are let in; nobody else.
pub fn peer_allowed(p: &Peer, ui_uid: u32) -> bool {
    p.uid == 0 || p.uid == ui_uid
}

/// The largest bulk block handed over in one memory file (a 4K frame is 33 MB; more is refused).
pub const MAX_BULK: usize = 64 * 1024 * 1024;

/// A sealed block of bytes in an anonymous memory file.
#[cfg(target_os = "linux")]
pub fn make_bulk(name: &str, bytes: &[u8]) -> io::Result<OwnedFd> {
    use std::io::Write;
    if bytes.len() > MAX_BULK {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "that block is too large for a hand-over"));
    }
    let cname = std::ffi::CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name has a NUL"))?;
    // SAFETY: a valid C string and flags; the result is checked.
    let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a new descriptor nobody else owns.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut f = std::fs::File::from(owned.try_clone()?);
    f.write_all(bytes)?;
    // seal it: the receiver reads exactly what was written and the sender cannot change it afterwards
    // SAFETY: fcntl on our own descriptor.
    let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL;
    if unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(owned)
}

#[cfg(not(target_os = "linux"))]
pub fn make_bulk(_name: &str, _bytes: &[u8]) -> io::Result<OwnedFd> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "memory files are Linux only"))
}

/// Read a received memory file back into bytes (at most `MAX_BULK`).
pub fn read_bulk(fd: OwnedFd) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::from(fd);
    let len = f.seek(SeekFrom::End(0))?;
    if len as usize > MAX_BULK {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the block is larger than a hand-over may be"));
    }
    f.seek(SeekFrom::Start(0))?;
    let mut v = Vec::with_capacity(len as usize);
    f.read_to_end(&mut v)?;
    Ok(v)
}

/// Send one line of text and, with it, a descriptor (`SCM_RIGHTS`). The line says what the block is ("frame 1920x1080 xrgb"); the bytes travel in the file.
pub fn send_with_fd(s: &UnixStream, line: &str, fd: RawFd) -> io::Result<()> {
    let data = line.as_bytes();
    let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut buf = vec![0u8; space];
    // SAFETY: msghdr is plain data; the pointers stay valid for the sendmsg call below.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = buf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer is big enough for one descriptor (CMSG_SPACE) so the first header is inside it.
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping(&fd as *const RawFd as *const u8, libc::CMSG_DATA(c), std::mem::size_of::<RawFd>());
    }
    // SAFETY: `msg` points at live buffers.
    let n = unsafe { libc::sendmsg(s.as_raw_fd(), &msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != data.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "the line was not sent whole"));
    }
    Ok(())
}

/// Receive one line (up to `max` bytes) and the descriptor sent with it, if any. More than one descriptor is refused (the extras are closed).
pub fn recv_with_fd(s: &UnixStream, max: usize) -> io::Result<(String, Option<OwnedFd>)> {
    let mut data = vec![0u8; max];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    let space = unsafe { libc::CMSG_SPACE((4 * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut buf = vec![0u8; space];
    // SAFETY: plain data, filled in below.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = buf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: `msg` points at live buffers; MSG_CMSG_CLOEXEC keeps a received descriptor out of children.
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let n = unsafe { libc::recvmsg(s.as_raw_fd(), &mut msg, flags) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut got: Vec<OwnedFd> = Vec::new();
    // SAFETY: walk the control messages the kernel filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let bytes = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let p = libc::CMSG_DATA(c);
                for i in 0..bytes / std::mem::size_of::<RawFd>() {
                    let mut fd: RawFd = -1;
                    std::ptr::copy_nonoverlapping(p.add(i * std::mem::size_of::<RawFd>()), &mut fd as *mut RawFd as *mut u8, std::mem::size_of::<RawFd>());
                    got.push(OwnedFd::from_raw_fd(fd));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    data.truncate(n as usize);
    let line = String::from_utf8(data).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the line is not text"))?;
    if got.len() > 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "more than one descriptor came with the line"));
    }
    Ok((line, got.pop()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_peer_is_the_process_on_the_other_end_as_the_kernel_says() {
        let (a, b) = UnixStream::pair().unwrap();
        let p = peer_of(&a).unwrap();
        // SAFETY: getuid/getpid cannot fail.
        assert_eq!(p.pid, std::process::id() as i32);
        assert_eq!(p.uid, unsafe { libc::getuid() });
        assert_eq!(peer_of(&b).unwrap(), p);
    }

    #[test]
    fn only_root_and_the_ui_user_are_let_in() {
        let p = |uid| Peer { pid: 1, uid, gid: 0 };
        assert!(peer_allowed(&p(0), 1000));
        assert!(peer_allowed(&p(1000), 1000));
        assert!(!peer_allowed(&p(1001), 1000));
    }

    #[test]
    fn a_block_travels_as_a_sealed_memory_file_and_comes_back_equal() {
        let (a, b) = UnixStream::pair().unwrap();
        let block: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
        let fd = make_bulk("frame", &block).unwrap();
        send_with_fd(&a, "frame 1250x1000 xrgb\n", fd.as_raw_fd()).unwrap();
        let (line, got) = recv_with_fd(&b, 256).unwrap();
        assert_eq!(line, "frame 1250x1000 xrgb\n");
        assert_eq!(read_bulk(got.expect("a descriptor came with the line")).unwrap(), block);
    }

    #[test]
    fn a_sealed_block_cannot_be_written_to_by_the_receiver() {
        use std::io::Write;
        let fd = make_bulk("x", b"abc").unwrap();
        let mut f = std::fs::File::from(fd);
        assert!(f.write_all(b"zzz").is_err(), "the seal refuses writes");
    }

    #[test]
    fn a_line_without_a_descriptor_gives_none_and_a_block_over_the_limit_is_refused() {
        let (a, b) = UnixStream::pair().unwrap();
        use std::io::Write;
        (&a).write_all(b"hello\n").unwrap();
        let (line, fd) = recv_with_fd(&b, 64).unwrap();
        assert_eq!(line, "hello\n");
        assert!(fd.is_none());
        assert!(make_bulk("big", &vec![0u8; MAX_BULK + 1]).is_err());
    }
}
