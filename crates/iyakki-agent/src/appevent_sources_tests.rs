//! The sources of the events apps hear (F-SDK-195 to 200): each source tells the engine through `emit`, with a payload of words and counts
//! (never a secret), proved on a real `Services` for the pushed ones and with made-up inputs for the pulled ones. No sleeping: every clock is
//! a number the test passes.

use super::*;
use crate::services::Services;
use iyakki_core_api::appevents::{AppEvent, AppEvents, Val};
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::sync::atomic::{AtomicU64, Ordering};

struct Dead;
impl Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn svc(tag: &str) -> Arc<Services> {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-evsrc-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap()
}

/// The words of a delivered event, `name field=value ...`, in field order.
fn show(e: &AppEvent) -> String {
    let f: Vec<String> = e.payload.iter().map(|(k, v)| format!("{k}={}", match v { Val::Text(t) => t.clone(), Val::Int(n) => n.to_string(), Val::Bool(b) => b.to_string() })).collect();
    format!("{} {}", e.name, f.join(" ")).trim().to_string()
}

fn listen(hub: &dyn AppEvents, user: &str, names: &[&str]) {
    for n in names {
        hub.register(user, "sample", n, "").unwrap_or_else(|e| panic!("{n}: {e}"));
    }
}

#[test]
fn sign_in_sign_out_and_the_vault_are_announced_with_words_only() {
    let s = svc("session");
    let hub = &*s.app_events;
    listen(hub, "pat", &["session.changed", "vault.changed"]);
    hub.start("pat", "sample");
    let step = |what: &str| -> Vec<String> {
        let _ = what;
        hub.poll("pat", "sample", 50).iter().map(show).collect()
    };
    s.on_login("pat", "correct horse battery", false);
    assert_eq!(step("in"), ["session.changed state=signed_in", "vault.changed state=unlocked"]);
    s.lock_user_vault("pat");
    assert_eq!(step("lock"), ["vault.changed state=locked"]);
    s.unlock_user_vault("pat", "correct horse battery").unwrap();
    assert_eq!(step("unlock"), ["vault.changed state=unlocked"]);
    // two vault changes between two looks are one event, the latest (a state event)
    s.lock_user_vault("pat");
    s.unlock_user_vault("pat", "correct horse battery").unwrap();
    s.on_logout("pat", false);
    assert_eq!(step("out"), ["vault.changed state=locked", "session.changed state=signed_out"]);
    assert!(hub.audit().iter().all(|l| !l.contains("correct horse")), "no secret in the audit lines");
    // somebody else's events do not reach pat's apps
    s.on_login("sam", "another password", false);
    assert!(hub.poll("pat", "sample", 50).is_empty());
}

#[test]
fn an_app_installed_updated_or_removed_is_announced_by_its_id_only() {
    let s = svc("apps");
    let hub = &*s.app_events;
    listen(hub, "pat", &["app.installed", "updates.changed"]);
    hub.start("pat", "sample");
    app_changed(hub, "pat", "tile//notes", "installed");
    app_changed(hub, "pat", "tile//notes", "updated");
    update_changed(hub, "tile//notes", "applied");
    app_changed(hub, "pat", "tile//notes", "removed");
    let got: Vec<String> = hub.poll("pat", "sample", 50).iter().map(show).collect();
    assert_eq!(got, ["app.installed app=tile//notes change=installed", "app.installed app=tile//notes change=updated", "updates.changed component=tile//notes phase=applied", "app.installed app=tile//notes change=removed"]);
    // a bad word is a bug of the source: refused by the engine, nothing is delivered
    emit(hub, "app.installed", Some("pat"), payload(&[("app", Val::Text("x".into())), ("change", Val::Text("exploded".into()))]));
    assert!(hub.poll("pat", "sample", 50).is_empty());
}

#[test]
fn a_backup_is_announced_by_its_file_name() {
    let s = svc("backup");
    listen(&*s.app_events, "pat", &["backup.finished"]);
    // storage scope is held by the sample app in the table
    backup_finished(&*s.app_events, "pat", "backup-2026-11-17.iyb", true);
    let got = s.app_events.start("pat", "sample");
    assert_eq!(show(&got[0]), "backup.finished name=backup-2026-11-17.iyb outcome=finished");
}

#[test]
fn the_pure_watches_report_the_difference_and_remember_the_first_look() {
    let mut f = FlowWatch::default();
    let run = |id: &str, ended: bool, ok: bool| (id.to_string(), "Backup".to_string(), ended, ok);
    assert!(f.observe("pat", &[run("r1", true, true)]).is_empty(), "the first look only remembers");
    assert_eq!(f.observe("pat", &[run("r1", true, true), run("r2", false, false)]), []);
    assert_eq!(f.observe("pat", &[run("r1", true, true), run("r2", true, false)]), [("Backup".to_string(), false)]);
    assert!(f.observe("pat", &[run("r2", true, false)]).is_empty(), "once");

    let mut l = LinkWatch::default();
    assert_eq!((l.observe("online"), l.observe("online"), l.observe("offline")), (None, None, Some("offline".to_string())));

    let mut p = PrefsWatch::default();
    assert_eq!(p.observe("pat", r#"{"theme":"dark","size":1,"wallpaper":"x"}"#, "{}"), (vec![], false));
    let (ch, acc) = p.observe("pat", r#"{"theme":"light","size":2,"wallpaper":"y"}"#, r#"{"reduce_motion":true}"#);
    assert_eq!(ch, [("display", "theme"), ("display", "font")]);
    assert!(acc);

    let mut c = ClockWatch::default();
    assert!(c.observe(1_000_000, 0, 0, "UTC").is_empty());
    assert_eq!(c.observe(1_000_061, 0, 61_000, "UTC"), [Tick::Minute]);
    // across midnight and the hour, in a zone with an offset
    let mid = 86_400 * 12 - 3600 * 2;
    let mut c = ClockWatch::default();
    c.observe(mid - 5, 7200, 0, "Europe/X");
    assert_eq!(c.observe(mid + 5, 7200, 10_000, "Europe/X"), [Tick::Minute, Tick::Hour, Tick::Midnight]);
    // the zone changed; the wall clock jumped by an hour while the steady clock moved a second
    assert_eq!(c.observe(mid + 6, 7200, 11_000, "Asia/Y"), [Tick::Zone]);
    assert_eq!(c.observe(mid + 3606, 7200, 12_000, "Asia/Y"), [Tick::Jump, Tick::Minute, Tick::Hour]);

    let snap = |v: &[(&str, u64)]| -> Snapshot { v.iter().map(|(n, s)| (n.to_string(), (false, *s, Some(1)))).collect() };
    let mut w = FileWatch::default();
    assert!(w.observe("pat", "Docs", Ok(snap(&[("a", 1)]))).is_empty());
    assert_eq!(w.observe("pat", "Docs", Ok(snap(&[("a", 2), ("b", 1)]))), [("a".to_string(), "modified"), ("b".to_string(), "created")]);
    assert_eq!(w.observe("pat", "Docs", Ok(snap(&[("b", 1)]))), [("a".to_string(), "removed")]);
    assert!(w.observe("pat", "Docs", Err("not allowed".into())).is_empty());
    assert!(w.observe("pat", "Docs", Ok(snap(&[("c", 1)]))).is_empty(), "a folder that comes back is looked at afresh");
}

#[test]
fn the_link_state_is_read_from_the_interface_table() {
    let d = std::env::temp_dir().join(format!("iyakki-evsrc-sys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (n, st) in [("lo", "unknown"), ("eth0", "down")] {
        std::fs::create_dir_all(d.join("class/net").join(n)).unwrap();
        std::fs::write(d.join("class/net").join(n).join("operstate"), st).unwrap();
    }
    assert_eq!(link_state(&d), "offline");
    std::fs::write(d.join("class/net/eth0/operstate"), "up\n").unwrap();
    assert_eq!(link_state(&d), "online");
    assert_eq!(link_state(&d.join("nothing")), "offline");
    let _ = std::fs::remove_dir_all(&d);
}

struct Fixed(Mutex<Vec<String>>);
impl SettingChanges for Fixed {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[test]
fn the_sources_look_when_an_app_asks_and_each_difference_is_announced_once() {
    let s = svc("pull");
    std::fs::create_dir_all(crate::scope::home_root_for_writes("pat", s.dir()).join("Docs")).unwrap();
    let hub = s.app_events.clone();
    let changes = Arc::new(Fixed(Mutex::new(Vec::new())));
    let state = Arc::new(Mutex::new(("online", "{}".to_string(), 1_000_000i64, "UTC".to_string(), Vec::<(String, String, bool, bool)>::new(), vec![("a.txt".to_string(), 1u64)])));
    let steady = Arc::new(AtomicU64::new(0));
    let (s1, s2, s3, s4, s5, st) = (state.clone(), state.clone(), state.clone(), state.clone(), state.clone(), steady.clone());
    let inputs = Inputs {
        settings: changes.clone(),
        flows: Box::new(move |_| Some(s1.lock().unwrap().4.clone())),
        link: Box::new(move || s2.lock().unwrap().0),
        prefs: Box::new(move |_| (s3.lock().unwrap().1.clone(), String::new())),
        clock: Box::new({
            let st = steady.clone();
            move || {
                let g = s4.lock().unwrap();
                (g.2, 0, st.load(Ordering::SeqCst), g.3.clone())
            }
        }),
        list: Box::new(move |_, folder| {
            assert_eq!(folder, "Docs");
            let files = s5.lock().unwrap().5.clone();
            Ok(iyakki_core_api::Listing { dir: "Docs".into(), entries: files.into_iter().map(|(name, size)| iyakki_core_api::FileEntry { name, is_dir: false, size, modified: Some(1) }).collect(), truncated: false })
        }),
        machine: Box::new(|| MachineFacts::default()),
        devices: Box::new(|| DeviceFacts::default()),
        person: Box::new(|_| PersonFacts::default()),
        steady_ms: Box::new(move || st.load(Ordering::SeqCst)),
    };
    let ev = SourcedEvents::new(hub.clone(), inputs);
    listen(&ev, "pat", &["settings.changed", "flows.run", "connectivity.changed", "display.changed", "clock.minute", "clock.changed", "locale.changed"]);
    ev.register("pat", "sample", "files.changed", "Docs").unwrap_or_else(|e| panic!("{e}"));
    ev.start("pat", "sample");
    // everything changes
    changes.0.lock().unwrap().push("time.zone".into());
    {
        let mut g = state.lock().unwrap();
        g.0 = "offline";
        g.1 = r#"{"theme":"light"}"#.into();
        g.4 = vec![("r1".into(), "Backup".into(), true, false)];
        g.5.push(("b.txt".into(), 3));
    }
    steady.store(2_000, Ordering::SeqCst);
    // the first look after start only remembered the quiet state (start looked once: nothing changed yet); this look sees the changes of flows
    // and the folder only after a baseline, so look twice with the changes in between
    ev.look();
    {
        let mut g = state.lock().unwrap();
        g.1 = r#"{"theme":"dark"}"#.into();
        g.2 += 61;
        g.4.push(("r2".into(), "Sync".into(), true, true));
        g.5.push(("c.txt".into(), 9));
    }
    steady.store(63_000, Ordering::SeqCst);
    let got: Vec<String> = ev.poll("pat", "sample", 100).iter().map(show).collect();
    assert!(got.contains(&"settings.changed setting=time.zone".to_string()), "{got:?}");
    assert!(got.contains(&"connectivity.changed state=offline".to_string()), "{got:?}");
    assert!(got.contains(&"display.changed what=theme".to_string()), "{got:?}");
    assert!(got.contains(&"flows.run flow=Sync outcome=finished".to_string()), "{got:?}");
    assert!(got.contains(&"files.changed change=created folder=Docs name=c.txt".to_string()), "{got:?}");
    assert!(got.contains(&"clock.minute".to_string()), "{got:?}");
    // no secret or value anywhere: only words, names and counts
    assert!(got.iter().all(|l| !l.contains("light") && !l.contains("dark")), "{got:?}");
    // a look less than a second later does nothing (a noisy source is paced)
    let before = ev.audit().len();
    ev.poll("pat", "sample", 10);
    assert_eq!(ev.audit().len(), before);
}

#[test]
fn a_folder_the_file_policy_refuses_cannot_be_watched() {
    let s = svc("policy");
    s.app_events.register("pat", "sample", "files.changed", "../etc").unwrap_err();
    let e = s.app_events.register("pat", "sample", "files.changed", "NoSuchFolderAnywhere").unwrap_err();
    assert!(e.contains("file policy"), "{e}");
}

#[test]
fn the_settings_contract_reports_the_keys_that_changed_whoever_changed_them() {
    use iyakki_core_api::contract::{CallContext, SettingValue, Settings as _};
    let s = svc("settings");
    let cs = crate::contract_local::CoreSettings::new(s.settings.clone());
    assert!(cs.take_changes().is_empty());
    cs.set(&CallContext::local("root", true), "show_all_ai", SettingValue::Bool(true), None).unwrap();
    // a change by a screen (straight into the store)
    s.settings.lock().unwrap().update(|x| x.time.no_ntp = true).unwrap();
    let mut got = cs.take_changes();
    got.sort();
    assert_eq!(got, ["show_all_ai", "time.no_ntp"]);
    assert!(cs.take_changes().is_empty(), "once");
    // and they reach an app as names only
    s.app_events.register("pat", "sample", "settings.changed", "time.").unwrap();
    let (n, p) = ae::from_setting_change("time.no_ntp");
    emit(&*s.app_events, n, None, p);
    let got = s.app_events.start("pat", "sample");
    assert_eq!(show(&got[0]), "settings.changed setting=time.no_ntp");
}

#[test]
fn a_handler_leaves_a_note_in_the_inbox_and_a_tool_request_waits_for_the_person() {
    use iyakki_core_api::appactions::AppActions;
    let s = svc("actions");
    let a = crate::appactions_real::AgentActions::new(s.clone());
    // sample is a built-in app that holds notify, flows.start and tools.request; another app holds nothing
    a.notify("pat", "sample", "The network went away").unwrap();
    let box_ = s.inbox.list("pat");
    assert!(box_.iter().any(|n| n.title == "sample" && n.body == "The network went away"), "{box_:?}");
    assert!(a.start_flow("pat", "sample", "Nothing").unwrap_err().contains("flows engine") , "no engine runs for a person who is not signed in");
    let id = a.request_tool("pat", "sample", "wifi_status", "{}").unwrap();
    assert_eq!(a.pending("pat").len(), 1);
    assert!(a.pending("sam").is_empty());
    assert!(a.request_tool("pat", "other-app", "wifi_status", "{}").unwrap_err().contains("tools.request"));
    // a restart: a new object over the same data folder still has the request, once
    let again = crate::appactions_real::AgentActions::new(s.clone());
    assert_eq!(again.pending("pat").len(), 1);
    assert_eq!(again.pending("pat")[0].id, id);
    assert_eq!(a.take("pat", &id).unwrap().tool, "wifi_status");
    assert!(crate::appactions_real::AgentActions::new(s.clone()).pending("pat").is_empty(), "the answer was kept too");
    assert!(a.audit().iter().all(|l| !l.contains("network went away") && !l.contains("{}")), "{:?}", a.audit());
}

#[test]
fn a_store_app_with_a_built_in_name_gets_only_what_the_person_approved_for_it() {
    use iyakki_core_api::appcaps::{record_approval, ApprovalBook, CapabilityRequest};
    let s = svc("scopes");
    let sc = AgentScopes { dir: s.dir().to_path_buf() };
    assert!(sc.holds("pat", "sample", "events.network"), "the built-in table");
    let store = crate::store::Store::open(s.dir());
    let mut book = crate::store::approvals::FileBook(&store);
    let reqs = vec![CapabilityRequest { name: "events.power".into(), why: "x".into(), param: None, extra: Default::default() }];
    book.put("tile/user/pat/sample", record_approval("pat", "sample", "1.0", "abc", 5, &reqs));
    assert!(sc.holds("pat", "sample", "events.power"), "what was approved");
    assert!(!sc.holds("pat", "sample", "events.network"), "the built-in table does not apply to an installed app of that name");
}

#[test]
fn power_memory_heat_inbox_and_vpn_are_announced_when_they_change() {
    let mut m = MachineWatch::default();
    let facts = |ac: bool, pct: u32, dis: bool, mem: &str, thr: u64| MachineFacts { ac: Some(ac), percent: Some(pct), discharging: dis, memory: mem.into(), throttle: thr };
    assert!(m.observe(facts(true, 80, false, "normal", 5)).is_empty(), "the first look only remembers");
    let names = |v: Vec<(&'static str, Payload)>| v.into_iter().map(|(n, p)| format!("{n} {}", p.values().map(|v| match v { Val::Text(t) => t.clone(), Val::Int(n) => n.to_string(), Val::Bool(b) => b.to_string() }).collect::<Vec<_>>().join(","))).collect::<Vec<_>>();
    assert_eq!(names(m.observe(facts(false, 14, true, "high", 9))), ["power.source battery", "power.battery_low 14", "memory.pressure high", "thermal.changed throttled"]);
    assert!(m.observe(facts(false, 13, true, "high", 9)).iter().all(|(n, _)| *n == "thermal.changed"), "low is told once, until it recovers");
    assert_eq!(names(m.observe(facts(true, 30, false, "normal", 9))), ["power.source ac", "memory.pressure normal"]);
    // the kernel's words
    assert_eq!(memory_level("some avg10=0.50 avg60=0.2 avg300=0.1 total=1\nfull avg10=0.00"), "normal");
    assert_eq!(memory_level("some avg10=12.00 avg60=1"), "high");
    assert_eq!(memory_level("some avg10=70.00"), "critical");
    assert_eq!(memory_level(""), "normal");
    let d = std::env::temp_dir().join(format!("iyakki-evsrc-thr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (cpu, n) in [("cpu0", "3"), ("cpu1", "4")] {
        std::fs::create_dir_all(d.join("devices/system/cpu").join(cpu).join("thermal_throttle")).unwrap();
        std::fs::write(d.join("devices/system/cpu").join(cpu).join("thermal_throttle/core_throttle_count"), n).unwrap();
    }
    assert_eq!(throttle_total(&d), 7);
    let _ = std::fs::remove_dir_all(&d);

    let mut p = PersonWatch::default();
    assert!(p.observe("pat", PersonFacts { inbox: 1, vpn: None }).is_empty());
    assert_eq!(names(p.observe("pat", PersonFacts { inbox: 3, vpn: Some(true) })), ["inbox.pending 3", "vpn.changed vpn,up"]);
    assert_eq!(names(p.observe("pat", PersonFacts { inbox: 3, vpn: None })), ["vpn.changed vpn,down"]);
    assert!(p.observe("sam", PersonFacts::default()).is_empty(), "each person has their own memory");
}

#[test]
fn the_pulled_power_and_inbox_sources_reach_an_app_that_listens() {
    let s = svc("machine");
    let facts = Arc::new(Mutex::new((MachineFacts { ac: Some(true), memory: "normal".into(), ..Default::default() }, PersonFacts::default())));
    let (f1, f2) = (facts.clone(), facts.clone());
    let steady = Arc::new(AtomicU64::new(0));
    let st = steady.clone();
    let inputs = Inputs {
        settings: Arc::new(Fixed(Mutex::new(vec![]))),
        flows: Box::new(|_| None),
        link: Box::new(|| "online"),
        prefs: Box::new(|_| (String::new(), String::new())),
        clock: Box::new(|| (1_000_000, 0, 0, "UTC".into())),
        list: Box::new(|_, _| Err("none".into())),
        machine: Box::new(move || f1.lock().unwrap().0.clone()),
        devices: Box::new(|| DeviceFacts::default()),
        person: Box::new(move |_| f2.lock().unwrap().1.clone()),
        steady_ms: Box::new(move || st.load(Ordering::SeqCst)),
    };
    let ev = SourcedEvents::new(s.app_events.clone(), inputs);
    s.app_events.set_flag("x", false);
    // SAMPLE holds power but not the inbox scope in the table: only the power events are heard; the inbox one is refused at registration
    ev.register("pat", "sample", "power.source", "").unwrap();
    assert!(ev.register("pat", "sample", "inbox.pending", "").unwrap_err().contains("events.inbox"));
    ev.start("pat", "sample");
    facts.lock().unwrap().0.ac = Some(false);
    steady.store(5_000, Ordering::SeqCst);
    let got: Vec<String> = ev.poll("pat", "sample", 10).iter().map(show).collect();
    assert_eq!(got, ["power.source source=battery"]);
}

#[test]
fn volumes_sound_cards_and_cameras_are_announced_by_name_when_they_come_and_go() {
    let mounts = "/dev/sda1 / ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sdb1 /media/pat/STICK vfat rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n";
    assert_eq!(block_mounts(mounts).into_iter().collect::<Vec<_>>(), ["/", "/media/pat/STICK"]);
    let cards = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7e00000 irq 33\n 1 [Headset        ]: USB-Audio - Logitech Headset\n";
    assert_eq!(sound_cards(cards).into_iter().collect::<Vec<_>>(), ["HDA-Intel - HDA Intel PCH", "USB-Audio - Logitech Headset"]);
    let mut w = DeviceWatch::default();
    let f = |v: &[&str], a: &[&str], c: &[&str]| DeviceFacts { volumes: v.iter().map(|x| x.to_string()).collect(), audio: a.iter().map(|x| x.to_string()).collect(), cameras: c.iter().map(|x| x.to_string()).collect() };
    assert!(w.observe(f(&["/"], &["PCH"], &[])).is_empty());
    let got: Vec<String> = w.observe(f(&["/", "/media/pat/STICK"], &["PCH", "Headset"], &["HD Webcam"])).iter().map(|(n, p)| format!("{n} {}", p.values().filter_map(|v| v.text()).collect::<Vec<_>>().join(","))).collect();
    assert_eq!(got, ["storage.volume mounted,/media/pat/STICK", "audio.devices added,Headset,speaker", "camera.devices added,HD Webcam"]);
    let got: Vec<String> = w.observe(f(&["/"], &["PCH"], &[])).iter().map(|(n, _)| n.to_string()).collect();
    assert_eq!(got, ["storage.volume", "audio.devices", "camera.devices"]);
}


/// Inputs with a link, a steady clock and the real settings store of a `Services`, everything else quiet.
fn quiet_inputs(s: &Arc<Services>, link: Arc<Mutex<&'static str>>, steady: Arc<AtomicU64>) -> Inputs {
    let (l, st) = (link, steady);
    Inputs {
        settings: Arc::new(StoreChanges::new(s.settings.clone())),
        flows: Box::new(|_| None),
        link: Box::new(move || *l.lock().unwrap()),
        prefs: Box::new(|_| (String::new(), String::new())),
        clock: Box::new(|| (1_000_000, 0, 0, "UTC".into())),
        list: Box::new(|_, _| Err("none".into())),
        machine: Box::new(MachineFacts::default),
        devices: Box::new(DeviceFacts::default),
        person: Box::new(|_| PersonFacts::default()),
        steady_ms: Box::new(move || st.load(Ordering::SeqCst)),
    }
}

#[test]
fn what_happens_while_the_app_is_closed_is_queued_by_the_cores_own_tick_and_arrives_late() {
    // KI-1011: the sources were looked at only when an app polled, so a closed app missed the link going down and up
    let s = svc("closed");
    let link = Arc::new(Mutex::new("online"));
    let steady = Arc::new(AtomicU64::new(0));
    let ev = SourcedEvents::new(s.app_events.clone(), quiet_inputs(&s, link.clone(), steady.clone()));
    ev.tick(); // nobody is registered: nothing is looked at
    ev.register("pat", "sample", "connectivity.changed", "").unwrap();
    ev.start("pat", "sample");
    ev.poll("pat", "sample", 5);
    ev.stop("pat", "sample"); // the app is closed: nobody polls from here on
    steady.store(2_000, Ordering::SeqCst);
    ev.tick(); // the baseline look
    *link.lock().unwrap() = "offline";
    steady.store(4_000, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.list("pat", "sample")[0].pending, 1, "Settings > App events shows what waits");
    *link.lock().unwrap() = "online";
    steady.store(6_000, Ordering::SeqCst);
    ev.tick();
    // the state event coalesces: the latest is what waits
    assert_eq!(ev.list("pat", "sample")[0].pending, 1);
    let got = ev.start("pat", "sample");
    assert_eq!(got.len(), 1);
    assert!(got[0].late, "it happened while the app was closed");
    assert_eq!(show(&got[0]), "connectivity.changed state=online");
    // a tick less than a second after the last look does nothing
    *link.lock().unwrap() = "offline";
    steady.store(6_500, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.list("pat", "sample")[0].pending, 0);
}

#[test]
fn a_setting_changed_by_dev_config_or_a_page_is_announced_by_its_dotted_name_and_never_its_value() {
    // KI-1012: only the contract's seven keys were watched; `time.hour12` is a field of the settings file and is not one of them
    let s = svc("storechg");
    let steady = Arc::new(AtomicU64::new(0));
    let ev = SourcedEvents::new(s.app_events.clone(), quiet_inputs(&s, Arc::new(Mutex::new("online")), steady.clone()));
    ev.register("pat", "sample", "settings.changed", "time.").unwrap();
    ev.start("pat", "sample");
    ev.tick(); // the baseline
    s.settings.lock().unwrap().update(|x| x.time.hour12 = true).unwrap();
    s.settings.lock().unwrap().update(|x| x.time.ntp_server = "hunter2.example".into()).unwrap();
    steady.store(2_000, Ordering::SeqCst);
    ev.tick();
    let mut got: Vec<String> = ev.poll("pat", "sample", 10).iter().map(show).collect();
    got.sort();
    assert_eq!(got, ["settings.changed setting=time.hour12", "settings.changed setting=time.ntp_server"]);
    assert!(got.iter().all(|l| !l.contains("hunter2")), "a name, never a value");
    // a change outside the prefix is not heard by this registration
    s.settings.lock().unwrap().update(|x| x.show_all_ai = true).unwrap();
    steady.store(4_000, Ordering::SeqCst);
    ev.tick();
    assert!(ev.poll("pat", "sample", 10).is_empty());
    assert!(changed_leaves(&BTreeMap::from([("a".to_string(), 1)]), &BTreeMap::from([("b".to_string(), 1)])).len() == 2);
}

fn approve(s: &Arc<Services>, inst: &str, app: &str, caps: &[&str]) {
    use iyakki_core_api::appcaps::{record_approval, ApprovalBook, CapabilityRequest};
    let store = crate::store::Store::open(s.dir());
    let mut book = crate::store::approvals::FileBook(&store);
    let reqs: Vec<CapabilityRequest> = caps.iter().map(|c| CapabilityRequest { name: (*c).into(), why: "x".into(), param: None, extra: Default::default() }).collect();
    book.put(inst, record_approval("pat", app, "1.0", "abc", 5, &reqs));
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn the_app_is_told_what_it_was_given_what_was_taken_away_and_that_its_registration_ended() {
    let s = svc("perm");
    let hub = &*s.app_events;
    // granted at install by the person
    approve(&s, "tile/user/pat/notes", "notes", &["events.network", "events.power"]);
    permissions_told(hub, "pat", "notes", &[], &names(&["events.network", "events.power"]), "person", false);
    let got = hub.start("pat", "notes");
    assert_eq!(got.len(), 1);
    let line = show(&got[0]);
    assert!(line.contains("by=person") && line.contains("granted=events.network,events.power") && line.contains("held=2") && line.contains("restart=false"), "{line}");
    assert!(got[0].late, "nobody was listening yet");
    hub.register("pat", "notes", "connectivity.changed", "").unwrap();
    hub.register("pat", "notes", "power.source", "").unwrap();
    // an administrator takes one away in Settings > Apps access: the broker's own revoke, then the app is told
    assert!(crate::accessbroker_revoke::revoke(s.dir(), "tile/user/pat/notes", "events.network").unwrap());
    permissions_told(hub, "pat", "notes", &names(&["events.network", "events.power"]), &names(&["events.power"]), "administrator", false);
    let got = hub.poll("pat", "notes", 10);
    assert_eq!(got.len(), 1);
    let line = show(&got[0]);
    assert!(line.contains("revoked=events.network") && line.contains("dropped=connectivity.changed") && line.contains("by=administrator") && line.contains("held=1"), "{line}");
    assert!(!got[0].late, "the app was running");
    assert_eq!(hub.list("pat", "notes").iter().map(|r| r.event.as_str()).collect::<Vec<_>>(), ["power.source"], "the registration ended with its scope");
    // a company policy removes the rest; nothing changed for a repeat
    permissions_told(hub, "pat", "notes", &names(&["events.power"]), &names(&["events.power"]), "policy", false);
    assert!(hub.poll("pat", "notes", 10).is_empty(), "no change, nothing told");
    assert!(crate::accessbroker_revoke::revoke(s.dir(), "tile/user/pat/notes", "events.power").unwrap());
    permissions_told(hub, "pat", "notes", &names(&["events.power"]), &[], "policy", true);
    let got = hub.poll("pat", "notes", 10);
    assert!(show(&got[0]).contains("by=policy") && show(&got[0]).contains("restart=true") && show(&got[0]).contains("dropped=power.source"), "{}", show(&got[0]));
}

#[test]
fn an_update_that_asks_for_one_more_capability_tells_the_app_and_no_other_app_hears_it() {
    let s = svc("permupd");
    let hub = &*s.app_events;
    approve(&s, "tile/user/pat/notes", "notes", &["events.network"]);
    approve(&s, "tile/user/pat/other", "other", &["events.network"]);
    hub.start("pat", "notes");
    hub.start("pat", "other");
    permissions_told(hub, "pat", "notes", &names(&["events.network"]), &names(&["events.network", "events.files"]), "store", true);
    let got = hub.poll("pat", "notes", 10);
    let line = show(&got[0]);
    assert!(line.contains("granted=events.files") && line.contains("by=store") && line.contains("restart=true"), "{line}");
    assert!(hub.poll("pat", "other", 10).is_empty(), "another app hears nothing");
    // another person's copy of the same app hears nothing either
    assert!(hub.start("sam", "notes").is_empty());
}

#[test]
fn a_change_made_while_the_device_was_off_is_there_after_a_restart_of_the_engine() {
    let s = svc("permoff");
    approve(&s, "tile/user/pat/notes", "notes", &["events.network"]);
    permissions_told(&*s.app_events, "pat", "notes", &[], &names(&["events.network"]), "person", false);
    let again = iyakki_core_local::appevents::local_app_events_with(s.dir(), Box::new(AgentScopes { dir: s.dir().to_path_buf() }));
    let got = again.start("pat", "notes");
    assert_eq!(got.len(), 1);
    assert!(got[0].late && show(&got[0]).contains("granted=events.network"), "{}", show(&got[0]));
    let (held, last) = again.permissions("pat", "notes");
    assert_eq!(held, ["events.network"]);
    assert!(last.unwrap().contains("by person"), "kept for the chat tool");
}


#[test]
fn an_app_that_went_home_without_stopping_gets_what_happened_meanwhile_marked_late_through_the_real_path() {
    // KI-1011 (candidate 3): the closed app was delivered the event but not marked late. The person goes Home: the app is never told to
    // stop, it just does not poll any more; the core's tick queues the link change; the next start must say it is late.
    let s = svc("late");
    let clock = Arc::new(AtomicU64::new(100_000));
    let c = clock.clone();
    let hub = iyakki_core_local::appevents::local_app_events_clocked(s.dir(), Box::new(AgentScopes { dir: s.dir().to_path_buf() }), Box::new(move || c.load(Ordering::SeqCst)));
    let link = Arc::new(Mutex::new("online"));
    let steady = Arc::new(AtomicU64::new(0));
    let ev = SourcedEvents::new(hub, quiet_inputs(&s, link.clone(), steady.clone()));
    ev.register("pat", "sample", "connectivity.changed", "").unwrap();
    ev.start("pat", "sample");
    ev.poll("pat", "sample", 5);
    // the app stops polling (Home); the tick baseline, then the link goes down and up
    clock.store(101_000, Ordering::SeqCst);
    steady.store(1_000, Ordering::SeqCst);
    ev.tick();
    *link.lock().unwrap() = "offline";
    clock.store(110_000, Ordering::SeqCst);
    steady.store(10_000, Ordering::SeqCst);
    ev.tick();
    *link.lock().unwrap() = "online";
    clock.store(120_000, Ordering::SeqCst);
    steady.store(20_000, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.list("pat", "sample")[0].pending, 1);
    clock.store(130_000, Ordering::SeqCst);
    let got = ev.start("pat", "sample");
    assert_eq!(got.len(), 1);
    assert_eq!(show(&got[0]), "connectivity.changed state=online");
    assert!(got[0].late, "it happened while the app was away");
    // the app polls on: the next change reaches it at once, not late
    *link.lock().unwrap() = "offline";
    clock.store(131_000, Ordering::SeqCst);
    steady.store(31_000, Ordering::SeqCst);
    ev.tick();
    clock.store(131_100, Ordering::SeqCst);
    let got = ev.poll("pat", "sample", 5);
    assert_eq!(got.len(), 1);
    assert!(!got[0].late);
}
