use super::*;
use crate::enginelog::Health;

/// A healthy made-up machine; each test breaks one thing.
fn fine() -> Facts {
    Facts {
        now: 1_800_000_000,
        uptime_secs: 3600,
        cluster_installed: true,
        cluster_state: "running".into(),
        cluster_autostart: true,
        node_ip_wanted: true,
        node_ip_present: true,
        sound_cards: 1,
        sound_chosen_present: true,
        disk_total: 100 * GIB,
        disk_free: 60 * GIB,
        route: true,
        clock_synced: true,
        network_time_on: true,
        ..Facts::default()
    }
}

fn ids(f: &Facts) -> Vec<&'static str> {
    diagnose(f).iter().map(|x| x.id).collect()
}

#[test]
fn a_healthy_machine_has_no_findings_and_one_calm_line() {
    assert!(diagnose(&fine()).is_empty());
    assert_eq!(report_lines(&[], true), vec!["Everything checked is fine.".to_string()]);
    // a computer with nothing installed and nothing running is not "wrong" either
    assert!(diagnose(&Facts { cluster_installed: false, sound_cards: 1, sound_chosen_present: true, disk_total: 10 * GIB, disk_free: 9 * GIB, clock_synced: true, ..Facts::default() }).is_empty());
}

#[test]
fn a_stopped_or_failed_cluster_is_reported_with_the_cluster_page() {
    let mut f = fine();
    f.cluster_state = "stopped".into();
    let x = cluster_stopped(&f).expect("autostart is on but it is stopped");
    assert_eq!((x.severity, x.fix.as_ref().unwrap().action.clone()), (Severity::Problem, Action::Settings("tab:devcluster")));
    f.cluster_autostart = false;
    assert!(cluster_stopped(&f).is_none(), "stopped on purpose");
    f.cluster_last_exit_at = f.now - 600;
    assert_eq!(cluster_stopped(&f).unwrap().severity, Severity::Warning, "it ended on its own a moment ago");
    f.cluster_last_exit_at = f.now - 3 * 86400;
    assert!(cluster_stopped(&f).is_none(), "an old exit is history");
    f.cluster_state = "failed".into();
    f.cluster_failed_reason = "exit status 1".into();
    let x = cluster_stopped(&f).unwrap();
    assert!(x.text.contains("error") && x.detail == "exit status 1");
    f.cluster_installed = false;
    assert!(cluster_stopped(&f).is_none());
}

#[test]
fn a_missing_node_address_is_reported_only_when_the_cluster_needs_it() {
    let mut f = fine();
    f.node_ip_present = false;
    let x = node_ip_missing(&f).expect("running without its address");
    assert_eq!(x.id, "node-ip-missing");
    assert!(x.detail.contains(crate::k3sdev::NODE_IP));
    f.node_ip_wanted = false;
    assert!(node_ip_missing(&f).is_none(), "the fixed address is not in use");
    f.node_ip_wanted = true;
    f.cluster_state = "stopped".into();
    f.cluster_autostart = false;
    assert!(node_ip_missing(&f).is_none(), "stopped on purpose: the address is not needed yet");
    f.cluster_autostart = true;
    assert!(node_ip_missing(&f).is_some(), "it is about to start");
    f.node_ip_present = true;
    assert!(node_ip_missing(&f).is_none());
}

#[test]
fn the_firewall_hint_matters_only_while_developer_mode_is_on() {
    let mut f = fine();
    f.dev_firewall_hint = Some("the firewall blocks the developer port 7979 from 192.0.2.7: Allow".into());
    assert!(firewall_blocks_developer_port(&f).is_none(), "developer mode is off");
    f.dev_on = true;
    let x = firewall_blocks_developer_port(&f).unwrap();
    assert_eq!(x.severity, Severity::Problem);
    assert!(!x.text.contains("192.0.2.7"), "the address is for administrators: it is in the detail");
    assert!(x.detail.contains("192.0.2.7"));
    assert_eq!(x.fix.unwrap().action, Action::Settings("tab:firewall"));
    f.dev_firewall_hint = None;
    assert!(firewall_blocks_developer_port(&f).is_none());
}

#[test]
fn developer_mode_about_to_switch_off_offers_four_more_hours() {
    let mut f = fine();
    f.dev_on = true;
    f.dev_until = 0;
    assert!(developer_mode_ending(&f).is_none(), "no switch-off time");
    f.dev_until = f.now + 2 * 3600;
    assert!(developer_mode_ending(&f).is_none(), "plenty of time");
    f.dev_until = f.now + 25 * 60;
    let x = developer_mode_ending(&f).unwrap();
    assert_eq!(x.text, "Developer mode switches off in 25 minutes.");
    assert_eq!(x.fix.unwrap().action, Action::ExtendDeveloperMode);
    f.dev_until = f.now + 90;
    assert!(developer_mode_ending(&f).unwrap().text.contains("2 minutes"));
    f.dev_on = false;
    assert!(developer_mode_ending(&f).is_none());
}

#[test]
fn a_locked_vault_matters_only_when_something_waits_for_it() {
    let mut f = fine();
    f.vault_locked = true;
    assert!(vault_locked_with_waiting(&f).is_none(), "locked and nothing waits");
    f.vault_waiting = vec!["ClickHouse".into()];
    let x = vault_locked_with_waiting(&f).unwrap();
    assert!(x.text.contains("ClickHouse"));
    assert_eq!(x.fix.unwrap().action, Action::Settings("tab:vault"), "the person types the password on that page; DOCTOR never does");
    f.vault_waiting.push("Matrix registration token".into());
    assert!(vault_locked_with_waiting(&f).unwrap().text.contains("2 passwords"));
    f.vault_locked = false;
    assert!(vault_locked_with_waiting(&f).is_none());
}

#[test]
fn engine_trouble_from_the_kept_log_is_reported_by_kind() {
    let mut f = fine();
    assert!(engine_crashed(&f).is_none() && engine_camera(&f).is_none() && engine_microphone(&f).is_none());
    f.engine = Health { crashed: 2, camera_trouble: 1, mic_trouble: 3, last: "alsasrc: could not open".into() };
    assert!(engine_crashed(&f).unwrap().text.contains("2 times"));
    assert!(engine_camera(&f).unwrap().text.contains("camera"));
    let m = engine_microphone(&f).unwrap();
    assert_eq!(m.fix.unwrap().action, Action::Settings("tab:sound"));
    assert_eq!(engine_crashed(&f).unwrap().detail, "alsasrc: could not open");
    assert_eq!(ids(&f).iter().filter(|i| i.starts_with("engine-")).count(), 3);
}

#[test]
fn a_missing_sound_card_or_output_is_reported_but_not_while_the_machine_is_still_starting() {
    let mut f = fine();
    f.sound_cards = 0;
    f.uptime_secs = 20;
    assert!(no_sound(&f).is_none(), "cards show up late");
    f.uptime_secs = 600;
    assert_eq!(no_sound(&f).unwrap().id, "sound-none");
    f.sound_cards = 1;
    assert!(no_sound(&f).is_none());
    f.sound_chosen = "PCH:3".into();
    f.sound_chosen_present = false;
    let x = no_sound(&f).unwrap();
    assert_eq!((x.id, x.detail.as_str()), ("sound-missing", "PCH:3"));
    f.sound_chosen_present = true;
    assert!(no_sound(&f).is_none());
}

#[test]
fn the_disk_warns_at_ten_percent_and_complains_at_five() {
    let mut f = fine();
    f.disk_total = 100 * GIB;
    f.disk_free = 8 * GIB;
    assert_eq!(disk_nearly_full(&f).unwrap().id, "disk-low");
    f.disk_free = 3 * GIB;
    assert_eq!(disk_nearly_full(&f).unwrap().id, "disk-full");
    f.disk_free = 30 * GIB;
    assert!(disk_nearly_full(&f).is_none());
    // a small disk: the absolute floor counts too
    f.disk_total = 8 * GIB;
    f.disk_free = 400 * MIB;
    assert_eq!(disk_nearly_full(&f).unwrap().severity, Severity::Problem);
    f.disk_total = 0;
    assert!(disk_nearly_full(&f).is_none(), "unknown size: say nothing");
}

#[test]
fn the_clock_is_reported_when_it_stays_unset_with_a_network_or_waits_to_be_put_back() {
    let mut f = fine();
    f.clock_synced = false;
    f.uptime_secs = 100;
    assert!(time_not_synchronised(&f).is_none(), "just started");
    f.uptime_secs = 900;
    assert_eq!(time_not_synchronised(&f).unwrap().id, "time-unsynced");
    f.route = false;
    assert!(time_not_synchronised(&f).is_none(), "no network: the network rule's business");
    f.route = true;
    f.network_time_on = false;
    assert_eq!(time_not_synchronised(&f).unwrap().severity, Severity::Note);
    f.clock_synced = true;
    f.clock_ahead_secs = Some(40);
    let x = time_not_synchronised(&f).unwrap();
    assert_eq!((x.id, x.fix.unwrap().action), ("time-ahead", Action::Settings("tab:time")));
}

#[test]
fn an_unexpected_exposure_comes_from_the_audit_line_and_is_not_scanned_here() {
    let mut f = fine();
    assert!(unexpected_exposure(&f).is_none());
    f.exposure_unexpected = Some("NodePort 30300 gitea-http".into());
    let x = unexpected_exposure(&f).unwrap();
    assert_eq!(x.severity, Severity::Problem);
    assert!(x.detail.contains("30300") && !x.text.contains("30300"));
}

#[test]
fn unreachable_model_servers_are_named_in_the_detail() {
    let mut f = fine();
    assert!(model_server_unreachable(&f).is_none());
    f.models_unreachable = vec!["lemonade".into()];
    let x = model_server_unreachable(&f).unwrap();
    assert_eq!((x.text.as_str(), x.detail.as_str()), ("A model server does not answer.", "lemonade"));
    f.models_unreachable.push("ollama".into());
    assert!(model_server_unreachable(&f).unwrap().text.starts_with("2 model servers"));
}

#[test]
fn failed_registry_services_without_their_own_rule_are_listed() {
    let mut f = fine();
    f.failed_services = vec![
        FailedService { id: "shares".into(), name: "Shared folders".into(), reason: "port taken".into(), open: Some("tab:shares") },
        FailedService { id: "k3s".into(), name: "Dev cluster (k3s)".into(), reason: "x".into(), open: Some("tab:devcluster") },
        FailedService { id: "service".into(), name: "Web service".into(), reason: "y".into(), open: None },
    ];
    let v = failed_services(&f);
    assert_eq!(v.len(), 2, "k3s has its own rule");
    assert_eq!(v[0].text, "Shared folders did not start.");
    assert!(v[1].fix.is_none());
}

#[test]
fn findings_are_sorted_problems_first_and_rules_keep_their_order_inside_a_group() {
    let mut f = fine();
    f.cluster_state = "failed".into(); // problem
    f.disk_free = 8 * GIB; // warning
    f.dev_on = true;
    f.dev_until = f.now + 600; // warning
    f.dev_firewall_hint = Some("blocked".into()); // problem
    f.clock_synced = false;
    f.uptime_secs = 900;
    f.network_time_on = false; // note
    let got = ids(&f);
    assert_eq!(got, vec!["cluster-stopped", "firewall-dev-port", "dev-mode-ending", "disk-low", "time-off"]);
    let sev: Vec<Severity> = diagnose(&f).iter().map(|x| x.severity).collect();
    assert!(sev.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn no_fix_can_do_anything_but_open_a_page_or_extend_developer_mode() {
    // a machine with everything broken
    let f = Facts {
        now: 1_800_000_000,
        uptime_secs: 5000,
        cluster_installed: true,
        cluster_state: "failed".into(),
        node_ip_wanted: true,
        dev_on: true,
        dev_until: 1_800_000_100,
        dev_firewall_hint: Some("x".into()),
        vault_locked: true,
        vault_waiting: vec!["A".into()],
        engine: Health { crashed: 1, camera_trouble: 1, mic_trouble: 1, last: "l".into() },
        sound_cards: 0,
        disk_total: GIB,
        disk_free: MIB,
        route: true,
        network_time_on: true,
        exposure_unexpected: Some("e".into()),
        models_unreachable: vec!["m".into()],
        failed_services: vec![FailedService { id: "shares".into(), name: "S".into(), reason: "r".into(), open: Some("tab:shares") }],
        ..Facts::default()
    };
    let all = diagnose(&f);
    assert!(all.len() >= 12, "{:?}", all.iter().map(|x| x.id).collect::<Vec<_>>());
    for x in &all {
        match &x.fix {
            Some(Fix { action: Action::Settings(i), .. }) => assert!(i.starts_with("tab:"), "{}: {i}", x.id),
            Some(Fix { action: Action::ExtendDeveloperMode, .. }) => assert_eq!(x.id, "dev-mode-ending"),
            None => {}
        }
        assert!(!x.text.is_empty() && x.text.ends_with('.'), "{}: a plain sentence", x.id);
    }
    // every Settings item a fix names is one the Settings page understands (the list in apps/settings.rs `open_item`)
    let known = ["tab:devcluster", "tab:firewall", "tab:vault", "tab:browser", "tab:sound", "tab:storage", "tab:time", "tab:models", "tab:shares"];
    for x in &all {
        if let Some(Fix { action: Action::Settings(i), .. }) = &x.fix {
            assert!(known.contains(i), "{i}");
        }
    }
}

#[test]
fn the_report_lines_show_details_only_when_asked() {
    let mut f = fine();
    f.dev_on = true;
    f.dev_firewall_hint = Some("blocks 192.0.2.7".into());
    let found = diagnose(&f);
    let plain = report_lines(&found, false).join("\n");
    let admin = report_lines(&found, true).join("\n");
    assert!(!plain.contains("192.0.2.7") && admin.contains("192.0.2.7"));
    assert!(plain.contains("[Problem]") && plain.contains("fix: Open Firewall"));
}

#[test]
fn span_words_round_up_to_whole_minutes() {
    assert_eq!(span_words(0), "a minute");
    assert_eq!(span_words(61), "2 minutes");
    assert_eq!(span_words(3600), "1 hour");
    assert_eq!(span_words(3660), "1 hour 1 minute");
    assert_eq!(span_words(7500), "2 hours 5 minutes");
}

#[test]
fn the_newest_exposure_audit_line_decides() {
    let l = |s: &str| s.to_string();
    assert_eq!(exposure_from_audit(&[]), None);
    let unexpected = vec![l("2026-10-27 18:00:00 system   devcluster.exposure.unexpected NodePort 30300 gitea-http"), l("2026-10-27 17:00:00 root     app.open doctor")];
    assert_eq!(exposure_from_audit(&unexpected).as_deref(), Some("NodePort 30300 gitea-http"));
    // a later clean line clears it
    let cleared = vec![l("2026-10-27 19:00:00 system   devcluster.exposure.ok nothing unexpected"), unexpected[0].clone()];
    assert_eq!(exposure_from_audit(&cleared), None);
    // unrelated lines are skipped
    let other = vec![l("2026-10-27 19:00:00 root     settings.sound.output automatic"), unexpected[0].clone()];
    assert!(exposure_from_audit(&other).is_some());
}

#[test]
fn only_local_model_servers_are_probed() {
    assert_eq!(local_endpoint("http://localhost:11434/v1").as_deref(), Some("localhost:11434"));
    assert_eq!(local_endpoint("http://127.0.0.1:8000").as_deref(), Some("127.0.0.1:8000"));
    assert_eq!(local_endpoint("http://192.168.1.20:13305/api").as_deref(), Some("192.168.1.20:13305"));
    assert_eq!(local_endpoint("https://api.example.com/v1"), None, "a cloud service is never touched");
    assert_eq!(local_endpoint("http://user:pw@10.0.0.5/x").as_deref(), Some("10.0.0.5:80"), "the password never leaves the address");
    assert_eq!(local_endpoint("http://8.8.8.8:80"), None);
}

#[test]
fn a_listening_port_answers_and_a_closed_one_does_not() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    assert!(answers(&addr, std::time::Duration::from_millis(500)));
    drop(l);
    assert!(!answers(&addr, std::time::Duration::from_millis(500)));
    assert!(!answers("not an address", std::time::Duration::from_millis(100)));
}

#[test]
fn disk_space_reads_a_real_folder() {
    let (total, free) = disk_space(&std::env::temp_dir()).expect("statvfs");
    assert!(total > 0 && free <= total);
    assert!(disk_space(std::path::Path::new("/definitely/not/here")).is_none());
}

// ------------------------------------------------------------------------------------------------ connection, memory, camera, firewall...

#[test]
fn the_connection_test_has_three_outcomes() {
    let mut f = fine();
    assert!(connection(&f).is_none(), "not run yet: nothing is claimed");
    f.net = Some(NetProbe { route: true, dns: true });
    assert!(connection(&f).is_none());
    f.net = Some(NetProbe { route: true, dns: false });
    let x = connection(&f).unwrap();
    assert_eq!((x.id, x.severity), ("net-dns", Severity::Warning));
    f.net = Some(NetProbe { route: false, dns: false });
    assert_eq!(connection(&f).unwrap().id, "net-none");
}

#[test]
fn a_silent_developer_port_is_a_problem_only_when_developer_mode_is_on() {
    let mut f = fine();
    f.dev_port = 7022;
    f.dev_port_answers = Some(false);
    assert!(developer_port_silent(&f).is_none(), "developer mode is off");
    f.dev_on = true;
    let x = developer_port_silent(&f).unwrap();
    assert_eq!((x.severity, x.detail.as_str(), x.fix.unwrap().action), (Severity::Problem, "port 7022", Action::Settings("tab:developer")));
    f.dev_port_answers = Some(true);
    assert!(developer_port_silent(&f).is_none());
    f.dev_port_answers = None;
    assert!(developer_port_silent(&f).is_none(), "not asked yet");
}

#[test]
fn memory_and_swap_levels() {
    let mut f = fine();
    assert!(memory_low(&f).is_none(), "unknown");
    f.mem_total = 8 * GIB;
    f.mem_available = 4 * GIB;
    assert!(memory_low(&f).is_none());
    f.mem_available = 600 * MIB;
    assert_eq!(memory_low(&f).unwrap().id, "memory-tight");
    f.mem_available = 200 * MIB;
    assert_eq!(memory_low(&f).unwrap().severity, Severity::Problem);
    f.mem_available = 4 * GIB;
    f.swap_total = 4 * GIB;
    f.swap_free = 100 * MIB;
    assert_eq!(memory_low(&f).unwrap().id, "swap-full");
    f.swap_free = 3 * GIB;
    assert!(memory_low(&f).is_none());
}

#[test]
fn camera_firewall_vault_update_crash_and_vpn() {
    let mut f = fine();
    assert!(no_camera(&f).is_none() && firewall_off(&f).is_none() && vault_locked_note(&f).is_none());
    assert!(update_pending(&f).is_none() && crashed_last_time(&f).is_none() && vpn_not_connected(&f).is_none());
    f.cameras = Some(0);
    assert_eq!(no_camera(&f).unwrap().severity, Severity::Note);
    f.cameras = Some(1);
    assert!(no_camera(&f).is_none());
    f.firewall_on = Some(false);
    assert_eq!(firewall_off(&f).unwrap().fix.unwrap().action, Action::Settings("tab:firewall"));
    f.vault_locked = true;
    assert_eq!(vault_locked_note(&f).unwrap().id, "vault-locked-note");
    f.vault_waiting = vec!["ClickHouse".into()];
    assert!(vault_locked_note(&f).is_none() && vault_locked_with_waiting(&f).is_some(), "the louder rule speaks alone");
    f.update_note = Some("it stopped half way".into());
    assert_eq!(update_pending(&f).unwrap().fix.unwrap().action, Action::Settings("tab:updates"));
    f.crash_note = Some("2026-10-27".into());
    assert_eq!(crashed_last_time(&f).unwrap().fix.unwrap().action, Action::Settings("tab:sessionlog"));
    use crate::vpn_mgr::Light;
    for (l, want) in [(Light::Secure, None), (Light::Working, None), (Light::Open, Some(Severity::Warning)), (Light::Blocked, Some(Severity::Problem))] {
        f.vpn = Some(l);
        assert_eq!(vpn_not_connected(&f).map(|x| x.severity), want);
    }
}

#[test]
fn new_findings_never_carry_a_secret_or_a_path() {
    let f = Facts { net: Some(NetProbe::default()), vpn: Some(crate::vpn_mgr::Light::Open), firewall_on: Some(false), cameras: Some(0), crash_note: Some("2026-10-27".into()), ..fine() };
    for x in diagnose(&f) {
        let all = format!("{} {}", x.text, x.detail);
        assert!(!all.contains("/home") && !all.contains("token"), "{all}");
    }
}

#[test]
fn meminfo_text_is_read_in_bytes() {
    let t = "MemTotal:        8000000 kB\nMemFree: 1 kB\nMemAvailable:    2000000 kB\nSwapTotal:       1000 kB\nSwapFree:        500 kB\n";
    assert_eq!(parse_meminfo(t), (8_000_000 * 1024, 2_000_000 * 1024, 1000 * 1024, 500 * 1024));
    assert_eq!(parse_meminfo(""), (0, 0, 0, 0));
}

#[test]
fn a_name_that_cannot_resolve_returns_within_the_wait() {
    let t = std::time::Instant::now();
    let _ = resolves("name.invalid", std::time::Duration::from_millis(1500));
    assert!(t.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn a_software_fall_back_of_the_browser_graphics_is_a_note_with_the_browser_settings() {
    let mut f = fine();
    f.graphics_line = "Automatic: hardware".into();
    assert!(engine_graphics_fallback(&f).is_none() && diagnose(&f).is_empty(), "hardware is not a finding");
    f.graphics_fallback = true;
    f.graphics_line = "Automatic: software because the last session stopped right after the browser started on this graphics card".into();
    let x = engine_graphics_fallback(&f).expect("a note");
    assert_eq!((x.severity, x.fix.as_ref().unwrap().action.clone()), (Severity::Note, Action::Settings("tab:browser")));
    assert!(x.text.contains("software") && x.detail.starts_with("Browser graphics: Automatic: software"));
    assert_eq!(ids(&f), vec!["engine-graphics-fallback"]);
}
