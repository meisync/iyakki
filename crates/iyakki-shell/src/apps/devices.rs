//! The hardware pages of Settings: Network (Ethernet, WiFi), Bluetooth, Display and Sound.
//!
//! Each page lists what the machine actually has: an adapter that is not detected is shown as such and cannot be
//! switched. Detection and switching live in `iyakki-devctl`; this file turns that into rows and key actions, and has
//! no drawing code so it can be tested against a fake machine.

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
        DeviceEnv { roots: Roots::real(), ctl: Arc::new(iyakki_devctl::System) }
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
}

pub struct DevicesPane {
    env: DeviceEnv,
    snap: Option<Snapshot>,
    at: u64,
}

impl DevicesPane {
    pub fn new(env: DeviceEnv) -> DevicesPane {
        DevicesPane { env, snap: None, at: 0 }
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
        self.snap = Some(Snapshot { links: iyakki_devctl::links(r), ips: self.env.ctl.ipv4(), rfkill: iyakki_devctl::rfkill_list(r), bt: iyakki_devctl::bluetooth(r), screen: iyakki_devctl::screen(r), cards: cards.collect() });
        self.at = tick;
    }

    fn snap(&mut self, tick: u64) -> &Snapshot {
        if self.snap.is_none() || tick.saturating_sub(self.at) >= REFRESH_TICKS {
            self.refresh(tick);
        }
        self.snap.as_ref().expect("just refreshed")
    }

    pub fn rows(&mut self, tab: DevTab, tick: u64) -> Vec<DevRow> {
        let s = self.snap(tick);
        match tab {
            DevTab::Network => network_rows(s),
            DevTab::Bluetooth => bluetooth_rows(s),
            DevTab::Display => display_rows(s),
            DevTab::Sound => sound_rows(s),
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
        let v = value.clamp(slider.min, slider.max);
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
        let r = if on { self.env.ctl.set_link(name, false) } else { self.env.ctl.set_link(name, true).and_then(|_| self.env.ctl.dhcp(name)) };
        match r {
            Ok(()) => Done { result: Ok(format!("Ethernet {name} turned {}.", if on { "off" } else { "on" })), audit: format!("ethernet {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    fn toggle_wifi(&mut self, name: &str, ids: &[String], row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        let res = (|| -> Result<(), String> {
            if on {
                self.env.ctl.set_link(name, false)?;
                ids.iter().try_for_each(|id| iyakki_devctl::rfkill_set(&self.env.roots, id, true))
            } else {
                ids.iter().try_for_each(|id| iyakki_devctl::rfkill_set(&self.env.roots, id, false))?;
                self.env.ctl.set_link(name, true)?;
                self.env.ctl.dhcp(name)
            }
        })();
        match res {
            Ok(()) => Done { result: Ok(format!("WiFi {name} turned {}.", if on { "off" } else { "on" })), audit: format!("wifi {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
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

fn network_rows(s: &Snapshot) -> Vec<DevRow> {
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
    rows
}

fn bluetooth_rows(s: &Snapshot) -> Vec<DevRow> {
    let mut rows = Vec::new();
    for k in s.bt.switches.iter() {
        let on = !k.soft && !k.hard;
        let (t, c) = onoff(on);
        let state = if k.hard { "switched off by a key or switch on the machine" } else if on { "on" } else { "off" };
        let mut row = DevRow::new(format!("Bluetooth · {}", if k.name.is_empty() { &k.id } else { &k.name }), state, if k.hard { Target::Nothing } else { Target::Radio(vec![k.id.clone()]) }).chip(t, c);
        if k.hard {
            row = row.chip("hardware off", WARN);
        }
        rows.push(row);
    }
    if rows.is_empty() {
        for a in &s.bt.adapters {
            rows.push(DevRow::new(format!("Bluetooth · {a}"), "on (this adapter has no software switch)", Target::Nothing).chip("ON", OK));
        }
    }
    if rows.is_empty() && s.bt.usb_present {
        rows.push(DevRow::new("Bluetooth adapter (USB)", "found, but no driver has taken it yet", Target::Nothing).chip("no driver", WARN));
    }
    if rows.is_empty() {
        rows.push(DevRow::new("Bluetooth", "no adapter detected, so Bluetooth stays disabled", Target::Nothing).chip("not found", DIM));
    }
    rows
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

fn sound_rows(s: &Snapshot) -> Vec<DevRow> {
    let mut rows = Vec::new();
    for (c, v) in &s.cards {
        let title = if c.long_name.is_empty() { c.name.clone() } else { c.long_name.clone() };
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
        rows.push(row);
    }
    if rows.is_empty() {
        rows.push(DevRow::new("Sound", "no sound card detected", Target::Nothing).chip("not found", DIM));
    }
    rows
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
        assert_eq!(rows.len(), 2);
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
        assert_eq!(d.result.unwrap(), "WiFi wlan0 turned on.");
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
        assert_eq!(rows.len(), 1);
        assert!(rows[0].title.contains("HDA Intel PCH") && rows[0].chips[0].0 == "50%");
        assert_eq!(p.act(DevTab::Sound, 0, &Key::Char('+'), 0).unwrap().result.unwrap(), "Volume 55%.");
        assert_eq!(p.act(DevTab::Sound, 0, &Key::Char('-'), 1).unwrap().result.unwrap(), "Volume 50%.");
        assert_eq!(p.act(DevTab::Sound, 0, &Key::Char('m'), 2).unwrap().result.unwrap(), "Muted.");
        assert_eq!(p.rows(DevTab::Sound, 3)[0].chips[0].0, "muted");
        assert_eq!(fake.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_card_without_a_mixer_is_listed_but_not_adjustable() {
        let (mut p, _, _) = pane("nomix", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Sound, 0);
        assert!(!rows[0].actionable() && rows[0].sub.contains("no volume control"));
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
        assert_eq!(rows[0].slider, Some(SliderVal { value: 50, min: 0, max: 100, step: 5, unit: "%" }));
        assert_eq!(p.set_value(DevTab::Sound, 0, 30, 4).unwrap().result.unwrap(), "Volume 30%.");
        assert_eq!(p.set_value(DevTab::Sound, 0, 999, 5).unwrap().result.unwrap(), "Volume 100%.", "values are kept in range");
        assert!(fake.calls.lock().unwrap().contains(&"set volume 0 30".to_string()));
        // a card without a mixer has no slider
        let (mut q, _, _) = pane("nomixer2", true, Arc::new(Fake::default()));
        assert!(q.rows(DevTab::Sound, 0)[0].slider.is_none());
    }
}
