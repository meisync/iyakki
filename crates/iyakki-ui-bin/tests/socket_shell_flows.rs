//! The FLOWS tile over the REAL core's UI socket (F-SPLIT-03, D-S5-01): its own test program, because the shell installs its core handles in one-shot
//! cells per process (the first program start wins), so a second socket in the same process would talk to the first one's device. Linux only.
#![cfg(all(target_os = "linux", feature = "socket-shell"))]

mod common;

use common::{drive, Device};
use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_ui_bin::socket_shell::environment;

/// The FLOWS tile over the socket (F-SPLIT-03, D-S5-01): the shell opens FLOWS from the six remote objects (not the "not served" screen), and the same
/// objects make a draft, publish it, start a run that waits for a person, decide it, with every line in the core's own audit log and the person
/// the core's, whatever the program claims.
#[test]
fn the_flows_tile_runs_over_the_real_cores_socket_make_publish_run_approve_and_the_core_audits() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use iyakki_agent::contract_local::uiserve::PROGRAM_SCOPES;
    use iyakki_agent_api::{AuditApi, FlowsJobs, ServicesHandle};
    use iyakki_flow::model::RunStatus;
    use iyakki_ui_client::client::Connection;
    use iyakki_ui_rpc::{RemoteServicesHandle, Rpc};

    let dev = Device::new("flows");
    // the daemon wires the core's audit log into the services (the engine's lines and the tile's lines go to it); the harness does the same
    let log = dev.audit.clone();
    dev.svc.browser.set_audit(Arc::new(move |u: &str, e: &str, d: &str| log.record(u, e, d)));
    let core = iyakki_agent::contract_local::core_for(&dev.svc);
    let facets = Facets { rpc: Some(dev.roots().factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: PROGRAM_SCOPES.iter().map(|s| s.to_string()).collect(), account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &dev.dir.join("on.sock"), &facets).unwrap();

    // the shell: signs in through the core, opens FLOWS and shows the tile, not the "not served" screen
    let env = environment(on.path(), false, 3, &std::thread::yield_now).unwrap();
    let mut sh = iyakki_shell::shell::Shell::new(env);
    drive(&mut sh, &dev);
    assert!(sh.open_for_demo("flows"), "FLOWS opens");
    let mut g = iyakki_shell::gfx::Gfx::new(1280, 720);
    for _ in 0..3 {
        sh.render(&mut g);
        sh.handle(&iyakki_shell::ui::Event::Tick);
    }

    // the program's six objects for the signed-in account, on a connection of their own
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(10)).unwrap());
    // the core binds a connection to the account that signed in on it (SEC3): this one signs in as the same person first
    match rpc.root("AccountsApi") {
        Some(iyakki_ui_rpc::RemoteAny::AccountsApi(a)) => {
            use iyakki_agent_api::AccountsApi as _;
            a.authenticate("root", "rootpass12").expect("the account signs in on this connection");
        }
        _ => panic!("AccountsApi root"),
    }
    let remote = RemoteServicesHandle::new(rpc);
    let jobs = remote.flows_jobs("root", &iyakki_sec::policy::Role::Admin.template()).expect("the core serves the flows objects to a socket with the program's scopes");
    let spec = serde_json::json!({"name": "Gate", "nodes": [{"id": "ok", "kind": "approval", "config": {"message": "Go on?"}}, {"id": "done", "kind": "notify", "config": {"title": "done"}}], "edges": [{"from": "start", "to": "ok"}, {"from": "ok", "to": "done", "port": "approved"}]});
    let made = jobs.create_draft(iyakki_agent::flows::build_flow(&spec).unwrap(), "user:claimed-by-the-program").unwrap();
    let v = jobs.publish(&made.id, "user:claimed-by-the-program", "", "first", Some("user:claimed-by-the-program".into())).unwrap();
    jobs.audit("flow.publish", &format!("{} v{v} by {}", made.id, jobs.who()));
    let run = jobs.start_run(iyakki_flow::StartRequest::new(&made.id).trigger("manual", "user:claimed-by-the-program")).unwrap().run_id;
    let wall = Instant::now();
    while jobs.get_run(&run).is_none_or(|r| r.status != RunStatus::Blocked) {
        assert!(wall.elapsed() < Duration::from_secs(30), "the run waits for a person");
        std::thread::yield_now();
    }
    // the tile still draws with the run waiting (the board reads it through the objects)
    for _ in 0..5 {
        sh.render(&mut g);
        sh.handle(&iyakki_shell::ui::Event::Tick);
    }
    assert_eq!(sh.screen_name(), "app:flows");
    assert_eq!(jobs.run_counts().needs_you, 1, "one run needs the person");
    jobs.decide(&run, "ok", true, "user:claimed-by-the-program", "").unwrap();
    while jobs.get_run(&run).is_none_or(|r| !r.status.is_terminal()) {
        assert!(wall.elapsed() < Duration::from_secs(30), "the run ends after the decision");
        std::thread::yield_now();
    }
    assert_eq!(jobs.get_run(&run).unwrap().status, RunStatus::Succeeded);
    // the core's own log has the lines, and the person is the account the core knows
    let audit = dev.audit_text();
    assert!(audit.contains("flow.publish"), "the publish line is in the core's log: {audit}");
    assert!(!audit.contains("claimed-by-the-program"), "the program's claim of a person is never written: {audit}");
    assert!(ServicesHandle::flows_jobs(&*dev.svc, "root", &iyakki_sec::policy::Role::Admin.template()).unwrap().get_run(&run).is_some(), "the run is the core's own");
}
