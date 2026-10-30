// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup and the clusters: the switcher in the popup of the items that deploy to a cluster (and nowhere else), the confirmation
//! texts that name the cluster, what Setup does when another cluster is active, and the AI profile's two lines on the tiles page
//! (the machine facts, the hidden-tools sentence with the "Show all AI tools" link). Fakes only.
use super::*;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_agent::clusters::{Kind, Registry};
use iyakki_sec::policy::Role;

fn facts_with(present: &[Item]) -> Facts {
    let mut f = Facts::default();
    for it in Item::ALL {
        f.presence.push((it, if present.contains(&it) { Presence::Installed(String::new()) } else { Presence::Missing }));
    }
    f
}

fn rig(tag: &str, role: Role, machine: Machine, present: &[Item], extra_cluster: bool) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    if extra_cluster {
        let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
        let mut r = Registry::load(&dir, &user);
        r.add_remote("gpu-box", Kind::Remote, "/fake/gpu.yaml", "", "").unwrap();
        r.save(&dir, &user).unwrap();
    }
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(machine);
    a.setup.set_facts(facts_with(present));
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: cx.services.dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&cx);
    (a, cx)
}

fn draw(a: &mut StoreApp, cx: &AppCx, w: u32, h: u32) -> (Gfx, R) {
    let s = crate::shell::scale_for(w, h);
    let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, area, s, cx, 3);
    (g, area)
}

fn switch_hits(g: &Gfx) -> Vec<(String, R)> {
    g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (SWITCH..SWITCH + 100).contains(&n))).map(|h| (h.label.clone(), h.rect)).collect()
}

fn audit(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default().replace("\\\"", "\"")
}

const CLUSTER_ITEMS: [Item; 4] = [Item::Registry, Item::Lemonade, Item::DuckDb, Item::Elasticsearch];

#[test]
fn with_one_cluster_no_popup_shows_a_switcher_and_the_deploy_text_names_this_machine() {
    let (mut a, cx) = rig("ssc-one", Role::Admin, Machine { ram_mb: 16_000, ..Default::default() }, &[Item::K3s], false);
    for item in CLUSTER_ITEMS {
        a.setup.open_popup(item);
        let (g, _) = draw(&mut a, &cx, 1280, 800);
        assert!(switch_hits(&g).is_empty(), "{item:?}: one cluster, nothing of the switcher");
        a.setup.popup = None;
    }
    assert_eq!(a.setup.deploy_text(Item::Lemonade).unwrap(), "Deploy Lemonade to cluster \"This machine\" (local)");
    assert_eq!(a.setup.deploy_text(Item::Node), None);
    assert_eq!(a.setup.deploy_text(Item::Ollama), None, "Ollama installs on this machine, not into a cluster");
}

#[test]
fn the_switcher_is_in_the_popup_of_the_cluster_items_only_at_every_size_and_stays_inside_it() {
    let m = Machine { ram_mb: 16_000, ..Default::default() };
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let (mut a, cx) = rig(&format!("ssc-{w}"), Role::Admin, m.clone(), &[Item::K3s], true);
        let s = crate::shell::scale_for(w, h);
        for item in Item::ALL {
            if item.fw_def().is_some() {
                continue;
            }
            a.setup.open_popup(item);
            let (g, area) = draw(&mut a, &cx, w, h);
            let sw = switch_hits(&g);
            if !deploys_to_cluster(item) {
                assert!(sw.is_empty(), "{w}x{h} {item:?}: no switcher for an item that does not deploy to a cluster");
            } else if sw.len() == 1 {
                let b = sw[0].1;
                assert!(b.x >= area.x - 0.5 && b.y >= area.y - 0.5 && b.x + b.w <= area.x + area.w + 0.5 && b.y + b.h <= area.y + area.h + 0.5, "{w}x{h} {item:?}: {b:?} outside {area:?}");
                // not on top of the popup's own controls
                for o in g.hits.iter().filter(|o| !matches!(o.ev, Event::Tab(n) if (SWITCH..SWITCH + 100).contains(&n)) && o.label != "backdrop" && o.label != "popup") {
                    let c = o.rect;
                    let over = b.x < c.x + c.w - 1.0 && c.x < b.x + b.w - 1.0 && b.y < c.y + c.h - 1.0 && c.y < b.y + b.h - 1.0;
                    assert!(!over, "{w}x{h} {item:?}: the switcher {b:?} covers `{}` {c:?}", o.label);
                }
            } else {
                // a popup with no room left for the box draws none, never a broken one
                assert!(sw.is_empty(), "{w}x{h} {item:?}: {sw:?} (scale {s})");
            }
            // open: the list stays inside the screen and Manage clusters... is reachable
            if sw.len() == 1 {
                a.handle(&Event::Tab(SWITCH), &cx);
                let (g, area) = draw(&mut a, &cx, w, h);
                assert!(g.hits.iter().any(|x| x.label == "Manage clusters..."), "{w}x{h} {item:?}: {:?} open={}", g.hits.iter().map(|x| (x.label.clone(), x.rect.x as i32, x.rect.y as i32, x.rect.w as i32, x.rect.h as i32)).collect::<Vec<_>>(), a.setup.host.as_ref().map(|h| h.sw.open).unwrap_or(false));
                for x in switch_hits(&g) {
                    let r = x.1;
                    assert!(r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.x + r.w <= area.x + area.w + 0.5 && r.y + r.h <= area.y + area.h + 0.5, "{w}x{h} {item:?}: `{}` {r:?} outside", x.0);
                }
                a.handle(&Event::Key(Key::Esc), &cx);
            }
            a.setup.popup = None;
        }
    }
}

#[test]
fn choosing_a_cluster_in_the_popup_switches_it_names_it_and_setup_installs_only_into_this_machines_cluster() {
    let (mut a, cx) = rig("ssc-switch", Role::Admin, Machine { ram_mb: 16_000, ..Default::default() }, &[Item::K3s], true);
    a.setup.open_popup(Item::Lemonade);
    draw(&mut a, &cx, 1280, 800);
    a.handle(&Event::Tab(SWITCH), &cx);
    let (g, _) = draw(&mut a, &cx, 1280, 800);
    let row = g.hits.iter().find(|h| h.label.starts_with("gpu-box (")).expect("a row for gpu-box").ev.clone();
    a.handle(&row, &cx);
    let reg = Registry::load(cx.services.dir(), &cx.session.user);
    assert_eq!(reg.active_entry().id, "gpu-box", "the account's active cluster changed");
    assert!(audit(&cx).contains("cluster.switch") && audit(&cx).contains("cluster \"gpu-box\" (remote)"));
    let t = a.setup.deploy_text(Item::Lemonade).unwrap();
    assert!(t.starts_with("Deploy Lemonade to cluster \"gpu-box\" (remote)"), "{t}");
    // Setup's installers run in this machine's own cluster: another active cluster is refused, with the way out, and nothing starts
    let before = a.setup.jobs.len();
    a.setup.press(Item::Lemonade, &cx);
    let note = a.setup.note.clone().unwrap();
    assert!(!note.1 && note.0.contains("cluster \"gpu-box\" (remote)") && note.0.contains("\"This machine\""), "{note:?}");
    assert_eq!(a.setup.jobs.len(), before);
    assert!(!audit(&cx).contains("quickinstall.start"));
    // Indic-TTS goes through Platform services, which follow the active cluster: the hand-off is audited with the cluster
    a.setup.popup = None;
}

#[test]
fn manage_clusters_in_the_popup_asks_for_settings_clusters_and_the_review_names_the_cluster() {
    let (mut a, cx) = rig("ssc-manage", Role::Admin, Machine { ram_mb: 16_000, ..Default::default() }, &[], true);
    a.setup.open_popup(Item::DuckDb);
    draw(&mut a, &cx, 1280, 800);
    a.handle(&Event::Tab(SWITCH), &cx);
    let (g, _) = draw(&mut a, &cx, 1280, 800);
    let m = g.hits.iter().find(|h| h.label == "Manage clusters...").unwrap().ev.clone();
    a.handle(&m, &cx);
    assert_eq!(a.take_request(), Some(("settings".to_string(), "tab:clusters".to_string())));
    // the review of several items says where the cluster items go
    a.setup.popup = None;
    let c = a.setup.plan_for(vec![Item::DuckDb, Item::Node], "Install 2 items?".into());
    let lines = Setup::confirm_lines(&c);
    assert!(lines.iter().any(|l| l == "Deploy the cluster items to cluster \"This machine\" (local)"), "{lines:?}");
    let c2 = a.setup.plan_for(vec![Item::Node], "Install Node?".into());
    assert!(!Setup::confirm_lines(&c2).iter().any(|l| l.contains("cluster")));
}

fn cpu_only_machine() -> Machine {
    let r = std::env::temp_dir().join(format!("iyakki-ssc-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(r.join("proc")).unwrap();
    std::fs::create_dir_all(r.join("sys/class/dmi/id")).unwrap();
    std::fs::write(r.join("proc/meminfo"), "MemTotal: 8388608 kB\n").unwrap();
    std::fs::write(r.join("sys/class/dmi/id/product_name"), "PC").unwrap();
    let p = iyakki_agent::aiprofile::detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    Machine { ram_mb: 8_000, ai: Some(p), ..Default::default() }
}

#[test]
fn the_tiles_page_draws_the_machine_facts_and_the_hidden_tools_sentence_with_the_link_for_an_administrator() {
    let (mut a, cx) = rig("ssc-ai", Role::Admin, cpu_only_machine(), &[], false);
    let (facts, hidden) = a.setup.ai_text();
    assert_eq!(facts.unwrap(), "This machine: No GPU: CPU only, 8 GB RAM; local models would run on the CPU and be slow");
    let hidden = hidden.unwrap();
    assert!(hidden.starts_with("2 AI tools are hidden because this machine has no GPU with 6 GB or more") && hidden.ends_with('.') && !hidden.contains("administrator"), "{hidden}");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let (g, area) = draw(&mut a, &cx, w, h);
        let link = g.hits.iter().find(|x| x.label == "Show all AI tools").unwrap_or_else(|| panic!("{w}x{h}: no link"));
        let r = link.rect;
        assert!(r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.x + r.w <= area.x + area.w + 0.5 && r.y + r.h <= area.y + area.h + 0.5, "{w}x{h}: {r:?}");
        // it does not sit on the search box or the filter chips
        assert!(!g.hits.iter().any(|o| o.label != "Show all AI tools" && o.rect.y < r.y + r.h - 1.0 && r.y < o.rect.y + o.rect.h - 1.0 && o.rect.x < r.x + r.w - 1.0 && r.x < o.rect.x + o.rect.w - 1.0 && !o.label.is_empty() && o.label != "field"), "{w}x{h}: the link overlaps another control");
    }
    // the machine is not read for facts when there is no profile: no lines, no link
    let (mut n, cx2) = rig("ssc-ai-none", Role::Admin, Machine { ram_mb: 16_000, ..Default::default() }, &[], false);
    assert_eq!(n.setup.ai_text(), (None, None));
    let (g, _) = draw(&mut n, &cx2, 1280, 800);
    assert!(!g.hits.iter().any(|x| x.label == "Show all AI tools"));
    // clicking the link is the audited administrator switch
    let (g, _) = draw(&mut a, &cx, 1280, 800);
    let ev = g.hits.iter().find(|x| x.label == "Show all AI tools").unwrap().ev.clone();
    a.handle(&ev, &cx);
    assert!(cx.services.show_all_ai());
    assert!(audit(&cx).contains("settings.show_all_ai") && audit(&cx).contains("\"on\"") || audit(&cx).contains("settings.show_all_ai"));
    assert!(a.setup.machine.show_all_ai && a.setup.ai_text().1.is_none(), "the sentence goes once everything is shown");
    let (g, _) = draw(&mut a, &cx, 1280, 800);
    assert!(!g.hits.iter().any(|x| x.label == "Show all AI tools"));
}

#[test]
fn a_user_sees_the_sentence_without_the_link_and_the_link_is_refused_for_them() {
    let (mut a, cx) = rig("ssc-ai-user", Role::User, cpu_only_machine(), &[], false);
    let (g, _) = draw(&mut a, &cx, 1280, 800);
    assert!(!g.hits.iter().any(|x| x.label == "Show all AI tools"), "no link for someone who cannot use it");
    a.handle(&Event::Tab(SHOW_ALL_AI), &cx);
    assert!(!cx.services.show_all_ai());
    assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
}
