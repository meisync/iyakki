//! Tests of parallel tasks with fake runners and temporary git repositories (real `git`, temp folders only).

// the boundary scan reads a file's code up to its first #[cfg(test)]: this whole file is test code (it runs real `git` in temp folders)
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::agent::{RunEvent, RunHandle, Tools, Verdict};
use crate::tasks::*;
use crate::tasktools::TaskTools;

fn tmp(tag: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-tasks-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn g(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

const BOARD: &str = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 3 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Started | Finished | Notes |\n|---|---|---|---|---|---|---|\n| F-A-01 | Make the first thing | planned |  |  |  |  |\n| F-A-02 | Make the second thing | planned |  |  |  |  |\n| F-A-03 | Make the third thing | planned |  |  |  |  |\n";

fn repo(tag: &str) -> PathBuf {
    let d = tmp(tag);
    g(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(d.join("FEATURES.md"), BOARD).unwrap();
    std::fs::write(d.join(".gitignore"), "target\n.iyakki/\n").unwrap();
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// What a fake run can do: it gets the spec, the event sender, the cancel flag and the decisions.
type Script = Arc<dyn Fn(RunSpec, Sender<RunEvent>, Arc<AtomicBool>, Receiver<(String, Verdict)>) + Send + Sync>;

fn runner(script: Script) -> Runner {
    Arc::new(move |spec: RunSpec| {
        let (etx, erx) = channel();
        let (dtx, drx) = channel();
        let h = RunHandle::from_parts(erx, dtx);
        let cancel = h.cancel_flag();
        let s = script.clone();
        std::thread::spawn(move || s(spec, etx, cancel, drx));
        h
    })
}

fn finish_with(etx: &Sender<RunEvent>, text: &str) {
    let _ = etx.send(RunEvent::Final { text: text.into() });
}

fn writer(file: &'static str, body: &'static str) -> Script {
    Arc::new(move |spec, etx, _, _| {
        std::fs::write(spec.tree.join(file), body).unwrap();
        let _ = etx.send(RunEvent::Note { text: format!("wrote {file}") });
        finish_with(&etx, "done");
    })
}

fn manager(root: &Path, s: Script) -> Manager {
    let m = Manager::open(root, &root.join("FEATURES.md"));
    m.set_runner(runner(s));
    m
}

fn nt(title: &str) -> NewTask {
    NewTask { title: title.into(), goal: format!("do {title}"), ..Default::default() }
}

fn status(m: &Manager, id: &str) -> Status {
    m.get(id).unwrap().status
}

fn done(m: &Manager, id: &str) {
    until("the task to finish", || !matches!(status(m, id), Status::Queued | Status::Running | Status::NeedsYou));
}

#[test]
fn a_git_task_works_in_its_own_worktree_and_merges_cleanly_without_touching_the_main_files_before() {
    let root = repo("clean");
    let m = manager(&root, writer("new.txt", "hello\n"));
    let id = m.spawn(nt("Add new file")).unwrap();
    done(&m, &id);
    let t = m.get(&id).unwrap();
    assert_eq!(t.status, Status::Done, "{}", t.error);
    assert_eq!(t.mode, Mode::Git);
    assert!(t.branch.starts_with(&format!("iyakki/task-{id}-add-new-file")), "{}", t.branch);
    assert_eq!(t.tree, root.join(".iyakki/tasks").join(&id).join("tree"));
    assert!(t.tree.join("new.txt").exists(), "the task wrote in its tree");
    assert!(!root.join("new.txt").exists(), "nothing reached the main workspace");
    assert!(g(&root, &["status", "--porcelain"]).trim().is_empty(), "the main repository stays clean: {}", g(&root, &["status", "--porcelain"]));
    assert_eq!(t.base, g(&root, &["rev-parse", "HEAD"]).trim());
    let files = m.files(&id).unwrap();
    assert_eq!(files, vec![('A', "new.txt".to_string())]);
    let d = m.diff(&id).unwrap();
    assert!(d.contains("+++ b/new.txt") && d.contains("+hello"), "{d}");
    let mc = m.merge_check(&id).unwrap();
    assert!(mc.clean && mc.files == 1, "{mc:?}");
    let msg = m.merge(&id).unwrap();
    assert!(msg.contains("merged"), "{msg}");
    assert_eq!(std::fs::read_to_string(root.join("new.txt")).unwrap(), "hello\n");
    assert_eq!(status(&m, &id), Status::Merged);
    assert!(!t.tree.exists(), "the worktree is removed after the merge");
    assert!(!g(&root, &["branch", "--list", "iyakki/*"]).contains("iyakki/task"), "the merged branch is deleted");
    assert!(g(&root, &["log", "--format=%s", "-n", "1"]).contains(&format!("Merge task {id}")), "a --no-ff merge commit");
}

#[test]
fn a_conflict_is_found_before_anything_is_applied_and_discard_cleans_up_the_branch_and_the_tree() {
    let root = repo("conflict");
    let s: Script = Arc::new(|spec, etx, _, _| {
        std::fs::write(spec.tree.join("a.txt"), "one\nTASK\nthree\n").unwrap();
        finish_with(&etx, "ok");
    });
    let m = manager(&root, s);
    let id = m.spawn(nt("Edit a")).unwrap();
    done(&m, &id);
    // the person changes the same line in the main branch meanwhile
    std::fs::write(root.join("a.txt"), "one\nMAIN\nthree\n").unwrap();
    g(&root, &["commit", "-qam", "main edit"]);
    let mc = m.merge_check(&id).unwrap();
    assert!(!mc.clean);
    assert_eq!(mc.conflicts, vec!["a.txt".to_string()], "{mc:?}");
    let err = m.merge(&id).unwrap_err();
    assert!(err.contains("a.txt"), "{err}");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\nMAIN\nthree\n", "nothing was applied");
    assert!(g(&root, &["status", "--porcelain"]).trim().is_empty(), "no half-done merge is left");
    let r = m.rebase(&id).unwrap_err();
    assert!(r.contains("conflicts in a.txt") && r.contains("unchanged"), "{r}");
    assert_eq!(status(&m, &id), Status::Done);
    let t = m.get(&id).unwrap();
    m.discard(&id).unwrap();
    assert_eq!(status(&m, &id), Status::Discarded);
    assert!(!t.tree.exists());
    assert!(g(&root, &["branch", "--list", "iyakki/*"]).trim().is_empty(), "the branch is deleted");
    assert!(g(&root, &["worktree", "list"]).lines().count() == 1, "the worktree is deregistered");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\nMAIN\nthree\n");
}

#[test]
fn rebase_on_latest_moves_a_stale_task_forward_when_the_changes_do_not_overlap() {
    let root = repo("rebase");
    let m = manager(&root, writer("b.txt", "b\n"));
    let id = m.spawn(nt("Add b")).unwrap();
    done(&m, &id);
    std::fs::write(root.join("c.txt"), "c\n").unwrap();
    g(&root, &["add", "c.txt"]);
    g(&root, &["commit", "-qm", "add c"]);
    m.rebase(&id).unwrap();
    assert_eq!(m.get(&id).unwrap().base, g(&root, &["rev-parse", "HEAD"]).trim());
    assert!(m.merge_check(&id).unwrap().clean);
    m.merge(&id).unwrap();
    assert!(root.join("b.txt").exists() && root.join("c.txt").exists());
}

#[test]
fn uncommitted_changes_in_a_file_the_task_changed_block_the_merge_with_a_clear_message() {
    let root = repo("dirty");
    let s: Script = Arc::new(|spec, etx, _, _| {
        std::fs::write(spec.tree.join("a.txt"), "one\ntwo\nTASK\n").unwrap();
        finish_with(&etx, "ok");
    });
    let m = manager(&root, s);
    let id = m.spawn(nt("Edit a")).unwrap();
    done(&m, &id);
    std::fs::write(root.join("a.txt"), "mine, not committed\n").unwrap();
    let mc = m.merge_check(&id).unwrap();
    assert!(!mc.clean && mc.blocking == vec!["a.txt".to_string()] && mc.message.contains("uncommitted"), "{mc:?}");
    assert!(m.merge(&id).is_err());
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "mine, not committed\n");
}

#[test]
fn without_git_the_task_gets_a_copy_that_skips_ignored_and_heavy_folders_and_merges_as_a_patch() {
    let root = tmp("copy");
    std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(root.join("FEATURES.md"), BOARD).unwrap();
    std::fs::write(root.join(".gitignore"), "out\n").unwrap();
    std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
    std::fs::write(root.join("node_modules/x/i.js"), "x").unwrap();
    std::fs::create_dir_all(root.join("out")).unwrap();
    std::fs::write(root.join("out/o.bin"), "x").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/m.rs"), "fn main() {}\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc", root.join("link")).unwrap();
    assert!(!is_git_root(&root));
    let s: Script = Arc::new(|spec, etx, _, _| {
        std::fs::write(spec.tree.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(spec.tree.join("src/new.rs"), "// new\n").unwrap();
        finish_with(&etx, "ok");
    });
    let m = manager(&root, s);
    let id = m.spawn(nt("Copy task")).unwrap();
    let t = m.get(&id).unwrap();
    assert_eq!(t.mode, Mode::Copy);
    assert!(t.tree.join("src/m.rs").exists() && !t.tree.join("node_modules").exists() && !t.tree.join("out").exists() && !t.tree.join("link").exists());
    done(&m, &id);
    assert_eq!(status(&m, &id), Status::Done, "{}", m.get(&id).unwrap().error);
    assert!(!root.join("src/new.rs").exists());
    let files: Vec<String> = m.files(&id).unwrap().into_iter().map(|f| f.1).collect();
    assert_eq!(files, vec!["a.txt".to_string(), "src/new.rs".to_string()]);
    let mc = m.merge_check(&id).unwrap();
    assert!(mc.clean, "{mc:?}");
    m.merge(&id).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
    assert!(root.join("src/new.rs").exists());
    assert!(!root.join(".git").exists(), "no repository is made in the user's folder");
    assert!(!t.tree.exists());
}

#[test]
fn a_patch_that_no_longer_applies_names_the_file_and_changes_nothing() {
    let root = tmp("copy-conflict");
    std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(root.join("FEATURES.md"), BOARD).unwrap();
    let s: Script = Arc::new(|spec, etx, _, _| {
        std::fs::write(spec.tree.join("a.txt"), "one\nTASK\n").unwrap();
        finish_with(&etx, "ok");
    });
    let m = manager(&root, s);
    let id = m.spawn(nt("Edit")).unwrap();
    done(&m, &id);
    std::fs::write(root.join("a.txt"), "one\nMAIN\n").unwrap();
    let mc = m.merge_check(&id).unwrap();
    assert!(!mc.clean && mc.conflicts.contains(&"a.txt".to_string()), "{mc:?}");
    assert!(m.merge(&id).is_err());
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\nMAIN\n");
}

#[test]
fn a_workspace_over_the_size_cap_is_refused_with_a_message_and_leaves_nothing_behind() {
    let root = tmp("big");
    std::fs::write(root.join("FEATURES.md"), BOARD).unwrap();
    let f = std::fs::File::create(root.join("huge.bin")).unwrap();
    f.set_len(COPY_MAX_BYTES + 1).unwrap();
    let m = manager(&root, writer("x", "x"));
    let e = m.spawn(nt("Big")).unwrap_err();
    assert!(e.contains("too big") && e.contains(".gitignore"), "{e}");
    assert!(m.list().is_empty());
    assert!(!root.join(".iyakki/tasks").read_dir().is_ok_and(|mut d| d.next().is_some()), "no half-made folder");
}

#[test]
fn the_cap_holds_extra_tasks_in_a_queue_and_the_hard_cap_cannot_be_raised() {
    let root = repo("cap");
    let (cur, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let release = Arc::new(AtomicBool::new(false));
    let (c2, m2, r2) = (cur.clone(), max.clone(), release.clone());
    let s: Script = Arc::new(move |_, etx, _, _| {
        let n = c2.fetch_add(1, Ordering::SeqCst) + 1;
        m2.fetch_max(n, Ordering::SeqCst);
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        c2.fetch_sub(1, Ordering::SeqCst);
        finish_with(&etx, "ok");
    });
    let m = manager(&root, s);
    m.set_concurrency(2);
    let ids: Vec<String> = (0..5).map(|i| m.spawn(nt(&format!("job {i}"))).unwrap()).collect();
    until("two to run", || m.counts().0 == 2);
    assert_eq!(m.counts(), (2, 0, 3), "two running, three queued");
    assert_eq!(status(&m, &ids[4]), Status::Queued);
    release.store(true, Ordering::SeqCst);
    for id in &ids {
        done(&m, id);
    }
    assert!(max.load(Ordering::SeqCst) <= 2, "never more than the cap ran at once");
    assert!(ids.iter().all(|i| status(&m, i) == Status::Done));
    m.set_concurrency(99);
    assert_eq!(m.concurrency(), HARD_CAP);
    // the choice is remembered for the workspace
    m.set_concurrency(4);
    assert_eq!(Manager::open(&root, &root.join("FEATURES.md")).concurrency(), 4);
    m.set_concurrency(0);
    assert_eq!(m.concurrency(), 1);
}

#[test]
fn a_per_agent_limit_keeps_a_busy_agent_from_taking_every_place() {
    let root = repo("agentcap");
    let release = Arc::new(AtomicBool::new(false));
    let r2 = release.clone();
    let m = manager(&root, Arc::new(move |_, etx, _, _| {
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        finish_with(&etx, "ok");
    }));
    m.set_concurrency(3);
    m.set_agent_limit("claude-code", 1);
    let a = m.spawn(NewTask { agent: "claude-code".into(), ..nt("c1") }).unwrap();
    let b = m.spawn(NewTask { agent: "claude-code".into(), ..nt("c2") }).unwrap();
    let c = m.spawn(nt("builtin one")).unwrap();
    until("a and c to run", || status(&m, &a) == Status::Running && status(&m, &c) == Status::Running);
    assert_eq!(status(&m, &b), Status::Queued, "the second claude-code task waits though a place is free");
    release.store(true, Ordering::SeqCst);
    done(&m, &b);
}

#[test]
fn two_tasks_run_concurrently_each_with_its_own_session_tree_board_and_port() {
    let root = repo("two");
    let seen = Arc::new(Mutex::new(Vec::<(String, PathBuf, PathBuf, String)>::new()));
    let barrier = Arc::new(AtomicUsize::new(0));
    let (s2, b2) = (seen.clone(), barrier.clone());
    let m = manager(&root, Arc::new(move |spec, etx, _, _| {
        s2.lock().unwrap().push((spec.task.session.clone(), spec.tree.clone(), spec.features.clone(), spec.prompt.clone()));
        b2.fetch_add(1, Ordering::SeqCst);
        // both must be inside their run at the same time
        let t = Instant::now();
        while b2.load(Ordering::SeqCst) < 2 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(spec.tree.join(format!("{}.txt", spec.task.id)), "x").unwrap();
        finish_with(&etx, "ok");
    }));
    let a = m.spawn(NewTask { rows: vec!["F-A-01".into()], ..nt("first") }).unwrap();
    let b = m.spawn(NewTask { rows: vec!["F-A-02".into()], ..nt("second") }).unwrap();
    done(&m, &a);
    done(&m, &b);
    assert_eq!((status(&m, &a), status(&m, &b)), (Status::Done, Status::Done));
    let v = seen.lock().unwrap().clone();
    assert_eq!(v.len(), 2);
    assert_ne!(v[0].0, v[1].0, "separate sessions");
    assert_ne!(v[0].1, v[1].1, "separate trees");
    assert_ne!(v[0].2, v[1].2, "separate boards");
    assert!(v.iter().all(|x| x.0.starts_with("task-")));
    let (ta, tb) = (m.get(&a).unwrap(), m.get(&b).unwrap());
    assert_ne!(ta.port, tb.port);
    assert!(ta.tree.join(format!("{a}.txt")).exists() && !ta.tree.join(format!("{b}.txt")).exists(), "each wrote only in its own tree");
    // each task's board holds its own row only
    let board_a = std::fs::read_to_string(root.join(".iyakki/tasks").join(&a).join("FEATURES.md")).unwrap();
    assert!(board_a.contains("F-A-01") && !board_a.contains("F-A-02") && !board_a.contains("F-A-03"), "{board_a}");
    assert!(v.iter().any(|x| x.3.contains("F-A-01")) && v.iter().any(|x| x.3.contains("port 2")), "the prompt names the rows and the port");
}

#[test]
fn a_row_belongs_to_one_unfinished_task_and_rows_follow_the_task_and_come_back_on_discard() {
    let root = repo("rows");
    let release = Arc::new(AtomicBool::new(false));
    let r2 = release.clone();
    let m = manager(&root, Arc::new(move |_, etx, _, _| {
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        finish_with(&etx, "ok");
    }));
    let a = m.spawn(NewTask { rows: vec!["F-A-01".into()], ..nt("owner") }).unwrap();
    let e = m.spawn(NewTask { rows: vec!["F-A-01".into(), "F-A-02".into()], ..nt("thief") }).unwrap_err();
    assert!(e.contains("F-A-01") && e.contains(&a), "{e}");
    assert_eq!(m.owner_of("F-A-01"), Some(a.clone()));
    assert_eq!(m.owner_of("F-A-02"), None, "the refused spawn took nothing");
    assert!(m.spawn(NewTask { rows: vec!["F-NOPE".into()], ..nt("x") }).unwrap_err().contains("no board row"));
    let row = |id: &str| crate::featureboard::BoardFile::load(&root.join("FEATURES.md")).unwrap().board.get(id).unwrap();
    until("the row to be marked", || row("F-A-01").status == "partial");
    assert!(row("F-A-01").notes.contains(&format!("task {a} running")), "{}", row("F-A-01").notes);
    release.store(true, Ordering::SeqCst);
    done(&m, &a);
    until("review", || row("F-A-01").status == "review");
    m.discard(&a).unwrap();
    assert_eq!(row("F-A-01").status, "planned", "the row is back as it was");
    assert_eq!(m.owner_of("F-A-01"), None);
    let b = m.spawn(NewTask { rows: vec!["F-A-01".into()], ..nt("again") }).unwrap();
    done(&m, &b);
}

#[test]
fn a_merged_task_with_verified_rows_marks_them_done_and_unverified_ones_stay_for_review() {
    let root = repo("verified");
    let m = manager(&root, writer("w.txt", "w"));
    let id = m.spawn(NewTask { rows: vec!["F-A-03".into()], ..nt("rows") }).unwrap();
    done(&m, &id);
    // the task's own board has no verified rows, so the row waits for review after the merge
    m.merge(&id).unwrap();
    let f = crate::featureboard::BoardFile::load(&root.join("FEATURES.md")).unwrap().board.get("F-A-03").unwrap();
    assert_eq!(f.status, "review", "not verified in the tree: the person reviews it");
    assert!(f.notes.contains("not verified"), "{}", f.notes);
}

#[test]
fn board_writes_from_many_tasks_at_once_lose_no_update() {
    let root = repo("lock");
    let features = root.join("FEATURES.md");
    let hs: Vec<_> = ["F-A-01", "F-A-02", "F-A-03"]
        .iter()
        .flat_map(|id| (0..4).map(move |k| (*id, k)))
        .map(|(id, k)| {
            let f = features.clone();
            std::thread::spawn(move || {
                with_board(&f, |bf| {
                    let u = crate::featureboard::Update { comment: Some(format!("w{k}")), ..Default::default() };
                    let cur = bf.board.get(id).unwrap().notes;
                    let u2 = crate::featureboard::Update { comment: Some(format!("{cur}{}", u.comment.unwrap())), ..Default::default() };
                    bf.board.update(id, &u2).unwrap();
                })
                .unwrap();
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let b = crate::featureboard::BoardFile::load(&features).unwrap().board;
    for id in ["F-A-01", "F-A-02", "F-A-03"] {
        let n = b.get(id).unwrap().notes;
        assert_eq!(n.matches('w').count(), 4, "{id}: {n}");
    }
}

#[test]
fn a_budget_stops_a_task_and_says_which() {
    let root = repo("budget");
    let m = manager(&root, Arc::new(|_, etx, cancel, _| {
        let _ = etx.send(RunEvent::Usage { tokens: 5_000, cost_cents: 1.0, files: 0, seconds: 1 });
        let t = Instant::now();
        while !cancel.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "cancelled".into() });
    }));
    let id = m.spawn(NewTask { budgets: Some(Budgets { max_tokens: 1_000, ..Default::default() }), ..nt("hungry") }).unwrap();
    done(&m, &id);
    let t = m.get(&id).unwrap();
    assert_eq!(t.status, Status::Failed);
    assert!(t.error.contains("token budget"), "{}", t.error);
    assert_eq!(t.tokens, 5_000);
    // cost budget
    let id2 = m.spawn(NewTask { budgets: Some(Budgets { max_cost_cents: 1, ..Default::default() }), ..nt("pricey") }).unwrap();
    done(&m, &id2);
    assert!(m.get(&id2).unwrap().error.contains("cost budget"));
}

#[test]
fn usage_adds_up_across_steps_when_each_step_starts_its_count_again() {
    let root = repo("usage");
    let m = manager(&root, Arc::new(|_, etx, _, _| {
        for t in [100, 300, 50, 200] {
            let _ = etx.send(RunEvent::Usage { tokens: t, cost_cents: t as f64 / 100.0, files: 0, seconds: 0 });
        }
        finish_with(&etx, "ok");
    }));
    let id = m.spawn(nt("steps")).unwrap();
    done(&m, &id);
    assert_eq!(m.get(&id).unwrap().tokens, 300 + 200);
}

#[test]
fn approvals_of_all_tasks_share_one_list_tagged_by_task_and_an_answer_reaches_only_its_own_run() {
    let root = repo("approvals");
    let got = Arc::new(Mutex::new(Vec::<(String, String, Verdict)>::new()));
    let g2 = got.clone();
    let m = manager(&root, Arc::new(move |spec, etx, _, drx| {
        // both tasks use the same call id on purpose
        let _ = etx.send(RunEvent::Approval { id: "c1".into(), name: "write_file".into(), summary: format!("write for {}", spec.task.title), detail: String::new() });
        let (cid, v) = drx.recv_timeout(Duration::from_secs(10)).unwrap();
        g2.lock().unwrap().push((spec.task.id.clone(), cid.clone(), v));
        let _ = etx.send(RunEvent::Decision { id: cid, name: "write_file".into(), approved: v != Verdict::Deny, by: "user".into() });
        finish_with(&etx, "ok");
    }));
    let a = m.spawn(nt("alpha")).unwrap();
    let b = m.spawn(nt("beta")).unwrap();
    until("both to ask", || m.approvals().len() == 2);
    let list = m.approvals();
    assert!(list.iter().any(|x| x.task == a && x.summary.contains("alpha")) && list.iter().any(|x| x.task == b && x.summary.contains("beta")));
    assert_eq!(m.counts(), (0, 2, 0));
    assert_eq!(status(&m, &a), Status::NeedsYou);
    assert!(m.answer(&a, "nope", Verdict::Approve).is_err(), "an unknown approval is refused");
    m.answer(&a, "c1", Verdict::Approve).unwrap();
    done(&m, &a);
    assert_eq!(status(&m, &b), Status::NeedsYou, "beta is still waiting: alpha's answer was not applied to it");
    assert_eq!(m.approvals().len(), 1);
    assert_eq!(m.approvals()[0].task, b);
    m.answer(&b, "c1", Verdict::Deny).unwrap();
    done(&m, &b);
    let got = got.lock().unwrap().clone();
    assert!(got.contains(&(a.clone(), "c1".into(), Verdict::Approve)) && got.contains(&(b.clone(), "c1".into(), Verdict::Deny)), "{got:?}");
}

#[test]
fn a_task_failing_or_stopped_can_be_retried_and_a_restart_shows_interrupted_with_resume() {
    let root = repo("restart");
    let specs = Arc::new(Mutex::new(Vec::<(bool, String)>::new()));
    let s2 = specs.clone();
    let m = manager(&root, Arc::new(move |spec, etx, cancel, _| {
        s2.lock().unwrap().push((spec.resume, spec.prompt.clone()));
        // run until the window closes (the pause flag arrives as a cancel on a plain handle)
        let t = Instant::now();
        while !cancel.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "closed".into() });
    }));
    let id = m.spawn(nt("long job")).unwrap();
    until("running", || status(&m, &id) == Status::Running);
    m.shutdown();
    assert_eq!(status(&m, &id), Status::Interrupted);
    // a new window: the saved record says interrupted
    let m2 = Manager::open(&root, &root.join("FEATURES.md"));
    assert_eq!(m2.get(&id).unwrap().status, Status::Interrupted);
    assert!(m2.get(&id).unwrap().last.contains("interrupted"));
    // a window that died without a clean shutdown: the file still says running
    let mut t = m2.get(&id).unwrap();
    t.status = Status::Running;
    std::fs::write(root.join(".iyakki/tasks").join(&id).join("task.json"), serde_json::to_string(&t).unwrap()).unwrap();
    let m3 = Manager::open(&root, &root.join("FEATURES.md"));
    assert_eq!(m3.get(&id).unwrap().status, Status::Interrupted, "a crashed window is recovered as interrupted too");
    m3.set_runner(runner(Arc::new(|_, etx, _, _| finish_with(&etx, "ok"))));
    m3.resume(&id, "").unwrap();
    done(&m3, &id);
    assert_eq!(status(&m3, &id), Status::Done);
}

#[test]
fn stop_pause_and_retry_follow_the_status() {
    let root = repo("controls");
    let m = manager(&root, Arc::new(|_, etx, cancel, _| {
        let t = Instant::now();
        while !cancel.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "stopped".into() });
    }));
    let id = m.spawn(nt("controlled")).unwrap();
    until("running", || status(&m, &id) == Status::Running);
    m.pause(&id).unwrap();
    done(&m, &id);
    assert_eq!(status(&m, &id), Status::Paused);
    assert!(m.stop(&id).is_err(), "a paused task is not running");
    m.resume(&id, "also do X").unwrap();
    until("running again", || status(&m, &id) == Status::Running);
    m.stop(&id).unwrap();
    done(&m, &id);
    let t = m.get(&id).unwrap();
    assert_eq!(t.status, Status::Failed);
    assert!(t.error.contains("stopped by you"));
    assert!(m.discard(&id).is_ok(), "a stopped task can be discarded");
}

#[test]
fn queued_tasks_do_not_start_without_a_free_place_and_can_be_stopped_before_they_start() {
    let root = repo("queuedstop");
    let m = Manager::open(&root, &root.join("FEATURES.md"));
    let id = m.spawn(nt("waits")).unwrap();
    assert_eq!(status(&m, &id), Status::Queued, "no runner yet: it waits");
    m.stop(&id).unwrap();
    assert_eq!(status(&m, &id), Status::Failed);
}

#[test]
fn the_namespace_is_reserved_per_task_and_the_cleanup_hook_runs_on_merge_and_discard() {
    let root = repo("ns");
    let torn = Arc::new(Mutex::new(Vec::<String>::new()));
    let t2 = torn.clone();
    let m = manager(&root, writer("n.txt", "n"));
    m.set_hooks(Hooks { cleanup: Some(Arc::new(move |t| t2.lock().unwrap().push(t.namespace.clone()))), ..Default::default() });
    let a = m.spawn(nt("one")).unwrap();
    let b = m.spawn(nt("two")).unwrap();
    done(&m, &a);
    done(&m, &b);
    let (ta, tb) = (m.get(&a).unwrap(), m.get(&b).unwrap());
    assert!(ta.namespace.ends_with(&format!("-dev-{a}")) && tb.namespace.ends_with(&format!("-dev-{b}")) && ta.namespace != tb.namespace, "{} {}", ta.namespace, tb.namespace);
    m.discard(&a).unwrap();
    m.merge(&b).unwrap();
    let v = torn.lock().unwrap().clone();
    assert!(v.contains(&ta.namespace) && v.contains(&tb.namespace), "{v:?}");
}

#[test]
fn audit_and_inbox_hooks_report_spawn_start_merge_and_the_end_of_a_task() {
    let root = repo("hooks");
    let (au, no) = (Arc::new(Mutex::new(Vec::<String>::new())), Arc::new(Mutex::new(Vec::<String>::new())));
    let (a2, n2) = (au.clone(), no.clone());
    let m = manager(&root, writer("h.txt", "h"));
    m.set_hooks(Hooks { audit: Some(Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k} {d}")))), notify: Some(Arc::new(move |t, _| n2.lock().unwrap().push(t.to_string()))), cleanup: None, record: None });
    let id = m.spawn(nt("hooked")).unwrap();
    done(&m, &id);
    m.merge(&id).unwrap();
    let a = au.lock().unwrap().join("|");
    assert!(a.contains("task.spawn") && a.contains("task.start") && a.contains("task.merge"), "{a}");
    assert!(no.lock().unwrap().iter().any(|t| t.starts_with("Task finished: hooked")));
}

#[test]
fn a_task_cannot_write_outside_its_tree_with_the_project_tools_rooted_there() {
    let root = repo("jail");
    let m = manager(&root, writer("j.txt", "j"));
    let id = m.spawn(nt("jailed")).unwrap();
    done(&m, &id);
    let t = m.get(&id).unwrap();
    struct Nop;
    impl Tools for Nop {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Ok("ok".into())
        }
        fn describe(&self, n: &str, _: &serde_json::Value) -> String {
            n.into()
        }
    }
    let pt = crate::boardtools::ProjectTools::new(Arc::new(Nop), &t.tree, "task", &root.join(".iyakki/tasks").join(&id).join("FEATURES.md"));
    assert!(pt.guard("write_file", &json!({"path": "src/ok.rs"})).is_ok());
    assert!(pt.guard("write_file", &json!({"path": root.join("a.txt").display().to_string()})).is_err(), "the main workspace is outside the task");
    assert!(pt.guard("write_file", &json!({"path": "../../../a.txt"})).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&root, t.tree.join("sneak")).unwrap();
        assert!(pt.guard("write_file", &json!({"path": "sneak/a.txt"})).is_err(), "a link out of the tree is refused");
    }
}

#[test]
fn run_tests_runs_in_the_tree_and_keeps_the_result_on_the_task() {
    let root = repo("tests");
    let m = manager(&root, writer("t.txt", "t"));
    let id = m.spawn(nt("tested")).unwrap();
    done(&m, &id);
    let r = m.run_tests(&id, "sh", &["-c".into(), "test -f t.txt && echo found-it".into()]).unwrap();
    assert!(r.contains("tests passed") && r.contains("found-it"), "{r}");
    assert!(m.get(&id).unwrap().tests.contains("tests passed"));
    let r = m.run_tests(&id, "sh", &["-c".into(), "test -f a-file-that-is-not-there".into()]).unwrap();
    assert!(r.contains("tests failed"));
}

#[test]
fn keep_leaves_the_branch_and_removes_the_tree() {
    let root = repo("keep");
    let m = manager(&root, writer("k.txt", "k"));
    let id = m.spawn(nt("kept")).unwrap();
    done(&m, &id);
    let t = m.get(&id).unwrap();
    m.keep(&id).unwrap();
    assert!(!t.tree.exists());
    assert!(g(&root, &["branch", "--list", "iyakki/*"]).contains(&t.branch));
    assert!(m.get(&id).unwrap().kept);
    assert_eq!(g(&root, &["show", &format!("{}:k.txt", t.branch)]), "k", "the work is on the branch");
}

#[test]
fn caps_on_open_tasks_rows_and_per_call_spawns_hold() {
    let root = repo("caps");
    let m = Manager::open(&root, &root.join("FEATURES.md"));
    let many: Vec<String> = (0..13).map(|i| format!("F-X-{i:02}")).collect();
    assert!(m.spawn(NewTask { rows: many, ..nt("x") }).unwrap_err().contains("at most 12"));
    for i in 0..MAX_OPEN {
        m.spawn(nt(&format!("t{i}"))).unwrap();
    }
    let e = m.spawn(nt("one too many")).unwrap_err();
    assert!(e.contains("unfinished tasks"), "{e}");
    assert_eq!(m.open_count(), MAX_OPEN);
}

#[test]
fn ids_and_branches_never_carry_model_text_into_git_or_paths() {
    let root = repo("slug");
    let m = Manager::open(&root, &root.join("FEATURES.md"));
    let id = m.spawn(nt("--upload-pack=evil; rm -rf / ../../x $(whoami)")).unwrap();
    let t = m.get(&id).unwrap();
    assert!(t.branch.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-')), "{}", t.branch);
    assert!(!t.branch.contains(".."));
    assert!(t.tree.starts_with(root.join(".iyakki/tasks")));
}

#[test]
fn task_spawn_asks_caps_the_count_and_the_lead_has_no_merge_tool() {
    let root = repo("tool");
    let m = Manager::open(&root, &root.join("FEATURES.md"));
    struct Nop;
    impl Tools for Nop {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _: &serde_json::Value) -> Result<String, String> {
            Ok(format!("inner {n}"))
        }
        fn describe(&self, n: &str, _: &serde_json::Value) -> String {
            n.into()
        }
    }
    let tt = TaskTools::new(Arc::new(Nop), m.clone(), "claude-code", "sonnet");
    let defs = tt.defs();
    let spawn = defs.iter().find(|d| d.spec.name == "task_spawn").unwrap();
    assert_eq!(spawn.risk, crate::agent::Risk::Ask);
    assert_eq!(defs.iter().find(|d| d.spec.name == "task_status").unwrap().risk, crate::agent::Risk::Safe);
    assert!(defs.iter().all(|d| !d.spec.name.contains("merge") && !d.spec.name.contains("discard")));
    assert!(!tt.approved_always("task_spawn"), "an always-allow never covers a spawn");
    let three = json!({"tasks": [{"title": "a", "goal": "do a"}, {"title": "b", "goal": "do b", "agent": "copilot"}, {"title": "c", "goal": "do c", "rows": ["F-A-01"]}]});
    let l = tt.requested(&three).unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!((l[0].agent.as_str(), l[0].model.as_str(), l[1].agent.as_str()), ("claude-code", "sonnet", "copilot"));
    assert!(tt.describe("task_spawn", &three).contains("3 parallel"));
    assert!(tt.preview("task_spawn", &three).unwrap().contains("rows F-A-01"));
    let seven = json!({"tasks": (0..7).map(|i| json!({"title": format!("t{i}"), "goal": "g"})).collect::<Vec<_>>()});
    assert!(tt.refusal("task_spawn", &seven).unwrap().contains("at most 6"));
    assert!(tt.refusal("task_spawn", &json!({"title": ""})).is_some());
    let out = tt.call("task_spawn", &three).unwrap();
    assert_eq!(out.matches("t").count() > 0, true);
    assert_eq!(m.list().len(), 3, "{out}");
    assert!(tt.call("task_status", &json!({})).unwrap().contains("queued"));
    assert_eq!(tt.call("other", &json!({})).unwrap(), "inner other");
    // an agent id cannot smuggle characters
    let l = tt.requested(&json!({"title": "x", "goal": "y", "agent": "a b;rm -rf"})).unwrap();
    assert_eq!(l[0].agent, "abrm-rf");
}

// ------------------------------------------------------------------------------------------------ the real Autopilot in a task

mod autopilot_in_tasks {
    use super::*;
    use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};

    struct Script(Mutex<std::collections::VecDeque<ModelTurn>>, Mutex<Vec<String>>);

    impl ChatBackend for Script {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, h: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
            self.1.lock().unwrap().push(h.iter().rev().find(|m| m.role == crate::chat::Role::User).map(|m| m.content.clone()).unwrap_or_default());
            self.0.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
        }
    }

    fn turn(text: &str, calls: Vec<(&str, &str, serde_json::Value)>) -> ModelTurn {
        ModelTurn { text: text.into(), calls: calls.into_iter().map(|(i, n, a)| ToolCall { id: i.into(), name: n.into(), args: a }).collect() }
    }

    /// The person's tools of a task: `write_file` writes under the tree it was made for.
    struct Files(PathBuf);

    impl Tools for Files {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![crate::agent::ToolDef { spec: ToolSpec { name: "write_file".into(), description: "write".into(), schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path"]}) }, risk: crate::agent::Risk::Ask, untrusted: false }]
        }
        fn call(&self, _: &str, a: &serde_json::Value) -> Result<String, String> {
            let p = self.0.join(a["path"].as_str().unwrap());
            std::fs::write(&p, a["content"].as_str().unwrap_or("hello\n")).map_err(|e| e.to_string())?;
            Ok(format!("wrote {}", p.display()))
        }
        fn describe(&self, n: &str, a: &serde_json::Value) -> String {
            format!("{n} {a}")
        }
    }

    fn parts(b: Arc<Script>) -> RunnerParts {
        RunnerParts {
            backend: b,
            tools: Arc::new(|spec| Arc::new(Files(spec.tree.clone()))),
            limits: crate::agent::RunLimits { max_steps: 20, max_tool_calls: 100, ..Default::default() },
            assist: crate::assistrun::Assist::standard(crate::toolblock::ReminderCtx { agent_on: true, browser_policy: "off".into(), engine_installed: false, screen_available: false, loaded: None }),
            executor: None,
            history: None,
            audit: None,
        }
    }

    #[test]
    fn a_task_runs_the_real_autopilot_in_its_tree_with_its_own_board_and_state_and_the_main_board_is_untouched() {
        let root = repo("ap");
        let script = Arc::new(Script(
            Mutex::new(
                vec![
                    turn("", vec![("a1", "board_add", json!({"prefix": "APP", "title": "Write note.txt with a greeting"}))]),
                    turn("Planned one step.", vec![]),
                    turn("", vec![("w1", "write_file", json!({"path": "note.txt", "content": "hi from the task\n"}))]),
                    turn("Wrote note.txt.", vec![]),
                    turn("Verified: note.txt exists. Done: 1 row. Blocked: none. You: nothing.", vec![]),
                ]
                .into(),
            ),
            Mutex::new(vec![]),
        ));
        let m = Manager::open(&root, &root.join("FEATURES.md"));
        m.set_runner(autopilot_runner(parts(script.clone())));
        let main_board = std::fs::read_to_string(root.join("FEATURES.md")).unwrap();
        let id = m.spawn(NewTask { title: "Greeting".into(), goal: "write a greeting file".into(), ..Default::default() }).unwrap();
        // the tool call is approved automatically by Autopilot (hard_ask calls would show up in approvals())
        done(&m, &id);
        let t = m.get(&id).unwrap();
        assert_eq!(t.status, Status::Done, "{} / {:?}", t.error, m.log(&id));
        assert!(t.summary.contains("Autopilot finished"), "{}", t.summary);
        assert_eq!(std::fs::read_to_string(t.tree.join("note.txt")).unwrap(), "hi from the task\n");
        assert!(!root.join("note.txt").exists(), "the main workspace has no new file");
        assert_eq!(std::fs::read_to_string(root.join("FEATURES.md")).unwrap(), main_board, "the main board is not touched by a task without rows");
        let dir = root.join(".iyakki/tasks").join(&id);
        assert!(dir.join("autopilot.json").exists() && dir.join("FEATURES.md").exists(), "own state and board");
        assert!(std::fs::read_to_string(dir.join("FEATURES.md")).unwrap().contains("Write note.txt"));
        assert!(!t.tree.join(".iyakki/autopilot.json").exists(), "the autopilot state is outside the tree");
        assert_eq!((t.progress_done, t.progress_total), (1, 1), "progress from the task's own board");
        assert!(t.verified);
        assert_eq!(m.files(&id).unwrap(), vec![('A', "note.txt".to_string())]);
        assert!(script.1.lock().unwrap()[0].contains("TASK: write a greeting file") && script.1.lock().unwrap()[0].contains("isolated copy"));
        m.merge(&id).unwrap();
        assert!(root.join("note.txt").exists());
    }

    #[test]
    fn a_tasks_file_tools_refuse_paths_outside_its_tree_even_when_the_model_asks() {
        let root = repo("ap-jail");
        let outside = root.join("a.txt");
        let script = Arc::new(Script(
            Mutex::new(
                vec![
                    turn("", vec![("a1", "board_add", json!({"prefix": "APP", "title": "Write a.txt to the main folder"}))]),
                    turn("Planned.", vec![]),
                    turn("", vec![("w1", "write_file", json!({"path": outside.display().to_string(), "content": "PWNED"}))]),
                    turn("It was refused.", vec![]),
                    turn("Summary: refused.", vec![]),
                    turn("Summary: refused.", vec![]),
                    turn("Summary: refused.", vec![]),
                    turn("Summary: refused.", vec![]),
                ]
                .into(),
            ),
            Mutex::new(vec![]),
        ));
        let m = Manager::open(&root, &root.join("FEATURES.md"));
        m.set_runner(autopilot_runner(parts(script)));
        let id = m.spawn(NewTask { title: "Escape".into(), goal: "write outside".into(), ..Default::default() }).unwrap();
        done(&m, &id);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "one\ntwo\nthree\n", "the main file is unchanged");
        assert!(m.log(&id).iter().any(|l| l.contains("outside the workspace") || l.contains("refused")), "{:?}", m.log(&id));
    }
}

// ------------------------------------------------------------------------------------------------ dependencies and agents

/// A repository whose board has F-A-02 waiting for F-A-01 and F-A-03 free.
fn dep_repo(tag: &str) -> PathBuf {
    let d = repo(tag);
    let board = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 3 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Started | Finished | Depends | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | Make the first thing | planned |  |  |  |  |  |\n| F-A-02 | Make the second thing | planned |  |  |  | F-A-01 |  |\n| F-A-03 | Make the third thing | planned |  |  |  |  |  |\n";
    std::fs::write(d.join("FEATURES.md"), board).unwrap();
    d
}

fn mark_done(root: &Path, id: &str) {
    with_board(&root.join("FEATURES.md"), |bf| {
        bf.board.update(id, &crate::featureboard::Update { status: Some("done".into()), tests: Some(vec!["a::b".into()]), ..Default::default() }).unwrap();
    })
    .unwrap();
}

#[test]
fn the_scheduler_starts_only_ready_rows_and_a_waiting_task_starts_when_its_dependency_is_done() {
    let root = dep_repo("deps");
    let started = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = started.clone();
    let m = manager(&root, Arc::new(move |spec, etx, _, _| {
        s2.lock().unwrap().push(spec.task.rows.join(","));
        finish_with(&etx, "ok");
    }));
    // the second row is spawned first: it waits, the free row goes ahead of it
    let second = m.spawn(NewTask { rows: vec!["F-A-02".into()], ..nt("second") }).unwrap();
    let third = m.spawn(NewTask { rows: vec!["F-A-03".into()], ..nt("third") }).unwrap();
    done(&m, &third);
    assert_eq!(status(&m, &second), Status::Queued, "its dependency is not done");
    assert_eq!(m.waiting_on(&second), ["F-A-01"]);
    assert!(m.get(&second).unwrap().last.contains("waiting for F-A-01"), "{}", m.get(&second).unwrap().last);
    assert_eq!(*started.lock().unwrap(), ["F-A-03"]);
    // the first row is done (by a merge or by hand): the waiting task goes
    mark_done(&root, "F-A-01");
    m.pump();
    done(&m, &second);
    assert_eq!(status(&m, &second), Status::Done);
    assert!(m.waiting_on(&second).is_empty());
    assert_eq!(*started.lock().unwrap(), ["F-A-03", "F-A-02"]);
}

#[test]
fn only_the_person_can_override_a_dependency_and_one_row_never_has_two_agents() {
    let root = dep_repo("override");
    let release = Arc::new(AtomicBool::new(false));
    let r2 = release.clone();
    let m = manager(&root, Arc::new(move |_, etx, _, _| {
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        finish_with(&etx, "ok");
    }));
    let forced = m.spawn(NewTask { rows: vec!["F-A-02".into()], override_deps: true, ..nt("forced") }).unwrap();
    until("the forced task to run", || status(&m, &forced) == Status::Running);
    assert!(m.waiting_on(&forced).is_empty());
    let e = m.spawn(NewTask { rows: vec!["F-A-02".into()], ..nt("twin") }).unwrap_err();
    assert!(e.contains("already belongs to task"), "{e}");
    // the assistant's own task_spawn cannot override (the field is not a tool argument)
    assert!(!TaskTools::new(Arc::new(crate::assign::tests_support::NoTools), m.clone(), "builtin", "").defs().iter().any(|d| d.spec.schema.to_string().contains("override_deps")));
    release.store(true, Ordering::SeqCst);
    done(&m, &forced);
}

#[test]
fn rows_that_become_ready_are_offered_or_started_by_the_setting_and_the_setting_is_remembered() {
    let root = dep_repo("auto");
    mark_done(&root, "F-A-01");
    let told = Arc::new(Mutex::new(Vec::<String>::new()));
    let t2 = told.clone();
    let m = manager(&root, Arc::new(|_, etx, _, _| finish_with(&etx, "ok")));
    m.set_hooks(Hooks { notify: Some(Arc::new(move |title, text| t2.lock().unwrap().push(format!("{title}: {text}")))), ..Default::default() });
    assert_eq!(m.auto_assign(), AutoAssign::Ask, "the default asks");
    assert_eq!(m.ready_offers(), ["F-A-02", "F-A-03"]);
    // ask: told, nothing started
    assert_eq!(m.offer_ready(), ["F-A-02", "F-A-03"]);
    assert!(told.lock().unwrap()[0].contains("F-A-02, F-A-03"), "{:?}", told.lock().unwrap());
    assert!(m.list().is_empty());
    // off: nothing at all
    m.set_auto_assign(AutoAssign::Off);
    assert!(m.offer_ready().is_empty() && m.list().is_empty());
    // on: a task per ready row, up to the places that are free
    m.set_auto_assign(AutoAssign::On);
    m.set_concurrency(1);
    let started = m.offer_ready();
    assert_eq!(started, ["F-A-02"], "one place is free");
    assert_eq!(m.owner_of("F-A-02").is_some(), true);
    assert!(!m.ready_offers().contains(&"F-A-02".to_string()), "a row that has an agent is not offered again");
    // a new window remembers both settings
    let again = Manager::open(&root, &root.join("FEATURES.md"));
    assert_eq!((again.auto_assign(), again.concurrency()), (AutoAssign::On, 1));
}

#[test]
fn every_running_task_is_an_agent_with_a_stable_name_state_and_timeline_that_a_restart_keeps() {
    let root = repo("agents");
    let release = Arc::new(AtomicBool::new(false));
    let r2 = release.clone();
    let m = manager(&root, Arc::new(move |_, etx, _, drx| {
        let _ = etx.send(RunEvent::Note { text: "autopilot: step 1 · row 1 of 2 · F-A-01 Make the first thing".into() });
        let _ = etx.send(RunEvent::Approval { id: "c1".into(), name: "write_file".into(), summary: "write x".into(), detail: String::new() });
        let (cid, v) = drx.recv_timeout(Duration::from_secs(10)).unwrap();
        let _ = etx.send(RunEvent::Decision { id: cid, name: "write_file".into(), approved: v != Verdict::Deny, by: "user".into() });
        let _ = etx.send(RunEvent::Usage { tokens: 500, cost_cents: 1.5, files: 0, seconds: 1 });
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        finish_with(&etx, "ok");
    }));
    let a = m.spawn(NewTask { rows: vec!["F-A-01".into()], agent: "claude-code".into(), model: "sonnet".into(), ..nt("first") }).unwrap();
    let b = m.spawn(NewTask { rows: vec!["F-A-02".into()], ..nt("second") }).unwrap();
    until("both to ask", || m.approvals().len() == 2);
    let d = crate::assign::load(&root);
    assert_eq!((d.get(&a).unwrap().name.as_str(), d.get(&b).unwrap().name.as_str()), ("Agent 1", "Agent 2"));
    assert_eq!(d.get(&a).unwrap().title(), "Agent 1 · Claude Code (sonnet)");
    assert_eq!(d.get(&b).unwrap().engine, "built-in");
    assert_eq!(d.get(&a).unwrap().state, crate::assign::AState::WaitingApproval);
    assert_eq!(d.assignee("F-A-02"), Some(("Agent 2", crate::assign::AState::WaitingApproval)));
    m.answer(&a, "c1", Verdict::Approve).unwrap();
    until("agent 1 to work", || crate::assign::load(&root).get(&a).unwrap().state == crate::assign::AState::Working && crate::assign::load(&root).get(&a).unwrap().tokens == 500);
    let d = crate::assign::load(&root);
    assert_eq!(d.get(&b).unwrap().state, crate::assign::AState::WaitingApproval, "agent 2 still waits: the answer went to agent 1 only");
    assert!(d.get(&a).unwrap().steps.iter().any(|s| s.kind == crate::assign::StepKind::Plan && s.text.contains("row 1 of 2")));
    assert_eq!(d.counts(), (1, 1, 0));
    m.answer(&b, "c1", Verdict::Approve).unwrap();
    release.store(true, Ordering::SeqCst);
    done(&m, &a);
    done(&m, &b);
    until("agents to be done", || crate::assign::load(&root).agents.iter().all(|x| x.state == crate::assign::AState::Done));
    // a restart keeps them; a run that was cut off is shown as interrupted, never as working
    let mut t = m.get(&a).unwrap();
    t.status = Status::Running;
    std::fs::write(root.join(".iyakki/tasks").join(&a).join("task.json"), serde_json::to_string(&t).unwrap()).unwrap();
    crate::assign::update(&root, |d| d.set_state(&a, crate::assign::AState::Working, "", 1));
    let m2 = Manager::open(&root, &root.join("FEATURES.md"));
    let d = crate::assign::load(&root);
    assert_eq!(d.get(&a).unwrap().state, crate::assign::AState::Interrupted);
    assert_eq!(d.get(&a).unwrap().name, "Agent 1");
    assert_eq!(m2.get(&a).unwrap().status, Status::Interrupted);
    m2.set_runner(runner(Arc::new(|_, etx, _, _| finish_with(&etx, "ok"))));
    m2.resume(&a, "").unwrap();
    done(&m2, &a);
    assert_eq!(crate::assign::load(&root).get(&a).unwrap().name, "Agent 1", "the same task keeps its name");
}

#[test]
fn guidance_to_a_running_agent_pauses_it_at_the_end_of_its_step_and_continues_with_the_message() {
    let root = repo("guide");
    let seen = Arc::new(Mutex::new(Vec::<(bool, Option<String>)>::new()));
    let s2 = seen.clone();
    let m = manager(&root, Arc::new(move |spec, etx, cancel, _| {
        s2.lock().unwrap().push((spec.resume, spec.interjection.clone()));
        if spec.interjection.is_some() {
            return finish_with(&etx, "ok");
        }
        let t = Instant::now();
        while !cancel.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "paused".into() });
    }));
    let id = m.spawn(nt("guided")).unwrap();
    until("running", || status(&m, &id) == Status::Running);
    assert!(m.guide(&id, "  ").is_err());
    m.guide(&id, "use the config file, not env vars").unwrap();
    until("the second run", || seen.lock().unwrap().len() == 2);
    done(&m, &id);
    assert_eq!(seen.lock().unwrap()[1].1.as_deref(), Some("use the config file, not env vars"));
    let d = crate::assign::load(&root);
    assert!(d.get(&id).unwrap().steps.iter().any(|s| s.kind == crate::assign::StepKind::Message && s.text.contains("use the config file")));
}

#[test]
fn two_running_tasks_that_change_the_same_file_are_flagged_with_the_size_of_each_change() {
    let root = repo("locks");
    let release = Arc::new(AtomicBool::new(false));
    let r2 = release.clone();
    let m = manager(&root, Arc::new(move |spec, etx, _, _| {
        // both change a.txt; each also writes a file of its own
        std::fs::write(spec.tree.join("a.txt"), format!("one\n{} was here\nthree\nfour\n", spec.task.title)).unwrap();
        std::fs::write(spec.tree.join(format!("{}.txt", spec.task.title)), "mine\n").unwrap();
        let _ = etx.send(RunEvent::Note { text: "autopilot: step 1".into() });
        while !r2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        finish_with(&etx, "ok");
    }));
    let a = m.spawn(nt("alpha")).unwrap();
    until("agent 1 to hold a.txt", || crate::assign::load(&root).get(&a).is_some_and(|x| x.files.iter().any(|f| f.path == "a.txt")));
    let b = m.spawn(nt("beta")).unwrap();
    until("a conflict", || !crate::assign::load(&root).live_conflicts().is_empty());
    let d = crate::assign::load(&root);
    let c = d.live_conflicts()[0].clone();
    assert_eq!((c.path.as_str(), c.holder_name.as_str(), c.other_name.as_str(), c.choice), ("a.txt", "Agent 1", "Agent 2", crate::assign::Choice::Pending));
    let fa = d.get(&a).unwrap().files.iter().find(|f| f.path == "a.txt").unwrap().clone();
    assert!(fa.adds >= 1 && fa.dels <= 1, "{fa:?}");
    assert!(d.get(&b).unwrap().files.iter().any(|f| f.path == "beta.txt"), "{:?}", d.get(&b).unwrap().files);
    assert!(m.stats(&a).iter().any(|(p, ad, _)| p == "a.txt" && *ad >= 1));
    release.store(true, Ordering::SeqCst);
    done(&m, &a);
    done(&m, &b);
    until("the conflict to clear when both are done", || crate::assign::load(&root).live_conflicts().is_empty());
}

#[test]
fn every_task_run_is_recorded_with_its_task_agent_and_row_and_the_mapping_is_kept_in_the_registry() {
    let root = repo("runlog");
    let got = Arc::new(Mutex::new(Vec::<RunRecord>::new()));
    let g2 = got.clone();
    let m = manager(&root, Arc::new(|_, etx, _, _| {
        let _ = etx.send(RunEvent::Note { text: "step".into() });
        finish_with(&etx, "ok");
    }));
    m.set_hooks(Hooks { record: Some(Arc::new(move |r| g2.lock().unwrap().push(r))), ..Default::default() });
    let id = m.spawn(NewTask { rows: vec!["F-A-01".into()], ..nt("recorded") }).unwrap();
    done(&m, &id);
    let all = got.lock().unwrap().clone();
    assert_eq!(all.first().unwrap().status, "running", "recorded at the start, so the run can be opened while it works");
    let last = all.last().unwrap();
    assert_eq!((last.status.as_str(), last.agent.as_str(), last.task.id.as_str(), last.task.rows.clone()), ("completed", "Agent 1", id.as_str(), vec!["F-A-01".to_string()]));
    assert!(last.events.iter().any(|e| matches!(e, RunEvent::Final { .. })) && !last.run_id.is_empty());
    assert!(all.iter().all(|r| r.run_id == last.run_id));
    assert_eq!(crate::assign::load(&root).get(&id).unwrap().run_id, last.run_id);
    assert_eq!(m.get(&id).unwrap().run_id, last.run_id);
    // a run started again is a new run
    m.resume(&id, "").unwrap();
    done(&m, &id);
    assert_ne!(m.get(&id).unwrap().run_id, last.run_id);
}

#[test]
fn run_logs_carry_the_task_and_old_logs_without_it_still_load() {
    let d = tmp("runmeta");
    let store = crate::runs::RunStore::new(&d);
    let mut log = crate::runs::RunLog { id: "aabbccddeeff".into(), user: "amy".into(), started: 5, prompt: "p".into(), status: "completed".into(), events: vec![], session: None, task_id: Some("t1".into()), row: Some("F-A-01".into()), agent: Some("Agent 2".into()) };
    store.save(&log).unwrap();
    let m = store.list("amy");
    assert_eq!(m[0].task_label(), "Task t1 · Agent 2 · F-A-01");
    // an old file has no such fields
    log.id = "112233445566".into();
    let mut v = serde_json::to_value(&log).unwrap();
    for k in ["task_id", "row", "agent"] {
        v.as_object_mut().unwrap().remove(k);
    }
    std::fs::write(d.join("amy").join("112233445566.json"), serde_json::to_vec(&v).unwrap()).unwrap();
    let old = store.list("amy").into_iter().find(|x| x.id == "112233445566").unwrap();
    assert_eq!((old.task_id.clone(), old.task_label().as_str()), (None, ""));
}

#[test]
fn each_task_has_its_own_live_approval_mode_and_leaving_autopilot_pauses_only_that_task_after_its_step() {
    use crate::livemode::Mode;
    let root = repo("livemode");
    let au = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = au.clone();
    // a fake run that works like the Autopilot loop at its step boundaries: it goes on while the mode is Autopilot
    let m = manager(&root, Arc::new(|spec, etx, cancel, _| {
        let live = spec.live.clone().expect("every task run gets a live mode");
        let t = Instant::now();
        while live.autopilot() && !cancel.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut st = crate::autopilot::State::new(&spec.prompt, 0);
        st.status = "paused".into();
        st.reason = "Autopilot switched off by the person".into();
        st.save(&spec.state_path);
        let _ = etx.send(RunEvent::Final { text: format!("Autopilot switched off ({})", live.get().label()) });
    }));
    m.set_hooks(Hooks { audit: Some(Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k} {d}")))), ..Default::default() });
    let a = m.spawn(nt("alpha")).unwrap();
    let b = m.spawn(nt("beta")).unwrap();
    until("both running", || status(&m, &a) == Status::Running && status(&m, &b) == Status::Running);
    assert_eq!((m.mode(&a), m.mode(&b)), (Some(Mode::Autopilot), Some(Mode::Autopilot)));
    // the chip of one task: Autopilot -> Ask first; the other task is untouched
    assert_eq!(m.cycle_mode(&a, "alice").unwrap(), Mode::Ask);
    done(&m, &a);
    assert_eq!(status(&m, &a), Status::Paused, "it paused after its step and keeps its place");
    assert_eq!(m.mode(&a), None, "a paused task has no live mode");
    assert_eq!((status(&m, &b), m.mode(&b)), (Status::Running, Some(Mode::Autopilot)));
    let run = m.get(&a).unwrap().run_id;
    assert!(au.lock().unwrap().iter().any(|l| l == &format!("task.mode {a}: approvals mode changed to Ask first during run {run} by alice")), "{:?}", au.lock().unwrap());
    assert!(m.set_mode(&a, Mode::Auto, "alice").is_err(), "only a running task switches");
    // Resume works the plan again: Autopilot
    m.resume(&a, "").unwrap();
    until("running again", || status(&m, &a) == Status::Running);
    assert_eq!(m.mode(&a), Some(Mode::Autopilot));
    m.set_mode(&b, Mode::Auto, "alice").unwrap();
    m.set_mode(&a, Mode::Ask, "alice").unwrap();
    done(&m, &a);
    done(&m, &b);
    assert_eq!((status(&m, &a), status(&m, &b)), (Status::Paused, Status::Paused), "Auto-approve safe is not Autopilot either: beta paused after its step");
    assert!(au.lock().unwrap().iter().any(|l| l.starts_with(&format!("task.mode {b}: approvals mode changed to Auto-approve safe during run "))));
}
