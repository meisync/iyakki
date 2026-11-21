//! RADIO, MEDIA, LAB and STUDY objects over the real UI socket against the real services (F-SPLIT-03, alpha.7 S3, D-SPLIT-03-POLICY): what the
//! tiles ask the core through the remote handle lands in the core's own stores, and the person's policy is never taken from the UI. Linux only:
//! a unix socket.
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_agent_api::ServicesHandle;
use iyakki_compose::rpc::RpcRoots;
use iyakki_sec::policy::Role;
use iyakki_shared::mediaqueue::{Request, Who};
use iyakki_ui_client::client::Connection;
use iyakki_ui_rpc::{RemoteServicesHandle, Rpc};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Machine {
    svc: Arc<iyakki_agent::services::Services>,
    remote: Arc<dyn ServicesHandle>,
    _on: iyakki_agent::contract_local::uiserve::UiServer,
    dir: std::path::PathBuf,
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A core with an administrator `ann` and a guest `gus`, the machine's file rules and its data folder.
fn machine(tag: &str) -> Machine {
    iyakki_compose::wire();
    let d = iyakki_agent::testdir::short("iyakki-tiles", tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = iyakki_agent::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let core = iyakki_agent::contract_local::core_for(&svc);
    let users = Arc::new(Mutex::new(iyakki_accounts::UserStore::open(&d.join("users.json")).unwrap()));
    {
        let mut u = users.lock().unwrap();
        u.create_first_admin("ann", "annpass12345").unwrap();
        let admin = u.authenticate("ann", "annpass12345").unwrap();
        u.create_user(&admin, "gus", "guspass12345", Role::Guest).unwrap();
    }
    let real: Arc<dyn iyakki_agent_api::AccountsApi> = Arc::new(users);
    let roots = RpcRoots::new().with_accounts(real).with_file_scope(Arc::new(iyakki_agent::corehandle::LocalFileScope));
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: vec!["settings:read".into()], account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &d.join("on.sock"), &facets).unwrap();
    let rpc = Rpc::new(Connection::unix(on.path(), Duration::from_secs(10)).unwrap());
    let remote: Arc<dyn ServicesHandle> = Arc::new(RemoteServicesHandle::new(rpc));
    Machine { svc, remote, _on: on, dir: d }
}

#[test]
fn the_radio_list_is_the_cores_and_a_station_added_over_the_socket_is_kept_there() {
    let m = machine("radio");
    let remote = m.remote.b5().radio_stations("ann").expect("the core serves the radio stations");
    let real = ServicesHandle::b5(&*m.svc).radio_stations("ann").unwrap();
    assert_eq!(remote.list(), real.list());
    let added = remote.add("Test FM", "https://radio.example.com/stream").unwrap();
    assert_eq!(added.name, "Test FM");
    assert!(real.list().iter().any(|s| s.name == "Test FM" && s.url == "https://radio.example.com/stream"), "the station is in the core's own list");
    assert_eq!(remote.list(), real.list());
    // the core's refusal comes back as the core's own words
    assert_eq!(remote.add("Bad", "not a web address").unwrap_err(), real.add("Bad", "not a web address").unwrap_err());
    remote.remove("Test FM").unwrap();
    assert!(!real.list().iter().any(|s| s.name == "Test FM"), "and the removal too");
    // another person has a list of their own
    assert!(m.remote.b5().radio_stations("gus").unwrap().list().iter().all(|s| s.name != "Test FM"));
}

#[test]
fn the_radio_search_uses_the_policy_of_the_named_account_and_never_the_one_the_ui_sends() {
    let m = machine("radiosearch");
    let (admin, guest) = (Role::Admin.template(), Role::Guest.template());
    // the UI sends an administrator's policy for the guest: the core reads the guest's own, so the answer is the one a guest gets in the core
    let over_the_socket = m.remote.b5().radio_search(&admin, "gus", "jazz", 5);
    let for_a_guest = ServicesHandle::b5(&*m.svc).radio_search(&guest, "gus", "jazz", 5);
    assert_eq!(over_the_socket, for_a_guest);
    // an account the machine does not have is refused by name, with no stations
    let (found, why) = m.remote.b5().radio_search(&admin, "nobody", "jazz", 5);
    assert!(found.is_empty());
    assert!(why.unwrap_or_default().contains("no account nobody"));
}

#[test]
fn a_download_asked_over_the_socket_is_the_cores_queue_and_the_ui_cannot_name_its_own_rights() {
    let m = machine("media");
    assert!(!Role::Guest.template().allows_host("files.example.com"), "the guest policy keeps the guest off the network, or the test shows nothing");
    let remote = m.remote.b5().media_queue("ann").expect("the core serves the download queue");
    let real = ServicesHandle::b5(&*m.svc).media_queue("ann").unwrap();
    let who = |user: &str, role: Role| Who { policy: role.template(), user: user.into(), folders: vec![] };
    let req = || Request { url: "https://files.example.com/a.mp3".into(), ..Request::default() };
    // an administrator's download is queued in the core's own queue of that person
    let id = remote.enqueue(who("ann", Role::Admin), req()).unwrap();
    assert!(real.view(id).is_some(), "the item is in the core's queue");
    assert_eq!(remote.view(id).map(|v| v.id), Some(id));
    // the UI names an administrator's policy for the guest: the core judges the guest's own and refuses, exactly as it does in the process
    let refused = remote.enqueue(who("gus", Role::Admin), req()).unwrap_err();
    let asked_locally = ServicesHandle::b5(&*m.svc).media_queue("gus").unwrap().enqueue(who("gus", Role::Guest), req()).unwrap_err();
    assert_eq!(refused, asked_locally);
    // a person the machine does not have is refused by name and queues nothing
    let nobody = remote.enqueue(who("nobody", Role::Admin), req()).unwrap_err();
    assert!(nobody.contains("no account nobody"), "{nobody}");
    // the item can be cancelled and removed through the socket, and the core's queue follows
    remote.cancel(id).ok();
    remote.remove(id).ok();
    assert_eq!(remote.counts(), real.counts());
}

#[test]
fn the_lab_models_and_the_catalogue_are_the_cores_and_a_frame_of_a_file_outside_the_persons_reach_is_refused() {
    let m = machine("lab");
    let b5 = m.remote.b5();
    assert!(b5.lab_backends().is_some(), "the picture and the vision model are remote objects of the core");
    assert_eq!(b5.lab_catalogue().vision.len(), ServicesHandle::b5(&*m.svc).lab_catalogue().vision.len());
    let frames = b5.lab_frames_for("gus");
    let refused = frames.info("/etc/shadow").unwrap_err();
    assert!(!refused.is_empty(), "the core's file rules refuse a file outside the guest's reach");
}

#[test]
fn the_study_pdf_export_judges_the_target_by_the_named_accounts_policy_in_the_core() {
    let m = machine("study");
    let (admin, guest) = (Role::Admin.template(), Role::Guest.template());
    let item = iyakki_shared::studypdf::Item::Plan("no-such-plan".into());
    // the UI sends an administrator's policy and a data folder of its own: neither is used; the answer is the core's for the guest
    let over_the_socket = m.remote.b5().study_pdf_export(&admin, "gus", std::path::Path::new("/tmp/elsewhere"), &item, true, Some("/etc/x.pdf"));
    let in_the_core = ServicesHandle::b5(&*m.svc).study_pdf_export(&guest, "gus", &m.dir.join("svc"), &item, true, Some("/etc/x.pdf"));
    assert_eq!(over_the_socket, in_the_core);
    assert!(over_the_socket.is_err());
    let nobody = m.remote.b5().study_pdf_export(&admin, "nobody", std::path::Path::new("/tmp"), &item, true, None).unwrap_err();
    assert!(nobody.contains("no account nobody"), "{nobody}");
}
