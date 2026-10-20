//! Settings → Network → Private MAC address: the rows, choosing a mode, applying now (with a stand-in for the driver, never a
//! real adapter), the question when the adapter carries the session in use, a driver that refuses, the window on another
//! computer, and geometry at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_devctl::mac::{MacEnv, MacMode, MacStore};
use iyakki_devctl::Control;
use std::path::PathBuf;
use std::sync::Mutex;

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}
fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    while a.tab() != t {
        key(a, cx, Key::Right);
    }
}
fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..2000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}
fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}
fn click(a: &mut SettingsApp, cx: &AppCx, g: &Gfx, label: &str) {
    let h = g.hits.iter().find(|h| h.label == label).or_else(|| g.hits.iter().find(|h| h.label.contains(label))).unwrap_or_else(|| panic!("no `{label}` among {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>()));
    let ev = h.ev.clone();
    a.handle(&ev, cx);
}

/// An Ethernet card and a WiFi adapter under a temp folder, and a driver stand-in that records what it is asked.
struct Nic {
    sys: PathBuf,
    calls: Mutex<Vec<String>>,
    refuse: bool,
}

fn put(p: PathBuf, c: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, c).unwrap();
}

impl Control for Nic {
    fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("link {n} {}", if up { "up" } else { "down" }));
        put(self.sys.join("class/net").join(n).join("flags"), if up { "0x1003\n" } else { "0x1002\n" });
        Ok(())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![("eno1".into(), "192.168.1.100".into())]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        Err("no".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("no".into())
    }
    fn dhcp(&self, n: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("dhcp {n}"));
        Ok(())
    }
    fn permanent_mac(&self, n: &str) -> Option<String> {
        Some(if n == "eno1" { "00:11:22:33:44:55" } else { "aa:bb:cc:dd:ee:ff" }.into())
    }
    fn set_mac(&self, n: &str, mac: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("mac {n} {mac}"));
        if self.refuse {
            return Err("eno1: not supported by this driver (Operation not supported)".into());
        }
        assert_eq!(std::fs::read_to_string(self.sys.join("class/net").join(n).join("flags")).unwrap().trim(), "0x1002", "only with the link down");
        put(self.sys.join("class/net").join(n).join("address"), &format!("{mac}\n"));
        put(self.sys.join("class/net").join(n).join("addr_assign_type"), "3\n");
        Ok(())
    }
}

struct World {
    env: DeviceEnv,
    mac: MacEnv,
    nic: Arc<Nic>,
    dir: PathBuf,
    wifi: WifiEnv,
    /// What the stand-in programs (`ip`, `iw`, `wpa_cli`, `wpa_supplicant`, `udhcpc`) were asked.
    rec: PathBuf,
}

fn world(tag: &str, refuse: bool, ssh: bool) -> World {
    let d = temp_dir(&format!("{tag}-mac"));
    for (n, wifi, a) in [("eno1", false, "00:11:22:33:44:55"), ("wlan0", true, "aa:bb:cc:dd:ee:ff")] {
        let b = d.join("sys/class/net").join(n);
        put(b.join("type"), "1\n");
        put(b.join("flags"), "0x1003\n");
        put(b.join("carrier"), "1\n");
        put(b.join("address"), &format!("{a}\n"));
        put(b.join("addr_assign_type"), "0\n");
        put(b.join("device/x"), "");
        if wifi {
            put(b.join("wireless/x"), "");
        }
    }
    std::fs::create_dir_all(d.join("etc")).unwrap();
    let nic = Arc::new(Nic { sys: d.join("sys"), calls: Mutex::new(vec![]), refuse });
    let roots = iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") };
    let mac = MacEnv {
        store: MacStore { primary: d.join("etc/net.conf"), fallback: d.join("var/net-mac.conf") },
        secret_path: d.join("var/mac-secret"),
        ctl: nic.clone(),
        roots: roots.clone(),
        ssh_connection: ssh.then(|| "192.168.1.5 51000 192.168.1.100 22".to_string()),
        host_managed: false,
        known_wifi: d.join("var/wifi.json"),
    };
    // stand-ins for the programs that scan, join and ask for an address: nothing real is ever run
    use std::os::unix::fs::PermissionsExt;
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let rec = d.join("rec.log");
    let script = |name: &str, body: &str| {
        let p = bin.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", rec.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("ip", "exit 0");
    script("iw", "printf 'Connected to aa:bb:cc:00:00:01 (on wlan0)\\n\\tSSID: Home Net\\n\\tfreq: 5180\\n'");
    script("wpa_cli", "exit 1");
    script("wpa_supplicant", "exit 0");
    script("udhcpc", "exit 0");
    let wifi = WifiEnv { wifi: Arc::new(iyakki_devctl::wifi::Wifi { run_dir: d.join("run"), path: Some(bin) }), known: Arc::new(iyakki_devctl::wifi::KnownStore::new(&d.join("var/wifi.json"))), roots: roots.clone() };
    World { env: DeviceEnv { roots, ctl: nic.clone() }, mac, nic, dir: d, wifi, rec }
}

fn app(cx: &AppCx, w: &World) -> SettingsApp {
    let mut a = SettingsApp::new(cx).with_hardware(w.env.clone()).with_mac(w.mac.clone()).with_wifi(w.wifi.clone());
    goto(&mut a, cx, Tab::Network);
    a
}

fn titles(a: &mut SettingsApp) -> Vec<(String, String)> {
    a.dev_rows().into_iter().map(|r| (r.title, r.sub)).collect()
}

#[test]
fn the_network_page_shows_each_adapters_mode_hardware_address_and_the_one_in_use() {
    let cx = admin("mac1");
    let w = world("mac1", false, false);
    let mut a = app(&cx, &w);
    let rows = titles(&mut a);
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(names, vec!["Ethernet · eno1", "WiFi · wlan0", "Private MAC · eno1", "Private MAC · wlan0"], "{rows:?}");
    assert!(rows[2].1.starts_with("off · hardware 00:11:22:33:44:55 · now 00:11:22:33:44:55"), "{}", rows[2].1);
    assert!(rows[3].1.starts_with("per-network · hardware aa:bb:cc:dd:ee:ff"), "WiFi is private per network by default: {}", rows[3].1);
    let chips: Vec<String> = a.dev_rows()[2].chips.iter().map(|c| c.0.clone()).collect();
    assert_eq!(chips, vec!["MAC OFF", "hardware"]);
    // an address that is not the hardware's shows as private and locally administered
    put(w.dir.join("sys/class/net/wlan0/address"), "02:01:02:03:04:05\n");
    put(w.dir.join("sys/class/net/wlan0/addr_assign_type"), "3\n");
    a.hw.invalidate();
    let r = a.dev_rows();
    assert!(r[3].sub.contains("now 02:01:02:03:04:05 · locally administered") && r[3].chips.iter().any(|c| c.0 == "private"), "{:?}", r[3]);
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn an_administrator_chooses_a_mode_with_the_button_and_the_list_and_it_is_saved_audited_and_listed() {
    let cx = admin("mac2");
    let w = world("mac2", false, false);
    let mut a = app(&cx, &w);
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Private MAC");
    assert!(a.has_modal(), "the list of modes is shown");
    let g = draw(&mut a, &cx, 1280, 720);
    for m in MacMode::ALL {
        assert!(g.hits.iter().any(|h| h.label.contains(m.label())), "{} is offered", m.label());
    }
    click(&mut a, &cx, &g, "Random each connection");
    assert!(!a.has_modal());
    assert_eq!(w.mac.config().ethernet, MacMode::Random, "the first row is Ethernet: its kind is changed");
    assert_eq!(w.mac.config().wifi, MacMode::PerNetwork, "the other kind is untouched");
    assert!(status(&a).contains("Random") || status(&a).contains("random"), "{}", status(&a));
    assert!(status(&a).contains("next connection"), "{}", status(&a));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.network.mac") && l.contains("ethernet random")), "{:?}", cx.audit.recent(10));
    assert!(a.dev_rows()[2].sub.starts_with("random"), "the row shows it at once");
    assert!(std::fs::read_to_string(w.dir.join("etc/net.conf")).unwrap().contains("mac.ethernet = random"));
    assert!(w.nic.calls.lock().unwrap().is_empty(), "choosing a mode does not touch the adapter");
    // Enter on a MAC row opens the same list; the WiFi row's kind is changed
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Down);
    assert!(matches!(a.selected_target(), Some(Target::Mac(n, _)) if n == "wlan0"));
    key(&mut a, &cx, Key::Enter);
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Down); // Per network
    key(&mut a, &cx, Key::Down); // Random
    key(&mut a, &cx, Key::Enter);
    assert_eq!(w.mac.config().wifi, MacMode::Random);
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn apply_now_asks_first_then_changes_the_address_with_the_link_down_and_reconnects() {
    let cx = admin("mac3");
    let w = world("mac3", false, false);
    w.mac.set_mode(Some(iyakki_devctl::LinkKind::Ethernet), None, MacMode::PerNetwork).unwrap();
    let mut a = app(&cx, &w);
    let g = draw(&mut a, &cx, 1280, 720);
    click(&mut a, &cx, &g, "Apply MAC now");
    assert!(a.has_modal(), "a question comes first");
    assert!(w.nic.calls.lock().unwrap().is_empty(), "nothing has changed yet");
    // Esc (any letter) cancels
    key(&mut a, &cx, Key::Esc);
    assert!(!a.has_modal() && w.nic.calls.lock().unwrap().is_empty());
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    let calls = w.nic.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0], "link eno1 down");
    assert!(calls[1].starts_with("mac eno1 ") && calls[2] == "link eno1 up", "{calls:?}");
    assert!(status(&a).contains("00:11:22:33:44:55 ->") && status(&a).contains("Reconnecting"), "{}", status(&a));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("settings.network.mac.apply") && l.contains("eno1")));
    let now = std::fs::read_to_string(w.dir.join("sys/class/net/eno1/address")).unwrap();
    let m = iyakki_devctl::mac::parse_mac(&now).unwrap();
    assert!(iyakki_devctl::mac::valid_unicast(&m) && iyakki_devctl::mac::locally_administered(&m));
    assert!(a.dev_rows()[2].chips.iter().any(|c| c.0 == "private"));
    let log = std::fs::read_to_string(&w.rec).unwrap();
    assert!(log.contains("udhcpc -b -R -i eno1 -p") && !log.contains(" -x") && !log.contains(" -F") && !log.contains(" -H"), "a new DHCP request, no host name: {log}");
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn applying_to_wifi_sets_the_address_for_the_network_it_is_on_and_the_supplicant_gets_scan_randomisation() {
    let cx = admin("mac3w");
    let w = world("mac3w", false, false);
    let mut a = app(&cx, &w);
    // select the WiFi adapter's MAC row
    for _ in 0..3 {
        key(&mut a, &cx, Key::Down);
    }
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    let now = std::fs::read_to_string(w.dir.join("sys/class/net/wlan0/address")).unwrap();
    let (secret, _) = iyakki_devctl::mac::load_secret(&w.mac.secret_path);
    let want = iyakki_devctl::mac::derive(&secret, iyakki_devctl::LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Home Net");
    assert_eq!(now.trim(), iyakki_devctl::mac::fmt_mac(&want), "made for the network the adapter is on, as `iw dev wlan0 link` says");
    let conf = std::fs::read_to_string(w.dir.join("run/wpa_supplicant-wlan0.conf")).unwrap();
    assert!(conf.contains("preassoc_mac_addr=1"), "{conf}");
    let log = std::fs::read_to_string(&w.rec).unwrap();
    assert!(log.contains("iw dev wlan0 link") && log.contains("wpa_supplicant -B -i wlan0") && log.contains("udhcpc -b -R -i wlan0"), "{log}");
    // joining another network sets that network's address first
    let k = iyakki_devctl::wifi::Known::new("Cafe", "", iyakki_devctl::wifi::Security::Open, false, 5).unwrap();
    a.join_wifi(&cx, k);
    settle(&mut a, &cx);
    let after = std::fs::read_to_string(w.dir.join("sys/class/net/wlan0/address")).unwrap();
    let cafe = iyakki_devctl::mac::derive(&secret, iyakki_devctl::LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Cafe");
    assert_eq!(after.trim(), iyakki_devctl::mac::fmt_mac(&cafe), "{}", status(&a));
    assert_ne!(after, now);
    // a network chosen as Off gets the hardware address back
    let mut cfg = w.mac.config();
    cfg.ssids.insert("Home Net".into(), MacMode::Off);
    w.mac.store.save(&cfg).unwrap();
    let k = iyakki_devctl::wifi::Known::new("Home Net", "", iyakki_devctl::wifi::Security::Open, false, 6).unwrap();
    a.join_wifi(&cx, k);
    settle(&mut a, &cx);
    assert_eq!(std::fs::read_to_string(w.dir.join("sys/class/net/wlan0/address")).unwrap().trim(), "aa:bb:cc:dd:ee:ff", "{}", status(&a));
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn the_adapter_that_carries_the_session_in_use_is_named_in_the_warning() {
    let cx = admin("mac4");
    let w = world("mac4", false, true);
    let mut a = app(&cx, &w);
    key(&mut a, &cx, Key::Char('a'));
    let Some(Modal::Confirm(_, text)) = &a.modal else { panic!("a question") };
    assert!(text.contains("WARNING") && text.contains("eno1 carries the connection you are using"), "{text}");
    // WiFi does not carry it
    key(&mut a, &cx, Key::Char('n')); // any letter cancels
    key(&mut a, &cx, Key::Down); // the WiFi row
    key(&mut a, &cx, Key::Char('a'));
    let Some(Modal::Confirm(_, text)) = &a.modal else { panic!("a question") };
    assert!(!text.contains("WARNING") && text.contains("wlan0"), "{text}");
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn a_driver_that_refuses_is_reported_clearly_and_the_adapter_is_left_as_it_was() {
    let cx = admin("mac5");
    let w = world("mac5", true, false);
    w.mac.set_mode(Some(iyakki_devctl::LinkKind::Ethernet), None, MacMode::Random).unwrap();
    let mut a = app(&cx, &w);
    key(&mut a, &cx, Key::Char('a'));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert!(status(&a).contains("not supported by this driver") && status(&a).contains("keeps 00:11:22:33:44:55"), "{}", status(&a));
    assert!(a.status().unwrap().1 || status(&a).contains("not"), "shown as a failure");
    assert_eq!(std::fs::read_to_string(w.dir.join("sys/class/net/eno1/address")).unwrap().trim(), "00:11:22:33:44:55");
    assert_eq!(w.nic.calls.lock().unwrap().last().unwrap(), "link eno1 up", "the link is brought back");
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn switching_ethernet_on_sets_the_private_address_before_the_link_and_dhcp() {
    let cx = admin("mac6");
    let w = world("mac6", false, false);
    w.mac.set_mode(Some(iyakki_devctl::LinkKind::Ethernet), None, MacMode::PerNetwork).unwrap();
    put(w.dir.join("sys/class/net/eno1/flags"), "0x1002\n"); // off
    let mut a = app(&cx, &w);
    key(&mut a, &cx, Key::Enter);
    let calls = w.nic.calls.lock().unwrap().clone();
    assert!(calls.len() == 2 + 1 && calls[0].starts_with("mac eno1 ") && calls[1] == "link eno1 up" && calls[2] == "dhcp eno1", "{calls:?}");
    assert!(status(&a).contains("turned on") && status(&a).contains("->"), "{}", status(&a));
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn a_window_on_another_computer_only_shows_the_address_and_says_the_host_manages_it() {
    let cx = admin("mac7");
    let w = world("mac7", false, false);
    let d = temp_dir("mac7-virt");
    let Ok(v) = iyakki_storage::virt::VirtualDisks::open(&d) else { return };
    let env = StorageEnv { roots: v.roots(), mounter: Arc::new(v.mounter()), mke2fs: "/usr/sbin/mke2fs".into(), scratch: d.clone(), data_dir: d.clone(), virtual_disks: Some(v) };
    let mut a = app(&cx, &w).with_storage(env);
    let rows = a.dev_rows();
    assert!(rows[2].sub.contains("managed by the host operating system") && rows[2].chips[0].0 == "host", "{:?}", rows[2]);
    key(&mut a, &cx, Key::Char('m'));
    assert!(!a.has_modal() && status(&a).contains("host operating system"), "{}", status(&a));
    key(&mut a, &cx, Key::Char('a'));
    assert!(!a.has_modal() && status(&a).contains("host operating system"));
    assert!(w.nic.calls.lock().unwrap().is_empty() && !w.dir.join("etc/net.conf").exists());
    let _ = std::fs::remove_dir_all(&w.dir);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_ordinary_account_cannot_change_it() {
    let cx = admin("mac8");
    let w = world("mac8", false, false);
    let mut ucx = cx.clone();
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut a = SettingsApp::new(&ucx).with_hardware(w.env.clone()).with_mac(w.mac.clone());
    assert!(!a.tabs().contains(&Tab::Network), "the page is for administrators");
    a.tabs.push(Tab::Network); // a hand-built request, as the other hardware tests do
    a.tab = a.tabs.len() - 1;
    key(&mut a, &ucx, Key::Char('m'));
    key(&mut a, &ucx, Key::Char('a'));
    assert!(!a.has_modal() && w.nic.calls.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn the_rows_and_buttons_fit_and_are_clickable_at_four_screen_sizes() {
    let cx = admin("mac9");
    let w = world("mac9", false, false);
    let mut a = app(&cx, &w);
    for (sw, sh) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let g = draw(&mut a, &cx, sw, sh);
        let what = format!("at {sw}x{sh}");
        for needed in ["Private MAC · eno1", "Private MAC · wlan0", "Private MAC", "Apply MAC now"] {
            assert!(g.hits.iter().any(|h| h.label == needed), "{what}: {needed} is there: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        for h in g.hits.iter().filter(|h| h.label.starts_with("Private MAC") || h.label == "Apply MAC now") {
            let r = h.rect;
            assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= sw as f32 + 0.5 && r.y + r.h <= sh as f32 + 0.5, "{what}: {:?} is outside the window {:?}", h.label, (r.x, r.y, r.w, r.h));
        }
        // the two buttons do not sit on each other or on the other buttons of the bar
        let bar: Vec<&crate::ui::Hit> = g.hits.iter().filter(|h| ["Turn on/off", "Join WiFi", "Private MAC", "Apply MAC now", "Look again"].contains(&h.label.as_str())).collect();
        assert_eq!(bar.len(), 5, "{what}: all five buttons: {:?}", bar.iter().map(|h| &h.label).collect::<Vec<_>>());
        for (i, x) in bar.iter().enumerate() {
            for y in &bar[i + 1..] {
                let ox = (x.rect.x + x.rect.w).min(y.rect.x + y.rect.w) - x.rect.x.max(y.rect.x);
                let oy = (x.rect.y + x.rect.h).min(y.rect.y + y.rect.h) - x.rect.y.max(y.rect.y);
                assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} and {:?} overlap", x.label, y.label);
            }
        }
        // the mode list is also on screen and its rows are clickable
        click(&mut a, &cx, &g, "Private MAC");
        let g2 = draw(&mut a, &cx, sw, sh);
        for m in MacMode::ALL {
            let hit = g2.hits.iter().find(|h| h.label.contains(m.label())).unwrap_or_else(|| panic!("{what}: {} missing", m.label()));
            assert!(hit.rect.x >= -0.5 && hit.rect.y >= 0.0 && hit.rect.x + hit.rect.w <= sw as f32 + 0.5 && hit.rect.y + hit.rect.h <= sh as f32 + 0.5, "{what}: {} outside", m.label());
        }
        key(&mut a, &cx, Key::Esc);
        assert!(!a.has_modal(), "{what}");
    }
    let _ = std::fs::remove_dir_all(&w.dir);
}
