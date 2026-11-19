//! `iyakki backup` (F-BACKUP-01): the backup of one person's own data from the command line, over the same functions as Settings -> Backup.
//!
//! ```text
//! iyakki backup areas
//! iyakki backup preview --user NAME [--areas a,b]
//! iyakki backup create  --user NAME --to DIR [--areas a,b] [--pass-stdin | --pass-file FILE]
//! iyakki backup inspect --file FILE [--user NAME] [--pass-stdin | --pass-file FILE]
//! iyakki backup restore --user NAME --file FILE --areas a,b --yes [--other-account] [--pass-stdin | --pass-file FILE]
//! ```
//! `--data DIR` (default `IYAKKI_DATA` or `/var/lib/iyakki`) is the data folder. A passphrase is never taken from an argument (it would
//! show in the process list). A restore never runs without `--yes` and an explicit list of areas, and keeps what was there (renamed).

use std::collections::BTreeSet;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backup::{self, BackupError, CreateOptions, Layout, RestoreRequest};

const USAGE: &str = "usage: iyakki backup <areas|preview|create|inspect|restore> [options] (see docs/BACKUP.md)";

struct Opts {
    user: Option<String>,
    data: Option<String>,
    to: Option<String>,
    file: Option<String>,
    areas: Option<BTreeSet<String>>,
    pass_stdin: bool,
    pass_file: Option<String>,
    yes: bool,
    other: bool,
}

fn parse(a: &[String]) -> Result<Opts, String> {
    let mut o = Opts { user: None, data: None, to: None, file: None, areas: None, pass_stdin: false, pass_file: None, yes: false, other: false };
    let mut i = 0;
    let val = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        a.get(*i).cloned().ok_or_else(|| format!("{name} needs a value"))
    };
    while i < a.len() {
        match a[i].as_str() {
            "--user" => o.user = Some(val(&mut i, "--user")?),
            "--data" => o.data = Some(val(&mut i, "--data")?),
            "--to" => o.to = Some(val(&mut i, "--to")?),
            "--file" => o.file = Some(val(&mut i, "--file")?),
            "--areas" => {
                let v = val(&mut i, "--areas")?;
                let known = backup::all_areas();
                let set: BTreeSet<String> = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if let Some(bad) = set.iter().find(|s| !known.contains(*s)) {
                    return Err(format!("{bad} is not a backup area (see `iyakki backup areas`)"));
                }
                o.areas = Some(set);
            }
            "--pass-stdin" => o.pass_stdin = true,
            "--pass-file" => o.pass_file = Some(val(&mut i, "--pass-file")?),
            "--yes" => o.yes = true,
            "--other-account" => o.other = true,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(o)
}

fn passphrase(o: &Opts, stdin: &mut dyn BufRead) -> Result<Option<String>, String> {
    if o.pass_stdin && o.pass_file.is_some() {
        return Err("use --pass-stdin or --pass-file, not both".into());
    }
    if o.pass_stdin {
        let mut l = String::new();
        stdin.read_line(&mut l).map_err(|e| format!("passphrase: {e}"))?;
        let p = l.trim_end_matches(['\n', '\r']).to_string();
        return if p.is_empty() { Err("no passphrase given on standard input".into()) } else { Ok(Some(p)) };
    }
    if let Some(f) = &o.pass_file {
        let t = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        let p = t.trim_end_matches(['\n', '\r']).to_string();
        return if p.is_empty() { Err(format!("{f} is empty")) } else { Ok(Some(p)) };
    }
    Ok(None)
}

fn layout(o: &Opts) -> Result<Layout, String> {
    let user = o.user.clone().ok_or("--user NAME is needed (whose data)")?;
    let data = PathBuf::from(o.data.clone().or_else(|| std::env::var("IYAKKI_DATA").ok()).unwrap_or_else(|| "/var/lib/iyakki".into()));
    let home = crate::scope::home_root(&user, &data);
    Ok(Layout { data, home, user })
}

fn err(e: BackupError) -> String {
    e.to_string()
}

/// Run one `iyakki backup ...` command. Lines go to `out`; the return value is the exit code (0 done, 1 refused or failed, 2 bad use).
pub fn run(args: &[String], stdin: &mut dyn BufRead, now: i64, release: &str, out: &mut dyn FnMut(String)) -> i32 {
    run_with_stop(args, stdin, now, release, out, &AtomicBool::new(false))
}

/// Set by Ctrl-C while `iyakki backup create` runs (see [`cli_main`]).
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Pure: the progress line to print now, if the backup has passed the next 10 % step (`next` is moved on). Nothing before the total is known.
pub fn progress_line(done: u64, total: u64, next: &mut u64) -> Option<String> {
    if total == 0 {
        return None;
    }
    let pct = (done.min(total) * 100 / total).min(100);
    if pct < *next {
        return None;
    }
    *next = pct / 10 * 10 + 10;
    Some(format!("{pct}% ({} of {} read)", backup::human(done.min(total)), backup::human(total)))
}

/// [`run`] with a stop flag: when it turns true while a backup is being made, the backup is stopped and no file is left behind.
pub fn run_with_stop(args: &[String], stdin: &mut dyn BufRead, now: i64, release: &str, out: &mut dyn FnMut(String), stop: &AtomicBool) -> i32 {
    let Some((cmd, rest)) = args.split_first() else {
        out(USAGE.into());
        return 2;
    };
    let o = match parse(rest) {
        Ok(o) => o,
        Err(e) => {
            out(format!("iyakki backup: {e}"));
            return 2;
        }
    };
    let r: Result<(), String> = (|| match cmd.as_str() {
        "areas" => {
            for id in backup::all_areas() {
                out(format!("{id}\t{}", backup::area_label(&id)));
            }
            Ok(())
        }
        "preview" => {
            let l = layout(&o)?;
            for a in backup::preview(&l, &o.areas.clone().unwrap_or_else(backup::all_areas)).map_err(err)? {
                out(format!("{}\t{} files\t{}", a.label, a.files, backup::human(a.bytes)));
                for s in &a.skipped {
                    out(format!("  left out: {s}"));
                }
            }
            Ok(())
        }
        "create" => {
            let l = layout(&o)?;
            let dest = PathBuf::from(o.to.clone().ok_or("--to DIR is needed (a folder that exists)")?);
            let pass = passphrase(&o, stdin)?;
            let opts = CreateOptions { areas: o.areas.clone().unwrap_or_else(backup::all_areas), passphrase: pass, now, release: release.to_string() };
            let pr = backup::Progress::default();
            pr.cancel.store(stop.load(Ordering::Relaxed), Ordering::Relaxed);
            let made = std::thread::scope(|s| {
                let worker = s.spawn(|| backup::create_with_progress(&l, &dest, &opts, &pr));
                let mut next = 10;
                while !worker.is_finished() {
                    if stop.load(Ordering::Relaxed) {
                        pr.cancel.store(true, Ordering::Relaxed);
                    }
                    if let Some(line) = progress_line(pr.done.load(Ordering::Relaxed), pr.total.load(Ordering::Relaxed), &mut next) {
                        out(line);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                worker.join()
            });
            let made = made.map_err(|_| "the backup stopped unexpectedly".to_string())?;
            let name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match &made {
                Ok(c) => crate::appevent_more::note_backup_made(&l.data, &l.user, &name(&c.path), true),
                Err(_) => crate::appevent_more::note_backup_made(&l.data, &l.user, "backup", false),
            }
            let c = made.map_err(err)?;
            out(format!("wrote {} ({} files, {}, {})", c.path.display(), c.files, backup::human(c.bytes), if c.encrypted { "encrypted" } else { "NOT encrypted: anyone who gets the file can read it" }));
            Ok(())
        }
        "inspect" | "restore" => {
            let user = o.user.clone().unwrap_or_default();
            let mut o2 = Opts { user: Some(user.clone()), ..parse(&[]).unwrap() };
            o2.data = o.data.clone();
            let l = if user.is_empty() { Layout { data: PathBuf::from("/nonexistent"), home: PathBuf::from("/nonexistent"), user: String::new() } } else { layout(&o2)? };
            let file = PathBuf::from(o.file.clone().ok_or("--file FILE is needed")?);
            let pass = passphrase(&o, stdin)?;
            if cmd == "inspect" {
                let c = backup::inspect(&file, pass.as_deref(), &l).map_err(err)?;
                out(format!("{}: made by {} with {} {}, {} files, {}", if c.encrypted { "encrypted" } else { "not encrypted" }, c.account, c.release, if c.same_account { "(this account)" } else { "(another account)" }, c.total_files, backup::human(c.total_bytes)));
                for a in &c.areas {
                    out(format!("{}\t{} files\t{}", a.label, a.files, backup::human(a.bytes)));
                }
                return Ok(());
            }
            let areas = o.areas.clone().ok_or("--areas a,b is needed: a restore names what it brings back")?;
            if !o.yes {
                return Err("a restore changes your data (what is there is kept, renamed): add --yes to go ahead".into());
            }
            if user.is_empty() {
                return Err("--user NAME is needed (whose data)".into());
            }
            let rep = backup::restore(&l, &file, pass.as_deref(), &RestoreRequest { areas, allow_other_account: o.other, now }).map_err(err)?;
            for a in &rep.restored {
                out(format!("restored {} ({} files)", a.label, a.files));
            }
            for k in &rep.kept {
                out(format!("kept the old data as {}", k.display()));
            }
            Ok(())
        }
        other => Err(format!("unknown command {other}. {USAGE}")),
    })();
    match r {
        Ok(()) => 0,
        Err(e) => {
            out(format!("iyakki backup: {e}"));
            1
        }
    }
}

/// The program's entry (`iyakki backup ...`).
pub fn cli_main(args: &[String]) -> i32 {
    let stdin = std::io::stdin();
    // SAFETY: the handler only stores to an atomic; Ctrl-C stops a running backup (the half file is removed) instead of killing the program.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as usize);
    }
    run_with_stop(args, &mut stdin.lock(), crate::clock::now_unix(), env!("CARGO_PKG_VERSION"), &mut |l| crate::qprintln!("{l}"), &INTERRUPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-backupcli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn go(a: &str, stdin: &str) -> (i32, String) {
        let mut text = Vec::new();
        let code = run(&args(a), &mut std::io::Cursor::new(stdin.as_bytes().to_vec()), 1_800_000_000, "test", &mut |l| text.push(l));
        (code, text.join("\n"))
    }

    #[test]
    fn bad_use_is_exit_2_and_names_the_problem() {
        assert_eq!(go("", "").0, 2);
        let (c, t) = go("create --areas nonsense", "");
        assert_eq!(c, 2);
        assert!(t.contains("not a backup area"), "{t}");
        assert_eq!(go("create --bogus", "").0, 2);
        let (c, t) = go("areas", "");
        assert_eq!((c, t.contains("chats") || t.contains("settings")), (0, true), "{t}");
    }

    #[test]
    fn create_inspect_and_restore_round_trip_with_a_passphrase_from_stdin_and_a_restore_needs_yes() {
        let r = root("rt");
        let data = r.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(r.join("out")).unwrap();
        let l = Layout { data: data.clone(), home: crate::scope::home_root("ann", &data), user: "ann".into() };
        let first = backup::sources(&l).into_iter().find(|s| s.kind == backup::Kind::File).expect("a file source");
        let p = first.path(&l);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{\"hello\":1}").unwrap();
        let base = format!("--user ann --data {}", data.display());
        let (c, t) = go(&format!("preview {base}"), "");
        assert_eq!(c, 0, "{t}");
        let (c, t) = go(&format!("create {base} --to {} --areas {} --pass-stdin", r.join("out").display(), first.area), "a long enough passphrase\n");
        assert_eq!(c, 0, "{t}");
        assert!(t.contains("encrypted") && !t.contains("NOT encrypted"), "{t}");
        let file = std::fs::read_dir(r.join("out")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(go(&format!("inspect --file {} {base}", file.display()), "").0, 1, "an encrypted file needs the passphrase");
        let (c, t) = go(&format!("inspect --file {} {base} --pass-stdin", file.display()), "a long enough passphrase\n");
        assert_eq!(c, 0, "{t}");
        let before = std::fs::read(&p).unwrap();
        std::fs::write(&p, b"changed").unwrap();
        let (c, t) = go(&format!("restore --file {} {base} --areas {} --pass-stdin", file.display(), first.area), "a long enough passphrase\n");
        assert_eq!(c, 1, "no --yes, nothing restored");
        assert!(t.contains("--yes"), "{t}");
        assert_eq!(std::fs::read(&p).unwrap(), b"changed");
        let (c, t) = go(&format!("restore --file {} {base} --areas {} --yes --pass-stdin", file.display(), first.area), "a long enough passphrase\n");
        assert_eq!(c, 0, "{t}");
        assert_eq!(std::fs::read(&p).unwrap(), before);
        assert!(t.contains("kept the old data"), "{t}");
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn progress_lines_come_at_each_tenth_and_not_before_the_total_is_known() {
        let mut next = 10;
        assert_eq!(progress_line(5, 0, &mut next), None);
        assert_eq!(progress_line(5, 100, &mut next), None, "5 % is below the first step");
        let a = progress_line(10, 100, &mut next).expect("10 %");
        assert!(a.starts_with("10%"), "{a}");
        assert_eq!(progress_line(15, 100, &mut next), None, "the next step is 20 %");
        assert!(progress_line(47, 100, &mut next).unwrap().starts_with("47%"));
        assert_eq!(next, 50);
        assert!(progress_line(900, 100, &mut next).unwrap().starts_with("100%"), "never above the total");
    }

    #[test]
    fn a_stopped_backup_says_so_and_leaves_no_file() {
        let r = root("stop");
        let data = r.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(r.join("out")).unwrap();
        let l = Layout { data: data.clone(), home: crate::scope::home_root("ann", &data), user: "ann".into() };
        let first = backup::sources(&l).into_iter().find(|s| s.kind == backup::Kind::File).expect("a file source");
        let p = first.path(&l);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![1u8; 4 << 20]).unwrap();
        let stop = AtomicBool::new(true);
        let mut text = Vec::new();
        let a = args(&format!("create --user ann --data {} --to {} --areas {}", data.display(), r.join("out").display(), first.area));
        let code = run_with_stop(&a, &mut std::io::Cursor::new(Vec::new()), 1_800_000_000, "test", &mut |l| text.push(l), &stop);
        let t = text.join("\n");
        assert!(code == 1 && t.contains("stopped"), "{code} {t}");
        assert!(std::fs::read_dir(r.join("out")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_short_or_missing_passphrase_is_refused_and_a_file_is_not_encrypted_unless_asked() {
        let r = root("pw");
        let data = r.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(r.join("out")).unwrap();
        let base = format!("--user ann --data {} --to {}", data.display(), r.join("out").display());
        let (c, t) = go(&format!("create {base} --pass-stdin"), "short\n");
        assert_eq!(c, 1, "{t}");
        assert_eq!(go(&format!("create {base} --pass-stdin"), "\n").0, 1);
        let (c, t) = go(&format!("create {base}"), "");
        assert_eq!(c, 0, "{t}");
        assert!(t.contains("NOT encrypted"), "{t}");
        let _ = std::fs::remove_dir_all(&r);
    }
}
