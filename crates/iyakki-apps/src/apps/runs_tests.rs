//! The RUNS tile in the shell: the real run store of a made-up account, the conversion of the records to the tile's plain types and the
//! old behaviour tests (the tile's own tests on a fake port are in `crates/iyakki-tile-runlog`).

use iyakki_agent::agent::CallInfo;
use iyakki_shared::agent::RunEvent;
use iyakki_shared::runs::{Integrity, RunLog, RunMeta};
use serde_json::json;

use super::runs::*;
use super::runs_port::{to_log, Bridge, RunsPort};
use super::tests::cx_with_admin_store;
use super::{App, Flow};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};


fn log(user: &str, id: &str, started: u64) -> RunLog {
    RunLog {
        id: id.into(),
        session: None,
        task_id: None,
        row: None,
        agent: None,
        user: user.into(),
        started,
        prompt: "tidy my notes".into(),
        status: "completed".into(),
        events: vec![
            RunEvent::Model { text: "Reading.".into(), calls: vec![CallInfo { id: "c1".into(), name: "read_file".into(), args: json!({"path": "n.txt"}) }] },
            RunEvent::Decision { id: "c1".into(), name: "read_file".into(), approved: true, by: "auto".into() },
            RunEvent::ToolResult { id: "c1".into(), name: "read_file".into(), ok: true, text: "line one\nline two".into() },
            RunEvent::Approval { id: "c2".into(), name: "write_file".into(), summary: "write 3 characters to /home/amy/n.txt".into(), detail: "@@ line 1 @@\n-old\n+new".into() },
            RunEvent::Decision { id: "c2".into(), name: "write_file".into(), approved: false, by: "user".into() },
            RunEvent::Final { text: "I left the file alone.".into() },
        ],
    }
}

#[test]
fn timeline_is_readable_and_honest_about_decisions() {
    let t: Vec<String> = timeline(&to_log(&log("amy", "aaaaaaaaaaaa", 1))).into_iter().map(|(t, _)| t).collect();
    assert_eq!(t[0], "You: tidy my notes");
    assert!(t.iter().any(|l| l.starts_with("→ asks to use read_file")));
    assert!(t.contains(&"  ✓ allowed read_file automatically (safe tool)".to_string()));
    assert!(t.contains(&"  ✗ denied write_file by you".to_string()));
    assert!(t.iter().any(|l| l.contains("waiting for approval: write 3 characters to /home/amy/n.txt")));
    assert!(t.iter().any(|l| l.contains("result read_file: line one line two")), "{t:?}");
    assert_eq!(t.last().unwrap(), "Answer: I left the file alone.");
}

#[test]
fn lists_views_verifies_copies_and_deletes_only_my_runs() {
    let _clip = iyakki_testkit::process_lock(iyakki_testkit::names::CLIPBOARD); // the clipboard is one per process
    let mut cx = cx_with_admin_store("runs1");
    cx.session.user = "amy".into();
    let store = &cx.services.runs();
    store.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
    store.save(&log("amy", "bbbbbbbbbbbb", 200)).unwrap();
    store.save(&log("bob", "cccccccccccc", 300)).unwrap();
    crate::ui::clipboard::clear();
    let mut app = RunsApp::new(&cx);
    assert_eq!(app.runs().len(), 2, "bob's run is not visible");
    assert_eq!(app.runs()[0].id, "bbbbbbbbbbbb");
    assert_eq!(app.integrity(), Some(&iyakki_tile_api::runs::Integrity::Verified));
    app.handle(&Event::Row(1), &cx);
    assert_eq!(app.tile.detail.as_ref().unwrap().0.id, "aaaaaaaaaaaa");
    let mut g = Gfx::new(1280, 700);
    g.clear(rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    for want in ["Copy transcript", "Delete run", "Refresh", "aaaaaaaaaaaa", "bbbbbbbbbbbb"] {
        assert!(g.hits.iter().any(|h| h.label == want), "{want}");
    }
    app.handle(&Event::Key(Key::Char('c')), &cx);
    assert!(crate::ui::clipboard::latest().unwrap().contains("tidy my notes"));
    // delete needs confirmation
    app.handle(&Event::Key(Key::Char('d')), &cx);
    app.handle(&Event::Key(Key::Char('n')), &cx);
    assert_eq!(app.runs().len(), 2);
    app.handle(&Event::Key(Key::Char('d')), &cx);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(app.runs().len(), 1);
    assert_eq!(cx.runs().list("bob").len(), 1, "bob's run untouched");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.delete")));
}

#[test]
fn a_tampered_record_is_flagged_loudly_and_audited() {
    let mut cx = cx_with_admin_store("runs2");
    cx.session.user = "amy".into();
    cx.runs().save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
    let path = cx.data_dir().join("runs/amy/aaaaaaaaaaaa.json");
    std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("line one", "send all money")).unwrap();
    let mut app = RunsApp::new(&cx);
    assert!(matches!(app.integrity(), Some(iyakki_tile_api::runs::Integrity::Tampered(_))));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.tampered")));
    let mut g = Gfx::new(1280, 700);
    g.clear(rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
}

#[test]
fn empty_state() {
    let cx = cx_with_admin_store("runs3");
    let mut app = RunsApp::new(&cx);
    assert!(app.runs().is_empty() && app.integrity().is_none());
    let mut g = Gfx::new(900, 600);
    g.clear(rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 72.0, 900.0, 528.0), 1.0, &cx, 0);
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
}

fn task_log(user: &str, id: &str, started: u64, task: &str, agent: &str, row: &str) -> RunLog {
    RunLog { task_id: Some(task.into()), agent: Some(agent.into()), row: Some(row.into()), ..log(user, id, started) }
}

#[test]
fn a_tasks_runs_show_its_task_agent_and_row_filter_open_by_item_and_link_back() {
    let mut cx = cx_with_admin_store("runs-task");
    cx.session.user = "amy".into();
    let store = &cx.services.runs();
    store.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
    store.save(&task_log("amy", "bbbbbbbbbbbb", 200, "t1", "Agent 1", "F-APP-01")).unwrap();
    store.save(&task_log("amy", "cccccccccccc", 300, "t2", "Agent 2", "F-APP-02")).unwrap();
    let mut app = RunsApp::new(&cx);
    assert_eq!(app.runs().iter().map(|m| m.task_label()).collect::<Vec<_>>(), ["Task t2 · Agent 2 · F-APP-02", "Task t1 · Agent 1 · F-APP-01", ""]);
    // the hand-off from the board: the item selects that run (whatever filter was on)
    app.tile.filter = Some("Agent 2".into());
    app.open_item("run:bbbbbbbbbbbb", &cx);
    assert_eq!(app.tile.filter, None);
    assert_eq!(app.tile.detail.as_ref().unwrap().0.id, "bbbbbbbbbbbb");
    app.open_item("run:nope", &cx);
    assert!(app.tile.status.as_ref().is_some_and(|s| !s.1));
    // the filter cycles agents, then tasks, then all
    app.handle(&Event::Key(Key::Char('f')), &cx);
    assert_eq!((app.tile.filter.clone(), app.runs().len()), (Some("Agent 1".into()), 1));
    app.handle(&Event::Key(Key::Char('f')), &cx);
    assert_eq!(app.tile.filter.as_deref(), Some("Agent 2"));
    app.handle(&Event::Key(Key::Char('f')), &cx);
    assert_eq!(app.tile.filter.as_deref(), Some("t1"));
    for _ in 0..2 {
        app.handle(&Event::Key(Key::Char('f')), &cx);
    }
    assert_eq!((app.tile.filter.clone(), app.runs().len()), (None, 3));
    // from a task's run back to its task and its row; a run that is not a task's has no such buttons
    app.open_item("run:cccccccccccc", &cx);
    app.handle(&Event::Key(Key::Char('g')), &cx);
    assert_eq!(app.take_request(), Some(("ide".into(), "task:t2".into())));
    app.handle(&Event::Key(Key::Char('b')), &cx);
    assert_eq!(app.take_request(), Some(("ide".into(), "row:F-APP-02".into())));
    app.open_item("run:aaaaaaaaaaaa", &cx);
    app.handle(&Event::Key(Key::Char('g')), &cx);
    assert_eq!(app.take_request(), None);
}

#[test]
fn the_runs_screen_with_task_runs_fits_the_four_sizes_and_offers_the_links() {
    let mut cx = cx_with_admin_store("runs-geom");
    cx.session.user = "amy".into();
    for i in 0..6 {
        cx.runs().save(&task_log("amy", &format!("{i:012x}"), 100 + i as u64, &format!("t{i}"), &format!("Agent {}", i % 3 + 1), "F-APP-01")).unwrap();
    }
    let mut app = RunsApp::new(&cx);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = crate::ui::scale_for(w, h);
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &cx, 0);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            assert!(hit.rect.x >= -0.5 && hit.rect.y >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, hit.rect);
        }
        for want in ["Show task", "Show row"] {
            assert!(g.hits.iter().any(|x| x.label == want || x.label.starts_with(want)), "{w}x{h}: {want}");
        }
    }
}

/// A run store in memory: the fake port of the tile.
struct FakePort {
    logs: std::cell::RefCell<Vec<(RunLog, Integrity)>>,
    audit: std::cell::RefCell<Vec<(String, String)>>,
    refuse_delete: bool,
}

impl FakePort {
    fn new(logs: Vec<(RunLog, Integrity)>) -> FakePort {
        FakePort { logs: std::cell::RefCell::new(logs), audit: Default::default(), refuse_delete: false }
    }
    fn meta(l: &RunLog) -> RunMeta {
        RunMeta { id: l.id.clone(), started: l.started, prompt: l.prompt.clone(), status: l.status.clone(), tool_calls: 1, model_ms: 0, tools_ms: 0, overhead_ms: 0, first_token_ms: 0, total_ms: 0, task_id: l.task_id.clone(), row: l.row.clone(), agent: l.agent.clone() }
    }
}

impl RunsPort for FakePort {
    fn list(&self) -> Vec<RunMeta> {
        let mut v: Vec<RunMeta> = self.logs.borrow().iter().map(|(l, _)| FakePort::meta(l)).collect();
        v.sort_by(|a, b| b.started.cmp(&a.started));
        v
    }
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)> {
        self.logs.borrow().iter().find(|(l, _)| l.id == id).cloned()
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        if self.refuse_delete {
            return Err("The record is read-only.".into());
        }
        self.logs.borrow_mut().retain(|(l, _)| l.id != id);
        Ok(())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.into(), subject.into()));
    }
}

#[test]
fn fake_port_the_tile_lists_newest_first_opens_the_first_run_and_says_nothing_to_the_audit_log() {
    let port = FakePort::new(vec![(log("amy", "aaaaaaaaaaaa", 100), Integrity::Verified), (log("amy", "bbbbbbbbbbbb", 200), Integrity::Verified)]);
    let app = RunsApp::with_port(&port);
    assert_eq!(app.runs().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["bbbbbbbbbbbb", "aaaaaaaaaaaa"]);
    assert_eq!(app.integrity(), Some(&iyakki_tile_api::runs::Integrity::Verified));
    assert!(port.audit.borrow().is_empty());
}

#[test]
fn fake_port_a_broken_chain_is_audited_once_per_opening() {
    let port = FakePort::new(vec![(log("amy", "aaaaaaaaaaaa", 100), Integrity::Tampered("line 3 does not follow line 2".into()))]);
    let app = RunsApp::with_port(&port);
    assert!(matches!(app.integrity(), Some(iyakki_tile_api::runs::Integrity::Tampered(_))));
    let a = port.audit.borrow();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].0, "runs.tampered");
    assert!(a[0].1.starts_with("aaaaaaaaaaaa: line 3"));
}

#[test]
fn fake_port_delete_asks_the_port_audits_and_reads_the_list_again() {
    let port = FakePort::new(vec![(log("amy", "aaaaaaaaaaaa", 100), Integrity::Verified), (log("amy", "bbbbbbbbbbbb", 200), Integrity::Verified)]);
    let mut app = RunsApp::with_port(&port);
    app.tile.delete_selected(&Bridge::runs_only(&port));
    assert_eq!(app.runs().len(), 1);
    assert_eq!(app.runs()[0].id, "aaaaaaaaaaaa");
    assert_eq!(app.status(), Some(&("Run deleted.".to_string(), true)));
    assert_eq!(port.audit.borrow().as_slice(), [("runs.delete".to_string(), "bbbbbbbbbbbb".to_string())]);
}

#[test]
fn fake_port_a_refused_delete_says_why_and_changes_nothing() {
    let mut port = FakePort::new(vec![(log("amy", "aaaaaaaaaaaa", 100), Integrity::Verified)]);
    port.refuse_delete = true;
    let mut app = RunsApp::with_port(&port);
    app.tile.delete_selected(&Bridge::runs_only(&port));
    assert_eq!(app.runs().len(), 1);
    assert_eq!(app.status(), Some(&("The record is read-only.".to_string(), false)));
    assert!(port.audit.borrow().is_empty());
}

#[test]
fn fake_port_the_filter_cycles_through_agents_and_tasks_and_back_to_all() {
    let port = FakePort::new(vec![
        (task_log("amy", "aaaaaaaaaaaa", 100, "t1", "Agent 1", "F-APP-01"), Integrity::Verified),
        (task_log("amy", "bbbbbbbbbbbb", 200, "t2", "Agent 2", "F-APP-02"), Integrity::Verified),
        (log("amy", "cccccccccccc", 300), Integrity::Verified),
    ]);
    let mut app = RunsApp::with_port(&port);
    assert_eq!(app.runs().len(), 3);
    app.tile.cycle_filter(&Bridge::runs_only(&port));
    assert_eq!(app.runs().len(), 1, "first value: Agent 1");
    assert_eq!(app.runs()[0].id, "aaaaaaaaaaaa");
    let mut seen = 1;
    while app.tile.filter.is_some() && seen < 10 {
        app.tile.cycle_filter(&Bridge::runs_only(&port));
        seen += 1;
    }
    assert!(app.tile.filter.is_none(), "the cycle ends on all runs");
    assert_eq!(app.runs().len(), 3);
}

// ---- the mirror of the tile crate: the plain records keep what the screens read -----------------------------------------------------------

#[test]
fn the_plain_run_events_of_the_tile_api_mirror_the_shared_ones() {
    use super::runs_port::{to_event, to_integrity, to_meta};
    use iyakki_tile_api::runs as api;
    let l = log("amy", "aaaaaaaaaaaa", 1);
    let mirrored = to_log(&l);
    assert_eq!((mirrored.id.as_str(), mirrored.prompt.as_str()), ("aaaaaaaaaaaa", "tidy my notes"));
    assert_eq!(mirrored.events.len(), l.events.len());
    match &mirrored.events[0] {
        api::RunEvent::Model { text, calls } => {
            assert_eq!(text, "Reading.");
            assert_eq!(calls, &[api::Call { name: "read_file".into(), args_text: "{\"path\":\"n.txt\"}".into() }]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(mirrored.events[1], api::RunEvent::Decision { name: "read_file".into(), approved: true, by: "auto".into() });
    assert_eq!(mirrored.events[3], api::RunEvent::Approval { summary: "write 3 characters to /home/amy/n.txt".into(), detail: "@@ line 1 @@\n-old\n+new".into() });
    assert_eq!(mirrored.events[5], api::RunEvent::Final { text: "I left the file alone.".into() });
    // the events the record screen never draws become one hidden event; the others keep their numbers
    assert_eq!(to_event(&RunEvent::Delta { text: "x".into() }), api::RunEvent::Hidden);
    assert_eq!(to_event(&RunEvent::Thinking { text: "x".into(), secs: 2 }), api::RunEvent::Hidden);
    assert_eq!(to_event(&RunEvent::Usage { tokens: 5, cost_cents: 1.5, files: 2, seconds: 3 }), api::RunEvent::Usage { tokens: 5, cost_cents: 1.5, files: 2, seconds: 3 });
    assert_eq!(to_event(&RunEvent::Note { text: "retry".into() }), api::RunEvent::Note { text: "retry".into() });
    assert_eq!(to_event(&RunEvent::Failed { error: "e".into() }), api::RunEvent::Failed { error: "e".into() });
    assert_eq!(to_event(&RunEvent::Stopped { reason: "r".into() }), api::RunEvent::Stopped { reason: "r".into() });
    let timing = RunEvent::Timing { first_token_ms: 1, first_text_ms: 2, total_ms: 3, model_ms: 4, tools_ms: 5, overhead_ms: 6, pre_request_ms: 7, prepare_ms: 8, thinking_ms: 9, rounds: 10 };
    assert_eq!(to_event(&timing), api::RunEvent::Timing { first_token_ms: 1, first_text_ms: 2, total_ms: 3, model_ms: 4, tools_ms: 5, overhead_ms: 6, thinking_ms: 9 });
    assert_eq!(to_integrity(&Integrity::Verified), api::Integrity::Verified);
    assert_eq!(to_integrity(&Integrity::Tampered("why".into())), api::Integrity::Tampered("why".into()));
    // the meta keeps the task label
    let m = FakePortMeta::of(&task_log("amy", "bbbbbbbbbbbb", 2, "t1", "Agent 1", "F-APP-01"));
    assert_eq!(to_meta(&m).task_label(), m.task_label());
    assert_eq!(m.task_label(), "Task t1 · Agent 1 · F-APP-01");
}

struct FakePortMeta;
impl FakePortMeta {
    fn of(l: &RunLog) -> RunMeta {
        RunMeta { id: l.id.clone(), started: l.started, prompt: l.prompt.clone(), status: l.status.clone(), tool_calls: 1, model_ms: 0, tools_ms: 0, overhead_ms: 0, first_token_ms: 0, total_ms: 0, task_id: l.task_id.clone(), row: l.row.clone(), agent: l.agent.clone() }
    }
}

#[test]
fn the_shell_bridge_without_a_context_has_no_jobs_and_an_unknown_job_is_refused() {
    use iyakki_tile_api::runs::{JobsPort, RunsPort as _};
    let port = FakePort::new(vec![]);
    let b = Bridge::runs_only(&port);
    assert!(b.visible(None).is_empty() && b.take_finished("w").is_empty());
    assert_eq!(b.log_len("j1"), 0);
    assert!(b.log_text("j1", 10).is_err() && b.dismiss("j1").is_err());
    b.stop_job("j1")();
    iyakki_tile_api::runs::RunsPort::audit(&b, "x", "y");
    assert_eq!(port.audit.borrow().as_slice(), [("x".to_string(), "y".to_string())]);
    assert!(b.list().is_empty());
}

#[test]
fn the_plain_job_of_the_tile_api_mirrors_the_shared_job_and_its_states() {
    use super::jobs_ui::{to_job, to_state};
    use iyakki_shared::jobs::{Job, State};
    use iyakki_tile_api::runs::{self as api, JobState};
    let pairs = [(State::Running, JobState::Running), (State::Succeeded, JobState::Succeeded), (State::Failed(2), JobState::Failed(2)), (State::Stopped, JobState::Stopped), (State::Lost, JobState::Lost)];
    for (a, b) in &pairs {
        assert_eq!(&to_state(a), b);
        assert_eq!(a.label(), b.label(), "the words of a state are the same on both sides");
    }
    assert_eq!(State::Failed(iyakki_shared::jobs::TIMED_OUT_EXIT).label(), JobState::Failed(api::TIMED_OUT_EXIT).label());
    assert_eq!(iyakki_shared::jobs::TIMED_OUT_EXIT, api::TIMED_OUT_EXIT);
    assert_eq!(iyakki_shared::jobs::STOP_GRACE, api::STOP_GRACE);
    let j = Job { id: "j1".into(), label: "build".into(), command: "iyakki quick-install registry".into(), cwd: "/".into(), owner: "amy".into(), workspace: "w".into(), pid: 7, state: State::Running, started_ms: 1_000, ended_ms: 0, progress: Some(40.0), last_line: "40%".into(), last_active_ms: 5_000, dismissed: false };
    let m = to_job(&j);
    assert_eq!((m.id.as_str(), m.label.as_str(), m.command.as_str(), m.progress, m.last_line.as_str()), ("j1", "build", "iyakki quick-install registry", Some(40.0), "40%"));
    // the same arithmetic on both sides
    for now in [1_000u64, 6_000, 90_000, 4_000_000] {
        assert_eq!((m.elapsed_secs(now), m.quiet_secs(now)), (j.elapsed_secs(now), j.quiet_secs(now)));
    }
    for s in [0u64, 59, 60, 3599, 3600, 90_000] {
        assert_eq!(api::fmt_duration(s), iyakki_shared::jobs::fmt_duration(s));
    }
    let done = Job { state: State::Failed(3), ended_ms: 9_000, ..j.clone() };
    assert_eq!(api::finish_line(&to_job(&done), 9_000), iyakki_shared::jobs::finish_line(&done).replace(&format!("after {}", iyakki_shared::jobs::fmt_duration(done.elapsed_secs(iyakki_shared::jobs::now_ms()))), "after 8s"));
    // the command the shell tells an install job by
    assert_eq!(iyakki_shared::quickinstall::item_of_command(&m.command), iyakki_shared::quickinstall::item_of(&j));
}
