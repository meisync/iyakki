//! `RemoteServicesHandle` against the real services over the real UI socket (F-SPLIT-03 part 1, D-SPLIT-03-HANDLE): one kind of method after the
//! other, the answer the core's own `Services` gives and, for a write, the change in the core's own state. Linux only: a unix socket.
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_agent_api::{AuditApi, ServicesHandle};
use iyakki_compose::rpc::RpcRoots;
use iyakki_shared::inbox::Kind;
use iyakki_ui_client::client::Connection;
use iyakki_ui_rpc::{RemoteServicesHandle, Rpc};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// An audit log that keeps what it was given.
#[derive(Default)]
struct Notes(Mutex<Vec<(String, String, String)>>);
impl AuditApi for Notes {
    fn path_buf(&self) -> std::path::PathBuf {
        std::path::PathBuf::new()
    }
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.0.lock().unwrap().push((user.into(), event.into(), detail.into()));
    }
    fn flush_notes(&self) {}
    fn start_new_log(&self) -> Result<std::path::PathBuf, String> {
        Err("none".into())
    }
    fn verify(&self) -> Result<usize, usize> {
        Ok(0)
    }
    fn recent(&self, _: usize) -> Vec<String> {
        Vec::new()
    }
}

struct Machine {
    svc: Arc<iyakki_agent::services::Services>,
    remote: Arc<dyn ServicesHandle>,
    rpc: Rpc,
    notes: Arc<Notes>,
    _on: iyakki_agent::contract_local::uiserve::UiServer,
    dir: std::path::PathBuf,
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn machine(tag: &str) -> Machine {
    machine_with(tag, |r| r)
}

fn machine_with(tag: &str, more: impl FnOnce(RpcRoots) -> RpcRoots) -> Machine {
    iyakki_compose::wire();
    let d = iyakki_agent::testdir::short("iyakki-remsvc", tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let notes = Arc::new(Notes::default());
    let roots = more(RpcRoots::new().with_audit(notes.clone()));
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: vec!["settings:read".into()], account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(10)).unwrap());
    let remote: Arc<dyn ServicesHandle> = Arc::new(RemoteServicesHandle::new(rpc.clone()));
    Machine { svc, remote, rpc, notes, _on: on, dir: d }
}

#[test]
fn plain_methods_are_answered_by_the_core_and_a_write_lands_in_its_own_state() {
    let m = machine("plain");
    let (h, svc) = (&m.remote, &m.svc);
    assert!(h.is_real(), "the real core answers");
    // a getter: the same answer the services give in the process
    assert_eq!(h.show_all_ai(), svc.show_all_ai());
    assert_eq!(h.system_slots(), svc.system_slots());
    assert_eq!(h.web_access_port(), svc.web_access_port());
    assert_eq!(h.sound_settings(), svc.sound_settings());
    assert_eq!(h.voice_settings(), svc.voice_settings());
    // a setter: the change is in the core's own settings, and a read after it sees it
    h.set_show_all_ai(!svc.show_all_ai()).unwrap();
    assert_eq!(h.show_all_ai(), svc.show_all_ai());
    h.set_announce_name(Some(false)).unwrap();
    assert!(!svc.announce_name_on() && !h.announce_name_on());
    // borrowed arguments cross as owned ones: a slice of names, a path, an option
    assert_eq!(h.search(&["user/ann"], "quokkas", 3), svc.search(&["user/ann"], "quokkas", 3));
    assert_eq!(h.sftp_status_lines(&m.dir), ServicesHandle::sftp_status_lines(&**svc, &m.dir));
    assert_eq!(h.page_snapshot_of("ann"), None);
    // an answer with a path, and an error of the core that comes back as the core's own text
    assert_eq!(h.session_workdir("ann", "s1"), svc.session_workdir("ann", "s1"));
    let err = h.lemonade_health("no-such-provider").unwrap_err();
    assert_eq!(err, ServicesHandle::lemonade_health(&**svc, "no-such-provider").unwrap_err());
    // a call that carries an audit callback in the trait carries none on the wire: the argument list is the data only
    let bare: Result<bool, String> = m.rpc.call("ServicesHandle", 0, "msg_mirror_tick", serde_json::json!(["ann"]));
    assert!(bare.is_ok(), "the data arguments are enough: {bare:?}");
    let with_callback: Result<bool, String> = m.rpc.call("ServicesHandle", 0, "msg_mirror_tick", serde_json::json!(["ann", "an audit callback"]));
    assert!(with_callback.unwrap_err().contains("arguments are wrong"), "a callback of the UI is refused, the core uses its own");
    assert!(h.msg_mirror_tick("ann", Arc::new(|_, _| panic!("a callback of the UI never runs"))) == svc_msg_tick(svc), "the served method is the real one");
    // a method that is not on the wire is refused by name, not guessed
    let unknown: Result<bool, String> = m.rpc.call("ServicesHandle", 0, "lock_the_world", serde_json::json!([]));
    assert!(unknown.unwrap_err().contains("no call lock_the_world"));
}

fn svc_msg_tick(svc: &Arc<iyakki_agent::services::Services>) -> bool {
    let audit: iyakki_shared::gitkeys::AuditFn = Arc::new(|_, _| {});
    iyakki_agent_api::ServicesB1::msg_mirror_tick(svc, "ann", audit)
}

#[test]
fn handles_of_generated_traits_are_remote_objects_and_the_stores_are_roots_of_the_same_core() {
    let m = machine("handles");
    let (h, svc) = (&m.remote, &m.svc);
    // a root the core already serves: the inbox of the core
    svc.inbox.add("ann", Kind::Agent, "Hello", "body", "test", None, 100).unwrap();
    let listed = h.inbox().list("ann");
    assert_eq!((listed.len(), listed[0].title.as_str()), (1, "Hello"));
    h.inbox().add("ann", Kind::Agent, "Second", "b", "test", None, 101).unwrap();
    assert_eq!(svc.inbox.list("ann").len(), 2, "a write through the accessor is in the core's store");
    // settings and the knowledge base as the interfaces that cross the socket
    assert_eq!(h.settings_api().snapshot().settings, svc.settings.lock().unwrap().get().clone());
    h.kb().add_document("user/ann", "t", "t.txt", "a note about quokkas", Default::default()).unwrap();
    assert_eq!(svc.kb.lock().unwrap().docs("user/ann").len(), 1);
    // a handle returned by a call of the ServicesHandle object: a new remote object per call, released on drop
    let kit = h.rag_kit("ann");
    assert_eq!(kit.runs().len(), ServicesHandle::rag_kit(&**svc, "ann").runs().len());
    drop(kit);
    assert!(h.contract_local().settings().is_none(), "the borrowed accessors of a facet keep the default of its trait");
    // the MCP list is the core's: a server added in the core is listed over the socket
    assert_eq!(h.mcp().list().len(), svc.mcp().list().len());
}

#[test]
fn borrowed_answers_are_kept_and_closures_that_change_a_value_write_the_whole_value() {
    let m = machine("kept");
    let (h, svc) = (&m.remote, &m.svc);
    assert_eq!(h.dir(), svc.dir(), "the data folder is read once and kept");
    assert_eq!(h.dir(), svc.dir());
    assert_eq!(h.cloud_stream_prompt_rule(), svc.cloud_stream_prompt_rule());
    assert_eq!(h.fleet_prompt_rule(), svc.fleet_prompt_rule());
    assert!(!h.secrets().system_unlocked(), "the plain secrets never cross the socket");
    assert!(h.secrets_api().has_ref("vault:nothing") == svc.secrets.has_ref("vault:nothing"));
    // a closure that changes a value: the copy is changed here, the whole value is written there, and the core answers what it kept
    let before = svc.sound_settings();
    let after = h.update_sound(|s| s.muted = !before.muted).unwrap();
    assert_eq!(after.muted, !before.muted);
    assert_eq!(svc.sound_settings().muted, !before.muted, "the change is in the core's own settings");
    let sounds = h.update_system_sounds(|s| s.enabled = !s.enabled).unwrap();
    assert_eq!(svc.system_sounds(), sounds);
    // the rows with a `&'static str` come back with the text interned on this side
    let rows = h.router_plan_rows("ann");
    assert_eq!(rows.len(), svc.router_plan_rows("ann").len());
}

/// F-SPLIT-03 part 2 (X3): the accessors of the traits generated after part 1 are remote objects of the same core, including the ones whose
/// local form takes an audit callback (it stays on the core's side).
#[test]
fn the_accessors_of_the_traits_generated_in_part_two_answer_what_the_core_holds() {
    let m = machine("part2");
    let (h, svc) = (&m.remote, &m.svc);
    assert_eq!(h.sessions().list("ann"), ServicesHandle::sessions(&**svc).list("ann"));
    assert_eq!(h.skills().list("ann").len(), ServicesHandle::skills(&**svc).list("ann").len());
    assert_eq!(h.runs().list("ann").len(), ServicesHandle::runs(&**svc).list("ann").len());
    assert_eq!(h.jobs().list("ann").len(), ServicesHandle::jobs(&**svc).list("ann").len());
    // a session made through the accessor is the core's; saving takes a `&mut`: the value is sent, saved by the core and sent back as the core changed it
    let mut made = h.sessions().create("ann").unwrap();
    assert_eq!(made.owner, "ann");
    assert!(!made.id.is_empty());
    made.messages.push(iyakki_shared::chat::Message::user("remember this"));
    h.sessions().save(&mut made).unwrap();
    let kept = ServicesHandle::sessions(&**svc).load("ann", &made.id).expect("the core kept the session the program saved");
    assert_eq!(kept.messages.len(), 1, "the change made in the program is in the core's store");
    assert_eq!(kept, made, "and the copy in the program is the one the core holds");
    let lost = RemoteServicesHandle::new(Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink()))));
    let mut mine = made.clone();
    assert!(lost.sessions().save(&mut mine).is_err() && mine == made, "a lost core saves nothing and changes nothing");
    // an accessor with an audit callback: the callback of the UI is not sent, the core passes its own
    let dom = h.domains_facet("ann", Arc::new(|_, _| panic!("a callback of the UI never runs")));
    assert_eq!(dom.status().enabled, ServicesHandle::domains_facet(&**svc, "ann", Arc::new(|_, _| {})).status().enabled);
    // the radio meter is an array of numbers
    assert_eq!(h.radio().levels().len(), ServicesHandle::radio(&**svc).levels().len());
    assert!(h.is_real());
}

#[test]
fn what_is_not_served_answers_not_served_and_never_a_success() {
    let m = machine("quiet");
    let h = &m.remote;
    // the cue output is served now (D-SPLIT-03-SERVED): it answers what the core's own output answers
    assert_eq!(h.cue_out().ready(), ServicesHandle::cue_out(&*m.svc).ready());
    assert!(h.wake_spawn(None).is_err());
    assert!(h.agent_vault_token("ann", "x").is_none(), "an agent's vault token never crosses");
    assert!(h.streamer().is_none());
    assert!(h.flows_jobs("ann", &iyakki_sec::policy::Role::Guest.template()).is_none(), "a socket without the flows scopes gets no flows objects");
    // the real core is still answering after all of these
    assert!(h.is_real());
}

#[test]
fn the_audit_of_a_call_is_the_cores_own_and_never_the_uis() {
    let m = machine("audit");
    // `install_bt_remote` is served with its data (the user) only; whatever the core writes goes to the core's audit log under the UI's name
    m.remote.install_bt_remote("ann", Arc::new(|_, _| panic!("a callback of the UI never runs")));
    for (user, _, _) in m.notes.0.lock().unwrap().iter() {
        assert_eq!(user, "iyakki-ui", "the lines are written by the core under the UI program's name");
    }
}

/// D-SPLIT-03-ENV: a call that took a UI object (the operating system handle, a gate, an audit callback, the login closures) runs over the
/// core's own, and the UI's object never crosses.
#[test]
fn calls_that_took_ui_objects_run_over_the_cores_own() {
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    let (os2, dir) = (os.clone(), std::env::temp_dir().join(format!("iyakki-env-users-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let users = iyakki_accounts::UsersRef::new(Arc::new(Arc::new(Mutex::new(iyakki_accounts::UserStore::open(&dir.join("users.json")).unwrap()))));
    let m = machine_with("env", move |r| r.with_env(os2, users));
    let h = &m.remote;
    // the fleet card: the UI sends nothing but the data; the core's own gate was asked to read, and refuses when the core's gate refuses
    let status = h.fleet_view(Arc::new(iyakki_core_fake::FakeOs::new()), Arc::new(Never), Arc::new(|_, _| panic!("a callback of the UI never runs"))).unwrap();
    assert!(!status.my_fingerprint.is_empty());
    assert!(os.fleet_fake().calls().iter().any(|c| c.contains("fleet")), "the core's own gate was asked: {:?}", os.fleet_fake().calls());
    os.fleet_fake().refuse(true);
    assert!(h.fleet_view(Arc::new(iyakki_core_fake::FakeOs::new()), Arc::new(Never), Arc::new(|_, _| {})).is_err(), "the core's gate decides, not the UI's");
    let act = h.fleet_act(Arc::new(iyakki_core_fake::FakeOs::new()), Arc::new(Never), Arc::new(|_, _| {}), iyakki_shared::fleetview::FleetAct::Tls(true));
    assert!(act.is_err() && !act.unwrap_err().contains("not served"), "a fleet change goes to the core's gate");
    os.fleet_fake().refuse(false);
    // handles: the router panel and the SFTP server are remote objects of the core's own
    let panel = h.router_panel("ann", Arc::new(|_, _| panic!("a callback of the UI never runs")), iyakki_core_fake::FakeOs::new().router_fake());
    assert_eq!(panel.config(), ServicesHandle::router_panel(&*m.svc, "ann", Arc::new(|_, _| {}), os.router_fake()).config());
    let sftp = h.sftp_server(&m.dir, Arc::new(|_, _| panic!("a login of the UI never runs")), Arc::new(|_| panic!("a lookup of the UI never runs")), Arc::new(|_, _| {}));
    assert_eq!(sftp.dir(), ServicesHandle::sftp_server(&*m.svc, &m.dir, Arc::new(|_, _| Err("no".into())), Arc::new(|_| None), Arc::new(|_, _| {})).dir());
    assert!(m.rpc.call::<bool>("ServicesHandle", 0, "sftp_server", serde_json::json!([m.dir.to_string_lossy(), "a login"])).is_err(), "a closure cannot ride along");
    assert!(h.is_real());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A gate that a test would notice if it were used.
struct Never;
impl Fleet for Never {
    fn read(&self, _: &str) -> Result<(), String> { panic!("the UI's gate never runs") }
    fn join(&self, _: &str) -> Result<(), String> { panic!("the UI's gate never runs") }
    fn manage(&self, _: &str) -> Result<(), String> { panic!("the UI's gate never runs") }
    fn assign(&self, _: &str) -> Result<(), String> { panic!("the UI's gate never runs") }
    fn receive(&self, _: &str) -> Result<(), String> { panic!("the UI's gate never runs") }
    fn record(&self, _: &str, _: &str) { panic!("the UI's gate never runs") }
}
use iyakki_core_api::fleet::Fleet;

/// Without the core's own handle those calls answer "not served over the UI socket", never a made-up success.
#[test]
fn without_the_cores_own_handle_the_env_calls_answer_not_served() {
    let m = machine("noenv");
    let h = &m.remote;
    let e = h.fleet_view(Arc::new(iyakki_core_fake::FakeOs::new()), Arc::new(Never), Arc::new(|_, _| {})).unwrap_err();
    assert!(e.contains("not served over the UI socket"), "{e}");
    assert!(h.fleet_act(Arc::new(iyakki_core_fake::FakeOs::new()), Arc::new(Never), Arc::new(|_, _| {}), iyakki_shared::fleetview::FleetAct::Tls(true)).is_err());
    assert!(h.router_panel("ann", Arc::new(|_, _| {}), iyakki_core_fake::FakeOs::new().router_fake()).config() == Default::default(), "the quiet panel");
    assert!(h.is_real());
}

/// The SFTP login check the core builds is its own accounts: a right password gives the account's policy, a wrong one is refused, and the
/// key lookup names the policy of an account that exists only.
#[test]
fn the_sftp_login_the_core_builds_is_its_own_accounts() {
    let dir = std::env::temp_dir().join(format!("iyakki-env-sftp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(Mutex::new(iyakki_accounts::UserStore::open(&dir.join("users.json")).unwrap()));
    store.lock().unwrap().create_first_admin("root", "rootpass12").unwrap();
    let env = iyakki_ui_rpc::CoreEnv::new(Arc::new(iyakki_core_fake::FakeOs::new()), iyakki_accounts::UsersRef::new(Arc::new(store)));
    let (login, lookup) = (env.sftp_login(), env.sftp_lookup());
    assert!(login("root", "rootpass12").is_ok());
    assert!(login("root", "wrong-pass").is_err() && login("nobody", "x").is_err());
    assert!(lookup("root").is_some() && lookup("nobody").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_file_scope_is_asked_of_the_core_and_answers_as_the_cores_own_rules_do() {
    // the policy of a call that names a person is the core's reading of the account (D-SEC1-VOUCH): the account "ann" is a plain user here
    let adir = std::env::temp_dir().join(format!("iyakki-scope-accounts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&adir);
    std::fs::create_dir_all(&adir).unwrap();
    let mut store = iyakki_accounts::UserStore::open(&adir.join("users.json")).unwrap();
    store.add_local("ann", "annpass12345", iyakki_sec::policy::Role::User).unwrap();
    let accounts: Arc<dyn iyakki_agent_api::AccountsApi> = Arc::new(Arc::new(Mutex::new(store)));
    let m = machine_with("scope", move |r| r.with_accounts(accounts));
    // the person signs in over the socket: the calls that name an account are held to it (alpha.8 SEC3)
    let session: serde_json::Value = m.rpc.call("AccountsApi", 0, "authenticate", serde_json::json!(["ann", "annpass12345"])).unwrap();
    assert!(session.get("Ok").is_some(), "{session}");
    let (h, svc) = (&m.remote, &m.svc);
    let policy = iyakki_sec::policy::Role::User.template();
    let data = m.dir.join("svc");
    let real = |f: &dyn Fn(&dyn ServicesHandle) -> String| (f(&**h), f(&**svc));
    // every answer is the one the core's own rules give
    let (a, b) = real(&|s| format!("{:?}", s.scope_home_root("ann", &data)));
    assert_eq!(a, b);
    let (a, b) = real(&|s| format!("{:?}", s.scope_home_root_for_writes("ann", &data)));
    assert_eq!(a, b);
    let (a, b) = real(&|s| format!("{:?}", s.scope_roots(&policy, "ann", &data)));
    assert_eq!(a, b);
    let (a, b) = real(&|s| format!("{:?}", s.scope_roots_with_home(&policy, "ann", &data, data.join("elsewhere"))));
    assert_eq!(a, b);
    let (a, b) = real(&|s| format!("{:?}", s.scope_system_home("ann")));
    assert_eq!(a, b);
    // a path outside the person's reach is refused with the core's sentence, over the socket as in the process
    let shadow = std::path::Path::new("/etc/shadow");
    let (a, b) = real(&|s| format!("{:?}", s.scope_check_read(&policy, "ann", shadow)));
    assert_eq!(a, b);
    assert!(h.scope_check_read(&policy, "ann", shadow).is_err() && h.scope_check_write(&policy, "ann", shadow).is_err());
    // paths are resolved by the core, and a folder the program asks for is made by the core
    let (a, b) = real(&|s| format!("{:?}", s.scope_resolve(&m.dir, "svc/../svc")));
    assert_eq!(a, b);
    let (a, b) = real(&|s| format!("{:?}", s.scope_resolve_for_write(&m.dir, "svc/new-file.txt")));
    assert_eq!(a, b);
    let made = m.dir.join("made-by-the-core").join("file.txt");
    h.scope_ensure_parent(&made).unwrap();
    assert!(made.parent().unwrap().is_dir(), "the folder is in the core's own file system");
    // a guest-as-administrator policy the UI names is not the account's: the answer is the one for the account's own policy
    let forged = iyakki_sec::policy::Role::Admin.template();
    assert_eq!(format!("{:?}", h.scope_roots(&forged, "ann", &data)), format!("{:?}", svc.scope_roots(&policy, "ann", &data)));
    assert!(h.scope_check_read(&forged, "ann", shadow).is_err(), "the claimed administrator policy does not open /etc/shadow");
    let _ = std::fs::remove_dir_all(&adir);
}
