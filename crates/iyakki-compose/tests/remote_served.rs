//! The accessors of `ServicesHandle` that took objects of the UI, served over the real UI socket against the real services (F-SPLIT-03 part 3,
//! D-SPLIT-03-SERVED): the core passes its own audit log, accounts, host and file rules; the UI sends data only. Linux only: a unix socket.
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_agent_api::{AuditApi, ServicesHandle};
use iyakki_compose::rpc::RpcRoots;
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

/// The host of the machine: it reports and does nothing to the machine.
struct TestHost;
impl iyakki_core_api::HostApi for TestHost {
    fn detect_env(&self) -> String {
        "test machine".into()
    }
    fn net_status(&self) -> String {
        "not reported".into()
    }
    fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
        Err("no programs here".into())
    }
    fn power_off(&self) {
        panic!("a terminal over the socket never switches the machine off")
    }
    fn reboot(&self) {
        panic!("a terminal over the socket never restarts the machine")
    }
    fn can_power(&self) -> bool {
        false
    }
}

struct Machine {
    svc: Arc<iyakki_agent::services::Services>,
    remote: Arc<dyn ServicesHandle>,
    rpc: Rpc,
    #[allow(dead_code)] // kept alive with the machine: the notes store outlives the served handles
    notes: Arc<Notes>,
    users: Arc<Mutex<iyakki_accounts::UserStore>>,
    _on: iyakki_agent::contract_local::uiserve::UiServer,
    dir: std::path::PathBuf,
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A core with accounts (`ann`, a normal user), a gate and the file rules of the machine.
fn machine(tag: &str) -> Machine {
    machine_with(tag, Arc::new(Dead), Arc::new(Dead))
}

fn machine_with(tag: &str, quick: Arc<dyn iyakki_proxy::Upstream>, slow: Arc<dyn iyakki_proxy::Upstream>) -> Machine {
    machine_scoped(tag, quick, slow, &["settings:read", "flows:read"])
}

/// The same machine with the scopes the socket holds named.
fn machine_scoped(tag: &str, quick: Arc<dyn iyakki_proxy::Upstream>, slow: Arc<dyn iyakki_proxy::Upstream>, scopes: &[&str]) -> Machine {
    iyakki_compose::wire();
    // flows are made in the account's home: without this variable that is the real `$HOME` of the host (a leftover ~/Flows from an earlier run broke the empty-start assertions)
    std::env::set_var("IYAKKI_NO_HOST_HOME", "1"); // flaky-ok: the same constant every test sets, never removed
    let d = iyakki_agent::testdir::short("iyakki-served", tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), quick, slow).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let notes = Arc::new(Notes::default());
    let users = Arc::new(Mutex::new(iyakki_accounts::UserStore::open(&d.join("users.json")).unwrap()));
    users.lock().unwrap().create_first_admin("ann", "annpass12345").unwrap();
    let real: Arc<dyn iyakki_agent_api::AccountsApi> = Arc::new(users.clone());
    let roots = RpcRoots::new().with_audit(notes.clone()).with_accounts(real).with_file_scope(Arc::new(iyakki_agent::corehandle::LocalFileScope)).with_host(Arc::new(TestHost));
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: scopes.iter().map(|s| s.to_string()).collect(), account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(10)).unwrap());
    let remote: Arc<dyn ServicesHandle> = Arc::new(RemoteServicesHandle::new(rpc.clone()));
    Machine { svc, remote, rpc, notes, users, _on: on, dir: d }
}

fn ui_audit() -> Arc<dyn Fn(&str, &str) + Send + Sync> {
    Arc::new(|_, _| panic!("the audit callback of the UI never runs"))
}

#[test]
fn the_five_store_facets_are_five_remote_objects_behind_one_handle_and_answer_what_the_core_holds() {
    let m = machine("store");
    let (h, svc) = (&m.remote, &m.svc);
    let (remote, real) = (h.store_facets(), ServicesHandle::store_facets(&**svc));
    assert_eq!(remote.components_channel(), real.components_channel());
    assert_eq!(remote.wasm_tiles_enabled(), real.wasm_tiles_enabled());
    assert_eq!(remote.stick_stores(), real.stick_stores());
    assert_eq!(remote.update_announce_stores(), real.update_announce_stores());
    assert_eq!(remote.permission_words("tile:storage", "ann"), real.permission_words("tile:storage", "ann"));
    assert_eq!(remote.components_check_lines(), real.components_check_lines());
    assert_eq!(remote.private_store_status("ann"), real.private_store_status("ann"));
    // the handle of the drivers pack is a sixth remote object of the same core, and an unreadable folder is the core's refusal, not a made-up pack
    let err = remote.drivers().folder_package("x", std::path::Path::new("/nonexistent-folder-for-test"), 0).unwrap_err();
    assert_eq!(err, real.drivers().folder_package("x", std::path::Path::new("/nonexistent-folder-for-test"), 0).unwrap_err());
    // a call the core refuses comes back as the core's own words
    assert_eq!(remote.components_set_channel("no-such-channel").unwrap_err(), real.components_set_channel("no-such-channel").unwrap_err());
    assert!(h.is_real());
}

#[test]
fn the_remote_files_manager_lists_connections_of_the_core_and_judges_paths_by_the_cores_file_rules() {
    let m = machine("rkit");
    let h = &m.remote;
    let never_local: Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync> = Arc::new(|_, _| panic!("the file rule of the UI never runs"));
    let kit = h.remote_kit("ann", std::path::Path::new("/a/path/of/the/ui"), "/tmp", never_local, ui_audit());
    use iyakki_ui_rpc::remote_files::{Auth, Conn, JobResult, Proto};
    assert!(kit.connections().is_empty());
    let c = Conn { name: "box".into(), host: "192.0.2.7".into(), port: 22, user: "ann".into(), proto: Proto::Sftp, auth: Auth::Password, key: String::new(), plain_ok: false, has_password: false };
    // a save is the core's own: the connection is listed afterwards, by the same core, and removing it takes it out again
    kit.save(&c, None, None).unwrap();
    let real_names: Vec<String> = kit.connections().into_iter().map(|c| c.name).collect();
    assert_eq!(real_names, vec!["box".to_string()]);
    kit.remove("box").unwrap();
    assert!(kit.connections().is_empty());
    // the ticket of a job that never existed is not "not yet": the core says so, and a lost core would say failed
    match kit.take(999_999) {
        None | Some(JobResult::Failed(_)) => {}
        other => panic!("a job nobody submitted must not look done: {other:?}"),
    }
    assert!(!kit.download_dir().is_empty());
    // a person that is not an account of this machine gets nothing
    let nobody = h.remote_kit("nobody", std::path::Path::new("/x"), "/tmp", Arc::new(|_, _| Err("x".into())), ui_audit());
    assert!(nobody.connections().is_empty());
    assert!(nobody.save(&c, None, None).is_err(), "no account, no manager: the quiet refusal");
    assert!(h.is_real());
    let _ = &m.users;
}

#[test]
fn a_core_without_the_parts_refuses_by_name_and_the_quiet_object_answers_nothing() {
    iyakki_compose::wire();
    let d = iyakki_agent::testdir::short("iyakki-served", "bare");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let roots = RpcRoots::new();
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: vec!["settings:read".into()], account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(10)).unwrap());
    let r: Result<u64, String> = rpc.call("ServicesHandle", 0, "remote_kit", serde_json::json!(["ann", "/tmp"]));
    assert!(r.unwrap_err().contains("no file rules"));
    let r: Result<u64, String> = rpc.call("ServicesHandle", 0, "terminal", serde_json::json!(["ann"]));
    assert!(r.unwrap_err().contains("no accounts"));
    let r: Result<u64, String> = rpc.call("ServicesHandle", 0, "core_terminal", serde_json::json!([]));
    assert!(r.unwrap_err().contains("no core terminal"));
    // the quiet objects answer nothing and make nothing up
    let h = RemoteServicesHandle::new(rpc);
    assert!(h.is_real(), "the core answers; only the parts it was not given are refused");
    assert!(h.remote_kit("ann", std::path::Path::new("/x"), "/tmp", Arc::new(|_, _| Err("x".into())), Arc::new(|_, _| {})).connections().is_empty());
    drop(on);
    let _ = std::fs::remove_dir_all(&d);
}

/// An upstream that answers with what it was sent, doubled, and says which method and address it saw; it keeps the length of the last body.
struct Echo(&'static str, Mutex<usize>);
impl iyakki_proxy::Upstream for Echo {
    fn request(&self, method: &str, url: &str, _: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        *self.1.lock().unwrap() = body.len();
        let mut out = format!("{}:{method}:{url}:", self.0).into_bytes();
        out.extend_from_slice(body);
        out.extend_from_slice(body);
        Ok(iyakki_proxy::UpstreamResponse { status: 207, body: out })
    }
    fn request_full(&self, method: &str, url: &str, h: &[(String, String)], body: &[u8]) -> Result<(iyakki_proxy::UpstreamResponse, Vec<(String, String)>), String> {
        self.request(method, url, h, body).map(|r| (r, vec![("x-seen".into(), self.0.into())]))
    }
}

#[test]
fn the_two_upstreams_run_in_the_core_and_a_body_bigger_than_a_line_travels_in_chunks_both_ways() {
    let (quick, slow) = (Arc::new(Echo("quick", Mutex::new(0))), Arc::new(Echo("slow", Mutex::new(0))));
    struct Hold(Arc<Echo>);
    impl iyakki_proxy::Upstream for Hold {
        fn request(&self, m: &str, u: &str, h: &[(String, String)], b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            self.0.request(m, u, h, b)
        }
        fn request_full(&self, m: &str, u: &str, h: &[(String, String)], b: &[u8]) -> Result<(iyakki_proxy::UpstreamResponse, Vec<(String, String)>), String> {
            self.0.request_full(m, u, h, b)
        }
    }
    let m = machine_with("up", Arc::new(Hold(quick.clone())), Arc::new(Hold(slow.clone())));
    let h = &m.remote;
    // a small body goes inline in the line
    let r = h.quick_upstream().request("POST", "http://a.test/x", &[], b"hello").unwrap();
    assert_eq!((r.status, r.body), (207, b"quick:POST:http://a.test/x:hellohello".to_vec()));
    // a body of 100 kB is an upload of three chunks; the answer of 200 kB is read back in six; both are byte for byte what they were
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let (r, headers) = h.slow_upstream().request_full("PUT", "http://b.test/", &[("a".into(), "b".into())], &big).unwrap();
    assert_eq!(*slow.1.lock().unwrap(), big.len(), "the core's upstream saw the whole body");
    assert_eq!(r.status, 207);
    let head = b"slow:PUT:http://b.test/:".to_vec();
    assert_eq!(&r.body[..head.len()], &head[..]);
    assert_eq!(r.body.len(), head.len() + 2 * big.len());
    assert_eq!(&r.body[head.len()..head.len() + big.len()], &big[..]);
    assert_eq!(headers, vec![("x-seen".to_string(), "slow".to_string())]);
    assert_eq!(*quick.1.lock().unwrap(), 5, "the other upstream was not used");
    // the uploads and downloads are freed when the call is over
    assert!(h.is_real());
}

/// A transport that streams a made-up body of any length, in whatever sizes it is asked for.
struct Endless;
impl iyakki_proxy::Streamer for Endless {
    fn open(&self, url: &str, _: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        let n: usize = url.rsplit('/').next().and_then(|s| s.parse().ok()).ok_or("the address ends with a length")?;
        Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![("content-type".into(), "audio/test".into())], body: Box::new(std::io::Read::take(std::io::repeat(7u8), n as u64)) })
    }
    fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
        Ok(vec![host.as_bytes().to_vec(), vec![port as u8; 3]])
    }
}

#[test]
fn the_streamer_of_the_core_streams_a_body_to_the_ui_a_chunk_at_a_time() {
    let m = machine("stream");
    let h = &m.remote;
    assert!(h.streamer().is_none() == m.svc.streamer().is_none(), "the core has the streamer it has; the UI sees the same");
    m.svc.set_streamer(Arc::new(Endless));
    let s = h.streamer().expect("the core now has one");
    let mut head = s.open("http://radio.test/200000", &[]).unwrap();
    assert_eq!((head.status, head.header("content-type")), (200, Some("audio/test")));
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut head.body, &mut got).unwrap();
    assert_eq!(got.len(), 200_000);
    assert!(got.iter().all(|b| *b == 7));
    assert!(s.open("http://radio.test/not-a-number", &[]).err().unwrap().contains("ends with a length"), "the core's own refusal comes back");
    assert_eq!(s.peer_chain("example.test", 443).unwrap(), vec![b"example.test".to_vec(), vec![187u8; 3]]);
    // the plain GET of a transport that cannot POST says so in the transport's words
    assert!(s.post("http://radio.test/1", &[], b"x").err().unwrap().contains("cannot stream a POST"));
}

#[test]
fn the_cue_output_the_synth_and_the_lesson_host_answer_what_the_core_has() {
    let m = machine("hosts");
    let (h, svc) = (&m.remote, &m.svc);
    // a cue: whether there is an output is the core's answer; a sound of 100 kB goes up in chunks and the core's own refusal (or its play) comes back
    assert_eq!(h.cue_out().ready(), ServicesHandle::cue_out(&**svc).ready());
    let wav = vec![0u8; 100_000];
    assert_eq!(h.cue_out().play(&wav).is_ok(), ServicesHandle::cue_out(&**svc).play(&wav).is_ok());
    // the reading synth: the same answer to the same text
    let opts = iyakki_shared::voice::SpeakOpts::default();
    let (rs, ls) = (h.reading_synth(opts.clone()), ServicesHandle::reading_synth(&**svc, opts));
    assert_eq!(rs.synth("hello").is_ok(), ls.synth("hello").is_ok());
    assert_eq!(rs.align("hello", &[0u8; 10]), ls.align("hello", &[0u8; 10]));
    // the lesson writer's host: the study folder is the core's, the picture model is the core's
    let (rl, ll) = (h.lesson_host(), ServicesHandle::lesson_host(&**svc));
    assert_eq!(rl.study().dir, ll.study().dir);
    assert_eq!(rl.has_image_model(), ll.has_image_model());
    assert_eq!(rl.generate_image("a cat", "64x64").is_ok(), ll.generate_image("a cat", "64x64").is_ok());
    assert!(h.is_real());
}

#[test]
fn the_dashboards_host_reads_the_cores_folders_and_vault_names_but_never_a_secret_or_a_program_and_only_reads_the_cluster() {
    let m = machine("dash");
    let (h, svc) = (&m.remote, &m.svc);
    let (rd, ld) = (h.dash_host(), ServicesHandle::dash_host(&**svc));
    assert_eq!(rd.data_dir(), ld.data_dir());
    assert_eq!(rd.home_root("ann"), ld.home_root("ann"));
    assert_eq!(rd.vault_names("ann"), ld.vault_names("ann"));
    assert!(rd.now_unix().abs_diff(ld.now_unix()) < 100, "the core's clock");
    let guest = iyakki_sec::policy::Role::Guest.template();
    let p = std::path::Path::new("/etc/hostname");
    assert_eq!(rd.read_ok(&guest, "ann", p), ld.read_ok(&guest, "ann", p), "the core judges the file rule");
    // a secret is never resolved for the UI
    assert!(rd.resolve_secret("ann", "vault:anything").unwrap_err().contains("not served over the UI socket"));
    // no program is run for the UI
    let out = rd.runner().run(std::path::Path::new("/"), &["true".to_string()], &[], Duration::from_secs(1), 100, None);
    assert!(out.missing && !rd.runner().has("true"));
    // the cluster: reads only, checked in the UI program and again in the core
    let k = rd.kubectl();
    for bad in [vec!["delete", "pod", "x"], vec!["get", "pods", "--kubeconfig=/x"], vec!["--server", "http://x", "get", "pods"], vec!["exec", "x"], vec!["apply", "-f", "-"]] {
        let e = k.kubectl(&bad).unwrap_err();
        assert!(e.contains("not served over the UI socket"), "{bad:?}: {e}");
    }
    // the core's own check, not only the client's: a call straight at the object is refused as well
    let id: u64 = m.rpc.call("ServicesHandle", 0, "dash_host", serde_json::json!([])).unwrap();
    let r: Result<Result<String, String>, String> = m.rpc.call("DashHost", id, "kubectl", serde_json::json!([["delete", "pod", "x"]]));
    assert!(r.unwrap_err().contains("may only read"));
    // an allowed read is the core's own run: whatever the core's kubectl answers (no cluster on a test machine) is what the UI gets
    assert_eq!(k.kubectl(&["version"]).is_ok(), ld.kubectl().kubectl(&["version"]).is_ok());
}

#[test]
fn the_book_of_one_time_codes_shows_the_ui_no_secret_computes_the_codes_in_the_core_and_keeps_a_hidden_secret_on_save() {
    use iyakki_shared::authcodes::{Book, NewEntry, VaultProblem};
    use iyakki_shared::totp::Algo;
    let m = machine("codes");
    let (h, svc) = (&m.remote, &m.svc);
    // locked before the person signs in
    assert_eq!(h.code_vault("ann").load().unwrap_err(), VaultProblem::Locked);
    svc.on_login("ann", "annpass12345", true);
    let real = ServicesHandle::code_vault(&**svc, "ann");
    let mut book = Book::default();
    let id = book.add(&NewEntry::typed("Mail", "ann", "JBSWY3DPEHPK3PXP", (Algo::Sha1, 6, 30), "Personal")).unwrap();
    real.save(&book).unwrap();
    let remote = h.code_vault("ann");
    let seen = remote.load().unwrap();
    assert_eq!(seen.entries.len(), 1);
    assert_eq!((seen.entries[0].issuer.as_str(), seen.entries[0].secret.as_str()), ("Mail", "*"), "the name crosses, the secret does not");
    // the core computes the code from the secret it holds; the UI cannot, and the in-process vault says it computes itself
    let at = 1_700_000_000;
    let want = real.load().unwrap().entries[0].code_at(at).unwrap();
    assert_eq!(remote.codes_now(at), Some(vec![(id, want)]));
    assert_eq!(real.codes_now(at), None);
    // a save that carries the hidden marker keeps the core's secret and takes the other changes
    let mut edited = seen.clone();
    edited.entries[0].group = "Work".into();
    edited.groups.push("Work".into());
    remote.save(&edited).unwrap();
    let after = real.load().unwrap();
    assert_eq!((after.entries[0].group.as_str(), after.entries[0].secret.as_str()), ("Work", "JBSWY3DPEHPK3PXP"));
    // a secret the person typed is written; a hidden marker for an entry the core does not know is refused
    let mut with_new = edited.clone();
    let mut typed = with_new.entries[0].clone();
    typed.id += 7;
    typed.secret = "MFRGGZDFMZTWQ2LK".into();
    with_new.entries.push(typed);
    remote.save(&with_new).unwrap();
    assert_eq!(real.load().unwrap().entries.len(), 2);
    let mut ghost = edited.clone();
    let mut e = ghost.entries[0].clone();
    e.id += 99;
    ghost.entries.push(e);
    assert!(matches!(remote.save(&ghost), Err(VaultProblem::Other(_))));
    assert_eq!(real.load().unwrap().entries.len(), 2, "nothing was written by the refused save");
    // locked again: the UI is told, nothing is shown
    svc.lock_user_vault("ann");
    assert_eq!(remote.load().unwrap_err(), VaultProblem::Locked);
    assert_eq!(remote.codes_now(at), None);
}

#[test]
fn the_line_terminal_uses_the_accounts_own_policy_and_the_cores_host_whatever_the_ui_names() {
    let m = machine("term");
    let (h, svc) = (&m.remote, &m.svc);
    let admin = iyakki_sec::policy::Role::Admin.template();
    let guest = iyakki_sec::policy::Role::Guest.template();
    let real = ServicesHandle::terminal(&**svc, "ann", admin.clone(), Arc::new(TestHost));
    // the UI names a guest's policy (and a host that panics when used); the core reads ann's own policy
    struct NeverHost;
    impl iyakki_core_api::HostApi for NeverHost {
        fn detect_env(&self) -> String {
            panic!("the host of the UI is never used")
        }
        fn net_status(&self) -> String {
            panic!("the host of the UI is never used")
        }
        fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
            panic!("the host of the UI is never used")
        }
        fn power_off(&self) {}
        fn reboot(&self) {}
        fn can_power(&self) -> bool {
            false
        }
    }
    let mut remote = h.terminal("ann", guest, Arc::new(NeverHost));
    let mut real = real;
    assert_eq!(remote.prompt(), real.prompt());
    assert_eq!(remote.full(), real.full(), "ann is an administrator: the UI could not make her smaller or bigger");
    assert_eq!(remote.exec("help").text, real.exec("help").text);
    // an account the machine does not have gets a terminal that answers nothing and runs nothing
    let mut nobody = h.terminal("nobody", admin, Arc::new(TestHost));
    assert!(!nobody.full());
    assert!(nobody.exec("help").text.contains("no Terminal object"), "the quiet terminal says there is none, it runs nothing");
    // the core's own terminal of the contract, over the machine's accounts
    use iyakki_core_api::Terminal as _;
    let t = RemoteServicesHandle::new(m.rpc.clone()).core_terminal();
    let open = t.open("ann", "sample").unwrap();
    assert!(!open.id.is_empty());
    let reply = t.run("ann", &open.id, "help").unwrap();
    assert!(!reply.text.is_empty());
    assert!(t.open("nobody", "sample").is_err(), "an account the machine does not have has no terminal");
    t.close("ann", &open.id);
}

#[test]
fn the_git_keys_of_an_account_are_made_in_the_cores_vault_and_the_ui_can_never_read_a_key_back() {
    use iyakki_shared::gitkeys::Session;
    let m = machine("git");
    let (h, svc) = (&m.remote, &m.svc);
    svc.on_login("ann", "annpass12345", true);
    let ui_lines = Arc::new(Mutex::new(Vec::new()));
    let u2 = ui_lines.clone();
    let fresh = || Arc::new(Session { pass: Default::default(), approved: Default::default() });
    let keys = h.git_keys("ann", Arc::new(move |a, d| u2.lock().unwrap().push(format!("{a} {d}"))), fresh());
    // a key made through the UI's GitKeys is in the core's own vault: the core's GitKeys lists it with the same fingerprint
    let made = keys.generate("work", "ann@test").unwrap();
    let real = ServicesHandle::git_keys(&**svc, "ann", Arc::new(|_, _| {}), fresh());
    assert_eq!(real.keys().iter().map(|k| k.id.clone()).collect::<Vec<_>>(), vec!["work".to_string()]);
    assert_eq!(real.key("work").unwrap().fingerprint, made.fingerprint);
    assert_eq!(keys.keys().iter().map(|k| k.fingerprint.clone()).collect::<Vec<_>>(), vec![made.fingerprint.clone()], "the UI lists from the table the core keeps");
    assert!(real.pair("work").is_ok(), "the core reads its own key");
    assert!(keys.pair("work").is_err(), "the UI is never given the key");
    // the store the UI holds refuses a read on both ends, and refuses names that are not the git keys'
    let obj: u64 = m.rpc.call("ServicesHandle", 0, "git_store", serde_json::json!(["ann"])).unwrap();
    let r: Result<Result<String, String>, String> = m.rpc.call("GitSecretStore", obj, "get", serde_json::json!(["ssh.work"]));
    assert!(r.unwrap_err().contains("never handed to the UI"));
    let r: Result<Result<(), String>, String> = m.rpc.call("GitSecretStore", obj, "set", serde_json::json!(["auth.codes", "x"]));
    assert!(r.unwrap_err().contains("not a name the git keys use"));
    let names: Result<Result<Vec<String>, String>, String> = m.rpc.call("GitSecretStore", obj, "names", serde_json::json!([]));
    assert!(names.unwrap().unwrap().iter().all(|n| n.starts_with("ssh.") || n.starts_with("git.")), "only the git names are listed");
    // a key is removed in the core too
    keys.delete_key("work").unwrap();
    assert!(real.keys().is_empty() && real.pair("work").is_err());
    assert!(h.is_real());
}

#[test]
fn the_flows_jobs_are_six_remote_objects_the_person_is_the_cores_and_each_call_needs_its_scope() {
    let policy = iyakki_sec::policy::Role::Admin.template();
    // a socket that may only look: the objects are served, the reads answer, every change is refused by name before the core is asked
    let m = machine("flows-read");
    let looks = m.remote.flows_jobs("ann", &policy).expect("flows:read serves the flows objects");
    assert!(looks.summaries().is_empty() && looks.run_summaries().is_empty());
    assert_eq!(looks.user(), "ann", "the account is the one the core read from its accounts");
    let spec = serde_json::json!({"name": "Probe", "nodes": [{"id": "say", "kind": "notify", "config": {"title": "hi"}}], "edges": [{"from": "start", "to": "say"}]});
    let flow = iyakki_agent::flows::build_flow(&spec).unwrap();
    let refused = looks.create_draft(flow.clone(), "user:ann");
    assert!(refused.as_ref().is_err_and(|e| e.contains("flows:edit")), "{refused:?}");
    assert!(looks.start_run(iyakki_flow::StartRequest::new("x")).is_err_and(|e| e.contains("flows:run")));
    assert!(looks.hook_enable("h", "user:ann").is_err_and(|e| e.contains("flows:hooks")));
    assert!(looks.allow_program("tool").is_err_and(|e| e.contains("flows:hooks")), "letting a program run is an ask");
    assert!(ServicesHandle::flows_jobs(&*m.svc, "ann", &policy).unwrap().summaries().is_empty(), "nothing was made behind the refusal");
    // a socket with the program's scopes: the whole of the tile works through the six objects against the real environment
    let m = machine_scoped("flows-run", Arc::new(Dead), Arc::new(Dead), &["settings:read", "flows:read", "flows:run", "flows:edit"]);
    let jobs = m.remote.flows_jobs("ann", &policy).expect("served");
    let made = jobs.create_draft(flow, "user:somebody-else").unwrap();
    assert!(jobs.summaries().iter().any(|s| s.id == made.id && s.name == "Probe"));
    // the person who acts is the account the core knows: the claim of the UI is not what is written
    assert_eq!(jobs.who(), "user:ann");
    let v = jobs.publish(&made.id, "user:somebody-else", "", "first", Some("user:somebody-else".into())).unwrap();
    assert_eq!(jobs.versions(&made.id).last().map(|e| (e.version, e.author.clone(), e.approved_by.clone())), Some((v, "user:ann".into(), "user:ann".into())), "the author and the approver are the account's, not the claim");
    let rev = jobs.revision();
    let run = jobs.start_run(iyakki_flow::StartRequest::new(&made.id).trigger("manual", "user:ann")).unwrap();
    // the real environment of the core saw the same run (one environment, six objects)
    let real = ServicesHandle::flows_jobs(&*m.svc, "ann", &policy).unwrap();
    assert!(real.get_run(&run.run_id).is_some(), "the run is the core's");
    assert!(jobs.revision() >= rev);
    assert!(jobs.get_run("nope").is_none());
    assert!(jobs.cancel("nope").is_err());
    // a hook's secret never crosses even when it is asked for; the asks still need their scope
    assert!(jobs.hook_create(&made.id, &Default::default(), "user:ann").is_err_and(|e| e.contains("flows:hooks")));
    // an account the core does not know is served nothing
    assert!(m.remote.flows_jobs("nobody", &policy).is_none());
    // the flows themselves also go over `flows.*` of the contract, the same socket and the same core
    let mut conn = Connection::unix(m._on.path(), Duration::from_secs(10)).unwrap();
    let listed = conn.call("flows.list", serde_json::Value::Null).expect("the contract serves the flows of the account");
    assert!(listed.as_array().is_some(), "{listed}");
    assert!(m.remote.is_real());
}

#[test]
fn the_platform_services_answer_from_the_core_and_hand_the_ui_no_runner_no_secret_and_no_closure_job() {
    let m = machine("platform");
    let (h, svc) = (&m.remote, &m.svc);
    // the UI's audit callback and (for the test seam) its runner never run
    struct NeverRun;
    impl iyakki_shared::wsexec::Runner for NeverRun {
        fn run(&self, _: &std::path::Path, _: &[String], _: &[(String, String)], _: Duration, _: usize, _: Option<iyakki_shared::wsexec::Sink>) -> iyakki_shared::wsexec::Out {
            panic!("the runner of the UI never runs")
        }
        fn has(&self, _: &str) -> bool {
            panic!("the runner of the UI is never asked")
        }
    }
    let remote = h.platform_real("ann", ui_audit());
    let real = ServicesHandle::platform_real(&**svc, "ann", Arc::new(|_, _| {}));
    assert_eq!(remote.dir(), real.dir());
    assert_eq!(remote.user(), real.user());
    let ids = |p: &Arc<dyn iyakki_agent_api::PlatformApi>| p.services().into_iter().map(|s| (s.id, s.title, s.kind)).collect::<Vec<_>>();
    assert_eq!(ids(&remote), ids(&real), "the services that can be deployed are the core's");
    assert!(!ids(&real).is_empty());
    let first = ids(&real)[0].0.clone();
    assert_eq!(remote.common_options(&first).len(), real.common_options(&first).len());
    assert_eq!(remote.list_profiles().len(), real.list_profiles().len());
    assert_eq!(remote.service(&first).map(|s| s.id), real.service(&first).map(|s| s.id));
    // what the core would not hand to a UI: a command runner, a secret, a closure job
    assert!(remote.vault_get("anything").unwrap_err().contains("does not serve this call"), "a secret of the vault is never answered");
    assert!(!remote.runner().has("true"));
    // the seam with a runner of the caller: the same platform, the caller's runner is not used
    let seam = h.platform_with("ann", Arc::new(NeverRun), iyakki_shared::devcluster::Env::default(), ui_audit(), std::path::PathBuf::from("/tmp/x"));
    assert_eq!(ids(&seam), ids(&real));
    assert!(h.is_real());
}

#[test]
fn the_store_over_the_socket_is_the_cores_own_and_the_person_and_the_fetch_are_the_cores() {
    let m = machine("storeapi");
    let (h, svc) = (&m.remote, &m.svc);
    let remote = h.b4().store().expect("the core serves its store");
    let real = ServicesHandle::b4(&**svc).store().expect("the real services have a store");
    assert_eq!(remote.dir(), real.dir(), "the store folder is the core's");
    assert_eq!(remote.official_store_url(), real.official_store_url());
    assert_eq!(remote.pf_yank_note(), real.pf_yank_note());
    assert_eq!(remote.keys_list("ann").len(), real.keys_list("ann").len());
    // the person is read from the core's accounts: the administrator rights of ann are the core's, whatever the UI claims
    let ann = iyakki_shared::store_install::Actor::from_policy("ann", &iyakki_sec::policy::Role::Admin.template());
    let claimed_guest = iyakki_shared::store_install::Actor { user: "ann".into(), allowed: false, can_own: false, can_shared: false, admin: false };
    let rt = serde_json::from_value::<Vec<iyakki_shared::store_catalog::Loaded>>(serde_json::to_value(real.local_catalogs()).unwrap());
    assert!(rt.is_ok(), "{:?}", rt.err());
    assert_eq!(remote.local_catalogs().len(), real.local_catalogs().len(), "the catalogs the core holds");
    let (a, b) = (remote.items(&claimed_guest, &remote.local_catalogs()), real.items(&ann, &real.local_catalogs()));
    assert_eq!(a.len(), b.len(), "the same catalog for the same account, the claim of the UI is ignored");
    // an account the core does not know gets the quiet value, not the catalog
    let nobody = iyakki_shared::store_install::Actor { user: "nobody".into(), allowed: true, can_own: true, can_shared: true, admin: true };
    assert!(remote.items(&nobody, &remote.local_catalogs()).is_empty());
    assert!(h.is_real());
}

#[test]
fn the_private_store_calls_that_took_the_install_services_run_on_the_core_with_its_own_services() {
    let m = machine("pmanage");
    let (h, svc) = (&m.remote, &m.svc);
    let remote = h.platform_real("ann", ui_audit());
    let real = ServicesHandle::platform_real(&**svc, "ann", Arc::new(|_, _| {}));
    let cluster: iyakki_shared::platform_types::ClusterRef = serde_json::from_value(serde_json::json!({"name": "none", "kubeconfig": "/nonexistent/kubeconfig", "context": "", "server": "", "source": "Project"})).unwrap();
    let ui_side: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
    let (a, b) = (remote.private_store_state(Some(ui_side), &cluster, "store"), real.private_store_state(Some(ServicesHandle::as_any_arc(svc.clone())), &cluster, "store"));
    assert_eq!(format!("{:?}", a.state), format!("{:?}", b.state), "the core read the cluster with its own services");
    assert_eq!((a.namespace.as_str(), a.instance.as_str()), (b.namespace.as_str(), b.instance.as_str()));
    assert!(h.is_real());
}
