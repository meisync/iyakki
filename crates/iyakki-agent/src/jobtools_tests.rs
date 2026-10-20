//! The job tools, their approvals, the notes a run gets when a job finishes, and Autopilot waiting for a running job.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_sec::policy::{CliLevel, Policy, Role};
use serde_json::{json, Value};

use crate::agent::{hard_ask, start_with, Risk, RunEvent, RunLimits, ToolDef, Tools};
use crate::assistrun::Assist;
use crate::autopilot::{wait_for_jobs, Config, Rows, State, StepEnd};
use crate::chat::{ChatBackend, Message, ModelTurn, Role as MsgRole, ToolSpec};
use crate::jobs::{Jobs, State as JobState};
use crate::jobtools::{is_job_tool, JobTools, JOB_TOOLS};

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-jobtools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("work/sub")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        Tmp(d)
    }
    fn work(&self) -> PathBuf {
        std::fs::canonicalize(self.0.join("work")).unwrap()
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Another tool set underneath (a stand-in for the user's tools).
struct Inner;
impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![ToolDef { spec: ToolSpec { name: "now".into(), description: "time".into(), schema: json!({"type": "object"}) }, risk: Risk::Safe, untrusted: false }]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Ok(format!("inner {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        format!("inner {name}")
    }
}

fn policy_for(work: &Path) -> Policy {
    let mut p = Role::Admin.template();
    p.cli = CliLevel::Full;
    p.fs_read = vec![work.display().to_string()];
    p.fs_write = vec![work.display().to_string()];
    p
}

fn tools(t: &Tmp, p: Policy, ws: Option<(&str, &Path)>) -> (JobTools, Jobs) {
    let jobs = Jobs::new(&t.0.join("jobs"));
    (JobTools::new(Arc::new(Inner), jobs.clone(), "alice", p, &t.work(), ws), jobs)
}

#[test]
fn the_six_tools_are_offered_with_the_right_risks_and_only_to_accounts_that_may_run_commands() {
    let t = Tmp::new("defs");
    let (jt, _) = tools(&t, policy_for(&t.work()), None);
    let defs = jt.defs();
    let risk = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap_or_else(|| panic!("{n} missing")).risk;
    assert_eq!((risk("job_start"), risk("job_stop")), (Risk::Ask, Risk::Ask), "starting and stopping ask");
    for n in ["job_status", "job_tail", "job_list", "job_wait"] {
        assert_eq!(risk(n), Risk::Safe, "{n}");
    }
    assert!(defs.iter().any(|d| d.spec.name == "now"), "the other tools stay");
    assert_eq!(JOB_TOOLS.iter().filter(|n| defs.iter().any(|d| &d.spec.name == *n)).count(), 6);
    // a person who may not run commands gets none, and a forced call is refused
    let mut low = policy_for(&t.work());
    low.cli = CliLevel::Restricted;
    let (jt2, _) = tools(&t, low, None);
    assert!(!jt2.defs().iter().any(|d| is_job_tool(&d.spec.name)));
    assert!(jt2.call("job_start", &json!({"command": "true"})).unwrap_err().contains("may not run commands"));
    assert!(jt2.take_notes().is_empty() && jt2.jobs_running().is_empty());
    // an external agent reaches them through the bridge: the same set, nothing hidden
    for n in JOB_TOOLS {
        let d = defs.iter().find(|d| d.spec.name == n).unwrap();
        assert!(crate::extagent::bridge::offered(d), "{n} is offered through the bridge");
    }
}

#[test]
fn starting_a_job_asks_and_in_auto_still_asks_for_what_run_command_always_asks_for() {
    // approval prompts: the same rules as run_command
    assert!(hard_ask("job_start", &json!({"command": "sdkmanager \"emulator\""})).is_none(), "an ordinary install runs in Auto");
    assert!(hard_ask("job_start", &json!({"command": "yes | sdkmanager --licenses && avdmanager create avd -n x -k y"})).is_none());
    assert!(hard_ask("job_start", &json!({"command": "make clean; rm -rf /"})).is_some());
    assert!(hard_ask("job_start", &json!({"command": "sudo apt install x"})).is_some());
    assert!(hard_ask("job_start", &json!({"command": "cat /etc/shadow"})).is_some(), "outside the workspace");
    assert!(hard_ask("job_start", &json!({"argv": ["rm", "x"]})).is_some());
    assert!(hard_ask("job_start", &json!({"argv": ["cargo", "build"]})).is_none());
    assert!(hard_ask("job_status", &json!({})).is_none() && hard_ask("job_stop", &json!({"id": "j1"})).is_none(), "stop is a plain Ask, not a hard one");
    let t = Tmp::new("always");
    let (jt, _) = tools(&t, policy_for(&t.work()), None);
    assert!(!jt.approved_always("job_start") && !jt.approved_always("job_stop"), "every start asks, whatever 'always' was said for others");
    assert!(jt.approved_always("now"));
    assert!(jt.describe("job_start", &json!({"command": "sleep 5", "label": "nap"})).contains("start a detached job “nap”"));
    let pv = jt.preview("job_start", &json!({"command": "sleep 5"})).unwrap();
    assert!(pv.contains("sleep 5") && pv.contains("detached"), "{pv}");
}

#[test]
fn start_status_tail_list_wait_and_stop_work_end_to_end() {
    let t = Tmp::new("flow");
    let (jt, jobs) = tools(&t, policy_for(&t.work()), None);
    let out = jt.call("job_start", &json!({"command": "echo step one; sleep 0.4; echo step two", "label": "demo"})).unwrap();
    assert!(out.contains("Started job j1 “demo”") && out.contains("job_wait"), "{out}");
    assert!(jt.jobs_running().iter().any(|l| l.contains("demo")), "{:?}", jt.jobs_running());
    assert!(jt.call("job_list", &json!({})).unwrap().contains("j1 · demo · running"));
    let w = jt.call("job_wait", &json!({"id": "j1", "timeout_seconds": 600})).unwrap();
    assert!(w.contains("succeeded") && w.contains("Finished.") && w.contains("step two"), "{w}");
    assert!(jt.take_notes().is_empty(), "the assistant already has the result from job_wait: no note follows");
    assert!(jt.jobs_running().is_empty());
    let tail = jt.call("job_tail", &json!({"id": "j1", "lines": 1})).unwrap();
    assert!(tail.ends_with("step two") && !tail.contains("step one"), "{tail}");
    assert!(jt.call("job_status", &json!({"id": "j1"})).unwrap().contains("command: echo step one"));
    // a still-running job: wait gives up politely, stop ends it
    jt.call("job_start", &json!({"argv": ["sleep", "60"]})).unwrap();
    let w = jt.call("job_wait", &json!({"id": "j2", "timeout_seconds": 1})).unwrap();
    assert!(w.contains("Still running"), "{w}");
    let s = jt.call("job_stop", &json!({"id": "j2"})).unwrap();
    assert!(s.contains("stopped"), "{s}");
    assert_eq!(jobs.get("alice", "j2").unwrap().state, JobState::Stopped);
    assert!(jt.call("job_tail", &json!({})).unwrap_err().contains("id"));
    assert!(jt.call("job_status", &json!({"id": "j99"})).unwrap_err().contains("no job"));
    assert!(jt.call("job_start", &json!({})).is_err());
    assert!(jt.call("job_start", &json!({"command": "true", "argv": ["true"]})).is_err());
}

#[test]
fn a_finished_job_reaches_the_assistant_once_as_a_note_and_secrets_are_not_in_it() {
    let t = Tmp::new("notes");
    let (jt, jobs) = tools(&t, policy_for(&t.work()), None);
    jt.call("job_start", &json!({"command": "echo 'API_TOKEN=supersecretvalue123'; echo built it; exit 3", "label": "build"})).unwrap();
    jobs.wait("alice", "j1", Duration::from_secs(20)).unwrap();
    let notes = jt.take_notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("JOB FINISHED: build (j1) failed (exit 3)") && notes[0].contains("built it"), "{}", notes[0]);
    assert!(!notes[0].contains("supersecretvalue123"), "{}", notes[0]);
    assert!(jt.take_notes().is_empty(), "once");
}

#[test]
fn the_working_folder_must_be_inside_the_scope_and_inside_the_workspace_in_the_ide() {
    let t = Tmp::new("scope");
    let (jt, jobs) = tools(&t, policy_for(&t.work()), None);
    let outside = t.0.join("outside").display().to_string();
    let e = jt.call("job_start", &json!({"command": "touch ran", "cwd": outside})).unwrap_err();
    assert!(e.contains("permission denied"), "{e}");
    assert!(jt.refusal("job_start", &json!({"command": "touch ran", "cwd": t.0.join("outside").display().to_string()})).is_some());
    assert!(jobs.list("alice").is_empty() && !t.0.join("outside/ran").exists());
    // a folder inside the scope works, relative to the working folder
    jt.call("job_start", &json!({"command": "pwd", "cwd": "sub"})).unwrap();
    let j = jobs.wait("alice", "j1", Duration::from_secs(20)).unwrap();
    assert!(j.cwd.ends_with("work/sub"), "{}", j.cwd);
    // in the IDE the workspace root is the limit, even if the account may write elsewhere
    let mut wide = policy_for(&t.work());
    wide.fs_write = vec![t.0.display().to_string()];
    let (jt, _) = tools(&t, wide, Some(("proj", &t.work())));
    let e = jt.call("job_start", &json!({"command": "true", "cwd": "../outside"})).unwrap_err();
    assert!(e.contains("outside the workspace"), "{e}");
    assert!(jt.call("job_start", &json!({"command": "true"})).is_ok());
    let l = jt.call("job_list", &json!({})).unwrap();
    assert!(l.contains("j2") && !l.contains("j1"), "the IDE's list holds only this workspace's jobs: {l}");
}

#[test]
fn an_account_that_may_not_reach_every_host_may_not_start_a_job() {
    let t = Tmp::new("hosts");
    let mut p = policy_for(&t.work());
    p.net_hosts = vec!["api.example.com".into()];
    let (jt, jobs) = tools(&t, p, None);
    let r = jt.refusal("job_start", &json!({"command": "curl evil.example"})).unwrap();
    assert!(r.contains("every host"), "{r}");
    assert!(jt.call("job_start", &json!({"command": "true"})).is_err());
    assert!(jobs.list("alice").is_empty());
}

#[test]
fn people_cannot_see_or_touch_each_others_jobs_through_the_tools() {
    let t = Tmp::new("owners");
    let jobs = Jobs::new(&t.0.join("jobs"));
    let mk = |user: &str| JobTools::new(Arc::new(Inner), jobs.clone(), user, policy_for(&t.work()), &t.work(), None);
    let (a, b) = (mk("alice"), mk("bob"));
    a.call("job_start", &json!({"command": "sleep 30"})).unwrap();
    assert_eq!(b.call("job_list", &json!({})).unwrap(), "No jobs.");
    assert!(b.call("job_stop", &json!({"id": "j1"})).is_err());
    assert!(b.jobs_running().is_empty());
    a.call("job_stop", &json!({"id": "j1"})).unwrap();
}

#[test]
fn the_prompt_and_the_external_agent_instructions_tell_it_to_use_job_start() {
    assert!(crate::jobtools::PROMPT_RULE.contains("job_start") && crate::jobtools::PROMPT_RULE.contains("30 seconds") && crate::jobtools::PROMPT_RULE.contains("job_wait"));
    let rules = crate::extagent::run::rules("Claude Code", crate::extagent::Mode::Bridged);
    assert!(rules.contains("mcp__iyakki__job_start") && rules.contains("30 seconds") && rules.contains("until-loops"), "{rules}");
    // an advisor cannot start anything, so it is not told to
    let adv = crate::extagent::run::rules("Copilot", crate::extagent::Mode::Advisor);
    assert!(!adv.contains("job_start"));
}

// ------------------------------------------------------------------------------------------------ the run loop

struct Say {
    seen: Mutex<Vec<Vec<Message>>>,
    turns: Mutex<VecDeque<String>>,
}
impl ChatBackend for Say {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("unused".into())
    }
    fn complete_tools(&self, h: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.seen.lock().unwrap().push(h.to_vec());
        Ok(ModelTurn { text: self.turns.lock().unwrap().pop_front().unwrap_or_else(|| "ok".into()), calls: vec![] })
    }
}

fn run_once(backend: Arc<Say>, tools: Arc<dyn Tools>) -> Vec<RunEvent> {
    let h = start_with(backend, tools, vec![Message::system("SYS"), Message::user("how is the install going?")], RunLimits::default(), Assist::off());
    let mut out = vec![];
    let end = Instant::now() + Duration::from_secs(20);
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

#[test]
fn a_run_is_told_about_a_job_that_finished_since_its_last_turn_and_the_chat_shows_it_as_a_step() {
    let t = Tmp::new("nudge");
    let (jt, jobs) = tools(&t, policy_for(&t.work()), None);
    jt.call("job_start", &json!({"command": "echo emulator installed", "label": "emulator install"})).unwrap();
    jobs.wait("alice", "j1", Duration::from_secs(20)).unwrap();
    let tools: Arc<dyn Tools> = Arc::new(jt);
    let b = Arc::new(Say { seen: Mutex::new(vec![]), turns: Mutex::new(VecDeque::new()) });
    let ev = run_once(b.clone(), tools.clone());
    assert!(matches!(ev.last(), Some(RunEvent::Final { .. })), "{ev:?}");
    let first = b.seen.lock().unwrap()[0].clone();
    let note = first.iter().find(|m| m.role == MsgRole::User && m.content.starts_with("[IYAKKI] JOB FINISHED")).expect("the model was told");
    assert!(note.content.contains("emulator install (j1) succeeded") && note.content.contains("emulator installed"), "{}", note.content);
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.starts_with("JOB FINISHED: emulator install"))), "visible as a step: {ev:?}");
    // the next run is not told again
    let b2 = Arc::new(Say { seen: Mutex::new(vec![]), turns: Mutex::new(VecDeque::new()) });
    run_once(b2.clone(), tools);
    assert!(!b2.seen.lock().unwrap()[0].iter().any(|m| m.content.contains("JOB FINISHED")));
}

// ------------------------------------------------------------------------------------------------ Autopilot

fn rows() -> Rows {
    Rows { open: vec![("F-1".into(), "install the emulator".into(), "partial".into())], total: 1, done: 0, sig: "same".into(), ..Default::default() }
}

#[test]
fn a_step_that_waited_for_a_job_is_not_a_stall_and_is_never_counted_against_the_row() {
    let cfg = Config::default();
    // without a job: the same board three steps in a row stops the run
    let mut s = State::new("goal", 1000);
    s.current_row = "F-1".into();
    s.step_changed = false;
    let mut stopped = false;
    for _ in 0..6 {
        if matches!(s.after_step(&cfg, &StepEnd::Final, &rows(), 10, 1001, false), crate::autopilot::Decision::Stop(w) if w.contains("no progress")) {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the control case really stops");
    // with a job running: any number of such steps goes on, and nothing is charged to the row
    let mut s = State::new("goal", 1000);
    s.current_row = "F-1".into();
    s.step_changed = false;
    for i in 0..12 {
        s.waiting = true;
        let d = s.after_step(&cfg, &StepEnd::Final, &rows(), 10, 1001, false);
        assert!(matches!(d, crate::autopilot::Decision::Continue { .. }), "step {i}: {d:?}");
    }
    assert_eq!((s.no_progress, s.stalls), (0, 0));
    assert_eq!(s.tries.get("F-1").copied().unwrap_or(0), 0);
    // when the job is over and the model still only talks, the normal stall rules apply again
    s.waiting = false;
    let mut blocked = false;
    for _ in 0..6 {
        match s.after_step(&cfg, &StepEnd::Final, &rows(), 10, 1001, false) {
            crate::autopilot::Decision::Stop(_) | crate::autopilot::Decision::Block { .. } => {
                blocked = true;
                break;
            }
            _ => {}
        }
    }
    assert!(blocked);
}

struct Running(AtomicUsize);
impl Tools for Running {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, _: &str, _: &Value) -> Result<String, String> {
        Err("none".into())
    }
    fn describe(&self, _: &str, _: &Value) -> String {
        String::new()
    }
    fn jobs_running(&self) -> Vec<String> {
        // running for the first few polls, then done
        if self.0.fetch_sub(1, Ordering::SeqCst) > 1 { vec!["emulator install (j1)".into()] } else { vec![] }
    }
}

#[test]
fn autopilot_waits_while_a_job_runs_says_so_and_can_still_be_stopped_or_paused() {
    let notes = Mutex::new(Vec::<String>::new());
    let emit = |e: RunEvent| {
        if let RunEvent::Note { text } = e {
            notes.lock().unwrap().push(text);
        }
    };
    let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
    let t0 = Instant::now();
    let (w, why) = wait_for_jobs(&Running(AtomicUsize::new(6)), &cancel, &pause, &emit, Duration::from_secs(30));
    assert!(w.waited && why.is_none() && t0.elapsed() >= Duration::from_millis(400), "waited {:?}", t0.elapsed());
    assert!(notes.lock().unwrap()[0].contains("waiting for emulator install (j1)") && notes.lock().unwrap()[0].contains("not stalled"));
    // nothing running: no wait, no note
    let before = notes.lock().unwrap().len();
    let (w, why) = wait_for_jobs(&Running(AtomicUsize::new(0)), &cancel, &pause, &emit, Duration::from_secs(30));
    assert!(!w.waited && why.is_none() && notes.lock().unwrap().len() == before);
    // a stop and a pause end the wait at once
    cancel.store(true, Ordering::SeqCst);
    let (_, why) = wait_for_jobs(&Running(AtomicUsize::new(1000)), &cancel, &pause, &emit, Duration::from_secs(30));
    assert_eq!(why, Some("stop"));
    cancel.store(false, Ordering::SeqCst);
    pause.store(true, Ordering::SeqCst);
    let (_, why) = wait_for_jobs(&Running(AtomicUsize::new(1000)), &cancel, &pause, &emit, Duration::from_secs(30));
    assert_eq!(why, Some("pause"));
    // the cap ends a wait that would never end
    pause.store(false, Ordering::SeqCst);
    let (w, why) = wait_for_jobs(&Running(AtomicUsize::new(100_000)), &cancel, &pause, &emit, Duration::from_millis(300));
    assert!(w.waited && why.is_none());
}

#[test]
fn a_real_job_makes_the_wait_real_and_ends_when_the_job_does() {
    let t = Tmp::new("apwait");
    let (jt, _) = tools(&t, policy_for(&t.work()), None);
    jt.call("job_start", &json!({"command": "sleep 1", "label": "nap"})).unwrap();
    let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
    let t0 = Instant::now();
    let (w, why) = wait_for_jobs(&jt, &cancel, &pause, &|_| {}, Duration::from_secs(30));
    assert!(w.waited && why.is_none() && t0.elapsed() >= Duration::from_millis(500) && t0.elapsed() < Duration::from_secs(15));
    assert!(jt.jobs_running().is_empty());
}

// ------------------------------------------------------------------------------------------------ an external agent, through the bridge

fn gate_for(t: &Tmp, auto: bool) -> (Arc<crate::extagent::bridge::Gate>, std::sync::mpsc::Receiver<RunEvent>, std::sync::mpsc::Sender<(String, crate::agent::Verdict)>, Jobs) {
    use crate::extagent::bridge::{Correlator, Decisions, Gate};
    let (jt, jobs) = tools(t, policy_for(&t.work()), None);
    let (etx, erx) = std::sync::mpsc::channel();
    let (dtx, drx) = std::sync::mpsc::channel();
    let gate = Gate::new(Arc::new(jt), auto, etx, Decisions::new(drx), Arc::new(Correlator::default()), Arc::new(AtomicBool::new(false)), Duration::from_secs(20), "Claude Code", Arc::new(|_, _| {}), &t.work(), 12_000);
    (Arc::new(gate), erx, dtx, jobs)
}

/// Call a tool through the gate from a thread and answer its approval card (if it shows one) with `verdict`.
fn via_gate(gate: &Arc<crate::extagent::bridge::Gate>, erx: &std::sync::mpsc::Receiver<RunEvent>, dtx: &std::sync::mpsc::Sender<(String, crate::agent::Verdict)>, name: &str, args: Value, verdict: crate::agent::Verdict) -> (Result<String, String>, bool) {
    let g = gate.clone();
    let n = name.to_string();
    let h = std::thread::spawn(move || g.call_tool(&n, &args, None));
    let mut asked = false;
    let end = Instant::now() + Duration::from_secs(20);
    while !h.is_finished() && Instant::now() < end {
        if let Ok(RunEvent::Approval { id, .. }) = erx.recv_timeout(Duration::from_millis(20)) {
            asked = true;
            dtx.send((id, verdict)).unwrap();
        }
    }
    (h.join().unwrap(), asked)
}

#[test]
fn an_external_agent_starts_jobs_through_the_bridge_with_the_same_approvals_as_the_built_in_assistant() {
    use crate::agent::Verdict;
    let t = Tmp::new("bridge");
    let (gate, erx, dtx, jobs) = gate_for(&t, false);
    assert!(gate.defs.iter().any(|d| d.spec.name == "job_start") && gate.defs.iter().any(|d| d.spec.name == "job_wait"));
    // starting asks; a denial starts nothing
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_start", json!({"command": "echo hi", "label": "x"}), Verdict::Deny);
    assert!(asked && r.unwrap_err().contains("denied"));
    assert!(jobs.list("alice").is_empty());
    // approved: it runs, detached
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_start", json!({"command": "echo from claude; sleep 0.3", "label": "claude job"}), Verdict::Approve);
    assert!(asked && r.unwrap().contains("Started job j1"));
    // status, wait, tail and list are safe: no card
    for (n, a) in [("job_list", json!({})), ("job_status", json!({"id": "j1"})), ("job_tail", json!({"id": "j1"}))] {
        let (r, asked) = via_gate(&gate, &erx, &dtx, n, a, Verdict::Deny);
        assert!(!asked && r.is_ok(), "{n}: {r:?}");
    }
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_wait", json!({"id": "j1", "timeout_seconds": 30}), Verdict::Deny);
    assert!(!asked);
    let text = r.unwrap();
    assert!(text.contains("untrusted data") && text.contains("succeeded") && text.contains("from claude"), "{text}");
    // stop asks
    jobs.start("alice", &crate::jobs::Spec { label: "long".into(), cmd: crate::jobs::Cmd::Shell("sleep 60".into()), cwd: t.work(), workspace: String::new() }, &|_| Ok(())).unwrap();
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_stop", json!({"id": "j2"}), Verdict::Approve);
    assert!(asked && r.is_ok());
    assert_eq!(jobs.get("alice", "j2").unwrap().state, JobState::Stopped);
}

#[test]
fn in_auto_an_ordinary_job_runs_without_a_card_but_a_destructive_or_outside_command_still_asks() {
    use crate::agent::Verdict;
    let t = Tmp::new("bridge-auto");
    let (gate, erx, dtx, jobs) = gate_for(&t, true);
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_start", json!({"command": "echo auto", "label": "ordinary"}), Verdict::Deny);
    assert!(!asked && r.is_ok(), "{r:?}");
    for line in ["rm -rf build && make", "cat /etc/passwd", "sudo make install"] {
        let (r, asked) = via_gate(&gate, &erx, &dtx, "job_start", json!({"command": line}), Verdict::Deny);
        assert!(asked && r.is_err(), "{line}: asked={asked} {r:?}");
    }
    assert_eq!(jobs.list("alice").len(), 1, "only the ordinary one started");
    // a job's folder outside the scope is a refusal (no card at all)
    let (r, asked) = via_gate(&gate, &erx, &dtx, "job_start", json!({"command": "true", "cwd": t.0.join("outside").display().to_string()}), Verdict::Approve);
    assert!(!asked && r.unwrap_err().contains("permission denied"));
}
