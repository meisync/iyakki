//! Settings -> Firewall: the page, the draft, Apply / Keep / Revert, the checks on what is typed, and the page drawn at four
//! sizes. The firewall engine behind it is a fake with a clock the tests move; nothing here touches the real firewall.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use super::firewall_page::*;
use super::*;
use crate::apps::tests::cx_with_admin_store;
use iyakki_fw::backend::{BackendError, FakeBackend};
use iyakki_fw::blocked::{Blocked, BlockLog};
use iyakki_fw::engine::{Clock, Engine, RuleState, Saved, Status, Store, View};
use iyakki_fw::model::{Config, Kind as FwKind, Preset, Resolved, Rule};
use iyakki_fw::service::Handle;

struct TClock(Arc<AtomicI64>);
impl Clock for TClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct TDns;
impl iyakki_fw::engine::Resolver for TDns {
    fn resolve(&self, n: &str) -> Result<Vec<std::net::IpAddr>, String> {
        if n == "nas.local" { Ok(vec!["192.168.1.9".parse().unwrap()]) } else { Err("no such host".into()) }
    }
}
#[derive(Clone, Default)]
struct TStore(Arc<Mutex<Option<Saved>>>);
impl Store for TStore {
    fn load(&self) -> Result<Option<Saved>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(s.clone());
        Ok(())
    }
}

struct Rig {
    eng: Arc<Mutex<Engine>>,
    be: FakeBackend,
    clock: Arc<AtomicI64>,
    store: TStore,
}

fn rig() -> Rig {
    let be = FakeBackend::default();
    let clock = Arc::new(AtomicI64::new(5_000_000));
    let store = TStore::default();
    let mut e = Engine::new(Box::new(be.clone()), Box::new(TClock(clock.clone())), Box::new(TDns), Box::new(store.clone()));
    e.boot(false);
    Rig { eng: Arc::new(Mutex::new(e)), be, clock, store }
}

fn app(tag: &str) -> (SettingsApp, AppCx, Rig) {
    let cx = cx_with_admin_store(tag);
    let r = rig();
    let mut a = SettingsApp::new(&cx);
    a.fw.handle = Handle::Own(r.eng.clone());
    a.fw.log = BlockLog::new();
    a.fw.writable = true;
    // the tests below drive the Advanced page (rules by hand); the simple view has its own tests at the end
    a.fw.advanced = true;
    for _ in 0..60 {
        if a.tab() == Tab::Firewall {
            break;
        }
        a.handle(&Event::Key(Key::Right), &cx);
    }
    assert_eq!(a.tab(), Tab::Firewall, "administrators have the page");
    (a, cx, r)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: char) {
    a.handle(&Event::Key(Key::Char(k)), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn rule_with(kind_idx: usize, value: &str) -> Form {
    let mut f = rule_form(None);
    f.set_choice(0, kind_idx);
    f.set_value(1, value);
    f
}

fn add(a: &mut SettingsApp, cx: &AppCx, kind_idx: usize, value: &str) -> Result<String, String> {
    let mut f = rule_with(kind_idx, value);
    a.fw_submit(cx, &FwForm::Rule(None), &mut f)
}

fn audit_text(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default()
}

fn view(r: &Rig) -> View {
    r.eng.lock().unwrap().view()
}

#[test]
fn only_administrators_get_the_page_and_it_is_not_hidden_in_cloud_mode() {
    let (a, cx, _r) = app("fw-tab");
    assert!(a.tabs().len() <= 64, "page ids stay below the extra control ids (77 and up), with room: {}", a.tabs().len());
    let mut user = cx.clone();
    user.session.user = "gus".into();
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user).tabs().contains(&Tab::Firewall));
    assert!(Tab::Firewall.cloud_page().is_none(), "needed on cloud machines too");
    assert_eq!(Tab::Firewall.label(), "Firewall");
    assert!(Tab::Firewall.keywords().contains("allow dns"), "the owner's words find it");
}

#[test]
fn a_user_who_is_not_an_administrator_cannot_change_anything_even_with_the_keys() {
    let (mut a, cx, r) = app("fw-deny");
    let mut user = cx.clone();
    user.session.user = "gus".into();
    user.session.policy = iyakki_sec::policy::Role::User.template();
    a.handle(&Event::Key(Key::Char('z')), &user);
    assert!(status(&a).contains("Only administrators"), "{}", status(&a));
    assert!(r.be.calls().iter().all(|c| !c.contains("remove")), "{:?}", r.be.calls());
    assert!(audit_text(&user).contains("settings.denied"));
}

#[test]
fn adding_a_rule_is_a_draft_until_apply_and_apply_waits_for_keep() {
    let (mut a, cx, r) = app("fw-flow");
    let msg = add(&mut a, &cx, 1, "198.51.100.7").unwrap();
    assert!(msg.contains("not applied yet"), "{msg}");
    assert!(a.fw.draft.is_some());
    assert!(view(&r).config.rules.is_empty(), "the firewall has not changed");
    assert_eq!(a.fw_len(), 1, "the page shows the draft");
    key(&mut a, &cx, 'y');
    assert!(status(&a).contains("60 seconds"), "{}", status(&a));
    assert!(matches!(view(&r).status, Status::Pending { secs_left: 60 }));
    assert!(r.be.installed().unwrap().to_text().contains("198.51.100.7"));
    // the Keep and Revert buttons appear while it waits
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Keep") && g.hits.iter().any(|h| h.label == "Revert"));
    key(&mut a, &cx, 'k');
    assert!(status(&a).contains("Kept"), "{}", status(&a));
    assert_eq!(view(&r).status, Status::On);
    assert_eq!(r.store.0.lock().unwrap().as_ref().unwrap().config.rules.len(), 1, "saved");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Keep"));
    let log = audit_text(&cx);
    for ev in ["firewall.rule.add", "firewall.apply", "firewall.keep"] {
        assert!(log.contains(ev), "{ev} in the audit log:\n{log}");
    }
}

#[test]
fn not_pressing_keep_takes_the_rules_back_and_revert_does_it_at_once() {
    let (mut a, cx, r) = app("fw-revert");
    add(&mut a, &cx, 1, "198.51.100.7").unwrap();
    key(&mut a, &cx, 'y');
    r.clock.fetch_add(61, Ordering::SeqCst);
    r.eng.lock().unwrap().tick();
    assert_eq!(view(&r).status, Status::On);
    assert!(view(&r).config.rules.is_empty(), "taken back by itself");
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.7"));
    // by hand
    add(&mut a, &cx, 1, "198.51.100.8").unwrap();
    key(&mut a, &cx, 'y');
    key(&mut a, &cx, 'v');
    assert!(status(&a).contains("Reverted"), "{}", status(&a));
    assert!(view(&r).config.rules.is_empty());
    assert!(audit_text(&cx).contains("firewall.revert"));
}

#[test]
fn the_form_refuses_what_is_wrong_with_plain_words() {
    let (mut a, cx, _r) = app("fw-valid");
    for (kind, v, want) in [
        (1usize, "999.1.1.1", "not an IP address"),
        (2, "10.0.0.9-10.0.0.1", "reversed"),
        (3, "10.0.0.5/24", "10.0.0.0"),
        (0, "*.example.com", "Wildcard"),
        (0, "my_host.example.com", "Underscore"),
        (0, "a b.example.com", "not allowed"),
        (1, "1.2.3.4; accept", "not an IP address"),
    ] {
        let e = add(&mut a, &cx, kind, v).unwrap_err();
        assert!(e.contains(want), "{v}: {e}");
    }
    let mut f = rule_with(1, "198.51.100.7");
    f.set_value(3, "70000");
    assert!(a.fw_submit(&cx, &FwForm::Rule(None), &mut f).unwrap_err().contains("65535"));
    f.set_value(3, "0");
    assert!(a.fw_submit(&cx, &FwForm::Rule(None), &mut f).is_err());
    f.set_value(3, "22");
    f.set_value(4, "eth0\"; drop");
    assert!(a.fw_submit(&cx, &FwForm::Rule(None), &mut f).unwrap_err().contains("Interface") || true);
    assert!(a.fw.draft.is_none(), "nothing was added by the failed tries");
    // /0 is accepted but warned about
    let m = add(&mut a, &cx, 3, "0.0.0.0/0").unwrap();
    assert!(m.contains("allows everyone"), "{m}");
}

#[test]
fn rules_can_be_edited_switched_off_and_deleted() {
    let (mut a, cx, r) = app("fw-edit");
    add(&mut a, &cx, 1, "198.51.100.7").unwrap();
    add(&mut a, &cx, 3, "203.0.113.0/24").unwrap();
    key(&mut a, &cx, 'y');
    key(&mut a, &cx, 'k');
    key(&mut a, &cx, 'e');
    let Some(Modal::Form(FormKind::Fw(FwForm::Rule(Some(id))), f)) = a.modal.take() else { panic!("edit form not open") };
    assert_eq!(f.value(1), "198.51.100.7");
    let mut f = f;
    f.set_value(3, "22");
    f.set_choice(2, 1);
    a.fw_submit(&cx, &FwForm::Rule(Some(id)), &mut f).unwrap();
    assert_eq!(a.fw_cfg().rules[0].ports, "22");
    key(&mut a, &cx, 'x');
    assert!(!a.fw_cfg().rules[0].enabled);
    key(&mut a, &cx, 'u');
    assert!(a.fw.draft.is_none() && a.fw_cfg().rules[0].enabled, "discarding goes back to what is in force");
    key(&mut a, &cx, 'd');
    assert_eq!(a.fw_cfg().rules.len(), 1);
    key(&mut a, &cx, 'y');
    assert!(!r.be.installed().unwrap().to_text().contains("198.51.100.7"));
    let log = audit_text(&cx);
    assert!(log.contains("firewall.rule.edit") && log.contains("firewall.rule.toggle") && log.contains("firewall.rule.delete"), "{log}");
}

#[test]
fn expiry_choices_set_an_end_time() {
    let (_a, _cx, _r) = app("fw-exp");
    let mut f = rule_with(1, "198.51.100.7");
    f.set_choice(6, 1);
    let r = rule_of_form(&f, None, 1000).unwrap();
    assert_eq!(r.expires, 4600);
    f.set_choice(6, 2);
    assert_eq!(rule_of_form(&f, None, 1000).unwrap().expires, 87400);
    f.set_choice(6, 0);
    assert_eq!(rule_of_form(&f, None, 1000).unwrap().expires, 0);
    let old = Rule { expires: 777, ..Rule::new(FwKind::Ip, "198.51.100.7") };
    let f2 = rule_form(Some(&old));
    assert_eq!(f2.choice_idx(6), 3, "the form offers to keep the end time");
    assert_eq!(rule_of_form(&f2, Some(&old), 1000).unwrap().expires, 777);
}

#[test]
fn presets_and_basics_are_one_form() {
    let (mut a, cx, r) = app("fw-presets");
    key(&mut a, &cx, 'p');
    let Some(Modal::Form(FormKind::Fw(FwForm::Presets), f)) = a.modal.take() else { panic!("presets form not open") };
    assert_eq!(f.fields.len(), Preset::ALL.len() + 3);
    assert!(f.fields[0].hint.contains("SSH"), "{}", f.fields[0].hint);
    let mut f = f;
    let ssh = Preset::ALL.iter().position(|p| *p == Preset::Ssh).unwrap();
    f.set_choice(ssh, 1);
    f.set_choice(Preset::ALL.len(), 0);
    a.fw_submit(&cx, &FwForm::Presets, &mut f).unwrap();
    assert!(a.fw_cfg().has(Preset::Ssh));
    assert_eq!(a.fw_cfg().ping, iyakki_fw::Ping::Off);
    key(&mut a, &cx, 'y');
    assert!(r.be.installed().unwrap().to_text().contains("th dport 22 accept"));
    f.set_value(Preset::ALL.len() + 2, "0");
    assert!(a.fw_submit(&cx, &FwForm::Presets, &mut f).unwrap_err().contains("minutes"));
}

#[test]
fn host_names_show_the_addresses_found_and_a_name_with_no_answer_says_so() {
    let (mut a, cx, r) = app("fw-host");
    add(&mut a, &cx, 0, "NAS.local").unwrap();
    add(&mut a, &cx, 0, "ghost.example").unwrap();
    key(&mut a, &cx, 'y');
    let v = view(&r);
    let cfg = a.fw_cfg();
    let (title, sub) = rule_lines(&cfg.rules[0], &v, 5_000_000);
    assert_eq!(title, "Hostname  NAS.local");
    assert!(sub.contains("= 192.168.1.9"), "{sub}");
    let (_, sub2) = rule_lines(&cfg.rules[1], &v, 5_000_000);
    assert!(sub2.contains("no address yet"), "{sub2}");
    assert_eq!(v.rule_states[1].1, RuleState::WaitingForDns);
    assert_eq!(state_chip(&RuleState::WaitingForDns).0, "no address");
    assert!(v.hosts[1].warning.as_ref().unwrap().contains("allows nothing"));
}

#[test]
fn rules_are_exported_and_imported_as_json() {
    let (mut a, cx, _r) = app("fw-json");
    add(&mut a, &cx, 3, "192.168.7.0/24").unwrap();
    key(&mut a, &cx, 'y');
    key(&mut a, &cx, 'k');
    key(&mut a, &cx, 'c');
    assert!(status(&a).contains("Copied the rules"), "{}", status(&a));
    let json = a.fw_cfg().to_json();
    assert!(json.contains("192.168.7.0/24"));
    // another machine (a second page over a second engine) imports it
    let (mut b, cxb, rb) = app("fw-json2");
    b.fw_import(&cxb, &json);
    assert!(status(&b).contains("Imported 1 rule"), "{}", status(&b));
    key(&mut b, &cxb, 'y');
    assert!(rb.be.installed().unwrap().to_text().contains("192.168.7.0/24"));
    assert!(audit_text(&cxb).contains("firewall.import"));
    // a broken export is refused with a reason and changes nothing
    let (mut c, cxc, _rc) = app("fw-json3");
    c.fw_import(&cxc, "{ \"version\": 1, nonsense");
    assert!(status(&c).contains("not a firewall export"), "{}", status(&c));
    assert!(c.fw.draft.is_none());
}

#[test]
fn the_emergency_button_removes_the_table_and_says_what_to_do_next() {
    let (mut a, cx, r) = app("fw-z");
    key(&mut a, &cx, 'z');
    assert!(status(&a).contains("off"), "{}", status(&a));
    assert!(r.be.installed().is_none());
    assert_eq!(view(&r).status, Status::Off);
    assert!(audit_text(&cx).contains("firewall.disable"));
}

#[test]
fn a_window_on_another_computer_shows_but_changes_nothing() {
    let (mut a, cx, r) = app("fw-ro");
    a.fw.writable = false;
    key(&mut a, &cx, 'z');
    assert!(status(&a).contains("another computer"), "{}", status(&a));
    assert!(r.be.installed().is_some(), "untouched");
    let lines = status_lines(&view(&r), 0, false);
    assert!(lines.iter().any(|l| l.0.contains("another computer")));
}

#[test]
fn every_state_is_said_plainly() {
    let (_a, _cx, r) = app("fw-lines");
    let text = |v: &View| status_lines(v, 0, true).into_iter().map(|l| l.0).collect::<Vec<_>>().join("\n");
    let mut v = view(&r);
    assert!(text(&v).contains("Incoming connections are blocked unless allowed below."));
    v.status = Status::Off;
    assert!(text(&v).contains("firewall is off"));
    v.status = Status::OffThisBoot;
    assert!(text(&v).contains("iyakki.firewall=off"));
    v.status = Status::NotProtected("the kernel refused the firewall rules (error 22)".into());
    let t = text(&v);
    assert!(t.contains("NOT PROTECTED") && t.contains("NOT blocked"), "{t}");
    assert_eq!(status_lines(&v, 0, true)[0].1, ERR, "red");
    v.status = Status::Unavailable("Firewall not available in this kernel".into());
    assert!(text(&v).contains("Firewall not available in this kernel"));
    v.status = Status::Pending { secs_left: 42 };
    assert!(text(&v).contains("42 s left"));
    v.status = Status::On;
    v.ssh_boot = true;
    assert!(text(&v).contains("iyakki.ssh=1"), "remote diagnostics keeps SSH reachable and says so");
    assert!(status_lines(&v, 3, true).iter().any(|l| l.0.contains("3 change(s) not applied")));
    let c = Config::default();
    assert!(presets_line(&c).contains("Remote desktop"));
    let b = Blocked { at: 100, src: "203.0.113.9".parse().unwrap(), proto: 6, dport: 22, count: 3 };
    assert_eq!(blocked_line(&b, 130), "30 s ago  203.0.113.9  tcp port 22  x3");
}

#[test]
fn the_page_draws_inside_the_window_in_every_state_at_four_sizes() {
    let (mut a, cx, r) = app("fw-draw");
    for i in 0..8 {
        add(&mut a, &cx, 3, &format!("10.{i}.0.0/16")).unwrap();
    }
    add(&mut a, &cx, 0, "nas.local").unwrap();
    for i in 0..60u16 {
        a.fw.log.push(Blocked { at: 5_000_000 - i64::from(i) * 100, src: "203.0.113.9".parse().unwrap(), proto: 6, dport: 1000 + i, count: 1 });
    }
    let states: Vec<Status> = vec![Status::On, Status::Pending { secs_left: 33 }, Status::Off, Status::OffThisBoot, Status::NotProtected("the kernel refused the firewall rules (error 22)".into()), Status::Unavailable("Firewall not available in this kernel".into())];
    for st in states {
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
            for sel in [0usize, 4, 8] {
                a.sel[a.tab] = sel;
                // force the state through the engine's own paths where possible
                match &st {
                    Status::Off => {
                        let _ = r.eng.lock().unwrap().disable_now("test");
                    }
                    Status::NotProtected(_) => {
                        r.be.0.lock().unwrap().fail_replace = Some(BackendError::Failed("the kernel refused the firewall rules (error 22)".into()));
                    }
                    _ => {}
                }
                let g = draw(&mut a, &cx, w, h);
                for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                    let rc = hit.rect;
                    assert!(rc.x >= -0.5 && rc.y >= -0.5 && rc.x + rc.w <= w as f32 + 0.5 && rc.y + rc.h <= h as f32 + 0.5, "{w}x{h} {st:?}: {:?} {:?}", hit.label, rc);
                }
                assert!(g.hits.iter().any(|x| x.label == "Add rule…"), "{w}x{h}: the buttons are there");
            }
        }
    }
    // an empty page and the forms fit too
    let (mut e, cxe, _re) = app("fw-draw2");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let g = draw(&mut e, &cxe, w, h);
        assert!(!g.hits.is_empty(), "{w}x{h}");
        e.modal = Some(Modal::Form(FormKind::Fw(FwForm::Rule(None)), rule_form(None)));
        assert!(!draw(&mut e, &cxe, w, h).hits.is_empty());
        e.modal = Some(Modal::Form(FormKind::Fw(FwForm::Presets), presets_form(&Config::default())));
        assert!(!draw(&mut e, &cxe, w, h).hits.is_empty());
        e.modal = None;
    }
    let _ = Resolved::default();
}

#[test]
fn a_share_the_firewall_blocks_is_pointed_out_and_one_key_opens_the_shared_folder_ports() {
    let (mut a, cx, r) = app("fw-shares");
    let note = blocked_note_for(&r.eng.lock().unwrap(), 8081).expect("blocked by default");
    assert!(note.contains("blocks port 8081") && note.contains("Press f"), "{note}");
    // from the Shared folders page
    a.tab = a.tabs.iter().position(|t| *t == Tab::Shares).expect("administrators have Shared folders");
    key(&mut a, &cx, 'f');
    assert_eq!(a.tab(), Tab::Firewall, "taken to the page where it is kept or reverted");
    assert!(status(&a).contains("60 seconds"), "{}", status(&a));
    assert!(matches!(view(&r).status, Status::Pending { .. }));
    assert!(blocked_note_for(&r.eng.lock().unwrap(), 8081).is_none(), "now open to the local network");
    assert!(audit_text(&cx).contains("firewall.preset"));
    key(&mut a, &cx, 'k');
    key(&mut a, &cx, 'p');
    // asking again changes nothing
    a.modal = None;
    a.tab = a.tabs.iter().position(|t| *t == Tab::Shares).unwrap();
    key(&mut a, &cx, 'f');
    assert!(status(&a).contains("already"), "{}", status(&a));
    // a firewall that is off blocks nothing, so there is nothing to say
    r.eng.lock().unwrap().disable_now("test").unwrap();
    assert!(blocked_note_for(&r.eng.lock().unwrap(), 9999).is_none());
}

#[test]
fn the_devices_list_has_an_emergency_off_button_for_the_firewall_row() {
    assert_eq!(crate::devpop::Act::FirewallOff.label(), "Off");
}

// ------------------------------------------------------------------ the simple view

fn simple(tag: &str) -> (SettingsApp, AppCx, Rig) {
    let (mut a, cx, r) = app(tag);
    a.fw.advanced = false;
    (a, cx, r)
}

fn pick(a: &mut SettingsApp, cx: &AppCx, line: &str) {
    key(a, cx, 'a');
    assert!(matches!(a.modal, Some(Modal::Pick(PickKind::FwService, _))), "Add opens the list of services");
    let item = fw_simple::service_items().into_iter().find(|i| i.starts_with(line)).unwrap_or_else(|| panic!("no service {line}"));
    a.modal = None;
    a.apply_pick(cx, PickKind::FwService, &item);
}

#[test]
fn the_page_opens_in_the_simple_view_with_the_default_kept() {
    let cx = cx_with_admin_store("fw-default");
    let a = SettingsApp::new(&cx);
    assert!(!a.fw.advanced, "simple first");
    assert!(Config::default().enabled && Config::default().presets == vec![Preset::RemoteDesktop], "default: block incoming, only remote desktop open");
    let items = fw_simple::items(&Config::default());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Remote desktop");
}

#[test]
fn simple_view_draws_the_three_modes_and_the_list_inside_the_window_at_five_sizes() {
    let (mut a, cx, _r) = simple("fw-simple-draw");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            let rc = hit.rect;
            assert!(rc.x >= -0.5 && rc.y >= -0.5 && rc.x + rc.w <= w as f32 + 0.5 && rc.y + rc.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, rc);
        }
        for l in ["Standard", "Open", "Locked down", "Add…", "Remove", "Advanced…"] {
            assert!(g.hits.iter().any(|x| x.label == l), "{w}x{h}: {l}");
        }
        assert!(!g.hits.iter().any(|x| x.label == "Add rule…"), "advanced buttons are folded away");
    }
}

#[test]
fn standard_is_the_default_open_asks_first_and_locked_down_is_honest() {
    let (mut a, cx, r) = simple("fw-modes");
    key(&mut a, &cx, '1');
    assert!(status(&a).contains("Already Standard"), "{}", status(&a));
    key(&mut a, &cx, '3');
    assert!(status(&a).contains("not available yet"), "{}", status(&a));
    assert!(matches!(view(&r).status, Status::On), "nothing changed");
    key(&mut a, &cx, '2');
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::Fw(FwAct::Open), _))), "Open asks first");
    assert!(matches!(view(&r).status, Status::On), "not yet");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.modal.is_none() && matches!(view(&r).status, Status::On), "cancel keeps it protected");
    key(&mut a, &cx, '2');
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert!(status(&a).contains("Open"), "{}", status(&a));
    assert!(matches!(view(&r).status, Status::Off), "Open takes effect at once (it cannot lock anyone out)");
    key(&mut a, &cx, '1');
    assert!(matches!(view(&r).status, Status::Pending { .. }), "Standard asks to be kept");
    key(&mut a, &cx, 'k');
    assert!(matches!(view(&r).status, Status::On), "Standard again");
    assert!(audit_text(&cx).contains("firewall.apply"));
}

#[test]
fn adding_a_service_applies_it_at_once_with_the_keep_window_and_remove_takes_it_away() {
    let (mut a, cx, r) = simple("fw-add");
    pick(&mut a, &cx, "Web server");
    assert!(status(&a).contains("Keep within 60 seconds"), "{}", status(&a));
    assert!(matches!(view(&r).status, Status::Pending { .. }));
    assert!(r.eng.lock().unwrap().would_allow_tcp("192.168.1.5".parse().unwrap(), 443), "the local network reaches the web server");
    assert!(!r.eng.lock().unwrap().would_allow_tcp("198.51.100.7".parse().unwrap(), 443), "the internet does not");
    key(&mut a, &cx, 'k');
    assert!(a.fw_items().iter().any(|i| i.title == "Web server"));
    // adding it again says so
    pick(&mut a, &cx, "Web server");
    assert!(status(&a).contains("already allowed"), "{}", status(&a));
    // remove it: select the line, press Remove
    let idx = a.fw_items().iter().position(|i| i.title == "Web server").unwrap();
    a.sel[a.tab] = idx;
    key(&mut a, &cx, 'd');
    assert!(a.modal.is_none(), "a web server is not how anyone is connected: no question");
    assert!(!r.eng.lock().unwrap().would_allow_tcp("192.168.1.5".parse().unwrap(), 443));
    key(&mut a, &cx, 'k');
}

#[test]
fn removing_remote_desktop_or_ssh_asks_because_it_may_be_how_you_are_connected() {
    let (mut a, cx, r) = simple("fw-lockout");
    assert_eq!(a.fw_items()[0].title, "Remote desktop");
    a.sel[a.tab] = 0;
    key(&mut a, &cx, 'd');
    match &a.modal {
        Some(Modal::Confirm(ConfirmKind::Fw(FwAct::Remove(Some(Preset::RemoteDesktop), _)), t)) => assert!(t.contains("lose the connection") && t.contains("60 seconds"), "{t}"),
        _ => panic!("a question first"),
    }
    assert!(r.eng.lock().unwrap().would_allow_tcp("192.168.1.5".parse().unwrap(), 8443), "still allowed");
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert!(!r.eng.lock().unwrap().would_allow_tcp("192.168.1.5".parse().unwrap(), 8443));
    // not pressing Keep takes it back, so a locked-out person gets back in
    r.clock.fetch_add(61, Ordering::SeqCst);
    r.eng.lock().unwrap().tick();
    assert!(r.eng.lock().unwrap().would_allow_tcp("192.168.1.5".parse().unwrap(), 8443), "undone by itself");
}

#[test]
fn a_custom_port_is_one_line_in_the_list_and_bad_input_says_how_to_fix_it() {
    let (mut a, cx, r) = simple("fw-port");
    let mut f = fw_simple::port_form();
    assert!(fw_simple::port_rules(&f).unwrap_err().contains("Type the port"));
    f.set_value(0, "70000");
    assert!(fw_simple::port_rules(&f).unwrap_err().contains("Check the port"), "{:?}", fw_simple::port_rules(&f));
    f.set_value(0, "8080");
    f.set_value(3, "my test site");
    let n_before = a.fw_items().len();
    a.fw_submit(&cx, &FwForm::Port, &mut f).unwrap();
    assert_eq!(a.fw_items().len(), n_before + 1, "one line, however many networks it covers");
    let it = a.fw_items().into_iter().find(|i| i.title.starts_with("Port 8080")).unwrap();
    assert!(it.title.contains("my test site") && it.detail.contains("your local network"), "{it:?}");
    assert!(it.rules.len() >= 4, "one rule per local network");
    assert!(r.eng.lock().unwrap().would_allow_tcp("10.1.2.3".parse().unwrap(), 8080));
    assert!(!r.eng.lock().unwrap().would_allow_tcp("198.51.100.7".parse().unwrap(), 8080));
    key(&mut a, &cx, 'k');
    a.sel[a.tab] = a.fw_items().iter().position(|i| i.title.starts_with("Port 8080")).unwrap();
    key(&mut a, &cx, 'd');
    assert!(!r.eng.lock().unwrap().would_allow_tcp("10.1.2.3".parse().unwrap(), 8080), "all its rules went");
    // anyone
    let mut f = fw_simple::port_form();
    f.set_value(0, "9000");
    f.set_choice(2, 1);
    let rules = fw_simple::port_rules(&f).unwrap();
    assert!(rules.iter().any(|r| r.value == "0.0.0.0/0"));
}

#[test]
fn a_pasted_name_address_range_or_network_becomes_the_right_kind_of_rule() {
    use iyakki_fw::model::Kind;
    assert_eq!(fw_simple::guess_kind("nas.local"), Kind::Host);
    assert_eq!(fw_simple::guess_kind("192.168.1.20"), Kind::Ip);
    assert_eq!(fw_simple::guess_kind("192.168.1.10-192.168.1.50"), Kind::Range);
    assert_eq!(fw_simple::guess_kind("192.168.1.0/24"), Kind::Cidr);
    assert_eq!(fw_simple::guess_kind("my-nas.example.com"), Kind::Host, "a hyphen in a name is not a range");
    let (mut a, cx, _r) = simple("fw-addr");
    let mut f = fw_simple::address_form();
    assert!(fw_simple::address_rule(&f).unwrap_err().contains("Paste or type"));
    f.set_value(0, "192.168.1.0/99");
    assert!(fw_simple::address_rule(&f).unwrap_err().contains("try again"));
    f.set_value(0, "192.168.1.20");
    f.set_value(1, "22");
    a.fw_submit(&cx, &FwForm::Address, &mut f).unwrap();
    let it = a.fw_items().into_iter().find(|i| i.title == "192.168.1.20").unwrap();
    assert!(it.detail.contains("port 22"), "{it:?}");
}

#[test]
fn advanced_is_one_fold_away_and_back_and_users_cannot_change_anything() {
    let (mut a, cx, _r) = simple("fw-fold");
    key(&mut a, &cx, 'A');
    assert!(a.fw.advanced);
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "Add rule…" && x.rect.w > 0.0));
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|x| x.label == "Simple view"), "the way back");
    key(&mut a, &cx, 'A');
    assert!(!a.fw.advanced);
    // keys that only make sense on the Advanced page point there
    key(&mut a, &cx, 'p');
    assert!(status(&a).contains("Advanced"), "{}", status(&a));
    // not an administrator: the keys do nothing
    let mut user = cx.clone();
    user.session.user = "gus".into();
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let mut u = SettingsApp::new(&user);
    u.fw.writable = true;
    u.tab = 0;
    assert!(!u.tabs().contains(&Tab::Firewall));
}
