//! The startup registry: transitions, dependency text, the strip and the lines for each role, the collapse, failures, and the
//! rules of the poll with fake facts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::startup::*;
use crate::startup_probe::{apply, Facts, K3sFact};

fn reg() -> (Registry, Arc<AtomicU64>) {
    let t = Arc::new(AtomicU64::new(100));
    let t2 = t.clone();
    (Registry::new(move || t2.load(Ordering::SeqCst)), t)
}

fn row(r: &Registry, id: &str) -> Row {
    r.snapshot().into_iter().find(|x| x.id == id).unwrap()
}

#[test]
fn the_catalogue_is_consistent() {
    let ids: Vec<&str> = CATALOGUE.iter().map(|d| d.id).collect();
    for (i, d) in CATALOGUE.iter().enumerate() {
        assert!(!ids[..i].contains(&d.id), "{} twice", d.id);
        for a in d.after {
            assert!(ids[..i].contains(a), "{} waits for {a}, which must be listed before it", d.id);
        }
        assert!(!d.name.contains('|') && !d.short.is_empty());
    }
}

#[test]
fn a_service_goes_pending_starting_ready_and_keeps_its_since_while_the_step_changes() {
    let (r, t) = reg();
    assert_eq!(row(&r, "web").state, StartupState::Pending);
    assert_eq!(row(&r, "models").state, StartupState::Disabled, "services that only push begin disabled");
    t.store(110, Ordering::SeqCst);
    assert!(r.starting("web", "starting the web engine"));
    assert!(!r.starting("web", "starting the web engine"), "the same state is no change");
    t.store(125, Ordering::SeqCst);
    assert!(r.starting("web", "loading the engine's files"));
    assert_eq!(row(&r, "web").age, 15, "a new step text of a service that keeps starting keeps its since");
    t.store(130, Ordering::SeqCst);
    assert!(r.ready("web"));
    assert_eq!(row(&r, "web").age, 0, "ready is a new state");
    assert_eq!(row(&r, "web").step, "");
    assert!(!r.ready("nonsense"), "unknown ids are ignored");
}

#[test]
fn a_service_waits_for_what_it_needs_and_says_so() {
    let (r, _) = reg();
    assert_eq!(row(&r, "k3s").step, "waiting for the network and the clock", "{:?}", row(&r, "k3s"));
    r.starting("clock", "waiting for a time server");
    r.ready("network");
    assert_eq!(row(&r, "k3s").step, "waiting for the clock");
    assert_eq!(row(&r, "k3s").waits_for, vec!["the clock"]);
    // its own step text wins over the dependency text
    r.starting("k3s", "starting k3s (about 20 s)");
    assert_eq!(row(&r, "k3s").step, "starting k3s (about 20 s)");
    r.ready("clock");
    r.ready("k3s");
    assert!(row(&r, "k3s").waits_for.is_empty());
    // a failed dependency is waited for too
    let (r, _) = reg();
    r.ready("clock");
    r.failed("network", "no cable");
    assert_eq!(row(&r, "k3s").step, "waiting for the network");
    assert_eq!(row(&r, "shares").step, "waiting for the network");
    // nothing to wait for: plain
    assert_eq!(row(&r, "web").step, "waiting to start");
}

#[test]
fn the_summary_lists_what_is_not_ready_failures_first_then_the_longest_starting() {
    let (r, t) = reg();
    for id in ["clock", "network", "firewall", "sound", "touch", "web", "shares", "agent"] {
        r.ready(id);
    }
    t.store(110, Ordering::SeqCst);
    r.starting("k3s", "starting k3s (about 20 s)");
    t.store(120, Ordering::SeqCst);
    r.starting("models", "loading the model qwen");
    r.failed("flows", "could not read the flows");
    t.store(125, Ordering::SeqCst);
    let s = r.summary();
    let ids: Vec<&str> = s.rows.iter().map(|x| x.id).collect();
    assert_eq!(ids, vec!["flows", "k3s", "models"], "failed, then the one that began first");
    assert_eq!(s.ready, 8);
    assert!(!s.all_ready() && s.busy() && s.failed() == 1);
    let l = s.lines(true);
    assert_eq!(l[0].text, "Flows scheduler: could not read the flows");
    assert_eq!(l[1].text, "Dev cluster (k3s): starting k3s (about 20 s)");
    assert!(l[1].starting && !l[1].failed);
    assert_eq!(s.strip(true), "Flows scheduler: could not read the flows  +2 more");
    // all ready: nothing to show
    r.ready("k3s");
    r.ready("models");
    r.ready("flows");
    let s = r.summary();
    assert!(s.all_ready() && s.strip(true).is_empty() && s.strip(false).is_empty() && s.lines(true).is_empty());
}

#[test]
fn a_person_who_is_not_an_administrator_sees_names_and_steps_but_no_reasons_and_no_buttons() {
    let (r, _) = reg();
    for id in ["clock", "network", "firewall", "sound", "touch", "web", "shares", "agent"] {
        r.ready(id);
    }
    r.starting("k3s", "starting k3s (about 20 s)");
    let s = r.summary();
    assert_eq!(s.strip(false), "Starting services, 1 left");
    assert_eq!(s.lines(false)[0].text, "Dev cluster (k3s): starting k3s (about 20 s)");
    r.failed("k3s", "k3s: /var/lib/iyakki/k3s-dev: permission denied for 10.0.0.5");
    let s = r.summary();
    let admin = s.lines(true);
    assert!(admin[0].text.contains("permission denied") && admin[0].actions.retry && admin[0].actions.open == Goes::Cluster, "{admin:?}");
    let other = s.lines(false);
    assert_eq!(other[0].text, "Dev cluster (k3s): did not start");
    assert!(!other[0].actions.retry && other[0].actions.open == Goes::Nothing);
    assert_eq!(s.strip(false), "A service did not start. Ask an administrator.");
    assert!(!s.strip(false).contains("10.0.0.5") && !other[0].text.contains("permission"));
}

#[test]
fn only_the_system_wide_services_show_before_sign_in() {
    let (r, _) = reg();
    let sys = r.summary().system_only();
    assert!(sys.rows.iter().all(|x| x.scope == Scope::System));
    let names: Vec<&str> = sys.rows.iter().map(|x| x.id).collect();
    assert_eq!(names, vec!["network", "clock", "firewall", "sound", "touch", "k3s"]);
    assert!(!names.contains(&"web") && !names.contains(&"agent"));
}

#[test]
fn a_failure_is_queued_once_and_stays_until_the_service_recovers() {
    let (r, _) = reg();
    assert!(r.failed("k3s", "k3s exited with 1"));
    assert!(!r.failed("k3s", "k3s exited with 1"), "the same failure again is no transition");
    let f = r.take_failures();
    assert_eq!(f, vec![Failure { id: "k3s", name: "Dev cluster (k3s)", reason: "k3s exited with 1".into() }]);
    assert!(r.take_failures().is_empty());
    assert!(r.summary().rows.iter().any(|x| x.id == "k3s" && x.state.failed()), "it stays in the list");
    // a different reason is a new line in the log; starting again is not a failure
    r.failed("k3s", "k3s exited with 2");
    assert_eq!(r.take_failures().len(), 1);
    r.starting("k3s", "starting k3s (about 20 s)");
    assert!(r.take_failures().is_empty() && !r.summary().rows.iter().any(|x| x.state.failed()));
    r.failed("k3s", "k3s exited with 2");
    assert_eq!(r.take_failures().len(), 1, "failing again after a start is a new transition");
    r.stopped("k3s");
    assert!(r.summary().rows.iter().all(|x| x.id != "k3s"), "stopped by choice is quiet");
}

#[test]
fn the_ready_note_holds_then_fades_then_goes_and_comes_back_when_something_starts() {
    let mut c = Calm::default();
    assert_eq!(c.update(false, 0), Phase::Gone, "nothing ever started: nothing to say");
    assert_eq!(c.update(true, 1000), Phase::Busy);
    assert_eq!(c.update(false, 5000), Phase::Done(255));
    assert_eq!(c.update(false, 5000 + Calm::HOLD_MS - 1), Phase::Done(255));
    match c.update(false, 5000 + Calm::HOLD_MS + Calm::FADE_MS / 2) {
        Phase::Done(a) => assert!((100..160).contains(&a), "half way: {a}"),
        p => panic!("{p:?}"),
    }
    assert_eq!(c.update(false, 5000 + Calm::HOLD_MS + Calm::FADE_MS), Phase::Gone);
    assert_eq!(c.update(false, 20_000), Phase::Gone, "stays gone");
    assert_eq!(c.update(true, 30_000), Phase::Busy, "a service starting later shows again");
    // a failure that comes while the note holds takes it back
    assert_eq!(c.update(false, 31_000), Phase::Done(255));
    assert_eq!(c.update(true, 31_500), Phase::Busy);
    assert_eq!(c.update(false, 40_000), Phase::Done(255), "the hold starts over");
}

#[test]
fn a_machine_with_no_network_device_does_not_wait_for_the_network_or_a_time_server() {
    let (r, _t) = reg();
    // drivers still loading: it waits as before
    apply(&r, &Facts { route: false, no_nic: true, clock_synced: false, ..up(10) });
    assert!(matches!(r.state("network"), Some(StartupState::Starting { .. })));
    // after the grace time with no device at all: nothing is coming, nothing waits
    apply(&r, &Facts { route: false, no_nic: true, clock_synced: false, ..up(25) });
    assert_eq!(r.state("network"), Some(StartupState::Disabled));
    assert_eq!(r.state("clock"), Some(StartupState::Ready));
    // a device is there but nothing is connected: the old wait
    apply(&r, &Facts { route: false, no_nic: false, clock_synced: false, ..up(25) });
    assert!(matches!(r.state("network"), Some(StartupState::Starting { .. })));
}

fn up(secs: u64) -> Facts {
    Facts { uptime_secs: secs, route: true, clock_synced: true, sound_cards: 1, web_engine: true, ..Facts::default() }
}

fn k(state: &str) -> K3sFact {
    K3sFact { installed: true, autostart: true, state: state.into(), ..K3sFact::default() }
}

#[test]
fn the_clock_waits_for_the_network_first_and_the_list_starts_with_the_network() {
    let (r, _) = reg();
    // no route yet: a time server cannot answer, so the clock says it waits for the network, not for a time server
    apply(&r, &Facts { route: false, clock_synced: false, ..up(30) });
    assert_eq!(r.state("clock"), Some(StartupState::Starting { step: "waiting for the network".into() }));
    let rows = r.snapshot();
    let ids: Vec<&str> = rows.iter().map(|x| x.id).collect();
    assert_eq!(&ids[..3], ["network", "devname", "clock"], "network, then the device name that rides on it, then the clock");
    // once there is a route, the time server is what is awaited
    apply(&r, &Facts { route: true, clock_synced: false, ..up(40) });
    assert_eq!(r.state("clock"), Some(StartupState::Starting { step: "waiting for a time server".into() }));
}

#[test]
fn the_poll_reports_the_clock_the_way_the_cluster_waits_for_it() {
    let (r, _) = reg();
    apply(&r, &Facts { clock_floor_pending: true, ..up(5) });
    assert_eq!(r.state("clock"), Some(StartupState::Starting { step: "setting the clock".into() }));
    apply(&r, &Facts { clock_synced: false, ..up(30) });
    assert_eq!(r.state("clock"), Some(StartupState::Starting { step: "waiting for a time server".into() }));
    apply(&r, &Facts { clock_synced: false, ..up(130) });
    assert_eq!(r.state("clock"), Some(StartupState::Ready), "offline: the saved time is good enough");
    apply(&r, &up(10));
    assert_eq!(r.state("clock"), Some(StartupState::Ready));
}

#[test]
fn the_poll_shows_k3s_waiting_for_the_clock_with_the_clusters_own_words() {
    let (r, _) = reg();
    let f = Facts { clock_floor_pending: true, k3s: K3sFact { note: crate::k3sdev::WAITING_CLOCK.into(), ..k("starting") }, ..up(8) };
    apply(&r, &f);
    let row = row(&r, "k3s");
    assert_eq!(row.step, "waiting for the clock to be set", "{row:?}");
    assert!(row.state.busy());
    // the clock gets ready and k3s moves on
    apply(&r, &Facts { k3s: k("starting"), ..up(40) });
    assert_eq!(self::row(&r, "k3s").step, "starting k3s (about 20 s)");
    apply(&r, &Facts { k3s: K3sFact { node_ready: Some(false), ..k("running") }, ..up(50) });
    assert_eq!(self::row(&r, "k3s").step, "waiting for the node to be ready");
    apply(&r, &Facts { k3s: K3sFact { node_ready: Some(true), ..k("running") }, ..up(60) });
    assert_eq!(r.state("k3s"), Some(StartupState::Ready));
}

#[test]
fn the_poll_k3s_cases_failed_stopped_not_installed_and_the_wait_for_autostart() {
    let (r, _) = reg();
    apply(&r, &Facts { k3s: K3sFact { installed: false, state: "not installed".into(), ..K3sFact::default() }, ..up(10) });
    assert_eq!(r.state("k3s"), Some(StartupState::Disabled));
    // installed, autostart on, not started yet: still pending in the first seconds
    apply(&r, &Facts { k3s: k("stopped"), ..up(10) });
    assert_eq!(r.state("k3s"), Some(StartupState::Pending), "installed now, and autostart will fire");
    let (r, _) = reg();
    apply(&r, &Facts { k3s: k("stopped"), ..up(10) });
    assert_eq!(r.state("k3s"), Some(StartupState::Pending));
    apply(&r, &Facts { k3s: k("stopped"), ..up(100) });
    assert_eq!(r.state("k3s"), Some(StartupState::Stopped), "later it was stopped on purpose");
    apply(&r, &Facts { k3s: K3sFact { failed_reason: "k3s stopped: port 6443 is busy".into(), ..k("failed") }, ..up(110) });
    assert_eq!(r.state("k3s"), Some(StartupState::Failed("k3s stopped: port 6443 is busy".into())));
    assert_eq!(r.take_failures().len(), 1);
    apply(&r, &Facts { k3s: K3sFact { failed_reason: "k3s stopped: port 6443 is busy".into(), ..k("failed") }, ..up(115) });
    assert!(r.take_failures().is_empty(), "polling a failure again queues nothing");
}

#[test]
fn the_poll_a_slow_k3s_start_says_so() {
    let (r, t) = reg();
    apply(&r, &Facts { k3s: k("starting"), ..up(30) });
    t.store(100 + 200, Ordering::SeqCst);
    apply(&r, &Facts { k3s: k("starting"), ..up(230) });
    assert_eq!(row(&r, "k3s").step, "starting k3s (taking longer than usual)");
}

#[test]
fn the_poll_network_sound_firewall_web_and_shares() {
    let (r, _) = reg();
    apply(&r, &Facts { route: false, sound_cards: 0, web_engine: false, firewall_alert: true, ..up(10) });
    assert!(matches!(r.state("network"), Some(StartupState::Starting { .. })));
    assert!(matches!(r.state("sound"), Some(StartupState::Starting { .. })));
    assert_eq!(r.state("web"), Some(StartupState::Disabled));
    assert!(r.state("firewall").unwrap().failed());
    assert_eq!(r.state("shares"), Some(StartupState::Disabled));
    // later: no cable and a cluster that wants one is a failure; without a cluster it is quiet
    apply(&r, &Facts { route: false, sound_cards: 0, k3s: k("starting"), ..up(100) });
    assert!(r.state("network").unwrap().failed());
    assert_eq!(r.state("sound"), Some(StartupState::Disabled), "no card is not a failure");
    assert_eq!(r.state("firewall"), Some(StartupState::Ready));
    apply(&r, &Facts { route: false, ..up(200) });
    assert_eq!(r.state("network"), Some(StartupState::Stopped));
    apply(&r, &Facts { shares_running: 2, ..up(210) });
    assert_eq!(r.state("shares"), Some(StartupState::Ready));
    assert_eq!(r.state("network"), Some(StartupState::Ready));
}

#[test]
fn a_whole_boot_ends_with_nothing_to_show_and_the_lines_for_dev_status_read_well() {
    let (r, _) = reg();
    r.ready("agent");
    apply(&r, &Facts { clock_floor_pending: true, route: false, k3s: k("stopped"), ..up(3) });
    let s = r.summary();
    assert!(s.rows.iter().any(|x| x.id == "k3s" && x.step == "waiting for the network and the clock"));
    let lines = r.report_lines();
    assert_eq!(lines.len(), CATALOGUE.len());
    assert!(lines.iter().any(|l| l.starts_with("Dev cluster (k3s)") && l.contains("pending") && l.contains("waiting for the network and the clock")), "{lines:?}");
    apply(&r, &Facts { k3s: K3sFact { node_ready: Some(true), ..k("running") }, ..up(60) });
    assert!(r.summary().all_ready(), "{:?}", r.summary().rows);
}

#[test]
fn the_developer_rows_carry_states_and_filter_credentials() {
    let g = global();
    g.failed("flows", "could not start: token=ghp_abcdefghijklmnopqrstuvwxyz0123456789abcd");
    let rows = crate::devmode::debug::startup_rows();
    let flows = rows.iter().find(|r| r["id"] == "flows").unwrap();
    assert_eq!(flows["state"], "failed");
    assert!(!flows["detail"].as_str().unwrap().contains("ghp_abcdefghijklmnop"), "{flows}");
    assert!(rows.iter().any(|r| r["id"] == "k3s"));
    g.disabled("flows");
    g.take_failures();
}

#[test]
fn wifi_is_a_background_row_only_beside_a_working_cable_and_never_counts_or_fails() {
    use crate::startup::{login_view, WIFI_ID};
    use crate::startup_probe::{wifi_state, WifiRow};
    let mac = Facts { route: true, wired_route: true, wifi_hw: true, ..up(12) };
    assert_eq!(wifi_state(&mac), WifiRow::Background);
    assert_eq!(wifi_state(&Facts { wifi_online: true, ..mac.clone() }), WifiRow::Ready);
    assert_eq!(wifi_state(&Facts { route: false, wired_route: false, ..mac.clone() }), WifiRow::Hidden, "no link at all: the network row tells the story as before");
    assert_eq!(wifi_state(&Facts { wifi_hw: false, ..mac.clone() }), WifiRow::Hidden, "no adapter, no row");
    assert_eq!(wifi_state(&Facts { uptime_secs: 600, ..mac.clone() }), WifiRow::Hidden, "a chip that never answers is left alone after a while");
    // on the sign-in screen: Wi-Fi is not in the "x of y", not pending, and not red
    let (r, _) = reg();
    apply(&r, &Facts { sound_cards: 1, ..mac.clone() });
    assert!(matches!(r.state(WIFI_ID), Some(StartupState::Starting { .. })));
    let v = login_view(r.snapshot());
    assert!(v.wifi_background);
    assert!(v.pending.iter().all(|x| x.id != WIFI_ID && !x.state.failed()));
    let without = login_view(r.snapshot().into_iter().filter(|x| x.id != WIFI_ID).collect());
    assert_eq!((v.ready, v.total), (without.ready, without.total), "the row is not counted");
    assert!(!without.wifi_background);
    // the strips and the calm check ignore it too
    assert!(r.summary().rows.iter().all(|x| x.id != WIFI_ID));
    // no cable and no route: no Wi-Fi row at all, the line is as it always was
    let (r2, _) = reg();
    apply(&r2, &Facts { route: false, wired_route: false, wifi_hw: true, ..up(12) });
    assert_eq!(r2.state(WIFI_ID), Some(StartupState::Disabled));
    assert!(!login_view(r2.snapshot()).wifi_background);
}

#[test]
fn a_wired_default_route_is_told_from_a_wireless_one() {
    use crate::startup_probe::wired_default_route;
    let head = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n";
    let eth = format!("{head}eth0\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n");
    let wl = format!("{head}wlan0\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n");
    let is_wl = |n: &str| n.starts_with("wlan");
    assert!(wired_default_route(&eth, &is_wl));
    assert!(!wired_default_route(&wl, &is_wl));
    assert!(wired_default_route(&format!("{wl}{}", eth.lines().nth(1).unwrap()), &is_wl), "a cable beside Wi-Fi counts");
    assert!(!wired_default_route(head, &is_wl));
}

#[test]
fn the_first_usable_link_makes_the_network_ready_whichever_it_is_and_the_other_one_never_holds_anything() {
    use crate::startup::{login_view, WIFI_ID};
    // cable first: network and clock go on, Wi-Fi is a side note
    let (r, _) = reg();
    apply(&r, &Facts { route: true, wired_route: true, wifi_hw: true, sound_cards: 1, clock_synced: false, ..up(12) });
    assert_eq!(r.state("network"), Some(StartupState::Ready));
    assert_eq!(row(&r, "clock").step, "waiting for a time server");
    assert!(login_view(r.snapshot()).wifi_background);
    // Wi-Fi first: the same, the cable that is still coming up is not a row at all
    let (r2, _) = reg();
    apply(&r2, &Facts { route: true, wired_route: false, wifi_hw: true, wifi_online: true, sound_cards: 1, ..up(12) });
    assert_eq!(r2.state("network"), Some(StartupState::Ready));
    assert_eq!(r2.state(WIFI_ID), Some(StartupState::Ready));
    assert!(!login_view(r2.snapshot()).wifi_background);
    // a Wi-Fi chip that never answers is quiet once the cable works, and never red
    let (r3, _) = reg();
    apply(&r3, &Facts { route: true, wired_route: true, wifi_hw: true, ..up(100) });
    apply(&r3, &Facts { route: true, wired_route: true, wifi_hw: true, ..up(400) });
    assert!(!r3.state(WIFI_ID).unwrap().failed());
    assert!(login_view(r3.snapshot()).pending.iter().all(|x| !x.state.failed()));
}

#[test]
fn the_flows_scheduler_and_the_vpn_are_in_the_catalogue_and_report_through_their_own_calls() {
    let (r, _t) = reg();
    assert_eq!(row(&r, "flows").state, StartupState::Disabled);
    crate::flows::report_scheduler(&r, true);
    assert_eq!(row(&r, "flows").state, StartupState::Ready);
    crate::flows::report_scheduler(&r, false);
    assert_eq!(row(&r, "flows").state, StartupState::Disabled);
    let vpn = CATALOGUE.iter().find(|d| d.id == "vpn").expect("the VPN has a row");
    assert!(vpn.after.contains(&"network"));
    assert_eq!(row(&r, "vpn").state, StartupState::Disabled, "nothing enabled, nothing to wait for");
}
