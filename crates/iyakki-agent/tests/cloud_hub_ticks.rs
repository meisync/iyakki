//! The notification hub's worker is driven by the cloud-flows thread (`cloudflows::real::start`): every second step it calls
//! `Hub::tick`, which takes the hub's lease on the tenant volume. Nothing else in the process calls `tick`, so a lease file with
//! this process as holder proves the loop ticked the hub, and that stopping the thread ends the loop. Own test binary on purpose:
//! `start` installs process-wide hooks (sandbox runner, flow callbacks) that must not leak into the library's other tests.
//! The loop's wait between two steps is a `Pacer` (the product waits five seconds of wall time): the test's pacer lets the loop take
//! exactly the steps the test grants and says when the loop is waiting again, so how fast the machine is cannot change the result
//! (the old test waited 25 s for the lease and 8 s for the stop and failed in the gate under a load above 100).

use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Grants steps one at a time; counts how often the loop has come back to wait.
#[derive(Default)]
struct Steps {
    st: Mutex<(u32, u32)>, // (permits granted and not yet used, times the loop has waited)
    cv: Condvar,
}
impl Steps {
    fn grant(&self) {
        self.st.lock().unwrap().0 += 1;
        self.cv.notify_all();
    }
    fn wake(&self) {
        self.cv.notify_all();
    }
    fn waited(&self) -> u32 {
        self.st.lock().unwrap().1
    }
}
impl iyakki_agent::cloudflows::real::Pacer for Steps {
    fn wait(&self, stop: &AtomicBool) {
        let mut g = self.st.lock().unwrap();
        g.1 += 1;
        self.cv.notify_all();
        while g.0 == 0 && !stop.load(Ordering::Relaxed) {
            g = self.cv.wait_timeout(g, Duration::from_millis(50)).unwrap().0;
        }
        g.0 = g.0.saturating_sub(1);
    }
}

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
    let steps = Arc::new(Steps::default());
    let handle = iyakki_agent::cloudflows::real::start_with_pacer(svc.clone(), Arc::new(NoAccounts), audit, stop.clone(), steps.clone()).expect("the thread starts");

    // step 1 ran at once (the loop waits for the first time); one granted step makes step 2, which ticks the hub (every second step)
    iyakki_testkit::wait_until(Duration::from_secs(120), || steps.waited() >= 1);
    assert!(!lease.exists(), "one step does not tick the hub");
    steps.grant();
    iyakki_testkit::wait_until(Duration::from_secs(120), || steps.waited() >= 2);
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

    // stopping: no step is granted any more, so the loop can only end because of the flag (it does not wait for another step)
    stop.store(true, Ordering::Relaxed);
    steps.wake();
    iyakki_testkit::wait_until(Duration::from_secs(120), || handle.is_finished());
    handle.join().expect("the loop ends when its flag is set");
    let _ = std::fs::remove_dir_all(&dir);
}
