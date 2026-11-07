// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup -> Browser audio and video -> "Build it here (k3s)": offered with k3s running, a plain message without it, the consent text
//! before anything starts, a message when this IYAKKI version has no map entry. Fakes only: no cluster, no download, no build.
use super::*;
use crate::apps::store::{Section, StoreApp};
use crate::apps::{App, AppCx};
use iyakki_sec::policy::Role;

const MAP: &str = "0.1.0-alpha.*  6.1.5  05fbe9db1f5452a3605bb1d9bf91da9d4bb08162891692e4f172b58c49109412  docker.io/library/alpine@sha256:1e42bbe2508154c9126d48c2b8a75420c3544343bf86fd041fb7527e017a4b4a  libavcodec.so.60,libavformat.so.60,libavutil.so.58,libavfilter.so.9,libswresample.so.4,libswscale.so.7\n";

fn rig(tag: &str, role: Role, k3s_running: bool, map: &str) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    let mut f = Facts::default();
    for it in Item::ALL {
        let p = if it == Item::K3s && k3s_running { Presence::Installed("k3s v1.31.4 · running".into()) } else { Presence::Missing };
        f.presence.push((it, p));
    }
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(Machine { ram_mb: 16_000, ..Default::default() });
    a.setup.set_facts(f);
    a.setup.av_map = Some(map.to_string());
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: cx.services.dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&cx);
    (a, cx)
}

fn idx() -> usize {
    Item::ALL.iter().position(|x| *x == Item::FfmpegLibs).unwrap()
}

fn draw(a: &mut StoreApp, cx: &AppCx) -> Gfx {
    let (w, h) = (1280u32, 720u32);
    let s = crate::shell::scale_for(w, h);
    let hh = 46.0 * s;
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, hh, w as f32, h as f32 - hh), s, cx, 3);
    g
}

#[test]
fn the_build_button_is_on_the_card_for_administrators_only() {
    let (mut a, cx) = rig("avb-btn", Role::Admin, true, MAP);
    a.setup.open_popup(Item::FfmpegLibs);
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(BUILD + idx()) && h.label == "Build it here (k3s)"));
    for role in [Role::User, Role::Operator] {
        let (mut b, cx) = rig(&format!("avb-btn-{}", role.name()), role, true, MAP);
        if cx.session.policy.manage_settings {
            continue;
        }
        b.setup.open_popup(Item::FfmpegLibs);
        let g = draw(&mut b, &cx);
        assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(BUILD + idx())), "no build button for {}", role.name());
        b.handle(&Event::Tab(BUILD + idx()), &cx);
        assert!(b.setup.confirm.is_none() && b.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
    }
}

#[test]
fn without_k3s_running_a_plain_message_points_to_the_file_and_mirror_routes() {
    let (mut a, cx) = rig("avb-nok3s", Role::Admin, false, MAP);
    a.setup.open_popup(Item::FfmpegLibs);
    a.handle(&Event::Tab(BUILD + idx()), &cx);
    assert!(a.setup.confirm.is_none(), "nothing to confirm");
    let n = a.setup.note.clone().unwrap();
    assert!(!n.1 && n.0.contains("k3s") && n.0.contains("/local/packages") && n.0.contains("mirror"), "{n:?}");
    assert!(!iyakki_agent::avbuild::take_request(cx.services.dir()), "no request was written");
}

#[test]
fn the_consent_text_is_shown_before_anything_starts_and_the_button_writes_the_request() {
    let (mut a, cx) = rig("avb-consent", Role::Admin, true, MAP);
    a.setup.open_popup(Item::FfmpegLibs);
    a.handle(&Event::Tab(BUILD + idx()), &cx);
    let c = a.setup.confirm.as_ref().expect("the consent step opens");
    assert_eq!(c.title, iyakki_agent::avbuild::CONFIRM_TITLE);
    let text = c.review.join("\n");
    for must in ["ffmpeg.org", "docker.io", "Alpine", "Total download", "LGPL-2.1", "Patents", "Nothing is installed"] {
        assert!(text.contains(must), "missing {must}");
    }
    assert!(!iyakki_agent::avbuild::take_request(cx.services.dir()), "nothing is requested before the button");
    assert!(qi::install_jobs(&cx.services.jobs, &cx.session.user).is_empty(), "no job before the button");
    a.handle(&Event::Tab(CONFIRM_YES), &cx);
    assert!(iyakki_agent::avbuild::take_request(cx.services.dir()), "the button records the request for the install job");
}

#[test]
fn a_version_without_a_map_entry_gets_a_clear_message_and_no_consent_step() {
    let (mut a, cx) = rig("avb-noentry", Role::Admin, true, "9.9.9  6.1.5  05fbe9db1f5452a3605bb1d9bf91da9d4bb08162891692e4f172b58c49109412  docker.io/library/alpine@sha256:1e42bbe2508154c9126d48c2b8a75420c3544343bf86fd041fb7527e017a4b4a  libavcodec.so.60\n");
    a.setup.open_popup(Item::FfmpegLibs);
    a.handle(&Event::Tab(BUILD + idx()), &cx);
    assert!(a.setup.confirm.is_none());
    let n = a.setup.note.clone().unwrap();
    assert!(!n.1 && n.0.contains("no build recipe") && n.0.contains("mirror") && n.0.contains("/local/packages"), "{n:?}");
}
