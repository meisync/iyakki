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
        folder_dir: Box::new(|_, f| std::path::PathBuf::from(f)),
        machine: Box::new(|| MachineFacts::default()),
        devices: Box::new(|| DeviceFacts::default()),
        person: Box::new(|_| PersonFacts::default()),
        more: Box::new(crate::appevent_more::MoreFacts::default),
        more_person: Box::new(|_| crate::appevent_more::MorePersonFacts::default()),
        extra: Box::new(|_| crate::appevent_extra::ExtraFacts::default()),
        perm: Box::new(crate::appevent_perm::PermFacts::default),
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
        folder_dir: Box::new(|_, f| std::path::PathBuf::from(f)),
        machine: Box::new(move || f1.lock().unwrap().0.clone()),
        devices: Box::new(|| DeviceFacts::default()),
        person: Box::new(move |_| f2.lock().unwrap().1.clone()),
        more: Box::new(crate::appevent_more::MoreFacts::default),
        more_person: Box::new(|_| crate::appevent_more::MorePersonFacts::default()),
        extra: Box::new(|_| crate::appevent_extra::ExtraFacts::default()),
        perm: Box::new(crate::appevent_perm::PermFacts::default),
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
        folder_dir: Box::new(|_, f| std::path::PathBuf::from(f)),
        machine: Box::new(MachineFacts::default),
        devices: Box::new(DeviceFacts::default),
        person: Box::new(|_| PersonFacts::default()),
        more: Box::new(crate::appevent_more::MoreFacts::default),
        more_person: Box::new(|_| crate::appevent_more::MorePersonFacts::default()),
        extra: Box::new(|_| crate::appevent_extra::ExtraFacts::default()),
        perm: Box::new(crate::appevent_perm::PermFacts::default),
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

#[test]
fn the_further_sources_reach_an_app_that_listens_through_the_one_look() {
    use crate::appevent_more::{MoreFacts, MorePersonFacts};
    let s = svc("more");
    let facts = Arc::new(Mutex::new(MoreFacts::default()));
    let f1 = facts.clone();
    let mut inputs = quiet_inputs(&s, Arc::new(Mutex::new("online")), Arc::new(AtomicU64::new(0)));
    inputs.more = Box::new(move || f1.lock().unwrap().clone());
    inputs.more_person = Box::new(|_| MorePersonFacts::default());
    let ev = SourcedEvents::new(s.app_events.clone(), inputs);
    // SAMPLE holds the power scope in the signed table
    ev.register("pat", "sample", "power.sleep", "").unwrap();
    ev.register("pat", "sample", "power.shutdown_pending", "").unwrap();
    ev.start("pat", "sample");
    ev.look(); // the first look only remembers
    facts.lock().unwrap().suspends = 1;
    facts.lock().unwrap().shutdown_in = Some(30);
    ev.look();
    let got: Vec<String> = ev.poll("pat", "sample", 10).iter().map(show).collect();
    assert_eq!(got, ["power.shutdown_pending in_seconds=30", "power.sleep phase=wake"]);
}

/// F-SDK-208: the kernel's own file watch makes the file source look when a file changes, not at the next once-a-second look. The steady clock
/// never moves here, so the ordinary look (which is held back to one a second) cannot be what finds the file.
#[cfg(target_os = "linux")]
#[test]
fn a_file_made_in_a_watched_folder_is_announced_by_the_kernel_watch_without_waiting_for_the_next_look() {
    let s = svc("inotify");
    let home = crate::scope::home_root_for_writes("pat", s.dir());
    std::fs::create_dir_all(home.join("Docs")).unwrap();
    let mut inputs = quiet_inputs(&s, Arc::new(Mutex::new("online")), Arc::new(AtomicU64::new(0)));
    let root = home.clone();
    inputs.list = Box::new(move |_, folder| iyakki_core_local::homebrowse::list(&root, folder));
    let root = home.clone();
    inputs.folder_dir = Box::new(move |_, folder| crate::fnotify::folder_path(&root, folder));
    let ev = Arc::new(SourcedEvents::new(s.app_events.clone(), inputs));
    ev.register("pat", "sample", "files.changed", "Docs").unwrap();
    ev.start("pat", "sample");
    ev.tick(); // the baseline look: the folder is empty; the next ordinary look is a second of the (frozen) steady clock away
    // the watch is on before the file is made (the ticker thread would set it up in its own time, and a file made before that is no change of a watch)
    assert_eq!(ev.watch_listened_folders(), 1, "the kernel watch takes the folder");
    SourcedEvents::spawn_ticker(&ev);
    std::fs::write(home.join("Docs/new.txt"), "x").unwrap();
    // the hub is polled directly (a poll of the sourced facet would itself look): the event can only have come from the watch
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut got = Vec::new();
    while got.is_empty() && std::time::Instant::now() < end {
        got = s.app_events.poll("pat", "sample", 10);
        std::thread::yield_now();
    }
    let lines: Vec<String> = got.iter().map(show).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("files.changed") && lines[0].contains("name=new.txt") && lines[0].contains("change=created"), "{lines:?}");
}

#[test]
fn a_finished_install_is_told_as_a_transfer_with_the_apps_name_only() {
    let s = svc("xfer");
    let hub = &*s.app_events;
    listen(hub, "pat", &["transfer.finished"]);
    hub.start("pat", "sample");
    crate::appevent_more::transfer_finished(hub, "pat", "install", "Notes", true);
    crate::appevent_more::transfer_finished(hub, "pat", "install", "Notes", false);
    let got: Vec<String> = hub.poll("pat", "sample", 50).iter().map(show).collect();
    assert_eq!(got, ["transfer.finished kind=install name=Notes outcome=finished", "transfer.finished kind=install name=Notes outcome=failed"]);
}

#[test]
fn a_media_download_that_ended_is_told_as_a_transfer_by_the_files_name_and_a_failed_one_by_a_fixed_word() {
    let s = svc("mediadl");
    let hub = &*s.app_events;
    listen(hub, "pat", &["transfer.finished"]);
    hub.start("pat", "sample");
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-evsrc-{}-mediadl-files", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let base = crate::mediadl::tests::serve(vec![("/a/Song.mp3", "audio/mpeg", crate::mediadl::tests::tiny_mp3())]);
    let st = crate::mediadl::tests::streamer();
    let policy = crate::mediadl::tests::policy_for(&dir);
    let choice = crate::mediadl::Choice::default();
    let progress = crate::mediadl::Progress::default();
    crate::mediadl::run_blocking(&s, &st, &policy, "pat", &format!("{base}/a/Song.mp3"), &dir, &choice, 1 << 20, &progress).unwrap();
    assert!(crate::mediadl::run_blocking(&s, &st, &policy, "pat", &format!("{base}/a/missing.mp3"), &dir, &choice, 1 << 20, &progress).is_err());
    let got: Vec<String> = hub.poll("pat", "sample", 50).iter().map(show).collect();
    assert_eq!(got, ["transfer.finished kind=download name=Song.mp3 outcome=finished", "transfer.finished kind=download name=media outcome=failed"]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------- UT2: the further sources, end to end
// The pulled facts (a fake machine) go through the one look of `SourcedEvents` into the hub and out at the app's poll, as on a machine.

use crate::appevent_more::{MoreFacts, MorePersonFacts, PeerFact};

/// A look source whose machine-wide and per-person facts the test edits between looks.
fn more_engine(tag: &str, names: &[&str]) -> (Arc<Services>, SourcedEvents, Arc<Mutex<MoreFacts>>, Arc<Mutex<MorePersonFacts>>) {
    let s = svc(tag);
    // the person approved SAMPLE for the groups these events belong to (an installed app of that name gets only what was approved)
    approve(&s, "tile/user/pat/sample", "sample", &["events.network", "events.power", "events.security", "events.reminders", "events.display", "events.locale", "events.devices", "events.storage", "events.updates", "events.models", "events.inbox", "events.voice"]);
    let facts = Arc::new(Mutex::new(MoreFacts::default()));
    let person = Arc::new(Mutex::new(MorePersonFacts::default()));
    let (f1, p1) = (facts.clone(), person.clone());
    let mut inputs = quiet_inputs(&s, Arc::new(Mutex::new("online")), Arc::new(AtomicU64::new(0)));
    inputs.more = Box::new(move || f1.lock().unwrap().clone());
    inputs.more_person = Box::new(move |_| p1.lock().unwrap().clone());
    let ev = SourcedEvents::new(s.app_events.clone(), inputs);
    ev.set_flag("wake_word", true); // the experimental wake phrase is switched on, as its own setting does
    for n in names {
        ev.register("pat", "sample", n, "").unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    ev.start("pat", "sample");
    ev.look(); // the first look only remembers
    (s, ev, facts, person)
}

fn heard(ev: &SourcedEvents) -> Vec<String> {
    ev.poll("pat", "sample", 50).iter().map(show).collect()
}

/// F-SDK-204: every clause of the row reaches an app, in order, once each, and nothing at a look that found no change.
#[test]
fn connectivity_peers_sleep_shutdown_and_quiet_hours_reach_an_app_once_each() {
    let (s, ev, facts, _) = more_engine("e2e204", &["connectivity.changed", "power.sleep", "power.shutdown_pending", "quiet_hours.changed", "peers.changed", "peers.battery"]);
    let set = |f: &dyn Fn(&mut MoreFacts)| f(&mut facts.lock().unwrap());
    set(&|f| f.metered = true);
    ev.look();
    assert_eq!(heard(&ev), ["connectivity.changed state=metered"]);
    ev.look();
    assert!(heard(&ev).is_empty(), "an unchanged look says nothing");
    set(&|f| f.captive = true);
    ev.look();
    assert_eq!(heard(&ev), ["connectivity.changed state=captive_portal"]);
    set(&|f| {
        f.captive = false;
        f.metered = false;
    });
    ev.look();
    assert_eq!(heard(&ev), ["connectivity.changed state=online"]);
    // the sleep side is pushed by the service just before it suspends, the wake side is found by the count of suspends
    crate::appevent_more::going_to_sleep(&*s.app_events);
    set(&|f| f.suspends = 1);
    ev.look();
    assert_eq!(heard(&ev), ["power.sleep phase=sleep", "power.sleep phase=wake"]);
    set(&|f| f.shutdown_in = Some(60));
    ev.look();
    ev.look();
    assert_eq!(heard(&ev), ["power.shutdown_pending in_seconds=60"], "told once while it stays scheduled");
    set(&|f| f.quiet = Some(false));
    ev.look(); // the setting appears: no change yet
    set(&|f| f.quiet = Some(true));
    ev.look();
    assert_eq!(heard(&ev), ["quiet_hours.changed state=on"]);
    set(&|f| {
        f.peers.insert("studio".into(), PeerFact { online: true, battery: Some(80) });
    });
    ev.look();
    assert_eq!(heard(&ev), ["peers.changed change=paired device=studio"], "a new computer is told as paired; its battery is told when it changes");
    set(&|f| {
        f.peers.insert("studio".into(), PeerFact { online: true, battery: Some(75) });
    });
    ev.look();
    assert_eq!(heard(&ev), ["peers.battery device=studio percent=75"]);
    set(&|f| {
        f.peers.insert("studio".into(), PeerFact { online: false, battery: Some(79) });
    });
    ev.look();
    assert_eq!(heard(&ev), ["peers.changed change=offline device=studio", "peers.battery device=studio percent=79"]);
    set(&|f| f.peers.clear());
    ev.look();
    assert_eq!(heard(&ev), ["peers.changed change=forgotten device=studio"]);
}

/// F-SDK-205: the audit log and the certificate on a (fake) disk, read by the real readers, become `security.alert` and `cert.expiring`.
#[test]
fn failed_sign_ins_new_devices_and_an_expiring_certificate_reach_an_app_from_the_real_readers() {
    let (_s, ev, facts, person) = more_engine("e2e205", &["security.alert", "cert.expiring"]);
    let line = |e: &str| format!(r#"{{"ts":1,"user":"-","event":"{e}","detail":"x","prev":"","hash":""}}"#);
    let mut log = vec![line("login.failed")];
    let totals = |log: &[String]| crate::appevent_feed::audit_totals(&log.join("\n"));
    let feed = |log: &[String]| {
        let t = totals(log);
        let mut p = person.lock().unwrap();
        p.failed_signins = t.failed_signins;
        p.new_paired_devices = t.new_paired_devices;
    };
    feed(&log);
    ev.look();
    assert_eq!(heard(&ev), ["security.alert count=1 kind=failed_signin"]);
    log.extend([line("login.failed"), line("login.failed"), line("devicekey.enroll")]);
    feed(&log);
    ev.look();
    let mut got = heard(&ev);
    got.sort();
    assert_eq!(got, ["security.alert count=1 kind=new_paired_device", "security.alert count=2 kind=failed_signin"]);
    ev.look();
    assert!(heard(&ev).is_empty());
    person.lock().unwrap().new_place_unlocks = 1;
    ev.look();
    assert_eq!(heard(&ev), ["security.alert count=1 kind=vault_unlock_new_place"]);
    // a real certificate file, its days left through the real reader, told once at each of 30, 7 and 1 days
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-e2e205-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tls")).unwrap();
    let mut p = rcgen::CertificateParams::new(vec!["a.example".to_string()]).unwrap();
    p.not_before = rcgen::date_time_ymd(2020, 1, 1);
    p.not_after = rcgen::date_time_ymd(2030, 1, 11);
    let cert = p.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap();
    std::fs::write(dir.join("tls/cert.pem"), cert.pem()).unwrap();
    let end = crate::appevent_feed::first_cert_not_after(&cert.pem()).unwrap();
    for (days_before, expect) in [(100, vec![]), (29, vec!["cert.expiring days=29 what=tls/cert.pem"]), (28, vec![]), (6, vec!["cert.expiring days=6 what=tls/cert.pem"]), (0, vec!["cert.expiring days=0 what=tls/cert.pem"])] {
        facts.lock().unwrap().certs = crate::appevent_feed::cert_days(&dir, end - days_before * 86_400);
        ev.look();
        assert_eq!(heard(&ev), expect, "{days_before} days before the end");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-SDK-206: language, resolution and a due calendar entry reach an app; the first sight of each says nothing.
#[test]
fn language_resolution_and_a_due_calendar_entry_reach_an_app() {
    let (_s, ev, facts, person) = more_engine("e2e206", &["locale.changed", "display.changed", "reminder.due"]);
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-e2e206-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    facts.lock().unwrap().locale = crate::appevent_more::machine_locale(&dir, "", "en_IN.UTF-8");
    facts.lock().unwrap().resolution = "1920x1080".into();
    ev.look();
    assert!(heard(&ev).is_empty());
    // the owner puts a `locale` file in the data folder: it wins over the environment
    std::fs::write(dir.join("locale"), "ta_IN\n").unwrap();
    facts.lock().unwrap().locale = crate::appevent_more::machine_locale(&dir, "", "en_IN.UTF-8");
    facts.lock().unwrap().resolution = "3840x2160".into();
    ev.look();
    assert_eq!(heard(&ev), ["locale.changed locale=ta_IN", "display.changed what=resolution"]);
    // an entry whose reminder time has come is told once, by its title, and not again while it stays due
    let occ = |start: i64| iyakki_shared::calendar::Occurrence { id: "e".into(), title: "Dentist".into(), start, end: start + 3600, all_day: false, repeat: iyakki_shared::calendar::Repeat::None, notes: String::new(), by: "pat".into(), remind: Some(10) };
    for (now, expect) in [(1_000_000 - 700, vec![]), (1_000_000 - 500, vec!["reminder.due title=Dentist"]), (1_000_000, vec![])] {
        person.lock().unwrap().reminders_due = crate::appevent_more::reminders_due_now(&[occ(1_000_000)], now);
        ev.look();
        assert_eq!(heard(&ev), expect, "at {now}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// F-SDK-207: approvals waiting, an update downloaded and then available, the model host, a download that ended, the wake phrase: the real
/// readers (a staged update folder, a model host list, the page's download note) feed the one look and the app hears each once.
#[test]
fn approvals_updates_model_host_downloads_and_the_wake_phrase_reach_an_app() {
    let (s, ev, facts, person) = more_engine("e2e207", &["approvals.pending", "updates.changed", "models.host", "transfer.finished", "voice.wake_heard"]);
    person.lock().unwrap().approvals = 2;
    ev.look();
    assert_eq!(heard(&ev), ["approvals.pending count=2"]);
    // an update waiting unpacked is `downloaded`; an installed item with a newer entry is `available` (the readers take the folder and the ids)
    std::fs::create_dir_all(crate::sysupdate::staged_root(s.dir()).join("v2")).unwrap();
    facts.lock().unwrap().updates = crate::appevent_feed::update_phases(s.dir(), &["skill:notes".to_string()]);
    ev.look();
    assert_eq!(heard(&ev), ["updates.changed component=skill:notes phase=available", "updates.changed component=system phase=downloaded"]);
    ev.look();
    assert!(heard(&ev).is_empty());
    // the model host: ready with its model, then another default model, then none
    let model = |m: Option<&str>| crate::appevent_feed::model_host(&["mac".to_string()], &|_| m.map(str::to_string));
    facts.lock().unwrap().model = model(Some("llama"));
    ev.look();
    assert_eq!(heard(&ev), ["models.host model=llama state=ready"]);
    facts.lock().unwrap().model = model(Some("qwen"));
    ev.look();
    assert_eq!(heard(&ev), ["models.host model=qwen state=changed"]);
    facts.lock().unwrap().model = model(None);
    ev.look();
    assert_eq!(heard(&ev), ["models.host model= state=failed"]);
    // a store install that failed (pushed), then the update centre page's own fetch (a machine-wide note, told once through the pulled source)
    crate::appevent_more::transfer_finished(&*s.app_events, "pat", "install", "Notes", false);
    crate::appevent_more::note_download_made(s.dir(), "system update", true);
    let (n, last) = crate::appevent_more::downloads_made(s.dir());
    {
        let mut p = person.lock().unwrap();
        p.downloads = n;
        p.download_last = last;
    }
    ev.look();
    ev.look();
    assert_eq!(heard(&ev), ["transfer.finished kind=install name=Notes outcome=failed", "transfer.finished kind=download name=system update outcome=finished"]);
    facts.lock().unwrap().wake_heard = 1;
    ev.look();
    assert_eq!(heard(&ev), ["voice.wake_heard"]);
}

/// F-SDK-211: low space (told when it falls, again only after it recovered), disk health, Bluetooth by name, a printer and a hand-made backup.
#[test]
fn low_space_disk_health_bluetooth_printers_and_a_hand_made_backup_reach_an_app() {
    let (s, ev, facts, person) = more_engine("e2e211", &["storage.volume", "backup.finished", "disk.health", "bluetooth.device", "printer.changed"]);
    let set = |f: &dyn Fn(&mut MoreFacts)| f(&mut facts.lock().unwrap());
    set(&|f| {
        f.free_percent.insert("/data".into(), 40);
        f.disk_ok.insert("nvme0".into(), true);
    });
    ev.look();
    assert!(heard(&ev).is_empty());
    for (pct, expect) in [(4, vec!["storage.volume change=low_space volume=/data"]), (3, vec![]), (7, vec![]), (4, vec![]), (12, vec![]), (5, vec!["storage.volume change=low_space volume=/data"])] {
        set(&|f| {
            f.free_percent.insert("/data".into(), pct);
        });
        ev.look();
        assert_eq!(heard(&ev), expect, "at {pct} percent free");
    }
    set(&|f| {
        f.disk_ok.insert("nvme0".into(), false);
        f.bluetooth.insert("Buds".into());
        f.printers.insert(("printer".into(), "lp0".into()));
    });
    ev.look();
    let mut got = heard(&ev);
    got.sort();
    assert_eq!(got, ["bluetooth.device change=connected device=Buds", "disk.health disk=nvme0 state=warning", "printer.changed change=added device=lp0 kind=printer"]);
    set(&|f| {
        f.bluetooth.clear();
        f.printers.clear();
    });
    ev.look();
    let mut got = heard(&ev);
    got.sort();
    assert_eq!(got, ["bluetooth.device change=disconnected device=Buds", "printer.changed change=removed device=lp0 kind=printer"]);
    // a backup made by hand leaves a count and the file's own name; the pulled source tells it once
    crate::appevent_more::note_backup_made(s.dir(), "pat", "pat-2026-11-24.iyb", true);
    let (n, last) = crate::appevent_more::backups_made(s.dir(), "pat");
    {
        let mut p = person.lock().unwrap();
        p.backups = n;
        p.backup_last = last;
    }
    ev.look();
    ev.look();
    assert_eq!(heard(&ev), ["backup.finished name=pat-2026-11-24.iyb outcome=finished"]);
}

/// F-SDK-217: the real inputs read from the files of a data folder (approvals, users, tenant policy, revocation list) feed the one look; the
/// app hears each loss once, with its cause: a company policy, a limited grant that ended, a folder the file rule narrowed, a withdrawal.
#[test]
fn a_policy_a_grant_that_ended_a_narrowed_folder_and_a_withdrawal_reach_an_app_each_with_its_cause() {
    use iyakki_core_api::appcaps::{Approval, Grant};
    let s = svc("e2e217");
    let dir = s.dir().to_path_buf();
    for d in ["store", "cloud/config"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    let users = |overrides: &str| format!(r#"{{"schema":1,"users":[{{"name":"pat","hash":"x","role":"user","overrides":{overrides}}}]}}"#);
    let write_approval = |grants: &[(&str, Option<&str>)], ends: &[(&str, i64)]| {
        let mut extra = serde_json::Map::new();
        if !ends.is_empty() {
            extra.insert("ends".into(), serde_json::Value::Object(ends.iter().map(|(k, v)| (k.to_string(), (*v).into())).collect()));
        }
        let a = Approval { who: "pat".into(), app: "sample".into(), app_version: "1.0".into(), app_sha256: "abc".into(), at: 0, grants: grants.iter().map(|(n, p)| Grant { name: n.to_string(), param: p.map(str::to_string) }).collect(), revoked: vec![], extra };
        std::fs::write(dir.join("store/approvals.json"), serde_json::to_string(&BTreeMap::from([("tile/user/pat/sample".to_string(), a)])).unwrap()).unwrap();
    };
    let tenant = |text: &str| std::fs::write(dir.join("cloud/config/tenant-config.json"), text).unwrap();
    std::fs::write(dir.join("users.json"), users("{}")).unwrap();
    tenant("v1");
    write_approval(&[("events.power", None), ("camera.view", None), ("files.read", Some("/home/pat/Documents")), ("files.read", Some("/home/pat/Pictures"))], &[("camera.view", 5000)]);
    let now = Arc::new(AtomicU64::new(1000));
    let mut inputs = quiet_inputs(&s, Arc::new(Mutex::new("online")), Arc::new(AtomicU64::new(0)));
    let (d2, n2) = (dir.clone(), now.clone());
    inputs.perm = Box::new(move || crate::appevent_permsrc::facts_from_dir(&d2, n2.load(Ordering::SeqCst) as i64));
    let ev = SourcedEvents::new(s.app_events.clone(), inputs);
    ev.start("pat", "sample");
    ev.look(); // the first look only remembers
    assert!(heard(&ev).is_empty());
    ev.look();
    assert!(heard(&ev).is_empty(), "nothing changed, nothing told");
    // 1. the company policy changed and took the Pictures folder away from the file rule
    tenant("v2");
    std::fs::write(dir.join("users.json"), users(r#"{"fs_read":["/home/pat/Documents"]}"#)).unwrap();
    ev.look();
    let got = heard(&ev);
    assert!(got.iter().any(|l| l.starts_with("app.permissions") && l.contains("by=policy") && l.contains("folder:/home/pat/Pictures")), "{got:?}");
    // 2. the limited grant ends at its second
    now.store(5000, Ordering::SeqCst);
    ev.look();
    let got = heard(&ev);
    assert!(got.iter().any(|l| l.contains("by=expiry") && l.contains("camera.view")), "{got:?}");
    // 3. the store's revocation list withdraws the app
    let root = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let roots = iyakki_shared::store_trust::Roots::with("r", &root);
    let mut revs = iyakki_shared::store_revocation::Revocations::default();
    revs.accept(&revs.sign_for_tests(&root, 1, 100, vec![], vec![], vec![("sample".into(), None)]), &roots, 100).unwrap();
    revs.save(&iyakki_shared::store_revocation::Revocations::path(&dir.join("store"))).unwrap();
    write_approval(&[("events.power", None)], &[]);
    ev.look();
    let got = heard(&ev);
    assert!(got.iter().any(|l| l.contains("by=store")), "{got:?}");
    ev.look();
    assert!(heard(&ev).is_empty(), "each loss was told once");
}

#[test]
fn an_app_hears_online_only_when_a_default_route_leaves_by_an_interface_that_is_up_read_from_a_fake_sysfs_and_route_table() {
    // F-LIFE-09 end to end: fake /sys and fake /proc/net/route -> link_state_routed -> the core's tick -> the app's connectivity.changed
    let sys = std::env::temp_dir().join(format!("iyakki-evsrc-routed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sys);
    std::fs::create_dir_all(sys.join("class/net/eth0")).unwrap();
    let state = |s: &str| std::fs::write(sys.join("class/net/eth0/operstate"), s).unwrap();
    state("up\n"); // a cable with no lease yet
    let head = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n";
    let lan = format!("{head}eth0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n");
    let dflt = format!("{lan}eth0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\n");
    let route = Arc::new(Mutex::new(lan.clone()));
    let s = svc("routedlink");
    let steady = Arc::new(AtomicU64::new(0));
    let mut inputs = quiet_inputs(&s, Arc::new(Mutex::new("online")), steady.clone());
    let (sys2, route2) = (sys.clone(), route.clone());
    inputs.link = Box::new(move || link_state_routed(&sys2, &route2.lock().unwrap()));
    let ev = SourcedEvents::new(s.app_events.clone(), inputs);
    ev.register("pat", "sample", "connectivity.changed", "").unwrap();
    ev.start("pat", "sample");
    steady.store(2_000, Ordering::SeqCst);
    ev.tick(); // the baseline: up, but no way out, so offline
    // the lease arrives with a default route: online is announced
    *route.lock().unwrap() = dflt.clone();
    steady.store(4_000, Ordering::SeqCst);
    ev.tick();
    let got = ev.poll("pat", "sample", 5);
    assert_eq!(got.iter().map(show).collect::<Vec<_>>(), ["connectivity.changed state=online"], "{got:?}");
    // the tether loses its way out (route gone, interface still up): offline
    *route.lock().unwrap() = lan;
    steady.store(6_000, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.poll("pat", "sample", 5).iter().map(show).collect::<Vec<_>>(), ["connectivity.changed state=offline"]);
    // the route is back: online again; then the interface goes down while the route stays: a route through a dead interface is offline
    *route.lock().unwrap() = dflt;
    steady.store(8_000, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.poll("pat", "sample", 5).iter().map(show).collect::<Vec<_>>(), ["connectivity.changed state=online"]);
    state("down\n");
    steady.store(10_000, Ordering::SeqCst);
    ev.tick();
    assert_eq!(ev.poll("pat", "sample", 5).iter().map(show).collect::<Vec<_>>(), ["connectivity.changed state=offline"]);
    let _ = std::fs::remove_dir_all(&sys);
}
