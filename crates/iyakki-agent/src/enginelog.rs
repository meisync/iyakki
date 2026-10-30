//! The browser engine's own messages, kept for later.
//!
//! The helper program that runs the web engine writes to its standard error (GStreamer and WebKit warnings, the sound doctor's
//! notes, the reason it stopped). The client keeps only the last twenty lines in memory, which are gone after a restart, so a
//! camera or sound fault could only be found with a special debug switch and a rebuild. This module keeps the last ~200 KB of those
//! lines in two small rotating files in the shell's log folder (`<data>/log/engine.log` and `engine.log.1`; the crash log's own
//! cleaning only touches `kmsg-*.log`, so the two do not meet).
//!
//! What is kept: the helper's lines, each with the time. What is NOT kept: page addresses (every address becomes `[address]`),
//! page console messages, anything that looks like a credential ([`crate::credguard`]), control characters. A line is cut at 400
//! characters. Writing never blocks the screen for long (one small append) and never fails the caller.
//!
//! Read it with `iyakki dev logs --system engine` (developer mode, scope `debug`), in DOCTOR (it looks for a crashed helper and a
//! failing camera or microphone path) and in the diagnostics bundle (`diagshare.rs`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const FILE: &str = "engine.log";
pub const PREVIOUS: &str = "engine.log.1";
/// The current file is moved aside when it passes this size, so the two files together hold about 200 KB.
pub const HALF_BYTES: u64 = 100 * 1024;
/// A line is cut here (characters).
pub const LINE_MAX: usize = 400;
/// The folder when nobody set one (the data disk's log folder, next to the crash log).
pub const DEFAULT_DIR: &str = "/var/lib/iyakki/log";

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the files go (the shell sets it once from its data folder; tests set their own).
pub fn set_dir(dir: &Path) {
    if let Ok(mut d) = DIR.lock() {
        *d = Some(dir.to_path_buf());
    }
}

pub fn dir() -> PathBuf {
    DIR.lock().ok().and_then(|d| d.clone()).unwrap_or_else(|| PathBuf::from(DEFAULT_DIR))
}

/// Replace page addresses: `https://example.org/a?b=c` becomes `[address]`; `data:` and `blob:` words too.
fn without_addresses(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("://") {
        // the scheme is the letters just before "://"
        let head = &rest[..i];
        let scheme_len = head.chars().rev().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')).count();
        let keep = i - scheme_len;
        out.push_str(&rest[..keep]);
        out.push_str("[address]");
        let after = &rest[i + 3..];
        let end = after.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']' | '}')).unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out.split(' ').map(|w| if w.starts_with("data:") || w.starts_with("blob:") { "[address]" } else { w }).collect::<Vec<_>>().join(" ")
}

/// A line as it is kept (empty: not worth keeping).
pub fn clean(line: &str) -> String {
    let t: String = line.chars().filter(|c| !c.is_control() || *c == '\t').collect();
    let t = t.trim();
    if t.is_empty() {
        return String::new();
    }
    // a page's own console output is page content
    if t.contains("CONSOLE") || t.to_ascii_lowercase().contains("console message") {
        return "[page console message not kept]".into();
    }
    let mut t = without_addresses(t);
    // credentials, as often as the detector finds one (it replaces them all in one go, but a second look is cheap)
    for _ in 0..3 {
        match crate::credguard::detect(&t) {
            Some(f) => t = f.redacted,
            None => break,
        }
    }
    t.chars().take(LINE_MAX).collect()
}

/// Add a line to the files in `dir`, with the time. Errors are ignored.
pub fn keep_in(dir: &Path, now_unix: i64, line: &str) {
    let text = clean(line);
    if text.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let cur = dir.join(FILE);
    if std::fs::metadata(&cur).map(|m| m.len()).unwrap_or(0) >= HALF_BYTES {
        let _ = std::fs::rename(&cur, dir.join(PREVIOUS));
    }
    // private to the account that runs the shell
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&cur) {
        let _ = writeln!(f, "{now_unix} {text}");
    }
}

/// Add a line to the files in the shell's log folder.
pub fn keep(line: &str) {
    keep_in(&dir(), crate::clock::now_unix(), line);
}

/// Everything kept, oldest first: `(unix time, text)`.
pub fn read_in(dir: &Path) -> Vec<(i64, String)> {
    let mut out = Vec::new();
    for name in [PREVIOUS, FILE] {
        let t = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        for l in t.lines() {
            if let Some((a, b)) = l.split_once(' ') {
                if let Ok(ts) = a.parse::<i64>() {
                    out.push((ts, b.to_string()));
                }
            }
        }
    }
    out
}

/// The kept lines as plain text (`<time> <text>`), the last `n` of them.
pub fn tail_text(dir: &Path, n: usize) -> String {
    let all = read_in(dir);
    let start = all.len().saturating_sub(n);
    all[start..].iter().map(|(t, s)| format!("{t} {s}")).collect::<Vec<_>>().join("\n")
}

/// What the kept lines say about the engine lately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// The helper stopped or crashed in the window.
    pub crashed: u32,
    /// Lines about the camera path failing.
    pub camera_trouble: u32,
    /// Lines about the microphone path failing.
    pub mic_trouble: u32,
    /// The newest line that proved any of it (kept text, already cleaned).
    pub last: String,
}

fn bad_word(l: &str) -> bool {
    ["error", "fail", "busy", "cannot", "could not", "denied", "no such device", "not found"].iter().any(|k| l.contains(k))
}

/// Count the signs of trouble in lines newer than `since` (unix seconds).
pub fn health(lines: &[(i64, String)], since: i64) -> Health {
    let mut h = Health::default();
    for (ts, text) in lines.iter().filter(|(t, _)| *t >= since) {
        let _ = ts;
        let l = text.to_ascii_lowercase();
        let crash = ["crashed", "web process terminated", "segmentation fault", "sigsegv", "sigabrt", "terminated unexpectedly", "the helper closed the connection", "the connection to the helper failed", "did not start in time"].iter().any(|k| l.contains(k));
        let cam = ["v4l2", "camera", "webcam"].iter().any(|k| l.contains(k)) && bad_word(&l);
        let mic = ["alsasrc", "microphone", "audio capture", "capture device"].iter().any(|k| l.contains(k)) && bad_word(&l);
        if crash {
            h.crashed += 1;
        }
        if cam {
            h.camera_trouble += 1;
        }
        if mic {
            h.mic_trouble += 1;
        }
        if crash || cam || mic {
            h.last = text.clone();
        }
    }
    h
}

#[cfg(test)]
#[path = "enginelog_tests.rs"]
mod tests;
