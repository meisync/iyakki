//! Tests of the Tasks tab inside the AI IDE, with a fake runner (no model, no vendor) and real temporary folders and `git`.
//! A child module of `ide_tests`, so it shares that file's world, project and render helpers.

// the boundary scan reads a file's code up to its first #[cfg(test)]: this whole file is test code (it runs real `git` in temp folders)
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::agent::{RunEvent, RunHandle, Verdict};
use iyakki_agent::tasks::{Runner, RunSpec, Status};

use super::super::ide_tasks::{self, DTab};
use super::super::ide_tasks_run::use_runner;
use super::*;

/// The fake runner is one global: tests that set it take turns.
static RUNNER_LOCK: Mutex<()> = Mutex::new(());

struct RunnerGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for RunnerGuard {
    fn drop(&mut self) {
        use_runner(None);
    }
}

type Script = Arc<dyn Fn(RunSpec, Sender<RunEvent>, Arc<AtomicBool>, Receiver<(String, Verdict)>) + Send + Sync>;

fn fake(script: Script) -> RunnerGuard {
    let g = RUNNER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let r: Runner = Arc::new(move |spec: RunSpec| {
        let (etx, erx) = channel();
        let (dtx, drx) = channel();
        let h = RunHandle::from_parts(erx, dtx);
        let cancel = h.cancel_flag();
        let s = script.clone();
        std::thread::spawn(move || s(spec, etx, cancel, drx));
        h
    });
    use_runner(Some(r));
    RunnerGuard(g)
}

fn writer(file: &'static str, body: &'static str, hold: Arc<AtomicBool>) -> Script {
    Arc::new(move |spec, etx, _, _| {
        while hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(spec.tree.join(file), body).unwrap();
        let _ = etx.send(RunEvent::Final { text: "Autopilot finished: 1 of 1 rows done.".into() });
    })
}

fn new_task(app: &mut IdeApp, cx: &AppCx, title: &str, goal: &str, rows: &str) {
    app.show_main(MainTab::Tasks);
    key(app, cx, Key::Char('n'));
    let Some(Modal::Form(FormKind::NewTask, f)) = &mut app.modal else { panic!("the new task form") };
    f.set_value(0, title);
    f.set_value(1, goal);
    f.set_value(2, rows);
    key(app, cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.modal.as_ref().and_then(|m| if let Modal::Form(_, f) = m { f.error.clone() } else { None }));
}

fn statuses(app: &IdeApp) -> Vec<Status> {
    cur(app).tasks.mgr.list().iter().map(|t| t.status).collect()
}

fn git_project(w: &World, name: &str) -> PathBuf {
    let d = project(w, name, &[(".gitignore", ".iyakki/\n")]);
    let g = |a: &[&str]| {
        let o = std::process::Command::new("git").current_dir(&d).args(["-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(a).output().unwrap();
        assert!(o.status.success(), "{a:?} {}", String::from_utf8_lossy(&o.stderr));
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["add", "-A"]);
    g(&["commit", "-q", "-m", "init"]);
    d
}

#[test]
fn two_tasks_run_in_parallel_show_in_the_top_bar_and_the_tab_and_merge_after_a_confirmation() {
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let _r = fake(Arc::new(move |spec, etx, _, _| {
        while h2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // each task writes its own words into the same new file: the second one conflicts with the first
        std::fs::write(spec.tree.join("made.txt"), format!("{}\n", spec.task.title)).unwrap();
        let _ = etx.send(RunEvent::Final { text: "Autopilot finished: 1 of 1 rows done.".into() });
    }));
    let w = world("tasks-flow");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(MainTab::ALL.iter().position(|t| *t == MainTab::Tasks), Some(2), "the tab sits right after Board");
    assert!(cur(&app).tasks.chip().is_none(), "no chip without tasks");
    new_task(&mut app, &w.cx, "First thing", "make the first thing", "");
    new_task(&mut app, &w.cx, "Second thing", "make the second thing", "F-A-02");
    until(&mut app, &w.cx, "both to run", |a| cur(a).tasks.mgr.counts() == (2, 0, 0));
    let chip = cur(&app).tasks.chip().unwrap();
    assert_eq!(chip.0, "2 running");
    // the chip is in the top bar and a click opens the tab
    app.show_main(MainTab::Editor);
    let g = render(&mut app, &w.cx, 1280, 720);
    let hit = g.hits.iter().find(|h| h.label == "open the Tasks tab").expect("the top bar chip");
    assert!(matches!(hit.ev, Event::Tab(n) if n == T_TOP + 11));
    app.handle(&hit.ev.clone(), &w.cx);
    assert_eq!(cur(&app).main_tab, MainTab::Tasks);
    // two cards and the row of the second one is owned
    let g = render(&mut app, &w.cx, 1280, 720);
    assert_eq!(g.hits.iter().filter(|h| h.label.starts_with("task t")).count(), 2);
    assert_eq!(cur(&app).tasks.mgr.owner_of("F-A-02").is_some(), true);
    // each task has its own folder
    let list = cur(&app).tasks.mgr.list();
    assert_ne!(list[0].tree, list[1].tree);
    assert!(list.iter().all(|t| t.tree.starts_with(d.join(".iyakki/tasks"))));
    hold.store(false, Ordering::SeqCst);
    until(&mut app, &w.cx, "both done", |a| statuses(a).iter().all(|s| *s == Status::Done));
    assert!(!d.join("made.txt").exists(), "nothing reached the workspace before the merge");
    // review the first (newest first: select by id)
    let id = cur(&app).tasks.mgr.list().iter().find(|t| t.title == "First thing").unwrap().id.clone();
    cur_mut(&mut app).tasks.select_id(&id);
    key(&mut app, &w.cx, Key::Char('m'));
    let Some(Modal::Confirm(Confirm::MergeTask(mid, text))) = &app.modal else { panic!("the merge asks first: {:?}", cur(&app).tasks.status) };
    assert_eq!(mid, &id);
    assert!(text.contains("merge cleanly"), "{text}");
    assert!(!d.join("made.txt").exists(), "asking changes nothing");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Sub(SUB, Key::Char('y')))), "the Merge button of the dialog");
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(app.modal.is_none());
    assert_eq!(std::fs::read_to_string(d.join("made.txt")).unwrap(), "First thing\n");
    assert_eq!(cur(&app).tasks.mgr.get(&id).unwrap().status, Status::Merged);
    // the second task changed the same new file: its merge is refused with the file named and nothing is changed
    let id2 = cur(&app).tasks.mgr.list().iter().find(|t| t.title == "Second thing").unwrap().id.clone();
    cur_mut(&mut app).tasks.select_id(&id2);
    key(&mut app, &w.cx, Key::Char('m'));
    match &app.modal {
        Some(Modal::Info(t, msg)) => assert!(t.contains("would not go through") && msg.contains("made.txt"), "{t}: {msg}"),
        o => panic!("a conflict report instead of the confirm, got confirm={}", matches!(o, Some(Modal::Confirm(_)))),
    }
}

#[test]
fn a_denied_discard_changes_nothing_and_a_confirmed_one_removes_the_tree_and_frees_the_rows() {
    let hold = Arc::new(AtomicBool::new(false));
    let _r = fake(writer("x.txt", "x\n", hold));
    let w = world("tasks-discard");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    new_task(&mut app, &w.cx, "Row work", "do the row", "F-A-02");
    until(&mut app, &w.cx, "done", |a| statuses(a) == vec![Status::Done]);
    let t = cur(&app).tasks.mgr.list().remove(0);
    key(&mut app, &w.cx, Key::Char('d'));
    assert!(matches!(app.modal, Some(Modal::Confirm(Confirm::DiscardTask(_)))));
    key(&mut app, &w.cx, Key::Char('n'));
    assert!(app.modal.is_none() && t.tree.exists(), "cancelled: the folder is still there");
    key(&mut app, &w.cx, Key::Char('d'));
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(!t.tree.exists());
    assert_eq!(cur(&app).tasks.mgr.get(&t.id).unwrap().status, Status::Discarded);
    assert!(cur(&app).tasks.mgr.owner_of("F-A-02").is_none());
}

#[test]
fn the_board_context_menu_runs_the_selected_row_as_a_task_and_a_row_in_a_task_is_refused_twice() {
    let hold = Arc::new(AtomicBool::new(true));
    let _r = fake(writer("y.txt", "y\n", hold.clone()));
    let w = world("tasks-board");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.show_main(MainTab::Board);
    cur_mut(&mut app).board.select_id("F-A-02");
    render(&mut app, &w.cx, 1280, 720);
    let m = app.layout.as_ref().unwrap().main;
    let mut items = App::context_items(&mut app, (m.x + m.w / 2.0) as i32, (m.y + m.h / 2.0) as i32);
    assert!(items.iter().any(|i| i.id == "task.row") && items.iter().any(|i| i.id == "task.rows"), "{:?}", items.iter().map(|i| i.id.clone()).collect::<Vec<_>>());
    items.clear();
    App::context_pick(&mut app, "task.row", &w.cx);
    assert_eq!(cur(&app).main_tab, MainTab::Tasks, "the tab opens on the new task");
    let t = cur(&app).tasks.mgr.list();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].rows, vec!["F-A-02".to_string()]);
    // the same row again: refused with the owner named, no second task
    app.show_main(MainTab::Board);
    App::context_pick(&mut app, "task.row", &w.cx);
    assert_eq!(cur(&app).tasks.mgr.list().len(), 1);
    assert!(app.notice.as_ref().is_some_and(|n| !n.1 && n.0.contains("already belongs")), "{:?}", app.notice);
    hold.store(false, Ordering::SeqCst);
    until(&mut app, &w.cx, "done", |a| statuses(a) == vec![Status::Done]);
}

#[test]
fn approvals_of_a_task_show_inline_tagged_with_its_name_and_the_buttons_answer_only_that_task() {
    let got = Arc::new(Mutex::new(Vec::<(String, Verdict)>::new()));
    let g2 = got.clone();
    let _r = fake(Arc::new(move |spec, etx, _, drx| {
        let _ = etx.send(RunEvent::Approval { id: "c1".into(), name: "write_file".into(), summary: format!("write for {}", spec.task.title), detail: String::new() });
        let (cid, v) = drx.recv_timeout(Duration::from_secs(10)).unwrap();
        g2.lock().unwrap().push((spec.task.title.clone(), v));
        let _ = etx.send(RunEvent::Decision { id: cid, name: "write_file".into(), approved: v != Verdict::Deny, by: "user".into() });
        let _ = etx.send(RunEvent::Final { text: "ok".into() });
    }));
    let w = world("tasks-approve");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    new_task(&mut app, &w.cx, "Alpha task", "a", "");
    new_task(&mut app, &w.cx, "Beta task", "b", "");
    until(&mut app, &w.cx, "two approvals", |a| cur(a).tasks.mgr.approvals().len() == 2);
    assert_eq!(cur(&app).tasks.chip().unwrap().0, "2 need you");
    assert!(cur(&app).tasks.chip().unwrap().1, "the chip is amber");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().filter(|h| h.label == "allow").count() == 2 && g.hits.iter().filter(|h| h.label == "deny").count() == 2, "an Allow and a Deny per waiting approval");
    // the first card allows, the second denies: each answer reaches its own run
    let order: Vec<String> = cur(&app).tasks.mgr.approvals().iter().map(|a| a.task.clone()).collect();
    let titles: Vec<String> = order.iter().map(|id| cur(&app).tasks.mgr.get(id).unwrap().title).collect();
    app.handle(&Event::Tab(ide_tasks::APPROVE_BASE), &w.cx);
    until(&mut app, &w.cx, "the first answer", |a| cur(a).tasks.mgr.approvals().len() == 1);
    app.handle(&Event::Tab(ide_tasks::APPROVE_BASE + 1), &w.cx);
    until(&mut app, &w.cx, "both done", |a| statuses(a).iter().all(|s| *s == Status::Done));
    let got = got.lock().unwrap().clone();
    assert!(got.contains(&(titles[0].clone(), Verdict::Approve)) && got.contains(&(titles[1].clone(), Verdict::Deny)), "{got:?} {titles:?}");
}

#[test]
fn the_diff_is_listed_per_file_tinted_foldable_and_selectable_and_follow_up_resumes_the_task() {
    let hold = Arc::new(AtomicBool::new(false));
    let _r = fake(Arc::new(move |spec, etx, _, _| {
        let _ = hold.load(Ordering::SeqCst);
        std::fs::write(spec.tree.join("src/main.rs"), "fn main() { println!(\"changed\"); }\n").unwrap();
        std::fs::write(spec.tree.join("new.txt"), "brand new\n").unwrap();
        let _ = etx.send(RunEvent::Final { text: "done".into() });
    }));
    let w = world("tasks-diff");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    new_task(&mut app, &w.cx, "Edit things", "edit", "");
    until(&mut app, &w.cx, "done", |a| statuses(a) == vec![Status::Done]);
    // the review is read on a worker thread
    app.handle(&Event::Tab(ide_tasks::TAB_BASE + 2), &w.cx);
    assert_eq!(cur(&app).tasks.tab, DTab::Diff);
    until(&mut app, &w.cx, "the diff", |a| {
        render(a, &w.cx, 1280, 720);
        cur(a).tasks.review_ready()
    });
    render(&mut app, &w.cx, 1280, 720);
    let lines = cur(&app).tasks.detail_lines().to_vec();
    assert!(lines.iter().any(|l| l.0.contains("src/main.rs") && l.0.contains("+1 -1")), "{lines:?} tab={:?} sel={:?}", cur(&app).tasks.tab, cur(&app).tasks.selected().map(|t| (t.status, t.tree.exists())));
    let add = lines.iter().find(|l| l.0 == "+brand new").expect("an added line");
    let del = lines.iter().find(|l| l.0.starts_with("-fn main")).expect("a removed line");
    assert!(add.1[1] > add.1[0] && del.1[0] > del.1[1], "added is greenish, removed is reddish: {add:?} {del:?}");
    // the Files tab: one row per file, a click folds that file out of the diff
    app.handle(&Event::Tab(ide_tasks::TAB_BASE + 1), &w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(r) if (ide_tasks::FILE_BASE..ide_tasks::FILE_BASE + 5000).contains(&r))).count(), 2);
    app.handle(&Event::Row(ide_tasks::FILE_BASE), &w.cx);
    app.handle(&Event::Tab(ide_tasks::TAB_BASE + 2), &w.cx);
    render(&mut app, &w.cx, 1280, 720);
    let folded = cur(&app).tasks.detail_lines().to_vec();
    assert!(folded.len() < lines.len(), "a folded file shows only its header ({} vs {})", folded.len(), lines.len());
    // selectable text: the pane registers an area for presses
    app.handle(&Event::Tab(ide_tasks::TAB_BASE + 2), &w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Area(id) if id == ide_tasks::DETAIL_AREA)), "the text registers an area for presses");
    // follow-up: a message queues the task again
    key(&mut app, &w.cx, Key::Char('f'));
    let Some(Modal::Form(FormKind::TaskFollowup(_), f)) = &mut app.modal else { panic!("the follow-up form") };
    f.set_value(0, "also fix the typo");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the second run", |a| cur(a).tasks.mgr.list()[0].interjection == "also fix the typo" || statuses(a) == vec![Status::Done]);
}

#[test]
fn the_tab_has_one_keyboard_owner_no_caret_and_esc_hands_the_keyboard_back() {
    let _r = fake(writer("k.txt", "k\n", Arc::new(AtomicBool::new(false))));
    let w = world("tasks-owner");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.show_main(MainTab::Tasks);
    assert_eq!(cur(&app).owner(), Owner::Tasks);
    assert!(!cur(&app).owner().is_text(), "letters are shortcuts: there is no caret on this tab");
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).focus, Pane::Tree);
    // Ctrl+Shift+G toggles the tab
    key(&mut app, &w.cx, Key::Ctrl('G'));
    assert_eq!(cur(&app).main_tab, MainTab::Tasks);
    // a dialog takes the keyboard: typing in the new task form is typed there, not read as shortcuts
    key(&mut app, &w.cx, Key::Char('n'));
    typ(&mut app, &w.cx, "snd");
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("form") };
    assert_eq!(f.value(0), "snd");
    key(&mut app, &w.cx, Key::Esc);
    assert!(app.modal.is_none() && cur(&app).tasks.mgr.list().is_empty());
}

#[test]
fn the_tab_never_overlaps_or_leaves_the_screen_from_1280x720_down_to_800x480_in_every_density_and_the_detail_becomes_a_page() {
    use super::super::ide_chrome::Density;
    let hold = Arc::new(AtomicBool::new(true));
    let hold2 = hold.clone();
    let _r = fake(Arc::new(move |spec, etx, _, _| {
        let _ = etx.send(RunEvent::Approval { id: "c1".into(), name: "write_file".into(), summary: "write a long file name that does not fit in a narrow card".into(), detail: String::new() });
        std::thread::sleep(Duration::from_millis(50));
        let _ = spec;
        let _ = etx;
        while hold2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }));
    let w = world("tasks-geometry");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    for i in 0..3 {
        new_task(&mut app, &w.cx, &format!("A task with a rather long title number {i} to test clipping"), "goal goal goal", "");
    }
    until(&mut app, &w.cx, "waiting", |a| cur(a).tasks.mgr.counts().1 == 3);
    let mut bad: Vec<String> = Vec::new();
    let (mut saw_narrow, mut saw_wide) = (false, false);
    for density in Density::ALL {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (800, 480)] {
            for page in [false] {
                app.show_main(MainTab::Tasks);
                cur_mut(&mut app).tasks.page = page;
                for tab in [DTab::Overview, DTab::Files, DTab::Diff, DTab::Log] {
                    cur_mut(&mut app).tasks.tab = tab;
                    ticks(&mut app, &w.cx, 2);
                    let g = render_at(&mut app, &w.cx, wd, ht);
                    let tag = format!("{} {wd}x{ht} page={page} {tab:?}", density.name());
                    let hits: Vec<_> = g.hits.iter().filter(|h| h.label != "divider" && h.rect.w > 0.0 && h.rect.h > 0.0).collect();
                    for h in &hits {
                        let r = h.rect;
                        if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
                            bad.push(format!("{tag}: {:?} is clipped by the screen edge ({:?})", h.label, r));
                        }
                        if matches!(h.ev, Event::Sub(ide_tasks::SUB, _)) && r.h < 19.5 {
                            bad.push(format!("{tag}: {:?} is only {:.1} px high", h.label, r.h));
                        }
                    }
                    let small: Vec<_> = hits.iter().filter(|h| h.rect.w * h.rect.h < (wd * ht) as f32 * 0.04 && (matches!(h.ev, Event::Sub(ide_tasks::SUB, _)) || matches!(h.ev, Event::Tab(n) if n >= ide_tasks::TAB_BASE))).collect();
                    for (i, x) in small.iter().enumerate() {
                        for y in &small[i + 1..] {
                            if partly_overlap(&x.rect, &y.rect) {
                                bad.push(format!("{tag}: {:?} {:?} overlaps {:?} {:?}", x.label, x.rect, y.label, y.rect));
                            }
                        }
                    }
                    // in a narrow pane the list and the detail are never both drawn (the detail is a page of its own)
                    let cards = hits.iter().filter(|h| h.label.starts_with("task t")).count();
                    let detail = hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if (ide_tasks::TAB_BASE..ide_tasks::TAB_BASE + 10).contains(&n)));
                    if cur(&app).tasks.narrow() && ((page && cards > 0) || (!page && detail)) {
                        bad.push(format!("{tag}: narrow pane draws {cards} cards and detail={detail}"));
                    }
                    if !cur(&app).tasks.narrow() && (cards == 0 || !detail) {
                        bad.push(format!("{tag}: wide pane lacks the list or the detail (cards {cards}, detail {detail})"));
                    }
                    saw_narrow |= cur(&app).tasks.narrow();
                    saw_wide |= !cur(&app).tasks.narrow();
                }
            }
        }
    }
    hold.store(false, Ordering::SeqCst);
    cur_mut(&mut app).tasks.page = false;
    assert!(saw_wide, "the two-column layout was exercised (narrow {saw_narrow})");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn closing_the_ide_with_a_running_task_saves_its_place_and_a_new_window_shows_it_interrupted() {
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let _r = fake(Arc::new(move |_, etx, cancel, _| {
        while h2.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "window closed".into() });
    }));
    let w = world("tasks-restart");
    let d = git_project(&w, "alpha");
    {
        let mut app = IdeApp::new(&w.cx);
        open(&mut app, &w.cx, "Alpha", &d);
        new_task(&mut app, &w.cx, "Long job", "long", "");
        until(&mut app, &w.cx, "running", |a| statuses(a) == vec![Status::Running]);
    }
    let mut app = IdeApp::new(&w.cx);
    let id = app.reg.ordered()[0].id.clone();
    app.open_by_id(&id, &w.cx).unwrap();
    assert_eq!(statuses(&app), vec![Status::Interrupted]);
    app.show_main(MainTab::Tasks);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Resume"), "Resume is offered: {:?} main={:?} focus={:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>(), cur(&app).main_tab, cur(&app).focus);
    hold.store(false, Ordering::SeqCst);
}

#[test]
fn in_a_narrow_pane_the_list_and_the_detail_are_pages_of_their_own_with_a_back_button() {
    use super::super::ide_tasks::TasksView;
    let hold = Arc::new(AtomicBool::new(false));
    let _r = fake(writer("n.txt", "n\n", hold));
    let w = world("tasks-narrow");
    let d = git_project(&w, "alpha");
    let mgr = iyakki_agent::tasks::Manager::open(&d, &d.join("FEATURES.md"));
    mgr.set_runner(super::super::ide_tasks_run::make_runner(&w.cx, &d));
    for i in 0..3 {
        mgr.spawn(iyakki_agent::tasks::NewTask { title: format!("Narrow task {i}"), goal: "g".into(), ..Default::default() }).unwrap();
    }
    let mut v = TasksView::new(mgr.clone(), "");
    let mut app_ticks = 0;
    while mgr.list().iter().any(|t| !matches!(t.status, Status::Done)) && app_ticks < 2000 {
        std::thread::sleep(Duration::from_millis(5));
        app_ticks += 1;
    }
    let draw = |v: &mut TasksView, wd: u32, ht: u32| {
        let mut g = Gfx::new(wd, ht);
        g.clear(crate::gfx::rgb(0, 0, 0));
        v.draw(&mut g, R::new(0.0, 0.0, wd as f32, ht as f32), 1.0, true, crate::gfx::rgb(100, 230, 255), 1);
        g
    };
    // list page
    let g = draw(&mut v, 420, 480);
    assert!(v.narrow());
    assert_eq!(g.hits.iter().filter(|h| h.label.starts_with("task t")).count(), 3);
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if (ide_tasks::TAB_BASE..ide_tasks::TAB_BASE + 10).contains(&n))), "no detail tabs on the list page");
    // a click on a card opens the detail page, with a way back
    let card = g.hits.iter().find(|h| h.label.starts_with("task t")).unwrap().ev.clone();
    v.handle(&card);
    let g = draw(&mut v, 420, 480);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if n == ide_tasks::TAB_BASE)), "the detail tabs");
    assert_eq!(g.hits.iter().filter(|h| h.label.starts_with("task t")).count(), 0, "no list on the detail page");
    let back = g.hits.iter().find(|h| h.label == "back to the list").expect("a back button").ev.clone();
    v.handle(&back);
    let g = draw(&mut v, 420, 480);
    assert_eq!(g.hits.iter().filter(|h| h.label.starts_with("task t")).count(), 3, "Esc or the button goes back to the list");
    // wide: both
    let g = draw(&mut v, 1000, 600);
    assert!(!v.narrow());
    assert!(g.hits.iter().filter(|h| h.label.starts_with("task t")).count() == 3 && g.hits.iter().any(|h| h.label == "Merge"));
    mgr.shutdown();
}

#[test]
fn claude_code_tasks_each_run_in_their_own_tree_with_their_own_session_and_never_resume_another() {
    use std::os::unix::fs::PermissionsExt;
    // the real runner (Autopilot with the external agent as executor) and a fake CLI: no vendor is called
    let _l = RUNNER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use_runner(None);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            use_runner(None);
        }
    }
    let _reset = Reset;
    let w = world("tasks-ext");
    let d = git_project(&w, "alpha");
    let bin = w.base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("claude");
    // logs "<task id> <args>" per call; the session id it reports is derived from the folder it runs in, so a session of one task
    // resumed in another would show
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 2.1.0; exit 0;; auth) echo '{{\"loggedIn\":true}}'; exit 0;; esac\nid=$(basename $(dirname \"$PWD\"))\nline=$(echo \"$id $PWD $*\" | tr '\\n' ' '); printf '%s\\n' \"$line\" >> {d}/calls.log\ncat <<JSONL\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-$id\",\"model\":\"m\"}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"Answered in words.\",\"session_id\":\"sess-$id\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":10,\"output_tokens\":5}}}}\nJSONL\n",
            d = bin.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    w.cx.services.settings.lock().unwrap().update(|s| s.agents.claude.command = script.display().to_string()).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let ids = app
        .spawn_tasks(
            vec![
                iyakki_agent::tasks::NewTask { title: "First".into(), goal: "make a".into(), agent: "claude-code".into(), ..Default::default() },
                iyakki_agent::tasks::NewTask { title: "Second".into(), goal: "make b".into(), agent: "claude-code".into(), ..Default::default() },
            ],
            &w.cx,
        )
        .unwrap();
    until(&mut app, &w.cx, "both tasks to end", |a| statuses(a).iter().all(|s| !matches!(s, Status::Queued | Status::Running | Status::NeedsYou)));
    let log = std::fs::read_to_string(bin.join("calls.log")).unwrap();
    for id in &ids {
        let mine: Vec<&str> = log.lines().filter(|l| l.starts_with(&format!("{id} "))).collect();
        assert!(mine.len() >= 2, "task {id} ran its steps through the agent: {log}");
        assert!(mine.iter().all(|l| l.contains(&format!("/tasks/{id}/tree"))), "the agent works in the task's own tree: {mine:?}");
        assert!(!mine[0].contains("--resume"), "the first step starts a fresh session: {}", mine[0]);
        assert!(mine[1].contains(&format!("--resume sess-{id}")), "the next step resumes this task's own session: {}", mine[1]);
        assert!(mine.iter().all(|l| ids.iter().filter(|o| *o != id).all(|o| !l.contains(&format!("sess-{o}")))), "never another task's session: {mine:?}");
        assert_eq!(w.cx.services.ext_sessions.get(&format!("task-{id}"), "claude-code").as_deref(), Some(format!("sess-{id}").as_str()), "the session is stored under the task's own key");
    }
    assert!(w.cx.services.ext_sessions.get("task-nothing", "claude-code").is_none());
}

#[test]
fn the_new_task_form_takes_budgets_and_refuses_a_number_that_is_not_one() {
    let _r = fake(writer("b.txt", "b\n", Arc::new(AtomicBool::new(false))));
    let w = world("tasks-budgets");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.show_main(MainTab::Tasks);
    key(&mut app, &w.cx, Key::Char('n'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("form") };
    f.set_value(0, "Budgeted");
    f.set_value(1, "goal");
    f.set_value(4, "12");
    f.set_value(5, "50,000");
    f.set_value(6, "lots");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("the form stays open with the problem") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("lots")), "{:?}", f.error);
    f.set_value(6, "250");
    assert!(cur(&app).tasks.mgr.list().is_empty(), "nothing started with a bad number");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none());
    let t = cur(&app).tasks.mgr.list().remove(0);
    assert_eq!((t.budgets.max_minutes, t.budgets.max_tokens, t.budgets.max_cost_cents), (12, 50_000, 250));
}

#[test]
fn a_new_task_goal_that_starts_with_remember_changes_the_context_and_the_rest_is_the_task() {
    let _r = fake(writer("c.txt", "c\n", Arc::new(AtomicBool::new(false))));
    let w = world("tasks-ctxgoal");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    // only an instruction: the context changes, no task is made
    new_task(&mut app, &w.cx, "Rule", "remember that we use pnpm", "");
    assert!(cur(&app).tasks.mgr.list().is_empty(), "nothing to do but the note");
    let notes = iyakki_agent::notes::load(&cur(&app).root()).0;
    let all: Vec<String> = notes.view(&iyakki_agent::notes::Filter::default()).iter().map(|n| format!("{} {}", n.title, n.text)).collect();
    assert!(all.iter().any(|t| t.contains("pnpm")), "the context has the rule: {all:?}");
    assert!(cur(&app).ctx_undo.is_some(), "and it can be undone");
    // an instruction and a task: the task is what is left
    new_task(&mut app, &w.cx, "Both", "remember that tabs are four spaces. Then add the cart page", "");
    let tasks = cur(&app).tasks.mgr.list();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].goal.contains("cart page") && !tasks[0].goal.contains("remember"), "{}", tasks[0].goal);
    // a plain goal is untouched
    new_task(&mut app, &w.cx, "Plain", "I remember when this was slow", "");
    assert!(cur(&app).tasks.mgr.list().iter().any(|t| t.goal == "I remember when this was slow"));
}

// ------------------------------------------------------------------------------------------------ agents on the board

fn board_click(app: &mut IdeApp, cx: &AppCx, label: &str) {
    let g = render(app, cx, 1280, 720);
    let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control {label:?} in {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
    let ev = h.ev.clone();
    app.handle(&ev, cx);
}

const DEP_BOARD: &str = "<!-- summary:start -->done 0 · partial 0 · planned 3 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Depends | Notes |\n|---|---|---|---|---|---|---|---|---|\n| F-A-01 | Make the base | P1 | planned | P0 | 0% | — |  |  |\n| F-A-02 | Build on the base | P1 | planned | P0 | 0% | — | F-A-01 |  |\n| F-A-03 | Free standing | P2 | planned | P1 | 0% | — |  |  |\n";

#[test]
fn running_tasks_are_agents_on_the_board_that_can_be_opened_paused_messaged_and_stopped_from_there() {
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let guided = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let g2 = guided.clone();
    let _r = fake(Arc::new(move |spec, etx, cancel, _| {
        g2.lock().unwrap().push(spec.interjection.clone());
        std::fs::write(spec.tree.join("work.txt"), format!("{}\n", spec.task.title)).unwrap();
        let _ = etx.send(RunEvent::Note { text: "autopilot: step 1 · row 1 of 1 · F-A-02 Build on the base".into() });
        if spec.interjection.is_some() {
            return let_finish(&etx);
        }
        while h2.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = etx.send(RunEvent::Stopped { reason: "cut".into() });
    }));
    let w = world("board-agents");
    let d = git_project(&w, "alpha");
    std::fs::write(d.join("FEATURES.md"), DEP_BOARD).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    new_task(&mut app, &w.cx, "Second row", "do it", "F-A-03");
    until(&mut app, &w.cx, "an agent with a file", |_| iyakki_agent::assign::load(&d).agents.iter().any(|x| x.files.iter().any(|f| f.path == "work.txt")));
    app.show_main(MainTab::Board);
    // the board header and the strip say so
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "agent Agent 1"), "{:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
    assert_eq!(cur(&app).board.ext.agents_working(), 1);
    // drill down: the panel shows the timeline of the task
    board_click(&mut app, &w.cx, "agent Agent 1");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "agent Stop"));
    assert!(cur(&app).board.ext.drilled().is_some_and(|a| a.steps.iter().any(|s| s.text.contains("row 1 of 1"))));
    // message the agent: the form opens, the agent pauses after its step and continues with the text
    board_click(&mut app, &w.cx, "agent Message");
    let Some(Modal::Form(FormKind::GuideAgent(id), f)) = &mut app.modal else { panic!("the guidance form: {:?}", app.modal.as_ref().map(|_| "a modal")) };
    let id = id.clone();
    f.set_value(0, "use the other port");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the second run with the message", |a| cur(a).tasks.mgr.get(&id).is_some_and(|t| t.interjection == "use the other port") || guided.lock().unwrap().len() >= 2);
    until(&mut app, &w.cx, "done", |a| cur(a).tasks.mgr.get(&id).is_some_and(|t| t.status == Status::Done));
    assert_eq!(guided.lock().unwrap().last().cloned().flatten().as_deref(), Some("use the other port"));
    assert!(iyakki_agent::assign::load(&d).get(&id).unwrap().steps.iter().any(|s| s.text.contains("you: use the other port")));
    // a stop from the panel: a new task, stopped
    hold.store(true, Ordering::SeqCst);
    guided.lock().unwrap().clear();
    new_task(&mut app, &w.cx, "Third", "x", "F-A-01");
    app.show_main(MainTab::Board);
    until(&mut app, &w.cx, "agent 2", |a| iyakki_agent::assign::load(&d).agents.len() == 2 && cur(a).tasks.mgr.counts().0 == 1);
    board_click(&mut app, &w.cx, "agent Agent 2");
    board_click(&mut app, &w.cx, "agent Stop");
    until(&mut app, &w.cx, "stopped", |a| cur(a).tasks.mgr.list().iter().any(|t| t.title == "Third" && t.status == Status::Failed));
    // the agent's record follows the task's status a moment later (another thread writes it): wait for it, then check it
    until(&mut app, &w.cx, "the agent record says failed", |_| iyakki_agent::assign::load(&d).by_name("Agent 2").is_some_and(|a| a.state == iyakki_agent::assign::AState::Failed));
    assert_eq!(iyakki_agent::assign::load(&d).by_name("Agent 2").unwrap().state, iyakki_agent::assign::AState::Failed);
    // Open run hands the task's run log to RUNS (the shell keeps the IDE aside); the log was recorded with the task, row and agent
    let run = iyakki_agent::assign::load(&d).by_name("Agent 2").unwrap().run_id.clone();
    assert!(!run.is_empty());
    board_click(&mut app, &w.cx, "agent Open run");
    assert_eq!(App::take_request(&mut app), Some(("runs".to_string(), format!("run:{run}"))));
    let metas = w.cx.services.runs.list(&w.cx.session.user);
    let m = metas.iter().find(|m| m.id == run).expect("the run is listed");
    assert_eq!(m.task_label(), format!("Task {} · Agent 2 · F-A-01", m.task_id.clone().unwrap()));
    // and from the run back: the task or the row
    let tid = m.task_id.clone().unwrap();
    App::open_item(&mut app, &format!("task:{tid}"), &w.cx);
    assert_eq!(cur(&app).main_tab, MainTab::Tasks);
    assert_eq!(cur(&app).tasks.selected().unwrap().title, "Third");
    App::open_item(&mut app, "row:F-A-01", &w.cx);
    assert_eq!((cur(&app).main_tab, cur(&app).board.selected().unwrap().id), (MainTab::Board, "F-A-01".to_string()));
}

fn let_finish(etx: &Sender<RunEvent>) {
    let _ = etx.send(RunEvent::Final { text: "Autopilot finished: 1 of 1 rows done.".into() });
}

#[test]
fn ready_rows_are_offered_in_the_strip_started_with_a_click_and_the_auto_assign_setting_is_kept() {
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let _r = fake(Arc::new(move |_, etx, _, _| {
        while h2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let_finish(&etx);
    }));
    let w = world("board-ready");
    let d = git_project(&w, "alpha");
    std::fs::write(d.join("FEATURES.md"), DEP_BOARD).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.show_main(MainTab::Board);
    cur_mut(&mut app).board.ext.poll_every = Duration::ZERO;
    let g = render(&mut app, &w.cx, 1280, 720);
    let ready = g.hits.iter().find(|h| h.label.contains("ready: F-A-01, F-A-03")).map(|h| h.label.clone()).expect("F-A-01 and F-A-03 are ready, F-A-02 waits");
    assert!(ready.contains("2 ready"), "{ready}");
    // the setting: ask, on, off and it is remembered by the workspace
    board_click(&mut app, &w.cx, "auto-assign: ask");
    assert_eq!(cur(&app).tasks.mgr.auto_assign(), iyakki_agent::tasks::AutoAssign::On);
    assert!(std::fs::read_to_string(d.join(".iyakki/tasks/config.json")).unwrap().contains("\"auto_assign\":\"on\""));
    board_click(&mut app, &w.cx, "auto-assign: on");
    assert_eq!(cur(&app).tasks.mgr.auto_assign(), iyakki_agent::tasks::AutoAssign::Off);
    board_click(&mut app, &w.cx, "auto-assign: off");
    assert_eq!(cur(&app).tasks.mgr.auto_assign(), iyakki_agent::tasks::AutoAssign::Ask);
    // one click starts an agent on each ready row; the board stays in front; the waiting row is not started
    board_click(&mut app, &w.cx, &ready);
    assert_eq!(cur(&app).main_tab, MainTab::Board);
    let rows: Vec<Vec<String>> = cur(&app).tasks.mgr.list().iter().map(|t| t.rows.clone()).collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.contains(&vec!["F-A-01".to_string()]) && rows.contains(&vec!["F-A-03".to_string()]) && !rows.contains(&vec!["F-A-02".to_string()]));
    until(&mut app, &w.cx, "both running", |a| cur(a).tasks.mgr.counts().0 == 2);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "agent Agent 1") && g.hits.iter().any(|h| h.label == "agent Agent 2"));
    assert!(!g.hits.iter().any(|h| h.label.contains("ready: F-A-01")), "rows that have an agent are not offered again");
    // a task for the waiting row is made but waits for the first row to be done
    new_task(&mut app, &w.cx, "Waiting", "w", "F-A-02");
    app.show_main(MainTab::Board);
    assert_eq!(cur(&app).tasks.mgr.list().iter().find(|t| t.title == "Waiting").unwrap().status, Status::Queued);
    assert_eq!(cur(&app).tasks.mgr.waiting_on(&cur(&app).tasks.mgr.list().iter().find(|t| t.title == "Waiting").unwrap().id), ["F-A-01"]);
    hold.store(false, Ordering::SeqCst);
    until(&mut app, &w.cx, "the first two done", |a| cur(a).tasks.mgr.list().iter().filter(|t| t.status == Status::Done).count() >= 2);
}

#[test]
fn the_board_at_the_four_sizes_with_agents_dependencies_and_a_panel_never_leaves_the_screen() {
    use super::super::ide_chrome::Density;
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let _r = fake(Arc::new(move |spec, etx, _, _| {
        std::fs::write(spec.tree.join("work.txt"), "x\n").unwrap();
        let _ = etx.send(RunEvent::Note { text: "autopilot: step 1".into() });
        while h2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }));
    let w = world("board-geometry");
    let d = git_project(&w, "alpha");
    std::fs::write(d.join("FEATURES.md"), DEP_BOARD).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).board.ext.poll_every = Duration::ZERO;
    new_task(&mut app, &w.cx, "One", "a", "F-A-01");
    new_task(&mut app, &w.cx, "Three", "c", "F-A-03");
    until(&mut app, &w.cx, "two agents", |_| iyakki_agent::assign::load(&d).agents.len() == 2);
    let mut bad: Vec<String> = Vec::new();
    use super::super::board::Mode as BMode;
    for density in [Density::ALL[0], Density::ALL[Density::ALL.len() - 1]] {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for (mode, open) in [(BMode::Kanban, false), (BMode::Kanban, true), (BMode::Table, false), (BMode::Graph, false), (BMode::Graph, true), (BMode::Phases, false)] {
                {
                    app.show_main(MainTab::Board);
                    cur_mut(&mut app).board.mode = mode;
                    cur_mut(&mut app).board.ext.drill = open.then(|| cur(&app).board.ext.doc.agents[0].key.clone());
                    ticks(&mut app, &w.cx, 1);
                    let g = render_at(&mut app, &w.cx, wd, ht);
                    let tag = format!("{} {wd}x{ht} {mode:?} panel={open}", density.name());
                    for h in g.hits.iter().filter(|h| h.rect.w > 0.0 && h.rect.h > 0.0 && h.label != "divider") {
                        let r = h.rect;
                        if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
                            bad.push(format!("{tag}: {:?} leaves the screen ({:?})", h.label, r));
                        }
                    }
                    let chips: Vec<_> = g.hits.iter().filter(|h| h.label.starts_with("agent Agent") || h.label.starts_with("agent:") || h.label.starts_with("auto-assign")).collect();
                    for (i, a) in chips.iter().enumerate() {
                        for b in &chips[i + 1..] {
                            if partly_overlap(&a.rect, &b.rect) {
                                bad.push(format!("{tag}: {:?} overlaps {:?}", a.label, b.label));
                            }
                        }
                    }
                    if open && !g.hits.iter().any(|h| h.label == "close the agent panel") {
                        bad.push(format!("{tag}: no panel"));
                    }
                }
            }
        }
    }
    cur_mut(&mut app).board.ext.drill = None;
    hold.store(false, Ordering::SeqCst);
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn the_tab_with_low_memory_shows_the_lowered_cap_and_a_waiting_task_at_the_four_sizes_without_overlap() {
    use super::super::ide_chrome::Density;
    let hold = Arc::new(AtomicBool::new(true));
    let h2 = hold.clone();
    let _r = fake(Arc::new(move |_, _, _, _| {
        while h2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }));
    let w = world("tasks-lowmem");
    let d = git_project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    // 2.5 GB free: the reserve and one task, so only one of the three runs although the maximum is 3
    cur(&app).tasks.mgr.set_memory_source(Some(Arc::new(|| Some((2u64 << 30) + (1 << 29)))));
    for i in 0..3 {
        new_task(&mut app, &w.cx, &format!("Task {i}"), "goal", "");
    }
    until(&mut app, &w.cx, "one running, two waiting", |a| cur(a).tasks.mgr.counts() == (1, 0, 2));
    let mgr = cur(&app).tasks.mgr.clone();
    assert_eq!((mgr.concurrency(), mgr.effective_concurrency()), (3, 1));
    assert!(mgr.list().iter().any(|t| t.last.starts_with("waiting for memory")), "{:?}", mgr.list().iter().map(|t| t.last.clone()).collect::<Vec<_>>());
    let mut bad: Vec<String> = Vec::new();
    for density in [Density::ALL[0], Density::ALL[Density::ALL.len() - 1]] {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            app.show_main(MainTab::Tasks);
            cur_mut(&mut app).tasks.page = false;
            ticks(&mut app, &w.cx, 2);
            let g = render_at(&mut app, &w.cx, wd, ht);
            let tag = format!("{} {wd}x{ht}", density.name());
            let hits: Vec<_> = g.hits.iter().filter(|h| h.label != "divider" && h.rect.w > 0.0 && h.rect.h > 0.0).collect();
            for h in &hits {
                let r = h.rect;
                if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
                    bad.push(format!("{tag}: {:?} leaves the screen ({:?})", h.label, r));
                }
            }
            let less = hits.iter().find(|h| h.label == "fewer at once");
            let more = hits.iter().find(|h| h.label == "more at once");
            match (less, more) {
                (Some(a), Some(b)) => {
                    if partly_overlap(&a.rect, &b.rect) || b.rect.x < a.rect.x + a.rect.w {
                        bad.push(format!("{tag}: the cap buttons overlap or are out of order ({:?} {:?})", a.rect, b.rect));
                    }
                    // the label "max 3 · now 1" sits between them: room for it
                    if b.rect.x - (a.rect.x + a.rect.w) < 40.0 {
                        bad.push(format!("{tag}: no room between the cap buttons for 'max 3 · now 1'"));
                    }
                }
                _ => bad.push(format!("{tag}: the cap buttons are missing")),
            }
        }
    }
    // the stepper still changes the setting, and the effective cap never exceeds it
    key(&mut app, &w.cx, Key::Char('-'));
    assert_eq!((mgr.concurrency(), mgr.effective_concurrency()), (2, 1));
    hold.store(false, Ordering::SeqCst);
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}
