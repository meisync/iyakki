//! Whose process is it? IYAKKI runs as one operating-system user, so `/proc` cannot say which IYAKKI person a process belongs to.
//! This module keeps the answer itself: a program started for a person (a command from the assistant, a script, a terminal) is
//! **claimed** for that person when it is spawned, and everything it starts below it belongs to the same person through the parent
//! links in `/proc`. SYSTEM's "My processes" page and the chat tools `system_my_processes` and `system_process_stop` read it.
//!
//! * [`claim`] / [`as_user`]: how a spawner says "this is for ann". `as_user` sets the person for the current thread; the one shared
//!   spawn path ([`crate::exec::run_prepared`]) claims every child it starts for that person and lets go when the child ends.
//! * [`owners`] (pure): processes and claims to owner per pid. [`mine`] adds the network use of each ([`crate::sysinfo::net_use`]).
//! * [`stop`]: the signal path. It re-reads the machine, accepts only a pid that is owned by the caller (or by anyone for an
//!   administrator), and never touches process 1 or the shell itself ([`crate::sysinfo::protected`]).
//!
//! Everything reads the folders given in [`SysRoots`], so tests use a made-up `/proc`.

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::sysinfo::{self, NetUse, Proc, SysRoots};
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// A program that was started for a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub pid: u32,
    pub user: String,
    /// What it was started as (the program name), for the page.
    pub what: String,
    /// When it was claimed (unix seconds).
    pub since: i64,
}

static CLAIMS: Mutex<Vec<(u64, Claim)>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The claim ends when this is dropped, whatever way the program ended.
pub struct ClaimGuard(u64);

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != self.0);
    }
}

/// Say that process `pid` runs for `user`.
pub fn claim(pid: u32, user: &str, what: &str) -> ClaimGuard {
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let c = Claim { pid, user: user.chars().take(64).collect(), what: what.chars().take(80).collect(), since: crate::clock::now_unix() };
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).push((id, c));
    ClaimGuard(id)
}

/// Every program claimed right now.
pub fn claims() -> Vec<Claim> {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, c)| c.clone()).collect()
}

/// Run `f` with `user` as the person any program started on this thread belongs to.
pub fn as_user<R>(user: &str, f: impl FnOnce() -> R) -> R {
    let before = CURRENT.with(|c| c.replace(Some(user.to_string())));
    let r = f();
    CURRENT.with(|c| *c.borrow_mut() = before);
    r
}

/// The person set by [`as_user`] on this thread, if any.
pub fn current_user() -> Option<String> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Owner of each process: the claimed process itself and everything below it. A process claimed twice keeps the first (oldest) claim.
pub fn owners(procs: &[Proc], claims: &[Claim]) -> HashMap<u32, (String, String)> {
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut sorted: Vec<&Claim> = claims.iter().collect();
    sorted.sort_by_key(|c| c.since);
    let mut roots: HashMap<u32, &Claim> = HashMap::new();
    for c in sorted {
        roots.entry(c.pid).or_insert(c);
    }
    let mut out = HashMap::new();
    for p in procs {
        // walk up the parent links (bounded: a made-up loop must not hang this)
        let mut at = p.pid;
        for _ in 0..64 {
            if let Some(c) = roots.get(&at) {
                out.insert(p.pid, (c.user.clone(), c.what.clone()));
                break;
            }
            match by_pid.get(&at).map(|x| x.ppid) {
                Some(pp) if pp > 1 && pp != at => at = pp,
                _ => break,
            }
        }
    }
    out
}

pub use iyakki_agent_api::OwnedProc as Owned;

/// The processes of `who` (everyone's claimed processes for an administrator), with their network use, in process-number order.
pub fn mine(r: &SysRoots, claims: &[Claim], who: &str, admin: bool) -> Vec<Owned> {
    let procs = sysinfo::processes(r);
    let own = owners(&procs, claims);
    let shown: Vec<(&Proc, &(String, String))> = procs.iter().filter_map(|p| own.get(&p.pid).map(|o| (p, o))).filter(|(_, o)| admin || o.0 == who).collect();
    let pids: Vec<u32> = shown.iter().map(|(p, _)| p.pid).collect();
    let net = sysinfo::net_use(r, &pids);
    let disk = sysinfo::disk_use(r, &pids);
    shown.into_iter().map(|(p, o)| Owned { proc: p.clone(), owner: o.0.clone(), what: o.1.clone(), net: net.get(&p.pid).copied().unwrap_or_default(), disk: disk.get(&p.pid).copied().unwrap_or_default() }).collect()
}

/// Ask a process to stop (`hard` kills it). Only a process [`mine`] would list for the caller; the machine is read again first, so a
/// number that was reused by something else is refused.
pub fn stop(r: &SysRoots, claims: &[Claim], who: &str, admin: bool, pid: u32, hard: bool) -> Result<String, String> {
    if let Some(why) = sysinfo::protected(pid) {
        return Err(format!("refused: {why}"));
    }
    let procs = sysinfo::processes(r);
    let own = owners(&procs, claims);
    let Some((owner, what)) = own.get(&pid) else { return Err(format!("process {pid} is not one of yours (or it has ended)")) };
    if !admin && owner != who {
        return Err(format!("process {pid} is not one of yours"));
    }
    let name = procs.iter().find(|p| p.pid == pid).map(|p| p.name.clone()).unwrap_or_default();
    sysinfo::signal(pid, hard)?;
    Ok(format!("{} {name} ({pid}, started as {what}).", if hard { "Killed" } else { "Asked to stop" }))
}

/// One line per process for the chat: number, owner, memory, processor time, network, command (secrets hidden).
pub fn text(rows: &[Owned], show_owner: bool) -> String {
    if rows.is_empty() {
        return "No process is running for you right now (programs started for you by the assistant or in a terminal are listed here).".into();
    }
    let mut out = String::new();
    for o in rows.iter().take(60) {
        let cmd = if o.proc.cmd.is_empty() { format!("[{}]", o.proc.name) } else { crate::sysprocs::redact(&o.proc.cmd.chars().take(160).collect::<String>()) };
        let net = [o.net.text(), o.disk.text()].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · ");
        out.push_str(&format!("{:>7} {}{:>9} {:>6}s cpu  {}{}\n", o.proc.pid, if show_owner { format!("{:<10} ", o.owner.chars().take(10).collect::<String>()) } else { String::new() }, sysinfo::human_bytes(o.proc.rss), o.proc.ticks / sysinfo::TICKS_PER_SEC, if net.is_empty() { String::new() } else { format!("[{net}]  ") }, cmd));
    }
    if rows.len() > 60 {
        out.push_str(&format!("... and {} more\n", rows.len() - 60));
    }
    out
}

pub const TOOLS: [&str; 2] = ["system_my_processes", "system_process_stop"];

pub fn is_tool(name: &str) -> bool {
    TOOLS.contains(&name)
}

fn allowed(p: &Policy) -> bool {
    p.apps.contains(&App::System)
}

/// `system_my_processes` only reads; `system_process_stop` asks first.
pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !allowed(p) {
        return Vec::new();
    }
    let t = |name: &str, description: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: description.into(), schema }, risk, untrusted: true };
    vec![
        t(
            "system_my_processes",
            "The programs running for the person: the ones the assistant or a terminal started for them and everything those started, each with its number, memory, processor time, and network use (open connections, listening ports, and the bytes of its container's own network when it has one). Secrets in command lines are hidden. Administrators also see every person's, with the owner. Read only.",
            json!({"type": "object", "properties": {}, "required": []}),
            Risk::Safe,
        ),
        t(
            "system_process_stop",
            "Ask one of the person's own programs to stop (`hard`: true kills it). Only a number listed by system_my_processes works (administrators: anyone's listed program). Process 1 and IYAKKI itself are refused. The person approves a card first.",
            json!({"type": "object", "properties": {"pid": {"type": "integer"}, "hard": {"type": "boolean"}}, "required": ["pid"]}),
            Risk::Ask,
        ),
    ]
}

pub fn describe(name: &str, args: &Value) -> String {
    match name {
        "system_process_stop" => format!("{} process {}", if args.get("hard").and_then(Value::as_bool).unwrap_or(false) { "KILL" } else { "stop" }, args.get("pid").and_then(Value::as_u64).map_or("?".to_string(), |p| p.to_string())),
        _ => "list the programs running for you".into(),
    }
}

/// Run a tool for `user`. `audit(event, detail)` writes the audit line.
pub fn call(name: &str, args: &Value, user: &str, p: &Policy, audit: &dyn Fn(&str, &str)) -> Result<String, String> {
    call_with(name, args, user, p, &SysRoots::real(), &claims(), audit)
}

/// [`call`] with the machine and the claims given (the tests give made-up ones).
pub fn call_with(name: &str, args: &Value, user: &str, p: &Policy, roots: &SysRoots, claims: &[Claim], audit: &dyn Fn(&str, &str)) -> Result<String, String> {
    if !allowed(p) {
        return Err("your account does not have the System tile".into());
    }
    let admin = p.manage_settings;
    match name {
        "system_my_processes" => Ok(text(&mine(roots, claims, user, admin), admin)),
        "system_process_stop" => {
            let pid = args.get("pid").and_then(Value::as_u64).and_then(|x| u32::try_from(x).ok()).ok_or("give the process number as `pid`")?;
            let hard = args.get("hard").and_then(Value::as_bool).unwrap_or(false);
            audit(if hard { "system.kill" } else { "system.stop" }, &format!("{pid} (chat)"));
            stop(roots, claims, user, admin, pid, hard)
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

#[cfg(test)]
#[path = "procowner_tests.rs"]
mod tests;
