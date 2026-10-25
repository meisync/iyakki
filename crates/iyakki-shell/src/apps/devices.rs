//! The hardware pages of Settings: Network (Ethernet, WiFi), Bluetooth, Display and Sound.
//!
//! Each page lists what the machine actually has: an adapter that is not detected is shown as such and cannot be
//! switched. Detection and switching live in `iyakki-devctl`; this file turns that into rows and key actions, and has
//! no drawing code so it can be tested against a fake machine.

use crate::safeclamp::SafeClamp;
use std::sync::Arc;

use iyakki_devctl::{Control, LinkKind, Roots};

use crate::gfx::rgb;
use crate::ui::{Key, DIM, OK};

/// The machine the pages look at and act on. Real in production; tests use a fake one.
#[derive(Clone)]
pub struct DeviceEnv {
    pub roots: Roots,
    pub ctl: Arc<dyn Control>,
}

impl DeviceEnv {
    pub fn real() -> DeviceEnv {
        #[cfg(test)]
        {
            // unit tests never look at (or touch) the machine they run on
            return DeviceEnv { roots: Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() }, ctl: Arc::new(NoControl) };
        }
        #[allow(unreachable_code)]
        {
            // "Load the Bluetooth driver anyway" lives in the driver code above the hardware library
            iyakki_agent::macfw::register_bluetooth_loader();
            DeviceEnv { roots: Roots::real(), ctl: Arc::new(iyakki_devctl::System) }
        }
    }
}

/// Does nothing (used where the real machine must not be touched).
pub struct NoControl;

impl Control for NoControl {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Err("not available here".into())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        Err("not available here".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("not available here".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevTab {
    Network,
    Bluetooth,
    Display,
    Sound,
}

/// What a row acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Nothing,
    Ethernet(String),
    /// A WiFi interface and the radio switches that belong to it.
    Wifi(String, Vec<String>),
    Radio(Vec<String>),
    Backlight(String),
    Card(u32),
    /// The private MAC address setting of one adapter.
    Mac(String, LinkKind),
    /// Settings -> Sound: which output IYAKKI plays on (Automatic or one device).
    SoundChoice,
    /// One sound output: its key (`PCH:3`) and ALSA device (`plughw:0,3`).
    SoundOut(String, String),
    /// IYAKKI's own volume and mute (applied in software).
    SoftVolume,
    /// Advanced: the software boost of this card (card id) in dB.
    SoundBoost(String),
    /// Advanced: which volume control this card uses (card id): Automatic, Hardware, Software.
    SoundVolMode(String),
    /// Advanced: "Does the hardware volume work?" (card, device, control name, card id).
    SoundHwCheck(u32, u32, String, String),
    /// Advanced: the stereo test (`false`) or the channel walk (`true`).
    SoundPattern(bool),
    /// Show or hide the Advanced rows.
    SoundAdvanced,
    /// Power a Bluetooth controller (by index) on (`true`) or off.
    BtPower(u16, bool),
    /// Scan for devices for 10 seconds with this controller (proof that it works; never pairs).
    BtScan(u16),
    /// "Load the Bluetooth driver anyway" (the drivers are set aside on a T2 Mac without firmware).
    BtLoad,
    /// The controller exists but has no firmware: opens Store -> Setup on the Mac firmware card (the shell is asked through [`DevicesPane::take_request`]).
    BtFirmware,
}

/// A value the row can show as a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliderVal {
    pub value: i64,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub unit: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DevRow {
    pub title: String,
    pub sub: String,
    pub chips: Vec<(String, [u8; 4])>,
    pub target: Target,
    /// Set for brightness and volume: the page draws a slider.
    pub slider: Option<SliderVal>,
}

impl DevRow {
    fn new(title: impl Into<String>, sub: impl Into<String>, target: Target) -> DevRow {
        DevRow { title: title.into(), sub: sub.into(), chips: vec![], target, slider: None }
    }
    fn with_slider(mut self, v: SliderVal) -> DevRow {
        self.slider = Some(v);
        self
    }
    fn chip(mut self, t: impl Into<String>, c: [u8; 4]) -> DevRow {
        self.chips.push((t.into(), c));
        self
    }
    /// Can be switched or adjusted.
    pub fn actionable(&self) -> bool {
        self.target != Target::Nothing
    }
}

/// What one key press did.
pub struct Done {
    pub result: Result<String, String>,
    /// For the audit log: what changed (names only).
    pub audit: String,
}

/// Seconds between looks at the hardware while a page is open, in ticks (30 per second).
const REFRESH_TICKS: u64 = 60;

struct Snapshot {
    links: Vec<iyakki_devctl::Link>,
    ips: Vec<(String, String)>,
    rfkill: Vec<iyakki_devctl::Rfkill>,
    bt: iyakki_devctl::Bluetooth,
    screen: iyakki_devctl::Screen,
    cards: Vec<(iyakki_devctl::Card, Option<iyakki_devctl::Volume>)>,
    outputs: Vec<iyakki_agent::audioout::Output>,
    /// The card whose software volume (IYAKKI's ALSA softvol) carries the IYAKKI volume.
    soft_card: Option<u32>,
    /// Channels of the shared device behind the volume (4 on the MacBookPro14,1); None: not a shared device.
    mix_channels: Option<u32>,
    /// A sound card is still starting up (the built-in speakers of a MacBook start up slowly).
    starting: Option<String>,
    /// The card whose own hardware volume control is not used (it does nothing, or Software was chosen).
    hw_ignored: Option<u32>,
    /// The sound card is held by another program: the sentence to show.
    busy: Option<String>,
    mac: Vec<iyakki_devctl::mac::MacInfo>,
    /// The Bluetooth controllers the kernel's management interface reports (`Err`: it could not be asked).
    btc: Result<Vec<iyakki_devctl::btview::Controller>, String>,
    /// The chip the machine model says the UART Bluetooth is (`BCM4364`), and whether its firmware file exists.
    bt_chip: Option<&'static str>,
    bt_fw: bool,
    /// A Mac whose Broadcom Bluetooth controller is listed but has no valid address and no patch file: it needs firmware, so no page may
    /// show it as an ordinary device list ([`iyakki_devctl::btview::needs_firmware`]).
    bt_needs_fw: bool,
    /// The Bluetooth drivers are set aside until firmware exists.
    bt_held: bool,
    /// A MacBookPro13,x/14,x or iMac whose CS8409 codec has no driver (`iyakki_drivers::cs8409`): the line to show.
    cs8409: Option<String>,
}

/// A scan for Bluetooth devices: running in the background, or its last result.
#[derive(Clone, Debug, Default)]
pub enum Scan {
    #[default]
    Idle,
    Running,
    Done(Result<Vec<iyakki_devctl::btmgmt::Found>, String>),
}

/// Seconds a scan lasts.
pub const SCAN_SECS: u64 = 10;

pub struct DevicesPane {
    scan: Arc<std::sync::Mutex<Scan>>,
    env: DeviceEnv,
    snap: Option<Snapshot>,
    at: u64,
    /// What the private MAC address setting reads and changes.
    mac: iyakki_devctl::mac::MacEnv,
    /// Settings -> Sound (output choice, IYAKKI's volume), given by the Settings app.
    sound: iyakki_agent::audioout::SoundSettings,
    /// The Advanced rows of the Sound page are shown.
    advanced: bool,
    /// An app the shell should open next (Bluetooth firmware -> Store, Setup): taken by [`DevicesPane::take_request`].
    request: Option<(String, String)>,
}

impl DevicesPane {
    pub fn new(env: DeviceEnv) -> DevicesPane {
        #[cfg(test)]
        let mac = iyakki_devctl::mac::MacEnv { roots: env.roots.clone(), ..iyakki_devctl::mac::MacEnv::nowhere(env.ctl.clone()) };
        #[cfg(not(test))]
        let mac = iyakki_devctl::mac::MacEnv { ctl: env.ctl.clone(), roots: env.roots.clone(), ..iyakki_devctl::mac::MacEnv::real() };
        DevicesPane { env, snap: None, at: 0, mac, sound: Default::default(), scan: Default::default(), advanced: false, request: None }
    }

    /// What Settings -> Sound says now (the rows show the choice and the volume).
    pub fn set_sound(&mut self, sound: iyakki_agent::audioout::SoundSettings) {
        self.sound = sound;
    }

    /// Show or hide the Advanced rows of the Sound page; the new state.
    pub fn toggle_advanced(&mut self) -> bool {
        self.advanced = !self.advanced;
        self.advanced
    }

    /// Does the page look at this computer's own sound cards (not a made-up tree)?
    pub fn real_sound(&self) -> bool {
        self.env.roots.proc_ == std::path::Path::new("/proc")
    }

    /// The sound outputs seen at the last look.
    pub fn outputs(&mut self, tick: u64) -> Vec<iyakki_agent::audioout::Output> {
        self.snap(tick).outputs.clone()
    }

    /// The private MAC address settings and actions (a copy).
    pub fn mac(&self) -> &iyakki_devctl::mac::MacEnv {
        &self.mac
    }

    /// Replace what the private MAC address setting acts on (tests).
    pub fn set_mac(&mut self, mac: iyakki_devctl::mac::MacEnv) {
        self.mac = mac;
        self.invalidate();
    }

    /// The desktop window on someone else's computer: the host operating system owns the addresses.
    pub fn set_host_managed(&mut self, on: bool) {
        if self.mac.host_managed != on {
            self.mac.host_managed = on;
            self.invalidate();
        }
    }

    /// Look at the hardware again on the next [`rows`](Self::rows).
    pub fn invalidate(&mut self) {
        self.snap = None;
    }

    fn refresh(&mut self, tick: u64) {
        let r = &self.env.roots;
        let cards = iyakki_devctl::cards(r).into_iter().map(|c| {
            let v = self.env.ctl.volume(c.index);
            (c, v)
        });
        let mac = iyakki_devctl::mac::infos(&*self.env.ctl, r, &self.mac.config());
        let outputs = iyakki_agent::audioout::outputs_at(&r.proc_);
        let soft_card = if self.real_sound() { iyakki_agent::audioout::global().softvol_card() } else { None };
        let (mix_channels, starting, hw_ignored, busy) = if self.real_sound() {
            let g = iyakki_agent::audioout::global();
            if g.is_active() {
                let st = g.status();
                (st.mix.map(|m| m.channels), st.starting, st.hw_ignored, st.busy)
            } else {
                (None, None, None, None)
            }
        } else {
            (None, iyakki_agent::audioout::card_starting(&r.proc_, &r.sys).map(|_| iyakki_agent::audioout::STARTING_LINE.to_string()), None, None)
        };
        let bt = iyakki_devctl::bluetooth(r);
        // the management interface answers where sysfs has no address file; asked only when the kernel made a controller
        let btc = if bt.adapters.is_empty() { Ok(Vec::new()) } else { self.env.ctl.bt_controllers() };
        let product = std::fs::read_to_string(r.sys.join("class/dmi/id/product_name")).unwrap_or_default();
        let (bt_chip, bt_fw, bt_held) = (iyakki_agent::macfw::uart_chip(product.trim()), self.env.ctl.bt_firmware_file(), self.env.ctl.bt_held());
        let bt_needs_fw = needs_bt_firmware(product.trim(), &btc, bt_fw);
        let cs8409 = iyakki_drivers::cs8409::missing_line(&iyakki_drivers::cs8409::Roots { proc_: r.proc_.clone(), sys: r.sys.clone(), lib_modules: "/lib/modules".into() });
        self.snap = Some(Snapshot { links: iyakki_devctl::links(r), ips: self.env.ctl.ipv4(), rfkill: iyakki_devctl::rfkill_list(r), bt, screen: iyakki_devctl::screen(r), cards: cards.collect(), outputs, soft_card, mix_channels, starting, hw_ignored, busy, mac, btc, bt_chip, bt_fw, bt_needs_fw, bt_held, cs8409 });
        self.at = tick;
    }

    fn snap(&mut self, tick: u64) -> &Snapshot {
        if self.snap.is_none() || tick.saturating_sub(self.at) >= REFRESH_TICKS {
            self.refresh(tick);
        }
        self.snap.as_ref().expect("just refreshed")
    }

    pub fn rows(&mut self, tab: DevTab, tick: u64) -> Vec<DevRow> {
        let host = self.mac.host_managed;
        let sound = self.sound.clone();
        let advanced = self.advanced;
        let scan = self.scan.lock().map(|g| g.clone()).unwrap_or_default();
        let s = self.snap(tick);
        match tab {
            DevTab::Network => network_rows(s, host),
            DevTab::Bluetooth => bluetooth_rows(s, &scan),
            DevTab::Display => display_rows(s),
            DevTab::Sound => sound_rows(s, &sound, advanced),
        }
    }

    /// A key on the selected row. `None` when the key means nothing on this page.
    pub fn act(&mut self, tab: DevTab, sel: usize, key: &Key, tick: u64) -> Option<Done> {
        let rows = self.rows(tab, tick);
        let row = rows.get(sel)?.clone();
        let toggle = matches!(key, Key::Enter | Key::Char('e' | ' '));
        let up = matches!(key, Key::Char('+' | '='));
        let down = matches!(key, Key::Char('-'));
        let done = match (&row.target, tab) {
            (Target::Nothing, _) => Some(Done { result: Err(format!("{}: nothing to switch here.", row.title)), audit: String::new() }).filter(|_| toggle || up || down),
            (Target::Ethernet(name), _) if toggle => Some(self.toggle_link(name, &row)),
            (Target::Wifi(name, ids), _) if toggle => Some(self.toggle_wifi(name, ids, &row)),
            (Target::Radio(ids), _) if toggle => Some(self.toggle_radio(ids, &row)),
            (Target::BtPower(i, on), _) if toggle => Some(self.bt_power(*i, *on)),
            (Target::BtScan(i), _) if toggle => Some(self.bt_scan(*i)),
            (Target::BtFirmware, _) if toggle => {
                self.request = Some(("quickinstall".into(), iyakki_agent::quickinstall::Item::MacFirmware.id().into()));
                Some(Done { result: Ok("Opening Store \u{2192} Setup \u{2192} Hardware: Mac Wi-Fi and Bluetooth firmware.".into()), audit: "bluetooth firmware setup opened".into() })
            }
            (Target::BtLoad, _) if toggle => Some(match self.env.ctl.bt_load_anyway() {
                Ok(m) => Done { result: Ok(m), audit: "bluetooth load anyway".into() },
                Err(e) => Done { result: Err(e), audit: String::new() },
            }),
            (Target::Backlight(name), _) if up || down => {
                let cur = self.snap.as_ref().and_then(|s| s.screen.backlights.iter().find(|b| &b.name == name).map(|b| b.percent())).unwrap_or(50);
                let want = if up { (cur + 10).min(100) } else { cur.saturating_sub(10).max(5) };
                Some(match iyakki_devctl::set_brightness(&self.env.roots, name, want) {
                    Ok(()) => Done { result: Ok(format!("Brightness {want}%.")), audit: format!("brightness {name} {want}") },
                    Err(e) => Done { result: Err(e), audit: String::new() },
                })
            }
            (Target::Card(i), _) if up || down || matches!(key, Key::Char('m')) => {
                let delta = if up { Some(5) } else if down { Some(-5) } else { None };
                Some(match self.env.ctl.change_volume(*i, delta) {
                    Ok(v) => Done { result: Ok(if v.muted { "Muted.".to_string() } else { format!("Volume {}%.", v.percent) }), audit: format!("volume card{i}") },
                    Err(e) => Done { result: Err(e), audit: String::new() },
                })
            }
            _ => None,
        };
        if done.is_some() {
            self.invalidate();
        }
        done
    }

    /// Set a slider row to an exact value (a click on its track).
    pub fn set_value(&mut self, tab: DevTab, sel: usize, value: i64, tick: u64) -> Option<Done> {
        let rows = self.rows(tab, tick);
        let row = rows.get(sel)?.clone();
        let slider = row.slider?;
        let v = value.clamp_safe(slider.min, slider.max);
        let done = match &row.target {
            Target::Backlight(name) => match iyakki_devctl::set_brightness(&self.env.roots, name, v as u32) {
                Ok(()) => Done { result: Ok(format!("Brightness {v}%.")), audit: format!("brightness {name} {v}") },
                Err(e) => Done { result: Err(e), audit: String::new() },
            },
            Target::Card(i) => match self.env.ctl.set_volume(*i, v as u32) {
                Ok(vol) => Done { result: Ok(if vol.muted { "Muted.".to_string() } else { format!("Volume {}%.", vol.percent) }), audit: format!("volume card{i}") },
                Err(e) => Done { result: Err(e), audit: String::new() },
            },
            _ => return None,
        };
        self.invalidate();
        Some(done)
    }

    fn toggle_link(&mut self, name: &str, row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        // switching on is a new connection: the private address (if chosen) is set while the link is still down
        let note = if on { String::new() } else { self.private_address_note(name) };
        let r = if on { self.env.ctl.set_link(name, false) } else { self.env.ctl.set_link(name, true).and_then(|_| self.env.ctl.dhcp(name)) };
        match r {
            Ok(()) => Done { result: Ok(format!("Ethernet {name} turned {}.{note}", if on { "off" } else { "on" })), audit: format!("ethernet {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    fn toggle_wifi(&mut self, name: &str, ids: &[String], row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        let mut note = String::new();
        let res = (|| -> Result<(), String> {
            if on {
                self.env.ctl.set_link(name, false)?;
                ids.iter().try_for_each(|id| iyakki_devctl::rfkill_set(&self.env.roots, id, true))
            } else {
                ids.iter().try_for_each(|id| iyakki_devctl::rfkill_set(&self.env.roots, id, false))?;
                note = self.private_address_note(name);
                self.env.ctl.set_link(name, true)?;
                self.env.ctl.dhcp(name)
            }
        })();
        match res {
            Ok(()) => Done { result: Ok(format!("WiFi {name} turned {}.{note}", if on { "off" } else { "on" })), audit: format!("wifi {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    /// Give the adapter its private address (when the setting calls for one) before it is switched on. A failure is said, never
    /// fatal: the adapter then keeps its hardware address.
    fn private_address_note(&self, name: &str) -> String {
        if self.mac.host_managed {
            return String::new();
        }
        let cfg = self.mac.config();
        let kind = iyakki_devctl::links(&self.env.roots).into_iter().find(|l| l.name == name).map(|l| l.kind);
        let Some(kind) = kind else { return String::new() };
        let ssid = if kind == LinkKind::Wifi { self.mac.preferred_ssid() } else { None };
        if !cfg.mode_for(kind, name, ssid.as_deref()).is_private() {
            return String::new();
        }
        match self.mac.apply_now(name, ssid.as_deref(), true) {
            Ok(m) => format!(" {m}"),
            Err(e) => format!(" Private MAC address not applied ({e}); the hardware address is used."),
        }
    }

    fn bt_power(&mut self, index: u16, on: bool) -> Done {
        match self.env.ctl.bt_power(index, on) {
            Ok(()) => Done { result: Ok(format!("Bluetooth controller hci{index} powered {}.", if on { "on" } else { "off" })), audit: format!("bluetooth hci{index} power {}", if on { "on" } else { "off" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    /// Start a scan in the background ([`SCAN_SECS`] seconds); the rows show it running and then the devices. Read-only: it never pairs.
    fn bt_scan(&mut self, index: u16) -> Done {
        {
            let mut g = self.scan.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*g, Scan::Running) {
                return Done { result: Err("A scan is running already.".into()), audit: String::new() };
            }
            *g = Scan::Running;
        }
        let (ctl, out) = (self.env.ctl.clone(), self.scan.clone());
        std::thread::spawn(move || {
            let r = ctl.bt_scan(index, SCAN_SECS);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Scan::Done(r);
        });
        Done { result: Ok(format!("Scanning for {SCAN_SECS} seconds with hci{index}. Pairing is not available yet.")), audit: format!("bluetooth hci{index} scan") }
    }

    /// The controller is listed but needs firmware (as of the last look): Settings shows the "Get firmware" button first.
    pub fn bt_needs_firmware(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.bt_needs_fw)
    }

    /// The app the shell should open next, once (the Bluetooth firmware row leads to Store -> Setup).
    /// Ask the shell to open another app next (Settings -> Storage opens DISKS).
    pub fn ask_open(&mut self, app: &str) {
        self.request = Some((app.into(), String::new()));
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    /// The paragraphs under the Bluetooth page: why the controller is limited, what is held back, and what is not available yet.
    pub fn bt_notes(&mut self, tick: u64) -> Vec<String> {
        let scan = self.scan.lock().map(|g| g.clone()).unwrap_or_default();
        bluetooth_notes(self.snap(tick), &scan)
    }

    fn toggle_radio(&mut self, ids: &[String], row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        match ids.iter().try_for_each(|id| iyakki_devctl::rfkill_set(&self.env.roots, id, on)) {
            Ok(()) => Done { result: Ok(format!("{} turned {}.", row.title, if on { "off" } else { "on" })), audit: format!("radio {} {}", ids.join(","), if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }
}

const WARN: [u8; 4] = rgb(255, 196, 94);

fn onoff(on: bool) -> (&'static str, [u8; 4]) {
    if on { ("ON", OK) } else { ("OFF", DIM) }
}

/// The private MAC address row of one adapter: what is chosen, the hardware address, the one in use now.
fn mac_row(i: &iyakki_devctl::mac::MacInfo, host: bool) -> DevRow {
    let now = format!("now {}{}", i.current, if i.locally_administered { " · locally administered" } else { "" });
    if host {
        return DevRow::new(format!("Private MAC · {}", i.iface), format!("{now} · managed by the host operating system"), Target::Nothing).chip("host", DIM);
    }
    let sub = format!("{} · hardware {} · {now}", i.mode.key(), i.permanent.as_deref().unwrap_or("unknown"));
    let (label, col) = match i.mode {
        iyakki_devctl::mac::MacMode::Off => ("MAC OFF", DIM),
        iyakki_devctl::mac::MacMode::PerNetwork => ("PER NETWORK", OK),
        iyakki_devctl::mac::MacMode::Random => ("RANDOM", OK),
    };
    DevRow::new(format!("Private MAC · {}", i.iface), sub, Target::Mac(i.iface.clone(), i.kind)).chip(label, col).chip(if i.private { "private" } else { "hardware" }, if i.private { OK } else { DIM })
}

fn network_rows(s: &Snapshot, host: bool) -> Vec<DevRow> {
    let mut rows = Vec::new();
    let ip_of = |n: &str| s.ips.iter().find(|(i, _)| i == n).map(|(_, a)| a.clone());
    let eth: Vec<_> = s.links.iter().filter(|l| l.kind == LinkKind::Ethernet).collect();
    for l in &eth {
        let state = if !l.up { "off".to_string() } else if !l.carrier { "no cable".to_string() } else { ip_of(&l.name).map_or("connected, waiting for an address".to_string(), |a| format!("connected · {a}")) };
        let speed = l.speed.map(|v| format!(" · {v} Mb/s")).unwrap_or_default();
        let (t, c) = onoff(l.up);
        rows.push(DevRow::new(format!("Ethernet · {}", l.name), format!("{state}{speed} · {}", l.mac), Target::Ethernet(l.name.clone())).chip(t, c));
    }
    if eth.is_empty() {
        rows.push(DevRow::new("Ethernet", "no adapter detected", Target::Nothing).chip("not found", DIM));
    }
    let wlan_ids: Vec<String> = s.rfkill.iter().filter(|k| k.kind == "wlan").map(|k| k.id.clone()).collect();
    let wlan_on = iyakki_devctl::radio_on(&s.rfkill.iter().filter(|k| k.kind == "wlan").cloned().collect::<Vec<_>>());
    let hard = s.rfkill.iter().any(|k| k.kind == "wlan" && k.hard);
    let wifi: Vec<_> = s.links.iter().filter(|l| l.kind == LinkKind::Wifi).collect();
    for l in &wifi {
        let on = l.up && wlan_on;
        let state = if hard { "switched off by a key or switch on the machine".to_string() } else if !on { "off".to_string() } else if !l.carrier { "on, not connected to a network".to_string() } else { ip_of(&l.name).map_or("connected, waiting for an address".to_string(), |a| format!("connected · {a}")) };
        let (t, c) = onoff(on);
        let mut row = DevRow::new(format!("WiFi · {}", l.name), format!("{state} · {}", l.mac), if hard { Target::Nothing } else { Target::Wifi(l.name.clone(), wlan_ids.clone()) }).chip(t, c);
        if hard {
            row = row.chip("hardware off", WARN);
        }
        rows.push(row);
    }
    if wifi.is_empty() {
        let sub = if s.rfkill.iter().any(|k| k.kind == "wlan") { "a radio is present but has no driver loaded (it may need firmware)" } else { "no adapter detected, so WiFi stays disabled" };
        rows.push(DevRow::new("WiFi", sub, Target::Nothing).chip("not found", DIM));
    }
    // the private MAC address of each adapter, after the adapters themselves
    rows.extend(s.mac.iter().map(|i| mac_row(i, host)));
    rows
}

/// A Mac whose Broadcom Bluetooth controller is listed without a valid address and without its patch file. Only the kernel's management
/// interface can tell (a USB adapter that works looks the same in sysfs), so when it cannot be asked the answer is no.
fn needs_bt_firmware(product: &str, btc: &Result<Vec<iyakki_devctl::btview::Controller>, String>, fw_file: bool) -> bool {
    let mac = product.starts_with("Mac") || product.starts_with("iMac") || product.starts_with("Xserve");
    if !mac || fw_file {
        return false;
    }
    match btc {
        Ok(cs) if !cs.is_empty() => cs.iter().any(|c| iyakki_devctl::btview::needs_firmware(c, fw_file)),
        _ => false,
    }
}

fn bluetooth_rows(s: &Snapshot, scan: &Scan) -> Vec<DevRow> {
    use iyakki_devctl::btview;
    let mut rows = Vec::new();
    // the kernel's management interface lists the controller even when sysfs has no address file
    let controllers = s.btc.as_ref().ok().filter(|c| !c.is_empty());
    if s.bt_needs_fw {
        // honest first: the controller is listed, but it is in its ROM state until Apple's patch file is installed
        rows.push(DevRow::new("Bluetooth needs firmware", btview::NEEDS_FIRMWARE, Target::BtFirmware).chip("needs firmware", WARN));
    }
    for k in s.bt.switches.iter() {
        let on = !k.soft && !k.hard;
        if s.bt_needs_fw && on {
            continue; // an "on" switch next to a controller that cannot work reads like a working device
        }
        let (t, c) = onoff(on);
        let state = if k.hard { "switched off by a key or switch on the machine" } else if on { "on" } else { "off" };
        let mut row = DevRow::new(format!("Bluetooth · {}", if k.name.is_empty() { &k.id } else { &k.name }), state, if k.hard { Target::Nothing } else { Target::Radio(vec![k.id.clone()]) }).chip(t, c);
        if k.hard {
            row = row.chip("hardware off", WARN);
        } else if on && controllers.is_none() && !s.bt.unstarted.is_empty() {
            // the kernel made hci0 but the management interface could not be asked: the old sign (no address file)
            row.sub = "the controller did not start (no address): firmware or UART problem, see TROUBLESHOOTING".into();
            row = row.chip("no answer", WARN);
        }
        rows.push(row);
    }
    if rows.is_empty() && controllers.is_none() {
        for a in &s.bt.adapters {
            rows.push(DevRow::new(format!("Bluetooth · {a}"), "on (this adapter has no software switch)", Target::Nothing).chip("ON", OK));
        }
    }
    if let Some(cs) = controllers {
        for c in cs {
            let st = c.addr_state();
            let mut head = DevRow::new(btview::headline(c, s.bt_chip, s.bt_fw), btview::detail(c), if c.configured { Target::BtPower(c.index, !c.powered()) } else { Target::Nothing });
            let (t, col) = onoff(c.powered());
            head = head.chip(if c.configured { t } else { "NOT SET UP" }, if c.configured { col } else { WARN });
            if !st.valid() {
                head = head.chip("no valid address", WARN);
            }
            rows.push(head);
            let (sub, chip) = match scan {
                Scan::Running => (format!("scanning for {SCAN_SECS} seconds ..."), Some(("scanning", OK))),
                Scan::Done(Ok(f)) => (format!("{} device(s) found in the last scan", f.len()), None),
                Scan::Done(Err(e)) => (e.clone(), Some(("failed", WARN))),
                Scan::Idle if !c.configured => ("not possible: the controller is not set up".to_string(), None),
                Scan::Idle if !c.powered() => ("power the controller on first (p), then scan: it proves the controller works".to_string(), None),
                Scan::Idle => ("lists the devices around with name, address and signal (s); it never pairs".to_string(), None),
            };
            let mut scan_row = DevRow::new(format!("Scan for devices ({SCAN_SECS} s)"), sub, if c.configured { Target::BtScan(c.index) } else { Target::Nothing });
            if let Some((t, c)) = chip {
                scan_row = scan_row.chip(t, c);
            }
            rows.push(scan_row);
            if let Scan::Done(Ok(f)) = scan {
                for d in f.iter().take(20) {
                    let (title, sub) = btview::found_line(d);
                    rows.push(DevRow::new(title, sub, Target::Nothing).chip("seen", DIM));
                }
            }
        }
    } else if rows.is_empty() && s.bt.usb_present {
        rows.push(DevRow::new("Bluetooth adapter (USB)", "found, but no driver has taken it yet", Target::Nothing).chip("no driver", WARN));
    }
    if s.bt_held {
        rows.push(DevRow::new("Load the Bluetooth driver anyway", "the Bluetooth drivers are set aside until firmware exists (l)", Target::BtLoad).chip("held", WARN));
    }
    if rows.is_empty() {
        rows.push(DevRow::new("Bluetooth", "no adapter detected, so Bluetooth stays disabled", Target::Nothing).chip("not found", DIM));
    }
    rows
}

/// The explanation under the Bluetooth page (the reason the controller is limited, what is held, what is not available yet).
fn bluetooth_notes(s: &Snapshot, scan: &Scan) -> Vec<String> {
    use iyakki_devctl::btview;
    let mut v = Vec::new();
    if s.bt_needs_fw {
        v.push(format!("{}. The controller is listed (hci0) but runs in its ROM state, so pairing and audio cannot work until the patch file is installed. Apple's file is fetched for your own Mac at your click, never shipped; macOS may not carry one for every model.", btview::NEEDS_FIRMWARE));
    }
    if let Ok(cs) = &s.btc {
        if let Some(c) = cs.iter().find(|c| !c.addr_state().valid()) {
            v.push(format!("{}: {}.", c.hci(), btview::NO_VALID_ADDRESS));
        }
    }
    if s.bt_chip.is_some() && !s.bt_fw {
        v.push(format!("{}.", iyakki_agent::macfw::NO_BCM4364_FILE));
    }
    if s.bt_held {
        v.push("The Bluetooth drivers are set aside on this Mac until firmware exists. Load the Bluetooth driver anyway (l) starts the controller without a file.".to_string());
    }
    if let Scan::Done(Err(e)) = scan {
        v.push(format!("Scan: {e}."));
    }
    v.push(format!("{}.", btview::NOT_YET));
    v
}

fn display_rows(s: &Snapshot) -> Vec<DevRow> {
    let mut rows = Vec::new();
    let scr = &s.screen;
    rows.push(DevRow::new("Screen in use", scr.framebuffer.clone().unwrap_or_else(|| "no framebuffer found".into()), Target::Nothing));
    let mut idle = 0;
    for o in &scr.outputs {
        if !o.connected {
            idle += 1;
            continue;
        }
        let mut modes: Vec<&String> = Vec::new();
        for m in &o.modes {
            if !modes.contains(&m) {
                modes.push(m); // the same size can be listed once per refresh rate
            }
        }
        let sub = match modes.split_first() {
            None => "connected".to_string(),
            Some((best, rest)) if rest.is_empty() => format!("best {best}"),
            Some((best, rest)) => format!("best {best} · also {}", rest.iter().take(4).map(|m| m.as_str()).collect::<Vec<_>>().join(", ")),
        };
        rows.push(DevRow::new(format!("Output · {}", o.name), sub, Target::Nothing).chip("connected", OK));
    }
    if idle > 0 {
        rows.push(DevRow::new("Other outputs", format!("{idle} not connected"), Target::Nothing).chip("free", DIM));
    }
    for b in &scr.backlights {
        rows.push(DevRow::new(format!("Brightness · {}", b.name), "How bright this screen's backlight is. It never goes fully dark.", Target::Backlight(b.name.clone())).with_slider(SliderVal { value: b.percent() as i64, min: 5, max: 100, step: 5, unit: "%" }));
    }
    rows
}

fn sound_rows(s: &Snapshot, prefs: &iyakki_agent::audioout::SoundSettings, advanced: bool) -> Vec<DevRow> {
    use iyakki_agent::audioout::{choose, Kind};
    if s.cards.is_empty() && s.outputs.is_empty() {
        if let Some(line) = &s.starting {
            return vec![DevRow::new("Sound", line.clone(), Target::Nothing).chip("starting", WARN)];
        }
        return vec![DevRow::new("Sound", format!("no sound card detected · {}", iyakki_agent::audioout::NO_OUTPUT), Target::Nothing).chip("not found", DIM)];
    }
    let mut rows = Vec::new();
    // which output IYAKKI plays on, and why
    let choice = choose(&s.outputs, &prefs.output);
    let mode = if choice.chosen {
        "chosen"
    } else if !prefs.output.is_empty() {
        "Automatic (chosen one missing)"
    } else {
        "Automatic"
    };
    let (sub, chip) = match &choice.output {
        Some(o) => (format!("playing on {} · {}", o.label(), choice.reason), (if choice.chosen { "CHOSEN" } else { "AUTO" }, OK)),
        None => (choice.reason.clone(), ("NO OUTPUT", WARN)),
    };
    rows.push(DevRow::new(format!("Sound output · {mode}"), sub, Target::SoundChoice).chip(chip.0, chip.1));
    // IYAKKI's own volume: a software volume on the output (ALSA softvol) when it has no hardware volume, else applied to
    // the sound IYAKKI plays
    let hdmi = choice.output.as_ref().is_some_and(|o| o.kind == Kind::Hdmi);
    let help = if s.soft_card.is_some() {
        "This output has no hardware volume: IYAKKI gives it a software volume (ALSA softvol) that every program follows, media players included."
    } else if hdmi {
        "Volume is applied by IYAKKI for HDMI (HDMI has no hardware volume): radio, media, reading aloud and sounds."
    } else {
        "Applied by IYAKKI to the radio, media, reading aloud and sounds."
    };
    let mut vol = DevRow::new("IYAKKI volume", help, Target::SoftVolume).with_slider(SliderVal { value: prefs.volume_pct as i64, min: 0, max: 100, step: 5, unit: "%" });
    vol = if prefs.muted { vol.chip("muted", WARN) } else { vol.chip(format!("{}%", prefs.volume_pct), OK) };
    rows.push(vol);
    for o in &s.outputs {
        let active = choice.output.as_ref().is_some_and(|a| a.key() == o.key());
        let mut row = DevRow::new(o.label(), format!("card {} · device {} · {} · {}", o.card, o.device, o.alsa(), o.state()), Target::SoundOut(o.key(), o.alsa()));
        if active {
            row = row.chip("IN USE", OK);
        } else if o.connected == Some(true) {
            row = row.chip("connected", OK);
        } else if o.connected == Some(false) {
            row = row.chip(if o.kind == Kind::Hdmi { "no monitor" } else { "unplugged" }, DIM);
        }
        rows.push(row);
    }
    if let Some(line) = &s.starting {
        rows.push(DevRow::new("Built-in speakers", line.clone(), Target::Nothing).chip("starting", WARN));
    }
    if let Some(line) = &s.busy {
        rows.push(DevRow::new("Sound card in use", line.clone(), Target::Nothing).chip("busy", WARN));
    }
    if let Some(line) = &s.cs8409 {
        rows.push(DevRow::new("Built-in speakers", line.clone(), Target::Nothing).chip("no driver", WARN));
    }
    for (c, v) in &s.cards {
        rows.push(card_row(c, v.as_ref(), s.soft_card.or(s.hw_ignored), s.hw_ignored, prefs));
    }
    advanced_sound_rows(s, prefs, &choice, advanced, &mut rows);
    rows
}

/// The Advanced part of the Sound page: boost, which volume control, the hardware self-check and the speaker tests.
fn advanced_sound_rows(s: &Snapshot, prefs: &iyakki_agent::audioout::SoundSettings, choice: &iyakki_agent::audioout::Choice, advanced: bool, rows: &mut Vec<DevRow>) {
    use iyakki_agent::audioout::VolumeMode;
    rows.push(DevRow::new("Advanced", if advanced { "Boost, which volume control, speaker tests. Press A or Enter to hide." } else { "Boost, which volume control, speaker tests. Press A or Enter to show." }, Target::SoundAdvanced).chip(if advanced { "shown" } else { "hidden" }, DIM));
    let (true, Some(o)) = (advanced, choice.output.as_ref()) else { return };
    if s.soft_card.is_some() {
        let db = prefs.boost_for(&o.card_id);
        let mut r = DevRow::new(format!("Boost · {}", o.card_name), "Extra software gain for a card whose speakers are too quiet at 100 %. May distort: loud peaks are clipped. Take effect with the next sound. 0 dB is off.", Target::SoundBoost(o.card_id.clone())).with_slider(SliderVal { value: db as i64, min: 0, max: iyakki_agent::softvol::MAX_BOOST_DB as i64, step: 1, unit: " dB" });
        r = if db > 0 { r.chip(format!("+{db} dB"), WARN) } else { r.chip("off", DIM) };
        rows.push(r);
    }
    if let Some(ctl) = &o.hw_volume {
        let mode = prefs.volume_mode_for(&o.card_id);
        let using = if o.uses_hw_volume(mode) { "the card's hardware control" } else { "IYAKKI's software volume" };
        let dummy = if o.dummy_volume { " This codec is known to have a hardware control that does nothing." } else { "" };
        rows.push(DevRow::new(format!("Volume control · {}", mode.label()), format!("Now using {using}. Enter switches Automatic, Hardware, Software.{dummy}"), Target::SoundVolMode(o.card_id.clone())).chip(mode.label(), if mode == VolumeMode::Automatic { DIM } else { WARN }));
        rows.push(DevRow::new("Does the hardware volume work?", format!("Plays a quiet tone at 100 % and again at about 12 % of the card's {ctl} control; you say whether the second was quieter. Enter starts."), Target::SoundHwCheck(o.card, o.device, ctl.clone(), o.card_id.clone())));
    }
    rows.push(DevRow::new("Stereo test", "The left speaker for 1 s, then the right one, through the same path and volume as the radio.", Target::SoundPattern(false)));
    if s.mix_channels.is_some_and(|n| n > 2) {
        rows.push(DevRow::new("Channel walk", "Every channel of this four-channel card alone for 1 s, quiet, to tell which speaker is which.", Target::SoundPattern(true)));
    }
}

/// One card's mixer row. The card that carries IYAKKI's software volume (its only control is IYAKKI's softvol) moves the
/// IYAKKI volume, so the two never disagree and mute keeps the level to come back to.
fn card_row(c: &iyakki_devctl::Card, v: Option<&iyakki_devctl::Volume>, soft_card: Option<u32>, ignored: Option<u32>, prefs: &iyakki_agent::audioout::SoundSettings) -> DevRow {
    let title = if c.long_name.is_empty() { c.name.clone() } else { c.long_name.clone() };
    let soft = soft_card == Some(c.index) && (ignored == Some(c.index) || v.is_none_or(|v| v.control == iyakki_agent::softvol::CONTROL));
    if soft {
        let row = DevRow::new(format!("Sound · {title}"), "no hardware volume · IYAKKI's software volume (the IYAKKI volume above)", Target::SoftVolume).with_slider(SliderVal { value: prefs.volume_pct as i64, min: 0, max: 100, step: 5, unit: "%" });
        return if prefs.muted { row.chip("muted", WARN) } else { row.chip(format!("{}%", prefs.volume_pct), OK) };
    }
    let (sub, chip) = match v {
        Some(v) if v.muted => (format!("{} · muted", v.control), ("muted".to_string(), WARN)),
        Some(v) => (format!("{} volume", v.control), (format!("{}%", v.percent), OK)),
        None => ("no volume control on this card".to_string(), ("no mixer".to_string(), DIM)),
    };
    let target = if v.is_some() { Target::Card(c.index) } else { Target::Nothing };
    let mut row = DevRow::new(format!("Sound · {title}"), sub, target).chip(chip.0, chip.1);
    if let Some(v) = v {
        row = row.with_slider(SliderVal { value: v.percent as i64, min: 0, max: 100, step: 5, unit: "%" });
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        volume: Mutex<Option<iyakki_devctl::Volume>>,
        fail_links: bool,
    }

    impl Control for Fake {
        fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
            if self.fail_links {
                return Err("refused".into());
            }
            self.calls.lock().unwrap().push(format!("link {n} {}", if up { "up" } else { "down" }));
            Ok(())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            vec![("eno1".into(), "192.168.1.9".into())]
        }
        fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
            self.volume.lock().unwrap().clone()
        }
        fn change_volume(&self, c: u32, d: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
            let mut v = self.volume.lock().unwrap();
            let cur = v.as_mut().ok_or("no mixer")?;
            match d {
                Some(d) => cur.percent = (cur.percent as i32 + d).clamp(0, 100) as u32,
                None => cur.muted = !cur.muted,
            }
            self.calls.lock().unwrap().push(format!("volume {c} {d:?}"));
            Ok(cur.clone())
        }
        fn set_volume(&self, c: u32, p: u32) -> Result<iyakki_devctl::Volume, String> {
            let mut v = self.volume.lock().unwrap();
            let cur = v.as_mut().ok_or("no mixer")?;
            cur.percent = p.min(100);
            self.calls.lock().unwrap().push(format!("set volume {c} {p}"));
            Ok(cur.clone())
        }
        fn dhcp(&self, n: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("dhcp {n}"));
            Ok(())
        }
    }

    fn machine(tag: &str, full: bool) -> (PathBuf, Roots) {
        let d = std::env::temp_dir().join(format!("iyakki-devpane-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (sys, proc_) = (d.join("sys"), d.join("proc"));
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&proc_).unwrap();
        if full {
            put(&sys, "class/net/eno1/type", "1\n");
            put(&sys, "class/net/eno1/flags", "0x1003\n");
            put(&sys, "class/net/eno1/carrier", "1\n");
            put(&sys, "class/net/eno1/address", "aa:bb:cc:00:00:01\n");
            put(&sys, "class/net/eno1/speed", "1000\n");
            put(&sys, "class/net/eno1/device/x", "");
            put(&sys, "class/net/wlan0/type", "1\n");
            put(&sys, "class/net/wlan0/flags", "0x1003\n");
            put(&sys, "class/net/wlan0/carrier", "0\n");
            put(&sys, "class/net/wlan0/address", "aa:bb:cc:00:00:02\n");
            put(&sys, "class/net/wlan0/wireless/x", "");
            put(&sys, "class/net/wlan0/device/x", "");
            put(&sys, "class/rfkill/rfkill0/name", "phy0\n");
            put(&sys, "class/rfkill/rfkill0/type", "wlan\n");
            put(&sys, "class/rfkill/rfkill0/soft", "0\n");
            put(&sys, "class/rfkill/rfkill0/hard", "0\n");
            put(&sys, "class/rfkill/rfkill1/name", "hci0\n");
            put(&sys, "class/rfkill/rfkill1/type", "bluetooth\n");
            put(&sys, "class/rfkill/rfkill1/soft", "0\n");
            put(&sys, "class/rfkill/rfkill1/hard", "0\n");
            put(&sys, "class/graphics/fb0/virtual_size", "1920,1080\n");
            put(&sys, "class/graphics/fb0/bits_per_pixel", "32\n");
            put(&sys, "class/graphics/fb0/name", "simpledrmdrmfb\n");
            put(&sys, "class/drm/card0-HDMI-A-1/status", "connected\n");
            put(&sys, "class/drm/card0-HDMI-A-1/modes", "1920x1080\n1280x720\n");
            put(&sys, "class/backlight/bl0/max_brightness", "100\n");
            put(&sys, "class/backlight/bl0/brightness", "50\n");
            put(&proc_, "asound/cards", " 0 [PCH]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1 irq 3\n");
        }
        (sys.clone(), Roots { sys, proc_ })
    }

    fn pane(tag: &str, full: bool, fake: Arc<Fake>) -> (DevicesPane, Roots, Arc<Fake>) {
        let (_, roots) = machine(tag, full);
        (DevicesPane::new(DeviceEnv { roots: roots.clone(), ctl: fake.clone() }), roots, fake)
    }

    #[test]
    fn a_machine_with_nothing_shows_every_page_as_not_found_and_cannot_be_switched() {
        let (mut p, _, fake) = pane("empty", false, Arc::new(Fake::default()));
        for tab in [DevTab::Network, DevTab::Bluetooth, DevTab::Sound] {
            let rows = p.rows(tab, 0);
            assert!(rows.iter().all(|r| !r.actionable()), "{tab:?}");
            assert!(rows.iter().any(|r| r.chips.iter().any(|c| c.0 == "not found")), "{tab:?} says nothing was detected");
        }
        let net = p.rows(DevTab::Network, 0);
        assert_eq!(net.len(), 2, "one line each for Ethernet and WiFi");
        assert!(net[1].sub.contains("WiFi stays disabled"));
        assert!(p.act(DevTab::Network, 1, &Key::Enter, 0).unwrap().result.is_err(), "pressing Enter on a missing adapter explains why");
        assert!(p.act(DevTab::Network, 1, &Key::Char('x'), 0).is_none(), "other keys do nothing");
        assert!(fake.calls.lock().unwrap().is_empty(), "nothing was touched");
    }

    #[test]
    fn ethernet_and_wifi_rows_show_their_state() {
        let (mut p, _, _) = pane("net", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Network, 0);
        assert_eq!(rows.len(), 4, "the two adapters, then the private MAC address row of each");
        assert!(rows[2].title.contains("Private MAC") && rows[2].title.contains("eno1") && matches!(rows[2].target, Target::Mac(..)) && rows[3].title.contains("wlan0"), "{:?}", rows);
        assert!(rows[0].title.contains("eno1") && rows[0].sub.contains("192.168.1.9") && rows[0].sub.contains("1000 Mb/s"), "{:?}", rows[0]);
        assert_eq!(rows[0].chips[0].0, "ON");
        assert!(rows[1].title.contains("wlan0") && rows[1].sub.contains("not connected"), "{:?}", rows[1]);
    }

    #[test]
    fn switching_ethernet_off_and_on_goes_through_the_link_and_asks_for_an_address() {
        let (mut p, _, fake) = pane("eth", true, Arc::new(Fake::default()));
        let d = p.act(DevTab::Network, 0, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap(), "Ethernet eno1 turned off.");
        assert_eq!(d.audit, "ethernet eno1 off");
        assert_eq!(fake.calls.lock().unwrap().clone(), vec!["link eno1 down"]);
        // the (fake) kernel now reports it down
        put(&machine("eth", true).0, "class/net/eno1/flags", "0x1002\n");
        let rows = p.rows(DevTab::Network, 1);
        assert_eq!(rows[0].chips[0].0, "OFF");
        let d = p.act(DevTab::Network, 0, &Key::Char('e'), 1).unwrap();
        assert_eq!(d.result.unwrap(), "Ethernet eno1 turned on.");
        assert_eq!(fake.calls.lock().unwrap()[1..], ["link eno1 up", "dhcp eno1"]);
    }

    #[test]
    fn switching_wifi_off_blocks_the_radio_and_on_unblocks_it() {
        let (mut p, roots, fake) = pane("wifi", true, Arc::new(Fake::default()));
        let d = p.act(DevTab::Network, 1, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap(), "WiFi wlan0 turned off.");
        assert!(iyakki_devctl::rfkill_list(&roots).iter().find(|k| k.kind == "wlan").unwrap().soft, "radio blocked");
        put(&roots.sys, "class/net/wlan0/flags", "0x1002\n");
        let rows = p.rows(DevTab::Network, 1);
        assert_eq!(rows[1].chips[0].0, "OFF");
        let d = p.act(DevTab::Network, 1, &Key::Enter, 1).unwrap();
        let said = d.result.unwrap();
        // WiFi is private per network by default; this stand-in driver cannot change an address, which is said, not fatal
        assert!(said.starts_with("WiFi wlan0 turned on.") && said.contains("Private MAC address not applied") && said.contains("hardware address is used"), "{said}");
        assert!(!iyakki_devctl::rfkill_list(&roots).iter().find(|k| k.kind == "wlan").unwrap().soft, "radio free again");
        assert!(fake.calls.lock().unwrap().contains(&"dhcp wlan0".to_string()));
    }

    #[test]
    fn a_hardware_switch_cannot_be_overridden() {
        let (mut p, roots, _) = pane("hard", true, Arc::new(Fake::default()));
        put(&roots.sys, "class/rfkill/rfkill0/hard", "1\n");
        put(&roots.sys, "class/rfkill/rfkill1/hard", "1\n");
        for (tab, i) in [(DevTab::Network, 1), (DevTab::Bluetooth, 0)] {
            let rows = p.rows(tab, 0);
            assert!(!rows[i].actionable(), "{:?}", rows[i]);
            assert!(rows[i].sub.contains("switched off by a key"), "{:?}", rows[i]);
        }
    }

    #[test]
    fn a_failing_switch_reports_the_error_and_changes_nothing_else() {
        let (mut p, _, _) = pane("fail", true, Arc::new(Fake { fail_links: true, ..Fake::default() }));
        let d = p.act(DevTab::Network, 0, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap_err(), "refused");
        assert!(d.audit.is_empty(), "nothing changed, nothing to audit");
    }

    #[test]
    fn bluetooth_can_be_switched_and_detection_covers_a_usb_adapter_without_a_driver() {
        let (mut p, roots, _) = pane("bt", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Bluetooth, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].chips[0].0, "ON");
        let d = p.act(DevTab::Bluetooth, 0, &Key::Enter, 0).unwrap();
        assert!(d.result.unwrap().contains("turned off"));
        assert!(iyakki_devctl::rfkill_list(&roots).iter().find(|k| k.kind == "bluetooth").unwrap().soft);
        let (mut q, _, _) = {
            let (_, r) = machine("btusb", false);
            put(&r.sys, "bus/usb/devices/1-1/bDeviceClass", "e0\n");
            (DevicesPane::new(DeviceEnv { roots: r.clone(), ctl: Arc::new(Fake::default()) }), r, ())
        };
        let rows = q.rows(DevTab::Bluetooth, 0);
        assert!(rows[0].title.contains("USB") && !rows[0].actionable());
    }

    #[test]
    fn brightness_moves_in_steps_and_never_goes_dark() {
        let (mut p, roots, _) = pane("bl", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Display, 0);
        assert!(rows[0].sub.contains("1920x1080"));
        let bl = rows.iter().position(|r| matches!(r.target, Target::Backlight(_))).unwrap();
        assert!(rows.iter().any(|r| r.title.contains("HDMI-A-1") && r.sub.contains("best 1920x1080")));
        assert_eq!(p.act(DevTab::Display, bl, &Key::Char('+'), 0).unwrap().result.unwrap(), "Brightness 60%.");
        assert_eq!(iyakki_devctl::screen(&roots).backlights[0].value, 60);
        for t in 1..12 {
            p.act(DevTab::Display, bl, &Key::Char('-'), t);
        }
        assert_eq!(iyakki_devctl::screen(&roots).backlights[0].value, 5, "the floor is 5 %");
        assert!(p.act(DevTab::Display, 0, &Key::Char('+'), 20).unwrap().result.is_err(), "the screen row has no brightness");
    }

    #[test]
    fn volume_and_mute_go_to_the_card() {
        let fake = Arc::new(Fake::default());
        *fake.volume.lock().unwrap() = Some(iyakki_devctl::Volume { control: "Master".into(), percent: 50, muted: false });
        let (mut p, _, fake) = pane("snd", true, fake);
        let rows = p.rows(DevTab::Sound, 0);
        assert_eq!(rows.len(), 4, "the output, IYAKKI's volume, the card, the Advanced switch: {rows:?}");
        let c = rows.iter().position(|r| matches!(r.target, Target::Card(0))).unwrap();
        assert!(rows[c].title.contains("HDA Intel PCH") && rows[c].chips[0].0 == "50%");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('+'), 0).unwrap().result.unwrap(), "Volume 55%.");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('-'), 1).unwrap().result.unwrap(), "Volume 50%.");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('m'), 2).unwrap().result.unwrap(), "Muted.");
        assert_eq!(p.rows(DevTab::Sound, 3)[c].chips[0].0, "muted");
        assert_eq!(fake.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn the_card_carrying_iyakkis_software_volume_moves_the_iyakki_volume_and_others_keep_their_mixer() {
        let t2 = iyakki_devctl::Card { index: 1, name: "AppleT2x1".into(), long_name: "Apple T2 Audio".into() };
        let prefs = iyakki_agent::audioout::SoundSettings { volume_pct: 35, ..Default::default() };
        let soft = iyakki_devctl::Volume { control: iyakki_agent::softvol::CONTROL.into(), percent: 35, muted: false };
        // the Mac mini 2018's T2 card: its only control is the softvol, so the row is the IYAKKI volume (mute keeps the level)
        for v in [Some(&soft), None] {
            let r = card_row(&t2, v, Some(1), None, &prefs);
            assert!(matches!(r.target, Target::SoftVolume) && r.sub.contains("software volume"), "{r:?}");
            assert_eq!(r.slider.unwrap().value, 35);
            assert_eq!(r.chips[0].0, "35%");
        }
        let muted = iyakki_agent::audioout::SoundSettings { muted: true, ..prefs.clone() };
        assert_eq!(card_row(&t2, Some(&soft), Some(1), None, &muted).chips[0].0, "muted");
        // another card, or a card with a hardware control, keeps its own mixer
        let pch = iyakki_devctl::Card { index: 0, name: "HDA-Intel".into(), long_name: "HDA Intel PCH".into() };
        assert!(matches!(card_row(&pch, None, Some(1), None, &prefs).target, Target::Nothing));
        let master = iyakki_devctl::Volume { control: "Master".into(), percent: 70, muted: false };
        assert!(matches!(card_row(&pch, Some(&master), Some(0), None, &prefs).target, Target::Card(0)), "a hardware Master stays the card's own");
        assert!(matches!(card_row(&t2, Some(&soft), None, None, &prefs).target, Target::Card(1)), "without IYAKKI's softvol in use the control is a plain mixer");
    }

    #[test]
    fn a_macbook_with_a_cs8409_codec_and_no_driver_says_so_instead_of_an_empty_mixer() {
        let (mut p, roots, _) = pane("cs8409", true, Arc::new(Fake::default()));
        assert!(!p.rows(DevTab::Sound, 0).iter().any(|r| r.title == "Built-in speakers"), "another machine: no such row");
        put(&roots.sys, "class/dmi/id/product_name", "MacBookPro14,1\n");
        put(&roots.proc_, "asound/card0/codec#0", "Codec: Cirrus Logic Generic\nAddress: 0\nVendor Id: 0x10138409\nSubsystem Id: 0x106b3300\n");
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 0);
        let r = rows.iter().find(|r| r.title == "Built-in speakers").expect("the explaining row");
        assert!(r.sub.starts_with("This Mac's speakers need the CS8409 driver (not loaded)") && r.sub.contains("MacBookPro14,1"), "{}", r.sub);
        assert!(!r.actionable() && r.chips[0].0 == "no driver");
    }

    #[test]
    fn a_card_without_a_mixer_is_listed_but_not_adjustable() {
        let (mut p, _, _) = pane("nomix", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Sound, 0);
        let c = rows.iter().position(|r| r.title.contains("HDA Intel PCH")).unwrap();
        assert!(!rows[c].actionable() && rows[c].sub.contains("no volume control"));
    }

    #[test]
    fn the_hardware_is_looked_at_again_every_two_seconds_or_after_a_change() {
        let (mut p, roots, _) = pane("refresh", true, Arc::new(Fake::default()));
        assert_eq!(p.rows(DevTab::Network, 0)[0].chips[0].0, "ON");
        put(&roots.sys, "class/net/eno1/flags", "0x1002\n");
        assert_eq!(p.rows(DevTab::Network, 30)[0].chips[0].0, "ON", "within two seconds the picture is reused");
        assert_eq!(p.rows(DevTab::Network, 61)[0].chips[0].0, "OFF", "then it is refreshed");
    }

    #[test]
    fn display_and_sound_rows_carry_a_slider_and_a_click_on_it_sets_an_exact_value() {
        let fake = Arc::new(Fake::default());
        *fake.volume.lock().unwrap() = Some(iyakki_devctl::Volume { control: "Master".into(), percent: 50, muted: false });
        let (mut p, roots, fake) = pane("slide", true, fake);
        let rows = p.rows(DevTab::Display, 0);
        let bl = rows.iter().position(|r| matches!(r.target, Target::Backlight(_))).unwrap();
        assert_eq!(rows[bl].slider, Some(SliderVal { value: 50, min: 5, max: 100, step: 5, unit: "%" }));
        assert_eq!(p.set_value(DevTab::Display, bl, 85, 0).unwrap().result.unwrap(), "Brightness 85%.");
        assert_eq!(iyakki_devctl::screen(&roots).backlights[0].value, 85);
        assert_eq!(p.set_value(DevTab::Display, bl, 0, 1).unwrap().result.unwrap(), "Brightness 5%.", "the lowest brightness is the floor");
        assert!(p.set_value(DevTab::Display, 0, 50, 2).is_none(), "the screen row has no slider");
        let rows = p.rows(DevTab::Sound, 3);
        let c = rows.iter().position(|r| matches!(r.target, Target::Card(0))).unwrap();
        assert_eq!(rows[c].slider, Some(SliderVal { value: 50, min: 0, max: 100, step: 5, unit: "%" }));
        assert_eq!(p.set_value(DevTab::Sound, c, 30, 4).unwrap().result.unwrap(), "Volume 30%.");
        assert_eq!(p.set_value(DevTab::Sound, c, 999, 5).unwrap().result.unwrap(), "Volume 100%.", "values are kept in range");
        assert!(fake.calls.lock().unwrap().contains(&"set volume 0 30".to_string()));
        // a card without a mixer has no slider
        let (mut q, _, _) = pane("nomixer2", true, Arc::new(Fake::default()));
        let rows = q.rows(DevTab::Sound, 0);
        assert!(rows.iter().find(|r| r.title.contains("HDA Intel PCH")).unwrap().slider.is_none());
    }

    #[test]
    fn a_slow_starting_card_is_said_and_the_advanced_rows_come_and_go() {
        let (mut p, roots, _) = pane("slow", true, Arc::new(Fake::default()));
        put(&roots.sys, "class/dmi/id/product_name", "MacBookPro14,1\n");
        put(&roots.proc_, "asound/card0/codec#0", "Codec: Cirrus Logic CS8409\n");
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 0);
        let r = rows.iter().find(|r| r.title == "Built-in speakers" && r.chips.iter().any(|c| c.0 == "starting"));
        assert!(r.is_some_and(|r| r.sub.contains("waiting for the sound card") && r.sub.contains("start up slowly")), "{rows:?}");
        // hidden by default; A shows them; the stereo test is always there, the channel walk only for four channels
        assert!(!rows.iter().any(|r| matches!(r.target, Target::SoundPattern(_))));
        assert!(p.toggle_advanced());
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 1);
        assert!(rows.iter().any(|r| matches!(r.target, Target::SoundPattern(false))) || rows.iter().all(|r| !matches!(r.target, Target::SoundPattern(_))), "{rows:?}");
        assert!(!rows.iter().any(|r| matches!(r.target, Target::SoundPattern(true))), "no four-channel chain here");
        assert!(rows.iter().any(|r| matches!(r.target, Target::SoundAdvanced) && r.chips[0].0 == "shown"));
    }
}
