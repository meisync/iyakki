//! Linux windows (phase 2 of `docs/LINUX-APPS.md`): a graphical program of the Linux apps sandbox shown in an IYAKKI window.
//!
//! Inside the sandbox (a Debian 12 named `gui`) one small script, written by IYAKKI, starts **Xvnc** (TigerVNC's X server: it draws
//! into memory and speaks RFB on a Unix socket), the one-window manager **matchbox** and the program. The socket is in a runtime
//! folder that is mounted into the sandbox and nothing else; IYAKKI's own RFB client (`iyakki-rfb`) connects to it from outside.
//! Everything inside is installed by `apt` at the person's click and never shipped. No GPU, no audio (software drawing only).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_proxy::Streamer;

use crate::linuxapps::{CreateReq, ExecOpts, LinuxApps};
use crate::rootfs;
use crate::wsexec::Sink;

/// The sandbox of the Linux windows.
pub const SANDBOX: &str = "gui";
pub const RELEASE: &str = "debian-12";
/// The runtime folder (inside the sandbox's own folder on the host) and where it appears inside.
pub const RUN_DIR: &str = "run";
pub const RUN_DST: &str = "/mnt/iyakki-display";
pub const SOCKET_NAME: &str = "display.sock";
/// The X display number inside the sandbox. Not `:0` or `:1`: a sandbox with its network on shares the host's abstract sockets, where a desktop's X server may
/// already be `:0` or `:1`, and X clients try the abstract socket first.
pub const DISPLAY_NUM: u32 = 73;
pub const SCRIPT_REL: &str = "usr/local/bin/iyakki-window-session";
pub const SCRIPT_IN: &str = "/usr/local/bin/iyakki-window-session";
/// Installed by the Setup item: the display server, the window manager, a font, software OpenGL and a small test program.
pub const DISPLAY_PACKAGES: [&str; 7] = ["tigervnc-standalone-server", "matchbox-window-manager", "fonts-dejavu-core", "xfonts-base", "libgl1-mesa-dri", "xauth", "xterm"];
/// Programs the window page offers to install with one click (package, what it is).
pub const TEST_PROGRAMS: [(&str, &str); 3] = [("xterm", "xterm: a small terminal window (MIT)"), ("x11-apps", "x11-apps: xclock, xeyes and friends (MIT)"), ("mousepad", "mousepad: a small text editor (GPL-2.0)")];
pub const MIN_SIZE: u32 = 64;
pub const MAX_SIZE: u32 = 8192;

pub const NOTE: &str = "Linux windows run a graphical program of a Debian sandbox in an IYAKKI window. Debian's packages (TigerVNC's X server, matchbox, fonts, Mesa and the programs you add) are downloaded by apt inside the sandbox at your click and never shipped; each keeps its own licence (/usr/share/doc/<package>/copyright). There is no graphics card and no sound in the window, so it is slower than a program on a normal computer.";

/// The script that starts the display and the program inside the sandbox. Fixed text: the sizes and the program come as separate
/// arguments (never pasted into a command string); sizes must be plain digits.
pub fn session_script() -> String {
    format!(
        r#"#!/bin/sh
# Written by IYAKKI (Linux windows). Usage: iyakki-window-session WIDTH HEIGHT PROGRAM [ARGUMENT...]
W="$1"; H="$2"
[ "$#" -ge 3 ] || exit 2
shift 2
case "$W$H" in ""|*[!0-9]*) exit 2;; esac
export HOME=/home/user DISPLAY=:{DISPLAY_NUM} XDG_RUNTIME_DIR=/tmp/xdg-run LIBGL_ALWAYS_SOFTWARE=1 GDK_BACKEND=x11 NO_AT_BRIDGE=1
export XAUTHORITY="$XDG_RUNTIME_DIR/Xauthority"
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
mkdir -p -m 1777 /tmp/.X11-unix
# no abstract socket (it would be in the host's network namespace when the network is on), and only clients that know a secret key
rm -f "$XAUTHORITY"
xauth -q -f "$XAUTHORITY" add :{DISPLAY_NUM} . "$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')" || exit 4
SOCK={RUN_DST}/{SOCKET_NAME}
rm -f "$SOCK"
Xvnc :{DISPLAY_NUM} -geometry "${{W}}x${{H}}" -depth 24 -rfbunixpath "$SOCK" -rfbport -1 -SecurityTypes None -nolisten tcp -nolisten local -auth "$XAUTHORITY" -AlwaysShared &
XPID=$!
i=0
while [ ! -S "$SOCK" ] && [ "$i" -lt 100 ]; do
  kill -0 "$XPID" 2>/dev/null || exit 3
  sleep 0.1
  i=$((i+1))
done
matchbox-window-manager -use_titlebar no -use_cursor no &
"$@" &
APP=$!
wait "$APP"
kill "$XPID" 2>/dev/null
exit 0
"#
    )
}

// ------------------------------------------------------------------------------------------------ installing

/// The sandbox exists and has the display server and the script.
pub fn ready(la: &LinuxApps) -> bool {
    la.get(SANDBOX).is_ok() && is_ready_root(&la.rootfs_of(SANDBOX))
}

pub fn is_ready_root(root: &Path) -> bool {
    // `Xvnc` is a link to `/etc/alternatives/Xvnc` in Debian (an absolute link, only meaningful inside the sandbox): a link counts, it is never followed
    std::fs::symlink_metadata(root.join("usr/bin/Xvnc")).is_ok() && root.join(SCRIPT_REL).is_file()
}

/// How many people have a ready Linux windows sandbox (for the Setup tab).
pub fn count_ready(data: &Path) -> usize {
    std::fs::read_dir(data.join(crate::linuxapps::DIR)).into_iter().flatten().flatten().filter(|u| u.path().is_dir()).filter(|u| u.path().join(SANDBOX).join("sandbox.json").is_file() && is_ready_root(&u.path().join(SANDBOX).join("rootfs"))).count()
}

fn write_script(la: &LinuxApps) -> Result<(), String> {
    rootfs::write_file_in(&la.rootfs_of(SANDBOX), SCRIPT_REL, session_script().as_bytes(), 0o755, (0, 0), la.id_shift(SANDBOX))
}

/// Make the `gui` sandbox when it is missing, install the display packages and the script. `sink` receives apt's output while it runs.
/// The network of a sandbox made here is switched off afterwards (programs that need the internet: turn it on on the Linux apps page).
pub fn install(la: &LinuxApps, st: &dyn Streamer, sink: Option<Sink>, say: &mut dyn FnMut(&str)) -> Result<String, String> {
    say(NOTE);
    la.note("window.install.start", SANDBOX);
    let mut made = false;
    let mut turned_on = false;
    match la.get(SANDBOX) {
        Ok(s) => {
            if !s.network {
                la.set_network(SANDBOX, true)?;
                turned_on = true;
            }
        }
        Err(_) => {
            say("[1/3] making the Debian 12 sandbox `gui`");
            let req = CreateReq { name: SANDBOX.into(), release: RELEASE.into(), network: true, mounts: vec![] };
            la.create(&req, st, &|_, _| Ok(()), &mut |l| say(l))?;
            made = true;
        }
    }
    let r = (|| -> Result<String, String> {
        say("[2/3] installing the display server, the window manager and a test program with apt");
        let pk: Vec<String> = DISPLAY_PACKAGES.iter().map(|s| s.to_string()).collect();
        let o = la.apt_install(SANDBOX, &pk, sink)?;
        if !o.ok() {
            return Err(apt_failure("the packages", &o.combined()));
        }
        write_script(la)?;
        if !is_ready_root(&la.rootfs_of(SANDBOX)) {
            return Err("the display server (Xvnc) is not in the sandbox after the install".into());
        }
        let v = la.exec(SANDBOX, &["dpkg-query".into(), "-W".into(), "-f=${Version}".into(), "tigervnc-standalone-server".into()], &ExecOpts { root: true, cwd: "/root".into(), timeout: Duration::from_secs(60), ..Default::default() }).ok().filter(|o| o.ok()).map(|o| o.stdout.trim().to_string()).unwrap_or_default();
        Ok(format!("[3/3] Linux windows are ready: display server {}, sandbox {} MB", if v.is_empty() { "installed".to_string() } else { v }, la.storage(SANDBOX) >> 20))
    })();
    if made || turned_on {
        let _ = la.set_network(SANDBOX, false);
    }
    match &r {
        Ok(l) => la.note("window.install", l),
        Err(e) => la.note("window.install.failed", e),
    }
    r
}

/// Install one of the offered test programs (a fixed list: the name is checked against it).
pub fn install_program(la: &LinuxApps, package: &str, sink: Option<Sink>) -> Result<(), String> {
    if !TEST_PROGRAMS.iter().any(|(p, _)| *p == package) {
        return Err(format!("`{package}` is not one of the offered programs; other packages: Linux apps page, Install"));
    }
    if !ready(la) {
        return Err("install Linux windows first (Store, Setup)".into());
    }
    let was_on = la.get(SANDBOX)?.network;
    if !was_on {
        la.set_network(SANDBOX, true)?;
    }
    let r = la.apt_install(SANDBOX, &[package.to_string()], sink);
    if !was_on {
        let _ = la.set_network(SANDBOX, false);
    }
    let o = r?;
    if o.ok() { Ok(()) } else { Err(apt_failure(package, &o.combined())) }
}

/// Remove the sandbox with everything in it (the name must be typed).
pub fn remove(la: &LinuxApps, typed: &str) -> Result<u64, String> {
    la.delete(SANDBOX, typed)
}

/// Why apt (run in the sandbox) failed, for the Installs row and the saved error. When the sandbox itself could not start, its own message
/// comes first and whole (`linuxapp-init: entering the sandbox failed: pivot_root: ...`), as one line that reads from the left; otherwise
/// the last lines of apt's output.
pub(crate) fn apt_failure(what: &str, output: &str) -> String {
    let init: Vec<&str> = output.lines().map(str::trim).filter_map(|l| l.strip_prefix("linuxapp-init:")).map(str::trim).filter(|l| !l.starts_with("note:")).collect();
    if let Some(first) = init.first() {
        return format!("the sandbox could not start: {first} (apt did not run)");
    }
    format!("apt could not install {what}: {}", last_lines(output, 6))
}

fn last_lines(t: &str, n: usize) -> String {
    let v: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    v[v.len().saturating_sub(n)..].join(" | ")
}

// ------------------------------------------------------------------------------------------------ the launcher

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// The file name without `.desktop`.
    pub id: String,
    pub name: String,
    /// The command as words (no shell): field codes removed.
    pub exec: Vec<String>,
}

const MAX_DESKTOP_FILES: usize = 400;
const MAX_DESKTOP_BYTES: usize = 64 * 1024;

/// The Exec line as words, by the Desktop Entry rules (double quotes group, a backslash escapes inside them), without the field
/// codes (`%f`, `%U`, ...). `%%` is a percent sign. None for an empty command or one with a control character.
pub fn split_exec(line: &str) -> Option<Vec<String>> {
    if line.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut words: Vec<String> = Vec::new();
    let (mut cur, mut have, mut quoted) = (String::new(), false, false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                quoted = !quoted;
                have = true;
            }
            '\\' if quoted => {
                if let Some(n) = it.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if have {
                    words.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if have {
        words.push(cur);
    }
    let mut out = Vec::new();
    for w in words {
        let mut r = String::new();
        let mut chars = w.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                match chars.next() {
                    Some('%') => r.push('%'),
                    Some(_) | None => {}
                }
            } else {
                r.push(c);
            }
        }
        if !r.is_empty() {
            out.push(r);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// One `.desktop` file: only the `[Desktop Entry]` group; applications that are shown, need no terminal and have a command.
pub fn parse_desktop(id: &str, text: &str) -> Option<Program> {
    let (mut in_entry, mut name, mut exec, mut kind) = (false, None::<String>, None::<String>, String::new());
    let (mut hidden, mut term) = (false, false);
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if !in_entry || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        match k.trim() {
            "Type" => kind = v.trim().to_string(),
            "Name" => name = Some(v.trim().chars().filter(|c| !c.is_control()).take(80).collect()),
            "Exec" => exec = Some(v.trim().to_string()),
            "NoDisplay" | "Hidden" if v.trim() == "true" => hidden = true,
            "Terminal" if v.trim() == "true" => term = true,
            _ => {}
        }
    }
    if kind != "Application" || hidden || term {
        return None;
    }
    let exec = split_exec(&exec?)?;
    let name = name.filter(|n: &String| !n.is_empty()).unwrap_or_else(|| id.to_string());
    Some(Program { id: id.to_string(), name, exec })
}

/// The graphical programs of the sandbox, read from `usr/share/applications` of its root without following a link (strict reader rules):
/// at most 400 files of 64 KiB, sorted by name.
pub fn programs(rootfs: &Path) -> Vec<Program> {
    let mut dir = rootfs.to_path_buf();
    for part in ["usr", "share", "applications"] {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            _ => return Vec::new(),
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".desktop")).collect();
    names.sort();
    names.truncate(MAX_DESKTOP_FILES);
    let mut out = Vec::new();
    for n in names {
        let Some(bytes) = rootfs::read_file_in(rootfs, &format!("usr/share/applications/{n}")) else { continue };
        if bytes.len() > MAX_DESKTOP_BYTES {
            continue;
        }
        let id = n.trim_end_matches(".desktop");
        if let Some(p) = parse_desktop(id, &String::from_utf8_lossy(&bytes)) {
            out.push(p);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ running a window

/// A started program (the host side of the sandbox command).
pub trait Proc: Send {
    fn pid(&self) -> Option<u32>;
    fn alive(&mut self) -> bool;
    /// Stop it and wait; the sandbox's own processes die with it (its pid namespace ends).
    fn kill(&mut self);
}

/// Starts a command without waiting. The real one is [`System`]; tests give a fake.
pub trait Spawn: Send + Sync {
    fn spawn(&self, argv: &[String], cwd: &Path, log: &Path) -> Result<Box<dyn Proc>, String>;
}

/// Starts real processes (the one process-start site of this module).
pub struct System;

struct Child(std::process::Child);

impl Proc for Child {
    fn pid(&self) -> Option<u32> {
        Some(self.0.id())
    }
    fn alive(&mut self) -> bool {
        iyakki_core_api::child_status(&mut self.0).is_running()
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Spawn for System {
    fn spawn(&self, argv: &[String], cwd: &Path, log: &Path) -> Result<Box<dyn Proc>, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let f2 = f.try_clone().map_err(|e| e.to_string())?;
        let child = std::process::Command::new(argv.first().ok_or("no command")?)
            .args(&argv[1..])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(f)
            .stderr(f2)
            .spawn()
            .map_err(|e| format!("could not start the sandbox: {e}"))?;
        Ok(Box::new(Child(child)))
    }
}

/// The exact argument array of a window: the script, the size, then the program's words.
pub fn session_command(w: u32, h: u32, program: &[String]) -> Result<Vec<String>, String> {
    if program.is_empty() || program.iter().any(|a| a.is_empty() || a.contains('\0')) {
        return Err("no program".into());
    }
    let (w, h) = (w.clamp(MIN_SIZE, MAX_SIZE), h.clamp(MIN_SIZE, MAX_SIZE));
    let mut v = vec![SCRIPT_IN.to_string(), w.to_string(), h.to_string()];
    v.extend(program.iter().cloned());
    Ok(v)
}

pub struct Window {
    la_name: String,
    run_dir: PathBuf,
    log: PathBuf,
    proc: Option<Box<dyn Proc>>,
    pub program: String,
    pub started: Instant,
    audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    pub argv: Vec<String>,
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Window({})", self.program)
    }
}

impl Window {
    /// Start `program` (a launcher entry) in a window of `w` x `h` pixels. Returns at once; [`Window::wait_socket`] waits for the display.
    pub fn start(la: &LinuxApps, spawn: &dyn Spawn, program: &Program, w: u32, h: u32) -> Result<Window, String> {
        if !ready(la) {
            return Err("Linux windows are not installed: Store, Setup, Linux windows".into());
        }
        let cmd = session_command(w, h, &program.exec)?;
        let own = la.dir_of(SANDBOX);
        let run_dir = own.join(RUN_DIR);
        rootfs::make_dir_in(&own, RUN_DIR, 0o700, (1000, 1000), la.id_shift(SANDBOX))?;
        let sock = run_dir.join(SOCKET_NAME);
        if sock.as_os_str().len() > 100 {
            return Err("the data folder path is too long for a socket".into());
        }
        let _ = std::fs::remove_file(&sock);
        let opts = ExecOpts { root: false, timeout: Duration::from_secs(24 * 3600), sink: None, env: vec![], cwd: "/home/user".into() };
        let argv = la.exec_argv_with_run(SANDBOX, &cmd, &opts, &run_dir, RUN_DST)?;
        la.note("window.start", &format!("{}: {} ({} argument(s)) {}x{}", SANDBOX, program.id, program.exec.len() - 1, w, h));
        let log = own.join("session.log");
        let proc = spawn.spawn(&argv, &own, &log)?;
        let nla = la.clone_audit();
        Ok(Window { la_name: SANDBOX.into(), run_dir, log, proc: Some(proc), program: program.id.clone(), started: Instant::now(), audit: nla, argv })
    }

    pub fn socket(&self) -> PathBuf {
        self.run_dir.join(SOCKET_NAME)
    }

    pub fn alive(&mut self) -> bool {
        self.proc.as_mut().is_some_and(|p| p.alive())
    }

    pub fn pid(&self) -> Option<u32> {
        self.proc.as_ref().and_then(|p| p.pid())
    }

    /// The tail of what the sandbox printed (for an error message).
    pub fn log_tail(&self) -> String {
        let t = std::fs::read(&self.log).unwrap_or_default();
        let t = &t[t.len().saturating_sub(2000)..];
        last_lines(&String::from_utf8_lossy(t), 6)
    }

    /// Wait until the display's socket exists. Err when the sandbox ended first or the time ran out.
    pub fn wait_socket(&mut self, wait: Duration, sleep: &dyn Fn(Duration)) -> Result<(), String> {
        self.wait_socket_or(wait, sleep, &|| false)
    }

    /// [`Window::wait_socket`] that gives up (Err "cancelled") as soon as `cancel` says so.
    pub fn wait_socket_or(&mut self, wait: Duration, sleep: &dyn Fn(Duration), cancel: &dyn Fn() -> bool) -> Result<(), String> {
        let t0 = Instant::now();
        loop {
            if self.socket().exists() {
                return Ok(());
            }
            if cancel() {
                return Err("cancelled".into());
            }
            if !self.alive() {
                return Err(format!("the program ended before its window appeared: {}", self.log_tail()));
            }
            if t0.elapsed() >= wait {
                return Err(format!("the window did not appear in time: {}", self.log_tail()));
            }
            sleep(Duration::from_millis(100));
        }
    }

    /// Stop the program and everything in the sandbox; remove the socket.
    pub fn stop(&mut self) {
        if let Some(mut p) = self.proc.take() {
            p.kill();
            (self.audit)("window.stop", &format!("{}: {}", self.la_name, self.program));
        }
        let _ = std::fs::remove_file(self.socket());
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.stop();
    }
}

impl LinuxApps {
    /// A closure that writes to the same audit log (it opens the file again, so it needs nothing of the manager).
    pub fn clone_audit(&self) -> Arc<dyn Fn(&str, &str) + Send + Sync> {
        let path = self.audit_path();
        let user = self.user().to_string();
        Arc::new(move |action, detail| {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let line = format!("{}\t{}\t{}\t{}\n", crate::clock::now_unix().max(0), user, action, detail.replace(['\n', '\t', '\r'], " "));
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path) {
                let _ = f.write_all(line.as_bytes());
            }
        })
    }
}

// ------------------------------------------------------------------------------------------------ cpu and memory

/// What the sandbox's processes use now: the process count, resident memory (kB) and cpu time (clock ticks, 100 a second).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub procs: u32,
    pub rss_kb: u64,
    pub ticks: u64,
}

/// The processes below `root_pid` (children, grandchildren, ...) read from a `/proc` folder; the sandbox's processes are children of the
/// process IYAKKI started. Missing or odd files count as nothing.
pub fn usage_of(proc_root: &Path, root_pid: u32) -> Usage {
    let mut table: Vec<(u32, u32, u64, u64)> = Vec::new();
    for e in std::fs::read_dir(proc_root).into_iter().flatten().flatten().take(100_000) {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // the name is in parentheses and may hold spaces: fields count from the last `)`
        let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        let (Some(ppid), Some(ut), Some(st)) = (f.get(1).and_then(|s| s.parse::<u32>().ok()), f.get(11).and_then(|s| s.parse::<u64>().ok()), f.get(12).and_then(|s| s.parse::<u64>().ok())) else { continue };
        let rss = std::fs::read_to_string(e.path().join("status")).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("VmRSS:").and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok()))).unwrap_or(0);
        table.push((pid, ppid, ut + st, rss));
    }
    let mut family = vec![root_pid];
    let mut u = Usage::default();
    let mut grew = true;
    while grew {
        grew = false;
        for (pid, ppid, _, _) in &table {
            if family.contains(ppid) && !family.contains(pid) && family.len() < 10_000 {
                family.push(*pid);
                grew = true;
            }
        }
    }
    for (pid, _, ticks, rss) in &table {
        if family.contains(pid) {
            u.procs += 1;
            u.rss_kb += rss;
            u.ticks += ticks;
        }
    }
    u
}

/// Cpu use in percent of one core between two samples taken `secs` apart.
pub fn cpu_percent(before: Usage, after: Usage, secs: f32) -> f32 {
    if secs <= 0.0 || after.ticks < before.ticks {
        return 0.0;
    }
    (((after.ticks - before.ticks) as f32) / 100.0 / secs * 100.0).clamp(0.0, 100_000.0)
}
