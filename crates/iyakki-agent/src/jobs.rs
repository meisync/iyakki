//! JOBS: long commands that run on their own, detached from the chat that started them. See `docs/JOBS.md`.
//!
//! A job is a command started in its own session and process group (so it survives the chat, the session and a restart of the
//! shell), with its output going to a log file and a small status file next to it. Everything about a job lives on disk
//! (`<root>/<user>/<id>/`: `meta.json`, `log.txt`, and `exit` once the command ended), so a new shell finds running and finished
//! jobs again just by listing the folder. The state is worked out from those files and from `/proc`:
//!
//! * `running`: the process is alive (checked by pid and start time, so a reused pid is not mistaken for it),
//! * `succeeded` / `failed (exit code)`: the wrapper wrote the exit code,
//! * `stopped`: a stop was asked for and the process is gone,
//! * `lost`: the process is gone and nothing says why (the machine was restarted, or it was killed from outside).
//!
//! Stop sends TERM to the whole group, then KILL after a grace period. The log is capped (the tail is kept) and everything
//! shown to a model or a person goes through the credential redaction first. No dependencies: `sh`, `setsid` and `kill` only
//! (BOUNDARY.md lists this file).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The log is cut to its last `LOG_KEEP` bytes when it grows past `LOG_CAP`.
pub const LOG_CAP: u64 = 5 * 1024 * 1024;
pub const LOG_KEEP: u64 = 1024 * 1024;
/// Most jobs one person may have running at a time.
pub const MAX_RUNNING: usize = 8;
/// Finished jobs kept per person (the oldest go first).
pub const KEEP_FINISHED: usize = 30;
/// The longest `job_wait`.
pub const MAX_WAIT_SECS: u64 = 60;
pub const STOP_GRACE: Duration = Duration::from_secs(3);
const MAX_COMMAND: usize = 8 * 1024;
const MAX_LABEL: usize = 80;
/// What is read from the end of the log to find the last line and a percentage.
const PEEK: u64 = 8 * 1024;
/// The most a tail returns.
pub const MAX_TAIL_BYTES: usize = 64 * 1024;
pub const MAX_TAIL_LINES: usize = 400;

/// The exit code of a step that ran out of time (the `timeout` program's convention).
pub const TIMED_OUT_EXIT: i32 = 124;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Running,
    Succeeded,
    /// Ended with this exit code (128 + n when a signal ended it).
    Failed(i32),
    Stopped,
    Lost,
}

impl State {
    pub fn finished(&self) -> bool {
        *self != State::Running
    }
    pub fn label(&self) -> String {
        match self {
            State::Running => "running".into(),
            State::Succeeded => "succeeded".into(),
            State::Failed(TIMED_OUT_EXIT) => "timed out".into(),
            State::Failed(c) => format!("failed (exit {c})"),
            State::Stopped => "stopped".into(),
            State::Lost => "lost".into(),
        }
    }
}

/// What to run: a shell line (`sh -c`), or a program with its arguments (no shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Shell(String),
    Argv(Vec<String>),
}

impl Cmd {
    pub fn text(&self) -> String {
        match self {
            Cmd::Shell(s) => s.clone(),
            Cmd::Argv(a) => crate::exec::command_line(a.first().map_or("", String::as_str), &a.iter().skip(1).cloned().collect::<Vec<_>>()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub label: String,
    pub cmd: Cmd,
    pub cwd: PathBuf,
    /// The workspace the job belongs to (its name), empty outside the IDE.
    pub workspace: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Meta {
    id: String,
    label: String,
    command: String,
    cwd: String,
    owner: String,
    workspace: String,
    pid: u32,
    /// The process's start time in clock ticks (from /proc), to tell it from a later process with the same pid.
    start_ticks: u64,
    started_ms: u64,
    ended_ms: u64,
    stop_requested: bool,
    dismissed: bool,
    announced_ui: bool,
    announced_agent: bool,
    announced_inbox: bool,
}

/// A job as the tools and the screen see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub owner: String,
    pub workspace: String,
    pub pid: u32,
    pub state: State,
    pub started_ms: u64,
    /// 0 while running.
    pub ended_ms: u64,
    /// 0..=100 when the output says how far it is, else None (indeterminate).
    pub progress: Option<f32>,
    /// The last line of output, cleaned and redacted.
    pub last_line: String,
    /// When the job last showed a sign of work, in ms: new output in its log, or CPU time used by any process of its group (checked
    /// each time the job is read). 0 for a job that is not running. The screen's spinner moves only while this is recent.
    pub last_active_ms: u64,
    pub dismissed: bool,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == State::Running
    }
    /// Seconds it has run (to now while running).
    /// Seconds since the running job last showed a sign of work (0 when it did just now or is not running).
    pub fn quiet_secs(&self, now_ms: u64) -> u64 {
        if !self.running() || self.last_active_ms == 0 {
            return 0;
        }
        now_ms.saturating_sub(self.last_active_ms) / 1000
    }

    pub fn elapsed_secs(&self, now_ms: u64) -> u64 {
        let end = if self.ended_ms > 0 { self.ended_ms } else { now_ms };
        end.saturating_sub(self.started_ms) / 1000
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `3m 12s`, `45s`, `1h 05m`.
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// Who is told about a finished job: the chat (a visible event), the assistant (a note at its next turn) and the inbox are told once each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Ui,
    Agent,
    /// A note in the person's INBOX (the bell), so a job that ends while they are in another app is not missed.
    Inbox,
}

#[derive(Clone)]
pub struct Jobs {
    root: PathBuf,
    cap: u64,
    keep: u64,
    lock: Arc<Mutex<()>>,
}

fn clean_owner(owner: &str) -> String {
    let s: String = owner.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    if s.is_empty() || s.starts_with('.') { "invalid".into() } else { s }
}

fn valid_id(id: &str) -> bool {
    id.len() <= 12 && id.starts_with('j') && id.len() > 1 && id[1..].chars().all(|c| c.is_ascii_digit())
}

/// Environment variables a job inherits: where to find programs and the person's own tool setup. Never anything that looks
/// like a secret (the person's tokens stay in the vault; a job that needs one asks for it with vault_request).
fn passes(name: &str) -> bool {
    matches!(
        name,
        "PATH" | "HOME" | "LANG" | "LC_ALL" | "TERM" | "TZ" | "USER" | "TMPDIR" | "CARGO_HOME" | "RUSTUP_HOME" | "JAVA_HOME" | "ANDROID_HOME" | "ANDROID_SDK_ROOT" | "ANDROID_AVD_HOME" | "NVM_DIR" | "GOPATH" | "GOROOT" | "HTTPS_PROXY" | "HTTP_PROXY" | "NO_PROXY" | "https_proxy" | "http_proxy" | "no_proxy" | "SSL_CERT_FILE" | "DISPLAY" | "XDG_RUNTIME_DIR"
    )
}

/// Drop terminal escape sequences and the carriage-return redraws of progress bars (only the last redraw of a line stays).
pub fn clean_output(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    // an operating-system command (a window title) ends with BEL or ESC \
                    for n in chars.by_ref() {
                        if n == '\u{7}' {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                // a redraw: the line so far is replaced by what follows
                while out.chars().last().is_some_and(|l| l != '\n') {
                    out.pop();
                }
            }
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// How far a job is, from the last lines of its output: `42%`, `42.5 %`, `[ 42/100 ]`, `[42/100]`, `(42/100)`. None when the
/// output says nothing like that.
pub fn progress_hint(text: &str) -> Option<f32> {
    // raw output may still hold \r redraws: look at every redraw, newest first
    let segs: Vec<&str> = text.split(['\n', '\r']).map(str::trim).filter(|s| !s.is_empty()).collect();
    for seg in segs.iter().rev().take(6) {
        if let Some(p) = percent_in(seg).or_else(|| fraction_in(seg)) {
            return Some(p);
        }
    }
    None
}

fn percent_in(line: &str) -> Option<f32> {
    let b = line.as_bytes();
    let mut best = None;
    for (i, &c) in b.iter().enumerate() {
        if c != b'%' {
            continue;
        }
        let mut j = i;
        while j > 0 && b[j - 1] == b' ' {
            j -= 1;
        }
        let end = j;
        while j > 0 && (b[j - 1].is_ascii_digit() || b[j - 1] == b'.') {
            j -= 1;
        }
        if j == end {
            continue;
        }
        if let Ok(v) = line[j..end].parse::<f32>() {
            if (0.0..=100.0).contains(&v) {
                best = Some(v);
            }
        }
    }
    best
}

fn fraction_in(line: &str) -> Option<f32> {
    let mut best = None;
    for (open, close) in [('[', ']'), ('(', ')')] {
        let mut rest = line;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + 1..];
            let Some(j) = after.find(close) else { break };
            let inner = after[..j].trim();
            if let Some((a, b)) = inner.split_once('/') {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
                    if b > 0.0 && a >= 0.0 && a <= b {
                        best = Some(a / b * 100.0);
                    }
                }
            }
            rest = &after[j + 1..];
        }
    }
    best
}

#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 1..];
    let mut t = rest.split_whitespace();
    let state = t.next()?.chars().next()?;
    let start = t.nth(18)?.parse().ok()?;
    Some((state, start))
}

/// Without `/proc` (macOS) `ps` gives the state letter and the start time text; the text is folded into a number that stays
/// the same for the life of the process, which is all `alive` needs to tell it from a later process with the same pid.
#[cfg(not(target_os = "linux"))]
fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let out = std::process::Command::new("ps").args(["-o", "stat=,lstart=", "-p", &pid.to_string()]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    let state = text.chars().next()?;
    let started = text.split_once(char::is_whitespace)?.1.split_whitespace().collect::<Vec<_>>().join(" ");
    // FNV-1a, never zero (zero means "not recorded")
    let h = started.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    Some((state, h.max(1)))
}

/// The program name of `pid` (`/proc/<pid>/comm`; `ps` on macOS), or `None` when there is no such process.
#[cfg(target_os = "linux")]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|c| c.trim().to_string())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let name = text.trim().rsplit('/').next()?.to_string();
    out.status.success().then_some(name).filter(|n| !n.is_empty())
}

/// Does a process with this id exist (any user)? Signal 0 only checks; "not permitted" still means it exists.
pub(crate) fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 sends nothing.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

fn alive(pid: u32, start_ticks: u64) -> bool {
    match proc_stat(pid) {
        Some((state, start)) => state != 'Z' && state != 'X' && (start_ticks == 0 || start == start_ticks),
        None => false,
    }
}

/// CPU time (user + system, in clock ticks) used by every process of the process group `pgid` right now (jobs run in a group of their
/// own). `None` where there is no `/proc`.
#[cfg(target_os = "linux")]
fn group_cpu_ticks(pgid: u32) -> Option<u64> {
    let mut sum = 0u64;
    for e in fs::read_dir("/proc").ok()?.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { continue };
        let Ok(text) = fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let Some(rest) = text.rfind(')').map(|i| &text[i + 1..]) else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        if f.get(2).and_then(|g| g.parse::<u32>().ok()) == Some(pgid) {
            sum += f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        }
    }
    Some(sum)
}

#[cfg(not(target_os = "linux"))]
fn group_cpu_ticks(_pgid: u32) -> Option<u64> {
    None
}

/// Per job: the CPU ticks last seen and when they last grew (ms).
static CPU_SEEN: Mutex<Option<std::collections::HashMap<String, (u64, u64)>>> = Mutex::new(None);

/// When this running job last did work, in ms: the later of its log's last write and the last time its group's CPU time grew. The first look
/// counts as active (it has just been seen), so a job is never called quiet before it has been watched for a while.
fn last_active(id: &str, pgid: u32, log_modified_ms: u64) -> u64 {
    let now = now_ms();
    let mut g = CPU_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(Default::default);
    let cpu_at = match group_cpu_ticks(pgid) {
        Some(t) => {
            let e = map.entry(id.to_string()).or_insert((t, now));
            if t > e.0 {
                *e = (t, now);
            }
            e.1
        }
        None => 0,
    };
    log_modified_ms.max(cpu_at)
}

fn group_alive(pgid: u32) -> bool {
    // SAFETY: signal 0 only checks that a process of the group exists.
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

fn signal_group(pgid: u32, sig: i32) {
    if pgid > 1 {
        // SAFETY: signalling the process group of a job we started with setsid (its id is the leader's pid).
        unsafe { libc::kill(-(pgid as i32), sig) };
    }
}

fn read_tail(path: &Path, max: u64) -> Vec<u8> {
    let Ok(mut f) = File::open(path) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(max);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(max).read_to_end(&mut buf);
    // do not begin in the middle of a character
    let skip = if start > 0 { buf.iter().take(4).take_while(|b| (**b & 0xC0) == 0x80).count() } else { 0 };
    buf.split_off(skip)
}

impl Jobs {
    pub fn new(root: &Path) -> Jobs {
        Jobs { root: root.to_path_buf(), cap: LOG_CAP, keep: LOG_KEEP, lock: Arc::new(Mutex::new(())) }
    }

    /// Smaller log limits (tests).
    pub fn with_log_limits(mut self, cap: u64, keep: u64) -> Jobs {
        self.cap = cap;
        self.keep = keep.min(cap);
        self
    }

    fn user_dir(&self, owner: &str) -> PathBuf {
        self.root.join(clean_owner(owner))
    }

    fn dir(&self, owner: &str, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("`{id}` is not a job id (they look like j1, j2)"));
        }
        let d = self.user_dir(owner).join(id);
        if d.join("meta.json").is_file() { Ok(d) } else { Err(format!("no job {id}")) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read_meta(dir: &Path) -> Option<Meta> {
        serde_json::from_slice(&fs::read(dir.join("meta.json")).ok()?).ok()
    }

    fn write_meta(dir: &Path, m: &Meta) -> Result<(), String> {
        let tmp = dir.join("meta.json.tmp");
        fs::write(&tmp, serde_json::to_vec(m).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        fs::rename(&tmp, dir.join("meta.json")).map_err(|e| e.to_string())
    }

    /// Start a job. `scope` is asked about the working folder first (the person's file scope); nothing starts if it refuses.
    pub fn start(&self, owner: &str, spec: &Spec, scope: &dyn Fn(&Path) -> Result<(), String>) -> Result<Job, String> {
        let text = spec.cmd.text();
        if text.trim().is_empty() || matches!(&spec.cmd, Cmd::Argv(a) if a.first().is_none_or(|p| p.is_empty())) {
            return Err("a job needs a command".into());
        }
        if text.len() > MAX_COMMAND || text.contains('\0') {
            return Err(format!("the command is longer than {MAX_COMMAND} bytes or holds a NUL"));
        }
        let cwd = fs::canonicalize(&spec.cwd).map_err(|_| format!("no such folder: {}", spec.cwd.display()))?;
        if !cwd.is_dir() {
            return Err(format!("{} is not a folder", cwd.display()));
        }
        scope(&cwd)?;
        let _g = self.lock();
        let ud = self.user_dir(owner);
        fs::create_dir_all(&ud).map_err(|e| e.to_string())?;
        let running = self.list_locked(owner).iter().filter(|j| j.running()).count();
        if running >= MAX_RUNNING {
            return Err(format!("{running} jobs are running already (at most {MAX_RUNNING}): wait for one or stop one"));
        }
        self.prune_locked(owner);
        // ids count up and are never reused while their folder exists
        let counter = ud.join("next");
        let n = fs::read_to_string(&counter).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(1).max(1);
        let id = format!("j{n}");
        let _ = fs::write(&counter, (n + 1).to_string());
        let dir = ud.join(&id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let log = OpenOptions::new().create(true).append(true).open(dir.join("log.txt")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let exit = dir.join("exit");
        // the wrapper runs the command and records its exit code, so the code is known even when the shell has gone
        const WRAP: &str = "f=\"$1\"; shift; \"$@\"; c=$?; printf '%s' \"$c\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; exit \"$c\"";
        let mut c = Command::new("sh");
        c.arg("-c").arg(WRAP).arg("sh").arg(&exit);
        match &spec.cmd {
            Cmd::Shell(line) => {
                c.args(["sh", "-c", line]);
            }
            Cmd::Argv(a) => {
                c.args(a);
            }
        }
        c.current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(log2)).env_clear();
        for (k, v) in std::env::vars() {
            if passes(&k) {
                c.env(k, v);
            }
        }
        c.env("IYAKKI_JOB_ID", &id);
        // SAFETY: setsid is async-signal-safe and the only thing done between fork and exec.
        unsafe {
            c.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
        }
        let child = match c.spawn() {
            Ok(ch) => ch,
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(format!("could not start the job: {e}"));
            }
        };
        let pid = child.id();
        let label: String = if spec.label.trim().is_empty() { text.chars().take(MAX_LABEL).collect() } else { spec.label.trim().chars().take(MAX_LABEL).collect() };
        let meta = Meta {
            id: id.clone(),
            label: label.chars().filter(|c| !c.is_control()).collect(),
            command: text,
            cwd: cwd.display().to_string(),
            owner: owner.into(),
            workspace: spec.workspace.clone(),
            pid,
            start_ticks: proc_stat(pid).map_or(0, |s| s.1),
            started_ms: now_ms(),
            ..Default::default()
        };
        Self::write_meta(&dir, &meta)?;
        // the child is reaped by this thread when it ends here; the job itself does not depend on it (a new shell adopts it)
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        Ok(self.load(&dir, &meta))
    }

    /// The job as it is now (state from the files and /proc). Records the end time the first time a finished job is seen.
    fn load(&self, dir: &Path, m: &Meta) -> Job {
        let exit = fs::read_to_string(dir.join("exit")).ok().and_then(|s| s.trim().parse::<i32>().ok());
        let state = if let Some(code) = exit {
            if code == 0 { State::Succeeded } else if m.stop_requested { State::Stopped } else { State::Failed(code) }
        } else if alive(m.pid, m.start_ticks) {
            State::Running
        } else if m.stop_requested {
            State::Stopped
        } else {
            State::Lost
        };
        let mut ended = m.ended_ms;
        if state.finished() && ended == 0 {
            ended = fs::metadata(dir.join("exit")).ok().and_then(|x| x.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or_else(now_ms, |d| d.as_millis() as u64);
            // file times come from a coarser clock and can read a few ms before the recorded start
            ended = ended.max(m.started_ms);
            let mut m2 = m.clone();
            m2.ended_ms = ended;
            let _ = Self::write_meta(dir, &m2);
        }
        let peek = read_tail(&dir.join("log.txt"), PEEK);
        let progress = if state == State::Succeeded { Some(100.0) } else { progress_hint(&String::from_utf8_lossy(&peek)) };
        let cleaned = clean_output(&peek);
        let last = cleaned.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let last_line: String = crate::bugs::redact(last).chars().take(160).collect();
        let last_active_ms = if state == State::Running {
            let log_ms = fs::metadata(dir.join("log.txt")).ok().and_then(|x| x.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            last_active(&m.id, m.pid, log_ms.max(m.started_ms))
        } else {
            0
        };
        Job { id: m.id.clone(), label: m.label.clone(), command: crate::bugs::redact(&m.command), cwd: m.cwd.clone(), owner: m.owner.clone(), workspace: m.workspace.clone(), pid: m.pid, state, started_ms: m.started_ms, ended_ms: ended, progress, last_line, last_active_ms, dismissed: m.dismissed }
    }

    fn list_locked(&self, owner: &str) -> Vec<Job> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.user_dir(owner)).into_iter().flatten().flatten() {
            let d = e.path();
            if let Some(m) = Self::read_meta(&d) {
                if m.id.is_empty() || e.file_name().to_string_lossy() != m.id {
                    continue;
                }
                let j = self.load(&d, &m);
                if j.running() {
                    self.trim_log(&d.join("log.txt"));
                }
                out.push(j);
            }
        }
        out.sort_by(|a, b| b.started_ms.cmp(&a.started_ms).then(b.id.len().cmp(&a.id.len())).then(b.id.cmp(&a.id)));
        out
    }

    /// All of a person's jobs, newest first (running ones also get their log trimmed when it grew too big).
    pub fn list(&self, owner: &str) -> Vec<Job> {
        let _g = self.lock();
        self.list_locked(owner)
    }

    /// The jobs of one workspace (or of no workspace when `workspace` is empty), not dismissed.
    pub fn visible(&self, owner: &str, workspace: &str) -> Vec<Job> {
        self.list(owner).into_iter().filter(|j| !j.dismissed && j.workspace == workspace).collect()
    }

    /// Every job of the person that is not dismissed, in any workspace (the full list in RUNS).
    pub fn visible_all(&self, owner: &str) -> Vec<Job> {
        self.list(owner).into_iter().filter(|j| !j.dismissed).collect()
    }

    pub fn running_count(&self, owner: &str) -> usize {
        self.list(owner).iter().filter(|j| j.running()).count()
    }

    pub fn get(&self, owner: &str, id: &str) -> Result<Job, String> {
        let _g = self.lock();
        let d = self.dir(owner, id)?;
        let m = Self::read_meta(&d).ok_or_else(|| format!("no job {id}"))?;
        Ok(self.load(&d, &m))
    }

    fn prune_locked(&self, owner: &str) {
        let done: Vec<Job> = self.list_locked(owner).into_iter().filter(|j| j.state.finished()).collect();
        for j in done.iter().skip(KEEP_FINISHED) {
            let _ = fs::remove_dir_all(self.user_dir(owner).join(&j.id));
        }
    }

    /// Keep the tail of a log that grew past the cap. The command writes in append mode, so rewriting the start in place is safe.
    fn trim_log(&self, path: &Path) {
        let Ok(meta) = fs::metadata(path) else { return };
        if meta.len() <= self.cap {
            return;
        }
        let tail = read_tail(path, self.keep);
        let Ok(mut f) = OpenOptions::new().write(true).open(path) else { return };
        let note = b"[earlier output was dropped to keep the log small]\n";
        if f.set_len(0).is_ok() && f.seek(SeekFrom::Start(0)).is_ok() {
            let _ = f.write_all(note);
            let _ = f.write_all(&tail);
        }
    }

    /// The end of the log: cleaned, redacted, at most `lines` lines and `MAX_TAIL_BYTES` bytes.
    pub fn tail(&self, owner: &str, id: &str, lines: usize) -> Result<String, String> {
        let d = {
            let _g = self.lock();
            let d = self.dir(owner, id)?;
            self.trim_log(&d.join("log.txt"));
            d
        };
        let lines = lines.clamp(1, MAX_TAIL_LINES);
        let raw = read_tail(&d.join("log.txt"), MAX_TAIL_BYTES as u64);
        let text = crate::bugs::redact(&clean_output(&raw));
        let all: Vec<&str> = text.lines().collect();
        let from = all.len().saturating_sub(lines);
        Ok(all[from..].join("\n"))
    }

    /// The whole kept log for the log view, cleaned and redacted (at most `max` bytes from the end).
    pub fn log_text(&self, owner: &str, id: &str, max: usize) -> Result<String, String> {
        let d = {
            let _g = self.lock();
            let d = self.dir(owner, id)?;
            self.trim_log(&d.join("log.txt"));
            d
        };
        let raw = read_tail(&d.join("log.txt"), max.min(LOG_CAP as usize) as u64);
        Ok(crate::bugs::redact(&clean_output(&raw)))
    }

    /// Append one line to a job's log (a later step that belongs to the job, such as the vault handoff).
    pub fn append_log(&self, owner: &str, id: &str, line: &str) -> Result<(), String> {
        use std::io::Write;
        let d = { let _g = self.lock(); self.dir(owner, id)? };
        let mut f = fs::OpenOptions::new().create(true).append(true).open(d.join("log.txt")).map_err(|e| e.to_string())?;
        writeln!(f, "{line}").map_err(|e| e.to_string())
    }

    pub fn log_len(&self, owner: &str, id: &str) -> u64 {
        self.dir(owner, id).ok().and_then(|d| fs::metadata(d.join("log.txt")).ok()).map_or(0, |m| m.len())
    }

    /// Stop a job: TERM to its whole group, KILL after `grace` for whatever is left. Returns once the group is gone (or after
    /// the KILL). Stopping a finished job is not an error.
    pub fn stop(&self, owner: &str, id: &str, grace: Duration) -> Result<Job, String> {
        let (d, m) = {
            let _g = self.lock();
            let d = self.dir(owner, id)?;
            let mut m = Self::read_meta(&d).ok_or_else(|| format!("no job {id}"))?;
            if !self.load(&d, &m).running() {
                return Ok(self.load(&d, &m));
            }
            m.stop_requested = true;
            Self::write_meta(&d, &m)?;
            (d, m)
        };
        signal_group(m.pid, libc::SIGTERM);
        let t = Instant::now();
        while group_alive(m.pid) && t.elapsed() < grace {
            std::thread::sleep(Duration::from_millis(25));
        }
        if group_alive(m.pid) {
            signal_group(m.pid, libc::SIGKILL);
            let t = Instant::now();
            while group_alive(m.pid) && t.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let _g = self.lock();
        let m = Self::read_meta(&d).unwrap_or(m);
        Ok(self.load(&d, &m))
    }

    /// Wait for a job to end, at most `timeout` (never more than `MAX_WAIT_SECS`); returns as soon as it ends.
    pub fn wait(&self, owner: &str, id: &str, timeout: Duration) -> Result<Job, String> {
        let end = Instant::now() + timeout.min(Duration::from_secs(MAX_WAIT_SECS));
        loop {
            let j = self.get(owner, id)?;
            if j.state.finished() || Instant::now() >= end {
                return Ok(j);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Hide a finished job from the strip (its files stay until they age out).
    pub fn dismiss(&self, owner: &str, id: &str) -> Result<(), String> {
        let _g = self.lock();
        let d = self.dir(owner, id)?;
        let mut m = Self::read_meta(&d).ok_or_else(|| format!("no job {id}"))?;
        if self.load(&d, &m).running() {
            return Err("a running job cannot be dismissed: stop it first".into());
        }
        m.dismissed = true;
        Self::write_meta(&d, &m)
    }

    pub fn dismiss_finished(&self, owner: &str, workspace: &str) {
        for j in self.visible(owner, workspace).iter().filter(|j| j.state.finished()) {
            let _ = self.dismiss(owner, &j.id);
        }
    }

    /// Jobs that finished since this channel was last told, once each (the workspace filter keeps one project's news out of
    /// another's chat).
    pub fn take_finished(&self, owner: &str, workspace: &str, who: Channel) -> Vec<Job> {
        let _g = self.lock();
        let mut out = Vec::new();
        for j in self.list_locked(owner) {
            if !j.state.finished() || j.workspace != workspace {
                continue;
            }
            let Ok(d) = self.dir(owner, &j.id) else { continue };
            let Some(mut m) = Self::read_meta(&d) else { continue };
            let seen = match who {
                Channel::Ui => &mut m.announced_ui,
                Channel::Agent => &mut m.announced_agent,
                Channel::Inbox => &mut m.announced_inbox,
            };
            if *seen {
                continue;
            }
            *seen = true;
            let _ = Self::write_meta(&d, &m);
            out.push(j);
        }
        out.sort_by_key(|j| j.ended_ms);
        out
    }

    /// Like [`Jobs::take_finished`] for every workspace (the inbox note is not tied to one chat).
    pub fn take_finished_all(&self, owner: &str, who: Channel) -> Vec<Job> {
        let workspaces: std::collections::BTreeSet<String> = self.list(owner).into_iter().map(|j| j.workspace).collect();
        let mut out: Vec<Job> = workspaces.iter().flat_map(|w| self.take_finished(owner, w, who)).collect();
        out.sort_by_key(|j| j.ended_ms);
        out
    }

    /// Whether the assistant was already told this job ended (a note at its turn, a wait or a stop). The pending-work ledger
    /// uses it to wake the chat only for what the assistant has not seen.
    pub fn agent_told(&self, owner: &str, id: &str) -> bool {
        let _g = self.lock();
        self.dir(owner, id).ok().and_then(|d| Self::read_meta(&d)).is_some_and(|m| m.announced_agent)
    }

    /// The assistant already has this job's result (it waited for it or stopped it): no note is sent later.
    pub fn mark_announced(&self, owner: &str, id: &str, who: Channel) {
        let _g = self.lock();
        let Ok(d) = self.dir(owner, id) else { return };
        let Some(mut m) = Self::read_meta(&d) else { return };
        match who {
            Channel::Ui => m.announced_ui = true,
            Channel::Agent => m.announced_agent = true,
            Channel::Inbox => m.announced_inbox = true,
        }
        let _ = Self::write_meta(&d, &m);
    }

    /// The text the assistant is given about a finished job.
    pub fn finish_note(&self, j: &Job) -> String {
        let tail = self.tail(&j.owner, &j.id, 12).unwrap_or_default();
        format!("JOB FINISHED: {} ({}) {} after {}. Last output:\n{}", j.label, j.id, j.state.label(), fmt_duration(j.elapsed_secs(now_ms())), if tail.is_empty() { "(no output)".to_string() } else { tail })
    }

    /// The same facts on one line, for the chat's visible event.
    pub fn finish_line(j: &Job) -> String {
        let mark = match j.state {
            State::Succeeded => "finished",
            State::Failed(_) => "failed",
            State::Stopped => "was stopped",
            _ => "was lost",
        };
        format!("Job “{}” ({}) {mark}: {} after {}", j.label, j.id, j.state.label(), fmt_duration(j.elapsed_secs(now_ms())))
    }
}

/// Why starting this shell line always asks, even in Auto: the same list `run_command` keeps (destructive programs and verbs,
/// anything outside the workspace), applied to every command of the line (split at `;`, `&`, `|`, newlines and `$(`).
pub fn hard_ask_line(line: &str) -> Option<&'static str> {
    let spaced = line.replace("$(", " ; ").replace('`', " ; ");
    for part in spaced.split([';', '&', '|', '\n', '(', ')']) {
        let words = crate::wsexec::split_command(part);
        let mut it = words.iter().skip_while(|w| w.contains('=') && !w.starts_with('/') && !w.starts_with('-'));
        let Some(prog) = it.next() else { continue };
        let args: Vec<String> = it.cloned().collect();
        if let Some(why) = crate::agent::hard_ask("run_command", &serde_json::json!({"program": prog, "args": args})) {
            return Some(why);
        }
    }
    None
}

/// The same check for a program with arguments.
pub fn hard_ask_argv(argv: &[String]) -> Option<&'static str> {
    let prog = argv.first()?;
    crate::agent::hard_ask("run_command", &serde_json::json!({"program": prog, "args": argv.iter().skip(1).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary folder removed when the test ends, whatever happens.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let d = std::env::temp_dir().join(format!("iyakki-jobs-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(d.join("work")).unwrap();
            Tmp(d)
        }
        fn jobs(&self) -> Jobs {
            Jobs::new(&self.0.join("jobs"))
        }
        fn work(&self) -> PathBuf {
            self.0.join("work")
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ok(_: &Path) -> Result<(), String> {
        Ok(())
    }
    fn sh(t: &Tmp, label: &str, line: &str) -> Spec {
        Spec { label: label.into(), cmd: Cmd::Shell(line.into()), cwd: t.work(), workspace: String::new() }
    }
    fn wait_done(j: &Jobs, id: &str) -> Job {
        j.wait("alice", id, Duration::from_secs(20)).unwrap()
    }

    #[test]
    fn a_silent_idle_job_goes_quiet_while_a_busy_one_and_a_talking_one_stay_active() {
        let t = Tmp::new("quiet");
        let j = t.jobs();
        j.start("alice", &sh(&t, "idle", "sleep 30"), &ok).unwrap();
        j.start("alice", &sh(&t, "busy", "while :; do :; done"), &ok).unwrap();
        j.start("alice", &sh(&t, "talker", "while :; do echo tick; sleep 0.2; done"), &ok).unwrap();
        let first: Vec<Job> = ["j1", "j2", "j3"].iter().map(|id| j.get("alice", id).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(2500));
        let later: Vec<Job> = ["j1", "j2", "j3"].iter().map(|id| j.get("alice", id).unwrap()).collect();
        let now = now_ms();
        assert!(later.iter().all(|x| x.running()), "{later:?}");
        assert_eq!(later[0].last_active_ms, first[0].last_active_ms, "no output and no CPU: the sign of work does not move");
        assert!(later[0].quiet_secs(now) >= 2, "the idle job is quiet: {}", later[0].quiet_secs(now));
        assert!(later[1].last_active_ms > first[1].last_active_ms && later[1].quiet_secs(now) <= 1, "CPU use counts as work");
        assert!(later[2].last_active_ms > first[2].last_active_ms && later[2].quiet_secs(now) <= 1, "output counts as work");
        for id in ["j1", "j2", "j3"] {
            let _ = j.stop("alice", id, Duration::from_millis(200));
        }
        // a job that is not running has no quiet time
        assert_eq!(j.get("alice", "j1").unwrap().quiet_secs(now_ms()), 0);
    }

    #[test]
    fn a_job_runs_detached_and_succeeds_with_its_output_in_the_log() {
        let t = Tmp::new("ok");
        let j = t.jobs();
        let job = j.start("alice", &sh(&t, "greet", "echo hello; echo world >&2"), &ok).unwrap();
        assert_eq!(job.id, "j1");
        let done = wait_done(&j, "j1");
        assert_eq!(done.state, State::Succeeded);
        assert_eq!(done.progress, Some(100.0));
        assert_eq!(done.last_line, "world");
        let tail = j.tail("alice", "j1", 10).unwrap();
        assert!(tail.contains("hello") && tail.contains("world"), "{tail}");
        assert!(done.ended_ms >= done.started_ms && done.ended_ms > 0);
    }

    #[test]
    fn exit_codes_are_kept_and_an_argv_job_needs_no_shell() {
        let t = Tmp::new("exit");
        let j = t.jobs();
        j.start("alice", &sh(&t, "bad", "echo nope; exit 7"), &ok).unwrap();
        assert_eq!(wait_done(&j, "j1").state, State::Failed(7));
        j.start("alice", &Spec { label: "argv".into(), cmd: Cmd::Argv(vec!["echo".into(), "a b".into(), "$HOME".into()]), cwd: t.work(), workspace: String::new() }, &ok).unwrap();
        assert_eq!(wait_done(&j, "j2").state, State::Succeeded);
        let tail = j.tail("alice", "j2", 5).unwrap();
        assert_eq!(tail.trim(), "a b $HOME", "no shell expansion");
        j.start("alice", &Spec { label: "missing".into(), cmd: Cmd::Argv(vec!["no-such-program-xyz".into()]), cwd: t.work(), workspace: String::new() }, &ok).unwrap();
        assert_eq!(wait_done(&j, "j3").state, State::Failed(127));
    }

    #[test]
    fn a_running_job_survives_dropping_every_handle_and_a_new_manager_adopts_it() {
        let t = Tmp::new("adopt");
        {
            let j = t.jobs();
            j.start("alice", &sh(&t, "slow", "sleep 1; echo done-late"), &ok).unwrap();
            assert!(j.get("alice", "j1").unwrap().running());
        } // every handle is gone: the shell "restarted"
        let j2 = t.jobs();
        let seen = j2.list("alice");
        assert_eq!(seen.len(), 1);
        assert!(seen[0].running(), "the job is still running and found again");
        let done = wait_done(&j2, "j1");
        assert_eq!(done.state, State::Succeeded);
        assert!(j2.tail("alice", "j1", 3).unwrap().contains("done-late"));
        // finished jobs are found again as well, and a new job gets the next id
        let j3 = t.jobs();
        assert_eq!(j3.list("alice")[0].state, State::Succeeded);
        assert_eq!(j3.start("alice", &sh(&t, "next", "true"), &ok).unwrap().id, "j2");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_job_runs_in_its_own_session_and_group() {
        let t = Tmp::new("session");
        let j = t.jobs();
        let job = j.start("alice", &sh(&t, "sess", "sleep 5"), &ok).unwrap();
        let sid = fs::read_to_string(format!("/proc/{}/stat", job.pid)).unwrap();
        let rest: Vec<&str> = sid[sid.rfind(')').unwrap() + 1..].split_whitespace().collect();
        // fields after the name: state ppid pgrp session
        assert_eq!(rest[2].parse::<u32>().unwrap(), job.pid, "own process group");
        assert_eq!(rest[3].parse::<u32>().unwrap(), job.pid, "own session");
        j.stop("alice", "j1", Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn stop_ends_the_whole_group_including_children_and_marks_it_stopped() {
        let t = Tmp::new("stop");
        let j = t.jobs();
        let pidfile = t.work().join("child.pid");
        // a grandchild that would outlive its parent if only the leader were signalled
        let line = format!("sh -c 'sleep 300 & echo $! > {}; wait' & wait", pidfile.display());
        j.start("alice", &sh(&t, "tree", &line), &ok).unwrap();
        let t0 = Instant::now();
        while !pidfile.exists() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let child: u32 = fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(proc_stat(child).is_some_and(|s| s.0 != 'Z'), "the grandchild runs");
        let stopped = j.stop("alice", "j1", Duration::from_secs(2)).unwrap();
        assert_eq!(stopped.state, State::Stopped);
        let t1 = Instant::now();
        while proc_stat(child).is_some_and(|s| s.0 != 'Z') && t1.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(proc_stat(child).is_none_or(|s| s.0 == 'Z'), "the grandchild is gone too");
        // stopping again is not an error
        assert_eq!(j.stop("alice", "j1", Duration::from_millis(10)).unwrap().state, State::Stopped);
    }

    #[test]
    fn a_job_that_ignores_term_is_killed_after_the_grace_period() {
        let t = Tmp::new("kill");
        let j = t.jobs();
        j.start("alice", &sh(&t, "stubborn", "trap '' TERM; while true; do sleep 0.1; done"), &ok).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        let s = j.stop("alice", "j1", Duration::from_millis(400)).unwrap();
        assert_eq!(s.state, State::Stopped);
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_process_that_vanished_without_an_exit_record_is_lost() {
        let t = Tmp::new("lost");
        let j = t.jobs();
        let job = j.start("alice", &sh(&t, "victim", "sleep 300"), &ok).unwrap();
        // killed from outside (not by Stop): the wrapper never writes an exit code
        signal_group(job.pid, libc::SIGKILL);
        let t0 = Instant::now();
        while j.get("alice", "j1").unwrap().running() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(j.get("alice", "j1").unwrap().state, State::Lost);
        // a stale record whose pid now belongs to some other process is not "running" either
        let d = j.dir("alice", "j1").unwrap();
        let mut m = Jobs::read_meta(&d).unwrap();
        m.pid = std::process::id();
        m.start_ticks = 1;
        Jobs::write_meta(&d, &m).unwrap();
        assert_eq!(j.get("alice", "j1").unwrap().state, State::Lost, "pid reuse is detected by the start time");
    }

    #[test]
    fn percentages_and_fractions_in_the_last_lines_give_a_progress() {
        assert_eq!(progress_hint("starting\nDownloading system-images 42%"), Some(42.0));
        assert_eq!(progress_hint("x\n[=====>   ] 87.5 % of 1GB"), Some(87.5));
        assert_eq!(progress_hint("[ 42/100 ] Building foo.o"), Some(42.0));
        assert_eq!(progress_hint("[12/48] CXX bar.o"), Some(25.0));
        assert_eq!(progress_hint("compiling (3/4)"), Some(75.0));
        assert_eq!(progress_hint("a\rb 10%\rb 55%"), Some(55.0), "the newest redraw counts");
        assert_eq!(progress_hint("downloaded 250% of nothing"), None, "out of range");
        assert_eq!(progress_hint("date 12/10/2026 and 100 files"), None);
        assert_eq!(progress_hint("nothing to see"), None);
        assert_eq!(progress_hint("[ 5/0 ] broken"), None);
        // an old percentage far above the last lines does not count
        let long = format!("5%\n{}", "line\n".repeat(20));
        assert_eq!(progress_hint(&long), None);
    }

    #[test]
    fn a_running_job_reports_progress_from_its_output_and_clean_last_line() {
        let t = Tmp::new("prog");
        let j = t.jobs();
        j.start("alice", &sh(&t, "dl", "printf '\\033[32mDownloading\\033[0m 10%%\\rDownloading 63%%'; sleep 5"), &ok).unwrap();
        let t0 = Instant::now();
        loop {
            let job = j.get("alice", "j1").unwrap();
            if job.progress == Some(63.0) {
                assert_eq!(job.last_line, "Downloading 63%", "colours and redraws are cleaned");
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "no progress seen: {job:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
        j.stop("alice", "j1", Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn the_log_is_capped_keeping_the_tail() {
        let t = Tmp::new("cap");
        let j = t.jobs().with_log_limits(20_000, 4_000);
        j.start("alice", &sh(&t, "noisy", "i=0; while [ $i -lt 3000 ]; do echo \"line number $i with some padding text\"; i=$((i+1)); done; sleep 1"), &ok).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        // reading trims the running job's log
        let tail = j.tail("alice", "j1", 3).unwrap();
        assert!(j.log_len("alice", "j1") <= 20_000, "{}", j.log_len("alice", "j1"));
        assert!(tail.contains("line number 2999"), "the newest lines are kept: {tail}");
        let all = j.log_text("alice", "j1", 1 << 20).unwrap();
        assert!(all.starts_with("[earlier output was dropped"), "{}", &all[..60.min(all.len())]);
        assert!(!all.contains("line number 0 "));
        wait_done(&j, "j1");
    }

    #[test]
    fn tails_are_capped_and_credentials_never_appear_in_them() {
        let t = Tmp::new("redact");
        let j = t.jobs();
        j.start("alice", &sh(&t, "leaky", "echo 'DB_PASSWORD=hunter2hunter2'; echo 'token: abcdef123456'; echo 'Authorization: Bearer sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; echo ok-line"), &ok).unwrap();
        let done = wait_done(&j, "j1");
        assert_eq!(done.state, State::Succeeded);
        let tail = j.tail("alice", "j1", 100).unwrap();
        assert!(!tail.contains("hunter2") && !tail.contains("abcdef123456") && !tail.contains("sk-ant-api03"), "{tail}");
        assert!(tail.contains("ok-line"));
        assert!(!j.finish_note(&done).contains("hunter2"));
        // the cap on lines
        j.start("alice", &sh(&t, "many", "seq 1 1000"), &ok).unwrap();
        wait_done(&j, "j2");
        assert_eq!(j.tail("alice", "j2", 1_000_000).unwrap().lines().count(), MAX_TAIL_LINES);
        assert_eq!(j.tail("alice", "j2", 2).unwrap(), "999\n1000");
    }

    #[test]
    fn scope_refusal_starts_nothing_and_leaves_no_job_behind() {
        let t = Tmp::new("scope");
        let j = t.jobs();
        let deny = |p: &Path| -> Result<(), String> { Err(format!("your account may not use {}", p.display())) };
        let e = j.start("alice", &sh(&t, "x", "touch ran.txt"), &deny).unwrap_err();
        assert!(e.contains("may not use"), "{e}");
        assert!(j.list("alice").is_empty());
        assert!(!t.work().join("ran.txt").exists());
        let e = j.start("alice", &Spec { cwd: t.0.join("nope"), ..sh(&t, "x", "true") }, &ok).unwrap_err();
        assert!(e.contains("no such folder"), "{e}");
        assert!(j.start("alice", &sh(&t, "x", "  "), &ok).is_err());
    }

    #[test]
    fn people_only_see_their_own_jobs_and_ids_are_checked() {
        let t = Tmp::new("owners");
        let j = t.jobs();
        j.start("alice", &sh(&t, "mine", "true"), &ok).unwrap();
        assert!(j.get("bob", "j1").is_err());
        assert!(j.list("bob").is_empty());
        assert!(j.tail("bob", "j1", 5).is_err());
        assert!(j.stop("bob", "j1", Duration::from_millis(10)).is_err());
        for bad in ["../j1", "j1/../../x", "", "j", "jx", "j1; rm"] {
            assert!(j.get("alice", bad).is_err(), "{bad}");
        }
        assert!(j.list("../alice").is_empty(), "owner names are cleaned");
    }

    #[test]
    fn at_most_eight_run_at_once_and_old_finished_jobs_age_out() {
        let t = Tmp::new("limits");
        let j = t.jobs();
        for i in 0..MAX_RUNNING {
            j.start("alice", &sh(&t, &format!("s{i}"), "sleep 30"), &ok).unwrap();
        }
        let e = j.start("alice", &sh(&t, "one more", "true"), &ok).unwrap_err();
        assert!(e.contains("at most"), "{e}");
        for x in j.list("alice") {
            j.stop("alice", &x.id, Duration::from_secs(1)).unwrap();
        }
        for _ in 0..(KEEP_FINISHED + 5) {
            j.start("alice", &sh(&t, "quick", "true"), &ok).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
        j.start("alice", &sh(&t, "last", "true"), &ok).unwrap();
        assert!(j.list("alice").len() <= KEEP_FINISHED + 2, "{}", j.list("alice").len());
    }

    #[test]
    fn a_finished_job_is_announced_once_to_each_channel_and_can_be_dismissed() {
        let t = Tmp::new("announce");
        let j = t.jobs();
        j.start("alice", &sh(&t, "a", "echo fine"), &ok).unwrap();
        j.start("alice", &Spec { workspace: "proj".into(), ..sh(&t, "b", "true") }, &ok).unwrap();
        j.start("alice", &sh(&t, "c", "sleep 30"), &ok).unwrap();
        wait_done(&j, "j1");
        wait_done(&j, "j2");
        let ui = j.take_finished("alice", "", Channel::Ui);
        assert_eq!(ui.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["j1"], "only this workspace's finished job, not the running one");
        assert!(j.take_finished("alice", "", Channel::Ui).is_empty(), "once");
        assert_eq!(j.take_finished("alice", "", Channel::Agent).len(), 1, "the assistant is told separately");
        assert_eq!(j.take_finished("alice", "proj", Channel::Agent).len(), 1);
        let note = j.finish_note(&ui[0]);
        assert!(note.starts_with("JOB FINISHED: a (j1) succeeded") && note.contains("fine"), "{note}");
        assert!(Jobs::finish_line(&ui[0]).contains("succeeded"));
        assert_eq!(j.visible("alice", "").len(), 2);
        j.dismiss("alice", "j1").unwrap();
        assert_eq!(j.visible("alice", "").iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["j3"]);
        assert!(j.dismiss("alice", "j3").is_err(), "a running job is stopped, not dismissed");
        j.stop("alice", "j3", Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn wait_returns_early_when_the_job_ends_and_never_waits_past_its_limit() {
        let t = Tmp::new("wait");
        let j = t.jobs();
        j.start("alice", &sh(&t, "quick", "sleep 0.3"), &ok).unwrap();
        let t0 = Instant::now();
        let done = j.wait("alice", "j1", Duration::from_secs(30)).unwrap();
        assert!(done.state.finished() && t0.elapsed() < Duration::from_secs(10));
        j.start("alice", &sh(&t, "long", "sleep 30"), &ok).unwrap();
        let t1 = Instant::now();
        let still = j.wait("alice", "j2", Duration::from_millis(300)).unwrap();
        assert!(still.running() && t1.elapsed() < Duration::from_secs(5));
        j.stop("alice", "j2", Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn the_hard_ask_list_applies_to_every_command_of_a_line() {
        assert!(hard_ask_line("sdkmanager \"emulator\" \"platform-tools\"").is_none());
        assert!(hard_ask_line("cargo build --release && cargo test").is_none());
        assert!(hard_ask_line("yes | sdkmanager --licenses").is_none());
        assert!(hard_ask_line("make; rm -rf build").is_some(), "rm anywhere in the line");
        assert!(hard_ask_line("echo hi && sudo apt install x").is_some());
        assert!(hard_ask_line("cat /etc/passwd").is_some(), "outside the workspace");
        assert!(hard_ask_line("echo $(kubectl delete ns x)").is_some());
        assert!(hard_ask_line("A=1 rm file").is_some());
        assert!(hard_ask_argv(&["rm".into(), "x".into()]).is_some());
        assert!(hard_ask_argv(&["cargo".into(), "build".into()]).is_none());
    }

    #[test]
    fn output_cleaning_drops_escapes_and_keeps_the_last_redraw() {
        assert_eq!(clean_output(b"\x1b[1;32mok\x1b[0m done"), "ok done");
        assert_eq!(clean_output(b"a 10%\rb 99%\nnext"), "b 99%\nnext");
        assert_eq!(clean_output(b"win\x1b]0;title\x07dow"), "window");
        assert_eq!(clean_output(b"line\r\nnext"), "line\nnext");
        assert_eq!(clean_output("é\u{7}x".as_bytes()), "éx");
        assert_eq!(fmt_duration(5), "5s");
        assert_eq!(fmt_duration(192), "3m 12s");
        assert_eq!(fmt_duration(3900), "1h 05m");
    }
}
