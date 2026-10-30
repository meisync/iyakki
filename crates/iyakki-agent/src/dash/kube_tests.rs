//! The `kube` data source and the default k3s dashboard, with FAKE cluster answers (JSON fixtures in `kube_fixtures/`): no cluster, no
//! kubectl, no network. What these cannot show is how a real k3s answers (see docs/DASHBOARD.md, "Not verified here").

use super::cluster::{no_cluster, ClusterApi, KubeCluster, KubeRead, Kubectl, SvcInfo};
use super::connector::fake::*;
use super::connector::*;
use super::frame::{parse_time_ms, Frame};
use super::kube::{age_text, quantity, run_view, view_name, view_names, Kube};
use super::registry;
use super::tools::Env;
use crate::wsexec::FakeRunner;
use iyakki_sec::policy::Role;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VERSION: &str = include_str!("kube_fixtures/version.json");
const NODES: &str = include_str!("kube_fixtures/nodes.json");
const PODS: &str = include_str!("kube_fixtures/pods.json");
const EVENTS: &str = include_str!("kube_fixtures/events.json");
const DEPLOYMENTS: &str = include_str!("kube_fixtures/deployments.json");
const SERVICES: &str = include_str!("kube_fixtures/services.json");
const PVCS: &str = include_str!("kube_fixtures/pvcs.json");
const NODE_STATS: &str = include_str!("kube_fixtures/node_stats.json");
const NODE_METRICS: &str = include_str!("kube_fixtures/node_metrics.json");
const POD_METRICS: &str = include_str!("kube_fixtures/pod_metrics.json");

fn now() -> i64 {
    parse_time_ms("2026-10-28T12:00:00Z").unwrap()
}

/// A scripted cluster: each look answers with its text or its error; unscripted looks say the cluster is down.
pub(super) struct FakeApi {
    answers: Mutex<HashMap<String, Result<String, DashError>>>,
    pub(super) asked: Mutex<Vec<KubeRead>>,
}

impl FakeApi {
    pub(super) fn empty() -> Arc<FakeApi> {
        Arc::new(FakeApi { answers: Mutex::new(HashMap::new()), asked: Mutex::new(vec![]) })
    }
    pub(super) fn full() -> Arc<FakeApi> {
        let a = FakeApi::empty();
        for (k, t) in [("Version", VERSION), ("Nodes", NODES), ("Pods", PODS), ("Events", EVENTS), ("Deployments", DEPLOYMENTS), ("Services", SERVICES), ("Pvcs", PVCS), ("NodeMetrics", NODE_METRICS), ("PodMetrics", POD_METRICS), ("NodeStats(\"iyakki-dev\")", NODE_STATS)] {
            a.say(k, t);
        }
        a
    }
    fn say(&self, key: &str, text: &str) {
        self.answers.lock().unwrap().insert(key.into(), Ok(text.into()));
    }
    fn fail(&self, key: &str, e: DashError) {
        self.answers.lock().unwrap().insert(key.into(), Err(e));
    }
    fn all(&self, text: &str) {
        for k in ["Version", "Nodes", "Pods", "Events", "Deployments", "Services", "Pvcs", "NodeMetrics", "PodMetrics"] {
            self.say(k, text);
        }
    }
}

impl ClusterApi for FakeApi {
    fn service(&self, _: &str, _: &str) -> Result<SvcInfo, DashError> {
        Err(no_cluster("fake"))
    }
    fn services(&self) -> Result<Vec<SvcInfo>, DashError> {
        Err(no_cluster("fake"))
    }
    fn host_listens(&self, _: u16) -> bool {
        false
    }
    fn kube_read(&self, what: &KubeRead) -> Result<String, DashError> {
        self.asked.lock().unwrap().push(what.clone());
        self.answers.lock().unwrap().get(&format!("{what:?}")).cloned().unwrap_or_else(|| Err(no_cluster("k3s is stopped")))
    }
}

/// A request whose range ends at the fixtures' "now".
fn rng() -> TimeRange {
    TimeRange { from_ms: now() - 3_600_000, to_ms: now() }
}

fn q(api: &Arc<FakeApi>, view: &str) -> Frame {
    run_view(api.as_ref(), view, now()).unwrap_or_else(|e| panic!("{view}: {e}"))
}

fn cell(f: &Frame, r: usize, c: &str) -> String {
    f.cell(r, f.column_index(c).unwrap_or_else(|| panic!("no column {c} in {:?}", f.columns)))
}

fn col(f: &Frame, c: &str) -> Vec<String> {
    (0..f.rows.len()).map(|r| cell(f, r, c)).collect()
}

// ------------------------------------------------------------------------------------------------------ helpers

#[test]
fn quantities_and_ages() {
    for (q, v) in [("250m", 0.25), ("1", 1.0), ("2", 2.0), ("1500000n", 0.0015), ("12345Ki", 12_641_280.0), ("1Gi", 1_073_741_824.0), ("128974848", 128_974_848.0), ("129M", 129e6), ("1e3", 1000.0), ("5u", 5e-6), ("1.5Gi", 1_610_612_736.0), ("2E", 2e18)] {
        let got = quantity(q).unwrap_or_else(|| panic!("{q}"));
        assert!((got - v).abs() <= v.abs() * 1e-9, "{q}: {got} != {v}");
    }
    for bad in ["", "  ", "lots", "-5", "NaN", "inf", "1Zi", "Ki", "m", "1.2.3"] {
        assert!(quantity(bad).is_none(), "{bad:?} should not parse");
    }
    assert_eq!(age_text(0), "0s");
    assert_eq!(age_text(-30), "0s");
    assert_eq!(age_text(59), "59s");
    assert_eq!(age_text(125), "2m");
    assert_eq!(age_text(3 * 3600 + 300), "3h 5m");
    assert_eq!(age_text(4 * 86_400 + 3 * 3600), "4d 3h");
}

// ------------------------------------------------------------------------------------------------------ the views, healthy fixtures

#[test]
fn every_view_runs_on_the_fixtures_with_rows_that_match_their_columns() {
    let api = FakeApi::full();
    for name in view_names() {
        let f = q(&api, name);
        assert!(!f.columns.is_empty(), "{name}");
        assert!(f.rows.iter().all(|r| r.len() == f.columns.len()), "{name}: a row does not match the columns");
        assert_eq!(f.meta.executed, name);
    }
}

#[test]
fn the_cluster_row_gives_state_version_nodes_age_and_metrics() {
    let f = q(&FakeApi::full(), "cluster");
    assert_eq!(f.rows.len(), 1);
    assert_eq!(cell(&f, 0, "State"), "degraded");
    assert_eq!(cell(&f, 0, "Detail"), "1 of 2 nodes not Ready");
    assert_eq!(cell(&f, 0, "Version"), "v1.31.4+k3s1");
    assert_eq!(cell(&f, 0, "Nodes ready"), "1/2");
    assert_eq!(cell(&f, 0, "Cluster age"), "8d 4h");
    assert_eq!(cell(&f, 0, "Ready for"), "3h 0m");
    assert_eq!(cell(&f, 0, "Metrics (CPU, memory)"), "available (3 nodes reporting)");
    // everything Ready
    let api = FakeApi::full();
    api.say("Nodes", &NODES.replace(r#""type":"Ready","status":"False""#, r#""type":"Ready","status":"True""#));
    assert_eq!(cell(&q(&api, "cluster"), 0, "State"), "running");
    assert_eq!(cell(&q(&api, "stat_nodes_not_ready"), 0, "nodes not ready"), "0");
    // an API that answers with no node yet
    api.say("Nodes", r#"{"items":[]}"#);
    assert_eq!(cell(&q(&api, "cluster"), 0, "State"), "no nodes");
    // version unreadable: still a row
    api.say("Version", "garbage");
    assert_eq!(cell(&q(&api, "cluster"), 0, "Version"), "unknown");
}

#[test]
fn nodes_show_ready_roles_and_bad_conditions() {
    let f = q(&FakeApi::full(), "nodes");
    assert_eq!(col(&f, "Node"), ["iyakki-dev", "worker-2"]);
    assert_eq!(col(&f, "State"), ["Ready", "NotReady"]);
    assert_eq!(col(&f, "Roles"), ["control-plane,master", "worker"]);
    assert_eq!(cell(&f, 0, "Conditions"), "ok");
    let c = cell(&f, 1, "Conditions");
    assert!(c.contains("MemoryPressure") && c.contains("SchedulingDisabled") && c.contains("Kubelet stopped posting node status"), "{c}");
    let s = q(&FakeApi::full(), "node_states");
    assert_eq!(col(&s, "Nodes"), ["1", "1", "0"]);
}

#[test]
fn pod_problems_name_the_reason_and_worst_restarts_come_first() {
    let f = q(&FakeApi::full(), "pod_problems");
    assert_eq!(col(&f, "Pod"), ["web-crash", "notready", "img-bad", "starting", "stuck-term", "evicted", "unsched"]);
    assert_eq!(col(&f, "Problem"), ["CrashLoopBackOff", "NotReady", "ImagePullBackOff", "Pending", "Terminating", "Evicted", "Pending"]);
    let r = cell(&f, 0, "Reason");
    assert!(r.contains("back-off 5m0s") && r.contains("OOMKilled (exit 137)"), "{r}");
    assert_eq!(cell(&f, 0, "Restarts"), "12");
    assert!(cell(&f, 2, "Reason").contains("registry.example/app:9"));
    assert_eq!(cell(&f, 1, "Reason"), "not ready: api");
    assert_eq!(cell(&f, 3, "Reason"), "waiting to start");
    assert_eq!(cell(&f, 4, "Reason"), "deleting for 1h 0m");
    assert!(cell(&f, 6, "Reason").starts_with("Unschedulable: 0/1 nodes are available"), "{}", cell(&f, 6, "Reason"));
    assert_eq!(cell(&f, 0, "Node"), "iyakki-dev");
    // the succeeded job and the healthy pods are not listed
    assert!(!col(&f, "Pod").iter().any(|p| p == "done" || p == "coredns-1" || p == "web-1"));
    assert_eq!(cell(&q(&FakeApi::full(), "stat_pods_with_problems"), 0, "pods with problems"), "7");
}

#[test]
fn a_credential_in_a_status_message_and_an_env_value_never_reach_a_frame() {
    let api = FakeApi::full();
    let mut all = String::new();
    for name in view_names() {
        let f = q(&api, name);
        all.push_str(&serde_json::to_string(&f).unwrap());
    }
    assert!(!all.contains("ghp_abcdefghijklmnop"), "a token in an eviction message was kept");
    assert!(!all.contains("planted-env-value-8841"), "an environment value was copied");
    assert!(all.contains("credential removed"), "the message is replaced, not dropped");
}

#[test]
fn pods_by_phase_namespace_and_restarts() {
    let f = q(&FakeApi::full(), "pods_by_phase");
    assert_eq!(col(&f, "Phase"), ["Running", "Pending", "Succeeded", "Failed"]);
    assert_eq!(col(&f, "Pods"), ["5", "3", "1", "1"]);
    let n = q(&FakeApi::full(), "pods_by_namespace");
    assert_eq!(col(&n, "Namespace"), ["default", "shop", "jobs", "kube-system"]);
    assert_eq!(col(&n, "Pods"), ["5", "3", "1", "1"]);
    let r = q(&FakeApi::full(), "top_restarts");
    assert_eq!(col(&r, "Pod"), ["web-crash", "web-1", "notready"]);
    assert_eq!(col(&r, "Restarts"), ["12", "3", "1"]);
    assert_eq!(col(&r, "Last exit"), ["OOMKilled (exit 137)", "Error (exit 2)", "Error (exit 1)"]);
}

#[test]
fn deployments_not_fully_available_with_the_reason() {
    let f = q(&FakeApi::full(), "deployments_unavailable");
    assert_eq!(col(&f, "Deployment"), ["stuck", "web"]);
    assert!(cell(&f, 0, "Problem").starts_with("ProgressDeadlineExceeded"), "{}", cell(&f, 0, "Problem"));
    assert!(cell(&f, 1, "Problem").starts_with("MinimumReplicasUnavailable"));
    assert_eq!((cell(&f, 1, "Desired"), cell(&f, 1, "Ready"), cell(&f, 1, "Available")), ("3".into(), "1".into(), "1".into()));
    assert_eq!(cell(&q(&FakeApi::full(), "stat_deployments_unavailable"), 0, "deployments unavailable"), "2");
}

#[test]
fn cpu_and_memory_per_node_namespace_and_pod() {
    let api = FakeApi::full();
    let n = q(&api, "node_usage");
    assert_eq!(col(&n, "Node"), ["iyakki-dev", "worker-2"], "the node with an unreadable value is left out");
    assert_eq!(cell(&n, 0, "CPU (cores)"), "1");
    assert_eq!(cell(&n, 0, "CPU %"), "25");
    assert_eq!(cell(&n, 0, "Memory"), "2048000000");
    assert_eq!(cell(&n, 0, "Memory %"), "25");
    assert_eq!(cell(&n, 1, "CPU (cores)"), "0.5");
    assert!(n.meta.notices.iter().any(|x| x.contains("could not be read")), "{:?}", n.meta.notices);
    let c = q(&api, "namespace_cpu");
    assert_eq!((col(&c, "Namespace"), col(&c, "CPU (m)")), (vec!["shop".to_string(), "kube-system".into()], vec!["400".to_string(), "3".into()]));
    let m = q(&api, "namespace_memory");
    assert_eq!(col(&m, "Namespace"), ["shop", "kube-system"]);
    assert_eq!(cell(&m, 0, "Memory"), "1677721600");
    let p = q(&api, "pod_usage");
    assert_eq!(col(&p, "Pod"), ["api-1", "web-1", "coredns-1"]);
    assert_eq!(cell(&p, 1, "CPU (m)"), "300");
}

#[test]
fn warning_events_of_the_last_hour_newest_first() {
    let f = q(&FakeApi::full(), "warning_events");
    assert_eq!(col(&f, "Reason"), ["BackOff", "FailedScheduling"]);
    assert_eq!(col(&f, "Object"), ["Pod/web-crash", "Pod/unsched"]);
    assert_eq!(col(&f, "Count"), ["40", "7"]);
    assert_eq!(col(&f, "Namespace"), ["default", "shop"]);
    assert_eq!(f.number(0, 0).unwrap() as i64, parse_time_ms("2026-10-28T11:58:00Z").unwrap());
    assert!(f.meta.notices.iter().any(|n| n.contains("1 warning event without a time")), "{:?}", f.meta.notices);
    assert_eq!(cell(&q(&FakeApi::full(), "stat_warnings_last_hour"), 0, "warnings (1 h)"), "2");
}

#[test]
fn exposed_services_mark_the_documented_ones() {
    let f = q(&FakeApi::full(), "exposed_services");
    assert_eq!(col(&f, "Service"), ["traefik", "web", "store"]);
    assert!(cell(&f, 0, "Status").starts_with("NOT documented") && cell(&f, 0, "Ports") == "-");
    assert_eq!(cell(&f, 1, "Ports"), "31555");
    assert!(cell(&f, 2, "Status").starts_with("documented: the private store"), "{}", cell(&f, 2, "Status"));
    assert_eq!(cell(&f, 2, "Type"), "NodePort");
    assert_eq!(cell(&q(&FakeApi::full(), "stat_exposed_undocumented"), 0, "exposed, undocumented"), "2");
    // nothing exposed: one "none" row, and the count is zero
    let api = FakeApi::full();
    api.say("Services", r#"{"items":[{"metadata":{"namespace":"a","name":"b"},"spec":{"type":"ClusterIP"}}]}"#);
    assert_eq!(cell(&q(&api, "exposed_services"), 0, "Namespace"), "none");
    assert_eq!(cell(&q(&api, "stat_exposed_undocumented"), 0, "exposed, undocumented"), "0");
}

#[test]
fn volume_claims_with_phase_size_and_usage_where_the_kubelet_reports_it() {
    let f = q(&FakeApi::full(), "pvcs");
    assert_eq!(col(&f, "Claim"), ["db", "odd", "es-data"], "claims that are not Bound come first");
    assert_eq!(col(&f, "Phase"), ["Pending", "Unknown", "Bound"]);
    assert_eq!(cell(&f, 2, "Size"), "10737418240");
    assert_eq!(cell(&f, 2, "Used"), "5368709120");
    assert_eq!(cell(&f, 2, "Used %"), "50");
    assert_eq!(cell(&f, 0, "Size"), "524288000");
    assert_eq!(cell(&f, 0, "Used"), "");
    assert_eq!(cell(&f, 1, "Size"), "");
    assert!(f.meta.notices.iter().any(|n| n.contains("volume statistics could not be read")), "worker-2 does not answer: {:?}", f.meta.notices);
    assert_eq!(cell(&q(&FakeApi::full(), "stat_pvcs_not_bound"), 0, "claims not bound"), "2");
    // no node answers for statistics: usage is blank and the panel says why
    let api = FakeApi::full();
    api.answers.lock().unwrap().remove("NodeStats(\"iyakki-dev\")");
    let f = q(&api, "pvcs");
    assert_eq!(cell(&f, 2, "Used"), "");
    assert!(f.meta.notices.iter().any(|n| n.contains("volume statistics")));
}

// ------------------------------------------------------------------------------------------------------ cluster down, metrics missing

#[test]
fn a_cluster_that_is_not_running_is_a_state_not_an_error() {
    let api = FakeApi::empty();
    let c = q(&api, "cluster");
    assert_eq!(cell(&c, 0, "State"), "not running");
    assert!(cell(&c, 0, "Detail").contains("The cluster is not running") && cell(&c, 0, "Detail").contains("k3s is stopped"), "{}", cell(&c, 0, "Detail"));
    for name in view_names() {
        let f = q(&api, name);
        let text = serde_json::to_string(&f).unwrap() + &f.meta.notices.join(" ");
        assert!(text.contains("not running"), "{name} does not say the cluster is not running: {text}");
        assert!(f.rows.iter().all(|r| r.len() == f.columns.len()), "{name}");
    }
    // a table says it in a row, a chart or a number carries a notice
    assert_eq!(cell(&q(&api, "pod_problems"), 0, "Namespace"), "cluster is not running");
    assert!(q(&api, "pods_by_phase").is_empty());
    // not installed reads the same way
    api.fail("Nodes", no_cluster("k3s is not installed by IYAKKI on this machine"));
    assert!(cell(&q(&api, "cluster"), 0, "Detail").contains("not installed"));
}

#[test]
fn an_api_error_that_is_not_a_stopped_cluster_is_shown_in_the_status_and_stays_an_error_elsewhere() {
    let api = FakeApi::full();
    api.fail("Nodes", DashError::new(ErrKind::Network, "kubectl said: Forbidden"));
    assert_eq!(cell(&q(&api, "cluster"), 0, "State"), "not answering");
    let e = run_view(api.as_ref(), "nodes", now()).unwrap_err();
    assert!(e.message.contains("Forbidden"));
}

#[test]
fn missing_metrics_are_said_plainly_and_do_not_hide_the_rest() {
    let api = FakeApi::full();
    let e = DashError::new(ErrKind::Missing, "the metrics API is not answering (Error from server (ServiceUnavailable))").fix("k3s ships metrics-server in kube-system; it needs a minute after the cluster starts.");
    api.fail("NodeMetrics", e.clone());
    api.fail("PodMetrics", e);
    let u = q(&api, "node_usage");
    assert_eq!(cell(&u, 0, "Node"), "not available");
    assert!(cell(&u, 0, "Detail").contains("metrics API is not answering") && cell(&u, 0, "Detail").contains("metrics-server"));
    assert!(q(&api, "namespace_cpu").meta.notices[0].contains("metrics API"));
    assert!(q(&api, "namespace_cpu").is_empty());
    assert!(cell(&q(&api, "cluster"), 0, "Metrics (CPU, memory)").starts_with("not available: the metrics API is not answering"));
    assert_eq!(cell(&q(&api, "cluster"), 0, "State"), "degraded");
    assert_eq!(col(&q(&api, "nodes"), "Node").len(), 2);
}

// ------------------------------------------------------------------------------------------------------ broken, empty, huge, odd

#[test]
fn broken_answers_never_panic_and_empty_lists_say_none() {
    for body in ["", "null", "[]", "[1,2]", "\"x\"", "42", "{", "{\"items\":", "<html>login</html>"] {
        let api = FakeApi::empty();
        api.all(body);
        for name in view_names() {
            match run_view(api.as_ref(), name, now()) {
                Ok(f) => assert!(f.rows.iter().all(|r| r.len() == f.columns.len()), "{name} {body:?}"),
                Err(e) => assert!(e.kind == ErrKind::Parse, "{name} {body:?}: {e}"),
            }
        }
        // the status row still answers
        assert!(!cell(&q(&api, "cluster"), 0, "State").is_empty());
    }
    for body in ["{}", r#"{"items":null}"#, r#"{"items":5}"#, r#"{"items":[]}"#, r#"{"items":[null,1,"x",[],{}]}"#] {
        let api = FakeApi::empty();
        api.all(body);
        for name in view_names() {
            let f = run_view(api.as_ref(), name, now()).unwrap_or_else(|e| panic!("{name} {body}: {e}"));
            assert!(f.rows.iter().all(|r| r.len() == f.columns.len()), "{name} {body}");
        }
    }
    let api = FakeApi::empty();
    api.all(r#"{"items":[]}"#);
    assert_eq!(cell(&q(&api, "pod_problems"), 0, "Namespace"), "none");
    assert_eq!(cell(&q(&api, "pod_problems"), 0, "Reason"), "nothing to report");
    assert_eq!(cell(&q(&api, "stat_pods_with_problems"), 0, "pods with problems"), "0");
    assert!(q(&api, "pods_by_phase").is_empty());
}

fn pod(i: usize, phase: &str, waiting: &str, restarts: i64) -> String {
    let st = if waiting.is_empty() { r#""state":{"running":{}}"#.to_string() } else { format!(r#""state":{{"waiting":{{"reason":"{waiting}","message":"m{i}"}}}}"#) };
    format!(r#"{{"metadata":{{"namespace":"ns{}","name":"pod-{i}"}},"spec":{{"nodeName":"n"}},"status":{{"phase":"{phase}","containerStatuses":[{{"name":"c","ready":false,"restartCount":{restarts},{st}}}]}}}}"#, i % 40)
}

#[test]
fn a_huge_cluster_is_cut_with_a_notice_and_stays_quick() {
    let api = FakeApi::full();
    let items: Vec<String> = (0..5000).map(|i| pod(i, "Running", if i % 2 == 0 { "CrashLoopBackOff" } else { "" }, (i % 97) as i64)).collect();
    api.say("Pods", &format!(r#"{{"items":[{}]}}"#, items.join(",")));
    let t = std::time::Instant::now();
    let f = q(&api, "pod_problems");
    assert_eq!(f.rows.len(), 200, "the table is capped");
    assert!(f.meta.notices.iter().any(|n| n.contains("2500 pods have a problem") && n.contains("200")), "{:?}", f.meta.notices);
    assert_eq!(cell(&f, 0, "Restarts"), "96", "worst first");
    let r: Vec<i64> = col(&f, "Restarts").iter().map(|x| x.parse().unwrap()).collect();
    assert!(r.windows(2).all(|w| w[0] >= w[1]));
    assert_eq!(cell(&q(&api, "stat_pods_with_problems"), 0, "pods with problems"), "2500");
    assert_eq!(q(&api, "pods_by_namespace").rows.len(), 30, "namespaces are capped too");
    assert_eq!(q(&api, "top_restarts").rows.len(), 10);
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    // more items than the limit: the first ones only, with a notice
    let items: Vec<String> = (0..20_050).map(|i| pod(i, "Running", "", 0)).collect();
    api.say("Pods", &format!(r#"{{"items":[{}]}}"#, items.join(",")));
    let f = q(&api, "pods_by_phase");
    assert_eq!(cell(&f, 0, "Pods"), "20000");
    assert!(f.meta.notices.iter().any(|n| n.contains("only the first 20000 pods")));
}

#[test]
fn an_answer_larger_than_the_limit_is_refused_with_a_sentence() {
    let api = FakeApi::full();
    api.say("Pods", &format!(r#"{{"items":[],"pad":"{}"}}"#, "x".repeat(MAX_RESPONSE_BYTES + 10)));
    let e = run_view(api.as_ref(), "pods_by_phase", now()).unwrap_err();
    assert_eq!(e.kind, ErrKind::Limit);
    assert!(e.to_string().contains("8 MB"));
}

#[test]
fn odd_values_are_tolerated() {
    let api = FakeApi::full();
    let long = "z".repeat(5000);
    let odd = format!(
        r#"{{"items":[
        {{"metadata":{{"namespace":"ünï-ns","name":"pödé-☃"}},"status":{{"phase":"Running","containerStatuses":[{{"name":"c","restartCount":"7","state":{{"waiting":{{"reason":"CrashLoopBackOff","message":"{long}"}}}}}}]}}}},
        {{"metadata":{{"namespace":"a\nb","name":"neg"}},"status":{{"phase":"Weird","containerStatuses":[{{"name":"c","restartCount":-5,"state":{{"waiting":{{"reason":"X"}}}}}}]}}}},
        {{"metadata":{{"namespace":"big","name":"big"}},"status":{{"phase":"Running","containerStatuses":[{{"name":"c","restartCount":9223372036854775807,"state":{{"waiting":{{"reason":"CrashLoopBackOff"}}}}}}]}}}},
        {{"status":{{}}}},
        {{"metadata":{{"name":"nophase"}}}}
        ]}}"#
    );
    api.say("Pods", &odd);
    let f = q(&api, "pod_problems");
    assert!(f.rows.iter().all(|r| r.len() == f.columns.len()));
    assert!(f.rows.iter().all(|r| r.iter().all(|c| c.as_str().is_none_or(|s| s.chars().count() <= 200 && !s.contains('\n')))), "free text is cut to a line");
    assert!(col(&f, "Pod").contains(&"pödé-☃".to_string()));
    let phases = col(&q(&api, "pods_by_phase"), "Phase");
    assert!(phases.contains(&"Weird".to_string()) && phases.contains(&"Unknown".to_string()), "{phases:?}");
    // an event with a huge count, a deployment with negative replicas, a claim with nonsense sizes
    api.say("Events", r#"{"items":[{"type":"Warning","reason":"R","message":"m","count":9223372036854775807,"lastTimestamp":"2026-10-28T11:59:59Z","involvedObject":{"kind":"Pod","name":"p"}},{"type":"Warning","lastTimestamp":"not a time"},{"type":"Warning","lastTimestamp":"2099-01-01T00:00:00Z","reason":"Future"}]}"#);
    let f = q(&api, "warning_events");
    assert_eq!(col(&f, "Reason"), ["Future", "R"], "a time in the future is kept, one that is not a time is left out");
    api.say("Deployments", r#"{"items":[{"metadata":{"name":"n"},"spec":{"replicas":-3}},{"metadata":{"name":"m"},"spec":{"replicas":"x"},"status":{"availableReplicas":-1}}]}"#);
    assert!(q(&api, "deployments_unavailable").rows.len() <= 2);
    api.say("Pvcs", r#"{"items":[{"metadata":{"name":"c"},"status":{"phase":"Bound","capacity":{"storage":"-1"}}},{"metadata":{"name":"d"},"spec":{"resources":{"requests":{"storage":"huge"}}},"status":{"phase":"Bound"}}]}"#);
    assert_eq!(q(&api, "pvcs").rows.len(), 2);
    api.say("PodMetrics", r#"{"items":[{"metadata":{"namespace":"a","name":"b"}},{"metadata":{"namespace":"a","name":"c"},"containers":[{"usage":{"cpu":"1e400","memory":"1Gi"}}]},{"metadata":{"namespace":"a","name":"d"},"containers":[{"usage":{"cpu":"1","memory":"1Gi"}}]}]}"#);
    let f = q(&api, "pod_usage");
    assert_eq!(col(&f, "Pod"), ["d"]);
}

#[test]
fn identical_names_in_different_namespaces_stay_apart() {
    let api = FakeApi::full();
    api.say("Pods", &format!(r#"{{"items":[{},{}]}}"#, pod(1, "Pending", "", 0).replace("ns1", "a"), pod(1, "Pending", "", 0).replace("ns1", "b")));
    assert_eq!(col(&q(&api, "pod_problems"), "Namespace"), ["a", "b"]);
}

// ------------------------------------------------------------------------------------------------------ read-only

#[test]
fn only_view_names_are_accepted_and_writes_are_refused_before_the_cluster_is_asked() {
    let api = FakeApi::full();
    let src = Kube::with_api("k", api.clone());
    for text in ["delete pods --all", "kubectl delete ns default", "apply -f x.yaml", "scale deploy web --replicas=0", "rollout restart deploy/web", "exec -it pod -- sh", "patch node x", "cordon iyakki-dev", "drain iyakki-dev", "DELETE", "label nodes a=b", "run x --image=y", "k3s kubectl get secrets"] {
        let e = src.query(&QueryRequest::new(text, range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::ReadOnly, "{text}: {e}");
    }
    for text in ["", "   ", "get pods", "get secrets", "nodes pods", "nodes; delete pods", "nodes && rm -rf /", "../../etc/passwd", "secrets", "logs web", "nodes -o yaml", "$(id)", "node"] {
        let e = src.query(&QueryRequest::new(text, range()), &Cancel::new()).unwrap_err();
        assert!(e.kind == ErrKind::BadQuery || e.kind == ErrKind::ReadOnly, "{text}: {e}");
    }
    assert!(api.asked.lock().unwrap().is_empty(), "nothing was asked of the cluster for any of them");
    assert_eq!(view_name(" NODES; ").unwrap(), "nodes");
    assert!(src.query(&QueryRequest::new("Nodes", range()), &Cancel::new()).is_ok());
    assert!(src.query(&QueryRequest::new("nodes", range()), &{
        let c = Cancel::new();
        c.cancel();
        c
    }).is_err());
}

/// A kubectl that records every call.
struct Spy {
    calls: Mutex<Vec<(bool, Vec<String>)>>,
    reply: Mutex<Result<String, String>>,
}

impl Spy {
    fn new(reply: Result<String, String>) -> Arc<Spy> {
        Arc::new(Spy { calls: Mutex::new(vec![]), reply: Mutex::new(reply) })
    }
}

impl Kubectl for Spy {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.calls.lock().unwrap().push((false, args.iter().map(|s| s.to_string()).collect()));
        self.reply.lock().unwrap().clone()
    }
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.calls.lock().unwrap().push((true, args.iter().map(|s| s.to_string()).collect()));
        self.reply.lock().unwrap().clone()
    }
}

#[test]
fn every_look_at_the_cluster_is_a_plain_get_without_secrets_logs_or_writes() {
    let spy = Spy::new(Ok("{}".into()));
    let c = KubeCluster::new(spy.clone(), Duration::ZERO);
    let all = [KubeRead::Version, KubeRead::Nodes, KubeRead::Pods, KubeRead::Events, KubeRead::Deployments, KubeRead::Services, KubeRead::Pvcs, KubeRead::NodeMetrics, KubeRead::PodMetrics, KubeRead::NodeStats("iyakki-dev".into())];
    for k in &all {
        c.kube_read(k).unwrap();
    }
    let calls = spy.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), all.len());
    for (big, args) in &calls {
        assert!(*big, "large answers use the large limit: {args:?}");
        assert!(args[0] == "get" || args[0] == "version", "{args:?}");
        let line = args.join(" ").to_ascii_lowercase();
        for bad in ["secret", "delete", "apply", "create", "patch", "scale", "exec", "logs", "edit", "replace", "drain", "cordon", "-f ", "--token", "--as", "port-forward", "proxy "] {
            assert!(!line.contains(bad), "{line} contains {bad}");
        }
        if args[0] == "get" && args[1] == "--raw" {
            assert!(args[2].starts_with("/apis/metrics.k8s.io/v1beta1/") || args[2].starts_with("/api/v1/nodes/") && args[2].ends_with("/proxy/stats/summary"), "{args:?}");
        }
    }
    // a node name that is not a name never reaches kubectl
    let n = calls.len();
    for bad in ["", "../x", "a b", "A", "x;y", "x/y", "x?y=1", "node\n"] {
        assert!(c.kube_read(&KubeRead::NodeStats(bad.into())).is_err(), "{bad:?}");
    }
    assert_eq!(spy.calls.lock().unwrap().len(), n);
}

#[test]
fn answers_are_remembered_briefly_so_panels_asking_together_make_one_call() {
    let spy = Spy::new(Ok(r#"{"items":[]}"#.into()));
    let c = KubeCluster::new(spy.clone(), Duration::from_secs(30));
    for _ in 0..5 {
        c.kube_read(&KubeRead::Pods).unwrap();
    }
    c.kube_read(&KubeRead::Nodes).unwrap();
    assert_eq!(spy.calls.lock().unwrap().len(), 2);
    // concurrent askers share one call too
    let spy = Spy::new(Ok(r#"{"items":[]}"#.into()));
    let c = Arc::new(KubeCluster::new(spy.clone(), Duration::from_secs(30)));
    let hs: Vec<_> = (0..8).map(|_| { let c = c.clone(); std::thread::spawn(move || c.kube_read(&KubeRead::Pods).unwrap()) }).collect();
    hs.into_iter().for_each(|h| { h.join().unwrap(); });
    assert_eq!(spy.calls.lock().unwrap().len(), 1);
    // a failure is not remembered
    let spy = Spy::new(Err("k3s is stopped".into()));
    let c = KubeCluster::new(spy.clone(), Duration::from_secs(30));
    assert!(c.kube_read(&KubeRead::Pods).is_err());
    *spy.reply.lock().unwrap() = Ok("{}".into());
    assert!(c.kube_read(&KubeRead::Pods).is_ok());
}

#[test]
fn what_kubectl_says_becomes_the_right_kind_of_answer() {
    let down = |m: &str| KubeCluster::new(Spy::new(Err(m.into())), Duration::ZERO).kube_read(&KubeRead::Nodes).unwrap_err();
    for m in ["k3s is stopped", "k3s is not installed by IYAKKI on this machine", "The connection to the server was refused", "Unable to connect to the server: dial tcp: i/o timeout"] {
        assert!(down(m).message.starts_with("the cluster is not available"), "{m}");
    }
    let other = down("Error from server (Forbidden): nodes is forbidden");
    assert!(other.message.starts_with("kubectl said") && !other.message.starts_with("the cluster is not available"));
    let metrics = KubeCluster::new(Spy::new(Err("Error from server (ServiceUnavailable): the server is currently unable to handle the request".into())), Duration::ZERO).kube_read(&KubeRead::PodMetrics).unwrap_err();
    assert_eq!(metrics.kind, ErrKind::Missing);
    assert!(metrics.fix.unwrap().contains("metrics-server"));
    // a stopped cluster is a stopped cluster even for the metrics look
    let m2 = KubeCluster::new(Spy::new(Err("k3s is stopped".into())), Duration::ZERO).kube_read(&KubeRead::NodeMetrics).unwrap_err();
    assert!(m2.message.starts_with("the cluster is not available"));
    // end to end over the real parsing: a stopped cluster through the data source is a state, not an error
    let src = Kube::with_api("k", Arc::new(KubeCluster::new(Spy::new(Err("k3s is stopped".into())), Duration::ZERO)));
    let f = src.query(&QueryRequest::new("cluster", range()), &Cancel::new()).unwrap();
    assert_eq!(cell(&f, 0, "State"), "not running");
}

// ------------------------------------------------------------------------------------------------------ the source and the connector model

#[test]
fn the_source_has_no_address_no_secret_and_only_the_local_cluster() {
    let info = registry::kind("kube").unwrap();
    assert!(info.fields.iter().all(|f| f.kind != FieldKind::Secret && f.kind != FieldKind::Url));
    let mut c = SourceConfig { id: "kube-local".into(), kind: "kube".into(), name: "Local cluster (k3s)".into(), ..Default::default() };
    c.values.insert("cluster".into(), "local".into());
    assert!(validate(&c, &info).is_empty());
    // an address, a token or a user name cannot be saved on it
    let mut bad = c.clone();
    bad.values.insert("url".into(), "https://user:pw@10.0.0.1:6443".into());
    assert!(validate(&bad, &info).join(" ").contains("unknown setting `url`"));
    let mut bad = c.clone();
    bad.secrets.insert("token".into(), "uvault:svc.k".into());
    assert!(!validate(&bad, &info).is_empty());
    // another cluster is refused when the connector is built
    let d = deps(&FakeUp::new(), &[], Arc::new(FakeRunner::new(&[])));
    let mut other = c.clone();
    other.values.insert("cluster".into(), "prod".into());
    let e = registry::build(&other, &d).err().unwrap();
    assert!(e.message.contains("only `local`"), "{e}");
    assert_eq!(registry::build(&c, &d).unwrap().kind(), "kube");
    // with no cluster at all the connector still answers (a state), and nothing is sent anywhere
    let up = FakeUp::new();
    let d = deps(&up, &[], Arc::new(FakeRunner::new(&[])));
    let f = registry::build(&c, &d).unwrap().query(&QueryRequest::new("pod_problems", range()), &Cancel::new()).unwrap();
    assert_eq!(cell(&f, 0, "Namespace"), "cluster is not running");
    assert!(up.urls().is_empty());
}

#[test]
fn test_browse_and_starter() {
    let api = FakeApi::full();
    let s = Kube::with_api("k", api.clone());
    assert_eq!(s.test(&Cancel::new()).unwrap().detail, "Kubernetes v1.31.4+k3s1");
    let names: Vec<String> = s.browse(&[], &Cancel::new()).unwrap().into_iter().map(|n| n.name).collect();
    assert_eq!(names, view_names());
    assert!(s.browse(&["nodes".into()], &Cancel::new()).unwrap().is_empty());
    assert_eq!(s.starter("nodes"), "nodes");
    assert_eq!(s.starter("secrets"), "cluster");
    assert!(Kube::with_api("k", FakeApi::empty()).test(&Cancel::new()).is_err());
}

fn env(tag: &str, api: &Arc<FakeApi>, policy: iyakki_sec::policy::Policy) -> Env {
    let dir = std::env::temp_dir().join(format!("iyakki-dashkube-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = deps(&FakeUp::new(), &[], Arc::new(FakeRunner::new(&[])));
    d.cluster = api.clone();
    Env { user: "amy".into(), policy, data_dir: dir.clone(), home: dir.join("home"), deps: d, now_ms: Arc::new(now), vault_names: Arc::new(Vec::new), vault_set: Arc::new(|_, _| Ok(())), audit: Arc::new(|_, _| {}), cache: crate::dash::multi::FetchCache::new(), people: Arc::new(crate::dash::tools::AnyPeople) }
}

#[test]
fn an_account_without_the_dashboard_app_cannot_read_the_cluster_through_it() {
    let api = FakeApi::full();
    let mut e = env("guest", &api, Role::Guest.template());
    super::seed::seed_defaults(&e.data_dir, &e.home, "amy", true).unwrap();
    assert!(!e.policy.apps.contains(&iyakki_sec::policy::App::Dash));
    assert!(e.source("kube-local").is_err());
    e.policy = Role::Admin.template();
    assert!(e.source("kube-local").is_ok());
    assert!(e.source("Local cluster (k3s)").is_ok(), "by name too");
    assert!(api.asked.lock().unwrap().is_empty(), "building the connector asks the cluster nothing");
}

// ------------------------------------------------------------------------------------------------------ the default dashboard

#[test]
fn the_default_dashboard_is_complete_valid_and_every_panel_answers() {
    let api = FakeApi::full();
    let e = env("board", &api, Role::Admin.template());
    super::seed::seed_defaults(&e.data_dir, &e.home, "amy", true).unwrap();
    let d = e.dashboards().load("k3s-local").unwrap();
    assert_eq!(d.title, "Local cluster (k3s)");
    assert_eq!(d.refresh_s, 30);
    assert_eq!(d.default_source, "kube-local");
    assert_eq!(d.panels.len(), 20);
    let mut ids: Vec<&str> = d.panels.iter().map(|p| p.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 20, "panel ids are unique");
    for (i, a) in d.panels.iter().enumerate() {
        assert!(d.panels.iter().skip(i + 1).all(|b| !a.grid.overlaps(&b.grid)), "{} overlaps", a.id);
        assert!(a.grid.x + a.grid.w <= 24);
    }
    // what each panel shows, from the fixtures; a number panel gets a number, a chart gets labels and values
    use super::model::Visual;
    let cancel = Cancel::new();
    let mut shown = vec![];
    for p in &d.panels {
        assert!(view_names().contains(&p.query.text.as_str()), "{}: {}", p.id, p.query.text);
        let (cfg, src) = e.source(&p.source).unwrap();
        assert_eq!(cfg.kind, "kube");
        let f = src.query(&QueryRequest::new(&p.query.text, rng()), &cancel).unwrap();
        match p.visual {
            Visual::Stat => assert!(f.number(0, 0).is_some(), "{}", p.id),
            Visual::Donut | Visual::Bar => assert!(f.columns.len() == 2 && f.number(0, 1).is_some(), "{}", p.id),
            Visual::Table => assert!(!f.rows.is_empty(), "{}", p.id),
            other => panic!("unexpected visual {other:?}"),
        }
        for k in p.options.column_units.keys() {
            assert!(f.column_index(k).is_some(), "{}: unit for a column that does not exist: {k}", p.id);
        }
        shown.push((p.id.clone(), f.rows.len()));
    }
    let n = |id: &str| shown.iter().find(|s| s.0 == id).unwrap().1;
    assert_eq!((n("nodes"), n("problems"), n("events"), n("exposed")), (2, 7, 2, 3));
    // the same panels with the cluster down: every one answers, none errors
    let down = env("board-down", &FakeApi::empty(), Role::Admin.template());
    super::seed::seed_defaults(&down.data_dir, &down.home, "amy", true).unwrap();
    for p in &d.panels {
        let (_, src) = down.source(&p.source).unwrap();
        src.query(&QueryRequest::new(&p.query.text, range()), &cancel).unwrap_or_else(|e| panic!("{} with the cluster down: {e}", p.id));
    }
}

#[test]
fn the_default_dashboard_passes_the_credential_scrub_and_holds_no_address() {
    let t = super::templates::find("k3s-local").unwrap();
    assert_eq!(t.kind, "kube");
    let mut d = t.instantiate("kube-local", "x");
    assert!(super::store::scrub_dashboard(&mut d).is_empty());
    let json = serde_json::to_string(&d).unwrap();
    assert!(!json.contains("http") && !json.contains("uvault") && !json.contains("token"));
}
