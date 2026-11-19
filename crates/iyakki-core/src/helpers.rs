//! Helper services that init starts and keeps running as child processes of the core: the fan service (F-DRIVERS-14) and the
//! privileged VPN helper (F-VPN-05, F-VPN-16). One small supervisor serves both.
//!
//! * A helper is the core's own program with a subcommand (`iyakki fan-service`, `iyakki vpn-helper`), started with a fixed
//!   argument list and no shell. Nothing is started unless the person's setting for it is on.
//! * It is restarted when it exits, with a growing pause; a helper that keeps dying straight after it starts is given up on
//!   (and the audit log says so) instead of being restarted forever.
//! * Stopping sends SIGTERM, waits a grace period, then kills it.
//! * Everything it prints on standard output is read by the supervisor: a line `AUDIT<TAB>event<TAB>detail` becomes an audit
//!   line written by the core (one writer of the audit log), any other line goes to the log. Every start, exit, restart and
//!   give-up is audited by the supervisor itself.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What to run.
#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// How hard to try.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub first_pause: Duration,
    pub max_pause: Duration,
    /// A helper that ran at least this long is healthy: the pause and the failure count start over.
    pub healthy_after: Duration,
    /// This many starts in a row that died before `healthy_after`: give up.
    pub max_rapid: u32,
    /// Time between SIGTERM and the kill.
    pub grace: Duration,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { first_pause: Duration::from_secs(1), max_pause: Duration::from_secs(30), healthy_after: Duration::from_secs(20), max_rapid: 5, grace: Duration::from_secs(5) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Running { pid: u32, starts: u32 },
    Waiting { starts: u32 },
    GaveUp { starts: u32 },
    Stopped { starts: u32 },
}

/// A running supervisor of one helper.
pub struct Handle {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub type Audit = Arc<dyn Fn(&str, &str) + Send + Sync>;

impl Handle {
    pub fn state(&self) -> State {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Stops the helper (SIGTERM, then the kill after the grace period) and ends the supervisor.
    pub fn stop(mut self) -> State {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.state()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts supervising `spec` on a thread of its own. `audit(event, detail)` receives the supervisor's lines and the helper's `AUDIT` lines.
pub fn supervise(spec: Spec, policy: Policy, audit: Audit) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(State::Waiting { starts: 0 }));
    let (s2, st2) = (stop.clone(), state.clone());
    let thread = std::thread::Builder::new().name(format!("iyakki-svc-{}", spec.name)).spawn(move || run(&spec, policy, &audit, &s2, &st2)).ok();
    Handle { stop, state, thread }
}

fn set(state: &Mutex<State>, s: State) {
    *state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = s;
}

fn pause(total: Duration, stop: &AtomicBool) {
    let end = Instant::now() + total;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits for the child; a child somebody else already reaped counts as gone.
fn try_exit(child: &mut Child) -> Option<String> {
    match child.try_wait() {
        Ok(Some(s)) => Some(s.to_string()),
        Ok(None) => None,
        Err(e) => Some(format!("gone ({e})")),
    }
}

fn terminate(child: &mut Child, grace: Duration) -> String {
    // SAFETY: kill(2) with a pid of our own child and a plain signal number.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let end = Instant::now() + grace;
    loop {
        if let Some(s) = try_exit(child) {
            return s;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            return "killed after the grace period".into();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn forward_output(child: &mut Child, name: &str, audit: &Audit) -> Option<std::thread::JoinHandle<()>> {
    let out = child.stdout.take()?;
    let (name, audit) = (name.to_string(), audit.clone());
    std::thread::Builder::new()
        .name(format!("iyakki-svc-{name}-out"))
        .spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                match line.strip_prefix("AUDIT\t").and_then(|r| r.split_once('\t')) {
                    Some((event, detail)) => audit(event, &detail.chars().take(500).collect::<String>()),
                    None => log::info!("{name}: {line}"),
                }
            }
        })
        .ok()
}

fn run(spec: &Spec, policy: Policy, audit: &Audit, stop: &AtomicBool, state: &Mutex<State>) {
    let (mut starts, mut rapid, mut wait) = (0u32, 0u32, policy.first_pause);
    while !stop.load(Ordering::SeqCst) {
        let began = Instant::now();
        let mut child = match Command::new(&spec.program).args(&spec.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
            Ok(c) => c,
            Err(e) => {
                audit("service.start-failed", &format!("{}: {e}", spec.name));
                rapid += 1;
                if rapid >= policy.max_rapid {
                    audit("service.gave-up", &format!("{}: could not be started {rapid} times in a row", spec.name));
                    set(state, State::GaveUp { starts });
                    return;
                }
                set(state, State::Waiting { starts });
                pause(wait, stop);
                wait = (wait * 2).min(policy.max_pause);
                continue;
            }
        };
        starts += 1;
        set(state, State::Running { pid: child.id(), starts });
        audit("service.start", &format!("{} pid {} (start {starts})", spec.name, child.id()));
        let reader = forward_output(&mut child, &spec.name, audit);
        let status = loop {
            if stop.load(Ordering::SeqCst) {
                let s = terminate(&mut child, policy.grace);
                if let Some(r) = reader {
                    let _ = r.join();
                }
                audit("service.stop", &format!("{}: {s}", spec.name));
                set(state, State::Stopped { starts });
                return;
            }
            if let Some(s) = try_exit(&mut child) {
                break s;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if let Some(r) = reader {
            let _ = r.join();
        }
        audit("service.exit", &format!("{}: {status}", spec.name));
        if began.elapsed() >= policy.healthy_after {
            rapid = 0;
            wait = policy.first_pause;
        } else {
            rapid += 1;
            if rapid >= policy.max_rapid {
                audit("service.gave-up", &format!("{}: stopped {rapid} times in a row soon after it started; not started again", spec.name));
                set(state, State::GaveUp { starts });
                return;
            }
        }
        set(state, State::Waiting { starts });
        pause(wait, stop);
        wait = (wait * 2).min(policy.max_pause);
    }
    set(state, State::Stopped { starts });
}

/// The fan service as the core starts it: the same program, the real `/sys` and the data folder.
pub fn fan_spec(exe: PathBuf, sys: &str, data_dir: &str) -> Spec {
    Spec { name: "fan".into(), program: exe, args: vec!["fan-service".into(), "--sys".into(), sys.into(), "--data".into(), data_dir.into()] }
}

/// The VPN helper as the core starts it: allowing only `uid` (the user the VPN session runs as) at `socket`.
pub fn vpn_helper_spec(exe: PathBuf, uid: u32, socket: &str) -> Spec {
    Spec { name: "vpn-helper".into(), program: exe, args: vec!["vpn-helper".into(), "--allow-uid".into(), uid.to_string(), "--socket".into(), socket.into()] }
}

/// What init should start now, given the settings below `data_dir`: the fan service only when the person turned fan control on, the
/// VPN helper only when its setting says so. Pure: the caller starts them.
pub fn wanted(exe: &std::path::Path, sys: &str, data_dir: &std::path::Path, own_uid: u32) -> Vec<Spec> {
    let mut v = Vec::new();
    if iyakki_drivers::fanservice::load(data_dir).enabled {
        v.push(fan_spec(exe.to_path_buf(), sys, &data_dir.to_string_lossy()));
    }
    #[cfg(feature = "headless")]
    {
        // one helper per socket, whichever account asked for it (the setting lives in each account's VPN folder)
        let mut dirs: Vec<_> = std::fs::read_dir(data_dir.join("vpn")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        let mut sockets: Vec<String> = Vec::new();
        for d in dirs {
            let s = iyakki_agent::vpn_helper::read_setting(&d);
            if s.enabled && !sockets.contains(&s.socket) {
                v.push(vpn_helper_spec(exe.to_path_buf(), s.allow_uid.unwrap_or(own_uid), &s.socket));
                sockets.push(s.socket);
            }
        }
    }
    let _ = own_uid;
    v
}

/// Keeps the helpers in line with the settings: a helper whose setting was turned on is started, one whose setting was turned
/// off (or changed) is stopped. [`Reconciler::tick`] is cheap; init calls it every few seconds on a thread.
pub struct Reconciler {
    running: Vec<(Spec, Handle)>,
    policy: Policy,
    audit: Audit,
}

impl Reconciler {
    pub fn new(policy: Policy, audit: Audit) -> Reconciler {
        Reconciler { running: Vec::new(), policy, audit }
    }

    pub fn tick(&mut self, wanted: Vec<Spec>) {
        let (keep, drop): (Vec<_>, Vec<_>) = std::mem::take(&mut self.running).into_iter().partition(|(s, _)| wanted.iter().any(|w| w.name == s.name && w.args == s.args));
        for (_, h) in drop {
            h.stop();
        }
        self.running = keep;
        for w in wanted {
            if !self.running.iter().any(|(s, _)| s.name == w.name && s.args == w.args) {
                let h = supervise(w.clone(), self.policy, self.audit.clone());
                self.running.push((w, h));
            }
        }
    }

    pub fn states(&self) -> Vec<(String, State)> {
        self.running.iter().map(|(s, h)| (s.name.clone(), h.state())).collect()
    }

    pub fn stop_all(&mut self) {
        for (_, h) in std::mem::take(&mut self.running) {
            h.stop();
        }
    }
}

/// Init's part: a thread that reconciles the helpers with the settings every `every` until `stop` is set.
pub fn keep_running(exe: PathBuf, sys: String, data_dir: PathBuf, own_uid: u32, audit: Audit, every: Duration, stop: Arc<AtomicBool>) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("iyakki-helpers".into()).spawn(move || {
        let mut r = Reconciler::new(Policy::default(), audit);
        while !stop.load(Ordering::SeqCst) {
            r.tick(wanted(&exe, &sys, &data_dir, own_uid));
            pause(every, &stop);
        }
        r.stop_all();
    })
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
