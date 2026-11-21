//! Running the tests of the open file from the editor (F-EDIT-16): `cargo test` for a Rust file in a Cargo project and `pytest` for
//! a Python test file, and a readable account of output that is not text.
//!
//! * [`plan`] decides what to run and where from the file's path alone (nothing is started).
//! * [`run`] starts the planned program in the project folder with a time limit and a stop flag, keeps at most [`MAX_OUTPUT`] bytes
//!   and reports [`TestRun`]: the counts, the failed test names and the last lines.
//! * [`describe_bytes`] turns bytes that are not text (an image, an archive, any binary) into one honest line instead of garbage.

pub use iyakki_shared::edittest::{MAX_SECONDS, TestPlan, TestRun, plan, up_to};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::testresults::{parse_output, Outcome};

/// The most output kept from a run.
pub const MAX_OUTPUT: usize = 256 * 1024;






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
    use iyakki_core_local::starters_launch::{Launch, Stream};
    let mut child = Launch::new(&p.program)
        .own_group()
        .args(&p.args)
        .cwd(&p.cwd)
        .env("NO_COLOR", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stream::Null)
        .stdout(Stream::Piped)
        .stderr(Stream::Piped)
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
                    iyakki_core_local::starters::kill_group(child.id());
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
