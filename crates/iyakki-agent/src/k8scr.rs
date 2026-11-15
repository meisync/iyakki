//! Custom resource instances for the K8S tile (F-K8S-26, data layer): the explorer lists the CustomResourceDefinitions (`Kind::Crds`);
//! this module reads what is INSIDE one: `kubectl get <plural>.<group> -o json` for a definition the cluster itself named, as table
//! rows (name, namespace, age, a status word), and one instance as YAML. Names are checked (DNS-1123) before they reach an argument
//! array; nothing here changes anything. The tile does not show these yet (that is the user-interface half of the row).

use crate::k8s::{age_of, age_text, at, classify_failure, json, text, Cluster, Kube, Target};
use crate::k8sexp::{to_yaml, valid_label, valid_name, Scope, MAX_KEPT};
use serde_json::Value;

/// One CustomResourceDefinition, as far as listing its instances needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrdInfo {
    pub name: String,
    pub group: String,
    pub plural: String,
    pub kind: String,
    pub namespaced: bool,
    /// The versions the cluster serves, storage version first.
    pub versions: Vec<String>,
}

impl CrdInfo {
    /// From one object of `kubectl get crd -o json` (an item). `None` when it lacks what listing needs or a name is not valid.
    pub fn from_object(v: &Value) -> Option<CrdInfo> {
        let (group, plural, kind) = (text(v, &["spec", "group"]), text(v, &["spec", "names", "plural"]), text(v, &["spec", "names", "kind"]));
        if !valid_name(&group) || !valid_label(&plural) || kind.is_empty() {
            return None;
        }
        let mut versions: Vec<(bool, String)> = at(v, &["spec", "versions"]).as_array()?.iter().filter(|x| x["served"].as_bool().unwrap_or(false)).filter_map(|x| Some((x["storage"].as_bool().unwrap_or(false), x["name"].as_str()?.to_string()))).collect();
        versions.sort_by_key(|(storage, _)| !*storage);
        Some(CrdInfo { name: text(v, &["metadata", "name"]), group, plural, kind, namespaced: text(v, &["spec", "scope"]) == "Namespaced", versions: versions.into_iter().map(|x| x.1).collect() })
    }

    /// `plural.group`, the resource argument of `kubectl get`.
    pub fn resource(&self) -> String {
        format!("{}.{}", self.plural, self.group)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CrRow {
    pub name: String,
    pub namespace: String,
    pub age_secs: u64,
    pub age: String,
    /// `status.phase`, else the `Ready` condition (`Ready`, `Not ready: reason`), else `state`, else empty.
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CrList {
    pub crd: String,
    pub rows: Vec<CrRow>,
    /// How many the cluster has (more than `rows` when cut at [`MAX_KEPT`]).
    pub total: usize,
}

fn status_word(it: &Value) -> String {
    let phase = text(it, &["status", "phase"]);
    if !phase.is_empty() {
        return phase;
    }
    if let Some(c) = at(it, &["status", "conditions"]).as_array().and_then(|a| a.iter().find(|c| c["type"] == "Ready")) {
        return if c["status"] == "True" { "Ready".into() } else { format!("Not ready: {}", c["reason"].as_str().unwrap_or("unknown")).chars().take(80).collect() };
    }
    text(it, &["status", "state"])
}

pub fn parse_instances(crd: &CrdInfo, v: &Value, now: u64) -> CrList {
    let items = at(v, &["items"]).as_array().cloned().unwrap_or_default();
    let rows = items
        .iter()
        .take(MAX_KEPT)
        .map(|it| {
            let age = age_of(it, now);
            CrRow { name: text(it, &["metadata", "name"]), namespace: text(it, &["metadata", "namespace"]), age_secs: age, age: age_text(age), status: status_word(it) }
        })
        .collect();
    CrList { crd: crd.name.clone(), rows, total: items.len() }
}

/// The instances of one definition: one `get` call. A cluster-wide scope is `-A` after the resource (kubectl 1.37 refuses it before the verb).
pub fn list_instances(kube: &dyn Kube, tg: &Target, crd: &CrdInfo, scope: &Scope, now: u64) -> Result<CrList, String> {
    let mut a = tg.base_args();
    if crd.namespaced {
        if let Scope::Ns(ns) = scope {
            if !valid_label(ns) {
                return Err(format!("\"{ns}\" is not a namespace name"));
            }
            a.extend(["-n".into(), ns.clone()]);
        }
    }
    a.extend(["get".into(), crd.resource()]);
    if crd.namespaced && *scope == Scope::All {
        a.push("-A".into());
    }
    a.extend(["-o".into(), "json".into()]);
    let o = kube.run(&a);
    if !o.ok() {
        return Err(match classify_failure(&o) {
            Cluster::Failed(why) => why,
            _ => "kubectl failed".into(),
        });
    }
    let v = json(&o).ok_or_else(|| if o.truncated { "there are too many instances to list at once: choose one namespace".to_string() } else { "kubectl printed something unreadable".to_string() })?;
    Ok(parse_instances(crd, &v, now))
}

/// One instance as YAML (`kubectl get <resource> <name> -o json`, shown as YAML).
pub fn instance_yaml(kube: &dyn Kube, tg: &Target, crd: &CrdInfo, ns: &str, name: &str) -> Result<String, String> {
    if !valid_name(name) || (crd.namespaced && !valid_label(ns)) {
        return Err("not a valid name".into());
    }
    let mut a = tg.base_args();
    if crd.namespaced {
        a.extend(["-n".into(), ns.into()]);
    }
    a.extend(["get".into(), crd.resource(), name.into(), "-o".into(), "json".into()]);
    let o = kube.run(&a);
    if !o.ok() {
        return Err(o.combined().lines().find(|l| !l.trim().is_empty()).unwrap_or("kubectl failed").trim().chars().take(300).collect());
    }
    json(&o).map(|v| to_yaml(&v)).ok_or_else(|| "kubectl printed something unreadable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k8s::FakeKube;
    use serde_json::json;

    fn crd_obj() -> Value {
        json!({"metadata":{"name":"widgets.example.com"},"spec":{"group":"example.com","scope":"Namespaced","names":{"kind":"Widget","plural":"widgets"},"versions":[{"name":"v1beta1","served":true,"storage":false},{"name":"v1","served":true,"storage":true},{"name":"v0","served":false,"storage":false}]}})
    }

    fn tg() -> Target {
        Target { kubeconfig: "/k/config".into(), context: "dev".into(), ..Default::default() }
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn a_definition_gives_its_resource_argument_and_served_versions_storage_first() {
        let c = CrdInfo::from_object(&crd_obj()).unwrap();
        assert_eq!((c.resource().as_str(), c.namespaced, c.kind.as_str()), ("widgets.example.com", true, "Widget"));
        assert_eq!(c.versions, vec!["v1".to_string(), "v1beta1".to_string()]);
        let mut bad = crd_obj();
        bad["spec"]["names"]["plural"] = json!("wid gets; rm");
        assert!(CrdInfo::from_object(&bad).is_none());
        bad = crd_obj();
        bad["spec"]["group"] = json!("--all");
        assert!(CrdInfo::from_object(&bad).is_none());
    }

    #[test]
    fn instances_are_listed_with_age_and_a_status_word() {
        let items = json!({"items":[
            {"metadata":{"name":"a","namespace":"shop","creationTimestamp":"2027-01-15T07:59:00Z"},"status":{"phase":"Running"}},
            {"metadata":{"name":"b","namespace":"shop","creationTimestamp":"2027-01-15T07:59:00Z"},"status":{"conditions":[{"type":"Ready","status":"False","reason":"NoQuota"}]}},
            {"metadata":{"name":"c","namespace":"dev","creationTimestamp":"2027-01-15T07:59:00Z"},"status":{"conditions":[{"type":"Ready","status":"True"}]}},
            {"metadata":{"name":"d","namespace":"dev"}}]});
        let k = FakeKube::new();
        k.answer("get widgets.example.com", &items.to_string());
        let c = CrdInfo::from_object(&crd_obj()).unwrap();
        let l = list_instances(&k, &tg(), &c, &Scope::All, 1_800_000_000).unwrap();
        assert_eq!(l.total, 4);
        assert_eq!(l.rows.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(), ["Running", "Not ready: NoQuota", "Ready", ""]);
        assert_eq!(l.rows[0].namespace, "shop");
        let call = k.calls().remove(0);
        assert!(call.contains("get widgets.example.com -A -o json"), "{call}");
    }

    #[test]
    fn scope_and_names_are_checked_before_anything_is_run() {
        let k = FakeKube::new();
        let c = CrdInfo::from_object(&crd_obj()).unwrap();
        assert!(list_instances(&k, &tg(), &c, &Scope::Ns("Bad NS".into()), NOW).is_err());
        assert!(instance_yaml(&k, &tg(), &c, "shop", "a b").is_err());
        assert!(instance_yaml(&k, &tg(), &c, "-x", "a").is_err());
        assert!(k.calls().is_empty(), "nothing was run");
        // a cluster-scoped definition ignores the namespace
        let mut co = crd_obj();
        co["spec"]["scope"] = json!("Cluster");
        let cc = CrdInfo::from_object(&co).unwrap();
        k.answer("get widgets.example.com", "{\"items\":[]}");
        list_instances(&k, &tg(), &cc, &Scope::Ns("shop".into()), NOW).unwrap();
        let call = k.calls().remove(0);
        assert!(!call.contains(" -n ") && !call.contains(" -A"), "{call}");
    }

    #[test]
    fn one_instance_is_read_as_yaml_and_failures_are_one_plain_line() {
        let k = FakeKube::new();
        k.answer("get widgets.example.com w1", &json!({"metadata":{"name":"w1","namespace":"shop"},"spec":{"size":3}}).to_string());
        let c = CrdInfo::from_object(&crd_obj()).unwrap();
        let y = instance_yaml(&k, &tg(), &c, "shop", "w1").unwrap();
        assert!(y.contains("size: 3") && y.contains("name: w1"), "{y}");
        let k2 = FakeKube::new();
        k2.fail("get widgets", "Error from server (NotFound): widgets.example.com \"zz\" not found\nmore");
        let e = instance_yaml(&k2, &tg(), &c, "shop", "zz").unwrap_err();
        assert!(e.contains("not found") && !e.contains('\n'), "{e}");
        let e = list_instances(&k2, &tg(), &c, &Scope::All, NOW).unwrap_err();
        assert!(!e.is_empty());
    }
}
