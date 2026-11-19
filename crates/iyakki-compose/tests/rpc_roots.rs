//! The real core serves its root objects over the UI socket (F-SPLIT-03): the composition root's `RpcRoots` over the real services, the
//! socket's `rpc` switch, and a UI-side `Rpc` reading the core's own inbox. Linux only: a unix socket.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_agent_api::InboxApi;
use iyakki_compose::rpc::{RpcRoots, ROOTS};
use iyakki_shared::inbox::Kind;
use iyakki_ui_client::client::Connection;
use iyakki_ui_rpc::{RemoteAny, Rpc};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn cfg(rpc: bool) -> UiSocketConfig {
    UiSocketConfig { enabled: true, scopes: vec!["settings:read".into()], account: "ui".into(), admin: false, rpc }
}

#[test]
fn the_core_serves_its_real_stores_as_roots_only_when_the_rpc_switch_is_on() {
    iyakki_compose::wire();
    let d = iyakki_agent::testdir::short("iyakki-rpcroots", "a");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    svc.inbox.add("ann", Kind::Agent, "Hello", "body", "test", None, 100).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let roots = RpcRoots::new();
    assert_eq!(roots.root_names().len(), ROOTS.len());
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };

    // off: the call is refused as unsupported
    let off = start_with(&core, &cfg(false), "iyakki-test", &d.join("off.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(off.path(), Duration::from_secs(5)).unwrap());
    let inbox = match rpc.root("InboxApi").expect("generated") {
        RemoteAny::InboxApi(i) => i,
        _ => panic!("InboxApi root"),
    };
    assert!(inbox.list("ann").is_empty(), "with the switch off the UI reads nothing: the answer of a lost call");

    // on: the real store answers, and a write through the socket lands in the core's inbox
    let on = start_with(&core, &cfg(true), "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(5)).unwrap());
    let inbox: Arc<dyn InboxApi> = match rpc.root("InboxApi").expect("generated") {
        RemoteAny::InboxApi(i) => i,
        _ => panic!("InboxApi root"),
    };
    let listed = inbox.list("ann");
    assert_eq!((listed.len(), listed[0].title.as_str()), (1, "Hello"));
    inbox.add("ann", Kind::Agent, "Second", "b", "test", None, 101).unwrap();
    assert_eq!(svc.inbox.list("ann").len(), 2);
    // a trait that is not a root is "no such object", not a panic
    assert!(matches!(rpc.root("CameraHubApi"), None));
    drop((off, on));
    let _ = std::fs::remove_dir_all(&d);
}
