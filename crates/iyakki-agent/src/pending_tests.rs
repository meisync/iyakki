//! F-AI-16, waiting that actually waits: the ledger, the waker, the gate and the tools, with real detached jobs and a scripted model.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{CliLevel, Policy, Role};
use serde_json::{json, Value};

use crate::agent::{start_with, Risk, RunEvent, RunLimits, ToolDef, Tools, Verdict};
use crate::assistrun::Assist;
use crate::chat::{ChatBackend, Message, ModelTurn, Role as MsgRole, ToolCall, ToolSpec};
use crate::jobs::{Cmd, Jobs, Spec};
use crate::jobtools::JobTools;
use crate::pending::{promises_to_report, Feed, Ledger, Settings, Source, State, Ticket, Wake, Waker, NO_REPORT_NOTICE};
use crate::pendingtools::{PendingTools, PENDING_TOOLS};
use crate::toolblock::ReminderCtx;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-pending-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("work")).unwrap();
        Tmp(d)
    }
    fn work(&self) -> PathBuf {
        std::fs::canonicalize(self.0.join("work")).unwrap()
    }
    fn ledger(&self) -> Ledger {
        Ledger::open(&self.0.join("pending"), "alice")
    }
    fn jobs(&self) -> Jobs {
        Jobs::new(&self.0.join("jobs"))
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Inner;
impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![ToolDef { spec: ToolSpec { name: "now".into(), description: "time".into(), schema: json!({"type": "object"}) }, risk: Risk::Safe, untrusted: false }]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Ok(format!("inner {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

fn policy_for(work: &Path) -> Policy {
    let mut p = Role::Admin.template();
    p.cli = CliLevel::Full;
    p.fs_read = vec![work.display().to_string()];
    p.fs_write = vec![work.display().to_string()];
    p
}

fn feed(t: &Tmp) -> Feed {
    Feed { jobs: Some(t.jobs()), tasks: None, user: "alice".into(), workspace: String::new() }
}

fn stack(t: &Tmp) -> (Arc<dyn Tools>, Ledger, Jobs) {
    let (jobs, ledger) = (t.jobs(), t.ledger());
    let jt = JobTools::new(Arc::new(Inner), jobs.clone(), "alice", policy_for(&t.work()), &t.work(), None);
    let pt = PendingTools::new(Arc::new(jt), ledger.clone(), feed(t), policy_for(&t.work()), &t.work());
    (Arc::new(pt), ledger, jobs)
}

fn start_job(jobs: &Jobs, t: &Tmp, label: &str, cmd: &str) -> String {
    jobs.start("alice", &Spec { label: label.into(), cmd: Cmd::Shell(cmd.into()), cwd: t.work(), workspace: String::new() }, &|_| Ok(())).unwrap().id
}

fn end_of(jobs: &Jobs, id: &str) {
    jobs.wait("alice", id, Duration::from_secs(20)).unwrap();
}

// ------------------------------------------------------------------------------------------------ the phrase test

#[test]
fn the_promise_test_catches_waiting_and_reporting_back_and_not_questions_or_refusals() {
    for yes in [
        "I'll report back when the build finishes.",
        "The install is running. I will let you know once it is done.",
        "Started the server. I\u{2019}ll wait for it and check back shortly.",
        "Working on it - I'll keep you posted.",
        "Once it finishes, I'll summarise the results.",
        "I'm waiting for the download to complete.",
    ] {
        assert!(promises_to_report(yes), "{yes}");
    }
    for no in [
        "The build finished and all 12 tests passed.",
        "Shall I wait and report back when it finishes?",
        "I can't report back later: I only answer when you write.",
        "I cannot wait for the job, so here is its state now.",
        "If you want, I can let you know when it is done.",
        "",
    ] {
        assert!(!promises_to_report(no), "{no}");
    }
}

// ------------------------------------------------------------------------------------------------ the ledger

#[test]
fn the_ledger_is_per_person_persistent_and_keeps_one_running_ticket_per_producer_item() {
    let t = Tmp::new("persist");
    let l = t.ledger();
    let a = l.register(Ticket { source: Source::Install, key: "sdk".into(), label: "SDK install".into(), on_done: "run the tests".into(), ..Ticket::default() });
    let again = l.register(Ticket { source: Source::Install, key: "sdk".into(), label: "SDK install".into(), ..Ticket::default() });
    assert_eq!(a.id, again.id, "the same producer item is one ticket");
    // another person sees nothing; a fresh handle on the same file sees it
    assert!(Ledger::open(&t.0.join("pending"), "bob").list("").is_empty());
    let reopened = t.ledger();
    assert_eq!(reopened.list("")[0].on_done, "run the tests");
    // a secret in a label is not stored
    let s = l.register(Ticket { label: "deploy with token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into(), ..Ticket::default() });
    assert!(!s.label.contains("abcdefghijklmnopqrstuvwxyz0123456789"), "{}", s.label);
    // the file is written atomically and holds states and short lines only
    let raw = std::fs::read_to_string(t.0.join("pending/alice.json")).unwrap();
    assert!(raw.contains("SDK install") && !raw.contains(".tmp"));
}

#[test]
fn after_a_restart_work_nobody_can_vouch_for_is_lost_and_said_once_while_jobs_and_probes_carry_on() {
    let t = Tmp::new("restart");
    let l = t.ledger();
    l.report(Source::Pull, "model-x", "pull model-x", "", State::Running, "");
    let job = l.register(Ticket { source: Source::Job, key: "j9".into(), label: "build".into(), ..Ticket::default() });
    let probe = l.register(Ticket { source: Source::Probe, label: "wait for file".into(), probe: Some(crate::pending::Probe::File { path: "/nonexistent/iyakki-never".into() }), ..Ticket::default() });
    // the machine restarted: every ticket belongs to an earlier process
    let path = t.0.join("pending/alice.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for tk in v["tickets"].as_array_mut().unwrap() {
        tk["boot"] = json!(1);
    }
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let l2 = t.ledger();
    let all = l2.list("");
    let by = |id: &str| all.iter().find(|x| x.id == id).unwrap().clone();
    assert_eq!(all.iter().find(|x| x.source == Source::Pull).unwrap().state, State::Lost, "never silently dropped");
    assert_eq!(by(&job.id).state, State::Running, "the job's own store says what became of it");
    assert_eq!(by(&probe.id).state, State::Running, "a probe is simply tested again");
    // the assistant is told once, on its next turn or wake, "while you were away"
    let wake = l2.take_wake("", crate::jobs::now_ms() + 1000, None);
    match wake {
        Some(Wake::Turn { text, .. }) => assert!(text.contains("WHILE YOU WERE AWAY") && text.contains("pull model-x") && text.contains("lost"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(l2.take_wake("", 0, None).is_none(), "once per ticket");
}

// ------------------------------------------------------------------------------------------------ the waker

#[test]
fn a_finished_job_wakes_the_session_once_with_its_result_and_the_job_tools_do_not_note_it_again() {
    let t = Tmp::new("wake");
    let (_, l, jobs) = stack(&t);
    let id = start_job(&jobs, &t, "emulator install", "echo emulator installed; sleep 0.3");
    l.sync(&feed(&t));
    assert_eq!(l.list("").len(), 1, "a running job is mirrored without anyone registering it");
    l.register(Ticket { source: Source::Job, key: id.clone(), on_done: "start the app".into(), ..Ticket::default() });
    assert!(l.take_wake("", 0, None).is_none(), "nothing ended yet");
    end_of(&jobs, &id);
    l.sync(&feed(&t));
    let wake = l.take_wake("", 0, Some((&jobs, "alice"))).expect("the job ended");
    let Wake::Turn { text, ids } = wake else { panic!("{wake:?}") };
    assert!(text.starts_with("[IYAKKI]") && text.contains("emulator install") && text.contains("finished") && text.contains("emulator installed"), "{text}");
    assert!(text.contains("You said you would: start the app") && text.contains("exited 0, not verified"), "{text}");
    assert_eq!(ids.len(), 1);
    // one wake per ticket
    l.sync(&feed(&t));
    assert!(l.take_wake("", 0, Some((&jobs, "alice"))).is_none());
    // and the woken turn is not told the same ending by the job tools
    assert!(jobs.take_finished("alice", "", crate::jobs::Channel::Agent).is_empty());
}

#[test]
fn a_job_the_run_already_reported_does_not_wake_the_session_again() {
    let t = Tmp::new("told");
    let (tools, l, jobs) = stack(&t);
    let id = start_job(&jobs, &t, "quick", "echo hi");
    l.sync(&feed(&t));
    end_of(&jobs, &id);
    // the next model turn: the job tools note it, the ledger learns the assistant has it
    let notes = tools.take_notes();
    assert!(notes.iter().any(|n| n.contains("JOB FINISHED: quick")), "{notes:?}");
    assert!(l.take_wake("", 0, None).is_none());
    assert!(!l.has_untold(""));
}

#[test]
fn two_pieces_of_work_ending_together_wake_the_session_once_and_the_daily_limit_stops_a_runaway_chain() {
    let t = Tmp::new("batch");
    let l = t.ledger();
    for k in ["a", "b"] {
        l.report(Source::Install, k, &format!("install {k}"), "", State::Running, "");
    }
    for k in ["a", "b"] {
        l.report(Source::Install, k, "", "", State::FinishedOk, "done");
    }
    let Some(Wake::Turn { text, ids }) = l.take_wake("", 0, None) else { panic!() };
    assert_eq!(ids.len(), 2);
    assert!(text.contains("2 pieces of work") && text.contains("install a") && text.contains("install b"), "{text}");
    // the chain limit: each ending is its own wake here
    l.set_settings(Settings { max_wakes_per_day: 3, ..Settings::default() });
    for n in 0..2 {
        let k = format!("c{n}");
        l.report(Source::Pull, &k, &format!("pull {k}"), "", State::Running, "");
        l.report(Source::Pull, &k, "", "", State::FinishedFailed, "no space");
        assert!(matches!(l.take_wake("", 0, None), Some(Wake::Turn { .. })), "wake {n}");
    }
    l.report(Source::Pull, "c9", "pull c9", "", State::Running, "");
    l.report(Source::Pull, "c9", "", "", State::FinishedFailed, "no space");
    match l.take_wake("", 0, None) {
        Some(Wake::Limit { text }) => assert!(text.contains("3 times today"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(l.take_wake("", 0, None).is_none(), "the limit message is said once");
    assert_eq!(l.wakes_left(), 0);
}

#[test]
fn with_auto_continue_off_nothing_wakes_and_the_note_waits_for_the_next_turn() {
    let t = Tmp::new("off");
    let (tools, l, _) = stack(&t);
    l.set_settings(Settings { auto_continue: false, ..Settings::default() });
    l.report(Source::Flow, "f1", "nightly flow", "", State::Running, "");
    l.report(Source::Flow, "f1", "", "", State::FinishedOk, "all steps ran");
    assert!(l.take_wake("", 0, None).is_none());
    let notes = tools.take_notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("nightly flow") && notes[0].contains("Not verified"), "{notes:?}");
    assert!(tools.take_notes().is_empty(), "once");
}

#[test]
fn a_wait_that_passes_its_deadline_is_timed_out_and_reported_once() {
    let t = Tmp::new("deadline");
    let l = t.ledger();
    let k = l.register(Ticket { source: Source::Probe, label: "wait for a file".into(), probe: Some(crate::pending::Probe::File { path: "/nonexistent/iyakki-never".into() }), deadline_ms: crate::jobs::now_ms() + 1, ..Ticket::default() });
    std::thread::sleep(Duration::from_millis(20));
    l.sync(&feed(&t));
    assert_eq!(l.get(&k.id).unwrap().state, State::TimedOut);
    let Some(Wake::Turn { text, .. }) = l.take_wake("", 0, None) else { panic!() };
    assert!(text.contains("timed out"), "{text}");
    assert!(l.take_wake("", 0, None).is_none());
}

#[test]
fn a_parallel_task_wakes_the_session_not_only_a_job() {
    let t = Tmp::new("task");
    let l = t.ledger();
    let status = Arc::new(Mutex::new(crate::tasks::Status::Running));
    let s2 = status.clone();
    let mut f = feed(&t);
    f.tasks = Some(Arc::new(move || vec![crate::tasks::Task { id: "t1".into(), title: "refactor parser".into(), status: *s2.lock().unwrap(), summary: "3 rows done".into(), ..Default::default() }]));
    l.sync(&f);
    let tk = l.list("");
    assert_eq!((tk.len(), tk[0].source, tk[0].state), (1, Source::Task, State::Running));
    assert!(l.take_wake("", 0, None).is_none());
    *status.lock().unwrap() = crate::tasks::Status::Done;
    l.sync(&f);
    let Some(Wake::Turn { text, .. }) = l.take_wake("", 0, None) else { panic!() };
    assert!(text.contains("refactor parser") && text.contains("3 rows done"), "{text}");
}

#[test]
fn the_waker_thread_raises_the_flag_when_a_job_ends_and_hands_the_turn_to_an_idle_session() {
    let t = Tmp::new("thread");
    let (_, l, jobs) = stack(&t);
    let w = Waker::start_every(l, feed(&t), Duration::from_millis(30));
    let id = start_job(&jobs, &t, "short build", "sleep 0.4; echo built");
    std::thread::sleep(Duration::from_millis(150));
    assert!(!w.ready(), "still running");
    end_of(&jobs, &id);
    let end = Instant::now() + Duration::from_secs(10);
    while !w.ready() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(w.ready());
    let Some(Wake::Turn { text, .. }) = w.take() else { panic!() };
    assert!(text.contains("short build") && text.contains("built"), "{text}");
    assert!(!w.ready() && w.take().is_none());
}

// ------------------------------------------------------------------------------------------------ checks: finished is not done

#[test]
fn a_job_that_exits_zero_but_fails_its_check_is_reported_as_a_failed_check() {
    let t = Tmp::new("check");
    let (tools, l, jobs) = stack(&t);
    let a = start_job(&jobs, &t, "build a", "sleep 2");
    let b = start_job(&jobs, &t, "build b", "sleep 2; touch made.bin");
    l.sync(&feed(&t));
    let tid = |key: &str| l.list("").into_iter().find(|x| x.key == key).unwrap_or_else(|| panic!("no ticket for {key}: {:?}", l.list(""))).id;
    // the check is attached through the tool (it asks)
    let r = tools.call("pending_check", &json!({"id": tid(&a), "command": "test -f missing.bin"})).unwrap();
    assert!(r.contains("runs in"), "{r}");
    tools.call("pending_check", &json!({"id": tid(&b), "command": "test -f made.bin"})).unwrap();
    end_of(&jobs, &a);
    end_of(&jobs, &b);
    l.sync(&feed(&t));
    let (ta, tb) = (l.get(&tid(&a)).unwrap(), l.get(&tid(&b)).unwrap());
    assert_eq!((ta.state, ta.check_ok), (State::FinishedFailed, false), "exit 0 but no missing.bin");
    assert!(ta.detail.contains("check failed"), "{}", ta.detail);
    assert_eq!((tb.state, tb.check_ok, tb.checked), (State::FinishedOk, true, true));
    let notes = tools.take_notes();
    assert!(notes.iter().any(|n| n.contains("build a") && n.contains("FAILED")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("build b") && n.contains("passed")), "{notes:?}");
}

#[test]
fn the_check_tool_refuses_a_destructive_command_and_a_failing_job_needs_no_check() {
    let t = Tmp::new("checkrefuse");
    let (tools, l, jobs) = stack(&t);
    let id = start_job(&jobs, &t, "doomed", "sleep 0.2; exit 3");
    l.sync(&feed(&t));
    let tid = l.list("")[0].id.clone();
    let e = tools.call("pending_check", &json!({"id": tid, "command": "rm -rf /"})).unwrap_err();
    assert!(e.contains("must only read"), "{e}");
    end_of(&jobs, &id);
    l.sync(&feed(&t));
    let tk = l.get(&tid).unwrap();
    assert_eq!(tk.state, State::FinishedFailed);
    assert!(tk.detail.contains("exit code 3"), "{}", tk.detail);
}

// ------------------------------------------------------------------------------------------------ the tools

#[test]
fn the_pending_tools_have_the_right_risks_and_an_account_that_may_not_run_commands_gets_no_check() {
    let t = Tmp::new("defs");
    let (tools, _, _) = stack(&t);
    let defs = tools.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing")).risk;
    assert_eq!((risk("wait_for"), risk("pending_list")), (Risk::Safe, Risk::Safe));
    assert_eq!((risk("pending_cancel"), risk("pending_check"), risk("pending_settings")), (Risk::Ask, Risk::Ask, Risk::Ask));
    assert_eq!(PENDING_TOOLS.iter().filter(|n| defs.iter().any(|d| &d.spec.name == *n)).count(), 5);
    assert!(defs.iter().any(|d| d.spec.name == "job_start") && defs.iter().any(|d| d.spec.name == "now"));
    assert!(!tools.approved_always("pending_cancel") && tools.approved_always("now"));
    for n in PENDING_TOOLS {
        assert!(crate::extagent::bridge::offered(defs.iter().find(|d| d.spec.name == n).unwrap()), "{n} is offered through the bridge");
    }
    assert_eq!(crate::toolblock::group_of("wait_for"), crate::toolblock::Group::Files, "always loaded");
    let mut low = policy_for(&t.work());
    low.cli = CliLevel::Restricted;
    let pt = PendingTools::new(Arc::new(Inner), t.ledger(), feed(&t), low, &t.work());
    assert!(!pt.defs().iter().any(|d| d.spec.name == "pending_check") && pt.defs().iter().any(|d| d.spec.name == "wait_for"));
    assert!(pt.describe("pending_cancel", &json!({"id": "p1"})).contains("p1"));
}

#[test]
fn wait_for_registers_a_file_a_port_or_a_job_and_the_ticket_lands_with_what_the_model_said_it_would_do() {
    let t = Tmp::new("waitfor");
    let (tools, l, jobs) = stack(&t);
    // a file inside the person's scope
    let r = tools.call("wait_for", &json!({"file": "ready.txt", "on_done": "open the page", "timeout_seconds": 120})).unwrap();
    assert!(r.contains("Ticket p1") && r.contains("Waiting for") && r.contains("do not poll"), "{r}");
    assert_eq!(tools.jobs_running().len(), 1, "Autopilot and the end-of-run gate see every source, not only jobs");
    assert!(tools.jobs_running()[0].contains("ready.txt"));
    std::fs::write(t.work().join("ready.txt"), "x").unwrap();
    l.sync(&feed(&t));
    let notes = tools.take_notes();
    assert!(notes.len() == 1 && notes[0].contains("ready.txt") && notes[0].contains("You said you would: open the page"), "{notes:?}");
    assert!(tools.jobs_running().is_empty());
    // already true: nothing to wait for
    assert!(tools.call("wait_for", &json!({"file": "ready.txt"})).unwrap().contains("nothing to wait for"));
    // a path outside the scope is refused
    assert!(tools.call("wait_for", &json!({"file": "/etc/hostname"})).is_err());
    // a port on this machine; another host is refused
    assert!(tools.call("wait_for", &json!({"port": 80, "host": "example.com"})).unwrap_err().contains("this machine"));
    let free = { let s = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); s.local_addr().unwrap().port() };
    tools.call("wait_for", &json!({"port": free, "label": "dev server"})).unwrap();
    let _server = std::net::TcpListener::bind(("127.0.0.1", free)).unwrap();
    l.sync(&feed(&t));
    assert!(tools.take_notes().iter().any(|n| n.contains("dev server") && n.contains("accepts connections")));
    // a running job, with the long-poll variant that blocks until it lands
    let id = start_job(&jobs, &t, "slow", "sleep 0.5; echo slow done");
    let t0 = Instant::now();
    let r = tools.call("wait_for", &json!({"job_id": id, "block_seconds": 30})).unwrap();
    assert!(r.contains("slow") && r.contains("finished") && t0.elapsed() < Duration::from_secs(20), "{r}");
    assert!(tools.take_notes().is_empty(), "the long-poll delivered it");
    assert!(tools.call("wait_for", &json!({})).is_err());
    assert!(tools.call("wait_for", &json!({"job_id": id})).unwrap().contains("already over"));
}

#[test]
fn pending_list_cancel_and_settings_work_by_chat_and_a_cancelled_job_is_stopped() {
    let t = Tmp::new("tools");
    let (tools, l, jobs) = stack(&t);
    let id = start_job(&jobs, &t, "forever", "sleep 60");
    let list = tools.call("pending_list", &json!({})).unwrap();
    assert!(list.contains("forever") && list.contains("running") && list.contains("auto-continue on"), "{list}");
    let tid = l.list("")[0].id.clone();
    let r = tools.call("pending_cancel", &json!({"id": tid})).unwrap();
    assert!(r.contains("job was stopped"), "{r}");
    assert!(!jobs.get("alice", &id).unwrap().running());
    assert!(tools.call("pending_cancel", &json!({"id": tid})).is_err());
    assert!(l.take_wake("", 0, None).is_none(), "the one who cancelled needs no wake");
    let s = tools.call("pending_settings", &json!({"auto_continue": false, "max_wakes_per_day": 5, "max_wait_hours": 2})).unwrap();
    assert!(s.contains("auto-continue off") && s.contains("5 wakes") && s.contains("2h 00m"), "{s}");
    assert_eq!(l.settings(), Settings { auto_continue: false, max_wakes_per_day: 5, max_wait_secs: 7200 });
    assert!(tools.call("pending_list", &json!({})).unwrap().contains("auto-continue off"));
}

// ------------------------------------------------------------------------------------------------ the gate and the whole story

struct Script {
    turns: Mutex<VecDeque<ModelTurn>>,
    seen: Mutex<Vec<Vec<Message>>>,
}
impl Script {
    fn new(turns: Vec<ModelTurn>) -> Arc<Script> {
        Arc::new(Script { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]) })
    }
    fn say(t: &str) -> ModelTurn {
        ModelTurn { text: t.into(), calls: vec![] }
    }
    fn call(name: &str, args: Value) -> ModelTurn {
        ModelTurn { text: String::new(), calls: vec![ToolCall { id: format!("c-{name}"), name: name.into(), args }] }
    }
}
impl ChatBackend for Script {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("unused".into())
    }
    fn complete_tools(&self, h: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push(h.to_vec());
        self.turns.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
    }
}

fn assist() -> Assist {
    let mut a = Assist::standard(ReminderCtx { agent_on: true, browser_policy: "off".into(), engine_installed: false, screen_available: false, loaded: None });
    a.lazy_groups = false;
    a.stream = false;
    a
}

fn run(b: Arc<Script>, tools: Arc<dyn Tools>, ask: &str, auto: bool) -> Vec<RunEvent> {
    let h = start_with(b, tools, vec![Message::system("SYS"), Message::user(ask)], RunLimits { auto, ..RunLimits::default() }, assist());
    let mut out = vec![];
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if let Ok(e) = h.events.recv_timeout(Duration::from_millis(50)) {
            let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
            out.push(e);
            if done {
                break;
            }
        }
    }
    out
}

fn final_text(ev: &[RunEvent]) -> String {
    match ev.last() {
        Some(RunEvent::Final { text }) => text.clone(),
        other => panic!("{other:?} in {ev:?}"),
    }
}

#[test]
fn the_reported_failure_a_run_that_promises_to_report_after_job_start_shows_waiting_and_is_continued_when_the_job_ends() {
    let t = Tmp::new("story");
    let (tools, l, jobs) = stack(&t);
    let w = Waker::start_every(l.clone(), feed(&t), Duration::from_millis(30));
    // turn 1: the model starts a build and promises to report back
    let b = Script::new(vec![Script::call("job_start", json!({"command": "sleep 1; echo build ok", "label": "release build"})), Script::say("The build is running. I'll report back when it finishes.")]);
    let ev = run(b.clone(), tools.clone(), "build the release", true);
    assert!(final_text(&ev).contains("I'll report back"));
    assert_eq!(b.seen.lock().unwrap().len(), 2, "no nudge: something is pending, so the promise is kept");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.starts_with("Waiting for: release build"))), "the chat shows the waiting state: {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("continuing"))), "the stall guard did not push the model to act");
    // the session is idle; the job ends; the waker has a turn ready
    let end = Instant::now() + Duration::from_secs(15);
    while !w.ready() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(w.ready(), "the waker noticed the job ending");
    let Some(Wake::Turn { text, .. }) = w.take() else { panic!() };
    // turn 2: the synthetic turn reaches the model with the result, and its follow-up reaches the chat
    let b2 = Script::new(vec![Script::say("The release build finished and printed build ok.")]);
    let ev2 = run(b2.clone(), tools.clone(), &text, true);
    assert_eq!(final_text(&ev2), "The release build finished and printed build ok.");
    let seen = b2.seen.lock().unwrap()[0].clone();
    assert!(seen.iter().any(|m| m.role == MsgRole::User && m.content.contains("release build") && m.content.contains("build ok")), "the model got the result");
    assert!(!seen.iter().any(|m| m.content.contains("JOB FINISHED")), "and was not told twice");
    drop(jobs);
}

#[test]
fn a_promise_with_nothing_pending_gets_exactly_one_nudge_then_the_answer_is_corrected() {
    let t = Tmp::new("nudge");
    let (tools, _, _) = stack(&t);
    let b = Script::new(vec![Script::say("Done for now. I'll report back when I have news."), Script::say("I will let you know as soon as it changes.")]);
    let ev = run(b.clone(), tools.clone(), "watch the thing", false);
    let text = final_text(&ev);
    assert!(text.ends_with(NO_REPORT_NOTICE), "{text}");
    let seen = b.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "exactly one retry");
    assert!(seen[1].last().unwrap().content.contains("nothing is pending"));
    drop(seen);
    // a model that registers what it waits for after the nudge is believed
    let b = Script::new(vec![Script::say("I'll let you know when the server is up."), Script::call("wait_for", json!({"port": 1, "host": "localhost", "label": "server up", "timeout_seconds": 60})), Script::say("Waiting for the server to open its port.")]);
    let ev = run(b, tools, "start it", false);
    assert!(!final_text(&ev).contains("Note from IYAKKI"), "{ev:?}");
    // an honest answer is left alone
    let t2 = Tmp::new("nudge2");
    let (tools2, _, _) = stack(&t2);
    let b = Script::new(vec![Script::say("All 12 tests passed.")]);
    assert_eq!(final_text(&run(b.clone(), tools2, "run tests", false)), "All 12 tests passed.");
    assert_eq!(b.seen.lock().unwrap().len(), 1);
}

#[test]
fn a_woken_turn_gets_no_approval_for_free() {
    let t = Tmp::new("approval");
    let (tools, _, _) = stack(&t);
    let b = Script::new(vec![Script::call("job_start", json!({"command": "echo again"})), Script::say("started")]);
    let h = start_with(b, tools, vec![Message::system("SYS"), Message::user("[IYAKKI] Work you were waiting for has ended.")], RunLimits::default(), assist());
    let mut asked = false;
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        match h.events.recv_timeout(Duration::from_millis(50)) {
            Ok(RunEvent::Approval { name, .. }) => {
                asked = name == "job_start";
                break;
            }
            Ok(RunEvent::Final { .. }) => break,
            _ => {}
        }
    }
    assert!(asked, "the woken turn asks like any other");
    h.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = Verdict::Deny;
}
