//! Tests of the AI IDE's git support with a fake `git` program on the search path (no network, no real repository): the Clone
//! dialog, the batches and their jobs, the cards (new host, pull over local changes, removal), the Repositories panel, the tree
//! badges and the per-repository commands. The real git is used in `iyakki_agent::gitjobs` tests.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use iyakki_agent::gitjobs::GitCheck;
use iyakki_sec::policy::CliLevel;

use super::*;
use crate::apps::ide::MainTab;
use crate::apps::tests::cx_with_backend;
use crate::apps::{App, AppCx};

struct Plain;

impl ChatBackend for Plain {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Ok("ok".into())
    }
    fn complete_tools(&self, _: &[Message], _: &[ToolSpec]) -> Result<ModelTurn, String> {
        Ok(ModelTurn { text: "ok".into(), calls: vec![] })
    }
}

struct World {
    cx: AppCx,
    base: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

fn world(tag: &str) -> World {
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("igit-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("bin")).unwrap();
    let (mut cx, _d) = cx_with_backend(&format!("igit-{tag}"), Plain);
    cx.session.policy.fs_read = vec![base.display().to_string()];
    cx.session.policy.fs_write = vec![base.display().to_string()];
    cx.session.policy.cli = CliLevel::Full;
    cx.session.policy.agent = true;
    // a fake git in a folder that is the only thing on the search path (plus the system's, for `env`, `sh`, `sleep`)
    let log = base.join("calls.log");
    let git = base.join("bin/git");
    std::fs::write(
        &git,
        format!(
            r#"#!/bin/sh
case "$1" in
  --version) echo "git version 9.9.9-fake"; exit 0 ;;
esac
{{ echo "ARGV: $*"; echo "SSH_AUTH_SOCK=$SSH_AUTH_SOCK"; echo "SOCK=$([ -S "$SSH_AUTH_SOCK" ] && echo yes || echo no)"; }} >> '{log}'
case "$*" in *failme*) echo "fatal: repository 'https://x/failme' not found" >&2; exit 128;; esac
case "$1" in
  clone) for a; do last=$a; done; mkdir -p "$last" && echo hi > "$last/README"; echo "Receiving objects: 100% (3/3), done." >&2 ;;
  fetch|pull) echo "Already up to date." ;;
esac
sleep 0.2
exit 0
"#,
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    cx.services.clis.set_search_path(Some(&format!("{}:/usr/bin:/bin", base.join("bin").display())));
    World { cx, base: base.clone(), bin: base.join("bin"), log }
}

fn project(w: &World, name: &str) -> PathBuf {
    let d = w.base.join(name);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
    d
}

fn open(app: &mut IdeApp, w: &World, name: &str, dir: &Path) {
    app.add_and_open(name, dir, &w.cx).unwrap_or_else(|e| panic!("{e}"));
}

fn render(app: &mut IdeApp, cx: &AppCx, wd: u32, ht: u32) -> Gfx {
    let s = crate::shell::scale_for(wd, ht);
    let mut g = Gfx::new(wd, ht);
    g.clear(crate::gfx::rgb(0, 0, 0));
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, wd as f32, ht as f32 - top), s, cx, 1);
    g
}

fn ticks_until(app: &mut IdeApp, cx: &AppCx, what: &str, mut f: impl FnMut(&mut IdeApp) -> bool) {
    for _ in 0..3000 {
        app.handle(&Event::Tick, cx);
        if f(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    panic!("timed out waiting for: {what}");
}

fn paste(app: &mut IdeApp, cx: &AppCx, t: &str) {
    app.handle(&Event::Paste(t.into()), cx);
}

fn press(app: &mut IdeApp, cx: &AppCx, k: Key) {
    app.handle(&Event::Key(k), cx);
}

fn log(w: &World) -> String {
    std::fs::read_to_string(&w.log).unwrap_or_default()
}

fn dialog(app: &IdeApp) -> &CloneDialog {
    match &app.modal {
        Some(Modal::Git(g)) => match &**g {
            GitModal::Clone(d) => d,
            _ => panic!("not the clone dialog"),
        },
        _ => panic!("no git dialog is open"),
    }
}

fn is_panel(app: &IdeApp) -> bool {
    matches!(&app.modal, Some(Modal::Git(g)) if matches!(**g, GitModal::Repos(_)))
}

fn cleanup(w: World) {
    let _ = std::fs::remove_dir_all(&w.base);
}

#[test]
fn the_clone_dialog_makes_a_row_per_line_names_the_problems_and_collects_the_specs() {
    let w = world("dlg");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    app.open_clone_dialog(false, &w.cx);
    paste(&mut app, &w.cx, "https://git.example.org/team/app.git\ngit@github.com:octo/tool.git #dev\nnot an address\nhttps://me:hunter2@host.example/o/r.git\n");
    let dlg = dialog(&app);
    assert_eq!(dlg.rows.len(), 4);
    assert!(dlg.rows[0].parsed.is_ok() && dlg.rows[1].parsed.is_ok() && dlg.rows[2].parsed.is_err() && dlg.rows[3].parsed.is_err());
    assert_eq!(dlg.rows[1].line.branch, "dev");
    assert_eq!(dlg.rows[1].uses, "no key set", "an ssh address with no key chosen is flagged in the table");
    // the problems come back by line, and the password is not echoed
    press(&mut app, &w.cx, Key::Ctrl('s'));
    let e = dialog(&app).error.clone().unwrap();
    assert!(e.contains("line 3") && e.contains("line 4") && !e.contains("hunter2"), "{e}");
    // nothing was started
    assert_eq!(log(&w), "");
    // fix the text: remove the two bad lines; the cells keep what was typed for the lines that stay
    if let Some(Modal::Git(g)) = &mut app.modal {
        if let GitModal::Clone(d) = &mut **g {
            d.area.set("https://git.example.org/team/app.git\ngit@github.com:octo/tool.git #dev\n");
        }
    }
    press(&mut app, &w.cx, Key::Tab);
    assert_eq!(dialog(&app).rows.len(), 2);
    cleanup(w);
}

#[test]
fn tab_walks_through_the_fields_and_depth_branch_and_folder_end_up_in_the_specs() {
    let w = world("tab");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    app.open_clone_dialog(false, &w.cx);
    paste(&mut app, &w.cx, "https://git.example.org/team/app.git\nhttps://git.example.org/team/web.git #main");
    // area -> target -> row 1: branch, depth, folder -> row 2 ...
    press(&mut app, &w.cx, Key::Tab);
    assert_eq!(dialog(&app).focus, super::super::ide_git_ui::Foc::Target);
    press(&mut app, &w.cx, Key::Tab);
    for c in "dev".chars() {
        press(&mut app, &w.cx, Key::Char(c));
    }
    press(&mut app, &w.cx, Key::Tab);
    press(&mut app, &w.cx, Key::Char('1'));
    press(&mut app, &w.cx, Key::Tab);
    for c in "backend".chars() {
        press(&mut app, &w.cx, Key::Char(c));
    }
    let specs = dialog(&app).specs().unwrap();
    assert_eq!((specs[0].branch.as_str(), specs[0].depth, specs[0].folder.as_str()), ("dev", 1, "backend"));
    assert_eq!((specs[1].branch.as_str(), specs[1].depth, specs[1].folder.as_str()), ("main", 0, ""), "the #branch of the line is the default");
    // a bad depth is named
    press(&mut app, &w.cx, Key::BackTab);
    press(&mut app, &w.cx, Key::Char('x'));
    assert!(dialog(&app).specs().unwrap_err()[0].1.contains("not a depth"));
    cleanup(w);
}

#[test]
fn cloning_several_repositories_runs_jobs_two_at_a_time_shows_each_and_lists_them_in_project_json() {
    let w = world("flow");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    app.open_clone_dialog(false, &w.cx);
    paste(&mut app, &w.cx, "https://git.example.org/a/one.git\nhttps://git.example.org/a/failme.git\nhttps://git.example.org/a/three.git\nhttps://other.example.org/b/one.git\n");
    press(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(is_panel(&app), "the Repositories panel opens and shows the progress");
    let id = app.ws[0].meta.id.clone();
    ticks_until(&mut app, &w.cx, "all four finished", |a| a.git.batches_finished(&id));
    let (rows, _) = app.repo_rows();
    let names: Vec<&str> = rows.iter().map(|r| r.folder.as_str()).collect();
    assert_eq!(names, ["one", "three", "one-2", "failme"], "{names:?}: the cloned ones first, then the failed one, folders de-duplicated");
    let bad = rows.iter().find(|r| r.folder == "failme").unwrap();
    assert!(bad.work_failed && bad.last.contains("not found"), "{bad:?}");
    assert!(d.join("one/README").is_file() && d.join("three/README").is_file() && d.join("one-2/README").is_file());
    // project.json lists the three that worked, in the workspace folder
    let cfg = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    assert_eq!(cfg.repos.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["one", "three", "one-2"]);
    assert_eq!(app.ws[0].cfg.repos.len(), 3, "the open workspace sees them at once");
    // the jobs are in the jobs engine under this workspace (the installer indicator and the live log)
    assert!(w.cx.services.jobs.visible("tester", "Alpha").len() >= 4);
    // the panel draws (rows, buttons) and the failed row has a Retry
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Retry") && g.hits.iter().any(|h| h.label == "Fetch all  f"));
    // retry runs it again and fails again, honestly
    let before = log(&w).matches("failme").count();
    press(&mut app, &w.cx, Key::Down);
    press(&mut app, &w.cx, Key::Down);
    press(&mut app, &w.cx, Key::Down);
    press(&mut app, &w.cx, Key::Char('r'));
    ticks_until(&mut app, &w.cx, "the retry", |a| a.git.batches_finished(&id) && log(&w).matches("failme").count() > before);
    cleanup(w);
}

#[test]
fn a_clone_into_a_new_workspace_creates_it_and_the_tree_and_the_assistant_see_the_repositories() {
    let w = world("newws");
    let parent = w.base.join("work");
    std::fs::create_dir_all(&parent).unwrap();
    let mut app = IdeApp::new(&w.cx);
    app.reg.parent = Some(parent.display().to_string());
    app.open_clone_dialog(true, &w.cx);
    assert!(dialog(&app).target_new && !dialog(&app).can_current);
    paste(&mut app, &w.cx, "https://git.example.org/a/one.git\nhttps://git.example.org/a/two.git");
    press(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(dialog(&app).error.as_ref().unwrap().contains("name"), "a new workspace needs a name");
    assert_eq!(dialog(&app).focus, super::super::ide_git_ui::Foc::NewName, "the cursor goes to the missing name");
    for c in "Mono Repo".chars() {
        press(&mut app, &w.cx, Key::Char(c));
    }
    press(&mut app, &w.cx, Key::Ctrl('s'));
    assert_eq!(app.ws.len(), 1, "{:?}", match &app.modal { Some(Modal::Git(g)) => match &**g { GitModal::Clone(d) => d.error.clone(), _ => None }, _ => None });
    assert_eq!(app.ws[0].meta.name, "Mono Repo");
    let root = app.ws[0].root();
    let id = app.ws[0].meta.id.clone();
    ticks_until(&mut app, &w.cx, "both cloned", |a| a.git.batches_finished(&id));
    assert!(root.join("one/README").is_file() && root.join("two/README").is_file());
    // the file tree shows both folders; the assistant's project state lists both repositories
    app.ws[0].tree.refresh();
    let names: Vec<String> = app.ws[0].tree.rows.iter().map(|r| r.name.clone()).collect();
    assert!(names.contains(&"one".to_string()) && names.contains(&"two".to_string()), "{names:?}");
    let inp = iyakki_agent::projectstate::Input { name: "Mono Repo", root: &root, features: &root.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: None, history: None };
    let state = iyakki_agent::projectstate::build(&inp);
    assert!(state.contains("- one/ <- https://git.example.org/a/one.git") && state.contains("- two/ <- https://git.example.org/a/two.git"), "{state}");
    cleanup(w);
}

#[test]
fn a_new_ssh_host_waits_for_the_fingerprint_card_and_nothing_runs_before_the_click() {
    let w = world("host");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    // a key for the host, a probe that presents one key, an ssh program that exists
    w.cx.services.on_login("tester", "a-test-password", false);
    let keys = app.git_keys(&w.cx);
    keys.generate("work", "w@iyakki").unwrap_or_else(|e| panic!("{e}"));
    keys.set_host_key("git.example.org", "work").unwrap();
    let fake_ssh = w.bin.join("git-ssh");
    std::fs::write(&fake_ssh, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("IYAKKI_GIT_SSH", &fake_ssh);
    app.git.probe = Some(std::sync::Arc::new(|h, p| {
        let mut blob = Vec::new();
        iyakki_agent::gitsite::redact_url("x");
        let k = iyakki_agent::sshkey::KeyPair::from_seed([7u8; 32], "").public_blob();
        blob.extend_from_slice(&k);
        Ok(vec![iyakki_agent::ssh::HostKey { pattern: iyakki_agent::ssh::pattern(h, p), kind: "ssh-ed25519".into(), b64: iyakki_agent::story::base64_encode(&blob) }])
    }));
    app.open_clone_dialog(false, &w.cx);
    paste(&mut app, &w.cx, "git@git.example.org:team/app.git");
    assert_eq!(dialog(&app).rows[0].uses, "key work");
    press(&mut app, &w.cx, Key::Ctrl('s'));
    ticks_until(&mut app, &w.cx, "the host card", |a| matches!(&a.modal, Some(Modal::Card(c)) if c.card.title.contains("git.example.org")));
    assert_eq!(log(&w), "", "nothing ran before the person decided");
    let card = match &app.modal {
        Some(Modal::Card(c)) => c.card.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n"),
        _ => unreachable!(),
    };
    assert!(card.contains("SHA256:") && card.contains("ssh-ed25519") && card.contains("Compare"), "{card}");
    // the card draws at the four sizes
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (640, 360)] {
        let g = render(&mut app, &w.cx, wd, ht);
        assert!(g.hits.iter().any(|h| h.label.starts_with("Trust")), "{wd}x{ht}");
    }
    press(&mut app, &w.cx, Key::Char('y'));
    let id = app.ws[0].meta.id.clone();
    ticks_until(&mut app, &w.cx, "the clone", |a| a.git.batches_finished(&id));
    assert!(log(&w).contains("SOCK=yes"), "git saw the key service socket while it ran: {}", log(&w));
    assert!(d.join("app/README").is_file());
    assert!(std::fs::read_to_string(app.git_keys(&w.cx).known_hosts_path()).unwrap().starts_with("git.example.org ssh-ed25519 "));
    std::env::remove_var("IYAKKI_GIT_SSH");
    cleanup(w);
}

#[test]
fn removing_a_repository_asks_what_would_be_lost_then_the_typed_name_and_stays_in_the_workspace() {
    let w = world("rm");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::write(d.join("lib/x"), "x").unwrap();
    let mut c = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    c.repos.push(iyakki_agent::gitsite::RepoEntry { url: "https://git.example.org/a/lib.git".into(), path: "lib".into(), state: "cloned".into(), ..Default::default() });
    c.save(&d).unwrap();
    app.ws[0].cfg.repos = c.repos.clone();
    app.open_repos_panel(&w.cx);
    assert!(is_panel(&app));
    press(&mut app, &w.cx, Key::Char('x'));
    assert!(matches!(&app.modal, Some(Modal::Card(c)) if c.card.title.contains("lib/")), "a card says what would be lost first");
    press(&mut app, &w.cx, Key::Char('c'));
    assert!(matches!(&app.modal, Some(Modal::Form(FormKind::Git(GitForm::RemoveType(f)), _)) if f == "lib"));
    for ch in "li".chars() {
        press(&mut app, &w.cx, Key::Char(ch));
    }
    press(&mut app, &w.cx, Key::Enter);
    assert!(d.join("lib").exists(), "a wrong name removes nothing");
    assert!(matches!(&app.modal, Some(Modal::Form(_, f)) if f.error.as_ref().is_some_and(|e| e.contains("exactly"))));
    press(&mut app, &w.cx, Key::Char('b'));
    press(&mut app, &w.cx, Key::Enter);
    assert!(!d.join("lib").exists() && app.ws[0].cfg.repos.is_empty());
    assert!(iyakki_agent::workspace::ProjectConfig::load(&d).unwrap().repos.is_empty());
    assert!(d.join("src/main.rs").exists(), "the rest of the workspace is untouched");
    cleanup(w);
}

#[test]
fn pull_all_asks_before_touching_repositories_with_local_changes_and_fetch_all_runs_everywhere() {
    let w = world("pull");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    let mut c = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    for n in ["clean", "dirty"] {
        std::fs::create_dir_all(d.join(n)).unwrap();
        c.repos.push(iyakki_agent::gitsite::RepoEntry { url: format!("https://git.example.org/a/{n}.git"), path: n.into(), state: "cloned".into(), ..Default::default() });
    }
    c.save(&d).unwrap();
    app.ws[0].cfg.repos = c.repos.clone();
    let id = app.ws[0].meta.id.clone();
    // the state the background read would have found
    app.git.set_status_for_test(&id, vec![("clean".into(), Ok(RepoStatus { branch: "main".into(), behind: 1, ..Default::default() })), ("dirty".into(), Ok(RepoStatus { branch: "main".into(), changed: 2, behind: 1, ..Default::default() }))]);
    app.open_repos_panel(&w.cx);
    // fetch all: both, no question
    press(&mut app, &w.cx, Key::Char('f'));
    ticks_until(&mut app, &w.cx, "fetch done", |a| a.git.batches_finished(&id));
    assert_eq!(log(&w).matches("ARGV: fetch --all --prune").count(), 2);
    // pull all: a card names the dirty one first
    press(&mut app, &w.cx, Key::Char('p'));
    let text = match &app.modal {
        Some(Modal::Card(c)) => c.card.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n"),
        _ => panic!("expected the card"),
    };
    assert!(text.contains("dirty/") && !text.contains("clean/") && text.contains("fast-forward"), "{text}");
    assert_eq!(log(&w).matches("ARGV: pull").count(), 0, "nothing was pulled before the answer");
    press(&mut app, &w.cx, Key::Char('c'));
    ticks_until(&mut app, &w.cx, "pull done", |a| a.git.batches_finished(&id));
    assert_eq!(log(&w).matches("ARGV: pull --ff-only --no-rebase").count(), 1, "only the clean one: {}", log(&w));
    // the tree shows the state next to the folders
    app.git.set_status_for_test(&id, vec![("dirty".into(), Ok(RepoStatus { branch: "main".into(), changed: 2, behind: 1, ..Default::default() }))]);
    app.ws[0].tree.badges.insert("dirty".into(), "main ↓1 ✎2".into());
    app.modal = None;
    app.ws[0].tree.refresh();
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "dirty"), "the folder row is there");
    cleanup(w);
}

#[test]
fn commands_set_for_a_repository_are_used_for_files_inside_it() {
    let w = world("cmd");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    std::fs::create_dir_all(d.join("lib/src")).unwrap();
    std::fs::write(d.join("lib/src/a.rs"), "fn a() {}\n").unwrap();
    let mut c = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    c.repos.push(iyakki_agent::gitsite::RepoEntry { url: "https://git.example.org/a/lib.git".into(), path: "lib".into(), state: "cloned".into(), ..Default::default() });
    c.save(&d).unwrap();
    app.ws[0].cfg.repos = c.repos.clone();
    app.open_repos_panel(&w.cx);
    press(&mut app, &w.cx, Key::Char('e'));
    assert!(matches!(&app.modal, Some(Modal::Form(FormKind::Git(GitForm::Commands(f)), _)) if f == "lib"));
    for ch in "echo lib-tests".chars() {
        press(&mut app, &w.cx, Key::Char(ch));
    }
    press(&mut app, &w.cx, Key::Enter);
    for ch in "echo lib-running".chars() {
        press(&mut app, &w.cx, Key::Char(ch));
    }
    press(&mut app, &w.cx, Key::Enter);
    assert_eq!(app.ws[0].cfg.repos[0].test, "echo lib-tests");
    assert_eq!(iyakki_agent::workspace::ProjectConfig::load(&d).unwrap().repos[0].run, "echo lib-running");
    // F5 on a file inside the repository runs its run command inside its folder
    app.modal = None;
    app.open_file("lib/src/a.rs", None, &w.cx);
    app.run_current(&w.cx);
    assert_eq!(app.ws[0].term.cwd, "lib");
    ticks_until(&mut app, &w.cx, "the run command", |a| a.ws[0].term.log.lines.iter().any(|l| l.0.contains("lib-running")));
    // the test run uses the repository's test command and folder
    app.run_tests(&w.cx);
    assert_eq!(app.ws[0].problems.command, "echo lib-tests");
    assert_eq!(app.ws[0].problems.root, std::fs::canonicalize(&d).unwrap_or(d.clone()).join("lib").canonicalize().unwrap_or(d.join("lib")));
    // a file outside any repository uses the workspace's commands
    app.open_file("src/main.rs", None, &w.cx);
    app.run_tests(&w.cx);
    assert_ne!(app.ws[0].problems.command, "echo lib-tests");
    cleanup(w);
}

#[test]
fn without_git_the_dialog_and_the_panel_say_so_plainly_and_start_nothing() {
    let w = world("nogit");
    let d = project(&w, "alpha");
    w.cx.services.clis.set_search_path(Some("/nonexistent-folder-for-iyakki-tests"));
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    assert!(matches!(app.git_check(&w.cx), GitCheck::Missing { .. }));
    app.open_clone_dialog(false, &w.cx);
    assert!(dialog(&app).git_missing.as_ref().is_some_and(|m| m.contains("git is not installed")));
    paste(&mut app, &w.cx, "https://git.example.org/a/one.git");
    press(&mut app, &w.cx, Key::Ctrl('s'));
    assert!(app.git.batches_empty());
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Close"));
    app.modal = None;
    app.open_repos_panel(&w.cx);
    let g = render(&mut app, &w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Close"));
    cleanup(w);
}

#[test]
fn every_git_screen_draws_inside_the_window_at_the_four_sizes() {
    let w = world("sizes");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    let mut c = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    for i in 0..9 {
        std::fs::create_dir_all(d.join(format!("r{i}"))).unwrap();
        c.repos.push(iyakki_agent::gitsite::RepoEntry { url: format!("https://git.example.org/team/repository-with-a-long-name-{i}.git"), path: format!("r{i}"), branch: "main".into(), state: "cloned".into(), ..Default::default() });
    }
    c.save(&d).unwrap();
    app.ws[0].cfg.repos = c.repos.clone();
    let id = app.ws[0].meta.id.clone();
    app.git.set_status_for_test(&id, (0..9).map(|i| (format!("r{i}"), Ok(RepoStatus { branch: "feature/a-rather-long-branch-name".into(), ahead: 1, behind: 12, changed: 3, untracked: 1, ..Default::default() }))).collect());
    let many: String = (0..14).map(|i| format!("https://git.example.org/team/app-{i}.git #release/{i}\n")).collect();
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (640, 360)] {
        for which in 0..3 {
            match which {
                0 => {
                    app.open_clone_dialog(false, &w.cx);
                    paste(&mut app, &w.cx, &many);
                    press(&mut app, &w.cx, Key::Tab);
                    press(&mut app, &w.cx, Key::Tab);
                    for ch in "a-folder-name".chars() {
                        press(&mut app, &w.cx, Key::Char(ch));
                    }
                }
                1 => app.open_repos_panel(&w.cx),
                _ => {
                    app.open_clone_dialog(true, &w.cx);
                    paste(&mut app, &w.cx, "git@github.com:octo/tool.git\nnot valid");
                }
            }
            let g = render(&mut app, &w.cx, wd, ht);
            let tag = format!("screen {which} at {wd}x{ht}");
            assert!(!g.hits.is_empty(), "{tag}");
            for h in g.hits.iter().filter(|h| h.label != "backdrop" && h.rect.w > 0.0) {
                let r = h.rect;
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= wd as f32 + 0.5 && r.y + r.h <= ht as f32 + 0.5, "{tag}: {:?} {:?} leaves the window", h.label, r);
            }
            // the buttons are big enough to hit and the dialog's own buttons are all there
            let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
            if which == 0 || which == 2 {
                assert!(labels.contains(&"Clone") && labels.contains(&"Cancel"), "{tag}: {labels:?}");
            } else {
                assert!(labels.contains(&"Close") && labels.contains(&"Fetch all  f"), "{tag}: {labels:?}");
            }
            for h in g.hits.iter().filter(|h| matches!(h.label.as_str(), "Clone" | "Cancel" | "Close" | "Remove" | "Commands" | "Retry")) {
                assert!(h.rect.h >= 19.5 && h.rect.w >= 19.5, "{tag}: {:?} is {}x{}", h.label, h.rect.w, h.rect.h);
            }
            // buttons of one dialog never overlap each other
            let btn: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Ctrl('s')) | Event::Key(Key::Esc)) && h.label != "backdrop").collect();
            for (i, a) in btn.iter().enumerate() {
                for b in &btn[i + 1..] {
                    let ox = (a.rect.x + a.rect.w).min(b.rect.x + b.rect.w) - a.rect.x.max(b.rect.x);
                    let oy = (a.rect.y + a.rect.h).min(b.rect.y + b.rect.h) - a.rect.y.max(b.rect.y);
                    assert!(!(ox > 1.0 && oy > 1.0), "{tag}: {:?} overlaps {:?}", a.label, b.label);
                }
            }
            app.modal = None;
        }
    }
    let _ = MainTab::Editor;
    cleanup(w);
}

#[test]
fn a_repository_of_the_platform_gitea_is_cloned_by_its_instance_name_with_iyakkis_token() {
    use iyakki_agent::platform::installed::{self, Installed};
    let w = world("gitea");
    let d = project(&w, "alpha");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Alpha", &d);
    w.cx.services.on_login("tester", "a-test-password", false);
    w.cx.services.user_set("tester", "svc.gitea-iyakki-gitea.token", "tok_abcdefghijklmnopqrstuv").unwrap();
    let rec = Installed {
        id: "gitea-iyakki-gitea".into(),
        service: "gitea".into(),
        urls: vec![iyakki_agent::platform::UrlInfo { label: "Gitea".into(), url: "http://gitea.lan.example:30300".into(), kind: "web".into() }],
        vault: vec!["svc.gitea-iyakki-gitea.token".into()],
        ..Default::default()
    };
    installed::save(w.cx.services.dir(), "tester", &[rec]).unwrap();
    app.open_clone_dialog(false, &w.cx);
    paste(&mut app, &w.cx, "@gitea-iyakki-gitea/team/app\n@nope/team/app\n");
    let dlg = dialog(&app);
    assert!(dlg.rows[0].parsed.is_ok() && dlg.rows[0].uses == "token of gitea-iyakki-gitea", "{:?}", dlg.rows[0].uses);
    assert!(dlg.rows[1].parsed.as_ref().unwrap_err().contains("gitea-iyakki-gitea"), "the error names the services that exist");
    // the bad line is reported, the good one waits
    press(&mut app, &w.cx, Key::Ctrl('s'));
    assert_eq!(log(&w), "");
    if let Some(Modal::Git(g)) = &mut app.modal {
        if let GitModal::Clone(d) = &mut **g {
            d.area.set("@gitea-iyakki-gitea/team/app");
        }
    }
    press(&mut app, &w.cx, Key::Tab);
    press(&mut app, &w.cx, Key::Ctrl('s'));
    let id = app.ws[0].meta.id.clone();
    ticks_until(&mut app, &w.cx, "the clone", |a| a.git.batches_finished(&id));
    assert!(log(&w).contains("clone --progress -- http://gitea.lan.example:30300/team/app.git app"), "{}", log(&w));
    assert!(app.git_keys(&w.cx).table().hosts["gitea.lan.example"].token, "IYAKKI's token for the service is the host's token now");
    let cfg = iyakki_agent::workspace::ProjectConfig::load(&d).unwrap();
    assert_eq!((cfg.repos[0].url.as_str(), cfg.repos[0].key.as_str()), ("http://gitea.lan.example:30300/team/app.git", "token"));
    cleanup(w);
}

// ------------------------------------------------------------------------------------------------ Connect Git / Gitea

use iyakki_agent::gitconn::{Connection, FakeGitHttp, Kind};
use iyakki_agent::wsexec::{Out, Runner as WsRunner};
use std::sync::{Arc as StdArc, Mutex};

/// A pretend `git` for the project's own repository: it remembers `init`, commits and `origin`, and lists every call.
struct ProjGit {
    root: String,
    st: Mutex<(bool, Option<String>, usize, usize)>,
    calls: Mutex<Vec<String>>,
}

impl ProjGit {
    fn new(root: &Path, repo: bool, origin: Option<&str>, commits: usize, changed: usize) -> StdArc<ProjGit> {
        StdArc::new(ProjGit { root: std::fs::canonicalize(root).unwrap().display().to_string(), st: Mutex::new((repo, origin.map(String::from), commits, changed)), calls: Mutex::new(vec![]) })
    }
    fn lines(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl WsRunner for ProjGit {
    fn run(&self, _cwd: &Path, argv: &[String], _env: &[(String, String)], _t: Duration, _cap: usize, _s: Option<iyakki_agent::wsexec::Sink>) -> Out {
        let line = argv.iter().skip(3).cloned().collect::<Vec<_>>().join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let mut st = self.st.lock().unwrap();
        let ok = |s: &str| Out { code: Some(0), stdout: s.into(), ..Default::default() };
        let no = |c: i32| Out { code: Some(c), stderr: "fatal".into(), ..Default::default() };
        match line.as_str() {
            "rev-parse --show-toplevel" if st.0 => ok(&format!("{}\n", self.root)),
            "symbolic-ref --short HEAD" if st.0 => ok("main\n"),
            "remote get-url origin" => match &st.1 {
                Some(o) if st.0 => ok(&format!("{o}\n")),
                _ => no(2),
            },
            "rev-list --count HEAD" if st.0 => ok(&format!("{}\n", st.2)),
            "status --porcelain" if st.0 => ok(&" M f\n".repeat(st.3)),
            "init -q" => {
                st.0 = true;
                ok("")
            }
            "add -A" => ok(""),
            l if l.starts_with("commit -q") => {
                st.2 = 1;
                st.3 = 0;
                ok("")
            }
            l if l.starts_with("remote add origin ") || l.starts_with("remote set-url origin ") => {
                st.1 = Some(l.rsplit(' ').next().unwrap().to_string());
                ok("")
            }
            "remote remove origin" => {
                st.1 = None;
                ok("")
            }
            l if l.starts_with("config --local --get") => no(1),
            _ => ok(""),
        }
    }
    fn has(&self, _: &str) -> bool {
        true
    }
}

const TOK: &str = "tok_FAKEFAKEFAKEFAKE1234";

struct Cw {
    w: World,
    app: IdeApp,
    git: StdArc<ProjGit>,
    http: StdArc<FakeGitHttp>,
    dir: PathBuf,
}

fn cworld(tag: &str, repo: bool, origin: Option<&str>, commits: usize, changed: usize) -> Cw {
    let w = world(tag);
    let dir = project(&w, "demo");
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w, "Demo App", &dir);
    w.cx.services.on_login("tester", "a-test-password", false);
    let git = ProjGit::new(&dir, repo, origin, commits, changed);
    let http = StdArc::new(FakeGitHttp::default());
    app.git.runner = Some(git.clone());
    app.git.http = Some(http.clone());
    Cw { w, app, git, http, dir }
}

impl Cw {
    fn add_conn(&self, kind: Kind, name: &str, url: &str) -> Connection {
        self.app.git_keys(&self.w.cx).conns().add(Connection { name: name.into(), kind, url: url.into(), account: "octo".into(), ..Default::default() }, Some(TOK)).unwrap()
    }
    fn gitea_api(&self) {
        self.http.answer("POST https://git.example.org/api/v1/user/repos", 201, &[], r#"{"full_name":"octo/demo-app","clone_url":"https://git.example.org/octo/demo-app.git","ssh_url":"git@git.example.org:octo/demo-app.git","html_url":"https://git.example.org/octo/demo-app","private":true}"#);
        self.http.answer("GET https://git.example.org/api/v1/user/repos?limit=50", 200, &[], r#"[{"full_name":"octo/api","clone_url":"https://git.example.org/octo/api.git","ssh_url":"git@git.example.org:octo/api.git","html_url":"x","private":true},{"full_name":"octo/web","clone_url":"https://git.example.org/octo/web.git","ssh_url":"git@git.example.org:octo/web.git","html_url":"x","private":false}]"#);
    }
    fn flow(&self) -> &ConnectFlow {
        match &self.app.modal {
            Some(Modal::Git(g)) => match &**g {
                GitModal::Connect(f) => f,
                _ => panic!("not the connect flow"),
            },
            _ => panic!("no git modal"),
        }
    }
    fn wait_head(&mut self) {
        ticks_until(&mut self.app, &self.w.cx, "the project's git was read", |a| a.git_head().is_some());
    }
    fn wait_stage(&mut self, st: super::super::ide_git_connect::Stage) {
        ticks_until(&mut self.app, &self.w.cx, "the stage", |a| matches!(&a.modal, Some(Modal::Git(g)) if matches!(&**g, GitModal::Connect(f) if f.stage == st)));
    }
    fn audit(&self) -> String {
        std::fs::read_to_string(self.w.cx.audit.path()).unwrap_or_default()
    }
}

use super::super::ide_git_connect::Stage;

#[test]
fn a_project_without_a_remote_shows_the_prompt_in_the_top_bar_the_menu_and_the_panel_and_a_connected_one_shows_who() {
    let mut c = cworld("prompt", true, None, 2, 0);
    c.wait_head();
    assert!(c.app.git_head().unwrap().origin.is_none());
    let g = render(&mut c.app, &c.w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Connect Git / Gitea"), "the top bar offers it when the project has no remote");
    assert!(c.app.top_overflow.iter().any(|(l, n, _)| l == "Connect Git / Gitea…" && *n == 21), "and the project menu (the top bar's ⋯)");
    // the panel says it and has the button
    c.app.open_repos_panel(&c.w.cx);
    let g = render(&mut c.app, &c.w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Connect to Git or Gitea  o"), "the panel's own prompt");
    // the button and the key open the flow
    press(&mut c.app, &c.w.cx, Key::Char('o'));
    assert_eq!(c.flow().stage, Stage::Pick);
    press(&mut c.app, &c.w.cx, Key::Esc);
    assert!(c.app.modal.is_none());
    c.app.handle(&Event::Tab(crate::apps::ide::T_TOP + 21), &c.w.cx);
    assert!(matches!(&c.app.modal, Some(Modal::Git(g)) if matches!(**g, GitModal::Connect(_))));
    // a connected project: the header names the connection, the remote and the buttons
    let mut d = cworld("connected", true, Some("https://git.example.org/octo/demo-app.git"), 2, 0);
    d.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    d.wait_head();
    let h = d.app.git_head().unwrap();
    assert_eq!((h.conn.as_deref(), h.host.as_str(), h.branch.as_str()), (Some("Home Gitea"), "git.example.org", "main"));
    assert!(h.line().contains("Connected to Home Gitea (git.example.org)") && h.line().contains("https://git.example.org/octo/demo-app.git"));
    d.app.open_repos_panel(&d.w.cx);
    let g = render(&mut d.app, &d.w.cx, 1280, 720);
    for l in ["Fetch  f", "Pull  p", "Push…  u", "Disconnect  d"] {
        assert!(g.hits.iter().any(|h| h.label == l), "{l}");
    }
    let g = {
        d.app.modal = None;
        render(&mut d.app, &d.w.cx, 1280, 720)
    };
    assert!(g.hits.iter().any(|h| h.label.starts_with("Git: ")), "the top bar chip leads to the panel");
    cleanup(c.w);
    cleanup(d.w);
}

#[test]
fn creating_a_repository_is_reviewed_first_then_made_with_origin_and_the_first_commit_and_pushed_after_its_own_review() {
    let mut c = cworld("create", false, None, 0, 3);
    let conn = c.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    c.gitea_api();
    c.wait_head();
    c.app.open_connect_flow(None, &c.w.cx);
    assert_eq!(c.flow().stage, Stage::Pick);
    assert_eq!(c.flow().conns.len(), 1);
    press(&mut c.app, &c.w.cx, Key::Enter);
    assert_eq!(c.flow().stage, Stage::Choose);
    press(&mut c.app, &c.w.cx, Key::Char('1'));
    assert_eq!(c.flow().stage, Stage::CreateReview);
    // the review: name from the project lowercased, private, every step in order; nothing was asked or changed yet
    assert_eq!((c.flow().repo_name().as_str(), c.flow().private, c.flow().commit), ("demo-app", true, true));
    let review = c.flow().create_review().join("\n");
    assert!(review.contains("Create the repository demo-app (PRIVATE): POST https://git.example.org/api/v1/user/repos") && review.contains("git init") && review.contains("Make the first commit of the project's files") && review.contains("Set origin") && review.contains("Nothing is pushed"), "{review}");
    assert!(!review.contains("Warning"), "no secret-looking file in the project: {review}");
    // a secret-looking file that .gitignore does not name is warned about before the first commit
    std::fs::write(c.dir.join(".env"), "A=1\n").unwrap();
    std::fs::write(c.dir.join("server.key"), "x").unwrap();
    std::fs::write(c.dir.join(".gitignore"), "server.key\n").unwrap();
    assert!(c.flow().create_review().join("\n").contains("Warning: .env look like secrets"), "{:?}", c.flow().create_review());
    std::fs::remove_file(c.dir.join(".env")).unwrap();
    assert!(c.http.requests().is_empty());
    assert!(!c.git.lines().iter().any(|l| l.starts_with("init") || l.starts_with("remote add") || l.starts_with("commit")), "{:?}", c.git.lines());
    let g = render(&mut c.app, &c.w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Create repository") && g.hits.iter().any(|h| h.label == "Back"));
    // Back and forward keep the name; then go
    press(&mut c.app, &c.w.cx, Key::PageUp);
    assert_eq!(c.flow().stage, Stage::Choose);
    press(&mut c.app, &c.w.cx, Key::Char('1'));
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    c.wait_stage(Stage::Connected);
    // the request: private, the name, the token in the header field (not in the body)
    let reqs = c.http.requests();
    assert_eq!((reqs.len(), reqs[0].method.as_str(), reqs[0].url.as_str(), reqs[0].token.as_deref()), (1, "POST", "https://git.example.org/api/v1/user/repos", Some(TOK)));
    let body = reqs[0].body.clone().unwrap();
    assert!(body.contains(r#""name":"demo-app""#) && body.contains(r#""private":true"#) && !body.contains("FAKEFAKE"), "{body}");
    // git: init, the first commit, then origin without a credential in it
    let lines = c.git.lines();
    let pos = |p: &str| lines.iter().position(|l| l.starts_with(p)).unwrap_or_else(|| panic!("{p} not in {lines:?}"));
    assert!(pos("init") < pos("add -A") && pos("add -A") < pos("commit -q") && pos("commit -q") < pos("remote add origin"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "remote add origin https://git.example.org/octo/demo-app.git"));
    assert!(c.flow().message.as_deref().unwrap().contains("Created octo/demo-app") && c.flow().state.origin.is_some() && c.flow().state.commits == 1);
    let a = c.audit();
    assert!(a.contains("git.repo.create") && a.contains("git.example.org octo/demo-app") && !a.contains("FAKEFAKE"), "{a}");
    // nothing was pushed by creating
    assert_eq!(log(&c.w), "");
    // the push has its own review: the exact remote and branch
    press(&mut c.app, &c.w.cx, Key::Char('p'));
    assert_eq!(c.flow().stage, Stage::PushReview);
    let pr = c.flow().push_review().join("\n");
    assert!(pr.contains("Remote: https://git.example.org/octo/demo-app.git") && pr.contains("Branch: main (1 commit(s))") && pr.contains("git push --set-upstream origin main") && pr.contains("token of the connection Home Gitea") && pr.contains("on the internet"), "{pr}");
    assert_eq!(log(&c.w), "", "still nothing sent");
    press(&mut c.app, &c.w.cx, Key::Enter);
    assert!(is_panel(&c.app), "the Repositories panel shows the push");
    let id = c.app.ws[0].meta.id.clone();
    ticks_until(&mut c.app, &c.w.cx, "the push job", |a| a.git.batches_finished(&id) && log(&c.w).contains("push"));
    let l = log(&c.w);
    assert!(l.contains("ARGV: push --set-upstream --progress origin main"), "{l}");
    assert!(!l.contains("FAKEFAKE"));
    let a = c.audit();
    assert!(a.contains("git.push") && a.contains("git.example.org octo/demo-app"), "{a}");
    // the token reached git only through the key service: the connection's own entry, not copied into the host table
    assert!(c.app.git_keys(&c.w.cx).table().hosts.is_empty());
    assert!(c.app.git_keys(&c.w.cx).conns().token_present(&conn));
    // a row for the push is shown
    let (rows, _) = c.app.repo_rows();
    assert!(rows.iter().any(|r| r.folder == "." && r.work == "pushed"), "{rows:?}");
    cleanup(c.w);
}

#[test]
fn create_errors_are_plain_words_and_leave_the_project_alone() {
    // the name is taken on the host: nothing local changes (not even git init)
    let mut c = cworld("taken", false, None, 0, 2);
    c.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    c.http.answer("POST https://git.example.org/api/v1/user/repos", 409, &[], r#"{"message":"repository already exists"}"#);
    c.app.open_connect_flow(None, &c.w.cx);
    press(&mut c.app, &c.w.cx, Key::Enter);
    press(&mut c.app, &c.w.cx, Key::Char('1'));
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    c.wait_stage(Stage::CreateReview);
    assert!(c.flow().error.as_deref().unwrap().contains("A repository called demo-app already exists"), "{:?}", c.flow().error);
    assert!(!c.git.lines().iter().any(|l| l.starts_with("init") || l.starts_with("commit") || l.starts_with("remote add")), "{:?}", c.git.lines());
    // a token without write rights
    let mut d = cworld("forbidden", true, None, 1, 0);
    d.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    d.http.answer("POST", 403, &[], "{}");
    d.app.open_connect_flow(None, &d.w.cx);
    press(&mut d.app, &d.w.cx, Key::Enter);
    press(&mut d.app, &d.w.cx, Key::Char('1'));
    press(&mut d.app, &d.w.cx, Key::Ctrl('s'));
    d.wait_stage(Stage::CreateReview);
    assert!(d.flow().error.as_deref().unwrap().contains("The token cannot write repositories"));
    // a bad name is refused before anything is sent
    d.app.modal.as_mut().map(|m| if let Modal::Git(g) = m { if let GitModal::Connect(f) = &mut **g { f.set_name("bad name!"); } });
    let n = d.http.requests().len();
    press(&mut d.app, &d.w.cx, Key::Ctrl('s'));
    assert!(d.flow().error.as_deref().unwrap().contains("not a repository name"));
    assert_eq!(d.http.requests().len(), n);
    // without a token nothing can be made
    let mut e = cworld("notoken", true, None, 1, 0);
    e.app.git_keys(&e.w.cx).conns().add(Connection { name: "Key only".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), method: iyakki_agent::gitconn::Method::Ssh, key: "k1".into(), ..Default::default() }, None).unwrap();
    e.app.open_connect_flow(None, &e.w.cx);
    press(&mut e.app, &e.w.cx, Key::Enter);
    press(&mut e.app, &e.w.cx, Key::Char('1'));
    assert_eq!(e.flow().stage, Stage::Choose);
    assert!(e.flow().error.as_deref().unwrap().contains("needs a token"));
    cleanup(c.w);
    cleanup(d.w);
    cleanup(e.w);
}

#[test]
fn connecting_to_an_existing_repository_lists_the_hosts_repositories_and_sets_origin_without_pushing() {
    let mut c = cworld("existing", true, None, 2, 0);
    c.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    c.gitea_api();
    c.app.open_connect_flow(None, &c.w.cx);
    press(&mut c.app, &c.w.cx, Key::Enter);
    press(&mut c.app, &c.w.cx, Key::Char('2'));
    assert_eq!(c.flow().stage, Stage::Existing);
    ticks_until(&mut c.app, &c.w.cx, "the list", |a| matches!(&a.modal, Some(Modal::Git(g)) if matches!(&**g, GitModal::Connect(f) if !f.repos.is_empty())));
    assert_eq!(c.flow().repos.iter().map(|r| r.full_name.as_str()).collect::<Vec<_>>(), ["octo/api", "octo/web"]);
    assert_eq!(c.http.lines(), ["GET https://git.example.org/api/v1/user/repos?limit=50"]);
    // typing filters; the first Enter shows what will happen, the second does it
    for ch in "web".chars() {
        press(&mut c.app, &c.w.cx, Key::Char(ch));
    }
    press(&mut c.app, &c.w.cx, Key::Enter);
    assert_eq!(c.flow().confirm.as_ref().map(|x| x.1.as_str()), Some("octo/web"));
    assert!(!c.git.lines().iter().any(|l| l.starts_with("remote add")), "the first Enter changes nothing");
    let g = render(&mut c.app, &c.w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Connect"));
    press(&mut c.app, &c.w.cx, Key::Enter);
    assert_eq!(c.flow().stage, Stage::Connected);
    assert!(c.git.lines().iter().any(|l| l == "remote add origin https://git.example.org/octo/web.git"), "{:?}", c.git.lines());
    assert!(c.flow().message.as_deref().unwrap().contains("Nothing was pushed or fetched"));
    assert_eq!(log(&c.w), "", "no push, no fetch");
    assert!(c.audit().contains("git.origin.set") && c.audit().contains("git.example.org octo/web"));
    // a typed address works too, and a credential in it is refused without echoing it
    let mut d = cworld("typed", true, None, 2, 0);
    d.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    d.app.open_connect_flow(None, &d.w.cx);
    press(&mut d.app, &d.w.cx, Key::Enter);
    press(&mut d.app, &d.w.cx, Key::Char('2'));
    paste(&mut d.app, &d.w.cx, "https://me:hunter2@git.example.org/octo/x.git");
    press(&mut d.app, &d.w.cx, Key::Enter);
    let e = d.flow().error.clone().unwrap();
    assert!(!e.contains("hunter2") && d.flow().confirm.is_none(), "{e}");
    d.app.modal.as_mut().map(|m| if let Modal::Git(g) = m { if let GitModal::Connect(f) = &mut **g { f.set_filter("octo/api"); } });
    press(&mut d.app, &d.w.cx, Key::Enter);
    press(&mut d.app, &d.w.cx, Key::Enter);
    assert!(d.git.lines().iter().any(|l| l == "remote add origin https://git.example.org/octo/api.git"), "{:?}", d.git.lines());
    cleanup(c.w);
    cleanup(d.w);
}

#[test]
fn add_connection_opens_the_wizard_in_place_and_comes_back_to_the_flow_with_the_new_connection() {
    let mut c = cworld("inplace", true, None, 1, 0);
    c.http.answer("GET https://git.example.org/api/v1/user/repos", 200, &[], "[]").answer("GET https://git.example.org/api/v1/user", 200, &[], r#"{"login":"octo","name":"Octo","email":"octo@example.org"}"#);
    c.app.open_connect_flow(None, &c.w.cx);
    assert_eq!(c.flow().conns.len(), 0);
    press(&mut c.app, &c.w.cx, Key::Enter); // the only row: "Add connection…"
    let wiz = |a: &IdeApp| matches!(&a.modal, Some(Modal::Git(g)) if matches!(&**g, GitModal::Wizard(_)));
    assert!(wiz(&c.app));
    // cancel: back to the flow, nothing stored
    press(&mut c.app, &c.w.cx, Key::Esc);
    assert_eq!(c.flow().stage, Stage::Pick);
    assert!(c.app.git_keys(&c.w.cx).conns().list().is_empty());
    press(&mut c.app, &c.w.cx, Key::Enter);
    press(&mut c.app, &c.w.cx, Key::Enter); // kind: Gitea
    if let Some(Modal::Git(g)) = &mut c.app.modal {
        if let GitModal::Wizard(w) = &mut **g {
            w.mode = 1;
            w.set_text("name", "Home Gitea");
            w.set_text("url", "https://git.example.org");
        }
    }
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    press(&mut c.app, &c.w.cx, Key::Down);
    paste(&mut c.app, &c.w.cx, TOK);
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    ticks_until(&mut c.app, &c.w.cx, "the test", |a| matches!(&a.modal, Some(Modal::Git(g)) if matches!(&**g, GitModal::Wizard(w) if !w.busy() && w.report.is_some())));
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    assert!(!wiz(&c.app), "the wizard closed");
    assert_eq!((c.flow().stage, c.flow().chosen.as_ref().map(|x| x.name.as_str())), (Stage::Choose, Some("Home Gitea")), "back in the flow, on the new connection");
    let stored = c.app.git_keys(&c.w.cx).conns().list();
    assert_eq!(stored.len(), 1);
    assert!(stored[0].default);
    // the wizard draws inside the IDE at four sizes
    c.app.modal = None;
    for (w, h) in [(1280, 720), (1920, 1080), (1024, 600), (640, 360)] {
        c.app.open_connect_flow(None, &c.w.cx);
        render(&mut c.app, &c.w.cx, w, h);
        press(&mut c.app, &c.w.cx, Key::Enter);
        render(&mut c.app, &c.w.cx, w, h);
        press(&mut c.app, &c.w.cx, Key::Char('1'));
        render(&mut c.app, &c.w.cx, w, h);
        press(&mut c.app, &c.w.cx, Key::Esc);
    }
    cleanup(c.w);
}

#[test]
fn disconnecting_is_its_own_confirmed_action_and_removes_only_origin() {
    let mut c = cworld("disc", true, Some("https://git.example.org/octo/demo-app.git"), 2, 0);
    c.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    c.wait_head();
    c.app.open_repos_panel(&c.w.cx);
    press(&mut c.app, &c.w.cx, Key::Char('d'));
    assert!(matches!(&c.app.modal, Some(Modal::Card(_))), "a card asks first");
    assert!(!c.git.lines().iter().any(|l| l.starts_with("remote remove")));
    // Esc keeps it
    press(&mut c.app, &c.w.cx, Key::Esc);
    assert!(!c.git.lines().iter().any(|l| l.starts_with("remote remove")));
    assert!(is_panel(&c.app));
    press(&mut c.app, &c.w.cx, Key::Char('d'));
    press(&mut c.app, &c.w.cx, Key::Char('y'));
    assert_eq!(c.git.lines().iter().filter(|l| l.starts_with("remote remove origin")).count(), 1);
    assert!(c.git.lines().iter().all(|l| !l.starts_with("rm") && !l.contains("push")));
    ticks_until(&mut c.app, &c.w.cx, "the header is read again", |a| a.git_head().is_some_and(|h| h.origin.is_none()));
    assert!(c.audit().contains("git.disconnect") && c.audit().contains("git.example.org octo/demo-app"));
    assert!(c.app.git.note.contains("Disconnected"));
    assert!(!c.app.git_keys(&c.w.cx).conns().list().is_empty(), "the connection stays");
    // the project is not connected again: the prompt is back
    let g = render(&mut c.app, &c.w.cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Connect to Git or Gitea  o"));
    cleanup(c.w);
}

#[test]
fn fetch_and_pull_use_the_projects_own_origin_and_the_push_review_says_what_is_missing() {
    let mut c = cworld("fetchpull", true, Some("https://git.example.org/octo/demo-app.git"), 2, 0);
    c.add_conn(Kind::Gitea, "Home Gitea", "https://git.example.org");
    c.wait_head();
    c.app.open_repos_panel(&c.w.cx);
    press(&mut c.app, &c.w.cx, Key::Char('f'));
    let id = c.app.ws[0].meta.id.clone();
    ticks_until(&mut c.app, &c.w.cx, "fetched", |a| a.git.batches_finished(&id) && log(&c.w).contains("fetch"));
    assert!(log(&c.w).contains("ARGV: fetch --all --prune --progress"));
    press(&mut c.app, &c.w.cx, Key::Char('p'));
    ticks_until(&mut c.app, &c.w.cx, "pulled", |a| a.git.batches_finished(&id) && log(&c.w).contains("pull --ff-only"));
    // no origin: the push review says so; no commit: it says that
    let mut d = cworld("nopush", true, None, 0, 1);
    d.app.open_repos_panel(&d.w.cx);
    d.wait_head();
    d.app.open_push_review(&d.w.cx);
    assert!(d.flow().error.as_deref().unwrap().contains("no origin"));
    let mut e = cworld("nocommit", true, Some("https://git.example.org/octo/demo-app.git"), 0, 1);
    e.wait_head();
    e.app.open_push_review(&e.w.cx);
    assert!(e.flow().error.as_deref().unwrap().contains("no commit"), "{:?}", e.flow().error);
    cleanup(c.w);
    cleanup(d.w);
    cleanup(e.w);
}

#[test]
fn the_gitea_of_my_cluster_is_connected_through_the_platform_services_connect_step() {
    use iyakki_agent::platform::connect::FakeHttp as PHttp;
    use iyakki_agent::platform::installed::{self, Installed};
    use iyakki_agent::platform::secrets::MemVault;
    let mut c = cworld("cluster", false, None, 0, 2);
    let rec = Installed {
        id: "gitea-iyakki-gitea".into(),
        service: "gitea".into(),
        title: "Gitea".into(),
        kind: "git".into(),
        urls: vec![iyakki_agent::platform::UrlInfo { label: "Gitea".into(), url: "http://10.0.0.5:30300".into(), kind: "web".into() }],
        vault: vec!["svc.gitea-iyakki-gitea.token".into()],
        ..Default::default()
    };
    installed::save(c.w.cx.services.dir(), "tester", &[rec]).unwrap();
    // the platform context the IDE hands the flow: its own git runner and http (fakes) and vault
    let pr = StdArc::new(iyakki_agent::wsexec::FakeRunner::new(&["git", "curl"]));
    pr.answer("rev-parse --is-inside-work-tree", "true").answer("symbolic-ref --short HEAD", "main").answer("rev-list --count HEAD", "1").fail("remote get-url origin", 2, "error: No such remote").answer("remote add origin", "");
    let ph = StdArc::new(PHttp::default());
    ph.answer("GET http://10.0.0.5:30300/api/v1/user", 200, r#"{"login":"admin"}"#);
    ph.answer("POST http://10.0.0.5:30300/api/v1/user/repos", 201, r#"{"clone_url":"http://10.0.0.5:30300/admin/demo-app.git","html_url":"http://10.0.0.5:30300/admin/demo-app","ssh_url":"ssh://git@10.0.0.5:30222/admin/demo-app.git"}"#);
    let vault = StdArc::new(MemVault::default());
    vault.items.lock().unwrap().insert("svc.gitea-iyakki-gitea.token".into(), "tok_CLUSTERFAKEFAKEFAKE1".into());
    let audit = c.w.cx.audit.clone();
    c.app.git.platform = Some(StdArc::new(iyakki_agent::platform::Platform {
        run: pr.clone(),
        dir: c.w.cx.services.dir().to_path_buf(),
        user: "tester".into(),
        vault,
        audit: StdArc::new(move |a, d| audit.record("tester", a, d)),
        env: Default::default(),
        tmp: std::env::temp_dir(),
        http: ph.clone(),
        gen: StdArc::new(|n| "A".repeat(n)),
        live: None,
    }));
    // a connection to it, as the wizard makes it (the platform's own token entry, no copy)
    let conn = c.app.git_keys(&c.w.cx).conns().add(Connection { name: "My cluster".into(), kind: Kind::Gitea, url: "http://10.0.0.5:30300".into(), cluster: "gitea-iyakki-gitea".into(), token_vault: "svc.gitea-iyakki-gitea.token".into(), ..Default::default() }, None).unwrap();
    c.w.cx.services.user_set("tester", "svc.gitea-iyakki-gitea.token", "tok_CLUSTERFAKEFAKEFAKE1").unwrap();
    c.app.open_connect_flow(None, &c.w.cx);
    press(&mut c.app, &c.w.cx, Key::Enter);
    press(&mut c.app, &c.w.cx, Key::Char('1'));
    let review = c.flow().create_review().join("\n");
    assert!(review.contains("on your cluster") && review.contains("plain http"), "{review}");
    press(&mut c.app, &c.w.cx, Key::Ctrl('s'));
    c.wait_stage(Stage::Connected);
    // the platform step made the repository and set origin (through its runner), the first commit came from the project's git
    assert!(ph.lines().iter().any(|l| l.starts_with("POST http://10.0.0.5:30300/api/v1/user/repos")), "{:?}", ph.lines());
    assert!(pr.lines().iter().any(|l| l.ends_with("remote add origin http://10.0.0.5:30300/admin/demo-app.git")), "{:?}", pr.lines());
    assert!(c.http.requests().is_empty(), "the generic API client was not used for the cluster's Gitea");
    assert!(c.git.lines().iter().any(|l| l.starts_with("commit -q")));
    assert!(c.flow().message.as_deref().unwrap().contains("admin/demo-app"));
    assert!(c.audit().contains("git.repo.create") && c.audit().contains("10.0.0.5 admin/demo-app") && !c.audit().contains("FAKEFAKE"));
    let _ = conn;
    cleanup(c.w);
}
