//! Settings -> Bluetooth on a made-up Mac mini 2018 whose controller runs in its ROM state (no firmware file, all-zero address, no sysfs
//! `address` file): the controller line from the kernel's management interface, Power on, the 10 second scan with its results, "Load the
//! Bluetooth driver anyway", the honest sentences, who may do what, cloud mode, and the page at four sizes. A fake machine answers; nothing
//! pairs.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_devctl::btmgmt::{Found, Kind};
use iyakki_devctl::btview::{Controller, NOT_YET};
use std::path::Path;
use std::sync::Mutex;

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    a.go_to(t);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

/// The Mac mini's controller as the kernel reports it, and what was asked of it.
struct Mini {
    powered: Mutex<bool>,
    calls: Mutex<Vec<String>>,
    held: Mutex<bool>,
    fw_file: bool,
    address: &'static str,
    power_error: Option<String>,
    scan_result: Result<Vec<Found>, String>,
}

impl Mini {
    fn new() -> Arc<Mini> {
        Arc::new(Mini { powered: Mutex::new(false), calls: Mutex::new(vec![]), held: Mutex::new(true), fw_file: false, address: "00:00:00:00:00:00", power_error: None, scan_result: Ok(vec![found("Keyboard K380", "11:22:33:44:55:66", -48), found("", "06:05:04:03:02:01", -80)]) })
    }
}

fn found(name: &str, address: &str, rssi: i8) -> Found {
    Found { address: address.into(), addr_type: 0, rssi, name: name.into(), class: None, appearance: None, services: vec![], kind: Kind::Other }
}

impl iyakki_devctl::Control for Mini {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        Err("no mixer".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("no mixer".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn bt_controllers(&self) -> Result<Vec<Controller>, String> {
        let on = *self.powered.lock().unwrap();
        Ok(vec![Controller { index: 0, configured: true, address: self.address.into(), version: Some(6), manufacturer: 15, supported: 0x281, current: if on { 1 } else { 0 }, name: "iyakki".into() }])
    }
    fn bt_power(&self, index: u16, on: bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("power {index} {on}"));
        if let Some(e) = &self.power_error {
            return Err(e.clone());
        }
        *self.powered.lock().unwrap() = on;
        Ok(())
    }
    fn bt_scan(&self, index: u16, secs: u64) -> Result<Vec<Found>, String> {
        self.calls.lock().unwrap().push(format!("scan {index} {secs}"));
        self.scan_result.clone()
    }
    fn bt_firmware_file(&self) -> bool {
        self.fw_file
    }
    fn bt_held(&self) -> bool {
        *self.held.lock().unwrap()
    }
    fn bt_load_anyway(&self) -> Result<String, String> {
        self.calls.lock().unwrap().push("load anyway".into());
        *self.held.lock().unwrap() = false;
        Ok("Bluetooth: 2 driver file(s) given back".into())
    }
}

fn mac_mini(tag: &str, ctl: Arc<Mini>) -> DeviceEnv {
    let d = temp_dir(&format!("{tag}-bt"));
    let sys = d.join("sys");
    put(&sys, "class/dmi/id/product_name", "Macmini8,1\n");
    put(&sys, "class/bluetooth/hci0/x", "");
    put(&sys, "class/rfkill/rfkill1/name", "hci0\n");
    put(&sys, "class/rfkill/rfkill1/type", "bluetooth\n");
    put(&sys, "class/rfkill/rfkill1/soft", "0\n");
    put(&sys, "class/rfkill/rfkill1/hard", "0\n");
    std::fs::create_dir_all(d.join("proc")).unwrap();
    DeviceEnv { roots: iyakki_devctl::Roots { sys, proc_: d.join("proc") }, ctl }
}

fn rows(a: &mut SettingsApp) -> Vec<DevRow> {
    a.dev_rows()
}

fn wait_scan(a: &mut SettingsApp, cx: &AppCx) -> Vec<DevRow> {
    for _ in 0..400 {
        a.hw.invalidate();
        let r = rows(a);
        if !r.iter().any(|x| x.chips.iter().any(|c| c.0 == "scanning")) {
            return r;
        }
        a.handle(&Event::Tick, cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the scan did not finish");
}

#[test]
fn the_rom_state_controller_is_described_with_chip_address_power_and_the_reason() {
    let cx = admin("bt1");
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt1", Mini::new()));
    goto(&mut a, &cx, Tab::Bluetooth);
    let r = rows(&mut a);
    let head = r.iter().find(|x| x.title.starts_with("Controller hci0")).expect("the controller row");
    assert_eq!(head.title, "Controller hci0: BCM4364 (ROM state, no firmware file)");
    assert!(head.sub.starts_with("address 00:00:00:00:00:00 (not set) · powered off · no valid address"), "{}", head.sub);
    assert!(head.chips.iter().any(|c| c.0 == "no valid address") && head.chips.iter().any(|c| c.0 == "OFF"));
    assert!(!r.iter().any(|x| x.chips.iter().any(|c| c.0 == "no answer")), "the old sysfs-address chip is gone when the kernel answers");
    assert!(r.iter().any(|x| x.title == "Scan for devices (10 s)"));
    assert!(r.iter().any(|x| x.title == "Load the Bluetooth driver anyway" && x.target == Target::BtLoad));
    // the sentences under the page
    let notes = a.hw.bt_notes(0).join(" ");
    for w in ["no valid address", "This Mac's Bluetooth chip has no firmware file in macOS: the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option", "Pairing and audio are not available yet", "Bluetooth stack", "docs/BETA-PLAN.md"] {
        assert!(notes.contains(w), "{w} in {notes}");
    }
    assert!(NOT_YET.contains("only reads the controller, powers it and scans"));
}

#[test]
fn power_on_works_a_refusal_is_said_plainly_and_a_viewer_cannot() {
    let cx = admin("bt2");
    let ctl = Mini::new();
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt2", ctl.clone()));
    goto(&mut a, &cx, Tab::Bluetooth);
    key(&mut a, &cx, Key::Char('p'));
    assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["power 0 true"]);
    assert!(status(&a).contains("powered on"), "{}", status(&a));
    let r = rows(&mut a);
    let head = r.iter().find(|x| x.title.starts_with("Controller hci0")).unwrap();
    assert!(head.sub.contains("powered on") && head.chips.iter().any(|c| c.0 == "ON"), "{head:?}");
    key(&mut a, &cx, Key::Char('p'));
    assert_eq!(ctl.calls.lock().unwrap().last().unwrap(), "power 0 false", "the same key switches it off again");
    // a refusal from the kernel
    let bad = Arc::new(Mini { power_error: Some("the kernel will not power this controller: it has no valid address".into()), ..Arc::try_unwrap(Mini::new()).ok().unwrap() });
    let mut b = SettingsApp::new(&cx).with_hardware(mac_mini("bt2b", bad));
    goto(&mut b, &cx, Tab::Bluetooth);
    key(&mut b, &cx, Key::Char('p'));
    assert!(status(&b).contains("no valid address"), "{}", status(&b));
}

#[test]
fn a_scan_runs_ten_seconds_in_the_background_and_lists_name_address_and_signal() {
    let cx = admin("bt3");
    let ctl = Mini::new();
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt3", ctl.clone()));
    goto(&mut a, &cx, Tab::Bluetooth);
    key(&mut a, &cx, Key::Char('p'));
    key(&mut a, &cx, Key::Char('s'));
    let r = wait_scan(&mut a, &cx);
    assert!(ctl.calls.lock().unwrap().contains(&"scan 0 10".to_string()));
    let kb = r.iter().find(|x| x.title == "Keyboard K380").expect("the keyboard is listed");
    assert_eq!(kb.sub, "11:22:33:44:55:66 · -48 dBm");
    assert!(r.iter().any(|x| x.title == "06:05:04:03:02:01" && x.sub.ends_with("-80 dBm")), "a device without a name shows its address");
    let scan_row = r.iter().find(|x| x.title.starts_with("Scan for devices")).unwrap();
    assert_eq!(scan_row.sub, "2 device(s) found in the last scan");
    // found devices cannot be acted on: no pairing anywhere
    assert!(r.iter().filter(|x| x.title == "Keyboard K380").all(|x| x.target == Target::Nothing));
    let sel = r.iter().position(|x| x.title == "Keyboard K380").unwrap();
    a.sel[a.tab] = sel;
    key(&mut a, &cx, Key::Enter);
    assert!(ctl.calls.lock().unwrap().iter().all(|c| c.starts_with("power") || c.starts_with("scan")), "only power and scan were ever called: {:?}", ctl.calls.lock().unwrap());
}

#[test]
fn a_failed_scan_says_why_and_a_scan_off_the_power_asks_for_power_first() {
    let cx = admin("bt4");
    let ctl = Arc::new(Mini { scan_result: Err("the controller is off: use Power on first".into()), ..Arc::try_unwrap(Mini::new()).ok().unwrap() });
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt4", ctl));
    goto(&mut a, &cx, Tab::Bluetooth);
    let r = rows(&mut a);
    assert!(r.iter().find(|x| x.title.starts_with("Scan for devices")).unwrap().sub.contains("power the controller on first"));
    key(&mut a, &cx, Key::Char('s'));
    let r = wait_scan(&mut a, &cx);
    let sc = r.iter().find(|x| x.title.starts_with("Scan for devices")).unwrap();
    assert!(sc.sub.contains("Power on first") && sc.chips.iter().any(|c| c.0 == "failed"), "{sc:?}");
    assert!(a.hw.bt_notes(0).join(" ").contains("Scan: the controller is off"));
}

#[test]
fn load_anyway_gives_the_held_drivers_back_once_and_the_row_goes_away() {
    let cx = admin("bt5");
    let ctl = Mini::new();
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt5", ctl.clone()));
    goto(&mut a, &cx, Tab::Bluetooth);
    assert!(a.hw.bt_notes(0).join(" ").contains("set aside on this Mac until firmware exists"));
    key(&mut a, &cx, Key::Char('l'));
    assert_eq!(ctl.calls.lock().unwrap().clone(), vec!["load anyway"]);
    assert!(status(&a).contains("given back"), "{}", status(&a));
    assert!(!rows(&mut a).iter().any(|x| x.target == Target::BtLoad));
    key(&mut a, &cx, Key::Char('l'));
    assert!(status(&a).contains("nothing to load"), "{}", status(&a));
    assert_eq!(ctl.calls.lock().unwrap().len(), 1);
}

#[test]
fn a_mac_with_the_firmware_file_does_not_say_rom_state_and_a_machine_without_a_controller_shows_none() {
    let cx = admin("bt6");
    let ctl = Arc::new(Mini { fw_file: true, held: Mutex::new(false), ..Arc::try_unwrap(Mini::new()).ok().unwrap() });
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt6", ctl));
    goto(&mut a, &cx, Tab::Bluetooth);
    let r = rows(&mut a);
    assert!(r.iter().any(|x| x.title == "Controller hci0: BCM4364") && !r.iter().any(|x| x.target == Target::BtLoad));
    assert!(!a.hw.bt_notes(0).join(" ").contains("ROM state"));
    // no hci0 in sysfs: the management interface is not even asked
    let d = temp_dir("bt6-none");
    std::fs::create_dir_all(d.join("sys")).unwrap();
    let env = DeviceEnv { roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl: Mini::new() };
    let mut b = SettingsApp::new(&cx).with_hardware(env);
    goto(&mut b, &cx, Tab::Bluetooth);
    assert!(!rows(&mut b).iter().any(|x| x.title.starts_with("Controller")));
}

#[test]
fn cloud_mode_hides_the_bluetooth_page() {
    use iyakki_agent::cloudmode::{cloud_mode, Facts, Setting};
    let cx = admin("bt7");
    let on = SettingsApp::new(&cx);
    assert!(on.tabs.contains(&Tab::Bluetooth));
    let hidden = SettingsApp::new(&cx).with_cloud(cloud_mode(Setting::On, &Facts::full()));
    assert!(!hidden.tabs.contains(&Tab::Bluetooth), "no hardware page on a server");
}

#[test]
fn the_page_draws_inside_the_window_at_four_sizes_with_its_buttons() {
    let cx = admin("bt8");
    for (w, h) in [(360u32, 740u32), (800, 480), (1024, 600), (1280, 720), (1920, 1080)] {
        let ctl = Mini::new();
        let mut a = SettingsApp::new(&cx).with_hardware(mac_mini(&format!("bt8-{w}"), ctl));
        goto(&mut a, &cx, Tab::Bluetooth);
        key(&mut a, &cx, Key::Char('p'));
        key(&mut a, &cx, Key::Char('s'));
        wait_scan(&mut a, &cx);
        for sel in [0usize, 1, 3] {
            a.sel[a.tab] = sel;
            let g = draw(&mut a, &cx, w, h);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
            }
            assert!(g.hits.iter().any(|x| x.label.starts_with("Controller hci0")), "{w}x{h}: the controller row is on the page");
            for b in ["Power", "Scan (10 s)"] {
                assert!(g.hits.iter().any(|x| x.label.contains(b)), "{w}x{h}: button {b} among {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>());
            }
        }
    }
}

#[test]
fn a_controller_without_firmware_says_so_first_and_get_firmware_leads_to_store_setup() {
    let cx = admin("bt9");
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt9", Mini::new()));
    goto(&mut a, &cx, Tab::Bluetooth);
    let r = rows(&mut a);
    assert_eq!(r[0].title, "Bluetooth needs firmware");
    assert_eq!(r[0].sub, "Bluetooth needs firmware: Store \u{2192} Setup \u{2192} Hardware");
    assert_eq!(r[0].target, Target::BtFirmware);
    assert!(!r.iter().any(|x| x.title.starts_with("Bluetooth \u{b7} ")), "no switch row that reads like a working device");
    assert!(a.hw.bt_notes(0).join(" ").starts_with("Bluetooth needs firmware"));
    // f (and Enter on the row) ask the shell for Store -> Setup on the Mac firmware card, once
    a.sel[a.tab] = 0;
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())));
    assert_eq!(a.take_request(), None);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())));
}

#[test]
fn a_working_controller_keeps_the_normal_list_and_has_no_firmware_button() {
    let cx = admin("bt10");
    let ctl = Arc::new(Mini { address: "A4:83:E7:11:22:33", held: Mutex::new(false), ..Arc::try_unwrap(Mini::new()).ok().unwrap() });
    let mut a = SettingsApp::new(&cx).with_hardware(mac_mini("bt10", ctl));
    goto(&mut a, &cx, Tab::Bluetooth);
    let r = rows(&mut a);
    assert!(!r.iter().any(|x| x.target == Target::BtFirmware || x.title == "Bluetooth needs firmware"));
    assert!(r.iter().any(|x| x.title.starts_with("Bluetooth \u{b7} ")) && r.iter().any(|x| x.title == "Controller hci0: BCM4364 (ROM state, no firmware file)"));
    assert!(!a.hw.bt_notes(0).join(" ").contains("needs firmware"));
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("does not need firmware") && a.take_request().is_none());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|x| x.label.contains("Firmware")));
}

#[test]
fn the_firmware_button_is_on_the_page_inside_the_window_and_a_click_on_it_asks_for_setup() {
    let cx = admin("bt11");
    for (w, h) in [(360u32, 740u32), (800, 480), (1024, 600), (1280, 720), (1920, 1080)] {
        let mut a = SettingsApp::new(&cx).with_hardware(mac_mini(&format!("bt11-{w}"), Mini::new()));
        goto(&mut a, &cx, Tab::Bluetooth);
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
        }
        let btn = g.hits.iter().find(|x| x.label.contains("Firmware")).unwrap_or_else(|| panic!("{w}x{h}: Firmware among {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        assert!(btn.rect.w >= 24.0 && btn.rect.h >= 16.0, "{w}x{h}: {:?}", btn.rect);
        let ev = btn.ev.clone();
        a.handle(&ev, &cx);
        assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())), "{w}x{h}");
    }
}
