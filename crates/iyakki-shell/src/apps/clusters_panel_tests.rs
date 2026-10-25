// #[cfg(test)] -- test code, included by clusters_panel.rs under #[cfg(test)].
//! Settings -> Clusters: list, active switch, add/rename/test/start/stop/remove with confirmations that name the cluster, roles,
//! vault entries kept unless asked, drawing at every size with hit regions inside the page. Fakes only: a fake runner, temp folders.

use super::*;
use iyakki_agent::clis::CliManager;
use iyakki_agent::k3sdev::K3sDev;
use iyakki_agent::wsexec::FakeRunner;
use std::time::{Duration, Instant};

struct Rig {
    p: ClustersPanel,
    run: Arc<FakeRunner>,
    dir: PathBuf,
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-clp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig(tag: &str, role: Role) -> Rig {
    let dir = tmp(tag);
    let run = Arc::new(FakeRunner::new(&[]));
    let clis = Arc::new(CliManager::new(&dir.join("cli")));
    let data = dir.join("data");
    let primary = K3sDev::new(&data, clis.clone(), run.clone());
    let fleet = Arc::new(Fleet::new(primary, &data, clis, run.clone()));
    fleet.assume_memory(Some(8 << 30));
    let p = ClustersPanel::new(fleet, dir.clone(), "alice", role).quiet();
    Rig { p, run, dir }
}

impl Rig {
    fn key(&mut self, c: char) {
        self.p.handle(&Event::Key(Key::Char(c)));
    }
    fn press(&mut self, k: Key) {
        self.p.handle(&Event::Key(k));
    }
    fn say(&self) -> String {
        self.p.message().map(|m| m.0.clone()).unwrap_or_default()
    }
    fn add_remote(&mut self, name: &str, kc: &str) {
        self.key('a');
        let Mode::Form(_, f) = &mut self.p.mode else { panic!("no form") };
        f.set_value(0, name);
        f.set_value(2, kc);
        self.p.handle(&Event::Key(Key::Ctrl('s')));
        assert!(matches!(self.p.mode, Mode::List), "{:?}", self.say());
    }
    fn select(&mut self, id: &str) {
        let i = self.p.rows().iter().position(|c| c.id == id).unwrap();
        self.p.handle(&Event::Row(i));
    }
    fn wait(&mut self, what: &str, f: impl Fn(&Rig) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && !f(self) {
            self.p.handle(&Event::Tick);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(f(self), "{what}: {}", self.say());
    }
}

#[test]
fn it_opens_with_the_local_cluster_active_and_saves_the_migrated_registry() {
    let r = rig("open", Role::Admin);
    assert_eq!(r.p.rows().len(), 1);
    assert_eq!(r.p.registry().active, "local");
    assert!(Registry::saved(&r.dir, "alice"));
    assert!(r.p.is_open());
}

#[test]
fn adding_a_remote_cluster_checks_the_form_names_it_and_tests_it_read_only() {
    let mut r = rig("addremote", Role::Admin);
    r.run.answer("get nodes -o json", r#"{"items":[{"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.30.0"}}}]}"#);
    // a bad name stays in the form with the reason
    r.key('a');
    {
        let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
        f.set_value(0, "bad/name");
        f.set_value(2, "/k.yaml");
    }
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    let Mode::Form(_, f) = &r.p.mode else { panic!("the form stays open") };
    assert!(f.error.as_deref().unwrap_or("").contains("letters, digits"));
    r.press(Key::Esc);
    r.add_remote("gpu-box", "uvault:gpu-kc");
    assert!(r.say().contains("cluster \"gpu-box\" (remote)"), "{}", r.say());
    assert_eq!(r.p.rows().len(), 2);
    r.wait("probe", |r| r.run.lines().iter().any(|l| l.contains("get nodes")));
    let line = r.run.lines().into_iter().find(|l| l.contains("get nodes")).unwrap();
    assert!(line.contains("--kubeconfig uvault:gpu-kc") && !line.contains("apply") && !line.contains("delete"), "{line}");
    r.wait("status", |r| r.p.status_of(&r.p.reg.get("gpu-box").unwrap().clone()).dot == Dot::Running);
    let st = r.p.status_of(&r.p.reg.get("gpu-box").unwrap().clone());
    assert_eq!((st.nodes, st.ready, st.version.as_str()), (1, 1, "v1.30.0"));
    // saved: a new panel sees it
    assert_eq!(Registry::load(&r.dir, "alice").clusters.len(), 2);
}

#[test]
fn enter_makes_the_selected_cluster_active_and_says_which() {
    let mut r = rig("active", Role::Admin);
    r.add_remote("gpu-box", "/k.yaml");
    r.select("gpu-box");
    r.press(Key::Enter);
    assert!(r.say().contains("Active cluster: cluster \"gpu-box\" (remote)") && r.say().contains("Nothing was moved or deleted"), "{}", r.say());
    assert_eq!(Registry::load(&r.dir, "alice").active, "gpu-box");
    assert_eq!(iyakki_agent::devcluster::Choice::load(&r.dir, "alice").unwrap().kubeconfig, "/k.yaml", "the old remembered choice follows");
}

#[test]
fn a_workspace_override_needs_a_workspace_and_can_be_cleared() {
    let mut r = rig("ws", Role::Admin);
    r.add_remote("gpu-box", "/k.yaml");
    r.select("gpu-box");
    r.key('w');
    assert!(r.say().contains("Open this page from a workspace"), "{}", r.say());
    r.p.workspace = Some("web".into());
    r.key('w');
    assert!(r.say().contains("Workspace web uses cluster \"gpu-box\""), "{}", r.say());
    assert_eq!(Registry::load(&r.dir, "alice").effective("web").id, "gpu-box");
    assert_eq!(Registry::load(&r.dir, "alice").effective("api").id, "local");
    r.key('W');
    assert_eq!(Registry::load(&r.dir, "alice").effective("web").id, "local");
}

#[test]
fn adding_a_local_cluster_warns_about_the_cost_and_respects_the_maximum() {
    let mut r = rig("addlocal", Role::Operator);
    r.key('l');
    {
        let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
        assert!(f.fields[0].hint.contains("2 GB") && f.fields[0].hint.contains("CPU"));
        f.set_value(0, "Second");
    }
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    assert!(r.say().contains("cluster \"Second\" (local)") && r.say().contains("not set up yet"), "{}", r.say());
    // the second extra is over the default maximum of two
    r.key('l');
    {
        let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
        f.set_value(0, "Third");
    }
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    let Mode::Form(_, f) = &r.p.mode else { panic!("refused, form stays") };
    assert!(f.error.as_deref().unwrap_or("").contains("2 local clusters"));
    r.press(Key::Esc);
    r.key('A');
    assert!(r.p.registry().advanced && r.say().contains("up to 4"), "{}", r.say());
}

#[test]
fn start_and_stop_ask_with_the_cluster_name_and_remote_clusters_cannot_be_started() {
    let mut r = rig("startstop", Role::Admin);
    r.add_remote("gpu-box", "/k.yaml");
    r.select("gpu-box");
    r.key('b');
    assert!(r.say().contains("not on this machine") && matches!(r.p.mode, Mode::List), "{}", r.say());
    r.select("local");
    r.key('b');
    let Mode::Confirm(Pending::Start(id), text) = &r.p.mode else { panic!("asks first") };
    assert_eq!(id, "local");
    assert!(text.starts_with("Start cluster \"This machine\" (local)?") && text.contains("2 GB"), "{text}");
    r.press(Key::Esc);
    assert!(matches!(r.p.mode, Mode::List));
    r.key('o');
    let Mode::Confirm(Pending::Stop(_), text) = &r.p.mode else { panic!() };
    assert!(text.starts_with("Stop cluster \"This machine\" (local)?"), "{text}");
    // confirming runs the real supervisor code: nothing is installed, so it answers why
    r.key('y');
    r.wait("start refused", |r| r.p.busy.is_none() && r.say().contains("no cluster is installed") || r.say().contains("not running") || r.say().contains("nothing"));
}

#[test]
fn removing_a_remote_cluster_only_forgets_the_profile_and_the_vault_entry_goes_only_when_asked() {
    let mut r = rig("forget", Role::Admin);
    iyakki_agent::kubeprofiles::vault_add(&r.dir, "alice", "lab", "uvault:lab-kc").unwrap();
    r.p.reg.add_remote("lab", Kind::Remote, "uvault:lab-kc", "", "").unwrap();
    r.p.reg.save(&r.dir, "alice").unwrap();
    let deleted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let d2 = deleted.clone();
    let fresh = ClustersPanel::new(r.p.fleet.clone(), r.dir.clone(), "alice", Role::Admin).quiet();
    r.p = std::mem::replace(&mut r.p, fresh).with_vault_delete(Arc::new(move |n: &str| {
        d2.lock().unwrap().push(n.to_string());
        Ok(())
    }));
    r.select("lab");
    r.key('x');
    let Mode::Confirm(Pending::ForgetRemote(_), text) = &r.p.mode else { panic!() };
    assert!(text.starts_with("Forget cluster \"lab\" (remote)?") && text.contains("Only the profile is forgotten"), "{text}");
    r.key('y');
    assert!(r.say().contains("not touched") && deleted.lock().unwrap().is_empty(), "{}", r.say());
    assert_eq!(r.p.rows().len(), 1);
    // again, asking for the vault entry too
    iyakki_agent::kubeprofiles::vault_add(&r.dir, "alice", "lab2", "uvault:lab2-kc").unwrap();
    r.p.reg.add_remote("lab2", Kind::Remote, "uvault:lab2-kc", "", "").unwrap();
    r.p.reg.save(&r.dir, "alice").unwrap();
    r.select("lab2");
    r.key('x');
    r.key('D');
    assert!(r.say().contains("vault entry was deleted"), "{}", r.say());
    assert_eq!(*deleted.lock().unwrap(), ["uvault:lab2-kc"]);
}

#[test]
fn removing_a_local_cluster_asks_and_keeps_its_data_folder() {
    let mut r = rig("removelocal", Role::Admin);
    r.key('l');
    {
        let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
        f.set_value(0, "Second");
    }
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    let folder = r.dir.join("data/k3s-dev-second");
    std::fs::create_dir_all(folder.join("data")).unwrap();
    std::fs::write(folder.join("data/keep.txt"), "x").unwrap();
    r.select("second");
    r.key('x');
    let Mode::Confirm(Pending::RemoveLocal(_), text) = &r.p.mode else { panic!() };
    assert!(text.starts_with("Remove cluster \"Second\" (local)?") && text.contains("keep its data folder") && text.contains("delete its data"), "{text}");
    r.key('y');
    r.wait("removed", |r| r.p.rows().len() == 1 && r.p.busy.is_none());
    assert!(folder.join("data/keep.txt").is_file(), "the data was kept");
    assert_eq!(Registry::load(&r.dir, "alice").clusters.len(), 1);
    // the built-in one cannot be removed from the list
    r.select("local");
    r.key('x');
    assert!(r.say().contains("cannot be removed") && matches!(r.p.mode, Mode::List), "{}", r.say());
}

#[test]
fn users_switch_but_do_not_manage_and_guests_see_nothing() {
    let mut r = rig("roles", Role::User);
    for c in ['a', 'l', 'b', 'o', 's', 'x', 'u', 'v', 'A', 'r'] {
        r.key(c);
        assert!(matches!(r.p.mode, Mode::List), "{c} must not open anything for a user");
    }
    assert!(r.say().contains("administrator or operator"), "{}", r.say());
    assert!(!r.p.buttons().iter().any(|b| matches!(b.2, 'a' | 'l' | 'x' | 'b')), "{:?}", r.p.buttons().iter().map(|b| b.0.clone()).collect::<Vec<_>>());
    assert!(r.p.buttons().iter().any(|b| b.2 == 'd'), "switching stays");
    let mut g = rig("roles-guest", Role::Guest);
    assert!(g.p.rows().is_empty());
    g.press(Key::Enter);
    assert!(g.say().contains("guests"), "{}", g.say());
}

#[test]
fn extra_local_cluster_autostart_toggles_and_the_primary_refuses() {
    let mut r = rig("auto", Role::Admin);
    r.key('l');
    {
        let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
        f.set_value(0, "Second");
    }
    r.p.handle(&Event::Key(Key::Ctrl('s')));
    r.select("second");
    r.key('u');
    assert!(r.say().contains("starts with IYAKKI") && Registry::load(&r.dir, "alice").get("second").unwrap().autostart, "{}", r.say());
    r.key('u');
    assert!(r.say().contains("no longer"));
    r.select("local");
    r.key('u');
    assert!(r.say().contains("own Autostart setting"), "{}", r.say());
}

fn assert_inside(g: &Gfx, a: R, what: &str) {
    assert!(!g.hits.is_empty(), "{what}: nothing is clickable");
    for h in &g.hits {
        let r = h.rect;
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: `{}` {:?} outside {:?}", h.label, r, a);
    }
}

#[test]
fn it_draws_at_every_size_in_every_mode_with_every_hit_region_inside_the_page() {
    for (w, h, s) in [(360u32, 740u32, 1.0f32), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0)] {
        let area = R::new(0.0, 0.0, w as f32, h as f32);
        let mut r = rig(&format!("draw{w}"), Role::Admin);
        r.add_remote("gpu-box", "/k.yaml");
        r.key('l');
        {
            let Mode::Form(_, f) = &mut r.p.mode else { panic!() };
            f.set_value(0, "Second");
        }
        r.p.handle(&Event::Key(Key::Ctrl('s')));
        let mut g = Gfx::new(w, h);
        r.p.draw(&mut g, area, s);
        assert_inside(&g, area, &format!("{w}x{h} list"));
        assert!(g.hits.iter().any(|x| x.label.contains("gpu-box")) && g.hits.iter().any(|x| x.label.contains("Second")));
        assert!(g.rgba().iter().any(|b| *b != 0));
        for (k, label) in [('a', "add remote"), ('l', "add local"), ('r', "rename")] {
            r.key(k);
            let mut g = Gfx::new(w, h);
            r.p.draw(&mut g, area, s);
            assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h} {label}");
            r.press(Key::Esc);
        }
        for k in ['b', 'o', 'x'] {
            r.select("second");
            r.key(k);
            let mut g = Gfx::new(w, h);
            r.p.draw(&mut g, area, s);
            assert_inside(&g, area, &format!("{w}x{h} confirm {k}"));
            r.press(Key::Esc);
        }
    }
}

#[test]
fn a_tiny_area_never_panics() {
    for (w, h) in [(1u32, 1u32), (60, 40), (200, 150)] {
        let mut r = rig(&format!("tiny{w}"), Role::Admin);
        let mut g = Gfx::new(w, h);
        r.p.draw(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0);
        r.key('a');
        r.p.draw(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0);
    }
}

#[test]
fn the_page_shows_the_gpu_badge_of_a_remote_cluster_from_the_ai_profile_cache() {
    let mut r = rig("gpubadge", Role::Admin);
    r.add_remote("gpu-badge-page", "/k.yaml");
    let area = R::new(0.0, 0.0, 1280.0, 720.0);
    let mut g0 = Gfx::new(1280, 720);
    r.p.draw(&mut g0, area, 1.0);
    let json = r#"{"items":[{"metadata":{"name":"g1","labels":{"nvidia.com/gpu.product":"NVIDIA-GeForce-RTX-4090","nvidia.com/gpu.memory":"24576"}},"status":{"allocatable":{"nvidia.com/gpu":"2"}}}]}"#;
    iyakki_agent::aiprofile_remote::note_nodes(&iyakki_agent::aiprofile_remote::clusters_cache(), "gpu-badge-page", "gpu-badge-page", json, 100);
    let e = r.p.registry().get("gpu-badge-page").cloned().unwrap();
    assert_eq!(r.p.status_of(&e).gpu.as_deref(), Some("2 x NVIDIA GeForce RTX 4090 (24 GB each)"));
    let mut g1 = Gfx::new(1280, 720);
    r.p.draw(&mut g1, area, 1.0);
    assert_ne!(g0.rgba(), g1.rgba(), "the badge is a chip on the row");
    iyakki_agent::aiprofile_remote::note_failure(&iyakki_agent::aiprofile_remote::clusters_cache(), "gpu-badge-page", "gpu-badge-page", "down", 200);
    assert!(r.p.status_of(&e).gpu.unwrap().ends_with("(last known)"));
}
