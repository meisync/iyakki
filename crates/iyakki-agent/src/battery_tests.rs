use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

/// A made-up /sys, removed when dropped.
struct Sys {
    root: PathBuf,
}

impl Sys {
    fn new() -> Sys {
        let root = std::env::temp_dir().join(format!("iyakki-batt-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Sys { root }
    }
    fn put(&self, rel: &str, v: &str) {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }
    fn supply(&self, name: &str, files: &[(&str, &str)]) {
        for (k, v) in files {
            self.put(&format!("class/power_supply/{name}/{k}"), v);
        }
    }
    fn env(&self) -> BatteryEnv {
        BatteryEnv::at(&self.root)
    }
    fn ac(&self, online: bool) {
        self.supply("ADP1", &[("type", "Mains"), ("online", if online { "1" } else { "0" })]);
    }
    fn read(&self) -> Reading {
        read(&self.env())
    }
}

impl Drop for Sys {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn laptop(s: &Sys, status: &str, now: u64, full: u64, power: i64) {
    s.supply(
        "BAT0",
        &[
            ("type", "Battery"),
            ("status", status),
            ("present", "1"),
            ("energy_now", &now.to_string()),
            ("energy_full", &full.to_string()),
            ("energy_full_design", "60000000"),
            ("power_now", &power.to_string()),
            ("cycle_count", "212"),
            ("technology", "Li-ion"),
            ("manufacturer", "SMP"),
            ("model_name", "bq40z651"),
        ],
    );
}

#[test]
fn no_power_supply_folder_shows_nothing() {
    let s = Sys::new();
    let r = s.read();
    assert!(!r.shown());
    assert_eq!(r.presence, Presence::None);
    assert_eq!(r.ac_online, None);
}

#[test]
fn a_desktop_with_only_mains_shows_nothing() {
    let s = Sys::new();
    s.ac(true);
    s.put("class/dmi/id/chassis_type", "3");
    let r = s.read();
    assert!(!r.shown());
    assert_eq!(ac_online(&s.env()), Some(true));
}

#[test]
fn a_laptop_chassis_alone_without_any_battery_device_shows_nothing() {
    let s = Sys::new();
    s.ac(false);
    s.put("class/dmi/id/chassis_type", "10");
    assert!(!s.read().shown());
}

#[test]
fn a_discharging_laptop_with_energy_files() {
    let s = Sys::new();
    s.ac(false);
    laptop(&s, "Discharging", 30_000_000, 50_000_000, 10_000_000);
    s.supply("BAT0", &[("capacity", "60")]);
    let mut r = s.read();
    assert_eq!(r.presence, Presence::Present);
    assert_eq!(r.percent_whole(), Some(60));
    assert_eq!(r.status, Status::Discharging);
    assert_eq!(r.ac_online, Some(false));
    assert_eq!(r.icon_state(), IconState::Discharging);
    assert_eq!(r.status_words(), "Discharging");
    Tracker::new().apply(&mut r);
    // 30 Wh at 10 W: 3 hours
    assert_eq!(r.time_secs, Some((TimeKind::UntilEmpty, 3 * 3600)));
    assert_eq!(r.time_words().as_deref(), Some("About 3 h until empty"));
    // 50 of 60 Wh
    assert_eq!(r.health_pct.map(|h| h.round() as u32), Some(83));
    assert_eq!(r.cycles, Some(212));
    assert_eq!(r.technology.as_deref(), Some("Li-ion"));
    assert_eq!(r.manufacturer.as_deref(), Some("SMP"));
    assert_eq!(r.model.as_deref(), Some("bq40z651"));
    let rows = r.rows();
    assert!(rows.iter().any(|(k, v)| k == "Health" && v.starts_with("83%")));
    assert!(rows.iter().any(|(k, v)| k == "Power adapter" && v == "Not connected"));
}

#[test]
fn the_charge_variant_works_the_same() {
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("charge_now", "2000000"), ("charge_full", "4000000"), ("charge_full_design", "5000000"), ("current_now", "1000000")]);
    let mut r = s.read();
    assert_eq!(r.percent_whole(), Some(50));
    Tracker::new().apply(&mut r);
    assert_eq!(r.time_secs, Some((TimeKind::UntilEmpty, 2 * 3600)));
    assert_eq!(r.health_pct.map(|h| h.round() as u32), Some(80));
    assert_eq!(r.ac_online, None, "no adapter listed: not reported");
}

#[test]
fn negative_current_while_discharging_is_still_a_draw() {
    let s = Sys::new();
    laptop(&s, "Discharging", 30_000_000, 50_000_000, -10_000_000);
    let mut r = s.read();
    Tracker::new().apply(&mut r);
    assert_eq!(r.time_secs.map(|t| t.1), Some(3 * 3600));
}

#[test]
fn charging_shows_a_bolt_and_time_to_full() {
    let s = Sys::new();
    s.ac(true);
    laptop(&s, "Charging", 20_000_000, 50_000_000, 15_000_000);
    let mut r = s.read();
    assert_eq!(r.icon_state(), IconState::Charging);
    assert_eq!(r.status_words(), "Charging");
    Tracker::new().apply(&mut r);
    // 30 Wh missing at 15 W: 2 hours
    assert_eq!(r.time_secs, Some((TimeKind::UntilFull, 2 * 3600)));
    assert_eq!(r.time_words().as_deref(), Some("About 2 h until full"));
    assert_eq!(r.low_band(), LowBand::Normal);
}

#[test]
fn full_and_not_charging_on_the_adapter_show_the_plug() {
    let s = Sys::new();
    s.ac(true);
    laptop(&s, "Full", 50_000_000, 50_000_000, 0);
    let mut r = s.read();
    assert_eq!(r.icon_state(), IconState::Plugged);
    assert_eq!(r.status_words(), "Full");
    Tracker::new().apply(&mut r);
    assert_eq!(r.time_secs, None);
    laptop(&s, "Not charging", 40_000_000, 50_000_000, 0);
    let r = s.read();
    assert_eq!(r.icon_state(), IconState::Plugged);
    assert_eq!(r.status_words(), "Plugged in but not charging (battery protection)");
    // not charging and NOT on the adapter is just "not charging", drawn as a plain battery
    s.ac(false);
    let r = s.read();
    assert_eq!(r.status_words(), "Not charging");
    assert_eq!(r.icon_state(), IconState::Discharging);
}

#[test]
fn capacity_only_capacity_level_only_and_unknown() {
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("capacity", "42")]);
    let r = s.read();
    assert_eq!(r.percent_whole(), Some(42));
    assert_eq!(r.status, Status::Unknown);
    assert_eq!(r.status_words(), "Status unknown");
    assert_eq!(r.health_pct, None);
    assert_eq!(r.time_words(), None);

    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("capacity_level", "Low"), ("status", "Discharging")]);
    let r = s.read();
    assert_eq!(r.percent, None);
    assert_eq!(r.level, Level::Low);
    assert_eq!(r.percent_text(), "?");
    assert_eq!(r.low_band(), LowBand::Amber);
    assert!((r.fill_fraction() - 0.15).abs() < 1e-6);
    assert!(r.rows().iter().any(|(k, v)| k == "Charge" && v.starts_with("Low")));
    assert_eq!(r.icon_state(), IconState::Discharging);

    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery")]);
    let r = s.read();
    assert!(r.shown() && r.readable());
    assert_eq!(r.icon_state(), IconState::Unknown, "nothing known: a question mark");
    assert_eq!(r.fill_fraction(), 0.0);
}

#[test]
fn a_bluetooth_mouse_is_not_the_computers_battery() {
    let s = Sys::new();
    s.ac(true);
    s.supply("hid-aa:bb:cc:dd:ee:ff-battery", &[("type", "Battery"), ("scope", "Device"), ("capacity", "15"), ("status", "Discharging")]);
    assert!(!s.read().shown(), "device scope: ignored");
    // an older kernel without the scope file: the name gives it away
    s.supply("hidpp_battery_0", &[("type", "Battery"), ("capacity", "15")]);
    assert!(!s.read().shown());
    // the real battery next to them is the only one counted
    laptop(&s, "Discharging", 25_000_000, 50_000_000, 10_000_000);
    let r = s.read();
    assert_eq!(r.cells.len(), 1);
    assert_eq!(r.cells[0].name, "BAT0");
    assert_eq!(r.percent_whole(), Some(50));
}

#[test]
fn an_empty_battery_bay_is_not_a_battery() {
    let s = Sys::new();
    laptop(&s, "Unknown", 0, 0, 0);
    s.supply("BAT0", &[("present", "0")]);
    assert!(!s.read().shown());
}

#[test]
fn two_batteries_are_combined_weighted_by_full_capacity() {
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("energy_now", "30000000"), ("energy_full", "30000000"), ("energy_full_design", "40000000"), ("power_now", "5000000"), ("capacity", "100")]);
    s.supply("BAT1", &[("type", "Battery"), ("status", "Discharging"), ("energy_now", "0"), ("energy_full", "10000000"), ("energy_full_design", "10000000"), ("power_now", "5000000"), ("capacity", "0")]);
    let mut r = s.read();
    // 30 of 40 Wh together, not the 50 % a plain average would say
    assert_eq!(r.percent_whole(), Some(75));
    assert_eq!(r.cells.len(), 2);
    assert_eq!(r.health_pct.map(|h| h.round() as u32), Some(80));
    Tracker::new().apply(&mut r);
    // 30 Wh at 10 W together
    assert_eq!(r.time_secs.map(|t| t.1), Some(3 * 3600));
    assert!(r.rows().iter().any(|(k, v)| k == "Batteries" && v == "2"));
    // one discharging, one charging: charging wins the status
    s.supply("BAT1", &[("status", "Charging")]);
    assert_eq!(s.read().status, Status::Charging);
}

#[test]
fn a_battery_in_charge_units_and_one_in_energy_units_meet_through_the_voltage() {
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("energy_now", "20000000"), ("energy_full", "40000000"), ("power_now", "10000000")]);
    s.supply("BAT1", &[("type", "Battery"), ("status", "Discharging"), ("charge_now", "2000000"), ("charge_full", "2000000"), ("current_now", "1000000"), ("voltage_now", "10000000")]);
    let r = s.read();
    // 20 + 20 Wh of 40 + 20 Wh
    assert_eq!(r.percent_whole(), Some(67));
    // without a voltage the mixed amounts are dropped: the percentage still comes from the files, the time does not
    s.supply("BAT1", &[("voltage_now", "0")]);
    let mut r = s.read();
    Tracker::new().apply(&mut r);
    assert_eq!(r.time_secs, None);
    assert!(r.percent.is_some());
}

#[test]
fn garbage_negative_and_zero_values_are_not_believed() {
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("status", "Discharging"), ("capacity", "250"), ("energy_now", "-5"), ("energy_full", "0"), ("power_now", "abc"), ("cycle_count", "-1"), ("technology", "Unknown"), ("manufacturer", "  "), ("model_name", "0")]);
    let mut r = s.read();
    assert_eq!(r.percent, None, "250 percent is not a charge");
    assert_eq!(r.health_pct, None);
    assert_eq!(r.cycles, None);
    assert_eq!(r.technology, None);
    assert_eq!(r.manufacturer, None);
    assert_eq!(r.model, None);
    Tracker::new().apply(&mut r);
    assert_eq!(r.time_secs, None);
    assert_eq!(r.icon_state(), IconState::Unknown);

    // a reported 0 that the energy files contradict is a driver that has not read the pack yet
    let s = Sys::new();
    laptop(&s, "Discharging", 25_000_000, 50_000_000, 10_000_000);
    s.supply("BAT0", &[("capacity", "0")]);
    assert_eq!(s.read().percent_whole(), Some(50));
    // a reported 0 that nothing contradicts stays 0
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("capacity", "0")]);
    assert_eq!(s.read().percent_whole(), Some(0));
    // energy_now far above energy_full is dropped, and a design capacity of zero gives no health
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("energy_now", "900000000"), ("energy_full", "50000000"), ("energy_full_design", "0")]);
    let r = s.read();
    assert_eq!(r.percent, None);
    assert_eq!(r.health_pct, None);
    // a worn-out claim of 300 % health is not shown, and a new battery above 100 % shows 100
    let s = Sys::new();
    s.supply("BAT0", &[("type", "Battery"), ("energy_now", "1"), ("energy_full", "60000000"), ("energy_full_design", "20000000")]);
    assert_eq!(s.read().health_pct, None);
    s.supply("BAT0", &[("energy_full", "62000000"), ("energy_full_design", "60000000")]);
    assert_eq!(s.read().health_pct, Some(100.0));
}

#[test]
fn time_estimates_only_when_sane() {
    let mk = |status: &str, now: u64, full: u64, power: i64| {
        let s = Sys::new();
        laptop(&s, status, now, full, power);
        let mut r = s.read();
        Tracker::new().apply(&mut r);
        r.time_secs
    };
    assert!(mk("Discharging", 30_000_000, 50_000_000, 10_000_000).is_some());
    assert_eq!(mk("Discharging", 30_000_000, 50_000_000, 0), None, "no draw: no estimate");
    assert_eq!(mk("Discharging", 30_000_000, 50_000_000, 1_000), None, "a draw of a milliwatt-ish: measuring error");
    assert_eq!(mk("Discharging", 10_000, 50_000_000, 10_000_000), None, "under a minute is not shown");
    assert_eq!(mk("Discharging", 50_000_000, 50_000_000, 400_000), None, "125 hours is not believed");
    assert_eq!(mk("Full", 50_000_000, 50_000_000, 10_000_000), None);
    assert_eq!(mk("Unknown", 30_000_000, 50_000_000, 10_000_000), None);
    assert_eq!(mk("Charging", 50_000_000, 50_000_000, 10_000_000), None, "already full");
}

#[test]
fn the_estimate_is_smoothed_and_restarts_when_the_state_changes() {
    let s = Sys::new();
    laptop(&s, "Discharging", 30_000_000, 50_000_000, 10_000_000);
    let mut t = Tracker::new();
    let mut r = s.read();
    t.apply(&mut r);
    assert_eq!(r.time_secs.map(|x| x.1), Some(3 * 3600));
    // a spike to 40 W moves the average only a little (0.7 * 10 + 0.3 * 40 = 19 W)
    laptop(&s, "Discharging", 30_000_000, 50_000_000, 40_000_000);
    let mut r = s.read();
    t.apply(&mut r);
    let secs = r.time_secs.unwrap().1 as f64;
    assert!((secs - 30.0 / 19.0 * 3600.0).abs() < 2.0, "{secs}");
    assert_eq!(r.smooth_rate.map(|x| x.round() as u64), Some(19_000_000));
    // plugging in resets it: the first charging reading uses its own number
    s.ac(true);
    laptop(&s, "Charging", 30_000_000, 50_000_000, 20_000_000);
    let mut r = s.read();
    t.apply(&mut r);
    assert_eq!(r.time_secs, Some((TimeKind::UntilFull, 3600)));
}

#[test]
fn duration_words_are_plain() {
    assert_eq!(duration_words(0), "under a minute");
    assert_eq!(duration_words(29), "under a minute");
    assert_eq!(duration_words(60), "1 min");
    assert_eq!(duration_words(45 * 60), "45 min");
    assert_eq!(duration_words(3600), "1 h");
    assert_eq!(duration_words(3600 + 10 * 60), "1 h 10 min");
}

#[test]
fn a_laptop_whose_kernel_has_no_battery_driver_says_so() {
    let s = Sys::new();
    s.ac(false);
    s.put("class/dmi/id/chassis_type", "10");
    s.put("bus/acpi/devices/PNP0C0A:00/status", "15");
    let r = s.read();
    assert_eq!(r.presence, Presence::NoDriver);
    assert!(r.shown() && !r.readable());
    assert_eq!(r.icon_state(), IconState::Unknown);
    assert_eq!(r.rows(), vec![("Battery".to_string(), "No battery driver: this image needs a newer kernel".to_string())]);
    assert_eq!(r.summary(), "Battery: No battery driver: this image needs a newer kernel");
    assert!(!is_critical(&r));
    // a battery that ACPI says is inserted counts even without a chassis type (status bit 4)
    let s = Sys::new();
    s.put("bus/acpi/devices/PNP0C0A:00/status", "31");
    assert_eq!(s.read().presence, Presence::NoDriver);
    // an ACPI battery device with nothing inside on a desktop chassis: nothing to show
    let s = Sys::new();
    s.put("class/dmi/id/chassis_type", "3");
    s.put("bus/acpi/devices/PNP0C0A:00/status", "15");
    assert!(!s.read().shown());
    // with a BAT0 there is no complaint
    let s = Sys::new();
    s.put("bus/acpi/devices/PNP0C0A:00/status", "31");
    laptop(&s, "Discharging", 30_000_000, 50_000_000, 10_000_000);
    assert_eq!(s.read().presence, Presence::Present);
}

#[test]
fn a_usb_c_charger_counts_as_the_adapter_but_a_device_scope_one_does_not() {
    let s = Sys::new();
    s.supply("ucsi-source-psy-1", &[("type", "USB"), ("online", "1")]);
    assert_eq!(ac_online(&s.env()), Some(true));
    let s = Sys::new();
    s.supply("ADP1", &[("type", "Mains"), ("online", "0")]);
    s.supply("peripheral", &[("type", "USB"), ("online", "1"), ("scope", "Device")]);
    assert_eq!(ac_online(&s.env()), Some(false));
}

#[test]
fn settings_default_to_everything_on_and_old_files_load() {
    let d = BatterySettings::default();
    assert!(d.show_percent() && d.warnings() && d.shutdown_at_critical());
    let back: BatterySettings = serde_json::from_str("{}").unwrap();
    assert_eq!(back, d);
    let off: BatterySettings = serde_json::from_str("{\"no_shutdown\":true}").unwrap();
    assert!(!off.shutdown_at_critical() && off.warnings());
    assert_eq!(AlertCfg::from(&off), AlertCfg { warnings: true, shutdown: false });
}

// ---- warnings and the critical shutdown ---------------------------------------------------------------------------------

/// A reading by hand: status, percent, adapter.
fn rd(status: Status, pct: Option<f32>, ac: Option<bool>) -> Reading {
    let mut r = Reading::none();
    r.presence = Presence::Present;
    r.status = status;
    r.percent = pct;
    r.ac_online = ac;
    r
}

const ON: AlertCfg = AlertCfg { warnings: true, shutdown: true };

fn dis(p: f32) -> Reading {
    rd(Status::Discharging, Some(p), Some(false))
}

#[test]
fn warnings_come_once_per_level() {
    let mut a = Alerter::new();
    assert!(a.step(&dis(50.0), 0, ON).is_empty());
    assert!(a.step(&dis(21.0), 10, ON).is_empty());
    assert_eq!(a.step(&dis(20.0), 20, ON), vec![Alert::Low(20)]);
    assert!(a.step(&dis(19.0), 30, ON).is_empty(), "no repeat");
    assert!(a.step(&dis(15.0), 40, ON).is_empty());
    assert_eq!(a.step(&dis(10.0), 50, ON), vec![Alert::Low(10)]);
    assert!(a.step(&dis(9.0), 60, ON).is_empty());
    assert_eq!(a.step(&dis(5.0), 70, ON), vec![Alert::PlugInNow]);
    assert!(a.step(&dis(4.0), 80, ON).is_empty());
}

#[test]
fn a_jump_across_levels_gives_only_the_deepest_warning() {
    let mut a = Alerter::new();
    assert_eq!(a.step(&dis(8.0), 0, ON), vec![Alert::Low(10)]);
    assert!(a.step(&dis(7.0), 10, ON).is_empty());
    let mut a = Alerter::new();
    assert_eq!(a.step(&dis(4.0), 0, ON), vec![Alert::PlugInNow]);
    assert!(a.step(&dis(3.9), 10, ON).is_empty(), "the 20 and 10 toasts are not queued behind it");
}

#[test]
fn a_level_rearms_only_after_the_hysteresis_or_a_cable() {
    let mut a = Alerter::new();
    assert_eq!(a.step(&dis(20.0), 0, ON), vec![Alert::Low(20)]);
    // flickering around the threshold does not repeat it
    for (i, p) in [21.0, 20.0, 22.0, 20.0, 19.0].iter().enumerate() {
        assert!(a.step(&dis(*p), 10 + i as u64, ON).is_empty());
    }
    // back above by the margin (a charge, then unplugged again)
    assert!(a.step(&dis(24.0), 30, ON).is_empty());
    assert_eq!(a.step(&dis(20.0), 40, ON), vec![Alert::Low(20)]);
    // a cable re-arms everything
    assert!(a.step(&rd(Status::Charging, Some(20.0), Some(true)), 50, ON).is_empty());
    assert_eq!(a.step(&dis(20.0), 60, ON), vec![Alert::Low(20)]);
}

#[test]
fn no_warning_while_charging_on_the_adapter_or_with_warnings_off_or_without_data() {
    let mut a = Alerter::new();
    assert!(a.step(&rd(Status::Charging, Some(4.0), Some(true)), 0, ON).is_empty());
    assert!(a.step(&rd(Status::Discharging, Some(4.0), Some(true)), 10, ON).is_empty(), "the adapter says it is connected");
    assert!(a.step(&rd(Status::Unknown, Some(4.0), None), 20, ON).is_empty(), "status unknown");
    assert!(a.step(&rd(Status::Discharging, Some(0.0), None), 30, ON).is_empty(), "0 is not a reading");
    assert!(a.step(&rd(Status::Discharging, None, None), 40, ON).is_empty());
    assert!(a.step(&Reading::none(), 50, ON).is_empty());
    let mut nd = Reading::none();
    nd.presence = Presence::NoDriver;
    assert!(a.step(&nd, 60, ON).is_empty());
    let off = AlertCfg { warnings: false, shutdown: true };
    assert!(Alerter::new().step(&dis(4.0), 0, off).is_empty());
}

#[test]
fn critical_is_decided_in_one_place_and_is_hard_to_reach() {
    assert!(is_critical(&dis(3.0)));
    assert!(is_critical(&dis(1.0)));
    assert!(!is_critical(&dis(4.0)));
    assert!(!is_critical(&dis(0.0)), "0 alone is a driver that has not read the battery");
    assert!(!is_critical(&rd(Status::Discharging, None, Some(false))), "unknown charge");
    assert!(!is_critical(&rd(Status::Charging, Some(2.0), Some(true))));
    assert!(!is_critical(&rd(Status::Charging, Some(2.0), None)), "charging, even without an adapter listed");
    assert!(!is_critical(&rd(Status::Discharging, Some(2.0), Some(true))), "adapter connected");
    assert!(!is_critical(&rd(Status::Full, Some(2.0), None)));
    assert!(!is_critical(&rd(Status::NotCharging, Some(2.0), Some(true))));
    assert!(!is_critical(&rd(Status::Unknown, Some(2.0), None)), "status unknown");
    assert!(!is_critical(&Reading::none()));
    // the firmware's own Critical counts, with or without a percentage, but only while discharging
    let mut r = rd(Status::Discharging, None, Some(false));
    r.level = Level::Critical;
    assert!(is_critical(&r));
    r.percent = Some(0.0);
    assert!(is_critical(&r), "0 with the firmware agreeing");
    r.status = Status::Charging;
    assert!(!is_critical(&r));
    r.status = Status::Discharging;
    r.ac_online = Some(true);
    assert!(!is_critical(&r));
    // a machine without readable battery data
    let mut nd = Reading::none();
    nd.presence = Presence::NoDriver;
    nd.level = Level::Critical;
    nd.status = Status::Discharging;
    assert!(!is_critical(&nd));
}

#[test]
fn the_countdown_needs_two_readings_runs_30_seconds_and_then_shuts_down_once() {
    let mut a = Alerter::new();
    // the 5 % card first
    assert_eq!(a.step(&dis(4.0), 0, ON), vec![Alert::PlugInNow]);
    // the first critical reading is not enough
    assert!(a.step(&dis(3.0), 10, ON).is_empty());
    assert!(!a.counting_down());
    assert_eq!(a.step(&dis(3.0), 20, ON), vec![Alert::CountdownStart]);
    assert_eq!(a.countdown_left(20), Some(30));
    assert_eq!(a.countdown_left(35), Some(15));
    assert!(a.step(&dis(3.0), 40, ON).is_empty());
    assert!(a.step(&dis(2.0), 49, ON).is_empty());
    assert_eq!(a.step(&dis(2.0), 50, ON), vec![Alert::Shutdown]);
    assert!(!a.counting_down());
    // after the order was given there is no second one from the same state until two new readings
    assert!(a.step(&dis(2.0), 51, ON).is_empty());
}

#[test]
fn plugging_in_cancels_the_countdown() {
    let mut a = Alerter::new();
    a.step(&dis(3.0), 0, ON);
    assert_eq!(a.step(&dis(3.0), 10, ON), vec![Alert::CountdownStart]);
    assert_eq!(a.step(&rd(Status::Charging, Some(3.0), Some(true)), 20, ON), vec![Alert::CountdownCancelled]);
    assert!(!a.counting_down());
    // and a shutdown cannot follow from the same minute
    assert!(a.step(&rd(Status::Charging, Some(3.0), Some(true)), 45, ON).is_empty());
    // unplugged again at 3 %: a fresh start needs two readings again
    // (the cable re-armed the warnings, so the 5 % card comes again, but no shutdown)
    assert_eq!(a.step(&dis(3.0), 50, ON), vec![Alert::PlugInNow]);
    assert_eq!(a.step(&dis(3.0), 60, ON), vec![Alert::CountdownStart]);
}

#[test]
fn losing_the_data_or_switching_the_setting_off_cancels_the_countdown() {
    let mut a = Alerter::new();
    a.step(&dis(3.0), 0, ON);
    a.step(&dis(3.0), 10, ON);
    assert_eq!(a.step(&Reading::none(), 15, ON), vec![Alert::CountdownCancelled]);
    let mut a = Alerter::new();
    a.step(&dis(3.0), 0, ON);
    a.step(&dis(3.0), 10, ON);
    assert_eq!(a.step(&rd(Status::Discharging, Some(0.0), Some(false)), 15, ON), vec![Alert::CountdownCancelled], "a reading of 0 is not a reason to shut down");
    let mut a = Alerter::new();
    a.step(&dis(3.0), 0, ON);
    a.step(&dis(3.0), 10, ON);
    let off = AlertCfg { warnings: true, shutdown: false };
    assert_eq!(a.step(&dis(3.0), 15, off), vec![Alert::CountdownCancelled]);
    assert!(Alerter::new().step(&dis(1.0), 0, off).iter().all(|x| *x == Alert::PlugInNow));
    for t in 1..100 {
        assert!(!Alerter::new().step(&dis(1.0), t, off).contains(&Alert::Shutdown));
    }
}

#[test]
fn a_person_who_cancels_is_left_alone_until_the_charge_recovers_or_a_cable_comes() {
    let mut a = Alerter::new();
    a.step(&dis(3.0), 0, ON);
    a.step(&dis(3.0), 10, ON);
    a.cancel_by_person();
    assert!(!a.counting_down());
    for t in 20..200 {
        assert!(a.step(&dis(3.0), t, ON).is_empty(), "no new countdown");
    }
    // a cable and then unplugged at 3 % starts again
    a.step(&rd(Status::Charging, Some(3.0), Some(true)), 300, ON);
    a.step(&dis(3.0), 310, ON);
    assert_eq!(a.step(&dis(3.0), 320, ON), vec![Alert::CountdownStart]);
}

#[test]
fn a_clock_jump_or_a_stalled_shell_never_cuts_the_30_seconds_short() {
    let mut a = Alerter::new();
    a.step(&dis(3.0), 100, ON);
    assert_eq!(a.step(&dis(3.0), 110, ON), vec![Alert::CountdownStart]);
    // the next reading is much later (the shell stalled, or the clock was set): the countdown starts over instead of firing
    assert!(a.step(&dis(3.0), 1_000, ON).is_empty());
    assert_eq!(a.countdown_left(1_000), Some(30));
    // a clock that went backwards too
    assert!(a.step(&dis(3.0), 500, ON).is_empty());
    assert!(a.counting_down());
    assert_eq!(a.step(&dis(3.0), 530, ON), vec![Alert::Shutdown]);
}

#[test]
fn a_firmware_critical_level_without_a_percentage_starts_the_countdown() {
    let mut r = rd(Status::Discharging, None, Some(false));
    r.level = Level::Critical;
    let mut a = Alerter::new();
    assert!(a.step(&r, 0, ON).is_empty());
    assert_eq!(a.step(&r, 10, ON), vec![Alert::CountdownStart]);
}

#[test]
fn nothing_ever_shuts_down_a_machine_that_merely_lacks_battery_data() {
    let mut a = Alerter::new();
    let mut nd = Reading::none();
    nd.presence = Presence::NoDriver;
    let desktops = [Reading::none(), nd, rd(Status::Unknown, None, None), rd(Status::Discharging, None, None), rd(Status::Discharging, Some(0.0), None), rd(Status::Unknown, Some(0.0), Some(false))];
    for t in 0..200 {
        for r in &desktops {
            assert!(!a.step(r, t, ON).iter().any(|x| matches!(x, Alert::Shutdown | Alert::CountdownStart)));
        }
    }
}

#[test]
fn a_real_fake_tree_end_to_end_reaches_the_countdown() {
    let s = Sys::new();
    laptop(&s, "Discharging", 1_000_000, 50_000_000, 10_000_000);
    s.supply("BAT0", &[("capacity", "2"), ("capacity_level", "Critical")]);
    s.ac(false);
    let mut a = Alerter::new();
    assert!(a.step(&s.read(), 0, ON).contains(&Alert::PlugInNow));
    assert_eq!(a.step(&s.read(), 10, ON), vec![Alert::CountdownStart], "the second critical reading");
    s.ac(true);
    assert_eq!(a.step(&s.read(), 15, ON), vec![Alert::CountdownCancelled]);
}
