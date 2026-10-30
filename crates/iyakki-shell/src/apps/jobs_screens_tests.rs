//! The Jobs strip in the chat and in the AI IDE, the Jobs list in RUNS, and the redraw rule: clicks, geometry at the four screen
//! sizes, jobs the assistant started through the real tools, jobs that outlive the IDE, and animation only while a job runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, Role, ToolCall, ToolSpec};
use iyakki_agent::jobs::{Cmd, Spec, State};
use iyakki_sec::policy::CliLevel;

use super::chat::ChatApp;
use super::ide::*;
use super::jobs_ui::{self, TAB_RUNS};
use super::runs::RunsApp;
use super::{App, AppCx};
use crate::apps::tests::cx_with_backend;
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};

/// `do:<tool>|<json args>` calls that tool; after a tool result it says it is finished.
struct Router;

impl ChatBackend for Router {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Ok("plain answer".into())
    }
    fn complete_tools(&self, msgs: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        let last = msgs.iter().rev().find(|m| m.role != Role::System).ok_or("no messages")?;
        if last.role == Role::Tool {
            return Ok(ModelTurn { text: "finished".into(), calls: vec![] });
        }
        let t = last.content.split("\n\n[TOOLS AVAILABLE NOW]").next().unwrap_or_default().trim();
        if let Some((name, args)) = t.strip_prefix("do:").and_then(|r| r.split_once('|')) {
            return Ok(ModelTurn { text: String::new(), calls: vec![ToolCall { id: "c1".into(), name: name.into(), args: serde_json::from_str(args).map_err(|e| e.to_string())? }] });
        }
        Ok(ModelTurn { text: "ok".into(), calls: vec![] })
    }
}

struct World {
    cx: AppCx,
    base: PathBuf,
    /// The IDE looks for a cluster through process-wide slots that the cluster tests fill with fakes (an ambiguous pair of clusters
    /// switches the IDE to the cluster page and opens a chooser): these tests take turns with them.
    _cluster_slot: std::sync::MutexGuard<'static, ()>,
}

impl Drop for World {
    fn drop(&mut self) {
        for j in self.cx.services.jobs.list(&self.cx.session.user) {
            let _ = self.cx.services.jobs.stop(&self.cx.session.user, &j.id, Duration::from_millis(300));
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn world(tag: &str) -> World {
    let slot = crate::apps::ide_tests::KUBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-jobsui-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (mut cx, _d) = cx_with_backend(&format!("jobsui-{tag}"), Router);
    cx.session.policy.fs_read = vec![base.display().to_string()];
    cx.session.policy.fs_write = vec![base.display().to_string()];
    cx.session.policy.cli = CliLevel::Full;
    cx.session.policy.agent = true;
    World { cx, base, _cluster_slot: slot }
}

fn project(w: &World, name: &str) -> PathBuf {
    let d = w.base.join(name);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
    d
}

fn start(cx: &AppCx, ws: &str, cwd: &Path, label: &str, line: &str) -> String {
    cx.services.jobs.start(&cx.session.user, &Spec { label: label.into(), cmd: Cmd::Shell(line.into()), cwd: cwd.into(), workspace: ws.into() }, &|_: &Path| Ok(())).unwrap().id
}

fn scale(w: u32, h: u32) -> f32 {
    crate::shell::scale_for(w, h)
}

fn render_ide(app: &mut IdeApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = scale(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(crate::gfx::rgb(0, 0, 0));
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, 1);
    g
}

fn until_ide(app: &mut IdeApp, cx: &AppCx, what: &str, mut f: impl FnMut(&mut IdeApp) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        app.handle(&Event::Tick, cx);
        if f(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let chats: Vec<String> = app.ws.iter().map(|w| format!("{}: running={} approval={:?} error={:?} msgs={:?}", w.meta.name, w.chat.is_running(), w.chat.pending_approval(), w.chat.error_text(), w.chat.messages().iter().map(|m| format!("{:?}:{}", m.role, m.content.chars().take(80).collect::<String>())).collect::<Vec<_>>())).collect();
    panic!("timed out waiting for: {what}; chats: {chats:?}");
}

fn click(g: &Gfx, label: &str) -> Event {
    g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`: {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>())).ev.clone()
}

/// Make the jobs strip read the engine now (the heartbeat does it once a second).
fn settle(app: &mut IdeApp, cx: &AppCx) {
    for _ in 0..3 {
        app.handle(&Event::Tick, cx);
    }
}

#[test]
fn an_assistant_started_job_appears_in_the_ides_strip_stops_with_a_click_and_the_chat_shows_what_happened() {
    let w = world("ide");
    let a = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    app.add_and_open("Alpha", &a, &w.cx).unwrap();
    app.focus_pane(super::ide_layout::Pane::Chat);
    // the assistant starts it through the real tool: the person approves first
    let args = serde_json::json!({"command": "echo 'Downloading system-images 30%'; sleep 120", "label": "sdk install"});
    app.active_mut().unwrap().chat.ask(&w.cx, &format!("do:job_start|{args}"));
    until_ide(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
    let (_, tool, summary) = app.active_ws().unwrap().chat.pending_approval().unwrap().clone();
    assert_eq!(tool, "job_start");
    assert!(summary.contains("start a detached job “sdk install”") && summary.contains("sleep 120"), "{summary}");
    assert!(w.cx.services.jobs.list(&w.cx.session.user).is_empty(), "nothing starts before the approval");
    app.handle(&Event::Key(Key::Ctrl('y')), &w.cx);
    until_ide(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    let result = app.active_ws().unwrap().chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(result.contains("Started job j1") && result.contains("job_wait"), "{result}");
    // the strip: a row for it with Log and Stop, at four screen sizes
    let t0 = Instant::now();
    loop {
        settle(&mut app, &w.cx);
        let g = render_ide(&mut app, &w.cx, 1280, 720);
        if g.hits.iter().any(|h| h.label == "Stop") {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "the strip never showed the job");
        std::thread::sleep(Duration::from_millis(100));
    }
    for (sw, sh) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = render_ide(&mut app, &w.cx, sw, sh);
        let (log_btn, stop_btn) = (g.hits.iter().find(|h| h.label == "Log").unwrap_or_else(|| panic!("{sw}x{sh}: no Log button")), g.hits.iter().find(|h| h.label == "Stop").unwrap_or_else(|| panic!("{sw}x{sh}: no Stop button")));
        for b in [log_btn, stop_btn] {
            assert!(b.rect.x >= 0.0 && b.rect.y >= 0.0 && b.rect.x + b.rect.w <= sw as f32 && b.rect.y + b.rect.h <= sh as f32, "{sw}x{sh}: button on the screen: {:?}", b.rect);
            assert!(b.rect.h >= 20.0 && b.rect.w >= 34.0, "{sw}x{sh}: clickable size {:?}", b.rect);
        }
        assert!(log_btn.rect.x + log_btn.rect.w <= stop_btn.rect.x, "{sw}x{sh}: the buttons do not overlap");
        // the composer is below the strip
        let field = g.hits.iter().filter(|h| h.label == "field").map(|h| h.rect).fold(None::<R>, |acc, r| match acc { Some(p) if p.y > r.y => Some(p), _ => Some(r) });
        if let Some(f) = field {
            assert!(stop_btn.rect.y + stop_btn.rect.h <= f.y + 1.0, "{sw}x{sh}: the strip ends above the composer ({:?} vs {:?})", stop_btn.rect, f);
        }
    }
    // Log opens the log view (with Close and the output), Esc closes it
    let g = render_ide(&mut app, &w.cx, 1280, 720);
    app.handle(&click(&g, "Log"), &w.cx);
    let g = render_ide(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Close") && g.hits.iter().any(|h| h.label == "log-backdrop"), "the log view is open");
    app.handle(&Event::Key(Key::Esc), &w.cx);
    let g = render_ide(&mut app, &w.cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Close"), "Esc closed the log and did not leave the IDE");
    // Stop: the job ends, the row becomes a result with Dismiss, and the chat shows a step about it
    app.handle(&click(&g, "Stop"), &w.cx);
    let user = w.cx.session.user.clone();
    until_ide(&mut app, &w.cx, "the job to stop", |_| !w.cx.services.jobs.get(&user, "j1").unwrap().running());
    assert_eq!(w.cx.services.jobs.get(&user, "j1").unwrap().state, State::Stopped);
    until_ide(&mut app, &w.cx, "the chat event", |a| {
        for _ in 0..3 {
            a.handle(&Event::Tick, &w.cx);
        }
        a.active_ws().unwrap().chat.messages().iter().any(|m| m.role == Role::Tool && m.content.contains("Job “sdk install” (j1) was stopped"))
    });
    let g = render_ide(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Dismiss") && !g.hits.iter().any(|h| h.label == "Stop"));
    app.handle(&click(&g, "Dismiss"), &w.cx);
    settle(&mut app, &w.cx);
    let g = render_ide(&mut app, &w.cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Dismiss" || h.label == "Log"), "a dismissed job is gone from the strip");
}

#[test]
fn a_job_outlives_the_ide_and_a_new_one_finds_it_again_with_its_own_workspace_only() {
    let w = world("adopt");
    let (a, b) = (project(&w, "alpha"), project(&w, "beta"));
    let id = start(&w.cx, "Alpha", &a, "long build", "echo compiling 10%; sleep 120");
    start(&w.cx, "Beta", &b, "other project", "sleep 120");
    // the first IDE never existed in this test: a fresh one adopts the running job
    let mut app = IdeApp::new(&w.cx);
    app.add_and_open("Alpha", &a, &w.cx).unwrap();
    let t0 = Instant::now();
    loop {
        settle(&mut app, &w.cx);
        let g = render_ide(&mut app, &w.cx, 1280, 720);
        if g.hits.iter().any(|h| h.label == "Stop") {
            // one row: Beta's job is not in Alpha's strip
            assert_eq!(g.hits.iter().filter(|h| h.label == "Stop").count(), 1, "only this workspace's job");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(id, "j1");
    assert!(w.cx.services.jobs.get(&w.cx.session.user, &id).unwrap().running());
}

#[test]
fn the_ide_asks_for_frames_only_while_a_job_runs() {
    let w = world("redraw");
    let a = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    app.add_and_open("Alpha", &a, &w.cx).unwrap();
    // idle: the IDE's own once-a-second refresh is the baseline (the strip adds nothing to it)
    for _ in 0..8 {
        app.handle(&Event::Tick, &w.cx);
        let _ = app.wants_redraw();
    }
    // real heartbeats: about 30 a second
    let count = |app: &mut IdeApp, beats: usize| -> usize {
        (0..beats).map(|_| {
            app.handle(&Event::Tick, &w.cx);
            std::thread::sleep(Duration::from_millis(33));
            usize::from(app.wants_redraw())
        }).sum()
    };
    // opening the workspace starts background work (file tree, cluster lookup, git); on a busy machine it can still be running
    // after the first beats and asks for frames until it is done. Measure only once a whole second of beats is quiet.
    let t0 = Instant::now();
    while count(&mut app, 30) > 2 {
        assert!(t0.elapsed() < Duration::from_secs(90), "the IDE never went quiet after opening a workspace");
    }
    let baseline = count(&mut app, 90);
    assert!(baseline <= 4, "the IDE refreshes about once a second when idle: {baseline}");
    // a running job: about every third heartbeat on top of that
    let id = start(&w.cx, "Alpha", &a, "nap", "sleep 120");
    // the chat notices a job started by someone else within a few seconds: wait for the first animated frames (the load of the
    // machine this runs on varies), then measure
    let seen = Instant::now();
    while count(&mut app, 10) <= 3 && seen.elapsed() < Duration::from_secs(15) {}
    let running = count(&mut app, 90);
    assert!(running >= baseline + 15 && running <= baseline + 45, "about one frame in three while a job runs: {running} (baseline {baseline})");
    // stopped: one last frame to draw the result, then back to the baseline
    w.cx.services.jobs.stop(&w.cx.session.user, &id, Duration::from_secs(1)).unwrap();
    for _ in 0..40 {
        app.handle(&Event::Tick, &w.cx);
        let _ = app.wants_redraw();
        std::thread::sleep(Duration::from_millis(30));
    }
    let after = count(&mut app, 90);
    assert!(after <= baseline + 1, "back to the baseline once the job has ended: {after} (baseline {baseline})");
}

#[test]
fn the_chat_screen_shows_the_strip_clicks_work_and_nothing_overlaps_at_every_size() {
    let w = world("chat");
    let mut app = ChatApp::new();
    start(&w.cx, "", &w.base, "Android emulator install", "echo 'Downloading 42%'; sleep 120");
    start(&w.cx, "", &w.base, "tests", "echo ok; sleep 120");
    for (sw, sh) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = scale(sw, sh);
        for _ in 0..4 {
            app.handle(&Event::Tick, &w.cx);
            std::thread::sleep(Duration::from_millis(5));
        }
        // the heartbeat reads the jobs once a second at most: force through a few ticks
        let t0 = Instant::now();
        let g = loop {
            app.handle(&Event::Tick, &w.cx);
            let mut g = Gfx::new(sw, sh);
            g.clear(crate::gfx::rgb(0, 0, 0));
            app.render(&mut g, R::new(0.0, 46.0 * s, sw as f32, sh as f32 - 46.0 * s), s, &w.cx, 3);
            if g.hits.iter().any(|h| h.label == "Stop") {
                break g;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "{sw}x{sh}: no strip");
            std::thread::sleep(Duration::from_millis(100));
        };
        let stops: Vec<R> = g.hits.iter().filter(|h| h.label == "Stop").map(|h| h.rect).collect();
        assert_eq!(stops.len(), 2, "{sw}x{sh}: a row for each job");
        for r in &stops {
            assert!(r.y >= 46.0 * s && r.y + r.h <= sh as f32 && r.x + r.w <= sw as f32, "{sw}x{sh}: {r:?}");
            assert!(r.h >= 20.0, "{sw}x{sh}: clickable height {r:?}");
        }
        assert!(stops[0].y + stops[0].h <= stops[1].y || stops[1].y + stops[1].h <= stops[0].y, "{sw}x{sh}: rows do not overlap");
        let field_top = g.hits.iter().filter(|h| h.label == "field").map(|h| h.rect.y).fold(f32::MAX, f32::min);
        if field_top < f32::MAX {
            assert!(stops.iter().all(|r| r.y + r.h <= field_top + 1.0), "{sw}x{sh}: the composer is below the strip");
        }
    }
    // clicking Log on a row opens the log view with the job's output; Close shuts it
    let s = scale(1280, 720);
    let mut g = Gfx::new(1280, 720);
    g.clear(crate::gfx::rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 720.0 - 46.0 * s), s, &w.cx, 3);
    let first_log = g.hits.iter().find(|h| h.label == "Log").unwrap().ev.clone();
    app.handle(&first_log, &w.cx);
    let mut g = Gfx::new(1280, 720);
    g.clear(crate::gfx::rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 720.0 - 46.0 * s), s, &w.cx, 3);
    assert!(g.hits.iter().any(|h| h.label == "Close"));
    app.handle(&click(&g, "Close"), &w.cx);
    let mut g = Gfx::new(1280, 720);
    g.clear(crate::gfx::rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 720.0 - 46.0 * s), s, &w.cx, 3);
    assert!(!g.hits.iter().any(|h| h.label == "Close"));
    // the chat's own Esc still leaves the chat when no log is open
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &w.cx), super::Flow::Back));
}

#[test]
fn runs_has_a_jobs_list_with_log_stop_and_dismiss_at_every_size() {
    let w = world("runs");
    start(&w.cx, "Alpha", &w.base, "install", "echo 'step 3/10'; sleep 120");
    start(&w.cx, "", &w.base, "quick", "echo done");
    w.cx.services.jobs.wait(&w.cx.session.user, "j2", Duration::from_secs(20)).unwrap();
    let mut app = RunsApp::new(&w.cx);
    assert!(!app.jobs_mode());
    for (sw, sh) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = scale(sw, sh);
        let mut g = Gfx::new(sw, sh);
        g.clear(crate::gfx::rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0 * s, sw as f32, sh as f32 - 72.0 * s), s, &w.cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "Runs") && g.hits.iter().any(|h| h.label.starts_with("Jobs")), "{sw}x{sh}: the switch");
        // open the list
        app.handle(&click(&g, &g.hits.iter().find(|h| h.label.starts_with("Jobs")).unwrap().label), &w.cx);
        assert!(app.jobs_mode());
        let mut g = Gfx::new(sw, sh);
        g.clear(crate::gfx::rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0 * s, sw as f32, sh as f32 - 72.0 * s), s, &w.cx, 0);
        let (logs, stops, dismiss) = (g.hits.iter().filter(|h| h.label == "Log").count(), g.hits.iter().filter(|h| h.label == "Stop").count(), g.hits.iter().filter(|h| h.label == "Dismiss").count());
        assert_eq!((logs, stops, dismiss), (2, 1, 1), "{sw}x{sh}: both jobs, the running one with Stop, the finished one with Dismiss");
        for h in g.hits.iter().filter(|h| matches!(h.label.as_str(), "Log" | "Stop" | "Dismiss")) {
            assert!(h.rect.x + h.rect.w <= sw as f32 && h.rect.y + h.rect.h <= sh as f32 && h.rect.h >= 20.0, "{sw}x{sh}: {:?}", h.rect);
        }
        app.handle(&Event::Tab(TAB_RUNS), &w.cx);
        assert!(!app.jobs_mode());
    }
    // key j toggles; a click on Log shows the output; Esc closes the log first and then leaves
    app.handle(&Event::Key(Key::Char('j')), &w.cx);
    let s = scale(1280, 720);
    let mut g = Gfx::new(1280, 720);
    g.clear(crate::gfx::rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 72.0 * s, 1280.0, 720.0 - 72.0 * s), s, &w.cx, 0);
    app.handle(&jobs_ui::ev_log(1), &w.cx);
    assert!(app.jobs().log_open());
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &w.cx), super::Flow::Stay) && !app.jobs().log_open());
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &w.cx), super::Flow::Back));
    // dismiss the finished one
    app.handle(&jobs_ui::ev_act(1), &w.cx);
    assert_eq!(app.jobs().jobs().len(), 1);
    // Stop the running one (on a thread) and the list shows it ended
    app.handle(&jobs_ui::ev_act(0), &w.cx);
    let end = Instant::now() + Duration::from_secs(10);
    while w.cx.services.jobs.get(&w.cx.session.user, "j1").unwrap().running() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(w.cx.services.jobs.get(&w.cx.session.user, "j1").unwrap().state, State::Stopped);
}

#[test]
fn a_finished_job_leaves_a_note_in_the_inbox_once_and_it_opens_the_jobs_list() {
    let w = world("inbox");
    let id = start(&w.cx, "Alpha", &w.base, "emulator", "echo installed; exit 0");
    w.cx.services.jobs.wait(&w.cx.session.user, &id, Duration::from_secs(20)).unwrap();
    // the shell side: the same call the heartbeat makes
    let mine = w.cx.services.jobs.take_finished_all(&w.cx.session.user, iyakki_agent::jobs::Channel::Inbox);
    assert_eq!(mine.len(), 1);
    assert!(w.cx.services.jobs.take_finished_all(&w.cx.session.user, iyakki_agent::jobs::Channel::Inbox).is_empty());
    // and the chat of that workspace is told separately (its own event)
    assert_eq!(w.cx.services.jobs.take_finished(&w.cx.session.user, "Alpha", iyakki_agent::jobs::Channel::Ui).len(), 1);
}
