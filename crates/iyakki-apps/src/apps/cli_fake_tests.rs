//! The CLI tile over a fake port (`CliPort`): no services, no folder, no host, no network. The tests in `cli_*_tests.rs` run the same tile
//! through the real port (`CxPort`) and prove it changed nothing; these prove what the tile asks of its port.

use std::sync::{Arc, Mutex, MutexGuard};

use iyakki_agent_api::{SshHost, Terminal};
use iyakki_shared::cli::{Action, CliOut};
use iyakki_shared::clis::{Installed, Job, SystemStatus};
use iyakki_shared::dlmanifest_fetch::CheckStatus;
use iyakki_shared::ssh::Connection;

use super::*;
use crate::agent_login;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A terminal that answers `poweroff` and `reboot` with their action, refuses `secret`, and says `ran <line>` to the rest.
struct FakeTerminal;
impl Terminal for FakeTerminal {
    fn exec(&mut self, line: &str) -> CliOut {
        match line {
            "poweroff" => CliOut { text: "Powering off.".into(), action: Action::PowerOff },
            "reboot" => CliOut { text: "Restarting.".into(), action: Action::Reboot },
            "secret" => CliOut { text: "secret: not permitted".into(), action: Action::None },
            other => CliOut { text: format!("ran {other}"), action: Action::None },
        }
    }
    fn full(&self) -> bool {
        true
    }
}

/// The saved connections, in memory.
#[derive(Default)]
struct FakeSsh {
    list: Mutex<Vec<Connection>>,
}
impl SshHost for FakeSsh {
    fn connections(&self) -> Vec<Connection> {
        lock(&self.list).clone()
    }
    fn save_connection(&self, c: Connection, replacing: Option<&str>) -> Result<(), String> {
        let mut l = lock(&self.list);
        l.retain(|x| Some(x.name.as_str()) != replacing);
        l.push(c);
        Ok(())
    }
    fn remove_connection(&self, name: &str) -> Result<(), String> {
        lock(&self.list).retain(|c| c.name != name);
        Ok(())
    }
}

struct FakePort {
    admin: bool,
    full: bool,
    hosts_ok: bool,
    audit: Mutex<Vec<(String, String)>>,
    powered: Mutex<Vec<&'static str>>,
    ssh: Arc<FakeSsh>,
    installed: Mutex<Vec<Installed>>,
    login: Mutex<Option<String>>,
    cluster_started: Mutex<u32>,
}

impl FakePort {
    fn new(admin: bool) -> FakePort {
        FakePort { admin, full: true, hosts_ok: true, audit: Mutex::new(vec![]), powered: Mutex::new(vec![]), ssh: Arc::new(FakeSsh::default()), installed: Mutex::new(vec![]), login: Mutex::new(None), cluster_started: Mutex::new(0) }
    }
    fn audited(&self, action: &str) -> Vec<String> {
        lock(&self.audit).iter().filter(|(a, _)| a == action).map(|(_, s)| s.clone()).collect()
    }
}

impl CliPort for FakePort {
    fn user(&self) -> String {
        "tester".into()
    }
    fn role_name(&self) -> String {
        if self.admin { "Administrator" } else { "User" }.into()
    }
    fn may_manage_tools(&self) -> bool {
        self.admin && self.full
    }
    fn full_terminal(&self) -> bool {
        self.full
    }
    fn allows_host(&self, _host: &str) -> bool {
        self.hosts_ok
    }
    fn audit(&self, action: &str, subject: &str) {
        lock(&self.audit).push((action.into(), subject.into()));
    }
    fn open_terminal(&self) -> Box<dyn Terminal> {
        Box::new(FakeTerminal)
    }
    fn power_off(&self) {
        lock(&self.powered).push("off");
    }
    fn reboot(&self) {
        lock(&self.powered).push("reboot");
    }
    fn take_ssh_login(&self) -> Option<String> {
        lock(&self.login).take()
    }
    fn ssh(&self) -> Arc<dyn SshHost> {
        self.ssh.clone()
    }
    fn make_ssh_key(&self, name: &str) -> Result<String, String> {
        Ok(format!("ssh-ed25519 FAKE {name}"))
    }
    fn ssh_key_in_vault(&self, _name: &str) -> Result<bool, String> {
        Ok(false)
    }
    fn ssh_client(&self) -> Option<SystemStatus> {
        None
    }
    fn tools_installed(&self) -> Vec<Installed> {
        lock(&self.installed).clone()
    }
    fn tool_installed(&self, id: &str) -> Option<Installed> {
        lock(&self.installed).iter().find(|i| i.id == id).cloned()
    }
    fn tool_system_status(&self, _id: &str) -> Option<SystemStatus> {
        None
    }
    fn tool_job(&self, _id: &str) -> Option<Job> {
        None
    }
    fn tools_busy(&self) -> bool {
        false
    }
    fn tool_install_plan(&self, _id: &str) -> Option<String> {
        None
    }
    fn tool_install(&self, id: &str) -> Result<(), String> {
        lock(&self.installed).push(Installed { id: id.into(), version: "v1".into(), reported: String::new(), at: 0, bins: vec![], verified: true, by: "tester".into(), dir: String::new(), previous: None });
        Ok(())
    }
    fn tool_remove(&self, id: &str) -> Result<(), String> {
        lock(&self.installed).retain(|i| i.id != id);
        Ok(())
    }
    fn tool_rollback(&self, _id: &str) -> Result<String, String> {
        Err("nothing to go back to".into())
    }
    fn download_list_check(&self, _force: bool) {}
    fn download_list_audit(&self) -> Vec<(String, String)> {
        vec![("dlmanifest.check".into(), "fake".into())]
    }
    fn download_list_status(&self) -> CheckStatus {
        CheckStatus::default()
    }
    fn agent_status(&self, _agent: &str) -> Option<iyakki_extagentx::Status> {
        None
    }
    fn agent_sign_in_unchecked(&self, _agent: &str) -> bool {
        false
    }
    fn agent_recheck(&self) {}
    fn agent_sign_in(&self, _agent: &str) -> agent_login::State {
        agent_login::State::SignedOut
    }
    fn ollama_state(&self) -> iyakki_shared::ollama::OllamaState {
        iyakki_shared::ollama::OllamaState::Stopped
    }
    fn ollama_url(&self) -> String {
        "http://127.0.0.1:11434".into()
    }
    fn ollama_models_dir(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("models")
    }
    fn ollama_start(&self) -> Result<(), String> {
        Ok(())
    }
    fn ollama_stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn cluster_state(&self) -> iyakki_shared::cluster::ClusterState {
        iyakki_shared::cluster::ClusterState::Stopped
    }
    fn cluster_requirements(&self) -> Vec<String> {
        vec!["a container runtime".into()]
    }
    fn cluster_start(&self) -> Result<(), String> {
        *lock(&self.cluster_started) += 1;
        Ok(())
    }
    fn cluster_stop(&self) -> Result<(), String> {
        Ok(())
    }
}

fn type_line(app: &mut CliApp, port: &dyn CliPort, s: &str) {
    for c in s.chars() {
        app.handle_with(&Event::Key(Key::Char(c)), port);
    }
    app.handle_with(&Event::Key(Key::Enter), port);
}

fn draw(app: &mut CliApp, port: &dyn CliPort, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    app.render_with(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, port, 3);
    g
}

fn select(app: &mut CliApp, id: &str) {
    app.sel = CATALOG.iter().position(|d| d.id == id).expect("catalogue entry");
}

#[test]
fn a_line_runs_through_the_port_terminal_and_the_audit_line_names_the_command_only() {
    let port = FakePort::new(true);
    let mut app = CliApp::with_port(&port);
    assert!(app.output_lines()[0].contains("tester (Administrator)"), "{:?}", app.output_lines());
    type_line(&mut app, &port, "whoami -secret-arg");
    assert!(app.output_lines().iter().any(|l| *l == "ran whoami -secret-arg"));
    assert_eq!(port.audited("cli.exec"), vec!["whoami".to_string()], "the arguments are never logged");
    type_line(&mut app, &port, "secret");
    assert_eq!(port.audited("cli.denied"), vec!["secret".to_string()]);
}

#[test]
fn power_commands_audit_first_and_reach_the_port() {
    let port = FakePort::new(true);
    let mut app = CliApp::with_port(&port);
    type_line(&mut app, &port, "poweroff");
    type_line(&mut app, &port, "reboot");
    assert_eq!(*lock(&port.powered), vec!["off", "reboot"]);
    assert_eq!(port.audited("power.off").len(), 1);
    assert_eq!(port.audited("power.reboot").len(), 1);
}

#[test]
fn an_administrator_installs_and_removes_a_tool_after_confirming_and_a_user_is_refused() {
    let port = FakePort::new(true);
    let mut app = CliApp::with_port(&port);
    app.handle_with(&Event::Tab(1), &port);
    select(&mut app, "kubectl");
    app.handle_with(&Event::Key(Key::Char('i')), &port);
    assert!(matches!(app.mode, Mode::ConfirmInstall("kubectl")));
    assert!(port.tools_installed().is_empty(), "nothing runs before the confirmation");
    app.handle_with(&Event::Key(Key::Char('y')), &port);
    assert_eq!(port.tools_installed().len(), 1);
    assert_eq!(port.audited("cli.install"), vec!["kubectl".to_string()]);
    app.handle_with(&Event::Key(Key::Char('x')), &port);
    app.handle_with(&Event::Key(Key::Char('y')), &port);
    assert!(port.tools_installed().is_empty());
    assert_eq!(port.audited("cli.remove"), vec!["kubectl".to_string()]);

    let user = FakePort::new(false);
    let mut app = CliApp::with_port(&user);
    app.handle_with(&Event::Tab(1), &user);
    select(&mut app, "kubectl");
    app.handle_with(&Event::Key(Key::Char('i')), &user);
    assert!(matches!(app.mode, Mode::Browse));
    assert!(app.status_text().is_some_and(|s| s.contains("Only administrators")));
    assert!(user.tools_installed().is_empty());
}

#[test]
fn the_cluster_entry_asks_first_and_starts_through_the_port() {
    let port = FakePort::new(true);
    let mut app = CliApp::with_port(&port);
    app.handle_with(&Event::Tab(1), &port);
    select(&mut app, "k3s");
    app.handle_with(&Event::Key(Key::Char('s')), &port);
    assert_eq!(*lock(&port.cluster_started), 0, "the start waits for the confirmation");
    app.handle_with(&Event::Key(Key::Char('y')), &port);
    assert_eq!(*lock(&port.cluster_started), 1);
    assert_eq!(port.audited("cluster.start").len(), 1);
}

#[test]
fn a_terminal_that_is_not_full_cannot_connect_and_the_hosts_policy_is_asked() {
    let mut port = FakePort::new(false);
    port.full = false;
    let mut app = CliApp::with_port(&port);
    app.conns.login_by_name(&port, "box");
    assert!(app.conns.status_text().is_some_and(|s| s.contains("no saved connection")), "{:?}", app.conns.status_text());
    lock(&port.ssh.list).push(Connection { name: "box".into(), host: "box.example.com".into(), port: 22, ..Default::default() });
    app.conns.login_by_name(&port, "box");
    assert!(app.conns.status_text().is_some_and(|s| s.contains("full terminal")), "{:?}", app.conns.status_text());
    assert!(app.conns.login.is_none());

    let mut port = FakePort::new(true);
    port.hosts_ok = false;
    lock(&port.ssh.list).push(Connection { name: "box".into(), host: "box.example.com".into(), port: 22, ..Default::default() });
    let mut app = CliApp::with_port(&port);
    app.conns.login_by_name(&port, "box");
    assert!(app.conns.status_text().is_some_and(|s| s.contains("may not reach box.example.com")), "{:?}", app.conns.status_text());
}

#[test]
fn a_login_the_assistant_asked_for_is_taken_from_the_port_once_and_switches_to_the_connections_tab() {
    let port = FakePort::new(true);
    *lock(&port.login) = Some("nothing".into());
    let mut app = CliApp::with_port(&port);
    app.handle_with(&Event::Tick, &port);
    assert_eq!(app.tab(), Tab::Connections);
    assert!(app.conns.status_text().is_some_and(|s| s.contains("no saved connection called nothing")));
    assert!(port.take_ssh_login().is_none(), "asked for once");
}

#[test]
fn the_tabs_draw_over_the_fake_port_at_four_sizes() {
    let port = FakePort::new(true);
    lock(&port.ssh.list).push(Connection { name: "web".into(), host: "web.example.com".into(), port: 22, ..Default::default() });
    let mut app = CliApp::with_port(&port);
    for (w, h) in [(1280, 720), (900, 560), (600, 420), (360, 640)] {
        for tab in 0..3 {
            app.handle_with(&Event::Tab(tab), &port);
            draw(&mut app, &port, w, h);
        }
    }
    app.handle_with(&Event::Tab(1), &port);
    select(&mut app, "k3s");
    let g = draw(&mut app, &port, 1280, 720);
    assert!(g.layout_log.texts.iter().any(|t| t.3.contains("a container runtime")), "the port's requirement is on the screen");
    assert_eq!(port.audited("dlmanifest.check"), vec!["fake".to_string(); port.audited("dlmanifest.check").len()], "the check's audit lines go through the port");
    assert!(!port.audited("dlmanifest.check").is_empty());
}
