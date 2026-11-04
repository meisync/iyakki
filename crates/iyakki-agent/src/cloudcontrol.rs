//! The control plane of cloud mode (docs/development/CLOUD-MULTITENANT-PLAN.md, step 7): a directory of tenant configuration files
//! (`TenantConfig` JSON, one per tenant) is the truth, and [`plan`] says what it takes to make the cluster match it. Tenants without a
//! namespace are created, tenants whose manifests changed are applied again, and namespaces labelled `iyakki.dev/tenant` that have no file
//! are removed, only when asked.
//!
//! * Planning is pure ([`plan`]); reading the cluster ([`list_namespaces`]) and acting ([`execute`]) take a `kubectl` closure like
//!   `cloudlaunch::execute`, so tests use a fake and this module starts nothing itself ([`run_kubectl`] is the real one).
//! * "Changed" is decided by a hash of the generated manifests kept in the namespace annotation `iyakki.dev/config-hash`, written after a
//!   successful apply. A namespace without the annotation (made by `deploy` or `bundle`) is applied once and then tracked.
//! * REMOVAL is dangerous and is never done unless the caller passes `prune` and every removal is listed. Only namespaces that carry the
//!   tenant label, and are named `iyakki-<label>`, are ever considered. Removing a namespace deletes its volumes whose reclaim policy is
//!   Delete, with the people's files on them: the plan says so. If any file could not be read (its tenant is unknown), or the directory
//!   holds no configuration at all, nothing is removed.

use std::path::{Path, PathBuf};

use crate::cloudtenant::{manifests, TenantConfig};

/// The label every tenant namespace carries (`cloudtenant::manifests`).
pub const TENANT_LABEL: &str = "iyakki.dev/tenant";
/// The annotation holding the hash of the manifests last applied by [`execute`].
pub const HASH_ANNOTATION: &str = "iyakki.dev/config-hash";

/// A namespace that carries the tenant label, as the cluster has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Existing {
    pub namespace: String,
    pub tenant: String,
    pub hash: Option<String>,
}

/// One tenant to create or apply again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub tenant: String,
    pub namespace: String,
    /// The configuration file it came from.
    pub file: String,
    pub yaml: String,
    pub hash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub create: Vec<Item>,
    pub update: Vec<Item>,
    pub unchanged: Vec<String>,
    /// Namespaces that will be deleted (only with `prune`).
    pub remove: Vec<String>,
    /// Labelled namespaces without a file that are left alone, and why.
    pub kept: Vec<(String, String)>,
    /// (file name, why) for every file that could not be used; the others are still planned.
    pub invalid: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.create.is_empty() && self.update.is_empty() && self.remove.is_empty()
    }

    /// The text shown before anything is done.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for (f, e) in &self.invalid {
            s.push_str(&format!("INVALID {f}: {e}\n"));
        }
        for i in &self.create {
            s.push_str(&format!("create  {} (namespace {}, from {})\n", i.tenant, i.namespace, i.file));
        }
        for i in &self.update {
            s.push_str(&format!("apply   {} (namespace {}, from {}): the manifests changed\n", i.tenant, i.namespace, i.file));
        }
        for n in &self.remove {
            s.push_str(&format!("REMOVE  namespace {n}: it has no configuration file\n"));
        }
        for (n, why) in &self.kept {
            s.push_str(&format!("keep    namespace {n}: {why}\n"));
        }
        for t in &self.unchanged {
            s.push_str(&format!("same    {t}\n"));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        if self.is_empty() {
            s.push_str("Nothing to do.\n");
        }
        s
    }
}

/// FNV-1a 64 as 16 hex digits: stable across versions, enough to see that a text changed.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Every `*.json` file of `dir` (sorted by name) with its parsed configuration or the reason it is not one.
pub fn read_dir(dir: &Path) -> Result<Vec<(String, Result<TenantConfig, String>)>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    Ok(files
        .into_iter()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let cfg = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| TenantConfig::parse(&t));
            (name, cfg)
        })
        .collect())
}

/// The plan. `files` is [`read_dir`]'s answer, `existing` is [`list_namespaces`]'s. Removals are planned only when `prune` is true.
pub fn plan(files: &[(String, Result<TenantConfig, String>)], existing: &[Existing], prune: bool) -> Plan {
    let mut p = Plan::default();
    let mut wanted: Vec<String> = Vec::new();
    let mut valid = 0usize;
    for (file, cfg) in files {
        let c = match cfg {
            Ok(c) => c,
            Err(e) => {
                p.invalid.push((file.clone(), e.clone()));
                continue;
            }
        };
        let ns = c.namespace();
        if wanted.contains(&ns) {
            p.invalid.push((file.clone(), format!("tenant \"{}\" is already described by another file", c.tenant)));
            continue;
        }
        let yaml = match manifests(c) {
            Ok(y) => y,
            Err(e) => {
                p.invalid.push((file.clone(), e));
                continue;
            }
        };
        valid += 1;
        wanted.push(ns.clone());
        let item = Item { tenant: c.tenant.clone(), namespace: ns.clone(), file: file.clone(), hash: hash(&yaml), yaml };
        match existing.iter().find(|e| e.namespace == ns) {
            None => p.create.push(item),
            Some(e) if e.hash.as_deref() == Some(item.hash.as_str()) => p.unchanged.push(item.tenant),
            Some(_) => p.update.push(item),
        }
    }
    let orphans: Vec<&Existing> = existing.iter().filter(|e| !wanted.contains(&e.namespace)).collect();
    let blocked = if !prune {
        Some("not removed: --prune was not given".to_string())
    } else if !p.invalid.is_empty() {
        Some("not removed: a configuration file could not be used, so it is not known which tenants should stay".to_string())
    } else if valid == 0 {
        Some("not removed: the directory holds no tenant configuration (is it the right one?)".to_string())
    } else {
        None
    };
    for e in orphans {
        if e.namespace != format!("iyakki-{}", e.tenant) {
            p.kept.push((e.namespace.clone(), format!("its name does not match its {TENANT_LABEL} label \"{}\"; never touched automatically", e.tenant)));
        } else if let Some(why) = &blocked {
            p.kept.push((e.namespace.clone(), why.clone()));
        } else {
            p.remove.push(e.namespace.clone());
        }
    }
    if !p.remove.is_empty() {
        p.warnings.push(format!(
            "removing {} deletes the namespace and everything in it: the people's files too, because volumes whose reclaim policy is Delete (the default of most storage classes) go with their claims. Back up first.",
            p.remove.join(", ")
        ));
    }
    p
}

/// The labelled namespaces of the cluster `kubectl` is bound to.
pub fn list_namespaces(kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>) -> Result<Vec<Existing>, String> {
    let out = kubectl(&["get".into(), "namespaces".into(), "-l".into(), TENANT_LABEL.into(), "-o".into(), "json".into()])?;
    let v: serde_json::Value = serde_json::from_str(&out).map_err(|e| format!("kubectl get namespaces: not JSON ({e})"))?;
    let items = v["items"].as_array().ok_or("kubectl get namespaces: no items")?;
    let mut list = Vec::new();
    for i in items {
        let (Some(name), Some(tenant)) = (i["metadata"]["name"].as_str(), i["metadata"]["labels"][TENANT_LABEL].as_str()) else { continue };
        list.push(Existing { namespace: name.into(), tenant: tenant.into(), hash: i["metadata"]["annotations"][HASH_ANNOTATION].as_str().map(String::from) });
    }
    Ok(list)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// What was done, in order.
    pub said: Vec<String>,
    /// The step that failed; nothing after it was tried.
    pub failed: Option<String>,
}

/// Do the plan: creates, then updates, then removals, in the order of the plan. Each document is written to `work` and applied, then the
/// namespace gets its hash annotation. The first failure stops everything. `plan.remove` is empty unless `prune` was passed to [`plan`].
pub fn execute(p: &Plan, work: &Path, kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>) -> Outcome {
    let mut o = Outcome::default();
    if let Err(e) = std::fs::create_dir_all(work) {
        o.failed = Some(format!("{}: {e}", work.display()));
        return o;
    }
    for (verb, i) in p.create.iter().map(|i| ("created", i)).chain(p.update.iter().map(|i| ("applied", i))) {
        let f = work.join(format!("{}.yaml", i.namespace));
        if let Err(e) = std::fs::write(&f, &i.yaml) {
            o.failed = Some(format!("{}: {e}", f.display()));
            return o;
        }
        let step = [vec!["apply".to_string(), "-f".into(), f.display().to_string()], vec!["annotate".into(), "namespace".into(), i.namespace.clone(), format!("{HASH_ANNOTATION}={}", i.hash), "--overwrite".into()]];
        for args in step {
            if let Err(e) = kubectl(&args) {
                o.failed = Some(format!("{} ({}): {e}", i.tenant, args[0]));
                return o;
            }
        }
        o.said.push(format!("{verb} {} (namespace {})", i.tenant, i.namespace));
    }
    for n in &p.remove {
        match kubectl(&["delete".into(), "namespace".into(), n.clone()]) {
            Ok(_) => o.said.push(format!("removed namespace {n}")),
            Err(e) => {
                o.failed = Some(format!("remove {n}: {e}"));
                return o;
            }
        }
    }
    o
}

/// The real `kubectl`, bound to the given kubeconfig and context (nothing is downloaded; it is the person's own).
pub fn run_kubectl(kubeconfig: Option<&str>, context: Option<&str>) -> impl FnMut(&[String]) -> Result<String, String> {
    let mut base: Vec<String> = Vec::new();
    if let Some(k) = kubeconfig {
        base.push(format!("--kubeconfig={k}"));
    }
    if let Some(c) = context {
        base.push(format!("--context={c}"));
    }
    move |args: &[String]| {
        let o = std::process::Command::new("kubectl")
            .args(&base)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("kubectl could not be started ({e}): install kubectl"))?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).into_owned())
        } else {
            Err(format!("kubectl {} failed: {}", args.first().map(String::as_str).unwrap_or(""), String::from_utf8_lossy(&o.stderr).trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(t: &str) -> (String, Result<TenantConfig, String>) {
        (format!("{t}.json"), Ok(TenantConfig::example(t)))
    }
    fn ex(t: &str, h: Option<&str>) -> Existing {
        Existing { namespace: format!("iyakki-{t}"), tenant: t.into(), hash: h.map(String::from) }
    }
    fn hash_of(t: &str) -> String {
        hash(&manifests(&TenantConfig::example(t)).unwrap())
    }
    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudcontrol-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_new_tenant_is_created_a_changed_one_applied_and_an_unchanged_one_left() {
        let mut changed = TenantConfig::example("beta");
        changed.storage = "5Gi".into();
        let files = vec![cfg("acme"), ("beta.json".into(), Ok(changed)), cfg("gamma")];
        let existing = vec![ex("acme", Some(&hash_of("acme"))), ex("beta", Some(&hash_of("beta")))];
        let p = plan(&files, &existing, false);
        assert_eq!(p.create.iter().map(|i| i.tenant.as_str()).collect::<Vec<_>>(), ["gamma"]);
        assert_eq!(p.update.iter().map(|i| i.tenant.as_str()).collect::<Vec<_>>(), ["beta"]);
        assert_eq!(p.unchanged, ["acme"]);
        assert!(p.remove.is_empty() && p.invalid.is_empty());
        let text = p.render();
        assert!(text.contains("create  gamma") && text.contains("apply   beta") && text.contains("same    acme"), "{text}");
        // a namespace without the annotation is applied once
        assert_eq!(plan(&[cfg("acme")], &[ex("acme", None)], false).update.len(), 1);
        // nothing at all to do
        let p = plan(&[cfg("acme")], &[ex("acme", Some(&hash_of("acme")))], true);
        assert!(p.is_empty() && p.render().contains("Nothing to do"));
    }

    #[test]
    fn prune_off_never_removes_and_prune_on_lists_each_removal_with_the_warning() {
        let files = vec![cfg("acme")];
        let existing = vec![ex("acme", Some(&hash_of("acme"))), ex("old", None), ex("older", None)];
        let off = plan(&files, &existing, false);
        assert!(off.remove.is_empty());
        assert_eq!(off.kept.len(), 2);
        assert!(off.kept[0].1.contains("--prune"));
        assert!(off.warnings.is_empty());
        let on = plan(&files, &existing, true);
        assert_eq!(on.remove, ["iyakki-old", "iyakki-older"]);
        assert!(on.warnings[0].contains("iyakki-old") && on.warnings[0].contains("reclaim policy is Delete"), "{:?}", on.warnings);
        let text = on.render();
        assert!(text.contains("REMOVE  namespace iyakki-old") && text.contains("REMOVE  namespace iyakki-older"));
    }

    #[test]
    fn a_namespace_whose_name_does_not_match_its_label_is_never_removed() {
        let odd = Existing { namespace: "kube-system".into(), tenant: "x".into(), hash: None };
        let p = plan(&[cfg("acme")], &[odd], true);
        assert!(p.remove.is_empty());
        assert!(p.kept[0].1.contains("never touched"));
    }

    #[test]
    fn unlabelled_namespaces_are_never_listed_and_never_touched() {
        let json = r#"{"items":[
          {"metadata":{"name":"iyakki-acme","labels":{"iyakki.dev/tenant":"acme"},"annotations":{"iyakki.dev/config-hash":"abc"}}},
          {"metadata":{"name":"default","labels":{"kubernetes.io/metadata.name":"default"}}},
          {"metadata":{"name":"iyakki-nolabel"}}]}"#;
        let mut calls: Vec<Vec<String>> = Vec::new();
        let mut k = |a: &[String]| {
            calls.push(a.to_vec());
            Ok(json.to_string())
        };
        let list = list_namespaces(&mut k).unwrap();
        assert_eq!(list, vec![Existing { namespace: "iyakki-acme".into(), tenant: "acme".into(), hash: Some("abc".into()) }]);
        assert!(calls[0].join(" ").contains("-l iyakki.dev/tenant"));
        // with the label selector honoured by the cluster, even a full prune sees only labelled ones
        let p = plan(&[cfg("beta")], &list, true);
        assert_eq!(p.remove, ["iyakki-acme"]);
        assert!(!p.remove.iter().any(|n| n == "default" || n == "iyakki-nolabel"));
        assert!(list_namespaces(&mut |_: &[String]| Ok("nonsense".to_string())).is_err());
    }

    #[test]
    fn an_invalid_file_is_reported_by_name_the_others_are_planned_and_nothing_is_pruned() {
        let files = vec![cfg("acme"), ("broken.json".into(), Err("tenant configuration: expected value".into())), ("dup.json".into(), Ok(TenantConfig::example("acme")))];
        let p = plan(&files, &[ex("orphan", None)], true);
        assert_eq!(p.create.len(), 1);
        assert_eq!(p.invalid.len(), 2);
        assert_eq!(p.invalid[0].0, "broken.json");
        assert!(p.invalid[1].0 == "dup.json" && p.invalid[1].1.contains("already described"));
        assert!(p.remove.is_empty(), "with an unreadable file it is not known which tenants should stay");
        assert!(p.kept[0].1.contains("could not be used"));
        assert!(p.render().contains("INVALID broken.json"));
        // an empty directory removes nothing either
        let p = plan(&[], &[ex("orphan", None)], true);
        assert!(p.remove.is_empty() && p.kept[0].1.contains("no tenant configuration"));
    }

    #[test]
    fn read_dir_takes_json_files_in_order_and_names_the_bad_ones() {
        let d = dir("read");
        std::fs::write(d.join("b.json"), serde_json::to_string(&TenantConfig::example("beta")).unwrap()).unwrap();
        std::fs::write(d.join("a.json"), "{ nope").unwrap();
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        let v = read_dir(&d).unwrap();
        assert_eq!(v.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["a.json", "b.json"]);
        assert!(v[0].1.is_err() && v[1].1.is_ok());
        assert!(read_dir(&d.join("missing")).is_err());
    }

    #[test]
    fn execute_creates_then_updates_then_removes_and_the_first_failure_stops() {
        let mut changed = TenantConfig::example("beta");
        changed.storage = "5Gi".into();
        let files = vec![cfg("acme"), ("beta.json".into(), Ok(changed))];
        let p = plan(&files, &[ex("beta", Some("old")), ex("gone", None)], true);
        let work = dir("exec");
        let mut calls: Vec<String> = Vec::new();
        let o = execute(&p, &work, &mut |a: &[String]| {
            calls.push(a.iter().map(|s| if s.ends_with(".yaml") { "FILE".to_string() } else { s.clone() }).collect::<Vec<_>>().join(" "));
            Ok(String::new())
        });
        assert_eq!(o.failed, None);
        assert_eq!(
            calls,
            [
                "apply -f FILE".to_string(),
                format!("annotate namespace iyakki-acme {HASH_ANNOTATION}={} --overwrite", p.create[0].hash),
                "apply -f FILE".to_string(),
                format!("annotate namespace iyakki-beta {HASH_ANNOTATION}={} --overwrite", p.update[0].hash),
                "delete namespace iyakki-gone".to_string(),
            ]
        );
        assert_eq!(o.said, ["created acme (namespace iyakki-acme)", "applied beta (namespace iyakki-beta)", "removed namespace iyakki-gone"]);
        assert!(work.join("iyakki-acme.yaml").is_file());
        // the create fails: the update and the removal are not tried
        let mut n = 0;
        let o = execute(&p, &work, &mut |_: &[String]| {
            n += 1;
            Err("boom".to_string())
        });
        assert_eq!(n, 1);
        assert!(o.failed.unwrap().contains("acme") && o.said.is_empty());
        // the update fails: the removal is not tried, the create stays done
        let mut seen: Vec<String> = Vec::new();
        let o = execute(&p, &work, &mut |a: &[String]| {
            seen.push(a[0].clone());
            if a[0] == "apply" && seen.iter().filter(|s| *s == "apply").count() == 2 { Err("denied".to_string()) } else { Ok(String::new()) }
        });
        assert_eq!(o.said.len(), 1);
        assert!(o.failed.unwrap().contains("beta"));
        assert!(!seen.contains(&"delete".to_string()));
    }

    #[test]
    fn a_plan_without_prune_never_calls_delete() {
        let p = plan(&[cfg("acme")], &[ex("old", None)], false);
        let work = dir("noprune");
        let mut calls: Vec<String> = Vec::new();
        let o = execute(&p, &work, &mut |a: &[String]| {
            calls.push(a[0].clone());
            Ok(String::new())
        });
        assert!(o.failed.is_none());
        assert!(!calls.contains(&"delete".to_string()), "{calls:?}");
    }
}
