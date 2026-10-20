//! The Devices popover against made-up machines: a well-equipped one, an empty one, a Mac mini that needs drivers, and a
//! computer with no `/sys` at all (the desktop window on macOS).

use super::*;
use iyakki_devctl::{Control, Roots};
use std::path::{Path, PathBuf};

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-devpop-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Fake {
    muted: std::sync::Mutex<bool>,
}

impl Control for Fake {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Err("no".into())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![("eth0".into(), "192.168.1.5".into()), ("wlan0".into(), "192.168.1.6".into())]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        Some(iyakki_devctl::Volume { control: "Master".into(), percent: 70, muted: *self.muted.lock().unwrap() })
    }
    fn change_volume(&self, _: u32, d: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        if d.is_none() {
            let mut m = self.muted.lock().unwrap();
            *m = !*m;
        }
        Ok(self.volume(0).unwrap())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("no".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn probe_at(root: &Path) -> Probe {
    let roots = Roots { sys: root.join("sys"), proc_: root.join("proc") };
    Probe {
        dev: DeviceEnv { roots: roots.clone(), ctl: Arc::new(Fake { muted: Default::default() }) },
        hw: iyakki_hwinfo::Roots { sys: roots.sys.clone(), proc: roots.proc_.clone() },
        data_dir: root.to_path_buf(),
        bt_dir: root.join("bluetooth"),
        drivers: None,
        hostname: "kitchen".into(),
        volatile: false,
        screen: (1280, 720),
        wifi: Arc::new(|i| (i == "wlan0").then(|| iyakki_devctl::WifiLink { ssid: "HomeNet".into(), signal_dbm: Some(-48) })),
        can_scan: true,
    }
}

/// Everything a laptop has.
fn full_machine(tag: &str) -> PathBuf {
    let r = tmp(tag);
    // Ethernet with a cable and a gigabit link; WiFi joined to a network
    for (n, extra) in [("eth0", ""), ("wlan0", "wireless")] {
        put(&r, &format!("sys/class/net/{n}/device/x"), "");
        put(&r, &format!("sys/class/net/{n}/type"), "1\n");
        put(&r, &format!("sys/class/net/{n}/flags"), "0x1003\n");
        put(&r, &format!("sys/class/net/{n}/carrier"), "1\n");
        put(&r, &format!("sys/class/net/{n}/address"), "aa:bb:cc:00:00:01\n");
        if !extra.is_empty() {
            put(&r, &format!("sys/class/net/{n}/wireless/x"), "");
        }
    }
    put(&r, "sys/class/net/eth0/speed", "1000\n");
    put(&r, "sys/class/rfkill/rfkill0/type", "bluetooth\n");
    put(&r, "sys/class/rfkill/rfkill0/name", "hci0\n");
    put(&r, "sys/class/rfkill/rfkill0/soft", "0\n");
    put(&r, "sys/class/rfkill/rfkill0/hard", "0\n");
    put(&r, "sys/class/bluetooth/hci0/x", "");
    put(&r, "bluetooth/AA/11/info", "[General]\nName=Magic Keyboard\n");
    put(&r, "bluetooth/AA/22/info", "[General]\nName=Headphones\n");
    // a 4K screen at 60 Hz and a second one that is not plugged in
    put(&r, "sys/class/drm/card0-HDMI-A-1/status", "connected\n");
    put(&r, "sys/class/drm/card0-HDMI-A-1/modes", "3840x2160\n1920x1080\n");
    put(&r, "sys/class/drm/card0-DP-1/status", "disconnected\n");
    put(&r, "sys/class/graphics/fb0/virtual_size", "3840,2160\n");
    put(&r, "sys/class/graphics/fb0/bits_per_pixel", "32\n");
    put(&r, "sys/class/graphics/fb0/name", "i915drmfb\n");
    put(&r, "sys/class/graphics/fb0/modes", "U:3840x2160p-60\n");
    // sound with a microphone
    put(&r, "proc/asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7f10000 irq 32\n");
    put(&r, "proc/asound/pcm", "00-00: ALC3220 Analog : ALC3220 Analog : playback 1 : capture 1\n");
    put(&r, "sys/class/video4linux/video0/name", "FaceTime HD Camera\n");
    put(&r, "proc/bus/input/devices", "I: Bus=0003 Vendor=05ac Product=0250 Version=0110\nN: Name=\"Apple Keyboard\"\nH: Handlers=sysrq kbd event3\nB: EV=120013\nB: KEY=1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe\n\nI: Bus=0003 Vendor=046d Product=c077 Version=0111\nN: Name=\"USB Optical Mouse\"\nH: Handlers=mouse0 event4\nB: EV=17\nB: KEY=70000 0 0 0 0\nB: REL=903\n");
    put(&r, "sys/class/power_supply/BAT0/type", "Battery\n");
    put(&r, "sys/class/power_supply/BAT0/capacity", "81\n");
    put(&r, "sys/class/power_supply/BAT0/status", "Discharging\n");
    put(&r, "sys/class/power_supply/AC/type", "Mains\n");
    put(&r, "sys/class/power_supply/AC/online", "0\n");
    // one disk
    put(&r, "sys/block/sda/size", "976773168\n");
    put(&r, "sys/block/sda/removable", "0\n");
    put(&r, "sys/block/sda/device/model", "Samsung SSD\n");
    r
}

fn item<'a>(s: &'a Snapshot, title: &str) -> &'a Item {
    s.items.iter().find(|i| i.title == title).unwrap_or_else(|| panic!("{title} in {:?}", s.items.iter().map(|i| &i.title).collect::<Vec<_>>()))
}

#[test]
fn a_well_equipped_machine_shows_every_group_with_live_facts() {
    let r = full_machine("full");
    let s = gather(&probe_at(&r));
    let eth = s.items.iter().find(|i| i.title == "Ethernet").unwrap();
    assert!(eth.detail.contains("host name kitchen") && eth.detail.contains("192.168.1.5") && eth.detail.contains("1000 Mb/s"), "{eth:?}");
    assert_eq!((eth.level, &eth.jump), (Level::Ok, &Jump::Settings("network")));
    let wifi = item(&s, "Wi-Fi");
    assert!(wifi.detail.contains("HomeNet") && wifi.detail.contains("-48 dBm (excellent)") && wifi.detail.contains("192.168.1.6"), "{wifi:?}");
    assert!(!wifi.detail.contains("host name"), "the host name is on the first network row only");
    let bt = item(&s, "Bluetooth");
    assert!(bt.detail.contains("hci0") && bt.detail.contains("powered on") && bt.detail.contains("2 paired") && bt.detail.contains("Magic Keyboard"), "{bt:?}");
    assert_eq!(bt.acts, vec![Act::BluetoothScan]);
    let disp = item(&s, "Display");
    assert!(disp.detail.contains("HDMI-A-1") && disp.detail.contains("3840x2160") && disp.detail.contains("60 Hz") && disp.detail.contains("interface size"), "{disp:?}");
    assert_eq!(s.items.iter().filter(|i| i.title == "Display").count(), 1, "the unplugged output is not listed");
    let snd = item(&s, "Sound");
    assert!(snd.detail.contains("HDA Intel PCH") && snd.detail.contains("volume 70%") && snd.detail.contains("microphone available"), "{snd:?}");
    assert_eq!(snd.acts, vec![Act::TestSound, Act::ToggleMute(0)]);
    assert_eq!(item(&s, "Camera").detail, "FaceTime HD Camera");
    let kb = item(&s, "Keyboard and pointer");
    assert!(kb.detail.contains("Apple Keyboard") && kb.detail.contains("USB Optical Mouse") && kb.level == Level::Ok, "{kb:?}");
    let st = item(&s, "Storage");
    assert!(st.detail.contains("1 drive") && st.detail.contains("Samsung SSD") && st.detail.contains("free of"), "{st:?}");
    let bat = item(&s, "Battery");
    assert!(bat.detail.starts_with("81% · discharging"), "{bat:?}");
    assert!(!s.needs_driver_help(), "nothing is missing");
}

#[test]
fn what_is_missing_says_so_in_plain_words_and_points_at_the_drivers() {
    let r = tmp("empty");
    std::fs::create_dir_all(r.join("sys/class")).unwrap();
    let s = gather(&probe_at(&r));
    let wifi = item(&s, "Wi-Fi");
    assert_eq!((wifi.level, &wifi.jump), (Level::Missing, &Jump::Drivers));
    assert!(wifi.detail.contains("host name kitchen") && wifi.detail.contains("no Wi-Fi interface"), "{wifi:?}");
    let bt = item(&s, "Bluetooth");
    assert_eq!((bt.level, &bt.jump), (Level::Missing, &Jump::Drivers));
    assert!(bt.detail.contains("no adapter found"), "{bt:?}");
    assert_eq!(item(&s, "Sound").level, Level::Missing);
    assert_eq!(item(&s, "Camera").level, Level::Absent, "no camera is not an error");
    assert!(s.items.iter().all(|i| i.title != "Battery"), "no battery row on a desktop");
    assert!(s.needs_driver_help());
    // a Bluetooth adapter that is plugged in but has no driver is a different sentence
    put(&r, "sys/bus/usb/devices/1-4/bDeviceClass", "e0\n");
    let s = gather(&probe_at(&r));
    assert!(item(&s, "Bluetooth").detail.contains("plugged in but no driver"), "{:?}", item(&s, "Bluetooth"));
}

#[test]
fn a_mac_mini_gets_the_reason_from_the_driver_list() {
    let r = tmp("mini");
    std::fs::create_dir_all(r.join("sys/class")).unwrap();
    let dr = tmp("mini-drv");
    for (rel, c) in [("sys/class/dmi/id/sys_vendor", "Apple Inc.\n"), ("sys/class/dmi/id/product_name", "Macmini7,1\n"), ("proc/sys/kernel/osrelease", "6.12.112\n"), ("proc/modules", "btusb 1 0 - Live 0x0\n"), ("sys/bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:03:00.0/device", "0x43a0\n"), ("sys/bus/usb/devices/1-4/idVendor", "05ac\n"), ("sys/bus/usb/devices/1-4/idProduct", "8289\n"), ("sys/bus/usb/devices/1-4/product", "Bluetooth USB Host Controller\n")] {
        put(&dr, rel, c);
    }
    let mut p = probe_at(&r);
    p.drivers = Some(iyakki_agent::store::drivers::Drivers::for_data(&dr.join("data")).with_roots(iyakki_drivers::Roots { sys: dr.join("sys"), proc: dr.join("proc") }));
    let s = gather(&p);
    assert!(!s.driver_help.is_empty(), "the Mac mini's Wi-Fi and Bluetooth packs are not installed");
    let wifi = item(&s, "Wi-Fi");
    assert!(wifi.detail.to_lowercase().contains("bcm4360") || wifi.detail.to_lowercase().contains("wi-fi"), "{wifi:?}");
    assert!(wifi.detail.len() > "no Wi-Fi interface found".len() + 20, "the reason from the driver list is in the sentence: {wifi:?}");
    assert!(s.needs_driver_help());
}

#[test]
fn a_computer_with_no_sys_still_answers_and_says_what_it_cannot_see() {
    let r = tmp("mac");
    let mut p = probe_at(&r);
    p.dev.roots = Roots { sys: r.join("nope/sys"), proc_: r.join("nope/proc") };
    p.hw = iyakki_hwinfo::Roots { sys: p.dev.roots.sys.clone(), proc: p.dev.roots.proc_.clone() };
    let s = gather(&p);
    for t in ["Bluetooth", "Sound", "Keyboard and pointer"] {
        assert!(item(&s, t).detail.contains("not available on this OS"), "{t}: {:?}", item(&s, t));
        assert_eq!(item(&s, t).level, Level::Absent);
    }
    assert!(item(&s, "Network").detail.contains("host name kitchen"));
    assert!(item(&s, "Display").detail.contains("this window 1280x720"), "{:?}", item(&s, "Display"));
    assert!(item(&s, "Storage").detail.contains("free of"), "storage works anywhere: {:?}", item(&s, "Storage"));
    assert!(!s.needs_driver_help(), "nothing to fix on a computer we only look at");
}

#[test]
fn cable_and_radio_problems_are_warnings_not_missing() {
    let r = full_machine("warn");
    put(&r, "sys/class/net/eth0/carrier", "0\n");
    put(&r, "sys/class/rfkill/rfkill0/hard", "1\n");
    put(&r, "sys/class/power_supply/BAT0/capacity", "9\n");
    let s = gather(&probe_at(&r));
    let eth = s.items.iter().find(|i| i.title == "Ethernet").unwrap();
    assert!(eth.detail.contains("no cable") && eth.level == Level::Warn, "{eth:?}");
    assert_eq!(item(&s, "Bluetooth").level, Level::Warn);
    assert!(item(&s, "Bluetooth").acts.is_empty(), "no Scan while it is switched off");
    assert_eq!(item(&s, "Battery").level, Level::Warn);
}

#[test]
fn signal_words_and_the_test_tone_are_sensible() {
    assert_eq!([signal_words(-40), signal_words(-60), signal_words(-70), signal_words(-85)], ["excellent", "good", "fair", "weak"]);
    let w = tone_wav(440.0, 0.5);
    assert_eq!(&w[..4], b"RIFF");
    assert_eq!(&w[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize, w.len() - 44);
    assert!(w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap() > 5000, "audible");
    assert_eq!(&w[44..46], &[0u8, 0u8][..], "starts silent (faded in)");
}

#[test]
fn keys_and_clicks_reach_the_right_outcomes() {
    let r = full_machine("keys");
    let p = probe_at(&r);
    let mut d = DevPop::new();
    assert_eq!(d.handle_key(&Key::Enter), Outcome::Nothing, "nothing to open before the first reading");
    d.toggle();
    d.refresh_now(&p);
    assert_eq!(d.handle_key(&Key::Enter), Outcome::Jump(Jump::Settings("network")), "the first row is the network");
    assert_eq!(d.handle_key(&Key::Down), Outcome::Nothing);
    assert_eq!(d.handle_key(&Key::Down), Outcome::Nothing);
    // Bluetooth is the third row (Ethernet, Wi-Fi, Bluetooth)
    assert_eq!(d.handle_key(&Key::Char('a')), Outcome::Act(Act::BluetoothScan));
    assert_eq!(d.handle_key(&Key::Char('m')), Outcome::Act(Act::ToggleMute(0)));
    assert_eq!(d.handle_key(&Key::Char('d')), Outcome::Nothing, "no Driver help when nothing is missing");
    assert_eq!(d.handle_key(&Key::Esc), Outcome::Close);
    assert_eq!(d.handle_key(&Key::F(8)), Outcome::Close, "F8 toggles");
    assert_eq!(d.handle_hit(CLOSE_BTN), Outcome::Close);
    assert_eq!(d.handle_hit(ROW + 1), Outcome::Jump(Jump::Settings("network")));
    assert_eq!(d.handle_hit(ACT + 2 * 2), Outcome::Act(Act::BluetoothScan));
    assert_eq!(d.handle_hit(ACT + 2 * 99), Outcome::Nothing);
    assert_eq!(d.handle_hit(ROW + 29), Outcome::Nothing, "past the last row");
}

#[test]
fn driver_help_is_one_key_away_when_something_is_missing() {
    let r = tmp("help");
    std::fs::create_dir_all(r.join("sys/class")).unwrap();
    let p = probe_at(&r);
    let mut d = DevPop::new();
    d.toggle();
    d.refresh_now(&p);
    assert_eq!(d.handle_key(&Key::Char('d')), Outcome::Jump(Jump::Drivers));
    assert_eq!(d.handle_hit(HELP_BTN), Outcome::Jump(Jump::Drivers));
}

#[test]
fn actions_run_quickly_or_say_why_not() {
    let r = full_machine("acts");
    let p = probe_at(&r);
    assert_eq!(run_act(&p, &Act::ToggleMute(0)).unwrap(), "Muted.");
    assert!(run_act(&p, &Act::ToggleMute(0)).unwrap().starts_with("Sound on"));
    let mut p2 = probe_at(&r);
    p2.can_scan = false;
    assert!(run_act(&p2, &Act::BluetoothScan).unwrap_err().contains("Bluetooth service"));
    let empty = tmp("acts-empty");
    std::fs::create_dir_all(empty.join("proc")).unwrap();
    assert!(run_act(&probe_at(&empty), &Act::TestSound).unwrap_err().contains("No sound card"));
}

#[test]
fn reading_in_the_background_never_blocks_and_arrives_once() {
    let r = full_machine("bg");
    let p = probe_at(&r);
    let mut d = DevPop::new();
    assert!(!d.wants_refresh(), "closed: no reading");
    d.toggle();
    assert!(d.wants_refresh());
    d.refresh_in_background(&p);
    assert!(!d.wants_refresh(), "one at a time, and not again for a few seconds");
    for _ in 0..200 {
        if d.snapshot().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(d.snapshot().is_some_and(|s| s.items.len() >= 8));
    assert!(d.take_fresh(), "the shell is told once");
    assert!(!d.take_fresh());
}

#[test]
fn the_popover_fits_at_every_size_and_scrolls_when_it_must() {
    let r = full_machine("draw");
    let p = probe_at(&r);
    for (w, h) in [(800u32, 480u32), (1024, 600), (1280, 720), (1920, 1080)] {
        let s = crate::shell::scale_for(w, h);
        let mut d = DevPop::new();
        d.toggle();
        d.refresh_now(&p);
        let mut g = Gfx::new(w, h);
        d.render(&mut g, w, h, s);
        let panel = DevPop::panel(w, h, s, d.snapshot().unwrap().items.len());
        assert!(panel.x >= 0.0 && panel.y >= 0.0 && panel.x + panel.w <= w as f32 + 0.5 && panel.y + panel.h <= h as f32 + 0.5, "{w}x{h} panel {panel:?}");
        assert!(g.hits.iter().all(|hit| hit.rect.x >= panel.x - 1.0 && hit.rect.x + hit.rect.w <= panel.x + panel.w + 1.0 && hit.rect.y >= panel.y - 1.0 && hit.rect.y + hit.rect.h <= panel.y + panel.h + 1.0), "{w}x{h}: every control is inside the panel");
        // click targets are big enough to hit: rows and buttons
        for hit in g.hits.iter().filter(|hit| matches!(hit.ev, Event::Tab(n) if n >= ROW)) {
            assert!(hit.rect.h >= 24.0 * s && hit.rect.w >= 24.0 * s, "{w}x{h}: {} is {}x{}", hit.label, hit.rect.w, hit.rect.h);
        }
        // no two buttons on one row overlap
        let btns: Vec<_> = g.hits.iter().filter(|hit| matches!(hit.ev, Event::Tab(n) if (ACT..ACT + 60).contains(&n))).collect();
        for (i, a) in btns.iter().enumerate() {
            assert!(btns[i + 1..].iter().all(|b| a.rect.x + a.rect.w <= b.rect.x + 0.5 || b.rect.x + b.rect.w <= a.rect.x + 0.5 || a.rect.y + a.rect.h <= b.rect.y || b.rect.y + b.rect.h <= a.rect.y), "{w}x{h} buttons overlap");
        }
        // walking down to the last row scrolls it into view
        for _ in 0..20 {
            d.handle_key(&Key::Down);
        }
        let mut g = Gfx::new(w, h);
        d.render(&mut g, w, h, s);
        let n = d.snapshot().unwrap().items.len();
        assert!(g.hits.iter().any(|hit| hit.ev == Event::Tab(ROW + n - 1)), "{w}x{h}: the last row is shown after pressing Down");
    }
}

/// The Mac mini 2018's sound card: HDMI devices 3, 7, 8 only (no device 0); `monitor` puts PDM-15T on the first port.
fn mac_mini_sound(tag: &str, monitor: bool) -> PathBuf {
    let r = tmp(tag);
    put(&r, "proc/asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xa1210000 irq 136\n");
    for (dev, id) in [(3, "HDMI 0"), (7, "HDMI 1"), (8, "HDMI 2")] {
        put(&r, &format!("proc/asound/card0/pcm{dev}p/info"), &format!("card: 0\ndevice: {dev}\nstream: PLAYBACK\nid: {id}\nname: {id}\n"));
    }
    for p in 0..9 {
        put(&r, &format!("proc/asound/card0/eld#2.{p}"), if monitor && p == 6 { "monitor_present\t1\neld_valid\t1\nmonitor_name\tPDM-15T\nsad_count\t1\n" } else { "monitor_present\t0\neld_valid\t0\n" });
    }
    std::fs::create_dir_all(r.join("sys")).unwrap();
    r
}

#[test]
fn the_sound_row_shows_the_hdmi_monitor_in_use_or_why_there_is_no_output() {
    let r = mac_mini_sound("snd-mon", true);
    let s = gather(&probe_at(&r));
    let snd = item(&s, "Sound");
    assert!(snd.detail.starts_with("output HDMI: PDM-15T (automatic)") && snd.level == Level::Ok, "{snd:?}");
    let _ = std::fs::remove_dir_all(&r);
    let r = mac_mini_sound("snd-none", false);
    let p = probe_at(&r);
    let snd = item(&gather(&p), "Sound").clone();
    assert!(snd.detail.contains(iyakki_agent::audioout::NO_OUTPUT) && snd.level == Level::Warn, "{snd:?}");
    // Test explains instead of playing into a port with nothing on it
    assert!(run_act(&p, &Act::TestSound).unwrap_err().contains("T2 Macs"));
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn the_sound_row_shows_and_mutes_iyakkis_software_volume_when_it_carries_the_card_that_plays() {
    let prefs = iyakki_agent::audioout::SoundSettings { volume_pct: 30, ..Default::default() };
    let soft = iyakki_devctl::Volume { control: "IYAKKI".into(), percent: 30, muted: false };
    // the T2 Mac's speaker (card 1) through IYAKKI's softvol: Mute switches IYAKKI's mute, which keeps the 30 %
    assert_eq!(sound_volume_view(1, Some(&soft), Some(1), &prefs), ("volume 30%".to_string(), Some(Act::ToggleSoundMute)));
    let muted = iyakki_agent::audioout::SoundSettings { muted: true, ..prefs.clone() };
    assert_eq!(sound_volume_view(1, None, Some(1), &muted).0, "muted (30%)");
    assert_eq!(Act::ToggleSoundMute.label(), "Mute");
    // a card with its own mixer, or none at all, as before
    let hw = iyakki_devctl::Volume { control: "Master".into(), percent: 62, muted: true };
    assert_eq!(sound_volume_view(0, Some(&hw), Some(1), &prefs), ("muted (62%)".to_string(), Some(Act::ToggleMute(0))));
    assert_eq!(sound_volume_view(0, None, None, &prefs), ("volume unknown".to_string(), None));
    // the popover cannot change Settings itself: the shell carries this one out
    assert!(run_act(&probe_at(&tmp("softmute")), &Act::ToggleSoundMute).is_err());
}
