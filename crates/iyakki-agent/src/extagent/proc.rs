//! The one place the external-agent code starts and stops a vendor CLI: no shell, an argument array, a cut-down
//! environment plus the variables given (an API key travels only here), its own process group (so Stop reaches
//! everything it started), capped output read as bytes (so a non-UTF-8 byte never stops a run) and a wait that always
//! reaps the child (no zombies).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One line longer than this is cut (a vendor CLI never needs more, and an endless line must not eat memory).
pub const LINE_CAP: usize = 4 * 1024 * 1024;
/// The captured error output kept for the run log.
pub const STDERR_CAP: usize = 64 * 1024;

pub struct Spawned {
    pub child: Child,
    pub lines: Receiver<Vec<u8>>,
    pub stderr: Arc<Mutex<Vec<u8>>>,
    /// Signalled when the reader of the error output has seen the end of the pipe (everything the child wrote is in `stderr`).
    pub stderr_done: Receiver<()>,
    pub pid: u32,
}

/// Variables of the person's environment a vendor CLI needs to find its own login and its runtime (never a token of ours).
const PASS: [&str; 14] = ["PATH", "HOME", "LANG", "LC_ALL", "TERM", "USER", "LOGNAME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "CLAUDE_CONFIG_DIR", "COPILOT_HOME"];

pub fn spawn(argv: &[String], cwd: &Path, env: &[(String, String)], stdin: Option<&[u8]>) -> Result<Spawned, std::io::Error> {
    let prog = argv.first().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no command"))?;
    let mut cmd = Command::new(prog);
    cmd.args(&argv[1..]).current_dir(cwd).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).env_clear();
    for k in PASS {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("NO_COLOR", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    // its own process group: the group id is the child's pid, so one kill reaches the whole tree
    cmd.process_group(0);
    let mut child = crate::wsexec::spawn_exec(&mut cmd)?;
    let pid = child.id();
    if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let (tx, rx) = channel::<Vec<u8>>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut r = BufReader::new(out);
            loop {
                let mut buf = Vec::new();
                match r.by_ref().take(LINE_CAP as u64).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if buf.last() == Some(&b'\n') {
                            buf.pop();
                        } else {
                            // cut at the cap: drop the rest of this line
                            let mut sink = Vec::new();
                            let _ = r.read_until(b'\n', &mut sink);
                        }
                        if buf.last() == Some(&b'\r') {
                            buf.pop();
                        }
                        if tx.send(buf).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let (dtx, stderr_done) = channel::<()>();
    if let Some(mut er) = child.stderr.take() {
        let sink = stderr.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = er.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut g = sink.lock().unwrap_or_else(|e| e.into_inner());
                if g.len() < STDERR_CAP {
                    let take = n.min(STDERR_CAP - g.len());
                    g.extend_from_slice(&chunk[..take]);
                }
            }
            let _ = dtx.send(());
        });
    }
    Ok(Spawned { child, lines: rx, stderr, stderr_done, pid })
}

/// Stop the whole process group: SIGTERM, then SIGKILL after `grace` if anything is still there; the child is reaped.
pub fn kill_group(child: &mut Child, grace: Duration) {
    let pgid = child.id() as i32;
    // SAFETY: signalling a process group we created; a stale id at worst finds no process
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    let end = Instant::now() + grace;
    while Instant::now() < end {
        if iyakki_core_api::child_status(child).is_over() {
            // the leader is gone; members that ignore TERM are killed below
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    let _ = child.wait();
}

/// Is any process of this group still alive? (Tests use it to prove Stop reached the children.)
pub fn group_alive(pgid: u32) -> bool {
    // SAFETY: signal 0 only checks existence
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

pub fn stderr_text(s: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&s.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
}
