//! Fetching the signed downloads manifest (docs/development/DOWNLOADS-MANIFEST.md, section 9).
//!
//! At most once per 24 hours on its own, and whenever the person clicks "Check for updates". Runs on a background thread and never blocks
//! the screen. The answer goes through [`Store::apply`]: signature, rollback and the rules against the bundled copy. Nothing is written
//! to the saved manifest unless all of that passes. No address, no network, a 404 (the repository may not exist yet) is a quiet "no update".

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Where the owner publishes the manifest; the signature is at the same address plus `.sig`. Keep the repository name stable.
/// A second address (a mirror) can be added in the second place; an empty entry is skipped.
pub const ADDRESSES: [&str; 2] = ["https://raw.githubusercontent.com/meisync/iyakki-downloads/main/downloads.json", ""];
/// A new check on its own happens at most this often (seconds).
pub const EVERY_S: u64 = 24 * 3600;
/// After "no connection" the next check on its own may come sooner (seconds).
pub const RETRY_OFFLINE_S: u64 = 3600;
/// The manifest may be at most this large when fetched.
pub const FETCH_MAX_BYTES: usize = 256 * 1024;
/// One request may take at most this long (seconds).
pub const TIMEOUT_S: u64 = 15;

/// What one check did.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Outcome {
    Updated { sequence: u64, changed: Vec<String> },
    Unchanged,
    Refused(String),
    /// No connection, no answer, or nothing published yet (HTTP 404); the text says which, for the audit line.
    Offline(String),
}

/// What the screen shows.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct CheckStatus {
    pub running: bool,
    /// Unix seconds of the last check.
    pub at: Option<u64>,
    /// The last result in plain words ("up to date", "no connection", "refused: signature did not match").
    pub result: String,
    /// After an applied update: "Download list updated: N items changed (names)".
    pub notice: Option<String>,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static AUDIT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn state_file(dir: &Path) -> PathBuf {
    dir.join("fetch-state.json")
}

pub fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The id as the person knows it ("cli:kubectl" -> "kubectl").
fn short(id: &str) -> &str {
    id.strip_prefix("cli:").unwrap_or(id)
}

pub fn notice_for(changed: &[String]) -> String {
    let names: Vec<&str> = changed.iter().map(|c| short(c)).take(6).collect();
    let more = if changed.len() > 6 { format!(", and {} more", changed.len() - 6) } else { String::new() };
    format!("Download list updated: {} item{} changed ({}{more})", changed.len(), if changed.len() == 1 { "" } else { "s" }, names.join(", "))
}

/// A refusal in plain words, one line.
fn refusal_words(e: &str) -> String {
    let e = e.trim();
    let low = e.to_ascii_lowercase();
    if low.contains("signature") {
        "signature did not match".into()
    } else if low.contains("rollback") || low.contains("older") || low.contains("already used") {
        "it is older than the list already held".into()
    } else if low.contains("too large") {
        "the file is too large".into()
    } else if low.contains("no manifest key") {
        "this program has no key built in".into()
    } else {
        e.lines().next().unwrap_or("").chars().take(120).collect()
    }
}

fn get(url: &str, max: usize) -> Result<String, Outcome> {
    // one byte more than allowed, so that "too large" can be told from "exactly the limit"
    match crate::netprobe::call("GET", url, Some("application/json"), TIMEOUT_S, max + 1, false) {
        Ok(a) if a.success() => {
            if a.body.len() > max {
                Err(Outcome::Refused("the manifest is too large".into()))
            } else {
                Ok(a.body)
            }
        }
        Ok(a) => Err(Outcome::Offline(format!("HTTP {}", a.status))),
        Err(e) if e.to_ascii_lowercase().contains("large") || e.to_ascii_lowercase().contains("limit") => Err(Outcome::Refused("the manifest is too large".into())),
        Err(e) => Err(Outcome::Offline(e)),
    }
}

/// One check against the addresses, in order; the first that answers decides. Writes the saved manifest only on `Updated` (inside
/// `Store::apply`), the small state file (time and result) always. `audit` gets `dlmanifest.fetch.*` lines (no secrets).
pub fn run_check(store: &Store, dir: &Path, addresses: &[&str], now: u64, audit: &dyn Fn(&str, &str)) -> Outcome {
    let mut out = Outcome::Offline("no address".into());
    for a in addresses.iter().filter(|a| !a.is_empty()) {
        let r = match get(a, FETCH_MAX_BYTES) {
            Err(o) => o,
            Ok(body) => match get(&format!("{a}.sig"), 4096) {
                Err(Outcome::Offline(w)) => Outcome::Offline(format!("signature file: {w}")),
                Err(o) => o,
                Ok(sig) => match store.apply(body.as_bytes(), &sig) {
                    Ok(Applied::Updated { sequence, changed }) => Outcome::Updated { sequence, changed },
                    Ok(Applied::Unchanged) => Outcome::Unchanged,
                    Err(e) => Outcome::Refused(e),
                },
            },
        };
        let stop = !matches!(r, Outcome::Offline(_));
        out = r;
        if stop {
            break;
        }
    }
    match &out {
        Outcome::Updated { sequence, changed } => audit("dlmanifest.fetch.ok", &format!("sequence {sequence}, {} changed: {}", changed.len(), changed.join(","))),
        Outcome::Unchanged => audit("dlmanifest.fetch.unchanged", "up to date"),
        Outcome::Refused(e) => audit("dlmanifest.fetch.refused", &refusal_words(e)),
        Outcome::Offline(w) => audit("dlmanifest.fetch.offline", w),
    }
    write_state(dir, now, &out);
    out
}

fn write_state(dir: &Path, now: u64, o: &Outcome) {
    let (result, notice) = match o {
        Outcome::Updated { changed, .. } => ("updated".to_string(), Some(notice_for(changed))),
        Outcome::Unchanged => ("up to date".into(), None),
        Outcome::Refused(e) => (format!("refused: {}", refusal_words(e)), None),
        Outcome::Offline(_) => ("no connection".into(), None),
    };
    // a notice stays until the next check that changes nothing newer; an "up to date" or "no connection" keeps the earlier one
    let keep = notice.or_else(|| if matches!(o, Outcome::Updated { .. }) { None } else { read_state(dir).and_then(|s| s.notice) });
    let j = serde_json::json!({ "at": now, "result": result, "notice": keep });
    if std::fs::create_dir_all(dir).is_ok() {
        let tmp = dir.join(format!("fetch-state.tmp{}", std::process::id()));
        if std::fs::write(&tmp, j.to_string()).is_ok() && std::fs::rename(&tmp, state_file(dir)).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn read_state(dir: &Path) -> Option<CheckStatus> {
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(state_file(dir)).ok()?).ok()?;
    Some(CheckStatus { running: false, at: j["at"].as_u64(), result: j["result"].as_str()?.chars().take(200).collect(), notice: j["notice"].as_str().map(|s| s.chars().take(300).collect()) })
}

pub fn status(dir: &Path) -> CheckStatus {
    let mut s = read_state(dir).unwrap_or_default();
    s.running = RUNNING.load(Ordering::SeqCst);
    s
}

/// Is a check on its own due (never checked, 24 h passed, or an hour after "no connection")?
pub fn due(dir: &Path, now: u64) -> bool {
    match read_state(dir) {
        Some(CheckStatus { at: Some(at), result, .. }) => {
            let wait = if result == "no connection" { RETRY_OFFLINE_S } else { EVERY_S };
            now < at || now - at >= wait
        }
        _ => true,
    }
}

/// Starts a check on a background thread: `force` for the person's click, otherwise only when due. Returns whether one was started.
pub fn start(dir: &Path, force: bool) -> bool {
    let now = now_s();
    if !force && !due(dir, now) {
        return false;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    let dir = dir.to_path_buf();
    let spawned = std::thread::Builder::new().name("dlmanifest-fetch".into()).spawn({
        let dir = dir.clone();
        move || {
            let store = Store::new(&dir);
            let addrs: Vec<&str> = ADDRESSES.to_vec();
            run_check(&store, &dir, &addrs, now_s(), &|a, d| {
                if let Ok(mut q) = AUDIT.lock() {
                    q.push((a.to_string(), d.to_string()));
                }
            });
            RUNNING.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

/// The audit lines the background thread has produced since the last call (the screen records them).
pub fn take_audit() -> Vec<(String, String)> {
    AUDIT.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(now: u64, at: u64) -> String {
    let d = now.saturating_sub(at);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

/// The one line of the "Check for updates" control: "Checked 3 h ago: up to date".
pub fn status_line(s: &CheckStatus, now: u64) -> String {
    if s.running {
        return "Checking for a newer download list...".into();
    }
    match s.at {
        Some(at) => format!("Checked {}: {}", ago(now, at), s.result),
        None => "Not checked yet".into(),
    }
}
