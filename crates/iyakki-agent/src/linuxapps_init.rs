//! The program side of a Linux apps sandbox (`docs/LINUX-APPS.md`): `iyakki linuxapp-init <options> -- <command...>`.
//!
//! The manager (`linuxapps.rs`) never builds a shell line. It starts this program (IYAKKI itself, the same binary) with an
//! argument array; this code then does what `unshare`, `chroot` and `mount` would do, directly with system calls, so nothing in the
//! image has to provide those programs and no string is ever parsed by a shell:
//!
//! 1. `fork`; the child makes new **user, mount, pid, uts, ipc** (and, with the network switch off, **network**) namespaces;
//! 2. the parent (still outside) writes the child's user id maps: a whole range when it runs as root (so `apt` and `dpkg` can
//!    change owners inside), or one id (the person's own) when it does not, and puts the child in a **cgroup** with the memory,
//!    cpu and process limits when cgroups v2 is there;
//! 3. the child (pid 1 of its namespace) makes the mounts private, **`pivot_root`s into the sandbox's folder** and detaches
//!    the old root, so the host's files are not reachable at all; it mounts `/proc` (its own pid namespace), a small `/dev` (six
//!    harmless nodes bound from the host: null, zero, full, random, urandom, tty, plus the sandbox's own `/dev/pts` instance), `/tmp` and `/dev/shm` as memory file systems,
//!    and only the folders the person chose, read-only or read-write; no `/sys`; the host's resolver file is copied in;
//! 4. it drops capabilities, switches to the sandbox user, forbids gaining privileges (`no_new_privs`), loads a **seccomp**
//!    deny-list (mount, ptrace, bpf, module loading, new namespaces, keyrings, ...) and runs the command.
//!
//! If the parent dies the child is killed (`PDEATHSIG`), and the whole namespace with it.

use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::Path;

pub const SUBCOMMAND: &str = "linuxapp-init";

/// The ids inside the sandbox and how they map to the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdMap {
    /// One id: the user running the manager is `uid`/`gid` inside (no root on the host needed; `chown` to other ids fails inside).
    Single,
    /// Ids `0..65536` inside are `base..base+65536` on the host (needs root on the host).
    Multi(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub src: String,
    pub dst: String,
    pub rw: bool,
}

/// Everything the child needs; [`Spec::to_args`] and [`Spec::parse`] are inverse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The sandbox's root folder on the host (absolute).
    pub root: String,
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub idmap: IdMap,
    /// Outbound network (the host's network is shared); off: an empty network namespace with only a loopback.
    pub net: bool,
    pub binds: Vec<Bind>,
    pub tmp_mb: u32,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub mem_mb: u32,
    pub cpu_pct: u32,
    pub pids: u32,
    pub seccomp: bool,
    pub cmd: Vec<String>,
}

impl Spec {
    pub fn new(root: &str, name: &str, cmd: Vec<String>) -> Spec {
        Spec { root: root.into(), name: name.into(), uid: 1000, gid: 1000, idmap: IdMap::Single, net: false, binds: vec![], tmp_mb: 256, cwd: "/".into(), env: vec![], mem_mb: 0, cpu_pct: 0, pids: 0, seccomp: true, cmd }
    }

    /// The argument array after the program name (`iyakki`).
    pub fn to_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![SUBCOMMAND.into(), "--root".into(), self.root.clone(), "--name".into(), self.name.clone(), "--uid".into(), self.uid.to_string(), "--gid".into(), self.gid.to_string()];
        a.push("--idmap".into());
        a.push(match self.idmap {
            IdMap::Single => "single".into(),
            IdMap::Multi(b) => format!("multi:{b}"),
        });
        a.push("--net".into());
        a.push(if self.net { "on" } else { "off" }.into());
        for b in &self.binds {
            a.push(if b.rw { "--bind-rw" } else { "--bind-ro" }.into());
            a.push(b.src.clone());
            a.push(b.dst.clone());
        }
        a.extend(["--tmp-mb".into(), self.tmp_mb.to_string(), "--cwd".into(), self.cwd.clone(), "--mem-mb".into(), self.mem_mb.to_string(), "--cpu-pct".into(), self.cpu_pct.to_string(), "--pids".into(), self.pids.to_string()]);
        a.push("--seccomp".into());
        a.push(if self.seccomp { "on" } else { "off" }.into());
        for (k, v) in &self.env {
            a.push("--env".into());
            a.push(format!("{k}={v}"));
        }
        a.push("--".into());
        a.extend(self.cmd.iter().cloned());
        a
    }

    /// Read the arguments after the program name (the first is [`SUBCOMMAND`]).
    pub fn parse(args: &[String]) -> Result<Spec, String> {
        let mut s = Spec::new("", "", vec![]);
        let mut i = 1;
        let val = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
        };
        let num = |v: String, what: &str| v.parse::<u32>().map_err(|_| format!("{what}: not a number: {v}"));
        while i < args.len() {
            match args[i].as_str() {
                "--" => {
                    s.cmd = args[i + 1..].to_vec();
                    break;
                }
                "--root" => s.root = val(&mut i)?,
                "--name" => s.name = val(&mut i)?,
                "--uid" => s.uid = num(val(&mut i)?, "--uid")?,
                "--gid" => s.gid = num(val(&mut i)?, "--gid")?,
                "--idmap" => {
                    let v = val(&mut i)?;
                    s.idmap = match v.strip_prefix("multi:") {
                        Some(b) => IdMap::Multi(num(b.to_string(), "--idmap")?),
                        None if v == "single" => IdMap::Single,
                        None => return Err(format!("--idmap: unknown value {v}")),
                    };
                }
                "--net" => s.net = val(&mut i)? == "on",
                "--seccomp" => s.seccomp = val(&mut i)? != "off",
                flag @ ("--bind-ro" | "--bind-rw") => {
                    let rw = flag == "--bind-rw";
                    let (src, dst) = (val(&mut i)?, val(&mut i)?);
                    s.binds.push(Bind { src, dst, rw });
                }
                "--tmp-mb" => s.tmp_mb = num(val(&mut i)?, "--tmp-mb")?,
                "--cwd" => s.cwd = val(&mut i)?,
                "--mem-mb" => s.mem_mb = num(val(&mut i)?, "--mem-mb")?,
                "--cpu-pct" => s.cpu_pct = num(val(&mut i)?, "--cpu-pct")?,
                "--pids" => s.pids = num(val(&mut i)?, "--pids")?,
                "--env" => {
                    let v = val(&mut i)?;
                    let (k, x) = v.split_once('=').ok_or("--env needs NAME=value")?;
                    s.env.push((k.to_string(), x.to_string()));
                }
                other => return Err(format!("unknown option {other}")),
            }
            i += 1;
        }
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.root.starts_with('/') || self.root.contains('\0') {
            return Err("the root folder must be an absolute path".into());
        }
        if self.cmd.is_empty() || self.cmd[0].is_empty() {
            return Err("no command to run".into());
        }
        if self.cmd.iter().any(|a| a.contains('\0')) {
            return Err("a NUL in the command".into());
        }
        if self.name.is_empty() || self.name.len() > 40 || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("the sandbox name is not valid".into());
        }
        if let IdMap::Multi(_) = self.idmap {
            if self.uid > 65535 || self.gid > 65535 {
                return Err("the user id is outside the mapped range".into());
            }
        }
        for b in &self.binds {
            if !b.src.starts_with('/') || b.src.contains('\0') {
                return Err(format!("mount source {} must be an absolute path", b.src));
            }
            bind_dst_ok(&b.dst)?;
        }
        if !self.cwd.starts_with('/') || self.cwd.contains('\0') {
            return Err("the working folder must be an absolute path".into());
        }
        for (k, _) in &self.env {
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) {
                return Err(format!("environment name {k} is not valid"));
            }
        }
        Ok(())
    }
}

/// Where a folder may be mounted inside the sandbox: an absolute path with only plain parts, not in the places the sandbox sets up itself.
pub fn bind_dst_ok(dst: &str) -> Result<(), String> {
    if !dst.starts_with('/') || dst == "/" || dst.contains('\0') {
        return Err(format!("the mount point {dst} must be an absolute path below /"));
    }
    for c in Path::new(dst).components() {
        match c {
            std::path::Component::RootDir | std::path::Component::Normal(_) => {}
            _ => return Err(format!("the mount point {dst} must not contain . or ..")),
        }
    }
    let first = dst.trim_start_matches('/').split('/').next().unwrap_or("");
    if matches!(first, "proc" | "dev" | "sys" | "tmp" | ".old" | "etc" | "usr" | "bin" | "sbin" | "lib" | "lib64" | "var" | "run" | "boot") {
        return Err(format!("{dst}: folders are mounted under /mnt, /home, /srv or /opt, not in system folders"));
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ seccomp

const AUDIT_ARCH: u32 = if cfg!(target_arch = "x86_64") { 0xC000_003E } else { 0xC000_00B7 };

/// System calls the sandbox may not make (they fail with EPERM). A deny-list: the programs of a Debian system keep working; what is
/// listed changes the host or opens a way around the sandbox.
pub fn denied_syscalls() -> Vec<i64> {
    let mut v: Vec<i64> = vec![
        libc::SYS_ptrace, libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_chroot, libc::SYS_reboot, libc::SYS_kexec_load,
        libc::SYS_init_module, libc::SYS_finit_module, libc::SYS_delete_module, libc::SYS_swapon, libc::SYS_swapoff, libc::SYS_bpf,
        libc::SYS_perf_event_open, libc::SYS_add_key, libc::SYS_request_key, libc::SYS_keyctl, libc::SYS_unshare, libc::SYS_setns,
        libc::SYS_open_by_handle_at, libc::SYS_name_to_handle_at, libc::SYS_acct, libc::SYS_settimeofday, libc::SYS_clock_settime,
        libc::SYS_clock_adjtime, libc::SYS_adjtimex, libc::SYS_userfaultfd, libc::SYS_syslog, libc::SYS_quotactl, libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev, libc::SYS_kcmp,
        // the new mount API (the same numbers on every 64-bit architecture)
        428, 429, 430, 431, 432, 433, 442,
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend([libc::SYS_iopl, libc::SYS_ioperm]);
    v.sort_unstable();
    v.dedup();
    v
}

/// The classic BPF program: a foreign architecture and every listed call return EPERM, everything else is allowed.
pub fn seccomp_program(denied: &[i64]) -> Vec<libc::sock_filter> {
    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15;
    const RET_K: u16 = 0x06;
    let ins = |code: u16, jt: u8, jf: u8, k: u32| libc::sock_filter { code, jt, jf, k };
    let eperm = 0x0005_0000 | libc::EPERM as u32; // SECCOMP_RET_ERRNO | errno
    let mut p = vec![ins(LD_W_ABS, 0, 0, 4), ins(JEQ_K, 1, 0, AUDIT_ARCH), ins(RET_K, 0, 0, eperm), ins(LD_W_ABS, 0, 0, 0)];
    let n = denied.len();
    for (i, nr) in denied.iter().enumerate() {
        // on a match jump to the final EPERM (the instructions that follow: the remaining checks, then ALLOW, then EPERM)
        p.push(ins(JEQ_K, (n - i) as u8, 0, *nr as u32));
    }
    p.push(ins(RET_K, 0, 0, 0x7fff_0000)); // ALLOW
    p.push(ins(RET_K, 0, 0, eperm));
    p
}

// ------------------------------------------------------------------------------------------------ the work

fn cs(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "a NUL in a path".to_string())
}

fn os(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

fn mnt(src: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: &str) -> Result<(), String> {
    let (s, t, f, d) = (cs(src)?, cs(target)?, cs(fstype)?, cs(data)?);
    // SAFETY: all four are valid NUL-terminated strings that live through the call.
    let r = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), flags, d.as_ptr() as *const libc::c_void) };
    if r != 0 { Err(format!("mount {target}: {}", std::io::Error::last_os_error())) } else { Ok(()) }
}

fn waitpid(pid: libc::pid_t) -> i32 {
    let mut st: libc::c_int = 0;
    loop {
        // SAFETY: `st` is a valid int.
        let r = unsafe { libc::waitpid(pid, &mut st, 0) };
        if r < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if r < 0 {
            return 125;
        }
        break;
    }
    if libc::WIFEXITED(st) {
        libc::WEXITSTATUS(st)
    } else if libc::WIFSIGNALED(st) {
        128 + libc::WTERMSIG(st)
    } else {
        125
    }
}

fn pipe() -> Result<(i32, i32), String> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` has room for two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(os("pipe"));
    }
    Ok((fds[0], fds[1]))
}

fn write_byte(fd: i32) {
    // SAFETY: one byte from a valid buffer.
    unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
}

fn read_byte(fd: i32) -> bool {
    let mut b = [0u8; 1];
    // SAFETY: one byte into a valid buffer.
    unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, 1) == 1 }
}

/// `iyakki linuxapp-init ...`: returns the command's exit status (125 when the sandbox itself could not be set up).
pub fn main(args: &[String]) -> i32 {
    let spec = match Spec::parse(args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("linuxapp-init: {e}");
            return 2;
        }
    };
    match run(&spec) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("linuxapp-init: {e}");
            125
        }
    }
}

fn run(spec: &Spec) -> Result<i32, String> {
    let (a_r, a_w) = pipe()?;
    let (b_r, b_w) = pipe()?;
    // SAFETY: fork in a process that has made no other thread of its own (the dispatch is the first thing `main` does).
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(os("fork"));
    }
    if pid == 0 {
        // the child never returns here
        let code = match child(spec, a_w, b_r) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("linuxapp-init: {e}");
                125
            }
        };
        // SAFETY: leave without running the parent's exit handlers.
        unsafe { libc::_exit(code) };
    }
    // the parent, outside the namespaces
    // SAFETY: closing descriptors this process opened.
    unsafe {
        libc::close(a_w);
        libc::close(b_r);
    }
    if !read_byte(a_r) {
        return Ok(waitpid(pid));
    }
    let setup = write_maps(spec, pid);
    let cg = match &setup {
        Ok(()) => cgroup_enter(spec, pid),
        Err(_) => None,
    };
    if let Err(e) = &setup {
        // SAFETY: stop the child that waits for its maps.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        waitpid(pid);
        return Err(e.clone());
    }
    write_byte(b_w);
    let code = waitpid(pid);
    if let Some(dir) = cg {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(code)
}

fn write_proc(pid: i32, file: &str, text: &str) -> Result<(), String> {
    std::fs::write(format!("/proc/{pid}/{file}"), text).map_err(|e| format!("could not write {file} of the sandbox ({e}); the kernel needs CONFIG_USER_NS and, for a user that is not root, user.max_user_namespaces above 0"))
}

fn write_maps(spec: &Spec, pid: i32) -> Result<(), String> {
    // SAFETY: plain getters.
    let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
    match spec.idmap {
        IdMap::Single => {
            write_proc(pid, "setgroups", "deny")?;
            write_proc(pid, "uid_map", &format!("{} {} 1\n", spec.uid, euid))?;
            write_proc(pid, "gid_map", &format!("{} {} 1\n", spec.gid, egid))
        }
        IdMap::Multi(base) => {
            write_proc(pid, "uid_map", &format!("0 {base} 65536\n"))?;
            write_proc(pid, "gid_map", &format!("0 {base} 65536\n"))
        }
    }
}

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Put the child in a cgroup with the limits (best effort: the message says when limits cannot be enforced). Returns the folder to remove.
fn cgroup_enter(spec: &Spec, pid: i32) -> Option<std::path::PathBuf> {
    if spec.mem_mb == 0 && spec.cpu_pct == 0 && spec.pids == 0 {
        return None;
    }
    // limits are set by the program that runs as root (a user that is not root cannot write cgroups; nothing is said then, the docs do)
    // SAFETY: a plain getter.
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let root = Path::new(CGROUP_ROOT);
    if !root.join("cgroup.controllers").is_file() {
        eprintln!("linuxapp-init: note: no cgroup v2 here, so memory, cpu and process limits are not enforced");
        return None;
    }
    let parent = root.join("iyakki-linuxapps");
    // leftovers of runs that were killed (a time limit) are removed when empty; a folder with processes in it cannot be removed and stays
    for e in std::fs::read_dir(&parent).into_iter().flatten().flatten() {
        let _ = std::fs::remove_dir(e.path());
    }
    let dir = parent.join(format!("{}-{pid}", spec.name));
    let r = (|| -> Result<(), String> {
        let _ = std::fs::write(root.join("cgroup.subtree_control"), "+memory +cpu +pids");
        std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let _ = std::fs::write(parent.join("cgroup.subtree_control"), "+memory +cpu +pids");
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        if spec.mem_mb > 0 {
            std::fs::write(dir.join("memory.max"), format!("{}", spec.mem_mb as u64 * 1024 * 1024)).map_err(|e| format!("memory.max: {e}"))?;
            let _ = std::fs::write(dir.join("memory.swap.max"), "0");
        }
        if spec.cpu_pct > 0 {
            std::fs::write(dir.join("cpu.max"), format!("{} 100000", spec.cpu_pct as u64 * 1000)).map_err(|e| format!("cpu.max: {e}"))?;
        }
        if spec.pids > 0 {
            std::fs::write(dir.join("pids.max"), spec.pids.to_string()).map_err(|e| format!("pids.max: {e}"))?;
        }
        std::fs::write(dir.join("cgroup.procs"), pid.to_string()).map_err(|e| format!("cgroup.procs: {e}"))
    })();
    match r {
        Ok(()) => Some(dir),
        Err(e) => {
            eprintln!("linuxapp-init: note: limits are not enforced ({e})");
            let _ = std::fs::remove_dir(&dir);
            None
        }
    }
}

fn child(spec: &Spec, a_w: i32, b_r: i32) -> Result<i32, String> {
    // SAFETY: prctl with plain integer arguments.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0);
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(os("no_new_privs"));
        }
    }
    let mut flags = libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID | libc::CLONE_NEWUTS | libc::CLONE_NEWIPC;
    if !spec.net {
        flags |= libc::CLONE_NEWNET;
    }
    // SAFETY: unshare takes only the flags.
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(format!("{}; this kernel may lack user, pid or network namespaces (CONFIG_USER_NS, CONFIG_PID_NS, CONFIG_NET_NS, CONFIG_UTS_NS, CONFIG_IPC_NS)", os("unshare")));
    }
    write_byte(a_w);
    if !read_byte(b_r) {
        return Ok(125);
    }
    // the next fork is pid 1 of the new pid namespace
    // SAFETY: as above.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(os("fork"));
    }
    if pid > 0 {
        return Ok(waitpid(pid));
    }
    let code = match init(spec) {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!("linuxapp-init: {e}");
            125
        }
    };
    // SAFETY: leave at once.
    unsafe { libc::_exit(code) }
}

enum Never {}

fn init(spec: &Spec) -> Result<Never, String> {
    // SAFETY: plain integer arguments.
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0) };
    mnt("", "/", "", libc::MS_REC | libc::MS_PRIVATE, "")?;
    let root = &spec.root;
    // the sandbox's folder must be a mount point of its own for pivot_root; nothing from it runs with set-user-id or as a device
    mnt(root, root, "", libc::MS_BIND | libc::MS_REC, "")?;
    mnt("", root, "", libc::MS_REMOUNT | libc::MS_BIND | libc::MS_NOSUID | libc::MS_NODEV | inherited_flags(root), "")?;
    let put_old = format!("{root}/.old");
    match std::fs::symlink_metadata(&put_old) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Ok(_) => return Err("the sandbox folder holds a file named .old; delete the sandbox and create it again".into()),
        Err(_) => std::fs::create_dir(&put_old).map_err(|e| format!("{put_old}: {e}"))?,
    }
    let (r, p) = (cs(root)?, cs(&put_old)?);
    // SAFETY: pivot_root(new_root, put_old) with valid strings.
    if unsafe { libc::syscall(libc::SYS_pivot_root, r.as_ptr(), p.as_ptr()) } != 0 {
        return Err(os("pivot_root"));
    }
    std::env::set_current_dir("/").map_err(|e| format!("chdir /: {e}"))?;
    // from here "/" is the sandbox; the host is at /.old until it is detached below, and a link inside the sandbox can only lead inside it
    let old = |p: &str| format!("/.old{p}");
    // /proc: this pid namespace's own
    std::fs::create_dir_all("/proc").map_err(|e| e.to_string())?;
    mnt("proc", "/proc", "proc", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "")?;
    // /dev: a memory file system with the six harmless nodes
    std::fs::create_dir_all("/dev").map_err(|e| e.to_string())?;
    mnt("tmpfs", "/dev", "tmpfs", libc::MS_NOSUID | libc::MS_NOEXEC, "mode=755,size=1m")?;
    for n in ["null", "zero", "full", "random", "urandom", "tty"] {
        let target = format!("/dev/{n}");
        std::fs::write(&target, b"").map_err(|e| format!("{target}: {e}"))?;
        mnt(&old(&format!("/dev/{n}")), &target, "", libc::MS_BIND, "")?;
    }
    std::fs::create_dir_all("/dev/shm").map_err(|e| e.to_string())?;
    mnt("tmpfs", "/dev/shm", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "mode=1777,size=64m")?;
    // pseudo-terminals: the sandbox's own devpts instance (`newinstance`: it shares nothing with the host's), so xterm, `script`, `ssh -t` and full-screen programs
    // can open one. Best effort: a kernel that refuses leaves the sandbox as before, without terminals.
    if std::fs::create_dir_all("/dev/pts").is_ok() && mnt("devpts", "/dev/pts", "devpts", libc::MS_NOSUID | libc::MS_NOEXEC, "newinstance,ptmxmode=0666,mode=0620").is_ok() {
        let _ = std::os::unix::fs::symlink("pts/ptmx", "/dev/ptmx");
    }
    for (l, t) in [("fd", "/proc/self/fd"), ("stdin", "/proc/self/fd/0"), ("stdout", "/proc/self/fd/1"), ("stderr", "/proc/self/fd/2")] {
        let _ = std::os::unix::fs::symlink(t, format!("/dev/{l}"));
    }
    // /tmp: memory, limited
    let _ = std::fs::remove_file("/tmp");
    std::fs::create_dir_all("/tmp").map_err(|e| e.to_string())?;
    mnt("tmpfs", "/tmp", "tmpfs", libc::MS_NOSUID | libc::MS_NODEV, &format!("mode=1777,size={}m", spec.tmp_mb.max(1)))?;
    // the folders the person chose
    for b in &spec.binds {
        std::fs::create_dir_all(&b.dst).map_err(|e| format!("mount point {}: {e}", b.dst))?;
        mnt(&old(&b.src), &b.dst, "", libc::MS_BIND | libc::MS_REC, "")?;
        let mut flags = libc::MS_REMOUNT | libc::MS_BIND | libc::MS_NOSUID | libc::MS_NODEV | inherited_flags(&b.dst);
        if !b.rw {
            flags |= libc::MS_RDONLY;
        }
        mnt("", &b.dst, "", flags, "")?;
    }
    // name resolution: the host's resolver file, copied (a link inside the sandbox is replaced by a plain file)
    if spec.net {
        if let Ok(text) = std::fs::read(old("/etc/resolv.conf")) {
            let _ = std::fs::remove_file("/etc/resolv.conf");
            let _ = std::fs::create_dir_all("/etc");
            let _ = std::fs::write("/etc/resolv.conf", text);
        }
    }
    // the host disappears
    let o = cs("/.old")?;
    // SAFETY: valid path.
    if unsafe { libc::umount2(o.as_ptr(), libc::MNT_DETACH) } != 0 {
        return Err(os("umount /.old"));
    }
    let _ = std::fs::remove_dir("/.old");
    // SAFETY: name pointer and length from a valid string.
    unsafe { libc::sethostname(spec.name.as_ptr() as *const libc::c_char, spec.name.len()) };
    if !spec.net {
        loopback_up();
    }
    drop_caps(spec.uid == 0);
    // SAFETY: plain ids.
    unsafe {
        libc::setgroups(0, std::ptr::null());
        if libc::setresgid(spec.gid, spec.gid, spec.gid) != 0 {
            return Err(os("setgid"));
        }
        if libc::setresuid(spec.uid, spec.uid, spec.uid) != 0 {
            return Err(os("setuid"));
        }
    }
    if spec.seccomp {
        let prog = seccomp_program(&denied_syscalls());
        let fprog = libc::sock_fprog { len: prog.len() as u16, filter: prog.as_ptr() as *mut libc::sock_filter };
        // SAFETY: `fprog` points at `prog`, which lives through the call.
        if unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER as libc::c_ulong, &fprog as *const libc::sock_fprog as libc::c_ulong, 0, 0) } != 0 {
            return Err(os("seccomp"));
        }
    }
    let cwd = if std::path::Path::new(&spec.cwd).is_dir() { spec.cwd.as_str() } else { "/" };
    let mut cmd = std::process::Command::new(&spec.cmd[0]);
    cmd.args(&spec.cmd[1..]).current_dir(cwd).env_clear();
    cmd.env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").env("TERM", "xterm").env("LANG", "C.UTF-8").env("HOSTNAME", &spec.name);
    cmd.env("HOME", if spec.uid == 0 { "/root".to_string() } else { "/home/user".to_string() });
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let e = cmd.exec();
    eprintln!("linuxapp-init: could not run {}: {e}", spec.cmd[0]);
    // SAFETY: leave at once.
    unsafe { libc::_exit(127) }
}

/// The mount flags the kernel locks when a namespace is copied from an unprivileged user (read-only, no set-user-id, no devices, no
/// exec, and the access-time flags): a remount has to repeat them or it fails with EPERM.
fn inherited_flags(path: &str) -> libc::c_ulong {
    let Ok(c) = cs(path) else { return 0 };
    // SAFETY: `c` is a valid path and `st` is plain data that statvfs fills in.
    let f = unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return 0;
        }
        st.f_flag as u64
    };
    let mut m: libc::c_ulong = 0;
    for (st, ms) in [(1u64, libc::MS_RDONLY), (2, libc::MS_NOSUID), (4, libc::MS_NODEV), (8, libc::MS_NOEXEC), (1024, libc::MS_NOATIME), (2048, libc::MS_NODIRATIME), (4096, libc::MS_RELATIME)] {
        if f & st != 0 {
            m |= ms;
        }
    }
    m
}

/// Capabilities kept by root inside the sandbox (what dpkg and apt use); a user that is not root keeps none.
const KEEP_AS_ROOT: [u32; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 28, 31];

fn drop_caps(root_inside: bool) {
    for cap in 0..64u32 {
        if root_inside && KEEP_AS_ROOT.contains(&cap) {
            continue;
        }
        // SAFETY: plain integer arguments; EINVAL past the last capability ends the loop.
        let r = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
        if r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
            break;
        }
    }
}

/// Bring `lo` up in an empty network namespace.
fn loopback_up() {
    // SAFETY: a datagram socket and two ioctls on a zeroed, correctly sized request.
    unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if s < 0 {
            return;
        }
        let mut ifr: libc::ifreq = std::mem::zeroed();
        for (i, c) in b"lo".iter().enumerate() {
            ifr.ifr_name[i] = *c as libc::c_char;
        }
        if libc::ioctl(s, libc::SIOCGIFFLAGS as _, &mut ifr) == 0 {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            libc::ioctl(s, libc::SIOCSIFFLAGS as _, &mut ifr);
        }
        libc::close(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        let mut s = Spec::new("/var/lib/iyakki/linuxapps/dev/rootfs", "dev", vec!["/usr/bin/apt-get".into(), "install".into(), "-y".into(), "curl".into()]);
        s.binds = vec![Bind { src: "/home/admin/work".into(), dst: "/mnt/work".into(), rw: true }, Bind { src: "/home/admin/docs".into(), dst: "/mnt/docs".into(), rw: false }];
        s.net = true;
        s.idmap = IdMap::Multi(2_000_000);
        s.uid = 0;
        s.gid = 0;
        s.mem_mb = 2048;
        s.cpu_pct = 200;
        s.pids = 512;
        s.env = vec![("DEBIAN_FRONTEND".into(), "noninteractive".into())];
        s
    }

    #[test]
    fn arguments_round_trip_and_are_an_array() {
        let s = spec();
        let a = s.to_args();
        assert_eq!(a[0], SUBCOMMAND);
        assert_eq!(Spec::parse(&a).unwrap(), s);
        // the command comes after `--` untouched, and an argument that looks like an option stays a plain word
        let mut t = Spec::new("/r", "x", vec!["sh".into(), "--root".into(), "a b; rm -rf /".into()]);
        t.cwd = "/".into();
        let back = Spec::parse(&t.to_args()).unwrap();
        assert_eq!(back.cmd, vec!["sh", "--root", "a b; rm -rf /"]);
    }

    #[test]
    fn bad_specs_are_refused() {
        let ok = spec().to_args();
        let with = |f: &dyn Fn(&mut Spec)| {
            let mut s = spec();
            f(&mut s);
            Spec::parse(&s.to_args()).unwrap_err()
        };
        assert!(with(&|s| s.root = "relative".into()).contains("absolute"));
        assert!(with(&|s| s.cmd.clear()).contains("no command"));
        assert!(with(&|s| s.name = "a b".into()).contains("name"));
        assert!(with(&|s| s.binds[0].dst = "/proc/x".into()).contains("system folders"));
        assert!(with(&|s| s.binds[0].dst = "/mnt/../etc".into()).contains("must not contain"));
        assert!(with(&|s| s.binds[0].src = "rel".into()).contains("absolute"));
        assert!(with(&|s| s.env = vec![("A B".into(), "x".into())]).contains("not valid"));
        assert!(with(&|s| s.uid = 70000).contains("range"));
        assert!(Spec::parse(&["linuxapp-init".into(), "--bogus".into()]).unwrap_err().contains("unknown"));
        assert!(Spec::parse(&ok[..3]).is_err());
    }

    #[test]
    fn the_seccomp_program_is_well_formed() {
        let denied = denied_syscalls();
        assert!(denied.contains(&libc::SYS_mount) && denied.contains(&libc::SYS_ptrace) && denied.contains(&libc::SYS_unshare) && denied.contains(&libc::SYS_bpf));
        assert!(!denied.contains(&libc::SYS_execve) && !denied.contains(&libc::SYS_read) && !denied.contains(&libc::SYS_openat), "ordinary calls stay allowed");
        let p = seccomp_program(&denied);
        assert_eq!(p.len(), 4 + denied.len() + 2);
        // every jump lands on the last instruction (EPERM) or stays inside the program
        for (i, ins) in p.iter().enumerate().skip(4).take(denied.len()) {
            assert_eq!(i + 1 + ins.jt as usize, p.len() - 1, "instruction {i} jumps to the EPERM return");
        }
        assert_eq!(p[p.len() - 2].k, 0x7fff_0000);
        assert!(p[p.len() - 1].k & 0xffff == libc::EPERM as u32);
        assert!(denied.len() < 250, "a jump distance has to fit in a byte");
    }
}
