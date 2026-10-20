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

/// How the person signs in to Claude Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A Claude subscription (Pro, Max, Team, Enterprise): `--claudeai`.
    Subscription,
    /// The Anthropic Console, billed per use through the API: `--console`.
    Console,
    /// Single sign-on of an organization: `--sso`.
    Sso,
}

impl Method {
    pub const ALL: [Method; 3] = [Method::Subscription, Method::Console, Method::Sso];
    pub fn label(self) -> &'static str {
        match self {
            Method::Subscription => "Claude subscription",
            Method::Console => "Anthropic Console (API billing)",
            Method::Sso => "Single sign-on (SSO)",
        }
    }
    pub fn flag(self) -> &'static str {
        match self {
            Method::Subscription => "--claudeai",
            Method::Console => "--console",
            Method::Sso => "--sso",
        }
    }
}

/// What to start: the program, its arguments, the folder it runs in and the environment it gets (nothing else is inherited but the
/// search path, language and proxy settings).
#[derive(Clone, Debug)]
pub struct Spec {
    pub label: String,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Does the vendor flow ask the person to type something back (a code shown by the browser)?
    pub takes_code: bool,
}

/// The home folder the vendor programs keep their login in: `/home/<user>` (made persistent by the system), else the account's own
/// folder in the data folder.
pub fn login_home(user: &str, data_dir: &Path) -> PathBuf {
    crate::scope::home_root(user, data_dir)
}

/// The command for an agent id (`claude-code`, `copilot`). `program` is the installed program.
pub fn spec(agent: &str, program: &str, method: Method, home: &Path) -> Option<Spec> {
    let mut env = vec![("HOME".to_string(), home.display().to_string()), ("TERM".to_string(), "xterm-256color".to_string())];
    match agent {
        super::CLAUDE_CODE => {
            env.push(("CLAUDE_CONFIG_DIR".into(), home.join(".claude").display().to_string()));
            Some(Spec { label: "Claude Code".into(), argv: vec![program.into(), "auth".into(), "login".into(), method.flag().into()], env, cwd: home.to_path_buf(), takes_code: true })
        }
        super::COPILOT => {
            // the device-code flow works on a screen without a browser: a code to enter at github.com/login/device
            Some(Spec { label: "GitHub Copilot".into(), argv: vec![program.into(), "login".into(), "--device-code".into()], env, cwd: home.to_path_buf(), takes_code: false })
        }
        _ => None,
    }
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

// ------------------------------------------------------------------------------------------------ the token in the person's vault

/// The name of Copilot's entry in the person's vault. Its value is a small JSON object: the account and the token.
pub const COPILOT_VAULT: &str = "github-copilot";
/// The variable GitHub's Copilot CLI reads first (before `GH_TOKEN` and `GITHUB_TOKEN`).
pub const COPILOT_TOKEN_ENV: &str = "COPILOT_GITHUB_TOKEN";

pub fn vault_entry(login: &str, token: &str) -> String {
    serde_json::json!({ "login": login, "token": token }).to_string()
}

/// (account, token) from a vault entry.
pub fn parse_vault_entry(v: &str) -> Option<(String, super::Zeroizing<String>)> {
    let j: serde_json::Value = serde_json::from_str(v).ok()?;
    let token = j.get("token")?.as_str().filter(|t| !t.is_empty())?;
    Some((j.get("login").and_then(|l| l.as_str()).unwrap_or("").to_string(), super::Zeroizing::new(token.to_string())))
}

/// What Copilot wrote to its own settings file after a login on a machine with no keychain.
pub struct FileToken {
    pub login: String,
    pub token: super::Zeroizing<String>,
}

fn copilot_config(home: &Path) -> PathBuf {
    home.join(".copilot").join("config.json")
}

/// The file's `//` comment header and its JSON.
fn read_config(path: &Path) -> Option<(String, serde_json::Value)> {
    let text = std::fs::read_to_string(path).ok()?;
    let header: String = text.lines().filter(|l| l.trim_start().starts_with("//")).map(|l| format!("{l}\n")).collect();
    let body: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    Some((header, serde_json::from_str(&body).ok()?))
}

/// The token (and whose it is) in `<home>/.copilot/config.json`, read leniently: `authTokens` may be an object (values, or objects with
/// a `token`) or an array; the account comes from `lastLoggedInUser` / `loggedInUsers` (or the snake_case spellings), else from the
/// end of the token's key (`https://github.com:octocat`). `None` when there is no token.
pub fn copilot_file_token(home: &Path) -> Option<FileToken> {
    let (_, v) = read_config(&copilot_config(home))?;
    let tok = |x: &serde_json::Value| x.as_str().map(str::to_string).or_else(|| x.get("token").and_then(|t| t.as_str()).map(str::to_string)).filter(|t| !t.is_empty());
    let (mut token, mut key_login) = (None, String::new());
    match v.get("authTokens").or_else(|| v.get("auth_tokens")) {
        Some(serde_json::Value::Object(m)) => {
            for (k, x) in m {
                if let Some(t) = tok(x) {
                    token = Some(t);
                    key_login = k.rsplit(':').next().unwrap_or("").to_string();
                    break;
                }
            }
        }
        Some(serde_json::Value::Array(a)) => token = a.iter().find_map(tok),
        _ => {}
    }
    let login_of = |u: &serde_json::Value| u.get("login").and_then(|l| l.as_str()).filter(|l| !l.is_empty()).map(str::to_string);
    let login = v.get("lastLoggedInUser").or_else(|| v.get("last_logged_in_user")).and_then(login_of).or_else(|| v.get("loggedInUsers").or_else(|| v.get("logged_in_users")).and_then(|a| a.as_array()).and_then(|a| a.iter().find_map(login_of))).unwrap_or(key_login);
    Some(FileToken { login, token: super::Zeroizing::new(token?) })
}

/// Take the token out of the plain file and keep everything else in it (the comment header, the other settings, the account list);
/// the file stays readable by its owner only. `users_too` (a sign-out) also forgets who was signed in.
pub fn scrub_copilot_file(home: &Path, users_too: bool) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = copilot_config(home);
    let Some((header, mut v)) = read_config(&path) else { return Ok(()) };
    let mut changed = false;
    for k in ["authTokens", "auth_tokens"] {
        if let Some(x) = v.get_mut(k) {
            let empty = if x.is_array() { serde_json::json!([]) } else { serde_json::json!({}) };
            if *x != empty {
                *x = empty;
                changed = true;
            }
        }
    }
    if users_too {
        for k in ["lastLoggedInUser", "last_logged_in_user"] {
            if v.as_object_mut().and_then(|m| m.remove(k)).is_some() {
                changed = true;
            }
        }
        for k in ["loggedInUsers", "logged_in_users"] {
            if let Some(x) = v.get_mut(k) {
                if *x != serde_json::json!([]) {
                    *x = serde_json::json!([]);
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(());
    }
    let out = format!("{header}{}\n", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

// ------------------------------------------------------------------------------------------------ the pasted code

/// What a pasted code looks like after cleaning, and what is wrong with it, before anything is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeCheck {
    /// What would be sent to the program.
    pub code: String,
    pub len: usize,
    /// Why it must not be sent (spaces, characters no code has, nothing there).
    pub problem: Option<String>,
    /// A doubt that does not block sending (it looks short; Claude's code usually has a `#`).
    pub warning: Option<String>,
    /// The person pasted the whole redirect address and the code was taken out of it.
    pub from_url: bool,
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(h(b[i + 1]) << 4 | h(b[i + 2]));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Clean what was pasted: surrounding spaces, line breaks and quotes go; a whole redirect address (`…?code=X&state=Y` or `…?code=X#Y`)
/// becomes `X#Y`; line breaks inside (a phone wrapping the code) are joined. Then check it. `want_state`: the program (Claude's) expects
/// `code#state`.
pub fn check_code(raw: &str, want_state: bool) -> CodeCheck {
    let mut s = raw.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>' | '“' | '”')).trim().to_string();
    let mut from_url = false;
    if s.contains("://") {
        let (before_frag, frag) = match s.split_once('#') {
            Some((a, f)) => (a.to_string(), Some(f.to_string())),
            None => (s.clone(), None),
        };
        let query = before_frag.split_once('?').map(|x| x.1).unwrap_or("");
        let get = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(n, _)| *n == k).map(|(_, v)| url_decode(v)));
        match get("code") {
            Some(code) => {
                from_url = true;
                let state = get("state").or(frag).filter(|x| !x.is_empty());
                s = match state {
                    Some(st) => format!("{code}#{st}"),
                    None => code,
                };
            }
            None => return CodeCheck { code: String::new(), len: 0, problem: Some("That address has no code in it. Copy the code the page shows, not the address of the page.".into()), warning: None, from_url: false },
        }
    }
    // a phone or a terminal may have wrapped it
    s = s.chars().filter(|c| !matches!(c, '\n' | '\r' | '\t')).collect();
    let len = s.chars().count();
    let mut problem = None;
    if s.is_empty() {
        problem = Some("Nothing to send yet. Paste the code.".to_string());
    } else if s.contains(' ') {
        problem = Some("The code has spaces in it: check that you copied all of it and only it.".to_string());
    } else if let Some(c) = s.chars().find(|c| !(c.is_ascii_alphanumeric() || "_-#.~%=+/".contains(*c))) {
        problem = Some(format!("The code has a character no code has ({c}): check that you copied the right thing."));
    }
    let mut warning = None;
    if problem.is_none() {
        if len < 20 {
            warning = Some(format!("{len} characters looks too short: check that you copied all of it."));
        } else if want_state && !s.contains('#') {
            warning = Some("Claude's code usually has a # in the middle: check that you copied all of it.".to_string());
        }
    }
    CodeCheck { code: s, len, problem, warning, from_url }
}

/// Does this look like something a vendor page shows as a code (for the one-tap paste from the clipboard)? Strict on purpose: a long
/// run of code characters, no spaces, not an address.
pub fn looks_like_code(text: &str, want_state: bool) -> bool {
    let c = check_code(text, want_state);
    c.problem.is_none() && c.len >= 40 && c.len <= 2000 && (!want_state || c.code.contains('#'))
}

// ------------------------------------------------------------------------------------------------ the session

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Started; no address yet.
    Starting,
    /// The address (and perhaps a code) is shown; the person is signing in in a browser.
    Waiting,
    /// The person's answer was sent; waiting for the program.
    Finishing,
    Succeeded,
    /// With the reason in plain words.
    Failed(String),
    Cancelled,
}

impl Phase {
    pub fn finished(&self) -> bool {
        matches!(self, Phase::Succeeded | Phase::Failed(_) | Phase::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub phase: Phase,
    pub url: Option<String>,
    pub code: Option<String>,
    /// The program waits for something typed (a code from the browser or a yes/no).
    pub wants_input: bool,
    pub question: Option<String>,
    /// The program answered what was sent without finishing (it refused the code and asks again): its words, plain.
    pub note: Option<String>,
    /// One live line for the person.
    pub line: String,
}

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
        let mut name = [0 as libc::c_char; 128];
        if libc::ptsname_r(m, name.as_mut_ptr(), name.len()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
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
