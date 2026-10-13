//! What the machine has (Ethernet, WiFi, Bluetooth, screens, sound) and how to switch it, for the Settings app.
//!
//! Detection reads `/sys` and `/proc` (all paths hang off [`Roots`], so tests use a fake machine). Switching uses the
//! kernel's own interfaces: `rfkill` files for radios, interface flags for links, backlight files for brightness and
//! the ALSA mixer (`amixer`) for volume. Nothing here is specific to one computer model.

use std::path::{Path, PathBuf};

pub mod wifi;

/// Where the kernel's views live. Real paths in production.
#[derive(Clone, Debug)]
pub struct Roots {
    pub sys: PathBuf,
    pub proc_: PathBuf,
}

impl Roots {
    pub fn real() -> Roots {
        Roots { sys: "/sys".into(), proc_: "/proc".into() }
    }
}

fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ radios (rfkill)

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rfkill {
    pub id: String,
    pub name: String,
    /// "wlan", "bluetooth", "wwan", ...
    pub kind: String,
    /// Switched off by software (what we control).
    pub soft: bool,
    /// Switched off by a hardware key or switch (we cannot undo it).
    pub hard: bool,
}

pub fn rfkill_list(r: &Roots) -> Vec<Rfkill> {
    dirs(&r.sys.join("class/rfkill"))
        .into_iter()
        .filter(|d| name_of(d).starts_with("rfkill"))
        .map(|d| Rfkill { id: name_of(&d), name: read(d.join("name")).unwrap_or_default(), kind: read(d.join("type")).unwrap_or_default(), soft: read(d.join("soft")).as_deref() == Some("1"), hard: read(d.join("hard")).as_deref() == Some("1") })
        .collect()
}

pub fn rfkill_set(r: &Roots, id: &str, blocked: bool) -> Result<(), String> {
    if !id.starts_with("rfkill") || id.contains('/') {
        return Err(format!("not a radio switch: {id}"));
    }
    std::fs::write(r.sys.join("class/rfkill").join(id).join("soft"), if blocked { "1" } else { "0" }).map_err(|e| format!("could not switch {id}: {e}"))
}

// ------------------------------------------------------------------------------------------------ network links

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Ethernet,
    Wifi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub kind: LinkKind,
    /// The interface is up (administratively).
    pub up: bool,
    /// A cable is plugged in / the radio is associated.
    pub carrier: bool,
    pub mac: String,
    /// Link speed in Mb/s, when known.
    pub speed: Option<u32>,
    pub driver: String,
}

/// Real network adapters (not loopback, bridges, tunnels or other software devices).
pub fn links(r: &Roots) -> Vec<Link> {
    let mut out = Vec::new();
    for d in dirs(&r.sys.join("class/net")) {
        let name = name_of(&d);
        if name == "lo" || !d.join("device").exists() {
            continue; // software interfaces have no device behind them
        }
        let wifi = d.join("wireless").exists() || d.join("phy80211").exists();
        if !wifi && read(d.join("type")).as_deref() != Some("1") {
            continue;
        }
        let flags = read(d.join("flags")).and_then(|f| u32::from_str_radix(f.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
        let driver = std::fs::read_link(d.join("device/driver")).ok().map(|p| name_of(&p)).unwrap_or_default();
        out.push(Link {
            name,
            kind: if wifi { LinkKind::Wifi } else { LinkKind::Ethernet },
            up: flags & 1 != 0,
            carrier: read(d.join("carrier")).as_deref() == Some("1"),
            mac: read(d.join("address")).unwrap_or_default(),
            speed: read(d.join("speed")).and_then(|s| s.parse::<i64>().ok()).filter(|s| *s > 0).map(|s| s as u32),
            driver,
        });
    }
    out
}

#[repr(C)]
struct IfReq {
    name: [u8; 16],
    flags: libc::c_short,
    pad: [u8; 22],
}

const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
const IFF_UP: libc::c_short = 1;

/// Bring an interface up or down (what `ip link set dev X up|down` does).
pub fn set_link(name: &str, up: bool) -> Result<(), String> {
    if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
        return Err(format!("not an interface name: {name:?}"));
    }
    let mut req = IfReq { name: [0; 16], flags: 0, pad: [0; 22] };
    req.name[..name.len()].copy_from_slice(name.as_bytes());
    // SAFETY: a plain datagram socket used only for the two interface-flag requests below; `req` matches the kernel's
    // `struct ifreq` (16-byte name, then a union of which we use the flags) and outlives both calls.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(format!("socket: {}", std::io::Error::last_os_error()));
        }
        let res = (|| {
            if libc::ioctl(fd, SIOCGIFFLAGS as _, &mut req as *mut IfReq) < 0 {
                return Err(format!("{name}: {}", std::io::Error::last_os_error()));
            }
            req.flags = if up { req.flags | IFF_UP } else { req.flags & !IFF_UP };
            if libc::ioctl(fd, SIOCSIFFLAGS as _, &mut req as *mut IfReq) < 0 {
                return Err(format!("{name}: {}", std::io::Error::last_os_error()));
            }
            Ok(())
        })();
        libc::close(fd);
        res
    }
}

/// IPv4 addresses of every interface, as `(name, address)`.
pub fn ipv4_addresses() -> Vec<(String, String)> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs allocates a list that freeifaddrs releases; every pointer read is checked for null first and
    // only used while the list is alive.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut p = head;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null() && (*a.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                let ip = u32::from_be(sin.sin_addr.s_addr);
                let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().to_string();
                out.push((name, format!("{}.{}.{}.{}", ip >> 24, (ip >> 16) & 255, (ip >> 8) & 255, ip & 255)));
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

// ------------------------------------------------------------------------------------------------ Bluetooth

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bluetooth {
    /// Controllers the kernel has set up (`hci0`...).
    pub adapters: Vec<String>,
    /// A USB Bluetooth device is plugged in (class e0), whether or not a driver took it.
    pub usb_present: bool,
    pub switches: Vec<Rfkill>,
}

pub fn bluetooth(r: &Roots) -> Bluetooth {
    let adapters = dirs(&r.sys.join("class/bluetooth")).iter().map(|d| name_of(d)).filter(|n| n.starts_with("hci") && !n.contains(':')).collect();
    let usb_present = dirs(&r.sys.join("bus/usb/devices")).iter().any(|d| read(d.join("bDeviceClass")).as_deref() == Some("e0"));
    let switches = rfkill_list(r).into_iter().filter(|k| k.kind == "bluetooth").collect();
    Bluetooth { adapters, usb_present, switches }
}

impl Bluetooth {
    /// Something Bluetooth is there to switch.
    pub fn detected(&self) -> bool {
        !self.adapters.is_empty() || !self.switches.is_empty() || self.usb_present
    }
}

// ------------------------------------------------------------------------------------------------ display

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// "HDMI-A-1", "eDP-1", ...
    pub name: String,
    pub connected: bool,
    /// Modes the screen offers, best first ("3840x2160").
    pub modes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    /// The framebuffer the desktop draws on, e.g. "3840x2160 · 32 bit · i915drmfb".
    pub framebuffer: Option<String>,
    pub outputs: Vec<Output>,
    pub backlights: Vec<Backlight>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlight {
    pub name: String,
    pub value: u32,
    pub max: u32,
}

impl Backlight {
    pub fn percent(&self) -> u32 {
        if self.max == 0 { 0 } else { (self.value * 100 + self.max / 2) / self.max }
    }
}

pub fn screen(r: &Roots) -> Screen {
    let framebuffer = {
        let fb = r.sys.join("class/graphics/fb0");
        read(fb.join("virtual_size")).map(|sz| format!("{} · {} bit · {}", sz.replace(',', "x"), read(fb.join("bits_per_pixel")).unwrap_or_else(|| "?".into()), read(fb.join("name")).unwrap_or_else(|| "framebuffer".into())))
    };
    let mut outputs = Vec::new();
    for d in dirs(&r.sys.join("class/drm")) {
        let n = name_of(&d);
        // connector directories are named cardN-<connector>
        let Some((_, conn)) = n.split_once('-') else { continue };
        if !n.starts_with("card") || read(d.join("status")).is_none() {
            continue;
        }
        let connected = read(d.join("status")).as_deref() == Some("connected");
        let modes: Vec<String> = read(d.join("modes")).map(|m| m.lines().map(str::to_string).collect()).unwrap_or_default();
        outputs.push(Output { name: conn.to_string(), connected, modes });
    }
    let backlights = dirs(&r.sys.join("class/backlight"))
        .into_iter()
        .filter_map(|d| {
            let max = read(d.join("max_brightness"))?.parse().ok()?;
            let value = read(d.join("brightness"))?.parse().ok()?;
            Some(Backlight { name: name_of(&d), value, max })
        })
        .collect();
    Screen { framebuffer, outputs, backlights }
}

/// Set the brightness to `percent` (never fully dark: at least 5 %).
pub fn set_brightness(r: &Roots, name: &str, percent: u32) -> Result<(), String> {
    if name.contains('/') || name.is_empty() {
        return Err(format!("not a backlight: {name}"));
    }
    let dir = r.sys.join("class/backlight").join(name);
    let max: u32 = read(dir.join("max_brightness")).and_then(|m| m.parse().ok()).ok_or_else(|| format!("{name}: no brightness control"))?;
    let v = (max as u64 * percent.clamp(5, 100) as u64 / 100).max(1) as u32;
    std::fs::write(dir.join("brightness"), v.to_string()).map_err(|e| format!("could not set the brightness: {e}"))
}

// ------------------------------------------------------------------------------------------------ sound

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub index: u32,
    pub name: String,
    pub long_name: String,
}

/// Sound cards from `/proc/asound/cards`:
/// ```text
///  0 [PCH            ]: HDA-Intel - HDA Intel PCH
///                       HDA Intel PCH at 0xf7f10000 irq 32
/// ```
pub fn cards(r: &Roots) -> Vec<Card> {
    let text = read(r.proc_.join("asound/cards")).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (idx, rest) = l.trim_start().split_once(' ')?;
            let index: u32 = idx.parse().ok()?;
            let (_, after) = rest.split_once("]:")?;
            let (name, long) = after.split_once(" - ").unwrap_or((after, ""));
            Some(Card { index, name: name.trim().to_string(), long_name: long.trim().to_string() })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub control: String,
    pub percent: u32,
    pub muted: bool,
}

/// Parse `amixer sget` output: `Front Left: Playback 40 [62%] [-20.00dB] [on]`.
pub fn parse_amixer(control: &str, out: &str) -> Option<Volume> {
    let line = out.lines().find(|l| l.contains('[') && l.contains('%'))?;
    let pct = line.split('[').find_map(|p| p.split_once("%]").map(|(a, _)| a))?.trim().parse().ok()?;
    let muted = line.contains("[off]");
    Some(Volume { control: control.to_string(), percent: pct, muted })
}

const CONTROLS: [&str; 4] = ["Master", "PCM", "Speaker", "Headphone"];

fn amixer(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("amixer").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// The playback volume of a card (the first of the usual controls it has).
pub fn volume(card: u32) -> Option<Volume> {
    let c = card.to_string();
    CONTROLS.iter().find_map(|ctl| parse_amixer(ctl, &amixer(&["-c", &c, "sget", ctl])?))
}

/// Change the volume by `delta` percent points, or toggle mute when `delta` is `None`.
pub fn change_volume(card: u32, delta: Option<i32>) -> Result<Volume, String> {
    let cur = volume(card).ok_or("this card has no volume control")?;
    let c = card.to_string();
    let arg = match delta {
        Some(d) => format!("{}%{}", d.unsigned_abs(), if d < 0 { "-" } else { "+" }),
        None => "toggle".into(),
    };
    amixer(&["-c", &c, "sset", &cur.control, &arg]).ok_or("the mixer did not accept the change")?;
    volume(card).ok_or_else(|| "could not read the volume back".into())
}

/// Set the playback volume of a card to `percent` (0..=100).
pub fn set_volume(card: u32, percent: u32) -> Result<Volume, String> {
    let cur = volume(card).ok_or("this card has no volume control")?;
    let c = card.to_string();
    amixer(&["-c", &c, "sset", &cur.control, &format!("{}%", percent.min(100))]).ok_or("the mixer did not accept the change")?;
    volume(card).ok_or_else(|| "could not read the volume back".into())
}

// ------------------------------------------------------------------------------------------------ acting on the machine

/// The actions that change the real machine, behind a trait so tests can use a fake one.
pub trait Control: Send + Sync {
    fn set_link(&self, name: &str, up: bool) -> Result<(), String>;
    fn ipv4(&self) -> Vec<(String, String)>;
    fn volume(&self, card: u32) -> Option<Volume>;
    fn change_volume(&self, card: u32, delta: Option<i32>) -> Result<Volume, String>;
    /// Set the volume to an exact percentage.
    fn set_volume(&self, card: u32, percent: u32) -> Result<Volume, String>;
    /// Ask for an address on this link (DHCP), in the background.
    fn dhcp(&self, name: &str) -> Result<(), String>;
}

/// The real machine.
pub struct System;

impl Control for System {
    fn set_link(&self, name: &str, up: bool) -> Result<(), String> {
        set_link(name, up)
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        ipv4_addresses()
    }
    fn volume(&self, card: u32) -> Option<Volume> {
        volume(card)
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> Result<Volume, String> {
        change_volume(card, delta)
    }
    fn set_volume(&self, card: u32, percent: u32) -> Result<Volume, String> {
        set_volume(card, percent)
    }
    fn dhcp(&self, name: &str) -> Result<(), String> {
        if name.is_empty() || name.len() > 15 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
            return Err(format!("not an interface name: {name:?}"));
        }
        std::process::Command::new("udhcpc").args(["-b", "-R", "-i", name, "-p", &format!("/run/udhcpc-{name}.pid")]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("could not start DHCP on {name}: {e}"))
    }
}

// ------------------------------------------------------------------------------------------------ summary used by the UI

/// Whether a radio of `kind` ("wlan", "bluetooth") is on, judging by its switches (no switch = not switchable = on).
pub fn radio_on(switches: &[Rfkill]) -> bool {
    switches.iter().all(|k| !k.soft && !k.hard)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-devctl-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn fake(tag: &str) -> Roots {
        let d = tmp(tag);
        Roots { sys: d.join("sys"), proc_: d.join("proc") }
    }

    #[test]
    fn ethernet_and_wifi_are_told_apart_and_software_devices_ignored() {
        let r = fake("net");
        // a real Ethernet card, cable plugged in, up
        put(&r.sys, "class/net/eno1/type", "1\n");
        put(&r.sys, "class/net/eno1/flags", "0x1003\n");
        put(&r.sys, "class/net/eno1/carrier", "1\n");
        put(&r.sys, "class/net/eno1/address", "aa:bb:cc:dd:ee:01\n");
        put(&r.sys, "class/net/eno1/speed", "1000\n");
        put(&r.sys, "class/net/eno1/device/x", "");
        // a WiFi adapter, down
        put(&r.sys, "class/net/wlan0/type", "1\n");
        put(&r.sys, "class/net/wlan0/flags", "0x1002\n");
        put(&r.sys, "class/net/wlan0/carrier", "0\n");
        put(&r.sys, "class/net/wlan0/wireless/x", "");
        put(&r.sys, "class/net/wlan0/device/x", "");
        // software devices: loopback, a bridge (no device behind it)
        put(&r.sys, "class/net/lo/type", "772\n");
        put(&r.sys, "class/net/br0/type", "1\n");
        let l = links(&r);
        assert_eq!(l.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["eno1", "wlan0"]);
        assert_eq!((l[0].kind, l[0].up, l[0].carrier, l[0].speed), (LinkKind::Ethernet, true, true, Some(1000)));
        assert_eq!((l[1].kind, l[1].up, l[1].carrier, l[1].speed), (LinkKind::Wifi, false, false, None));
        assert_eq!(l[0].mac, "aa:bb:cc:dd:ee:01");
    }

    #[test]
    fn a_machine_without_adapters_lists_nothing() {
        let r = fake("none");
        assert!(links(&r).is_empty());
        assert!(rfkill_list(&r).is_empty());
        assert!(!bluetooth(&r).detected());
        assert!(cards(&r).is_empty());
        assert_eq!(screen(&r), Screen { framebuffer: None, outputs: vec![], backlights: vec![] });
    }

    #[test]
    fn radios_are_read_and_switched_through_rfkill() {
        let r = fake("rf");
        put(&r.sys, "class/rfkill/rfkill0/name", "phy0\n");
        put(&r.sys, "class/rfkill/rfkill0/type", "wlan\n");
        put(&r.sys, "class/rfkill/rfkill0/soft", "0\n");
        put(&r.sys, "class/rfkill/rfkill0/hard", "0\n");
        put(&r.sys, "class/rfkill/rfkill1/name", "hci0\n");
        put(&r.sys, "class/rfkill/rfkill1/type", "bluetooth\n");
        put(&r.sys, "class/rfkill/rfkill1/soft", "1\n");
        put(&r.sys, "class/rfkill/rfkill1/hard", "0\n");
        let l = rfkill_list(&r);
        assert_eq!(l.len(), 2);
        assert!(!l[0].soft && l[1].soft && l[1].kind == "bluetooth");
        rfkill_set(&r, "rfkill1", false).unwrap();
        assert!(!rfkill_list(&r)[1].soft, "switched on");
        rfkill_set(&r, "rfkill0", true).unwrap();
        assert!(rfkill_list(&r)[0].soft, "switched off");
        assert!(rfkill_set(&r, "../etc", true).is_err() && rfkill_set(&r, "rfkill0/x", true).is_err(), "only real switch names");
        assert!(radio_on(&[l[0].clone()]) && !radio_on(&[l[1].clone()]) && radio_on(&[]));
        assert!(!radio_on(&[Rfkill { hard: true, ..l[0].clone() }]), "a hardware switch counts as off");
    }

    #[test]
    fn bluetooth_is_detected_from_the_adapter_the_switch_or_a_usb_device() {
        let r = fake("bt1");
        put(&r.sys, "bus/usb/devices/1-1/bDeviceClass", "e0\n");
        let b = bluetooth(&r);
        assert!(b.detected() && b.usb_present && b.adapters.is_empty(), "plugged in, no driver yet");
        let r = fake("bt2");
        put(&r.sys, "class/bluetooth/hci0/x", "");
        put(&r.sys, "class/bluetooth/hci0:12/x", "");
        let b = bluetooth(&r);
        assert_eq!(b.adapters, vec!["hci0"], "connections are not adapters");
        let r = fake("bt3");
        put(&r.sys, "bus/usb/devices/1-1/bDeviceClass", "09\n");
        assert!(!bluetooth(&r).detected(), "a hub is not Bluetooth");
    }

    #[test]
    fn screens_modes_and_brightness() {
        let r = fake("scr");
        put(&r.sys, "class/graphics/fb0/virtual_size", "3840,2160\n");
        put(&r.sys, "class/graphics/fb0/bits_per_pixel", "32\n");
        put(&r.sys, "class/graphics/fb0/name", "i915drmfb\n");
        put(&r.sys, "class/drm/card0-HDMI-A-1/status", "connected\n");
        put(&r.sys, "class/drm/card0-HDMI-A-1/modes", "3840x2160\n1920x1080\n1280x720\n");
        put(&r.sys, "class/drm/card0-DP-1/status", "disconnected\n");
        put(&r.sys, "class/drm/card0-DP-1/modes", "");
        put(&r.sys, "class/drm/card0/dev", "226:0\n");
        put(&r.sys, "class/backlight/intel_backlight/max_brightness", "1000\n");
        put(&r.sys, "class/backlight/intel_backlight/brightness", "400\n");
        let s = screen(&r);
        assert_eq!(s.framebuffer.as_deref(), Some("3840x2160 · 32 bit · i915drmfb"));
        assert_eq!(s.outputs.len(), 2, "the card itself is not an output");
        let hdmi = s.outputs.iter().find(|o| o.name == "HDMI-A-1").unwrap();
        assert!(hdmi.connected && hdmi.modes[0] == "3840x2160" && hdmi.modes.len() == 3);
        assert!(!s.outputs.iter().find(|o| o.name == "DP-1").unwrap().connected);
        assert_eq!(s.backlights[0].percent(), 40);
        set_brightness(&r, "intel_backlight", 80).unwrap();
        assert_eq!(screen(&r).backlights[0].value, 800);
        set_brightness(&r, "intel_backlight", 0).unwrap();
        assert_eq!(screen(&r).backlights[0].value, 50, "never fully dark");
        assert!(set_brightness(&r, "../x", 50).is_err());
        assert!(set_brightness(&r, "nope", 50).is_err());
    }

    #[test]
    fn sound_cards_and_mixer_output_are_parsed() {
        let r = fake("snd");
        put(&r.proc_, "asound/cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7f10000 irq 32\n 1 [Device         ]: USB-Audio - USB Audio Device\n                      C-Media USB Audio Device at usb-0000:00:14.0-2\n");
        let c = cards(&r);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].index, c[0].name.as_str(), c[0].long_name.as_str()), (0, "HDA-Intel", "HDA Intel PCH"));
        assert_eq!(c[1].index, 1);
        let out = "Simple mixer control 'Master',0\n  Capabilities: pvolume pswitch\n  Front Left: Playback 40 [62%] [-20.00dB] [on]\n  Front Right: Playback 40 [62%] [-20.00dB] [on]\n";
        assert_eq!(parse_amixer("Master", out), Some(Volume { control: "Master".into(), percent: 62, muted: false }));
        let muted = "  Mono: Playback 10 [100%] [off]\n";
        assert_eq!(parse_amixer("PCM", muted), Some(Volume { control: "PCM".into(), percent: 100, muted: true }));
        assert_eq!(parse_amixer("Master", "no controls here"), None);
    }

    #[test]
    fn link_names_are_checked_before_the_kernel_is_asked() {
        assert!(set_link("", true).is_err());
        assert!(set_link("a/b", true).is_err());
        assert!(set_link("abcdefghijklmnop", true).is_err(), "longer than an interface name can be");
        assert!(set_link("nonexistent0", true).is_err(), "the kernel refuses an unknown interface");
    }

    #[test]
    fn the_loopback_address_is_found() {
        assert!(ipv4_addresses().iter().any(|(n, a)| n == "lo" && a == "127.0.0.1"));
    }
}
