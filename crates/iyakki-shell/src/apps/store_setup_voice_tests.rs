// #[cfg(test)] -- test code, included by store_setup.rs under #[cfg(test)].
//! Store -> Setup and "Voice: Whisper tiny + Kokoro": recommended only where no speech engine is set up, a review with the licence texts
//! before anything starts, the cluster rule, and Remove (Undo). Fakes only: no cluster, no download, no model.
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

fn machine(voice: Option<bool>) -> Machine {
    Machine { ram_mb: 16_000, voice_engines: voice, ..Default::default() }
}

fn rig(tag: &str, role: Role, m: Machine, present: &[Item], other_cluster: bool) -> (StoreApp, AppCx) {
    let cx = super::super::tests::cx(tag, role);
    if other_cluster {
        let (dir, user) = (cx.services.dir().to_path_buf(), cx.session.user.clone());
        let mut r = Registry::load(&dir, &user);
        r.add_remote("gpu-box", Kind::Remote, "/fake/gpu.yaml", "", "").unwrap();
        r.save(&dir, &user).unwrap();
        iyakki_agent::clusters::switch(&dir, &dir, &user, Role::Admin, "gpu-box").unwrap();
    }
    let mut a = StoreApp::new(&cx);
    a.setup.set_machine(m);
    a.setup.set_facts(facts_with(present));
    a.setup.launch = Some(qi::Launch { exe: "/usr/bin/true".into(), data: cx.services.dir().join("qi") });
    a.handle(&Event::Tab(4), &cx);
    assert!(a.section == Section::Setup);
    a.setup.enter(&cx);
    (a, cx)
}

fn audit(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default().replace("\\\"", "\"")
}

#[test]
fn it_is_recommended_only_where_no_speech_engine_is_set_up_yet() {
    let none = |_: Item| false;
    assert!(machine(Some(false)).recommend(Item::VoicePack, &none).is_some_and(|w| w.contains("No speech engine")));
    assert_eq!(machine(Some(true)).recommend(Item::VoicePack, &none), None, "an engine exists: not recommended");
    assert_eq!(machine(None).recommend(Item::VoicePack, &none), None, "not read: not recommended");
    assert_eq!(machine(Some(false)).recommend(Item::VoicePack, &|i| i == Item::VoicePack), None, "already installed");
    assert_eq!(Machine { ram_mb: 4_000, ..machine(Some(false)) }.recommend(Item::VoicePack, &none), None, "too little memory");
    assert_eq!(Machine { window: true, ..machine(Some(false)) }.recommend(Item::VoicePack, &none), None, "a desktop window never gets a cluster");
    // the tile: the card carries the reason, the Voices category lists it, the filter finds it
    let (a, _cx) = rig("svp-rec", Role::Admin, machine(Some(false)), &[Item::K3s], false);
    assert!(a.setup.card(Item::VoicePack).reason.is_some() && a.setup.recommended(Item::VoicePack).is_some());
    assert_eq!(Gk::of(Item::VoicePack), Gk::Voices);
    let (b, _cx) = rig("svp-rec2", Role::Admin, machine(Some(true)), &[Item::K3s], false);
    assert!(b.setup.card(Item::VoicePack).reason.is_none());
    // the action is Install; it is in the plan of "Install all recommended", with the same review
    let c = a.setup.plan_all();
    assert!(c.targets.contains(&Item::VoicePack) && c.steps == [Item::Lemonade, Item::VoicePack], "{:?}", c.steps);
    assert!(iyakki_agent::voicepack::review_has_required(&c.review), "the licence texts are in the plan of Install all recommended");
    assert!(c.cluster.is_some(), "it names the cluster it deploys to");
}

#[test]
fn pressing_install_opens_the_review_with_both_licences_and_starts_nothing_until_it_is_confirmed() {
    let (mut a, cx) = rig("svp-review", Role::Admin, machine(Some(false)), &[Item::K3s], false);
    a.setup.open_popup(Item::VoicePack);
    a.setup.press(Item::VoicePack, &cx);
    let c = a.setup.confirm.as_ref().expect("the review is open");
    assert_eq!(c.steps, [Item::Lemonade, Item::VoicePack]);
    let t = c.review.join("\n");
    for need in ["MIT License", "Apache License", "espeak-ng (GPL-3.0-or-later)", "Not shipped with IYAKKI; downloaded at your click", "Kokoro reads English only", "about 75 MB", "about 330 MB"] {
        assert!(t.contains(need), "{need}");
    }
    assert!(a.setup.popup.is_none(), "the popup makes room for the review");
    assert!(a.setup.jobs.is_empty() && !audit(&cx).contains("quickinstall.start"), "nothing started yet");
    // the review is drawn, with its own buttons, and scrolls with the keys and the wheel
    let s = crate::shell::scale_for(1280, 800);
    let mut g = Gfx::new(1280, 800);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 46.0 * s, 1280.0, 800.0 - 46.0 * s), s, &cx, 3);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(CONFIRM_YES)) && g.hits.iter().any(|h| h.ev == Event::Tab(CONFIRM_NO)));
    a.handle(&Event::Key(Key::PageDown), &cx);
    a.handle(&Event::Key(Key::Down), &cx);
    assert_eq!(a.setup.confirm.as_ref().unwrap().scroll, 13);
    a.handle(&Event::Scroll(-1), &cx);
    assert_eq!(a.setup.confirm.as_ref().unwrap().scroll, 10);
    a.handle(&Event::Key(Key::End), &cx);
    let last = a.setup.confirm.as_ref().unwrap().scroll;
    assert!(last > 100, "the Apache text makes it long: {last}");
    a.handle(&Event::Key(Key::PageDown), &cx);
    assert_eq!(a.setup.confirm.as_ref().unwrap().scroll, last, "never past the end");
    a.handle(&Event::Key(Key::Home), &cx);
    assert_eq!(a.setup.confirm.as_ref().unwrap().scroll, 0);
    // Esc cancels: still nothing started
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.setup.confirm.is_none() && a.setup.jobs.is_empty() && !audit(&cx).contains("quickinstall.start"));
    // confirming starts Lemonade (not there) and the pack as jobs, audited with the cluster
    a.setup.press(Item::VoicePack, &cx);
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert!(a.setup.confirm.is_none());
    let au = audit(&cx);
    assert!(au.contains("quickinstall.start") && au.contains("voice-pack") && au.contains("Install Lemonade (1/2)"), "{au}");
    assert!(au.contains("on cluster \"This machine\" (local)"), "{au}");
}

#[test]
fn another_active_cluster_and_a_viewer_are_refused_before_the_review_opens() {
    let (mut a, cx) = rig("svp-cluster", Role::Admin, machine(Some(false)), &[Item::K3s], true);
    a.setup.press(Item::VoicePack, &cx);
    let note = a.setup.note.clone().unwrap();
    assert!(!note.1 && note.0.contains("cluster \"gpu-box\" (remote)") && note.0.contains("\"This machine\""), "{note:?}");
    assert!(a.setup.confirm.is_none() && a.setup.jobs.is_empty());
    assert!(a.setup.deploy_text(Item::VoicePack).is_some_and(|t| t.starts_with("Deploy Voice: Whisper tiny + Kokoro to cluster")), "the popup's Deploys to row exists for it");
    let (mut v, cxv) = rig("svp-viewer", Role::User, machine(Some(false)), &[Item::K3s], false);
    v.setup.press(Item::VoicePack, &cxv);
    assert!(v.setup.confirm.is_none() && v.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
}

#[test]
fn remove_is_offered_to_an_administrator_once_installed_and_undoes_through_the_pack_code() {
    let (mut a, cx) = rig("svp-remove", Role::Admin, machine(Some(false)), &[Item::K3s, Item::Lemonade, Item::VoicePack], false);
    a.setup.open_popup(Item::VoicePack);
    let buttons = a.setup.popup_button_list(Item::VoicePack);
    assert!(buttons.iter().any(|(l, on)| l == "Remove (undo)" && *on), "{buttons:?}");
    // not offered before it is installed, nor to a standard user
    let (b, _cxb) = rig("svp-remove2", Role::Admin, machine(Some(false)), &[Item::K3s], false);
    let mut b = b;
    b.setup.open_popup(Item::VoicePack);
    assert!(!b.setup.popup_button_list(Item::VoicePack).iter().any(|(l, _)| l == "Remove (undo)"));
    // the engines before, the pack's record, and a fake Lemonade that deletes
    let old = iyakki_agent::voice::SpeechEngine::Command { command: "piper --output_file {out}".into() };
    cx.services.settings.lock().unwrap().update(|s| s.voice.tts = Some(old.clone())).unwrap();
    cx.services.use_local_lemonade("http://127.0.0.1:13305/api/v1", None);
    let id = cx.services.settings.lock().unwrap().get().providers.iter().find(|p| p.kind == iyakki_agent::providers::Kind::Lemonade).unwrap().id.clone();
    iyakki_agent::voicepack::record_pulled(cx.services.dir(), &["Whisper-Tiny".to_string(), "kokoro-v1".to_string()]).unwrap();
    iyakki_agent::voicepack::configure(&cx.services, &id).unwrap();
    let deleted = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = deleted.clone();
    a.setup.lemonade_delete = Arc::new(move |m: &str| {
        log.lock().unwrap().push(m.to_string());
        Ok(())
    });
    a.handle(&Event::Tab(REMOVE + idx_of(Item::VoicePack)), &cx);
    assert_eq!(*deleted.lock().unwrap(), ["Whisper-Tiny", "kokoro-v1"]);
    let note = a.setup.note.clone().unwrap();
    assert!(note.1 && note.0.contains("Your earlier speech engines are back") && note.0.contains("Deleted kokoro-v1"), "{note:?}");
    assert_eq!(cx.services.settings.lock().unwrap().get().voice.tts, Some(old));
    assert!(audit(&cx).contains("quickinstall.voicepack.remove"));
    // a standard user cannot
    let (mut v, cxv) = rig("svp-remove3", Role::User, machine(Some(false)), &[Item::K3s, Item::Lemonade, Item::VoicePack], false);
    v.setup.remove_voice_pack(&cxv);
    assert!(v.setup.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("administrator")));
}

fn idx_of(i: Item) -> usize {
    Item::ALL.iter().position(|x| *x == i).unwrap()
}
