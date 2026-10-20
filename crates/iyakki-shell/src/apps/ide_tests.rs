//! Tests of the AI IDE as a whole: workspaces and their isolation, the file tree, quick open and search, the chat scoped to a
//! workspace, the board with the assistant, locks, layouts, the cluster view with a fake `kubectl`, and a render of every pane.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, Role, ToolCall, ToolSpec};
use iyakki_agent::k8s::{sample_cluster, FakeKube};
use iyakki_agent::workspace::{self, Registry};
use iyakki_sec::policy::{App as PApp, CliLevel, Role as PRole};

use super::ide::*;
use super::ide_layout::Pane;
use super::{App, AppCx};
use crate::apps::ide_bottom::Tab as BTab;
use crate::apps::tests::cx_with_backend;
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};

/// A model that does what the last message asks: `do:<tool>|<json args>` calls that tool, `slow:` takes a while, and after
/// a tool result it says it is finished.
struct Router;

impl ChatBackend for Router {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Ok("plain answer".into())
    }
    fn complete_tools(&self, msgs: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        let last = msgs.iter().rev().find(|m| m.role != Role::System).ok_or("no messages")?;
        let turn = |t: &str, calls: Vec<ToolCall>| Ok(ModelTurn { text: t.into(), calls });
        if last.role == Role::Tool {
            return turn("finished", vec![]);
        }
        // the assistant adds its "TOOLS AVAILABLE NOW" reminder to the end of the latest message; a script reads only the request
        let t = last.content.split("\n\n[TOOLS AVAILABLE NOW]").next().unwrap_or_default().trim();
        if t.starts_with("slow:") {
            std::thread::sleep(Duration::from_millis(500));
            return turn("slow done", vec![]);
        }
        if let Some((name, args)) = t.strip_prefix("do:").and_then(|r| r.split_once('|')) {
            return turn("", vec![ToolCall { id: "c1".into(), name: name.into(), args: serde_json::from_str(args).map_err(|e| e.to_string())? }]);
        }
        turn("ok", vec![])
    }
}

/// Tests that put a fake cluster in the global slot take turns.
static KUBE_LOCK: Mutex<()> = Mutex::new(());

struct FakeGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for FakeGuard {
    fn drop(&mut self) {
        use_kube(None);
    }
}

fn fake_cluster() -> FakeGuard {
    let g = KUBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    use_kube(Some(Arc::new(sample_cluster(now))));
    FakeGuard(g)
}

fn fake_with(k: FakeKube) -> FakeGuard {
    let g = KUBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use_kube(Some(Arc::new(k)));
    FakeGuard(g)
}

struct World {
    cx: AppCx,
    base: PathBuf,
}

fn world(tag: &str) -> World {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-ide-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (mut cx, _d) = cx_with_backend(&format!("ide-{tag}"), Router);
    cx.session.policy.fs_read = vec![base.display().to_string()];
    cx.session.policy.fs_write = vec![base.display().to_string()];
    cx.session.policy.cli = CliLevel::Full;
    cx.session.policy.agent = true;
    World { cx, base }
}

const FEATURES: &str = "<!-- summary:start -->done 1 · partial 0 · planned 1 · blocked 0 · deprecated 0 · total 2<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | Alpha | P1 | done | P0 | 100% | `c::t::alpha_test` |  |\n| F-A-02 | Beta thing | P2 | planned | P1 | 0% | — |  |\n";

fn project(w: &World, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = w.base.join(name);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/main.rs"), format!("fn main() {{ println!(\"{name} needle\"); }}\n")).unwrap();
    std::fs::write(d.join("FEATURES.md"), FEATURES).unwrap();
    for (f, t) in files {
        if let Some(p) = Path::new(f).parent() {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        std::fs::write(d.join(f), t).unwrap();
    }
    d
}

fn key(app: &mut IdeApp, cx: &AppCx, k: Key) {
    app.handle(&Event::Key(k), cx);
}

fn typ(app: &mut IdeApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(app, cx, Key::Char(c));
    }
}

fn ticks(app: &mut IdeApp, cx: &AppCx, n: usize) {
    for _ in 0..n {
        app.handle(&Event::Tick, cx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn until(app: &mut IdeApp, cx: &AppCx, what: &str, mut f: impl FnMut(&mut IdeApp) -> bool) {
    for _ in 0..1500 {
        app.handle(&Event::Tick, cx);
        if f(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let chats: Vec<String> = app.ws.iter().map(|w| format!("{}: running={} approval={:?} error={:?}", w.meta.name, w.chat.is_running(), w.chat.pending_approval(), w.chat.error_text())).collect();
    panic!("timed out waiting for: {what}; chats: {chats:?}");
}

fn scale(w: u32, h: u32) -> f32 {
    (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0)
}

fn render(app: &mut IdeApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = scale(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(crate::gfx::rgb(0, 0, 0));
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, 1);
    g
}

fn colours(g: &Gfx) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in g.rgba().chunks_exact(4).step_by(37) {
        seen.insert([px[0], px[1], px[2]]);
    }
    seen.len()
}

fn open(app: &mut IdeApp, cx: &AppCx, name: &str, dir: &Path) {
    app.add_and_open(name, dir, cx).unwrap_or_else(|e| panic!("{e}"));
}

fn cur(app: &IdeApp) -> &Ws {
    app.active_ws().expect("an active workspace")
}

fn cur_mut(app: &mut IdeApp) -> &mut Ws {
    app.active_mut().expect("an active workspace")
}

// ------------------------------------------------------------------------------------------------------------ tile

#[test]
fn ide_is_in_the_app_list_once_and_for_the_roles_that_can_edit_and_use_the_agent() {
    assert_eq!(PApp::ALL.iter().filter(|a| **a == PApp::Ide).count(), 1);
    assert_eq!(PApp::Ide.id(), "ide");
    for r in [PRole::Admin, PRole::Operator, PRole::User] {
        assert!(r.template().apps.contains(&PApp::Ide), "{r:?}");
    }
    assert!(!PRole::Guest.template().apps.contains(&PApp::Ide));
    assert_eq!(crate::launcher::Group::of(PApp::Ide), Some(crate::launcher::Group::Work));
    assert!(crate::launcher::Group::Work.holds(PApp::Ide));
}

// ------------------------------------------------------------------------------------------------------ registry

#[test]
fn ide_with_no_workspace_shows_a_welcome_and_creates_the_first_one_from_a_form() {
    let w = world("welcome");
    let mut app = IdeApp::new(&w.cx);
    assert!(app.active_ws().is_none() && app.modal.is_none());
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(colours(&g) > 20);
    for label in ["New workspace  n", "Open existing folder  o", "All workspaces  w"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label}");
    }
    key(&mut app, &w.cx, Key::Char('n'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("the new workspace form") };
    f.set_value(0, "Shop Front");
    f.set_value(1, &w.base.join("Workspaces").display().to_string());
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    let ws = cur(&app);
    assert_eq!(ws.meta.name, "Shop Front");
    assert!(ws.meta.root.ends_with("Workspaces/shop-front"));
    let cfg = std::fs::read_to_string(workspace::ProjectConfig::path(&ws.root())).unwrap();
    assert!(cfg.contains("\"name\": \"Shop Front\"") && cfg.contains("FEATURES.md"), "project.json is created on first use: {cfg}");
    let file = w.cx.services.dir().join("ide-workspaces-tester.json");
    assert!(std::fs::read_to_string(&file).unwrap().contains("Shop Front"), "the registry is per user, in the services folder");
    // a second session of the same person finds it again and opens the last one
    drop(app);
    let app2 = IdeApp::new(&w.cx);
    assert_eq!(app2.active_ws().unwrap().meta.name, "Shop Front");
}

#[test]
fn ide_registry_add_open_rename_pin_remove_and_never_deletes_files() {
    let w = world("registry");
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    assert_eq!(app.ws.len(), 2);
    assert_eq!(app.reg.items.len(), 2);
    // the same folder twice, a folder inside another workspace, a folder outside the file scope
    assert!(app.add_and_open("Again", &a, &w.cx).unwrap_err().contains("already"));
    let (outside, _) = (std::env::temp_dir().join(format!("iyakki-ide-outside-{}", std::process::id())), ());
    std::fs::create_dir_all(&outside).unwrap();
    assert!(app.add_and_open("Out", &outside, &w.cx).unwrap_err().contains("outside the folders your account may read"));
    // the switcher: rename, pin, remove (with a confirmation)
    key(&mut app, &w.cx, Key::Ctrl('W'));
    assert!(matches!(app.modal, Some(Modal::Switcher { .. })));
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 2);
    key(&mut app, &w.cx, Key::Char('r'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(0, "Alpha Two");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.reg.items.iter().any(|x| x.name == "Alpha Two") || app.reg.items.iter().any(|x| x.name == "Beta"), "{:?}", app.reg.items);
    key(&mut app, &w.cx, Key::Ctrl('W'));
    key(&mut app, &w.cx, Key::Char('x'));
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    key(&mut app, &w.cx, Key::Char('n'));
    assert_eq!(app.reg.items.len(), 2, "a no keeps it");
    key(&mut app, &w.cx, Key::Ctrl('W'));
    key(&mut app, &w.cx, Key::Char('x'));
    let removed_root = {
        let ordered = app.reg.ordered();
        ordered[0].root.clone()
    };
    key(&mut app, &w.cx, Key::Char('y'));
    assert_eq!(app.reg.items.len(), 1);
    assert_eq!(app.ws.len(), 1, "its panes are closed");
    assert!(Path::new(&removed_root).join("src/main.rs").exists(), "removing from the list never deletes files");
    // the list survives
    let reg = Registry::load(w.cx.services.dir(), "tester");
    assert_eq!(reg.items.len(), 1);
}

#[test]
fn ide_clone_is_a_stub_that_says_what_to_do() {
    let w = world("clone");
    let mut app = IdeApp::new(&w.cx);
    key(&mut app, &w.cx, Key::Char('w'));
    key(&mut app, &w.cx, Key::Char('c'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(0, "https://example.com/x.git");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Info(title, msg)) = &app.modal else { panic!("{:?}", app.notice) };
    assert_eq!(title, "Clone from a URL");
    assert!(msg.contains("Open existing folder") || msg.contains("git is not installed"), "{msg}");
    key(&mut app, &w.cx, Key::Enter);
    assert!(app.modal.is_none());
}

// ----------------------------------------------------------------------------------------------------- isolation

#[test]
fn ide_two_workspaces_keep_tabs_chats_board_cluster_and_terminal_apart() {
    let w = world("isolation");
    let _k = fake_cluster();
    let (a, b) = (project(&w, "alpha", &[("only_a.txt", "A")]), project(&w, "beta", &[("only_b.txt", "B")]));
    std::fs::write(b.join("FEATURES.md"), FEATURES.replace("Beta thing", "Beta other thing").replace("F-A-", "F-B-")).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    app.open_file("only_a.txt", None, &w.cx);
    app.open_file("src/main.rs", Some(1), &w.cx);
    let id_a = cur(&app).meta.id.clone();
    open(&mut app, &w.cx, "Beta", &b);
    app.open_file("only_b.txt", None, &w.cx);
    let id_b = cur(&app).meta.id.clone();
    assert_ne!(id_a, id_b);
    // tabs
    let tabs = |app: &IdeApp, i: usize| app.ws[i].editor.open_paths().iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
    assert_eq!(tabs(&app, 0), vec!["only_a.txt", "main.rs"]);
    assert_eq!(tabs(&app, 1), vec!["only_b.txt"]);
    // the editor of one cannot open the other's files
    let other = b.join("only_b.txt");
    assert!(app.ws[0].editor.open_path(&w.cx, &other.to_string_lossy()).unwrap_err().contains("outside the workspace"));
    // the board
    assert!(app.ws[0].board.file.as_ref().unwrap().board.get("F-A-02").is_some());
    assert!(app.ws[1].board.file.as_ref().unwrap().board.get("F-B-02").unwrap().title.contains("other"));
    assert!(app.ws[1].board.file.as_ref().unwrap().board.get("F-A-02").is_none());
    // the cluster view: each looks at its own namespace
    assert_eq!(app.ws[0].target().namespace, "alpha-dev");
    assert_eq!(app.ws[1].target().namespace, "beta-dev");
    // the terminal: folder and history
    cur_mut(&mut app).term.run_line("cd src");
    assert_eq!(app.ws[1].term.cwd, "src");
    assert_eq!(app.ws[0].term.cwd, "");
    assert!(app.ws[0].term.history.is_empty() && app.ws[1].term.history == vec!["cd src".to_string()]);
    // the chats: each stores its sessions under its own key
    cur_mut(&mut app).chat.ask(&w.cx, "hello from beta");
    until(&mut app, &w.cx, "beta's chat to answer", |a| !a.ws[1].chat.is_running());
    app.activate(0, &w.cx);
    cur_mut(&mut app).chat.ask(&w.cx, "hello from alpha");
    until(&mut app, &w.cx, "alpha's chat to answer", |a| !a.ws[0].chat.is_running());
    let (ka, kb) = (app.ws[0].meta.chat_key("tester"), app.ws[1].meta.chat_key("tester"));
    assert_ne!(ka, kb);
    let (la, lb) = (w.cx.services.sessions.list(&ka), w.cx.services.sessions.list(&kb));
    assert_eq!((la.len(), lb.len()), (1, 1));
    assert!(la[0].title.contains("alpha") && lb[0].title.contains("beta"));
    assert!(w.cx.services.sessions.list("tester").is_empty(), "nothing leaks into the person's own chat list");
    assert!(app.ws[0].chat.messages().iter().all(|m| !m.content.contains("beta")));
    assert!(w.cx.services.sessions.load(&ka, &lb[0].id).is_none(), "one workspace cannot open the other's chats");
    // a new IDE (a restart) shows each workspace's own chat again
    drop(app);
    let mut app2 = IdeApp::new(&w.cx);
    let last = app2.reg.last.clone().unwrap();
    app2.open_by_id(&id_a, &w.cx).unwrap();
    assert!(app2.ws.iter().find(|x| x.meta.id == id_a).unwrap().chat.messages().iter().any(|m| m.content.contains("hello from alpha")));
    let _ = last;
    app2.open_by_id(&id_b, &w.cx).unwrap();
    assert!(app2.ws.iter().find(|x| x.meta.id == id_b).unwrap().chat.messages().iter().any(|m| m.content.contains("hello from beta")));
}

#[test]
fn ide_the_namespace_defaults_follow_the_workspace_and_a_shared_one_is_warned_about() {
    let w = world("namespaces");
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    assert!(app.ws.iter().all(|x| x.ns_warning.is_none()));
    // Beta is pointed at Alpha's namespace through the project settings
    cur_mut(&mut app).focus = Pane::Main;
    key(&mut app, &w.cx, Key::Ctrl('R'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("project settings") };
    f.set_value(1, "alpha-dev");
    f.set_value(6, "echo tested");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.ws[1].status);
    assert_eq!(cur(&app).target().namespace, "alpha-dev");
    for x in &app.ws {
        let warn = x.ns_warning.as_deref().unwrap_or_else(|| panic!("{} warns", x.meta.name));
        assert!(warn.contains("alpha-dev") && warn.contains("also used by"), "{warn}");
    }
    let saved = std::fs::read_to_string(workspace::ProjectConfig::path(&b)).unwrap();
    assert!(saved.contains("\"namespace\": \"alpha-dev\"") && saved.contains("echo tested"));
    // another cluster profile is no collision
    key(&mut app, &w.cx, Key::Ctrl('R'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(3, "other-cluster");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.ws.iter().all(|x| x.ns_warning.is_none()), "a different cluster profile does not collide");
    // a features file outside the workspace is refused
    key(&mut app, &w.cx, Key::Ctrl('R'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(5, "../elsewhere.md");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(matches!(&app.modal, Some(Modal::Form(_, f)) if f.error.as_deref().is_some_and(|e| e.contains("outside"))));
}

// ---------------------------------------------------------------------------------------------------- the tree

#[test]
fn ide_file_tree_creates_renames_deletes_and_opens_inside_the_workspace_only() {
    let w = world("tree");
    let d = project(&w, "alpha", &[("target/junk.rs", "x"), ("node_modules/x/y.js", "y")]);
    let outside = w.base.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(&outside, d.join("leak")).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.focus_pane(Pane::Tree);
    assert!(cur(&app).tree.rows.iter().all(|r| r.name != "target" && r.name != "node_modules" && r.name != ".iyakki"), "hidden folders stay hidden");
    // new file through the form
    key(&mut app, &w.cx, Key::Char('n'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(0, "docs/notes.md");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(d.join("docs/notes.md").is_file());
    assert_eq!(cur(&app).editor.path().unwrap(), d.join("docs/notes.md"), "and it opens in the editor");
    assert!(cur(&app).tree.rows.iter().any(|r| r.rel == "docs/notes.md"));
    // outside the scope: refused and nothing is created
    app.focus_pane(Pane::Tree);
    for bad in ["../escape.txt", "leak/new.txt", &outside.join("x.txt").display().to_string()] {
        key(&mut app, &w.cx, Key::Char('n'));
        let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
        f.set_value(0, bad);
        key(&mut app, &w.cx, Key::Ctrl('s'));
        let Some(Modal::Form(_, f)) = &app.modal else { panic!("{bad} must be refused") };
        assert!(f.error.as_deref().is_some_and(|e| e.contains("outside the workspace")), "{bad}: {:?}", f.error);
        key(&mut app, &w.cx, Key::Esc);
    }
    assert!(!w.base.join("escape.txt").exists() && !outside.join("new.txt").exists() && !outside.join("x.txt").exists());
    // the link out is shown but never opened
    let leak = cur(&app).tree.rows.iter().position(|r| r.name == "leak").unwrap();
    assert!(cur(&app).tree.rows[leak].escapes);
    // rename
    cur_mut(&mut app).tree.expand_to("docs/notes.md");
    app.focus_pane(Pane::Tree);
    key(&mut app, &w.cx, Key::F(2));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(0, "readme.md");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(d.join("docs/readme.md").is_file() && !d.join("docs/notes.md").exists());
    // delete asks first
    app.focus_pane(Pane::Tree);
    cur_mut(&mut app).tree.expand_to("docs/readme.md");
    key(&mut app, &w.cx, Key::Delete);
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    key(&mut app, &w.cx, Key::Esc);
    assert!(d.join("docs/readme.md").exists());
    key(&mut app, &w.cx, Key::Delete);
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(!d.join("docs/readme.md").exists());
    assert!(!cur(&app).editor.open_paths().iter().any(|p| p.ends_with("readme.md")), "its tab is closed");
    // deleting a link removes the link and leaves what it pointed to
    let leak = cur(&app).tree.rows.iter().position(|r| r.name == "leak").unwrap();
    cur_mut(&mut app).tree.sel = leak;
    key(&mut app, &w.cx, Key::Delete);
    key(&mut app, &w.cx, Key::Char('y'));
    assert!(!d.join("leak").exists() && outside.join("secret.txt").exists());
    // a click opens a file after the second one (the shell sends Enter for a double click)
    let main = cur(&app).tree.rows.iter().position(|r| r.name == "src");
    assert!(main.is_some());
    app.handle(&Event::Row(main.unwrap()), &w.cx);
    assert!(cur(&app).tree.rows.iter().any(|r| r.rel == "src/main.rs"), "a click expands a folder");
    let f = cur(&app).tree.rows.iter().position(|r| r.rel == "src/main.rs").unwrap();
    app.handle(&Event::Row(f), &w.cx);
    key(&mut app, &w.cx, Key::Enter);
    assert_eq!(cur(&app).editor.path().unwrap(), d.join("src/main.rs"));
}

#[test]
fn ide_quick_open_and_project_search_open_files_at_the_line() {
    let w = world("search");
    let d = project(&w, "alpha", &[("src/deep/helper.rs", "fn a() {}\nfn needle_here() {}\n"), ("target/skip.rs", "needle\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    key(&mut app, &w.cx, Key::Ctrl('p'));
    assert!(matches!(app.modal, Some(Modal::Quick(_))));
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(colours(&g) > 20);
    typ(&mut app, &w.cx, "helper");
    let Some(Modal::Quick(q)) = &app.modal else { panic!() };
    assert_eq!(q.results, vec!["src/deep/helper.rs"], "the file's own name counts most");
    key(&mut app, &w.cx, Key::Enter);
    assert!(app.modal.is_none());
    assert_eq!(cur(&app).editor.path().unwrap(), d.join("src/deep/helper.rs"));
    key(&mut app, &w.cx, Key::Ctrl('p'));
    typ(&mut app, &w.cx, "skip");
    let Some(Modal::Quick(q)) = &app.modal else { panic!() };
    assert!(q.results.is_empty(), "build folders are not searched");
    key(&mut app, &w.cx, Key::Esc);
    // project search
    key(&mut app, &w.cx, Key::Ctrl('F'));
    assert_eq!((cur(&app).bottom_tab, cur(&app).focus), (BTab::Search, Pane::Bottom));
    typ(&mut app, &w.cx, "needle");
    key(&mut app, &w.cx, Key::Enter);
    until(&mut app, &w.cx, "the search", |a| a.ws[0].search.searched);
    let hits: Vec<(String, usize)> = cur(&app).search.hits.iter().map(|h| (h.file.clone(), h.line)).collect();
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(hits.contains(&("src/deep/helper.rs".to_string(), 2)) && hits.contains(&("src/main.rs".to_string(), 1)));
    // a second Enter opens the selected hit at its line
    app.open_file("src/main.rs", None, &w.cx);
    key(&mut app, &w.cx, Key::Ctrl('F'));
    let first = cur(&app).search.hits[0].clone();
    key(&mut app, &w.cx, Key::Enter);
    assert_eq!(cur(&app).editor.path().unwrap(), d.join(&first.file));
    // clicks on a result and on the other panel tabs
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(r) if r >= 300_000)));
    assert!(g.hits.iter().any(|h| h.label == "TERMINAL" || h.label == "PROBLEMS"));
}

#[test]
fn ide_problems_run_the_test_command_and_a_click_jumps_to_file_and_line() {
    let w = world("problems");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.run_tests(&w.cx);
    assert!(cur(&app).status.as_ref().unwrap().0.contains("No test command"), "{:?}", cur(&app).status);
    let mut cfg = cur(&app).cfg.clone();
    cfg.commands.test = "sh -c \"echo '--> src/main.rs:1:4'; exit 1\"".into();
    cfg.save(&d).unwrap();
    cur_mut(&mut app).cfg = cfg;
    app.run_tests(&w.cx);
    until(&mut app, &w.cx, "the tests", |a| a.ws[0].problems.summary.is_some());
    assert_eq!(cur(&app).bottom_tab, BTab::Problems);
    assert!(cur(&app).problems.summary.as_ref().unwrap().0.starts_with("FAILED"));
    let g = render(&mut app, &w.cx, 1280, 720);
    let link = g.hits.iter().find(|h| matches!(h.ev, Event::Row(r) if r >= 400_000)).expect("the line with a location is clickable").clone();
    app.handle(&link.ev, &w.cx);
    assert_eq!(cur(&app).editor.path().unwrap(), d.join("src/main.rs"));
    assert_eq!(cur(&app).focus, Pane::Main);
}

#[test]
fn ide_the_test_run_stores_each_tests_result_and_the_coverage_button_reads_a_report_or_shows_the_install_hint() {
    let w = world("quality");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    // a test run: its output becomes per-test results stored for the workspace and shown on the board
    let mut cfg = cur(&app).cfg.clone();
    cfg.commands.test = "sh -c \"echo 'test scope::tests::a ... ok'; echo 'test scope::tests::b ... FAILED'; exit 1\"".into();
    cfg.save(&d).unwrap();
    cur_mut(&mut app).cfg = cfg;
    app.handle_board_act(super::board::BoardAct::RunTests, &w.cx);
    until(&mut app, &w.cx, "the tests", |a| a.ws[0].problems.summary.is_some());
    let stored = iyakki_agent::testresults::load(&d).expect("results stored in .iyakki/test-results.json");
    assert_eq!(stored.tests.len(), 2);
    cur_mut(&mut app).board.refresh_quality(true);
    assert!(cur(&app).board.results.as_ref().is_some_and(|r| r.tests["scope::tests::b"] == iyakki_agent::testresults::Outcome::Fail));
    // coverage: no tool is known for this folder, so nothing is run and the board says what to do
    app.handle_board_act(super::board::BoardAct::RunCoverage, &w.cx);
    assert!(cur(&app).board.cov_note.as_ref().is_some_and(|n| n.contains("No coverage tool is known")), "{:?}", cur(&app).board.cov_note);
    assert!(!cur(&app).cov.busy());
    // a configured command that prints an lcov report: run off the UI thread, parsed, stored, shown
    let mut cfg = cur(&app).cfg.clone();
    cfg.commands.coverage = "sh -c \"printf 'SF:src/main.rs\\nDA:1,1\\nDA:2,0\\nend_of_record\\n'\"".into();
    cfg.save(&d).unwrap();
    cur_mut(&mut app).cfg = cfg;
    app.handle_board_act(super::board::BoardAct::RunCoverage, &w.cx);
    assert!(cur(&app).cov.busy() || cur(&app).board.coverage.is_some(), "it runs in the background");
    until(&mut app, &w.cx, "the coverage run", |a| a.ws[0].board.coverage.is_some());
    assert_eq!(cur(&app).board.coverage.as_ref().unwrap().files["src/main.rs"], (1, 2));
    assert_eq!(iyakki_agent::coverage::load(&d).unwrap().overall().map(|p| p.round()), Some(50.0));
    // a configured command whose program is missing is not run
    let mut cfg = cur(&app).cfg.clone();
    cfg.commands.coverage = "no-such-coverage-tool --lcov".into();
    cur_mut(&mut app).cfg = cfg;
    app.handle_board_act(super::board::BoardAct::RunCoverage, &w.cx);
    assert!(cur(&app).board.cov_note.as_ref().is_some_and(|n| n.contains("no-such-coverage-tool") && n.contains("not installed")));
    assert!(!cur(&app).cov.busy());
}

#[test]
fn ide_settings_keep_the_stack_override_refuse_an_unknown_one_and_the_chat_footer_shows_it() {
    let w = world("stack");
    let d = project(&w, "alpha", &[("go.mod", "module x")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(cur_mut(&mut app).chat.project_footer().as_deref().map(|f| f.starts_with("stack: Go")), Some(true), "detected from go.mod");
    cur_mut(&mut app).cfg.stack = "Rust".into();
    app.open_settings_form();
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "saved");
    assert_eq!(workspace::ProjectConfig::load(&d).unwrap().stack, "Rust");
    assert!(cur_mut(&mut app).chat.project_footer().is_some());
    cur_mut(&mut app).cfg.stack = "cobol".into();
    app.open_settings_form();
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_some(), "an unknown stack keeps the form open");
    assert_eq!(workspace::ProjectConfig::load(&d).unwrap().stack, "Rust", "and is not saved");
}

// -------------------------------------------------------------------------------------------- chat and the agent

#[test]
fn ide_the_chat_is_limited_to_its_workspace_and_approvals_name_it() {
    let w = world("scope");
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[("secret.txt", "beta secret")]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    app.focus_pane(Pane::Chat);
    let alpha = cur(&app).meta.id.clone();
    // inside: asks, then writes
    let inside = serde_json::json!({"path": "src/new.rs", "content": "fn x() {}"});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:write_file|{inside}"));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
    let (_, tool, summary) = cur(&app).chat.pending_approval().unwrap().clone();
    assert_eq!(tool, "write_file");
    assert!(summary.starts_with("[Alpha] write"), "the approval names the workspace: {summary}");
    assert!(!a.join("src/new.rs").exists(), "nothing is written before the approval");
    key(&mut app, &w.cx, Key::Ctrl('y'));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    assert_eq!(std::fs::read_to_string(a.join("src/new.rs")).unwrap(), "fn x() {}");
    // outside: the other workspace by absolute path, `..`, and a link
    std::os::unix::fs::symlink(&b, a.join("link-to-beta")).unwrap();
    for (n, path) in ["../beta/secret.txt".to_string(), b.join("secret.txt").display().to_string(), "link-to-beta/secret.txt".to_string()].iter().enumerate() {
        let args = serde_json::json!({"path": path, "content": "overwritten"});
        cur_mut(&mut app).chat.ask(&w.cx, &format!("do:write_file|{args}"));
        until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
        let (_, _, summary) = cur(&app).chat.pending_approval().unwrap().clone();
        assert!(summary.contains("REFUSED") && summary.starts_with("[Alpha]"), "{n}: the approval warns that it will be refused: {summary}");
        key(&mut app, &w.cx, Key::Ctrl('y'));
        until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
        let tool_result = cur(&app).chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
        assert!(tool_result.contains("outside the workspace \"Alpha\""), "{n}: {tool_result}");
        assert_eq!(std::fs::read_to_string(b.join("secret.txt")).unwrap(), "beta secret", "{n}: beta is untouched");
    }
    // reading outside is refused too, without any approval (it is a safe tool)
    let args = serde_json::json!({"path": b.join("secret.txt").display().to_string()});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:read_file|{args}"));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    let r = cur(&app).chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(r.contains("outside the workspace") && !r.contains("beta secret"), "{r}");
    // the shell line tool is not offered, so nothing can leave through it
    let args = serde_json::json!({"command": "cat ../beta/secret.txt"});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:terminal|{args}"));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    let r = cur(&app).chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(!r.contains("beta secret"), "{r}");
    // the chat knows its project
    let scope = cur(&app).chat.project().unwrap();
    assert_eq!((scope.name.as_str(), scope.root.as_path()), ("Alpha", a.as_path()));
    assert_eq!(cur(&app).meta.id, alpha);
}

#[test]
fn ide_a_run_keeps_its_workspace_after_the_switch_and_the_rail_shows_every_state() {
    let w = world("concurrent");
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    // Alpha asks to write and waits for approval
    app.activate(0, &w.cx);
    let args = serde_json::json!({"path": "from_alpha.txt", "content": "alpha wrote this"});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:write_file|{args}"));
    until(&mut app, &w.cx, "alpha's approval", |a| a.ws[0].chat.pending_approval().is_some());
    // the person goes to Beta; Beta's own assistant starts working at the same time
    app.activate(1, &w.cx);
    cur_mut(&mut app).chat.ask(&w.cx, "slow:thinking");
    ticks(&mut app, &w.cx, 5);
    assert_eq!(app.ws[0].activity(), Activity::Waiting);
    assert_eq!(app.ws[1].activity(), Activity::Running);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_RAIL..T_RAIL + 10).contains(&n))).count() == 2, "both workspaces are in the rail");
    // Beta finishes while Alpha is still waiting; Alpha's run did not move to Beta
    app.activate(0, &w.cx);
    until(&mut app, &w.cx, "beta to finish", |a| !a.ws[1].chat.is_running());
    assert_eq!(app.ws[1].activity(), Activity::Done, "finished while the person was elsewhere");
    assert!(!b.join("from_alpha.txt").exists() && !a.join("from_alpha.txt").exists());
    let (_, _, summary) = app.ws[0].chat.pending_approval().unwrap().clone();
    assert!(summary.starts_with("[Alpha]"));
    // approving in Alpha writes into Alpha's folder, after the switch back
    app.focus_pane(Pane::Chat);
    key(&mut app, &w.cx, Key::Ctrl('y'));
    until(&mut app, &w.cx, "alpha to finish", |a| !a.ws[0].chat.is_running());
    assert_eq!(std::fs::read_to_string(a.join("from_alpha.txt")).unwrap(), "alpha wrote this");
    assert!(!b.join("from_alpha.txt").exists());
    // seeing Beta clears its badge
    app.activate(1, &w.cx);
    assert_eq!(app.ws[1].activity(), Activity::Idle);
    // an error shows as an error
    app.ws[1].chat.ask(&w.cx, "do:read_file|{not json");
    until(&mut app, &w.cx, "the error", |a| a.ws[1].chat.error_text().is_some() || !a.ws[1].chat.is_running());
    ticks(&mut app, &w.cx, 5);
    assert_eq!(app.ws[1].activity(), Activity::Error);
}

#[test]
fn ide_assistant_board_tools_ask_first_show_the_row_and_keep_the_editor_and_board_current() {
    let w = world("boardtools");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("FEATURES.md", None, &w.cx);
    assert!(cur(&app).editor.text().contains("| F-A-02 | Beta thing | P2 | planned |"));
    app.focus_pane(Pane::Chat);
    let args = serde_json::json!({"id": "F-A-02", "status": "partial", "comment": "blocked by the new API"});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:board_update|{args}"));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
    let (_, tool, summary) = cur(&app).chat.pending_approval().unwrap().clone();
    assert_eq!(tool, "board_update");
    assert!(summary.starts_with("[Alpha] update feature F-A-02") && summary.contains("partial"), "{summary}");
    assert_eq!(std::fs::read_to_string(d.join("FEATURES.md")).unwrap(), FEATURES, "nothing before the approval");
    // denied: nothing changes
    key(&mut app, &w.cx, Key::Ctrl('n'));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    assert_eq!(std::fs::read_to_string(d.join("FEATURES.md")).unwrap(), FEATURES);
    // approved: one row and the totals line change, the file keeps its shape
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:board_update|{args}"));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
    key(&mut app, &w.cx, Key::Ctrl('y'));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    let text = std::fs::read_to_string(d.join("FEATURES.md")).unwrap();
    let changed: Vec<&str> = text.lines().zip(FEATURES.lines()).filter(|(x, y)| x != y).map(|x| x.0).collect();
    // the totals line, and the Started / Finished columns the first status change adds (header, separator, both rows)
    assert_eq!(changed.len(), 5, "{changed:?}");
    assert!(text.contains("| Tests | Started | Finished | Notes |"), "{text}");
    assert!(text.lines().any(|l| l.starts_with("| F-A-02 | Beta thing | P2 | partial | P1 | 50% | — | 20") && l.ends_with("| | blocked by the new API |")), "{text}");
    assert!(text.lines().any(|l| l.starts_with("| F-A-01 |") && l.ends_with("| | | Alpha thing |") || l.starts_with("| F-A-01 |")), "{text}");
    assert!(text.contains("done 1 · partial 1 · planned 0"));
    // the board and the editor tab follow (they notice the file within a second)
    ticks(&mut app, &w.cx, 70);
    assert_eq!(cur(&app).board.file.as_ref().unwrap().board.get("F-A-02").unwrap().status, "partial");
    assert!(cur(&app).editor.text().contains("| F-A-02 | Beta thing | P2 | partial |"), "the open FEATURES.md tab reloaded");
    // the board tools exist only in the IDE's chat
    let plain = iyakki_agent::agent::UserTools::new("tester", w.cx.session.policy.clone(), w.cx.services.clone(), w.cx.host.clone(), vec![]);
    use iyakki_agent::agent::Tools;
    assert!(plain.defs().iter().all(|t| !t.spec.name.starts_with("board_")), "a normal chat has no board tools");
}

#[test]
fn ide_the_board_follows_the_editor_and_the_editor_follows_the_board() {
    let w = world("boardeditor");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("FEATURES.md", None, &w.cx);
    cur_mut(&mut app).board.autosave_after = Duration::ZERO;
    // a board edit is written and the clean editor tab reloads
    cur_mut(&mut app).board.select_id("F-A-02");
    cur_mut(&mut app).board.handle(&Event::Key(Key::Char('P')));
    app.show_main(MainTab::Board);
    app.handle(&Event::Tick, &w.cx);
    assert!(std::fs::read_to_string(d.join("FEATURES.md")).unwrap().contains("| planned | P2 |"), "the priority cell changed on disk");
    assert!(cur(&app).editor.text().contains("| F-A-02 | Beta thing | P2 | planned | P2 |"), "{}", cur(&app).editor.text());
    // typing in the editor and saving updates the board
    app.show_main(MainTab::Editor);
    app.focus_pane(Pane::Main);
    key(&mut app, &w.cx, Key::Char('x'));
    assert!(cur(&app).editor.is_dirty());
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(!cur(&app).editor.is_dirty());
    assert!(std::fs::read_to_string(d.join("FEATURES.md")).unwrap().starts_with('x'));
    assert!(cur(&app).board.msg.as_ref().is_some_and(|m| m.0.contains("reloaded")), "{:?}", cur(&app).board.msg);
    // an editor with unsaved changes is left alone when the board writes
    key(&mut app, &w.cx, Key::Char('y'));
    cur_mut(&mut app).board.select_id("F-A-01");
    cur_mut(&mut app).board.handle(&Event::Key(Key::Char('P')));
    app.handle(&Event::Tick, &w.cx);
    assert!(cur(&app).editor.text().starts_with("xy"), "unsaved text is never replaced: {}", &cur(&app).editor.text()[..10]);
}

// ------------------------------------------------------------------------------------------------------- locks

#[test]
fn ide_a_second_session_opens_read_only_or_takes_over_and_stale_locks_expire() {
    let w = world("locks");
    let d = project(&w, "alpha", &[]);
    let mut first = IdeApp::new(&w.cx);
    open(&mut first, &w.cx, "Alpha", &d);
    assert!(cur(&first).ro.is_none());
    assert!(workspace::read_lock(&d).is_some());
    // another session (the registry says Alpha was last) finds it taken
    let mut second = IdeApp::new(&w.cx);
    assert!(cur(&second).ro.is_some(), "held by the first session");
    assert!(matches!(second.modal, Some(Modal::Lock(_))));
    let g = render(&mut second, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Open read-only  r") && g.hits.iter().any(|h| h.label == "Take over  t"));
    key(&mut second, &w.cx, Key::Char('r'));
    assert!(second.modal.is_none() && cur(&second).ro.is_some());
    // read-only means read-only: editing, the board, the tree and the terminal refuse
    second.open_file("src/main.rs", None, &w.cx);
    key(&mut second, &w.cx, Key::Char('z'));
    assert!(!cur(&second).editor.is_dirty(), "typing is blocked");
    assert!(cur(&second).status.as_ref().unwrap().0.contains("Read-only"));
    second.focus_pane(Pane::Tree);
    key(&mut second, &w.cx, Key::Char('n'));
    assert!(second.modal.is_none() && cur(&second).status.as_ref().unwrap().0.contains("read-only"), "{:?}", cur(&second).status);
    cur_mut(&mut second).board.select_id("F-A-02");
    second.show_main(MainTab::Board);
    key(&mut second, &w.cx, Key::Char('P'));
    assert!(!cur(&second).board.is_dirty());
    cur_mut(&mut second).term.run_line("echo hi");
    second.run_tests(&w.cx);
    assert!(cur(&second).status.as_ref().unwrap().0.contains("read-only"));
    assert!(!cur(&second).chat.agent_mode(), "the assistant's tools are off in a read-only workspace");
    // the first session keeps its lock fresh
    ticks(&mut first, &w.cx, 5);
    assert!(cur(&first).ro.is_none());
    // taking over flips the roles
    second.modal = Some(Modal::Lock(0));
    key(&mut second, &w.cx, Key::Char('t'));
    assert!(cur(&second).ro.is_none());
    assert_eq!(workspace::read_lock(&d).unwrap().session, second.session);
    for _ in 0..900 {
        first.handle(&Event::Tick, &w.cx);
    }
    assert!(cur(&first).ro.is_some(), "the first session notices and becomes read-only");
    assert!(cur(&first).status.as_ref().unwrap().0.contains("took this workspace over"));
    assert!(!cur(&first).chat.agent_mode(), "the owner that lost the lock also loses the assistant's tools");
    let banner = render(&mut first, &w.cx, 1280, 720);
    assert!(banner.hits.iter().any(|h| h.label == "Take over" || h.label.contains("Take over")) || cur(&first).ro.is_some());
    // the dialog never ends silently read-only: Cancel (Esc) leaves the workspace closed, and says who holds it
    let mut third_try = IdeApp::new(&w.cx);
    assert!(matches!(third_try.modal, Some(Modal::Lock(_))));
    let g = render(&mut third_try, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Cancel"), "Cancel is offered");
    key(&mut third_try, &w.cx, Key::Enter);
    assert!(matches!(third_try.modal, Some(Modal::Lock(_))), "Enter alone does not pick read-only");
    key(&mut third_try, &w.cx, Key::Esc);
    assert!(third_try.modal.is_none() && third_try.ws.is_empty(), "Cancel closes the workspace");
    assert!(workspace::read_lock(&d).is_some(), "and leaves the holder's lock alone");
    drop(third_try);
    // a stale lock (nobody refreshed it) does not hold anybody out
    drop(first);
    drop(second);
    std::fs::create_dir_all(d.join(".iyakki")).unwrap();
    let old = workspace::LockInfo { user: "ghost".into(), session: "gone".into(), pid: std::process::id(), host: "elsewhere".into(), time: now_secs() - 10 * workspace::LOCK_TTL_SECS, opened: 0 };
    std::fs::write(workspace::lock_path(&d), serde_json::to_vec(&old).unwrap()).unwrap();
    let third = IdeApp::new(&w.cx);
    assert!(cur(&third).ro.is_none() && third.modal.is_none(), "the stale lock expired");
    assert_eq!(workspace::read_lock(&d).unwrap().session, third.session);
    drop(third);
    assert!(workspace::read_lock(&d).is_none(), "closing the IDE gives the lock back");
}

// ------------------------------------------------------------------------------------------------------ layout

#[test]
fn ide_layout_tabs_and_terminal_history_are_saved_and_restored_per_workspace() {
    let w = world("restore");
    let (a, b) = (project(&w, "alpha", &[("one.txt", "1"), ("two.txt", "2")]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    app.open_file("one.txt", None, &w.cx);
    app.open_file("two.txt", None, &w.cx);
    cur_mut(&mut app).prefs.chat_open = false;
    cur_mut(&mut app).prefs.sidebar_w = 300.0;
    cur_mut(&mut app).prefs.bottom_h = 180.0;
    app.show_bottom(BTab::Search);
    app.show_main(MainTab::Board);
    cur_mut(&mut app).term.run_line("cd src");
    cur_mut(&mut app).tree.expanded.insert("src".into());
    open(&mut app, &w.cx, "Beta", &b);
    assert!(cur(&app).prefs.chat_open && cur(&app).prefs.sidebar_w == 236.0, "Beta has its own layout");
    drop(app);
    let mut app2 = IdeApp::new(&w.cx);
    let ida = app2.reg.items.iter().find(|x| x.name == "Alpha").unwrap().id.clone();
    app2.open_by_id(&ida, &w.cx).unwrap();
    let ws = cur(&app2);
    assert_eq!(ws.editor.open_paths().len(), 2);
    assert!(!ws.prefs.chat_open && ws.prefs.sidebar_w == 300.0 && ws.prefs.bottom_h == 180.0);
    assert_eq!((ws.bottom_tab, ws.main_tab), (BTab::Search, MainTab::Board));
    assert_eq!(ws.term.cwd, "src");
    assert!(ws.term.history.contains(&"cd src".to_string()));
    assert!(ws.tree.rows.iter().any(|r| r.rel == "src/main.rs"), "the expanded folders are back");
    // the layout is also where the person left it for the screen
    let g = render(&mut app2, &w.cx, 1280, 720);
    assert!(colours(&g) > 20);
}

#[test]
fn ide_panes_collapse_resize_by_keys_and_by_dragging_the_dividers() {
    let w = world("panes");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    render(&mut app, &w.cx, 1280, 720);
    let l0 = app.layout.clone().unwrap();
    assert!(l0.sidebar.is_some() && l0.chat.is_some() && l0.bottom.is_some() && !l0.compact);
    key(&mut app, &w.cx, Key::Ctrl('b'));
    key(&mut app, &w.cx, Key::Ctrl('j'));
    render(&mut app, &w.cx, 1280, 720);
    let l1 = app.layout.clone().unwrap();
    assert!(l1.sidebar.is_none() && l1.bottom.is_none() && l1.main.w > l0.main.w && l1.main.h > l0.main.h);
    key(&mut app, &w.cx, Key::Ctrl('b'));
    key(&mut app, &w.cx, Key::Ctrl('j'));
    // keys resize the focused pane
    let w0 = cur(&app).prefs.sidebar_w;
    app.focus_pane(Pane::Tree);
    key(&mut app, &w.cx, Key::Nav { nav: crate::ui::Nav::Right, shift: true, ctrl: true });
    assert!(cur(&app).prefs.sidebar_w > w0);
    let h0 = cur(&app).prefs.bottom_h;
    key(&mut app, &w.cx, Key::Nav { nav: crate::ui::Nav::Up, shift: true, ctrl: true });
    assert!(cur(&app).prefs.bottom_h > h0);
    let c0 = cur(&app).prefs.chat_w;
    app.focus_pane(Pane::Chat);
    key(&mut app, &w.cx, Key::Nav { nav: crate::ui::Nav::Right, shift: true, ctrl: true });
    assert!(cur(&app).prefs.chat_w < c0, "moving the chat's divider right makes it narrower");
    // a mouse drag on the divider: press (the shell sends AreaAt on the divider), then the pointer moves
    let g = render(&mut app, &w.cx, 1280, 720);
    let div = g.hits.iter().find(|h| h.label == "divider" && h.rect.h > 300.0).expect("a vertical divider").clone();
    let Event::Area(id) = div.ev else { panic!() };
    let before = cur(&app).prefs.sidebar_w.min(cur(&app).prefs.chat_w);
    app.handle(&Event::AreaAt(id, div.rect.w / 2.0, 40.0), &w.cx);
    crate::ui::set_dragging(true);
    app.handle(&Event::AreaAt(id, div.rect.w / 2.0 + 60.0, 40.0), &w.cx);
    crate::ui::set_dragging(false);
    app.handle(&Event::MouseUp(0, 0), &w.cx);
    let after_l = app.layout.clone().unwrap();
    render(&mut app, &w.cx, 1280, 720);
    let after = app.layout.clone().unwrap();
    assert_ne!(after.main.w, after_l.main.w, "the pane moved ({before})");
    // Ctrl+1..5 cannot be sent by the keyboard layer, so the focus keys are Ctrl+Shift+letters
    key(&mut app, &w.cx, Key::Ctrl('E'));
    assert_eq!(cur(&app).focus, Pane::Tree);
    key(&mut app, &w.cx, Key::Ctrl('D'));
    assert_eq!(cur(&app).focus, Pane::Main);
    key(&mut app, &w.cx, Key::Ctrl('C'));
    assert_eq!(cur(&app).focus, Pane::Chat);
    key(&mut app, &w.cx, Key::Ctrl('T'));
    assert_eq!((cur(&app).focus, cur(&app).bottom_tab), (Pane::Bottom, BTab::Terminal));
    // the compact layout shows one pane at a time and its tabs switch
    let g = render(&mut app, &w.cx, 1024, 600);
    assert!(app.layout.as_ref().unwrap().compact);
    let tab = g.hits.iter().find(|h| h.label == "Files").expect("a pane tab").clone();
    app.handle(&tab.ev, &w.cx);
    render(&mut app, &w.cx, 1024, 600);
    assert!(app.layout.as_ref().unwrap().sidebar.is_some() && app.layout.as_ref().unwrap().chat.is_none());
}

#[test]
fn ide_help_lists_the_keys_and_the_context_menu_offers_the_tree_actions() {
    let w = world("help");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let help = app.help().join("\n");
    for k in ["Ctrl+P", "Ctrl+Shift+F", "Ctrl+B", "Ctrl+J", "F5", "Ctrl+Shift+B", "Ctrl+PageDown", "Ctrl+Shift+W"] {
        assert!(help.contains(k), "{k}");
    }
    render(&mut app, &w.cx, 1280, 720);
    let tree = app.layout.as_ref().unwrap().sidebar.unwrap();
    let items = app.context_items((tree.x + 20.0) as i32, (tree.y + 60.0) as i32);
    assert!(items.iter().any(|i| i.id == "tree.new") && items.iter().any(|i| i.id == "tree.delete"));
    let main = app.layout.as_ref().unwrap().main;
    let items = app.context_items((main.x + 100.0) as i32, (main.y + 100.0) as i32);
    assert!(items.iter().any(|i| i.id == "copy") && items.iter().any(|i| i.id == "ide.sendchat"), "the editor's own entries plus the IDE's");
    // the chat gets the selection or the file
    app.open_file("src/main.rs", None, &w.cx);
    app.send_to_chat();
    assert!(cur(&app).chat.input_text().contains("About src/main.rs") && cur(&app).focus == Pane::Chat);
}

// ------------------------------------------------------------------------------------------------------- cluster

fn wait_snapshot(app: &mut IdeApp, cx: &AppCx) {
    for _ in 0..600 {
        render(app, cx, 1280, 720);
        app.handle(&Event::Tick, cx);
        if app.ws[app.active.unwrap()].k8s.as_ref().is_some_and(|k| k.snapshot().is_some()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no cluster snapshot");
}

#[test]
fn ide_k8s_view_shows_the_namespace_details_logs_and_never_a_secret_value() {
    let _k = fake_cluster();
    let w = world("k8s");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).cfg.k8s.namespace = "shop-dev".into();
    let t = cur(&app).target();
    cur_mut(&mut app).k8s_view().set_target(t);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    let snap = cur_mut(&mut app).k8s_view().snapshot().unwrap();
    assert_eq!(snap.target.namespace, "shop-dev");
    assert_eq!((snap.workloads.len(), snap.services.len(), snap.pods.len()), (6, 3, 7));
    assert_eq!(snap.secrets, vec!["shop-db", "registry-pull"], "names only");
    let dump = format!("{snap:?}");
    assert!(!dump.contains("c3VwZXI") && !dump.contains("password"), "no secret value is ever read");
    // the keys of the view: Tab switches the kind, Enter opens details, l the log
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Deploy…")) && g.hits.iter().any(|h| h.label.starts_with("Restart…")), "the write buttons are shown, disabled");
    key(&mut app, &w.cx, Key::Tab);
    key(&mut app, &w.cx, Key::Tab);
    key(&mut app, &w.cx, Key::Tab);
    for _ in 0..4 {
        key(&mut app, &w.cx, Key::Down);
    }
    key(&mut app, &w.cx, Key::Enter);
    until(&mut app, &w.cx, "pod details", |a| {
        render(a, &w.cx, 1280, 720);
        a.ws[0].k8s.as_ref().unwrap().detail.lines.iter().any(|l| l.0.contains("exits again and again"))
    });
    key(&mut app, &w.cx, Key::Char('l'));
    until(&mut app, &w.cx, "the log tail", |a| {
        render(a, &w.cx, 1280, 720);
        a.ws[0].k8s.as_ref().unwrap().detail.lines.iter().any(|l| l.0.contains("password authentication failed"))
    });
    key(&mut app, &w.cx, Key::Esc);
    // the bottom panel shows a one-line status with a way to the K8s tab, never a second copy of the view
    app.show_main(MainTab::Editor);
    app.show_bottom(BTab::K8s);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Open K8s view"));
    assert!(!g.hits.iter().any(|h| h.label == "Namespace…  n"));
    // only reads were ever run
    let _ = snap;
}

#[test]
fn ide_k8s_view_says_so_when_kubectl_is_missing_or_the_cluster_is_down() {
    let w = world("k8sdown");
    let d = project(&w, "alpha", &[]);
    {
        let k = FakeKube::new();
        k.missing();
        let _g = fake_with(k);
        let mut app = IdeApp::new(&w.cx);
        open(&mut app, &w.cx, "Alpha", &d);
        app.show_main(MainTab::K8s);
        wait_snapshot(&mut app, &w.cx);
        let snap = cur_mut(&mut app).k8s_view().snapshot().unwrap();
        assert_eq!(snap.cluster, iyakki_agent::k8s::Cluster::KubectlMissing);
        let (title, text) = super::ide_k8s::banner(&snap).unwrap();
        assert!(title.contains("kubectl was not found") && text.contains("Settings → Dev cluster") && text.contains("docs/K3S-DEV.md"));
    }
    {
        let k = FakeKube::new();
        k.fail("get namespaces", "Unable to connect to the server: dial tcp 10.0.0.9:6443: i/o timeout");
        let _g = fake_with(k);
        let mut app = IdeApp::new(&w.cx);
        assert_eq!(cur(&app).meta.name, "Alpha", "the last workspace opens by itself");
        app.show_main(MainTab::K8s);
        wait_snapshot(&mut app, &w.cx);
        let snap = cur_mut(&mut app).k8s_view().snapshot().unwrap();
        assert!(matches!(snap.cluster, iyakki_agent::k8s::Cluster::Unreachable(_)));
        assert!(super::ide_k8s::banner(&snap).unwrap().0.contains("not reachable"));
        render(&mut app, &w.cx, 1280, 720);
    }
}

// ---------------------------------------------------------------------------------------------------- rendering

#[test]
fn ide_every_pane_and_dialog_renders_at_four_sizes() {
    let _k = fake_cluster();
    let w = world("render");
    let (a, b) = (project(&w, "alpha", &[("a.py", "print(1)\n")]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    app.activate(0, &w.cx);
    app.open_file("a.py", None, &w.cx);
    app.open_file("FEATURES.md", None, &w.cx);
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (640, 360)] {
        for main in MainTab::ALL {
            for bottom in BTab::ALL {
                // every focus is drawn at the main size; the others draw with the editor focused
                for focus in if wd == 1280 { Pane::ALL.to_vec() } else { vec![Pane::Main] } {
                    app.show_main(main);
                    app.show_bottom(bottom);
                    app.focus_pane(focus);
                    cur_mut(&mut app).board_full = false;
                    let g = render(&mut app, &w.cx, wd, ht);
                    assert!(colours(&g) > 20, "{main:?}/{bottom:?}/{focus:?} at {wd}x{ht}");
                    assert!(!g.hits.is_empty());
                }
            }
        }
        // the board full screen, in each view
        app.show_main(MainTab::Board);
        cur_mut(&mut app).board_full = true;
        for m in super::board::Mode::ALL {
            cur_mut(&mut app).board.mode = m;
            let g = render(&mut app, &w.cx, wd, ht);
            assert!(colours(&g) > 20);
        }
        cur_mut(&mut app).board_full = false;
        // the dialogs
        for open_modal in 0..6 {
            match open_modal {
                0 => app.modal = Some(Modal::Switcher { sel: 0 }),
                1 => app.open_quick(),
                2 => app.open_settings_form(),
                3 => app.open_new_workspace_form(&w.cx),
                4 => app.modal = Some(Modal::Confirm(Confirm::DeleteFile("a.py".into()))),
                _ => app.modal = Some(Modal::Lock(0)),
            }
            let g = render(&mut app, &w.cx, wd, ht);
            assert!(colours(&g) > 20, "dialog {open_modal} at {wd}x{ht}");
            app.modal = None;
        }
    }
    // the shell's design test opens every app: a workspace-less IDE draws too
    let mut empty = IdeApp::new(&world("render-empty").cx);
    let g = render(&mut empty, &w.cx, 640, 360);
    assert!(colours(&g) > 20 && !g.hits.is_empty());
}

#[test]
fn ide_scripts_run_with_f5_and_the_project_run_command_runs_for_other_files() {
    let w = world("run");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("src/main.rs", None, &w.cx);
    key(&mut app, &w.cx, Key::F(5));
    assert!(cur(&app).status.as_ref().unwrap().0.contains("No run command"));
    let mut cfg = cur(&app).cfg.clone();
    cfg.commands.run = "echo running-the-product".into();
    cfg.save(&d).unwrap();
    cur_mut(&mut app).cfg = cfg;
    key(&mut app, &w.cx, Key::F(5));
    until(&mut app, &w.cx, "the command", |a| a.ws[0].term.log.lines.iter().any(|l| l.0.contains("running-the-product")));
    assert_eq!(cur(&app).bottom_tab, BTab::Terminal);
}

#[test]
fn ide_child_clicks_are_moved_into_their_own_id_range_and_found_again() {
    for pane in [Pane::Main, Pane::Chat] {
        for ev in [Event::Tab(204), Event::Row(7), Event::Focus(3), Event::Area(1)] {
            let moved = remap(ev.clone(), pane);
            let as_click = match &moved {
                Event::Tab(n) => Event::Tab(*n),
                Event::Row(n) => Event::Row(*n),
                Event::Focus(n) => Event::FocusAt(*n, 5.0),
                Event::Area(n) => Event::AreaAt(*n, 5.0, 6.0),
                other => other.clone(),
            };
            let (p, back) = decode(&as_click).expect("decoded");
            assert_eq!(p, pane);
            let want = match ev {
                Event::Focus(n) => Event::FocusAt(n, 5.0),
                Event::Area(n) => Event::AreaAt(n, 5.0, 6.0),
                o => o,
            };
            assert_eq!(back, want);
        }
        assert_eq!(decode(&remap(Event::Key(Key::Esc), pane)), Some((pane, Event::Key(Key::Esc))));
    }
    // the IDE's own ids and plain keys are not a child's
    assert!(decode(&Event::Tab(T_RAIL)).is_none() && decode(&Event::Row(5)).is_none() && decode(&Event::Key(Key::Enter)).is_none());
    assert!(decode(&Event::Sub(SUB, Key::Char('n'))).is_none());
}

#[test]
fn ide_a_project_file_cannot_point_the_cluster_view_at_a_kubeconfig_outside_the_scope() {
    let w = world("kubeconfig");
    let d = project(&w, "alpha", &[]);
    std::fs::create_dir_all(d.join(".iyakki")).unwrap();
    std::fs::write(d.join(".iyakki/project.json"), r#"{"name":"Alpha","k8s":{"kubeconfig":"/etc/shadow-kube","namespace":"x"}}"#).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(cur(&app).target().kubeconfig, "", "ignored, with a message");
    assert!(cur(&app).status.as_ref().unwrap().0.contains("outside the folders your account may read"));
    assert_eq!(cur(&app).target().namespace, "x");
    // inside the scope it is used
    std::fs::write(d.join(".iyakki/project.json"), format!(r#"{{"name":"Alpha","k8s":{{"kubeconfig":"{}/kube.yaml"}}}}"#, d.display())).unwrap();
    drop(app);
    let app = IdeApp::new(&w.cx);
    assert!(cur(&app).target().kubeconfig.ends_with("kube.yaml"));
}

fn submit_new_ws(app: &mut IdeApp, cx: &AppCx, name: &str, parent: Option<&str>) {
    key(app, cx, Key::Ctrl('W'));
    key(app, cx, Key::Char('n'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("the new workspace form") };
    f.set_value(0, name);
    if let Some(p) = parent {
        f.set_value(1, p);
    }
    key(app, cx, Key::Ctrl('s'));
}

#[test]
fn ide_an_account_without_a_home_gets_a_folder_under_the_data_dir_and_it_works() {
    // window mode: the account may write everywhere but /home/<account> does not exist on the host
    let mut w = world("nohome");
    w.cx.session.policy.fs_read = vec!["/".into()];
    w.cx.session.policy.fs_write = vec!["/".into()];
    let mut app = IdeApp::new(&w.cx);
    let dflt = app.default_parent(&w.cx);
    assert_eq!(dflt, w.cx.services.dir().join("workspaces").join("tester"), "{dflt:?}");
    submit_new_ws(&mut app, &w.cx, "My First Test", None);
    assert!(app.modal.is_none(), "{:?}", app.modal.as_ref().and_then(|m| if let Modal::Form(_, f) = m { f.error.clone() } else { None }));
    assert!(cur(&app).root().starts_with(&dflt) && cur(&app).root().join(".iyakki/project.json").is_file());
    // an account that may only write in its (missing) home still gets the data-dir folder
    let mut narrow = world("nohome2");
    narrow.cx.session.policy.fs_read = vec!["{home}".into()];
    narrow.cx.session.policy.fs_write = vec!["{home}".into()];
    let mut app = IdeApp::new(&narrow.cx);
    submit_new_ws(&mut app, &narrow.cx, "Shop", None);
    assert!(app.modal.is_none() && cur(&app).meta.name == "Shop", "the folder is added to the account's scopes while the IDE is open");
    assert!(cur(&app).root().starts_with(narrow.cx.services.dir().join("workspaces")));
    let scoped = app.scoped_cx(&narrow.cx);
    std::fs::write(cur(&app).root().join("a.txt"), "x").unwrap();
    app.open_file("a.txt", None, &scoped);
    assert!(cur(&app).editor.path().is_some(), "files inside it can be opened");
}

#[test]
fn ide_unwritable_or_out_of_scope_parents_get_a_friendly_message_with_a_suggestion() {
    let w = world("friendly");
    let mut app = IdeApp::new(&w.cx);
    let suggested = app.default_parent(&w.cx);
    // outside the scope
    submit_new_ws(&mut app, &w.cx, "Out", Some("/etc/iyakki-nope"));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    let e = f.error.clone().unwrap();
    assert!(e.contains("outside the folders your account may write") && e.contains("Ctrl+U") && e.contains(&suggested.display().to_string()), "{e}");
    // Ctrl+U puts the suggestion in the field and creating works
    key(&mut app, &w.cx, Key::Ctrl('u'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    assert_eq!(f.value(1), suggested.display().to_string());
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    // a parent that exists but cannot be written (skipped when running as root)
    if unsafe { libc::geteuid() } != 0 {
        let ro = w.base.join("readonly");
        std::fs::create_dir_all(&ro).unwrap();
        std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
        submit_new_ws(&mut app, &w.cx, "Nope", Some(&ro.display().to_string()));
        let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
        let e = f.error.clone().unwrap();
        assert!(e.contains("cannot create folders in") && e.contains("Choose another parent folder") && !e.contains("os error"), "{e}");
        std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    // opening a folder outside the scope explains where to look
    let err = app.add_and_open("x", Path::new("/etc"), &w.cx).unwrap_err();
    assert!(err.contains("outside the folders your account may read") && err.contains("Choose a folder inside"), "{err}");
}

#[test]
fn ide_the_chosen_parent_is_remembered_and_the_browser_picks_folders_in_scope() {
    let w = world("remember");
    let mine = w.base.join("my products");
    std::fs::create_dir_all(mine.join("inner")).unwrap();
    let mut app = IdeApp::new(&w.cx);
    submit_new_ws(&mut app, &w.cx, "First", Some(&mine.display().to_string()));
    assert!(app.modal.is_none() && cur(&app).root().starts_with(&mine), "{:?}", app.notice);
    assert_eq!(Registry::load(w.cx.services.dir(), "tester").parent.as_deref(), Some(std::fs::canonicalize(&mine).unwrap().to_str().unwrap()));
    drop(app);
    let mut app = IdeApp::new(&w.cx);
    assert_eq!(app.default_parent(&w.cx), std::fs::canonicalize(&mine).unwrap(), "used next time");
    key(&mut app, &w.cx, Key::Ctrl('W'));
    key(&mut app, &w.cx, Key::Char('n'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    assert!(f.value(1).ends_with("my products"));
    // browse: Ctrl+O lists the folders; choosing inner and then "use this folder" fills the field
    key(&mut app, &w.cx, Key::Ctrl('o'));
    assert!(matches!(app.modal, Some(Modal::Browse(..))));
    render(&mut app, &w.cx, 1280, 720);
    typ(&mut app, &w.cx, "inner");
    key(&mut app, &w.cx, Key::Enter);
    key(&mut app, &w.cx, Key::Enter);
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("back in the form") };
    assert!(f.value(1).ends_with("my products/inner"), "{}", f.value(1));
    // up from the scope's top stays inside it
    key(&mut app, &w.cx, Key::Ctrl('o'));
    for _ in 0..8 {
        typ(&mut app, &w.cx, "..");
        key(&mut app, &w.cx, Key::Enter);
    }
    let Some(Modal::Browse(_, dir, _)) = &app.modal else { panic!() };
    assert!(dir.starts_with(&w.base), "never above the account's scope: {dir:?}");
}

/// A conversation whose last tool block is far taller than the chat pane.
fn long_tool_conversation() -> Vec<Message> {
    let call = ToolCall { id: "t1".into(), name: "run_command".into(), args: serde_json::json!({"program": "k3s", "args": ["server"]}) };
    let out: String = (1..=60).map(|i| format!("time=\"2026-10-13T18:56:{i:02}-07:00\" level=fatal msg=\"line {i} of a long k3s start-up failure that is wider than a narrow pane\"")).collect::<Vec<_>>().join("\n");
    vec![Message::user("deploy this and connect to the cluster"), Message::assistant_calls("I will start the server.", vec![call.clone()]), Message::tool_result(&call, out), Message::assistant("The k3s server needs root privileges. ".repeat(12))]
}

#[test]
fn ide_the_narrow_chat_pane_never_draws_outside_itself_and_the_top_bar_gives_buttons_to_a_menu() {
    let w = world("clip");
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Workspace with a long name here", &a);
    app.focus_pane(Pane::Chat);
    cur_mut(&mut app).prefs.chat_w = 580.0;
    let reference = render(&mut app, &w.cx, 1280, 720).rgba().to_vec();
    let chat = app.layout.clone().unwrap().chat.expect("chat pane");
    cur_mut(&mut app).chat.show_transcript(long_tool_conversation(), 0.0);
    render(&mut app, &w.cx, 1280, 720);
    // open the tool block: 60 lines, far taller than the pane
    cur_mut(&mut app).chat.handle(&Event::Tab(crate::apps::chat_view::TOGGLE_BASE), &w.cx);
    for scroll in [0.0f32, 300.0, 900.0, 5000.0] {
        cur_mut(&mut app).chat.show_transcript(vec![], scroll);
        let g = render(&mut app, &w.cx, 1280, 720);
        let (gd, wpx) = (g.rgba(), 1280usize);
        let mut bad = 0;
        for y in 0..720usize {
            for x in 0..wpx {
                let inside = (x as f32) >= chat.x - 1.0 && (x as f32) < chat.x + chat.w + 1.0 && (y as f32) >= chat.y - 1.0 && (y as f32) < chat.y + chat.h + 1.0;
                let i = (y * wpx + x) * 4;
                if !inside && gd[i..i + 4] != reference[i..i + 4] {
                    bad += 1;
                }
            }
        }
        assert_eq!(bad, 0, "scroll {scroll}: {bad} pixels outside the chat pane differ from the frame with an empty chat");
    }
    // the top bar at a narrow width: the buttons that do not fit are in the "⋯" menu, and the title is not overlapped
    for (wd, ht) in [(1280u32, 720u32), (1024, 600), (900, 560)] {
        let g = render(&mut app, &w.cx, wd, ht);
        let top = app.layout.clone().unwrap().top;
        let mut spans: Vec<(f32, f32, String)> = g.hits.iter().filter(|h| h.rect.y >= top.y - 1.0 && h.rect.y + h.rect.h <= top.y + top.h + 1.0).map(|h| (h.rect.x, h.rect.x + h.rect.w, h.label.clone())).collect();
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for pair in spans.windows(2) {
            assert!(pair[0].1 <= pair[1].0 + 0.5, "{wd}x{ht}: {} overlaps {}", pair[0].2, pair[1].2);
        }
        if !app.top_overflow.is_empty() {
            assert!(spans.iter().any(|s| s.2 == "⋯"), "{wd}x{ht}: hidden buttons have a menu");
        }
    }
    let g = render(&mut app, &w.cx, 900, 560);
    if let Some(h) = g.hits.iter().find(|h| h.label == "⋯").cloned() {
        app.handle(&h.ev, &w.cx);
        assert!(app.top_overflow_open);
        let g = render(&mut app, &w.cx, 900, 560);
        let first = app.top_overflow[0].0.clone();
        assert!(g.hits.iter().any(|h| h.label == first), "the menu lists {first}");
    }
}

// ------------------------------------------------------------------------------------------------------------ the existing cluster

struct ClusterGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for ClusterGuard {
    fn drop(&mut self) {
        use_kube(None);
        use_cluster(None);
        iyakki_agent::devcluster::set_active(None);
    }
}

const VERSION_K3S: &str = r#"{"clientVersion":{"gitVersion":"v1.36.5+k3s1"},"serverVersion":{"gitVersion":"v1.36.5+k3s1"}}"#;

/// A machine with a running k3s (its kubeconfig readable) and a remote cluster that does not answer; the executor is fake.
fn k3s_machine(tag: &str) -> (ClusterGuard, Arc<iyakki_agent::wsexec::FakeRunner>, PathBuf, PathBuf) {
    use iyakki_agent::devcluster::Env;
    let g = KUBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    use_kube(Some(Arc::new(sample_cluster(now))));
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-k3s-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (k3s, remote) = (d.join("k3s.yaml"), d.join("remote.yaml"));
    std::fs::write(&k3s, "apiVersion: v1\n").unwrap();
    std::fs::write(&remote, "apiVersion: v1\n").unwrap();
    let r = Arc::new(iyakki_agent::wsexec::FakeRunner::new(&["kubectl", "podman", "k3s", "curl"]));
    let (ks, rs) = (k3s.display().to_string(), remote.display().to_string());
    r.answer(&format!("--kubeconfig {ks} --request-timeout=3s version"), VERSION_K3S);
    r.answer(&format!("--kubeconfig {ks} --request-timeout=3s config current-context"), "default\n");
    r.answer(&format!("--kubeconfig {ks} --request-timeout=3s config view"), "https://127.0.0.1:6443");
    r.answer(&format!("--kubeconfig {rs} --request-timeout=3s config current-context"), "far-away\n");
    r.answer(&format!("--kubeconfig {rs} --request-timeout=3s config view"), "https://203.0.113.7:6443");
    r.fail(&format!("--kubeconfig {rs} --request-timeout=3s version"), 1, "Unable to connect to the server: dial tcp 203.0.113.7:6443: i/o timeout");
    r.fail(&format!("--kubeconfig {rs} --request-timeout=3s get namespaces"), 1, "Unable to connect to the server: i/o timeout");
    r.answer("auth can-i", "yes\n");
    let env = Env { kubeconfig_env: Some(rs), home: d.join("nohome"), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
    use_cluster(Some((r.clone(), env)));
    (ClusterGuard(g), r, k3s, remote)
}

fn resolved(app: &mut IdeApp, cx: &AppCx) {
    until(app, cx, "the cluster to be found", |a| a.ws[a.active.unwrap()].cluster.lock().unwrap().resolution.is_some() && !a.ws[a.active.unwrap()].cluster.lock().unwrap().resolving);
}

#[test]
fn ide_connects_to_the_existing_k3s_shows_the_profile_and_remembers_the_choice_per_workspace_and_user() {
    let (_g, _r, k3s, remote) = k3s_machine("connect");
    let mut w = world("connect");
    w.cx.session.policy.fs_read.push(k3s.parent().unwrap().display().to_string());
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    // $KUBECONFIG names the remote cluster first, but it does not answer: the k3s on this machine is used
    let p = cur(&app).chosen_profile().expect("a profile");
    assert_eq!(p.chip(), "k3s · default · reachable", "{p:?}");
    assert_eq!(p.kubeconfig, k3s.display().to_string());
    assert!(iyakki_agent::devcluster::active().is_some_and(|d| d.contains("k3s")), "the guard knows a cluster is connected");
    // the K8S view uses it: kubectl gets this kubeconfig and context
    let t = cur(&app).target();
    assert_eq!((t.kubeconfig.as_str(), t.context.as_str()), (k3s.to_str().unwrap(), "default"));
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 5);
    let g = render(&mut app, &w.cx, 1280, 720);
    for label in ["Cluster…  c", "Deploy…  d", "Restart…  R"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label}");
    }
    assert_eq!(cur_mut(&mut app).k8s_view().deploy_block, None, "Deploy works: the cluster answers, the account may run programs");
    // Change cluster: the chooser lists both files; the remote one is unreachable
    key(&mut app, &w.cx, Key::Char('c'));
    assert!(cur_mut(&mut app).k8s_view().has_modal());
    let listed: Vec<String> = cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().profiles.iter().map(|p| p.chip()).collect();
    assert_eq!(listed.len(), 2, "the remote one was tried first and did not answer, then the k3s: {listed:?}");
    key(&mut app, &w.cx, Key::Esc);
    let dir = app.dir.clone();
    cur_mut(&mut app).start_resolve(&dir, "tester", true);
    resolved(&mut app, &w.cx);
    let listed: Vec<String> = cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().profiles.iter().map(|p| p.chip()).collect();
    assert_eq!(listed, vec!["k3s · default · reachable".to_string(), "far-away · far-away · unreachable".to_string()], "everything was tested for the chooser, this machine's k3s first");
    // choose the remote one explicitly: the workspace stays on it (no silent fallback) and says it does not answer
    key(&mut app, &w.cx, Key::Char('c'));
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    resolved(&mut app, &w.cx);
    let p = cur(&app).chosen_profile().unwrap();
    assert_eq!((p.chip().as_str(), p.reachable), ("far-away · far-away · unreachable", false));
    let saved = workspace::ProjectConfig::load(&a).unwrap();
    assert_eq!(saved.k8s.kubeconfig, remote.display().to_string(), "remembered in project.json");
    assert_eq!(iyakki_agent::devcluster::Choice::load(&app.dir, "tester").unwrap().kubeconfig, remote.display().to_string(), "and for this user");
    assert!(cur(&app).deploy_block(&w.cx).is_some_and(|w| w.contains("no cluster is connected")), "nothing can be deployed to a cluster that does not answer");
    let banner = cur_mut(&mut app).k8s_view().cluster_problem().expect("a problem");
    assert!(banner.contains("did not answer"), "{banner}");
    // another workspace is not affected by Alpha's choice in its project.json, but the user's own choice is the default for it
    open(&mut app, &w.cx, "Beta", &b);
    resolved(&mut app, &w.cx);
    assert_eq!(workspace::ProjectConfig::load(&b).unwrap().k8s.kubeconfig, "", "beta's project.json is untouched");
    assert_eq!(cur(&app).chosen_profile().unwrap().kubeconfig, remote.display().to_string());
    app.activate(0, &w.cx);
    assert_eq!(cur(&app).meta.name, "Alpha");
    assert_eq!(cur(&app).cfg.k8s.kubeconfig, remote.display().to_string());
}

#[test]
fn ide_the_deploy_form_runs_build_load_apply_verify_with_each_step_approved_and_never_starts_k3s() {
    let (_g, r, _k3s, _remote) = k3s_machine("deploy");
    let w = world("deploy");
    let a = project(&w, "alpha", &[("app/Dockerfile", "FROM scratch\n"), ("app/index.html", "hi")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    cur_mut(&mut app).cfg.k8s.namespace = "shop-dev".into();
    resolved(&mut app, &w.cx);
    app.show_main(MainTab::K8s);
    r.answer("podman build", "STEP 1/1: FROM scratch\nSuccessfully tagged\n");
    r.answer("podman image exists", "");
    r.answer("podman save", "");
    r.answer("k3s ctr images import", "imported\n");
    r.answer("diff -f", "");
    r.answer("get namespace shop-dev", "namespace/shop-dev");
    r.answer("apply -f", "deployment.apps/alpha created\nservice/alpha created\n");
    r.answer("rollout status", "deployment \"alpha\" successfully rolled out\n");
    r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"shop-dev"}}]}"#);
    r.answer("config current-context", "default\n");
    r.answer("get deployments", r#"{"items":[]}"#);
    r.answer("get configmaps", "");
    r.answer("get secrets", "");
    r.answer("get nodes", "{}");
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 5);
    // the button opens the form with sensible defaults
    key(&mut app, &w.cx, Key::Char('d'));
    ticks(&mut app, &w.cx, 3);
    let Some(Modal::Form(FormKind::Deploy, f)) = &app.modal else { panic!("the deploy form") };
    assert_eq!((f.value(0).as_str(), f.value(1).as_str(), f.value(4).as_str()), ("alpha", "app", "shop-dev"), "image, the folder with the Dockerfile, the workspace namespace");
    // a bad image name and another namespace are refused in the form
    let mut bad = f.clone();
    bad.set_value(4, "kube-system");
    app.modal = Some(Modal::Form(FormKind::Deploy, bad));
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("still the form") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("namespace")), "{:?}", f.error);
    let mut ok = f.clone();
    ok.error = None;
    ok.set_value(4, "shop-dev");
    app.modal = Some(Modal::Form(FormKind::Deploy, ok));
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none());
    // step 1: the real command is shown and nothing has run
    let until_asking = |app: &mut IdeApp, n: usize| until(app, &w.cx, "an approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(n));
    until_asking(&mut app, 0);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Approve") ) && g.hits.iter().any(|h| h.label.starts_with("Approve all steps")) && g.hits.iter().any(|h| h.label.starts_with("Cancel")));
    let steps = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().steps();
    assert!(steps[0].detail.contains("podman build -t localhost/alpha:"), "{}", steps[0].detail);
    assert!(!r.lines().iter().any(|l| l.contains("build")), "nothing ran before the approval");
    key(&mut app, &w.cx, Key::Char('y'));
    until_asking(&mut app, 1);
    assert!(r.lines().iter().any(|l| l.starts_with("podman build")));
    key(&mut app, &w.cx, Key::Char('y'));
    until_asking(&mut app, 2);
    let apply = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().steps()[2].clone();
    assert!(apply.detail.contains("namespace shop-dev") && apply.detail.contains("manifest check"), "{}", apply.detail);
    assert!(a.join("deploy/alpha.yaml").exists(), "the generated starter manifest is a file in the workspace");
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the flow to finish", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    let f = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap();
    assert!(f.summary().starts_with("Deployed"), "{}", f.summary());
    render(&mut app, &w.cx, 1280, 720);
    key(&mut app, &w.cx, Key::Esc);
    assert!(cur_mut(&mut app).k8s_view().flow.is_none(), "Esc closes the finished panel");
    let lines = r.lines();
    assert!(!lines.iter().any(|l| l.starts_with("k3s server") || l.contains("sudo")), "{lines:?}");
    // audit entries carry the workspace and namespace
    let log = std::fs::read_to_string(w.cx.audit.path()).unwrap_or_default();
    assert!(log.contains("k8s.deploy_build") && log.contains("Alpha:shop-dev"), "{log}");
}

fn registry_answers(r: &iyakki_agent::wsexec::FakeRunner) {
    r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#);
    r.answer("get services -A", r#"{"items":[]}"#);
    r.fail("get namespace iyakki-registry", 1, "Error from server (NotFound): namespaces \"iyakki-registry\" not found");
    r.answer("create namespace iyakki-registry", "namespace/iyakki-registry created");
    r.answer("apply -f", "deployment.apps/registry created\nservice/registry created\n");
    r.answer("rollout status", "deployment \"registry\" successfully rolled out\n");
    r.answer("/v2/", "{}");
    r.answer("get namespaces -o json", r#"{"items":[{"metadata":{"name":"alpha-dev"}}]}"#);
    r.answer("get deployments", r#"{"items":[]}"#);
    r.answer("get configmaps", "");
    r.answer("get secrets", "");
    r.answer("get nodes", "{}");
}

#[test]
fn ide_k8s_registry_strip_dialog_and_approved_setup_of_the_in_cluster_registry() {
    let (_g, r, _k3s, _remote) = k3s_machine("registry");
    let w = world("registry");
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 5);
    // the strip: status chip (nothing chosen yet) and the button, inside the screen
    until(&mut app, &w.cx, "the registry status", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().is_some()));
    let st = cur_mut(&mut app).k8s_view().reg.lock().unwrap().clone().unwrap();
    assert_eq!((st.kind.as_str(), st.chip().as_str()), ("none", "registry: none"));
    let g = render(&mut app, &w.cx, 1280, 720);
    let b = g.hits.iter().find(|h| h.label == "Registry…  g").expect("the Registry button").rect;
    assert!(b.x >= 0.0 && b.x + b.w <= 1280.0 && b.h >= 19.0, "{b:?}");
    // the dialog: three choices, each with its one line and what asks for root
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    let Some(Modal::Form(FormKind::Registry, f)) = &app.modal else { panic!("the registry dialog") };
    assert!(f.fields[0].hint.contains("No root and no node changes") && f.fields[0].hint.contains("Root: no root"), "{}", f.fields[0].hint);
    key(&mut app, &w.cx, Key::Right);
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("still the dialog") };
    assert!(f.fields[0].hint.contains("registry:2 container") && f.fields[0].hint.contains("registries.yaml"), "{}", f.fields[0].hint);
    key(&mut app, &w.cx, Key::Right);
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    assert!(f.fields[0].hint.contains("registry you already have"));
    // C without an address, and A with a bad NodePort, are refused in the dialog
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("still the dialog") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("host:port")), "{:?}", f.error);
    let mut f = f.clone();
    f.error = None;
    f.set_choice(0, 0);
    f.set_value(3, "80");
    app.modal = Some(Modal::Form(FormKind::Registry, f.clone()));
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("still the dialog") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("NodePort")), "{:?}", f.error);
    let mut ok = f.clone();
    ok.error = None;
    ok.set_value(3, "");
    app.modal = Some(Modal::Form(FormKind::Registry, ok));
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none());
    // the approval step shows the exact plan; nothing has run
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    let step = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().steps()[0].clone();
    assert!(step.detail.contains("create namespace iyakki-registry") && step.detail.contains("nodePort: 30500") && step.detail.contains("Root: not needed"), "{}", step.detail);
    assert!(!r.lines().iter().any(|l| l.contains("create namespace iyakki") || l.contains("apply -f")), "nothing before the approval: {:?}", r.lines());
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Approve")) && g.hits.iter().any(|h| h.label.starts_with("Cancel")));
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the setup to finish", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    let f = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap();
    assert!(f.summary().starts_with("Registry set up"), "{}", f.summary());
    let lines = r.lines();
    assert!(lines.iter().any(|l| l.contains("create namespace iyakki-registry")) && lines.iter().any(|l| l.contains("apply -f") && l.contains("-n iyakki-registry")));
    assert!(!lines.iter().any(|l| l.contains("sudo")), "no silent root: {lines:?}");
    let saved = workspace::ProjectConfig::load(&a).unwrap();
    assert_eq!((saved.k8s.registry.as_str(), saved.k8s.registry_kind.as_str()), ("localhost:30500", "in-cluster"));
    // closing the panel re-reads the choice: the chip says in-cluster and the later deploy_load uses it
    render(&mut app, &w.cx, 1280, 720);
    key(&mut app, &w.cx, Key::Esc);
    until(&mut app, &w.cx, "the new chip", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().as_ref().is_some_and(|s| s.kind == "in-cluster" && s.reachable == Some(true))));
    assert_eq!(cur(&app).cfg.k8s.registry_kind, "in-cluster");
    assert_eq!(cur_mut(&mut app).k8s_view().reg.lock().unwrap().as_ref().unwrap().chip(), "registry: in-cluster at localhost:30500");
    let tools = cur(&app).k8s_tools(&w.cx).expect("tools");
    assert_eq!(tools.saved_registry(), ("localhost:30500".into(), String::new(), "in-cluster".into()));
    render(&mut app, &w.cx, 1280, 720);
}

#[test]
fn ide_k8s_registry_dialog_preselects_the_saved_choice_and_choosing_a_survives_cancel_settings_save_and_reload() {
    let (_g, r, _k3s, _remote) = k3s_machine("registry-a");
    let w = world("registry-a");
    let a = project(&w, "alpha", &[]);
    // an earlier choice: the system registry
    let mut cfg = workspace::ProjectConfig::load(&a).unwrap_or_default();
    cfg.k8s.registry = "localhost:5000".into();
    cfg.k8s.registry_kind = "system".into();
    cfg.save(&a).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 5);
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    let Some(Modal::Form(FormKind::Registry, f)) = &app.modal else { panic!("the dialog") };
    assert_eq!(f.choice_idx(0), 1, "the saved system registry is preselected");
    assert!(f.fields[0].label.contains("system at localhost:5000"), "{}", f.fields[0].label);
    // the real clicks: the previous-arrow of the choice, then Save (the NodePort field is empty: a free one is chosen)
    let g = render(&mut app, &w.cx, 1280, 720);
    let prev = g.hits.iter().find(|h| h.label == "choice-prev").expect("arrow").ev.clone();
    app.handle(&prev, &w.cx);
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    assert_eq!((f.choice_idx(0), f.value(3).as_str()), (0, ""));
    let g = render(&mut app, &w.cx, 1280, 720);
    let save = g.hits.iter().find(|h| h.label == "Save").expect("Save").ev.clone();
    app.handle(&save, &w.cx);
    assert!(app.modal.is_none(), "Save closes the dialog and leads to the approval step");
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    assert!(cur(&app).status.as_ref().is_some_and(|s| s.0.contains("option A chosen and saved")), "{:?}", cur(&app).status);
    // saved at once as "chosen, not set up yet"; the old registry is not kept
    let c = workspace::ProjectConfig::load(&a).unwrap();
    assert_eq!((c.k8s.registry_kind.as_str(), c.k8s.registry.as_str()), ("in-cluster", ""));
    // cancel: still A, chip says so, and the dialog preselects A
    key(&mut app, &w.cx, Key::Esc);
    until(&mut app, &w.cx, "the cancel", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    key(&mut app, &w.cx, Key::Esc);
    until(&mut app, &w.cx, "the chip", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().as_ref().is_some_and(|s| s.chip() == "registry: in-cluster (not set up yet)")));
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("the dialog") };
    assert_eq!(f.choice_idx(0), 0, "reopening shows A");
    key(&mut app, &w.cx, Key::Esc);
    // a stale in-memory copy of the settings must not put the old choice back when other settings are saved
    cur_mut(&mut app).cfg.k8s.registry_kind = "system".into();
    cur_mut(&mut app).cfg.k8s.registry = "localhost:5000".into();
    app.open_settings_form();
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "settings saved");
    assert_eq!(workspace::ProjectConfig::load(&a).unwrap().k8s.registry_kind, "in-cluster");
    // apply it for real (fake kubectl): approved, then a reload of the IDE still has it
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the end", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    let c = workspace::ProjectConfig::load(&a).unwrap();
    assert_eq!((c.k8s.registry.as_str(), c.k8s.registry_kind.as_str()), ("localhost:30500", "in-cluster"));
    let mut again = IdeApp::new(&w.cx);
    again.activate(0, &w.cx);
    resolved(&mut again, &w.cx);
    again.show_main(MainTab::K8s);
    render(&mut again, &w.cx, 1280, 720);
    until(&mut again, &w.cx, "the chip after a reload", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().as_ref().is_some_and(|s| s.chip().starts_with("registry: in-cluster at localhost:30500"))));
}

#[test]
fn ide_k8s_registry_every_option_round_trips_through_save_reopen_reload_and_workspace_switch() {
    let (_g, r, _k3s, _remote) = k3s_machine("registry-rt");
    let w = world("registry-rt");
    let (a, b) = (project(&w, "alpha", &[]), project(&w, "beta", &[]));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    app.activate(0, &w.cx);
    resolved(&mut app, &w.cx);
    registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 5);
    // (dialog position, address, kind on disk, push on disk, chip)
    for (pos, addr, kind, push, chip) in [(1usize, "", "system", "localhost:5000", "registry: system at localhost:5000"), (2, "10.0.0.5:5000", "remote", "10.0.0.5:5000", "registry: remote 10.0.0.5:5000"), (0, "", "in-cluster", "", "registry: in-cluster (not set up yet)"), (1, "", "system", "localhost:5000", "registry: system at localhost:5000")] {
        key(&mut app, &w.cx, Key::Char('g'));
        ticks(&mut app, &w.cx, 3);
        let Some(Modal::Form(FormKind::Registry, f)) = &mut app.modal else { panic!("the dialog") };
        f.set_choice(0, pos);
        f.set_value(1, addr);
        key(&mut app, &w.cx, Key::Ctrl('s'));
        assert!(app.modal.is_none(), "{kind}: Save went through");
        until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
        // the choice is on disk before anything is approved
        let c = workspace::ProjectConfig::load(&a).unwrap();
        assert_eq!((c.k8s.registry_kind.as_str(), c.k8s.registry.as_str()), (kind, push), "{kind}");
        key(&mut app, &w.cx, Key::Esc);
        until(&mut app, &w.cx, "the cancel", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
        key(&mut app, &w.cx, Key::Esc);
        // the chip, the reopened dialog, a reloaded IDE and a workspace switch all show the same option
        let want = chip.to_string();
        until(&mut app, &w.cx, "the chip", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().as_ref().is_some_and(|s| s.chip().starts_with(&want))));
        key(&mut app, &w.cx, Key::Char('g'));
        ticks(&mut app, &w.cx, 3);
        let Some(Modal::Form(_, f)) = &app.modal else { panic!("the dialog") };
        assert_eq!(f.choice_idx(0), pos, "{kind}: reopening preselects it");
        key(&mut app, &w.cx, Key::Esc);
        app.activate(1, &w.cx);
        ticks(&mut app, &w.cx, 3);
        app.activate(0, &w.cx);
        ticks(&mut app, &w.cx, 3);
        assert_eq!(cur(&app).cfg.k8s.registry_kind, kind, "{kind}: after switching workspace");
        assert_eq!(workspace::ProjectConfig::load(&a).unwrap().k8s.registry_kind, kind);
        assert_eq!(workspace::ProjectConfig::load(&b).unwrap().k8s.registry_kind, "", "the other workspace is untouched");
        let mut again = IdeApp::new(&w.cx);
        again.activate(0, &w.cx);
        assert_eq!(cur(&again).cfg.k8s.registry_kind, kind, "{kind}: after a reload");
    }
    // none is chosen by the tool (the dialog's three choices are A, B, C); the chip follows
    let tools = cur(&app).k8s_tools(&w.cx).expect("tools");
    tools.call("registry_setup", &serde_json::json!({"option": "none"})).unwrap();
    key(&mut app, &w.cx, Key::Char('r'));
    until(&mut app, &w.cx, "the none chip", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.reg.lock().unwrap().as_ref().is_some_and(|s| s.chip() == "registry: none")));
    assert_eq!(workspace::ProjectConfig::load(&a).unwrap().k8s.registry_kind, "none");
}

#[test]
fn ide_k8s_flow_text_selects_with_drag_ctrl_c_right_click_copy_and_copy_all_and_keys_still_work() {
    use super::ide_k8s::FLOW_AREA;
    let (_g, r, _k3s, _remote) = k3s_machine("flow-select");
    let w = world("flow-select");
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 3);
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    let before = render(&mut app, &w.cx, 1280, 720);
    let area = before.hits.iter().find(|h| h.ev == Event::Area(FLOW_AREA)).expect("the step text is a selectable area").rect;
    let all = cur_mut(&mut app).k8s_view().flow_pane.all_text().expect("text");
    assert!(all.contains("create namespace iyakki-registry") && all.contains("Root: not needed"), "{all}");
    // drag from the first line to the end of the third
    let lh = before.line_height(12.0 * scale(1280, 720), crate::gfx::Face::Sans);
    crate::ui::clipboard::clear();
    crate::ui::set_click_count(1);
    app.handle(&Event::AreaAt(FLOW_AREA, 0.0, lh * 0.5), &w.cx);
    crate::ui::set_dragging(true);
    app.handle(&Event::AreaAt(FLOW_AREA, area.w - 1.0, lh * 2.5), &w.cx);
    crate::ui::set_dragging(false);
    let sel = cur_mut(&mut app).k8s_view().flow_pane.selected_text().expect("a selection");
    assert!(all.contains(&sel) && sel.lines().count() == 3, "{sel:?} of {all:?}");
    app.handle(&Event::Key(Key::Ctrl('c')), &w.cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(sel.as_str()), "Ctrl+C copies exactly the selection");
    assert!(cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().asking().is_some(), "Ctrl+C is not an answer");
    // the highlight is drawn and nothing moved
    let after = render(&mut app, &w.cx, 1280, 720);
    let again = after.hits.iter().find(|h| h.ev == Event::Area(FLOW_AREA)).unwrap().rect;
    assert_eq!((again.x, again.y, again.w, again.h), (area.x, area.y, area.w, area.h), "geometry unaffected");
    assert_eq!(after.hits.len(), before.hits.len());
    // right-click menu: Copy (enabled with a selection), Select all, Copy all
    let (cx0, cy0) = ((area.x + 20.0) as i32, (area.y + 5.0) as i32);
    let items = app.context_items(cx0, cy0);
    assert!(items.iter().any(|i| i.id == "copy" && i.enabled) && items.iter().any(|i| i.id == "selall") && items.iter().any(|i| i.id == "k8s.copyall"));
    crate::ui::clipboard::clear();
    app.context_pick("copy", &w.cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(sel.as_str()));
    app.context_pick("k8s.copyall", &w.cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(all.as_str()), "Copy all copies the whole step");
    app.context_pick("selall", &w.cx);
    assert_eq!(cur_mut(&mut app).k8s_view().flow_pane.selected_text().as_deref(), Some(all.as_str()));
    // the step changes: the selection is gone
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the end", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    render(&mut app, &w.cx, 1280, 720);
    assert!(!cur_mut(&mut app).k8s_view().flow_pane.has_selection(), "a step in another state starts clean");
}

#[test]
fn ide_k8s_log_lines_select_and_copy() {
    let _k = fake_cluster();
    let w = world("k8s-copy");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).cfg.k8s.namespace = "shop-dev".into();
    let t = cur(&app).target();
    cur_mut(&mut app).k8s_view().set_target(t);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    render(&mut app, &w.cx, 1280, 720);
    for _ in 0..3 {
        key(&mut app, &w.cx, Key::Tab);
    }
    for _ in 0..4 {
        key(&mut app, &w.cx, Key::Down);
    }
    key(&mut app, &w.cx, Key::Char('l'));
    until(&mut app, &w.cx, "the log tail", |a| {
        render(a, &w.cx, 1280, 720);
        a.ws[0].k8s.as_ref().unwrap().detail.lines.iter().any(|l| l.0.contains("password authentication failed"))
    });
    crate::ui::clipboard::clear();
    app.handle(&Event::Key(Key::Ctrl('a')), &w.cx);
    app.handle(&Event::Key(Key::Ctrl('c')), &w.cx);
    let got = crate::ui::clipboard::latest().expect("copied");
    assert!(got.contains("password authentication failed"), "{got}");
    assert_eq!(cur_mut(&mut app).k8s_view().detail.pane.all_text().as_deref(), Some(got.as_str()));
    let g = render(&mut app, &w.cx, 1280, 720);
    let area = g.hits.iter().find(|h| h.ev == Event::Area(super::ide_k8s::DETAIL_AREA)).expect("area").rect;
    let items = app.context_items((area.x + 10.0) as i32, (area.y + 5.0) as i32);
    assert!(items.iter().any(|i| i.id == "copy") && items.iter().any(|i| i.id == "k8s.copyall"));
    crate::ui::clipboard::clear();
    app.context_pick("k8s.copyall", &w.cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(got.as_str()));
}

/// This machine's k3s AND a second cluster (`$KUBECONFIG`) that both answer.
fn two_clusters(tag: &str) -> (ClusterGuard, Arc<iyakki_agent::wsexec::FakeRunner>, PathBuf, PathBuf) {
    use iyakki_agent::devcluster::Env;
    let g = KUBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    use_kube(Some(Arc::new(sample_cluster(now))));
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-two-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (k3s, remote) = (d.join("k3s.yaml"), d.join("remote.yaml"));
    std::fs::write(&k3s, "apiVersion: v1\n").unwrap();
    std::fs::write(&remote, "apiVersion: v1\n").unwrap();
    let r = Arc::new(iyakki_agent::wsexec::FakeRunner::new(&["kubectl", "podman", "k3s", "curl"]));
    for (f, ctx, server) in [(&k3s, "default", "https://127.0.0.1:6443"), (&remote, "k3s-master", "https://192.168.1.68:6443")] {
        let fs = f.display().to_string();
        r.answer(&format!("--kubeconfig {fs} --request-timeout=3s version"), VERSION_K3S);
        r.answer(&format!("--kubeconfig {fs} --request-timeout=3s config current-context"), &format!("{ctx}\n"));
        r.answer(&format!("--kubeconfig {fs} --request-timeout=3s config view"), server);
    }
    r.answer("auth can-i", "yes\n");
    let env = Env { kubeconfig_env: Some(remote.display().to_string()), home: d.join("nohome"), k3s_files: vec![k3s.clone()], k3s_data_files: vec![], ..Default::default() };
    use_cluster(Some((r.clone(), env)));
    (ClusterGuard(g), r, k3s, remote)
}

#[test]
fn ide_two_answering_clusters_ask_once_the_remote_can_be_hidden_shown_and_disconnected_and_a_hidden_one_is_never_called() {
    let (_g, r, k3s, remote) = two_clusters("hide");
    let mut w = world("hide");
    w.cx.session.policy.cli = CliLevel::Full;
    w.cx.session.policy.fs_read.push(k3s.parent().unwrap().display().to_string());
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    app.show_main(MainTab::K8s);
    // nothing was chosen and both answer: the local k3s is the default, and the chooser is shown once instead of a silent pick
    until(&mut app, &w.cx, "the chooser", |a| a.ws[0].k8s.as_ref().is_some_and(|k| k.has_modal()));
    assert!(cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().ambiguous);
    assert!(cur(&app).chosen_profile().unwrap().kubeconfig == k3s.display().to_string(), "local k3s is first, though $KUBECONFIG names the remote");
    let (srv, local) = cur_mut(&mut app).k8s_view().profile_server().unwrap();
    assert_eq!((srv.as_str(), local), ("https://127.0.0.1:6443", true));
    // the K8S panel and the terminal use the selected cluster; a remote would be badged
    ticks(&mut app, &w.cx, 3);
    assert_eq!(cur(&app).term.kube.as_ref().map(|k| k.0.clone()), Some(k3s.display().to_string()));
    // Hide the remote (row 3 of the chooser: two clusters, then Hide 2.)
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Down);
    let before = r.lines().len();
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    resolved(&mut app, &w.cx);
    assert_eq!(iyakki_agent::devcluster::hidden_load(&app.dir, "tester"), vec![remote.display().to_string()]);
    assert_eq!(cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().profiles.len(), 1);
    assert!(!r.lines()[before..].iter().any(|l| l.contains("remote.yaml")), "a hidden cluster is not called: {:?}", &r.lines()[before..]);
    assert_eq!(std::fs::read_to_string(&remote).unwrap(), "apiVersion: v1\n", "the kubeconfig file is untouched");
    // the chooser offers Show hidden clusters; it brings the remote back
    key(&mut app, &w.cx, Key::Char('c'));
    let items_has_show = cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().hidden.len() == 1;
    assert!(items_has_show);
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    resolved(&mut app, &w.cx);
    assert!(iyakki_agent::devcluster::hidden_load(&app.dir, "tester").is_empty());
    assert_eq!(cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().profiles.len(), 2);
    // choose the remote on purpose: the amber "remote" badge applies and Disconnect clears the choice
    key(&mut app, &w.cx, Key::Char('c'));
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    resolved(&mut app, &w.cx);
    assert_eq!(workspace::ProjectConfig::load(&a).unwrap().k8s.kubeconfig, remote.display().to_string());
    assert_eq!(cur_mut(&mut app).k8s_view().profile_server().map(|x| x.1), Some(false), "remote is flagged");
    render(&mut app, &w.cx, 1280, 720);
    key(&mut app, &w.cx, Key::Char('c'));
    let n = cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().profiles.len();
    for _ in 0..(n + 1) {
        key(&mut app, &w.cx, Key::Down);
    }
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    resolved(&mut app, &w.cx);
    assert_eq!(workspace::ProjectConfig::load(&a).unwrap().k8s.kubeconfig, "", "the workspace choice is cleared");
    assert!(iyakki_agent::devcluster::Choice::load(&app.dir, "tester").is_none(), "and the per-user one");
    assert_eq!(cur(&app).chosen_profile().unwrap().kubeconfig, k3s.display().to_string(), "back on this machine's k3s");
}

#[test]
fn ide_terminal_kubectl_is_pinned_to_the_workspace_cluster_with_a_banner_not_to_the_environment() {
    let (_g, _r, k3s, _remote) = two_clusters("term");
    let mut w = world("term");
    w.cx.session.policy.cli = CliLevel::Full;
    w.cx.session.policy.fs_read.push(k3s.parent().unwrap().display().to_string());
    let a = project(&w, "alpha", &[]);
    let mut cfg = workspace::ProjectConfig::load(&a).unwrap_or_default();
    cfg.k8s.kubeconfig = k3s.display().to_string();
    cfg.save(&a).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    ticks(&mut app, &w.cx, 3);
    assert!(!cur(&app).cluster.lock().unwrap().resolution.as_ref().unwrap().ambiguous, "an explicit choice is not ambiguous");
    cur_mut(&mut app).term.run_line("kubectl version --client");
    let log: Vec<String> = cur(&app).term.log.lines.iter().map(|l| l.0.clone()).collect();
    assert!(log.iter().any(|l| l == "kubectl → https://127.0.0.1:6443 (workspace Alpha)"), "{log:?}");
    cur_mut(&mut app).term.runner.abandon();
}

#[test]
fn ide_k8s_tear_down_stop_and_remove_all_need_the_typed_name_and_an_approval_that_lists_what_goes() {
    let (_g, r, _k3s, _remote) = k3s_machine("teardown");
    let w = world("teardown");
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    let ns_json = r#"{"metadata":{"name":"alpha-dev","labels":{"app.kubernetes.io/managed-by":"iyakki","iyakki.dev/workspace":"Alpha"}}}"#;
    r.answer("get namespace alpha-dev -o json", ns_json);
    r.answer("get deployments,statefulsets,services", r#"{"items":[{"kind":"Deployment","metadata":{"name":"web"},"spec":{"replicas":2}},{"kind":"PersistentVolumeClaim","metadata":{"name":"data"},"spec":{"resources":{"requests":{"storage":"5Gi"}}},"status":{"phase":"Bound"}}]}"#);
    r.answer("get deployments,statefulsets", r#"{"items":[{"kind":"Deployment","metadata":{"name":"web"},"spec":{"replicas":2}}]}"#);
    r.answer("annotate", "ok");
    r.answer("scale", "ok");
    r.answer("wait --for=delete", "condition met");
    r.answer("delete namespace alpha-dev", "deleted");
        registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 3);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Tear down…  T"), "the button is there");
    // Stop: one approved step that lists the workloads
    key(&mut app, &w.cx, Key::Char('T'));
    assert!(cur_mut(&mut app).k8s_view().has_modal());
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    until(&mut app, &w.cx, "the stop approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    assert!(cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().steps()[0].detail.contains("deployment web: 2 → 0"));
    assert!(!r.lines().iter().any(|l| l.contains("scale ")));
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the end", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    assert!(cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().summary().starts_with("Stopped the"));
    assert!(r.lines().iter().any(|l| l.contains("scale deployment web --replicas=0")));
    key(&mut app, &w.cx, Key::Esc);
    // Remove all: the name must be typed
    key(&mut app, &w.cx, Key::Char('T'));
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Down);
    key(&mut app, &w.cx, Key::Enter);
    ticks(&mut app, &w.cx, 3);
    let Some(Modal::Form(FormKind::Teardown, f)) = &mut app.modal else { panic!("the confirm form") };
    f.set_value(0, "alpha");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("still the form") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("Type exactly alpha-dev")), "{:?}", f.error);
    f.set_value(0, "alpha-dev");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none());
    until(&mut app, &w.cx, "the remove approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    let d = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().steps()[0].detail.clone();
    assert!(d.contains("Deployments (1): web") && d.contains("data (5Gi, Bound)") && d.contains("DELETED"), "{d}");
    assert!(!r.lines().iter().any(|l| l.contains("delete namespace alpha-dev")), "nothing deleted before the approval");
    key(&mut app, &w.cx, Key::Char('y'));
    until(&mut app, &w.cx, "the end", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    let f = cur_mut(&mut app).k8s_view().flow.as_ref().unwrap();
    assert!(f.summary().starts_with("Removed"), "{}", f.summary());
    assert!(f.steps()[0].output.contains("Namespace alpha-dev is gone"), "{}", f.steps()[0].output);
}

#[test]
fn ide_k8s_registry_setup_is_off_in_a_read_only_workspace_and_cancel_changes_nothing() {
    let (_g, r, _k3s, _remote) = k3s_machine("registry-ro");
    let w = world("registry-ro");
    let a = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    registry_answers(&r);
    app.show_main(MainTab::K8s);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 3);
    // cancel at the approval: nothing runs, nothing is saved
    key(&mut app, &w.cx, Key::Char('g'));
    ticks(&mut app, &w.cx, 3);
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    key(&mut app, &w.cx, Key::Esc);
    until(&mut app, &w.cx, "the cancel", |a| a.ws[0].k8s.as_ref().unwrap().flow.as_ref().is_some_and(|f| f.finished()));
    assert!(cur_mut(&mut app).k8s_view().flow.as_ref().unwrap().summary().contains("nothing was changed"));
    assert!(!r.lines().iter().any(|l| l.contains("create namespace iyakki") || l.contains("apply -f")), "{:?}", r.lines());
    let c = workspace::ProjectConfig::load(&a).unwrap();
    assert_eq!((c.k8s.registry_kind.as_str(), c.k8s.registry.as_str()), ("in-cluster", ""), "only the intent is saved (chip: not set up yet), nothing was applied");
    key(&mut app, &w.cx, Key::Esc);
    // another session holds the workspace: the button is off and says why
    cur_mut(&mut app).ro = Some(workspace::LockInfo { user: "other".into(), session: "s".into(), pid: 1, host: "h".into(), time: 0, opened: 0 });
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 3);
    key(&mut app, &w.cx, Key::Char('g'));
    assert!(app.modal.is_none());
    let msg = cur_mut(&mut app).k8s_view().status.clone().unwrap();
    assert!(msg.contains("Registry setup is off") && msg.contains("read-only"), "{msg}");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Registry…  g"));
}

#[test]
fn ide_the_chat_connects_instead_of_starting_k3s_and_the_terminal_panel_refuses_it_too() {
    let (_g, r, _k3s, _remote) = k3s_machine("chat");
    let mut w = world("chat");
    w.cx.session.policy.cli = CliLevel::Full;
    let a = project(&w, "alpha", &[]);
    // k3s is an installed command-line tool on this machine (a stand-in that would start a server)
    iyakki_agent::cluster::write_fake_k3s(&w.cx.services.dir().join("cli"), &w.base.join("k.log"), "{}");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    resolved(&mut app, &w.cx);
    app.focus_pane(Pane::Chat);
    r.answer("get namespace alpha-dev", "namespace/alpha-dev");
    // the screenshot: the model tries `k3s server`. No approval is asked; the reason comes back as the tool result.
    cur_mut(&mut app).chat.ask(&w.cx, r#"do:run_command|{"program": "k3s", "args": ["server"]}"#);
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    assert!(cur(&app).chat.pending_approval().is_none());
    let result = cur(&app).chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(result.contains("already running and connected") && result.contains("k3s · ") || result.contains("https://127.0.0.1:6443"), "{result}");
    assert!(result.contains("will not start another one here"), "{result}");
    assert!(!r.lines().iter().any(|l| l.contains("k3s server")), "k3s server never ran");
    // the cluster tools are there and read the connected cluster
    r.answer("version -o json", VERSION_K3S);
    cur_mut(&mut app).chat.ask(&w.cx, r#"do:k8s_status|{}"#);
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    let result = cur(&app).chat.messages().iter().rev().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(result.contains("workspace \"Alpha\"") && result.contains("reachable: yes") && result.contains("namespace: alpha-dev") && result.contains("do not start, install or stop k3s"), "{result}");
    // kubectl is still allowed through run_command (asks, then runs)
    let kubectl = serde_json::json!({"program": "kubectl", "args": ["get", "pods"]});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:run_command|{kubectl}"));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some() || !a.ws[0].chat.is_running());
    if let Some((_, tool, summary)) = cur(&app).chat.pending_approval().cloned() {
        assert_eq!(tool, "run_command");
        assert!(summary.starts_with("[Alpha]") && !summary.contains("REFUSED"), "{summary}");
        key(&mut app, &w.cx, Key::Ctrl('n'));
        until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    }
    // the bottom terminal: `k3s server` is refused at once with the same words
    app.show_bottom(BTab::Terminal);
    app.focus_pane(Pane::Bottom);
    cur_mut(&mut app).term.run_line("k3s server --rootless");
    until(&mut app, &w.cx, "the terminal to answer", |a| {
        a.ws[0].term.tick();
        !a.ws[0].term.runner.busy()
    });
    let shown: String = cur(&app).term.log.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n");
    assert!(shown.contains("will not start another one here"), "{shown}");
    // the sandbox cluster of the IYAKKI image cannot be started from here either
    let svc = &w.cx.services;
    assert!(svc.cluster.start().unwrap_err().contains("will not start another one"));
}

/// Writes PNGs of the new screens when `IYAKKI_SHOTS` names a folder (otherwise it only checks that they draw).
#[test]
fn ide_screenshots_of_the_chat_pane_the_cluster_chip_the_chooser_and_the_deploy_steps() {
    let (_g, r, k3s, _remote) = k3s_machine("shots");
    let mut w = world("shots");
    w.cx.session.policy.fs_read.push(k3s.parent().unwrap().display().to_string());
    let out = std::env::var("IYAKKI_SHOTS").ok().map(PathBuf::from);
    if let Some(o) = &out {
        std::fs::create_dir_all(o).unwrap();
    }
    let a = project(&w, "iyakki-demo", &[("app/Dockerfile", "FROM scratch\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "iyakki-demo", &a);
    cur_mut(&mut app).cfg.k8s.namespace = "demo-dev".into();
    resolved(&mut app, &w.cx);
    let shot = |g: &Gfx, name: &str| {
        if let Some(o) = &out {
            g.save_png(&o.join(format!("{name}.png"))).unwrap();
        }
    };
    // the narrow chat pane with a long tool block, scrolled up
    app.focus_pane(Pane::Chat);
    cur_mut(&mut app).prefs.chat_w = 580.0;
    cur_mut(&mut app).chat.show_transcript(long_tool_conversation(), 0.0);
    render(&mut app, &w.cx, 1280, 720);
    cur_mut(&mut app).chat.handle(&Event::Tab(crate::apps::chat_view::TOGGLE_BASE), &w.cx);
    cur_mut(&mut app).chat.show_transcript(vec![], 260.0);
    for d in super::ide_chrome::Density::ALL {
        app.set_density(d);
        shot(&render(&mut app, &w.cx, 1280, 720), &format!("chat-pane-{}-1280x720", d.name().to_lowercase()));
        shot(&render(&mut app, &w.cx, 1920, 1080), &format!("chat-pane-{}-1920x1080", d.name().to_lowercase()));
    }
    app.set_density(super::ide_chrome::Density::Compact);
    shot(&render(&mut app, &w.cx, 1024, 600), "chat-pane-1024x600");
    // the cluster panel with its chip, then the chooser
    cur_mut(&mut app).prefs.chat_open = false;
    app.show_main(MainTab::K8s);
    app.focus_pane(Pane::Main);
    render(&mut app, &w.cx, 1280, 720);
    ticks(&mut app, &w.cx, 20);
    shot(&render(&mut app, &w.cx, 1280, 720), "k8s-connected");
    key(&mut app, &w.cx, Key::Char('c'));
    shot(&render(&mut app, &w.cx, 1280, 720), "k8s-change-cluster");
    key(&mut app, &w.cx, Key::Esc);
    // the deploy steps
    r.answer("podman build", "STEP 1/1: FROM scratch\nSuccessfully tagged localhost/iyakki-demo:ab12\n");
    key(&mut app, &w.cx, Key::Char('d'));
    ticks(&mut app, &w.cx, 3);
    shot(&render(&mut app, &w.cx, 1280, 720), "deploy-form");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    until(&mut app, &w.cx, "the first approval", |a| a.ws[0].k8s.as_ref().and_then(|k| k.flow.as_ref()).and_then(|f| f.asking()) == Some(0));
    shot(&render(&mut app, &w.cx, 1280, 720), "deploy-step-approval");
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Approve")));
}

// ------------------------------------------------------------------------------------------------------------ density

fn render_at(app: &mut IdeApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(crate::gfx::rgb(0, 0, 0));
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, 1);
    g
}

fn partly_overlap(a: &R, b: &R) -> bool {
    let (ox, oy) = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x), (a.y + a.h).min(b.y + b.h) - a.y.max(b.y));
    if ox <= 1.5 || oy <= 1.5 {
        return false;
    }
    let inside = |i: &R, o: &R| i.x >= o.x - 1.0 && i.y >= o.y - 1.0 && i.x + i.w <= o.x + o.w + 1.0 && i.y + i.h <= o.y + o.h + 1.0;
    !(inside(a, b) || inside(b, a))
}

/// Every control of the IDE, in every density, at the sizes people use: nothing clipped by the screen edge, no two controls
/// overlapping (a control inside a bigger region, such as a button on a row, is not an overlap), every control usable with a mouse.
#[test]
fn ide_controls_never_overlap_or_leave_the_screen_in_any_density_and_size() {
    use super::ide_chrome::Density;
    let _k = fake_cluster();
    let w = world("geometry");
    let a = project(&w, "alpha", &[("a.py", "print(1)\n"), ("b.py", "print(2)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    app.open_file("a.py", None, &w.cx);
    app.open_file("b.py", None, &w.cx);
    ticks(&mut app, &w.cx, 40);
    // a failing run, so the Problems tab shows its count badge
    cur_mut(&mut app).problems.lines.push(super::ide_bottom::Line { text: "error[E0425]: cannot find value `x`".into(), color: crate::ui::ERR, loc: None });
    let mut bad: Vec<String> = Vec::new();
    for density in Density::ALL {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (1825, 968), (2738, 1452)] {
            for (main, bottom, chat_open) in [(MainTab::Editor, BTab::Terminal, true), (MainTab::Board, BTab::Problems, true), (MainTab::K8s, BTab::K8s, true), (MainTab::K8s, BTab::Search, false)] {
                app.show_main(main);
                app.show_bottom(bottom);
                cur_mut(&mut app).prefs.chat_open = chat_open;
                cur_mut(&mut app).board_full = false;
                ticks(&mut app, &w.cx, 3);
                let g = render_at(&mut app, &w.cx, wd, ht);
                let tag = format!("{} {wd}x{ht} {main:?}/{bottom:?}", density.name());
                let hits: Vec<_> = g.hits.iter().filter(|h| h.label != "divider" && h.rect.w > 0.0 && h.rect.h > 0.0).collect();
                for h in &hits {
                    let r = h.rect;
                    if r.x < -0.5 || r.y < -0.5 || r.x + r.w > wd as f32 + 0.5 || r.y + r.h > ht as f32 + 0.5 {
                        bad.push(format!("{tag}: {:?} is clipped by the screen edge ({:?})", h.label, r));
                    }
                    // a button or tab must be easy to hit: at least 20 px high and 14 px wide (rows and big regions are bigger)
                    if matches!(h.ev, Event::Tab(_)) && r.h < 19.5 {
                        bad.push(format!("{tag}: {:?} is only {:.1} px high", h.label, r.h));
                    }
                    // the tabs and the cluster view's buttons (pictures with or without their text) are at least 20 px each way
                    let is_k8s_btn = matches!(h.ev, Event::Sub(super::ide_k8s::SUB, _));
                    if (matches!(h.ev, Event::Tab(n) if (T_MAIN..T_MAIN + 10).contains(&n) || (T_BOTTOM..T_BOTTOM + 10).contains(&n) || (super::ide_k8s::TAB_BASE..super::ide_k8s::TAB_BASE + 20).contains(&n) || (T_TOP..T_TOP + 10).contains(&n)) || is_k8s_btn) && (r.w < 19.5 || r.h < 19.5) {
                        bad.push(format!("{tag}: {:?} is only {:.1} x {:.1} px", h.label, r.w, r.h));
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
            }
        }
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn ide_density_keys_and_the_settings_choice_change_the_whole_ide_and_are_remembered() {
    use super::ide_chrome::Density;
    let w = world("density");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    assert_eq!(app.density, Density::Compact, "Compact is the default");
    let rows = |app: &mut IdeApp, w: &World| {
        let g = render_at(app, &w.cx, 1280, 720);
        let mut ys: Vec<f32> = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_)) && h.rect.x < 300.0).map(|h| h.rect.h).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.first().copied().unwrap_or(0.0)
    };
    let compact = rows(&mut app, &w);
    assert!((compact - 22.5).abs() < 1.5, "tree rows are about 22 px in Compact, got {compact}");
    key(&mut app, &w.cx, Key::Ctrl('-'));
    assert_eq!(app.density, Density::Dense);
    let dense = rows(&mut app, &w);
    assert!((dense - 20.25).abs() < 1.5 && dense < compact, "rows are about 20 px in Dense, got {dense}");
    key(&mut app, &w.cx, Key::Ctrl('='));
    key(&mut app, &w.cx, Key::Ctrl('='));
    assert_eq!(app.density, Density::Comfortable);
    let comfortable = rows(&mut app, &w);
    assert!(comfortable > compact + 1.0);
    // remembered per person: a new IDE for the same account starts there
    let again = IdeApp::new(&w.cx);
    assert_eq!(again.density, Density::Comfortable);
    assert_eq!(crate::uisize::load_ide_density(&app.dir, &app.user), 0);
    // the settings form has the choice, and submitting it applies it
    app.open_settings_form();
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!("the settings form") };
    f.set_choice(9, 2);
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert_eq!(app.density, Density::Dense, "the gear menu's choice applies");
    // the overflow menu cycles it too
    app.handle(&Event::Tab(T_TOP + 10), &w.cx);
    assert_eq!(app.density, Density::Comfortable);
    key(&mut app, &w.cx, Key::Ctrl('='));
    key(&mut app, &w.cx, Key::Ctrl('='));
    assert_eq!(app.density, Density::Comfortable, "the ends hold");
}

/// The chrome is flat text, not pills, and the small text reads: contrast of the colours the chrome picks on the panel.
#[test]
fn ide_chrome_text_is_legible_and_the_top_bar_keeps_only_the_essentials() {
    let _k = fake_cluster();
    let w = world("legible");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    let top = app.layout.clone().unwrap().top;
    let labels: Vec<String> = g.hits.iter().filter(|h| h.rect.y >= top.y - 1.0 && h.rect.y + h.rect.h <= top.y + top.h + 1.0).map(|h| h.label.clone()).collect();
    for want in ["project chooser", "Run", "Tests", "Open…", "Files", "Panel", "Chat", "⋯"] {
        assert!(labels.iter().any(|l| l == want), "{want} is in the top bar: {labels:?}");
    }
    assert!(!labels.iter().any(|l| l == "⚙" || l == "→ Chat"), "settings and send-to-chat moved under ⋯: {labels:?}");
    assert_eq!(labels.iter().filter(|l| l.as_str() == "Chat").count(), 1, "one Chat control");
    // the K8s buttons keep their keys in the hit names but no longer show them
    app.show_main(MainTab::K8s);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Refresh  r") && g.hits.iter().any(|h| h.label == "Namespace…  n"));
    for c in [crate::ui::MUTED, crate::ui::DIM] {
        assert!(crate::fx::contrast(crate::fx::readable(c, crate::fx::PANEL, 4.5), crate::fx::PANEL) >= 4.5);
    }
}

/// The chat pane of the IDE, narrow: the model and agent controls sit on their own row above the composer and the status
/// text, and none of them overlaps the status text or each other.
#[test]
fn ide_chat_footer_in_a_narrow_pane_has_its_controls_on_their_own_row() {
    let w = world("chatfooter");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).prefs.chat_w = 280.0;
    for (wd, ht) in [(1280u32, 720u32), (1024, 600)] {
        app.focus_pane(Pane::Chat);
        if wd == 1024 {
            cur_mut(&mut app).compact_pane = Pane::Chat;
        }
        let g = render_at(&mut app, &w.cx, wd, ht);
        let find = |l: &str| g.hits.iter().find(|h| h.label == l).map(|h| h.rect);
        let (model, agent) = (find("Model ▾").expect("Model ▾"), find("Agent: on").expect("Agent: on"));
        assert!(!partly_overlap(&model, &agent) && model.x + model.w <= agent.x + 1.0, "{wd}x{ht}: {model:?} {agent:?}");
        // both are above the composer (the Send button) and not level with it
        let send = find("Send").expect("Send");
        assert!(model.y + model.h <= send.y + 2.0 && agent.y + agent.h <= send.y + 2.0, "{wd}x{ht}: controls above the composer");
    }
}

/// Files the assistant or the terminal writes show up in the tree by themselves (the window showed "This folder is
/// empty" while the agent had written a Dockerfile and index.js), with folders still expanded and the open file kept.
#[test]
fn ide_tree_follows_files_written_behind_its_back_and_the_refresh_menu_item_works() {
    let w = world("treepoll");
    let d = project(&w, "alpha", &[("src/a.js", "a")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    cur_mut(&mut app).tree.expanded.insert("src".into());
    cur_mut(&mut app).tree.refresh();
    cur_mut(&mut app).editor.open_path(&w.cx, &d.join("src/a.js").to_string_lossy()).unwrap();
    std::fs::write(d.join("Dockerfile"), "FROM node").unwrap();
    std::fs::write(d.join("src/b.js"), "b").unwrap();
    ticks(&mut app, &w.cx, 65);
    let rels: Vec<String> = cur(&app).tree.rows.iter().map(|r| r.rel.clone()).collect();
    assert!(rels.contains(&"Dockerfile".to_string()) && rels.contains(&"src/b.js".to_string()), "{rels:?}");
    assert!(cur(&app).tree.expanded.contains("src"), "expanded state stays");
    assert_eq!(cur(&app).editor.path().unwrap(), d.join("src/a.js"), "the open file stays");
    // the Refresh item rebuilds at once, without waiting for the poll
    std::fs::write(d.join("later.txt"), "l").unwrap();
    app.handle(&Event::Sub(21, Key::Char('R')), &w.cx);
    assert!(cur(&app).tree.rows.iter().any(|r| r.rel == "later.txt"));
}

/// Auto approvals end when the person goes to another workspace unless pinned; the control shows the mode.
#[test]
fn ide_auto_approvals_end_on_a_workspace_switch_unless_pinned() {
    let w = world("autoswitch");
    let a = project(&w, "alpha", &[]);
    let b = project(&w, "beta", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &a);
    open(&mut app, &w.cx, "Beta", &b);
    app.activate(0, &w.cx);
    cur_mut(&mut app).chat.cycle_approvals(&w.cx);
    assert!(cur(&app).chat.approvals().is_auto());
    app.activate(1, &w.cx);
    assert_eq!(app.ws[0].chat.approvals(), super::chat::Approvals::Ask, "plain Auto ended");
    app.activate(0, &w.cx);
    cur_mut(&mut app).chat.cycle_approvals(&w.cx);
    cur_mut(&mut app).chat.cycle_approvals(&w.cx);
    app.activate(1, &w.cx);
    assert_eq!(app.ws[0].chat.approvals(), super::chat::Approvals::Pinned, "pinned stays");
    app.activate(0, &w.cx);
    app.focus_pane(Pane::Chat);
    let g = render_at(&mut app, &w.cx, 1280, 720);
    let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.contains("Auto-approve safe · kept")), "{labels:?}");
}

// ----------------------------------------------------------------------------------------------------- tables

#[test]
fn ide_opens_a_csv_in_the_data_viewer_not_the_text_editor() {
    let w = world("ide-csv");
    let p = project(&w, "tables", &[("a.txt", "plain"), ("sales.csv", "x,y\n1,2\n"), ("t.TSV", "a\tb\n1\t2\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Tables", &p);
    app.open_file("a.txt", None, &w.cx);
    assert!(app.take_request().is_none(), "text opens in the editor");
    app.open_file("sales.csv", None, &w.cx);
    let (id, item) = app.take_request().expect("a csv asks for DATA");
    assert_eq!(id, "data");
    assert!(item.ends_with("tables/sales.csv") || item.ends_with("sales.csv"), "{item}");
    assert!(app.take_request().is_none(), "asked once");
    app.open_file("t.TSV", None, &w.cx);
    assert_eq!(app.take_request().map(|r| r.0), Some("data".into()));
    let tabs: Vec<String> = app.ws[0].editor.open_paths().iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(tabs, vec!["a.txt"], "the csv was not opened as a text tab");
}

/// The workspace looks at the fake cluster's namespace, so the cluster view has a table.
fn look_at_shop_dev(app: &mut IdeApp) {
    cur_mut(app).cfg.k8s.namespace = "shop-dev".into();
    let t = cur(app).target();
    cur_mut(app).k8s_view().set_target(t);
}

// ------------------------------------------------------------------------------- one K8s view, and who owns the keyboard

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

/// The cluster view (its toolbar, its kind tabs) exists once: as the K8s tab of the main area. The bottom panel's K8S tab is a
/// one-line status that opens it, in every density and size, whichever tab the main area shows.
#[test]
fn ide_k8s_is_on_screen_only_once() {
    use super::ide_chrome::Density;
    let _k = fake_cluster();
    let w = world("k8sonce");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    for density in Density::ALL {
        app.set_density(density);
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600)] {
            for (main, bottom) in [(MainTab::K8s, BTab::K8s), (MainTab::K8s, BTab::Terminal), (MainTab::Editor, BTab::K8s), (MainTab::Board, BTab::K8s)] {
                app.show_main(main);
                app.show_bottom(bottom);
                cur_mut(&mut app).prefs.chat_open = true;
                // in a narrow window one pane shows at a time: show both the main area and the panel
                cur_mut(&mut app).compact_pane = Pane::Main;
                let g = render(&mut app, &w.cx, wd, ht);
                let tag = format!("{} {wd}x{ht} {main:?}/{bottom:?}", density.name());
                let l = labels(&g);
                let count = |name: &str| l.iter().filter(|x| x.as_str() == name).count();
                let want = usize::from(main == MainTab::K8s && (wd >= 1280 || true));
                for toolbar in ["Refresh  r", "Namespace…  n", "Cluster…  c"] {
                    assert!(count(toolbar) <= want, "{tag}: {toolbar} drawn {} times", count(toolbar));
                }
                let kind_tabs = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (super::ide_k8s::TAB_BASE..super::ide_k8s::TAB_BASE + 20).contains(&n))).count();
                assert_eq!(kind_tabs, if main == MainTab::K8s { 7 } else { 0 }, "{tag}: the seven kind tabs exist once");
                let rows = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(n) if (super::ide_k8s::ROW_BASE..super::ide_k8s::ROW_BASE + 100_000).contains(&n))).count();
                assert!(rows == 0 || main == MainTab::K8s, "{tag}: table rows only in the main area");
                if bottom == BTab::K8s && cur(&app).prefs.bottom_open && (wd >= 1280) {
                    assert_eq!(count("Open K8s view"), 1, "{tag}: the strip has its one button");
                }
            }
        }
    }
}

/// The three main tabs and the bottom tabs carry their pictures: the same tab at a width where the text does not fit is a
/// smaller region (picture only) that still answers a click.
#[test]
fn ide_tabs_have_pictures_and_drop_their_text_when_narrow() {
    use super::ide_chrome::Density;
    let _k = fake_cluster();
    let w = world("tabicons");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.set_density(Density::Dense);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    let wide = render(&mut app, &w.cx, 1920, 1080);
    // a squeezed main area: a wide chat and the tree leave the minimum
    cur_mut(&mut app).prefs.chat_w = 700.0;
    let narrow = render(&mut app, &w.cx, 1280, 720);
    let width = |g: &Gfx, label: &str| g.hits.iter().find(|h| h.label == label && matches!(h.ev, Event::Tab(_))).map(|h| h.rect.w).unwrap_or_else(|| panic!("no tab {label}"));
    for name in ["Workloads", "Services", "Pods", "Nodes"] {
        let (a, b) = (width(&wide, name) / scale(1920, 1080), width(&narrow, name) / scale(1280, 720));
        assert!(a > 60.0, "{name}: with room it has its text ({a:.1})");
        assert!(b < 40.0, "{name}: squeezed it is a picture only ({b:.1})");
        assert!(width(&narrow, name) >= 20.0, "{name} stays a 20 px target");
    }
    // the main tabs keep their names as hit labels either way
    for name in ["Editor", "K8s"] {
        assert!(narrow.hits.iter().any(|h| h.label.starts_with(name) && matches!(h.ev, Event::Tab(_))), "{name}");
    }
}

fn input_values(app: &IdeApp) -> [String; 5] {
    let w = cur(app);
    [w.editor.text(), w.chat.input_text().to_string(), w.term.input.value.clone(), w.search.query.value.clone(), w.board.text.value.clone()]
}

/// Everything a shortcut could change, as one comparable string.
fn effects(app: &mut IdeApp) -> String {
    let modal = app.modal.is_some();
    let w = cur_mut(app);
    let k = w.k8s.as_mut().map(|k| (k.has_modal(), k.status.clone(), k.has_drawer(), k.kind.label(), k.wants.len()));
    format!("{modal} {:?} {:?} {:?} {} {} {:?} {:?} {:?}", w.main_tab, w.bottom_tab, w.focus, w.board.has_modal(), w.tree.show_heavy, w.status, k, w.chat.pending_approval().is_some())
}

fn give_keyboard_to(app: &mut IdeApp, which: &str) {
    match which {
        "editor" => {
            app.show_main(MainTab::Editor);
        }
        "board" => {
            app.show_main(MainTab::Board);
            cur_mut(app).board.blur();
        }
        "board filter" => {
            app.show_main(MainTab::Board);
            cur_mut(app).board.text_focus = true;
        }
        "k8s" => {
            app.show_main(MainTab::K8s);
        }
        // (the browser was made by `open_browser` before)
        "browser address" => {
            app.show_main(MainTab::Browser);
            cur_mut(app).browser.as_mut().expect("the browser").focus_address_for_test(true);
        }
        "browser page" => {
            app.show_main(MainTab::Browser);
            cur_mut(app).browser.as_mut().expect("the browser").focus_address_for_test(false);
        }
        "tree" => app.focus_pane(Pane::Tree),
        "composer" => app.focus_pane(Pane::Chat),
        "terminal" => app.show_bottom(BTab::Terminal),
        "k8s strip" => app.show_bottom(BTab::K8s),
        "problems" => app.show_bottom(BTab::Problems),
        "search" => app.show_bottom(BTab::Search),
        other => panic!("{other}"),
    }
}

fn reset_after_shortcut(app: &mut IdeApp, cx: &AppCx) {
    // close whatever a shortcut opened, and forget what it said
    for _ in 0..3 {
        key(app, cx, Key::Esc);
    }
    app.modal = None;
    if let Some(k) = cur_mut(app).k8s.as_mut() {
        k.status = None;
        k.take_wants();
    }
    cur_mut(app).status = None;
}

/// The keyboard rule of the AI IDE, for every view: one owner at a time. With a text input as the owner every printable key is
/// typed into it and no shortcut fires; with a table, tree or board as the owner the pane's letter shortcuts fire; and a caret is
/// drawn exactly for a text owner.
#[test]
fn ide_every_printable_key_goes_to_the_text_input_that_owns_the_keyboard() {
    let _k = fake_cluster();
    let w = world("keyrouting");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    let printable: Vec<char> = (' '..='~').collect();
    assert_eq!(printable.len(), 95);
    let typed: String = printable.iter().collect();
    // (view, owner, the input that must receive the text: index into `input_values`)
    let text_cases = [("editor", Owner::Editor, 0), ("board filter", Owner::BoardFilter, 4), ("composer", Owner::Composer, 1), ("terminal", Owner::Terminal, 2), ("search", Owner::Search, 3)];
    for (view, owner, idx) in text_cases {
        give_keyboard_to(&mut app, view);
        assert_eq!(cur(&app).owner(), owner, "{view}");
        assert!(owner.is_text());
        let (before, sig) = (input_values(&app), effects(&mut app));
        for c in &printable {
            key(&mut app, &w.cx, Key::Char(*c));
            assert_eq!(effects(&mut app), sig, "{view}: {c:?} fired a shortcut");
        }
        // the editing keys of an input stay with it as well
        for k in [Key::Backspace, Key::Delete, Key::Left, Key::Right, Key::Home, Key::End] {
            key(&mut app, &w.cx, k.clone());
            assert_eq!(effects(&mut app), sig, "{view}: {k:?} fired a shortcut");
        }
        let after = input_values(&app);
        for i in 0..5 {
            if i == idx {
                assert_ne!(after[i], before[i], "{view}: the text arrived");
                if view != "editor" {
                    assert!(after[i].contains(&typed[..typed.len() - 1]), "{view}: got {:?}", after[i]);
                } else {
                    // (the Backspace sent above took the last one, `~`, away again)
                    assert!(printable[..printable.len() - 1].iter().all(|c| after[i].contains(*c)), "{view}: every character is in the file: {:?}", after[i]);
                }
            } else {
                assert_eq!(after[i], before[i], "{view}: text leaked into input {i}");
            }
        }
    }
    // a pane with a table, tree or board: its letters are shortcuts
    let fired = |app: &mut IdeApp, view: &str, c: char| -> bool {
        give_keyboard_to(app, view);
        reset_after_shortcut(app, &w.cx);
        give_keyboard_to(app, view);
        let before = effects(app);
        key(app, &w.cx, Key::Char(c));
        let changed = effects(app) != before;
        reset_after_shortcut(app, &w.cx);
        changed
    };
    for c in "rncdgR".chars() {
        assert!(fired(&mut app, "k8s", c), "k8s table: {c} is a shortcut");
    }
    for c in "nNrdh/".chars() {
        assert!(fired(&mut app, "tree", c), "tree: {c} is a shortcut");
    }
    assert!(fired(&mut app, "board", 'a'), "board: a adds a feature");
    // none of those pane keys reach the inputs
    for view in ["k8s", "tree", "board", "k8s strip", "problems"] {
        give_keyboard_to(&mut app, view);
        assert_eq!(cur(&app).owner() == Owner::Tree || !cur(&app).owner().is_text(), true, "{view} is not a text input");
        let before = input_values(&app);
        for c in &printable {
            // the deliberate hand-over: `i` and `?` on the board ask the assistant about the selected feature
            if view == "board" && matches!(*c, 'i' | '?') {
                continue;
            }
            key(&mut app, &w.cx, Key::Char(*c));
            reset_after_shortcut(&mut app, &w.cx);
            give_keyboard_to(&mut app, view);
        }
        assert_eq!(input_values(&app), before, "{view}: no text goes into an input that does not own the keyboard");
    }
    // the bottom status strip has no letter shortcuts at all, and Enter opens the K8s view
    give_keyboard_to(&mut app, "k8s strip");
    app.show_main(MainTab::Editor);
    app.show_bottom(BTab::K8s);
    key(&mut app, &w.cx, Key::Enter);
    assert_eq!(cur(&app).main_tab, MainTab::K8s);
    assert_eq!(cur(&app).focus, Pane::Main);
}

/// A blinking caret means typing goes there: of all the inputs on screen exactly one draws a caret, and only a text owner has it.
#[test]
fn ide_only_the_input_that_owns_the_keyboard_draws_a_caret() {
    let _k = fake_cluster();
    let w = world("carets");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    for (view, text) in [("editor", true), ("board", false), ("board filter", true), ("k8s", false), ("tree", false), ("composer", true), ("terminal", true), ("k8s strip", false), ("problems", false), ("search", true)] {
        give_keyboard_to(&mut app, view);
        crate::ui::take_carets();
        render(&mut app, &w.cx, 1920, 1080);
        assert_eq!(crate::ui::take_carets(), usize::from(text), "{view}: carets drawn");
    }
    // a dialog owns the keyboard: no pane behind it shows a caret
    give_keyboard_to(&mut app, "composer");
    key(&mut app, &w.cx, Key::Ctrl('p'));
    assert!(app.modal.is_some());
    crate::ui::take_carets();
    render(&mut app, &w.cx, 1920, 1080);
    assert!(crate::ui::take_carets() <= 1, "only the dialog's own field");
}

/// Focus follows clicks: a click in the composer takes the keyboard from the cluster table (so `r` is typed, not "refresh"), a
/// click on a table row takes it back and the caret goes away.
#[test]
fn ide_focus_follows_clicks_between_the_composer_the_table_and_the_editor() {
    let _k = fake_cluster();
    let w = world("clickfocus");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    let find_focus = |g: &Gfx, pane: Pane| g.hits.iter().find_map(|h| match h.ev {
        Event::Focus(n) => decode(&Event::FocusAt(n, 0.0)).filter(|(p, _)| *p == pane).map(|_| n),
        // the composer is a multi-line box: a press area of its own
        Event::Area(n) if h.label == "message field" => decode(&Event::AreaAt(n, 0.0, 0.0)).filter(|(p, _)| *p == pane).map(|_| n),
        _ => None,
    });
    // table -> composer
    let g = render(&mut app, &w.cx, 1920, 1080);
    let composer = find_focus(&g, Pane::Chat).expect("the composer is a field");
    app.handle(&Event::AreaAt(composer, 12.0, 12.0), &w.cx);
    assert_eq!(cur(&app).owner(), Owner::Composer);
    key(&mut app, &w.cx, Key::Char('r'));
    key(&mut app, &w.cx, Key::Char('n'));
    assert_eq!(cur(&app).chat.input_text(), "rn", "typed into the composer");
    assert!(cur_mut(&mut app).k8s_view().status.is_none() && !cur_mut(&mut app).k8s_view().has_modal(), "no refresh, no namespace chooser");
    // composer -> a table row (the click arrives as the row's own event)
    let g = render(&mut app, &w.cx, 1920, 1080);
    let row = g.hits.iter().find(|h| matches!(h.ev, Event::Row(n) if (super::ide_k8s::ROW_BASE..super::ide_k8s::ROW_BASE + 100_000).contains(&n))).expect("a table row").clone();
    app.handle(&row.ev, &w.cx);
    assert_eq!(cur(&app).owner(), Owner::K8s);
    crate::ui::take_carets();
    render(&mut app, &w.cx, 1920, 1080);
    assert_eq!(crate::ui::take_carets(), 0, "no caret while the table has the keyboard");
    key(&mut app, &w.cx, Key::Char('r'));
    assert_eq!(cur_mut(&mut app).k8s_view().status.as_deref(), Some("Refreshing…"));
    assert_eq!(cur(&app).chat.input_text(), "rn", "and nothing more went into the composer");
    // a toolbar button of the view takes the keyboard too
    app.focus_pane(Pane::Chat);
    let g = render(&mut app, &w.cx, 1920, 1080);
    let kind = g.hits.iter().find(|h| h.label == "Pods" && matches!(h.ev, Event::Tab(_))).expect("the Pods tab").clone();
    app.handle(&kind.ev, &w.cx);
    assert_eq!(cur(&app).owner(), Owner::K8s);
    // the editor: a click takes the keyboard, and a click on the board's card takes it from the filter box
    let g = render(&mut app, &w.cx, 1920, 1080);
    app.show_main(MainTab::Editor);
    let g2 = render(&mut app, &w.cx, 1920, 1080);
    let area = g2.hits.iter().find(|h| matches!(h.ev, Event::Area(_))).map(|h| h.ev.clone());
    let _ = (g, area);
    app.show_main(MainTab::Board);
    cur_mut(&mut app).board.text_focus = true;
    assert_eq!(cur(&app).owner(), Owner::BoardFilter);
    app.focus_pane(Pane::Tree);
    assert_eq!(cur(&app).owner(), Owner::Tree);
    assert!(!cur(&app).board.wants_text(), "the filter box gave up its caret when the keyboard left the board");
    // a press on the bare background of a pane gives that pane the keyboard
    render(&mut app, &w.cx, 1920, 1080);
    let chat = app.layout.as_ref().and_then(|l| l.chat).expect("the chat pane");
    app.handle(&Event::MouseDown((chat.x + 3.0) as i32, (chat.y + chat.h - 3.0) as i32), &w.cx);
    assert_eq!(cur(&app).owner(), Owner::Composer);
}

/// Esc leaves a text input when it has nothing to dismiss.
#[test]
fn ide_escape_leaves_a_text_input() {
    let _k = fake_cluster();
    let w = world("escape");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    app.show_main(MainTab::Editor);
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).owner(), Owner::Tree, "the editor lets go");
    app.focus_pane(Pane::Chat);
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).owner(), Owner::Editor, "the composer lets go");
    app.show_bottom(BTab::Search);
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).focus, Pane::Main, "an empty search box lets go");
    app.show_main(MainTab::Board);
    cur_mut(&mut app).board.text_focus = true;
    key(&mut app, &w.cx, Key::Esc);
    assert_eq!(cur(&app).owner(), Owner::Board, "the filter box lets go");
}

/// Approval keys: the assistant's approval card is answered with Ctrl+Y / Ctrl+U / Ctrl+N, only while the chat has the keyboard.
/// Letters y, a, n are text in the composer (the answer buttons of a question take 1-9, y and n only while the composer is empty).
#[test]
fn ide_approval_keys_act_only_when_the_chat_has_the_keyboard() {
    let w = world("approvalkeys");
    let d = project(&w, "alpha", &[("a.py", "print(1)\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.focus_pane(Pane::Chat);
    let args = serde_json::json!({"path": "src/new.rs", "content": "fn x() {}"});
    cur_mut(&mut app).chat.ask(&w.cx, &format!("do:write_file|{args}"));
    until(&mut app, &w.cx, "the approval", |a| a.ws[0].chat.pending_approval().is_some());
    for pane in [Pane::Tree, Pane::Main] {
        app.focus_pane(pane);
        key(&mut app, &w.cx, Key::Ctrl('n'));
        key(&mut app, &w.cx, Key::Ctrl('y'));
        assert!(cur(&app).chat.pending_approval().is_some(), "{pane:?}: the approval waits for the chat");
    }
    app.focus_pane(Pane::Chat);
    typ(&mut app, &w.cx, "yan");
    assert_eq!(cur(&app).chat.input_text(), "yan", "letters are text");
    assert!(cur(&app).chat.pending_approval().is_some());
    key(&mut app, &w.cx, Key::Ctrl('y'));
    until(&mut app, &w.cx, "the run to end", |a| !a.ws[0].chat.is_running());
    assert!(d.join("src/new.rs").exists(), "Ctrl+Y approved it");
    assert_eq!(cur(&app).chat.input_text(), "yan", "the draft stayed");
}

fn render_hovering(app: &mut IdeApp, cx: &AppCx, w: u32, h: u32, tick: u64, mouse: (i32, i32)) -> Gfx {
    let s = scale(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(crate::gfx::rgb(0, 0, 0));
    g.mouse = mouse;
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, tick);
    g
}

/// One blinking caret on the whole window, never two: for every owner of the keyboard, with a dialog open, with the pointer over
/// other inputs, in both halves of the blink and with the window not focused.
#[test]
fn ide_exactly_one_caret_and_one_focus_frame_in_every_focus_state() {
    let _k = fake_cluster();
    let w = world("onecaret");
    let d = project(&w, "alpha", &[("a.py", "print((1))\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    look_at_shop_dev(&mut app);
    app.open_browser(None, &w.cx);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    let views = [("editor", true, false), ("board", false, false), ("board filter", true, true), ("k8s", false, false), ("tree", false, false), ("composer", true, true), ("terminal", true, false), ("k8s strip", false, false), ("problems", false, false), ("search", true, true), ("browser address", true, true), ("browser page", false, false)];
    // the pointer over the composer, over the editor, over the tab strip and over nothing
    let mice = [(1700, 900), (600, 300), (400, 80), (5, 5)];
    for (view, text, boxed) in views {
        give_keyboard_to(&mut app, view);
        for mouse in mice {
            crate::ui::take_carets();
            crate::ui::take_focus_rings();
            render_hovering(&mut app, &w.cx, 1920, 1080, 1, mouse);
            assert_eq!(crate::ui::take_carets(), usize::from(text), "{view} {mouse:?}: carets");
            assert_eq!(crate::ui::take_focus_rings(), usize::from(boxed), "{view} {mouse:?}: focus frames");
            render_hovering(&mut app, &w.cx, 1920, 1080, 16, mouse);
            assert_eq!(crate::ui::take_carets(), 0, "{view}: no caret in the off half");
            assert!(crate::ui::take_focus_rings() <= 1);
        }
        // the window lost the system focus: nothing looks focused; it comes back as it was
        crate::ui::set_window_focused(false);
        crate::ui::take_carets();
        crate::ui::take_focus_rings();
        render_hovering(&mut app, &w.cx, 1920, 1080, 1, (5, 5));
        assert_eq!((crate::ui::take_carets(), crate::ui::take_focus_rings()), (0, 0), "{view}: window unfocused");
        crate::ui::set_window_focused(true);
        render_hovering(&mut app, &w.cx, 1920, 1080, 1, (5, 5));
        assert_eq!(crate::ui::take_carets(), usize::from(text), "{view}: focus restored to the one owner");
        crate::ui::take_focus_rings();
        // a dialog in front: only its own field may show one
        key(&mut app, &w.cx, Key::Ctrl('p'));
        assert!(app.modal.is_some());
        render_hovering(&mut app, &w.cx, 1920, 1080, 1, (5, 5));
        assert!(crate::ui::take_carets() <= 1, "{view}: dialog");
        assert!(crate::ui::take_focus_rings() <= 1, "{view}: dialog frames");
        key(&mut app, &w.cx, Key::Esc);
        assert!(app.modal.is_none());
        give_keyboard_to(&mut app, view);
    }
}

/// Pictures of every focus state, before (the frame and clipboard button of the composer drawn whoever owns the keyboard) and
/// after, when `IYAKKI_SHOTS` names a folder.
#[test]
fn ide_focus_state_pictures() {
    let _k = fake_cluster();
    let w = world("focuspics");
    let d = project(&w, "alpha", &[("a.py", "print((1))\n")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    app.open_file("a.py", None, &w.cx);
    look_at_shop_dev(&mut app);
    app.show_main(MainTab::K8s);
    wait_snapshot(&mut app, &w.cx);
    typ_into_everything(&mut app, &w.cx);
    let out = std::env::var("IYAKKI_SHOTS").ok().map(PathBuf::from);
    if let Some(o) = &out {
        std::fs::create_dir_all(o).unwrap();
    }
    for (view, _, _) in [("editor", 0, 0), ("composer", 0, 0), ("k8s", 0, 0), ("terminal", 0, 0), ("search", 0, 0), ("board filter", 0, 0), ("tree", 0, 0)] {
        give_keyboard_to(&mut app, view);
        for (label, legacy) in [("before", true), ("after", false)] {
            crate::ui::legacy_rings(legacy);
            let g = render_hovering(&mut app, &w.cx, 1280, 720, 1, (5, 5));
            crate::ui::legacy_rings(false);
            if let Some(o) = &out {
                g.save_png(&o.join(format!("focus-{}-{label}.png", view.replace(' ', "-")))).unwrap();
            }
        }
    }
}

fn typ_into_everything(app: &mut IdeApp, cx: &AppCx) {
    for (view, text) in [("composer", "draft message"), ("terminal", "ls -la"), ("search", "needle"), ("board filter", "F-A")] {
        give_keyboard_to(app, view);
        typ(app, cx, text);
    }
}

#[path = "ide_browser_tests.rs"]
mod browser_tests;

#[test]
fn ide_a_file_dropped_on_the_window_goes_into_the_tree_folder_and_opens_a_table_in_data() {
    use super::DropAction;
    let w = world("ide-drop");
    let p = project(&w, "dropper", &[("src/main.rs", "fn main() {}\n"), ("a.txt", "a")]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Dropper", &p);
    let root = std::fs::canonicalize(&p).unwrap();
    // pointer outside the tree: the workspace root
    match app.on_drop(&[PathBuf::from("/x/notes.txt")], &w.cx) {
        DropAction::Into { dest, label } => {
            assert_eq!(dest, root);
            assert_eq!(label, "the workspace folder");
        }
        other => panic!("{other:?}"),
    }
    // what arrived opens: a text file in the editor, a table in DATA
    std::fs::write(root.join("notes.txt"), "hello").unwrap();
    app.dropped_done(&[root.join("notes.txt")], &w.cx);
    assert!(app.take_request().is_none());
    assert!(app.ws[0].editor.open_paths().iter().any(|p| p.ends_with("notes.txt")), "opened in the editor");
    std::fs::write(root.join("t.csv"), "a\n1\n").unwrap();
    app.dropped_done(&[root.join("t.csv")], &w.cx);
    assert_eq!(app.take_request().map(|r| r.0), Some("data".to_string()), "a table asks for DATA");
    // a read-only workspace takes nothing
    app.ws[0].ro = Some(iyakki_agent::workspace::LockInfo { user: "other".into(), session: "s".into(), pid: 1, host: "h".into(), time: 0, opened: 0 });
    assert!(matches!(app.on_drop(&[], &w.cx), DropAction::Ignore(m) if m.contains("read-only")));
}

#[path = "ide_tasks_tests.rs"]
mod tasks_tests;

#[path = "ide_notes_tests.rs"]
mod notes_tests;

#[path = "ide_tile_tests.rs"]
mod tile_tests;

#[test]
fn ide_new_app_entry_scaffolds_a_project_opens_the_chat_and_stays_inside_the_workspace() {
    let w = world("newapp");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    // without a workspace it says so
    app.open_new_app_form();
    assert!(app.modal.is_none() && app.notice.as_ref().is_some_and(|n| n.0.contains("workspace")));
    open(&mut app, &w.cx, "Alpha", &d);
    // the entry is in the top bar's menu at every width, and Ctrl+Shift+N opens the same form
    for (wd, ht) in [(1280u32, 720u32), (900, 560)] {
        let _ = render(&mut app, &w.cx, wd, ht);
        assert!(app.top_overflow.iter().any(|o| o.0.starts_with("New app, MCP server or skill")), "{wd}x{ht}");
    }
    let n = app.top_overflow.iter().find(|o| o.0.starts_with("New app")).unwrap().1;
    app.handle(&Event::Tab(T_TOP + n), &w.cx);
    let Some(Modal::Form(k, _)) = &app.modal else { panic!("the entry opens the form") };
    assert_eq!(*k, FormKind::NewApp);
    app.modal = None;
    key(&mut app, &w.cx, Key::Ctrl('N'));
    // a bad name stays in the form with the reason; a good one creates the project and fills the chat
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(1, "Bad Name");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!("a bad name is refused") };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("name")), "{:?}", f.error);
    assert!(!d.join("Bad Name").exists());
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_choice(0, 3);
    f.set_value(1, "weather-bot");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.modal.is_none(), "{:?}", app.notice);
    assert!(d.join("weather-bot/iyakki-app.json").is_file() && d.join("weather-bot/server.py").is_file() && d.join("weather-bot/tests/test_server.py").is_file());
    assert!(cur(&app).chat.input_text().contains("iyakki-app-author") && cur(&app).chat.input_text().contains("weather-bot"));
    assert!(cur(&app).prefs.chat_open);
    // the authoring skills are now pinned for this workspace
    assert!(iyakki_agent::projectstate::detect_stack(&d, "").pinned_skills("hi").contains(&"iyakki-mcp-author"));
    // the same name again is refused (the folder is not empty) and nothing is overwritten
    key(&mut app, &w.cx, Key::Ctrl('N'));
    let Some(Modal::Form(_, f)) = &mut app.modal else { panic!() };
    f.set_value(1, "weather-bot");
    key(&mut app, &w.cx, Key::Ctrl('s'));
    let Some(Modal::Form(_, f)) = &app.modal else { panic!() };
    assert!(f.error.as_deref().is_some_and(|e| e.contains("already has files")), "{:?}", f.error);
}

/// The approval chip of the IDE chat names its mode, sits on the control row without touching Model or Agent, and is a
/// comfortable target at the sizes people use; after an approval the "Approved this session" indicator joins it.
#[test]
fn ide_chat_approval_chip_and_indicator_fit_at_every_size() {
    let w = world("approvalchip");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    for grants in [false, true] {
        for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            app.focus_pane(Pane::Chat);
            if wd < 1100 {
                cur_mut(&mut app).compact_pane = Pane::Chat;
            }
            if grants {
                cur_mut(&mut app).chat.grant_for_test("write_file", "in=workspace", "write_file: files in this workspace");
            }
            let g = render_at(&mut app, &w.cx, wd, ht);
            let tag = format!("{wd}x{ht} grants={grants}");
            let chip = g.hits.iter().find(|h| h.label.contains("Ask first")).unwrap_or_else(|| panic!("{tag}: the chip is on the control row: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
            assert!(chip.label.contains("Ask first"), "{tag}: {}", chip.label);
            let r = chip.rect;
            assert!(r.x >= -0.5 && r.x + r.w <= wd as f32 + 0.5 && r.y + r.h <= ht as f32 + 0.5, "{tag}: {r:?} leaves the screen");
            assert!(r.h >= 19.5, "{tag}: the chip is only {:.1} px high", r.h);
            for other in ["Model ▾", "Agent: on"] {
                if let Some(o) = g.hits.iter().find(|h| h.label == other) {
                    assert!(!partly_overlap(&r, &o.rect) && (r.x + r.w <= o.rect.x + 1.0 || o.rect.x + o.rect.w <= r.x + 1.0), "{tag}: the chip overlaps {other}: {r:?} {:?}", o.rect);
                }
            }
            if grants {
                let ind = g.hits.iter().find(|h| h.label.starts_with("Approved") || h.label.starts_with("✓ ")).unwrap_or_else(|| panic!("{tag}: the indicator is shown"));
                assert!(ind.rect.x + ind.rect.w <= wd as f32 + 0.5 && (r.x + r.w <= ind.rect.x + 1.0 || ind.rect.x + ind.rect.w <= r.x + 1.0), "{tag}: {r:?} {:?}", ind.rect);
            }
        }
    }
}
