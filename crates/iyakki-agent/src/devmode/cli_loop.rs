//! `iyakki dev loop`: the fast loop as a subcommand of the developer CLI (F-SPLIT-08, docs/development/FAST-LOOP.md).
//!
//! It watches the repository folder; when files change it settles for a moment, runs the loop script (`dist/scripts/fast-loop.py`: detect the layer, build,
//! test only what changed, push the program, optionally apply it without a device restart), opens the app on the device again and prints the timings,
//! from the moment of the edit (the newest modified time of the changed files) to the end of the last step. The work is done by the script and the
//! existing `push-apps` / `open` commands; this file only watches, decides which app to reopen and keeps the clock.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::cli::Args;
use super::client::DevClient;

pub const USAGE: &str = "  loop [--repo DIR] [--reopen APP] [--base REV] [--no-test] [--apply] [--fetch CMD] [--once]   the fast loop: watch the repository, test what changed, push the program, reopen the app, print the timings (scope update)";

/// Folders that never count as an edit.
const SKIP: [&str; 6] = ["target", ".git", "node_modules", ".claude", ".kube", "iyakki-boot"];

/// Path -> (modified seconds with fraction as milliseconds, size) of every file under `root` that counts as an edit.
pub fn snapshot(root: &Path) -> BTreeMap<String, (u128, u64)> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !(d == root && SKIP.contains(&name.as_str())) && !(name == "target" || name == ".git") {
                    todo.push(e.path());
                }
            } else if ft.is_file() {
                let Ok(md) = e.metadata() else { continue };
                let ms = md.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
                let rel = e.path().strip_prefix(root).map(|p| p.to_string_lossy().to_string()).unwrap_or(name);
                out.insert(rel, (ms, md.len()));
            }
        }
    }
    out
}

/// The paths that are new, gone or different between two snapshots, and the newest modified time (ms since the epoch) among the new or changed ones.
pub fn changed(old: &BTreeMap<String, (u128, u64)>, new: &BTreeMap<String, (u128, u64)>) -> (Vec<String>, u128) {
    let mut files: Vec<String> = new.iter().filter(|(k, v)| old.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
    let edit_ms = files.iter().filter_map(|f| new.get(f)).map(|v| v.0).max().unwrap_or(0);
    files.extend(old.keys().filter(|k| !new.contains_key(*k)).cloned());
    files.sort();
    (files, edit_ms)
}

/// The app to open again on the device for a change: `crates/iyakki-apps/src/apps/NAME.rs` (or `NAME/...`, `NAME_tests.rs`) is the app NAME, a tile crate
/// `crates/iyakki-tile-NAME/` is the tile NAME. A change that names several apps, or none, gives None (use --reopen).
pub fn guess_app(files: &[String]) -> Option<String> {
    let mut names = std::collections::BTreeSet::new();
    for f in files {
        if let Some(rest) = f.strip_prefix("crates/iyakki-apps/src/apps/") {
            let first = rest.split('/').next().unwrap_or("");
            let stem = first.strip_suffix(".rs").unwrap_or(first);
            let stem = stem.strip_suffix("_tests").or_else(|| stem.strip_suffix("_test")).unwrap_or(stem);
            if !stem.is_empty() && stem != "mod" {
                names.insert(stem.to_string());
            }
        } else if let Some(rest) = f.strip_prefix("crates/iyakki-tile-") {
            if let Some(n) = rest.split('/').next().filter(|n| !n.is_empty()) {
                names.insert(n.to_string());
            }
        }
    }
    if names.len() == 1 { names.into_iter().next() } else { None }
}

/// The command line of one pass: the loop script with the options that were given to `dev loop`.
pub(super) fn script_argv(repo: &Path, args: &Args) -> Vec<String> {
    let mut v = vec!["python3".to_string(), repo.join("dist/scripts/fast-loop.py").display().to_string()];
    for k in ["--device", "--base", "--home", "--fetch", "--against"] {
        if let Some(x) = args.get(k) {
            v.push(k.into());
            v.push(x.into());
        }
    }
    for k in ["--no-test", "--apply", "--no-push"] {
        if args.has(k) {
            v.push(k.into());
        }
    }
    v
}

/// What one pass prints at its end: the time from the edit, and from the start of the pass.
pub fn timing_line(edit_to_start: Duration, pass: Duration, ok: bool) -> String {
    format!("loop: {} edit -> done {:.1} s (settle and detect {:.1} s, steps {:.1} s)", if ok { "OK" } else { "FAILED:" }, (edit_to_start + pass).as_secs_f64(), edit_to_start.as_secs_f64(), pass.as_secs_f64())
}

/// The loop itself, with its effects handed in so it can be tested: `step` runs one pass (the script) and returns its exit code, `reopen` opens an app on
/// the device, `now_ms` is the clock, `go_on` says whether to continue.
pub struct Loop<'a> {
    pub repo: PathBuf,
    pub reopen_app: Option<String>,
    pub once: bool,
    pub poll: Duration,
    pub settle: Duration,
    pub step: &'a mut dyn FnMut(&mut dyn FnMut(&str)) -> i32,
    pub reopen: &'a mut dyn FnMut(&str) -> Result<(), String>,
    pub now_ms: &'a dyn Fn() -> u128,
    pub go_on: &'a dyn Fn() -> bool,
}

impl Loop<'_> {
    pub fn run(&mut self, out: &mut dyn FnMut(&str)) -> Result<(), String> {
        let mut seen = snapshot(&self.repo);
        let mut pending: Option<(Vec<String>, u128)> = if self.once { Some((Vec::new(), (self.now_ms)())) } else { None };
        if !self.once {
            out(&format!("loop: watching {} ({} files). Save a file; it is tested, pushed and opened again. Ctrl+C stops.", self.repo.display(), seen.len()));
        }
        loop {
            if pending.is_none() {
                if !(self.go_on)() {
                    return Ok(());
                }
                std::thread::sleep(self.poll);
                let now = snapshot(&self.repo);
                let (files, edit_ms) = changed(&seen, &now);
                if files.is_empty() {
                    continue;
                }
                std::thread::sleep(self.settle); // let a burst of saves finish
                let again = snapshot(&self.repo);
                let (files, edit_ms2) = changed(&seen, &again);
                pending = Some((files, edit_ms.max(edit_ms2)));
            }
            let (files, edit_ms) = pending.take().unwrap_or_default();
            let t = Instant::now();
            let started_ms = (self.now_ms)();
            out(&format!("loop: {} changed file(s){}", files.len(), files.first().map(|f| format!(", first {f}")).unwrap_or_default()));
            let rc = (self.step)(out);
            let mut ok = rc == 0;
            if ok {
                let app = self.reopen_app.clone().or_else(|| guess_app(&files));
                if let Some(app) = app {
                    let t2 = Instant::now();
                    match (self.reopen)(&app) {
                        Ok(()) => out(&format!("loop: opened {app} on the device again ({:.1} s)", t2.elapsed().as_secs_f64())),
                        Err(e) => {
                            out(&format!("loop: could not open {app}: {e}"));
                            ok = false;
                        }
                    }
                } else {
                    out("loop: no single app to open again (name one with --reopen APP)");
                }
            } else {
                out(&format!("loop: the pass stopped with exit code {rc}; nothing was opened"));
            }
            let from_edit = if edit_ms > 0 { Duration::from_millis(started_ms.saturating_sub(edit_ms) as u64) } else { Duration::ZERO };
            out(&timing_line(from_edit, t.elapsed(), ok));
            if self.once {
                return if ok { Ok(()) } else { Err("the loop pass failed".into()) };
            }
            seen = snapshot(&self.repo);
        }
    }
}

pub(super) fn run(cmd: &str, args: &Args, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    (cmd == "loop").then(|| run_real(args, connect, out))
}

fn run_real(args: &Args, connect: &dyn Fn() -> Result<DevClient, String>, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let repo = PathBuf::from(args.get("--repo").unwrap_or("."));
    let repo = std::fs::canonicalize(&repo).map_err(|e| format!("{}: {e}", repo.display()))?;
    if !repo.join("dist/scripts/fast-loop.py").is_file() {
        return Err(format!("{} is not an IYAKKI checkout (dist/scripts/fast-loop.py is missing); give it with --repo", repo.display()));
    }
    let argv = script_argv(&repo, args);
    let mut step = |out: &mut dyn FnMut(&str)| -> i32 {
        use std::io::BufRead;
        let mut child = match std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(&repo).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn() {
            Ok(c) => c,
            Err(e) => {
                out(&format!("loop: cannot start {}: {e}", argv[0]));
                return 127;
            }
        };
        if let Some(so) = child.stdout.take() {
            for l in std::io::BufReader::new(so).lines().map_while(Result::ok) {
                out(&l);
            }
        }
        child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
    };
    let mut reopen = |app: &str| -> Result<(), String> { connect()?.open(app).map(|_| ()) };
    let now_ms = || SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let go_on = || true;
    Loop { repo: repo.clone(), reopen_app: args.get("--reopen").map(String::from), once: args.has("--once"), poll: Duration::from_millis(500), settle: Duration::from_millis(400), step: &mut step, reopen: &mut reopen, now_ms: &now_ms, go_on: &go_on }.run(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-devloop-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn args(a: &[&str]) -> Args {
        Args::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn a_change_is_seen_but_target_and_git_are_not() {
        let d = tmp("snap");
        std::fs::create_dir_all(d.join("crates/a/src")).unwrap();
        std::fs::create_dir_all(d.join("target/debug")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join("crates/a/src/lib.rs"), "1").unwrap();
        std::fs::write(d.join("target/debug/x"), "1").unwrap();
        std::fs::write(d.join(".git/HEAD"), "1").unwrap();
        let a = snapshot(&d);
        assert_eq!(a.keys().collect::<Vec<_>>(), vec!["crates/a/src/lib.rs"]);
        std::fs::write(d.join("crates/a/src/lib.rs"), "22").unwrap();
        std::fs::write(d.join("target/debug/x"), "333").unwrap();
        let b = snapshot(&d);
        let (files, edit) = changed(&a, &b);
        assert_eq!(files, vec!["crates/a/src/lib.rs"]);
        assert!(edit > 0);
        assert!(changed(&b, &b).0.is_empty());
        std::fs::remove_file(d.join("crates/a/src/lib.rs")).unwrap();
        assert_eq!(changed(&b, &snapshot(&d)).0, vec!["crates/a/src/lib.rs"]);
    }

    #[test]
    fn the_app_to_open_again_is_guessed_only_when_it_is_one() {
        let g = |f: &[&str]| guess_app(&f.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(g(&["crates/iyakki-apps/src/apps/radio.rs"]), Some("radio".into()));
        assert_eq!(g(&["crates/iyakki-apps/src/apps/radio_tests.rs", "crates/iyakki-apps/src/apps/radio/x.rs"]), Some("radio".into()));
        assert_eq!(g(&["crates/iyakki-tile-pdf/src/lib.rs"]), Some("pdf".into()));
        assert_eq!(g(&["crates/iyakki-apps/src/apps/radio.rs", "crates/iyakki-apps/src/apps/pdf.rs"]), None);
        assert_eq!(g(&["crates/iyakki-core/src/main.rs"]), None);
        assert_eq!(g(&["crates/iyakki-apps/src/apps/mod.rs"]), None);
    }

    #[test]
    fn the_script_gets_the_options_it_understands() {
        let v = script_argv(Path::new("/r"), &args(&["loop", "--device", "lab", "--no-test", "--apply", "--fetch", "scp x y", "--base", "HEAD", "--reopen", "radio"]));
        assert_eq!(v[..2], ["python3".to_string(), "/r/dist/scripts/fast-loop.py".to_string()]);
        let s = v.join(" ");
        assert!(s.contains("--device lab") && s.contains("--no-test") && s.contains("--apply") && s.contains("--fetch scp x y") && s.contains("--base HEAD"));
        assert!(!s.contains("--reopen"), "{s}");
    }

    #[test]
    fn one_pass_runs_the_script_opens_the_app_and_prints_the_timing_from_the_edit() {
        let d = tmp("once");
        std::fs::create_dir_all(d.join("crates/iyakki-apps/src/apps")).unwrap();
        std::fs::write(d.join("crates/iyakki-apps/src/apps/radio.rs"), "a").unwrap();
        let opened = RefCell::new(Vec::new());
        let ran = Cell::new(0);
        let mut step = |out: &mut dyn FnMut(&str)| {
            ran.set(ran.get() + 1);
            out("[fast] total 12.0 s");
            0
        };
        let mut reopen = |a: &str| {
            opened.borrow_mut().push(a.to_string());
            Ok(())
        };
        let now = || 5_000u128;
        let go = || true;
        let mut lines = Vec::new();
        Loop { repo: d.clone(), reopen_app: Some("radio".into()), once: true, poll: Duration::ZERO, settle: Duration::ZERO, step: &mut step, reopen: &mut reopen, now_ms: &now, go_on: &go }.run(&mut |s| lines.push(s.to_string())).unwrap();
        assert_eq!((ran.get(), opened.borrow().clone()), (1, vec!["radio".to_string()]));
        assert!(lines.iter().any(|l| l.starts_with("loop: OK edit -> done")), "{lines:?}");
    }

    #[test]
    fn a_failed_pass_opens_nothing_and_says_so() {
        let d = tmp("fail");
        let opened = Cell::new(false);
        let mut step = |_: &mut dyn FnMut(&str)| 1;
        let mut reopen = |_: &str| {
            opened.set(true);
            Ok(())
        };
        let now = || 1u128;
        let go = || true;
        let mut lines = Vec::new();
        let r = Loop { repo: d, reopen_app: Some("radio".into()), once: true, poll: Duration::ZERO, settle: Duration::ZERO, step: &mut step, reopen: &mut reopen, now_ms: &now, go_on: &go }.run(&mut |s| lines.push(s.to_string()));
        assert!(r.is_err() && !opened.get());
        assert!(lines.iter().any(|l| l.contains("nothing was opened")) && lines.iter().any(|l| l.contains("FAILED")), "{lines:?}");
    }

    #[test]
    fn a_saved_file_starts_a_pass_and_the_loop_stops_when_told() {
        let d = tmp("watch");
        std::fs::create_dir_all(d.join("crates/iyakki-tile-pdf/src")).unwrap();
        std::fs::write(d.join("crates/iyakki-tile-pdf/src/lib.rs"), "a").unwrap();
        let polls = Cell::new(0);
        let go = || {
            polls.set(polls.get() + 1);
            if polls.get() == 2 {
                std::fs::write(d.join("crates/iyakki-tile-pdf/src/lib.rs"), "bb").unwrap();
            }
            polls.get() < 6
        };
        let opened = RefCell::new(Vec::new());
        let mut step = |_: &mut dyn FnMut(&str)| 0;
        let mut reopen = |a: &str| {
            opened.borrow_mut().push(a.to_string());
            Ok(())
        };
        let now = || SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
        let mut lines = Vec::new();
        Loop { repo: d.clone(), reopen_app: None, once: false, poll: Duration::from_millis(5), settle: Duration::from_millis(5), step: &mut step, reopen: &mut reopen, now_ms: &now, go_on: &go }.run(&mut |s| lines.push(s.to_string())).unwrap();
        assert_eq!(opened.borrow().clone(), vec!["pdf".to_string()], "{lines:?}");
    }
}
