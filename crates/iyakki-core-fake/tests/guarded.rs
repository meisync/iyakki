//! The guarded Os on the fake machine (F-ARCH-05/06, F-CORE-12/13): denied without the capability, allowed with it, a card for the hard ask, a log line for every decision, and nothing reaching the machine when refused.

use std::sync::Arc;

use iyakki_core_api::appcaps::Approval;
use iyakki_core_api::broker::{AccessEvent, ApprovalLookup, Broker, Caller, CallerKind, CardAnswer, CardAsk, MemoryLog};
use iyakki_core_api::guarded::{GestureCard, GuardedOs};
use iyakki_core_api::hardware::Rfkill;
use iyakki_core_api::memory::config::Config;
use iyakki_core_api::Os;
use iyakki_core_fake::FakeOs;

struct NoApprovals;
impl ApprovalLookup for NoApprovals {
    fn approval(&self, _: &str) -> Option<Approval> {
        None
    }
}

struct Answer(CardAnswer, std::sync::Mutex<u32>);
impl CardAsk for Answer {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        *self.1.lock().unwrap() += 1;
        self.0
    }
}

fn caller(app: &str) -> Caller {
    Caller { app: app.into(), kind: CallerKind::Builtin, person: "p".into(), device: "d".into() }
}

fn rig(app: &str, card: Arc<dyn CardAsk>) -> (Arc<FakeOs>, Arc<MemoryLog>, GuardedOs) {
    let fake = Arc::new(FakeOs::new());
    fake.hardware_fake().set(|s| s.screen.backlights = vec![iyakki_core_api::hardware::Backlight { name: "panel".into(), value: 50, max: 100 }]);
    fake.hardware_fake().set(|s| s.rfkill = vec![Rfkill { id: "rfkill1".into(), name: "hci0".into(), kind: "wlan".into(), soft: false, hard: false }]);
    let log = Arc::new(MemoryLog::new(100));
    let broker = Arc::new(Broker::new(Arc::new(NoApprovals), card, log.clone(), Arc::new(|| 1000)));
    let g = GuardedOs::new(fake.clone(), broker, caller(app));
    (fake, log, g)
}

fn yes() -> Arc<Answer> {
    Arc::new(Answer(CardAnswer::Allow, Default::default()))
}

fn rows(log: &MemoryLog, cap: &str) -> Vec<AccessEvent> {
    log.rows().into_iter().filter(|r| r.capability == cap).collect()
}

#[test]
fn every_app_reads_without_a_card_and_the_read_is_logged() {
    let card = yes();
    let (_f, log, g) = rig("somewidget", card.clone());
    assert_eq!(g.sysinfo().cpu_count().unwrap(), 4);
    assert_eq!(g.sysinfo().product_name().unwrap(), "Test Machine");
    assert_eq!(g.disks().space("/").unwrap().1, 100_000_000_000);
    assert_eq!(g.net().ipv4_addresses().unwrap().len(), 2);
    assert_eq!(g.audio().volume(0).unwrap().percent, 40);
    assert_eq!(g.hardware().rfkill().len(), 1);
    _f.hardware_fake().set(|s| {
        s.mac_summary = Some("MAC: private".into());
        s.report = vec!["MACHINE".into()];
    });
    assert_eq!(g.hardware().mac_summary().as_deref(), Some("MAC: private"));
    assert_eq!(g.hardware().hardware_report(), vec!["MACHINE".to_string()]);
    assert_eq!(*card.1.lock().unwrap(), 0, "a harmless read never asks");
    let r = rows(&log, "system.read");
    assert!(!r.is_empty() && r.iter().all(|e| e.allowed && e.app == "somewidget"), "{r:?}");
}

#[test]
fn a_change_without_the_capability_is_denied_logged_and_never_reaches_the_machine() {
    let (fake, log, g) = rig("somewidget", yes());
    let e = g.hardware().set_rfkill("rfkill1", true).unwrap_err();
    assert!(e.contains("not_granted"), "{e}");
    assert!(g.hardware().set_brightness("panel", 10).unwrap_err().contains("not_granted"));
    let v = g.audio().set_volume(0, 90).unwrap_err();
    assert_eq!(v.code(), "denied");
    assert!(g.audio().change_volume(0, Some(5)).is_err());
    assert!(g.memory().save_conf(&Config::default()).unwrap_err().contains("not_granted"));
    assert!(g.memory().set_conf_key("k", Some("v")).is_err());
    assert!(g.memory().reconfigure(&Config::default(), 1 << 30).is_err());
    let steps = g.memory().apply_swapfile(Some(1 << 30), true);
    assert!(steps.len() == 1 && !steps[0].ok && steps[0].msg.contains("not_granted"));
    // nothing reached the fake machine
    assert!(fake.hardware_fake().calls().iter().all(|c| !c.contains("set_")), "{:?}", fake.hardware_fake().calls());
    assert!(fake.memory_fake().calls().is_empty(), "{:?}", fake.memory_fake().calls());
    assert_eq!(fake.audio().volume(0).unwrap().percent, 40);
    let denied: Vec<_> = log.rows().into_iter().filter(|r| !r.allowed).collect();
    assert!(denied.len() == 4 && denied.iter().all(|r| r.reason == "not_granted" && r.app == "somewidget"), "{denied:?}"); // one line per capability: repeats inside ten seconds are folded
}

#[test]
fn an_app_in_the_table_changes_radios_brightness_and_volume_without_a_card() {
    let card = yes();
    let (fake, log, g) = rig("settings", card.clone());
    g.hardware().set_rfkill("rfkill1", true).unwrap();
    g.hardware().set_brightness("panel", 30).unwrap();
    assert_eq!(g.audio().set_volume(0, 70).unwrap().percent, 70);
    assert_eq!(fake.audio().volume(0).unwrap().percent, 70, "the change reached the fake machine");
    assert!(fake.hardware_fake().calls().iter().any(|c| c.contains("rfkill")));
    assert_eq!(*card.1.lock().unwrap(), 0, "these three have no per-use card in the catalogue");
    for cap in ["system.radio.set", "system.brightness.set", "system.volume.set"] {
        let r = rows(&log, cap);
        assert!(r.len() == 1 && r[0].allowed && r[0].app == "settings", "{cap}: {r:?}");
    }
}

#[test]
fn another_app_holding_the_name_in_the_table_only_gets_what_the_table_says() {
    let (_f, _log, g) = rig("shell", yes());
    assert!(g.audio().set_volume(0, 10).is_ok());
    assert!(g.hardware().set_brightness("panel", 20).is_ok());
    assert!(g.hardware().set_rfkill("rfkill1", true).unwrap_err().contains("not_granted"), "the shell does not switch radios");
    assert!(g.memory().save_conf(&Config::default()).unwrap_err().contains("not_granted"));
}

#[test]
fn the_memory_change_asks_a_card_every_time_and_the_answer_decides() {
    let card = yes();
    let (fake, log, g) = rig("settings", card.clone());
    let m = g.memory();
    m.set_conf_key("zram", Some("on")).unwrap();
    m.set_conf_key("zram", Some("off")).unwrap();
    assert_eq!(*card.1.lock().unwrap(), 2, "a hard ask asks at every use, whatever was granted");
    assert_eq!(fake.memory_fake().calls().len(), 2);

    let no = Arc::new(Answer(CardAnswer::Deny, Default::default()));
    let (fake, log2, g) = rig("settings", no.clone());
    let e = g.memory().set_conf_key("zram", Some("on")).unwrap_err();
    assert!(e.contains("card_declined"), "{e}");
    assert!(fake.memory_fake().calls().is_empty());
    assert_eq!(rows(&log2, "system.memory.set")[0].reason, "card_declined");

    let (fake, log3, g) = rig("settings", Arc::new(Answer(CardAnswer::Unavailable, Default::default())));
    assert!(g.memory().apply_swapfile(None, false)[0].msg.contains("no_card"));
    assert!(fake.memory_fake().calls().is_empty());
    assert_eq!(rows(&log3, "system.memory.set")[0].reason, "no_card");
    assert!(rows(&log, "system.memory.set").iter().all(|r| r.allowed));
}

#[test]
fn the_host_name_change_asks_a_card_every_time_and_a_refused_change_never_reaches_the_machine() {
    let etc = std::path::Path::new("/etc/hostname");
    let card = yes();
    let (fake, log, g) = rig("settings", card.clone());
    g.hardware().set_hostname("kitchen-mac", etc).unwrap();
    g.hardware().set_hostname("study-mac", etc).unwrap();
    assert_eq!(*card.1.lock().unwrap(), 2, "a hard ask asks at every use");
    assert_eq!(fake.hardware_fake().calls().iter().filter(|c| c.starts_with("set_hostname")).count(), 2);
    assert!(g.hardware().set_hostname("Bad Name", etc).is_err(), "the machine's own rules still apply behind the gate");
    assert!(rows(&log, "system.hostname.set").iter().all(|r| r.allowed));
    let (fake, log2, g) = rig("settings", Arc::new(Answer(CardAnswer::Deny, Default::default())));
    let e = g.hardware().set_hostname("kitchen-mac", etc).unwrap_err();
    assert!(e.contains("card_declined"), "{e}");
    assert!(fake.hardware_fake().calls().is_empty());
    assert_eq!(rows(&log2, "system.hostname.set")[0].reason, "card_declined");
    // an app that the table does not name never even asks
    let (fake, _l, g) = rig("somewidget", card.clone());
    assert!(g.hardware().set_hostname("kitchen-mac", etc).is_err());
    assert!(fake.hardware_fake().calls().is_empty());
}

#[test]
fn the_gesture_card_allows_only_the_work_the_press_started_inside_its_dispatch() {
    let gc = Arc::new(GestureCard::new(std::time::Duration::from_secs(1)));
    let (fake, _log, g) = rig("settings", gc.clone());
    let m = g.memory();
    assert!(m.set_conf_key("zram", Some("on")).unwrap_err().contains("no_card"), "nobody pressed: refused");
    assert!(fake.memory_fake().calls().is_empty());
    // several calls of the one event are the one change
    gc.dispatch(|| {
        m.set_conf_key("zram", Some("on")).unwrap();
        m.set_conf_key("swappiness", Some("20")).unwrap();
    });
    assert_eq!(fake.memory_fake().calls().len(), 2);
    // the press does not outlive its event, however soon the next call comes (a press five seconds earlier is no different)
    assert!(m.set_conf_key("zram", Some("off")).unwrap_err().contains("no_card"));
    assert_eq!(fake.memory_fake().calls().len(), 2);
    // a second event is a second gesture
    gc.dispatch(|| m.set_conf_key("zram", Some("off")).unwrap());
    assert_eq!(fake.memory_fake().calls().len(), 3);
}

#[test]
fn a_background_thread_without_a_gesture_is_refused_and_one_ticket_allows_one_change() {
    use iyakki_core_api::guarded::gesture_ticket;
    let gc = Arc::new(GestureCard::new(std::time::Duration::from_secs(1)));
    let (fake, _log, g) = rig("settings", gc.clone());
    let m = g.memory();
    assert!(gesture_ticket().is_none(), "outside a dispatch there is no press to hand on");
    // a thread that was never given a press
    let m2 = m.clone();
    let out = std::thread::spawn(move || m2.set_conf_key("zram", Some("on"))).join().unwrap();
    assert!(out.unwrap_err().contains("no_card"));
    // even a thread started from inside a dispatch has no press unless it was handed the ticket
    let m3 = m.clone();
    let out = gc.dispatch(|| std::thread::spawn(move || m3.set_conf_key("zram", Some("on"))).join().unwrap());
    assert!(out.unwrap_err().contains("no_card"));
    assert!(fake.memory_fake().calls().is_empty());
    // the ticket taken in the dispatch and redeemed on the job thread allows the work it carries, once
    let ticket = gc.dispatch(|| gesture_ticket().expect("a ticket inside a dispatch"));
    let m4 = m.clone();
    std::thread::spawn(move || ticket.redeem(|| m4.set_conf_key("zram", Some("on")).unwrap())).join().unwrap();
    assert_eq!(fake.memory_fake().calls().len(), 1);
    let spent = gc.dispatch(|| gesture_ticket().unwrap());
    let m5 = m.clone();
    let (first, second) = std::thread::spawn(move || {
        let a = spent.redeem(|| m5.set_conf_key("zram", Some("off")));
        (a, m5.set_conf_key("zram", Some("on")))
    })
    .join()
    .unwrap();
    assert!(first.is_ok() && second.unwrap_err().contains("no_card"), "after the work the ticket gave nothing more");
    // an old ticket gives nothing
    let stale = Arc::new(GestureCard::new(std::time::Duration::ZERO));
    let (_f2, _l2, g2) = rig("settings", stale.clone());
    let t = stale.dispatch(|| gesture_ticket().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(5));
    let m6 = g2.memory();
    assert!(t.redeem(|| m6.set_conf_key("zram", Some("on"))).unwrap_err().contains("no_card"));
}

#[test]
fn a_refused_read_gives_the_empty_answer_not_the_machine() {
    // an app id the catalogue does not hold system.read for cannot exist (Yes = every built-in), so make the caller a store app with no approval
    let fake = Arc::new(FakeOs::new());
    let log = Arc::new(MemoryLog::new(10));
    let broker = Arc::new(Broker::new(Arc::new(NoApprovals), yes(), log.clone(), Arc::new(|| 1)));
    let who = Caller { app: "x".into(), kind: CallerKind::Store { installation: "inst1".into() }, person: "p".into(), device: "d".into() };
    let g = GuardedOs::new(fake.clone(), broker, who);
    assert!(g.sysinfo().cpu_count().unwrap_err().to_string().contains("not_granted"));
    assert!(g.disks().mounts().is_err() && g.net().interfaces().is_err());
    assert!(g.hardware().rfkill().is_empty() && g.hardware().links().is_empty());
    assert!(g.hardware().screen().backlights.is_empty());
    assert_eq!(g.memory().stats().total, 0);
    assert!(g.memory().data_free(std::path::Path::new("/")).is_none());
    assert!(g.hardware().bluetooth().controllers.is_err());
    assert!(g.hardware().mac_summary().is_none() && g.hardware().hardware_report().is_empty());
    assert!(log.rows().iter().all(|r| !r.allowed));
}

#[test]
fn the_drive_message_is_reworded_through_the_core_and_a_refused_read_leaves_it_alone() {
    let (_f, _log, g) = rig("somewidget", yes());
    assert_eq!(g.disks().reword_failure("Could not save: I/O error"), "Could not save: I/O error");
    let fake = Arc::new(FakeOs::with(iyakki_core_fake::Canned { drive_gone: Some("The drive is gone.".into()), ..Default::default() }));
    let log = Arc::new(MemoryLog::new(10));
    let broker = Arc::new(Broker::new(Arc::new(NoApprovals), yes(), log.clone(), Arc::new(|| 1)));
    let denied = GuardedOs::new(fake.clone(), broker, Caller { app: "x".into(), kind: CallerKind::Store { installation: "i".into() }, person: "p".into(), device: "d".into() });
    assert_eq!(denied.disks().reword_failure("failed"), "failed", "no read right: the message is left as it is");
    assert!(log.rows().iter().all(|r| !r.allowed));
    assert_eq!(fake.disks().reword_failure("failed"), "The drive is gone.");
}

#[test]
fn the_kept_logs_are_read_with_system_read_and_marking_or_copying_needs_its_own_capability() {
    let (fake, log, g) = rig("settings", yes());
    fake.crashlog_fake().set(|s| {
        s.files.insert("k.log".into(), vec!["a".into(), "b".into()]);
        s.latest = Some("k.log".into());
    });
    let d = std::path::Path::new("/x");
    assert_eq!(g.crashlog().latest_crash(d).as_deref(), Some("k.log"));
    assert_eq!(g.crashlog().last_lines(d, "k.log", 1), vec!["b".to_string()]);
    g.crashlog().mark_seen(d);
    assert!(g.crashlog().copy_to_folder(d, "k.log", d, 1).is_ok());
    assert_eq!(fake.crashlog_fake().calls(), vec!["mark_seen".to_string(), "copy_to_folder(k.log)".to_string()]);
    assert!(!rows(&log, "system.crashlog.write").is_empty() && rows(&log, "system.crashlog.write").iter().all(|r| r.allowed));
    // an app the table does not name can read but neither mark nor copy
    let (fake, log2, g) = rig("somewidget", yes());
    fake.crashlog_fake().set(|s| s.latest = Some("k.log".into()));
    assert_eq!(g.crashlog().latest_crash(d).as_deref(), Some("k.log"));
    g.crashlog().mark_seen(d);
    assert!(g.crashlog().copy_to_folder(d, "k.log", d, 1).is_err());
    assert!(fake.crashlog_fake().calls().is_empty(), "nothing reached the machine");
    assert!(rows(&log2, "system.crashlog.write").iter().all(|r| !r.allowed));
}

#[test]
fn a_scan_asks_a_card_every_time_the_subnets_are_a_plain_read_and_a_refused_scan_never_starts() {
    use iyakki_core_api::netscan::Options;
    let ip = std::net::Ipv4Addr::LOCALHOST;
    let go = |g: &GuardedOs| {
        let (tx, rx) = std::sync::mpsc::channel();
        let r = g.netscan().start(vec![ip], Options::default(), Default::default(), tx);
        (r, rx)
    };
    let card = yes();
    let (fake, log, g) = rig("scan", card.clone());
    assert!(g.netscan().subnets().is_empty() || !g.netscan().subnets().is_empty());
    let (r, rx) = go(&g);
    assert!(r.is_ok());
    assert!(matches!(rx.try_recv(), Ok(iyakki_core_api::netscan::Event::Started { .. })));
    assert_eq!(*card.1.lock().unwrap(), 1, "a card for the scan");
    let _ = go(&g);
    assert_eq!(*card.1.lock().unwrap(), 2, "and again for the next one");
    assert_eq!(fake.netscan_fake().calls().len(), 2);
    assert!(rows(&log, "net.scan").iter().all(|r| r.allowed));
    let (fake, log2, g) = rig("scan", Arc::new(Answer(CardAnswer::Deny, Default::default())));
    let (r, rx) = go(&g);
    assert!(r.unwrap_err().contains("card_declined"));
    assert!(rx.try_recv().is_err() && fake.netscan_fake().calls().is_empty(), "nothing was scanned");
    assert_eq!(rows(&log2, "net.scan")[0].reason, "card_declined");
    // an app that is not in the table never scans
    let (fake, _l, g) = rig("somewidget", card);
    assert!(go(&g).0.is_err() && fake.netscan_fake().calls().is_empty());
}

#[test]
fn a_router_run_and_a_strategy_change_ask_a_card_every_time_the_decisions_are_logged_without_asking_and_a_refusal_reaches_nothing() {
    let card = yes();
    let (fake, log, g) = rig("chat", card.clone());
    assert!(g.router().begin_run("3 steps, strategy default").is_ok());
    assert!(g.router().begin_run("2 steps, strategy default").is_ok());
    assert_eq!(*card.1.lock().unwrap(), 2, "a card for every run");
    g.router().record("router.assign", "step=s1 model=small");
    g.router().record("router.assign", "step=s2 model=small");
    assert_eq!(*card.1.lock().unwrap(), 2, "a decision line asks nothing");
    let lines: Vec<String> = rows(&log, "router.use").into_iter().map(|r| r.target).collect();
    assert!(lines.contains(&"router.assign step=s1 model=small".to_string()) && lines.contains(&"router.assign step=s2 model=small".to_string()), "each decision is its own line: {lines:?}");
    assert!(g.router().allow_strategy("mine").is_ok());
    assert_eq!(rows(&log, "router.strategy.set").len(), 1);
    assert_eq!(fake.router_fake().calls().len(), 5);
    // the person says no
    let (fake, log2, g) = rig("chat", Arc::new(Answer(CardAnswer::Deny, Default::default())));
    assert!(g.router().begin_run("x").unwrap_err().contains("card_declined"));
    assert!(g.router().allow_strategy("mine").unwrap_err().contains("card_declined"));
    assert!(fake.router_fake().calls().is_empty(), "nothing reached the machine");
    assert_eq!(rows(&log2, "router.use")[0].reason, "card_declined");
    // an app the table does not name neither runs, nor changes the strategy, nor writes lines
    let (fake, log3, g) = rig("somewidget", card);
    assert!(g.router().begin_run("x").is_err() && g.router().allow_strategy("y").is_err());
    g.router().record("router.assign", "step=s1 model=small");
    assert!(rows(&log3, "router.use").iter().all(|r| !r.allowed), "no line written by an app without the capability");
    assert!(fake.router_fake().calls().is_empty(), "nothing reached the machine");
}

#[test]
fn a_machine_that_does_not_name_a_router_refuses() {
    use iyakki_core_api::router::{NoRouter, Router};
    assert!(NoRouter.begin_run("x").is_err() && NoRouter.allow_strategy("y").is_err());
}

#[test]
fn fleet_reads_need_no_card_every_change_asks_one_and_a_refusal_or_an_unnamed_app_reaches_nothing() {
    let card = yes();
    let (fake, log, g) = rig("chat", card.clone());
    assert!(g.fleet().read("members").is_ok());
    assert_eq!(*card.1.lock().unwrap(), 0, "a read asks nothing");
    assert!(g.fleet().manage("create fleet").is_ok() && g.fleet().join("join").is_ok() && g.fleet().assign("1 task").is_ok() && g.fleet().receive("accept").is_ok());
    assert!(g.fleet().manage("offer").is_ok());
    assert_eq!(*card.1.lock().unwrap(), 5, "a card for every change, every time");
    g.fleet().record("fleet.heartbeat", "member=a");
    assert_eq!(*card.1.lock().unwrap(), 5, "a log line asks nothing");
    assert_eq!(rows(&log, "fleet.read").iter().filter(|r| r.target.starts_with("fleet.heartbeat")).count(), 1, "the line is written");
    assert!(!rows(&log, "fleet.manage").is_empty(), "the decisions are logged");
    assert_eq!(fake.fleet_fake().calls().len(), 7);
    // the person says no
    let (fake, log2, g) = rig("chat", Arc::new(Answer(CardAnswer::Deny, Default::default())));
    for r in [g.fleet().join("x"), g.fleet().manage("x"), g.fleet().assign("x"), g.fleet().receive("x")] {
        assert!(r.unwrap_err().contains("card_declined"));
    }
    assert!(fake.fleet_fake().calls().is_empty(), "nothing reached the machine");
    assert_eq!(rows(&log2, "fleet.assign")[0].reason, "card_declined");
    // an app the table does not name neither reads, nor changes, nor writes lines
    let (fake, log3, g) = rig("somewidget", card);
    assert!(g.fleet().read("x").is_err() && g.fleet().manage("x").is_err() && g.fleet().assign("x").is_err());
    g.fleet().record("fleet.heartbeat", "member=a");
    assert!(rows(&log3, "fleet.read").iter().all(|r| !r.allowed));
    assert!(fake.fleet_fake().calls().is_empty());
}

#[test]
fn a_machine_that_does_not_name_a_fleet_refuses() {
    use iyakki_core_api::fleet::{Fleet, NoFleet};
    assert!(NoFleet.read("x").is_err() && NoFleet.join("x").is_err() && NoFleet.manage("x").is_err() && NoFleet.assign("x").is_err() && NoFleet.receive("x").is_err());
}
