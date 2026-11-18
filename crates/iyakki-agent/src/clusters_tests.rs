// #[cfg(test)] -- this whole file is test code (clusters.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Tests of the cluster registry: load/save/migration, limits, distinct instances, active and per-workspace choice, role gating,
//! confirmation texts, the memory gate and independent start/stop of two local clusters. Fakes only: a stand-in k3s script, a fake
//! runner, folders under the temp directory. Nothing here talks to a real cluster.

use super::*;
use crate::archive::tests::tmp;
use crate::wsexec::FakeRunner;
use std::time::{Duration, Instant};

fn reg() -> Registry {
    Registry::default()
}

#[test]
fn a_fresh_account_has_only_the_local_cluster_and_no_switcher() {
    let dir = tmp("cl-fresh");
    let r = Registry::load(&dir, "alice");
    assert_eq!(r.clusters.len(), 1);
    assert_eq!((r.clusters[0].id.as_str(), r.clusters[0].kind), (LOCAL_ID, Kind::Local));
    assert_eq!(r.active_entry().id, LOCAL_ID);
    assert!(!r.has_choice(), "one cluster: no switcher, no clutter");
    assert!(!Registry::saved(&dir, "alice"), "loading writes nothing");
    assert!(!dir.join("clusters-alice.json").exists());
}

#[test]
fn old_vault_profiles_and_the_remembered_choice_migrate_without_touching_the_old_files() {
    let dir = tmp("cl-migrate");
    crate::kubeprofiles::vault_add(&dir, "alice", "lab", "uvault:lab-kubeconfig").unwrap();
    crate::kubeprofiles::vault_add(&dir, "alice", "stage", "vault:stage-kc").unwrap();
    Choice { kubeconfig: "uvault:lab-kubeconfig".into(), context: "lab".into() }.save(&dir, "alice").unwrap();
    let before_vault = std::fs::read_to_string(dir.join("devcluster-vault-alice.json")).unwrap();
    let before_choice = std::fs::read_to_string(dir.join("devcluster-alice.json")).unwrap();
    let r = Registry::load(&dir, "alice");
    assert_eq!(r.clusters.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["This machine", "lab", "stage"]);
    let lab = r.find_name("lab").unwrap();
    assert_eq!((lab.kind, lab.kubeconfig.as_str(), lab.in_vault()), (Kind::Remote, "uvault:lab-kubeconfig", true));
    assert_eq!(r.find_name("stage").unwrap().kubeconfig, "vault:stage-kc");
    assert_eq!(r.active_entry().name, "lab", "the remembered choice stays the active cluster");
    assert_eq!(std::fs::read_to_string(dir.join("devcluster-vault-alice.json")).unwrap(), before_vault);
    assert_eq!(std::fs::read_to_string(dir.join("devcluster-alice.json")).unwrap(), before_choice);
    // saving writes the new file only; loading it again gives the same registry
    r.save(&dir, "alice").unwrap();
    assert!(Registry::saved(&dir, "alice"));
    assert_eq!(Registry::load(&dir, "alice"), r);
    assert_eq!(std::fs::read_to_string(dir.join("devcluster-vault-alice.json")).unwrap(), before_vault);
    // another account is not affected
    assert_eq!(Registry::load(&dir, "bob").clusters.len(), 1);
}

#[test]
fn a_remembered_kubeconfig_path_becomes_a_remote_entry_and_the_machines_k3s_stays_local() {
    let dir = tmp("cl-migrate2");
    Choice { kubeconfig: "/home/x/.kube/gpu-box.yaml".into(), context: "gpu".into() }.save(&dir, "alice").unwrap();
    let r = Registry::load(&dir, "alice");
    let e = r.active_entry();
    assert_eq!((e.kind, e.kubeconfig.as_str(), e.context.as_str(), e.name.as_str()), (Kind::Remote, "/home/x/.kube/gpu-box.yaml", "gpu", "gpu-box"));
    let dir2 = tmp("cl-migrate3");
    Choice { kubeconfig: "/etc/rancher/k3s/k3s.yaml".into(), context: String::new() }.save(&dir2, "alice").unwrap();
    let r2 = Registry::load(&dir2, "alice");
    assert_eq!((r2.clusters.len(), r2.active.as_str()), (1, LOCAL_ID));
}

#[test]
fn a_damaged_or_hand_edited_file_is_repaired_on_load() {
    let dir = tmp("cl-repair");
    std::fs::write(dir.join("clusters-alice.json"), r#"{"clusters":[{"id":"x1","name":"X","kind":"remote","kubeconfig":"/k"},{"id":"x1","name":"dup","kind":"remote","kubeconfig":"/k"},{"id":"ex","name":"Ex","kind":"local"}],"active":"gone","workspace_active":{"w":"gone","v":"x1"}}"#).unwrap();
    let r = Registry::load(&dir, "alice");
    assert_eq!(r.clusters[0].id, LOCAL_ID, "local is always first");
    assert_eq!(r.clusters.iter().filter(|c| c.id == "x1").count(), 1);
    assert_eq!(r.active, LOCAL_ID);
    assert_eq!(r.get("ex").unwrap().slot, 1, "an extra local cluster without a slot gets one");
    assert!(!r.workspace_active.contains_key("w") && r.workspace_active.get("v").map(String::as_str) == Some("x1"));
    std::fs::write(dir.join("clusters-bob.json"), "not json").unwrap();
    assert_eq!(Registry::load(&dir, "bob").clusters.len(), 1, "unreadable: start again from the old settings, never crash");
}

#[test]
fn local_clusters_have_a_default_maximum_of_two_and_advanced_raises_it() {
    let mut r = reg();
    let a = r.add_local("Second").unwrap();
    assert_eq!((a.id.as_str(), a.slot, a.kind), ("second", 1, Kind::Local));
    let e = r.add_local("Third").unwrap_err();
    assert!(e.contains("2 local clusters") && e.contains("Advanced") && e.contains("2 GB"), "{e}");
    r.advanced = true;
    let b = r.add_local("Third").unwrap();
    let c = r.add_local("Fourth").unwrap();
    assert_eq!((b.slot, c.slot), (2, 3));
    assert!(r.add_local("Fifth").unwrap_err().contains("4 local clusters"));
    // remote clusters do not count against the local maximum
    r.add_remote("gpu-box", Kind::Remote, "/k.yaml", "", "").unwrap();
    // two entries never share a port, an address range, a folder or a node name
    assert!(Fleet::port_clashes(&r).is_empty(), "{:?}", Fleet::port_clashes(&r));
    let ins: Vec<Instance> = r.clusters.iter().filter_map(|c| c.instance()).collect();
    assert_eq!(ins.len(), 4);
    for (i, x) in ins.iter().enumerate() {
        for y in ins.iter().skip(i + 1) {
            assert!(x.dir_name != y.dir_name && x.node_name != y.node_name && x.cluster_cidr != y.cluster_cidr && x.service_cidr != y.service_cidr && x.api_port != y.api_port);
        }
    }
    // a removed slot is reused
    r.remove("third").unwrap();
    assert_eq!(r.add_local("Again").unwrap().slot, 2);
}

#[test]
fn names_are_checked_and_ids_are_stable_when_renamed() {
    let mut r = reg();
    assert!(r.add_local("").is_err() && r.add_local("a/b").is_err() && r.add_local(&"x".repeat(41)).is_err());
    let e = r.add_local("Local").unwrap();
    assert_ne!(e.id, LOCAL_ID, "a new cluster can be called Local, its id is not `local`");
    assert!(r.add_local("local").unwrap_err().contains("exists already") || r.local_count() == 2);
    let mut r = reg();
    let g = r.add_remote("GPU box", Kind::Remote, "/k.yaml", "ctx", "dev").unwrap();
    assert_eq!(g.id, "gpu-box");
    assert!(r.add_remote("gpu BOX", Kind::Tunnel, "/k2.yaml", "", "").unwrap_err().contains("exists already"));
    r.rename("gpu-box", "Lab").unwrap();
    assert_eq!((r.get("gpu-box").unwrap().name.as_str(), r.find_name("lab").unwrap().id.as_str()), ("Lab", "gpu-box"));
    assert!(r.rename("gpu-box", "This machine").is_err());
    r.rename(LOCAL_ID, "My PC").unwrap();
    assert_eq!(r.get(LOCAL_ID).unwrap().id, LOCAL_ID);
}

#[test]
fn remote_profiles_take_a_path_or_a_vault_entry_in_each_kind_and_refuse_nonsense() {
    let mut r = reg();
    for (i, k) in [Kind::Remote, Kind::Tunnel, Kind::CloudCli].into_iter().enumerate() {
        let e = r.add_remote(&format!("c{i}"), k, "/k.yaml", "", "").unwrap();
        assert_eq!(e.kind, k);
        assert_eq!(Kind::from_word(k.word()), Some(k));
    }
    assert!(r.add_remote("x", Kind::Local, "/k", "", "").is_err());
    assert!(r.add_remote("y", Kind::Remote, "  ", "", "").unwrap_err().contains("kubeconfig"));
    assert!(r.add_remote("z", Kind::Remote, "uvault:Bad Name", "", "").unwrap_err().contains("vault entry"));
    let v = r.add_remote("vaulted", Kind::Remote, "uvault:lab-kc", "", "").unwrap();
    assert!(v.in_vault());
    assert_eq!(r.add_remote("v2", Kind::Remote, "vault:lab-kc", "", "").unwrap().kubeconfig, "vault:lab-kc");
    // the program is given the reference as it is: the runner's vault handling turns it into a temp file
    assert_eq!(v.kubeconfig_for(Path::new("/d")), "uvault:lab-kc");
    // a local entry is given its own folder's kubeconfig
    let l = r.add_local("Second").unwrap();
    assert_eq!(l.kubeconfig_for(Path::new("/d")), "/d/k3s-dev-second/kubeconfig");
    assert_eq!(ClusterEntry::local_primary().kubeconfig_for(Path::new("/d")), "/d/k3s-dev/kubeconfig");
}

#[test]
fn removing_a_cluster_resets_active_and_overrides_and_the_local_one_cannot_go() {
    let mut r = reg();
    r.add_remote("gpu-box", Kind::Remote, "uvault:gpu-kc", "", "").unwrap();
    r.set_active("gpu-box").unwrap();
    r.set_workspace("web", Some("gpu-box")).unwrap();
    assert!(r.remove(LOCAL_ID).unwrap_err().contains("Uninstall"));
    let e = r.remove("gpu-box").unwrap();
    assert_eq!(e.name, "gpu-box");
    assert_eq!((r.active.as_str(), r.workspace_active.len()), (LOCAL_ID, 0));
    assert!(r.remove("gpu-box").is_err());
}

#[test]
fn forgetting_a_remote_profile_keeps_its_vault_entry_unless_asked() {
    let dir = tmp("cl-forget");
    crate::kubeprofiles::vault_add(&dir, "alice", "lab", "uvault:lab-kc").unwrap();
    let mut r = Registry::load(&dir, "alice");
    let id = r.find_name("lab").unwrap().id.clone();
    let mut deleted: Vec<String> = vec![];
    forget_remote(&dir, "alice", &mut r, &id, false, &mut |n| {
        deleted.push(n.into());
        Ok(())
    })
    .unwrap();
    assert!(deleted.is_empty(), "the vault entry stays");
    assert_eq!(crate::kubeprofiles::vault_load(&dir, "alice").len(), 1, "the old profile list is not touched either");
    let mut r2 = Registry::load(&dir, "alice");
    forget_remote(&dir, "alice", &mut r2, &id, true, &mut |n| {
        deleted.push(n.into());
        Ok(())
    })
    .unwrap();
    assert_eq!(deleted, ["uvault:lab-kc"]);
    assert!(crate::kubeprofiles::vault_load(&dir, "alice").is_empty());
    let local = r2.get(LOCAL_ID).unwrap().id.clone();
    assert!(forget_remote(&dir, "alice", &mut r2, &local, true, &mut |_| Ok(())).is_err());
}

#[test]
fn the_active_cluster_and_a_workspace_override_are_chosen_and_followed() {
    let dir = tmp("cl-active");
    let data = dir.join("data");
    let mut r = Registry::load(&dir, "alice");
    r.add_remote("gpu-box", Kind::Remote, "/k.yaml", "gpu-ctx", "").unwrap();
    let second = r.add_local("Second").unwrap();
    r.save(&dir, "alice").unwrap();
    assert!(r.has_choice());
    assert_eq!(r.effective("web").id, LOCAL_ID);
    let e = switch(&dir, &data, "alice", Role::Admin, "gpu-box").unwrap();
    assert_eq!(e.id, "gpu-box");
    let r = Registry::load(&dir, "alice");
    assert_eq!((r.active.as_str(), r.effective("web").id.as_str()), ("gpu-box", "gpu-box"));
    // the old remembered choice follows, so code that reads it uses the same cluster
    let c = Choice::load(&dir, "alice").unwrap();
    assert_eq!((c.kubeconfig.as_str(), c.context.as_str()), ("/k.yaml", "gpu-ctx"));
    // a workspace override wins for that workspace only, and can be cleared
    switch_workspace(&dir, "alice", Role::User, "web", Some(&second.id)).unwrap();
    let r = Registry::load(&dir, "alice");
    assert_eq!((r.effective("web").id.as_str(), r.effective("api").id.as_str()), ("second", "gpu-box"));
    assert_eq!(choice_for(&r.effective("web"), &data).kubeconfig, format!("{}/k3s-dev-second/kubeconfig", data.display()));
    switch_workspace(&dir, "alice", Role::User, "web", None).unwrap();
    assert_eq!(Registry::load(&dir, "alice").effective("web").id, "gpu-box");
    // switching moved and deleted nothing
    assert_eq!(Registry::load(&dir, "alice").clusters.len(), 3);
    assert!(switch(&dir, &data, "alice", Role::Admin, "nope").is_err());
}

#[test]
fn roles_manage_switch_or_see_nothing() {
    assert!(can_manage(Role::Admin) && can_manage(Role::Operator) && !can_manage(Role::User) && !can_manage(Role::Guest));
    assert!(can_switch(Role::Admin) && can_switch(Role::User) && !can_switch(Role::Guest));
    assert!(manage_refusal(Role::User).unwrap().contains("administrator or operator") && manage_refusal(Role::Operator).is_none());
    let dir = tmp("cl-roles");
    let mut r = Registry::load(&dir, "u");
    r.add_remote("private-lab", Kind::Remote, "/k.yaml", "", "").unwrap();
    r.add_remote("shared", Kind::Remote, "/k2.yaml", "", "").unwrap();
    r.set_visible_to_users("private-lab", false).unwrap();
    r.save(&dir, "u").unwrap();
    let r = Registry::load(&dir, "u");
    assert_eq!(r.visible_to(Role::Guest).len(), 0);
    assert_eq!(r.visible_to(Role::User).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["local", "shared"]);
    assert_eq!(r.visible_to(Role::Operator).len(), 3);
    assert!(switch(&dir, &dir, "u", Role::Guest, "local").unwrap_err().contains("guests"));
    assert!(switch(&dir, &dir, "u", Role::User, "private-lab").unwrap_err().contains("not available"));
    assert!(switch(&dir, &dir, "u", Role::User, "shared").is_ok());
    assert!(switch_workspace(&dir, "u", Role::Guest, "w", Some("local")).is_err());
    // the built-in cluster is always visible to users
    let mut r2 = r.clone();
    r2.set_visible_to_users(LOCAL_ID, false).unwrap();
    assert!(r2.get(LOCAL_ID).unwrap().visible_to_users);
}

#[test]
fn confirmation_texts_name_the_cluster_and_its_kind() {
    let mut r = reg();
    let g = r.add_remote("gpu-box", Kind::Remote, "/k.yaml", "", "").unwrap();
    assert_eq!(confirm_text("Deploy Lemonade", &g), "Deploy Lemonade to cluster \"gpu-box\" (remote)");
    assert_eq!(confirm_text_from("Remove Registry", &g), "Remove Registry from cluster \"gpu-box\" (remote)");
    assert_eq!(confirm_text("Deploy Registry", &ClusterEntry::local_primary()), "Deploy Registry to cluster \"This machine\" (local)");
    let t = r.add_remote("tun", Kind::Tunnel, "/k.yaml", "", "").unwrap();
    assert!(confirm_text("Apply", &t).ends_with("(tunnel)"));
}

#[test]
fn production_looking_clusters_stay_read_only_for_the_assistant() {
    let mut r = reg();
    let p = r.add_remote("prod-eu", Kind::Remote, "/k.yaml", "", "").unwrap();
    assert!(p.read_only_reason().unwrap().contains("production"));
    let np = r.add_remote("nonprod-eu", Kind::Remote, "/k2.yaml", "", "").unwrap();
    assert!(np.read_only_reason().is_none());
    assert!(ClusterEntry::local_primary().read_only_reason().is_none());
}

#[test]
fn the_memory_gate_never_refuses_the_first_local_cluster_and_refuses_a_second_when_memory_is_low() {
    assert!(local_start_gate(0, Some(0)).is_ok() && local_start_gate(0, None).is_ok(), "today's single-cluster behaviour is unchanged");
    assert!(local_start_gate(1, Some(LOCAL_K3S_NEED)).is_ok());
    let low = local_start_gate(1, Some(1 << 30)).unwrap_err();
    assert!(low.contains("1.0 GB") && low.contains("2.5 GB") && low.contains("Stop the other"), "{low}");
    assert!(local_start_gate(2, None).unwrap_err().contains("cannot be read"));
    assert!(COST_WARNING.contains("2 GB") && COST_WARNING.contains("CPU"));
}

#[test]
fn status_lines_dots_and_staleness() {
    let n = r#"{"items":[{"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.31.1+k3s1"}}},{"status":{"conditions":[{"type":"Ready","status":"False"}]}}]}"#;
    assert_eq!(parse_nodes(n), Some((2, 1, "v1.31.1+k3s1".into())));
    assert!(parse_nodes("nope").is_none() && parse_nodes("{}").is_none());
    let run: Arc<dyn Runner> = {
        let f = FakeRunner::new(&["kubectl"]);
        f.answer("get nodes", n);
        Arc::new(f)
    };
    let e = ClusterEntry { id: "g".into(), name: "g".into(), kubeconfig: "uvault:g-kc".into(), ..ClusterEntry::default() };
    let s = probe(&run, &e);
    assert_eq!((s.dot, s.nodes, s.ready), (Dot::Running, 2, 1));
    assert!(s.line().contains("running") && s.line().contains("1/2 nodes ready") && s.line().contains("v1.31.1"));
    assert_eq!(s.clone().aged(s.checked_at + STALE_AFTER_SECS + 1).dot, Dot::Stale);
    assert_eq!(s.clone().aged(s.checked_at + 5).dot, Dot::Running);
    let bad: Arc<dyn Runner> = {
        let f = FakeRunner::new(&["kubectl"]);
        f.fail("get nodes", 1, "Unable to connect to the server: dial tcp: i/o timeout\nmore");
        Arc::new(f)
    };
    let s = probe(&bad, &e);
    assert_eq!(s.dot, Dot::Unreachable);
    assert!(s.last_error.contains("Unable to connect") && !s.last_error.contains("more"));
    assert!(gpu_summary("g").is_none(), "the GPU badge hook returns nothing until the AI hardware profile is connected");
    assert_eq!(Dot::Stale.word(), "stale");
}

// ------------------------------------------------------------------------------------------------ the fleet

const ARGS_K3S: &str = "#!/bin/sh\ncase \"$1\" in\nserver)\n  kc=\n  while [ $# -gt 0 ]; do if [ \"$1\" = \"--write-kubeconfig\" ]; then kc=\"$2\"; fi; shift; done\n  printf 'apiVersion: v1\\n' > \"$kc\"\n  exec sleep 600;;\nesac\n";

fn install_fake(d: &K3sDev, machine: &k3sdev::Machine, route: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let l = d.layout().clone();
    std::fs::create_dir_all(l.bin().parent().unwrap()).unwrap();
    std::fs::write(l.bin(), ARGS_K3S).unwrap();
    std::fs::set_permissions(l.bin(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(l.state(), "{}").unwrap();
    d.assume(true, true, false, false, machine.clone(), Duration::from_millis(10), Duration::from_millis(20));
    d.assume_connected(None);
    d.assume_route_file(route, Duration::from_millis(100));
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

struct World {
    fleet: Fleet,
    reg: Registry,
    run: Arc<FakeRunner>,
    #[allow(dead_code)]
    dir: PathBuf,
}

fn world(tag: &str) -> World {
    let dir = tmp(tag);
    let run = Arc::new(FakeRunner::new(&[]));
    run.answer("jsonpath={.items[*].status.conditions", "True");
    run.answer("kubeletVersion", "v1.31.1+k3s1");
    run.answer("get pods -A", "kube-system coredns-1 1/1 Running 0 1m\n");
    let clis = Arc::new(CliManager::new(&dir.join("cli")));
    let data = dir.join("data");
    let primary = K3sDev::new(&data, clis.clone(), run.clone());
    let machine = k3sdev::Machine { k3s_yaml: dir.join("etc/k3s.yaml"), lib_dir: dir.join("lib"), bins: vec![], units: vec![], proc_dir: dir.join("proc") };
    std::fs::create_dir_all(&machine.proc_dir).unwrap();
    let route = dir.join("route");
    std::fs::write(&route, "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\neth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n").unwrap();
    install_fake(&primary, &machine, &route);
    let fleet = Fleet::new(primary, &data, clis, run.clone());
    let mut reg = Registry::default();
    reg.advanced = true;
    for n in ["Second", "Third"] {
        let e = reg.add_local(n).unwrap();
        install_fake(&fleet.get(&e).unwrap(), &machine, &route);
    }
    reg.add_remote("gpu-box", Kind::Remote, "uvault:gpu-kc", "", "").unwrap();
    World { fleet, reg, run, dir }
}

#[test]
fn two_local_clusters_start_and_stop_independently_through_the_fleet() {
    let w = world("cl-fleet");
    let f = &w.fleet;
    w.fleet.assume_memory(Some(8 << 30));
    f.start(&w.reg, LOCAL_ID, "root", Role::Admin).unwrap();
    f.start(&w.reg, "second", "root", Role::Admin).unwrap();
    let a = f.get(w.reg.get(LOCAL_ID).unwrap()).unwrap();
    let b = f.get(w.reg.get("second").unwrap()).unwrap();
    assert!(wait_for(10, || a.state() == Svc::Running && b.state() == Svc::Running), "{:?} {:?}", a.state(), b.state());
    assert_ne!(a.layout().root, b.layout().root);
    assert_ne!(a.id(), b.id());
    assert_eq!(f.running_others(&w.reg, "second"), 1);
    f.stop(&w.reg, "second", "root", Role::Operator).unwrap();
    assert_eq!((a.state(), b.state()), (Svc::Running, Svc::Stopped));
    assert_eq!(f.status(w.reg.get(LOCAL_ID).unwrap(), false).dot, Dot::Running);
    assert_eq!(f.status(w.reg.get("second").unwrap(), false).dot, Dot::Stopped);
    f.stop(&w.reg, LOCAL_ID, "root", Role::Admin).unwrap();
    assert_eq!(f.running_others(&w.reg, "third"), 0);
}

#[test]
fn a_second_local_cluster_is_refused_when_memory_is_low_naming_the_cluster_and_the_reason() {
    let w = world("cl-fleet-mem");
    let f = &w.fleet;
    f.assume_memory(Some(1 << 30));
    // the first is never refused by the gate
    f.start(&w.reg, LOCAL_ID, "root", Role::Admin).unwrap();
    let e = f.start(&w.reg, "second", "root", Role::Admin).unwrap_err();
    assert!(e.contains("cluster \"Second\" (local)") && e.contains("1.0 GB") && e.contains("Stop the other"), "{e}");
    assert_eq!(f.get(w.reg.get("second").unwrap()).unwrap().state(), Svc::Stopped, "nothing was started");
    f.assume_memory(None);
    assert!(f.start(&w.reg, "second", "root", Role::Admin).unwrap_err().contains("cannot be read"));
    f.assume_memory(Some(8 << 30));
    f.start(&w.reg, "second", "root", Role::Admin).unwrap();
    f.stop(&w.reg, "second", "root", Role::Admin).unwrap();
    f.stop(&w.reg, LOCAL_ID, "root", Role::Admin).unwrap();
}

#[test]
fn only_administrators_and_operators_start_and_stop_and_remote_clusters_are_not_ours_to_start() {
    let w = world("cl-fleet-role");
    let f = &w.fleet;
    f.assume_memory(Some(8 << 30));
    assert!(f.start(&w.reg, "second", "bob", Role::User).unwrap_err().contains("administrator or operator"));
    assert!(f.start(&w.reg, "second", "bob", Role::Guest).is_err());
    assert!(f.stop(&w.reg, "second", "bob", Role::User).is_err());
    let e = f.start(&w.reg, "gpu-box", "root", Role::Admin).unwrap_err();
    assert!(e.contains("cluster \"gpu-box\" (remote)") && e.contains("cannot start"), "{e}");
    assert!(f.stop(&w.reg, "gpu-box", "root", Role::Admin).unwrap_err().contains("does not stop it"));
    assert!(f.start(&w.reg, "nope", "root", Role::Admin).is_err());
    // nothing was run against the remote cluster
    assert!(!w.run.lines().iter().any(|l| l.contains("gpu-kc")));
}

#[test]
fn autostart_starts_only_the_extra_clusters_marked_and_reports_refusals_by_name() {
    let mut w = world("cl-fleet-auto");
    w.fleet.assume_memory(Some(8 << 30));
    let none = w.fleet.autostart_extras(&w.reg);
    assert!(none.is_empty());
    let b = w.fleet.get(w.reg.get("second").unwrap()).unwrap();
    let c = w.fleet.get(w.reg.get("third").unwrap()).unwrap();
    assert_eq!((b.state(), c.state()), (Svc::Stopped, Svc::Stopped), "default: nothing but local autostarts");
    w.reg.set_autostart("second", true).unwrap();
    assert!(w.reg.set_autostart("gpu-box", true).is_err() && w.reg.set_autostart(LOCAL_ID, true).is_err());
    assert!(w.fleet.autostart_extras(&w.reg).is_empty());
    assert!(wait_for(10, || b.state() == Svc::Running));
    assert_eq!(c.state(), Svc::Stopped);
    // with the first one up and little memory, the second extra is refused and named
    w.reg.set_autostart("third", true).unwrap();
    w.fleet.assume_memory(Some(1 << 30));
    let errs = w.fleet.autostart_extras(&w.reg);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("cluster \"Third\" (local)"), "{errs:?}");
    b.stop("root").unwrap();
}

#[test]
fn a_remote_status_comes_from_a_read_only_probe_and_a_local_one_from_its_supervisor() {
    let w = world("cl-fleet-status");
    let g = w.reg.get("gpu-box").unwrap();
    assert_eq!(w.fleet.status(g, false).dot, Dot::Unknown, "not tested unless asked");
    w.run.answer("get nodes -o json", r#"{"items":[{"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.30.0"}}}]}"#);
    let s = w.fleet.status(g, true);
    assert_eq!((s.dot, s.nodes, s.ready, s.version.as_str()), (Dot::Running, 1, 1, "v1.30.0"));
    let line = w.run.lines().into_iter().find(|l| l.contains("get nodes")).unwrap();
    assert!(line.contains("--kubeconfig uvault:gpu-kc") && !line.contains(" apply") && !line.contains(" delete"), "{line}");
    assert_eq!(w.fleet.status(w.reg.get("second").unwrap(), true).dot, Dot::Stopped);
    assert!(Fleet::port_clashes(&w.reg).is_empty());
}

#[test]
fn local_clusters_of_two_accounts_never_share_a_slot_an_id_or_a_port() {
    let dir = tmp("cl-two-accounts");
    let a = add_local_saved(&dir, "alice", Role::Admin, "Lab").unwrap();
    let b = add_local_saved(&dir, "bob", Role::Operator, "Lab").unwrap();
    assert_ne!(a.id, b.id, "the same name in two accounts is two different clusters on disk");
    assert_ne!(a.slot, b.slot);
    let (ia, ib) = (a.instance().unwrap(), b.instance().unwrap());
    assert!(!ia.all_ports().iter().any(|p| ib.all_ports().contains(p)) && ia.dir_name != ib.dir_name && ia.cluster_cidr != ib.cluster_cidr);
    assert!(add_local_saved(&dir, "carol", Role::User, "x1").unwrap_err().contains("administrator or operator"));
    assert_eq!(taken_by_others(&dir, "alice").ids, [b.id.clone()]);
    // boot autostart looks at every account's marks once
    let mut ra = Registry::load(&dir, "alice");
    ra.set_autostart(&a.id, true).unwrap();
    ra.save(&dir, "alice").unwrap();
    let mut rb = Registry::load(&dir, "bob");
    rb.set_autostart(&b.id, true).unwrap();
    rb.save(&dir, "bob").unwrap();
    let w = world("cl-two-accounts-fleet");
    w.fleet.assume_memory(Some(8 << 30));
    // nothing is installed under those ids: nothing starts and nothing fails
    assert!(w.fleet.autostart_all(&dir).is_empty());
    assert_eq!(confirm_text_on("Start", &a), format!("Start cluster \"Lab\" (local)"));
}

// ------------------------------------------------------------------------------------------------ the GPU badge

fn gpu_nodes(count: u32) -> String {
    format!(r#"{{"items":[{{"metadata":{{"name":"g1","labels":{{"nvidia.com/gpu.product":"NVIDIA-GeForce-RTX-4090","nvidia.com/gpu.memory":"24576"}}}},"status":{{"allocatable":{{"nvidia.com/gpu":"{count}"}},"capacity":{{"nvidia.com/gpu":"{count}"}},"conditions":[{{"type":"Ready","status":"True"}}]}}}}]}}"#)
}

fn remote_entry(id: &str) -> ClusterEntry {
    ClusterEntry { id: id.into(), name: id.into(), kind: Kind::Remote, kubeconfig: format!("/fake/{id}.yaml"), ..ClusterEntry::default() }
}

#[test]
fn the_gpu_badge_comes_from_the_nodes_of_each_remote_cluster_and_never_asks_a_local_one() {
    let run = Arc::new(FakeRunner::new(&["kubectl"]));
    run.answer("gpu-fresh.yaml", &gpu_nodes(2));
    run.answer("gpu-none.yaml", r#"{"items":[{"metadata":{"name":"a"},"status":{"allocatable":{"cpu":"8"}}}]}"#);
    run.fail("gpu-down.yaml", 1, "Unable to connect to the server");
    let dynrun: Arc<dyn Runner> = run.clone();
    assert_eq!(gpu_summary("gpu-fresh"), None, "nothing is known before the first look");
    let entries = vec![ClusterEntry::local_primary(), remote_entry("gpu-fresh"), remote_entry("gpu-none"), remote_entry("gpu-down")];
    assert_eq!(refresh_gpus(&dynrun, &entries, false), 3, "the local cluster is not asked");
    assert_eq!(gpu_summary("gpu-fresh").as_deref(), Some("2 x NVIDIA GeForce RTX 4090 (24 GB each)"));
    assert_eq!(gpu_summary("gpu-none"), None, "a cluster without GPUs shows no badge");
    assert_eq!(gpu_summary("gpu-down"), None, "a cluster that never answered shows no badge");
    assert_eq!(gpu_summary(LOCAL_ID), None);
    assert!(run.lines().iter().all(|l| l.contains("get nodes -o json")), "only the read-only node list is run: {:?}", run.lines());
    // the next look waits for the back-off
    assert_eq!(refresh_gpus(&dynrun, &entries, false), 0);
}

#[test]
fn a_cluster_that_stops_answering_keeps_its_badge_marked_as_last_known() {
    let cache = crate::aiprofile_remote::clusters_cache();
    crate::aiprofile_remote::note_nodes(&cache, "gpu-stale", "gpu-stale", &gpu_nodes(1), 1000);
    assert_eq!(gpu_summary("gpu-stale").as_deref(), Some("1 x NVIDIA GeForce RTX 4090 (24 GB)"));
    crate::aiprofile_remote::note_failure(&cache, "gpu-stale", "gpu-stale", "no answer", 1100);
    assert_eq!(gpu_summary("gpu-stale").as_deref(), Some("1 x NVIDIA GeForce RTX 4090 (24 GB) (last known)"));
    // a good answer clears the mark
    crate::aiprofile_remote::note_nodes(&cache, "gpu-stale", "gpu-stale", &gpu_nodes(1), 1200);
    assert_eq!(gpu_summary("gpu-stale").as_deref(), Some("1 x NVIDIA GeForce RTX 4090 (24 GB)"));
    // garbage is a failure, never a panic
    crate::aiprofile_remote::note_nodes(&cache, "gpu-stale", "gpu-stale", "<html>", 1300);
    assert!(gpu_summary("gpu-stale").unwrap().ends_with("(last known)"));
}

#[test]
fn the_clusters_page_status_reads_the_same_answer_and_carries_the_badge() {
    let run = Arc::new(FakeRunner::new(&["kubectl"]));
    run.answer("gpu-page.yaml", &gpu_nodes(4));
    let dynrun: Arc<dyn Runner> = run.clone();
    let st = probe(&dynrun, &remote_entry("gpu-page"));
    assert_eq!(st.dot, Dot::Running);
    assert_eq!(st.gpu.as_deref(), Some("4 x NVIDIA GeForce RTX 4090 (24 GB each)"));
    assert_eq!(gpu_summary("gpu-page"), st.gpu);
}
