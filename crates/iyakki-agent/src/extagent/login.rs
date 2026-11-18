//! Signing in to Claude Code and GitHub Copilot from the screen. The vendor's own login command (`claude auth login`,
//! `copilot login`) runs in a pseudo-terminal, so it behaves as it does in a terminal; this module only reads what it prints
//! (the sign-in address, a one-time device code, a question) and forwards what the person types back (the code a browser shows).
//! It never reads or stores a token: the vendor program keeps its credentials in its own files in the person's home folder.
//! Nothing here writes to a log or the audit; the output is kept in memory only, and a failure message has every long token-like
//! word and every address cut out of it.
//!
//! * [`Parsed`] / [`parse`]: the pure part: strip colour codes, join wrapped lines, find the address and the code.
//! * [`LoginSession`]: the process in its PTY, polled with [`LoginSession::snapshot`].
//! * [`spec`]: the command, the home folder and the environment for each vendor.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
pub use iyakki_extagentx::login::{Method, Spec, CodeCheck, Phase, Snapshot, spec, check_code, looks_like_code};




/// The home folder the vendor programs keep their login in: `/home/<user>` (made persistent by the system), else the account's own
/// folder in the data folder.
pub fn login_home(user: &str, data_dir: &Path) -> PathBuf {
    crate::scope::home_root(user, data_dir)
}


/// `claude auth logout` for Claude Code. Copilot's CLI has no logout command outside the program (`/logout`).
pub fn logout(agent: &str, program: &str, home: &Path) -> Result<String, String> {
    if agent != super::CLAUDE_CODE {
        return Err("GitHub Copilot signs out inside the program: start `copilot` and type /logout.".into());
    }
    let env = vec![("HOME".to_string(), home.display().to_string()), ("CLAUDE_CONFIG_DIR".to_string(), home.join(".claude").display().to_string())];
    let o = crate::wsexec::run_argv(home, &[program.into(), "auth".into(), "logout".into()], &env, Duration::from_secs(30), 16 * 1024);
    if o.ok() {
        Ok("Signed out of Claude Code.".into())
    } else if o.missing {
        Err("Claude Code is not installed.".into())
    } else {
        Err(format!("Claude Code did not sign out: {}", plain_failure(&o.combined())))
    }
}

// ------------------------------------------------------------------------------------------------ reading the output

/// Remove colour and cursor codes and other control characters; `\r\n` and a lone `\r` become line breaks.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                Some('[') => {
                    for n in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(n) = it.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {
                if it.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

/// Lines longer than this are taken to be wrapped by the program (a terminal 80 to 120 columns wide), so a following line made only of
/// address characters continues the address.
const WRAP_MIN: usize = 40;

/// The first `https://` address in `text`, with lines the program wrapped joined back.
pub fn find_url(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let Some(at) = l.find("https://").or_else(|| l.find("http://")) else { continue };
        let mut url: String = l[at..].chars().take_while(|c| is_url_char(*c)).collect();
        // the address reaches the end of the line: it may go on in the next
        let mut reached_end = l[at..].chars().all(is_url_char);
        let mut j = i + 1;
        while reached_end && url.len() >= WRAP_MIN && j < lines.len() {
            let next = lines[j].trim();
            // a continuation is made only of address characters and looks like part of one (a word such as "Done" is not)
            let looks = next.len() >= 20 || next.chars().any(|c| c.is_ascii_digit() || "/?&=%-_.:#+".contains(c));
            if next.is_empty() || !next.chars().all(is_url_char) || !looks {
                break;
            }
            url.push_str(next);
            reached_end = next.len() >= WRAP_MIN;
            j += 1;
        }
        let url = url.trim_end_matches(['.', ',', ')', ';']).to_string();
        if url.len() > "https://".len() + 3 {
            return Some(url);
        }
    }
    None
}

/// The last one-time device code (`ABCD-1234`) in `text`.
pub fn find_code(text: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let b: Vec<char> = line.chars().collect();
        let ok = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit();
        let mut i = 0;
        while i + 9 <= b.len() {
            let w = &b[i..i + 9];
            let shape = w[..4].iter().all(|c| ok(*c)) && w[4] == '-' && w[5..].iter().all(|c| ok(*c));
            let before = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == '-');
            let after = i + 9 == b.len() || !(b[i + 9].is_ascii_alphanumeric() || b[i + 9] == '-');
            // "ABCD-EFGH" with no digit is more likely a word than a code
            if shape && before && after && (w.iter().any(|c| c.is_ascii_digit()) || line.to_lowercase().contains("code")) {
                found = Some(w.iter().collect::<String>());
                i += 9;
            } else {
                i += 1;
            }
        }
    }
    found
}

/// Does the program wait for a code the browser showed?
pub fn asks_for_code(text: &str) -> bool {
    let l = text.to_lowercase();
    ["paste code", "paste the code", "paste your code", "authorization code"].iter().any(|w| l.contains(w))
}

/// A yes/no question on the last line (Copilot asks before keeping its token in a plain file when the system has no keychain).
pub fn find_question(text: &str) -> Option<String> {
    let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let low = last.to_lowercase();
    (low.contains("(y/n)") || low.contains("[y/n]") || low.contains("(yes/no)") || (last.ends_with('?') && low.contains("plaintext"))).then(|| redact(last))
}

/// What can be shown of program output: addresses and long token-like words replaced.
pub fn redact(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            if w.starts_with("http://") || w.starts_with("https://") {
                "(address)".to_string()
            } else if w.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '/' | '+')).count() >= 24 && w.chars().all(|c| !c.is_whitespace()) && !w.contains(':') || w.starts_with("sk-") || w.starts_with("ghp_") || w.starts_with("gho_") || w.starts_with("ghu_") || w.starts_with("github_pat_") {
                "(removed)".to_string()
            } else {
                w.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Plain words for a failure from what the program printed: its last lines, redacted and cut short.
pub fn plain_failure(out: &str) -> String {
    let clean = strip_ansi(out);
    let lines: Vec<String> = clean.lines().map(str::trim).filter(|l| !l.is_empty()).map(redact).collect();
    let tail = lines.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" ");
    let low = tail.to_lowercase();
    let hint = if low.contains("network") || low.contains("enotfound") || low.contains("could not resolve") || low.contains("econnrefused") || low.contains("timed out") || low.contains("timeout") {
        " The network seems to be down; check the connection and try again."
    } else if low.contains("expired") {
        " The code expired; start the sign-in again."
    } else if low.contains("denied") || low.contains("cancel") {
        " The sign-in was refused or cancelled in the browser."
    } else {
        ""
    };
    let t = super::tidy(&tail, 240);
    if t.is_empty() {
        format!("The program stopped without saying why.{hint}")
    } else {
        format!("{t}{hint}")
    }
}

/// Everything read from the output so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub url: Option<String>,
    pub code: Option<String>,
    pub wants_code: bool,
    pub question: Option<String>,
    pub success_text: bool,
}

pub fn parse(raw: &str) -> Parsed {
    let text = strip_ansi(raw);
    let low = text.to_lowercase();
    Parsed { url: find_url(&text), code: find_code(&text), wants_code: asks_for_code(&text), question: find_question(&text), success_text: low.contains("login successful") || low.contains("logged in as") || low.contains("successfully logged in") || low.contains("signed in as") }
}

pub use iyakki_extagentx::login::{copilot_file_token, parse_vault_entry, scrub_copilot_file, vault_entry, FileToken, COPILOT_TOKEN_ENV, COPILOT_VAULT};

// ------------------------------------------------------------------------------------------------ the pasted code





// ------------------------------------------------------------------------------------------------ the session




#[derive(Default)]
struct Shared {
    raw: Vec<u8>,
    parsed: Parsed,
    sent: bool,
    sent_at: Option<Instant>,
    /// Output length when the program had had time to answer; growth after it is the answer.
    sent_base: Option<usize>,
    /// What the program said after the last thing sent (a refusal), in plain words.
    note: Option<String>,
    exit: Option<Option<i32>>,
    cancelled: bool,
    start_error: Option<String>,
}

/// The longest output kept (a login prints a few lines).
const KEEP: usize = 64 * 1024;
/// A sign-in nobody finished is stopped after this long.
pub const TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct LoginSession {
    shared: Arc<Mutex<Shared>>,
    master: Arc<File>,
    pid: i32,
    takes_code: bool,
    label: String,
    done: Arc<AtomicBool>,
}

fn pty_pair() -> std::io::Result<(File, File)> {
    // SAFETY: the usual POSIX sequence; every descriptor is owned by a File at once, and errors are reported
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if m < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let master = OwnedFd::from_raw_fd(m);
        if libc::grantpt(m) != 0 || libc::unlockpt(m) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let name = crate::oscompat::pts_name(m)?;
        let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if s < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let slave = OwnedFd::from_raw_fd(s);
        // wide enough that the program does not wrap the address; no echo, so a pasted code is not printed back
        let ws = libc::winsize { ws_row: 50, ws_col: 1000, ws_xpixel: 0, ws_ypixel: 0 };
        libc::ioctl(m, libc::TIOCSWINSZ, &ws);
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(s, &mut t) == 0 {
            t.c_lflag &= !(libc::ECHO | libc::ECHONL);
            libc::tcsetattr(s, libc::TCSANOW, &t);
        }
        Ok((File::from(master), File::from(slave)))
    }
}

impl LoginSession {
    /// Start the vendor's login. A program that cannot be started gives a session that has already failed, with the reason.
    pub fn start(spec: &Spec) -> LoginSession {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let done = Arc::new(AtomicBool::new(false));
        let fail = |why: String| {
            let m = File::open("/dev/null").expect("/dev/null");
            shared.lock().unwrap_or_else(|e| e.into_inner()).start_error = Some(why);
            done.store(true, Ordering::SeqCst);
            LoginSession { shared: shared.clone(), master: Arc::new(m), pid: 0, takes_code: spec.takes_code, label: spec.label.clone(), done: done.clone() }
        };
        let Some(prog) = spec.argv.first() else { return fail("no command".into()) };
        let (master, slave) = match pty_pair() {
            Ok(p) => p,
            Err(e) => return fail(format!("could not open a terminal: {e}")),
        };
        let mut cmd = Command::new(prog);
        cmd.args(&spec.argv[1..]).current_dir(&spec.cwd).env_clear();
        for k in ["PATH", "LANG", "LC_ALL", "TMPDIR", "HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR", "USER", "LOGNAME"] {
            if let Ok(v) = std::env::var(k) {
                cmd.env(k, v);
            }
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let (a, b, c) = match (slave.try_clone(), slave.try_clone()) {
            (Ok(a), Ok(b)) => (a, b, slave),
            _ => return fail("could not open a terminal".into()),
        };
        cmd.stdin(Stdio::from(a)).stdout(Stdio::from(b)).stderr(Stdio::from(c));
        // SAFETY: only async-signal-safe calls between fork and exec: a new session with the terminal as its controlling terminal
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let spawned = crate::wsexec::spawn_exec(&mut cmd);
        drop(cmd); // the parent's copies of the terminal's far end close here, so the reader sees the end
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return fail(format!("`{prog}` is not installed on this machine")),
            Err(e) => return fail(format!("could not start `{prog}`: {e}")),
        };
        let pid = child.id() as i32;
        let master = Arc::new(master);
        // the reader
        let (sh, rd, dn) = (shared.clone(), master.clone(), done.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let mut p = libc::pollfd { fd: rd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: one valid pollfd
                let n = unsafe { libc::poll(&mut p, 1, 100) };
                if n > 0 {
                    match (&*rd).read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => {
                            let mut g = sh.lock().unwrap_or_else(|e| e.into_inner());
                            g.raw.extend_from_slice(&buf[..k]);
                            if g.raw.len() > KEEP {
                                let cut = g.raw.len() - KEEP;
                                g.raw.drain(..cut);
                            }
                            let text = String::from_utf8_lossy(&g.raw).into_owned();
                            let mut p = parse(&text);
                            // the first address stays; a repaint of the screen must not change it
                            if g.parsed.url.is_some() {
                                p.url = g.parsed.url.clone();
                            }
                            g.parsed = p;
                        }
                    }
                } else if sh.lock().unwrap_or_else(|e| e.into_inner()).exit.is_some() {
                    // The program is gone. Its last words (the reason for a refusal) may have been written after the poll above timed
                    // out and before the exit was seen: look once more, now that the exit is known, and only stop when nothing waits.
                    let mut again = libc::pollfd { fd: rd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                    // SAFETY: one valid pollfd
                    if unsafe { libc::poll(&mut again, 1, 0) } > 0 {
                        continue;
                    }
                    break; // the program is gone and nothing more is waiting
                }
            }
            dn.store(true, Ordering::SeqCst);
        });
        // the waiter
        let sh = shared.clone();
        std::thread::spawn(move || {
            let st = child.wait().ok();
            sh.lock().unwrap_or_else(|e| e.into_inner()).exit = Some(st.and_then(|s| s.code()));
        });
        // the watchdog: a sign-in nobody finishes is stopped
        let (sh, dn) = (shared.clone(), done.clone());
        std::thread::spawn(move || {
            let t0 = Instant::now();
            while !dn.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
                if t0.elapsed() > TIMEOUT {
                    sh.lock().unwrap_or_else(|e| e.into_inner()).cancelled = true;
                    kill_group(pid);
                    break;
                }
            }
        });
        LoginSession { shared, master, pid, takes_code: spec.takes_code, label: spec.label.clone(), done }
    }

    /// Stop the program (the person pressed Cancel).
    pub fn cancel(&self) {
        {
            let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if g.exit.is_some() && self.done.load(Ordering::SeqCst) {
                return;
            }
            g.cancelled = true;
        }
        if self.pid > 0 {
            kill_group(self.pid);
        }
    }

    /// Type a line to the program (the code the browser showed, or `y`/`n`). The text goes only to the program.
    pub fn send_line(&self, text: &str) {
        {
            let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            g.sent = true;
            g.sent_at = Some(Instant::now());
            g.sent_base = None;
            g.note = None;
        }
        if self.pid == 0 {
            return;
        }
        let m = self.master.clone();
        let text = text.trim().as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = (&*m).write_all(&text);
            // a separate write for the Enter key, as a typist would: some programs treat one burst as a paste
            std::thread::sleep(Duration::from_millis(80));
            let _ = (&*m).write_all(b"\r");
        });
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        // after what was sent, new output that is not the end is the program's answer: a refusal, and it asks again
        if g.sent && !self.done.load(Ordering::SeqCst) && g.exit.is_none() {
            if let Some(at) = g.sent_at.filter(|t| t.elapsed() > Duration::from_millis(800)) {
                let _ = at;
                match g.sent_base {
                    None => g.sent_base = Some(g.raw.len()),
                    Some(n) if g.raw.len() > n && !g.parsed.success_text => {
                        let tail = strip_ansi(&String::from_utf8_lossy(&g.raw[n..]));
                        let kept: Vec<&str> = tail.lines().filter(|l| !asks_for_code(l)).collect();
                        let said = plain_failure(&kept.join("\n"));
                        g.note = Some(if said.contains("without saying why") { "The program did not accept that code. Check it and try again.".to_string() } else { said });
                        g.sent = false;
                        g.sent_base = None;
                    }
                    _ => {}
                }
            }
        }
        let p = &g.parsed;
        let finished = self.done.load(Ordering::SeqCst) && g.exit.is_some() || g.start_error.is_some();
        let phase = if let Some(e) = &g.start_error {
            Phase::Failed(e.clone())
        } else if finished {
            if g.cancelled {
                Phase::Cancelled
            } else if g.exit == Some(Some(0)) {
                Phase::Succeeded
            } else {
                Phase::Failed(plain_failure(&String::from_utf8_lossy(&g.raw)))
            }
        } else if g.sent {
            Phase::Finishing
        } else if p.url.is_some() || p.code.is_some() {
            Phase::Waiting
        } else {
            Phase::Starting
        };
        let wants_input = !phase.finished() && (p.wants_code || p.question.is_some() || (self.takes_code && p.url.is_some()));
        let line = match &phase {
            Phase::Starting => format!("Starting {}'s sign-in…", self.label),
            Phase::Waiting if g.note.is_some() => g.note.clone().unwrap_or_default(),
            Phase::Waiting if p.code.is_some() => "Open the address on any device, enter the code, and approve. This screen continues by itself.".to_string(),
            Phase::Waiting if wants_input => "Open the address on any device, sign in, and type the code it shows below.".to_string(),
            Phase::Waiting => "Open the address on any device and sign in.".to_string(),
            Phase::Finishing => "Checking the sign-in…".to_string(),
            Phase::Succeeded => "Signed in.".to_string(),
            Phase::Failed(e) => e.clone(),
            Phase::Cancelled => "Sign-in cancelled.".to_string(),
        };
        Snapshot { phase, url: p.url.clone(), code: p.code.clone(), wants_input, question: p.question.clone(), note: g.note.clone(), line }
    }
}

impl Drop for LoginSession {
    fn drop(&mut self) {
        // leaving the screen stops a sign-in that is still running
        if !self.done.load(Ordering::SeqCst) {
            self.cancel();
        }
    }
}

fn kill_group(pid: i32) {
    if pid > 1 {
        // SAFETY: the program was started in a session of its own, so its process group id is its pid
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            // SAFETY: as above; a group that is gone gives an error that is ignored
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        });
    }
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
