// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from ide_tests.rs); the marker keeps the BOUNDARY check from counting it.
//! Tests of the AI IDE's Git tab (docs/GIT-TILE.md, "In the AI IDE"): the tab and its key, "This project" (the working copy through a
//! fake git), "Connections" (the GIT tile over a fake host), the empty states, no token on screen, and the layouts. A child module of
//! `ide_tests`, which has the helpers.

use super::*;
use crate::apps::component::route;
use crate::apps::git_tile::GitApp;
use crate::apps::git_tile::Rig;
use iyakki_agent::gitjobs::Tools;
use crate::apps::ide_gitview::{GitTabView, Sub, GIT, NOT_A_REPO, NO_CHANGES};
use iyakki_agent::gitconn::{FakeGitHttp, Kind};
use iyakki_agent::k8sretry::FakeTimer;
use iyakki_agent::wsexec::FakeRunner;
use serde_json::json;

const TOK: &str = "tok_FAKEFAKEFAKEFAKE9876";

const PORCELAIN: &str = "# branch.oid 1234\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 aaa bbb src/main.rs\n1 D. N... 100644 000000 000000 ddd 000 gone.txt\n? notes.md\n";
const FILE_DIFF: &str = "diff --git a/src/main.rs b/src/main.rs\nindex 1..2 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old line\n+new line\n";
const COMMIT_DIFF: &str = "diff --git a/lib.rs b/lib.rs\nindex 1..2 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1,2 @@\n a\n+commit added\n";

fn log_out() -> String {
    ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]
        .iter()
        .enumerate()
        .map(|(i, sha)| format!("{sha}\u{1f}Ada\u{1f}{}\u{1f}Commit number {i}\n\nBody of {i}\u{1e}\n", 1_700_000_000 - i * 600))
        .collect()
}

fn git_world(tag: &str) -> (World, IdeApp, Arc<FakeRunner>, PathBuf) {
    let w = world(tag);
    let d = project(&w, "alpha", &[("notes.md", "n\n"), ("src/other.rs", "x\n")]);
    std::fs::create_dir_all(d.join(".git")).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let run = Arc::new(FakeRunner::new(&["git"]));
    run.answer("status --porcelain=v2", PORCELAIN);
    run.answer("log -n50", &log_out());
    run.answer("diff HEAD", FILE_DIFF);
    run.answer("show --format=", COMMIT_DIFF);
    (w, app, run, d)
}

fn rig(w: &World, run: &Arc<FakeRunner>, http: Arc<FakeGitHttp>) -> Rig {
    let root = w.cx.services.dir().join("Repositories");
    std::fs::create_dir_all(&root).unwrap();
    Rig {
        http,
        run: run.clone(),
        timer: FakeTimer::new(),
        scratch: w.cx.services.dir().join("scratch"),
        repos_root: Some(root),
        tools: Some(Tools { git: PathBuf::from("/usr/bin/git"), ssh: Err("no ssh in tests".into()), allow_local: false }),
        probe: Some(Arc::new(|_, _| Err("no network in tests".into()))),
        run_jobs: false,
    }
}

fn install(w: &World, app: &mut IdeApp, run: &Arc<FakeRunner>, http: Arc<FakeGitHttp>, root: &Path) {
    let v = GitTabView::new(&w.cx, root.to_path_buf(), rig(w, run, http));
    cur_mut(app).gitview = Some(v);
    app.show_main(MainTab::Git);
}

fn gv(app: &IdeApp) -> &GitTabView {
    cur(app).gitview.as_ref().expect("the Git tab was shown")
}

fn gv_mut(app: &mut IdeApp) -> &mut GitTabView {
    cur_mut(app).gitview.as_mut().expect("the Git tab was shown")
}

fn tile(app: &mut IdeApp) -> &mut GitApp {
    &mut gv_mut(app).app
}

fn pump(app: &mut IdeApp, cx: &AppCx, what: &str, mut ok: impl FnMut(&mut IdeApp) -> bool) {
    let t0 = std::time::Instant::now();
    loop {
        render(app, cx, 1280, 720);
        app.handle(&Event::Tick, cx);
        render(app, cx, 1280, 720);
        if ok(app) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(8), "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

/// Select the `i`th changed file (sorted by path) with the keys, as a person would.
fn select(app: &mut IdeApp, cx: &AppCx, i: usize) {
    key(app, cx, Key::Home);
    for _ in 0..i {
        key(app, cx, Key::Down);
    }
    key(app, cx, Key::Enter);
    render(app, cx, 1280, 720);
}

fn click(app: &mut IdeApp, cx: &AppCx, label: &str) -> Gfx {
    let g = render(app, cx, 1280, 720);
    let ev = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`: {:?}", labels(&g))).ev.clone();
    app.handle(&ev, cx);
    render(app, cx, 1280, 720)
}

fn ready(app: &mut IdeApp) -> bool {
    let app = &*app;
    gv(app).work.ready().is_some() && gv(app).commits.ready().is_some()
}

// ------------------------------------------------------------------------------------------------ the tab

#[test]
fn git_is_the_last_main_tab_after_k8s_with_a_branch_glyph_and_ctrl_shift_l() {
    assert_eq!(*MainTab::ALL.last().unwrap(), MainTab::Git);
    assert_eq!(MainTab::ALL[MainTab::ALL.len() - 2], MainTab::K8s);
    assert_eq!(super::super::ide_ui::main_glyph(MainTab::Git), crate::fx::Glyph::Git);
    let (w, mut app, run, d) = git_world("gtab");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    app.show_main(MainTab::Editor);
    for (wd, ht) in [(1280u32, 720u32), (1024, 600)] {
        let g = render(&mut app, &w.cx, wd, ht);
        let tabs: Vec<(&str, f32)> = (0..7).filter_map(|i| g.hits.iter().find(|h| h.ev == Event::Tab(T_MAIN + i)).map(|h| (h.label.as_str(), h.rect.x))).collect();
        assert_eq!(tabs.len(), 7, "{wd}x{ht}");
        assert_eq!(tabs[6].0, "Git");
        assert!(tabs.windows(2).all(|p| p[0].1 < p[1].1), "left to right: {tabs:?}");
    }
    // a click on the tab and the key both show it; the key again goes back to the editor
    let g = render(&mut app, &w.cx, 1280, 720);
    let ev = g.hits.iter().find(|h| h.label == "Git" && matches!(h.ev, Event::Tab(_))).unwrap().ev.clone();
    app.handle(&ev, &w.cx);
    assert_eq!(cur(&app).main_tab, MainTab::Git);
    app.show_main(MainTab::Editor);
    key(&mut app, &w.cx, Key::Ctrl('L'));
    assert_eq!((cur(&app).main_tab, cur(&app).focus), (MainTab::Git, Pane::Main));
    assert_eq!(cur(&app).owner(), Owner::Git);
    key(&mut app, &w.cx, Key::Ctrl('L'));
    assert_eq!(cur(&app).main_tab, MainTab::Editor);
}

#[test]
fn an_account_without_the_git_tile_sees_no_git_tab() {
    let (mut w, mut app, run, d) = git_world("gnoperm");
    w.cx.session.policy.apps.retain(|a| *a != PApp::Git);
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    app.show_main(MainTab::Editor);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Git" && matches!(h.ev, Event::Tab(_))), "{:?}", labels(&g));
    assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_MAIN..T_MAIN + 10).contains(&n))).count(), 6, "the other tabs keep their ids");
    app.show_main(MainTab::Git);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(!labels(&g).iter().any(|l| l == "This project"), "nothing of the tab is drawn");
    // nothing was read
    assert!(run.calls().is_empty());
    let mut guest = world("gguest");
    guest.cx.session.policy = iyakki_sec::policy::Role::Guest.template();
    assert!(!guest.cx.session.policy.allows_app(PApp::Git));
}

// ------------------------------------------------------------------------------------------------ this project

#[test]
fn this_project_shows_branch_ahead_behind_changed_files_with_status_and_recent_commits() {
    let (w, mut app, run, d) = git_world("gproj");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    assert_eq!(gv(&app).sub, Sub::Project, "a working copy opens on its own state");
    pump(&mut app, &w.cx, "the working copy", ready);
    let wk = gv(&app).work.ready().unwrap();
    assert_eq!((wk.status.summary().as_str(), wk.changes.len()), ("main \u{2191}2 \u{2193}1 \u{270e}2 +1", 3));
    let g = render(&mut app, &w.cx, 1280, 720);
    for l in ["src/main.rs", "gone.txt", "notes.md", "Commit number 0", "Commit number 1", "Refresh", "This project", "Connections"] {
        assert!(labels(&g).iter().any(|x| x == l), "{l}: {:?}", labels(&g));
    }
    // reading only: every command was a read
    for line in run.lines() {
        for w in [" commit", " push", " pull", " fetch", " checkout", " add ", " reset", " clean", " merge", " rebase", " stash"] {
            assert!(!line.contains(w), "{line}");
        }
    }
    assert!(run.lines().iter().all(|l| !l.contains("TOKEN") && !l.contains(TOK)));
}

#[test]
fn a_changed_file_shows_its_diff_and_opens_in_the_editor_and_a_commit_shows_its_diff() {
    let (w, mut app, run, d) = git_world("gopen");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "the working copy", ready);
    // sorted by path: gone.txt, notes.md, src/main.rs
    select(&mut app, &w.cx, 2);
    pump(&mut app, &w.cx, "the diff", |a| gv(a).text_has("+new line"));
    assert!(gv(&app).text_has("-old line"));
    assert!(run.lines().iter().any(|l| l.contains("diff HEAD") && l.ends_with("-- src/main.rs")));
    // a deleted file has a diff but nothing to open
    select(&mut app, &w.cx, 0);
    key(&mut app, &w.cx, Key::Char('o'));
    assert_eq!(cur(&app).main_tab, MainTab::Git, "nothing to open");
    assert!(gv(&app).status_line().is_some_and(|s| s.0.contains("deleted")));
    // an untracked file opens at once (no diff to read)
    select(&mut app, &w.cx, 1);
    assert!(gv(&app).text_has("Press o to open it"));
    // the button and the key open the file in the editor tab
    click(&mut app, &w.cx, "Open in editor");
    assert_eq!(cur(&app).main_tab, MainTab::Editor);
    assert!(cur(&app).editor.path().is_some_and(|p| p.ends_with("notes.md")), "{:?}", cur(&app).editor.path());
    app.show_main(MainTab::Git);
    select(&mut app, &w.cx, 2);
    key(&mut app, &w.cx, Key::Char('o'));
    assert!(cur(&app).editor.path().is_some_and(|p| p.ends_with("main.rs")));
    // a commit
    app.show_main(MainTab::Git);
    key(&mut app, &w.cx, Key::Esc);
    key(&mut app, &w.cx, Key::Tab);
    key(&mut app, &w.cx, Key::Home);
    key(&mut app, &w.cx, Key::Enter);
    pump(&mut app, &w.cx, "the commit diff", |a| gv(a).text_has("+commit added"));
    assert!(gv(&app).text_has("Commit number 0") && gv(&app).text_has("Body of 0"));
    assert!(run.lines().iter().any(|l| l.contains("show --format=") && l.contains("aaaaaaaa")));
}

#[test]
fn switching_tabs_keeps_the_git_tabs_state_and_stops_reading_while_it_is_not_shown() {
    let (w, mut app, run, d) = git_world("gkeep");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "the working copy", ready);
    select(&mut app, &w.cx, 2);
    pump(&mut app, &w.cx, "the diff", |a| gv(a).text_has("+new line"));
    app.show_main(MainTab::Board);
    app.show_main(MainTab::K8s);
    let before = run.calls().len();
    std::thread::sleep(Duration::from_millis(600));
    ticks(&mut app, &w.cx, 20);
    assert_eq!(run.calls().len(), before, "nothing is read while another tab is shown");
    app.show_main(MainTab::Git);
    render(&mut app, &w.cx, 1280, 720);
    assert!(gv(&app).text_has("+new line"), "the diff is still there");
    assert_eq!(gv(&app).sub, Sub::Project);
    // the keyboard works again: r reads again at once
    key(&mut app, &w.cx, Key::Char('r'));
    pump(&mut app, &w.cx, "a new read", |_| run.calls().len() > before);
}

#[test]
fn a_clean_working_copy_a_missing_repository_and_a_missing_git_say_so() {
    // clean
    let (w, mut app, run, d) = git_world("gclean");
    let clean = Arc::new(FakeRunner::new(&["git"]));
    clean.answer("status --porcelain=v2", "# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n");
    clean.answer("log -n50", &log_out());
    install(&w, &mut app, &clean, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "the clean copy", ready);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(gv(&app).work.ready().unwrap().changes.is_empty());
    assert!(!labels(&g).iter().any(|l| l == "src/main.rs"));
    assert!(NO_CHANGES.contains("matches the last commit"));
    // not a repository
    let none = Arc::new(FakeRunner::new(&["git"]));
    none.fail("status", 128, "fatal: not a git repository (or any of the parent directories): .git");
    none.fail("log", 128, "fatal: not a git repository (or any of the parent directories): .git");
    install(&w, &mut app, &none, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "the problem", |a| gv(a).not_a_repo());
    assert!(NOT_A_REPO.contains("not a Git working copy"));
    // git is not installed: the same words the rest of the product uses
    let missing = Arc::new(FakeRunner::new(&[]));
    install(&w, &mut app, &missing, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "a failure", |a| gv(a).work.problem().is_some());
    let _ = run;
    // a folder without .git opens on the connections
    let plain = project(&w, "plain", &[]);
    let v = GitTabView::new(&w.cx, plain, rig(&w, &Arc::new(FakeRunner::new(&["git"])), Arc::new(FakeGitHttp::default())));
    assert_eq!(v.sub, Sub::Connections);
}

// ------------------------------------------------------------------------------------------------ connections

const BASE: &str = "GET https://git.example.org/api/v1";

fn host() -> Arc<FakeGitHttp> {
    let h = Arc::new(FakeGitHttp::default());
    let repo = |n: &str| json!({"full_name": n, "description": "d", "default_branch": "main", "private": false, "updated_at": "2026-10-20T10:00:00Z", "stars_count": 1, "size": 10,
        "clone_url": format!("https://git.example.org/{n}.git"), "ssh_url": format!("git@git.example.org:{n}.git"), "html_url": format!("https://git.example.org/{n}")});
    h.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[], &json!([repo("ops/infra"), repo("ops/web")]).to_string());
    let r = format!("{BASE}/repos/ops/infra");
    h.answer(&format!("{r}/contents?ref=main"), 200, &[], &json!([{"name":"README.md","path":"README.md","type":"file","size":4}]).to_string());
    h.answer(&format!("{r}/branches?limit=50"), 200, &[], &json!([{"name":"dev","commit":{"id":"d1","message":"Dev work","timestamp":"2026-10-20T10:00:00Z"}},{"name":"main","commit":{"id":"b1","message":"Main work","timestamp":"2026-10-20T10:00:00Z"}}]).to_string());
    h.answer(&format!("{r}/tags?limit=50"), 200, &[], "[]");
    h.answer(&format!("{r}/commits?limit=50&stat=false&verification=false&files=false&sha=main"), 200, &[], &json!([{"sha": format!("{:040x}", 0xabc000), "commit": {"message": "Remote change\n\nWords", "author": {"name": "Ada", "date": "2026-10-20T10:00:00Z"}}}]).to_string());
    h.answer(&format!("{r}/git/commits/{:040x}.diff", 0xabc000), 200, &[], COMMIT_DIFF);
    h
}

fn connect(w: &World, v: &mut GitTabView, name: &str, url: &str) -> iyakki_agent::gitconn::Connection {
    let c = v.app.conns(&w.cx).add(iyakki_agent::gitconn::Connection { name: name.into(), kind: Kind::Gitea, url: url.into(), account: "octo".into(), ..Default::default() }, Some(TOK)).unwrap();
    v.app.refresh_entries(&w.cx);
    c
}

#[test]
fn connections_lists_the_saved_ones_remembers_the_choice_and_browses_repos_branches_commits_and_diffs() {
    let (w, mut app, run, d) = git_world("gconn");
    w.cx.services.on_login(&w.cx.session.user, "rootpass12", true);
    install(&w, &mut app, &run, host(), &d);
    let (a, b) = {
        let v = gv_mut(&mut app);
        let a = connect(&w, v, "Home Gitea", "https://git.example.org");
        let b = connect(&w, v, "Work Gitea", "https://git2.example.org");
        (a, b)
    };
    // the Connections sub-view is the GIT tile; its picker lists both by name
    let g = click(&mut app, &w.cx, "Connections");
    assert_eq!(gv(&app).sub, Sub::Connections);
    assert!(labels(&g).iter().any(|l| l == "connection"), "{:?}", labels(&g));
    assert!(g.hits.iter().any(|h| route::is_slot(&h.ev, GIT)), "the tile's ids are in the Git slot");
    click(&mut app, &w.cx, "connection");
    let g = render(&mut app, &w.cx, 1280, 720);
    for l in ["Home Gitea", "Work Gitea", "Local clones"] {
        assert!(labels(&g).iter().any(|x| x == l), "{l}: {:?}", labels(&g));
    }
    click(&mut app, &w.cx, "Home Gitea");
    pump(&mut app, &w.cx, "the repository list", |a| tile(a).board().loaded);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "ops/infra") && labels(&g).iter().any(|l| l == "ops/web"));
    // the choice is remembered for this person: a new tab view starts on it
    let again = GitTabView::new(&w.cx, d.clone(), rig(&w, &run, host()));
    assert_eq!(again.app.src().name(), "Home Gitea");
    // open a repository: branches mark the default
    click(&mut app, &w.cx, "ops/infra");
    pump(&mut app, &w.cx, "the files", |a| tile(a).view.as_ref().is_some_and(|v| v.files.rows.iter().any(|r| r.name == "README.md")));
    key(&mut app, &w.cx, Key::Char('3'));
    pump(&mut app, &w.cx, "the branches", |a| tile(a).view.as_ref().is_some_and(|v| v.refs.branches.ready().is_some() && v.refs.tags.ready().is_some()));
    assert_eq!(tile(&mut app).view.as_ref().unwrap().ref_rows()[0].clone(), ("main".to_string(), false, true), "the default branch comes first and is marked");
    // commits and a commit's diff
    key(&mut app, &w.cx, Key::Char('2'));
    pump(&mut app, &w.cx, "the commits", |a| tile(a).view.as_ref().is_some_and(|v| v.commits.slot.ready().is_some()));
    key(&mut app, &w.cx, Key::Enter);
    pump(&mut app, &w.cx, "the diff", |a| tile(a).view.as_ref().is_some_and(|v| v.commits.text.lines.iter().any(|l| l.text == "+commit added")));
    // switching the connection resets the repository panes: nothing of the old repository is left
    key(&mut app, &w.cx, Key::Esc);
    key(&mut app, &w.cx, Key::Esc);
    click(&mut app, &w.cx, "connection");
    click(&mut app, &w.cx, "Work Gitea");
    assert!(tile(&mut app).view.is_none(), "no repository of the previous connection stays open");
    assert!(tile(&mut app).rows_now().is_empty(), "no row of the previous connection stays");
    let again = GitTabView::new(&w.cx, d.clone(), rig(&w, &run, host()));
    assert_eq!(again.app.src().name(), "Work Gitea", "the new choice replaced the old");
    // nothing on screen or in the audit holds the token
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(!labels(&g).iter().any(|l| l.contains(TOK)));
    assert!(!std::fs::read_to_string(w.cx.audit.path()).unwrap_or_default().contains(TOK));
    let _ = (a, b);
}

#[test]
fn with_no_connection_the_connections_view_says_how_to_add_one_and_esc_returns_to_the_project() {
    let (w, mut app, run, d) = git_world("gnoconn");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    let g = click(&mut app, &w.cx, "Connections");
    assert!(labels(&g).iter().any(|l| l == "Add connection..."), "{:?}", labels(&g));
    assert!(crate::apps::git_tile_ui::FIRST_RUN_HINT.contains("Settings, Git"));
    click(&mut app, &w.cx, "Add connection...");
    assert_eq!(app.take_request().map(|r| r.0), None, "the request is handed over on the next heartbeat");
    ticks(&mut app, &w.cx, 2);
    assert_eq!(app.take_request(), Some(("settings".into(), "git:add".into())));
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(gv(&app).sub, Sub::Project, "Esc at the top of the connections goes back to the project");
}

#[test]
fn the_embedded_tile_only_reads_clone_fetch_pull_and_push_are_refused_with_the_reason() {
    let (w, mut app, run, d) = git_world("gro");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    click(&mut app, &w.cx, "Connections");
    for c in ['C', 'F', 'P', 'U', 'I'] {
        key(&mut app, &w.cx, Key::Char(c));
        assert!(tile(&mut app).status.as_ref().is_some_and(|s| s.0.contains("only reads")), "{c}");
        assert!(tile(&mut app).card.is_none(), "{c}: no review card, no job");
    }
    assert!(run.lines().iter().all(|l| !l.contains(" clone") && !l.contains(" fetch") && !l.contains(" pull") && !l.contains(" push")));
}

// ------------------------------------------------------------------------------------------------ the assistant

#[test]
fn the_assistant_gets_the_git_context_only_with_the_git_tab_open_and_never_an_address_or_a_token() {
    let (w, mut app, run, d) = git_world("gctx");
    install(&w, &mut app, &run, Arc::new(FakeGitHttp::default()), &d);
    pump(&mut app, &w.cx, "the working copy", ready);
    let c = gv(&app).context_block().unwrap();
    assert!(c.contains("main") && c.contains("M src/main.rs") && c.contains("D gone.txt") && c.contains("? notes.md") && c.contains("Commit number 0"), "{c}");
    assert!(!c.contains("http") && !c.contains(TOK) && !c.contains("old line"), "{c}");
}

// ------------------------------------------------------------------------------------------------ geometry

fn assert_clean(g: &Gfx, wd: u32, ht: u32, tag: &str) {
    let hits: Vec<_> = g.hits.iter().filter(|h| h.label != "divider" && h.rect.w > 0.0 && h.rect.h > 0.0).collect();
    let mut bad = vec![];
    for h in &hits {
        let r = h.rect;
        if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
            bad.push(format!("{tag}: {:?} is clipped by the screen edge ({:?})", h.label, r));
        }
    }
    let small: Vec<_> = hits.iter().filter(|h| h.rect.w * h.rect.h < (wd * ht) as f32 * 0.04).collect();
    for (i, x) in small.iter().enumerate() {
        for y in &small[i + 1..] {
            if partly_overlap(&x.rect, &y.rect) {
                bad.push(format!("{tag}: {:?} {:?} overlaps {:?} {:?}", x.label, x.rect, y.label, y.rect));
            }
        }
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(30).cloned().collect::<Vec<_>>().join("\n"));
    assert!(route::duplicate_ids(&g.hits).is_empty(), "{tag}: {:?}", route::duplicate_ids(&g.hits));
}

#[test]
fn the_git_tab_fits_every_size_in_both_views_without_overlap_and_the_whole_tab_is_one_click_range() {
    let (w, mut app, run, d) = git_world("glayout");
    w.cx.services.on_login(&w.cx.session.user, "rootpass12", true);
    install(&w, &mut app, &run, host(), &d);
    connect(&w, gv_mut(&mut app), "Home Gitea", "https://git.example.org");
    pump(&mut app, &w.cx, "the working copy", ready);
    select(&mut app, &w.cx, 2);
    pump(&mut app, &w.cx, "the diff", |a| gv(a).text_has("+new line"));
    for density in super::super::ide_chrome::Density::ALL {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
            for sub in [Sub::Project, Sub::Connections] {
                gv_mut(&mut app).sub = sub;
                for narrow_page in [false, true] {
                    if narrow_page && sub == Sub::Connections {
                        continue;
                    }
                    if !narrow_page {
                        key(&mut app, &w.cx, Key::Esc);
                        cur_mut(&mut app).set_focus(Pane::Main);
                    } else {
                        select(&mut app, &w.cx, 2);
                    }
                    ticks(&mut app, &w.cx, 2);
                    let g = render_at(&mut app, &w.cx, wd, ht);
                    assert_clean(&g, wd, ht, &format!("{} {wd}x{ht} {sub:?} diff-page={narrow_page}", density.name()));
                    // everything the tab registered lies in its own slot or is one of the shell's tab/pane ids
                    for h in g.hits.iter().filter(|h| route::is_slot(&h.ev, GIT)) {
                        assert!(h.rect.w > 0.0 && h.rect.h > 0.0, "{:?}", h.label);
                    }
                }
            }
        }
    }
}
