//! `shell` scope: run commands on the device from a paired computer (`iyakki dev shell`). The most dangerous power there is, so:
//!
//! * the computer must hold the `shell` scope AND an approved, time-limited GRANT ([`super::grants`]) for every session;
//! * a session ends when its grant ends, when the administrator disconnects it, when the computer is revoked or developer mode goes
//!   off, or after ten idle minutes; every command still running then is killed (its whole process group);
//! * commands run as the account's own system user (looked up in `/etc/passwd`) with a clean environment, in the account's home;
//!   **root only if the account is root and the grant says "as root"**; a process that is not root itself cannot change user and
//!   says so;
//! * every command line is written to the activity log and the audit log (credentials hidden), with the exit status when it ends.
//!
//! There is no terminal (no pty): the CLI tile has none either. Each line is one command (`sh -c`), its output streams back while
//! it runs, Ctrl+C sends it SIGINT, and the folder carries over from one line to the next. Environment changes do not.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_sec::policy::Policy;

use super::scopes::Scope;
use super::sessions;
use super::{DevKey, DevMode};

pub const MAX_COMMAND: usize = 8 * 1024;
/// Output kept per command (the oldest bytes go first; the reader is told how many were lost).
pub const MAX_OUTPUT: usize = 4 << 20;
pub const MAX_RUN_SECS: u64 = 3600;
pub const IDLE_SECS: u64 = 600;
pub const MAX_EXECS: usize = 4;
pub const MAX_STDIN: usize = 64 * 1024;
pub const MAX_WAIT_MS: u64 = 20_000;
pub const MAX_READ: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// None: stay as the process is (it is not root, or the account is root).
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub home: PathBuf,
    pub user: String,
    pub root: bool,
}

/// Who a command runs as. `passwd` is the text of `/etc/passwd`; `euid` the user of this process.
pub fn identity(account: &str, as_root_granted: bool, passwd: &str, euid: u32, default_home: &Path) -> Result<Identity, String> {
    if as_root_granted && account == "root" {
        return Ok(Identity { uid: None, gid: None, home: default_home.to_path_buf(), user: "root".into(), root: true });
    }
    let entry = passwd.lines().filter(|l| !l.starts_with('#')).map(|l| l.split(':').collect::<Vec<_>>()).find(|f| f.len() >= 6 && f[0] == account);
    match entry {
        Some(f) => {
            let (uid, gid): (u32, u32) = (f[2].parse().map_err(|_| "bad passwd entry")?, f[3].parse().map_err(|_| "bad passwd entry")?);
            if uid == 0 {
                return Err(format!("{account} is root on this device: commands run as root only when the grant says \"as root\" (ask with --root)"));
            }
            Ok(Identity { uid: (euid == 0).then_some(uid), gid: (euid == 0).then_some(gid), home: PathBuf::from(f[5]), user: account.into(), root: false })
        }
        // an IYAKKI account is not always a system user: as root the commands get the least-privileged user there is, never root
        None if euid == 0 => Ok(Identity { uid: Some(65534), gid: Some(65534), home: PathBuf::from("/tmp/iyakki-dev"), user: "nobody".into(), root: false }),
        // not running as root: nothing to drop; the process's own user it is
        None => Ok(Identity { uid: None, gid: None, home: default_home.to_path_buf(), user: account.into(), root: false }),
    }
}

struct Buf {
    data: Vec<u8>,
    /// Bytes dropped from the front.
    base: u64,
    done: bool,
}

struct Exec {
    id: u64,
    pgid: i32,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    buf: Mutex<Buf>,
    cv: Condvar,
    code: Mutex<Option<i32>>,
    started: Instant,
    killed: AtomicBool,
}

impl Exec {
    fn kill(&self, sig: i32) {
        if self.pgid > 1 {
            // SAFETY: kill(2) takes two integers; the negative pid addresses the process group this command leads.
            unsafe {
                libc::kill(-self.pgid, sig);
            }
        }
    }
}

struct Session {
    id: u64,
    computer: String,
    account: String,
    guard: sessions::Guard,
    ident: Identity,
    cwd: Mutex<PathBuf>,
    execs: Mutex<HashMap<u64, Arc<Exec>>>,
    last: Mutex<Instant>,
    next: AtomicU64,
    closed: AtomicBool,
}

fn registry() -> std::sync::MutexGuard<'static, HashMap<(PathBuf, u64), Arc<Session>>> {
    static R: OnceLock<Mutex<HashMap<(PathBuf, u64), Arc<Session>>>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn find(dm: &DevMode, sid: u64, computer: &str) -> Result<Arc<Session>, String> {
    let s = registry().get(&(dm.data_dir().to_path_buf(), sid)).cloned().filter(|s| s.computer == computer && !s.closed.load(Ordering::SeqCst));
    let s = s.ok_or("no such shell session (it ended: grant expired, disconnected, idle or closed)")?;
    *s.last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    Ok(s)
}

/// Open a session. Needs the scope (checked by the caller) and a valid grant.
pub fn open(dm: &DevMode, key: &DevKey, policy: &Policy, passwd: &str, euid: u32, home_fallback: &Path) -> Result<u64, String> {
    let g = dm.shell_grant(&key.name).filter(|g| g.kind == "shell").ok_or("no shell grant: ask for one first (the administrator approves on the device's screen)")?;
    if !policy.manage_settings {
        return Err(format!("the shell works only for an administrator account; {} is not one", key.account));
    }
    let ident = identity(&key.account, g.as_root, passwd, euid, home_fallback)?;
    let guard = sessions::open(dm.data_dir(), &key.name, &key.account, Scope::Shell, if ident.root { "shell (as root)" } else { "shell" }, dm.now())?;
    let cwd = if ident.home.is_dir() { ident.home.clone() } else { home_fallback.to_path_buf() };
    let sid = guard.id();
    let s = Arc::new(Session { id: sid, computer: key.name.clone(), account: key.account.clone(), guard, ident, cwd: Mutex::new(cwd), execs: Mutex::new(HashMap::new()), last: Mutex::new(Instant::now()), next: AtomicU64::new(1), closed: AtomicBool::new(false) });
    registry().insert((dm.data_dir().to_path_buf(), sid), s.clone());
    let dm2 = dm.clone();
    std::thread::Builder::new().name("iyakki-dev-shell-reaper".into()).spawn(move || reap(dm2, s)).map_err(|e| e.to_string())?;
    Ok(sid)
}

/// Watches one session: ends it (and kills what it runs) when its grant, its scope, the administrator, developer mode or idleness say so.
fn reap(dm: DevMode, s: Arc<Session>) {
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if s.closed.load(Ordering::SeqCst) {
            return;
        }
        let idle = s.last.lock().unwrap_or_else(|e| e.into_inner()).elapsed() > Duration::from_secs(IDLE_SECS) && s.execs.lock().unwrap_or_else(|e| e.into_inner()).values().all(|e| e.buf.lock().unwrap_or_else(|x| x.into_inner()).done);
        let key_ok = dm.keys().iter().any(|k| k.name == s.computer && k.has(Scope::Shell));
        let overlong = s.execs.lock().unwrap_or_else(|e| e.into_inner()).values().any(|e| e.started.elapsed() > Duration::from_secs(MAX_RUN_SECS) && !e.buf.lock().unwrap_or_else(|x| x.into_inner()).done);
        for e in s.execs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            if e.started.elapsed() > Duration::from_secs(MAX_RUN_SECS) && !e.killed.swap(true, Ordering::SeqCst) {
                e.kill(libc::SIGKILL);
            }
        }
        let _ = overlong;
        if s.guard.ended() || idle || !key_ok || !dm.is_on() || dm.shell_grant(&s.computer).is_none() {
            end(&dm, &s, if idle { "idle" } else { "grant ended, disconnected or revoked" });
            return;
        }
    }
}

fn end(dm: &DevMode, s: &Arc<Session>, why: &str) {
    if s.closed.swap(true, Ordering::SeqCst) {
        return;
    }
    for e in s.execs.lock().unwrap_or_else(|e| e.into_inner()).values() {
        e.killed.store(true, Ordering::SeqCst);
        e.kill(libc::SIGKILL);
    }
    registry().remove(&(dm.data_dir().to_path_buf(), s.id));
    dm.log(&s.computer, &s.account, "dev.shell.close", &format!("session {} ended ({why})", s.id), true);
    dm.shell_session_closed(&s.computer);
}

pub fn close(dm: &DevMode, sid: u64, computer: &str) -> Result<(), String> {
    let s = find(dm, sid, computer)?;
    end(dm, &s, "closed by the computer");
    Ok(())
}

/// The device nodes every ordinary command line uses (`> /dev/null`, `2>/dev/null`, `/dev/urandom`, `/dev/tty`): name, kernel major and
/// minor. They must be character devices that anybody may open for reading and writing (mode 0666).
pub const STD_NODES: [(&str, u32, u32); 6] = [("null", 1, 3), ("zero", 1, 5), ("full", 1, 7), ("random", 1, 8), ("urandom", 1, 9), ("tty", 5, 0)];

/// What a standard node is like now: `None` when it does not exist, else (is a character device, permission bits, major, minor).
pub type NodeNow = Option<(bool, u32, u32, u32)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeFix {
    /// Remove whatever is there (a plain file or a link) and make the character device again.
    Recreate,
    Chmod(u32),
}

/// What has to be done to one standard node so that a command running as an ordinary user can open it. A device that is right (0666,
/// the right numbers) needs nothing.
pub fn node_fix(want: (u32, u32), now: NodeNow) -> Vec<NodeFix> {
    match now {
        None => vec![NodeFix::Recreate],
        Some((false, ..)) => vec![NodeFix::Recreate],
        Some((true, _, maj, min)) if (maj, min) != want => vec![NodeFix::Recreate],
        Some((true, mode, ..)) if mode & 0o666 != 0o666 => vec![NodeFix::Chmod(0o666)],
        Some(_) => vec![],
    }
}

/// Make the standard device nodes of `dev` usable by every user (the data folder of an image started from RAM can have `/dev/null`
/// as root-only, which made every redirect of a shell command fail with "can't create /dev/null: Permission denied"). Needs root;
/// returns what it did and what it could not do, for the activity log. Does nothing for the nodes that are right.
pub fn ensure_std_nodes(dev: &Path) -> Vec<String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let mut notes = Vec::new();
    for (name, maj, min) in STD_NODES {
        let p = dev.join(name);
        let now: NodeNow = std::fs::symlink_metadata(&p).ok().map(|m| {
            let rdev = m.rdev();
            let (a, b) = (libc::major(rdev), libc::minor(rdev));
            (m.file_type().is_char_device(), m.mode() & 0o7777, a, b)
        });
        for fix in node_fix((maj, min), now) {
            let r: Result<(), String> = match fix {
                NodeFix::Chmod(m) => std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m)).map_err(|e| e.to_string()),
                NodeFix::Recreate => {
                    let _ = std::fs::remove_file(&p);
                    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string());
                    c.and_then(|c| {
                        // SAFETY: mknod(2) with a valid NUL-terminated path; it creates a character device node.
                        let rc = unsafe { libc::mknod(c.as_ptr(), libc::S_IFCHR | 0o666, libc::makedev(maj, min)) };
                        if rc != 0 {
                            return Err(std::io::Error::last_os_error().to_string());
                        }
                        // the process umask must not take the bits away again
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).map_err(|e| e.to_string())
                    })
                }
            };
            notes.push(match r {
                Ok(()) => format!("{}: {}", p.display(), match fix { NodeFix::Chmod(_) => "made usable by everyone", NodeFix::Recreate => "made again as a device" }),
                Err(e) => format!("{}: could not be fixed ({e})", p.display()),
            });
        }
    }
    notes
}

/// The environment a command of the shell starts with (nothing is inherited from the device's own process). `path` is the PATH.
pub fn command_env(home: &Path, user: &str) -> Vec<(&'static str, String)> {
    vec![
        ("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()),
        ("HOME", home.display().to_string()),
        ("USER", user.to_string()),
        ("LOGNAME", user.to_string()),
        ("SHELL", "/bin/sh".to_string()),
        ("TERM", "dumb".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("IYAKKI_DEV", "1".to_string()),
    ]
}

fn cwd_file_wrapper() -> &'static str {
    // $1 folder, $2 command line; the folder after the command goes to fd 3 so the next line starts there
    "cd -- \"$1\" 2>/dev/null; eval \"$2\"; rc=$?; pwd >&3 2>/dev/null; exit $rc"
}

/// Start one command line. Returns the command's id and the folder it started in.
pub fn exec(dm: &DevMode, sid: u64, computer: &str, line: &str) -> Result<(u64, PathBuf), String> {
    let s = find(dm, sid, computer)?;
    if line.trim().is_empty() || line.len() > MAX_COMMAND || line.contains('\0') {
        return Err(format!("a command is 1 to {MAX_COMMAND} bytes of text"));
    }
    {
        let mut ex = s.execs.lock().unwrap_or_else(|e| e.into_inner());
        ex.retain(|_, e| !e.buf.lock().unwrap_or_else(|x| x.into_inner()).done || e.started.elapsed() < Duration::from_secs(60));
        if ex.values().filter(|e| !e.buf.lock().unwrap_or_else(|x| x.into_inner()).done).count() >= MAX_EXECS {
            return Err(format!("{MAX_EXECS} commands are running already in this session"));
        }
    }
    let cwd = s.cwd.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (mut fds, home) = ([0i32; 2], s.ident.home.clone());
    // SAFETY: pipe(2) fills the two-element array with new descriptors, which are closed below or handed to the child.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("could not make a pipe".into());
    }
    let (r_fd, w_fd) = (fds[0], fds[1]);
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(cwd_file_wrapper()).arg("iyakki-dev").arg(&cwd).arg(line);
    // the device nodes a redirect needs, when this process may make them right (it runs the commands as another user)
    // SAFETY: geteuid(2) takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        for n in ensure_std_nodes(Path::new("/dev")) {
            dm.log(&s.computer, &s.account, "dev.shell.devnodes", &n, !n.contains("could not"));
        }
    }
    cmd.env_clear().envs(command_env(&home, &s.ident.user))
        .current_dir(if cwd.is_dir() { &cwd } else { Path::new("/") })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(u) = s.ident.uid {
        cmd.uid(u);
    }
    if let Some(g) = s.ident.gid {
        cmd.gid(g);
    }
    // SAFETY: runs in the forked child before exec and only calls async-signal-safe functions (dup2, close) on descriptors we made.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(w_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if w_fd != 3 {
                libc::close(w_fd);
            }
            libc::close(r_fd);
            Ok(())
        });
    }
    let spawned = cmd.spawn();
    // SAFETY: the write end is only needed in the child.
    unsafe {
        libc::close(w_fd);
    }
    let mut child: Child = match spawned {
        Ok(c) => c,
        Err(e) => {
            // SAFETY: closing the read end we still own.
            unsafe {
                libc::close(r_fd);
            }
            return Err(format!("could not start the command: {e}"));
        }
    };
    let id = s.next.fetch_add(1, Ordering::SeqCst);
    let pgid = child.id() as i32;
    let ex = Arc::new(Exec { id, pgid, stdin: Mutex::new(child.stdin.take()), buf: Mutex::new(Buf { data: Vec::new(), base: 0, done: false }), cv: Condvar::new(), code: Mutex::new(None), started: Instant::now(), killed: AtomicBool::new(false) });
    let mut readers = Vec::new();
    for mut src in [child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), child.stderr.take().map(|o| Box::new(o) as Box<dyn Read + Send>)].into_iter().flatten() {
        let e = ex.clone();
        readers.push(std::thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            while let Ok(n) = src.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut b = e.buf.lock().unwrap_or_else(|x| x.into_inner());
                b.data.extend_from_slice(&chunk[..n]);
                if b.data.len() > MAX_OUTPUT {
                    let cut = b.data.len() - MAX_OUTPUT;
                    b.data.drain(..cut);
                    b.base += cut as u64;
                }
                e.cv.notify_all();
            }
        }));
    }
    let (e2, s2, dm2, line_len) = (ex.clone(), s.clone(), dm.clone(), line.len());
    std::thread::spawn(move || {
        let status = child.wait();
        for r in readers {
            let _ = r.join();
        }
        // the folder the line ended in
        let mut path = String::new();
        // SAFETY: r_fd is the read end made above and owned by this thread; File takes ownership and closes it.
        let _ = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(r_fd) }.take(4096).read_to_string(&mut path);
        let p = PathBuf::from(path.trim());
        if p.is_absolute() && p.is_dir() {
            *s2.cwd.lock().unwrap_or_else(|e| e.into_inner()) = p;
        }
        let code = status.ok().map(|st| {
            use std::os::unix::process::ExitStatusExt;
            st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0))
        });
        *e2.code.lock().unwrap_or_else(|e| e.into_inner()) = code.or(Some(-1));
        e2.buf.lock().unwrap_or_else(|e| e.into_inner()).done = true;
        e2.cv.notify_all();
        dm2.log(&s2.computer, &s2.account, "dev.shell.exit", &format!("session {} command {} ({line_len} bytes) exit {} after {} s{}", s2.id, e2.id, code.unwrap_or(-1), e2.started.elapsed().as_secs(), if e2.killed.load(Ordering::SeqCst) { " (stopped)" } else { "" }), true);
    });
    s.execs.lock().unwrap_or_else(|e| e.into_inner()).insert(id, ex);
    Ok((id, cwd))
}

fn exec_of(s: &Session, id: u64) -> Result<Arc<Exec>, String> {
    s.execs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| format!("no command {id} in this session"))
}

/// Output of a command from byte `since`, waiting up to `wait_ms` for some. Returns (bytes, next, lost, done, exit code).
pub fn read(dm: &DevMode, sid: u64, computer: &str, id: u64, since: u64, wait_ms: u64) -> Result<(Vec<u8>, u64, u64, bool, Option<i32>), String> {
    let s = find(dm, sid, computer)?;
    let e = exec_of(&s, id)?;
    let deadline = Instant::now() + Duration::from_millis(wait_ms.min(MAX_WAIT_MS));
    let mut b = e.buf.lock().unwrap_or_else(|x| x.into_inner());
    loop {
        let end = b.base + b.data.len() as u64;
        if since < end || b.done || Instant::now() >= deadline {
            let from = since.max(b.base);
            let lost = from - since;
            let take = ((end - from) as usize).min(MAX_READ);
            let off = (from - b.base) as usize;
            let out = b.data[off..off + take].to_vec();
            let next = from + take as u64;
            let done = b.done && next >= end;
            let code = if done { *e.code.lock().unwrap_or_else(|x| x.into_inner()) } else { None };
            return Ok((out, next, lost, done, code));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        b = e.cv.wait_timeout(b, left).unwrap_or_else(|x| x.into_inner()).0;
    }
}

pub fn write_stdin(dm: &DevMode, sid: u64, computer: &str, id: u64, data: &[u8], eof: bool) -> Result<(), String> {
    if data.len() > MAX_STDIN {
        return Err(format!("at most {} KiB of input at a time", MAX_STDIN / 1024));
    }
    let s = find(dm, sid, computer)?;
    let e = exec_of(&s, id)?;
    let mut g = e.stdin.lock().unwrap_or_else(|x| x.into_inner());
    if let Some(w) = g.as_mut() {
        if !data.is_empty() {
            w.write_all(data).map_err(|e| e.to_string())?;
            let _ = w.flush();
        }
    }
    if eof {
        *g = None;
    }
    Ok(())
}

/// `int` (Ctrl+C), `term` or `kill` to the whole process group of a command.
pub fn signal(dm: &DevMode, sid: u64, computer: &str, id: u64, sig: &str) -> Result<(), String> {
    let s = find(dm, sid, computer)?;
    let e = exec_of(&s, id)?;
    let n = match sig {
        "int" => libc::SIGINT,
        "term" => libc::SIGTERM,
        "kill" => libc::SIGKILL,
        other => return Err(format!("unknown signal {other:?}: int, term or kill")),
    };
    e.killed.store(true, Ordering::SeqCst);
    e.kill(n);
    Ok(())
}

pub fn cwd(dm: &DevMode, sid: u64, computer: &str) -> Result<PathBuf, String> {
    Ok(find(dm, sid, computer)?.cwd.lock().unwrap_or_else(|e| e.into_inner()).clone())
}
