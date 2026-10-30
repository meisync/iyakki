//! The Devices popover: the button next to the screenshot icon in the top bar. A compact list of what this machine has and
//! how it is doing (network, Bluetooth, screens, sound, camera, keyboard and pointer, storage, battery), one click to the
//! Settings page that changes it, and "Driver help" to STORE -> Drivers when something is missing.
//!
//! The facts come from the same places as Settings -> Network/Bluetooth/Display/Sound and the Drivers list
//! (`iyakki-devctl`, `iyakki-hwinfo`, `iyakki-storage`, `iyakki-agent`'s driver detection). They are read on a background
//! thread ([`DevPop::tick`]) so drawing never waits for `/sys`, `amixer` or `iw`; every missing file is just "not there".
//! [`gather`] is a plain function over a [`Probe`] (the paths and the seams), so tests feed it a made-up `/sys`.

use crate::safeclamp::SafeClamp;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::apps::devices::DeviceEnv;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// Hit ids: a row (`ROW + index`), a row's own button (`ACT + index`), the footer buttons.
pub const ROW: usize = 930;
pub const ACT: usize = 960;
pub const HELP_BTN: usize = 1030;
pub const CLOSE_BTN: usize = 1031;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Working.
    Ok,
    /// Present, but something is off (no cable, no address, a driver wants firmware).
    Warn,
    /// Expected and not there, or there without a driver.
    Missing,
    /// Not present, and that is fine (no camera, no battery on a desktop).
    Absent,
}

/// Where a row leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Jump {
    None,
    /// A Settings page, by the name Settings knows it under ("network", "bluetooth", "display", "sound", "storage").
    Settings(&'static str),
    /// STORE -> Drivers.
    Drivers,
    /// IYAKKI menu -> Mac drivers (a T2 Mac whose Wi-Fi or Bluetooth needs Apple's firmware).
    MacDrivers,
    /// STORE -> Setup on one card, by its installer id (`mac-firmware`: the Bluetooth patch file a Mac needs).
    Setup(&'static str),
}

/// A Wi-Fi or Bluetooth row whose driver help says it needs firmware leads to the Mac drivers page, where the install is.
fn firmware_jump(why: &str) -> Jump {
    if why.contains("needs firmware:") { Jump::MacDrivers } else { Jump::Drivers }
}

/// A small action on a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    BluetoothScan,
    /// "Load the Bluetooth driver anyway" (a T2 Mac keeps the drivers set aside until firmware exists).
    BluetoothLoad,
    TestSound,
    ToggleMute(u32),
    /// Mute or unmute IYAKKI's volume (Settings -> Sound): the output's card has no hardware volume and IYAKKI's software
    /// volume carries it. The shell carries it out (it needs the settings).
    ToggleSoundMute,
    /// Stop every shared folder at once (Settings -> Shared folders).
    StopSharing,
    /// The firewall emergency switch (administrators; the shell carries it out): take the rules out until switched on again.
    FirewallOff,
}

impl Act {
    pub fn label(&self) -> &'static str {
        match self {
            Act::BluetoothScan => "Scan",
            Act::BluetoothLoad => "Load",
            Act::TestSound => "Test",
            Act::ToggleMute(_) | Act::ToggleSoundMute => "Mute",
            Act::StopSharing => "Stop",
            Act::FirewallOff => "Off",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    pub detail: String,
    pub level: Level,
    pub jump: Jump,
    /// Small buttons on the row (at most two).
    pub acts: Vec<Act>,
}

impl Item {
    fn new(title: &str, detail: impl Into<String>, level: Level, jump: Jump) -> Item {
        Item { title: title.into(), detail: detail.into(), level, jump, acts: Vec::new() }
    }
    fn with_act(mut self, a: Act) -> Item {
        self.acts.push(a);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<Item>,
    /// Drivers that are not working, in plain words: (name, reason).
    pub driver_help: Vec<(String, String)>,
    /// When this was read (seconds since the epoch).
    pub at: i64,
}

impl Snapshot {
    /// Something is missing or off in a way the Drivers list might explain.
    pub fn needs_driver_help(&self) -> bool {
        !self.driver_help.is_empty() || self.items.iter().any(|i| i.level == Level::Missing)
    }
}

/// Everything [`gather`] looks at. Real in production; tests point it at a made-up machine.
#[derive(Clone)]
pub struct Probe {
    pub dev: DeviceEnv,
    pub hw: iyakki_hwinfo::Roots,
    /// Where the data volume is mounted (free space is read from it).
    pub data_dir: PathBuf,
    /// Where a Bluetooth service keeps its paired devices (`/var/lib/bluetooth`).
    pub bt_dir: PathBuf,
    pub drivers: Option<iyakki_agent::store::drivers::Drivers>,
    pub hostname: String,
    /// The data volume lives in memory (a live session).
    pub volatile: bool,
    /// The drawing surface (the window or the framebuffer), for "this window is ..." when `/sys` has nothing.
    pub screen: (u32, u32),
    /// Which network a WiFi interface is joined to (asks `iw`; a seam for tests).
    pub wifi: Arc<dyn Fn(&str) -> Option<iyakki_devctl::WifiLink> + Send + Sync>,
    /// The kernel lists a configured Bluetooth controller: scanning is possible (through the in-house management channel, no program).
    pub can_scan: bool,
}

impl Probe {
    pub fn real(dev: DeviceEnv, drivers: Option<iyakki_agent::store::drivers::Drivers>, data_dir: PathBuf, hostname: String, volatile: bool) -> Probe {
        let roots = dev.roots.clone();
        let wifi_roots = roots.clone();
        Probe {
            dev,
            hw: iyakki_hwinfo::Roots { sys: roots.sys.clone(), proc: roots.proc_.clone() },
            data_dir,
            bt_dir: "/var/lib/bluetooth".into(),
            drivers,
            hostname,
            volatile,
            screen: (0, 0),
            wifi: Arc::new(move |i| iyakki_devctl::wifi_link(&wifi_roots, i)),
            can_scan: iyakki_devctl::btview::controllers().map(|c| c.iter().any(|c| c.configured)).unwrap_or(false),
        }
    }
}

fn read(p: impl AsRef<std::path::Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn names(p: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// "excellent", "good", "fair" or "weak" for a signal in dBm.
pub fn signal_words(dbm: i32) -> &'static str {
    match dbm {
        d if d >= -55 => "excellent",
        d if d >= -67 => "good",
        d if d >= -75 => "fair",
        _ => "weak",
    }
}

fn gb(b: u64) -> String {
    iyakki_storage::human(b)
}

/// The driver rows that are not working, matched to a kind of device by words in their name ("bluetooth", "wi-fi").
fn driver_reason(help: &[(String, String)], words: &[&str]) -> Option<String> {
    help.iter().find(|(n, _)| words.iter().any(|w| n.to_lowercase().contains(w))).map(|(n, why)| format!("{n}: {why}"))
}

/// Read the machine. Safe to call from any thread; nothing here writes.
pub fn gather(p: &Probe) -> Snapshot {
    let r = &p.dev.roots;
    let linux = r.sys.join("class").exists() || r.proc_.join("asound").exists();
    let no_os = "not available on this OS (the window shows what the computer it runs on offers)";
    let mut items = Vec::new();
    let driver_help: Vec<(String, String)> = p
        .drivers
        .as_ref()
        .map(|d| d.rows().into_iter().filter(|row| row.status != iyakki_drivers::known::Status::Working).map(|row| (row.known.title.to_string(), if row.detail.is_empty() { row.known.why.to_string() } else { row.detail.clone() })).collect())
        .unwrap_or_default();

    // ---- network
    if linux {
        let links = iyakki_devctl::links(r);
        let ips = p.dev.ctl.ipv4();
        let ip_of = |n: &str| ips.iter().find(|(i, _)| i == n).map(|(_, a)| a.clone());
        let rf = iyakki_devctl::rfkill_list(r);
        let host = format!("host name {}", p.hostname);
        let mut first = true;
        let mut tag = |detail: String| -> String {
            if std::mem::take(&mut first) { format!("{host} · {detail}") } else { detail }
        };
        let eth: Vec<_> = links.iter().filter(|l| l.kind == iyakki_devctl::LinkKind::Ethernet).collect();
        for l in &eth {
            let (level, state) = if !l.up {
                (Level::Warn, "switched off".to_string())
            } else if !l.carrier {
                (Level::Warn, "no cable plugged in".to_string())
            } else {
                match ip_of(&l.name) {
                    Some(a) => (Level::Ok, format!("connected · {a}")),
                    None => (Level::Warn, "cable in, waiting for an address".to_string()),
                }
            };
            let speed = l.speed.map(|s| format!(" · {s} Mb/s")).unwrap_or_default();
            items.push(Item::new("Ethernet", tag(format!("{} · {state}{speed}", l.name)), level, Jump::Settings("network")));
        }
        let wifis: Vec<_> = links.iter().filter(|l| l.kind == iyakki_devctl::LinkKind::Wifi).collect();
        for l in &wifis {
            let off = rf.iter().any(|k| k.kind == "wlan" && (k.soft || k.hard));
            let (level, state) = if off {
                (Level::Warn, "radio switched off".to_string())
            } else if !l.up || !l.carrier {
                (Level::Warn, "on, not joined to a network".to_string())
            } else {
                let link = (p.wifi)(&l.name);
                let net = link.as_ref().map(|w| w.ssid.clone()).unwrap_or_else(|| "joined".into());
                let sig = link.and_then(|w| w.signal_dbm).map(|d| format!(" · signal {d} dBm ({})", signal_words(d))).unwrap_or_default();
                let ip = ip_of(&l.name).map(|a| format!(" · {a}")).unwrap_or_else(|| " · waiting for an address".into());
                (Level::Ok, format!("{net}{sig}{ip}"))
            };
            items.push(Item::new("Wi-Fi", tag(format!("{} · {state}", l.name)), level, Jump::Settings("network")));
        }
        if wifis.is_empty() {
            let why = driver_reason(&driver_help, &["wi-fi", "wifi", "wireless"]);
            let base = if rf.iter().any(|k| k.kind == "wlan") { "a radio is there but no driver has taken it (it may need firmware)" } else { "no Wi-Fi interface found" };
            let jump = why.as_deref().map_or(Jump::Drivers, firmware_jump);
            items.push(Item::new("Wi-Fi", tag(why.map_or(base.to_string(), |w| format!("{base} · {w}"))), Level::Missing, jump));
        }
        if eth.is_empty() {
            items.push(Item::new("Ethernet", "no Ethernet adapter found", Level::Absent, Jump::Settings("network")));
        }
    } else {
        items.push(Item::new("Network", format!("host name {} · {no_os}", p.hostname), Level::Absent, Jump::None));
    }

    // ---- shared folders (only while something is shared)
    let shared = iyakki_agent::sharedfolder::running_total();
    if shared > 0 {
        items.push(Item::new("Sharing", format!("{shared} shared folder(s) can be opened from the network"), Level::Warn, Jump::Settings("shares")).with_act(Act::StopSharing));
    }

    // ---- Bluetooth
    if linux {
        let bt = iyakki_devctl::bluetooth(r);
        if bt.detected() && (!bt.adapters.is_empty() || !bt.switches.is_empty()) {
            let blocked = bt.switches.iter().any(|k| k.soft || k.hard);
            let name = bt.adapters.first().cloned().unwrap_or_else(|| "adapter".into());
            let paired = paired_devices(&p.bt_dir);
            let pair_text = match paired.len() {
                0 => "no paired devices".to_string(),
                1 => format!("1 paired: {}", paired[0]),
                n => format!("{n} paired: {}", paired.iter().take(3).cloned().collect::<Vec<_>>().join(", ")),
            };
            let level = if blocked { Level::Warn } else { Level::Ok };
            let mut it = Item::new("Bluetooth", format!("{name} · {} · {pair_text}", if blocked { "switched off" } else { "powered on" }), level, Jump::Settings("bluetooth"));
            // what the kernel's management interface says (works where sysfs has no address file): chip, address, power and the reason
            let mgmt = p.dev.ctl.bt_controllers().ok().and_then(|c| c.into_iter().next());
            if let Some(c) = &mgmt {
                let fw = p.dev.ctl.bt_firmware_file();
                let product = read(p.dev.roots.sys.join("class/dmi/id/product_name")).unwrap_or_default();
                it.detail = iyakki_devctl::btview::short(c, iyakki_agent::macfw::uart_chip(&product), fw);
                if !c.addr_state().valid() {
                    it.level = Level::Warn;
                    it.detail.push_str(" · pairing and audio are not available yet");
                }
                // a Mac controller that is listed but has no patch file: say so first and lead to the Setup card, not to a device list
                let mac = product.starts_with("Mac") || product.starts_with("iMac") || product.starts_with("Xserve");
                if mac && iyakki_devctl::btview::needs_firmware(c, fw) {
                    it.level = Level::Missing;
                    it.detail = format!("{} \u{b7} {}", iyakki_devctl::btview::NEEDS_FIRMWARE, it.detail);
                    it.jump = Jump::Setup(iyakki_agent::quickinstall::Item::MacFirmware.id());
                }
            }
            if p.dev.ctl.bt_held() {
                it = it.with_act(Act::BluetoothLoad);
            }
            if !blocked && mgmt.is_none() {
                if p.can_scan {
                    it = it.with_act(Act::BluetoothScan);
                } else {
                    it.detail.push_str(" · scanning needs the Bluetooth service, which this image does not have yet");
                }
            }
            items.push(it);
        } else if bt.usb_present {
            let why = driver_reason(&driver_help, &["bluetooth"]).unwrap_or_else(|| "it may need a driver or firmware".into());
            let jump = firmware_jump(&why);
            items.push(Item::new("Bluetooth", format!("an adapter is plugged in but no driver has taken it · {why}"), Level::Missing, jump));
        } else {
            let why = driver_reason(&driver_help, &["bluetooth"]).unwrap_or_else(|| "no adapter driver or firmware".into());
            let jump = firmware_jump(&why);
            let mut it = Item::new("Bluetooth", format!("no adapter found · {why}"), Level::Missing, jump);
            if p.dev.ctl.bt_held() {
                // a T2 Mac keeps the drivers set aside until firmware exists: nothing is loaded, so no controller appeared
                let product = read(p.dev.roots.sys.join("class/dmi/id/product_name")).unwrap_or_default();
                let chip = if iyakki_agent::macfw::uart_chip(&product).is_some() { format!(" · {}", iyakki_agent::macfw::NO_BCM4364_FILE) } else { String::new() };
                it.detail = format!("no adapter yet · the Bluetooth drivers are set aside until firmware exists{chip}");
                it = it.with_act(Act::BluetoothLoad);
            }
            items.push(it);
        }
    } else {
        items.push(Item::new("Bluetooth", no_os, Level::Absent, Jump::None));
    }

    // ---- displays
    let scr = iyakki_devctl::screen(r);
    let refresh = read(r.sys.join("class/graphics/fb0/modes")).and_then(|m| m.lines().next().and_then(|l| l.rsplit_once('-').map(|(_, hz)| hz.trim().to_string()))).and_then(|hz| hz.parse::<u32>().ok());
    let ui_scale = crate::uisize::LEVELS.iter().min_by(|a, b| (a.1 - crate::ui::ui_size()).abs().total_cmp(&(b.1 - crate::ui::ui_size()).abs())).map_or("Normal", |l| l.0);
    let connected: Vec<_> = scr.outputs.iter().filter(|o| o.connected).collect();
    for o in &connected {
        let card_conn = names(&r.sys.join("class/drm")).into_iter().find(|n| n.ends_with(&format!("-{}", o.name)) && n.starts_with("card"));
        let monitor = card_conn.as_ref().and_then(|n| std::fs::read(r.sys.join("class/drm").join(n).join("edid")).ok()).and_then(|b| iyakki_hwinfo::parse_edid(&b)).map(|m| m.name);
        let mode = o.modes.first().cloned().unwrap_or_else(|| "resolution unknown".into());
        let hz = refresh.map(|h| format!(" @ {h} Hz")).unwrap_or_default();
        items.push(Item::new("Display", format!("{} · {}{mode}{hz} · interface size {ui_scale}", o.name, monitor.map(|m| format!("{m} · ")).unwrap_or_default()), Level::Ok, Jump::Settings("display")));
    }
    if connected.is_empty() {
        let detail = match (&scr.framebuffer, p.screen) {
            (Some(fb), _) => format!("{fb} · interface size {ui_scale}"),
            (None, (w, h)) if w > 0 => format!("this window {w}x{h} · interface size {ui_scale}"),
            _ => "no display information".to_string(),
        };
        items.push(Item::new("Display", detail, if scr.framebuffer.is_some() || p.screen.0 > 0 { Level::Ok } else { Level::Absent }, Jump::Settings("display")));
    }

    // ---- sound
    if linux {
        let cards = iyakki_devctl::cards(r);
        if cards.is_empty() {
            let why = driver_reason(&driver_help, &["audio", "sound"]).unwrap_or_else(|| "no sound card found (it may need a driver)".into());
            items.push(Item::new("Sound", why, Level::Missing, Jump::Drivers));
        } else {
            let pcm = read(r.proc_.join("asound/pcm")).unwrap_or_default();
            let capture = pcm.lines().any(|l| l.contains("capture"));
            // the output IYAKKI plays on (Settings -> Sound), and why there is none
            let (outs, choice) = sound_choice(p);
            let prefs = iyakki_agent::audioout::global().prefs();
            // the volume of the card that plays (the T2 Mac's speaker is card 1, not card 0)
            let c = choice.output.as_ref().and_then(|o| cards.iter().find(|c| c.index == o.card)).unwrap_or(&cards[0]);
            let soft_card = if p.dev.roots.proc_ == std::path::Path::new("/proc") { iyakki_agent::audioout::global().softvol_card() } else { None };
            let (v, mute) = sound_volume_view(c.index, p.dev.ctl.volume(c.index).as_ref(), soft_card, &prefs);
            let soft = if soft_card == Some(c.index) {
                String::new() // the volume above is IYAKKI's
            } else if prefs.muted {
                " · IYAKKI sound muted".to_string()
            } else if prefs.volume_pct < 100 {
                format!(" · IYAKKI volume {}%", prefs.volume_pct)
            } else {
                String::new()
            };
            let (output, level) = match &choice.output {
                Some(o) => (format!("output {}{}", o.label(), if choice.chosen { " (chosen)" } else { " (automatic)" }), Level::Ok),
                None if outs.is_empty() => (format!("output {}", if c.long_name.is_empty() { &c.name } else { &c.long_name }), Level::Ok),
                None => (choice.reason.clone(), Level::Warn),
            };
            let mut it = Item::new("Sound", format!("{output}{soft} · {v} · input {}", if capture { "microphone available" } else { "none" }), level, Jump::Settings("sound")).with_act(Act::TestSound);
            if let Some(m) = mute {
                it = it.with_act(m);
            }
            items.push(it);
        }
    } else {
        items.push(Item::new("Sound", no_os, Level::Absent, Jump::None));
    }

    // ---- camera
    let cams: Vec<String> = names(&r.sys.join("class/video4linux")).into_iter().filter(|n| n.starts_with("video")).filter_map(|n| read(r.sys.join("class/video4linux").join(&n).join("name"))).collect();
    let mut seen: Vec<String> = Vec::new();
    for c in cams {
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    if seen.is_empty() && linux && crate::camera::detect::has_facetime_pci(&r.sys) {
        // a MacBook's built-in camera is there, but without Apple's firmware its driver has no device to offer
        items.push(Item::new("Camera", "built-in FaceTime HD camera needs Apple's firmware (STORE, Setup, Hardware, Camera firmware)", Level::Warn, Jump::None));
    } else if seen.is_empty() {
        items.push(Item::new("Camera", if linux { "none found" } else { no_os }, Level::Absent, Jump::None));
    } else {
        items.push(Item::new("Camera", seen.join(", "), Level::Ok, Jump::None));
    }

    // ---- keyboard and pointer
    if linux {
        let devs = read(r.proc_.join("bus/input/devices")).map(|t| iyakki_hwinfo::parse_input_devices(&t)).unwrap_or_default();
        let of = |k: iyakki_hwinfo::InputKind| -> Vec<&iyakki_hwinfo::InputDevice> { devs.iter().filter(|d| d.kind == k).collect() };
        let kb = of(iyakki_hwinfo::InputKind::Keyboard);
        let ptr: Vec<String> = [iyakki_hwinfo::InputKind::Mouse, iyakki_hwinfo::InputKind::Touchpad, iyakki_hwinfo::InputKind::Touchscreen, iyakki_hwinfo::InputKind::Tablet].iter().flat_map(|k| of(*k).into_iter().map(|d| format!("{} ({})", d.name, d.kind.label()))).collect();
        let detail = format!("keyboard: {} · pointer: {}", if kb.is_empty() { "none found".to_string() } else { kb.iter().map(|d| d.name.clone()).collect::<Vec<_>>().join(", ") }, if ptr.is_empty() { "none found".to_string() } else { ptr.join(", ") });
        items.push(Item::new("Keyboard and pointer", detail, if kb.is_empty() || ptr.is_empty() { Level::Warn } else { Level::Ok }, Jump::None));
    } else {
        items.push(Item::new("Keyboard and pointer", no_os, Level::Absent, Jump::None));
    }

    // ---- storage
    // drives straight from /sys/block (name, size, model): no device is opened or read, so refreshing every few seconds is cheap
    let mut disks: Vec<(String, u64)> = Vec::new();
    for n in names(&r.sys.join("block")) {
        if ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| n.starts_with(p)) {
            continue;
        }
        let dir = r.sys.join("block").join(&n);
        let sectors = read(dir.join("size")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let model = ["vendor", "model"].iter().filter_map(|f| read(dir.join("device").join(f))).collect::<Vec<_>>().join(" ");
        disks.push((if model.is_empty() { n } else { model }, sectors * 512));
    }
    let data = iyakki_storage::space(&p.data_dir).map(|(free, total)| format!("data {} free of {}{}", gb(free), gb(total), if p.volatile { " (in memory: not kept at power-off)" } else { "" }));
    let drives = if disks.is_empty() { "no drives listed".to_string() } else { format!("{} drive{}: {}", disks.len(), if disks.len() == 1 { "" } else { "s" }, disks.iter().take(3).map(|d| format!("{} {}", d.0, gb(d.1))).collect::<Vec<_>>().join(", ")) };
    items.push(Item::new("Storage", [Some(drives), data].into_iter().flatten().collect::<Vec<_>>().join(" · "), if p.volatile { Level::Warn } else { Level::Ok }, Jump::Settings("storage")));

    // ---- firewall (only where boot applied it; a window on someone else's computer has none to show)
    if linux {
        if let Some(e) = iyakki_fw::service::try_engine().filter(|e| e.managed()) {
            use iyakki_fw::Status;
            let (level, detail) = match e.status() {
                Status::On => (Level::Ok, "on · incoming connections are blocked unless allowed".to_string()),
                Status::Pending { secs_left } => (Level::Warn, format!("waiting for you to keep the new rules ({secs_left} s)")),
                Status::Off => (Level::Warn, "off · incoming connections are not blocked".to_string()),
                Status::OffThisBoot => (Level::Warn, "off for this boot (iyakki.firewall=off)".to_string()),
                Status::NotProtected(m) => (Level::Warn, format!("NOT PROTECTED · {m}")),
                Status::Unavailable(m) => (Level::Warn, format!("NOT PROTECTED · {m}")),
            };
            let in_force = e.status().protected() || matches!(e.status(), Status::NotProtected(_));
            let item = Item::new("Firewall", detail, level, Jump::Settings("firewall"));
            // the emergency switch: F8, choose the row, press its button (administrators; docs/FIREWALL.md)
            items.push(if in_force { item.with_act(Act::FirewallOff) } else { item });
        }
    }

    // ---- battery and power
    let ps = r.sys.join("class/power_supply");
    let mut battery = None;
    let mut mains = false;
    for n in names(&ps) {
        match read(ps.join(&n).join("type")).as_deref() {
            Some("Battery") if read(ps.join(&n).join("scope")).as_deref() != Some("Device") => {
                let cap = read(ps.join(&n).join("capacity")).and_then(|c| c.parse::<u32>().ok());
                let status = read(ps.join(&n).join("status")).unwrap_or_default().to_lowercase();
                battery = Some((cap, status));
            }
            Some("Mains") => mains = read(ps.join(&n).join("online")).as_deref() == Some("1") || mains,
            _ => {}
        }
    }
    if let Some((cap, status)) = battery {
        let pct = cap.map_or("level unknown".to_string(), |c| format!("{c}%"));
        let level = if cap.is_some_and(|c| c <= 15) && status != "charging" { Level::Warn } else { Level::Ok };
        items.push(Item::new("Battery", format!("{pct} · {}{}", if status.is_empty() { "unknown" } else { &status }, if mains { " · mains power connected" } else { "" }), level, Jump::Settings("battery")));
    }

    Snapshot { items, driver_help, at: iyakki_agent::clock::now_unix() }
}

/// Names of paired devices from a Bluetooth service's folder (`<adapter>/<device>/info` with `Name=`).
pub fn paired_devices(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for a in names(dir) {
        for d in names(&dir.join(&a)) {
            if let Some(info) = read(dir.join(&a).join(&d).join("info")) {
                let name = info.lines().find_map(|l| l.strip_prefix("Name=")).map(str::to_string).unwrap_or(d);
                out.push(name);
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ doing things

/// Carry out a row's small action. Quick ones return a message to show; slow ones (a scan, a tone) run in the background.
pub fn run_act(p: &Probe, act: &Act) -> Result<String, String> {
    match act {
        Act::StopSharing => Ok(format!("Stopped {} shared folder(s).", iyakki_agent::sharedfolder::stop_everything())),
        Act::FirewallOff => Err("the firewall is switched off by the shell (administrators only)".into()),
        Act::ToggleSoundMute => Err("IYAKKI's mute is switched by the shell (it changes Settings -> Sound)".into()),
        Act::ToggleMute(card) => p.dev.ctl.change_volume(*card, None).map(|v| if v.muted { "Muted.".to_string() } else { format!("Sound on, volume {}%.", v.percent) }),
        Act::TestSound => {
            if !iyakki_devctl::cards(&p.dev.roots).iter().any(|_| true) {
                return Err("No sound card found.".into());
            }
            let (outs, choice) = sound_choice(p);
            let cmd = match &choice.output {
                Some(o) => iyakki_agent::audioout::command_with_device(iyakki_agent::voice::DEFAULT_PLAY_CMD, &o.alsa())?,
                None if outs.is_empty() => iyakki_agent::voice::DEFAULT_PLAY_CMD.to_string(),
                None => return Err(choice.reason),
            };
            let wav = tone_wav(440.0, 0.6);
            // through IYAKKI's player: the output of Settings -> Sound and IYAKKI's volume apply
            let mut player = iyakki_agent::voice::Player::start(&cmd, &wav)?;
            std::thread::spawn(move || {
                player.wait(std::time::Duration::from_secs(5));
            });
            Ok(match &choice.output {
                Some(o) => format!("Playing a short tone on {}.", o.label()),
                None => "Playing a short tone.".into(),
            })
        }
        Act::BluetoothLoad => p.dev.ctl.bt_load_anyway(),
        Act::BluetoothScan => {
            if !p.can_scan {
                return Err("Scanning needs a Bluetooth controller that the kernel lists and that is switched on.".into());
            }
            std::thread::spawn(|| {
                let _ = iyakki_devctl::btview::scan(0, 8);
            });
            Ok("Scanning for Bluetooth devices for 8 seconds.".into())
        }
    }
}

/// The volume shown for the card that plays, and its Mute button. When IYAKKI's software volume carries that card's
/// volume (it has no hardware control), the IYAKKI volume of Settings -> Sound is shown and Mute switches it, so muting
/// keeps the level to come back to; else the card's own mixer, as before.
pub fn sound_volume_view(card: u32, vol: Option<&iyakki_devctl::Volume>, soft_card: Option<u32>, prefs: &iyakki_agent::audioout::SoundSettings) -> (String, Option<Act>) {
    if soft_card == Some(card) {
        let v = if prefs.muted { format!("muted ({}%)", prefs.volume_pct) } else { format!("volume {}%", prefs.volume_pct) };
        return (v, Some(Act::ToggleSoundMute));
    }
    match vol {
        Some(v) => (if v.muted { format!("muted ({}%)", v.percent) } else { format!("volume {}%", v.percent) }, Some(Act::ToggleMute(card))),
        None => ("volume unknown".to_string(), None),
    }
}

/// The sound outputs of the machine the popover looks at, and the one IYAKKI plays on.
fn sound_choice(p: &Probe) -> (Vec<iyakki_agent::audioout::Output>, iyakki_agent::audioout::Choice) {
    let outs = iyakki_agent::audioout::outputs_at(&p.dev.roots.proc_);
    let choice = iyakki_agent::audioout::choose(&outs, &iyakki_agent::audioout::global().prefs().output);
    (outs, choice)
}

/// A mono 16-bit 22.05 kHz WAV of a sine tone, with a short fade at both ends (so it does not click).
pub fn tone_wav(freq: f32, secs: f32) -> Vec<u8> {
    let rate = 22_050u32;
    let n = (rate as f32 * secs) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        let fade = (i.min(n - i) as f32 / (rate as f32 * 0.03)).min(1.0);
        let v = (t * freq * std::f32::consts::TAU).sin() * 0.35 * fade;
        data.extend_from_slice(&((v * i16::MAX as f32) as i16).to_le_bytes());
    }
    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}

// ------------------------------------------------------------------------------------------------ the popover

/// What a key or click asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Close,
    Jump(Jump),
    Act(Act),
}

/// The popover's state. The snapshot is filled by a background thread and read when drawing.
pub struct DevPop {
    pub open: bool,
    sel: usize,
    scroll: usize,
    snap: Arc<Mutex<Option<Snapshot>>>,
    busy: Arc<AtomicBool>,
    last: Option<Instant>,
    seen: i64,
    /// Rows that fit, from the last drawing (for keeping the selection in view).
    visible: usize,
    /// Cloud mode: rows (by title) that describe hardware this machine does not have.
    hide: Vec<&'static str>,
    /// The tooltip with a row's whole text (a cut detail line, the address in it), fed by [`DevPop::tip_event`].
    tip: crate::tooltip::Tooltip,
    /// Where the tooltip was drawn last frame.
    pub tip_rect: Option<R>,
}

/// Take the rows named in `hide` out of a reading.
pub fn without_rows(mut s: Snapshot, hide: &[&str]) -> Snapshot {
    s.items.retain(|i| !hide.contains(&i.title.as_str()));
    s
}

const REFRESH_EVERY: Duration = Duration::from_secs(3);

impl Default for DevPop {
    fn default() -> DevPop {
        DevPop::new()
    }
}

impl DevPop {
    pub fn new() -> DevPop {
        DevPop { open: false, sel: 0, scroll: 0, snap: Arc::new(Mutex::new(None)), busy: Arc::new(AtomicBool::new(false)), last: None, seen: 0, visible: 6, hide: Vec::new(), tip: crate::tooltip::Tooltip::new(), tip_rect: None }
    }

    /// Cloud mode: leave these rows (hardware rows by title) out of every reading from now on.
    pub fn set_hidden(&mut self, rows: Vec<&'static str>) {
        self.hide = rows;
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.sel = 0;
        self.scroll = 0;
        self.last = None; // look again as soon as it opens
        self.tip = crate::tooltip::Tooltip::new();
        self.tip_rect = None;
    }

    /// Every event the shell sees while the popover is open (movement, press, release, wheel, keys, the heartbeat): the
    /// tooltip needs them even when a click region takes the click.
    pub fn tip_event(&mut self, ev: &Event) {
        if self.open {
            self.tip.event(ev);
        }
    }

    /// The tooltip appeared or went away since the last call: the shell draws again without further input.
    pub fn tip_changed(&mut self) -> bool {
        self.open && self.tip.take_changed()
    }

    /// The text of the tooltip drawn last frame, if there was one.
    pub fn tip_text(&self) -> Option<&str> {
        self.tip.shown.as_deref()
    }

    /// Put a reading in without the background thread (tests).
    #[cfg(test)]
    pub fn set_snapshot(&mut self, s: Snapshot) {
        // let a reading in flight finish, and keep new ones away for a while, so the made-up one stays
        while self.busy.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.last = Some(Instant::now() + Duration::from_secs(3600));
        if let Ok(mut slot) = self.snap.lock() {
            *slot = Some(s);
        }
    }

    /// Read again at the next tick (after an action changed something).
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.snap.lock().ok().and_then(|s| s.clone())
    }

    /// The last reading is old (or there is none), open or not: the login screens' top bar keeps its chips fresh with it.
    pub fn wants_refresh_idle(&self) -> bool {
        self.last.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) && !self.busy.load(Ordering::Relaxed)
    }

    /// The popover is open and the last reading is old (or there is none): the shell should call
    /// [`DevPop::refresh_in_background`].
    pub fn wants_refresh(&self) -> bool {
        self.open && self.last.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) && !self.busy.load(Ordering::Relaxed)
    }

    /// Read the machine on a thread of its own, so drawing never waits for `/sys`, `amixer` or `iw`.
    pub fn refresh_in_background(&mut self, probe: &Probe) {
        self.last = Some(Instant::now());
        self.busy.store(true, Ordering::Relaxed);
        let (probe, snap, busy, hide) = (probe.clone(), self.snap.clone(), self.busy.clone(), self.hide.clone());
        let spawned = std::thread::Builder::new().name("iyakki-devices".into()).spawn(move || {
            let s = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| without_rows(gather(&probe), &hide))).ok();
            if let (Some(s), Ok(mut slot)) = (s, snap.lock()) {
                *slot = Some(s);
            }
            busy.store(false, Ordering::Relaxed);
        });
        if spawned.is_err() {
            self.busy.store(false, Ordering::Relaxed);
        }
    }

    /// A new reading has arrived since the last call (the shell then draws again).
    pub fn take_fresh(&mut self) -> bool {
        let now_at = self.snapshot().map_or(0, |s| s.at);
        let fresh = now_at != self.seen;
        self.seen = now_at;
        fresh
    }

    /// Read the machine now, on this thread (tests, and the first open when there is nothing to show yet).
    pub fn refresh_now(&mut self, probe: &Probe) {
        let s = without_rows(gather(probe), &self.hide);
        if let Ok(mut slot) = self.snap.lock() {
            *slot = Some(s);
        }
    }

    fn count(&self) -> usize {
        self.snapshot().map_or(0, |s| s.items.len())
    }

    fn keep_in_view(&mut self) {
        let n = self.visible.max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + n {
            self.scroll = self.sel + 1 - n;
        }
    }

    pub fn handle_key(&mut self, key: &Key) -> Outcome {
        let Some(snap) = self.snapshot() else {
            return if matches!(key, Key::Esc | Key::F(8)) { Outcome::Close } else { Outcome::Nothing };
        };
        let n = snap.items.len();
        match key {
            Key::Esc | Key::F(8) => Outcome::Close,
            Key::Down | Key::Tab if n > 0 => {
                self.sel = (self.sel + 1).min(n - 1);
                self.keep_in_view();
                Outcome::Nothing
            }
            Key::Up if n > 0 => {
                self.sel = self.sel.saturating_sub(1);
                self.keep_in_view();
                Outcome::Nothing
            }
            Key::Home => {
                self.sel = 0;
                self.keep_in_view();
                Outcome::Nothing
            }
            Key::End if n > 0 => {
                self.sel = n - 1;
                self.keep_in_view();
                Outcome::Nothing
            }
            Key::Enter => snap.items.get(self.sel).filter(|i| i.jump != Jump::None).map_or(Outcome::Nothing, |i| Outcome::Jump(i.jump.clone())),
            // a / t / s: the row's first button (Scan, Test); m: its mute button
            Key::Char('a' | 't' | 's') => snap.items.get(self.sel).and_then(|i| i.acts.first().cloned()).map_or(Outcome::Nothing, Outcome::Act),
            Key::Char('m') => snap.items.iter().flat_map(|i| i.acts.iter()).find(|a| matches!(a, Act::ToggleMute(_) | Act::ToggleSoundMute)).cloned().map_or(Outcome::Nothing, Outcome::Act),
            Key::Char('d') | Key::Char('D') if snap.needs_driver_help() => Outcome::Jump(Jump::Drivers),
            Key::Char('r') => {
                self.last = None;
                Outcome::Nothing
            }
            _ => Outcome::Nothing,
        }
    }

    /// A click on one of the popover's own regions (their ids are in the constants above).
    pub fn handle_hit(&mut self, id: usize) -> Outcome {
        let Some(snap) = self.snapshot() else { return Outcome::Nothing };
        match id {
            CLOSE_BTN => Outcome::Close,
            HELP_BTN => Outcome::Jump(Jump::Drivers),
            i if (ROW..ROW + 30).contains(&i) => {
                let idx = i - ROW;
                self.sel = idx.min(snap.items.len().saturating_sub(1));
                snap.items.get(idx).filter(|it| it.jump != Jump::None).map_or(Outcome::Nothing, |it| Outcome::Jump(it.jump.clone()))
            }
            i if (ACT..ACT + 60).contains(&i) => snap.items.get((i - ACT) / 2).and_then(|it| it.acts.get((i - ACT) % 2).cloned()).map_or(Outcome::Nothing, Outcome::Act),
            _ => Outcome::Nothing,
        }
    }

    pub fn scroll_by(&mut self, d: i32) {
        let n = self.count();
        let max = n.saturating_sub(self.visible.max(1));
        self.scroll = (self.scroll as i32 + d).clamp_safe(0, max as i32) as usize;
    }

    /// The panel's rectangle at this size: under the top bar, at the right.
    pub fn panel(w: u32, h: u32, s: f32, rows: usize) -> R {
        let pw = (500.0 * s).min(w as f32 - 24.0 * s);
        let top = 56.0 * s;
        let want = (92.0 + 50.0 * rows as f32) * s;
        let ph = want.min(h as f32 - top - 12.0 * s);
        R::new(w as f32 - pw - 12.0 * s, top, pw, ph)
    }

    /// The pointer is on one of the row's buttons (they sit at the right end of the row, as drawn below).
    fn on_button(&self, g: &Gfx, r: R, widths: &[f32], s: f32) -> bool {
        let mut bx = r.x + r.w - 10.0 * s;
        widths.iter().any(|bw| {
            bx -= bw;
            let br = R::new(bx, r.y + (r.h - 32.0 * s) / 2.0, *bw, 32.0 * s);
            bx -= 8.0 * s;
            br.contains(g.mouse.0, g.mouse.1)
        })
    }

    /// Draw the popover. Registers its own click regions and clears the others (a click outside it closes it).
    pub fn render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if !self.open {
            return;
        }
        g.clear_hits();
        self.tip_rect = None;
        let snap = self.snapshot();
        let rows = snap.as_ref().map_or(2, |x| x.items.len().max(1));
        let p = Self::panel(w, h, s, rows);
        crate::fx::modal_panel(g, p, s, crate::fx::CYAN, false);
        g.text(p.x + 22.0 * s, p.y + 14.0 * s, 19.0 * s, TEXT, "Devices", Face::Bold);
        let hint = "F8 or Esc closes · Up/Down · Enter opens · r refreshes";
        let hw = g.text_width(12.0 * s, hint, Face::Sans);
        if hw < p.w - 190.0 * s {
            g.text(p.x + p.w - hw - 56.0 * s, p.y + 19.0 * s, 12.0 * s, DIM, hint, Face::Sans);
        }
        let close = R::new(p.x + p.w - 44.0 * s, p.y + 8.0 * s, 34.0 * s, 34.0 * s);
        let hot = g.hovered(close);
        crate::fx::chrome_button(g, close, s, hot, crate::fx::CYAN);
        let cw = g.text_width(18.0 * s, "x", Face::Bold);
        g.text(close.x + (close.w - cw) / 2.0, close.y + 6.0 * s, 18.0 * s, MUTED, "x", Face::Bold);
        g.hit(close, Event::Tab(CLOSE_BTN), "close");
        let Some(snap) = snap else {
            g.text(p.x + 22.0 * s, p.y + 62.0 * s, 15.0 * s, MUTED, "Looking at this computer...", Face::Sans);
            return;
        };
        let help_h = if snap.needs_driver_help() { 48.0 * s } else { 0.0 };
        let list = R::new(p.x + 10.0 * s, p.y + 54.0 * s, p.w - 20.0 * s, p.h - 54.0 * s - help_h - 10.0 * s);
        let row_h = 50.0 * s;
        self.visible = ((list.h / row_h).floor() as usize).max(1);
        self.keep_in_view();
        let last = (self.scroll + self.visible).min(snap.items.len());
        for (k, it) in snap.items.iter().enumerate().take(last).skip(self.scroll) {
            let r = R::new(list.x, list.y + (k - self.scroll) as f32 * row_h, list.w, row_h - 4.0 * s);
            let hot = g.hovered(r) || k == self.sel;
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, k == self.sel, hot, crate::fx::CYAN);
            let dot = match it.level {
                Level::Ok => OK,
                Level::Warn => rgb(255, 196, 94),
                Level::Missing => ERR,
                Level::Absent => DIM,
            };
            g.circle(r.x + 16.0 * s, r.y + r.h / 2.0, 5.0 * s, dot);
            let widths: Vec<f32> = it.acts.iter().map(|a| g.text_width(13.0 * s, a.label(), Face::Bold) + 28.0 * s).collect();
            let btn_total: f32 = widths.iter().map(|w| w + 8.0 * s).sum();
            let tx = r.x + 32.0 * s;
            let avail = r.w - 32.0 * s - 14.0 * s - btn_total;
            g.text(tx, r.y + 6.0 * s, 15.0 * s, TEXT, &it.title, Face::Bold);
            // the address (Wi-Fi, Ethernet) is the part that is never cut; the name before it gives way first
            let d = crate::tooltip::fit_keep_ip(g, 13.0 * s, &it.detail, Face::Sans, avail);
            g.text(tx, r.y + 6.0 * s + g.line_height(15.0 * s, Face::Bold) + 1.0 * s, 13.0 * s, MUTED, &d, Face::Sans);
            g.hit(r, Event::Tab(ROW + k), &format!("{}: {}", it.title, it.detail));
            // a cut detail line has its whole text in a tooltip, but not while the pointer is on one of the row's buttons
            // (the tooltip would sit on it)
            if d != it.detail && !self.on_button(g, r, &widths, s) {
                self.tip.offer(g, r, k == self.sel, &format!("{}\n{}", it.title, it.detail));
            }
            let mut bx = r.x + r.w - 10.0 * s;
            for (j, (a, bw)) in it.acts.iter().zip(&widths).enumerate() {
                bx -= bw;
                let br = R::new(bx, r.y + (r.h - 32.0 * s) / 2.0, *bw, 32.0 * s);
                bx -= 8.0 * s;
                let hot = g.hovered(br);
                crate::fx::chrome_button(g, br, s, hot, crate::fx::CYAN);
                let lw = g.text_width(13.0 * s, a.label(), Face::Bold);
                g.text(br.x + (br.w - lw) / 2.0, br.y + (br.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, a.label(), Face::Bold);
                g.hit(br, Event::Tab(ACT + k * 2 + j), a.label());
            }
        }
        if snap.items.len() > last || self.scroll > 0 {
            let more = format!("{} of {} shown · scroll for the rest", last - self.scroll, snap.items.len());
            g.text(p.x + 22.0 * s, p.y + p.h - help_h - 8.0 * s - 14.0 * s, 12.0 * s, DIM, &more, Face::Sans);
        }
        if help_h > 0.0 {
            let n = snap.driver_help.len();
            let label = if n > 0 { format!("Driver help ({n})") } else { "Driver help".to_string() };
            let br = R::new(p.x + 12.0 * s, p.y + p.h - help_h - 4.0 * s, p.w - 24.0 * s, 40.0 * s);
            let hot = g.hovered(br);
            crate::fx::row_at(g, br.x, br.y, br.w, br.h, 10.0 * s, false, hot, rgb(255, 196, 94));
            g.text(br.x + 16.0 * s, br.y + 4.0 * s, 15.0 * s, TEXT, &label, Face::Bold);
            let why = snap.driver_help.first().map(|(n, w)| format!("{n}: {w}")).unwrap_or_else(|| "something is missing: open STORE -> Drivers".to_string());
            let all = if n > 1 { snap.driver_help.iter().map(|(n, w)| format!("{n}: {w}")).collect::<Vec<_>>().join("\n") } else { why.clone() };
            let why = crate::tooltip::fit_keep_ip(g, 12.0 * s, &why, Face::Sans, br.w - 32.0 * s);
            g.text(br.x + 16.0 * s, br.y + 4.0 * s + g.line_height(15.0 * s, Face::Bold), 12.0 * s, MUTED, &why, Face::Sans);
            g.hit(br, Event::Tab(HELP_BTN), "Driver help");
            if n > 1 || why.ends_with('…') {
                self.tip.offer(g, br, false, &format!("{label}\n{all}"));
            }
        }
        self.tip_rect = self.tip.draw(g, s);
    }
}

#[cfg(test)]
#[path = "devpop_tests.rs"]
mod tests;
