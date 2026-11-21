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
    // the assistant's questions about the browser and the job queue are roots too: a question made in the core is listed and answered over the socket
    let consent = match rpc.root("ConsentApi").expect("generated") {
        RemoteAny::ConsentApi(c) => c,
        _ => panic!("ConsentApi root"),
    };
    let id = svc.browser.consent.create("ann", iyakki_shared::browserconsent::ConsentKind::Look, "a.test", "Look at a.test?", "", 5).unwrap();
    let pending = consent.pending("ann");
    assert_eq!((pending.len(), pending[0].id.as_str()), (1, id.as_str()));
    consent.resolve("ann", &id, iyakki_shared::browserconsent::Answer::Allow).unwrap();
    assert!(svc.browser.consent.pending("ann").is_empty(), "the answer given over the socket reached the core's own store");
    let jobs = match rpc.root("BrowserJobsApi").expect("generated") {
        RemoteAny::BrowserJobsApi(j) => j,
        _ => panic!("BrowserJobsApi root"),
    };
    svc.browser.jobs.submit_detached("ann", iyakki_shared::browserjobs::Op::State);
    assert_eq!(jobs.pending_for("ann"), 1);
    let taken = jobs.take("ann");
    assert_eq!(taken.len(), 1);
    jobs.complete(taken[0].id, iyakki_shared::browserjobs::Reply::Ok);
    assert_eq!(jobs.pending_for("ann"), 0);
    // the settings: a screen over the socket reads a snapshot and changes the real store through the same guard the in-process screens use
    let remote_settings = match rpc.root("SettingsApi").expect("generated") {
        RemoteAny::SettingsApi(s) => s,
        _ => panic!("SettingsApi root"),
    };
    let snap = remote_settings.snapshot();
    assert_eq!(snap.settings, svc.settings.lock().unwrap().get().clone());
    let view = iyakki_shared::settings_ref::SettingsRef::new(Arc::new(iyakki_agent_api::SettingsSrc(remote_settings)));
    let days = snap.settings.tool_log_days;
    view.lock().unwrap().update(|s| s.tool_log_days = days + 3).unwrap();
    assert_eq!(svc.settings.lock().unwrap().get().tool_log_days, days + 3, "the change made over the socket is in the core's own settings");
    assert_eq!(view.lock().unwrap().get().tool_log_days, days + 3);
    // the knowledge base: a document added over the socket is the core's own
    let kb = match rpc.root("KnowledgeBaseApi").expect("generated") {
        RemoteAny::KnowledgeBaseApi(k) => k,
        _ => panic!("KnowledgeBaseApi root"),
    };
    kb.add_document("user/ann", "t", "t.txt", "a note about quokkas", Default::default()).unwrap();
    assert_eq!(svc.kb.lock().unwrap().docs("user/ann").len(), 1);
    assert_eq!(kb.search_in(&["user/ann".to_string()], "quokkas", 2).len(), 1);
    // a trait this build does not know is `None`; one it knows but the core does not register as a root answers "no such object", not a panic
    assert!(rpc.root("NoSuchApiAtAll").is_none());
    assert!(rpc.call::<serde_json::Value>("CameraHubApi", 0, "busy_clear", serde_json::json!([])).is_err());
    drop((off, on));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_accounts_are_a_root_the_actor_is_read_again_in_the_core_and_the_hash_never_crosses() {
    use iyakki_accounts::{AccountError, Session, UserStore, UsersRef};
    use iyakki_agent_api::UsersSrc;
    use iyakki_sec::policy::Role;
    iyakki_compose::wire();
    let d = iyakki_agent::testdir::short("iyakki-rpcroots", "acc");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let store = Arc::new(std::sync::Mutex::new(UserStore::open(&d.join("users.json")).unwrap()));
    let real: Arc<dyn iyakki_agent_api::AccountsApi> = Arc::new(store.clone());
    let roots = RpcRoots::new().with_accounts(real);
    assert!(roots.root_names().contains(&"AccountsApi") && RpcRoots::new().root_names().iter().all(|n| *n != "AccountsApi"), "served only when the core gives its accounts");
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let on = start_with(&core, &cfg(true), "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(5)).unwrap());
    let remote = match rpc.root("AccountsApi").expect("generated") {
        RemoteAny::AccountsApi(a) => a,
        _ => panic!("AccountsApi root"),
    };
    let users = UsersRef::new(Arc::new(UsersSrc(remote.clone())));

    // the first run: nobody exists, the first administrator is made over the socket, the second attempt is refused
    assert!(users.lock().unwrap().is_empty());
    users.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
    assert_eq!(users.lock().unwrap().create_first_admin("other", "otherpass12"), Err(AccountError::Forbidden));
    assert!(!store.lock().unwrap().is_empty() && !users.lock().unwrap().is_empty(), "the core's own store has the account");

    // a sign-in over the socket is the store's own: right password, wrong password, and the session carries the policy
    let admin = users.lock().unwrap().authenticate("root", "rootpass12").unwrap();
    assert_eq!((admin.user.as_str(), admin.role), ("root", Role::Admin));
    assert_eq!(users.lock().unwrap().authenticate("root", "wrong-pass"), Err(AccountError::InvalidCredentials));

    // the list and the single account carry no hash, the core's store does
    users.lock().unwrap().create_user(&admin, "gus", "guspass-123", Role::Guest).unwrap();
    let listed = users.lock().unwrap().list();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|u| u.hash == "*"), "no password hash on the served list");
    assert!(users.lock().unwrap().get("gus").unwrap().hash == "*");
    assert!(store.lock().unwrap().get("gus").unwrap().hash.starts_with("$argon2"), "the hash stays in the core");

    // a forged session: the guest claims to be an administrator. The core reads the role of "gus" from its own store, so nothing changes
    let forged = Session { user: "gus".into(), role: Role::Admin, policy: Role::Admin.template() };
    assert_eq!(users.lock().unwrap().create_user(&forged, "evil", "evilpass-123", Role::Admin), Err(AccountError::Forbidden));
    assert_eq!(users.lock().unwrap().set_role(&forged, "gus", Role::Admin), Err(AccountError::Forbidden));
    assert!(store.lock().unwrap().get("evil").is_none() && store.lock().unwrap().get("gus").unwrap().role == Role::Guest);
    // an actor the core has never heard of
    let ghost = Session { user: "ghost".into(), role: Role::Admin, policy: Role::Admin.template() };
    // refused before the core's object runs (the vouch step, D-SEC1-VOUCH): "no account ghost", never a deletion
    let refused = users.lock().unwrap().delete_user(&ghost, "gus");
    assert!(matches!(&refused, Err(AccountError::Forbidden)) || matches!(&refused, Err(AccountError::Storage(m)) if m.contains("no account ghost")), "{refused:?}");
    assert!(store.lock().unwrap().get("gus").is_some(), "the ghost deleted nothing");
    // an honest administrator, even with a stale role in the session it sends, is judged by what the core knows
    let stale = Session { user: "root".into(), role: Role::Guest, policy: Role::Guest.template() };
    users.lock().unwrap().set_role(&stale, "gus", Role::User).unwrap();
    assert_eq!(store.lock().unwrap().get("gus").unwrap().role, Role::User);
    // the last administrator stays protected, and a deleted account is gone from the core's store
    assert_eq!(users.lock().unwrap().delete_user(&admin, "root"), Err(AccountError::LastAdmin));
    users.lock().unwrap().delete_user(&admin, "gus").unwrap();
    assert!(store.lock().unwrap().get("gus").is_none());

    // the command-line-only way to add an account is not on the interface
    assert!(rpc.call::<serde_json::Value>("AccountsApi", 0, "add_local", serde_json::json!(["x", "xxxxxxxx12", "admin"])).is_err());
    drop(on);
    let _ = std::fs::remove_dir_all(&d);
}
