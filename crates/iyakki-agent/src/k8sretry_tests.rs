//! Transient failures of kubectl: the classification table, the retries with their waits, the tracker (stale data, banner, backoff)
//! and the log. Everything runs on a fake clock and a scripted fake kubectl: nothing sleeps and no cluster is asked.

use super::*;
use crate::k8sexp::{ExpFeed, Kind, Scope, Want};
use crate::k8s::{Cluster, Target};
use std::sync::atomic::AtomicUsize;

use super::FakeTimer as ManualTimer;

/// A fake kubectl that answers from a script (then the default), counts calls and notices two calls at once.
struct Script {
    answers: Mutex<VecDeque<Out>>,
    default: Mutex<Out>,
    calls: Mutex<Vec<String>>,
    inflight: AtomicUsize,
    overlapped: AtomicBool,
}

impl Script {
    fn new(default: Out) -> Arc<Script> {
        Arc::new(Script { answers: Mutex::new(VecDeque::new()), default: Mutex::new(default), calls: Mutex::new(vec![]), inflight: AtomicUsize::new(0), overlapped: AtomicBool::new(false) })
    }
    fn then(&self, o: Out) -> &Self {
        self.answers.lock().unwrap().push_back(o);
        self
    }
    fn set_default(&self, o: Out) {
        *self.default.lock().unwrap() = o;
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Kube for Script {
    fn run(&self, args: &[String]) -> Out {
        if self.inflight.fetch_add(1, Ordering::SeqCst) > 0 {
            self.overlapped.store(true, Ordering::SeqCst);
        }
        self.calls.lock().unwrap().push(args.join(" "));
        std::thread::sleep(Duration::from_millis(2));
        let o = self.answers.lock().unwrap().pop_front().unwrap_or_else(|| self.default.lock().unwrap().clone());
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        o
    }
}

fn good() -> Out {
    Out { code: Some(0), stdout: r#"{"items":[{"kind":"Pod","metadata":{"name":"web","namespace":"dev"},"status":{"phase":"Running"}}]}"#.into(), ..Default::default() }
}
fn bad(msg: &str) -> Out {
    Out { code: Some(1), stderr: msg.into(), ..Default::default() }
}
fn killed() -> Out {
    Out { code: None, ..Default::default() }
}
fn retry_with(timer: Arc<ManualTimer>) -> Retry {
    Retry::new(&QUICK_RETRY_MS, timer)
}
fn args() -> Vec<String> {
    vec!["--kubeconfig".into(), "/vault/tmp/kc-123".into(), "--context".into(), "prod".into(), "--request-timeout=8s".into(), "get".into(), "pods".into(), "-A".into(), "-o".into(), "json".into()]
}
const COLD: Ctx = Ctx { starting: false, worked_before: false };
const WARM: Ctx = Ctx { starting: false, worked_before: true };
const BOOT: Ctx = Ctx { starting: true, worked_before: false };

#[test]
fn classification_table() {
    use Verdict::{Persistent as P, Transient as T};
    let timed_out = Out { code: None, timed_out: true, ..Default::default() };
    let missing = Out { missing: true, stderr: "kubectl: No such file or directory".into(), ..Default::default() };
    let cases: Vec<(&str, Out, Ctx, Verdict)> = vec![
        ("timed out flag", timed_out, COLD, T),
        ("killed, no exit code", killed(), COLD, T),
        ("exit with no message", Out { code: Some(1), ..Default::default() }, COLD, T),
        ("killed get leaves a lone brace", Out { code: None, stdout: "{\n".into(), ..Default::default() }, COLD, T),
        ("timed out text", bad("Error from server (Timeout): the server was unable to return a response in the time allotted, request timed out"), COLD, T),
        ("context deadline", bad("Error from server: context deadline exceeded"), COLD, T),
        ("tls handshake", bad("Unable to connect to the server: net/http: TLS handshake timeout"), COLD, T),
        ("unable to handle", bad("Error from server (ServiceUnavailable): the server is currently unable to handle the request"), COLD, T),
        ("etcd", bad("Error from server: etcdserver: request timed out"), COLD, T),
        ("etcd leader", bad("Error from server: etcdserver: leader changed"), COLD, T),
        ("eof", bad("Unable to connect to the server: EOF"), COLD, T),
        ("connection reset", bad("read tcp 10.0.0.2:5555->10.0.0.1:6443: read: connection reset by peer"), COLD, T),
        ("metrics 503", bad("Error from server (ServiceUnavailable): error: metrics not available yet: 503 Service Unavailable"), COLD, T),
        ("aggregated api 503", bad("unable to retrieve the complete list of server APIs: metrics.k8s.io/v1beta1: the server is currently unable to handle the request"), COLD, T),
        ("refused while k3s starts", bad("The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?"), BOOT, T),
        ("refused after it worked", bad("dial tcp 127.0.0.1:6443: connect: connection refused"), WARM, T),
        ("refused, never worked, not starting", bad("dial tcp 10.1.2.3:6443: connect: connection refused"), COLD, P),
        ("no route, never worked", bad("Unable to connect to the server: dial tcp 10.1.2.3:6443: connect: no route to host"), COLD, P),
        ("no such host, never worked", bad("Unable to connect to the server: dial tcp: lookup k8s.example.org: no such host"), COLD, P),
        ("no such host after it worked", bad("Unable to connect to the server: dial tcp: lookup k8s.example.org: no such host"), WARM, T),
        ("kubectl missing", missing, WARM, P),
        ("forbidden", bad("Error from server (Forbidden): pods is forbidden: User \"x\" cannot list resource \"pods\""), WARM, P),
        ("unauthorized", bad("error: You must be logged in to the server (Unauthorized)"), WARM, P),
        ("bad context", bad("error: context \"nope\" does not exist"), WARM, P),
        ("no kubeconfig", bad("error: no configuration has been provided, try setting KUBERNETES_MASTER environment variable"), BOOT, P),
        ("kubeconfig file missing", bad("error: error loading config file \"/x/kc\": open /x/kc: no such file or directory"), WARM, P),
        ("unknown resource type", bad("error: the server doesn't have a resource type \"ingresses\""), WARM, P),
        ("unknown message", bad("error: something nobody has seen before"), WARM, P),
        ("port number 403 is not a status", bad("dial tcp 10.0.0.1:6403: connect: connection refused"), COLD, P),
    ];
    for (name, o, ctx, want) in cases {
        assert_eq!(classify(&o, ctx), want, "{name}");
    }
}

#[test]
fn a_success_makes_exactly_one_call_and_no_log() {
    let (k, t) = (Script::new(good()), ManualTimer::new());
    let r = retry_with(t.clone());
    let a = r.run(&*k, &args());
    assert!(a.out.ok() && a.verdict.is_none() && a.retries == 0);
    assert_eq!(k.count(), 1);
    assert!(t.slept.lock().unwrap().is_empty());
    assert!(r.log.events().is_empty());
}

#[test]
fn a_transient_failure_succeeds_on_the_second_and_third_try() {
    for fails in [1usize, 2] {
        let (k, t) = (Script::new(good()), ManualTimer::new());
        for _ in 0..fails {
            k.then(bad("Unable to connect to the server: net/http: TLS handshake timeout"));
        }
        let r = retry_with(t.clone());
        let a = r.run(&*k, &args());
        assert!(a.out.ok(), "worked on try {}", fails + 1);
        assert_eq!((a.retries as usize, k.count()), (fails, fails + 1));
        assert_eq!(*t.slept.lock().unwrap(), QUICK_RETRY_MS[..fails].to_vec());
        // the failures and the call that worked again are in the log
        let ev = r.log.events();
        assert_eq!(ev.len(), fails + 1);
        assert!(ev.last().unwrap().verdict.is_none());
    }
}

#[test]
fn it_gives_up_after_three_quick_retries_with_the_waits_700_1500_3000() {
    let (k, t) = (Script::new(bad("Error from server: context deadline exceeded")), ManualTimer::new());
    let a = retry_with(t.clone()).run(&*k, &args());
    assert!(!a.out.ok() && a.verdict == Some(Verdict::Transient) && a.retries == 3 && !a.cancelled);
    assert_eq!(k.count(), 4);
    assert_eq!(*t.slept.lock().unwrap(), vec![700, 1500, 3000]);
}

#[test]
fn a_persistent_failure_is_not_retried() {
    let (k, t) = (Script::new(bad("Error from server (Forbidden): pods is forbidden")), ManualTimer::new());
    let a = retry_with(t.clone()).run(&*k, &args());
    assert_eq!((a.verdict, k.count()), (Some(Verdict::Persistent), 1));
    assert!(t.slept.lock().unwrap().is_empty());
}

#[test]
fn refresh_cancels_the_wait_and_the_caller_starts_over() {
    let (k, t) = (Script::new(bad("etcdserver: request timed out")), ManualTimer::new());
    t.cancel_next.store(true, Ordering::SeqCst);
    let r = retry_with(t.clone());
    let e0 = r.gate.epoch();
    let a = r.run(&*k, &args());
    assert!(a.cancelled, "the wait ended early");
    assert_eq!(k.count(), 1, "no call after a cancelled wait");
    assert_ne!(r.gate.epoch(), e0);
}

#[test]
fn the_real_timer_ends_a_wait_when_the_gate_is_bumped() {
    let (t, g) = (SystemTimer::new(), Arc::new(Gate::default()));
    let g2 = g.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        g2.bump();
    });
    let t0 = Instant::now();
    assert!(!t.sleep(10_000, &g));
    assert!(t0.elapsed() < Duration::from_secs(3));
    h.join().unwrap();
    assert!(t.sleep(30, &g), "an untouched wait completes");
}

#[test]
fn backoff_grows_2_4_8_15_and_stops_at_15() {
    let secs: Vec<u64> = (1..=7).map(|n| background_delay(n).as_secs()).collect();
    assert_eq!(secs, vec![2, 4, 8, 15, 15, 15, 15]);
    let mut tr = Tracker::default();
    assert_eq!(tr.next_delay(Duration::from_secs(5)), Duration::from_secs(5), "healthy: the normal cadence");
    for (i, want) in [2u64, 4, 8, 15, 15].into_iter().enumerate() {
        tr.failed(1000 * (i as u64 + 1), Verdict::Transient, "x", false);
        assert_eq!(tr.next_delay(Duration::from_secs(5)).as_secs(), want);
    }
    tr.reset_backoff();
    assert_eq!(tr.next_delay(Duration::from_secs(5)).as_secs(), 2, "Refresh starts the backoff over");
    tr.ok(5);
    assert_eq!(tr.next_delay(Duration::from_secs(5)), Duration::from_secs(5));
}

#[test]
fn stale_data_stays_for_25_seconds_then_the_banner_says_when_it_last_worked() {
    let mut tr = Tracker::default();
    tr.ok(1_000);
    assert!(tr.failed(10_000, Verdict::Transient, "TLS handshake timeout", false), "inside the window: keep the data");
    let st = tr.status(12_000, 1_002);
    assert!(st.failing && !st.banner && st.last_ok_age_s == Some(2));
    assert!(stale_line(&st).contains("the cluster did not answer, retrying") && stale_line(&st).contains("2 s old"), "{}", stale_line(&st));
    assert!(tr.failed(20_000, Verdict::Transient, "TLS handshake timeout", false));
    assert!(tr.failed(30_000, Verdict::Transient, "TLS handshake timeout", false));
    assert!(tr.failed(34_999, Verdict::Transient, "TLS handshake timeout", false));
    assert!(!tr.failed(35_000, Verdict::Transient, "TLS handshake timeout", false), "25 s of failure: banner");
    let st = tr.status(35_000, 1_040);
    assert!(st.banner);
    assert_eq!(banner_tail(&st), "Last worked 40 s ago. Retrying every 15 s. Press Refresh to retry now.");
    assert_eq!(st.error, "TLS handshake timeout");
    // never worked
    let mut n = Tracker::default();
    n.failed(0, Verdict::Transient, "x", false);
    assert!(banner_tail(&n.status(1000, 5)).starts_with("It has not answered since this view opened."));
}

#[test]
fn a_persistent_failure_shows_the_banner_at_once_and_k3s_starting_waits_longer() {
    let mut tr = Tracker::default();
    tr.ok(1);
    assert!(!tr.failed(100, Verdict::Persistent, "forbidden", false));
    let mut s = Tracker::default();
    s.ok(1);
    assert!(s.failed(10_000, Verdict::Transient, "refused", true));
    assert!(s.failed(10_000 + STALE_WINDOW_MS + 1_000, Verdict::Transient, "refused", true), "starting: still calm after 25 s");
    assert!(!s.failed(10_000 + STARTING_WINDOW_MS, Verdict::Transient, "refused", true));
    assert!(stale_line(&Status { starting: true, ..Default::default() }).starts_with("k3s is starting"));
}

#[test]
fn hints_remember_a_recent_change_of_the_managed_state() {
    let h = Hints::default();
    assert!(!h.starting_now(0));
    h.set_starting(true, 100);
    assert!(h.starting_now(50_000));
    h.set_starting(false, 60_000);
    assert!(h.starting_now(60_000 + CHANGE_GRACE_MS - 1), "just changed: refused is still expected");
    assert!(!h.starting_now(60_000 + CHANGE_GRACE_MS));
    assert!(!h.ctx(0).worked_before);
    h.worked();
    assert!(h.ctx(0).worked_before);
    h.forget();
    assert!(!h.ctx(0).worked_before);
}

#[test]
fn the_log_has_no_secrets_keeps_50_events_and_counts_episodes() {
    let a = summarize(&args());
    assert_eq!(a, "--kubeconfig=\u{2026} --context=\u{2026} get pods -A -o json");
    assert!(!a.contains("kc-123") && !a.contains("prod") && !a.contains("request-timeout"));
    assert_eq!(summarize(&["--token=abc123".into(), "get".into(), "pods".into()]), "--token=\u{2026} get pods");
    let o = bad("error loading /vault/tmp/kc-123: Authorization: Bearer abc.def.ghi rejected");
    let said = first_line(&o, &["/vault/tmp/kc-123".into()]);
    assert!(!said.contains("kc-123") && !said.contains("abc.def") && said.contains("<kubeconfig>"), "{said}");
    let (k, t) = (Script::new(bad("Unable to connect to the server: EOF")), ManualTimer::new());
    let r = retry_with(t);
    r.run(&*k, &args());
    let text = r.log.text();
    assert!(text.contains("kubectl --kubeconfig=\u{2026} --context=\u{2026} get pods -A -o json") && text.contains("exit 1") && text.contains("transient, try 1") && text.contains("transient, try 4") && text.contains("EOF"), "{text}");
    assert!(!text.contains("kc-123") && !text.contains("prod"));
    let log = ClusterLog::default();
    let ev = |v: Option<Verdict>| LogEvent { at: 0, call: "get pods".into(), ms: 5, code: Some(1), said: "x".into(), verdict: v, retry: 0 };
    assert!(log.push(ev(Some(Verdict::Transient))), "the first failure starts an episode");
    assert!(!log.push(ev(Some(Verdict::Transient))), "the next ones belong to it");
    assert!(!log.push(ev(None)));
    assert!(log.push(ev(Some(Verdict::Transient))), "a failure after a success is a new episode");
    for _ in 0..80 {
        log.push(ev(Some(Verdict::Persistent)));
    }
    assert_eq!(log.events().len(), LOG_EVENTS);
    assert!(ClusterLog::default().text().contains("No kubectl problems"));
}

// ------------------------------------------------------------------------------------------------------ the feed

fn want() -> Want {
    Want { target: Target { namespace: "dev".into(), ..Default::default() }, scope: Scope::All, kind: Kind::Pods }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn feed(k: &Arc<Script>, t: &Arc<ManualTimer>) -> (ExpFeed, Retry) {
    let r = retry_with(t.clone());
    let f = ExpFeed::start_with(k.clone(), Duration::from_secs(5), Duration::from_secs(10), r.clone());
    f.set_want(want());
    f.set_visible(true);
    (f, r)
}

#[test]
fn the_feed_keeps_the_last_good_list_then_shows_the_banner_and_recovers() {
    let (k, t) = (Script::new(good()), ManualTimer::new());
    let (f, _r) = feed(&k, &t);
    wait_for("first list", || f.fetches() >= 1 && f.ns_fetches() >= 1);
    assert_eq!(f.listing().unwrap().rows.len(), 1);
    assert!(!f.status().failing);
    let calls_ok = k.count();
    assert_eq!(calls_ok, 2, "one call per list on the success path (list + namespaces)");
    // the API server stops answering
    k.set_default(bad("Unable to connect to the server: net/http: TLS handshake timeout"));
    let n = f.fetches();
    f.refresh();
    wait_for("a failed refresh", || f.fetches() > n);
    let l = f.listing().unwrap();
    assert!(l.cluster == Cluster::Ok && l.rows.len() == 1, "the old list stays");
    let st = f.status();
    assert!(st.failing && !st.banner && st.fails == 1, "{st:?}");
    assert!(stale_line(&st).contains("retrying"));
    // keep failing past the window: the banner replaces the list
    let mut guard = 0;
    while !f.status().banner {
        guard += 1;
        assert!(guard < 40, "the banner never came");
        let n = f.fetches();
        t.advance(16_000);
        wait_for("a background try", || f.fetches() > n);
    }
    let l = f.listing().unwrap();
    assert!(matches!(&l.cluster, Cluster::Unreachable(m) if m.contains("TLS handshake")), "{:?}", l.cluster);
    let st = f.status();
    assert!(st.last_ok_age_s.is_some_and(|a| a >= 25) && st.error.contains("TLS handshake timeout"), "{st:?}");
    assert!(banner_tail(&st).contains(&format!("Retrying every {} s", st.retry_every_s)) && st.retry_every_s >= 2);
    // it comes back: the banner goes away by itself
    k.set_default(good());
    let n = f.fetches();
    t.advance(16_000);
    wait_for("recovery", || f.fetches() > n && f.listing().is_some_and(|l| l.cluster == Cluster::Ok));
    assert!(!f.status().failing);
    assert!(!k.overlapped.load(Ordering::SeqCst), "two kubectl calls never ran at once");
}

#[test]
fn the_feed_retries_quietly_and_recovers_inside_one_refresh() {
    let (k, t) = (Script::new(good()), ManualTimer::new());
    k.then(bad("etcdserver: request timed out")).then(bad("etcdserver: request timed out"));
    let (f, r) = feed(&k, &t);
    wait_for("first list", || f.fetches() >= 1);
    assert!(f.listing().is_some_and(|l| l.cluster == Cluster::Ok && l.rows.len() == 1), "two retries, then the data: no banner ever");
    assert!(!f.status().failing);
    assert!(r.log.events().len() >= 3);
}

#[test]
fn a_persistent_failure_replaces_the_list_at_once() {
    let (k, t) = (Script::new(good()), ManualTimer::new());
    let (f, _r) = feed(&k, &t);
    wait_for("first list", || f.fetches() >= 1);
    k.set_default(bad("Error from server (Forbidden): pods is forbidden: User \"x\" cannot list"));
    let n = f.fetches();
    f.refresh();
    wait_for("failure", || f.fetches() > n);
    assert!(matches!(f.listing().unwrap().cluster, Cluster::Forbidden(_)));
    assert!(f.status().banner);
}

#[test]
fn refresh_during_a_retry_cycle_cancels_the_wait_and_resets_the_backoff() {
    let (k, t) = (Script::new(bad("Unable to connect to the server: EOF")), ManualTimer::new());
    let (f, _r) = feed(&k, &t);
    wait_for("first failure", || f.fetches() >= 1);
    for _ in 0..3 {
        let n = f.fetches();
        t.advance(20_000);
        wait_for("another failure", || f.fetches() > n);
    }
    assert!(f.status().fails >= 3);
    // the next wait is cut short by Refresh
    t.cancel_next.store(true, Ordering::SeqCst);
    let n = f.fetches();
    t.advance(20_000);
    wait_for("the wait was cancelled", || !t.cancel_next.load(Ordering::SeqCst));
    f.refresh();
    wait_for("a refresh after the cancel", || f.fetches() > n);
    assert!(!k.overlapped.load(Ordering::SeqCst));
}

#[test]
fn k3s_starting_keeps_the_banner_away_for_longer() {
    let (k, t) = (Script::new(good()), ManualTimer::new());
    let (f, _r) = feed(&k, &t);
    wait_for("first list", || f.fetches() >= 1);
    f.set_starting(true);
    k.set_default(bad("The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?"));
    for _ in 0..4 {
        let n = f.fetches();
        t.advance(16_000);
        wait_for("a try", || f.fetches() > n);
    }
    let st = f.status();
    assert!(st.failing && st.starting && !st.banner, "{st:?}");
    assert!(f.listing().is_some_and(|l| l.cluster == Cluster::Ok), "the last data is still there");
}

#[test]
fn the_managed_local_cluster_counts_a_refused_connection_as_a_restart() {
    let refused = bad("The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?");
    let h = Hints::default();
    assert_eq!(classify(&refused, h.ctx(0)), Verdict::Persistent, "an address that never answered");
    h.set_local(true);
    assert_eq!(classify(&refused, h.ctx(0)), Verdict::Transient);
    h.set_local(false);
    assert_eq!(classify(&refused, h.ctx(0)), Verdict::Persistent);
}
