//! The RUNS tile on a fake port: a run store and a job list in memory, an audit log that records. No clock, no process, no sleep.

use std::cell::RefCell;

use iyakki_tile_api::runs::{Call, Integrity, Job, JobState, JobsPort, RunEvent, RunLog, RunMeta, RunsPort};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

#[test]
fn formats_dates() {
    assert_eq!(fmt_time(0), "1970-01-01 00:00");
    assert_eq!(fmt_time(1_000_000_000), "2001-09-09 01:46");
    assert_eq!(fmt_time(1_791_335_444), "2026-10-07 01:10");
    assert_eq!(fmt_time(951_782_400 + 86_400 * 59 + 3600 * 13 + 60 * 7), "2000-04-28 13:07", "leap year handled (2000-02-29 exists)");
}

fn log(id: &str, started: u64) -> (RunLog, u64) {
    (
        RunLog {
            id: id.into(),
            prompt: "tidy my notes".into(),
            events: vec![
                RunEvent::Model { text: "Reading.".into(), calls: vec![Call { name: "read_file".into(), args_text: "{\"path\":\"n.txt\"}".into() }] },
                RunEvent::Decision { name: "read_file".into(), approved: true, by: "auto".into() },
                RunEvent::ToolResult { name: "read_file".into(), ok: true, text: "line one\nline two".into() },
                RunEvent::Approval { summary: "write 3 characters to /home/amy/n.txt".into(), detail: "@@ line 1 @@\n-old\n+new".into() },
                RunEvent::Decision { name: "write_file".into(), approved: false, by: "user".into() },
                RunEvent::Hidden,
                RunEvent::Final { text: "I left the file alone.".into() },
            ],
        },
        started,
    )
}

#[derive(Clone)]
struct Entry {
    log: RunLog,
    started: u64,
    integrity: Integrity,
    task: Option<(String, String, String)>,
}

struct Fake {
    runs: RefCell<Vec<Entry>>,
    audit: RefCell<Vec<(String, String)>>,
    refuse_delete: bool,
    jobs: RefCell<Vec<Job>>,
    finished: RefCell<Vec<String>>,
}

fn entry(id: &str, started: u64) -> Entry {
    let (l, s) = log(id, started);
    Entry { log: l, started: s, integrity: Integrity::Verified, task: None }
}

fn task(id: &str, started: u64, t: &str, agent: &str, row: &str) -> Entry {
    Entry { task: Some((t.into(), agent.into(), row.into())), ..entry(id, started) }
}

impl Fake {
    fn new(runs: Vec<Entry>) -> Fake {
        Fake { runs: RefCell::new(runs), audit: Default::default(), refuse_delete: false, jobs: Default::default(), finished: Default::default() }
    }
}

impl RunsPort for Fake {
    fn list(&self) -> Vec<RunMeta> {
        let mut v: Vec<RunMeta> = self
            .runs
            .borrow()
            .iter()
            .map(|e| RunMeta {
                id: e.log.id.clone(),
                started: e.started,
                prompt: e.log.prompt.clone(),
                status: "completed".into(),
                tool_calls: 1,
                task_id: e.task.as_ref().map(|t| t.0.clone()),
                agent: e.task.as_ref().map(|t| t.1.clone()),
                row: e.task.as_ref().map(|t| t.2.clone()),
            })
            .collect();
        v.sort_by(|a, b| b.started.cmp(&a.started));
        v
    }
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)> {
        self.runs.borrow().iter().find(|e| e.log.id == id).map(|e| (e.log.clone(), e.integrity.clone()))
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        if self.refuse_delete {
            return Err("The record is read-only.".into());
        }
        self.runs.borrow_mut().retain(|e| e.log.id != id);
        Ok(())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.into(), subject.into()));
    }
}

impl JobsPort for Fake {
    fn visible(&self, _ws: Option<&str>) -> Vec<Job> {
        self.jobs.borrow().clone()
    }
    fn take_finished(&self, _ws: &str) -> Vec<String> {
        std::mem::take(&mut *self.finished.borrow_mut())
    }
    fn log_len(&self, _id: &str) -> u64 {
        0
    }
    fn log_text(&self, _id: &str, _max: usize) -> Result<String, String> {
        Ok(String::new())
    }
    fn stop_job(&self, _id: &str) -> Box<dyn FnOnce() + Send> {
        Box::new(|| {})
    }
    fn dismiss(&self, id: &str) -> Result<(), String> {
        self.jobs.borrow_mut().retain(|j| j.id != id);
        Ok(())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.into(), subject.into()));
    }
}

fn job(id: &str, state: JobState) -> Job {
    Job { id: id.into(), label: format!("job {id}"), command: "sleep 30".into(), state, started_ms: 1_000, ended_ms: 0, progress: None, last_line: "line".into(), last_active_ms: 9_000 }
}

fn draw(t: &mut RunsTile, p: &Fake, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = 1.0;
    t.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), s, p, 0);
    g
}

#[test]
fn the_timeline_is_readable_and_honest_about_decisions() {
    let t: Vec<String> = timeline(&log("aaaaaaaaaaaa", 1).0).into_iter().map(|(t, _)| t).collect();
    assert_eq!(t[0], "You: tidy my notes");
    assert!(t.iter().any(|l| l.starts_with("→ asks to use read_file")));
    assert!(t.contains(&"  ✓ allowed read_file automatically (safe tool)".to_string()));
    assert!(t.contains(&"  ✗ denied write_file by you".to_string()));
    assert!(t.iter().any(|l| l.contains("waiting for approval: write 3 characters to /home/amy/n.txt")));
    assert!(t.iter().any(|l| l.contains("result read_file: line one line two")), "{t:?}");
    assert_eq!(t.last().unwrap(), "Answer: I left the file alone.", "a hidden event adds no line");
}

#[test]
fn a_usage_and_a_timing_line_close_the_timeline_in_words() {
    let mut l = log("aaaaaaaaaaaa", 1).0;
    l.events.insert(0, RunEvent::Timing { first_token_ms: 1500, first_text_ms: 0, total_ms: 9000, model_ms: 6000, tools_ms: 2000, overhead_ms: 40, thinking_ms: 500 });
    l.events.push(RunEvent::Usage { tokens: 1200, cost_cents: 150.0, files: 1, seconds: 9 });
    l.events.push(RunEvent::Failed { error: "boom".into() });
    let t: Vec<String> = timeline(&l).into_iter().map(|(t, _)| t).collect();
    assert!(t.iter().any(|x| x == "Time: first token 1.5 s · thinking 0.5 s · total 9.0 s · model 6.0 s · tools 2.0 s · IYAKKI 40 ms"), "{t:?}");
    assert!(t.iter().any(|x| x == "Failed: boom"));
    assert_eq!(t.last().unwrap(), "Used about 1200 tokens (~$1.50) · 1 file written · 9 s");
}

#[test]
fn the_tile_lists_newest_first_opens_the_first_run_and_says_nothing_to_the_audit_log() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100), entry("bbbbbbbbbbbb", 200)]);
    let t = RunsTile::new(&p);
    assert_eq!(t.runs().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["bbbbbbbbbbbb", "aaaaaaaaaaaa"]);
    assert_eq!(t.integrity(), Some(&Integrity::Verified));
    assert!(p.audit.borrow().is_empty());
}

#[test]
fn a_broken_chain_is_flagged_loudly_and_audited_once_per_opening() {
    let mut e = entry("aaaaaaaaaaaa", 100);
    e.integrity = Integrity::Tampered("line 3 does not follow line 2".into());
    let p = Fake::new(vec![e]);
    let mut t = RunsTile::new(&p);
    assert!(matches!(t.integrity(), Some(Integrity::Tampered(_))));
    let a = p.audit.borrow().clone();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].0, "runs.tampered");
    assert!(a[0].1.starts_with("aaaaaaaaaaaa: line 3"));
    let g = draw(&mut t, &p, 1280, 700);
    assert!(g.text_log.iter().any(|x| x.2.contains("NOT TRUSTWORTHY")));
}

#[test]
fn delete_asks_for_confirmation_then_the_port_audits_and_the_list_is_read_again() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100), entry("bbbbbbbbbbbb", 200)]);
    let mut t = RunsTile::new(&p);
    t.handle(&Event::Key(Key::Char('d')), &p);
    t.handle(&Event::Key(Key::Char('n')), &p);
    assert_eq!(t.runs().len(), 2, "any other key cancels");
    t.handle(&Event::Key(Key::Char('d')), &p);
    t.handle(&Event::Key(Key::Char('y')), &p);
    assert_eq!(t.runs().len(), 1);
    assert_eq!(t.runs()[0].id, "aaaaaaaaaaaa");
    assert_eq!(t.status(), Some(&("Run deleted.".to_string(), true)));
    assert_eq!(p.audit.borrow().as_slice(), [("runs.delete".to_string(), "bbbbbbbbbbbb".to_string())]);
}

#[test]
fn a_refused_delete_says_why_and_changes_nothing() {
    let mut p = Fake::new(vec![entry("aaaaaaaaaaaa", 100)]);
    p.refuse_delete = true;
    let mut t = RunsTile::new(&p);
    t.delete_selected(&p);
    assert_eq!(t.runs().len(), 1);
    assert_eq!(t.status(), Some(&("The record is read-only.".to_string(), false)));
    assert!(p.audit.borrow().is_empty());
}

#[test]
fn up_down_and_row_clicks_choose_a_run_and_esc_leaves() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100), entry("bbbbbbbbbbbb", 200), entry("cccccccccccc", 300)]);
    let mut t = RunsTile::new(&p);
    assert_eq!(t.detail.as_ref().unwrap().0.id, "cccccccccccc");
    t.handle(&Event::Key(Key::Down), &p);
    assert_eq!(t.detail.as_ref().unwrap().0.id, "bbbbbbbbbbbb");
    t.handle(&Event::Row(2), &p);
    assert_eq!(t.detail.as_ref().unwrap().0.id, "aaaaaaaaaaaa");
    t.handle(&Event::Key(Key::Down), &p);
    assert_eq!(t.sel, 2, "stops at the last");
    t.handle(&Event::Key(Key::Up), &p);
    assert_eq!(t.sel, 1);
    t.handle(&Event::Key(Key::PageDown), &p);
    assert_eq!(t.scroll, 12);
    t.handle(&Event::Key(Key::PageUp), &p);
    assert_eq!(t.scroll, 0);
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
}

#[test]
fn copy_transcript_puts_the_readable_record_on_the_clipboard() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100)]);
    let mut t = RunsTile::new(&p);
    iyakki_uikit::ui::clipboard::clear();
    t.handle(&Event::Key(Key::Char('c')), &p);
    let c = iyakki_uikit::ui::clipboard::latest().unwrap();
    assert!(c.starts_with("You: tidy my notes") && c.contains("Answer: I left the file alone."), "{c}");
}

#[test]
fn the_filter_cycles_agents_then_tasks_then_all_and_the_board_links_ask_the_ide() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100), task("bbbbbbbbbbbb", 200, "t1", "Agent 1", "F-APP-01"), task("cccccccccccc", 300, "t2", "Agent 2", "F-APP-02")]);
    let mut t = RunsTile::new(&p);
    assert_eq!(t.runs().iter().map(|m| m.task_label()).collect::<Vec<_>>(), ["Task t2 · Agent 2 · F-APP-02", "Task t1 · Agent 1 · F-APP-01", ""]);
    // the hand-off from the board: the item selects that run (whatever filter was on)
    t.filter = Some("Agent 2".into());
    t.open_item("run:bbbbbbbbbbbb", &p);
    assert_eq!(t.filter, None);
    assert_eq!(t.detail.as_ref().unwrap().0.id, "bbbbbbbbbbbb");
    t.open_item("run:nope", &p);
    assert!(t.status.as_ref().is_some_and(|s| !s.1));
    t.handle(&Event::Key(Key::Char('f')), &p);
    assert_eq!((t.filter.clone(), t.runs().len()), (Some("Agent 1".into()), 1));
    t.handle(&Event::Key(Key::Char('f')), &p);
    assert_eq!(t.filter.as_deref(), Some("Agent 2"));
    t.handle(&Event::Key(Key::Char('f')), &p);
    assert_eq!(t.filter.as_deref(), Some("t1"));
    for _ in 0..2 {
        t.handle(&Event::Key(Key::Char('f')), &p);
    }
    assert_eq!((t.filter.clone(), t.runs().len()), (None, 3));
    t.open_item("run:cccccccccccc", &p);
    t.handle(&Event::Key(Key::Char('g')), &p);
    assert_eq!(t.take_request(), Some(("ide".into(), "task:t2".into())));
    t.handle(&Event::Key(Key::Char('b')), &p);
    assert_eq!(t.take_request(), Some(("ide".into(), "row:F-APP-02".into())));
    assert_eq!(t.take_request(), None, "taken once");
    t.open_item("run:aaaaaaaaaaaa", &p);
    t.handle(&Event::Key(Key::Char('g')), &p);
    assert_eq!(t.take_request(), None, "a run that is not a task's has no link");
}

#[test]
fn the_empty_state_and_the_screens_fit_the_four_sizes_and_offer_the_links() {
    let p = Fake::new(vec![]);
    let mut t = RunsTile::new(&p);
    assert!(t.runs().is_empty() && t.integrity().is_none());
    let g = draw(&mut t, &p, 900, 600);
    assert!(g.text_log.iter().any(|x| x.2.contains("No agent runs yet")));
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
    let many = Fake::new((0..6).map(|i| task(&format!("{i:012x}"), 100 + i as u64, &format!("t{i}"), &format!("Agent {}", i % 3 + 1), "F-APP-01")).collect());
    let mut t = RunsTile::new(&many);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut t, &many, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            assert!(hit.rect.x >= -0.5 && hit.rect.y >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, hit.rect);
        }
        for want in ["Show task", "Show row"] {
            assert!(g.hits.iter().any(|x| x.label.starts_with(want)), "{w}x{h}: {want}");
        }
    }
}

#[test]
fn the_jobs_switch_shows_the_running_count_and_j_toggles_between_runs_and_jobs() {
    let p = Fake::new(vec![entry("aaaaaaaaaaaa", 100)]);
    p.jobs.borrow_mut().extend([job("j1", JobState::Running), job("j2", JobState::Succeeded)]);
    let mut t = RunsTile::new(&p);
    assert!(!t.jobs_mode());
    t.handle(&Event::Key(Key::Char('j')), &p);
    assert!(t.jobs_mode() && t.jobs().jobs().len() == 2);
    let g = draw(&mut t, &p, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Jobs (1 running)"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert!(g.hits.iter().any(|h| h.label == "Dismiss") && g.hits.iter().any(|h| h.label == "Stop"));
    // Esc in the jobs list leaves the tile; the Runs switch and j go back
    t.handle(&Event::Tab(jobs::TAB_RUNS), &p);
    assert!(!t.jobs_mode());
    t.open_item("jobs", &p);
    assert!(t.jobs_mode());
    t.handle(&Event::Key(Key::Char('j')), &p);
    assert!(!t.jobs_mode());
    t.show_jobs(&p);
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
}

#[test]
fn an_empty_jobs_list_says_so_and_a_job_is_dismissed_through_the_port() {
    let p = Fake::new(vec![]);
    let mut t = RunsTile::new(&p);
    t.show_jobs(&p);
    let g = draw(&mut t, &p, 1280, 720);
    assert!(g.text_log.iter().any(|x| x.2.contains("No detached jobs")));
    p.jobs.borrow_mut().push(job("j1", JobState::Failed(2)));
    t.jobs_mut().poll(&p, true);
    t.handle(&jobs::ev_act(0), &p);
    assert!(p.jobs.borrow().is_empty(), "Dismiss went to the port");
}
