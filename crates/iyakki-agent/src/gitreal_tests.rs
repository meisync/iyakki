//! `LocalRepo` against the real git program and real repositories in a temp folder (no fake runner, no network). Skipped when git
//! is not installed.

#[allow(unused_imports)]
use iyakki_shared::gitbrowse::*;
#[allow(unused_imports)]
#[allow(unused_imports)]
use crate::gitconn;
#[allow(unused_imports)]
use iyakki_shared::gitconn::{Connection, GitHttp, Kind, NetErr, Req, Resp, TOO_BIG};
#[allow(unused_imports)]
use iyakki_shared::wsexec::Runner;
#[allow(unused_imports)]
use serde_json::Value;
#[allow(unused_imports)]
use zeroize::Zeroizing;
#[allow(unused_imports)]
use std::time::Duration;
use crate::wsexec::System;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@example.org", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false", "-c", "protocol.file.allow=always"]).args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-gitreal-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_real_working_copy_reads_branch_ahead_behind_every_change_kind_and_a_file_diff_without_writing() {
    if !have_git() {
        eprintln!("skipped: no git");
        return;
    }
    let base = tmp("work");
    let (origin, clone, other) = (base.join("origin.git"), base.join("clone"), base.join("other"));
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare"]);
    git(&base, &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()]);
    for (n, t) in [("keep.txt", "keep\n"), ("edit.txt", "one\n"), ("gone.txt", "bye\n"), ("old name.txt", "same content here\nmore\nlines\n")] {
        std::fs::write(clone.join(n), t).unwrap();
    }
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-q", "-m", "first"]);
    git(&clone, &["push", "-q", "-u", "origin", "main"]);
    // upstream moves on by one commit, which this clone fetches (so: behind 1)
    git(&base, &["clone", "-q", origin.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("up.txt"), "u").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "upstream"]);
    git(&other, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q"]);
    // and one local commit (ahead 1)
    std::fs::write(clone.join("local.txt"), "l").unwrap();
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-q", "-m", "local"]);
    // now every kind of change
    std::fs::write(clone.join("edit.txt"), "two\n").unwrap();
    std::fs::remove_file(clone.join("gone.txt")).unwrap();
    git(&clone, &["rm", "-q", "--cached", "gone.txt"]);
    git(&clone, &["mv", "old name.txt", "new name.txt"]);
    std::fs::write(clone.join("added.txt"), "a").unwrap();
    git(&clone, &["add", "added.txt"]);
    std::fs::write(clone.join("notes.md"), "n").unwrap();
    let before = std::fs::read(clone.join(".git/index")).unwrap();

    let repo = LocalRepo::new(Arc::new(System), &clone, &base.join("scratch"));
    let w = repo.working().unwrap();
    assert_eq!((w.status.branch.as_str(), w.status.ahead, w.status.behind), ("main", 1, 1), "{:?}", w.status);
    let by = |p: &str| w.changes.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("{p} in {:?}", w.changes));
    assert_eq!(by("edit.txt").kind, ChangeKind::Modified);
    assert!(by("edit.txt").unstaged && !by("edit.txt").staged);
    assert_eq!(by("gone.txt").kind, ChangeKind::Deleted);
    assert_eq!((by("new name.txt").kind, by("new name.txt").from.as_str()), (ChangeKind::Renamed, "old name.txt"));
    assert_eq!((by("added.txt").kind, by("added.txt").staged), (ChangeKind::Added, true));
    assert_eq!(by("notes.md").kind, ChangeKind::Untracked);
    assert_eq!(w.more, 0);
    // one file's difference against the last commit
    let d = repo.diff_head("edit.txt").unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!((d.files[0].added, d.files[0].removed), (1, 1));
    assert!(repo.diff_head("../etc/passwd").is_err() && repo.diff_head("--output=x").is_err(), "paths that could become options are refused");
    // reading changed nothing
    assert_eq!(std::fs::read(clone.join(".git/index")).unwrap(), before, "status takes no lock and rewrites no index");
    // a merge conflict is reported as conflicted
    git(&clone, &["stash", "-q", "-u"]);
    std::fs::write(other.join("edit.txt"), "theirs\n").unwrap();
    git(&other, &["commit", "-q", "-am", "theirs"]);
    git(&other, &["push", "-q", "origin", "main"]);
    std::fs::write(clone.join("edit.txt"), "mine\n").unwrap();
    git(&clone, &["commit", "-q", "-am", "mine"]);
    git(&clone, &["fetch", "-q"]);
    let o = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@example.org", "merge", "origin/main"]).current_dir(&clone).output().unwrap();
    assert!(!o.status.success(), "the merge must conflict");
    let w = repo.working().unwrap();
    assert!(w.changes.iter().any(|c| c.path == "edit.txt" && c.kind == ChangeKind::Conflicted), "{:?}", w.changes);
    assert_eq!(w.status.conflicted, 1);
    let _ = std::fs::remove_dir_all(&base);
}

/// The real runner, with git told not to look above one folder (`GIT_CEILING_DIRECTORIES`): a folder that must NOT be a repository stays
/// one even when the temp folder sits inside a checkout (the public CI sets TMPDIR to a folder inside the repository).
struct Ceiling(PathBuf);

impl Runner for Ceiling {
    fn run(&self, cwd: &Path, argv: &[String], env: &[(String, String)], timeout: Duration, cap: usize, sink: Option<iyakki_shared::wsexec::Sink>) -> iyakki_shared::wsexec::Out {
        let mut env = env.to_vec();
        env.push(("GIT_CEILING_DIRECTORIES".into(), self.0.display().to_string()));
        System.run(cwd, argv, &env, timeout, cap, sink)
    }
    fn has(&self, program: &str) -> bool {
        System.has(program)
    }
}

#[test]
fn a_real_log_pages_newest_first_and_a_non_repository_is_a_calm_error() {
    if !have_git() {
        return;
    }
    let base = tmp("log");
    git(&base, &["init", "-q"]);
    for i in 0..5 {
        std::fs::write(base.join("f.txt"), format!("{i}\n")).unwrap();
        git(&base, &["add", "."]);
        git(&base, &["commit", "-q", "-m", &format!("commit {i}")]);
    }
    let repo = LocalRepo::new(Arc::new(System), &base, &base.join(".scratch-none"));
    let (p1, more) = repo.commits_page("HEAD", 0, 2).unwrap();
    assert_eq!((p1.len(), more), (2, true));
    assert_eq!(p1[0].subject(), "commit 4");
    let (p3, more) = repo.commits_page("HEAD", 4, 2).unwrap();
    assert_eq!((p3.len(), more, p3[0].subject()), (1, false, "commit 0".to_string()));
    assert!(repo.commits_page("--all", 0, 2).is_err(), "a ref that could become an option is refused");
    let plain = tmp("plain");
    let none = LocalRepo::new(Arc::new(Ceiling(plain.parent().unwrap().to_path_buf())), &plain, &plain);
    assert!(none.working().is_err());
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&plain);
}
