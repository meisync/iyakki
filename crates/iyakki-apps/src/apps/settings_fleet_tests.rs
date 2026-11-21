//! Settings -> Fleet (experimental): who sees it, that every change asks first and then does what the chat tool does, the core's say, a join
//! with a second system over a real loopback socket, the heartbeat numbers on the page, and the page from a phone to a large screen and at
//! the largest text. Fake machine; the other system is a second hub in this process. Nobody looked at the page on a screen.

use std::time::Duration;

use super::fleet_page::{FleetAct, Kind};
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::fleet::{FixedSource, Heartbeat};
use iyakki_agent::fleet_hub::Hub;

struct Rig {
    cx: AppCx,
    a: SettingsApp,
    os: Arc<iyakki_core_fake::FakeOs>,
    hub: Arc<Hub>,
}

fn rig(tag: &str) -> Rig {
    let mut cx = cx_with_admin_store(tag);
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    cx.os = os.clone();
    let hub = Hub::open(cx.data_dir()).unwrap();
    hub.set_bind("127.0.0.1:0");
    let mut a = SettingsApp::new(&cx);
    for _ in 0..90 {
        if a.tab() == Tab::Fleet {
            return Rig { cx, a, os, hub };
        }
        a.handle(&Event::Key(Key::Right), &cx);
    }
    panic!("no page Fleet");
}

fn key(r: &mut Rig, c: char) {
    r.a.handle(&Event::Key(Key::Char(c)), &r.cx);
}

fn yes(r: &mut Rig) {
    key(r, 'y');
}

fn audit(cx: &AppCx) -> Vec<String> {
    cx.audit.recent(300)
}

fn hb(n: u64) -> Heartbeat {
    Heartbeat { disk_free_mb: 1000 * n, disk_total_mb: 9000, cpu_load_pct: 10 * n as u32, cpus: 4, mem_used_pct: 20, mem_total_mb: 8000, tasks_running: 1, models: vec!["m".into()], battery_pct: None }
}

fn remote(tag: &str) -> (Arc<Hub>, String) {
    let h = Arc::new(Hub::load(&temp_dir(tag)).unwrap());
    let a = h.start_node("127.0.0.1:0", Arc::new(FixedSource(hb(2)))).unwrap().to_string();
    (h, a)
}

fn select(r: &mut Rig, f: impl Fn(&super::fleet_page::FRow) -> bool) {
    let snap = r.a.fleet_snapshot(&r.cx).unwrap();
    let want = snap.rows.iter().position(f).expect("such a row");
    for _ in 0..80 {
        let cur = r.a.sel[r.a.tab].min(snap.rows.len().saturating_sub(1));
        if cur == want {
            return;
        }
        r.a.handle(&Event::Key(Key::Down), &r.cx);
    }
    panic!("could not select");
}

fn draw_big(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32, big: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp_safe(0.6, 3.0) * big;
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

#[test]
fn only_an_administrator_has_the_page_and_it_sits_under_the_network_group_and_search_finds_it() {
    let r = rig("flt-admin");
    assert!(r.a.tabs().contains(&Tab::Fleet));
    assert_eq!(Tab::Fleet.label(), "Fleet (experimental)");
    assert_eq!(Tab::Fleet.group(), Tab::Shares.group());
    let mut cx = cx_with_admin_store("flt-user");
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Fleet));
    let cx = cx_with_admin_store("flt-find");
    let mut a = SettingsApp::new(&cx);
    a.handle(&Event::Key(Key::Ctrl('f')), &cx);
    for c in "heartbeat".chars() {
        a.handle(&Event::Key(Key::Char(c)), &cx);
    }
    let i = a.tabs().iter().position(|t| *t == Tab::Fleet).unwrap();
    assert!(a.nav_matches().contains(&i), "searching heartbeat finds the page");
}

#[test]
fn creating_a_fleet_asks_first_does_nothing_until_y_and_asks_the_core_which_can_refuse() {
    let mut r = rig("flt-create");
    key(&mut r, 'c');
    let f = Form::new("x").text("Fleet name", "workshop", "").text("Name of this computer", "alpha", "");
    r.a.fleet_form_submit(&r.cx, &FormKind::FleetCreate, &f).unwrap();
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Create(..)), _))), "a card first");
    assert!(r.hub.status().fleets.is_empty(), "nothing changed while the card is up");
    r.a.handle(&Event::Key(Key::Esc), &r.cx);
    assert!(r.a.modal.is_none() && r.hub.status().fleets.is_empty());
    // the core says no
    r.os.fleet_fake().refuse(true);
    r.a.fleet_form_submit(&r.cx, &FormKind::FleetCreate, &f).unwrap();
    yes(&mut r);
    assert!(r.hub.status().fleets.is_empty(), "the core refused: no fleet, no listener");
    assert!(r.hub.listening().is_none());
    assert!(r.a.status.as_ref().is_some_and(|(m, ok)| !ok && m.contains("refused")), "{:?}", r.a.status);
    r.os.fleet_fake().refuse(false);
    r.a.fleet_form_submit(&r.cx, &FormKind::FleetCreate, &f).unwrap();
    yes(&mut r);
    let st = r.hub.status();
    assert_eq!((st.fleets.len(), st.fleets[0].name.as_str(), st.fleets[0].members.len()), (1, "workshop", 1));
    assert!(st.listening.is_some(), "the channel started with the first fleet");
    assert!(audit(&r.cx).iter().any(|l| l.contains("fleet.create")), "the same audit line as the chat tool");
    assert!(r.os.fleet_fake().calls().iter().any(|c| c.starts_with("manage(")), "the core was asked");
    r.hub.stop_node();
}

#[test]
fn a_join_code_is_shown_on_the_page_only_a_second_system_joins_over_a_socket_and_its_numbers_show() {
    let mut r = rig("flt-join");
    let (other, other_addr) = remote("flt-join-other");
    // the other system makes a fleet and offers a code
    let fid = other.create_fleet("remote-lab", "bravo").unwrap();
    let (code, _) = other.offer_code(&fid).unwrap();
    // this page asks to join
    let f = Form::new("x").text("Address and port", &other_addr, "").text("Join code", &code, "").text("Name of this computer", "alpha", "");
    r.a.fleet_form_submit(&r.cx, &FormKind::FleetJoin, &f).unwrap();
    assert!(r.hub.status().pending.is_empty(), "asking waits for the card");
    yes(&mut r);
    assert_eq!(r.hub.status().pending.len(), 1, "{:?}", r.a.status);
    // the administrator there approves (a card on that side in a real system)
    let req = other.status().requests;
    assert_eq!(req.len(), 1);
    assert_eq!(req[0].fingerprint, iyakki_agent::fleettools::fp(&r.hub.my_id()), "the fingerprint on their card is this computer's");
    other.approve_join(&req[0].id).unwrap();
    r.hub.poll_pending();
    // the offer is a row with the other side's fingerprint; accepting asks a card
    let snap = r.a.fleet_snapshot(&r.cx).unwrap();
    assert!(snap.rows.iter().any(|x| matches!(x.kind, Kind::Pending { offered: true }) && x.sub.contains(&iyakki_agent::fleettools::fp(&other.my_id()))), "{:?}", snap.rows.iter().map(|x| &x.sub).collect::<Vec<_>>());
    select(&mut r, |x| matches!(x.kind, Kind::Pending { offered: true }));
    key(&mut r, 'y');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Accept(_)), _))));
    assert!(r.hub.status().fleets.is_empty());
    yes(&mut r);
    assert_eq!(r.hub.status().fleets.len(), 1, "{:?}", r.a.status);
    other.settle();
    // numbers both ways
    r.hub.heartbeat_tick(&FixedSource(hb(1)));
    other.heartbeat_tick(&FixedSource(hb(2)));
    let snap = r.a.fleet_snapshot(&r.cx).unwrap();
    let theirs = snap.rows.iter().find(|x| x.title.starts_with("bravo")).unwrap();
    assert!(theirs.sub.contains("load 20%") && theirs.sub.contains("disk 2.0 GB free"), "{}", theirs.sub);
    // the page draws; the code of an offer is on the page of the one who offered, not in the assistant's status
    let g = draw_big(&mut r.a, &r.cx, 1280, 720, 1.0);
    assert!(g.hits.iter().any(|h| h.label.contains("Create fleet")));
    r.hub.stop_node();
    other.stop_node();
}

#[test]
fn removing_a_member_trusting_and_the_accept_work_setting_each_ask_first() {
    let mut r = rig("flt-members");
    let (other, other_addr) = remote("flt-members-other");
    let fid = r.hub.create_fleet("lab", "alpha").unwrap();
    r.hub.start_node("127.0.0.1:0", Arc::new(FixedSource(hb(1)))).unwrap();
    let (code, _) = r.hub.offer_code(&fid).unwrap();
    let my_addr = r.hub.listening().unwrap().to_string();
    let t = other.join_start(&my_addr, &code, "bravo").unwrap();
    r.hub.approve_join(&r.hub.status().requests[0].id).unwrap();
    other.poll_pending();
    other.join_accept(&t).unwrap();
    r.hub.settle();
    let _ = other_addr;
    // trust bravo
    select(&mut r, |x| x.title.starts_with("bravo"));
    key(&mut r, 't');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Trust(..)), _))));
    yes(&mut r);
    assert!(r.hub.status().fleets[0].members.iter().any(|m| m.name == "bravo" && m.trusted));
    // the accept-work setting goes to the next value
    key(&mut r, 'm');
    yes(&mut r);
    assert_eq!(r.hub.status().accept, iyakki_shared::fleetview::Accept::Never);
    assert!(r.os.fleet_fake().calls().iter().any(|c| c.starts_with("receive(")));
    // remove asks with the consequence on the card, and nothing happens until y
    key(&mut r, 'x');
    match &r.a.modal {
        Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Remove(..)), text)) => assert!(text.contains("group key changes"), "{text}"),
        m => panic!("not the remove card (a card at all: {})", m.is_some()),
    }
    assert_eq!(r.hub.status().fleets[0].members.len(), 2);
    yes(&mut r);
    assert_eq!(r.hub.status().fleets[0].members.len(), 1);
    assert_eq!(r.hub.status().fleets[0].generation, 1);
    r.hub.settle();
    r.hub.stop_node();
    other.stop_node();
}

#[test]
fn the_page_has_the_tls_switch_a_members_address_the_relay_switch_and_the_integration_keys_and_each_asks_first() {
    let mut r = rig("flt-channel");
    let (other, _other_addr) = remote("flt-channel-other");
    let fid = r.hub.create_fleet("lab", "alpha").unwrap();
    r.hub.start_node("127.0.0.1:0", Arc::new(FixedSource(hb(1)))).unwrap();
    let (code, _) = r.hub.offer_code(&fid).unwrap();
    let t = other.join_start(&r.hub.listening().unwrap().to_string(), &code, "bravo").unwrap();
    r.hub.approve_join(&r.hub.status().requests[0].id).unwrap();
    other.poll_pending();
    other.join_accept(&t).unwrap();
    r.hub.settle();
    // the head says what the channel is and what the relay is
    let snap = r.a.fleet_snapshot(&r.cx).unwrap();
    assert!(!snap.tls && snap.relay.is_empty());
    assert!(snap.head.iter().any(|l| l.starts_with("Channel: plain")) && snap.head.iter().any(|l| l.contains("Relay: off")), "{:?}", snap.head);
    // T asks, and says what changes
    key(&mut r, 'T');
    match &r.a.modal {
        Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Tls(true)), text)) => assert!(text.contains("TLS") && text.contains("fingerprint"), "{text}"),
        m => panic!("not the TLS card (a card at all: {})", m.is_some()),
    }
    assert!(!r.hub.tls_wanted(), "nothing changes until y");
    r.a.modal = None;
    // a: the address of the chosen member, a form, then a card; a mailbox address is taken
    select(&mut r, |x| x.title.starts_with("bravo"));
    key(&mut r, 'a');
    assert!(matches!(r.a.modal, Some(Modal::Form(FormKind::FleetAddress(..), _))));
    let who = other.my_id();
    let f = Form::new("x").text("Address", &format!("relay://{}", iyakki_agent::fleet_hub::relay_mailbox_of(&who)), "");
    r.a.fleet_form_submit(&r.cx, &FormKind::FleetAddress(fid.clone(), who.clone()), &f).unwrap();
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Address(..)), _))));
    yes(&mut r);
    let snap = r.a.fleet_snapshot(&r.cx).unwrap();
    assert!(snap.rows.iter().any(|x| x.title.starts_with("bravo") && x.sub.contains("through the relay")), "{:?}", snap.rows.iter().map(|x| &x.sub).collect::<Vec<_>>());
    // an empty address is refused on the form
    assert!(r.a.fleet_form_submit(&r.cx, &FormKind::FleetAddress(fid.clone(), who), &Form::new("x").text("Address", "", "")).is_err());
    // u: the relay asks first; with no relay to point at, the page says so plainly and nothing is switched on
    key(&mut r, 'u');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Relay(true)), _))));
    yes(&mut r);
    assert!(r.a.status.as_ref().is_some_and(|s| s.0.contains("no relay to connect to")), "{:?}", r.a.status);
    assert!(r.a.fleet_snapshot(&r.cx).unwrap().relay.is_empty());
    // i and f: each asks first; with no project open the answer says to open one
    key(&mut r, 'i');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Integrate(_, true)), _))));
    yes(&mut r);
    assert!(r.a.status.as_ref().is_some_and(|s| s.0.contains("no IDE workspace is open")), "{:?}", r.a.status);
    key(&mut r, 'f');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Finalize(_)), _))));
    r.a.modal = None;
    let g = draw_big(&mut r.a, &r.cx, 1280, 720, 1.0);
    for label in ["TLS", "Address", "Relay", "Integrate", "Finalize"] {
        assert!(g.hits.iter().any(|h| h.label.contains(label)), "{label}");
    }
    r.hub.stop_node();
    other.stop_node();
}

#[test]
fn a_waiting_task_from_another_member_is_accepted_or_declined_with_a_card() {
    use iyakki_shared::fleetview::AssignBody;
    let mut r = rig("flt-inbox");
    let (other, _addr) = remote("flt-inbox-other");
    let fid = r.hub.create_fleet("lab", "alpha").unwrap();
    r.hub.start_node("127.0.0.1:0", Arc::new(FixedSource(hb(1)))).unwrap();
    let (code, _) = r.hub.offer_code(&fid).unwrap();
    let my_addr = r.hub.listening().unwrap().to_string();
    let t = other.join_start(&my_addr, &code, "bravo").unwrap();
    r.hub.approve_join(&r.hub.status().requests[0].id).unwrap();
    other.poll_pending();
    other.join_accept(&t).unwrap();
    r.hub.settle();
    other.heartbeat_tick(&FixedSource(hb(2)));
    r.hub.heartbeat_tick(&FixedSource(hb(1)));
    let body = AssignBody { id: "ab".repeat(16), title: "speed up the parser".into(), goal: "profile and fix".into(), base_branch: "main".into(), base_commit: String::new(), rows: vec![], context_digest: String::new(), budget_tokens: 1000, model_class: String::new(), after: vec![] };
    assert_eq!(other.assign(&fid, "alpha", &body).unwrap(), "waiting");
    select(&mut r, |x| matches!(x.kind, Kind::Inbox { waiting: true }));
    key(&mut r, 'y');
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::Fleet(FleetAct::Decide(_, true)), _))));
    yes(&mut r);
    other_wait_for(&other, "accepted");
    r.hub.stop_node();
    other.stop_node();
}

fn other_wait_for(h: &Arc<Hub>, state: &str) {
    let t0 = std::time::Instant::now();
    loop {
        h.settle();
        if h.status().outbox.iter().any(|o| o.state == state) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "the answer did not come back");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_page_fits_the_window_from_a_phone_to_a_large_screen_and_at_the_largest_text() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for big in [1.0f32, 2.0] {
            let mut r = rig(&format!("flt-geo-{w}-{}", big as u32));
            let fid = r.hub.create_fleet("a-fleet-with-a-rather-long-name", "alpha-with-a-long-name").unwrap();
            let _ = fid;
            r.hub.heartbeat_tick(&FixedSource(hb(1)));
            let g = draw_big(&mut r.a, &r.cx, w, h, big);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                let q = hit.rect;
                assert!(q.x >= -0.5 && q.x + q.w <= w as f32 + 0.5, "{w}x{h} x{big}: {:?} {q:?}", hit.label);
            }
            let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.rect.h < 90.0 * big).collect();
            assert!(buttons.len() >= 2, "{w}x{h} x{big}: {} buttons", buttons.len());
            let o = |p: R, q: R| p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
            for (i, b) in buttons.iter().enumerate() {
                assert!(buttons[i + 1..].iter().all(|n| !o(b.rect, n.rect)), "{w}x{h} x{big}: {} overlaps another button", b.label);
            }
        }
    }
}
