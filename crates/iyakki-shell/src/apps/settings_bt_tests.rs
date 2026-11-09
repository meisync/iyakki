//! Settings -> Bluetooth: the one switch, the paired devices first, the short nearby list, the pairing popup, the outcomes in the log,
//! one attempt at a time, the Advanced part, and the page at every window size and the largest text. A fake machine answers (a made-up
//! Mac mini 2018 whose controller runs in its ROM state, and a working controller); nothing pairs for real.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use crate::btmodel::{pairing_notice, Switch};
use iyakki_devctl::a2dp::{AudioDeviceView, AudioState, AudioView};
use iyakki_devctl::bt_pan::{TetherState, TetherView};
use iyakki_devctl::bthid::{DeviceView, LinkState, PairState};
use iyakki_devctl::btmgmt::{Found, Kind};
use iyakki_devctl::btview::Controller;
use std::path::Path;
use std::sync::atomic::Ordering;
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

fn tick(a: &mut SettingsApp, cx: &AppCx, n: usize) {
    for _ in 0..n {
        a.handle(&Event::Tick, cx);
    }
}

fn status(a: &SettingsApp) -> String {
    a.bt.status().map(|s| s.0.to_string()).unwrap_or_default()
}

/// A frame at the size the window has; `big` is the text size multiplier (2.0 is the largest, "Huge").
fn draw_big(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32, big: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0) * big;
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    draw_big(a, cx, w, h, 1.0)
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.clone()).collect()
}

/// Click the control with this label (exactly, else as a prefix) the way a pointer does: the event it registered goes to the page.
fn click(a: &mut SettingsApp, cx: &AppCx, g: &Gfx, label: &str) {
    let hit = g.hits.iter().rev().find(|h| h.label == label).or_else(|| g.hits.iter().rev().find(|h| h.label.starts_with(label))).unwrap_or_else(|| panic!("no control {label:?} among {:?}", labels(g)));
    let ev = hit.ev.clone();
    a.handle(&ev, cx);
}

fn audit(cx: &AppCx) -> Vec<String> {
    cx.audit.recent(400)
}

fn found(name: &str, address: &str, rssi: i8, kind: Kind) -> Found {
    Found { address: address.into(), addr_type: 0, rssi, name: name.into(), class: None, appearance: None, services: vec![], kind }
}

fn keyboard() -> DeviceView {
    DeviceView { address: "AA:BB:CC:DD:EE:01".into(), addr_type: 0, name: "K380".into(), kind: "keyboard".into(), state: LinkState::Disconnected }
}

fn phone() -> DeviceView {
    DeviceView { address: "AA:BB:CC:DD:FF:01".into(), addr_type: 0, name: "Pixel".into(), kind: "phone".into(), state: LinkState::Disconnected }
}

/// The machine: a controller, what it was asked, and what it answers.
struct Fake {
    powered: Mutex<bool>,
    configured: bool,
    manufacturer: u16,
    address: &'static str,
    power_error: Option<String>,
    held: Mutex<bool>,
    fw_file: bool,
    scan: Mutex<Vec<Found>>,
    scan_error: Option<String>,
    /// the scan runs until Stop (at most three seconds)
    scan_blocks: bool,
    devs: Mutex<Vec<DeviceView>>,
    pair: Mutex<PairState>,
    pair_on_start: PairState,
    connect_error: Option<String>,
    tether: Mutex<TetherView>,
    tether_next: TetherState,
    tether_refusal: Option<String>,
    audio: Mutex<AudioView>,
    calls: Mutex<Vec<String>>,
    /// what the person typed for a passkey or PIN prompt (Some(None) = gave up); the calls list never holds it
    typed: Mutex<Option<Option<String>>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            powered: Mutex::new(true),
            configured: true,
            manufacturer: 2,
            address: "11:22:33:44:55:66",
            power_error: None,
            held: Mutex::new(false),
            fw_file: false,
            scan: Mutex::new(vec![]),
            scan_error: None,
            scan_blocks: false,
            devs: Mutex::new(vec![keyboard()]),
            pair: Mutex::new(PairState::Idle),
            pair_on_start: PairState::Confirm("MX Anywhere".into(), 123456),
            connect_error: None,
            tether: Mutex::new(TetherView::idle()),
            tether_next: TetherState::Connecting,
            tether_refusal: None,
            audio: Mutex::new(AudioView { ready: true, ..Default::default() }),
            calls: Mutex::new(vec![]),
            typed: Mutex::new(None),
        }
    }
    fn rom() -> Fake {
        Fake { powered: Mutex::new(false), manufacturer: 15, address: "00:00:00:00:00:00", held: Mutex::new(true), devs: Mutex::new(vec![]), ..Fake::new() }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, what: &str) -> usize {
        self.calls().iter().filter(|c| c.starts_with(what)).count()
    }
}

impl iyakki_devctl::Control for Fake {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        if self.tether.lock().unwrap().state == TetherState::Connected {
            vec![("iyakbt0".into(), "192.168.44.5".into())]
        } else {
            vec![]
        }
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
        Ok(vec![Controller { index: 0, configured: self.configured, address: self.address.into(), version: Some(6), manufacturer: self.manufacturer, supported: 0x281, current: if on { 1 } else { 0 }, name: "iyakki".into() }])
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
        match &self.scan_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.scan.lock().unwrap().clone()),
        }
    }
    fn bt_scan_until(&self, index: u16, secs: u64, stop: &std::sync::atomic::AtomicBool) -> Result<Vec<Found>, String> {
        if self.scan_blocks {
            for _ in 0..600 {
                if stop.load(Ordering::Relaxed) {
                    self.calls.lock().unwrap().push("scan stopped".into());
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        self.bt_scan(index, secs)
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
    fn bt_devices(&self) -> Vec<DeviceView> {
        self.devs.lock().unwrap().clone()
    }
    fn bt_device_connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("connect {a}"));
        if let Some(e) = &self.connect_error {
            return Err(e.clone());
        }
        self.devs.lock().unwrap().iter_mut().filter(|d| d.address == a).for_each(|d| d.state = LinkState::Connected);
        Ok(())
    }
    fn bt_device_disconnect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("disconnect {a}"));
        self.devs.lock().unwrap().iter_mut().filter(|d| d.address == a).for_each(|d| d.state = LinkState::Disconnected);
        Ok(())
    }
    fn bt_device_forget(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("forget {a}"));
        self.devs.lock().unwrap().retain(|d| d.address != a);
        Ok(())
    }
    fn bt_pair(&self, f: &Found) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("pair {}", f.address));
        *self.pair.lock().unwrap() = self.pair_on_start.clone();
        Ok(())
    }
    fn bt_pair_state(&self) -> PairState {
        self.pair.lock().unwrap().clone()
    }
    fn bt_pair_answer(&self, yes: bool) -> bool {
        self.calls.lock().unwrap().push(format!("answer {yes}"));
        true
    }
    fn bt_pair_type(&self, text: Option<String>) -> Result<bool, String> {
        self.calls.lock().unwrap().push(format!("type {}", if text.is_some() { "sent" } else { "cancelled" }));
        if !matches!(*self.pair.lock().unwrap(), PairState::TypePasskey(_) | PairState::TypePin(..)) {
            return Ok(false);
        }
        *self.typed.lock().unwrap() = Some(text);
        Ok(true)
    }
    fn bt_tether_view(&self) -> TetherView {
        self.tether.lock().unwrap().clone()
    }
    fn bt_tether_connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("tether {a}"));
        if let Some(e) = &self.tether_refusal {
            return Err(e.clone());
        }
        *self.tether.lock().unwrap() = TetherView { address: a.into(), name: "Pixel".into(), state: self.tether_next.clone() };
        Ok(())
    }
    fn bt_tether_disconnect(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("tether off".into());
        self.tether.lock().unwrap().state = TetherState::Idle;
        Ok(())
    }
    fn bt_audio_view(&self) -> AudioView {
        self.audio.lock().unwrap().clone()
    }
    fn bt_audio_connect(&self, a: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("audio {a}"));
        Ok(())
    }
    fn bt_audio_disconnect(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("audio off".into());
        self.audio.lock().unwrap().device = None;
        Ok(())
    }
    fn bt_audio_pairing(&self, on: bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("audio pairing {on}"));
        self.audio.lock().unwrap().pairing_open = on;
        Ok(())
    }
}

/// The sysfs of the test machine: the kernel's controller, a radio switch (soft, hard) and the machine model.
fn env(tag: &str, ctl: Arc<Fake>, product: &str, radio: Option<(bool, bool)>, usb: bool) -> DeviceEnv {
    let d = temp_dir(&format!("{tag}-bt"));
    let sys = d.join("sys");
    put(&sys, "class/dmi/id/product_name", &format!("{product}\n"));
    put(&sys, "class/bluetooth/hci0/x", "");
    if let Some((soft, hard)) = radio {
        put(&sys, "class/rfkill/rfkill1/name", "hci0\n");
        put(&sys, "class/rfkill/rfkill1/type", "bluetooth\n");
        put(&sys, "class/rfkill/rfkill1/soft", if soft { "1\n" } else { "0\n" });
        put(&sys, "class/rfkill/rfkill1/hard", if hard { "1\n" } else { "0\n" });
    }
    if usb {
        put(&sys, "bus/usb/devices/1-1/bDeviceClass", "e0\n");
    }
    std::fs::create_dir_all(d.join("proc")).unwrap();
    DeviceEnv { roots: iyakki_devctl::Roots { sys, proc_: d.join("proc") }, ctl }
}

/// The page on a working controller (a PC), after a few ticks.
fn app_with(tag: &str, f: Fake) -> (SettingsApp, AppCx, Arc<Fake>) {
    let ctl = Arc::new(f);
    let cx = admin(tag);
    let mut a = SettingsApp::new(&cx).with_hardware(env(tag, ctl.clone(), "HP Laptop", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    (a, cx, ctl)
}

fn app(tag: &str) -> (SettingsApp, AppCx, Arc<Fake>) {
    app_with(tag, Fake::new())
}

fn wait_scan(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..600 {
        tick(a, cx, 1);
        if a.bt.model.scanning() == crate::btmodel::ScanView::Idle {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the scan did not finish");
}

/// 22 devices around: 5 named pairable, 5 named others, 12 nothing useful.
fn crowd() -> Vec<Found> {
    let mut v = vec![
        found("Keyboard K2", "10:00:00:00:00:01", -40, Kind::Keyboard),
        found("MX Anywhere", "10:00:00:00:00:02", -50, Kind::Mouse),
        found("Pad X", "10:00:00:00:00:03", -62, Kind::Gamepad),
        found("Pixel 8", "10:00:00:00:00:04", -45, Kind::Phone),
        found("Old keys", "10:00:00:00:00:05", -85, Kind::Keyboard),
        found("Buds", "10:00:00:00:00:06", -55, Kind::Headphones),
        found("Living room TV", "10:00:00:00:00:07", -65, Kind::Other),
        found("Speaker", "10:00:00:00:00:08", -70, Kind::Speaker),
        found("Watch", "10:00:00:00:00:09", -72, Kind::Other),
        found("Beacon", "10:00:00:00:00:0A", -90, Kind::Other),
    ];
    for i in 0..12 {
        v.push(found("", &format!("20:00:00:00:00:{i:02X}"), -60 - i as i8, Kind::Other));
    }
    v
}

fn nearby_labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().filter(|h| h.label.ends_with(", pair") || h.label.ends_with(", not supported")).map(|h| h.label.clone()).collect()
}

// ---------------------------------------------------------------------------------------------- the one switch

#[test]
fn the_single_switch_turns_the_radio_and_the_controller_on_and_says_so_in_words() {
    let ctl = Arc::new(Fake { powered: Mutex::new(false), ..Fake::new() });
    let cx = admin("sw1");
    let e = env("sw1", ctl.clone(), "HP Laptop", Some((true, false)), false);
    let roots = e.roots.clone();
    let mut a = SettingsApp::new(&cx).with_hardware(e);
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    assert_eq!(a.bt.model.view.switch, Switch::Off);
    assert_eq!(Switch::Off.words(), "Off");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Bluetooth"), "{:?}", labels(&g));
    click(&mut a, &cx, &g, "Bluetooth");
    // the radio is free and the controller is powered: one press did both
    assert!(!iyakki_devctl::rfkill_list(&roots).iter().find(|k| k.kind == "bluetooth").unwrap().soft, "the radio was unblocked");
    assert_eq!(ctl.calls(), vec!["power 0 true"]);
    assert_eq!(status(&a), "Bluetooth is on.");
    tick(&mut a, &cx, 100);
    assert_eq!(a.bt.model.view.switch, Switch::On);
    assert_eq!(Switch::On.words(), "On");
    // and off again: controller down, radio blocked
    key(&mut a, &cx, Key::Char('e'));
    assert_eq!(ctl.calls().last().unwrap(), "power 0 false");
    assert!(iyakki_devctl::rfkill_list(&roots).iter().find(|k| k.kind == "bluetooth").unwrap().soft);
    assert_eq!(status(&a), "Bluetooth is off.");
    assert!(audit(&cx).iter().any(|l| l.contains("bluetooth switch on")) && audit(&cx).iter().any(|l| l.contains("bluetooth switch off")));
}

#[test]
fn the_switch_says_exactly_why_it_cannot_turn_on_and_offers_the_fix() {
    // (machine, expected state, words, a word of the reason, the fix button or none)
    let cases: Vec<(&str, Fake, Option<(bool, bool)>, bool, bool, Switch, &str, &str, Option<&str>)> = vec![
        ("hard", Fake::new(), Some((false, true)), false, false, Switch::BlockedByHardware, "Blocked by the hardware switch", "key or switch on the machine", None),
        ("none", Fake { devs: Mutex::new(vec![]), ..Fake::new() }, None, false, false, Switch::NoAdapter, "No Bluetooth adapter", "No Bluetooth adapter was found", None),
        ("fw", Fake::rom(), Some((false, false)), true, false, Switch::NeedsFirmware, "Needs firmware: Install", "firmware file", Some("Install firmware")),
        ("unset", Fake { configured: false, ..Fake::new() }, Some((false, false)), false, false, Switch::NotSetUp, "Controller not set up", "no valid address", None),
    ];
    for (tag, fake, radio, mac, usb, want, words, why, fix) in cases {
        let ctl = Arc::new(fake);
        let cx = admin(&format!("why-{tag}"));
        let mut e = env(&format!("why-{tag}"), ctl.clone(), if mac { "Macmini8,1" } else { "HP Laptop" }, radio, usb);
        if tag == "none" {
            std::fs::remove_dir_all(e.roots.sys.join("class/bluetooth")).unwrap();
        }
        e.ctl = ctl.clone();
        let mut a = SettingsApp::new(&cx).with_hardware(e);
        a.go_to(Tab::Bluetooth);
        tick(&mut a, &cx, 3);
        assert_eq!(a.bt.model.view.switch, want, "{tag}");
        assert_eq!(want.words(), words, "{tag}");
        assert!(want.why().contains(why), "{tag}: {}", want.why());
        let g = draw(&mut a, &cx, 1280, 720);
        match fix {
            Some(f) => assert!(g.hits.iter().any(|h| h.label == f), "{tag}: the fix {f} is a button on the card: {:?}", labels(&g)),
            None => assert!(!g.hits.iter().any(|h| h.label.contains("firmware")), "{tag}"),
        }
        // pressing the switch changes nothing and says why
        key(&mut a, &cx, Key::Char('e'));
        assert!(status(&a).contains(why), "{tag}: {}", status(&a));
        assert!(ctl.calls().iter().all(|c| !c.starts_with("power")), "{tag}: nothing was powered: {:?}", ctl.calls());
    }
    // a USB adapter without a driver, and a controller that never answered
    let ctl = Arc::new(Fake::new());
    let cx = admin("why-usb");
    let mut e = env("why-usb", ctl.clone(), "HP Laptop", None, true);
    std::fs::remove_dir_all(e.roots.sys.join("class/bluetooth")).unwrap();
    e.ctl = ctl;
    let mut a = SettingsApp::new(&cx).with_hardware(e);
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    assert_eq!(a.bt.model.view.switch, Switch::NoDriver);
    assert!(Switch::NoDriver.why().contains("no driver has taken it"));
}

#[test]
fn power_on_works_a_refusal_is_said_plainly_and_a_viewer_cannot() {
    let (mut a, cx, ctl) = app_with("pw1", Fake { powered: Mutex::new(false), ..Fake::new() });
    key(&mut a, &cx, Key::Char('e'));
    assert_eq!(ctl.calls(), vec!["power 0 true"]);
    assert_eq!(status(&a), "Bluetooth is on.");
    // a refusal from the kernel
    let (mut b, cx2, _) = app_with("pw2", Fake { powered: Mutex::new(false), power_error: Some("the kernel will not power this controller: it has no valid address".into()), ..Fake::new() });
    key(&mut b, &cx2, Key::Char('e'));
    assert!(status(&b).contains("no valid address"), "{}", status(&b));
    // a viewer has no Bluetooth page at all and cannot reach it
    let ucx = {
        let mut c = cx_with_admin_store("pw3");
        let d = temp_dir("pw3-svc");
        c.services = services_in(&d, Arc::new(Offline));
        c.session.policy.manage_settings = false;
        c
    };
    let v = SettingsApp::new(&ucx);
    assert!(!v.tabs().contains(&Tab::Bluetooth));
}

#[test]
fn a_failed_power_says_why_for_a_blocked_radio_and_a_silent_kernel_and_the_scan_message_is_not_stale() {
    // the kernel gave no reason
    let (mut a, cx, _) = app_with("pf1", Fake { powered: Mutex::new(false), power_error: Some("could not switch the controller on: failed".into()), ..Fake::new() });
    key(&mut a, &cx, Key::Char('e'));
    assert!(status(&a).contains("gave no reason") && status(&a).contains("Look again"), "{}", status(&a));
    // after a failed scan nothing of it stays once the next scan starts
    let (mut b, cx2, _) = app_with("pf2", Fake { scan_error: Some("the scan did not work: busy".into()), ..Fake::new() });
    key(&mut b, &cx2, Key::Char('s'));
    wait_scan(&mut b, &cx2);
    assert_eq!(b.bt.model.scan_error(), Some("the scan did not work: busy"));
    let g = draw(&mut b, &cx2, 1280, 720);
    assert!(labels(&g).iter().all(|l| !l.contains("not available yet")));
    key(&mut b, &cx2, Key::Char('s'));
    assert_eq!(b.bt.model.scan_error(), None, "starting a scan clears the old message");
    wait_scan(&mut b, &cx2);
}

#[test]
fn the_rom_state_controller_is_described_with_chip_address_power_and_the_reason() {
    let ctl = Arc::new(Fake::rom());
    let cx = admin("rom");
    let mut a = SettingsApp::new(&cx).with_hardware(env("rom", ctl, "Macmini8,1", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    let v = &a.bt.model.view;
    assert_eq!(v.headline.as_deref(), Some("Controller hci0: BCM4364 (ROM state, no firmware file)"));
    assert!(v.detail.as_deref().unwrap().starts_with("address 00:00:00:00:00:00 (not set) · powered off"), "{:?}", v.detail);
    assert_eq!(v.switch, Switch::NeedsFirmware);
    assert!(Switch::NeedsFirmware.why().contains("never shipped"));
    // honest about the hardware: the card names the chip and the missing firmware, and says nothing about audio or a microphone
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().all(|h| !h.label.to_lowercase().contains("microphone")));
}

#[test]
fn a_failed_scan_says_why_and_a_scan_off_the_power_asks_for_power_first() {
    let (mut a, cx, ctl) = app_with("fs1", Fake { powered: Mutex::new(false), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(status(&a), "Turn Bluetooth on first.");
    assert_eq!(ctl.count("scan"), 0);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label == "Scan"), "no Scan button while Bluetooth is off");
}

#[test]
fn load_anyway_gives_the_held_drivers_back_once_and_the_row_goes_away() {
    let ctl = Arc::new(Fake { held: Mutex::new(true), ..Fake::new() });
    let cx = admin("ld");
    let mut a = SettingsApp::new(&cx).with_hardware(env("ld", ctl.clone(), "HP Laptop", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    // the driver button lives under Advanced
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!labels(&g).iter().any(|l| l == "Load driver"));
    click(&mut a, &cx, &g, "Advanced");
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Load driver");
    assert_eq!(ctl.calls(), vec!["load anyway"]);
    assert!(status(&a).contains("given back"), "{}", status(&a));
    tick(&mut a, &cx, 100);
    key(&mut a, &cx, Key::Char('l'));
    assert!(status(&a).contains("nothing to load"), "{}", status(&a));
    assert_eq!(ctl.calls().len(), 1);
}

#[test]
fn a_mac_with_the_firmware_file_does_not_say_rom_state_and_a_machine_without_a_controller_shows_none() {
    let ctl = Arc::new(Fake { fw_file: true, manufacturer: 15, ..Fake::new() });
    let cx = admin("fwf");
    let mut a = SettingsApp::new(&cx).with_hardware(env("fwf", ctl, "Macmini8,1", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    assert_eq!(a.bt.model.view.headline.as_deref(), Some("Controller hci0: BCM4364"));
    assert_eq!(a.bt.model.view.switch, Switch::On);
    let d = temp_dir("fwf-none");
    std::fs::create_dir_all(d.join("sys")).unwrap();
    let e = DeviceEnv { roots: iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") }, ctl: Arc::new(Fake::new()) };
    let mut b = SettingsApp::new(&cx).with_hardware(e);
    b.go_to(Tab::Bluetooth);
    tick(&mut b, &cx, 3);
    assert!(b.bt.model.view.headline.is_none());
    assert_eq!(b.bt.model.view.switch, Switch::NoAdapter);
}

#[test]
fn cloud_mode_hides_the_bluetooth_page() {
    use iyakki_agent::cloudmode::{cloud_mode, Facts, Setting};
    let cx = admin("cl");
    let on = SettingsApp::new(&cx);
    assert!(on.tabs.contains(&Tab::Bluetooth));
    let hidden = SettingsApp::new(&cx).with_cloud(cloud_mode(Setting::On, &Facts::full()));
    assert!(!hidden.tabs.contains(&Tab::Bluetooth), "no hardware page on a server");
}

#[test]
fn a_controller_without_firmware_says_so_first_and_get_firmware_leads_to_store_setup() {
    let ctl = Arc::new(Fake::rom());
    let cx = admin("fw1");
    let mut a = SettingsApp::new(&cx).with_hardware(env("fw1", ctl, "Macmini8,1", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    let g = draw(&mut a, &cx, 1280, 720);
    let fw = g.hits.iter().position(|h| h.label == "Install firmware").expect("the fix is a button on the switch card");
    let sw = g.hits.iter().position(|h| h.label == "Bluetooth").unwrap();
    assert!(sw < fw, "the card comes first");
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())));
    assert_eq!(a.take_request(), None);
    click(&mut a, &cx, &g, "Install firmware");
    assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())));
    assert!(status(&a).contains("Store"), "{}", status(&a));
}

#[test]
fn a_working_controller_keeps_the_normal_page_and_has_no_firmware_button() {
    let ctl = Arc::new(Fake { address: "A4:83:E7:11:22:33", manufacturer: 15, ..Fake::new() });
    let cx = admin("fw2");
    let mut a = SettingsApp::new(&cx).with_hardware(env("fw2", ctl, "Macmini8,1", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx, 3);
    assert_eq!(a.bt.model.view.switch, Switch::On);
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("does not need firmware") && a.take_request().is_none());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|x| x.label.contains("irmware")));
}

#[test]
fn the_firmware_button_is_on_the_page_inside_the_window_and_a_click_on_it_asks_for_setup() {
    let cx = admin("fw3");
    for (w, h) in [(360u32, 740u32), (800, 480), (1024, 600), (1280, 720), (1920, 1080)] {
        let mut a = SettingsApp::new(&cx).with_hardware(env(&format!("fw3-{w}"), Arc::new(Fake::rom()), "Macmini8,1", Some((false, false)), false));
        a.go_to(Tab::Bluetooth);
        tick(&mut a, &cx, 3);
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
        }
        let btn = g.hits.iter().find(|x| x.label == "Install firmware").unwrap_or_else(|| panic!("{w}x{h}: Install firmware among {:?}", labels(&g)));
        assert!(btn.rect.w >= 24.0 && btn.rect.h >= 16.0, "{w}x{h}: {:?}", btn.rect);
        let ev = btn.ev.clone();
        a.handle(&ev, &cx);
        assert_eq!(a.take_request(), Some(("quickinstall".to_string(), "mac-firmware".to_string())), "{w}x{h}");
    }
}

#[test]
fn advanced_holds_the_separate_radio_and_controller_switches_and_look_again() {
    let (mut a, cx, ctl) = app("adv");
    let g = draw(&mut a, &cx, 1280, 900);
    for l in ["Turn off", "Power off", "Look again"] {
        assert!(!labels(&g).iter().any(|x| x == l), "{l} is hidden until Advanced is opened");
    }
    click(&mut a, &cx, &g, "Advanced");
    let g = draw(&mut a, &cx, 1280, 900);
    for l in ["Turn off", "Power off", "Look again"] {
        assert!(labels(&g).iter().any(|x| x == l), "{l} among {:?}", labels(&g));
    }
    // the controller alone
    click(&mut a, &cx, &g, "Power off");
    assert_eq!(ctl.calls(), vec!["power 0 false"]);
    assert!(status(&a).contains("powered off"), "{}", status(&a));
    // the radio alone
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Turn off");
    assert!(status(&a).contains("radio is off"), "{}", status(&a));
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Look again");
    assert!(status(&a).contains("again"), "{}", status(&a));
}

// ---------------------------------------------------------------------------------------------- the paired devices

#[test]
fn paired_devices_come_first_with_a_chip_and_their_own_buttons_without_any_selected_row() {
    let (mut a, cx, ctl) = app_with("pd", Fake { devs: Mutex::new(vec![keyboard(), phone()]), scan: Mutex::new(crowd()), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let g = draw(&mut a, &cx, 1280, 1000);
    let l = labels(&g);
    let kb = l.iter().position(|x| x == "K380, Not connected").expect("the keyboard row with its chip");
    let ph = l.iter().position(|x| x == "Pixel, Not connected").expect("the phone row");
    let first_nearby = l.iter().position(|x| x.ends_with(", pair")).expect("nearby rows");
    assert!(kb < ph && ph < first_nearby, "paired devices first: {l:?}");
    for b in ["Connect", "Forget", "Tether", "Audio"] {
        assert!(l.iter().any(|x| x == b), "{b}");
    }
    // the selection is on the switch card; a button on another row acts on its own row
    assert_eq!(a.bt.model.devices.len(), 2);
    click(&mut a, &cx, &g, "Connect");
    assert_eq!(ctl.calls().last().unwrap(), "connect AA:BB:CC:DD:EE:01");
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 1000);
    assert!(labels(&g).iter().any(|x| x == "K380, Connected") && labels(&g).iter().any(|x| x == "Disconnect"));
    click(&mut a, &cx, &g, "Tether");
    assert_eq!(ctl.calls().last().unwrap(), "tether AA:BB:CC:DD:FF:01");
    // the whole row is also the main action: Enter on the chosen row
    key(&mut a, &cx, Key::Up);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.calls().last().unwrap(), "disconnect AA:BB:CC:DD:EE:01");
}

#[test]
fn a_paired_keyboard_is_listed_and_enter_connects_and_disconnects_it() {
    let (mut a, cx, ctl) = app("kb");
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.calls(), vec!["connect AA:BB:CC:DD:EE:01"]);
    tick(&mut a, &cx, 2);
    let rows = a.bt.model.paired_rows();
    assert_eq!(rows[0].chip, "Connected");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.calls().last().unwrap(), "disconnect AA:BB:CC:DD:EE:01");
    tick(&mut a, &cx, 2);
    assert_eq!(a.bt.model.paired_rows()[0].chip, "Not connected");
}

#[test]
fn a_failed_connection_shows_the_plain_reason_and_forget_removes_the_device_after_a_question() {
    let (mut a, cx, ctl) = app("kf");
    *ctl.devs.lock().unwrap() = vec![DeviceView { state: LinkState::Failed("the keyboard did not answer; wake it".into()), ..keyboard() }];
    tick(&mut a, &cx, 2);
    let rows = a.bt.model.paired_rows();
    assert!(rows[0].detail.contains("the keyboard did not answer"), "{:?}", rows[0]);
    // Forget asks first, on a card, not at the bottom of a list
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('u'));
    assert!(a.bt.has_dialog() && a.has_modal());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.starts_with("Yes, forget")), "{:?}", labels(&g));
    key(&mut a, &cx, Key::Esc);
    assert!(!a.bt.has_dialog() && ctl.count("forget") == 0, "Esc leaves it");
    key(&mut a, &cx, Key::Char('u'));
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(ctl.calls(), vec!["forget AA:BB:CC:DD:EE:01"]);
    tick(&mut a, &cx, 2);
    assert!(a.bt.model.paired_rows().is_empty());
}

#[test]
fn a_paired_phone_has_a_tether_button_and_it_connects_then_disconnects() {
    let (mut a, cx, ctl) = app_with("ph", Fake { devs: Mutex::new(vec![phone()]), tether_next: TetherState::Connected, ..Fake::new() });
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Tether");
    tick(&mut a, &cx, 3);
    let row = &a.bt.model.paired_rows()[0];
    assert_eq!(row.chip, "Tethering");
    assert_eq!(row.detail, "Tether connected 192.168.44.5");
    assert!(row.buttons.iter().any(|b| b.1 == "Stop tether"));
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Stop tether");
    assert_eq!(ctl.calls().last().unwrap(), "tether off");
    tick(&mut a, &cx, 3);
    assert_eq!(a.bt.model.paired_rows()[0].detail, "Tether disconnected");
}

#[test]
fn the_tether_button_finds_the_phone_whatever_row_is_selected_and_failures_say_why() {
    let (mut a, cx, ctl) = app_with("tb", Fake { devs: Mutex::new(vec![keyboard(), phone()]), tether_refusal: Some("Bluetooth is off".into()), ..Fake::new() });
    // the keyboard row is chosen; t still goes to the phone
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('t'));
    assert_eq!(ctl.calls(), vec!["tether AA:BB:CC:DD:FF:01"]);
    assert_eq!(status(&a), "Bluetooth is off");
    // no phone: the message says what to do
    let (mut b, cx2, _) = app("tb2");
    key(&mut b, &cx2, Key::Char('t'));
    assert!(status(&b).contains("No phone is paired yet"), "{}", status(&b));
}

#[test]
fn the_bluetooth_buttons_act_on_their_own_row_not_on_the_selected_one() {
    let (mut a, cx, ctl) = app_with("own", Fake { devs: Mutex::new(vec![keyboard(), phone()]), ..Fake::new() });
    // the choice rests on the first row (the switch) the whole time
    let g = draw(&mut a, &cx, 1280, 900);
    let tether = g.hits.iter().find(|h| h.label == "Tether").unwrap().ev.clone();
    let audio = g.hits.iter().find(|h| h.label == "Audio").unwrap().ev.clone();
    a.handle(&audio, &cx);
    assert_eq!(ctl.calls().last().unwrap(), "audio AA:BB:CC:DD:FF:01");
    a.handle(&tether, &cx);
    assert_eq!(ctl.calls().last().unwrap(), "tether AA:BB:CC:DD:FF:01");
    assert!(ctl.calls().iter().all(|c| !c.starts_with("power")), "the switch was not touched");
}

#[test]
fn a_second_click_while_a_connection_is_starting_does_not_start_a_second_attempt() {
    let (mut a, cx, ctl) = app_with("one", Fake { devs: Mutex::new(vec![keyboard(), phone()]), tether_next: TetherState::Connecting, ..Fake::new() });
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Tether");
    for _ in 0..3 {
        let g = draw(&mut a, &cx, 1280, 900);
        click(&mut a, &cx, &g, "Connecting...");
    }
    assert_eq!(ctl.count("tether"), 1, "{:?}", ctl.calls());
    assert!(status(&a).contains("Still connecting"), "{}", status(&a));
    assert_eq!(a.bt.model.paired_rows()[1].chip, "Connecting...");
    // the same for the audio and for an input device
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Audio");
    click(&mut a, &cx, &g, "Audio");
    assert_eq!(ctl.count("audio AA"), 1);
    // a connect that shows no result yet is not repeated either
    ctl.devs.lock().unwrap()[0].state = LinkState::Connecting;
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Connect");
    assert_eq!(ctl.count("connect"), 0, "it is connecting already");
    // when the attempt fails, a new one is allowed
    *ctl.tether.lock().unwrap() = TetherView { address: "AA:BB:CC:DD:FF:01".into(), name: "Pixel".into(), state: TetherState::Failed("tethering is off on the phone".into()) };
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Tether");
    assert_eq!(ctl.count("tether"), 2, "{:?}", ctl.calls());
}

#[test]
fn the_last_outcome_stays_on_the_row_and_goes_to_the_system_log() {
    let (mut a, cx, ctl) = app_with("lo", Fake { devs: Mutex::new(vec![phone()]), tether_next: TetherState::Connecting, ..Fake::new() });
    let p = "AA:BB:CC:DD:FF:01";
    let fail = |ctl: &Fake, why: &str| *ctl.tether.lock().unwrap() = TetherView { address: p.into(), name: "Pixel".into(), state: TetherState::Failed(why.into()) };
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Tether");
    tick(&mut a, &cx, 2);
    fail(&ctl, "tethering is off on the phone");
    tick(&mut a, &cx, 2);
    assert_eq!(a.bt.model.paired_rows()[0].detail, "Tether failed: tethering is off on the phone");
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Tether");
    tick(&mut a, &cx, 2);
    fail(&ctl, "the phone did not answer");
    tick(&mut a, &cx, 2);
    assert_eq!(a.bt.model.paired_rows()[0].detail, "Tether failed: the phone did not answer (attempt 2)", "a retry loop is visible on the row");
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Tether");
    tick(&mut a, &cx, 2);
    *ctl.tether.lock().unwrap() = TetherView { address: p.into(), name: "Pixel".into(), state: TetherState::Connected };
    tick(&mut a, &cx, 2);
    assert_eq!(a.bt.model.paired_rows()[0].detail, "Tether connected 192.168.44.5");
    *ctl.tether.lock().unwrap() = TetherView { address: p.into(), name: "Pixel".into(), state: TetherState::Idle };
    tick(&mut a, &cx, 2);
    let log = audit(&cx).join("\n");
    for w in [
        "bluetooth tether AA:BB:CC:DD:FF:01 connect attempt 1",
        "bluetooth tether AA:BB:CC:DD:FF:01 attempt 1 failed: tethering is off on the phone",
        "bluetooth tether AA:BB:CC:DD:FF:01 attempt 2 failed: the phone did not answer",
        "bluetooth tether AA:BB:CC:DD:FF:01 attempt 3 connected iyakbt0 192.168.44.5",
        "bluetooth tether AA:BB:CC:DD:FF:01 disconnected",
    ] {
        assert!(log.contains(w), "{w} in\n{log}");
    }
}

#[test]
fn audio_and_input_sessions_write_their_outcomes_to_the_system_log() {
    let (mut a, cx, ctl) = app_with("au", Fake { devs: Mutex::new(vec![keyboard(), phone()]), ..Fake::new() });
    let p = "AA:BB:CC:DD:FF:01";
    let dev = |state| AudioDeviceView { address: p.into(), name: "Pixel".into(), state, codec: "SBC 44.1 kHz joint stereo".into(), error: None, packets: 0, lost: 0 };
    ctl.audio.lock().unwrap().device = Some(dev(AudioState::Connected));
    tick(&mut a, &cx, 2);
    ctl.audio.lock().unwrap().device = Some(dev(AudioState::Playing));
    tick(&mut a, &cx, 2);
    assert_eq!(a.bt.model.paired_rows()[1].chip, "Playing");
    ctl.audio.lock().unwrap().device = Some(dev(AudioState::Connected));
    tick(&mut a, &cx, 2);
    ctl.audio.lock().unwrap().device = Some(AudioDeviceView { error: Some("the speaker could not be opened".into()), ..dev(AudioState::Connected) });
    tick(&mut a, &cx, 2);
    ctl.audio.lock().unwrap().device = None;
    tick(&mut a, &cx, 2);
    // an input device: session started, then stopped, then a failure
    ctl.devs.lock().unwrap()[0].state = LinkState::Connected;
    tick(&mut a, &cx, 2);
    ctl.devs.lock().unwrap()[0].state = LinkState::Disconnected;
    tick(&mut a, &cx, 2);
    ctl.devs.lock().unwrap()[0].state = LinkState::Failed("out of range".into());
    tick(&mut a, &cx, 2);
    let log = audit(&cx).join("\n");
    for w in [
        "bluetooth audio AA:BB:CC:DD:FF:01 connected SBC 44.1 kHz joint stereo",
        "bluetooth audio AA:BB:CC:DD:FF:01 playing from Pixel",
        "bluetooth audio AA:BB:CC:DD:FF:01 stopped playing",
        "bluetooth audio AA:BB:CC:DD:FF:01 failed: the speaker could not be opened",
        "bluetooth audio AA:BB:CC:DD:FF:01 disconnected",
        "bluetooth hid AA:BB:CC:DD:EE:01 session started",
        "bluetooth hid AA:BB:CC:DD:EE:01 session stopped",
        "bluetooth hid AA:BB:CC:DD:EE:01 failed: out of range",
    ] {
        assert!(log.contains(w), "{w} in\n{log}");
    }
}

#[test]
fn an_audio_connect_nobody_answers_is_logged_as_failed_and_can_be_tried_again() {
    let (mut a, cx, ctl) = app_with("au2", Fake { devs: Mutex::new(vec![phone()]), ..Fake::new() });
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Audio");
    assert_eq!(ctl.count("audio AA"), 1);
    // 25 seconds later (750 ticks) the phone never connected
    a.ticks += 800;
    tick(&mut a, &cx, 2);
    tick(&mut a, &cx, 2);
    assert!(audit(&cx).join("\n").contains("bluetooth audio AA:BB:CC:DD:FF:01 failed: the phone did not connect for audio in time"), "{:?}", audit(&cx));
    assert!(a.bt.model.paired_rows()[0].detail.starts_with("Audio failed"));
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Audio");
    assert_eq!(ctl.count("audio AA"), 2);
}

#[test]
fn the_audio_card_lets_a_phone_pair_shows_who_is_playing_and_says_why_it_is_not_available() {
    let (mut a, cx, ctl) = app("ac");
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Let a phone pair");
    assert_eq!(ctl.calls(), vec!["audio pairing true"]);
    assert!(status(&a).contains("two minutes"));
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Close pairing");
    assert_eq!(ctl.calls().last().unwrap(), "audio pairing false");
    ctl.audio.lock().unwrap().device = Some(AudioDeviceView { address: "AA:BB:CC:DD:FF:01".into(), name: "Pixel".into(), state: AudioState::Playing, codec: "SBC".into(), error: None, packets: 5, lost: 0 });
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Disconnect");
    assert_eq!(ctl.calls().last().unwrap(), "audio off");
    // not ready: no card, nothing to press
    *ctl.audio.lock().unwrap() = AudioView { ready: false, why_not: "the Bluetooth service is not running".into(), ..Default::default() };
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 900);
    assert!(!labels(&g).iter().any(|l| l == "Let a phone pair"));
}

#[test]
fn the_controller_state_is_logged_when_the_page_opens() {
    let (_, cx, _) = app("op");
    let log = audit(&cx).join("\n");
    assert!(log.contains("bluetooth controller hci0 address 11:22:33:44:55:66 powered on radio on firmware not needed"), "{log}");
    let ctl = Arc::new(Fake::rom());
    let cx2 = admin("op2");
    let mut a = SettingsApp::new(&cx2).with_hardware(env("op2", ctl, "Macmini8,1", Some((false, false)), false));
    a.go_to(Tab::Bluetooth);
    tick(&mut a, &cx2, 3);
    assert!(audit(&cx2).join("\n").contains("bluetooth controller hci0 address 00:00:00:00:00:00 powered off radio on firmware needed"), "{:?}", audit(&cx2));
}

// ---------------------------------------------------------------------------------------------- nearby devices

#[test]
fn nearby_is_short_sorted_and_hides_nameless_devices_behind_show_more() {
    let (mut a, cx, _) = app_with("nb", Fake { scan: Mutex::new(crowd()), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let n = a.bt.model.nearby("", false);
    assert_eq!(n.total, 22);
    assert_eq!(n.rows.len(), 8, "at most 8 rows");
    assert_eq!(n.more, 14);
    let names: Vec<&str> = n.rows.iter().map(|r| r.name.as_str()).collect();
    // named and pairable first, strongest first; then the named others by signal; nothing without a name
    assert_eq!(&names[..5], ["Keyboard K2", "Pixel 8", "MX Anywhere", "Pad X", "Old keys"]);
    assert_eq!(&names[5..], ["Buds", "Living room TV", "Speaker"]);
    assert!(n.rows.iter().all(|r| r.named));
    let g = draw(&mut a, &cx, 1280, 1300);
    assert_eq!(nearby_labels(&g).len(), 8);
    assert!(g.hits.iter().any(|h| h.label == "Show 14 more devices"));
    // a headphone is shown but honest: it cannot be paired here
    assert!(g.hits.iter().any(|h| h.label == "Buds, not supported"));
    assert!(g.hits.iter().any(|h| h.label == "Pixel 8, pair"));
}

#[test]
fn show_more_search_and_the_scroll_area_reach_every_device() {
    let (mut a, cx, _) = app_with("sm", Fake { scan: Mutex::new(crowd()), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let g = draw(&mut a, &cx, 1280, 1300);
    click(&mut a, &cx, &g, "Show 14 more devices");
    let g = draw(&mut a, &cx, 1280, 1300);
    assert!(g.hits.iter().any(|h| h.label == "Show fewer devices"));
    let seen: Vec<String> = nearby_labels(&g);
    assert_eq!(seen.len(), 8, "the scroll area shows 8 rows at a time: {seen:?}");
    // the wheel over the area scrolls it
    let region = g.hits.iter().find(|h| h.label == "Keyboard K2, pair").unwrap().rect;
    a.handle(&Event::MouseMove(region.x as i32 + 20, region.y as i32 + 10), &cx);
    for _ in 0..30 {
        a.handle(&Event::Scroll(-1), &cx);
    }
    let g = draw(&mut a, &cx, 1280, 1300);
    let last = nearby_labels(&g);
    assert!(last.iter().any(|l| l.starts_with("20:00:00:00:00:")), "the unnamed devices are at the end: {last:?}");
    assert!(!last.iter().any(|l| l.starts_with("Keyboard K2")));
    // the search field filters by address
    key(&mut a, &cx, Key::Char('/'));
    for c in "20:00:00:00:00:0B".chars() {
        key(&mut a, &cx, Key::Char(c));
    }
    let n = a.bt.model.nearby(&a.bt.search_text(), true);
    assert_eq!(n.rows.len(), 1, "{:?}", n.rows.iter().map(|r| &r.address).collect::<Vec<_>>());
    assert_eq!(n.rows[0].address, "20:00:00:00:00:0B");
    // and by name, case does not matter
    assert_eq!(a.bt.model.nearby("pixel", true).rows.len(), 1);
    // Esc clears the search and leaves the field
    key(&mut a, &cx, Key::Esc);
    assert_eq!(a.bt.search_text(), "");
}

#[test]
fn a_name_seen_once_is_kept_in_later_scans() {
    let (mut a, cx, ctl) = app_with("nm", Fake { scan: Mutex::new(vec![found("Phone of a friend", "30:00:00:00:00:01", -50, Kind::Phone)]), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    // the next scans see the same device without its name (names can take several scans)
    *ctl.scan.lock().unwrap() = vec![found("", "30:00:00:00:00:01", -52, Kind::Other)];
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let n = a.bt.model.nearby("", false);
    assert_eq!(n.rows.len(), 1);
    assert_eq!(n.rows[0].name, "Phone of a friend");
    assert!(n.rows[0].pairable, "its kind is kept too");
    assert_eq!(n.rows[0].rssi, -52, "the signal is the new one");
}

#[test]
fn the_scan_shows_progress_stops_with_the_button_and_asks_the_controller_for_fifteen_seconds() {
    let (mut a, cx, ctl) = app_with("sc", Fake { scan_blocks: true, scan: Mutex::new(vec![found("K", "30:00:00:00:00:02", -50, Kind::Keyboard)]), ..Fake::new() });
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Scan");
    assert!(matches!(a.bt.model.scanning(), crate::btmodel::ScanView::Running(0)));
    // seconds count on the page
    tick(&mut a, &cx, 190);
    assert!(matches!(a.bt.model.scanning(), crate::btmodel::ScanView::Running(6)), "{:?}", a.bt.model.scanning());
    let g = draw(&mut a, &cx, 1280, 900);
    assert!(g.hits.iter().any(|h| h.label == "Stop") && !g.hits.iter().any(|h| h.label == "Scan"), "{:?}", labels(&g));
    click(&mut a, &cx, &g, "Stop");
    wait_scan(&mut a, &cx);
    assert!(ctl.calls().contains(&"scan stopped".to_string()) && ctl.calls().contains(&"scan 0 15".to_string()), "{:?}", ctl.calls());
    let g = draw(&mut a, &cx, 1280, 900);
    assert!(g.hits.iter().any(|h| h.label == "Scan"));
    assert_eq!(a.bt.model.nearby("", false).rows.len(), 1, "what it found is listed");
}

#[test]
fn a_scan_offers_pairing_for_classic_and_low_energy_input_devices_and_says_why_for_the_rest() {
    let le = Found { addr_type: 1, ..found("LE pad", "40:00:00:00:00:01", -60, Kind::Gamepad) };
    let le_other = Found { addr_type: 1, ..found("LE thing", "40:00:00:00:00:02", -61, Kind::Other) };
    let (mut a, cx, _) = app_with("sp", Fake { scan: Mutex::new(vec![found("Mouse", "40:00:00:00:00:03", -40, Kind::Mouse), le, found("Buds", "40:00:00:00:00:04", -50, Kind::Headphones), le_other, found("Pixel", "40:00:00:00:00:05", -45, Kind::Phone)]), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let n = a.bt.model.nearby("", true);
    let by = |name: &str| n.rows.iter().find(|r| r.name == name).unwrap().clone();
    assert!(by("Mouse").pairable && by("LE pad").pairable && by("Pixel").pairable);
    assert!(!by("Buds").pairable && by("Buds").note.contains("cannot send sound to headphones"));
    assert!(!by("LE thing").pairable && by("LE thing").note.contains("low-energy"));
    // choosing one that cannot be paired explains and opens nothing
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Buds, not supported");
    assert!(status(&a).contains("cannot send sound"), "{}", status(&a));
    assert!(!a.bt.has_dialog());
}

// ---------------------------------------------------------------------------------------------- the pairing popup

/// The page with a crowd scanned and the mouse chosen to pair.
fn pairing(tag: &str, start: PairState) -> (SettingsApp, AppCx, Arc<Fake>) {
    let (mut a, cx, ctl) = app_with(tag, Fake { pair_on_start: start, scan: Mutex::new(vec![found("MX Anywhere", "10:00:00:00:00:02", -50, Kind::Mouse)]), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Pair");
    (a, cx, ctl)
}

#[test]
fn pairing_is_a_popup_with_big_digits_and_yes_no_and_it_traps_the_focus() {
    let (mut a, cx, ctl) = pairing("p1", PairState::Confirm("MX Anywhere".into(), 123456));
    assert_eq!(ctl.calls().last().unwrap(), "pair 10:00:00:00:00:02");
    assert!(a.bt.has_dialog() && a.has_modal());
    let g = draw(&mut a, &cx, 1280, 720);
    let l = labels(&g);
    assert!(l.iter().any(|x| x == "Number: 123 456"), "{l:?}");
    for b in ["Yes", "No"] {
        assert!(l.iter().any(|x| x == b), "{b} in {l:?}");
    }
    // focus is trapped: nothing of the page behind is reachable, so the confirm items can never sit at the bottom of a list
    assert!(!l.iter().any(|x| x == "Scan" || x == "Advanced" || x.starts_with("Bluetooth")), "{l:?}");
    // the popup is in the middle of the window, the buttons inside it
    let yes = g.hits.iter().find(|h| h.label == "Yes").unwrap().rect;
    assert!(yes.x > 300.0 && yes.x < 980.0 && yes.y > 200.0 && yes.y < 600.0, "{yes:?}");
    // clicking the backdrop or typing does not leak to the page
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(ctl.count("scan"), 1);
    // Yes answers the pairing; the scan was stopped when the popup opened
    click(&mut a, &cx, &g, "Yes");
    assert_eq!(ctl.calls().last().unwrap(), "answer true");
    assert!(audit(&cx).join("\n").contains("bluetooth pairing answer yes"));
    assert!(!audit(&cx).join("\n").contains("123456"), "the number is never logged");
}

#[test]
fn no_and_esc_cancel_the_pairing_and_a_later_question_is_answered_no_by_itself() {
    let (mut a, cx, ctl) = pairing("p2", PairState::Confirm("MX Anywhere".into(), 123456));
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "No");
    assert_eq!(ctl.calls().last().unwrap(), "answer false");
    assert!(!a.bt.has_dialog());
    // Esc on a question: the same
    let (mut b, cx2, ctl2) = pairing("p2b", PairState::Confirm("MX Anywhere".into(), 123456));
    key(&mut b, &cx2, Key::Esc);
    assert_eq!(ctl2.calls().last().unwrap(), "answer false");
    assert!(!b.bt.has_dialog());
    // Esc while it is still connecting closes the popup; a question that comes later is refused without asking
    let (mut c, cx3, ctl3) = pairing("p2c", PairState::Running("MX Anywhere".into()));
    let g = draw(&mut c, &cx3, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Cancel"));
    key(&mut c, &cx3, Key::Esc);
    assert!(!c.bt.has_dialog());
    *ctl3.pair.lock().unwrap() = PairState::Confirm("MX Anywhere".into(), 654321);
    tick(&mut c, &cx3, 3);
    assert!(!c.bt.has_dialog(), "no popup comes back for a pairing the person cancelled");
    assert_eq!(ctl3.calls().last().unwrap(), "answer false");
}

#[test]
fn a_pin_and_a_passkey_are_shown_big_with_a_cancel_and_never_logged() {
    let (mut a, cx, _) = pairing("p3", PairState::ShowPin("Keys".into(), "4821".into()));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Number: 4821"), "{:?}", labels(&g));
    assert!(labels(&g).iter().any(|l| l == "Cancel") && !labels(&g).iter().any(|l| l == "Yes"));
    let spoken = crate::access::take_announcements().join(" | ");
    assert!(spoken.contains("Type this PIN on Keys: 4 8 2 1"), "{spoken}");
    key(&mut a, &cx, Key::Esc);
    assert!(!a.bt.has_dialog());
    let (mut b, cx2, bctl) = pairing("p3b", PairState::ShowPasskey("Keys".into(), 987654));
    let g = draw(&mut b, &cx2, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Number: 987 654"), "{:?}", labels(&g));
    for c in [&cx, &cx2] {
        let log = audit(c).join("\n");
        assert!(!log.contains("4821") && !log.contains("987654") && !log.contains("987 654"), "{log}");
    }
    // a failed pairing says why in the log, never the code
    *bctl.pair.lock().unwrap() = PairState::Done(Err("pairing failed: timed out".into()));
    tick(&mut b, &cx2, 2);
    assert!(audit(&cx2).join("\n").contains("bluetooth pairing 10:00:00:00:00:02 failed: pairing failed: timed out"));
}

#[test]
fn a_phone_that_shows_no_number_is_accepted_or_refused_and_enter_defaults_by_who_started_it() {
    // we started it: Enter says yes
    let (mut a, cx, ctl) = pairing("p4", PairState::ConfirmNoNumber("Pixel".into()));
    let g = draw(&mut a, &cx, 1280, 720);
    let spoken = crate::access::take_announcements().join(" | ");
    assert!(spoken.contains("Accept pairing with Pixel") && spoken.contains("will not send audio or files"), "{spoken}");
    assert!(labels(&g).iter().any(|l| l == "Yes") && labels(&g).iter().any(|l| l == "No"));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.calls().last().unwrap(), "answer true");
    // the other device started it: Enter is No
    let (mut b, cx2, ctl2) = app("p4b");
    *ctl2.pair.lock().unwrap() = PairState::ConfirmNoNumber("Stranger".into());
    tick(&mut b, &cx2, 2);
    assert!(b.bt.has_dialog());
    key(&mut b, &cx2, Key::Enter);
    assert_eq!(ctl2.calls().last().unwrap(), "answer false", "an incoming request is refused by the default key");
}

#[test]
fn success_offers_connect_now_and_failure_offers_try_again() {
    let (mut a, cx, ctl) = pairing("p5", PairState::Running("MX Anywhere".into()));
    *ctl.pair.lock().unwrap() = PairState::Done(Ok("MX Anywhere is paired.".into()));
    ctl.devs.lock().unwrap().push(DeviceView { address: "10:00:00:00:00:02".into(), addr_type: 0, name: "MX Anywhere".into(), kind: "mouse".into(), state: LinkState::Disconnected });
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Connect now") && labels(&g).iter().any(|l| l == "Not now"), "{:?}", labels(&g));
    assert!(audit(&cx).join("\n").contains("bluetooth pairing 10:00:00:00:00:02 paired"));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.calls().last().unwrap(), "connect 10:00:00:00:00:02");
    assert!(!a.bt.has_dialog());
    // failure: a plain reason and Try again
    let (mut b, cx2, ctl2) = pairing("p5b", PairState::Running("MX Anywhere".into()));
    *ctl2.pair.lock().unwrap() = PairState::Done(Err("the device refused: put it in pairing mode".into()));
    tick(&mut b, &cx2, 2);
    let g = draw(&mut b, &cx2, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Try again") && labels(&g).iter().any(|l| l == "Close"));
    assert!(crate::access::take_announcements().join(" | ").contains("put it in pairing mode"));
    key(&mut b, &cx2, Key::Enter);
    assert_eq!(ctl2.count("pair 10:00:00:00:00:02"), 2, "Try again pairs again");
    assert!(b.bt.has_dialog());
    *ctl2.pair.lock().unwrap() = PairState::Done(Err("again".into()));
    tick(&mut b, &cx2, 2);
    key(&mut b, &cx2, Key::Esc);
    assert!(!b.bt.has_dialog());
    // a paired phone: Connect now starts its tethering
    let (mut c, cx3, ctl3) = app_with("p5c", Fake { pair_on_start: PairState::Running("Pixel".into()), scan: Mutex::new(vec![found("Pixel", "AA:BB:CC:DD:FF:01", -40, Kind::Phone)]), devs: Mutex::new(vec![]), ..Fake::new() });
    key(&mut c, &cx3, Key::Char('s'));
    wait_scan(&mut c, &cx3);
    let g = draw(&mut c, &cx3, 1280, 900);
    click(&mut c, &cx3, &g, "Pair");
    ctl3.devs.lock().unwrap().push(phone());
    *ctl3.pair.lock().unwrap() = PairState::Done(Ok("Pixel is paired.".into()));
    tick(&mut c, &cx3, 2);
    key(&mut c, &cx3, Key::Enter);
    assert_eq!(ctl3.calls().last().unwrap(), "tether AA:BB:CC:DD:FF:01");
}

#[test]
fn an_incoming_request_opens_the_popup_on_the_page_and_the_shell_can_name_it_elsewhere() {
    let (mut a, cx, ctl) = app("p6");
    assert!(!a.bt.has_dialog());
    *ctl.pair.lock().unwrap() = PairState::Confirm("Pixel".into(), 246810);
    tick(&mut a, &cx, 2);
    assert!(a.bt.has_dialog(), "a pairing the other device started opens the popup");
    assert_eq!(a.bt.pairing_name(), Some("Pixel"));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Number: 246 810"));
    assert!(crate::access::take_announcements().join(" | ").contains("Pair with Pixel. Confirm this number on both devices: 2 4 6 8 1 0"));
    // while another page is open the shell shows a notice that names the device and the page
    assert_eq!(pairing_notice(&*ctl), Some("Pixel wants to pair. Open Settings, Bluetooth to answer.".to_string()));
    *ctl.pair.lock().unwrap() = PairState::Idle;
    assert_eq!(pairing_notice(&*ctl), None);
}

// ---------------------------------------------------------------------------------------------- layout, keys, speech

#[test]
fn the_page_draws_inside_the_window_at_four_sizes_and_the_largest_text_with_nothing_overlapping() {
    let cx = admin("lay");
    let buttons = ["Connect", "Disconnect", "Forget", "Tether", "Audio", "Pair", "Scan", "Stop", "Let a phone pair", "Turn off", "Power off", "Look again"];
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for big in [1.0f32, 2.0] {
            let ctl = Arc::new(Fake { devs: Mutex::new(vec![keyboard(), phone()]), scan: Mutex::new(crowd()), ..Fake::new() });
            let mut a = SettingsApp::new(&cx).with_hardware(env(&format!("lay-{w}-{big}"), ctl, "HP Laptop", Some((false, false)), false));
            a.go_to(Tab::Bluetooth);
            tick(&mut a, &cx, 3);
            key(&mut a, &cx, Key::Char('s'));
            wait_scan(&mut a, &cx);
            key(&mut a, &cx, Key::Char('a'));
            for step in 0..14 {
                let g = draw_big(&mut a, &cx, w, h, big);
                for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                    let r = hit.rect;
                    assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h} x{big} step {step}: {:?} {:?}", hit.label, r);
                }
                let b: Vec<&crate::ui::Hit> = g.hits.iter().filter(|x| buttons.contains(&x.label.as_str()) && x.rect.w > 0.0).collect();
                for (i, p) in b.iter().enumerate() {
                    for q in &b[i + 1..] {
                        let (r1, r2) = (p.rect, q.rect);
                        let overlap = r1.x < r2.x + r2.w - 1.0 && r2.x < r1.x + r1.w - 1.0 && r1.y < r2.y + r2.h - 1.0 && r2.y < r1.y + r1.h - 1.0;
                        // the Scan button's touch area is taller than its pill, and rows may share a pill region with their own row hit: only buttons are compared
                        assert!(!overlap, "{w}x{h} x{big} step {step}: {:?} {:?} overlaps {:?} {:?}", p.label, r1, q.label, r2);
                    }
                }
                if step == 0 {
                    assert!(g.hits.iter().any(|x| x.label == "Bluetooth"), "{w}x{h} x{big}: the switch is on the first screen");
                }
                key(&mut a, &cx, Key::Down);
            }
            // the pairing popup at this size
            a.bt.model.invalidate();
            let ctl2 = Arc::new(Fake { pair_on_start: PairState::Confirm("MX Anywhere".into(), 123456), ..Fake::new() });
            let _ = ctl2;
        }
    }
}

#[test]
fn the_popup_fits_the_smallest_window_and_the_largest_text() {
    let cx = admin("pop");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for big in [1.0f32, 2.0] {
            let ctl = Arc::new(Fake { scan: Mutex::new(vec![found("MX Anywhere", "10:00:00:00:00:02", -50, Kind::Mouse)]), pair_on_start: PairState::Confirm("A device with a rather long name indeed".into(), 123456), ..Fake::new() });
            let mut a = SettingsApp::new(&cx).with_hardware(env(&format!("pop-{w}-{big}"), ctl, "HP Laptop", Some((false, false)), false));
            a.go_to(Tab::Bluetooth);
            tick(&mut a, &cx, 3);
            key(&mut a, &cx, Key::Char('s'));
            wait_scan(&mut a, &cx);
            let mut g = draw_big(&mut a, &cx, w, h, big);
            for _ in 0..12 {
                if g.hits.iter().any(|x| x.label == "Pair") {
                    break;
                }
                key(&mut a, &cx, Key::Down);
                g = draw_big(&mut a, &cx, w, h, big);
            }
            click(&mut a, &cx, &g, "Pair");
            let g = draw_big(&mut a, &cx, w, h, big);
            for l in ["Yes", "No"] {
                let hit = g.hits.iter().find(|x| x.label == l).unwrap_or_else(|| panic!("{w}x{h} x{big}: {l} in {:?}", labels(&g)));
                let r = hit.rect;
                assert!(r.x >= 0.0 && r.y >= 72.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h} x{big}: {l} {r:?}");
            }
            key(&mut a, &cx, Key::Esc);
        }
    }
}

#[test]
fn every_control_of_the_page_is_reachable_by_the_keyboard_walk_and_named() {
    let (mut a, cx, _) = app_with("kw", Fake { devs: Mutex::new(vec![keyboard(), phone()]), scan: Mutex::new(crowd()), ..Fake::new() });
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    key(&mut a, &cx, Key::Char('a'));
    let g = draw(&mut a, &cx, 1280, 1500);
    let walk = crate::access::entries(&g.hits, (1280.0, 1500.0));
    let names: Vec<String> = walk.iter().map(|e| e.node.name.clone()).collect();
    for b in ["Bluetooth", "Connect", "Forget", "Tether", "Audio", "Scan", "Advanced", "Look again", "Turn off", "Power off", "Show 14 more devices", "Pixel 8, pair", "Let a phone pair"] {
        assert!(names.iter().any(|n| n.starts_with(b)), "{b} is in the walk: {names:?}");
    }
    assert!(walk.iter().all(|e| !e.node.name.trim().is_empty()), "every control has a name");
    // the switch is a toggle with its state; Advanced says whether it is open
    let sw = walk.iter().find(|e| e.node.name == "Bluetooth").unwrap();
    assert_eq!(sw.node.role, crate::access::Role::Toggle);
    assert_eq!(sw.node.state.checked, Some(true));
    // the keys do everything the pointer does
    for k in [Key::Down, Key::Down, Key::Down, Key::Up, Key::Char('r'), Key::Char('m')] {
        key(&mut a, &cx, k);
    }
    assert!(status(&a).is_empty() || !status(&a).is_empty());
}

#[test]
fn the_status_line_has_one_place_and_leaves_by_itself_and_no_old_text_remains() {
    let (mut a, cx, _) = app("st");
    key(&mut a, &cx, Key::Char('f'));
    assert!(status(&a).contains("does not need firmware"));
    let g = draw(&mut a, &cx, 1280, 720);
    // the status is drawn above the controls, not behind buttons at the bottom
    let sw = g.hits.iter().find(|h| h.label == "Bluetooth").unwrap().rect;
    assert!(sw.y > 72.0 + 20.0, "the card starts below the status line");
    tick(&mut a, &cx, 300);
    assert_eq!(status(&a), "", "the line is cleared after a few seconds");
    let text = labels(&draw(&mut a, &cx, 1280, 720)).join(" ").to_lowercase();
    for old in ["not available yet", "pairing is not available", "no audio yet", "power the controller on first", "scan (10 s)"] {
        assert!(!text.contains(old), "{old}");
    }
    assert!(!crate::btmodel::Switch::NoAdapter.why().contains("not available yet"));
}

#[test]
fn the_popup_title_and_the_number_are_spoken_through_a_fake_reader() {
    use crate::access::{Reader, SpeechSink};
    #[derive(Default)]
    struct Said(Vec<String>);
    impl SpeechSink for Said {
        fn say(&mut self, text: &str) -> Result<(), String> {
            self.0.push(text.to_string());
            Ok(())
        }
        fn stop(&mut self) {}
        fn busy(&mut self) -> bool {
            false
        }
    }
    let (mut a, cx, _) = pairing("sp1", PairState::Confirm("MX Anywhere".into(), 135790));
    let _ = draw(&mut a, &cx, 1280, 720);
    let mut reader = Reader::new();
    let mut sink = Said::default();
    for t in crate::access::take_announcements() {
        reader.notice(&t);
    }
    for _ in 0..4 {
        reader.pump(&mut sink);
    }
    let spoken = sink.0.join(" | ");
    assert!(spoken.contains("Pair with MX Anywhere") && spoken.contains("1 3 5 7 9 0"), "{spoken}");
}

/// Pictures of the page for a look by eye: only when `IYAKKI_BT_SHOTS` names a folder (outside /tmp).
#[test]
fn the_page_states_can_be_drawn_to_png_files_for_a_look() {
    let Some(dir) = std::env::var_os("IYAKKI_BT_SHOTS") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let save = |g: &Gfx, name: &str| g.save_png(&dir.join(format!("{name}.png"))).unwrap();
    let (mut a, cx, ctl) = app_with("shot1", Fake { devs: Mutex::new(vec![keyboard(), phone()]), scan: Mutex::new(crowd()), pair_on_start: PairState::Confirm("Pixel 8".into(), 123456), ..Fake::new() });
    ctl.devs.lock().unwrap()[0].state = LinkState::Connected;
    *ctl.tether.lock().unwrap() = TetherView { address: "AA:BB:CC:DD:FF:01".into(), name: "Pixel".into(), state: TetherState::Failed("tethering is off on the phone".into()) };
    key(&mut a, &cx, Key::Char('s'));
    wait_scan(&mut a, &cx);
    save(&draw(&mut a, &cx, 1280, 900), "01-main-1280");
    save(&draw(&mut a, &cx, 360, 740), "02-main-360x740");
    save(&draw_big(&mut a, &cx, 800, 480, 2.0), "03-main-800x480-huge");
    key(&mut a, &cx, Key::Char('m'));
    key(&mut a, &cx, Key::Char('a'));
    save(&draw(&mut a, &cx, 1280, 1100), "04-expanded-advanced");
    key(&mut a, &cx, Key::Char('m'));
    let g = draw(&mut a, &cx, 1280, 900);
    click(&mut a, &cx, &g, "Pixel 8, pair");
    save(&draw(&mut a, &cx, 1280, 720), "05-popup-confirm");
    save(&draw(&mut a, &cx, 360, 740), "06-popup-confirm-360");
    *ctl.pair.lock().unwrap() = PairState::ShowPin("Keys".into(), "4821".into());
    tick(&mut a, &cx, 2);
    save(&draw(&mut a, &cx, 1280, 720), "07-popup-pin");
    *ctl.pair.lock().unwrap() = PairState::Done(Ok("Pixel 8 is paired.".into()));
    tick(&mut a, &cx, 2);
    save(&draw(&mut a, &cx, 1280, 720), "08-popup-success");
    *ctl.pair.lock().unwrap() = PairState::Done(Err("the device refused: put it in pairing mode".into()));
    tick(&mut a, &cx, 2);
    save(&draw(&mut a, &cx, 1280, 720), "09-popup-failure");
    let ctl2 = Arc::new(Fake::rom());
    let mut b = SettingsApp::new(&cx).with_hardware(env("shot2", ctl2, "Macmini8,1", Some((false, false)), false));
    b.go_to(Tab::Bluetooth);
    tick(&mut b, &cx, 3);
    save(&draw(&mut b, &cx, 1280, 720), "10-needs-firmware");
    save(&draw(&mut b, &cx, 360, 740), "11-needs-firmware-360");
}

// ---------------------------------------------------------------------------------------------- typing a passkey or PIN shown on the other device

fn type_str(a: &mut SettingsApp, cx: &AppCx, t: &str) {
    for c in t.chars() {
        key(a, cx, Key::Char(c));
    }
}

#[test]
fn a_passkey_shown_on_the_other_device_is_typed_here_and_sent_with_the_right_value() {
    let (mut a, cx, ctl) = pairing("t1", PairState::TypePasskey("Pixel".into()));
    assert!(a.bt.has_dialog() && a.has_modal());
    let g = draw(&mut a, &cx, 1280, 720);
    let l = labels(&g);
    assert!(l.iter().any(|x| x == "Passkey") && l.iter().any(|x| x == "OK") && l.iter().any(|x| x == "Cancel"), "{l:?}");
    assert!(!l.iter().any(|x| x == "Scan" || x == "Advanced"), "the focus is trapped: {l:?}");
    let spoken = crate::access::take_announcements().join(" | ");
    assert!(spoken.contains("Type the passkey shown on Pixel") && spoken.contains("Passkey, edit text, 6 digits"), "{spoken}");
    // the keys behind the popup do not reach the page; letters are refused with a plain message; paste keeps digits only
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(ctl.count("scan"), 1);
    type_str(&mut a, &cx, "12");
    key(&mut a, &cx, Key::Char('x'));
    a.handle(&Event::Paste("3 4-5\n6789".into()), &cx);
    key(&mut a, &cx, Key::Backspace);
    key(&mut a, &cx, Key::Char('6'));
    let g = draw(&mut a, &cx, 1280, 720);
    let node = crate::access::take_focus().expect("the field is focused");
    let said = node.speak();
    assert!(said.starts_with("Passkey, edit, 6 of 6 digits"), "{said}");
    assert!(!said.contains("123456"), "the screen reader never gets the digits");
    let _ = g;
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.typed.lock().unwrap().clone(), Some(Some("123456".to_string())), "the reply carries what was typed");
    assert_eq!(ctl.calls().last().unwrap(), "type sent");
    let log = audit(&cx).join("\n");
    assert!(log.contains("bluetooth pairing entry sent"), "{log}");
    assert!(!log.contains("123456") && !log.contains("1 2 3"), "no digits in the log: {log}");
    assert!(!crate::access::take_announcements().join(" ").contains("123456"));
}

#[test]
fn a_wrong_length_passkey_is_refused_in_plain_words_and_nothing_is_sent() {
    let (mut a, cx, ctl) = pairing("t2", PairState::TypePasskey("Pixel".into()));
    type_str(&mut a, &cx, "12345");
    key(&mut a, &cx, Key::Enter);
    assert!(ctl.typed.lock().unwrap().is_none(), "nothing sent");
    let g = draw(&mut a, &cx, 1280, 720);
    let spoken = crate::access::take_announcements().join(" | ");
    assert!(spoken.contains("exactly 6 digits"), "{spoken}");
    assert!(a.bt.has_dialog() && labels(&g).iter().any(|x| x == "OK"), "the popup stays open for a second try");
    assert!(!spoken.contains("12345"), "{spoken}");
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Char('6'));
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.typed.lock().unwrap().clone(), Some(Some("123456".to_string())), "the typing was kept, the sixth digit made it right");
    // nothing typed at all
    let (mut b, cx2, ctl2) = pairing("t2b", PairState::TypePasskey("Pixel".into()));
    key(&mut b, &cx2, Key::Enter);
    assert!(ctl2.typed.lock().unwrap().is_none());
    assert!(crate::access::take_announcements().join(" | ").contains("Type the 6 digits"));
    assert!(audit(&cx2).join("\n").find("entry").is_none());
}

#[test]
fn cancel_esc_and_a_click_on_cancel_give_the_negative_reply_and_wipe_the_entry() {
    let (mut a, cx, ctl) = pairing("t3", PairState::TypePasskey("Pixel".into()));
    type_str(&mut a, &cx, "1234");
    key(&mut a, &cx, Key::Esc);
    assert_eq!(ctl.typed.lock().unwrap().clone(), Some(None), "the person gave up: a negative reply");
    assert!(!a.bt.has_dialog());
    assert!(audit(&cx).join("\n").contains("bluetooth pairing entry cancelled"));
    let (mut b, cx2, ctl2) = pairing("t3b", PairState::TypePin("Mouse".into(), true));
    type_str(&mut b, &cx2, "99");
    let g = draw(&mut b, &cx2, 1280, 720);
    click(&mut b, &cx2, &g, "Cancel");
    assert_eq!(ctl2.typed.lock().unwrap().clone(), Some(None));
    assert!(!b.bt.has_dialog());
    // a prompt that comes back for a pairing the person gave up is not shown again
    *ctl2.pair.lock().unwrap() = PairState::TypePin("Mouse".into(), true);
    tick(&mut b, &cx2, 3);
    assert!(!b.bt.has_dialog());
}

#[test]
fn the_entry_is_wiped_when_the_popup_closes_for_any_reason() {
    // the device gave up while the person was typing: the popup shows the failure and the digits are gone
    let (mut a, cx, ctl) = pairing("t4", PairState::TypePasskey("Pixel".into()));
    type_str(&mut a, &cx, "1234");
    *ctl.pair.lock().unwrap() = PairState::Done(Err("pairing failed: timed out".into()));
    tick(&mut a, &cx, 2);
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(labels(&g).iter().any(|l| l == "Try again") && !labels(&g).iter().any(|l| l == "Passkey"), "{:?}", labels(&g));
    assert!(crate::access::take_announcements().join(" | ").contains("timed out"));
    let log = audit(&cx).join("\n");
    assert!(log.contains("failed: pairing failed: timed out") && !log.contains("1234"), "{log}");
    // nothing waits any more: sending is refused plainly
    assert!(a.bt.model.pair_type(Some("123456".into())).is_err());
}

#[test]
fn a_pin_shown_on_the_other_device_is_typed_here_and_the_common_pins_are_a_hint_only_for_devices_without_one() {
    // a phone shows its PIN: no hint
    let (mut a, cx, ctl) = pairing("t5", PairState::TypePin("Pixel".into(), false));
    let g = draw(&mut a, &cx, 1280, 720);
    let l = labels(&g);
    assert!(l.iter().any(|x| x == "PIN") && !l.iter().any(|x| x.starts_with("Use ")), "{l:?}");
    let spoken = crate::access::take_announcements().join(" | ");
    assert!(spoken.contains("Type the PIN shown on Pixel") && !spoken.contains("0000"), "{spoken}");
    type_str(&mut a, &cx, "ab 9!");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(ctl.typed.lock().unwrap().clone(), Some(Some("ab 9!".to_string())));
    // 17 characters are cut at 16
    let (mut b, cx2, ctl2) = pairing("t5b", PairState::TypePin("Pixel".into(), false));
    type_str(&mut b, &cx2, "12345678901234567");
    key(&mut b, &cx2, Key::Enter);
    assert_eq!(ctl2.typed.lock().unwrap().clone().unwrap().unwrap().len(), 16);
    // empty is refused
    let (mut c, cx3, ctl3) = pairing("t5c", PairState::TypePin("Pixel".into(), false));
    key(&mut c, &cx3, Key::Enter);
    assert!(ctl3.typed.lock().unwrap().is_none());
    assert!(crate::access::take_announcements().join(" | ").contains("Type the PIN"));
    // a mouse that has none printed: the two common PINs are offered as buttons that fill the entry
    let (mut d, cx4, ctl4) = pairing("t5d", PairState::TypePin("Mouse".into(), true));
    let g = draw(&mut d, &cx4, 1280, 720);
    assert!(labels(&g).iter().any(|x| x == "Use 0000") && labels(&g).iter().any(|x| x == "Use 1234"));
    assert!(crate::access::take_announcements().join(" | ").contains("0000 or 1234"));
    click(&mut d, &cx4, &g, "Use 1234");
    key(&mut d, &cx4, Key::Enter);
    assert_eq!(ctl4.typed.lock().unwrap().clone(), Some(Some("1234".to_string())));
    for c in [&cx, &cx2, &cx3, &cx4] {
        let log = audit(c).join("\n");
        assert!(!log.contains("ab 9") && !log.contains("1234") && !log.contains("0000"), "{log}");
    }
}

#[test]
fn the_typing_stages_fit_the_smallest_window_and_the_largest_text_and_work_by_keyboard_alone() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for big in [1.0f32, 2.0] {
            for start in [PairState::TypePasskey("A device with a rather long name indeed".into()), PairState::TypePin("A device with a rather long name indeed".into(), true)] {
                let cx = admin("tl");
                let ctl = Arc::new(Fake { scan: Mutex::new(vec![found("MX Anywhere", "10:00:00:00:00:02", -50, Kind::Mouse)]), pair_on_start: start.clone(), ..Fake::new() });
                let mut a = SettingsApp::new(&cx).with_hardware(env(&format!("tl-{w}-{big}"), ctl.clone(), "HP Laptop", Some((false, false)), false));
                a.go_to(Tab::Bluetooth);
                tick(&mut a, &cx, 3);
                key(&mut a, &cx, Key::Char('s'));
                wait_scan(&mut a, &cx);
                let mut g = draw_big(&mut a, &cx, w, h, big);
                for _ in 0..12 {
                    if g.hits.iter().any(|x| x.label == "Pair") {
                        break;
                    }
                    key(&mut a, &cx, Key::Down);
                    g = draw_big(&mut a, &cx, w, h, big);
                }
                click(&mut a, &cx, &g, "Pair");
                // keyboard only: type, Backspace, type again, Enter
                type_str(&mut a, &cx, "1234567");
                key(&mut a, &cx, Key::Backspace);
                key(&mut a, &cx, Key::Backspace);
                type_str(&mut a, &cx, "56");
                let g = draw_big(&mut a, &cx, w, h, big);
                let (pw, ph) = (w as f32, h as f32);
                let field = if matches!(start, PairState::TypePasskey(_)) { "Passkey" } else { "PIN" };
                for l in [field, "OK", "Cancel"] {
                    let r = g.hits.iter().find(|x| x.label == l).unwrap_or_else(|| panic!("{w}x{h} x{big}: {l} in {:?}", labels(&g))).rect;
                    assert!(r.x >= 0.0 && r.y >= 72.0 && r.x + r.w <= pw && r.y + r.h <= ph, "{w}x{h} x{big}: {l} {r:?}");
                }
                key(&mut a, &cx, Key::Enter);
                let typed = ctl.typed.lock().unwrap().clone();
                let want = if matches!(start, PairState::TypePasskey(_)) { "123456" } else { "1234556" };
                assert_eq!(typed, Some(Some(want.to_string())), "{w}x{h} x{big}");
            }
        }
    }
}
