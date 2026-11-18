//! The K8S tile's engine against a fake kubectl with canned JSON. No real cluster is ever asked.

use super::*;
use crate::k8s::FakeKube;
use serde_json::json;

const NOW: u64 = 1_800_000_000;

fn ts(ago: u64) -> String {
    let t = (NOW - ago) as i64;
    let (days, rem) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn target() -> Target {
    Target { kubeconfig: "/k/config".into(), context: "dev".into(), ..Default::default() }
}

fn pod(name: &str, ns: &str, phase: &str, ready: bool, restarts: i64, state: Value, ago: u64) -> Value {
    json!({"kind":"Pod","metadata":{"name":name,"namespace":ns,"creationTimestamp":ts(ago),"labels":{"app":"web"},"ownerReferences":[{"kind":"ReplicaSet","name":"web-7d9"}]},
        "spec":{"nodeName":"node-a","containers":[{"name":"app","image":"reg/web:1"},{"name":"side","image":"reg/side:1"}]},
        "status":{"phase":phase,"podIP":"10.42.0.9","containerStatuses":[{"name":"app","ready":ready,"restartCount":restarts,"state":state,"lastState":{}},{"name":"side","ready":ready,"restartCount":0,"state":{"running":{}},"lastState":{}}]}})
}

fn list(items: Vec<Value>) -> String {
    json!({"kind":"List","items":items}).to_string()
}

fn all_phase_pods() -> Vec<Value> {
    let mut term = pod("web-term", "shop", "Running", true, 0, json!({"running":{}}), 500);
    term["metadata"]["deletionTimestamp"] = json!(ts(5));
    vec![
        pod("web-run", "shop", "Running", true, 0, json!({"running":{}}), 3 * 86_400),
        pod("web-pending", "shop", "Pending", false, 0, json!({"waiting":{"reason":"ContainerCreating"}}), 40),
        pod("web-crash", "shop", "Running", false, 14, json!({"waiting":{"reason":"CrashLoopBackOff"}}), 7200),
        pod("job-done", "shop", "Succeeded", false, 0, json!({"terminated":{"reason":"Completed"}}), 900),
        term,
        pod("web-pull", "shop", "Pending", false, 0, json!({"waiting":{"reason":"ImagePullBackOff"}}), 120),
    ]
}

fn cell<'a>(r: &'a Row, cols: &[Col], title: &str) -> &'a str {
    let i = cols.iter().position(|c| c.title == title).unwrap_or_else(|| panic!("no column {title}"));
    &r.cells[i]
}

#[test]
fn every_kind_has_columns_that_line_up_with_its_cells() {
    let samples: Vec<(Kind, Value)> = vec![
        (Kind::Pods, pod("p", "shop", "Running", true, 0, json!({"running":{}}), 60)),
        (Kind::Deployments, json!({"metadata":{"name":"d","namespace":"shop","creationTimestamp":ts(60)},"spec":{"replicas":3},"status":{"readyReplicas":2,"updatedReplicas":3,"availableReplicas":2}})),
        (Kind::StatefulSets, json!({"metadata":{"name":"s","namespace":"shop","creationTimestamp":ts(60)},"spec":{"replicas":2},"status":{"readyReplicas":2}})),
        (Kind::DaemonSets, json!({"metadata":{"name":"ds","namespace":"shop","creationTimestamp":ts(60)},"status":{"desiredNumberScheduled":3,"numberReady":3,"updatedNumberScheduled":3,"numberAvailable":3}})),
        (Kind::ReplicaSets, json!({"metadata":{"name":"rs","namespace":"shop","creationTimestamp":ts(60)},"spec":{"replicas":2},"status":{"replicas":2,"readyReplicas":2}})),
        (Kind::Jobs, json!({"metadata":{"name":"j","namespace":"shop","creationTimestamp":ts(60)},"spec":{"completions":1},"status":{"succeeded":1}})),
        (Kind::CronJobs, json!({"metadata":{"name":"cj","namespace":"shop","creationTimestamp":ts(60)},"spec":{"schedule":"*/5 * * * *","suspend":false},"status":{"lastScheduleTime":ts(120),"active":[{}]}})),
        (Kind::Services, json!({"metadata":{"name":"svc","namespace":"shop","creationTimestamp":ts(60)},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.5","ports":[{"port":80,"protocol":"TCP","targetPort":8080}]}})),
        (Kind::Ingresses, json!({"metadata":{"name":"ing","namespace":"shop","creationTimestamp":ts(60)},"spec":{"ingressClassName":"traefik","rules":[{"host":"shop.example"}]},"status":{"loadBalancer":{"ingress":[{"ip":"10.0.0.7"}]}}})),
        (Kind::ConfigMaps, json!({"metadata":{"name":"cm","namespace":"shop","creationTimestamp":ts(60)},"data":{"a":"1","b":"2"}})),
        (Kind::Secrets, json!({"metadata":{"name":"sec","namespace":"shop","creationTimestamp":ts(60)},"type":"Opaque","data":{"pw":"aHVudGVyMg=="}})),
        (Kind::Pvcs, json!({"metadata":{"name":"pvc","namespace":"shop","creationTimestamp":ts(60)},"spec":{"storageClassName":"local-path"},"status":{"phase":"Bound","capacity":{"storage":"1Gi"}}})),
        (Kind::Events, json!({"type":"Warning","reason":"BackOff","message":"Back-off restarting","involvedObject":{"kind":"Pod","name":"web-crash"},"metadata":{"name":"e1","namespace":"shop","creationTimestamp":ts(60)},"lastTimestamp":ts(30)})),
        (Kind::Nodes, json!({"metadata":{"name":"node-a","creationTimestamp":ts(60),"labels":{"node-role.kubernetes.io/control-plane":"true"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"allocatable":{"cpu":"4","memory":"16314232Ki"},"nodeInfo":{"kubeletVersion":"v1.30.4+k3s1"},"addresses":[{"type":"InternalIP","address":"10.0.0.2"}]}})),
        (Kind::Namespaces, json!({"metadata":{"name":"shop","creationTimestamp":ts(60)},"status":{"phase":"Active"}})),
        (Kind::ServiceAccounts, json!({"metadata":{"name":"default","namespace":"shop","creationTimestamp":ts(60)},"secrets":[{"name":"x"}]})),
        (Kind::Crds, json!({"metadata":{"name":"widgets.example.com","creationTimestamp":ts(60)},"spec":{"group":"example.com","scope":"Namespaced","names":{"kind":"Widget"}}})),
    ];
    assert_eq!(samples.len(), Kind::ALL.len(), "a sample for every kind");
    for (kind, item) in samples {
        for with_ns in [false, true] {
            let cols = columns(kind, with_ns);
            let (rows, total) = parse_rows(kind, &json!({"items":[item.clone()]}), NOW, with_ns);
            assert_eq!(total, 1);
            assert_eq!(rows[0].cells.len(), cols.len(), "{kind:?} ns={with_ns}");
            assert_eq!(rows[0].keys.len(), cols.len());
            assert_eq!(cols.iter().any(|c| c.title == "Namespace"), with_ns && kind.namespaced(), "{kind:?}");
            assert!(cols.iter().any(|c| c.prio == 0), "something is always shown");
        }
    }
}

#[test]
fn pods_in_every_phase() {
    let (rows, total) = parse_rows(Kind::Pods, &json!({"items": all_phase_pods()}), NOW, false);
    let cols = columns(Kind::Pods, false);
    assert_eq!(total, 6);
    let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
    assert_eq!(cell(by("web-run"), &cols, "Status"), "Running");
    assert_eq!(cell(by("web-run"), &cols, "Ready"), "2/2");
    assert_eq!(cell(by("web-run"), &cols, "Age"), "3d");
    assert_eq!(cell(by("web-run"), &cols, "IP"), "10.42.0.9");
    assert_eq!(cell(by("web-run"), &cols, "Node"), "node-a");
    assert_eq!(by("web-run").health, Some(Health::Good));
    assert_eq!(cell(by("web-pending"), &cols, "Status"), "ContainerCreating");
    assert_eq!(by("web-pending").health, Some(Health::Warn));
    assert_eq!(cell(by("web-crash"), &cols, "Status"), "CrashLoopBackOff");
    assert_eq!(cell(by("web-crash"), &cols, "Restarts"), "14");
    assert_eq!(cell(by("web-crash"), &cols, "Ready"), "0/2");
    assert_eq!(by("web-crash").health, Some(Health::Bad));
    assert_eq!(cell(by("job-done"), &cols, "Status"), "Completed");
    assert_eq!(by("job-done").health, Some(Health::Idle));
    assert_eq!(cell(by("web-term"), &cols, "Status"), "Terminating");
    assert_eq!(by("web-term").health, Some(Health::Warn));
    assert_eq!(cell(by("web-pull"), &cols, "Status"), "ImagePullBackOff");
    assert_eq!(by("web-pull").health, Some(Health::Bad));
    let line = count_line(Kind::Pods, &rows, total);
    assert!(line.starts_with("6 pods \u{b7} 4 not ready"), "{line}");
}

#[test]
fn age_strings() {
    assert_eq!(age_text(5), "5s");
    assert_eq!(age_text(59), "59s");
    assert_eq!(age_text(60), "1m");
    assert_eq!(age_text(7200), "2h");
    assert_eq!(age_text(86_400 * 3 + 5), "3d");
    assert_eq!(human_mem("16314232Ki"), "15.6Gi");
    assert_eq!(human_mem("512Mi"), "512Mi");
    assert_eq!(human_mem("weird"), "weird");
}

#[test]
fn workloads_services_and_the_rest() {
    let dep = json!({"items":[
        {"metadata":{"name":"web","namespace":"shop","creationTimestamp":ts(3600)},"spec":{"replicas":3},"status":{"readyReplicas":2,"updatedReplicas":3,"availableReplicas":2}},
        {"metadata":{"name":"off","namespace":"shop","creationTimestamp":ts(3600)},"spec":{"replicas":0},"status":{}},
        {"metadata":{"name":"dead","namespace":"shop","creationTimestamp":ts(3600)},"spec":{"replicas":2},"status":{}}]});
    let (rows, _) = parse_rows(Kind::Deployments, &dep, NOW, false);
    let cols = columns(Kind::Deployments, false);
    assert_eq!(cell(&rows[0], &cols, "Ready"), "2/3");
    assert_eq!(cell(&rows[0], &cols, "Up-to-date"), "3");
    assert_eq!(cell(&rows[0], &cols, "Available"), "2");
    assert_eq!(rows[0].health, Some(Health::Warn));
    assert_eq!(rows[1].health, Some(Health::Idle));
    assert_eq!(rows[2].health, Some(Health::Bad));

    let svc = json!({"items":[
        {"metadata":{"name":"a","namespace":"shop","creationTimestamp":ts(60)},"spec":{"type":"ClusterIP","clusterIP":"10.43.1.1","ports":[{"port":80,"protocol":"TCP"}]}},
        {"metadata":{"name":"b","namespace":"shop","creationTimestamp":ts(60)},"spec":{"type":"NodePort","clusterIP":"10.43.1.2","ports":[{"port":80,"protocol":"TCP","targetPort":8080,"nodePort":30080}]}},
        {"metadata":{"name":"c","namespace":"shop","creationTimestamp":ts(60)},"spec":{"type":"LoadBalancer","clusterIP":"10.43.1.3","ports":[{"port":443,"protocol":"TCP"}]},"status":{"loadBalancer":{"ingress":[{"ip":"192.0.2.10"}]}}},
        {"metadata":{"name":"d","namespace":"shop","creationTimestamp":ts(60)},"spec":{"type":"LoadBalancer","clusterIP":"10.43.1.4","ports":[{"port":443,"protocol":"TCP"}]}}]});
    let (rows, _) = parse_rows(Kind::Services, &svc, NOW, false);
    let cols = columns(Kind::Services, false);
    assert_eq!((cell(&rows[0], &cols, "Type"), cell(&rows[0], &cols, "Ports"), cell(&rows[0], &cols, "External")), ("ClusterIP", "80/TCP", "\u{2014}"));
    assert_eq!(cell(&rows[1], &cols, "Ports"), "80/TCP\u{2192}8080:30080");
    assert_eq!(cell(&rows[2], &cols, "External"), "192.0.2.10");
    assert_eq!(cell(&rows[3], &cols, "External"), "<pending>");

    let (jobs, _) = parse_rows(Kind::Jobs, &json!({"items":[{"metadata":{"name":"f","creationTimestamp":ts(5)},"spec":{"completions":1},"status":{"failed":2}}]}), NOW, false);
    assert_eq!(jobs[0].cells[2], "2 failed");
    assert_eq!(jobs[0].health, Some(Health::Bad));
    let (ns, _) = parse_rows(Kind::Namespaces, &json!({"items":[{"metadata":{"name":"gone"},"status":{"phase":"Terminating"}}]}), NOW, false);
    assert_eq!(ns[0].health, Some(Health::Warn));
}

#[test]
fn events_newest_first_with_warnings_marked() {
    let ev = |name: &str, ty: &str, ago: u64| json!({"type":ty,"reason":"R","message":format!("m {name}\nsecond line"),"involvedObject":{"kind":"Pod","name":"p"},"metadata":{"name":name,"namespace":"shop"},"lastTimestamp":ts(ago)});
    let k = FakeKube::new();
    k.answer("-n shop get events -o json", &list(vec![ev("old", "Normal", 900), ev("new", "Warning", 10), ev("mid", "Normal", 300)]));
    let l = list_fn(&k, Scope::Ns("shop".into()), Kind::Events);
    assert_eq!(l.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["new", "mid", "old"], "newest first");
    assert!(l.rows[0].warn && !l.rows[1].warn);
    let cols = columns(Kind::Events, false);
    assert_eq!(cell(&l.rows[0], &cols, "Object"), "pod/p");
    assert!(!cell(&l.rows[0], &cols, "Message").contains('\n'), "one line");
    assert!(count_line(Kind::Events, &l.rows, l.total).contains("1 warnings"));
}

fn list_fn(k: &FakeKube, scope: Scope, kind: Kind) -> Listing {
    super::list(k, &target(), &scope, kind, NOW)
}

#[test]
fn one_call_per_list_with_the_right_arguments() {
    let k = FakeKube::new();
    k.answer("get pods -o json", &list(all_phase_pods()));
    let l = list_fn(&k, Scope::Ns("shop".into()), Kind::Pods);
    assert_eq!(l.cluster, Cluster::Ok);
    assert_eq!(k.calls(), vec!["--kubeconfig /k/config --context dev --request-timeout=8s -n shop get pods -o json"], "one call");
    let k = FakeKube::new();
    k.answer("get pods -A -o json", &list(vec![]));
    list_fn(&k, Scope::All, Kind::Pods);
    assert_eq!(k.calls().len(), 1);
    // -A goes after the resource: kubectl 1.37 answers "flags cannot be placed before plugin name" to `-A get pods`
    assert!(k.calls()[0].contains(" get pods -A -o json") && !k.calls()[0].contains(" -A get"), "{:?}", k.calls());
    let k = FakeKube::new();
    k.answer("get nodes -o json", &list(vec![]));
    list_fn(&k, Scope::Ns("shop".into()), Kind::Nodes);
    assert!(!k.calls()[0].contains("-n shop") && !k.calls()[0].contains("-A"), "nodes are not namespaced: {:?}", k.calls());
    // a namespace name that is not a name never reaches kubectl
    let k = FakeKube::new();
    let l = list_fn(&k, Scope::Ns("x;rm -rf".into()), Kind::Pods);
    assert!(matches!(l.cluster, Cluster::Failed(_)));
    assert!(k.calls().is_empty());
}

#[test]
fn nodes_show_usage_only_when_metrics_answer() {
    let node = json!({"metadata":{"name":"node-a","creationTimestamp":ts(86_400),"labels":{"node-role.kubernetes.io/control-plane":"true","node-role.kubernetes.io/master":"true"}},"spec":{"unschedulable":true},
        "status":{"conditions":[{"type":"Ready","status":"True"}],"allocatable":{"cpu":"4","memory":"16314232Ki"},"nodeInfo":{"kubeletVersion":"v1.30.4+k3s1"}}});
    let k = FakeKube::new();
    k.answer("get nodes -o json", &list(vec![node.clone()]));
    k.fail("top nodes", "error: Metrics API not available");
    let l = list_fn(&k, Scope::All, Kind::Nodes);
    assert_eq!(l.metrics, Some(false));
    let cols = columns(Kind::Nodes, false);
    assert_eq!(cell(&l.rows[0], &cols, "CPU use"), "\u{2014}");
    assert_eq!(cell(&l.rows[0], &cols, "Mem use"), "\u{2014}");
    assert_eq!(cell(&l.rows[0], &cols, "CPU"), "4");
    assert_eq!(cell(&l.rows[0], &cols, "Memory"), "15.6Gi");
    assert_eq!(cell(&l.rows[0], &cols, "Status"), "Ready,SchedulingDisabled", "a cordoned node says so");
    assert_eq!(l.rows[0].health, Some(Health::Warn));
    k.answer("top nodes", "node-a   250m   6%   1024Mi   6%\n");
    let l = list_fn(&k, Scope::All, Kind::Nodes);
    assert_eq!(l.metrics, Some(true));
    assert_eq!(cell(&l.rows[0], &cols, "CPU use"), "250m (6%)");
    assert_eq!(cell(&l.rows[0], &cols, "Mem use"), "1024Mi (6%)");
}

#[test]
fn failed_slow_and_missing_calls_are_named() {
    let k = FakeKube::new();
    k.missing();
    assert_eq!(list_fn(&k, Scope::All, Kind::Pods).cluster, Cluster::KubectlMissing);
    let k = FakeKube::new();
    k.fail("get pods", "The connection to the server 127.0.0.1:6443 was refused - did you specify the right host or port?");
    assert!(matches!(list_fn(&k, Scope::All, Kind::Pods).cluster, Cluster::Unreachable(_)));
    let k = FakeKube::new();
    k.fail("get pods", "Error from server (Forbidden): pods is forbidden: User \"x\" cannot list resource \"pods\"");
    assert!(matches!(list_fn(&k, Scope::All, Kind::Pods).cluster, Cluster::Forbidden(_)));
    struct Slow;
    impl Kube for Slow {
        fn run(&self, _: &[String]) -> crate::wsexec::Out {
            crate::wsexec::Out { code: None, timed_out: true, ..Default::default() }
        }
    }
    assert!(matches!(super::list(&Slow, &target(), &Scope::All, Kind::Pods, NOW).cluster, Cluster::Unreachable(m) if m.contains("timed out")));
    // output that hit the cap and so is cut off in the middle
    struct Cut;
    impl Kube for Cut {
        fn run(&self, _: &[String]) -> crate::wsexec::Out {
            crate::wsexec::Out { code: Some(0), stdout: "{\"items\":[{\"metadata\":".into(), truncated: true, ..Default::default() }
        }
    }
    let l = super::list(&Cut, &target(), &Scope::All, Kind::Pods, NOW);
    assert!(matches!(&l.cluster, Cluster::Failed(m) if m.contains("too many")), "{:?}", l.cluster);
}

#[test]
fn huge_lists_keep_a_bounded_number_of_rows() {
    let items: Vec<Value> = (0..MAX_KEPT + 700).map(|i| pod(&format!("p{i:05}"), "shop", "Running", true, 0, json!({"running":{}}), 60)).collect();
    let k = FakeKube::new();
    k.answer("get pods -A -o json", &list(items));
    let l = list_fn(&k, Scope::All, Kind::Pods);
    assert_eq!(l.total, MAX_KEPT + 700);
    assert_eq!(l.rows.len(), MAX_KEPT, "memory is bounded");
    assert!(MAX_SHOWN < MAX_KEPT);
}

#[test]
fn sorting_and_filtering() {
    let (mut rows, _) = parse_rows(Kind::Pods, &json!({"items": all_phase_pods()}), NOW, false);
    let cols = columns(Kind::Pods, false);
    let idx = |t: &str| cols.iter().position(|c| c.title == t).unwrap();
    sort_rows(&mut rows, idx("Restarts"), true);
    assert_eq!(rows[0].name, "web-crash", "numbers by value, biggest first");
    sort_rows(&mut rows, idx("Age"), false);
    assert_eq!(rows[0].name, "web-pending", "youngest first");
    sort_rows(&mut rows, idx("Age"), true);
    assert_eq!(rows[0].name, "web-run", "oldest first (3d)");
    sort_rows(&mut rows, 0, false);
    assert_eq!(rows[0].name, "job-done");
    let hits = filter_rows(&rows, "WEB crash");
    assert_eq!(hits.len(), 1);
    assert_eq!(rows[hits[0]].name, "web-crash");
    assert_eq!(filter_rows(&rows, "").len(), rows.len());
    assert_eq!(filter_rows(&rows, "imagepull").len(), 1, "searches the cells, not only the name");
    assert!(filter_rows(&rows, "nothing-like-this").is_empty());
    assert_eq!(default_sort(Kind::Events, false), (4, false));
    assert_eq!(default_sort(Kind::Events, true), (5, false));
    assert_eq!(default_sort(Kind::Pods, true), (0, false));
}

#[test]
fn namespaces_come_from_one_call_with_pod_counts_and_failing_marks() {
    let k = FakeKube::new();
    let mut items = vec![json!({"kind":"Namespace","metadata":{"name":"shop"},"status":{"phase":"Active"}}), json!({"kind":"Namespace","metadata":{"name":"empty"},"status":{"phase":"Active"}}), json!({"kind":"Namespace","metadata":{"name":"kube-system"},"status":{"phase":"Active"}})];
    items.extend(all_phase_pods());
    items.push(pod("coredns", "kube-system", "Running", true, 0, json!({"running":{}}), 9000));
    k.answer("get namespaces,pods -A -o json", &list(items));
    let l = namespaces(&k, &target(), NOW);
    assert_eq!(k.calls().len(), 1, "one call for all namespaces and pods");
    assert_eq!(l.cluster, Cluster::Ok);
    assert!(l.pods_known);
    let by = |n: &str| l.items.iter().find(|i| i.name == n).unwrap().clone();
    assert_eq!((by("shop").pods, by("shop").failing), (6, 2), "CrashLoopBackOff and ImagePullBackOff fail; Pending, Completed and Terminating do not");
    assert_eq!((by("empty").pods, by("empty").failing), (0, 0));
    assert_eq!((by("kube-system").pods, by("kube-system").failing), (1, 0));
}

#[test]
fn namespaces_fall_back_when_pods_may_not_be_listed() {
    let k = FakeKube::new();
    k.fail("namespaces,pods", "Error from server (Forbidden): pods is forbidden");
    k.answer("get namespaces -o json", &list(vec![json!({"kind":"Namespace","metadata":{"name":"mine"},"status":{"phase":"Active"}})]));
    let l = namespaces(&k, &target(), NOW);
    assert_eq!(l.cluster, Cluster::Ok);
    assert!(!l.pods_known);
    assert_eq!(l.items.len(), 1);
    let k = FakeKube::new();
    k.missing();
    assert_eq!(namespaces(&k, &target(), NOW).cluster, Cluster::KubectlMissing);
}

// ------------------------------------------------------------------------------------------------ details

fn secret() -> Value {
    json!({"kind":"Secret","type":"Opaque","metadata":{"name":"db","namespace":"shop","creationTimestamp":ts(60),
        "annotations":{"kubectl.kubernetes.io/last-applied-configuration":"{\"data\":{\"pw\":\"aHVudGVyMg==\"}}","note":"x"}},
        "data":{"pw":"aHVudGVyMg==","bin":"//79"},"stringData":{"plain":"hunter2-plain"}})
}

#[test]
fn secret_values_are_masked_in_the_yaml_the_summary_and_the_rows() {
    let y = object_yaml(Kind::Secrets, &secret(), false);
    for leak in ["hunter2", "aHVudGVyMg", "//79", "last-applied"] {
        assert!(!y.contains(leak), "{leak} leaked: {y}");
    }
    assert!(y.contains(&format!("pw: {MASK}")) && y.contains(&format!("plain: {MASK}")), "{y}");
    assert!(y.contains("name: db") && y.contains("note: x"));
    let sum = summary(Kind::Secrets, &secret(), NOW).iter().map(|(a, b)| format!("{a} {b}")).collect::<Vec<_>>().join("\n");
    assert!(!sum.contains("hunter2") && !sum.contains("aHVudGVyMg"), "{sum}");
    assert!(sum.contains("bin, pw"), "key names are fine: {sum}");
    let (rows, _) = parse_rows(Kind::Secrets, &json!({"items":[secret()]}), NOW, false);
    let all = format!("{:?}", rows);
    assert!(!all.contains("hunter2") && !all.contains("aHVudGVyMg"), "{all}");
    assert_eq!(rows[0].cells[2], "2");
}

#[test]
fn revealing_decodes_text_and_says_so_for_bytes() {
    let y = object_yaml(Kind::Secrets, &secret(), true);
    assert!(y.contains("pw: hunter2"), "{y}");
    assert!(y.contains("plain: hunter2-plain"));
    assert!(y.contains("(binary, 3 bytes)"), "{y}");
    assert!(!y.contains("last-applied"), "the annotation that copies the values is never shown");
}

#[test]
fn yaml_writer_handles_the_shapes_the_api_returns() {
    let v = json!({"a":1,"b":"two","c":["x",{"k":"v","n":[1,2]}],"d":{"e":true,"f":null,"g":"true","h":"12","i":"a: b","j":"","k":"line1\nline2\n"},"l":[],"m":{}});
    let y = to_yaml(&v);
    assert!(y.contains("a: 1\n") && y.contains("b: two\n"));
    assert!(y.contains("c:\n- x\n- k: v\n  n:\n  - 1\n  - 2\n"), "{y}");
    assert!(y.contains("  e: true\n") && y.contains("  f: null\n"));
    assert!(y.contains("  g: \"true\"\n") && y.contains("  h: \"12\"\n") && y.contains("  i: \"a: b\"\n") && y.contains("  j: \"\"\n"), "{y}");
    assert!(y.contains("  k: |\n    line1\n    line2\n"), "{y}");
    assert!(y.contains("l: []\n") && y.contains("m: {}\n"));
}

#[test]
fn summaries_are_plain_words() {
    let s = summary(Kind::Pods, &all_phase_pods()[2], NOW);
    let get = |k: &str| s.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone()).unwrap_or_default();
    assert!(get("Status").starts_with("CrashLoopBackOff"));
    assert!(get("What it means").contains("starts and then exits"), "the shared explanations are reused");
    assert_eq!(get("Containers ready"), "0/2");
    assert_eq!(get("Restarts"), "14");
    assert!(get("Owned by").contains("recreates"));
    assert!(get("Containers").contains("app \u{2192} reg/web:1"));
    let mut orphan = all_phase_pods()[0].clone();
    orphan["metadata"].as_object_mut().unwrap().remove("ownerReferences");
    let s = summary(Kind::Pods, &orphan, NOW);
    assert!(s.iter().any(|(a, b)| a == "Owned by" && b.contains("deleting it removes it for good")));
    let svc = summary(Kind::Services, &json!({"metadata":{"name":"s","namespace":"n"},"spec":{"type":"NodePort","clusterIP":"10.43.0.1","selector":{"app":"web"},"ports":[{"port":80,"protocol":"TCP","nodePort":30001}]}}), NOW);
    assert!(svc.iter().any(|(a, b)| a == "Sends traffic to pods with" && b == "app=web"));
    assert!(svc.iter().any(|(a, b)| a == "External address" && b == "none"));
}

#[test]
fn get_object_validates_names_and_quotes_nothing() {
    let k = FakeKube::new();
    k.answer("get pods web-run -o json", &all_phase_pods()[0].to_string());
    assert!(get_object(&k, &target(), Kind::Pods, "shop", "web-run").is_ok());
    assert_eq!(k.calls(), vec!["--kubeconfig /k/config --context dev --request-timeout=8s -n shop get pods web-run -o json"]);
    for bad in ["", "-A", "web run", "web;ls", "Web", "a/b", "$(x)", "-n"] {
        assert!(get_object(&k, &target(), Kind::Pods, "shop", bad).is_err(), "{bad:?}");
    }
    assert!(get_object(&k, &target(), Kind::Pods, "sh op", "web").is_err());
    assert_eq!(k.calls().len(), 1, "bad names never reach kubectl");
    // cluster-scoped kinds have no -n
    let k = FakeKube::new();
    k.answer("get nodes node-a", "{}");
    get_object(&k, &target(), Kind::Nodes, "", "node-a").unwrap();
    assert!(!k.calls()[0].contains("-n "));
}

#[test]
fn events_for_an_object() {
    let k = FakeKube::new();
    let ev = |reason: &str, ago: u64, ty: &str| json!({"type":ty,"reason":reason,"message":"m","count":3,"lastTimestamp":ts(ago)});
    k.answer("get events", &list(vec![ev("Pulled", 100, "Normal"), ev("BackOff", 10, "Warning")]));
    let e = object_events(&k, &target(), Kind::Pods, "shop", "web-crash", NOW).unwrap();
    assert_eq!(e[0].reason, "BackOff", "newest first");
    assert_eq!(e[0].count, 3);
    assert!(k.calls()[0].contains("--field-selector involvedObject.name=web-crash,involvedObject.kind=Pod"), "{:?}", k.calls());
    object_events(&k, &target(), Kind::Nodes, "", "node-a", NOW).unwrap();
    assert!(k.calls()[1].contains(" get events -A "), "node events live in any namespace (-A after the verb: kubectl 1.37 refuses it before)");
    assert!(object_events(&k, &target(), Kind::Pods, "shop", "a b", NOW).is_err());
}

#[test]
fn related_objects_for_pods_services_workloads_and_nodes() {
    let k = FakeKube::new();
    let rs = json!({"metadata":{"name":"web-7d9","namespace":"shop","ownerReferences":[{"kind":"Deployment","name":"web"}]}});
    let svc = |name: &str, app: &str| json!({"metadata":{"name":name,"namespace":"shop"},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.2","selector":{"app":app},"ports":[{"port":80,"protocol":"TCP"}]}});
    k.answer("get replicasets web-7d9 -o json", &rs.to_string());
    k.answer("get services -o json", &list(vec![svc("front", "web"), svc("other", "db")]));
    let r = related(&k, &target(), Kind::Pods, "shop", &all_phase_pods()[0]);
    let names: Vec<String> = r.iter().map(|x| format!("{}:{:?}:{}", x.relation, x.kind, x.name)).collect();
    assert!(names.contains(&"Owner:ReplicaSets:web-7d9".to_string()), "{names:?}");
    assert!(names.contains(&"Controlled by:Deployments:web".to_string()), "{names:?}");
    assert!(names.contains(&"Service selecting it:Services:front".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("other")), "a service for another app is not related");
    assert!(names.contains(&"Runs on node:Nodes:node-a".to_string()));

    let k = FakeKube::new();
    k.answer("get pods -l app=web", &list(all_phase_pods()[..2].to_vec()));
    k.answer("get endpoints front", &json!({"subsets":[{"addresses":[{"ip":"1"},{"ip":"2"}],"notReadyAddresses":[{"ip":"3"}]}]}).to_string());
    let r = related(&k, &target(), Kind::Services, "shop", &svc("front", "web"));
    assert_eq!(r.iter().filter(|x| x.relation == "Pod behind it").count(), 2);
    assert!(r.iter().any(|x| x.relation == "Endpoints" && x.note == "2 ready, 1 not ready"));

    let k = FakeKube::new();
    k.answer("get pods -l app=web", &list(vec![all_phase_pods()[0].clone()]));
    k.answer("get services -o json", &list(vec![svc("front", "web")]));
    let dep = json!({"metadata":{"name":"web","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{"labels":{"app":"web"}}}}});
    let r = related(&k, &target(), Kind::Deployments, "shop", &dep);
    assert!(r.iter().any(|x| x.relation == "Pod" && x.name == "web-run"));
    assert!(r.iter().any(|x| x.relation == "Service in front" && x.name == "front"));
    assert!(k.calls().len() <= 3);

    let k = FakeKube::new();
    k.answer("--field-selector spec.nodeName=node-a", &list(all_phase_pods()[..1].to_vec()));
    let r = related(&k, &target(), Kind::Nodes, "", &json!({"metadata":{"name":"node-a"}}));
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].namespace, "shop");
    // a selector that is not label characters is never passed on
    let bad = json!({"metadata":{"name":"web","namespace":"shop"},"spec":{"selector":{"matchLabels":{"app":"$(x)"}}}});
    assert_eq!(selector_string(&bad), None);
}

#[test]
fn exec_command_is_text_only() {
    let t = Target { kubeconfig: "vault:prod-kube".into(), context: "dev".into(), ..Default::default() };
    let c = exec_command(&t, "shop", "web-1", "app");
    assert_eq!(c, "kubectl --context dev -n shop exec -it web-1 -c app -- sh");
    assert!(!c.contains("vault"), "the kubeconfig is never printed");
    assert_eq!(exec_command(&Target::default(), "shop", "web-1", ""), "kubectl -n shop exec -it web-1 -- sh");
}

// ------------------------------------------------------------------------------------------------ logs

fn ll(ts: &str, text: &str) -> LogLine {
    LogLine { ts: ts.into(), text: text.into() }
}

#[test]
fn log_arguments() {
    let r = LogReq { container: "app".into(), previous: true, tail: 500, since_time: None };
    assert_eq!(logs_args(&target(), "shop", "web-1", &r).unwrap(), ["--kubeconfig", "/k/config", "--context", "dev", "--request-timeout=8s", "-n", "shop", "logs", "web-1", "--timestamps", "-c", "app", "--previous", "--tail=500"].map(String::from).to_vec());
    let r = LogReq { container: String::new(), previous: false, tail: 99_999, since_time: Some("2026-10-22T10:00:00.123456789Z".into()) };
    let a = logs_args(&target(), "shop", "web-1", &r).unwrap();
    assert!(a.contains(&"--since-time=2026-10-22T10:00:00.123456789Z".to_string()) && !a.iter().any(|x| x.starts_with("--tail")), "{a:?}");
    let r = LogReq { container: String::new(), previous: false, tail: 99_999, since_time: Some("$(evil)".into()) };
    assert!(logs_args(&target(), "shop", "web-1", &r).unwrap().contains(&"--tail=2000".to_string()), "a strange timestamp is ignored and the tail is capped");
    for bad in ["-f", "a b", ""] {
        assert!(logs_args(&target(), "shop", bad, &LogReq { container: String::new(), previous: false, tail: 100, since_time: None }).is_err());
    }
    assert!(logs_args(&target(), "shop", "p", &LogReq { container: "-x".into(), previous: false, tail: 1, since_time: None }).is_err());
}

#[test]
fn log_lines_split_off_their_timestamps() {
    let k = FakeKube::new();
    k.answer("logs web-1", "2026-10-22T10:00:00.1Z first line\n2026-10-22T10:00:01.2Z second line\nno timestamp here\n");
    let l = fetch_logs(&k, &target(), "shop", "web-1", &LogReq { container: String::new(), previous: false, tail: 100, since_time: None }).unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!((l[0].ts.as_str(), l[0].text.as_str()), ("2026-10-22T10:00:00.1Z", "first line"));
    assert_eq!((l[2].ts.as_str(), l[2].text.as_str()), ("", "no timestamp here"));
    let k = FakeKube::new();
    k.fail("logs", "Error from server (BadRequest): container \"x\" in pod \"web-1\" is waiting to start: ContainerCreating");
    let e = fetch_logs(&k, &target(), "shop", "web-1", &LogReq { container: String::new(), previous: false, tail: 100, since_time: None }).unwrap_err();
    assert!(e.contains("waiting to start"));
}

#[test]
fn the_log_buffer_is_bounded_and_follow_adds_only_new_lines() {
    let mut b = LogBuf::new(5);
    b.set((0..8).map(|i| ll(&format!("2026-10-22T10:00:0{i}Z"), &format!("l{i}"))).collect());
    assert_eq!(b.lines.len(), 5, "bounded");
    assert_eq!(b.lines[0].text, "l3");
    assert_eq!(b.dropped, 3);
    // a poll that overlaps what is held, with two new lines
    let added = b.merge((5..10).map(|i| ll(&format!("2026-10-22T10:00:0{i}Z"), &format!("l{i}"))).collect());
    assert_eq!(added, 2);
    assert_eq!(b.lines.back().unwrap().text, "l9");
    assert_eq!(b.lines.len(), 5, "still bounded");
    // the same poll again adds nothing
    assert_eq!(b.merge((5..10).map(|i| ll(&format!("2026-10-22T10:00:0{i}Z"), &format!("l{i}"))).collect()), 0);
    // lines that share the newest timestamp are not lost or doubled
    let mut b = LogBuf::new(100);
    b.set(vec![ll("2026-10-22T10:00:00Z", "a"), ll("2026-10-22T10:00:01Z", "b1")]);
    assert_eq!(b.merge(vec![ll("2026-10-22T10:00:01Z", "b1"), ll("2026-10-22T10:00:01Z", "b2"), ll("2026-10-22T10:00:02Z", "c")]), 2);
    assert_eq!(b.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["a", "b1", "b2", "c"]);
    assert_eq!(b.last_ts().unwrap(), "2026-10-22T10:00:02Z");
    // a huge flood stays at the cap
    let mut big = LogBuf::new(LOG_CAP);
    big.set((0..LOG_CAP + 3000).map(|i| ll("", &format!("line {i}"))).collect());
    assert_eq!(big.lines.len(), LOG_CAP);
    assert_eq!(big.dropped, 3000);
    // an empty buffer takes everything
    let mut e = LogBuf::new(10);
    assert_eq!(e.merge(vec![ll("", "x")]), 1);
}

#[test]
fn long_log_lines_are_cut() {
    let k = FakeKube::new();
    k.answer("logs", &format!("2026-10-22T10:00:00Z {}\n", "x".repeat(10_000)));
    let l = fetch_logs(&k, &target(), "shop", "web-1", &LogReq { container: String::new(), previous: false, tail: 100, since_time: None }).unwrap();
    assert_eq!(l[0].text.len(), LOG_LINE_MAX);
}

// ------------------------------------------------------------------------------------------------ actions

fn argv(a: &Action) -> Vec<Vec<String>> {
    a.steps(&target()).unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn golden_argument_arrays_for_every_action() {
    let pre = ["--kubeconfig", "/k/config", "--context", "dev", "--request-timeout=25s"];
    let with = |rest: &[&str]| -> Vec<String> { pre.iter().chain(rest.iter()).map(|x| x.to_string()).collect() };
    assert_eq!(argv(&Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "web".into() }), vec![with(&["-n", "shop", "rollout", "restart", "deployment/web"]), with(&["-n", "shop", "rollout", "status", "deployment/web", "--timeout=20s"])]);
    assert_eq!(argv(&Action::Restart { kind: Kind::StatefulSets, ns: "shop".into(), name: "db".into() })[0], with(&["-n", "shop", "rollout", "restart", "statefulset/db"]));
    assert_eq!(argv(&Action::Restart { kind: Kind::DaemonSets, ns: "kube-system".into(), name: "agent".into() })[0], with(&["-n", "kube-system", "rollout", "restart", "daemonset/agent"]));
    assert_eq!(argv(&Action::Scale { kind: Kind::Deployments, ns: "shop".into(), name: "web".into(), replicas: 3 }), vec![with(&["-n", "shop", "scale", "deployment/web", "--replicas=3"])]);
    assert_eq!(argv(&Action::Scale { kind: Kind::StatefulSets, ns: "shop".into(), name: "db".into(), replicas: 0 }), vec![with(&["-n", "shop", "scale", "statefulset/db", "--replicas=0"])]);
    assert_eq!(argv(&Action::DeletePod { ns: "shop".into(), name: "web-1".into() }), vec![with(&["-n", "shop", "delete", "pod", "web-1", "--wait=false"])]);
    assert_eq!(argv(&Action::Cordon { node: "node-a".into() }), vec![with(&["cordon", "node-a"])]);
    assert_eq!(argv(&Action::Uncordon { node: "node-a".into() }), vec![with(&["uncordon", "node-a"])]);
    assert_eq!(argv(&Action::Delete { kind: Kind::Services, ns: "shop".into(), name: "front".into() }), vec![with(&["-n", "shop", "delete", "services", "front", "--wait=false"])]);
    assert_eq!(argv(&Action::Delete { kind: Kind::Pvcs, ns: "shop".into(), name: "data".into() }), vec![with(&["-n", "shop", "delete", "persistentvolumeclaims", "data", "--wait=false"])]);
    let _ = s;
}

#[test]
fn names_are_checked_as_dns_1123() {
    for ok in ["web", "web-1", "a.b.c", "0abc", "node-a.example.com"] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in ["", "-web", "web-", "Web", "web_1", "web 1", "a..b", ".a", "a.", "web;rm", "$(x)", "web\n", &"a".repeat(254), "caf\u{e9}"] {
        assert!(!valid_name(bad), "{bad:?}");
    }
    assert!(valid_label("kube-system") && !valid_label("a.b") && !valid_label(&"a".repeat(64)) && !valid_label("-a"));
    for (a, why) in [
        (Action::Restart { kind: Kind::Deployments, ns: "sh op".into(), name: "web".into() }, "namespace with a space"),
        (Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "-rf".into() }, "name that looks like a flag"),
        (Action::Restart { kind: Kind::Pods, ns: "shop".into(), name: "web".into() }, "pods are not restarted"),
        (Action::Scale { kind: Kind::Services, ns: "shop".into(), name: "web".into(), replicas: 1 }, "services do not scale"),
        (Action::Scale { kind: Kind::Deployments, ns: "shop".into(), name: "web".into(), replicas: MAX_REPLICAS + 1 }, "too many"),
        (Action::DeletePod { ns: "shop".into(), name: "a;b".into() }, "shell characters"),
        (Action::Cordon { node: "n 1".into() }, "node with a space"),
        (Action::Delete { kind: Kind::Nodes, ns: String::new(), name: "node-a".into() }, "nodes are cordoned, not deleted here"),
        (Action::Delete { kind: Kind::Namespaces, ns: String::new(), name: "shop".into() }, "namespaces are not deleted here"),
        (Action::Delete { kind: Kind::Pods, ns: "shop".into(), name: "p".into() }, "pods have their own action"),
        (Action::Delete { kind: Kind::Crds, ns: String::new(), name: "x.example.com".into() }, "no CRDs"),
    ] {
        assert!(a.validate().is_err(), "{why}");
        assert!(a.steps(&target()).is_err(), "{why}: no argument array for a refused action");
    }
}

#[test]
fn confirmations_and_audit_text() {
    let d = Action::Delete { kind: Kind::Secrets, ns: "shop".into(), name: "db".into() };
    assert_eq!(d.confirm(), Confirm::TypeName("db".into()));
    assert_eq!(Action::DeletePod { ns: "shop".into(), name: "web-1".into() }.confirm(), Confirm::Click);
    assert_eq!(Action::Cordon { node: "n".into() }.confirm(), Confirm::Click);
    assert!(Action::DeletePod { ns: "shop".into(), name: "web-1".into() }.warning(true).contains("creates a new one"));
    assert!(Action::DeletePod { ns: "shop".into(), name: "web-1".into() }.warning(false).contains("gone for good"));
    assert!(Action::Scale { kind: Kind::Deployments, ns: "a".into(), name: "b".into(), replicas: 0 }.warning(true).contains("stops every pod"));
    assert_eq!(d.audit_name(), "k8s.delete");
    assert_eq!(d.audit_detail("managed"), "cluster=managed ns=shop Secret/db");
    assert_eq!(Action::Scale { kind: Kind::Deployments, ns: "shop".into(), name: "web".into(), replicas: 4 }.audit_detail("managed"), "cluster=managed ns=shop Deployment/web replicas=4");
    assert_eq!(Action::Cordon { node: "node-a".into() }.audit_detail("vault:x"), "cluster=vault:x Node/node-a");
    assert_eq!(Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "web".into() }.title(), "Restart deployment web");
}

#[test]
fn running_an_action_streams_output_and_reports() {
    let k = FakeKube::new();
    k.answer("rollout restart", "deployment.apps/web restarted\n");
    k.answer("rollout status", "Waiting for deployment \"web\" rollout to finish\ndeployment \"web\" successfully rolled out\n");
    let mut lines = Vec::new();
    let r = run_action(&k, &target(), &Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "web".into() }, &mut |l| lines.push(l.to_string())).unwrap();
    assert_eq!(r, "Restart deployment web done.");
    assert_eq!(lines[0], "$ kubectl -n shop rollout restart deployment/web", "the panel never shows the kubeconfig or context");
    assert!(lines.iter().any(|l| l.contains("restarted")) && lines.iter().any(|l| l.contains("successfully rolled out")));
    assert_eq!(k.calls().len(), 2);
    // the status step running out of time is a note, not a failure
    let k = FakeKube::new();
    k.answer("rollout restart", "restarted");
    k.fail("rollout status", "error: timed out waiting for the condition");
    let mut lines = Vec::new();
    let r = run_action(&k, &target(), &Action::Restart { kind: Kind::Deployments, ns: "shop".into(), name: "web".into() }, &mut |l| lines.push(l.to_string())).unwrap();
    assert!(r.contains("still going"), "{r}");
    // a failing first step is an error
    let k = FakeKube::new();
    k.fail("delete pod", "Error from server (Forbidden): pods \"web-1\" is forbidden: User cannot delete resource");
    let e = run_action(&k, &target(), &Action::DeletePod { ns: "shop".into(), name: "web-1".into() }, &mut |_| {}).unwrap_err();
    assert!(e.contains("Forbidden"), "{e}");
    let k = FakeKube::new();
    k.missing();
    assert_eq!(run_action(&k, &target(), &Action::Cordon { node: "n".into() }, &mut |_| {}).unwrap_err(), "kubectl was not found");
    // a refused action never runs anything
    let k = FakeKube::new();
    assert!(run_action(&k, &target(), &Action::DeletePod { ns: "shop".into(), name: "x y".into() }, &mut |_| {}).is_err());
    assert!(k.calls().is_empty());
}

#[test]
fn actions_ask_kubectl_for_thirty_seconds() {
    struct Spy(Mutex<Vec<u64>>);
    impl Kube for Spy {
        fn run(&self, _: &[String]) -> crate::wsexec::Out {
            self.0.lock().unwrap().push(0);
            crate::wsexec::Out { code: Some(0), ..Default::default() }
        }
        fn run_with(&self, _: &[String], secs: u64) -> crate::wsexec::Out {
            self.0.lock().unwrap().push(secs);
            crate::wsexec::Out { code: Some(0), ..Default::default() }
        }
    }
    let spy = Spy(Mutex::new(vec![]));
    run_action(&spy, &target(), &Action::Uncordon { node: "n".into() }, &mut |_| {}).unwrap();
    assert_eq!(*spy.0.lock().unwrap(), vec![ACTION_TIMEOUT_SECS]);
}

// ------------------------------------------------------------------------------------------------ choices and the feed

#[test]
fn choices_are_kept_per_person_and_per_cluster() {
    let d = std::env::temp_dir().join(format!("iyakki-k8sprefs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(Prefs::load(&d, "alice"), Prefs::default());
    let mut p = Prefs { cluster: "managed".into(), ..Default::default() };
    p.edit("managed", |c| {
        c.namespace = "shop".into();
        c.allow_changes = true;
    });
    p.edit("vault:lab", |c| c.namespace = "*".into());
    p.save(&d, "alice").unwrap();
    let back = Prefs::load(&d, "alice");
    assert_eq!(back, p);
    assert!(back.of("managed").allow_changes && !back.of("vault:lab").allow_changes, "per cluster");
    assert_eq!(back.of("unknown"), ClusterPrefs::default());
    assert_eq!(Prefs::load(&d, "bob"), Prefs::default(), "per person");
    assert!(!std::fs::read_to_string(d.join("k8s-tile-alice.json")).unwrap().contains("kubeconfig"));
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(Scope::from_id("*"), Scope::All);
    assert_eq!(Scope::from_id("shop"), Scope::Ns("shop".into()));
    assert_eq!(Scope::All.id(), "*");
}

#[test]
fn the_feed_stays_quiet_until_visible_and_makes_one_list_call_per_refresh() {
    let k = Arc::new(FakeKube::new());
    k.answer("get pods -o json", &list(all_phase_pods()));
    k.answer("get namespaces,pods -A -o json", &list(vec![json!({"kind":"Namespace","metadata":{"name":"shop"},"status":{"phase":"Active"}})]));
    let feed = ExpFeed::start(k.clone(), Duration::from_millis(60_000), Duration::from_millis(60_000));
    feed.set_want(Want { target: target(), scope: Scope::Ns("shop".into()), kind: Kind::Pods });
    std::thread::sleep(Duration::from_millis(150));
    assert!(k.calls().is_empty(), "a tile that is not on screen asks nothing");
    feed.set_visible(true);
    let t0 = Instant::now();
    while feed.fetches() < 1 || feed.ns_fetches() < 1 {
        assert!(t0.elapsed() < Duration::from_secs(5), "no answer");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(feed.listing().unwrap().rows.len(), 6);
    assert_eq!(feed.ns().unwrap().items.len(), 1);
    assert_eq!(k.calls().len(), 2, "one call for the list, one for the namespaces");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(k.calls().len(), 2, "nothing more until the interval passes");
    // another kind drops the old table at once and fetches again
    feed.set_want(Want { target: target(), scope: Scope::Ns("shop".into()), kind: Kind::Services });
    assert!(feed.listing().is_none());
    assert!(feed.ns().is_some(), "same cluster: the namespaces stay");
    // another cluster drops both
    feed.set_want(Want { target: Target { context: "other".into(), ..target() }, scope: Scope::All, kind: Kind::Pods });
    assert!(feed.listing().is_none() && feed.ns().is_none());
    feed.set_visible(false);
    std::thread::sleep(Duration::from_millis(100));
    let n = k.calls().len();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(k.calls().len(), n, "hidden again: quiet");
}

#[test]
fn the_feed_backs_off_when_the_cluster_is_slow() {
    struct SlowKube(Mutex<u32>);
    impl Kube for SlowKube {
        fn run(&self, a: &[String]) -> crate::wsexec::Out {
            *self.0.lock().unwrap() += 1;
            std::thread::sleep(Duration::from_millis(120));
            let body = if a.join(" ").contains("namespaces,pods") { list(vec![]) } else { list(vec![]) };
            crate::wsexec::Out { code: Some(0), stdout: body, ..Default::default() }
        }
    }
    let k = Arc::new(SlowKube(Mutex::new(0)));
    // a 20 ms interval would mean ~50 calls a second; calls that take 120 ms push the next one out to three times that
    let feed = ExpFeed::start(k.clone(), Duration::from_millis(20), Duration::from_millis(20));
    feed.set_want(Want { target: target(), scope: Scope::All, kind: Kind::Pods });
    feed.set_visible(true);
    std::thread::sleep(Duration::from_millis(1500));
    let calls = *k.0.lock().unwrap();
    assert!(calls <= 14, "backed off: {calls} calls in 1.5 s");
    assert!(calls >= 2);
}

#[test]
fn yaml_never_gains_entries_the_object_did_not_have() {
    let pod = json!({"kind":"Pod","metadata":{"name":"p"}});
    let y = object_yaml(Kind::Pods, &pod, false);
    assert!(!y.contains("null") && !y.contains("annotations"), "{y}");
    let s = json!({"kind":"Secret","metadata":{"name":"s"},"data":{"a":"Yg=="}});
    let y = object_yaml(Kind::Secrets, &s, false);
    assert!(!y.contains("stringData") && !y.contains("null"), "{y}");
    let mut s2 = s.clone();
    scrub_secret(&mut s2);
    assert!(s2.get("stringData").is_none() && s2["metadata"].get("annotations").is_none());
}
