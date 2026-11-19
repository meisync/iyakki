//! A program on a pseudo-terminal (Linux and macOS): the master side is read without blocking, so the owner can poll it on a tick; the child
//! has the slave as its controlling terminal in a session of its own, so Ctrl+C reaches it and `top` or `vi` see a real terminal.
//! Calls: `posix_openpt`, `grantpt`, `unlockpt`, `ptsname_r` (`ptsname` behind a lock on macOS, which has no `_r`), `fcntl`, `ioctl` (`TIOCSWINSZ`, `TIOCSCTTY`), `setsid`, `poll`, `read`,
//! `write`, `kill`. The listed boundary row in BOUNDARY.md says why.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// What a read found.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    /// Bytes were added (maybe none).
    Data,
    /// The program closed its side: nothing more will come.
    Closed,
}

/// Where and as whom a program starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnOpts {
    pub cwd: Option<std::path::PathBuf>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

pub struct Pty {
    master: OwnedFd,
    child: Child,
}

fn last_err<T>() -> io::Result<T> {
    Err(io::Error::last_os_error())
}

/// The path of the slave side. Linux has the thread-safe `ptsname_r`; macOS only `ptsname`, which keeps its answer in a static
/// buffer, so the call and the copy happen under one lock.
#[cfg(target_os = "linux")]
fn slave_path(fd: libc::c_int) -> io::Result<String> {
    let mut buf = [0 as libc::c_char; 128];
    // SAFETY: ptsname_r writes at most `buf.len()` bytes into a buffer of that size; the descriptor is a pty master this caller owns.
    if unsafe { libc::ptsname_r(fd, buf.as_mut_ptr(), buf.len()) } != 0 {
        return last_err();
    }
    // SAFETY: ptsname_r wrote a NUL-terminated string into `buf`.
    Ok(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
fn slave_path(fd: libc::c_int) -> io::Result<String> {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: ptsname returns a pointer into a static buffer or NULL; the string is copied before the lock is released.
    let p = unsafe { libc::ptsname(fd) };
    if p.is_null() {
        return last_err();
    }
    // SAFETY: non-NULL and NUL-terminated, see above.
    Ok(unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
}

impl Pty {
    /// Start `program` with `args` on a new terminal of `cols` x `rows`. The child gets exactly the environment given (no more).
    pub fn spawn(program: &str, args: &[String], env: &[(String, String)], cols: u16, rows: u16) -> io::Result<Pty> {
        Pty::spawn_with(program, args, env, cols, rows, &SpawnOpts::default())
    }

    /// As [`Pty::spawn`], with the folder to start in and the user and group to run as (a core running as root starts a person's
    /// terminal as that person, never as itself).
    pub fn spawn_with(program: &str, args: &[String], env: &[(String, String)], cols: u16, rows: u16, opts: &SpawnOpts) -> io::Result<Pty> {
        // SAFETY: plain libc calls on descriptors this function creates and owns; every error is checked.
        let (master, slave_path) = unsafe {
            // macOS accepts only O_RDWR and O_NOCTTY here: close-on-exec is set afterwards
            #[cfg(target_os = "linux")]
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            #[cfg(not(target_os = "linux"))]
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            if fd < 0 {
                return last_err();
            }
            let master = OwnedFd::from_raw_fd(fd);
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                return last_err();
            }
            if libc::grantpt(fd) != 0 || libc::unlockpt(fd) != 0 {
                return last_err();
            }
            let path = slave_path(fd)?;
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return last_err();
            }
            (master, path)
        };
        let slave = std::fs::OpenOptions::new().read(true).write(true).open(&slave_path)?;
        // the size is set on the slave once it is open (macOS refuses it on a master whose slave nobody has opened yet)
        let ws = winsize(cols, rows);
        // SAFETY: TIOCSWINSZ with a pointer to a winsize on the slave descriptor this function just opened.
        if unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } != 0 {
            return last_err();
        }
        let mut cmd = Command::new(program);
        cmd.args(args).env_clear().envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if let Some(d) = &opts.cwd {
            cmd.current_dir(d);
        }
        if let Some(g) = opts.gid {
            cmd.gid(g);
        }
        if let Some(u) = opts.uid {
            // the terminal device is the person's, so a program running as them may open it again (/dev/tty)
            // SAFETY: fchown on the slave descriptor opened above; -1 leaves the group as it is.
            unsafe { libc::fchown(slave.as_raw_fd(), u, opts.gid.unwrap_or(u32::MAX)) };
            cmd.uid(u);
        }
        cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        // SAFETY: setsid and ioctl are async-signal-safe and the only things done between fork and exec; fd 0 is the slave by now.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                // a program started in the background by a non-interactive shell (nohup, a CI runner) has SIGINT and SIGQUIT ignored, and an ignored
                // signal survives exec: the terminal command must get the normal ones, or Ctrl+C would not stop it (found 2026-11-13 on a build host)
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        Ok(Pty { master, child })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Append what the program has written so far to `out`, without waiting. At most 256 KB per call, so one noisy program cannot
    /// hold the caller for long.
    pub fn read_available(&mut self, out: &mut Vec<u8>) -> io::Result<Read> {
        let mut chunk = [0u8; 8192];
        let mut total = 0;
        while total < 256 * 1024 {
            // SAFETY: reads into a buffer of the stated length from a descriptor this struct owns.
            let n = unsafe { libc::read(self.master.as_raw_fd(), chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
            if n > 0 {
                out.extend_from_slice(&chunk[..n as usize]);
                total += n as usize;
                continue;
            }
            if n == 0 {
                return Ok(Read::Closed);
            }
            let e = io::Error::last_os_error();
            return match e.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(Read::Data),
                _ if e.raw_os_error() == Some(libc::EIO) => Ok(Read::Closed), // Linux: the slave side is gone
                _ => Err(e),
            };
        }
        Ok(Read::Data)
    }

    /// Send bytes to the program. Waits at most about a quarter of a second for a program that does not read.
    pub fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        let mut waited = 0;
        while !bytes.is_empty() {
            // SAFETY: writes from a slice of the stated length to a descriptor this struct owns.
            let n = unsafe { libc::write(self.master.as_raw_fd(), bytes.as_ptr() as *const libc::c_void, bytes.len()) };
            if n > 0 {
                bytes = &bytes[n as usize..];
                continue;
            }
            let e = io::Error::last_os_error();
            if n < 0 && matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                if waited >= 250 {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the program is not reading"));
                }
                let mut p = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
                // SAFETY: one valid pollfd.
                unsafe { libc::poll(&mut p, 1, 25) };
                waited += 25;
                continue;
            }
            return Err(e);
        }
        Ok(())
    }

    /// Tell the terminal its new size; the program gets SIGWINCH from the kernel.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let ws = winsize(cols, rows);
        // SAFETY: TIOCSWINSZ with a pointer to a winsize on a descriptor this struct owns.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } != 0 {
            return last_err();
        }
        Ok(())
    }

    /// `Some(code)` once the program has ended (128 + signal when a signal ended it), else `None`.
    pub fn exit_code(&mut self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        match self.child.try_wait() {
            Ok(Some(s)) => Some(s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))),
            _ => None,
        }
    }

    /// Hang up the program (SIGHUP to its process group) and wait for it.
    pub fn close(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // SAFETY: signals the process group of the child this struct started as a session leader.
            unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGHUP) };
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
    }
}
