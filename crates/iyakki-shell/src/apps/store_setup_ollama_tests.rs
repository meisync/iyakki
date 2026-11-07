// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup -> Ollama: the host install is shown as such with "Move into the cluster" and "Start fresh in the cluster, keep host models
//! where they are"; the review card before anything starts; a move that is too large is not started; the choice reaches the job; the
//! pod's numbers on the popups; the memory warning in the review; everything at four sizes. Fakes only: no cluster, no network.
use super::*;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_agent::quickinstall::ollama_cluster as oc;
use iyakki_sec::policy::Role;

const FOUR: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];
const GB: u64 = 1 << 30;
const HOST: &str = "version 0.5.1 · Ollama runs on this machine's host";

fn rig(tag: &str, role: Role, ollama: Presence, host_bytes: u64) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    let mut f = Facts::default();
    for it in Item::ALL {
        let p = match it {
            Item::K3s => Presence::Installed("k3s v1.31.4 · running".into()),
            Item::Ollama => ollama.clone(),
            _ => Presence::Missing,
        };
        f.presence.push((it, p));
    }
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(Machine { ram_mb: 32_000, ..Default::default() });
    a.setup.set_facts(f);
    a.setup.ollama_host = Some(oc::HostInfo { models_dir: "/data/ollama/models".into(), models_bytes: host_bytes, free_bytes: Some(200 * GB) });
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: cx.services.dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&cx);
    (a, cx)
}

fn draw(a: &mut StoreApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let hh = 46.0 * s;
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, hh, w as f32, h as f32 - hh), s, cx, 3);
    g
}

fn hit(g: &Gfx, code: usize) -> Option<R> {
    g.hits.iter().find(|h| h.ev == Event::Tab(code)).map(|h| h.rect)
}

fn inside(r: R, w: u32, h: u32) -> bool {
    r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5
}

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

#[test]
fn a_host_ollama_is_shown_as_such_with_the_two_ways_into_the_cluster_for_administrators_only() {
    let (mut a, _cx) = rig("ol-host", Role::Admin, Presence::Installed(HOST.into()), 5 * GB);
    assert!(a.setup.ollama_on_host());
    a.setup.open_popup(Item::Ollama);
    let labels: Vec<String> = a.setup.popup_button_list(Item::Ollama).into_iter().filter(|(_, on)| *on).map(|(l, _)| l).collect();
    assert!(labels.contains(&"Move into the cluster".to_string()), "{labels:?}");
    assert!(labels.contains(&"Start fresh in the cluster, keep host models where they are".to_string()), "{labels:?}");
    assert!(a.setup.card(Item::Ollama).notes.iter().any(|(n, _)| n.contains("Ollama runs on this machine's host")));
    // in the cluster: neither button
    let (mut c, _cx) = rig("ol-cluster", Role::Admin, Presence::Installed("in the cluster: http://127.0.0.1:11436 · answering".into()), 0);
    assert!(!c.setup.ollama_on_host());
    c.setup.open_popup(Item::Ollama);
    assert!(!c.setup.popup_button_list(Item::Ollama).iter().any(|(l, _)| l.contains("cluster, keep") || l.starts_with("Move")));
    // not installed: neither
    let (mut m, _cx) = rig("ol-missing", Role::Admin, Presence::Missing, 0);
    m.setup.open_popup(Item::Ollama);
    assert!(!m.setup.popup_button_list(Item::Ollama).iter().any(|(l, _)| l.starts_with("Move")));
    // a person who is not an administrator is told to ask one
    let (mut u, cx) = rig("ol-user", Role::User, Presence::Installed(HOST.into()), 5 * GB);
    u.setup.open_popup(Item::Ollama);
    assert!(!u.setup.popup_button_list(Item::Ollama).iter().any(|(l, _)| l.starts_with("Move")));
    u.setup.ollama_review(oc::Mode::Move, &cx);
    assert!(u.setup.confirm.is_none() && u.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
}

#[test]
fn the_move_review_comes_first_and_the_confirm_leaves_the_choice_for_the_job() {
    let (mut a, cx) = rig("ol-move", Role::Admin, Presence::Installed(HOST.into()), 5 * GB);
    a.setup.open_popup(Item::Ollama);
    a.handle(&Event::Tab(OLLAMA_MOVE), &cx);
    let c = a.setup.confirm.as_ref().expect("the review opens");
    assert_eq!(c.title, oc::MOVE_TITLE);
    let text = c.review.join("\n");
    assert!(text.contains("5.0 GB") && text.contains("nothing is deleted from the host") && text.contains("only then is it stopped") && text.contains("MIT") && text.contains("There is room"), "{text}");
    assert_eq!(oc::take_mode(cx.services.dir()), None, "nothing is recorded before the person confirms");
    a.handle(&Event::Tab(CONFIRM_YES), &cx);
    assert!(a.setup.confirm.is_none());
    assert_eq!(oc::take_mode(cx.services.dir()), Some(oc::Mode::Move));
}

#[test]
fn start_fresh_says_the_host_models_stay_and_leaves_that_choice() {
    let (mut a, cx) = rig("ol-fresh", Role::Admin, Presence::Installed(HOST.into()), 40 * GB);
    a.setup.open_popup(Item::Ollama);
    a.handle(&Event::Tab(OLLAMA_FRESH), &cx);
    let c = a.setup.confirm.as_ref().expect("the review opens, also for a big folder");
    assert_eq!(c.title, oc::FRESH_TITLE);
    assert!(c.review.join("\n").contains("not touched and not copied"));
    a.handle(&Event::Tab(CONFIRM_YES), &cx);
    assert_eq!(oc::take_mode(cx.services.dir()), Some(oc::Mode::Fresh));
}

#[test]
fn a_move_that_is_too_large_shows_why_and_starts_nothing() {
    let (mut a, cx) = rig("ol-big", Role::Admin, Presence::Installed(HOST.into()), 40 * GB);
    a.setup.open_popup(Item::Ollama);
    a.handle(&Event::Tab(OLLAMA_MOVE), &cx);
    let c = a.setup.confirm.as_ref().expect("the card is shown so the reason is read");
    let text = c.review.join("\n");
    assert!(text.contains("too large") && text.contains("Start fresh in the cluster, keep host models where they are"), "{text}");
    a.handle(&Event::Tab(CONFIRM_YES), &cx);
    assert!(a.setup.confirm.is_some(), "the card stays");
    assert!(a.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("Start fresh")));
    assert_eq!(oc::take_mode(cx.services.dir()), None, "no choice is recorded and no job started");
}

#[test]
fn a_new_install_uses_the_cluster_and_no_host_path() {
    let (a, _cx) = rig("ol-new", Role::Admin, Presence::Missing, 0);
    let c = a.setup.card(Item::Ollama);
    assert_eq!(c.label, "Install");
    assert!(c.deps.is_none(), "k3s is there: nothing to install first");
    assert!(deploys_to_cluster(Item::Ollama));
    assert!(a.setup.deploy_text(Item::Ollama).is_some());
}

#[test]
fn the_popup_shows_the_pod_and_its_requests_limits_and_idle_memory() {
    let (a, cx) = rig("ol-numbers", Role::Admin, Presence::Missing, 0);
    let mut g = Gfx::new(1280, 720);
    let _ = &cx;
    let mut rows = |item: Item, setup: &Setup| -> Vec<(String, String)> {
        let card = setup.card(item);
        setup.popup_body(&mut g, &card, false, 600.0, 1.0).into_iter().filter_map(|l| if let view::Line::Row(k, v, _) = l { Some((k, v.join(" "))) } else { None }).collect()
    };
    let ch = rows(Item::ClickHouse, &a.setup);
    for k in ["Pod", "CPU", "Memory", "Idle (measured)"] {
        assert!(ch.iter().any(|(l, _)| l == k), "{k} in {ch:?}");
    }
    assert!(ch.iter().any(|(l, v)| l == "Pod" && v.contains("clickhouse") && v.contains("iyakki-data")));
    assert!(ch.iter().any(|(l, v)| l == "Idle (measured)" && v.contains("775")));
    for it in [Item::OpenSearch, Item::Elasticsearch, Item::GraphDb, Item::Prometheus, Item::Gitea, Item::Lemonade, Item::Ollama] {
        let r = rows(it, &a.setup);
        assert!(r.iter().any(|(l, _)| l == "CPU") && r.iter().any(|(l, _)| l == "Memory"), "{it:?}");
    }
    assert!(rows(Item::Node, &a.setup).iter().all(|(l, _)| l != "Pod"), "programs have no pod");
}

#[test]
fn the_review_warns_in_plain_words_when_the_selected_services_may_pass_sixty_percent_of_the_memory() {
    let (mut a, _cx) = rig("ol-warn", Role::Admin, Presence::Missing, 0);
    let c = a.setup.plan_for(vec![Item::Lemonade, Item::Ollama, Item::ClickHouse], "Install 3 items?".into());
    assert!(c.review.iter().any(|l| l.contains("Memory warning") && l.contains("60 %")), "{:?}", c.review);
    let few = a.setup.plan_for(vec![Item::ClickHouse], "Install 1 item?".into());
    assert!(few.review.is_empty(), "no warning for a light plan");
    // unknown memory: no warning
    a.setup.set_machine(Machine { ram_mb: 0, ..Default::default() });
    assert!(a.setup.plan_for(vec![Item::Lemonade, Item::Ollama], "x".into()).review.is_empty());
}

#[test]
fn every_screen_of_it_fits_at_four_sizes() {
    for (w, h) in FOUR {
        let (mut a, cx) = rig(&format!("ol-four-{w}"), Role::Admin, Presence::Installed(HOST.into()), 5 * GB);
        a.setup.open_popup(Item::Ollama);
        let g = draw(&mut a, &cx, w, h);
        let mv = hit(&g, OLLAMA_MOVE).unwrap_or_else(|| panic!("the Move button at {w}x{h}"));
        let fr = hit(&g, OLLAMA_FRESH).unwrap_or_else(|| panic!("the Start fresh button at {w}x{h}"));
        assert!(inside(mv, w, h) && inside(fr, w, h), "{w}x{h}: {mv:?} {fr:?}");
        assert!(!overlap(mv, fr), "{w}x{h}: the buttons overlap");
        // the review card, with the warning
        a.handle(&Event::Tab(OLLAMA_MOVE), &cx);
        assert!(a.setup.confirm.is_some());
        let g = draw(&mut a, &cx, w, h);
        for code in [CONFIRM_YES, CONFIRM_NO] {
            let r = hit(&g, code).unwrap_or_else(|| panic!("button {code} at {w}x{h}"));
            assert!(inside(r, w, h), "{w}x{h}: {r:?}");
        }
        // the popup of a pod item, with its numbers
        let (mut b, cx2) = rig(&format!("ol-four-ch-{w}"), Role::Admin, Presence::Missing, 0);
        b.setup.open_popup(Item::ClickHouse);
        let g = draw(&mut b, &cx2, w, h);
        assert!(g.hits.iter().all(|x| x.rect.w >= 0.0 && x.rect.h >= 0.0));
    }
}
