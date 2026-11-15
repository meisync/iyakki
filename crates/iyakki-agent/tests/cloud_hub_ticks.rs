//! The notification hub's worker is driven by the cloud-flows thread (`cloudflows::real::start`): every second step it calls
//! `Hub::tick`, which takes the hub's lease on the tenant volume. Nothing else in the process calls `tick`, so a lease file with
//! this process as holder proves the loop ticked the hub, and that stopping the thread ends the loop. Own test binary on purpose:
//! `start` installs process-wide hooks (sandbox runner, flow callbacks) that must not leak into the library's other tests.
//! Takes about ten seconds (the loop steps every five).

use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}
struct NoAccounts;
impl Authenticator for NoAccounts {
    fn login(&self, _: &str, _: &str) -> ApiResult<Principal> {
        Err(ApiError::Denied("no accounts".into()))
    }
    fn lookup(&self, _: &str) -> Option<Principal> {
        None
    }
}
struct Lines(std::sync::Mutex<Vec<String>>);
impl AuditSink for Lines {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.0.lock().unwrap().push(format!("{user} {event} {detail}"));
    }
}

#[test]
fn the_cloud_flows_loop_ticks_the_notification_hub_and_stops_with_its_flag() {
    let dir = std::env::temp_dir().join(format!("iyakki-hubtick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    let svc = iyakki_agent::services::Services::open(&dir.join("tenant"), Arc::new(Off), Arc::new(Off)).unwrap();
    let lease = svc.dir().join("cloud").join("notify").join("lease.json");
    assert!(!lease.exists(), "nothing has ticked before the loop starts");

    // the tenant has one team with one person in it
    std::fs::create_dir_all(svc.dir().join("cloud")).unwrap();
    std::fs::write(svc.dir().join("cloud").join("roles.json"), r#"{"tenant":{"ann":"member"},"namespaces":{"ops":{"ann":"editor"}}}"#).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let audit = Arc::new(Lines(Default::default()));
    let handle = iyakki_agent::cloudflows::real::start(svc.clone(), Arc::new(NoAccounts), audit, stop.clone()).expect("the thread starts");

    let began = Instant::now();
    while !lease.exists() && began.elapsed() < Duration::from_secs(25) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(lease.exists(), "after two steps of the loop the hub took its lease (it was ticked)");
    let rec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&lease).unwrap()).unwrap();
    assert_eq!(rec["holder"].as_str().unwrap(), format!("pid{}", std::process::id()), "this process holds the hub's lease");
    assert!(rec["expires"].as_u64().unwrap() > iyakki_schema::now_secs(), "the lease is in force");

    // `start` also bound a flow's `notify` step to the hub: the note reaches the person's inbox and nobody else's
    let hook = iyakki_agent::flows::CLOUD_NOTIFY.get().expect("start installs the flow notify hook");
    let notice = iyakki_flow::host::Notice { title: "Nightly check failed".into(), body: "step 3".into(), level: "error".into(), flow_id: "nightly".into(), run_id: "r1".into(), needs_you: false };
    hook("ann", &notice, &serde_json::json!({}));
    let hub = iyakki_agent::notifyhub::service::hub_for(svc.dir());
    let mine = hub.inbox.list("ann");
    assert_eq!(mine.len(), 1, "one inbox note for the person the flow belongs to");
    assert_eq!(mine[0].title, "Nightly check failed");
    assert!(hub.inbox.list("bob").is_empty(), "nobody else gets it");

    stop.store(true, Ordering::Relaxed);
    let t = Instant::now();
    handle.join().expect("the loop ends when its flag is set");
    assert!(t.elapsed() < Duration::from_secs(8), "stopping does not wait for another full step");
    let _ = std::fs::remove_dir_all(&dir);
}
