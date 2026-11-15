//! Running the tests of the open file from the editor (F-EDIT-16): `cargo test` for a Rust file in a Cargo project and `pytest` for
//! a Python test file, and a readable account of output that is not text.
//!
//! * [`plan`] decides what to run and where from the file's path alone (nothing is started).
//! * [`run`] starts the planned program in the project folder with a time limit and a stop flag, keeps at most [`MAX_OUTPUT`] bytes
//!   and reports [`TestRun`]: the counts, the failed test names and the last lines.
//! * [`describe_bytes`] turns bytes that are not text (an image, an archive, any binary) into one honest line instead of garbage.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::testresults::{parse_output, Outcome};

/// The most output kept from a run.
pub const MAX_OUTPUT: usize = 256 * 1024;
/// The longest a test run may take.
pub const MAX_SECONDS: u64 = 600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestPlan {
    /// `cargo test` or `pytest`.
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

fn up_to(start: &Path, marker: &[&str]) -> Option<PathBuf> {
    let mut d = start.parent()?.to_path_buf();
    loop {
        if marker.iter().any(|m| d.join(m).is_file()) {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

/// What to run for this file, or why nothing can be run.
pub fn plan(file: &Path) -> Result<TestPlan, String> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match ext.as_str() {
        "rs" => {
            let cwd = up_to(file, &["Cargo.toml"]).ok_or("This Rust file is not inside a Cargo project (no Cargo.toml above it).")?;
            let mut args = vec!["test".to_string(), "--color".into(), "never".into()];
            // a file under tests/ is one test binary: run just that one
            if let Ok(rel) = file.strip_prefix(&cwd) {
                let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if parts.len() == 2 && parts[0] == "tests" {
                    args.push("--test".into());
                    args.push(parts[1].trim_end_matches(".rs").to_string());
                }
            }
            Ok(TestPlan { label: "cargo test".into(), program: "cargo".into(), args, cwd })
        }
        "py" => {
            let is_test = name.starts_with("test_") || name.ends_with("_test.py") || name == "conftest.py";
            if !is_test {
                return Err("Run tests works on a Python file named test_*.py or *_test.py.".into());
            }
            let cwd = up_to(file, &["pyproject.toml", "pytest.ini", "setup.cfg", "tox.ini"]).unwrap_or_else(|| file.parent().map(Path::to_path_buf).unwrap_or_default());
            Ok(TestPlan { label: "pytest".into(), program: "python3".into(), args: vec!["-m".into(), "pytest".into(), "-v".into(), "--color=no".into(), "-p".into(), "no:cacheprovider".into(), file.display().to_string()], cwd })
        }
        _ => Err("Tests can be run from a .rs file in a Cargo project or a test_*.py file.".into()),
    }
}

/// The account of one test run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRun {
    pub ok: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
    /// The last lines of the output (text).
    pub tail: String,
    pub stopped: bool,
    pub timed_out: bool,
    pub exit: Option<i32>,
}

impl TestRun {
    /// The text shown under the editor.
    pub fn text(&self, label: &str) -> String {
        let mut s = if self.stopped {
            format!("{label}: stopped.")
        } else if self.timed_out {
            format!("{label}: ran out of time and was ended.")
        } else if self.passed + self.failed + self.skipped == 0 {
            format!("{label}: no test results found (exit {}).", self.exit.map_or("?".to_string(), |c| c.to_string()))
        } else {
            format!("{label}: {} passed, {} failed, {} skipped.", self.passed, self.failed, self.skipped)
        };
        for f in self.failures.iter().take(10) {
            s.push_str(&format!("\n  FAILED {f}"));
        }
        if self.failures.len() > 10 {
            s.push_str(&format!("\n  ... and {} more", self.failures.len() - 10));
        }
        if !self.ok && !self.tail.is_empty() {
            s.push_str(&format!("\n{}", self.tail));
        }
        s
    }
}

/// Count the results in a run's output.
pub fn summarize(output: &[u8], exit: Option<i32>) -> TestRun {
    let text = text_of(output);
    let res = parse_output(&text);
    let count = |o: Outcome| res.values().filter(|x| **x == o).count();
    let failures: Vec<String> = res.iter().filter(|(_, o)| **o == Outcome::Fail).map(|(n, _)| n.clone()).collect();
    let tail = text.lines().rev().filter(|l| !l.trim().is_empty()).take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    TestRun { ok: exit == Some(0) && failures.is_empty(), passed: count(Outcome::Pass), failed: failures.len(), skipped: count(Outcome::Skip), failures, tail, stopped: false, timed_out: false, exit }
}

/// Text for output bytes: readable text as it is (invalid UTF-8 replaced), anything else described by [`describe_bytes`].
pub fn text_of(bytes: &[u8]) -> String {
    if is_text(bytes) { String::from_utf8_lossy(bytes).into_owned() } else { describe_bytes(bytes) }
}

fn is_text(b: &[u8]) -> bool {
    let head = &b[..b.len().min(4096)];
    !head.contains(&0) && head.iter().filter(|c| **c < 9 || (**c > 13 && **c < 32)).count() * 100 <= head.len().max(1)
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(*b.get(i)?) << 8 | u32::from(*b.get(i + 1)?))
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(be16(b, i)? << 16 | be16(b, i + 2)?)
}

/// One honest line about bytes that are not text: what they look like, their size and a short hex start.
pub fn describe_bytes(b: &[u8]) -> String {
    let size = b.len();
    let kind: String = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        match (be32(b, 16), be32(b, 20)) {
            (Some(w), Some(h)) => format!("PNG picture, {w} x {h}"),
            _ => "PNG picture".into(),
        }
    } else if b.starts_with(b"GIF8") {
        let w = u32::from(*b.get(6).unwrap_or(&0)) | u32::from(*b.get(7).unwrap_or(&0)) << 8;
        let h = u32::from(*b.get(8).unwrap_or(&0)) | u32::from(*b.get(9).unwrap_or(&0)) << 8;
        format!("GIF picture, {w} x {h}")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        "JPEG picture".into()
    } else if b.starts_with(b"%PDF") {
        "PDF document".into()
    } else if b.starts_with(b"PK\x03\x04") {
        "ZIP archive".into()
    } else if b.starts_with(&[0x1f, 0x8b]) {
        "gzip data".into()
    } else if b.starts_with(b"\0asm") {
        "WebAssembly module".into()
    } else if b.starts_with(b"\x7fELF") {
        "program (ELF)".into()
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WAVE") {
        "WAV sound".into()
    } else {
        "binary data".into()
    };
    let hex = b.iter().take(16).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ");
    format!("[not text: {kind}, {size} bytes; starts {hex}]")
}

/// Run the plan. `cancel` stops it for real (the process is killed).
pub fn run(p: &TestPlan, seconds: u64, cancel: &AtomicBool) -> Result<TestRun, String> {
    let mut cmd = Command::new(&p.program);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .args(&p.args)
        .current_dir(&p.cwd)
        .env("NO_COLOR", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { format!("{} is not installed on this computer", p.program) } else { format!("{}: {e}", p.program) })?;
    // a test run started for a person is hers in SYSTEM's "My processes" while it runs (see `procowner`)
    let _claim = crate::procowner::current_user().map(|u| crate::procowner::claim(child.id(), &u, &p.label));
    // each pipe is read on its own thread; what arrives is sent in pieces so a stop never waits for a grandchild that holds the pipe
    let drain = |r: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let Some(mut r) = r else { return };
            let (mut left, mut chunk) = (MAX_OUTPUT, [0u8; 8192]);
            while left > 0 {
                match r.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let n = n.min(left);
                        left -= n;
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        rx
    };
    let (o, e) = (drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>)), drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)));
    let limit = Duration::from_secs(seconds.clamp(5, MAX_SECONDS));
    let t0 = Instant::now();
    let (mut stopped, mut timed_out) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if cancel.load(Ordering::SeqCst) {
                    stopped = true;
                } else if t0.elapsed() >= limit {
                    timed_out = true;
                }
                if stopped || timed_out {
                    // the whole group: cargo's test programs and a script's children end with it
                    #[cfg(unix)]
                    let _ = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let grab = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        let mut all = Vec::new();
        // after the program ended the pipe closes at once; a stopped one gets a moment, then what came is what there is
        while let Ok(piece) = rx.recv_timeout(Duration::from_millis(400)) {
            all.extend(piece);
        }
        all
    };
    let mut out = grab(o);
    out.extend(grab(e));
    let mut r = summarize(&out, status.and_then(|s| s.code()));
    r.stopped = stopped;
    r.timed_out = timed_out;
    r.ok = r.ok && !stopped && !timed_out;
    Ok(r)
}
