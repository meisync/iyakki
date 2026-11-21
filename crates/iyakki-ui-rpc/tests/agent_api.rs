//! `RemoteAgentApi` over a real loopback socket (F-SPLIT-03): the three traits the core serves are called on the core's object (the fake of the
//! agent records each by name), every other trait of the facade answers its own quiet default, and the handle is real only while the core answers.

use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_agent_api::{AgentApi, AgentFacets, FakeAgentApi};
use iyakki_core_api::contract::RateLimit;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAgentApi, Rpc};

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    agent: Arc<FakeAgentApi>,
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn machine(serve_roots: bool) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let agent = Arc::new(FakeAgentApi::default());
    let mut host = Host::new();
    if serve_roots {
        host = host.with_root(Obj::AgentDisplay(agent.clone())).with_root(Obj::AgentFiles(agent.clone())).with_root(Obj::AgentStore(agent.clone()));
    }
    let served = Served {
        hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        appevents: None,
        rpc: Some(Arc::new(host)),
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, agent }
}

fn remote(m: &Machine) -> Arc<dyn AgentApi> {
    Arc::new(RemoteAgentApi::new(Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap())))
}

#[test]
fn every_method_of_the_three_served_traits_is_called_on_the_core_and_nothing_is_dropped() {
    let m = machine(true);
    let a = remote(&m);
    let p = Path::new("/data");
    a.cameras_release_screen_at(p, &["cam1".into()]);
    let _ = (a.camrec_active_ids(p), a.camrec_active_names(p), a.camrec_stop_all_for(p, "ann"));
    let _ = (a.disks_account_owner("ann", "x"), a.disks_media_dir("/home/ann"), a.disks_set_signout_pref("/home/ann", true), a.disks_signout_pref("/home/ann"));
    let _ = (a.gitconn_host_of("https://h/x"), a.media_is_mp3("a.mp3"), a.pkg_newer("2", "1"));
    a.sharedfolder_ask_share("/f");
    a.sharedfolder_on_logout("ann");
    let _ = (a.sharedfolder_random_hex(4), a.sharedfolder_running_total(), a.sharedfolder_stop_everything(), a.sharedfolder_take_share_ask());
    let _ = (a.wsexec_git_available(), a.wsexec_on_path("git"));
    let _ = (a.linuxapps_ide_context(), a.linuxapps_ide_wrap(p, vec!["x".into()]), a.linuxapps_valid_name("n"), a.linuxapps_valid_package("p"), a.linuxapps_workspace_root_of(p));
    a.linuxapps_set_ide_context(p, "ann");
    let mut calls = m.agent.calls();
    calls.sort();
    calls.dedup();
    assert_eq!(calls.len(), 25, "25 methods of the three traits, each reached the core's object once or more: {calls:?}");
}

#[test]
fn the_traits_the_core_does_not_serve_answer_their_quiet_defaults_and_nothing_reaches_the_core() {
    let m = machine(true);
    let a = remote(&m);
    assert!(a.profilepolicy_clean_site("x").unwrap_err().contains("unavailable"), "the trait's own default says the core does not serve it");
    assert!(a.ntp_servers_for("").is_empty());
    assert!(!a.ntp_valid_server("pool.example"));
    assert!(m.agent.calls().is_empty(), "a trait that is not a root never travels: the core's object saw nothing");
}

#[test]
fn the_handle_is_real_only_while_the_core_answers_its_files_root() {
    let m = machine(true);
    let a = remote(&m);
    assert!(AgentFacets::is_real(&*a));
    // a connection whose other end is closed
    let lost = RemoteAgentApi::new(Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink()))));
    assert!(!AgentFacets::is_real(&lost), "a core that is gone is not the machine");
    // a core that serves no agent roots is not real either
    let none = machine(false);
    assert!(!AgentFacets::is_real(&*remote(&none)));
}
