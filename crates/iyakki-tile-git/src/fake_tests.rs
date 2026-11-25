//! The GIT tile against a fake port: no host, no git program, no keys, no workspace registry, no clock, no sleep. The same tile is also proved through
//! the real context in `crates/iyakki-apps/src/apps/git_tile_tests.rs` (scripted hosts, scripted git) and in the AI IDE's Git tab tests.

use iyakki_git_model::gitbrowse::LocalClone;
use iyakki_git_model::gitconn::{Connection, Kind};
use iyakki_git_model::gitjobs::Op;
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use iyakki_git_model::keys::SecretStore as _;
use crate::fake::FakePort;
use crate::port::GitPort as _;
use crate::git_tile::{GitApp, Rig};
use crate::port::Flow;

struct Fx {
    app: GitApp,
    port: FakePort,
    g: Gfx,
    tick: u64,
}

impl Fx {
    fn new() -> Fx {
        let port = FakePort::new();
        let app = GitApp::with_rig(&port, Rig::offline());
        let mut f = Fx { app, port, g: Gfx::new(1280, 720), tick: 0 };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &self.port, self.tick);
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn conn(&self, name: &str) -> Connection {
        self.port.conns().add(Connection { name: name.into(), kind: Kind::Gitea, url: "https://git.example.org".into(), account: "me".into(), ..Default::default() }, Some("tok_FAKEFAKEFAKEFAKE1234")).unwrap()
    }
    fn clone_of(&self, name: &str) -> LocalClone {
        let dir = self.port.dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        LocalClone { name: name.into(), dir, remote: Some("https://git.example.org/me/app.git".into()), branch: "main".into() }
    }
}

fn audited(f: &Fx, action: &str) -> Vec<String> {
    f.port.st().audit.iter().filter(|a| a.0 == action).map(|a| a.1.clone()).collect()
}

#[test]
fn a_connection_the_port_gives_is_a_source_of_the_tile_and_the_vault_holds_its_token() {
    let mut f = Fx::new();
    let c = f.conn("Home");
    f.app.refresh_entries(&f.port);
    assert!(f.app.srcs.iter().any(|s| s.key() == c.id), "the host is listed as a source");
    assert!(f.port.vault.names().unwrap().iter().any(|n| n.contains(&c.id)), "the token went to the person's vault, not to the tile");
}

#[test]
fn the_tile_starts_in_the_repositories_folder_the_port_names_and_esc_leaves_it() {
    let mut f = Fx::new();
    assert_eq!(f.app.repos_root, f.port.root);
    assert_eq!(f.app.handle_event(&Event::Key(Key::Esc), &f.port), Flow::Back);
}

#[test]
fn a_clone_makes_the_folder_through_the_port_plans_the_job_there_and_audits_it() {
    let mut f = Fx::new();
    let c = f.conn("Home");
    f.app.start_clone(&f.port, &c, "me/app", "https://git.example.org/me/app.git", false, false);
    assert_eq!(f.port.st().ensured, vec![f.port.root.clone()], "the port made the folder and its project.json");
    let planned = f.port.st().planned.clone();
    assert_eq!(planned.len(), 1);
    assert!(planned[0].starts_with("clone ") && planned[0].contains("Repositories") && planned[0].contains("https://git.example.org/me/app.git"), "{planned:?}");
    assert_eq!(audited(&f, "git.clone.start"), ["1 repositories into Repositories"]);
    let run = f.app.run.as_ref().expect("the run is on screen");
    assert_eq!(run.op, Op::Clone);
    assert!(run.label.contains("me/app") && run.label.contains("Home"));
}

#[test]
fn a_plan_the_port_refuses_says_why_and_starts_nothing() {
    let mut f = Fx::new();
    let c = f.conn("Home");
    f.port.st().plan_error = Some("git is not installed on this computer".into());
    f.app.start_clone(&f.port, &c, "me/app", "https://git.example.org/me/app.git", false, false);
    assert!(f.app.run.is_none());
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("not installed")), "{:?}", f.app.status);
    assert!(audited(&f, "git.clone.start").is_empty(), "a refused plan is not audited as started");
}

#[test]
fn a_push_plans_the_branch_to_the_remote_and_audits_the_host_not_the_address() {
    let mut f = Fx::new();
    let cl = f.clone_of("app");
    f.app.start_push(&f.port, &cl, "main", "https://git.example.org/me/app.git");
    let planned = f.port.st().planned.clone();
    assert!(planned[0].starts_with("push ") && planned[0].ends_with("https://git.example.org/me/app.git . main"), "{planned:?}");
    assert_eq!(audited(&f, "git.push"), ["git.example.org app"]);
    assert_eq!(f.app.run.as_ref().map(|r| r.op), Some(Op::Push));
}

#[test]
fn a_run_is_driven_by_ticks_and_says_how_it_ended() {
    let mut f = Fx::new();
    let cl = f.clone_of("app");
    f.app.rig.run_jobs = true;
    f.app.start_push(&f.port, &cl, "main", "https://git.example.org/me/app.git");
    f.app.tick(&f.port);
    assert_eq!(f.port.st().batches[0].lock().unwrap().ticks, 1, "the tile ticks the batch the port made");
    f.app.tick(&f.port);
    assert!(f.app.status.as_ref().is_some_and(|s| s.1 && s.0 == "Pushed app."), "{:?}", f.app.status);
}

#[test]
fn a_failed_job_shows_the_reason_git_gave() {
    let mut f = Fx::new();
    let cl = f.clone_of("app");
    f.app.rig.run_jobs = true;
    f.app.start_update(&f.port, Op::Fetch, &cl);
    // no origin is readable with the offline rig: the tile says so without asking the port
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("no origin")), "{:?}", f.app.status);
    f.app.start_push(&f.port, &cl, "main", "https://git.example.org/me/app.git");
    {
        let log = f.port.st().batches[0].clone();
        let mut l = log.lock().unwrap();
        l.failed = 1;
        l.error = "remote: permission denied".into();
    }
    f.app.tick(&f.port);
    f.app.tick(&f.port);
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("permission denied")), "{:?}", f.app.status);
}

#[test]
fn opening_a_clone_in_the_ide_adds_a_workspace_once_and_asks_the_shell_to_open_it() {
    let mut f = Fx::new();
    let cl = f.clone_of("app");
    f.app.open_ide(&f.port, &cl);
    assert_eq!(f.app.request.as_ref().map(|r| r.0.as_str()), Some("ide"));
    assert_eq!(f.app.request.as_ref().map(|r| r.1.as_str()), Some("ws:ws1"));
    assert_eq!(f.port.st().workspaces.len(), 1);
    assert_eq!(audited(&f, "ide.workspace.add").len(), 1);
    f.app.take_request();
    f.app.open_ide(&f.port, &cl);
    assert_eq!(f.port.st().workspaces.len(), 1, "the same folder is not added twice");
    assert_eq!(f.app.request.as_ref().map(|r| r.1.as_str()), Some("ws:ws1"));
}

#[test]
fn a_folder_the_account_may_not_write_in_is_not_taken_as_a_workspace() {
    let mut f = Fx::new();
    let cl = f.clone_of("app");
    f.port.st().write_ok = false;
    f.app.open_ide(&f.port, &cl);
    assert!(f.port.st().workspaces.is_empty());
    assert!(f.app.request.is_none());
    assert!(f.app.status.as_ref().is_some_and(|s| !s.1 && s.0.contains("may not write")), "{:?}", f.app.status);
}

#[test]
fn the_first_screen_with_no_connection_says_how_to_add_one() {
    let f = Fx::new();
    assert!(f.text().contains("Settings, Git"), "{}", f.text());
}
