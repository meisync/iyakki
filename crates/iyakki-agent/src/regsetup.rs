//! Container registries for k3s (docs/K3S-DEV.md): why a node cannot pull from one and the exact fix, and the registry option of the
//! cluster setup plan.
//!
//! * [`diagnose`] recognises the registry errors that end in `ErrImagePull` ("server gave HTTP response to HTTPS client", `x509`,
//!   "no basic auth credentials", "connection refused") and returns the cause and the fix. [`setup_help`] is the ready-to-copy
//!   `/etc/rancher/k3s/registries.yaml` with the commands for every node. Nothing here runs anything or edits `/etc`.
//! * [`plan`] / [`preview`] / [`apply`] are the setup: [`RegistryOption`] none, A a registry pod inside the cluster (namespace
//!   `iyakki-registry`, PVC, ClusterIP Service published as a loopback hostPort; pods pull `localhost:<port>`, so no node needs configuring and no root is used),
//!   B the system registry (a `registry:2` container on `localhost:5000`), or C an existing remote registry. The plan lists the
//!   exact objects, file and commands; [`apply`] needs the approval flag, writes `registries.yaml` only when running as root
//!   (otherwise the commands are returned for the person to run), runs kubectl / the registry container through the [`Runner`], and
//!   records the choice in `.iyakki/project.json` (`k8s.registry` = where this machine pushes, `k8s.registry_pull` = what the pods
//!   pull, `k8s.registry_kind`) so `deploy_load` uses it. The IDE's K8S panel calls it (Set up registry…) and so does the
//!   `registry_setup` tool; `cluster_setup_plan` is the read-only preview. [`status`] feeds the panel's Registry chip.

pub use iyakki_shared::regsetup::{Applied, DEFAULT_NODE_PORT, DEFAULT_SIZE_GI, Facts, KubeCtx, LOCAL_NAME, LOCAL_VOLUME, NODE_PORTS, PlanEnv, REGISTRY_IMAGE, RESTART, RegistryOption, SetupPlan, YAML_PATH, choose_port, diagnose, hostport_of, http_mirror_yaml, in_cluster_manifest, in_cluster_manifest_sized, insecure_tls_yaml, loopback, plan, preview, registry_in, root_line, setup_help, valid_hostport};
pub use iyakki_shared::regsetup::{kubectl_line, tee_commands, unsure};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::wsexec::{Out, Runner};
pub use iyakki_shared::regsetup::{RegStatus};
pub use iyakki_shared::regsetup::{CHOICES, LOCAL_REGISTRY};
pub use iyakki_shared::regsetup::{IN_NS, IN_NAME};










// ------------------------------------------------------------------------------------------------------------------ setup plan





















fn kubectl(run: &dyn Runner, k: &KubeCtx, args: &[&str], secs: u64) -> Out {
    let mut argv: Vec<String> = vec!["kubectl".into()];
    if !k.kubeconfig.is_empty() {
        argv.extend(["--kubeconfig".into(), k.kubeconfig.clone()]);
    }
    if !k.context.is_empty() {
        argv.extend(["--context".into(), k.context.clone()]);
    }
    argv.push("--request-timeout=20s".into());
    argv.extend(args.iter().map(|s| s.to_string()));
    let env: Vec<(String, String)> = if k.kubeconfig.is_empty() { vec![] } else { vec![("KUBECONFIG".into(), k.kubeconfig.clone())] };
    run.run(&std::env::temp_dir(), &argv, &env, Duration::from_secs(secs), 256 * 1024, None)
}

/// Read what the in-cluster plan depends on (read-only kubectl).
pub fn probe(run: &dyn Runner, k: &KubeCtx) -> Facts {
    let mut f = Facts::default();
    let sc = kubectl(run, k, &["get", "storageclass", "-o", "json"], 20);
    if sc.ok() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&sc.stdout) {
            let items = j["items"].as_array().cloned().unwrap_or_default();
            let name = |s: &serde_json::Value| s["metadata"]["name"].as_str().map(str::to_string);
            f.storage_class = items
                .iter()
                .find(|s| s["metadata"]["annotations"]["storageclass.kubernetes.io/is-default-class"].as_str() == Some("true"))
                .and_then(name)
                .or_else(|| items.iter().filter_map(name).find(|n| n == "local-path"));
        }
    }
    let sv = kubectl(run, k, &["get", "services", "-A", "-o", "json"], 20);
    if sv.ok() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&sv.stdout) {
            for s in j["items"].as_array().cloned().unwrap_or_default() {
                let ours = s["metadata"]["namespace"].as_str() == Some(IN_NS) && s["metadata"]["name"].as_str() == Some(IN_NAME);
                for port in s["spec"]["ports"].as_array().cloned().unwrap_or_default() {
                    if let Some(n) = port["nodePort"].as_u64().map(|n| n as u16) {
                        if ours {
                            f.existing_port = Some(n);
                            f.legacy_exposed = true;
                        } else {
                            f.taken_ports.push(n);
                        }
                    }
                }
            }
        }
    }
    if f.existing_port.is_none() {
        // the converted registry has no NodePort: its port is the hostPort of the Deployment
        let d = kubectl(run, k, &["get", "deployment", IN_NAME, "-n", IN_NS, "-o", "json"], 20);
        if d.ok() {
            f.existing_port = serde_json::from_str::<serde_json::Value>(&d.stdout).ok().and_then(|j| hostport_of(&j));
        }
    }
    f
}


/// An older install published the registry as a NodePort Service: open to the whole network, no password. Convert it in place, with
/// the cluster's own kubectl (`kube` runs `kubectl <args>`): first the Service becomes ClusterIP (the NodePort closes at once), then
/// the Deployment gets the loopback hostPort (`127.0.0.1:<port>`, the old NodePort number, so `localhost:<port>` keeps working for
/// pods, the node and this machine). Returns the sentence for the audit log when something was converted, `None` when there was
/// nothing to do (no registry, or already converted). Safe to call again and again.
pub fn close_legacy(kube: &dyn Fn(&[&str]) -> Out) -> Result<Option<String>, String> {
    let sv = kube(&["get", "service", IN_NAME, "-n", IN_NS, "-o", "json"]);
    if !sv.ok() {
        return Ok(None);
    }
    let j: serde_json::Value = serde_json::from_str(&sv.stdout).unwrap_or(serde_json::Value::Null);
    let port = j["spec"]["ports"].as_array().and_then(|p| p.iter().find_map(|p| p["nodePort"].as_u64())).map(|p| p as u16);
    let is_node_port = matches!(j["spec"]["type"].as_str(), Some("NodePort") | Some("LoadBalancer"));
    let Some(port) = port else {
        return Ok(None);
    };
    if !is_node_port && j["spec"]["type"].as_str().is_some() {
        return Ok(None);
    }
    let svc_patch = r#"{"spec":{"type":"ClusterIP","ports":[{"port":5000,"targetPort":5000,"protocol":"TCP"}]}}"#;
    let a = kube(&["patch", "service", IN_NAME, "-n", IN_NS, "--type=merge", "-p", svc_patch]);
    if !a.ok() {
        return Err(format!("The registry is still open on NodePort {port} (no password). Could not close it: {}. Run `kubectl -n {IN_NS} patch service {IN_NAME} --type=merge -p '{svc_patch}'` yourself.", a.combined().lines().next().unwrap_or("").trim()));
    }
    let dep_patch = format!(r#"{{"spec":{{"template":{{"spec":{{"containers":[{{"name":"{IN_NAME}","ports":[{{"containerPort":5000,"hostPort":{port},"hostIP":"127.0.0.1"}}]}}]}}}}}}}}"#);
    let b = kube(&["patch", "deployment", IN_NAME, "-n", IN_NS, "-p", &dep_patch]);
    if !b.ok() {
        return Err(format!("The registry's NodePort {port} is closed, but its Deployment could not be bound to 127.0.0.1:{port}: {}. Pushing and pulling localhost:{port} fails until it is; run the registry step again (`iyakki quick-install registry`).", b.combined().lines().next().unwrap_or("").trim()));
    }
    Ok(Some(format!("registry NodePort {port} closed: the in-cluster registry (no password) was reachable from the network; it is now ClusterIP plus 127.0.0.1:{port} on the node only")))
}

/// Do the plan. `approved` must be true (the person saw [`preview`]). Root is never taken: without it the mirror file is returned as
/// commands in `to_run`.
pub fn apply(p: &SetupPlan, run: &dyn Runner, approved: bool) -> Result<Applied, String> {
    if !approved {
        return Err("the registry setup was not approved".into());
    }
    let mut a = Applied::default();
    if let (Some(m), Some(k)) = (&p.manifest, &p.env.kube) {
        let lint = crate::deploylint::lint(m, IN_NS, &|_| false);
        if !lint.ok() {
            return Err(format!("the registry manifest did not pass the manifest check:\n{}", lint.report()));
        }
        let ns = kubectl(run, k, &["get", "namespace", IN_NS, "-o", "name"], 30);
        if !ns.ok() {
            let t = ns.combined();
            if !(t.contains("NotFound") || t.contains("not found")) {
                return Err(format!("could not check namespace {IN_NS}: {}", t.lines().next().unwrap_or("").trim()));
            }
            let c = kubectl(run, k, &["create", "namespace", IN_NS], 30);
            if !c.ok() {
                return Err(format!("could not create namespace {IN_NS}: {}", c.combined().lines().next().unwrap_or("").trim()));
            }
            a.done.push(format!("created namespace {IN_NS}"));
        }
        let dir = p.env.project_root.clone().unwrap_or_else(std::env::temp_dir).join(".iyakki/deploy");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join("iyakki-registry.yaml");
        std::fs::write(&file, &lint.text).map_err(|e| format!("{}: {e}", file.display()))?;
        let f = file.display().to_string();
        if p.env.facts.legacy_exposed {
            match close_legacy(&|args: &[&str]| kubectl(run, k, args, 60))? {
                Some(line) => a.done.push(format!("SECURITY: {line}")),
                None => {}
            }
        }
        let kube = |args: &[&str]| {
            let o = kubectl(run, k, args, 60);
            crate::pvcguard::KOut { ok: o.ok(), stdout: o.stdout.clone(), text: o.combined() }
        };
        let g = crate::pvcguard::guard(&lint.text, Some(IN_NS), &kube)?;
        a.done.extend(g.notes.iter().cloned());
        if g.manifest.trim() != lint.text.trim() {
            std::fs::write(&file, &g.manifest).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        let o = kubectl(run, k, &["apply", "-f", &f, "-n", IN_NS], 90);
        if !o.ok() {
            return Err(format!("kubectl apply failed: {}", crate::pvcguard::explain_apply_error(o.combined().trim())));
        }
        a.done.push(format!("applied the registry objects to {IN_NS}"));
        let r = kubectl(run, k, &["rollout", "status", &format!("deployment/{IN_NAME}"), "-n", IN_NS, "--timeout=120s"], 150);
        if r.ok() {
            a.done.push("the registry pod is running".into());
        } else {
            a.done.push(format!("the registry pod is not ready yet ({}); check with k8s_get pods namespace={IN_NS}", r.combined().lines().next().unwrap_or("").trim()));
        }
        {
            let up = crate::netprobe::get_ok(&format!("http://{}/v2/", p.registry), 4);
            a.done.push(if up { format!("{} answers GET /v2/", p.registry) } else { format!("{} did not answer GET /v2/ from this machine yet (the pod may still be starting, or the NodePort is not reachable here)", p.registry) });
        }
    }
    if let Some((path, yaml)) = &p.file {
        if p.env.is_root {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(path, yaml).map_err(|e| format!("{}: {e}", path.display()))?;
            a.done.push(format!("wrote {}", path.display()));
        } else {
            a.to_run.push(tee_commands(yaml));
            a.to_run.push(RESTART.to_string());
        }
    }
    if let Some(argv) = &p.container {
        let engine = &argv[0];
        let cwd = std::env::temp_dir();
        let t = Duration::from_secs(600);
        let exists = run.run(&cwd, &[engine.clone(), "inspect".into(), LOCAL_NAME.into()], &[], Duration::from_secs(20), 4096, None).ok();
        let (what, args) = if exists { ("started the existing registry container", vec![engine.clone(), "start".into(), LOCAL_NAME.into()]) } else { ("created the registry container", argv.clone()) };
        let o = run.run(&cwd, &args, &[], t, 16 * 1024, None);
        if !o.ok() {
            return Err(format!("{} failed: {}", args.join(" "), o.combined().trim()));
        }
        a.done.push(what.into());
    }
    if let Some(root) = &p.env.project_root {
        if !p.registry.is_empty() || p.option == RegistryOption::None {
            let mut c = crate::workspace::ProjectConfig::load(root)?;
            c.k8s.registry = p.registry.clone();
            c.k8s.registry_pull = if p.pull == p.registry { String::new() } else { p.pull.clone() };
            c.k8s.registry_kind = p.option.kind().to_string();
            c.save(root)?;
            a.done.push(if p.registry.is_empty() { "cleared the registry setting in .iyakki/project.json".to_string() } else { format!("recorded k8s.registry = {} in .iyakki/project.json", p.registry) });
        }
    }
    Ok(a)
}

// ------------------------------------------------------------------------------------------------------------------ status



/// The saved choice plus `GET /v2/` from this machine (http, then https without verification).
pub fn status(c: &crate::workspace::K8sConfig, _run: &dyn Runner) -> RegStatus {
    let mut s = RegStatus::saved(c);
    if s.push.is_empty() {
        return s;
    }
    let get = |scheme: &str, k: bool| crate::netprobe::call("GET", &format!("{scheme}://{}/v2/", s.push), None, 3, 4096, k).map(|a| a.success()).unwrap_or(false);
    s.reachable = Some(get("http", false) || get("https", true));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsexec::FakeRunner;

    const ERR: &str = "Failed to pull image \"192.168.1.68:30500/shop-dev/web:abc\": failed to do request: Head \"https://192.168.1.68:30500/v2/shop-dev/web/manifests/abc\": http: server gave HTTP response to HTTPS client";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-regsetup-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env(d: &Path, root: bool) -> PlanEnv {
        PlanEnv { is_root: root, engine: Some("podman".into()), registries_yaml: d.join("etc/registries.yaml"), project_root: Some(d.join("ws")), ..Default::default() }
    }

    fn kube_env(d: &Path, root: bool, host: &str, facts: Facts) -> PlanEnv {
        PlanEnv { kube: Some(KubeCtx { kubeconfig: "/tmp/k.yaml".into(), context: "default".into() }), api_host: host.into(), facts, ..env(d, root) }
    }

    fn free() -> Facts {
        Facts { storage_class: Some("local-path".into()), ..Default::default() }
    }

    #[test]
    fn the_http_to_https_error_gives_the_cause_the_yaml_and_the_commands_for_every_node() {
        let t = diagnose(ERR, "").unwrap();
        assert!(t.contains("plain HTTP") && t.contains("192.168.1.68:30500"));
        assert!(t.contains("mirrors:\n  \"192.168.1.68:30500\":\n    endpoint:\n      - \"http://192.168.1.68:30500\""), "{t}");
        assert!(t.contains("kubectl set image") && t.contains("localhost:30500/<namespace>") && t.find("kubectl set image") < t.find("registries.yaml"), "{t}");
        assert!(t.contains("/etc/rancher/k3s/registries.yaml") && t.contains("sudo systemctl restart k3s ") && t.contains("k3s-agent") && t.contains("EACH node"), "{t}");
        assert!(t.contains("hostname the nodes already trust"), "{t}");
        assert!(!t.to_lowercase().contains("docker hub") && !t.contains("docker.io"), "never Docker Hub: {t}");
    }

    #[test]
    fn x509_auth_and_refused_errors_are_recognised_and_other_text_is_not() {
        let x = diagnose("failed to pull \"10.0.0.5:5000/a:1\": tls: failed to verify certificate: x509: certificate signed by unknown authority from https://10.0.0.5:5000/v2/", "").unwrap();
        assert!(x.contains("x509") && x.contains("insecure_skip_verify") && x.contains("10.0.0.5:5000"), "{x}");
        let a = diagnose("pull access denied ... failed to authorize: no basic auth credentials", "reg.lan:5000").unwrap();
        assert!(a.contains("reg.lan:5000") && a.contains("imagePullSecrets") && !a.contains("password: hunter"), "{a}");
        let c = diagnose("failed to pull image: dial tcp 10.0.0.9:30500: connect: connection refused", "").unwrap();
        assert!(c.contains("10.0.0.9:30500") && c.contains("curl"), "{c}");
        assert!(diagnose("back-off restarting failed container", "x:1").is_none());
        assert!(diagnose("dial tcp 10.0.0.9:30500: connection refused", "").is_none(), "a refused connection that is not a pull is not blamed on a registry");
    }

    #[test]
    fn the_local_option_plans_the_mirror_the_container_and_the_setting_and_never_takes_root() {
        let d = tmp("local");
        let p = plan(&RegistryOption::Local, &env(&d, false)).unwrap();
        let v = preview(&p);
        assert!(v.contains("localhost:5000") && v.contains("podman run -d --restart=always --name iyakki-registry") && v.contains("iyakki-registry:/var/lib/registry"), "{v}");
        assert!(v.contains("not root") && v.contains("sudo tee /etc/rancher/k3s/registries.yaml"), "{v}");
        let r = FakeRunner::new(&["podman"]);
        r.fail("inspect", 1, "no such container");
        r.answer("run -d", "abc");
        std::fs::create_dir_all(d.join("ws")).unwrap();
        assert!(apply(&p, &r, false).is_err(), "no approval, no action");
        assert!(r.calls().is_empty());
        let a = apply(&p, &r, true).unwrap();
        assert!(!d.join("etc/registries.yaml").exists(), "not root: nothing written");
        assert!(a.to_run.iter().any(|c| c.contains("sudo tee")) && a.to_run.iter().any(|c| c.contains("k3s-agent")));
        assert!(r.lines().iter().any(|l| l.starts_with("podman run -d")));
        assert_eq!(crate::workspace::ProjectConfig::load(&d.join("ws")).unwrap().k8s.registry, "localhost:5000");
    }

    #[test]
    fn as_root_the_file_is_written_before_anything_starts_and_an_existing_one_is_never_overwritten() {
        let d = tmp("root");
        let e = env(&d, true);
        let p = plan(&RegistryOption::parse("remote", "http://10.0.0.5:5000/", "http", false).unwrap(), &e).unwrap();
        let r = FakeRunner::new(&[]);
        let a = apply(&p, &r, true).unwrap();
        assert!(a.to_run.is_empty() && r.calls().is_empty());
        let t = std::fs::read_to_string(&e.registries_yaml).unwrap();
        assert_eq!(t, http_mirror_yaml("10.0.0.5:5000"));
        assert_eq!(crate::workspace::ProjectConfig::load(&d.join("ws")).unwrap().k8s.registry, "10.0.0.5:5000");
        // second plan: already there, and a different registry is not merged in silently
        let again = plan(&RegistryOption::Local, &e).unwrap();
        assert!(again.file.is_none() && preview(&again).contains("never overwritten"));
        let same = plan(&RegistryOption::parse("remote", "10.0.0.5:5000", "http", false).unwrap(), &e).unwrap();
        assert!(same.file.is_none() && preview(&same).contains("already has an entry"));
        assert_eq!(std::fs::read_to_string(&e.registries_yaml).unwrap(), t);
    }

    #[test]
    fn existing_container_is_started_and_options_are_validated() {
        let d = tmp("exists");
        let p = plan(&RegistryOption::Local, &env(&d, true)).unwrap();
        let r = FakeRunner::new(&["podman"]);
        r.answer("inspect", "{}");
        r.answer("start iyakki-registry", "iyakki-registry");
        let a = apply(&p, &r, true).unwrap();
        assert!(a.done.iter().any(|x| x.contains("started the existing")) && !r.lines().iter().any(|l| l.contains("run -d")));
        let mut no_engine = env(&d, true);
        no_engine.engine = None;
        assert!(plan(&RegistryOption::Local, &no_engine).unwrap_err().contains("podman or docker"));
        assert!(RegistryOption::parse("remote", "nonsense", "http", false).is_err());
        assert!(RegistryOption::parse("weird", "", "", false).is_err());
        assert_eq!(RegistryOption::parse("none", "", "", false).unwrap(), RegistryOption::None);
        let https = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", false).unwrap(), &env(&tmp("https"), true)).unwrap();
        assert!(https.file.is_none(), "a trusted https registry needs no mirror");
        let insecure = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", true).unwrap(), &env(&tmp("https2"), true)).unwrap();
        assert!(preview(&insecure).contains("insecure_skip_verify"));
    }

    #[test]
    fn the_in_cluster_plan_is_namespaced_objects_that_pass_the_manifest_check_and_needs_no_root() {
        let d = tmp("in-plan");
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", free())).unwrap();
        assert_eq!((p.registry.as_str(), p.pull.as_str()), ("localhost:30500", "localhost:30500"));
        let m = p.manifest.clone().unwrap();
        assert!(m.contains("kind: PersistentVolumeClaim") && m.contains("storageClassName: local-path") && m.contains("hostPort: 30500\n          hostIP: 127.0.0.1") && m.contains("type: ClusterIP") && !m.contains("nodePort") && !m.contains("NodePort") && m.contains("docker.io/library/registry:2"), "{m}");
        assert!(!m.contains("kind: Namespace") && !m.contains("hostPath") && !m.contains("emptyDir"), "{m}");
        let lint = crate::deploylint::lint(&m, IN_NS, &|_| false);
        assert!(lint.ok(), "{}", lint.report());
        let v = preview(&p);
        assert!(v.contains("create namespace iyakki-registry") && v.contains("cluster-scoped") && v.contains("Root: not needed") && v.contains("k8s.registry = \"localhost:30500\""), "{v}");
        assert!(v.contains("localhost:30500/<namespace>/<image>:<tag>") && v.contains("no node needs a registries.yaml"), "{v}");
        assert!(p.file.is_none() && p.container.is_none(), "no node file, no container, so nothing for root");
        // a cluster on another machine cannot be pushed to over loopback: refused, nothing planned
        let e = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "192.168.1.68", free())).unwrap_err();
        assert!(e.contains("127.0.0.1") && e.contains("192.168.1.68"), "{e}");
        // an older NodePort install is called out as a security fix in the plan
        let old = Facts { existing_port: Some(30500), legacy_exposed: true, ..free() };
        let n = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", old)).unwrap();
        assert!(n.notes.iter().any(|x| x.contains("SECURITY") && x.contains("ClusterIP") && x.contains("127.0.0.1:30500")), "{:?}", n.notes);
    }

    #[test]
    fn the_in_cluster_plan_picks_a_free_port_falls_back_to_emptydir_and_validates() {
        let d = tmp("in-port");
        let busy = Facts { taken_ports: vec![30500, 30501], ..Default::default() };
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", busy.clone())).unwrap();
        assert_eq!(p.registry, "localhost:30502");
        let m = p.manifest.unwrap();
        assert!(m.contains("emptyDir: {}") && !m.contains("PersistentVolumeClaim") && m.contains("hostPort: 30502") && !m.contains("nodePort"), "{m}");
        assert!(preview(&plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", busy.clone())).unwrap()).contains("no default StorageClass"));
        assert!(plan(&RegistryOption::InCluster { node_port: 30501 }, &kube_env(&d, false, "", busy)).unwrap_err().contains("already used"));
        let again = Facts { existing_port: Some(30777), taken_ports: vec![30500], ..free() };
        assert_eq!(plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", again)).unwrap().registry, "localhost:30777", "re-running keeps our own port");
        assert!(plan(&RegistryOption::InCluster { node_port: 0 }, &env(&d, false)).unwrap_err().contains("connected cluster"));
        assert_eq!(RegistryOption::parse("in-cluster", "31000", "", false).unwrap(), RegistryOption::InCluster { node_port: 31000 });
        assert!(RegistryOption::parse("in-cluster", "80", "", false).is_err());
        assert!(RegistryOption::parse("in-cluster", "abc", "", false).is_err());
        assert_eq!(RegistryOption::parse("system", "", "", false).unwrap(), RegistryOption::Local);
    }

    #[test]
    fn applying_the_in_cluster_option_creates_the_namespace_once_applies_waits_and_records_both_addresses() {
        let d = tmp("in-apply");
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", free())).unwrap();
        let r = FakeRunner::new(&["kubectl"]);
        r.fail("get namespace iyakki-registry", 1, "Error from server (NotFound): namespaces \"iyakki-registry\" not found");
        r.answer("create namespace iyakki-registry", "namespace/iyakki-registry created");
        r.fail("get pvc", 1, "Error from server (NotFound): persistentvolumeclaims not found");
        r.answer("apply -f", "deployment.apps/registry created\nservice/registry created");
        r.answer("rollout status", "successfully rolled out");
        crate::netprobe::fake("/v2/", 200, &[], "{}");
        assert!(apply(&p, &r, false).is_err());
        assert!(r.calls().is_empty(), "no approval, no kubectl");
        let a = apply(&p, &r, true).unwrap();
        let lines = r.lines();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle} in {lines:?}"));
        assert!(pos("create namespace") < pos("apply -f") && pos("apply -f") < pos("rollout status"));
        assert!(lines.iter().all(|l| !l.contains("sudo") && !l.contains("tee ")), "{lines:?}");
        assert!(lines[pos("apply -f")].contains("-n iyakki-registry") && lines[pos("apply -f")].contains("--kubeconfig /tmp/k.yaml"));
        assert!(a.to_run.is_empty() && a.done.iter().any(|x| x.contains("created namespace")) && a.done.iter().any(|x| x.contains("answers GET /v2/")), "{a:?}");
        let ws = d.join("ws");
        assert!(ws.join(".iyakki/deploy/iyakki-registry.yaml").is_file());
        let c = crate::workspace::ProjectConfig::load(&ws).unwrap();
        assert_eq!((c.k8s.registry.as_str(), c.k8s.registry_pull.as_str(), c.k8s.registry_kind.as_str()), ("localhost:30500", "", "in-cluster"));
        // a second run finds the namespace: it is not created again
        let r2 = FakeRunner::new(&["kubectl"]);
        r2.answer("get namespace iyakki-registry", "namespace/iyakki-registry");
        r2.fail("get pvc", 1, "Error from server (NotFound): persistentvolumeclaims not found");
        r2.answer("apply -f", "unchanged");
        r2.answer("rollout status", "ok");
        apply(&p, &r2, true).unwrap();
        assert!(!r2.lines().iter().any(|l| l.contains("create namespace")));
        // a failing apply stops with kubectl's words and records nothing new
        let d2 = tmp("in-fail");
        let p2 = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d2, false, "", free())).unwrap();
        let r3 = FakeRunner::new(&["kubectl"]);
        r3.answer("get namespace iyakki-registry", "namespace/iyakki-registry");
        r3.fail("get pvc", 1, "Error from server (NotFound): persistentvolumeclaims not found");
        r3.fail("apply -f", 1, "error: forbidden");
        assert!(apply(&p2, &r3, true).unwrap_err().contains("forbidden"));
        assert!(!d2.join("ws/.iyakki/project.json").exists());
        let r4 = FakeRunner::new(&["kubectl"]);
        r4.fail("get namespace", 1, "Unable to connect to the server");
        assert!(apply(&p2, &r4, true).unwrap_err().contains("could not check namespace"));
    }

    #[test]
    fn probing_reads_the_default_storage_class_and_the_nodeports_in_use() {
        let r = FakeRunner::new(&["kubectl"]);
        r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"fast"}},{"metadata":{"name":"local-path","annotations":{"storageclass.kubernetes.io/is-default-class":"true"}}}]}"#);
        r.answer("get services", r#"{"items":[{"metadata":{"name":"web","namespace":"a"},"spec":{"ports":[{"nodePort":30500}]}},{"metadata":{"name":"registry","namespace":"iyakki-registry"},"spec":{"ports":[{"nodePort":30600}]}}]}"#);
        let f = probe(&r, &KubeCtx::default());
        assert_eq!((f.storage_class.as_deref(), f.taken_ports.clone(), f.existing_port), (Some("local-path"), vec![30500], Some(30600)));
        let r2 = FakeRunner::new(&["kubectl"]);
        r2.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path"}}]}"#);
        r2.fail("get services", 1, "forbidden");
        let f2 = probe(&r2, &KubeCtx::default());
        assert_eq!(f2.storage_class.as_deref(), Some("local-path"), "local-path without being the default is still used");
        assert!(f2.taken_ports.is_empty());
        assert_eq!(probe(&FakeRunner::new(&["kubectl"]), &KubeCtx::default()).storage_class, None);
    }

    #[test]
    fn probing_finds_the_loopback_port_of_a_converted_registry_and_flags_an_open_one() {
        let r = FakeRunner::new(&["kubectl"]);
        r.answer("get storageclass", r#"{"items":[{"metadata":{"name":"local-path"}}]}"#);
        r.answer("get services", r#"{"items":[{"metadata":{"name":"registry","namespace":"iyakki-registry"},"spec":{"type":"ClusterIP","ports":[{"port":5000}]}}]}"#);
        r.answer("get deployment registry", r#"{"spec":{"template":{"spec":{"containers":[{"name":"registry","ports":[{"containerPort":5000,"hostPort":30611,"hostIP":"127.0.0.1"}]}]}}}}"#);
        let f = probe(&r, &KubeCtx::default());
        assert_eq!((f.existing_port, f.legacy_exposed), (Some(30611), false));
        let old = FakeRunner::new(&["kubectl"]);
        old.answer("get services", r#"{"items":[{"metadata":{"name":"registry","namespace":"iyakki-registry"},"spec":{"type":"NodePort","ports":[{"port":5000,"nodePort":30500}]}}]}"#);
        let f = probe(&old, &KubeCtx::default());
        assert_eq!((f.existing_port, f.legacy_exposed), (Some(30500), true));
    }

    #[test]
    fn applying_over_an_open_nodeport_registry_closes_it_first() {
        let d = tmp("in-apply-legacy");
        let facts = Facts { existing_port: Some(30500), legacy_exposed: true, ..free() };
        let p = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", facts)).unwrap();
        let r = FakeRunner::new(&["kubectl"]);
        r.answer("get namespace iyakki-registry", "namespace/iyakki-registry");
        r.answer("get service registry", r#"{"spec":{"type":"NodePort","ports":[{"port":5000,"nodePort":30500}]}}"#);
        r.answer("patch", "patched");
        r.fail("get pvc", 1, "Error from server (NotFound): persistentvolumeclaims not found");
        r.answer("apply -f", "configured");
        r.answer("rollout status", "ok");
        let a = apply(&p, &r, true).unwrap();
        let lines = r.lines();
        let pos = |n: &str| lines.iter().position(|l| l.contains(n)).unwrap_or_else(|| panic!("{n} in {lines:?}"));
        assert!(pos("patch service registry") < pos("patch deployment registry") && pos("patch deployment registry") < pos("apply -f"), "{lines:?}");
        assert!(a.done.iter().any(|x| x.starts_with("SECURITY:") && x.contains("NodePort 30500 closed")), "{a:?}");
    }

    #[test]
    fn root_is_said_plainly_for_each_option_and_none_clears_the_setting() {
        let d = tmp("root-lines");
        let a = plan(&RegistryOption::InCluster { node_port: 0 }, &kube_env(&d, false, "", free())).unwrap();
        assert_eq!(root_line(&a), "Root: not needed.");
        let b = plan(&RegistryOption::Local, &env(&d, false)).unwrap();
        assert!(root_line(&b).contains("needed to write") && root_line(&b).contains("never uses sudo"), "{}", root_line(&b));
        let c_http = plan(&RegistryOption::parse("remote", "10.0.0.5:5000", "http", false).unwrap(), &env(&d, false)).unwrap();
        assert!(root_line(&c_http).contains("needed"));
        let c_tls = plan(&RegistryOption::parse("remote", "reg.lan:443", "https", false).unwrap(), &env(&d, false)).unwrap();
        assert_eq!(root_line(&c_tls), "Root: not needed.");
        let c_local = plan(&RegistryOption::parse("remote", "localhost:31000", "http", false).unwrap(), &env(&d, false)).unwrap();
        assert_eq!(root_line(&c_local), "Root: not needed.", "localhost over http needs no mirror");
        let root = plan(&RegistryOption::Local, &env(&tmp("root-lines2"), true)).unwrap();
        assert!(root_line(&root).contains("running as root"));
        // none clears what an earlier choice saved
        let ws = d.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let mut cfg = crate::workspace::ProjectConfig::load(&ws).unwrap();
        cfg.k8s.registry = "localhost:30500".into();
        cfg.k8s.registry_pull = "localhost:30500".into();
        cfg.k8s.registry_kind = "in-cluster".into();
        cfg.save(&ws).unwrap();
        apply(&plan(&RegistryOption::None, &env(&d, false)).unwrap(), &FakeRunner::new(&[]), true).unwrap();
        let c = crate::workspace::ProjectConfig::load(&ws).unwrap();
        assert_eq!((c.k8s.registry.as_str(), c.k8s.registry_pull.as_str(), c.k8s.registry_kind.as_str()), ("", "", "none"));
    }

    #[test]
    fn the_status_chip_says_what_is_saved_and_whether_it_answers() {
        let mut c = crate::workspace::K8sConfig::default();
        assert_eq!(RegStatus::saved(&c).chip(), "registry: none");
        c.registry = "localhost:5000".into();
        assert_eq!(RegStatus::saved(&c).kind, "system");
        c.registry = "10.0.0.5:5000".into();
        assert_eq!(RegStatus::saved(&c).kind, "remote");
        c.registry = "192.168.1.68:30500".into();
        c.registry_pull = "localhost:30500".into();
        c.registry_kind = "in-cluster".into();
        let none = FakeRunner::new(&[]);
        crate::netprobe::fake("GET http://192.168.1.68:30500/v2/", 200, &[], "{}");
        let s = status(&c, &none);
        assert_eq!((s.chip().as_str(), s.ok(), s.reachable), ("registry: in-cluster at localhost:30500", true, Some(true)));
        c.registry = "192.168.1.69:30500".into();
        let s = status(&c, &none);
        assert_eq!((s.chip().as_str(), s.ok()), ("registry: in-cluster at localhost:30500 (not answering)", false));
        c.registry = String::new();
        c.registry_kind = String::new();
        assert_eq!(status(&c, &none).reachable, None, "no registry: not asked");
    }
}
