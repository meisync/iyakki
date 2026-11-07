// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup: removing a Quick install item. The Remove button, the review card (what goes, what stays, the data tick off by default,
//! the dependants), the progress and result, the retry, and the card at four sizes. Fakes only: no cluster, no network.
use super::remove::*;
use super::*;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_sec::policy::Role;

const FOUR: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn facts_with(present: &[Item]) -> Facts {
    let mut f = Facts::default();
    for it in Item::ALL {
        f.presence.push((it, if present.contains(&it) { Presence::Installed(String::new()) } else { Presence::Missing }));
    }
    f
}

fn rig(tag: &str, role: Role, present: &[Item]) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(Machine { ram_mb: 16_000, ..Default::default() });
    a.setup.set_facts(facts_with(present));
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

fn idx(i: Item) -> usize {
    Item::ALL.iter().position(|x| *x == i).unwrap()
}

fn hit(g: &Gfx, code: usize) -> Option<R> {
    g.hits.iter().find(|h| h.ev == Event::Tab(code)).map(|h| h.rect)
}

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

fn press(a: &mut StoreApp, cx: &AppCx, code: usize) {
    a.handle(&Event::Tab(code), cx);
}

#[test]
fn every_installed_item_has_a_remove_button_for_an_administrator_and_nobody_else() {
    let (mut a, _cx) = rig("rm-btn", Role::Admin, &Item::ALL.into_iter().filter(|i| *i != Item::MacHost).collect::<Vec<_>>());
    for it in Item::ALL {
        if !a.setup.machine.shows(it) || it == Item::VoicePack || it == Item::MacHost {
            continue;
        }
        a.setup.open_popup(it);
        assert!(a.setup.popup_button_list(it).iter().any(|(l, on)| l.starts_with("Remove") && *on), "{it:?}");
    }
    let (mut m, _cx) = rig("rm-btn-missing", Role::Admin, &[]);
    m.setup.open_popup(Item::Elasticsearch);
    assert!(!m.setup.popup_button_list(Item::Elasticsearch).iter().any(|(l, _)| l.starts_with("Remove")), "nothing to remove");
    let (mut v, cx) = rig("rm-btn-viewer", Role::User, &[Item::Elasticsearch]);
    v.setup.open_popup(Item::Elasticsearch);
    assert!(!v.setup.popup_button_list(Item::Elasticsearch).iter().any(|(l, _)| l.starts_with("Remove")));
    v.setup.open_remove(Item::Elasticsearch, &cx);
    assert!(v.setup.rm.is_none() && v.setup.note.is_some(), "a viewer is told to ask an administrator");
}

#[test]
fn a_cluster_found_elsewhere_is_never_offered_for_removal() {
    let (mut a, _cx) = rig("rm-else", Role::Admin, &[]);
    let mut f = facts_with(&[]);
    for (i, p) in f.presence.iter_mut() {
        if *i == Item::K3s {
            *p = Presence::Elsewhere("/usr/local/bin/k3s".into());
        }
    }
    a.setup.set_facts(f);
    assert!(!a.setup.can_remove(Item::K3s));
}

#[test]
fn the_card_keeps_the_data_by_default_and_a_removal_ends_with_the_item_not_installed() {
    let (mut a, cx) = rig("rm-flow", Role::Admin, &[Item::Elasticsearch]);
    qi::mark_done(cx.services.dir(), Item::Elasticsearch, "up").unwrap();
    a.setup.open_popup(Item::Elasticsearch);
    press(&mut a, &cx, RM_OPEN + idx(Item::Elasticsearch));
    let c = a.setup.rm.as_ref().expect("the review card opened");
    assert!(!c.delete_data, "the data tick is off by default");
    let text = c.lines().into_iter().map(|l| l.1).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Deployment elasticsearch") && text.contains("(kept)") && text.contains("elasticsearch-data"), "{text}");
    // d ticks the data, again unticks; Esc cancels and removes nothing
    a.handle(&Event::Key(Key::Char('d')), &cx);
    assert!(a.setup.rm.as_ref().unwrap().delete_data);
    a.handle(&Event::Key(Key::Char('d')), &cx);
    assert!(!a.setup.rm.as_ref().unwrap().delete_data);
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.setup.rm.is_none() && qi::marked(cx.services.dir(), Item::Elasticsearch).is_some());
    // Enter does not confirm: only the Remove button or y
    press(&mut a, &cx, RM_OPEN + idx(Item::Elasticsearch));
    a.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(a.setup.rm.as_ref().unwrap().phase, Phase::Review);
    // the explicit confirm
    press(&mut a, &cx, RM_YES);
    match &a.setup.rm.as_ref().unwrap().phase {
        Phase::Done(m) => assert!(m.contains("Elasticsearch removed") && m.contains("kept"), "{m}"),
        p => panic!("{p:?}"),
    }
    assert!(qi::marked(cx.services.dir(), Item::Elasticsearch).is_none());
    assert!(!a.setup.present(Item::Elasticsearch), "the item shows as not installed afterwards");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.setup.rm.is_none());
}

#[test]
fn k3s_with_dependants_cannot_be_confirmed_until_they_are_ticked_and_the_card_lists_them() {
    let (mut a, cx) = rig("rm-deps", Role::Admin, &[Item::K3s, Item::Lemonade, Item::Elasticsearch]);
    a.setup.open_popup(Item::K3s);
    press(&mut a, &cx, RM_OPEN + idx(Item::K3s));
    let c = a.setup.rm.as_ref().unwrap();
    assert_eq!(c.deps.len(), 2);
    assert!(c.blocked().unwrap().contains("Lemonade"));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(hit(&g, RM_YES).is_none(), "the Remove button is off while the dependants are not ticked");
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(a.setup.rm.as_ref().unwrap().phase, Phase::Review, "y does nothing either");
    press(&mut a, &cx, RM_DEPS);
    assert!(a.setup.rm.as_ref().unwrap().blocked().is_none());
}

#[test]
fn an_item_that_is_its_own_data_needs_the_data_tick() {
    let (mut a, cx) = rig("rm-own", Role::Admin, &[Item::LinuxApps]);
    a.setup.open_popup(Item::LinuxApps);
    press(&mut a, &cx, RM_OPEN + idx(Item::LinuxApps));
    assert!(a.setup.rm.as_ref().unwrap().blocked().is_some());
    press(&mut a, &cx, RM_DATA);
    assert!(a.setup.rm.as_ref().unwrap().blocked().is_none());
}

#[test]
fn a_failed_removal_shows_what_happened_and_retry_runs_it_again() {
    // nothing is installed here, so the engine refuses: the card shows the reason and offers Retry
    let (mut a, cx) = rig("rm-fail", Role::Admin, &[Item::Prometheus]);
    a.setup.open_popup(Item::Prometheus);
    press(&mut a, &cx, RM_OPEN + idx(Item::Prometheus));
    press(&mut a, &cx, RM_YES);
    assert!(matches!(a.setup.rm.as_ref().unwrap().phase, Phase::Failed(_)));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(hit(&g, RM_RETRY).is_some() && hit(&g, RM_CLOSE).is_some());
    a.handle(&Event::Key(Key::Char('r')), &cx);
    assert!(matches!(a.setup.rm.as_ref().unwrap().phase, Phase::Failed(_)), "ran again");
    press(&mut a, &cx, RM_CLOSE);
    assert!(a.setup.rm.is_none());
}

#[test]
fn the_card_fits_at_the_four_sizes_in_every_state_and_nothing_overlaps() {
    for (w, h) in FOUR {
        let s = crate::shell::scale_for(w, h);
        let screen = R::new(0.0, 0.0, w as f32, h as f32);
        for (tag, item, present) in [("a", Item::K3s, vec![Item::K3s, Item::Lemonade, Item::Elasticsearch, Item::Kokoro]), ("b", Item::Gitea, vec![Item::Gitea]), ("c", Item::Prometheus, vec![Item::Prometheus])] {
            let (mut a, cx) = rig(&format!("rm-size-{tag}-{w}"), Role::Admin, &present);
            a.setup.open_popup(item);
            press(&mut a, &cx, RM_OPEN + idx(item));
            for (data, deps) in [(false, false), (true, true)] {
                if let Some(c) = a.setup.rm.as_mut() {
                    c.delete_data = data;
                    c.with_deps = deps;
                    c.phase = Phase::Review;
                }
                let g = draw(&mut a, &cx, w, h);
                let ours: Vec<(usize, R)> = [RM_YES, RM_NO, RM_DATA, RM_DEPS, RM_CLOSE].iter().filter_map(|c| hit(&g, *c).map(|r| (*c, r))).collect();
                assert!(hit(&g, RM_NO).is_some() && hit(&g, RM_DATA).is_some(), "{w}x{h} {tag}: cancel and the data tick are always there");
                for (c, r) in &ours {
                    assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= screen.w + 0.5 && r.y + r.h <= screen.h + 0.5, "{w}x{h} {tag}: {c} leaves the screen: {r:?}");
                }
                for i in 0..ours.len() {
                    for j in i + 1..ours.len() {
                        let (a1, b1) = (ours[i], ours[j]);
                        if [a1.0, b1.0].contains(&RM_CLOSE) {
                            continue;
                        }
                        assert!(!overlap(a1.1, b1.1), "{w}x{h} {tag}: {} and {} overlap", a1.0, b1.0);
                    }
                }
                if a.setup.rm.as_ref().unwrap().blocked().is_none() {
                    assert!(hit(&g, RM_YES).is_some(), "{w}x{h} {tag}: Remove is pressable once confirmed");
                }
            }
            for ph in [Phase::Running, Phase::Done("Gitea removed. Your data is kept, so installing it again finds it. ".repeat(3)), Phase::Failed("could not remove the Deployment gitea (the cluster did not answer). 2 step(s) were done; press Retry to run the rest again.".into())] {
                a.setup.rm.as_mut().unwrap().phase = ph.clone();
                let g = draw(&mut a, &cx, w, h);
                for code in [RM_RETRY, RM_CLOSE] {
                    if let Some(r) = hit(&g, code) {
                        assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= screen.w + 0.5 && r.y + r.h <= screen.h + 0.5, "{w}x{h} {tag} {ph:?}: {code} leaves the screen");
                    }
                }
                if matches!(ph, Phase::Failed(_)) {
                    assert!(hit(&g, RM_RETRY).is_some(), "{w}x{h}: Retry");
                    assert!(!overlap(hit(&g, RM_RETRY).unwrap(), hit(&g, RM_CLOSE).unwrap()));
                }
            }
            let _ = s;
        }
    }
}
