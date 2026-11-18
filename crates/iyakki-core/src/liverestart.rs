//! Live restart of the core program without a device restart (docs/development/LIVE-RESTART.md).
//!
//! The core is PID 1 and the shell is one of its threads, so a new program normally runs only after the device restarts. This module is the part
//! of the in-place re-exec (like `systemd daemon-reexec`) that does not need hardware: the state file, the adoption of running children, the
//! secret hand-over through a memory-only file, the health handshake and the rollback to the previous program kept beside the new one.
//!
//! Off by default: nothing here acts unless the flag file [`FLAG_FILE`] exists. Nothing in this file starts the real exec by itself; the caller
//! (the approved request path) calls [`perform`] with an [`Exec`], and the real one is [`RealExec`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

/// The switch. Absent on every image until the owner has tried it on a real device.
pub const FLAG_FILE: &str = "/etc/iyakki/live-restart.enabled";
/// State and the kept previous program. On `/run` (memory): a reboot clears every trace, which is the safe fallback.
pub const DIR: &str = "/run/iyakki/live-restart";
/// The new program must report healthy within this many seconds or the old one is started again.
pub const HEALTH_SECONDS: u64 = 20;
const FORMAT: u64 = 1;

/// Where things are (a test uses a scratch folder and a fake `/proc`).
#[derive(Clone, Debug)]
pub struct Paths {
    pub dir: PathBuf,
    pub proc_root: PathBuf,
    pub flag: PathBuf,
}

impl Paths {
    pub fn real() -> Paths {
        Paths { dir: DIR.into(), proc_root: "/proc".into(), flag: FLAG_FILE.into() }
    }
    fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    fn previous(&self) -> PathBuf {
        self.dir.join("previous")
    }
    fn healthy(&self) -> PathBuf {
        self.dir.join("healthy")
    }
    fn blocked(&self) -> PathBuf {
        self.dir.join("reboot-needed")
    }
}

/// A child of the core that must survive the restart (k3s, a tile helper, audio, ...), identified so that a reused pid is not adopted by mistake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildRec {
    pub pid: u32,
    pub comm: String,
    /// Field 22 of `/proc/PID/stat`: the clock tick the process started at.
    pub start_ticks: u64,
}

/// Where a restart is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Written by the old program just before the exec.
    Exec,
    /// The new program read it and is starting.
    Starting,
    /// The new program reported healthy.
    Healthy,
    /// The new program did not report in time; the old one runs again and a reboot is needed before another try.
    RolledBack,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Exec => "exec",
            Phase::Starting => "starting",
            Phase::Healthy => "healthy",
            Phase::RolledBack => "rolled-back",
        }
    }
    fn parse(s: &str) -> Option<Phase> {
        [Phase::Exec, Phase::Starting, Phase::Healthy, Phase::RolledBack].into_iter().find(|p| p.name() == s)
    }
}

/// The small state file (no secrets: a secret travels in a memory file, see [`hand_over`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub phase: Phase,
    pub pid: u32,
    pub from_version: String,
    pub new_exe: String,
    pub previous_exe: String,
    pub created_unix: u64,
    pub children: Vec<ChildRec>,
    /// File descriptor numbers of memory files that carry secrets across the exec.
    pub secret_fds: Vec<i32>,
}

impl State {
    fn to_json(&self) -> Value {
        json!({
            "format": FORMAT, "phase": self.phase.name(), "pid": self.pid, "from_version": self.from_version, "new_exe": self.new_exe,
            "previous_exe": self.previous_exe, "created_unix": self.created_unix, "secret_fds": self.secret_fds,
            "children": self.children.iter().map(|c| json!({"pid": c.pid, "comm": c.comm, "start_ticks": c.start_ticks})).collect::<Vec<_>>(),
        })
    }
    fn from_json(v: &Value) -> Result<State, String> {
        if v["format"].as_u64() != Some(FORMAT) {
            return Err("the live-restart state is from another format".into());
        }
        let s = |k: &str| v[k].as_str().map(str::to_string).ok_or_else(|| format!("state field {k} missing"));
        Ok(State {
            phase: Phase::parse(&s("phase")?).ok_or("unknown phase")?,
            pid: v["pid"].as_u64().ok_or("pid missing")? as u32,
            from_version: s("from_version")?,
            new_exe: s("new_exe")?,
            previous_exe: s("previous_exe")?,
            created_unix: v["created_unix"].as_u64().unwrap_or(0),
            secret_fds: v["secret_fds"].as_array().into_iter().flatten().filter_map(|x| x.as_i64().map(|n| n as i32)).collect(),
            children: v["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| Some(ChildRec { pid: c["pid"].as_u64()? as u32, comm: c["comm"].as_str()?.to_string(), start_ticks: c["start_ticks"].as_u64()? }))
                .collect(),
        })
    }
}

pub fn enabled(p: &Paths) -> bool {
    p.flag.is_file()
}

/// True while a reboot is required before another live restart (the last one rolled back).
pub fn reboot_needed(p: &Paths) -> bool {
    p.blocked().exists()
}

pub fn read_state(p: &Paths) -> Option<State> {
    let text = std::fs::read_to_string(p.state()).ok()?;
    State::from_json(&serde_json::from_str(&text).ok()?).ok()
}

fn write_state(p: &Paths, s: &State) -> Result<(), String> {
    let tmp = p.dir.join("state.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&s.to_json()).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p.state()).map_err(|e| e.to_string())
}

/// Parse `/proc/PID/stat`: (comm, ppid, start ticks). The name may hold spaces and brackets, so the split is at the LAST `)`.
pub fn parse_stat(text: &str) -> Option<(String, u32, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // rest[0] = state, rest[1] = ppid, ... rest[19] = starttime (field 22 overall)
    Some((comm, rest.get(1)?.parse().ok()?, rest.get(19)?.parse().ok()?))
}

/// The processes whose parent is `parent` (the core's own children).
pub fn children_of(proc_root: &Path, parent: u32) -> Vec<ChildRec> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(proc_root).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(text) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        if let Some((comm, ppid, start_ticks)) = parse_stat(&text) {
            if ppid == parent && pid != parent {
                out.push(ChildRec { pid, comm, start_ticks });
            }
        }
    }
    out.sort_by_key(|c| c.pid);
    out
}

/// What the new program found of the children the old one listed.
#[derive(Debug, PartialEq, Eq)]
pub struct Adoption {
    /// Same pid, same start tick, still our child: adopted as it is.
    pub kept: Vec<ChildRec>,
    /// Gone, or the pid belongs to another process now: the owner of that service starts it again.
    pub gone: Vec<ChildRec>,
}

pub fn adopt(s: &State, proc_root: &Path) -> Adoption {
    let now = children_of(proc_root, s.pid);
    let (kept, gone) = s.children.iter().cloned().partition(|c| now.iter().any(|n| n == c));
    Adoption { kept, gone }
}

/// Everything the exec needs, decided before anything changes.
pub fn prepare(p: &Paths, new_exe: &Path, self_exe: &Path, from_version: &str, now_unix: u64, secret_fds: Vec<i32>) -> Result<State, String> {
    if !enabled(p) {
        return Err("live restart is off on this device (no flag file); it is not enabled by default".into());
    }
    if reboot_needed(p) {
        return Err("the last live restart did not come up and was rolled back: restart the device before trying again".into());
    }
    let md = std::fs::metadata(new_exe).map_err(|e| format!("{}: {e}", new_exe.display()))?;
    if !md.is_file() || md.len() < 1024 {
        return Err(format!("{} is not a program file", new_exe.display()));
    }
    let mut magic = [0u8; 4];
    io::Read::read_exact(&mut std::fs::File::open(new_exe).map_err(|e| e.to_string())?, &mut magic).map_err(|e| e.to_string())?;
    if &magic != b"\x7fELF" {
        return Err(format!("{} is not an executable program (no ELF header)", new_exe.display()));
    }
    std::fs::create_dir_all(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
    let _ = std::fs::set_permissions(&p.dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    // the old program stays beside, so the way back never depends on the file the patch replaced
    let prev = p.previous();
    std::fs::copy(self_exe, &prev).map_err(|e| format!("could not keep the current program: {e}"))?;
    std::fs::set_permissions(&prev, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(p.healthy());
    let st = State {
        phase: Phase::Exec,
        pid: std::process::id(),
        from_version: from_version.into(),
        new_exe: new_exe.display().to_string(),
        previous_exe: prev.display().to_string(),
        created_unix: now_unix,
        children: children_of(&p.proc_root, std::process::id()),
        secret_fds,
    };
    write_state(p, &st)?;
    Ok(st)
}

/// Replaces the running program. The real one never returns on success.
pub trait Exec {
    /// Only returns when the exec failed.
    fn exec(&self, exe: &Path, args: &[String]) -> io::Error;
}

type Hook = Box<dyn Fn() + Send>;
static BEFORE_EXEC: std::sync::Mutex<Vec<Hook>> = std::sync::Mutex::new(Vec::new());

/// Register something the running program does just before it replaces itself (stop the shell loop so the screen is let go, sync the disks,
/// ...). Hooks run in the order they were added, each once per exec attempt, on the thread that does the exec.
pub fn before_exec(f: impl Fn() + Send + 'static) {
    BEFORE_EXEC.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Box::new(f));
}

/// Run the hooks (what [`RealExec`] does before the exec; a test calls it by hand).
pub fn run_before_exec_hooks() -> usize {
    let hooks = BEFORE_EXEC.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for h in hooks.iter() {
        h();
    }
    hooks.len()
}

pub struct RealExec;

impl Exec for RealExec {
    fn exec(&self, exe: &Path, args: &[String]) -> io::Error {
        run_before_exec_hooks();
        std::os::unix::process::CommandExt::exec(std::process::Command::new(exe).args(args))
    }
}

/// Prepare and exec. Returns only when something failed (nothing is left half done: the state file is removed again).
pub fn perform(p: &Paths, new_exe: &Path, self_exe: &Path, from_version: &str, now_unix: u64, secret_fds: Vec<i32>, args: &[String], exec: &dyn Exec) -> String {
    let st = match prepare(p, new_exe, self_exe, from_version, now_unix, secret_fds) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let err = exec.exec(Path::new(&st.new_exe), args);
    let _ = std::fs::remove_file(p.state());
    format!("the new program could not be started ({err}); the old one keeps running")
}

// ---- the request path (F-SPLIT-06): a command asks, the running program (PID 1) does it ----

/// A request is good for this many seconds (an old one left in the folder is ignored, never acted on).
pub const REQUEST_SECONDS: u64 = 60;

impl Paths {
    fn request(&self) -> PathBuf {
        self.dir.join("request.json")
    }
}

/// `iyakki live-restart PROGRAM`: leave a request for the running program. Refused unless live restart is on and the file is a program
/// (the same checks as [`prepare`], so a bad request is refused here, in front of the person who asked, not later inside PID 1). The command
/// is run through developer mode's shell (one approval card per command on the device) or by a chat tool with a card; this function does
/// not ask anything itself.
pub fn write_request(p: &Paths, new_exe: &Path, now_unix: u64) -> Result<(), String> {
    if !enabled(p) {
        return Err("live restart is off on this device (no flag file); it is not enabled by default".into());
    }
    if reboot_needed(p) {
        return Err("the last live restart did not come up and was rolled back: restart the device before trying again".into());
    }
    let exe = std::fs::canonicalize(new_exe).map_err(|e| format!("{}: {e}", new_exe.display()))?;
    let md = std::fs::metadata(&exe).map_err(|e| e.to_string())?;
    if !md.is_file() || md.len() < 1024 || std::os::unix::fs::PermissionsExt::mode(&md.permissions()) & 0o111 == 0 {
        return Err(format!("{} is not an executable program file", exe.display()));
    }
    std::fs::create_dir_all(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
    let _ = std::fs::set_permissions(&p.dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    let tmp = p.dir.join("request.json.tmp");
    std::fs::write(&tmp, json!({"exe": exe.display().to_string(), "at": now_unix}).to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p.request()).map_err(|e| e.to_string())
}

/// The program a fresh request names, once: the file is removed whatever it says. A request older than [`REQUEST_SECONDS`], from the
/// future, or damaged is dropped.
pub fn take_request(p: &Paths, now_unix: u64) -> Option<PathBuf> {
    let text = std::fs::read_to_string(p.request()).ok()?;
    let _ = std::fs::remove_file(p.request());
    let v: Value = serde_json::from_str(&text).ok()?;
    let (exe, at) = (v["exe"].as_str()?, v["at"].as_u64()?);
    (at <= now_unix && now_unix - at <= REQUEST_SECONDS).then(|| PathBuf::from(exe))
}

/// One look at the request folder by the running program: a fresh request is carried out with [`perform`], which only returns when the new
/// program could not be started (the answer says why and the old program keeps running). `None` when there was no request.
pub fn serve_once(p: &Paths, self_exe: &Path, from_version: &str, now_unix: u64, args: &[String], exec: &dyn Exec) -> Option<String> {
    let exe = take_request(p, now_unix)?;
    Some(perform(p, &exe, self_exe, from_version, now_unix, Vec::new(), args, exec))
}

static RESTARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `main` says: this process is the new program of a live restart (and not a rollback), so it owes a health report.
pub fn note_restarted() {
    RESTARTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Called by the shell thread right where the first frame (the sign-in screen) is drawn: when this process is a live-restarted one, report
/// healthy after `delay`. A process that is not (a normal boot, a rollback) does nothing. Returns whether a report was scheduled.
pub fn report_healthy_soon(p: Paths, delay: Duration) -> bool {
    if !RESTARTED.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    std::thread::Builder::new()
        .name("iyakki-live-health".into())
        .spawn(move || {
            std::thread::sleep(delay);
            if let Err(e) = mark_healthy(&p) {
                log::error!("live restart: could not report healthy: {e}");
            }
        })
        .is_ok()
}

/// What the new program learns when it starts.
#[derive(Debug)]
pub struct Started {
    pub state: State,
    pub adoption: Adoption,
    /// This start is the old program again after a rollback: do not try another live restart.
    pub rolled_back: bool,
}

/// First thing the program does when it starts: is this a live-restart start?
pub fn begin(p: &Paths) -> Option<Started> {
    let mut st = read_state(p)?;
    // a state of another process (a stale file from a boot long ago on a persistent folder) is not ours
    if st.pid != std::process::id() {
        return None;
    }
    match st.phase {
        Phase::Exec => {
            let adoption = adopt(&st, &p.proc_root);
            st.phase = Phase::Starting;
            let _ = write_state(p, &st);
            Some(Started { state: st, adoption, rolled_back: false })
        }
        Phase::RolledBack => {
            let adoption = adopt(&st, &p.proc_root);
            Some(Started { state: st, adoption, rolled_back: true })
        }
        Phase::Starting | Phase::Healthy => None,
    }
}

/// The new program calls this when the screen drew and the services answer. It also lets the memory files with secrets go.
pub fn mark_healthy(p: &Paths) -> io::Result<()> {
    if let Some(mut st) = read_state(p) {
        st.phase = Phase::Healthy;
        write_state(p, &st).map_err(io::Error::other)?;
        for fd in &st.secret_fds {
            release(*fd);
        }
    }
    std::fs::write(p.healthy(), b"ok\n")
}

/// Wait for the health report; if it does not come in time, start the old program again and block live restarts until a reboot.
/// Returns true when the new program reported healthy. Meant for a thread of the new program; `sleep` is injectable for the tests.
pub fn watchdog(p: &Paths, deadline: Duration, step: Duration, exec: &dyn Exec, args: &[String], sleep: &dyn Fn(Duration)) -> bool {
    let mut waited = Duration::ZERO;
    while waited < deadline {
        if p.healthy().exists() {
            return true;
        }
        sleep(step);
        waited += step;
    }
    if p.healthy().exists() {
        return true;
    }
    rollback(p, exec, args);
    false
}

/// Exec the kept old program. Writes the block first, so even a failed exec leaves "reboot needed" behind.
pub fn rollback(p: &Paths, exec: &dyn Exec, args: &[String]) -> String {
    let _ = std::fs::write(p.blocked(), b"the last live restart did not become healthy\n");
    let Some(mut st) = read_state(p) else { return "no state file: nothing to go back to, restart the device".into() };
    st.phase = Phase::RolledBack;
    let _ = write_state(p, &st);
    let err = exec.exec(Path::new(&st.previous_exe), args);
    format!("the old program could not be started again ({err}): restart the device")
}

// ------------------------------------------------------------------------------------------------ secrets in memory

/// Put a secret (for example the key of an unlocked vault) into a memory-only file that is inherited across the exec. Never on a disk.
pub fn hand_over(name: &str, secret: &[u8]) -> io::Result<i32> {
    let cname = std::ffi::CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: plain syscall with a valid NUL-terminated name; the flags ask for nothing else.
    let fd = unsafe { libc::syscall(libc::SYS_memfd_create, cname.as_ptr(), 0u32) } as i32;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut off = 0usize;
    while off < secret.len() {
        // SAFETY: the buffer range is inside `secret`.
        let n = unsafe { libc::pwrite(fd, secret[off..].as_ptr().cast(), secret.len() - off, off as libc::off_t) };
        if n <= 0 {
            let e = io::Error::last_os_error();
            release(fd);
            return Err(e);
        }
        off += n as usize;
    }
    Ok(fd)
}

/// Read what [`hand_over`] stored. Does not close the file: on a rollback the old program needs it again; [`mark_healthy`] lets it go.
pub fn take_over(fd: i32) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: `buf` is a valid writable buffer of its length.
        let n = unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), buf.len(), out.len() as libc::off_t) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

pub fn release(fd: i32) {
    // SAFETY: closing a descriptor this module opened (or inherited for this purpose).
    unsafe { libc::close(fd) };
}

/// The descriptors made by [`hand_over`] are kept open over exec: Rust's own files are close-on-exec, this one must not be.
pub fn keep_across_exec(fd: i32) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    let r = unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let d = std::env::temp_dir().join(format!("iyakki-liverestart-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }
        fn paths(&self, flag: bool) -> Paths {
            let p = Paths { dir: self.0.join("run"), proc_root: self.0.join("proc"), flag: self.0.join("enabled") };
            std::fs::create_dir_all(&p.proc_root).unwrap();
            if flag {
                std::fs::write(&p.flag, b"").unwrap();
            }
            p
        }
        fn program(&self, name: &str) -> PathBuf {
            let f = self.0.join(name);
            let mut b = b"\x7fELF".to_vec();
            b.extend(vec![7u8; 2000]);
            std::fs::write(&f, b).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            f
        }
        fn fake_proc(&self, p: &Paths, pid: u32, comm: &str, ppid: u32, start: u64) {
            let d = p.proc_root.join(pid.to_string());
            std::fs::create_dir_all(&d).unwrap();
            let mut f: Vec<String> = vec!["S".into(), ppid.to_string()];
            f.extend((0..17).map(|_| "0".to_string()));
            f.push(start.to_string());
            std::fs::write(d.join("stat"), format!("{pid} ({comm}) {}", f.join(" "))).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeExec {
        calls: RefCell<Vec<String>>,
        fail: bool,
    }
    impl Exec for FakeExec {
        fn exec(&self, exe: &Path, _args: &[String]) -> io::Error {
            self.calls.borrow_mut().push(exe.display().to_string());
            io::Error::other(if self.fail { "no such program" } else { "fake exec returns" })
        }
    }

    #[test]
    fn a_request_is_refused_while_the_feature_is_off_or_for_a_file_that_is_not_a_program() {
        let s = Scratch::new("req-off");
        let new = s.program("new");
        assert!(write_request(&s.paths(false), &new, 100).unwrap_err().contains("off"));
        let p = s.paths(true);
        std::fs::write(s.0.join("text"), vec![b'x'; 3000]).unwrap();
        assert!(write_request(&p, &s.0.join("text"), 100).unwrap_err().contains("not an executable"), "no execute bit");
        assert!(write_request(&p, &s.0.join("missing"), 100).is_err());
        assert!(!p.request().exists(), "a refused request leaves nothing");
    }

    #[test]
    fn a_fresh_request_is_taken_once_and_an_old_future_or_damaged_one_is_dropped() {
        let s = Scratch::new("req-take");
        let p = s.paths(true);
        let new = s.program("new");
        write_request(&p, &new, 1000).unwrap();
        assert_eq!(std::fs::metadata(&p.request()).unwrap().len() > 0, true);
        assert_eq!(take_request(&p, 1000 + REQUEST_SECONDS).map(|x| x.ends_with("new")), Some(true));
        assert_eq!(take_request(&p, 1001), None, "taken once");
        write_request(&p, &new, 1000).unwrap();
        assert_eq!(take_request(&p, 1000 + REQUEST_SECONDS + 1), None, "too old");
        write_request(&p, &new, 5000).unwrap();
        assert_eq!(take_request(&p, 4000), None, "from the future");
        std::fs::write(p.request(), "not json").unwrap();
        assert_eq!(take_request(&p, 10), None);
        assert!(!p.request().exists(), "a damaged request is removed too");
    }

    #[test]
    fn the_running_program_carries_out_a_request_with_the_exec_and_when_it_fails_the_old_one_stays_and_no_state_is_left() {
        let s = Scratch::new("req-serve");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        let ex = FakeExec { fail: true, ..Default::default() };
        assert_eq!(serve_once(&p, &old, "1", 100, &[], &ex), None, "no request, nothing happens");
        write_request(&p, &new, 100).unwrap();
        let said = serve_once(&p, &old, "1", 105, &["serve".into()], &ex).unwrap();
        assert!(said.contains("could not be started") && said.contains("old one keeps running"), "{said}");
        assert_eq!(ex.calls.borrow().len(), 1);
        assert!(ex.calls.borrow()[0].ends_with("new"));
        assert!(read_state(&p).is_none(), "the failed exec leaves no state behind");
        assert_eq!(serve_once(&p, &old, "1", 106, &[], &ex), None, "the request was used up");
        // a rollback needing a reboot stops a second request at the door
        std::fs::create_dir_all(&p.dir).unwrap();
        std::fs::write(p.blocked(), b"").unwrap();
        assert!(write_request(&p, &new, 107).unwrap_err().contains("restart the device"));
    }

    #[test]
    fn the_health_report_comes_from_the_shells_first_frame_only_in_a_live_restarted_process() {
        let s = Scratch::new("health");
        let p = s.paths(true);
        std::fs::create_dir_all(&p.dir).unwrap();
        // a normal start: nothing is reported
        assert!(!report_healthy_soon(Paths { dir: p.dir.clone(), proc_root: p.proc_root.clone(), flag: p.flag.clone() }, Duration::from_millis(0)));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!p.healthy().exists());
        // the new program of a live restart: the report is written when the shell thread asks
        note_restarted();
        assert!(report_healthy_soon(Paths { dir: p.dir.clone(), proc_root: p.proc_root.clone(), flag: p.flag.clone() }, Duration::from_millis(0)));
        for _ in 0..100 {
            if p.healthy().exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.healthy().exists(), "the health file is there");
        RESTARTED.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn the_hooks_run_in_order_before_the_exec() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
        let (a, b) = (seen.clone(), seen.clone());
        let before = run_before_exec_hooks();
        before_exec(move || a.lock().unwrap().push(1));
        before_exec(move || b.lock().unwrap().push(2));
        assert_eq!(run_before_exec_hooks(), before + 2);
        assert_eq!(*seen.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn stat_lines_with_spaces_and_brackets_in_the_name_parse() {
        let line = "812 (k3s (server) x) S 1 812 812 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 30 0 987654 123 456";
        let (comm, ppid, start) = parse_stat(line).unwrap();
        assert_eq!((comm.as_str(), ppid, start), ("k3s (server) x", 1, 987654));
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn it_does_nothing_unless_the_flag_file_exists() {
        let s = Scratch::new("flag");
        let p = s.paths(false);
        let new = s.program("new");
        let old = s.program("old");
        let e = prepare(&p, &new, &old, "1", 10, vec![]).unwrap_err();
        assert!(e.contains("off"), "{e}");
        assert!(!p.dir.exists(), "nothing may be created while it is off");
    }

    #[test]
    fn a_text_file_or_a_tiny_file_is_not_taken_for_a_program() {
        let s = Scratch::new("elf");
        let p = s.paths(true);
        let old = s.program("old");
        let junk = s.0.join("junk");
        std::fs::write(&junk, vec![b'x'; 5000]).unwrap();
        assert!(prepare(&p, &junk, &old, "1", 1, vec![]).unwrap_err().contains("ELF"));
        std::fs::write(&junk, b"x").unwrap();
        assert!(prepare(&p, &junk, &old, "1", 1, vec![]).is_err());
        assert!(prepare(&p, &s.0.join("missing"), &old, "1", 1, vec![]).is_err());
        assert!(read_state(&p).is_none(), "a refused request leaves no state behind");
    }

    #[test]
    fn prepare_keeps_the_old_program_and_lists_the_children() {
        let s = Scratch::new("prep");
        let p = s.paths(true);
        let me = std::process::id();
        s.fake_proc(&p, 501, "k3s", me, 1000);
        s.fake_proc(&p, 502, "aplay", me, 2000);
        s.fake_proc(&p, 503, "other", 77, 3000);
        let (new, old) = (s.program("new"), s.program("old"));
        let st = prepare(&p, &new, &old, "0.1.0-a", 42, vec![]).unwrap();
        assert_eq!(st.children.iter().map(|c| c.pid).collect::<Vec<_>>(), vec![501, 502]);
        assert_eq!(std::fs::read(p.previous()).unwrap(), std::fs::read(&old).unwrap());
        assert_eq!(std::fs::metadata(p.previous()).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(read_state(&p).unwrap(), st);
    }

    #[test]
    fn the_new_program_adopts_live_children_and_names_the_lost_ones() {
        let s = Scratch::new("adopt");
        let p = s.paths(true);
        let me = std::process::id();
        s.fake_proc(&p, 501, "k3s", me, 1000);
        s.fake_proc(&p, 502, "aplay", me, 2000);
        s.fake_proc(&p, 504, "tile", me, 4000);
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        // while the exec happens: aplay ended, and its pid went to a different process; tile is still there
        std::fs::remove_dir_all(p.proc_root.join("502")).unwrap();
        s.fake_proc(&p, 502, "someone-else", me, 9999);
        let started = begin(&p).expect("a live-restart start");
        assert!(!started.rolled_back);
        let kept: Vec<u32> = started.adoption.kept.iter().map(|c| c.pid).collect();
        let gone: Vec<u32> = started.adoption.gone.iter().map(|c| c.pid).collect();
        assert_eq!((kept, gone), (vec![501, 504], vec![502]), "a reused pid must not be adopted");
        assert_eq!(read_state(&p).unwrap().phase, Phase::Starting);
        assert!(begin(&p).is_none(), "the second look is a normal start");
    }

    #[test]
    fn a_state_file_of_another_process_is_not_ours() {
        let s = Scratch::new("stale");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        let mut st = prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        st.pid = std::process::id() + 1;
        write_state(&p, &st).unwrap();
        assert!(begin(&p).is_none());
    }

    #[test]
    fn perform_runs_the_new_program_and_a_failed_exec_leaves_no_state() {
        let s = Scratch::new("perform");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        let ex = FakeExec { fail: true, ..FakeExec::default() };
        let msg = perform(&p, &new, &old, "1", 1, vec![], &["serve".into()], &ex);
        assert_eq!(ex.calls.borrow().as_slice(), [new.display().to_string()]);
        assert!(msg.contains("old one keeps running"), "{msg}");
        assert!(read_state(&p).is_none());
        assert!(!reboot_needed(&p), "a failed exec is not a rollback");
    }

    #[test]
    fn a_healthy_report_in_time_keeps_the_new_program() {
        let s = Scratch::new("healthy");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        let ex = FakeExec::default();
        let ticks = RefCell::new(0u32);
        let ok = watchdog(&p, Duration::from_secs(20), Duration::from_secs(1), &ex, &[], &|_| {
            *ticks.borrow_mut() += 1;
            if *ticks.borrow() == 3 {
                mark_healthy(&p).unwrap();
            }
        });
        assert!(ok && ex.calls.borrow().is_empty() && !reboot_needed(&p));
        assert_eq!(read_state(&p).unwrap().phase, Phase::Healthy);
    }

    #[test]
    fn no_health_report_starts_the_old_program_and_blocks_another_try_until_a_reboot() {
        let s = Scratch::new("rollback");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        let ex = FakeExec::default();
        let waited = RefCell::new(Duration::ZERO);
        let ok = watchdog(&p, Duration::from_secs(20), Duration::from_secs(5), &ex, &["serve".into()], &|d| *waited.borrow_mut() += d);
        assert!(!ok);
        assert_eq!(*waited.borrow(), Duration::from_secs(20), "waits the whole time, no longer");
        assert_eq!(ex.calls.borrow().as_slice(), [p.previous().display().to_string()], "the kept old program is started");
        assert_eq!(read_state(&p).unwrap().phase, Phase::RolledBack);
        assert!(reboot_needed(&p));
        let again = prepare(&p, &new, &old, "1", 2, vec![]).unwrap_err();
        assert!(again.contains("restart the device"), "{again}");
    }

    #[test]
    fn after_a_rollback_the_old_program_knows_it_and_still_adopts_its_children() {
        let s = Scratch::new("rolled");
        let p = s.paths(true);
        let me = std::process::id();
        s.fake_proc(&p, 501, "k3s", me, 1000);
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        rollback(&p, &FakeExec::default(), &[]);
        let st = begin(&p).unwrap();
        assert!(st.rolled_back);
        assert_eq!(st.adoption.kept.len(), 1);
    }

    #[test]
    fn a_failed_rollback_exec_still_says_restart_the_device() {
        let s = Scratch::new("rollfail");
        let p = s.paths(true);
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![]).unwrap();
        let msg = rollback(&p, &FakeExec { fail: true, ..FakeExec::default() }, &[]);
        assert!(msg.contains("restart the device") && reboot_needed(&p));
    }

    #[test]
    fn a_secret_travels_in_memory_twice_and_is_never_in_the_state_file() {
        let s = Scratch::new("secret");
        let p = s.paths(true);
        let secret = b"vault-key-0123456789".repeat(300);
        let fd = hand_over("iyakki-test-secret", &secret).unwrap();
        keep_across_exec(fd).unwrap();
        let link = std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap();
        assert!(link.to_string_lossy().contains("memfd:"), "must be a memory file, was {}", link.display());
        assert_eq!(take_over(fd).unwrap(), secret);
        assert_eq!(take_over(fd).unwrap(), secret, "a rollback reads it again");
        let (new, old) = (s.program("new"), s.program("old"));
        prepare(&p, &new, &old, "1", 1, vec![fd]).unwrap();
        let text = std::fs::read_to_string(p.state()).unwrap();
        assert!(!text.contains("vault-key") && text.contains(&fd.to_string()));
        mark_healthy(&p).unwrap();
        // another test thread may reuse the number, so look at what it is, not whether it exists
        assert!(std::fs::read_link(format!("/proc/self/fd/{fd}")).map_or(true, |l| !l.to_string_lossy().contains("iyakki-test-secret")), "closed once healthy");
    }

    /// The real exec, in a child process: it keeps its pid, a child that was running before the exec is still the same process afterwards, and the new
    /// program reads the state and the secret. The "new program" is a shell script, so no hardware and no image are needed.
    #[test]
    fn a_real_exec_keeps_the_pid_and_the_running_child() {
        if std::env::var("IYAKKI_LR_CHILD").is_ok() {
            return;
        }
        let s = Scratch::new("real");
        let p = s.paths(true);
        let p = Paths { proc_root: "/proc".into(), ..p };
        let report = s.0.join("report");
        let script = s.0.join("new.sh");
        // a script has no ELF header, so the "new program" is a copy of the shell
        std::fs::copy("/bin/sh", s.0.join("newsh")).unwrap();
        let new = s.0.join("newsh");
        std::fs::write(
            &script,
            format!(
                "echo \"pid=$$\" > {r}\nread -r line < /proc/$$/fd/$IYAKKI_LR_FD; echo \"secret=$line\" >> {r}\nkill -0 $IYAKKI_LR_CHILDPID 2>/dev/null && echo child-alive >> {r}\ngrep -c exec {d}/state.json >> {r}\n",
                r = report.display(),
                d = p.dir.display()
            ),
        )
        .unwrap();
        // the child process that must survive
        let mut sleeper = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        // run this test binary again as the "old core": it hands the secret over and execs the shell
        let me = std::env::current_exe().unwrap();
        let out = std::process::Command::new(me)
            .args(["--exact", "liverestart::tests::child_entry", "--nocapture", "--test-threads=1"])
            .env("IYAKKI_LR_CHILD", "1")
            .env("IYAKKI_LR_DIR", &p.dir)
            .env("IYAKKI_LR_FLAG", &p.flag)
            .env("IYAKKI_LR_NEW", &new)
            .env("IYAKKI_LR_SCRIPT", &script)
            .env("IYAKKI_LR_CHILDPID", sleeper.id().to_string())
            .output()
            .unwrap();
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        let _ = out;
        let text = std::fs::read_to_string(&report).unwrap_or_else(|_| format!("no report; child said: {}", String::from_utf8_lossy(&out.stdout)));
        assert!(text.contains("secret=open-sesame") && text.contains("child-alive"), "{text}");
        let pid_after: u32 = text.lines().find_map(|l| l.strip_prefix("pid=")).and_then(|v| v.trim().parse().ok()).unwrap();
        let pid_before: u32 = std::fs::read_to_string(s.0.join("pid-before")).unwrap().trim().parse().unwrap();
        assert_eq!(pid_before, pid_after, "an exec keeps the process id");
    }

    #[test]
    fn child_entry() {
        if std::env::var("IYAKKI_LR_CHILD").is_err() {
            return;
        }
        let dir: PathBuf = std::env::var("IYAKKI_LR_DIR").unwrap().into();
        let p = Paths { dir: dir.clone(), proc_root: "/proc".into(), flag: std::env::var("IYAKKI_LR_FLAG").unwrap().into() };
        let base = dir.parent().unwrap().to_path_buf();
        std::fs::write(base.join("pid-before"), std::process::id().to_string()).unwrap();
        let fd = hand_over("iyakki-real-exec", b"open-sesame\n").unwrap();
        keep_across_exec(fd).unwrap();
        std::env::set_var("IYAKKI_LR_FD", fd.to_string());
        let new = PathBuf::from(std::env::var("IYAKKI_LR_NEW").unwrap());
        let script = std::env::var("IYAKKI_LR_SCRIPT").unwrap();
        let msg = perform(&p, &new, &new, "test", 1, vec![fd], &[script], &RealExec);
        println!("exec returned: {msg}");
    }
}
