// #[cfg(test)] -- test code, included by cluster_host.rs under #[cfg(test)].
//! The shared host of the cluster switcher: rows with the GPU badge (fresh, last known, none), the role-checked and audited switch, the
//! per-workspace override, vault profiles added later, and the box at every size and inside panes. Fakes only: the GPU cache is filled with
//! fake node JSON and nothing here runs kubectl.
use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::gfx::rgb;
use crate::ui::Key;
use iyakki_agent::clusters::Kind;
use iyakki_sec::policy::Policy;

const BASE: usize = 7_700_000;

fn nodes(count: u32) -> String {
    format!(r#"{{"items":[{{"metadata":{{"name":"g1","labels":{{"nvidia.com/gpu.product":"NVIDIA-GeForce-RTX-4090","nvidia.com/gpu.memory":"24576"}}}},"status":{{"allocatable":{{"nvidia.com/gpu":"{count}"}}}}}}]}}"#)
}

fn cx_with(tag: &str, remotes: &[&str]) -> AppCx {
    let cx = cx_with_admin_store(tag);
    let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
    let mut r = Registry::load(&dir, &user);
    for n in remotes {
        r.add_remote(n, Kind::Remote, &format!("/fake/{n}.yaml"), "", "").unwrap();
    }
    r.save(&dir, &user).unwrap();
    cx
}

#[test]
fn one_cluster_means_no_box_no_hit_and_the_phrase_names_this_machine() {
    let cx = cx_with_admin_store("host-one");
    let mut h = ClusterHost::new(BASE, &cx, "Shop");
    assert!(!h.visible());
    let mut g = Gfx::new(800, 480);
    assert!(h.draw(&mut g, 10.0, 10.0, 300.0, 1.0, [1, 2, 3, 255]).is_none() && g.hits.is_empty());
    assert_eq!(h.width(&mut g, 1.0), 0.0);
    assert_eq!(h.phrase(), "cluster \"This machine\" (local)");
    assert_eq!(h.handle(&Event::Tab(BASE), &|_| None), HostOut::None);
}

#[test]
fn the_gpu_badge_is_fresh_last_known_or_absent_in_the_rows_and_drawn_in_the_list() {
    let cx = cx_with("host-gpu", &["gpu-host-fresh", "gpu-host-old", "gpu-host-none"]);
    let cache = iyakki_agent::aiprofile_remote::clusters_cache();
    iyakki_agent::aiprofile_remote::note_nodes(&cache, "gpu-host-fresh", "gpu-host-fresh", &nodes(2), 100);
    iyakki_agent::aiprofile_remote::note_nodes(&cache, "gpu-host-old", "gpu-host-old", &nodes(1), 100);
    iyakki_agent::aiprofile_remote::note_failure(&cache, "gpu-host-old", "gpu-host-old", "no answer", 200);
    iyakki_agent::aiprofile_remote::note_nodes(&cache, "gpu-host-none", "gpu-host-none", r#"{"items":[{"metadata":{"name":"a"},"status":{"allocatable":{"cpu":"4"}}}]}"#, 100);
    let mut h = ClusterHost::new(BASE, &cx, "");
    let gpu = |h: &ClusterHost, id: &str| h.rows().iter().find(|r| r.id == id).unwrap().gpu.clone();
    assert_eq!(gpu(&h, "gpu-host-fresh").as_deref(), Some("2 x NVIDIA GeForce RTX 4090 (24 GB each)"));
    assert_eq!(gpu(&h, "gpu-host-old").as_deref(), Some("1 x NVIDIA GeForce RTX 4090 (24 GB) (last known)"));
    assert_eq!(gpu(&h, "gpu-host-none"), None);
    assert_eq!(gpu(&h, "local"), None, "a cluster on this machine has the machine's own GPUs: no badge");
    // the dots: answered, answered once and stale, answered with no GPU
    let dots: Vec<_> = h.rows().iter().map(|r| (r.id.as_str(), r.dot)).collect();
    assert!(dots.contains(&("gpu-host-fresh", Dot::Running)) && dots.contains(&("gpu-host-old", Dot::Stale)) && dots.contains(&("gpu-host-none", Dot::Running)), "{dots:?}");
    // the open list draws a chip for the badge: the same list without the badge looks different
    h.sw.open = true;
    let mut g1 = Gfx::new(1280, 720);
    h.draw(&mut g1, 10.0, 10.0, 300.0, 1.0, [64, 196, 232, 255]);
    h.draw_popup(&mut g1, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, [64, 196, 232, 255]);
    let mut rows: Vec<_> = h.rows().to_vec();
    rows.iter_mut().for_each(|r| r.gpu = None);
    let mut sw = crate::apps::cluster_switch::ClusterSwitch::new(BASE);
    sw.open = true;
    let mut g2 = Gfx::new(1280, 720);
    sw.draw(&mut g2, 10.0, 10.0, 300.0, 1.0, &rows, [64, 196, 232, 255]);
    sw.draw_popup(&mut g2, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &rows, [64, 196, 232, 255]);
    assert_ne!(g1.rgba(), g2.rgba(), "the GPU badge is drawn");
}

#[test]
fn switching_is_role_checked_audited_and_a_workspace_with_its_own_cluster_switches_its_own() {
    let cx = cx_with("host-switch", &["lab"]);
    let mut h = ClusterHost::new(BASE, &cx, "Shop");
    assert!(h.visible());
    h.handle(&Event::Tab(BASE), &|_| None);
    assert!(h.sw.open);
    let lab = h.rows().iter().position(|r| r.id == "lab").unwrap();
    let out = h.handle(&Event::Tab(BASE + 10 + lab), &|_| None);
    assert!(matches!(&out, HostOut::Switched(e) if e.id == "lab"), "{out:?}");
    assert_eq!(h.effective().unwrap().id, "lab");
    assert_eq!(Registry::load(cx.services.dir(), &cx.session.user).active_entry().id, "lab");
    let a = std::fs::read_to_string(cx.audit.path()).unwrap().replace("\\\"", "\"");
    assert!(a.contains("cluster.switch") && a.contains("cluster \"lab\" (remote)"), "{a}");
    // keys: Down, Enter on the first row (local) goes back
    h.handle(&Event::Tab(BASE), &|_| None);
    assert!(h.owns_keys());
    h.handle(&Event::Key(Key::Up), &|_| None);
    let out = h.handle(&Event::Key(Key::Enter), &|_| None);
    assert!(matches!(out, HostOut::Switched(e) if e.id == "local"));
    // Manage
    h.handle(&Event::Tab(BASE), &|_| None);
    assert_eq!(h.handle(&Event::Tab(BASE + 2), &|_| None), HostOut::Manage);
    // a workspace with its own cluster: the switch sets that one and leaves the account's alone
    iyakki_agent::clusters::switch_workspace(cx.services.dir(), &cx.session.user, cx.session.role, "Shop", Some("local")).unwrap();
    let mut w = ClusterHost::new(BASE, &cx, "Shop");
    w.handle(&Event::Tab(BASE), &|_| None);
    let lab = w.rows().iter().position(|r| r.id == "lab").unwrap();
    w.handle(&Event::Tab(BASE + 10 + lab), &|_| None);
    let reg = Registry::load(cx.services.dir(), &cx.session.user);
    assert_eq!((reg.effective("Shop").id.as_str(), reg.active_entry().id.as_str()), ("lab", "local"));
    let a = std::fs::read_to_string(cx.audit.path()).unwrap();
    assert!(a.contains("(workspace Shop)"));
}

#[test]
fn a_guest_gets_no_rows_and_a_user_only_the_clusters_visible_to_users() {
    let mut cx = cx_with("host-roles", &["lab", "ops"]);
    let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
    let mut r = Registry::load(&dir, &user);
    r.set_visible_to_users("ops", false).unwrap();
    r.save(&dir, &user).unwrap();
    cx.session.role = Role::Guest;
    cx.session.policy = Policy::resolve(Role::Guest, &Default::default());
    assert!(!ClusterHost::new(BASE, &cx, "").visible());
    cx.session.role = Role::User;
    cx.session.policy = Policy::resolve(Role::User, &Default::default());
    let h = ClusterHost::new(BASE, &cx, "");
    assert_eq!(h.rows().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["local", "lab"]);
    // a user cannot switch to what they do not see
    let mut h = h;
    let id = "ops";
    assert!(iyakki_agent::clusters::switch(&dir, &dir, &user, Role::User, id).is_err());
    assert_eq!(h.handle(&Event::Tab(BASE + 10 + 5), &|_| None), HostOut::None);
}

#[test]
fn a_vault_profile_added_after_the_registry_was_saved_still_shows_up() {
    let cx = cx_with("host-vault", &["lab"]);
    let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
    iyakki_agent::kubeprofiles::vault_add(&dir, &user, "later", "uvault:later-kube").unwrap();
    let h = ClusterHost::new(BASE, &cx, "");
    assert!(h.rows().iter().any(|r| r.name == "later"), "{:?}", h.rows());
    // once, not on every read
    let h2 = ClusterHost::new(BASE, &cx, "");
    assert_eq!(h2.rows().iter().filter(|r| r.name == "later").count(), 1);
}

#[test]
fn the_box_fits_every_size_and_pane_and_the_list_stays_inside() {
    let cx = cx_with("host-geo", &["gpu-host-geo"]);
    iyakki_agent::aiprofile_remote::note_nodes(&iyakki_agent::aiprofile_remote::clusters_cache(), "gpu-host-geo", "gpu-host-geo", &nodes(8), 100);
    for (w, h, s) in [(360u32, 740u32, 1.0f32), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0)] {
        for pane in [R::new(0.0, 0.0, w as f32, h as f32), R::new(w as f32 * 0.3, h as f32 * 0.2, w as f32 * 0.6, h as f32 * 0.6)] {
            let mut host = ClusterHost::new(BASE, &cx, "");
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            let b = host.draw(&mut g, pane.x + 6.0, pane.y + 6.0, pane.w - 12.0, s, [64, 196, 232, 255]).expect("shown");
            assert!(b.x >= pane.x && b.x + b.w <= pane.x + pane.w + 0.5, "{w}x{h}: {b:?} in {pane:?}");
            host.sw.open = true;
            host.draw_popup(&mut g, pane, s, [64, 196, 232, 255]);
            for hh in &g.hits {
                let r = hh.rect;
                assert!(r.x >= pane.x - 0.5 && r.y >= pane.y - 0.5 && r.x + r.w <= pane.x + pane.w + 0.5 && r.y + r.h <= pane.y + pane.h + 0.5, "{w}x{h}: `{}` {r:?} outside {pane:?}", hh.label);
            }
            assert!(g.hits.iter().any(|x| x.label == "Manage clusters..."), "{w}x{h}");
        }
    }
}
