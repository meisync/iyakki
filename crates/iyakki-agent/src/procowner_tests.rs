use super::*;
use crate::sysinfo::tests::machine;
use iyakki_sec::policy::Role;

fn proc(pid: u32, ppid: u32) -> Proc {
    Proc { pid, ppid, uid: 0, user: "root".into(), name: format!("p{pid}"), cmd: String::new(), state: 'S', threads: 1, ticks: 0, rss: 0, start_ticks: 1, cgroup: String::new() }
}

fn claim_of(pid: u32, user: &str, what: &str, since: i64) -> Claim {
    Claim { pid, user: user.into(), what: what.into(), since }
}

#[test]
fn a_claimed_program_and_everything_below_it_belong_to_the_person_and_nothing_else_does() {
    // 1 init -> 10 (ann's shell script) -> 11 -> 12 ; 1 -> 20 (bob's) -> 21 ; 1 -> 30 (nobody's)
    let procs = vec![proc(1, 0), proc(10, 1), proc(11, 10), proc(12, 11), proc(20, 1), proc(21, 20), proc(30, 1)];
    let o = owners(&procs, &[claim_of(10, "ann", "sh", 5), claim_of(20, "bob", "python3", 6)]);
    assert_eq!(o.get(&12).map(|x| x.0.as_str()), Some("ann"), "a grandchild is hers");
    assert_eq!(o.get(&21).map(|x| (x.0.as_str(), x.1.as_str())), Some(("bob", "python3")));
    assert!(!o.contains_key(&30) && !o.contains_key(&1), "an unclaimed process and process 1 belong to nobody");
    // two claims on one pid: the older stays
    let o = owners(&procs, &[claim_of(10, "late", "x", 9), claim_of(10, "ann", "sh", 5)]);
    assert_eq!(o[&10].0, "ann");
    // a made-up loop in the parent links ends
    let looped = vec![proc(5, 6), proc(6, 5)];
    assert!(owners(&looped, &[claim_of(99, "ann", "x", 1)]).is_empty());
}

#[test]
fn guards_and_the_thread_setting_come_and_go() {
    let me = std::process::id();
    {
        let _g = claim(me, "zed-guard", "test");
        assert!(claims().iter().any(|c| c.pid == me && c.user == "zed-guard"));
    }
    assert!(!claims().iter().any(|c| c.user == "zed-guard"), "dropping the guard ends the claim");
    assert_eq!(current_user(), None);
    as_user("ann", || {
        assert_eq!(current_user().as_deref(), Some("ann"));
        as_user("bob", || assert_eq!(current_user().as_deref(), Some("bob")));
        assert_eq!(current_user().as_deref(), Some("ann"), "the inner setting is undone");
    });
    assert_eq!(current_user(), None);
    // another thread does not see it
    as_user("ann", || assert_eq!(std::thread::spawn(current_user).join().unwrap(), None));
}

#[test]
fn a_command_run_for_a_person_is_claimed_while_it_runs_and_listed_under_her_name() {
    let d = crate::archive::tests::tmp("pown-run");
    let who = format!("pown-{}", std::process::id());
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let who2 = who.clone();
    let watcher = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Some(c) = claims().into_iter().find(|c| c.user == who2) {
                s2.lock().unwrap().push(c);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let out = as_user(&who, || {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("1").current_dir(&d);
        crate::exec::run_prepared(cmd, "sleep", 10, None)
    });
    watcher.join().unwrap();
    assert!(out.is_ok(), "{out:?}");
    let c = seen.lock().unwrap().first().cloned().expect("the sleeping child was claimed for the person");
    assert_eq!(c.what, "sleep");
    assert!(claims().iter().all(|c| c.user != who), "and the claim is gone when the command ended");
    // a command run with nobody set is not claimed
    let before = claims().len();
    let _ = crate::exec::run_prepared(std::process::Command::new("true"), "true", 10, None);
    assert_eq!(claims().len(), before);
}

fn two_owners() -> (SysRoots, Vec<Claim>) {
    // the made-up machine has processes 1 (root) and 42; make 42 a child of 1 and add 43 below 42
    let r = machine(&format!("pown-mine-{:?}", std::thread::current().id()));
    let p = r.proc.join("43");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("stat"), "43 (worker) S 42 0 0 0 0 0 0 0 0 0 20 10 0 0 20 0 1 0 950 123456 700 0\n").unwrap();
    std::fs::write(p.join("status"), "Name:\tworker\nUid:\t65534\t65534\t65534\t65534\n").unwrap();
    std::fs::write(p.join("cmdline"), "worker\0--api-key\0sekret-value\0").unwrap();
    (r, vec![claim_of(42, "ann", "my (odd) prog", 1)])
}

#[test]
fn a_person_sees_only_her_own_and_an_administrator_sees_all_with_the_owner() {
    let (r, claims) = two_owners();
    let other = claim_of(43, "bob", "worker", 2);
    let both = vec![claims[0].clone(), other];
    let ann = mine(&r, &both, "ann", false);
    // 43 is below 42 (ann's) but has its own, newer claim for bob: the claim on the process itself wins over the walk upwards
    assert_eq!(ann.iter().map(|o| o.proc.pid).collect::<Vec<_>>(), vec![42]);
    let all = mine(&r, &both, "ann", true);
    assert_eq!(all.iter().map(|o| (o.proc.pid, o.owner.as_str())).collect::<Vec<_>>(), vec![(42, "ann"), (43, "bob")]);
    let nobody = mine(&r, &both, "carol", false);
    assert!(nobody.is_empty());
    assert!(text(&nobody, false).contains("No process is running for you"));
    // the text hides secrets, shows the owner only when asked, and the processor seconds
    let t = text(&all, true);
    assert!(t.contains("bob") && t.contains("ann") && t.contains("worker --api-key [hidden]") && !t.contains("sekret-value"), "{t}");
    assert!(!text(&ann, false).contains("ann "), "no owner column for a person's own list");
}

#[test]
fn stopping_needs_ownership_refuses_the_system_and_really_signals_a_real_child() {
    let (r, claims) = two_owners();
    assert!(stop(&r, &claims, "bob", false, 42, false).unwrap_err().contains("not one of yours"));
    assert!(stop(&r, &claims, "ann", false, 1, false).unwrap_err().contains("refused"), "process 1");
    assert!(stop(&r, &claims, "ann", false, std::process::id(), false).unwrap_err().contains("refused"), "the shell itself");
    assert!(stop(&r, &claims, "ann", false, 777, false).unwrap_err().contains("not one of yours"), "a number nobody owns");
    // a real child, shown through a made-up /proc entry that names its number
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let p = r.proc.join(pid.to_string());
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("stat"), format!("{pid} (sleep) S 1 0 0 0 0 0 0 0 0 0 0 0 0 0 20 0 1 0 5 1000 10 0\n")).unwrap();
    std::fs::write(p.join("status"), "Name:\tsleep\nUid:\t0\t0\t0\t0\n").unwrap();
    std::fs::write(p.join("cmdline"), "sleep\x0030\0").unwrap();
    let c = vec![claim_of(pid, "ann", "sleep", 1)];
    assert!(stop(&r, &c, "bob", false, pid, true).is_err(), "not bob's");
    assert!(child.try_wait().unwrap().is_none(), "bob's try did nothing");
    let said = stop(&r, &c, "ann", false, pid, true).unwrap();
    assert!(said.starts_with("Killed sleep"), "{said}");
    assert!(!child.wait().unwrap().success(), "the kill reached the program");
}

#[test]
fn the_chat_tools_follow_the_policy_ask_for_stopping_and_write_the_audit() {
    let (r, claims) = two_owners();
    let mut user = Role::User.template();
    user.apps.insert(App::System);
    let audit = Mutex::new(Vec::<String>::new());
    let a = |e: &str, d: &str| audit.lock().unwrap().push(format!("{e} {d}"));
    let defs_user = defs(&user);
    assert_eq!(defs_user.iter().map(|d| (d.spec.name.as_str(), d.risk)).collect::<Vec<_>>(), vec![("system_my_processes", Risk::Safe), ("system_process_stop", Risk::Ask)]);
    // a guest without the System tile gets neither tool, and a direct call is refused
    let mut guest = Role::Guest.template();
    guest.apps.remove(&App::System);
    assert!(defs(&guest).is_empty());
    assert!(call_with("system_my_processes", &json!({}), "g", &guest, &r, &claims, &a).is_err());
    // a person lists her own, hears about someone else's number, and her stop is audited
    let listed = call_with("system_my_processes", &json!({}), "ann", &user, &r, &claims, &a).unwrap();
    assert!(listed.contains("42"), "{listed}");
    assert!(call_with("system_my_processes", &json!({}), "bob", &user, &r, &claims, &a).unwrap().contains("No process"));
    assert!(call_with("system_process_stop", &json!({"pid": 42}), "bob", &user, &r, &claims, &a).is_err());
    assert!(call_with("system_process_stop", &json!({}), "ann", &user, &r, &claims, &a).unwrap_err().contains("pid"));
    assert!(audit.lock().unwrap().iter().any(|l| l.starts_with("system.stop 42")));
    assert!(describe("system_process_stop", &json!({"pid": 42, "hard": true})).starts_with("KILL process 42"));
    assert!(is_tool("system_process_stop") && !is_tool("system_processes"));
}

#[test]
fn disk_use_reads_the_kernel_count_of_real_storage_traffic() {
    let t = "rchar: 5000\nwchar: 100\nsyscr: 3\nsyscw: 2\nread_bytes: 1048576\nwrite_bytes: 2048\ncancelled_write_bytes: 0\n";
    assert_eq!(crate::sysinfo::parse_disk_use(t), Some(crate::sysinfo::DiskUse { read_bytes: 1048576, write_bytes: 2048 }));
    assert_eq!(crate::sysinfo::parse_disk_use("rchar: 1\n"), None, "a file without the two counts says nothing");
    assert_eq!(crate::sysinfo::parse_disk_use("read_bytes: x\nwrite_bytes: 1\n"), None);
    assert!(crate::sysinfo::DiskUse::default().text().is_empty());
    assert!(crate::sysinfo::DiskUse { read_bytes: 1048576, write_bytes: 2048 }.text().starts_with("disk"));
    // from made-up files in a made-up /proc: a process with the file is listed, one without is not
    let d = crate::archive::tests::tmp("pown-disk");
    std::fs::create_dir_all(d.join("42")).unwrap();
    std::fs::write(d.join("42/io"), t).unwrap();
    std::fs::create_dir_all(d.join("43")).unwrap();
    let r = crate::sysinfo::SysRoots { proc: d.clone(), ..crate::sysinfo::SysRoots::real() };
    let m = crate::sysinfo::disk_use(&r, &[42, 43]);
    assert_eq!(m.len(), 1);
    assert_eq!(m[&42].read_bytes, 1048576);
}
