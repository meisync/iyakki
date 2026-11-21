//! Fleet work end to end (F-FLEET-03 to 06, F-ROUTER-08's member choice): three hubs over real loopback sockets, the real `git` binary in temp
//! repositories (bundles made and applied by git), real task managers with a scripted runner in place of a model. What is NOT real here:
//! other computers, a LAN, a model, a person pressing Accept (the hub call is the same one the card makes).

// the boundary scan reads a file's code up to its first #[cfg(test)]: this whole file is test code (real `git` in temp folders)
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::{RunEvent, RunHandle, Verdict};
use crate::fleet::{FixedSource, Heartbeat, HeartbeatSource};
use crate::fleet_hub::{Accept, AssignBody, Hub, InboxItem};
use crate::fleet_integrate::{finalize, integrate};
use crate::fleet_work::{attach, pick_member, Want};
use crate::tasks::{Manager, RunSpec, Runner};

fn tmp(tag: &str) -> PathBuf {
    crate::testdir::outside_project("fleetwork", tag)
}

fn g(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").current_dir(dir).args(["-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

const BOARD: &str = "# Feature board\n\n<!-- summary:start -->done 0 · partial 0 · planned 2 · blocked 0 · deprecated 0 · total 2<!-- summary:end -->\n\n| ID | Feature | Status | Tests | Started | Finished | Notes |\n|---|---|---|---|---|---|---|\n| F-A-01 | Make the first thing | planned |  |  |  |  |\n| F-A-02 | Make the second thing | planned |  |  |  |  |\n";

fn planner_repo(tag: &str) -> PathBuf {
    let d = tmp(tag).join("planner");
    std::fs::create_dir_all(&d).unwrap();
    g(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(d.join("FEATURES.md"), BOARD).unwrap();
    std::fs::write(d.join(".gitignore"), "target\n.iyakki/\n").unwrap();
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "init"]);
    d
}

fn clone_of(p: &Path, name: &str) -> PathBuf {
    let d = p.parent().unwrap().join(name);
    g(p.parent().unwrap(), &["clone", "-q", &p.to_string_lossy(), name]);
    d
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The scripted run: a title starting `conf` rewrites `a.txt` with its own text, any other title writes `<title>.txt`.
fn runner() -> Runner {
    Arc::new(move |spec: RunSpec| {
        let (etx, erx): (Sender<RunEvent>, Receiver<RunEvent>) = channel();
        let (dtx, _drx): (Sender<(String, Verdict)>, _) = channel();
        let h = RunHandle::from_parts(erx, dtx);
        let _cancel: Arc<AtomicBool> = h.cancel_flag();
        std::thread::spawn(move || {
            let t = spec.task.title.clone();
            if t.starts_with("conf") {
                std::fs::write(spec.tree.join("a.txt"), format!("{t}\n")).unwrap();
            } else {
                std::fs::write(spec.tree.join(format!("{t}.txt")), format!("from {t}\n")).unwrap();
            }
            let _ = etx.send(RunEvent::Final { text: format!("did {t}") });
        });
        h
    })
}

fn hb(n: u64) -> Heartbeat {
    Heartbeat { disk_free_mb: 1000 * n, disk_total_mb: 9000, cpu_load_pct: 10 * n as u32, cpus: 4, mem_used_pct: 20 + n as u32, mem_total_mb: 8000, tasks_running: n as u32, models: vec![format!("model-{n}")], battery_pct: None }
}

fn up(h: &Arc<Hub>, n: u64) -> String {
    let src: Arc<dyn HeartbeatSource> = Arc::new(FixedSource(hb(n)));
    h.start_node("127.0.0.1:0", src).unwrap().to_string()
}

fn pair(a: &Arc<Hub>, b: &Arc<Hub>, a_addr: &str, name: &str) -> String {
    let fid = if a.status().fleets.is_empty() { a.create_fleet("lab", "alpha").unwrap() } else { a.status().fleets[0].id.clone() };
    let (code, _) = a.offer_code(&fid).unwrap();
    let t = b.join_start(a_addr, &code, name).unwrap();
    b.poll_pending();
    let req = a.status().requests;
    a.approve_join(&req[0].id).unwrap();
    b.poll_pending();
    b.join_accept(&t).unwrap();
    a.settle();
    fid
}

struct Site {
    hub: Arc<Hub>,
    root: PathBuf,
    mgr: Option<Arc<Manager>>,
}

fn hub(tag: &str) -> Arc<Hub> {
    Arc::new(Hub::load(&tmp(tag)).unwrap())
}

/// A planner (alpha) and two workers (bravo, charlie) in one fleet. The workers are clones of the planner's repository, each with a task
/// manager attached as the IDE would; their test command is `test -f a.txt`.
fn fleet3(tag: &str) -> (Site, Site, Site, String) {
    let proot = planner_repo(tag);
    let (a, b, c) = (hub(&format!("{tag}a")), hub(&format!("{tag}b")), hub(&format!("{tag}c")));
    let a_addr = up(&a, 1);
    up(&b, 2);
    up(&c, 3);
    let fid = pair(&a, &b, &a_addr, "bravo");
    pair(&a, &c, &a_addr, "charlie");
    for _ in 0..2 {
        a.heartbeat_tick(&FixedSource(hb(1)));
        b.heartbeat_tick(&FixedSource(hb(2)));
        c.heartbeat_tick(&FixedSource(hb(3)));
    }
    let worker = |hub: &Arc<Hub>, name: &str| {
        let root = clone_of(&proot, name);
        let mgr = Arc::new(Manager::open(&root, &root.join("FEATURES.md")));
        mgr.set_runner(runner());
        attach(hub, mgr.clone(), &root, "test -f a.txt", &root.join(".fleetctx"));
        Site { hub: hub.clone(), root, mgr: Some(mgr) }
    };
    let (sb, sc) = (worker(&b, "w-bravo"), worker(&c, "w-charlie"));
    (Site { hub: a, root: proot, mgr: None }, sb, sc, fid)
}

fn body(n: u8, title: &str, after: &[&AssignBody], base: &str, rows: &[&str]) -> AssignBody {
    AssignBody { id: format!("{n:02x}").repeat(16), title: title.into(), goal: format!("do {title}"), base_branch: "main".into(), base_commit: base.into(), rows: rows.iter().map(|s| s.to_string()).collect(), context_digest: "dead".into(), budget_tokens: 5000, model_class: String::new(), after: after.iter().map(|b| b.id.clone()).collect() }
}

/// Drive the worker passes until the planner holds the answer for `id`.
fn until_answered(p: &Site, ws: &[&Site], id: &str, want: &str) {
    until(&format!("task {} to be {want}", &id[..8]), || {
        for w in ws {
            if let Some(a) = w.hub.attached() {
                a.worker.tick();
            }
        }
        p.hub.status().outbox.iter().any(|o| o.id == id && o.state == want)
    });
}

#[test]
fn an_accepted_task_runs_as_a_real_task_on_the_worker_and_comes_back_as_a_git_bundle_that_the_planner_integrates_in_order() {
    let (p, w1, w2, fid) = fleet3("e2e");
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    // t2 comes after t1 although it will be handed back first or last: the order is the planner's
    let t1 = body(1, "w1", &[], &base, &["F-A-01"]);
    let t2 = body(2, "w2", &[&t1], &base, &["F-A-02"]);
    // bravo asks its person, charlie trusts the planner
    w2.hub.set_accept(Accept::Members);
    w2.hub.trust(&fid, "alpha", true).unwrap();
    assert_eq!(p.hub.assign(&fid, "bravo", &t1).unwrap(), "waiting");
    assert_eq!(p.hub.assign(&fid, "charlie", &t2).unwrap(), "accepted");
    // awareness: the rows are with members, and waiting shows as waiting
    let rows = p.hub.rows_with_members();
    assert!(rows.iter().any(|r| r.row == "F-A-01" && r.member == "bravo" && r.state == "waiting"), "{rows:?}");
    // nothing runs on bravo until its person accepts
    w1.hub.settle();
    assert!(w1.mgr.as_ref().unwrap().list().is_empty());
    w1.hub.inbox_decide(&t1.id, true).unwrap();
    w1.hub.settle();
    until_answered(&p, &[&w1, &w2], &t1.id, "returned");
    until_answered(&p, &[&w1, &w2], &t2.id, "returned");
    // each worker ran it as its own task, on its own branch
    for (w, t) in [(&w1, &t1), (&w2, &t2)] {
        let item: InboxItem = w.hub.status().inbox.into_iter().find(|i| i.id == t.id).unwrap();
        assert_eq!(item.state, "handed_back");
        let task = w.mgr.as_ref().unwrap().get(&item.task).expect("the local task");
        assert!(task.branch.starts_with("iyakki/task-") && task.base == base, "{task:?}");
    }
    let back = p.hub.returned(&fid);
    assert_eq!(back.len(), 2);
    for r in &back {
        assert!(r.bundle && r.status == "done" && r.head.len() == 40, "{r:?}");
        assert!(r.tests.contains("tests passed"), "the test result the worker saw: {}", r.tests);
        assert!(p.hub.bundle_path(&r.id).unwrap().exists());
    }
    assert_eq!(back.iter().find(|r| r.id == t2.id).unwrap().after, vec![t1.id.clone()]);
    assert!(p.hub.rows_with_members().is_empty(), "nothing is with a member any more");
    // the planner integrates: both bundles, in dependency order, tests after each merge
    let items: Vec<_> = back.iter().map(|r| (r.clone(), p.hub.bundle_path(&r.id).unwrap())).collect();
    let test: (&str, Vec<String>) = ("sh", vec!["-c".into(), "test -f w1.txt && test -f a.txt".into()]);
    let r = integrate(&p.root, "lab", &items, Some((test.0, &test.1))).unwrap();
    // with that test command, t2's merge (which adds w2.txt) still passes; the order is t1 then t2
    assert_eq!(r.applied, vec![t1.id.clone(), t2.id.clone()], "{r:?}");
    assert!(r.followups.is_empty(), "{r:?}");
    assert!(r.tests.contains("tests passed"));
    let tree = p.root.join(".iyakki/fleet-integration/lab");
    assert!(tree.join("w1.txt").exists() && tree.join("w2.txt").exists());
    // the person's own folder is untouched until finalize
    assert!(!p.root.join("w1.txt").exists());
    // applying again changes nothing
    let again = integrate(&p.root, "lab", &items, None).unwrap();
    assert!(again.applied.is_empty() && again.skipped.len() == 2, "{again:?}");
    // finalize: refused with uncommitted changes, then merged
    std::fs::write(p.root.join("a.txt"), "dirty\n").unwrap();
    assert!(finalize(&p.root, "lab").unwrap_err().contains("uncommitted"));
    g(&p.root, &["checkout", "-q", "--", "a.txt"]);
    assert!(finalize(&p.root, "lab").unwrap().contains("merged"));
    assert!(p.root.join("w1.txt").exists() && p.root.join("w2.txt").exists());
    assert!(g(&p.root, &["log", "--oneline"]).contains("Integrate task"));
}

#[test]
fn a_conflicting_bundle_and_a_bundle_that_breaks_the_tests_are_undone_and_become_new_tasks() {
    let (p, w1, w2, fid) = fleet3("conf");
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    for w in [&w1, &w2] {
        w.hub.set_accept(Accept::Members);
        w.hub.trust(&fid, "alpha", true).unwrap();
    }
    let c1 = body(3, "conf-one", &[], &base, &[]);
    let c2 = body(4, "conf-two", &[&c1], &base, &[]);
    let (k1, k2) = (body(5, "k1", &[], &base, &[]), body(6, "k2", &[], &base, &[]));
    for (to, t) in [("bravo", &c1), ("charlie", &c2), ("bravo", &k1), ("charlie", &k2)] {
        assert_eq!(p.hub.assign(&fid, to, t).unwrap(), "accepted");
    }
    for t in [&c1, &c2, &k1, &k2] {
        until_answered(&p, &[&w1, &w2], &t.id, "returned");
    }
    let items = |ids: &[&AssignBody]| -> Vec<_> { ids.iter().map(|t| (p.hub.returned(&fid).into_iter().find(|r| r.id == t.id).unwrap(), p.hub.bundle_path(&t.id).unwrap())).collect() };
    // both changed a.txt differently: the second does not apply, the first stays, nothing is half merged
    let r = integrate(&p.root, "conflict", &items(&[&c1, &c2]), None).unwrap();
    assert_eq!(r.applied, vec![c1.id.clone()], "{r:?}");
    assert_eq!(r.followups.len(), 1);
    let f = &r.followups[0];
    assert!(f.reason.contains("conflicts in a.txt"), "{}", f.reason);
    f.next.check().unwrap();
    assert!(f.next.goal.contains("conflict-lab") || f.next.goal.contains("fleet/integration-conflict"), "{}", f.next.goal);
    assert_eq!(f.next.base_commit, r.head, "the follow-up starts from the integration head");
    let tree = p.root.join(".iyakki/fleet-integration/conflict");
    assert_eq!(std::fs::read_to_string(tree.join("a.txt")).unwrap(), "conf-one\n");
    assert!(g(&tree, &["status", "--porcelain"]).trim().is_empty(), "no half-finished merge is left");
    // tests that fail after the second merge: it is undone
    let failing: (&str, Vec<String>) = ("sh", vec!["-c".into(), "! test -f k2.txt".into()]);
    let r = integrate(&p.root, "tests", &items(&[&k1, &k2]), Some((failing.0, &failing.1))).unwrap();
    assert_eq!(r.applied, vec![k1.id.clone()], "{r:?}");
    assert!(r.followups[0].reason.contains("tests failed"), "{}", r.followups[0].reason);
    assert!(!p.root.join(".iyakki/fleet-integration/tests/k2.txt").exists());
    assert!(p.root.join(".iyakki/fleet-integration/tests/k1.txt").exists());
    // a truncated bundle is refused by git, not applied (a changed byte on the way is caught earlier, by the hand-back's checksum)
    let (rec, path) = items(&[&k2]).remove(0);
    let bytes = std::fs::read(&path).unwrap();
    let bad = path.with_extension("bad");
    std::fs::write(&bad, &bytes[..bytes.len() / 2]).unwrap();
    // (in a repository that does not already hold the objects, a clone that shares none: git does not read a pack whose commits it has)
    g(p.root.parent().unwrap(), &["clone", "-q", "--no-local", &p.root.to_string_lossy(), "fresh-planner"]);
    let fresh = p.root.parent().unwrap().join("fresh-planner");
    let r = integrate(&fresh, "tamper", &[(rec, bad)], None).unwrap();
    assert!(r.applied.is_empty() && r.followups.len() == 1, "{r:?}");
}

#[test]
fn a_handback_is_checked_for_who_sent_it_its_checksum_its_order_and_secrets_and_a_second_answer_is_dropped() {
    let (a, b) = (hub("sa"), hub("sb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let t = body(7, "safe", &[], "", &[]);
    let item = |id: &str| InboxItem { id: id.into(), fleet: fid.clone(), from: a.my_id(), from_name: "alpha".into(), body: t.clone(), state: "accepted".into(), at: 0, task: String::new(), detail: String::new(), answer: String::new() };
    // a hand-back for a task the planner never sent
    let e = b.send_handback(&item(&"09".repeat(16)), "failed", "", "", "", "", "x", None).unwrap_err();
    assert!(e.contains("no such task"), "{e}");
    assert_eq!(a.assign(&fid, "bravo", &t).unwrap(), "waiting");
    // a bundle whose bytes do not match its checksum
    let dir = tmp("sdir");
    let file = dir.join("x.bundle");
    std::fs::write(&file, vec![7u8; 70_000]).unwrap(); // (the bundle file itself is not used: the chunks below are sent by hand)
    let chunk = |index: u32, total: u32, data: &[u8], sha: &str| serde_json::json!({"id": t.id, "status": "done", "head": "", "base": "", "branch": "b", "tests": "", "summary": "", "sha256": sha, "size": 70_000, "index": index, "total": total, "data": crate::cloudkeys::hex(data)});
    let bogus = "ab".repeat(32);
    let whole = vec![7u8; 70_000];
    let parts: Vec<&[u8]> = whole.chunks(crate::fleet_hub::CHUNK).collect();
    assert_eq!(parts.len(), 3);
    b.send_member(&fid, "alpha", "handback", &chunk(0, 3, parts[0], &bogus)).unwrap();
    // out of order
    let e = b.send_member(&fid, "alpha", "handback", &chunk(2, 3, parts[2], &bogus)).unwrap_err();
    assert!(e.contains("out of order"), "{e}");
    b.send_member(&fid, "alpha", "handback", &chunk(0, 3, parts[0], &bogus)).unwrap();
    b.send_member(&fid, "alpha", "handback", &chunk(1, 3, parts[1], &bogus)).unwrap();
    let e = b.send_member(&fid, "alpha", "handback", &chunk(2, 3, parts[2], &bogus)).unwrap_err();
    assert!(e.contains("checksum"), "{e}");
    assert!(a.returned(&fid).is_empty() && !a.bundle_path(&t.id).unwrap().exists());
    // a failure message with a secret in its text is kept without the secret
    b.send_handback(&item(&t.id), "failed", "", "", "", "", "the password is hunter2", None).unwrap();
    let r = a.returned(&fid).remove(0);
    assert!(!r.summary.contains("hunter2") && r.summary.contains("withheld"), "{}", r.summary);
    assert!(a.status().outbox[0].state.starts_with("failed"));
    // one answer per task: a second is dropped, not applied
    b.send_handback(&item(&t.id), "done", "", "", "", "", "late", None).unwrap();
    assert_eq!(a.returned(&fid).len(), 1);
    assert_eq!(a.returned(&fid)[0].status, "failed");
}

#[test]
fn a_silent_member_is_shown_as_lost_its_task_is_requeued_to_the_member_that_fits_and_its_late_answer_is_dropped() {
    let (a, b, c) = (hub("la"), hub("lb"), hub("lc"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    up(&c, 3);
    let fid = pair(&a, &b, &a_addr, "bravo");
    pair(&a, &c, &a_addr, "charlie");
    for _ in 0..2 {
        a.heartbeat_tick(&FixedSource(hb(1)));
        b.heartbeat_tick(&FixedSource(hb(2)));
        c.heartbeat_tick(&FixedSource(hb(3)));
    }
    for h in [&b, &c] {
        h.set_accept(Accept::Members);
        h.trust(&fid, "alpha", true).unwrap();
    }
    a.set_interval(Duration::from_secs(1));
    let t = body(8, "lost-work", &[], "", &["F-A-01"]);
    assert_eq!(a.assign(&fid, "bravo", &t).unwrap(), "accepted");
    // bravo goes silent: the planner's clock moves on, charlie still talks
    let offset = Arc::new(AtomicU64::new(0));
    let o2 = offset.clone();
    a.set_clock(Arc::new(move || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + o2.load(Ordering::SeqCst)));
    offset.store(10, Ordering::SeqCst);
    c.heartbeat_tick(&FixedSource(hb(3)));
    let st = a.status();
    let state = |n: &str| st.fleets[0].members.iter().find(|m| m.name == n).unwrap().state.clone();
    assert_eq!((state("bravo").as_str(), state("charlie").as_str()), ("lost", "up"));
    assert!(a.rows_with_members().iter().any(|r| r.row == "F-A-01" && r.state == "lost"));
    let lost = a.requeue_lost();
    assert_eq!(lost.len(), 1);
    assert_eq!(a.status().outbox[0].state, "requeued");
    let m = pick_member(&a.status(), &fid, &Want { not: vec![lost[0].to.clone()], ..Default::default() }).expect("charlie fits");
    assert_eq!(m.name, "charlie");
    // the same task goes out again under a new id, charlie takes it
    let mut again = lost[0].body.clone().unwrap();
    again.id = "99".repeat(16);
    assert_eq!(a.assign(&fid, &m.id, &again).unwrap(), "accepted");
    // bravo wakes up and answers the old task: dropped
    offset.store(0, Ordering::SeqCst);
    let item = InboxItem { id: t.id.clone(), fleet: fid.clone(), from: a.my_id(), from_name: "alpha".into(), body: t.clone(), state: "accepted".into(), at: 0, task: String::new(), detail: String::new(), answer: String::new() };
    b.send_handback(&item, "done", "", "", "", "ok", "late", None).unwrap();
    assert!(a.returned(&fid).is_empty(), "a requeued task has one answer: the new one");
}

fn roomy(mut h: Heartbeat) -> Heartbeat {
    h.disk_free_mb = 5000;
    h
}

fn member(name: &str, id: &str, state: &str, h: Heartbeat) -> crate::fleet_hub::MemberView {
    crate::fleet_hub::MemberView { id: id.into(), fingerprint: id.into(), name: name.into(), admin: false, me: false, trusted: false, state: state.into(), seen_secs_ago: Some(1), address_known: true, transport: "lan".into(), heartbeat: Some(roomy(h)) }
}

#[test]
fn the_member_whose_models_and_load_fit_is_chosen_and_nobody_is_chosen_when_nobody_fits() {
    let mut idle = hb(1);
    idle.models = vec!["qwen2.5:7b".into()];
    let mut busy = hb(2);
    busy.cpu_load_pct = 90;
    busy.models = vec!["qwen2.5:7b".into()];
    let mut seeing = hb(3);
    seeing.models = vec!["llava:13b".into()];
    seeing.cpu_load_pct = 70;
    let mut full = hb(1);
    full.tasks_running = 4;
    full.models = vec!["llava:13b".into()];
    let fleet = crate::fleet_hub::FleetView { id: "f".repeat(64), short: "f".into(), name: "lab".into(), version: 1, generation: 0, i_am_admin: true, members: vec![member("idle", "m1", "up", idle), member("busy", "m2", "up", busy), member("seeing", "m3", "up", seeing), member("full", "m4", "up", full), member("gone", "m5", "lost", hb(1))] };
    let st = crate::fleet_hub::Status { me: "me".into(), my_fingerprint: "x".into(), listening: None, accept: Accept::Ask, fleets: vec![fleet.clone()], offers: vec![], requests: vec![], pending: vec![], inbox: vec![], outbox: vec![], tls: false, relay: String::new() };
    let pick = |w: Want| pick_member(&st, &fleet.id, &w).map(|m| m.name);
    assert_eq!(pick(Want::default()).as_deref(), Some("idle"), "the least loaded that is up");
    assert_eq!(pick(Want { vision: true, ..Default::default() }).as_deref(), Some("seeing"), "a model that sees pictures; the full one has no free place");
    assert_eq!(pick(Want { not: vec!["m1".into()], ..Default::default() }).as_deref(), Some("busy"), "without idle: the next lowest score");
    assert_eq!(pick(Want { audio: true, ..Default::default() }), None, "no member has an audio model");
    assert_eq!(pick(Want { not: vec!["m1".into(), "m2".into(), "m3".into()], ..Default::default() }), None, "a lost member and a full one never");
}

/// A core that allows everything and remembers what it was asked (the real one asks the person).
struct Gate(std::sync::Mutex<Vec<String>>);

impl iyakki_core_api::fleet::Fleet for Gate {
    fn read(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn join(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn manage(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn assign(&self, l: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(l.to_string());
        Ok(())
    }
    fn receive(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn record(&self, _: &str, _: &str) {}
}

struct NoModels;

impl crate::router::ModelPool for NoModels {
    fn backend(&self, _: &str) -> Option<Arc<dyn crate::chat::ChatBackend>> {
        None
    }
}

#[test]
fn a_router_step_goes_to_the_fleet_member_that_fits_runs_there_as_a_task_and_comes_back_as_the_steps_result() {
    use crate::router::{DefaultStrategy, Plan, Router, RouterConfig, RunOutcome, Step};
    use crate::router_registry::{Caps, Cost, Profile, Role, Zone};
    let (p, w1, w2, fid) = fleet3("rt");
    for w in [&w1, &w2] {
        w.hub.set_accept(Accept::Members);
        w.hub.trust(&fid, "alpha", true).unwrap();
    }
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let dispatch = Arc::new(crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), &fid, Duration::from_secs(25)));
    // the planner computer has one local model that could take the step; the fleet is preferred whenever a member fits
    let local = Profile { id: "t/local".into(), provider: "t".into(), model: "local".into(), role: Role::Executor, caps: Caps { tools: true, vision: false, audio: false, context: 4096 }, cost: Cost::Free, zone: Zone::Computer, local: true, enabled: true };
    let audit: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(|_, _| {});
    let cfg = RouterConfig { enabled: true, max_zone: Zone::Network, ..RouterConfig::default() };
    let mut r = Router::new(cfg, &[local.clone()], Arc::new(NoModels), Arc::new(DefaultStrategy), audit.clone()).with_fleet(dispatch.clone());
    let step = |id: &str| Step { id: id.into(), title: format!("work-{id}"), task: "write the file".into(), after: vec![], role: Role::Executor, needs: Default::default(), check: String::new() };
    let plan = Plan { goal: "g".into(), steps: vec![step("s1")] };
    // keep the workers' loads apart so the choice is known: bravo is the lighter one
    let report = {
        let (b, c) = (w1.hub.clone(), w2.hub.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let (pa, wa, wb) = (p.hub.clone(), w1.hub.attached().unwrap(), w2.hub.attached().unwrap());
        // the heartbeat threads run every 20 s: drive the workers and heartbeats by hand while the router waits
        let drive = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                wa.worker.tick();
                wb.worker.tick();
                b.heartbeat_tick(&FixedSource(hb(2)));
                c.heartbeat_tick(&FixedSource(hb(3)));
                pa.heartbeat_tick(&FixedSource(hb(1)));
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let rep = r.run_plan("make the thing", plan, None);
        stop.store(true, Ordering::SeqCst);
        drive.join().unwrap();
        rep
    };
    assert_eq!(report.outcome, RunOutcome::Finished, "{}", report.summary());
    let s = &report.states["s1"];
    assert!(s.model.as_deref().is_some_and(|m| m.starts_with("fleet:bravo")), "the lighter member took it: {:?}", s.model);
    assert!(s.result.as_deref().unwrap_or("").contains("did Step s1"), "{:?}", s.result);
    assert_eq!(gate.0.lock().unwrap().len(), 1, "the core was asked once for the step");
    // it really ran as a task on bravo
    let tasks = w1.mgr.as_ref().unwrap().list();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].title.starts_with("Step s1"));
    assert!(w2.mgr.as_ref().unwrap().list().is_empty());
    // when only this computer is allowed the step is never offered; when nobody fits it stays here
    let cfg = RouterConfig { enabled: true, max_zone: Zone::Computer, ..RouterConfig::default() };
    let mut r = Router::new(cfg, &[local], Arc::new(NoModels), Arc::new(DefaultStrategy), audit).with_fleet(dispatch.clone());
    let rep = r.run_plan("g", Plan { goal: "g".into(), steps: vec![step("s2")] }, None);
    assert_ne!(rep.outcome, RunOutcome::Finished, "no model can be reached here");
    assert_eq!(gate.0.lock().unwrap().len(), 1, "nothing was sent: the person allows only this computer");
}

#[test]
fn the_planners_api_picks_a_member_when_none_is_named_asks_the_core_for_each_change_and_integrates_and_finalizes() {
    use crate::fleettools::FleetApi;
    let (p, w1, w2, fid) = fleet3("api");
    for w in [&w1, &w2] {
        w.hub.set_accept(Accept::Members);
        w.hub.trust(&fid, "alpha", true).unwrap();
    }
    // the planner has a workspace too (a task manager is attached the way the IDE does it)
    let pm = Arc::new(Manager::open(&p.root, &p.root.join("FEATURES.md")));
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let api = FleetApi::new(p.hub.clone(), gate.clone(), Arc::new(FixedSource(hb(1))), Arc::new(|_, _| {}));
    api.attach(pm, &p.root, "test -f a.txt", &p.root.join(".fleetctx"));
    let out = api.assign(&serde_json::json!({"title": "auto", "goal": "do it", "base_commit": g(&p.root, &["rev-parse", "HEAD"]).trim()})).unwrap();
    assert!(out.contains("bravo") && out.contains("accepted"), "the lighter member was chosen: {out}");
    assert!(gate.0.lock().unwrap()[0].contains("bravo"), "the card names the member that was chosen");
    let id = p.hub.status().outbox[0].id.clone();
    until_answered(&p, &[&w1, &w2], &id, "returned");
    let text = api.integrate("", false).unwrap();
    assert!(text.contains("1 applied") && text.contains("Nothing is in your own branch yet"), "{text}");
    assert!(!p.root.join("auto.txt").exists());
    assert!(api.finalize("").unwrap().contains("merged"));
    assert!(p.root.join("auto.txt").exists());
    let status = api.status_text().unwrap();
    assert!(status.contains("Handed back:") && status.contains("tests passed"), "{status}");
    // nothing fits when nobody is named and every member is full
    assert!(api.assign(&serde_json::json!({"title": "x", "goal": "y", "model_class": "audio"})).unwrap_err().contains("no member fits"));
}

#[test]
fn a_worker_with_an_ide_workspace_shares_the_tasks_it_really_runs_not_the_figure_of_its_source() {
    let (p, w1, _w2, _fid) = fleet3("hbtasks");
    // the source says 2 tasks, the workspace has none running: the planner sees 0
    w1.hub.heartbeat_tick(&FixedSource(hb(2)));
    let seen = |p: &Site| {
        let t0 = std::time::Instant::now();
        loop {
            let s = p.hub.status();
            // the setup sent earlier figures (2) before the workspace was attached: wait for the new one
            if let Some(h) = s.fleets[0].members.iter().find(|m| m.name == "bravo").and_then(|m| m.heartbeat.clone()) {
                if h.tasks_running == 0 {
                    return 0;
                }
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(10), "the planner still sees the source's figure");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    assert_eq!(seen(&p), 0);
    assert_eq!(w1.mgr.as_ref().unwrap().counts().0, 0);
}

/// Assigns every ready step to the busiest member the input offers (the built-in choice would take the lightest).
struct BusiestMember(Arc<std::sync::Mutex<Vec<String>>>);

impl crate::router::Strategy for BusiestMember {
    fn name(&self) -> String {
        "busiest".into()
    }
    fn decide(&self, input: &serde_json::Value) -> Result<serde_json::Value, String> {
        let members = input["members"].as_array().cloned().unwrap_or_default();
        let ready = input["ready"].as_array().cloned().unwrap_or_default();
        let model = input["models"][0]["id"].clone();
        let pick = members.iter().max_by_key(|m| m["cpu_load_pct"].as_u64().unwrap_or(0)).map(|m| m["name"].clone());
        if let Some(n) = members.iter().filter_map(|m| m["name"].as_str()).next() {
            self.0.lock().unwrap().push(n.to_string());
        }
        let assign: Vec<serde_json::Value> = ready.iter().map(|s| serde_json::json!({"step": s, "model": model, "member": pick})).collect();
        Ok(serde_json::json!({"v": 1, "assign": assign}))
    }
}

#[test]
fn a_strategy_sees_the_fleet_members_and_can_send_a_step_to_one_it_names() {
    use crate::router::{Plan, Router, RouterConfig, RunOutcome, Step};
    use crate::router_registry::{Caps, Cost, Profile, Role, Zone};
    let (p, w1, w2, fid) = fleet3("rtm");
    for w in [&w1, &w2] {
        w.hub.set_accept(Accept::Members);
        w.hub.trust(&fid, "alpha", true).unwrap();
    }
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let dispatch = Arc::new(crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), &fid, Duration::from_secs(25)));
    let local = Profile { id: "t/local".into(), provider: "t".into(), model: "local".into(), role: Role::Executor, caps: Caps { tools: true, vision: false, audio: false, context: 4096 }, cost: Cost::Free, zone: Zone::Computer, local: true, enabled: true };
    let audit: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(|_, _| {});
    let cfg = RouterConfig { enabled: true, max_zone: Zone::Network, ..RouterConfig::default() };
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let mut r = Router::new(cfg, &[local], Arc::new(NoModels), Arc::new(BusiestMember(seen.clone())), audit).with_fleet(dispatch);
    let plan = Plan { goal: "g".into(), steps: vec![Step { id: "s1".into(), title: "work".into(), task: "write the file".into(), after: vec![], role: Role::Executor, needs: Default::default(), check: String::new() }] };
    let report = {
        let (b, c) = (w1.hub.clone(), w2.hub.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let (pa, wa, wb) = (p.hub.clone(), w1.hub.attached().unwrap(), w2.hub.attached().unwrap());
        let drive = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                wa.worker.tick();
                wb.worker.tick();
                b.heartbeat_tick(&FixedSource(hb(2)));
                c.heartbeat_tick(&FixedSource(hb(3)));
                pa.heartbeat_tick(&FixedSource(hb(1)));
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let rep = r.run_plan("make the thing", plan, None);
        stop.store(true, Ordering::SeqCst);
        drive.join().unwrap();
        rep
    };
    assert_eq!(report.outcome, RunOutcome::Finished, "{}", report.summary());
    assert!(!seen.lock().unwrap().is_empty(), "the strategy was given the members");
    let m = report.states["s1"].model.clone().unwrap_or_default();
    assert!(m.starts_with("fleet:charlie"), "the strategy's choice (the busier member) took it, not the built-in lightest: {m}");
    assert_eq!(w2.mgr.as_ref().unwrap().list().len(), 1);
    assert!(w1.mgr.as_ref().unwrap().list().is_empty());
}

#[test]
fn a_task_with_a_model_class_runs_on_the_model_of_this_computer_that_has_the_ability() {
    use crate::router_registry::{Caps, Cost, Profile, Role, Zone};
    let prof = |id: &str, vision: bool, audio: bool, cost: Cost, local: bool, enabled: bool| Profile { id: format!("p/{id}"), provider: "p".into(), model: id.into(), role: Role::Executor, caps: Caps { tools: true, vision, audio, context: 4096 }, cost, zone: if local { Zone::Computer } else { Zone::Internet }, local, enabled };
    let ps = vec![prof("plain", false, false, Cost::Free, true, true), prof("big-eyes", true, false, Cost::High, false, true), prof("small-eyes", true, false, Cost::Low, true, true), prof("off-eyes", true, false, Cost::Free, true, false), prof("ears", false, true, Cost::Low, true, true)];
    assert_eq!(crate::fleet_work::pick_model(&ps, "vision").as_deref(), Some("small-eyes"), "enabled, on this computer first, then the cheapest");
    assert_eq!(crate::fleet_work::pick_model(&ps, "audio").as_deref(), Some("ears"));
    assert_eq!(crate::fleet_work::pick_model(&ps, ""), None);
    assert_eq!(crate::fleet_work::pick_model(&ps, "telepathy"), None);
    assert_eq!(crate::fleet_work::pick_model(&ps[..1], "vision"), None, "no such model: the default runs it");
    // end to end: a vision task starts as a task whose model is the picked one
    let (p, w1, _w2, fid) = fleet3("mclass");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let psc = ps.clone();
    w1.hub.attached().unwrap().worker.set_models(Arc::new(move || psc.clone()));
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    let mut t = body(1, "see", &[], &base, &["F-A-01"]);
    t.model_class = "vision".into();
    assert_eq!(p.hub.assign(&fid, "bravo", &t).unwrap(), "accepted");
    w1.hub.settle();
    until("the task starts", || !w1.mgr.as_ref().unwrap().list().is_empty());
    assert_eq!(w1.mgr.as_ref().unwrap().list()[0].model, "small-eyes");
}

#[test]
fn a_member_with_little_free_memory_takes_fewer_tasks_than_it_has_processors() {
    let h = |cpus: u32, total: u64, used: u32, running: u32| Heartbeat { disk_free_mb: 9000, disk_total_mb: 9000, cpu_load_pct: 10, cpus, mem_used_pct: used, mem_total_mb: total, tasks_running: running, models: vec!["m".into()], battery_pct: None };
    // 16 processors but 8 GB with 50% used: 4 GB free, 2 GB kept back, one 1.5 GB task fits
    assert_eq!(crate::fleet_work::member_cap(&h(16, 8000, 50, 0)), 1);
    // plenty of memory: the processors are the limit
    assert_eq!(crate::fleet_work::member_cap(&h(4, 64000, 10, 0)), 4);
    // a task already running pays for its own memory
    assert_eq!(crate::fleet_work::member_cap(&h(8, 16000, 50, 2)), 5);
}

#[test]
fn a_received_task_remembers_which_member_sent_it() {
    let (p, w1, _w2, fid) = fleet3("frommember");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(p.hub.assign(&fid, "bravo", &body(1, "hello", &[], &base, &["F-A-01"])).unwrap(), "accepted");
    w1.hub.settle();
    until("the task starts", || !w1.mgr.as_ref().unwrap().list().is_empty());
    assert_eq!(w1.mgr.as_ref().unwrap().list()[0].from_member, "alpha");
}

#[test]
fn a_hand_back_cut_in_half_by_a_restart_is_finished_from_where_it_stopped_and_the_limit_is_above_64_mb() {
    use crate::fleet_hub::{CHUNK, MAX_BUNDLE};
    assert!(MAX_BUNDLE > 64 * 1024 * 1024);
    let (p, w1, _w2, fid) = fleet3("resume");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    let t = body(1, "big", &[], &base, &["F-A-01"]);
    assert_eq!(p.hub.assign(&fid, "bravo", &t).unwrap(), "accepted");
    w1.hub.settle();
    let item = w1.hub.status().inbox.into_iter().find(|i| i.id == t.id).unwrap();
    // a bundle of five whole chunks and a bit
    let bytes: Vec<u8> = (0..(5 * CHUNK + 777)).map(|n| (n % 251) as u8).collect();
    let file = tmp("resume-bundle").join("big.bundle");
    std::fs::write(&file, &bytes).unwrap();
    let sha = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let total = bytes.chunks(CHUNK).count() as u32;
    let hex = |c: &[u8]| c.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // the first three chunks arrive, then the planner restarts (its count in memory is gone)
    for (n, c) in bytes.chunks(CHUNK).take(3).enumerate() {
        let m = serde_json::json!({"id": t.id, "status": "done", "head": "", "base": "", "branch": "", "tests": "", "summary": "s", "sha256": sha, "size": bytes.len(), "index": n, "total": total, "data": hex(c)});
        w1.hub.send_member(&fid, &item.from, "handback", &m).unwrap();
    }
    p.hub.forget_assemblies();
    // sending the whole hand-back again asks how far it got and sends only the rest
    w1.hub.send_handback_with(&item, "done", &"ab".repeat(20), &"cd".repeat(20), "fleet/t", "3 passed, all green", "s", Some(&file), &["src/a.rs".to_string(), "b.txt".to_string()]).unwrap();
    let r = p.hub.returned(&fid).into_iter().find(|r| r.id == t.id).expect("the hand-back arrived");
    assert!(r.bundle && r.size == bytes.len() as u64);
    // the planner sees what the step did without opening the bundle: branch, head, the files and the test lines
    assert_eq!(r.files, vec!["src/a.rs".to_string(), "b.txt".to_string()]);
    let said = crate::fleet_work::step_result_text(&r);
    assert!(said.contains("Made fleet/t (head ababababab") && said.contains("Changed 2 file(s): src/a.rs, b.txt.") && said.contains("[tests: 3 passed, all green]"), "{said}");
    assert_eq!(std::fs::read(p.hub.bundle_path(&t.id).unwrap()).unwrap(), bytes, "the bundle is whole and equal");
}

#[test]
fn the_tasks_of_a_silent_member_go_to_another_member_by_themselves_only_after_the_person_switched_it_on() {
    let (a, b, c) = (hub("aa"), hub("ab"), hub("ac"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    up(&c, 3);
    let fid = pair(&a, &b, &a_addr, "bravo");
    pair(&a, &c, &a_addr, "charlie");
    for _ in 0..2 {
        a.heartbeat_tick(&FixedSource(hb(1)));
        b.heartbeat_tick(&FixedSource(hb(2)));
        c.heartbeat_tick(&FixedSource(hb(3)));
    }
    for h in [&b, &c] {
        h.set_accept(Accept::Members);
        h.trust(&fid, "alpha", true).unwrap();
    }
    a.set_interval(Duration::from_secs(1));
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let audit: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(|_, _| {});
    let api = crate::fleettools::FleetApi::new(a.clone(), gate.clone(), Arc::new(FixedSource(hb(1))), audit);
    let t = body(9, "auto-lost", &[], "", &["F-A-01"]);
    assert_eq!(a.assign(&fid, "bravo", &t).unwrap(), "accepted");
    let offset = Arc::new(AtomicU64::new(0));
    let o2 = offset.clone();
    a.set_clock(Arc::new(move || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + o2.load(Ordering::SeqCst)));
    offset.store(10, Ordering::SeqCst);
    c.heartbeat_tick(&FixedSource(hb(3)));
    // off: nothing moves by itself
    assert!(!a.auto_requeue());
    a.kick_lost();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(a.status().outbox[0].state, "accepted");
    // the person switches it on (the core is asked once) and the next round moves the task
    let asked = gate.0.lock().unwrap().len();
    api.set_auto_requeue(true).unwrap();
    assert_eq!(gate.0.lock().unwrap().len(), asked + 1, "switching it on asked the core");
    assert!(a.auto_requeue());
    a.kick_lost();
    until("the task goes to charlie", || a.status().outbox.iter().any(|o| o.to_name == "charlie") && a.status().outbox.iter().any(|o| o.state == "requeued"));
    assert_eq!(gate.0.lock().unwrap().len(), asked + 1, "the moving itself asked nobody: the standing yes was given");
}

#[test]
fn the_board_member_strip_names_each_member_with_its_figures_and_the_rows_that_are_with_it() {
    let (p, w1, _w2, fid) = fleet3("chips");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(p.hub.assign(&fid, "bravo", &body(1, "chips", &[], &base, &["F-A-01"])).unwrap(), "accepted");
    let chips = crate::fleettools::member_chips(&p.hub);
    assert_eq!(chips.len(), 2, "{chips:?}");
    let bravo = chips.iter().find(|c| c.0.starts_with("bravo")).unwrap();
    assert!(bravo.1 && bravo.0.contains("load") && bravo.0.contains("memory") && bravo.0.contains("F-A-01"), "{bravo:?}");
    let charlie = chips.iter().find(|c| c.0.starts_with("charlie")).unwrap();
    assert!(!charlie.0.contains("F-A-01"));
}

#[test]
fn the_changes_of_a_task_are_scanned_for_credentials_before_they_travel() {
    let root = planner_repo("secretscan");
    g(&root, &["config", "user.email", "t@example.invalid"]);
    g(&root, &["config", "user.name", "t"]);
    std::fs::write(root.join("notes.txt"), "just words\nnothing to see\n").unwrap();
    g(&root, &["add", "."]);
    g(&root, &["commit", "-qm", "clean"]);
    assert_eq!(crate::fleet_work::secret_in_changes(&root, "HEAD~1..HEAD"), None);
    std::fs::write(root.join("deploy.env"), "region=eu\napi_key = sk-abcdefghijklmnopqrstuvwxyz0123456789\n").unwrap();
    g(&root, &["add", "."]);
    g(&root, &["commit", "-qm", "oops"]);
    let found = crate::fleet_work::secret_in_changes(&root, "HEAD~1..HEAD").expect("the key is found");
    assert!(found.contains("deploy.env"), "{found}");
    assert!(!found.contains("sk-abcdef"), "the value is never in the message: {found}");
}

#[test]
fn a_dispatcher_with_no_fleet_named_looks_across_every_fleet_of_the_computer() {
    use crate::router::FleetRunner;
    let (p, _w1, _w2, fid) = fleet3("allfleets");
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let all = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), "", Duration::from_secs(5));
    let one = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), &fid, Duration::from_secs(5));
    let names = |d: &crate::fleet_work::FleetDispatch| {
        let mut n: Vec<String> = d.members().into_iter().map(|m| m.name).collect();
        n.sort();
        n
    };
    assert_eq!(names(&all), vec!["bravo".to_string(), "charlie".to_string()]);
    assert_eq!(names(&all), names(&one));
    let other = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate, "no-such-fleet", Duration::from_secs(5));
    assert!(names(&other).is_empty());
}

#[test]
fn a_members_own_router_answers_the_model_query_and_its_models_are_shown_as_network_profiles() {
    use crate::router::FleetRunner;
    use crate::router_registry::{Caps, Cost, Profile, Role, Zone};
    let (p, w1, _w2, fid) = fleet3("modelq");
    // bravo has no list to share yet, charlie neither: the query is refused with a plain reason
    assert!(p.hub.query_models(&fid, "bravo").is_err());
    let mine = Profile { id: "local/qwen3:8b".into(), provider: "local".into(), model: "qwen3:8b".into(), role: Role::Executor, caps: Caps { tools: true, vision: false, audio: false, context: 8192 }, cost: Cost::Free, zone: Zone::Computer, local: true, enabled: true };
    let m2 = mine.clone();
    w1.hub.attached().unwrap().worker.set_models(Arc::new(move || vec![m2.clone()]));
    let got = p.hub.query_models(&fid, "bravo").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "local/qwen3:8b");
    assert_eq!(got[0].zone, Zone::Network, "a model on another computer is no closer than the network");
    assert_eq!(got[0].cost, Cost::Free, "the member's own rating of cost is kept");
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let d = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate, "", Duration::from_secs(5));
    let all = d.member_models();
    assert_eq!(all.len(), 1, "only the member that shares a list answers");
    assert_eq!(all[0].0, "bravo");
}

#[test]
fn a_member_with_only_a_mailbox_address_answers_through_the_relay_and_the_relay_sees_no_plaintext() {
    use crate::devmode::relay::FakeRelay;
    use crate::router_registry::{Caps, Cost, Profile, Role, Zone};
    let (a, b) = (hub("relay-a"), hub("relay-b"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    // the relay is not switched on: a relay address is refused with the reason, nothing is sent
    let mine = |name: &str| Profile { id: format!("local/{name}"), provider: "local".into(), model: name.into(), role: Role::Executor, caps: Caps { tools: true, vision: false, audio: false, context: 8192 }, cost: Cost::Free, zone: Zone::Computer, local: true, enabled: true };
    let m = mine("only-seen-by-the-member");
    b.set_models_handler(Arc::new(move || vec![m.clone()]));
    let relay = Arc::new(std::sync::Mutex::new(FakeRelay::new()));
    let box_b = crate::fleet_hub::relay_mailbox_of(&b.my_id());
    assert!(crate::fleet_hub::relay_valid_mailbox(&box_b));
    a.set_member_address(&fid, &b.my_id(), &format!("relay://{box_b}")).unwrap();
    let e = a.query_models(&fid, "bravo").unwrap_err();
    assert!(e.contains("not switched on"), "{e}");
    // both ends switch the relay on; the relay loops look at the mailboxes
    assert_eq!(a.set_relay(relay.clone(), Duration::from_secs(20)), crate::fleet_hub::relay_mailbox_of(&a.my_id()));
    assert_eq!(b.set_relay(relay.clone(), Duration::from_secs(20)), box_b);
    a.start_relay(Duration::from_millis(1)).unwrap();
    b.start_relay(Duration::from_millis(1)).unwrap();
    let got = a.query_models(&fid, "bravo").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].model, "only-seen-by-the-member");
    // the relay carried envelopes, and nothing it holds shows a model name, the fleet name or a key
    let seen = relay.lock().unwrap().seen.clone();
    assert!(seen.len() >= 2, "a request and its answer");
    for bytes in seen {
        let env: crate::devmode::relay::Envelope = serde_json::from_slice(&bytes).unwrap();
        let inner: String = env.body.as_bytes().chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap() as char).collect();
        assert!(!inner.contains("only-seen-by-the-member") && !inner.contains("\"models\":["), "the answer is sealed with the group key");
    }
    // the other way: b reaches a only by mailbox too; its heartbeat goes through the relay and a sees the numbers, and is told no address
    b.set_member_address(&fid, &a.my_id(), &format!("relay://{}", crate::fleet_hub::relay_mailbox_of(&a.my_id()))).unwrap();
    b.heartbeat_tick(&FixedSource(hb(7)));
    let seen_by_a = a.status().fleets[0].members.iter().find(|m| !m.me).unwrap().heartbeat.clone().unwrap();
    assert_eq!(seen_by_a.models, vec!["model-7".to_string()], "the heartbeat came through the relay");
    a.stop_relay();
    b.stop_relay();
    // switched off again: a mailbox address is refused with the reason
    a.relay_off();
    a.set_member_address(&fid, &b.my_id(), &format!("relay://{box_b}")).unwrap();
    assert!(a.query_models(&fid, "bravo").unwrap_err().contains("not switched on"));
}

#[test]
fn the_door_switches_the_relay_on_and_off_asks_first_and_says_plainly_when_there_is_no_relay() {
    use crate::devmode::relay::FakeRelay;
    let a = hub("relay-door");
    let os = iyakki_core_fake::FakeOs::new();
    let gate = os.fleet_fake();
    let none = crate::fleettools::FleetApi::new(a.clone(), gate.clone(), Arc::new(FixedSource(hb(1))), Arc::new(|_, _| {}));
    assert!(none.use_relay(true).unwrap_err().contains("no relay to connect to"));
    assert!(a.relay_address().is_none());
    let api = none.with_relay(Arc::new(std::sync::Mutex::new(FakeRelay::new())));
    let said = api.use_relay(true).unwrap();
    let addr = a.relay_address().unwrap();
    assert!(said.contains(&addr), "{said}");
    assert_eq!(addr, format!("relay://{}", crate::fleet_hub::relay_mailbox_of(&a.my_id())));
    assert!(a.status().relay == addr);
    api.use_relay(false).unwrap();
    assert!(a.relay_address().is_none());
    assert_eq!(gate.calls().iter().filter(|c| c.starts_with("manage(")).count(), 3, "every switch asked the core: {:?}", gate.calls());
}

/// The relay's HTTP wire over a `FakeRelay` in memory, for the fleet's own outgoing connection.
struct WireRelay(std::sync::Mutex<crate::devmode::relay::FakeRelay>);

impl iyakki_proxy::Upstream for WireRelay {
    fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        let now = crate::clock::now_unix() as u64 * 1000;
        let mut r = self.0.lock().unwrap();
        if method == "POST" && url.ends_with("/v1/relay/post") {
            return Ok(match r.post(body, now) {
                Ok(()) => iyakki_proxy::UpstreamResponse { status: 204, body: vec![] },
                Err(d) => iyakki_proxy::UpstreamResponse { status: 422, body: d.word().as_bytes().to_vec() },
            });
        }
        let mailbox = url.rsplit('/').next().unwrap_or("");
        let list: Vec<String> = r.fetch(mailbox, now).iter().map(|e| crate::devmode::relay_link::to_hex(&e.encode().unwrap())).collect();
        Ok(iyakki_proxy::UpstreamResponse { status: 200, body: serde_json::to_vec(&list).unwrap() })
    }
}

#[test]
fn the_fleet_connects_out_to_the_relay_address_the_person_set_and_not_before_and_an_https_relay_needs_its_pin() {
    use crate::fleettools::{relay_from_settings, FleetApi};
    let dir = std::env::temp_dir().join(format!("iyakki-fleet-relay-wire-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wire = Arc::new(WireRelay(std::sync::Mutex::new(crate::devmode::relay::FakeRelay::new())));
    // no address yet: nothing is built, nothing is connected to
    assert!(relay_from_settings(&dir, wire.clone()).err().unwrap().contains("no relay to connect to"));
    let dm = crate::devmode::DevMode::open(&dir);
    let admin = iyakki_sec::policy::Role::Admin.template();
    dm.set_relay_url(&admin, "root", "https://relay.example").unwrap();
    assert!(relay_from_settings(&dir, wire.clone()).err().unwrap().contains("fingerprint"), "an https relay with no pin is refused");
    dm.set_relay_url(&admin, "root", "http://relay.test").unwrap();
    // the door takes the address when the relay is switched on, so one set after the door was made is used
    let (a, b) = (hub("relay-wire-a"), hub("relay-wire-b"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    let gate = iyakki_core_fake::FakeOs::new().fleet_fake();
    let (w1, d1) = (wire.clone(), dir.clone());
    let source: crate::fleettools::RelaySource = Arc::new(move || relay_from_settings(&d1, w1.clone()));
    let api_a = FleetApi::new(a.clone(), gate.clone(), Arc::new(FixedSource(hb(1))), Arc::new(|_, _| {})).with_relay_source(source.clone());
    let api_b = FleetApi::new(b.clone(), gate, Arc::new(FixedSource(hb(2))), Arc::new(|_, _| {})).with_relay_source(source);
    let m = crate::router_registry::Profile { id: "local/wire-model".into(), provider: "local".into(), model: "wire-model".into(), role: crate::router_registry::Role::Executor, caps: crate::router_registry::Caps { tools: true, vision: false, audio: false, context: 8192 }, cost: crate::router_registry::Cost::Free, zone: crate::router_registry::Zone::Computer, local: true, enabled: true };
    b.set_models_handler(Arc::new(move || vec![m.clone()]));
    let (said_a, said_b) = (api_a.use_relay(true).unwrap(), api_b.use_relay(true).unwrap());
    assert!(said_a.contains("relay://") && said_b.contains("relay://"));
    let box_b = crate::fleet_hub::relay_mailbox_of(&b.my_id());
    a.set_member_address(&fid, &b.my_id(), &format!("relay://{box_b}")).unwrap();
    let got = a.query_models(&fid, "bravo");
    api_a.use_relay(false).unwrap();
    api_b.use_relay(false).unwrap();
    assert_eq!(got.unwrap()[0].model, "wire-model", "the question and its answer went through the relay's wire");
    assert!(wire.0.lock().unwrap().seen.len() >= 2, "a request and its answer were posted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dispatcher_with_no_fleet_named_looks_across_two_fleets_and_a_named_one_stays_inside_its_fleet() {
    use crate::router::FleetRunner;
    let (p, _w1, _w2, fid) = fleet3("twofleets");
    // a second fleet of the same planner computer, with one member of its own
    let d = hub("twofleets-d");
    up(&d, 4);
    let fid2 = p.hub.create_fleet("second", "alpha").unwrap();
    let (code, _) = p.hub.offer_code(&fid2).unwrap();
    let a_addr = p.hub.listening().unwrap().to_string();
    let t = d.join_start(&a_addr, &code, "delta").unwrap();
    d.poll_pending();
    p.hub.approve_join(&p.hub.status().requests[0].id).unwrap();
    d.poll_pending();
    d.join_accept(&t).unwrap();
    p.hub.settle();
    for _ in 0..2 {
        p.hub.heartbeat_tick(&FixedSource(hb(1)));
        d.heartbeat_tick(&FixedSource(hb(2)));
    }
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let names = |x: &crate::fleet_work::FleetDispatch| {
        let mut n: Vec<String> = x.members().into_iter().map(|m| m.name).collect();
        n.sort();
        n
    };
    let all = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), "", Duration::from_secs(5));
    assert_eq!(names(&all), vec!["bravo".to_string(), "charlie".to_string(), "delta".to_string()], "the members of both fleets are offered");
    let first = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate.clone(), &fid, Duration::from_secs(5));
    assert_eq!(names(&first), vec!["bravo".to_string(), "charlie".to_string()]);
    let second = crate::fleet_work::FleetDispatch::new(p.hub.clone(), gate, &fid2, Duration::from_secs(5));
    assert_eq!(names(&second), vec!["delta".to_string()], "a named fleet never offers the members of the other");
    d.stop_node();
}

#[test]
fn a_task_that_asks_a_question_shows_it_to_the_planner_and_the_planners_answer_reaches_the_running_task() {
    let (p, w1, _w2, fid) = fleet3("qa");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let base = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    let t = body(1, "ask", &[], &base, &["F-A-01"]);
    assert_eq!(p.hub.assign(&fid, "bravo", &t).unwrap(), "accepted");
    w1.hub.settle();
    until("the task starts", || !w1.mgr.as_ref().unwrap().list().is_empty());
    // no answer to a task that is not out, nor to one that is somebody else's
    let gate = Arc::new(Gate(std::sync::Mutex::new(vec![])));
    let api = crate::fleettools::FleetApi::new(p.hub.clone(), gate.clone(), Arc::new(FixedSource(hb(1))), Arc::new(|_, _| {}));
    assert!(api.answer_task("ffff", "x").is_err());
    assert!(api.answer_task(&t.id[..8], "  ").is_err());
    // the answer is sealed to the member, asks the core once and is kept for the task
    let asked = gate.0.lock().unwrap().len();
    api.answer_task(&t.id[..8], "Use the second approach.").unwrap();
    assert_eq!(gate.0.lock().unwrap().len(), asked + 1);
    let item = w1.hub.status().inbox.into_iter().find(|i| i.id == t.id).unwrap();
    assert_eq!(item.answer, "Use the second approach.");
    // the worker's pass hands it to the task once and clears it
    w1.hub.attached().unwrap().worker.tick();
    assert_eq!(w1.hub.status().inbox.into_iter().find(|i| i.id == t.id).unwrap().answer, "", "given once");
    // a member that did not send the task cannot answer it: the third computer's answer is refused
    let c_item = InboxItem { id: t.id.clone(), fleet: fid.clone(), from: p.hub.my_id(), from_name: "alpha".into(), body: t.clone(), state: "running".into(), at: 0, task: String::new(), detail: String::new(), answer: String::new() };
    let _ = c_item;
    assert!(w1.hub.send_answer(&fid, &p.hub.my_id(), &t.id, "from the worker to the planner").is_err(), "the worker has no such task out");
}

#[test]
fn a_follow_up_starts_on_the_worker_from_the_integration_branch_the_planner_sends_first_as_a_bundle() {
    let (p, w1, w2, fid) = fleet3("followbase");
    for w in [&w1, &w2] {
        w.hub.set_accept(Accept::Members);
        w.hub.trust(&fid, "alpha", true).unwrap();
    }
    // the planner has a branch with a commit the workers do not have
    g(&p.root, &["checkout", "-q", "-b", "fleet/integration-t"]);
    std::fs::write(p.root.join("integrated.txt"), "from the integration branch\n").unwrap();
    g(&p.root, &["add", "-A"]);
    g(&p.root, &["commit", "-q", "-m", "integration work"]);
    let head = g(&p.root, &["rev-parse", "HEAD"]).trim().to_string();
    g(&p.root, &["checkout", "-q", "main"]);
    assert!(!crate::tasks::git_run(&w1.root, &["cat-file", "-e", &format!("{head}^{{commit}}")]).0, "the worker does not have it");
    // not from a member the worker does not trust, nor an id that is not a commit
    let bundle = tmp("followbase-b").join("base.bundle");
    std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
    g(&p.root, &["bundle", "create", bundle.to_str().unwrap(), "refs/heads/fleet/integration-t"]);
    assert!(p.hub.send_base(&fid, "bravo", "not-a-commit", &bundle).is_err());
    p.hub.send_base(&fid, "bravo", &head, &bundle).unwrap();
    assert!(w1.hub.base_path(&head).is_some(), "kept on the worker, checked by its checksum");
    // the follow-up names that commit as its base: the worker's task starts from it
    let mut t = body(1, "redo", &[], &head, &[]);
    t.base_branch = "fleet/integration-t".into();
    assert_eq!(p.hub.assign(&fid, "bravo", &t).unwrap(), "accepted");
    w1.hub.settle();
    until("the task starts", || !w1.mgr.as_ref().unwrap().list().is_empty());
    assert!(crate::tasks::git_run(&w1.root, &["cat-file", "-e", &format!("{head}^{{commit}}")]).0, "the commits arrived in the worker's workspace");
    let task = w1.mgr.as_ref().unwrap().list().remove(0);
    assert!(task.tree.join("integrated.txt").exists(), "the task's own tree starts from the integration branch: {:?}", task.tree);
    // a worker that takes no work refuses a base
    w2.hub.set_accept(Accept::Never);
    assert!(p.hub.send_base(&fid, "charlie", &head, &bundle).is_err());
}

#[test]
fn a_base_cut_in_half_by_a_restart_is_finished_from_where_it_stopped_and_a_base_the_worker_holds_is_not_sent_again() {
    use crate::fleet_hub::CHUNK;
    let (p, w1, _w2, fid) = fleet3("baseresume");
    w1.hub.set_accept(Accept::Members);
    w1.hub.trust(&fid, "alpha", true).unwrap();
    let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let l = lines.clone();
        p.hub.set_audit(Arc::new(move |w, d| l.lock().unwrap().push(format!("{w} {d}"))));
    }
    // a file of three whole chunks and a bit stands for the bundle (the worker keeps the bytes; git reads them only when a task starts)
    let bytes: Vec<u8> = (0..(3 * CHUNK + 99)).map(|n| (n % 241) as u8).collect();
    let file = tmp("baseresume-bundle").join("base.bundle");
    std::fs::write(&file, &bytes).unwrap();
    let sha = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let commit = "ab".repeat(20);
    let total = bytes.chunks(CHUNK).count() as u32;
    let hex = |c: &[u8]| c.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // the first two chunks arrive, then the worker restarts (its count in memory is gone)
    for (n, c) in bytes.chunks(CHUNK).take(2).enumerate() {
        let m = serde_json::json!({"commit": commit, "sha256": sha, "size": bytes.len(), "index": n, "total": total, "data": hex(c)});
        p.hub.send_member(&fid, "bravo", "base_chunk", &m).unwrap();
    }
    w1.hub.forget_assemblies();
    assert!(w1.hub.base_path(&commit).is_none(), "not whole yet");
    p.hub.send_base(&fid, "bravo", &commit, &file).unwrap();
    assert_eq!(std::fs::read(w1.hub.base_path(&commit).expect("whole now")).unwrap(), bytes, "the base is whole and equal");
    assert!(lines.lock().unwrap().iter().any(|l| l.starts_with("fleet.base.out") && l.contains("resumed_at=2")), "{:?}", lines.lock().unwrap());
    // sending it again finds it there and sends nothing
    p.hub.send_base(&fid, "bravo", &commit, &file).unwrap();
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("already_there=yes")));
    // a worker that takes no work answers the question with a refusal, never with how far it got
    w1.hub.set_accept(Accept::Never);
    let again = "cd".repeat(20);
    assert!(p.hub.send_base(&fid, "bravo", &again, &file).is_err());
}
