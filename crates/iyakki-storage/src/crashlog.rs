//! Crash forensics (docs/TROUBLESHOOTING.md, "The computer crashed or froze: where is the log"), in the normal boot and the lean boot alike.
//!
//! The kernel log lives in RAM and is gone after a crash or a power cut. This module follows `/dev/kmsg` from the moment the data volume is
//! mounted and appends every line to `/var/lib/iyakki/log/kmsg-<boot-id>.log`, with the wall-clock time when the clock is plausible. It also
//! remembers whether the last session ended cleanly (`session.json`, a clean marker written by the power-off and reboot paths): when it did
//! not, that boot's kernel log is kept as a crash file, `unclean.json` says so, the audit log gets a line and Settings -> Overview shows
//! "The last session ended unexpectedly".
//!
//! What it cannot do: a kernel that dies cannot write its own last words to a file. Lines are written at most 2 s late (at once for errors,
//! BUG, Oops, panic, Call Trace, RIP:, hung task, soft lockup, out of memory), so the log ends within 2 s of where the machine stopped, and shows
//! the oops when the kernel got to print one while the disk still answered. A hard hang with no printed message leaves a log that simply stops.
//!
//! Never slows the system: the reader thread only fills a bounded queue, a separate writer thread does the (possibly slow) disk I/O, lines are
//! dropped and counted when the disk stalls, and nothing is written while the drive-loss watcher says the data volume is not healthy.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Folder under the data directory.
pub const LOG_SUBDIR: &str = "log";
pub const SESSION_FILE: &str = "session.json";
pub const UNCLEAN_FILE: &str = "unclean.json";
pub const SESSIONS_FILE: &str = "sessions.tsv";
/// One boot's file may not grow past this.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// The whole folder is kept under this (oldest non-crash files go first).
pub const MAX_DIR_BYTES: u64 = 64 * 1024 * 1024;
/// How many earlier boots' files are kept besides crash files.
pub const KEEP_PREVIOUS: usize = 3;
/// Lines waiting for the writer.
pub const QUEUE_CAP: usize = 4096;
/// The writer flushes at least this often.
pub const FLUSH_EVERY: Duration = Duration::from_secs(2);

// ------------------------------------------------------------------------------------------------ one kernel log record

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub level: u8,
    pub seq: u64,
    pub ts_us: u64,
    pub text: String,
}

/// `pri,seq,usec,flags;text` (the first line of a `/dev/kmsg` read; continuation lines start with a space and are dropped).
pub fn parse_record(rec: &str) -> Option<Record> {
    let first = rec.lines().next()?;
    let (head, text) = first.split_once(';')?;
    let mut f = head.split(',');
    let pri: u32 = f.next()?.parse().ok()?;
    let seq: u64 = f.next()?.parse().ok()?;
    let ts_us: u64 = f.next()?.parse().ok()?;
    Some(Record { level: (pri & 7) as u8, seq, ts_us, text: text.to_string() })
}

/// Lines that must reach the disk at once: level 3 (error) or worse, and the words a crash leaves.
pub fn is_urgent(level: u8, text: &str) -> bool {
    level <= 3 || ["BUG", "Oops", "panic", "Call Trace", "segfault", "Out of memory", "hung task", "soft lockup", "RIP:"].iter().any(|m| text.contains(m))
}

/// Words that mark a log tail as the end of a crash (a kernel that printed one of these did not end well).
pub fn has_crash_marker(tail: &str) -> bool {
    ["Kernel panic", "BUG:", "Oops", "Call Trace", "RIP:", "hung task", "soft lockup", "Out of memory", "general protection fault"].iter().any(|m| tail.contains(m))
}

/// Civil date of a day count since 1970-01-01 (the same algorithm as `iyakki_agent::clock::civil`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A clock before this (2025-01-01) has not been set: no wall-clock time is written.
pub const EARLIEST_SANE: i64 = 1_735_689_600;

/// `2026-10-23 16:40:01` (UTC) for a Unix time.
pub fn wall(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let rem = unix.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The text of one log line: wall-clock time (only when `now_unix` is plausible), the kernel's own time, the level, the message.
/// `uptime_us` is the kernel clock now: a record from the replayed buffer is placed on the wall clock by its distance from now.
pub fn format_line(r: &Record, now_unix: Option<i64>, uptime_us: u64) -> String {
    let wall_text = now_unix.map(|n| wall(n - (uptime_us.saturating_sub(r.ts_us) / 1_000_000) as i64) + " ").unwrap_or_default();
    format!("{wall_text}[{:>5}.{:06}] <{}> {}", r.ts_us / 1_000_000, r.ts_us % 1_000_000, r.level, r.text)
}

// ------------------------------------------------------------------------------------------------ the bounded queue

#[derive(Default)]
pub struct Queue {
    lines: VecDeque<String>,
    pub dropped: u64,
    cap: usize,
    pub urgent: bool,
}

impl Queue {
    pub fn new(cap: usize) -> Queue {
        Queue { lines: VecDeque::new(), dropped: 0, cap, urgent: false }
    }

    /// Add a line; when full the OLDEST waiting line is dropped (and counted): the newest lines are the ones about the crash.
    pub fn push(&mut self, line: String, urgent: bool) {
        if self.lines.len() >= self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
        self.urgent |= urgent;
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Everything waiting as file text, with a note when lines were dropped since the last take.
    pub fn take(&mut self) -> String {
        let mut out = String::new();
        if self.dropped > 0 {
            out.push_str(&format!("-- {} kernel log lines were dropped here (the disk was slow or not answering) --\n", self.dropped));
            self.dropped = 0;
        }
        for l in self.lines.drain(..) {
            out.push_str(&l);
            out.push('\n');
        }
        self.urgent = false;
        out
    }
}

// ------------------------------------------------------------------------------------------------ the writer (pure parts)

/// What one flush does: nothing (nothing waiting, or the drive is not healthy: lines stay queued, bounded), or append `text`.
pub enum Flush {
    Nothing,
    Append(String),
}

/// Decide a flush. `healthy` is the drive-loss watcher's verdict; `due` is "the 2 s are over or an urgent line is waiting".
pub fn plan_flush(q: &mut Queue, healthy: bool, due: bool) -> Flush {
    if !healthy || q.is_empty() && q.dropped == 0 || !(due || q.urgent) {
        return Flush::Nothing;
    }
    Flush::Append(q.take())
}

/// Append `text` to `path` and flush it to the disk. A file already at `MAX_FILE_BYTES` gets one closing line and nothing more.
pub fn append_synced(path: &Path, text: &str) -> std::io::Result<()> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len >= MAX_FILE_BYTES {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path)?;
    let room = (MAX_FILE_BYTES - len) as usize;
    if text.len() > room {
        let mut cut = room;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        f.write_all(text[..cut].as_bytes())?;
        f.write_all(b"\n-- this boot's kernel log reached 8 MB; the rest is not kept --\n")?;
    } else {
        f.write_all(text.as_bytes())?;
    }
    f.sync_data()
}

// ------------------------------------------------------------------------------------------------ sessions and rotation

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub boot_id: String,
    pub started: i64,
    pub cmdline: String,
    pub clean: bool,
}

fn esc(s: &str) -> String {
    s.chars().flat_map(|c| match c {
        '"' => vec!['\\', '"'],
        '\\' => vec!['\\', '\\'],
        '\n' | '\r' | '\t' => vec![' '],
        c => vec![c],
    }).collect()
}

fn field(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = text[at..].trim_start();
    if let Some(r) = rest.strip_prefix('"') {
        let (mut out, mut chars) = (String::new(), r.chars());
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?),
                '"' => return Some(out),
                c => out.push(c),
            }
        }
        None
    } else {
        Some(rest.split([',', '}']).next()?.trim().to_string())
    }
}

impl Session {
    pub fn to_json(&self) -> String {
        format!("{{\"boot_id\":\"{}\",\"started\":{},\"cmdline\":\"{}\",\"clean\":{}}}\n", esc(&self.boot_id), self.started, esc(&self.cmdline), self.clean)
    }

    pub fn parse(text: &str) -> Option<Session> {
        Some(Session { boot_id: field(text, "boot_id")?, started: field(text, "started")?.parse().ok()?, cmdline: field(text, "cmdline").unwrap_or_default(), clean: field(text, "clean")? == "true" })
    }
}

/// What the next boot learns about the one before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unclean {
    pub boot_id: String,
    pub started: i64,
    /// The previous boot's last kernel log line and how long it had been up (from that line's kernel time).
    pub last_line: String,
    pub uptime_s: u64,
    /// File name (inside the log folder) the kernel log was kept under.
    pub file: String,
    pub audited: bool,
    pub seen: bool,
    pub marker: bool,
}

impl Unclean {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"boot_id\":\"{}\",\"started\":{},\"last_line\":\"{}\",\"uptime_s\":{},\"file\":\"{}\",\"audited\":{},\"seen\":{},\"marker\":{}}}\n",
            esc(&self.boot_id), self.started, esc(&self.last_line), self.uptime_s, esc(&self.file), self.audited, self.seen, self.marker
        )
    }

    pub fn parse(text: &str) -> Option<Unclean> {
        Some(Unclean {
            boot_id: field(text, "boot_id")?,
            started: field(text, "started")?.parse().ok()?,
            last_line: field(text, "last_line").unwrap_or_default(),
            uptime_s: field(text, "uptime_s")?.parse().ok()?,
            file: field(text, "file")?,
            audited: field(text, "audited")? == "true",
            seen: field(text, "seen")? == "true",
            marker: field(text, "marker")? == "true",
        })
    }
}

fn is_log_file(name: &str) -> bool {
    name.starts_with("kmsg-") && name.ends_with(".log")
}

fn is_crash_file(name: &str) -> bool {
    name.starts_with("kmsg-crash-")
}

/// The last `max` bytes of a file as text.
pub fn tail_text(path: &Path, max: u64) -> String {
    use std::io::{Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

/// The last `n` lines of a file.
pub fn last_lines(path: &Path, n: usize) -> Vec<String> {
    let t = tail_text(path, 256 * 1024);
    let all: Vec<&str> = t.lines().collect();
    all[all.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect()
}

/// Kernel seconds of a log line (`... [  123.456789] ...`).
pub fn line_uptime(line: &str) -> Option<u64> {
    let a = line.find('[')? + 1;
    let b = line[a..].find(']')? + a;
    line[a..b].trim().split('.').next()?.parse().ok()
}

fn dir_files(dir: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        is_log_file(&n).then(|| (n, e.metadata().map(|m| m.len()).unwrap_or(0)))
    }).collect();
    v.sort();
    v
}

/// Keep the folder under `MAX_DIR_BYTES`: the oldest NON-crash files go first; crash files only when nothing else is left to drop.
/// `age` orders files (smaller = older). Returns the names removed.
pub fn enforce_dir_bound(dir: &Path, max: u64, age: &dyn Fn(&str) -> u64) -> Vec<String> {
    let mut files = dir_files(dir);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    let mut removed = Vec::new();
    files.sort_by_key(|(n, _)| (is_crash_file(n), age(n)));
    for (n, size) in files {
        if total <= max {
            break;
        }
        if std::fs::remove_file(dir.join(&n)).is_ok() {
            total -= size;
            removed.push(n);
        }
    }
    removed
}

fn mtime(dir: &Path, name: &str) -> u64 {
    std::fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Start-of-boot bookkeeping in `dir` (the `log` folder): read the previous session, keep or shift its kernel log, note an unclean end, write this
/// session. Returns what was found about the previous boot (None when it ended cleanly or there was none).
pub fn begin_session(dir: &Path, now: &Session) -> Option<Unclean> {
    let _ = std::fs::create_dir_all(dir);
    let prev = std::fs::read_to_string(dir.join(SESSION_FILE)).ok().and_then(|t| Session::parse(&t));
    let mut found = None;
    if let Some(p) = prev.filter(|p| p.boot_id != now.boot_id) {
        let src = format!("kmsg-{}.log", p.boot_id);
        let tail = tail_text(&dir.join(&src), 64 * 1024);
        let marker = has_crash_marker(&tail);
        let crash = !p.clean || marker;
        let last = last_lines(&dir.join(&src), 1).into_iter().next().unwrap_or_default();
        let uptime = line_uptime(&last).unwrap_or(0);
        let kept = if dir.join(&src).exists() {
            if crash {
                let to = format!("kmsg-crash-{}.log", p.boot_id);
                let _ = std::fs::rename(dir.join(&src), dir.join(&to));
                to
            } else {
                shift_previous(dir, &src);
                "kmsg-previous-1.log".to_string()
            }
        } else {
            String::new()
        };
        // history for `iyakki log sessions`
        append_session_history(dir, &p, if p.clean && !marker { "clean" } else if p.clean { "clean-with-errors" } else { "unclean" }, uptime);
        if !p.clean {
            let u = Unclean { boot_id: p.boot_id.clone(), started: p.started, last_line: last, uptime_s: uptime, file: kept, audited: false, seen: false, marker };
            let _ = std::fs::write(dir.join(UNCLEAN_FILE), u.to_json());
            found = Some(u);
        }
    }
    let _ = std::fs::write(dir.join(SESSION_FILE), now.to_json());
    enforce_dir_bound(dir, MAX_DIR_BYTES, &|n| mtime(dir, n));
    found
}

/// previous-3 is dropped, 2 -> 3, 1 -> 2, the finished boot's file -> 1.
fn shift_previous(dir: &Path, src: &str) {
    let name = |i: usize| format!("kmsg-previous-{i}.log");
    let _ = std::fs::remove_file(dir.join(name(KEEP_PREVIOUS)));
    for i in (1..KEEP_PREVIOUS).rev() {
        if dir.join(name(i)).exists() {
            let _ = std::fs::rename(dir.join(name(i)), dir.join(name(i + 1)));
        }
    }
    let _ = std::fs::rename(dir.join(src), dir.join(name(1)));
}

fn append_session_history(dir: &Path, p: &Session, how: &str, uptime: u64) {
    let path = dir.join(SESSIONS_FILE);
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().map(str::to_string).collect();
    lines.push(format!("{}\t{}\t{}\t{}\t{}", p.boot_id, p.started, how, uptime, p.cmdline.split_whitespace().filter(|w| w.starts_with("iyakki.") || *w == "quiet").collect::<Vec<_>>().join(" ")));
    let skip = lines.len().saturating_sub(200);
    let _ = std::fs::write(path, lines[skip..].join("\n") + "\n");
}

/// The marker the power-off and reboot paths write: this session ended cleanly.
pub fn mark_clean(dir: &Path) {
    if let Some(mut s) = std::fs::read_to_string(dir.join(SESSION_FILE)).ok().and_then(|t| Session::parse(&t)) {
        s.clean = true;
        let _ = std::fs::write(dir.join(SESSION_FILE), s.to_json());
    }
}

/// The pending "ended unexpectedly" note, if any.
pub fn read_unclean(dir: &Path) -> Option<Unclean> {
    std::fs::read_to_string(dir.join(UNCLEAN_FILE)).ok().and_then(|t| Unclean::parse(&t))
}

fn write_unclean(dir: &Path, u: &Unclean) {
    let _ = std::fs::write(dir.join(UNCLEAN_FILE), u.to_json());
}

/// "I have seen it": the flag is cleared (the crash file stays until the owner deletes it).
pub fn mark_seen(dir: &Path) {
    if let Some(mut u) = read_unclean(dir) {
        u.seen = true;
        write_unclean(dir, &u);
    }
}

/// Once per unclean end: the audit line's text, then the note is marked as audited. `record(event, detail)`.
pub fn audit_pending(dir: &Path, record: &dyn Fn(&str, &str)) {
    if let Some(mut u) = read_unclean(dir).filter(|u| !u.audited) {
        record(
            "system.unclean-shutdown",
            &format!("the session that started {} ended without a clean shutdown; up for about {} s; last kernel line: {}; log kept as {}", wall(u.started), u.uptime_s, u.last_line, if u.file.is_empty() { "(none)" } else { &u.file }),
        );
        u.audited = true;
        write_unclean(dir, &u);
    }
}

/// Files for the CLI and the viewer: (name, size) of kernel logs, newest first by name rules: current, previous-N, crash files.
pub fn list_logs(dir: &Path, boot_id: &str) -> Vec<(String, u64)> {
    let mut v = dir_files(dir);
    let rank = |n: &str| -> (u8, String) {
        if n == format!("kmsg-{boot_id}.log") { (0, String::new()) } else if let Some(i) = n.strip_prefix("kmsg-previous-") { (1, i.to_string()) } else { (2, n.to_string()) }
    };
    v.sort_by_key(|(n, _)| rank(n));
    v
}

/// This boot's file: a `kmsg-<id>.log` that is neither a previous-N nor a crash file (the newest one).
pub fn current_file(dir: &Path) -> Option<String> {
    let mut v: Vec<String> = dir_files(dir).into_iter().map(|f| f.0).filter(|n| !n.starts_with("kmsg-previous-") && !is_crash_file(n)).collect();
    v.sort_by_key(|n| mtime(dir, n));
    v.pop()
}

/// The newest crash file name, if any.
pub fn latest_crash(dir: &Path) -> Option<String> {
    let mut v: Vec<String> = dir_files(dir).into_iter().map(|f| f.0).filter(|n| is_crash_file(n)).collect();
    v.sort_by_key(|n| mtime(dir, n));
    v.pop()
}

/// `iyakki log sessions` text.
pub fn sessions_text(dir: &Path, current: Option<&Session>) -> String {
    let mut out = String::from("boot id\tstarted (UTC)\tended\tup (s)\tentry\n");
    for l in std::fs::read_to_string(dir.join(SESSIONS_FILE)).unwrap_or_default().lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() >= 4 {
            out.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", f[0], wall(f[1].parse().unwrap_or(0)), f[2], f[3], f.get(4).unwrap_or(&"")));
        }
    }
    if let Some(c) = current {
        out.push_str(&format!("{}\t{}\trunning\t-\t{}\n", c.boot_id, wall(c.started), c.cmdline.split_whitespace().filter(|w| w.starts_with("iyakki.") || *w == "quiet").collect::<Vec<_>>().join(" ")));
    }
    out
}

// ------------------------------------------------------------------------------------------------ the reader and writer threads

/// Where kernel log records come from (blocking). The real one reads `/dev/kmsg`; tests feed records.
pub trait Source: Send {
    /// The next record, or None when the source is finished.
    fn next(&mut self) -> Option<String>;
}

#[cfg(target_os = "linux")]
pub struct KmsgSource(std::fs::File);

#[cfg(target_os = "linux")]
impl KmsgSource {
    pub fn open() -> Option<KmsgSource> {
        std::fs::OpenOptions::new().read(true).open("/dev/kmsg").ok().map(KmsgSource)
    }
}

#[cfg(target_os = "linux")]
impl Source for KmsgSource {
    fn next(&mut self) -> Option<String> {
        let mut buf = vec![0u8; 8192];
        loop {
            match self.0.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => return Some(String::from_utf8_lossy(&buf[..n]).into_owned()),
                // records were overwritten before we read them: go on with the oldest left
                Err(e) if e.raw_os_error() == Some(libc::EPIPE) || e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
    }
}

pub struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    /// The file this boot writes to.
    pub path: PathBuf,
}

impl Shared {
    pub fn new(path: PathBuf) -> Arc<Shared> {
        Arc::new(Shared { queue: Mutex::new(Queue::new(QUEUE_CAP)), wake: Condvar::new(), path })
    }

    /// The reader's job for one record. Never touches the disk.
    pub fn feed(&self, rec: &str, now_unix: Option<i64>, uptime_us: u64) {
        let Some(r) = parse_record(rec) else { return };
        let urgent = is_urgent(r.level, &r.text);
        let line = format_line(&r, now_unix, uptime_us);
        if let Ok(mut q) = self.queue.lock() {
            q.push(line, urgent);
            if urgent {
                self.wake.notify_one();
            }
        }
    }

    /// One writer round: wait up to `FLUSH_EVERY` (or until an urgent line), then write what is waiting if the drive is healthy.
    /// Returns false when nothing was written. `healthy` and `write` are parameters so a test can fake the drive.
    pub fn round(&self, wait: Duration, healthy: &dyn Fn() -> bool, write: &dyn Fn(&Path, &str) -> std::io::Result<()>) -> bool {
        let text = {
            let Ok(mut q) = self.queue.lock() else { return false };
            if !q.urgent {
                q = match self.wake.wait_timeout(q, wait) {
                    Ok((g, _)) => g,
                    Err(_) => return false,
                };
            }
            match plan_flush(&mut q, healthy(), true) {
                Flush::Nothing => return false,
                Flush::Append(t) => t,
            }
        };
        // the disk is touched WITHOUT the queue lock: a stalled write never stops the reader
        if write(&self.path, &text).is_err() {
            // the lines are lost; the count says so in the next flush
            if let Ok(mut q) = self.queue.lock() {
                q.dropped += text.lines().count() as u64;
            }
            return false;
        }
        true
    }
}

static ACTIVE: Mutex<Option<Arc<Shared>>> = Mutex::new(None);
static CURRENT: Mutex<Option<Session>> = Mutex::new(None);

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn uptime_us() -> u64 {
    std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok())).map_or(0, |s| (s * 1e6) as u64)
}

fn boot_id() -> String {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default().trim().replace('-', "");
    if id.len() >= 8 { id[..8].to_string() } else { format!("{:08x}", now_unix() as u32) }
}

/// Start the crash log (once per boot, from `boot.rs`, as soon as the data volume is a writable mount). Safe to call again.
/// Returns what was found about an unclean end of the previous session (once per boot).
pub fn start(data_dir: &Path) -> Option<Unclean> {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if active.is_some() {
        return None;
    }
    let dir = data_dir.join(LOG_SUBDIR);
    let session = Session { boot_id: boot_id(), started: now_unix(), cmdline: std::fs::read_to_string("/proc/cmdline").unwrap_or_default().trim().to_string(), clean: false };
    let found = begin_session(&dir, &session);
    let shared = Shared::new(dir.join(format!("kmsg-{}.log", session.boot_id)));
    *active = Some(shared.clone());
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
    #[cfg(target_os = "linux")]
    {
        let reader = shared.clone();
        let _ = std::thread::Builder::new().name("iyakki-kmsg-read".into()).spawn(move || {
            let Some(mut src) = KmsgSource::open() else { return };
            while let Some(rec) = src.next() {
                let n = now_unix();
                reader.feed(&rec, (n >= EARLIEST_SANE).then_some(n), uptime_us());
            }
        });
    }
    let _ = std::thread::Builder::new().name("iyakki-kmsg-write".into()).spawn(move || loop {
        shared.round(FLUSH_EVERY, &crate::loss::is_healthy, &|p, t| append_synced(p, t));
    });
    found
}

/// Write what is waiting right now and mark the session clean (called by the power-off and reboot paths; best effort, never blocks long).
pub fn finish_clean(data_dir: &Path) {
    let shared = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(s) = shared {
        s.round(Duration::from_millis(1), &crate::loss::is_healthy, &|p, t| append_synced(p, t));
    }
    if CURRENT.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        mark_clean(&data_dir.join(LOG_SUBDIR));
    }
}

/// The current session (None before `start`).
pub fn current_session() -> Option<Session> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ------------------------------------------------------------------------------------------------ `iyakki log ...`

/// `iyakki log kernel [--previous] [--crash]` and `iyakki log sessions`; returns the text and whether it succeeded.
pub fn cli(args: &[String], data_dir: &Path, boot: &str) -> (String, bool) {
    let dir = data_dir.join(LOG_SUBDIR);
    match args.first().map(String::as_str) {
        Some("sessions") => (sessions_text(&dir, std::fs::read_to_string(dir.join(SESSION_FILE)).ok().and_then(|t| Session::parse(&t)).as_ref()), true),
        Some("kernel") => {
            let flag = |f: &str| args.iter().any(|a| a == f);
            let name = if flag("--crash") {
                match latest_crash(&dir) {
                    Some(n) => n,
                    None => return ("No crash log is kept (no session ended unexpectedly).\n".into(), true),
                }
            } else if flag("--previous") {
                "kmsg-previous-1.log".to_string()
            } else {
                format!("kmsg-{boot}.log")
            };
            match std::fs::read_to_string(dir.join(&name)) {
                Ok(t) => (t, true),
                Err(e) => (format!("{}: {e}\nFiles kept: {}\n", dir.join(&name).display(), list_logs(&dir, boot).iter().map(|f| f.0.clone()).collect::<Vec<_>>().join(", ")), false),
            }
        }
        _ => ("usage: iyakki log kernel [--previous] [--crash]   |   iyakki log sessions\n".into(), false),
    }
}

#[cfg(test)]
#[path = "crashlog_tests.rs"]
mod tests;
