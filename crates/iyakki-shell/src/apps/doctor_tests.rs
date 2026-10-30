//! The DOCTOR tile on a made-up machine: the list, the keys, the fixes, the slow checks arriving later, the share preview and the file
//! it saves. Nothing here reads the real machine except the one test that says so.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use iyakki_agent::diagshare::Inputs;
use iyakki_agent::doctor::{FailedService, Facts, Finding};
use iyakki_agent::enginelog::Health;
use iyakki_sec::policy::{Policy, Role};

use super::doctor::*;
use super::doctor_facts::SlowOut;
use super::tests::cx_with_admin_store;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

const GIB: u64 = 1024 * 1024 * 1024;

fn sick() -> Facts {
    Facts {
        now: 1_800_000_000,
        uptime_secs: 5000,
        cluster_installed: true,
        cluster_state: "stopped".into(),
        cluster_autostart: true,
        dev_on: true,
        dev_until: 1_800_000_000 + 1500,
        dev_port: 7979,
        dev_firewall_hint: Some("the firewall blocks the developer port 7979 from 192.0.2.7: Allow".into()),
        vault_locked: true,
        vault_waiting: vec!["ClickHouse".into()],
        engine: Health { crashed: 1, camera_trouble: 0, mic_trouble: 0, last: "the helper closed the connection".into() },
        sound_cards: 1,
        sound_chosen_present: true,
        disk_total: 100 * GIB,
        disk_free: 80 * GIB,
        route: true,
        clock_synced: true,
        network_time_on: true,
        ..Facts::default()
    }
}

fn healthy() -> Facts {
    Facts { cluster_installed: false, dev_on: false, dev_firewall_hint: None, vault_locked: false, vault_waiting: vec![], engine: Health::default(), ..sick() }
}

fn tmp(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-doctor-{}-{tag}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SECRET: &str = "sk-abcdefghijklmnopqrstuvwxyz0123456789";

fn parts(facts: Facts, slow: SlowOut, home: PathBuf) -> Parts {
    Parts {
        gather: Arc::new(move |_| facts.clone()),
        slow: Arc::new(move |_| slow.clone()),
        inputs: Arc::new(|_, found: &[Finding]| Inputs {
            version: vec!["IYAKKI 0.1.0-alpha.2".into()],
            services: vec!["Clock ready".into()],
            hardware: "CPU Intel".into(),
            doctor: iyakki_agent::doctor::report_lines(found, true),
            engine_log: format!("100 helper: v4l2src busy\n101 key {SECRET} seen\n102 password=hunter2hunter2 at https://shop.example.org/c?session=abc123"),
            boot_log: "booted".into(),
            switches: vec![],
            checkup: vec![],
        }),
        home: Arc::new(move |_| home.clone()),
        now: Arc::new(|| 1_800_000_000),
    }
}

struct Fx {
    app: DoctorApp,
    cx: AppCx,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
    home: PathBuf,
}

fn fixture_with(tag: &str, role: Role, facts: Facts, slow: SlowOut, w: u32, h: u32, s: f32) -> Fx {
    let mut cx = cx_with_admin_store(&format!("doctor-{tag}"));
    cx.session.role = role;
    cx.session.policy = Policy::resolve(role, &Default::default());
    let home = tmp(tag);
    let app = DoctorApp::with_parts(&cx, parts(facts, slow, home.clone()));
    let mut f = Fx { app, cx, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0, home };
    f.settle();
    f.draw();
    f
}

fn fixture(tag: &str) -> Fx {
    fixture_with(tag, Role::Admin, sick(), SlowOut::default(), 1280, 720, 1.0)
}

impl Fx {
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    /// Wait for the slow thread.
    fn settle(&mut self) {
        for _ in 0..400 {
            if !self.app.checking {
                return;
            }
            self.app.wants_redraw();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the slow checks never finished");
    }
    fn ev(&mut self, e: Event) -> Flow {
        let r = self.app.handle(&e, &self.cx);
        self.draw();
        r
    }
    fn key(&mut self, k: Key) -> Flow {
        self.ev(Event::Key(k))
    }
    fn chr(&mut self, c: char) {
        self.key(Key::Char(c));
    }
    fn ids(&self) -> Vec<&'static str> {
        self.app.found.iter().map(|f| f.id).collect()
    }
    fn has_hit(&self, e: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == e)
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn audit(&self) -> String {
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default()
    }
}

fn assert_hits_sane(f: &Fx, what: &str) {
    let a = f.area;
    for h in &f.g.hits {
        let r = h.rect;
        assert!(r.w > 0.0 && r.h > 0.0, "{what}: {:?} is empty", h.label);
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: {:?} {:?} leaves {:?}", h.label, r, a);
    }
    for (i, p) in f.g.hits.iter().enumerate() {
        for q in &f.g.hits[i + 1..] {
            let (x0, y0) = (p.rect.x.max(q.rect.x), p.rect.y.max(q.rect.y));
            let (x1, y1) = ((p.rect.x + p.rect.w).min(q.rect.x + q.rect.w), (p.rect.y + p.rect.h).min(q.rect.y + q.rect.h));
            if x1 - x0 > 1.0 && y1 - y0 > 1.0 {
                let holds = |o: &R, n: &R| n.x >= o.x - 0.5 && n.y >= o.y - 0.5 && n.x + n.w <= o.x + o.w + 0.5 && n.y + n.h <= o.y + o.h + 0.5;
                assert!(holds(&p.rect, &q.rect) || holds(&q.rect, &p.rect), "{what}: {:?} {:?} overlaps {:?} {:?}", p.label, p.rect, q.label, q.rect);
            }
        }
    }
    assert!(f.g.rgba().iter().any(|b| *b != 0), "{what}: nothing drawn");
}

const SIZES: [(u32, u32, f32); 6] = [(360, 740, 1.0), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0), (2880, 1800, 2.25)];

#[test]
fn the_tile_lists_what_is_wrong_worst_first_each_with_its_fix_button() {
    let f = fixture("list");
    assert_eq!(f.ids(), ["cluster-stopped", "firewall-dev-port", "dev-mode-ending", "vault-locked", "engine-crashed"]);
    assert_eq!(f.app.title(), "Doctor");
    assert!(f.app.hint().contains("Enter fix") && !f.app.help().is_empty());
    assert_eq!(f.g.hits.iter().filter(|h| h.label == "finding").count(), 5);
    for i in 0..5 {
        assert!(f.has_hit(&Event::Tab(T_FIX + i)), "finding {i} has its fix button");
    }
    let t = f.text();
    assert!(t.contains("5 things to look at") && t.contains("The dev cluster is stopped"), "{t}");
    assert!(f.has_hit(&Event::Tab(T_RECHECK)) && f.has_hit(&Event::Tab(T_SHARE)));
    assert_eq!(f.app.rounds, 1, "one check when the tile opened");
}

#[test]
fn administrators_see_the_technical_detail_and_other_people_do_not() {
    let admin = fixture("detail-admin");
    assert!(admin.text().contains("192.0.2.7"), "the address is in the detail line");
    let user = fixture_with("detail-user", Role::User, sick(), SlowOut::default(), 1280, 720, 1.0);
    assert!(!user.text().contains("192.0.2.7"), "{}", user.text());
    assert!(user.text().contains("The firewall blocks the developer-mode port"), "the plain sentence stays");
}

#[test]
fn a_healthy_machine_shows_one_calm_line() {
    let f = fixture_with("calm", Role::Admin, healthy(), SlowOut::default(), 1280, 720, 1.0);
    assert!(f.app.found.is_empty());
    let t = f.text();
    assert!(t.contains("Everything checked is fine."), "{t}");
    assert!(!f.g.hits.iter().any(|h| h.label == "finding"));
    assert!(f.has_hit(&Event::Tab(T_SHARE)), "sharing diagnostics is always possible");
}

#[test]
fn up_down_and_clicks_choose_a_finding_and_esc_leaves() {
    let mut f = fixture("keys");
    assert_eq!(f.app.sel, 0);
    f.key(Key::Down);
    f.key(Key::Down);
    assert_eq!(f.app.sel, 2);
    f.key(Key::Up);
    assert_eq!(f.app.sel, 1);
    for _ in 0..10 {
        f.key(Key::Down);
    }
    assert_eq!(f.app.sel, 4, "stops at the last");
    f.ev(Event::Row(3));
    assert_eq!(f.app.sel, 3);
    assert!(matches!(f.key(Key::Esc), Flow::Back));
}

#[test]
fn a_fix_opens_the_settings_page_and_is_audited_and_nothing_else_happens() {
    let mut f = fixture("fix");
    // the firewall finding
    f.key(Key::Down);
    f.key(Key::Enter);
    assert_eq!(f.app.take_request(), Some(("settings".into(), "tab:firewall".into())));
    assert_eq!(f.app.take_request(), None, "taken once");
    assert!(f.audit().contains("doctor.fix") && f.audit().contains("firewall-dev-port"));
    // by clicking the button of the first finding
    f.ev(Event::Tab(T_FIX));
    assert_eq!(f.app.take_request(), Some(("settings".into(), "tab:devcluster".into())));
    // the vault: the page where the person types the password, never DOCTOR itself
    let v = f.app.found.iter().position(|x| x.id == "vault-locked").unwrap();
    f.ev(Event::Tab(T_FIX + v));
    assert_eq!(f.app.take_request(), Some(("settings".into(), "tab:vault".into())));
    // a finding without a fix does nothing
    let mut g = fixture_with("fix-none", Role::Admin, Facts { failed_services: vec![FailedService { id: "service".into(), name: "Web service".into(), reason: "x".into(), open: None }], ..healthy() }, SlowOut::default(), 1280, 720, 1.0);
    g.key(Key::Enter);
    assert_eq!(g.app.take_request(), None);
}

#[test]
fn the_developer_mode_fix_extends_the_switch_off_time_for_administrators_only() {
    let mut f = fixture("extend");
    // real developer mode in the test's data folder, switching off in 20 minutes
    let d = iyakki_agent::devmode::DevMode::open(f.cx.services.dir());
    d.enable(&f.cx.session.policy, "root", 1).unwrap();
    let before = d.state().until;
    let i = f.app.found.iter().position(|x| x.id == "dev-mode-ending").unwrap();
    f.ev(Event::Tab(T_FIX + i));
    let after = d.state().until;
    assert_eq!(after - before, 4 * 3600, "four hours more");
    assert!(f.audit().contains("devmode.extend"), "{}", f.audit());
    assert!(f.app.msg.as_ref().is_some_and(|m| m.1 && m.0.contains("switches off at")), "{:?}", f.app.msg);
    assert!(f.app.rounds >= 2, "it checks again after a fix");
    // an ordinary account is sent to the page instead
    let mut u = fixture_with("extend-user", Role::User, sick(), SlowOut::default(), 1280, 720, 1.0);
    let i = u.app.found.iter().position(|x| x.id == "dev-mode-ending").unwrap();
    u.ev(Event::Tab(T_FIX + i));
    assert_eq!(u.app.take_request(), Some(("settings".into(), "tab:developer".into())));
}

#[test]
fn the_slow_checks_arrive_later_and_add_their_findings() {
    let slow = SlowOut { models_unreachable: vec!["lemonade".into()], exposure: Some("NodePort 30300 gitea-http".into()), ..SlowOut::default() };
    let mut f = fixture_with("slow", Role::Admin, healthy(), slow, 1280, 720, 1.0);
    assert!(!f.app.checking, "settled");
    assert_eq!(f.ids(), ["exposure", "model-unreachable"]);
    assert!(f.text().contains("NodePort 30300"), "the audit line's detail, for the administrator");
    // checking again repeats it
    f.chr('r');
    f.settle();
    assert_eq!(f.app.rounds, 2);
    assert_eq!(f.ids().len(), 2);
}

#[test]
fn a_slow_check_that_is_still_going_says_so_and_the_screen_does_not_wait() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = std::sync::Mutex::new(rx);
    let cx = cx_with_admin_store("doctor-wait");
    let mut p = parts(sick(), SlowOut::default(), tmp("wait"));
    p.slow = Arc::new(move |_| {
        let _ = rx.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5));
        SlowOut { models_unreachable: vec!["x".into()], exposure: None, ..SlowOut::default() }
    });
    let mut app = DoctorApp::with_parts(&cx, p);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(8, 10, 22));
    app.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &cx, 3);
    assert!(app.checking && !app.found.is_empty(), "the cheap findings are on screen already");
    assert!(g.text_log.iter().any(|t| t.2.starts_with("still checking")));
    assert!(app.wants_redraw(), "it keeps asking for frames while it waits");
    tx.send(()).unwrap();
    for _ in 0..400 {
        if !app.checking {
            break;
        }
        app.wants_redraw();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!app.checking && app.found.iter().any(|f| f.id == "model-unreachable"));
}

#[test]
fn it_draws_inside_its_area_at_every_size_and_state() {
    for (w, h, s) in SIZES {
        let mut f = fixture_with("size", Role::Admin, sick(), SlowOut { models_unreachable: vec!["a".into(), "b".into()], exposure: Some("e".repeat(300)), ..SlowOut::default() }, w, h, s);
        assert_hits_sane(&f, &format!("{w}x{h} list"));
        f.key(Key::Down);
        f.key(Key::End);
        assert_hits_sane(&f, &format!("{w}x{h} selected"));
        f.app.fail("Something failed with a rather long explanation that has to be cut to the width of the screen somewhere.");
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} message"));
        f.chr('s');
        assert!(f.app.preview.is_some(), "{w}x{h}");
        assert_hits_sane(&f, &format!("{w}x{h} preview"));
        f.key(Key::Down);
        f.key(Key::Char('2'));
        assert_hits_sane(&f, &format!("{w}x{h} preview scrolled"));
        f.key(Key::Esc);
        assert!(f.app.preview.is_none());
        let calm = fixture_with("size-calm", Role::Admin, healthy(), SlowOut::default(), w, h, s);
        assert_hits_sane(&calm, &format!("{w}x{h} calm"));
    }
}

#[test]
fn it_draws_inside_a_pane_that_does_not_start_at_the_corner() {
    for (x, y, w, h) in [(640.0f32, 60.0f32, 640.0f32, 600.0f32), (0.0, 360.0, 1280.0, 360.0), (320.0, 0.0, 320.0, 720.0)] {
        let mut f = fixture("pane");
        f.area = R::new(x, y, w, h);
        f.draw();
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h}"));
        f.chr('s');
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h} preview"));
    }
}

#[test]
fn many_findings_scroll_and_the_chosen_one_stays_in_view() {
    let many = Facts { failed_services: (0..30).map(|i| FailedService { id: format!("svc{i}"), name: format!("Service {i}"), reason: format!("reason {i}"), open: Some("tab:shares") }).collect(), ..healthy() };
    let mut f = fixture_with("many", Role::Admin, many, SlowOut::default(), 800, 480, 1.0);
    assert!(f.app.list.scrolls());
    assert_eq!(f.app.found.len(), 30);
    for _ in 0..25 {
        f.key(Key::Down);
    }
    assert_eq!(f.app.sel, 25);
    assert!(f.app.list.offset > 0.0, "scrolled down to follow the choice");
    let shown: Vec<&str> = f.g.hits.iter().filter(|h| h.label == "finding").map(|_| "x").collect();
    assert!(shown.len() < 30 && !shown.is_empty());
    assert!(f.has_hit(&Event::Tab(T_FIX + 25)), "the chosen finding's button is on screen");
    assert!(f.app.scroll_px(-120, &f.cx), "the wheel scrolls the list");
    assert_hits_sane(&f, "scrolled list");
}

#[test]
fn the_preview_shows_exactly_what_the_file_will_hold_and_no_secret() {
    let mut f = fixture("preview");
    f.chr('s');
    let p = f.app.preview.as_ref().expect("preview is open");
    for bad in [SECRET, "hunter2", "abc123", "192.0.2.7"] {
        assert!(!p.text.contains(bad), "{bad:?} must not reach the preview:\n{}", p.text);
    }
    for part in ["=== Version", "=== Services", "=== Doctor", "=== Hardware", "=== Web engine log", "v4l2src busy"] {
        assert!(p.text.contains(part), "{part}");
    }
    assert!(p.text.contains("https://shop.example.org/c?***"), "an address keeps its host and path, never its query:\n{}", p.text);
    assert!(f.audit().contains("doctor.share.preview"));
    assert!(f.has_hit(&Event::Tab(T_SAVE)) && f.has_hit(&Event::Tab(T_CLOSE)));
    assert!(f.app.hint().contains("save"), "{}", f.app.hint());
    // closing it writes nothing
    f.key(Key::Esc);
    assert!(f.app.preview.is_none());
    assert_eq!(std::fs::read_dir(&f.home).unwrap().count(), 0, "nothing was saved");
}

#[test]
fn saving_writes_the_previewed_text_byte_for_byte_in_the_home_folder_once() {
    let mut f = fixture("save");
    f.chr('s');
    let shown = f.app.preview.as_ref().unwrap().text.clone();
    f.chr('s');
    let saved = f.app.preview.as_ref().unwrap().saved.clone().expect("saved");
    assert!(saved.starts_with(&f.home) && saved.file_name().unwrap().to_string_lossy().starts_with("iyakki-diagnostics-"));
    assert_eq!(std::fs::read(&saved).unwrap(), shown.as_bytes());
    assert!(f.audit().contains("doctor.share.saved") && !f.audit().contains(SECRET));
    assert!(f.app.msg.as_ref().unwrap().0.contains("Saved on my home folder"));
    assert!(f.has_hit(&Event::Tab(T_COPY)) && !f.has_hit(&Event::Tab(T_SAVE)), "the save buttons give way to Copy path");
    // a second press saves nothing more
    f.chr('s');
    assert_eq!(std::fs::read_dir(&f.home).unwrap().flatten().filter(|e| e.path().is_file()).count(), 1);
    f.chr('c');
    assert!(f.app.msg.as_ref().unwrap().0.contains("copied"));
}

#[test]
fn a_usb_stick_opened_by_disks_is_offered_as_a_place() {
    let mut f = fixture("usb");
    std::fs::create_dir_all(f.home.join("media/STICK")).unwrap();
    std::fs::create_dir_all(f.home.join("media/BACKUP")).unwrap();
    std::fs::write(f.home.join("media/not-a-folder.txt"), "x").unwrap();
    f.chr('s');
    let labels: Vec<String> = f.app.preview.as_ref().unwrap().dests.iter().map(|d| d.label.clone()).collect();
    assert_eq!(labels, ["my home folder", "BACKUP", "STICK"]);
    assert!(f.text().contains("Save on STICK"));
    // key 3 is the stick
    f.chr('3');
    let saved = f.app.preview.as_ref().unwrap().saved.clone().unwrap();
    assert!(saved.starts_with(f.home.join("media/STICK")), "{saved:?}");
    assert!(f.app.msg.as_ref().unwrap().0.contains("Saved on STICK"));
    // a press on a button of a place that is not there does nothing
    let mut g = fixture("usb2");
    g.chr('s');
    g.chr('9');
    assert!(g.app.preview.as_ref().unwrap().saved.is_none());
}

#[test]
fn a_folder_that_cannot_be_written_says_so_and_the_preview_stays_open() {
    let mut f = fixture("nowrite");
    f.chr('s');
    std::fs::remove_dir_all(&f.home).unwrap();
    f.chr('s');
    let p = f.app.preview.as_ref().expect("still open");
    assert!(p.saved.is_none());
    let (m, ok) = f.app.msg.clone().unwrap();
    assert!(!ok && m.contains("cannot write"), "{m}");
}

#[test]
fn settings_asks_for_the_share_preview_and_the_overview_has_the_button() {
    let mut f = fixture("item");
    // another tile opens DOCTOR with the item
    let mut a = DoctorApp::with_parts(&f.cx, parts(healthy(), SlowOut::default(), f.home.clone()));
    assert!(a.preview.is_none());
    App::open_item(&mut a, "share", &f.cx);
    assert!(a.preview.is_some());
    // Settings -> Overview
    let cx = cx_with_admin_store("doctor-settings");
    let mut st = super::settings::SettingsApp::new(&cx);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(8, 10, 22));
    g.clear_hits();
    st.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &cx, 1);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('s'))), "the Share diagnostics button is on the Overview page");
    st.handle(&Event::Key(Key::Char('s')), &cx);
    assert_eq!(st.take_request(), Some(("doctor".into(), "share".into())));
    // a person without the tile does not get the button
    let mut cx2 = cx_with_admin_store("doctor-settings-guest");
    cx2.session.role = Role::Guest;
    cx2.session.policy = Policy::resolve(Role::Guest, &Default::default());
    let mut st2 = super::settings::SettingsApp::new(&cx2);
    st2.handle(&Event::Key(Key::Char('s')), &cx2);
    assert_ne!(st2.take_request(), Some(("doctor".into(), "share".into())));
    f.draw();
}

#[test]
fn the_real_readers_run_on_this_computer_without_panicking() {
    // reads files and the cached cluster health only; changes nothing
    let cx = cx_with_admin_store("doctor-real");
    let mut a = DoctorApp::new(&cx);
    for _ in 0..400 {
        if !a.checking {
            break;
        }
        a.wants_redraw();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!a.checking, "the slow reads finish");
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(8, 10, 22));
    a.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &cx, 1);
    a.open_share(&cx);
    let t = a.preview.as_ref().unwrap().text.clone();
    assert!(t.starts_with("IYAKKI diagnostics.") && t.contains("=== Version"));
    let home = std::env::var("HOME").unwrap_or_default();
    if home.len() > 1 {
        assert!(!t.contains(&format!("{home}/")), "the home path is not in the bundle");
    }
}
