use super::*;

fn dep(ns: &str, name: &str, ready: u64, host: Option<u16>, label: Option<&str>) -> String {
    let ports = host.map(|p| format!(r#","ports":[{{"containerPort":3000,"hostPort":{p},"hostIP":"127.0.0.1"}}]"#)).unwrap_or_default();
    let labels = label.map(|l| format!(r#","labels":{{"app.kubernetes.io/name":"{l}"}}"#)).unwrap_or_default();
    format!(r#"{{"metadata":{{"namespace":"{ns}","name":"{name}"{labels}}},"spec":{{"replicas":1,"template":{{"spec":{{"containers":[{{"name":"c"{ports}}}]}}}}}},"status":{{"readyReplicas":{ready}}}}}"#)
}

fn list(items: &[String]) -> String {
    format!(r#"{{"items":[{}]}}"#, items.join(","))
}

fn ok(s: String) -> KOut {
    KOut { ok: true, stdout: s.clone(), text: s }
}

#[test]
fn a_ready_gitea_deployment_is_found_with_its_namespace() {
    let j = list(&[dep("kube-system", "coredns", 1, None, None), dep("iyakki-gitea", "gitea", 1, None, None)]);
    let f = parse(&j);
    assert_eq!(f, vec![Found { namespace: "iyakki-gitea".into(), name: "gitea".into(), ready: true, loopback: None }]);
    let m = merge(vec![], &f);
    assert_eq!(m.len(), 1);
    assert!(m[0].found && m[0].title.contains("found on your cluster") && m[0].namespace == "iyakki-gitea");
    assert!(m[0].problem.contains("no address on this computer") && m[0].problem.contains("no token"), "{}", m[0].problem);
}

#[test]
fn a_label_is_enough_and_a_loopback_port_is_read() {
    let f = parse(&list(&[dep("code", "forge", 1, Some(3300), Some("gitea"))]));
    assert_eq!(f[0].loopback, Some(3300));
    let m = merge(vec![], &f);
    assert_eq!(m[0].url, "http://127.0.0.1:3300");
    assert!(!m[0].problem.contains("no address") && m[0].problem.contains("no token"));
}

#[test]
fn a_gitea_that_is_not_ready_is_not_offered() {
    let f = parse(&list(&[dep("iyakki-gitea", "gitea", 0, None, None)]));
    assert_eq!(f.len(), 1);
    assert!(!f[0].ready);
    assert!(merge(vec![], &f).is_empty());
}

#[test]
fn nothing_found_and_two_instances() {
    assert!(parse(&list(&[dep("a", "web", 1, None, None)])).is_empty());
    assert!(parse("not json").is_empty());
    let f = parse(&list(&[dep("b", "gitea", 1, None, None), dep("a", "gitea", 1, None, None), dep("a", "gitea-runner", 1, None, None)]));
    assert_eq!(f.iter().map(|x| x.namespace.as_str()).collect::<Vec<_>>(), ["a", "b"], "one per namespace, runners are not a Gitea");
    assert_eq!(merge(vec![], &f).len(), 2);
}

#[test]
fn a_record_for_the_same_namespace_wins() {
    let rec = ClusterGitea { instance: "gitea-iyakki-gitea".into(), title: "Local Gitea".into(), url: "http://127.0.0.1:3300".into(), token_vault: "svc.x.token".into(), problem: String::new(), namespace: "iyakki-gitea".into(), found: false };
    let f = parse(&list(&[dep("iyakki-gitea", "gitea", 1, None, None), dep("other", "gitea", 1, None, None)]));
    let m = merge(vec![rec], &f);
    assert_eq!(m.len(), 2);
    assert_eq!((m[0].instance.as_str(), m[0].found), ("gitea-iyakki-gitea", false));
    assert_eq!(m[1].namespace, "other");
}

#[test]
fn a_kubectl_error_is_an_empty_list() {
    let fail = |_: &[&str]| KOut { ok: false, stdout: String::new(), text: "The connection to the server was refused".into() };
    assert!(discover(&fail).is_empty());
    let seen = std::sync::Mutex::new(Vec::new());
    let good = |a: &[&str]| {
        seen.lock().unwrap().push(a.join(" "));
        ok(list(&[dep("iyakki-gitea", "gitea", 1, None, None)]))
    };
    assert_eq!(discover(&good).len(), 1);
    assert_eq!(seen.lock().unwrap()[0], "get deployments -A -o json", "read-only");
}
