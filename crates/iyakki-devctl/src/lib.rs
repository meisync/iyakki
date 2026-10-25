//! What the machine has (Ethernet, WiFi, Bluetooth, screens, sound) and how to switch it, for the Settings app.
//!
//! Detection reads `/sys` and `/proc` (all paths hang off [`Roots`], so tests use a fake machine). Switching uses the
//! kernel's own interfaces: `rfkill` files for radios, interface flags for links, backlight files for brightness and
//! the ALSA mixer (`amixer`) for volume. Nothing here is specific to one computer model.

use std::path::{Path, PathBuf};

pub mod btmgmt;
pub mod btview;
pub mod direct;
pub mod mac;
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

#[cfg(target_os = "linux")]
#[repr(C)]
struct IfReq {
    name: [u8; 16],
    flags: libc::c_short,
    pad: [u8; 22],
}

#[cfg(target_os = "linux")]
const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
#[cfg(target_os = "linux")]
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
#[cfg(target_os = "linux")]
const IFF_UP: libc::c_short = 1;

/// Interface flag requests (`SIOCSIFFLAGS`) and `SOCK_CLOEXEC` are the Linux ones; the Mac manages its own network.
#[cfg(not(target_os = "linux"))]
pub fn set_link(name: &str, _up: bool) -> Result<(), String> {
    Err(format!("{name}: changing a network link is not available on this OS"))
}

#[cfg(target_os = "linux")]
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

/// Hardware (MAC) address requests: read the adapter's permanent address (`SIOCETHTOOL`/`ETHTOOL_GPERMADDR`) and set a new
/// one (`SIOCSIFHWADDR`, needs the link down and administrator rights). Never called by tests on a real interface.
#[cfg(target_os = "linux")]
mod hw {
    use super::mac::{fmt_mac, parse_mac, valid_unicast};

    const SIOCSIFHWADDR: libc::c_ulong = 0x8924;
    const SIOCETHTOOL: libc::c_ulong = 0x8946;
    const ETHTOOL_GPERMADDR: u32 = 0x20;
    const ARPHRD_ETHER: u16 = 1;

    #[repr(C)]
    struct IfReqAddr {
        name: [u8; 16],
        family: u16,
        data: [u8; 14],
        pad: [u8; 8],
    }

    #[repr(C)]
    struct IfReqPtr {
        name: [u8; 16],
        data: *mut libc::c_void,
        pad: [u8; 16],
    }

    #[repr(C)]
    struct PermAddr {
        cmd: u32,
        size: u32,
        data: [u8; 32],
    }

    fn name_ok(name: &str) -> bool {
        !name.is_empty() && name.len() <= 15 && !name.contains('/') && !name.bytes().any(|b| b == 0)
    }

    fn name_bytes(name: &str) -> [u8; 16] {
        let mut n = [0u8; 16];
        n[..name.len()].copy_from_slice(name.as_bytes());
        n
    }

    pub fn permanent(name: &str) -> Option<String> {
        if !name_ok(name) {
            return None;
        }
        let mut perm = PermAddr { cmd: ETHTOOL_GPERMADDR, size: 32, data: [0; 32] };
        let mut req = IfReqPtr { name: name_bytes(name), data: (&mut perm as *mut PermAddr).cast(), pad: [0; 16] };
        // SAFETY: a datagram socket used for one read-only ethtool request; `req` is laid out like the kernel's `struct ifreq`
        // (16-byte name, then a union of which the data pointer is used) and points at `perm`, which has room for the 32
        // address bytes announced in `size`; both outlive the call.
        let ok = unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return None;
            }
            let r = libc::ioctl(fd, SIOCETHTOOL as _, &mut req as *mut IfReqPtr);
            libc::close(fd);
            r >= 0
        };
        if !ok || perm.size != 6 {
            return None;
        }
        let m = [perm.data[0], perm.data[1], perm.data[2], perm.data[3], perm.data[4], perm.data[5]];
        // an adapter without a stored address reports zeros
        valid_unicast(&m).then(|| fmt_mac(&m))
    }

    pub fn set(name: &str, mac: &str) -> Result<(), String> {
        if !name_ok(name) {
            return Err(format!("not an interface name: {name:?}"));
        }
        let m = parse_mac(mac).filter(valid_unicast).ok_or_else(|| format!("{mac}: not a valid unicast MAC address"))?;
        let mut req = IfReqAddr { name: name_bytes(name), family: ARPHRD_ETHER, data: [0; 14], pad: [0; 8] };
        req.data[..6].copy_from_slice(&m);
        // SAFETY: a datagram socket used for the one address request; `req` matches the kernel's `struct ifreq` with the
        // `ifr_hwaddr` (`struct sockaddr`: 2-byte family, 14 bytes of address) member and outlives the call.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(format!("socket: {}", std::io::Error::last_os_error()));
            }
            let r = libc::ioctl(fd, SIOCSIFHWADDR as _, &mut req as *mut IfReqAddr);
            let err = std::io::Error::last_os_error();
            libc::close(fd);
            if r >= 0 {
                return Ok(());
            }
            Err(match err.raw_os_error() {
                Some(libc::EPERM) | Some(libc::EACCES) => format!("{name}: changing the address needs administrator rights"),
                Some(libc::EBUSY) => format!("{name}: the adapter is busy or still up, so its address cannot change now"),
                Some(libc::EOPNOTSUPP) | Some(524) | Some(libc::EINVAL) | Some(libc::EADDRNOTAVAIL) => format!("{name}: not supported by this driver ({err})"),
                _ => format!("{name}: {err}"),
            })
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod hw {
    pub fn permanent(_name: &str) -> Option<String> {
        None
    }
    pub fn set(name: &str, _mac: &str) -> Result<(), String> {
        Err(format!("{name}: the MAC address is managed by the host operating system"))
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
    /// Controllers that exist but never started: the kernel creates `hciN` first and the address only after the
    /// controller answered its reset (UART controllers that stay silent, missing firmware).
    pub unstarted: Vec<String>,
}

pub fn bluetooth(r: &Roots) -> Bluetooth {
    let adapters: Vec<String> = dirs(&r.sys.join("class/bluetooth")).iter().map(|d| name_of(d)).filter(|n| n.starts_with("hci") && !n.contains(':')).collect();
    let usb_present = dirs(&r.sys.join("bus/usb/devices")).iter().any(|d| read(d.join("bDeviceClass")).as_deref() == Some("e0"));
    let switches = rfkill_list(r).into_iter().filter(|k| k.kind == "bluetooth").collect();
    let unstarted: Vec<String> = adapters.iter().filter(|a| !r.sys.join("class/bluetooth").join(a).join("address").exists()).cloned().collect();
    Bluetooth { adapters, usb_present, switches, unstarted }
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

/// The controls that set a card's playback volume, in the order they are looked for. `IYAKKI` is the software volume IYAKKI
/// adds (ALSA softvol, `iyakki_agent::softvol`) to a card that has none, such as the HDMI devices of Intel graphics and the
/// T2 Mac's `Apple T2 Audio`; a hardware control comes first.
pub const CONTROLS: [&str; 5] = ["Master", "PCM", "Speaker", "Headphone", "IYAKKI"];

fn amixer(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("amixer").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// A control without a mute switch (`[on]`/`[off]`), such as a softvol: muting it sets 0 % and remembers the level here.
#[derive(Default)]
pub struct Mutes(std::sync::Mutex<std::collections::HashMap<(u32, String), u32>>);

impl Mutes {
    fn get(&self, card: u32, control: &str) -> Option<u32> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&(card, control.to_string())).copied()
    }
    fn set(&self, card: u32, control: &str, level: Option<u32>) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match level {
            Some(l) => m.insert((card, control.to_string()), l),
            None => m.remove(&(card, control.to_string())),
        };
    }
}

static MUTES: std::sync::OnceLock<Mutes> = std::sync::OnceLock::new();

fn mutes() -> &'static Mutes {
    MUTES.get_or_init(Mutes::default)
}

/// Runs `amixer` with these arguments (its output, None when it failed). Real or fake.
pub type Amixer<'a> = &'a dyn Fn(&[&str]) -> Option<String>;

/// Does this `amixer sget` output show a mute switch?
fn has_switch(out: &str) -> bool {
    out.contains("[on]") || out.contains("[off]")
}

/// The playback volume of a card (the first of [`CONTROLS`] it has), asking `run`.
pub fn volume_via(run: Amixer, mutes: &Mutes, card: u32) -> Option<Volume> {
    let c = card.to_string();
    CONTROLS.iter().find_map(|ctl| {
        let out = run(&["-c", &c, "sget", ctl])?;
        let mut v = parse_amixer(ctl, &out)?;
        if !has_switch(&out) {
            // muted by setting 0 %: muted while it is still at 0 %
            match mutes.get(card, ctl) {
                Some(_) if v.percent == 0 => v.muted = true,
                Some(_) => mutes.set(card, ctl, None), // moved by someone else meanwhile
                None => {}
            }
        }
        Some(v)
    })
}

/// Change the volume by `delta` percent points, or mute/unmute when `delta` is `None`. A control without a switch is muted
/// by setting 0 % and unmuted by setting the level it had; louder while muted starts from that level.
pub fn change_volume_via(run: Amixer, mutes: &Mutes, card: u32, delta: Option<i32>) -> Result<Volume, String> {
    let c = card.to_string();
    let cur = volume_via(run, mutes, card).ok_or("this card has no volume control")?;
    let switch = run(&["-c", &c, "sget", &cur.control]).is_some_and(|o| has_switch(&o));
    let arg = match (delta, switch) {
        (Some(d), false) if cur.muted => {
            let back = mutes.get(card, &cur.control).unwrap_or(0) as i32;
            mutes.set(card, &cur.control, None);
            format!("{}%", (back + d).clamp(0, 100))
        }
        (Some(d), _) => format!("{}%{}", d.unsigned_abs(), if d < 0 { "-" } else { "+" }),
        (None, true) => "toggle".into(),
        (None, false) if cur.muted => {
            let back = mutes.get(card, &cur.control).filter(|l| *l > 0).unwrap_or(50);
            mutes.set(card, &cur.control, None);
            format!("{back}%")
        }
        (None, false) => {
            mutes.set(card, &cur.control, Some(cur.percent));
            "0%".into()
        }
    };
    run(&["-c", &c, "sset", &cur.control, &arg]).ok_or("the mixer did not accept the change")?;
    volume_via(run, mutes, card).ok_or_else(|| "could not read the volume back".into())
}

/// Set the playback volume of a card to `percent` (0..=100); this also ends a mute of a control without a switch.
pub fn set_volume_via(run: Amixer, mutes: &Mutes, card: u32, percent: u32) -> Result<Volume, String> {
    let cur = volume_via(run, mutes, card).ok_or("this card has no volume control")?;
    let c = card.to_string();
    mutes.set(card, &cur.control, None);
    run(&["-c", &c, "sset", &cur.control, &format!("{}%", percent.min(100))]).ok_or("the mixer did not accept the change")?;
    volume_via(run, mutes, card).ok_or_else(|| "could not read the volume back".into())
}

/// The playback volume of a card (the first of the usual controls it has).
pub fn volume(card: u32) -> Option<Volume> {
    volume_via(&amixer, mutes(), card)
}

/// Change the volume by `delta` percent points, or toggle mute when `delta` is `None`.
pub fn change_volume(card: u32, delta: Option<i32>) -> Result<Volume, String> {
    change_volume_via(&amixer, mutes(), card, delta)
}

/// Set the playback volume of a card to `percent` (0..=100).
pub fn set_volume(card: u32, percent: u32) -> Result<Volume, String> {
    set_volume_via(&amixer, mutes(), card, percent)
}

// ------------------------------------------------------------------------------------------------ host name

/// Is this a host name the kernel and DNS accept (letters, digits and '-', not starting or ending with '-', up to 63)?
pub fn hostname_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 63 && !h.starts_with('-') && !h.ends_with('-') && h.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// The host name now: the kernel's, else the file, else `fallback`.
pub fn current_hostname(fallback: &str) -> String {
    read("/proc/sys/kernel/hostname").or_else(|| read("/etc/hostname")).filter(|h| !h.is_empty()).unwrap_or_else(|| fallback.to_string())
}

/// Change the host name of the running system and write it to `etc_hostname` (needs root for the first part).
#[cfg(unix)]
pub fn apply_hostname(name: &str, etc_hostname: &Path) -> Result<(), String> {
    if !hostname_ok(name) {
        return Err(format!("\"{name}\" is not a host name: use lower-case letters, digits and '-'"));
    }
    // SAFETY: pointer and length describe the bytes of `name`, which outlives the call.
    let rc = unsafe { libc::sethostname(name.as_ptr().cast(), name.len() as _) };
    if rc != 0 {
        return Err(format!("could not change the host name: {}", std::io::Error::last_os_error()));
    }
    std::fs::write(etc_hostname, format!("{name}\n")).map_err(|e| format!("the name is changed but {} could not be written: {e}", etc_hostname.display()))
}

#[cfg(not(unix))]
pub fn apply_hostname(_name: &str, _etc_hostname: &Path) -> Result<(), String> {
    Err("not available on this OS".into())
}

// ------------------------------------------------------------------------------------------------ WiFi link details

/// The network a WiFi interface is joined to and how strong the signal is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiLink {
    pub ssid: String,
    pub signal_dbm: Option<i32>,
}

/// `iw dev wlan0 link`: "Connected to ..." with an `SSID:` line and a `signal: -48 dBm` line, or "Not connected.".
pub fn parse_iw_link(text: &str) -> Option<WifiLink> {
    if text.trim_start().starts_with("Not connected") {
        return None;
    }
    let ssid = text.lines().find_map(|l| l.trim().strip_prefix("SSID:")).map(|s| s.trim().to_string())?;
    let signal_dbm = text.lines().find_map(|l| l.trim().strip_prefix("signal:")).and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f32>().ok())).map(|v| v.round() as i32);
    Some(WifiLink { ssid, signal_dbm })
}

/// The signal level (dBm) of `iface` from `/proc/net/wireless`.
pub fn parse_proc_wireless(text: &str, iface: &str) -> Option<i32> {
    let line = text.lines().find(|l| l.trim_start().starts_with(&format!("{iface}:")))?;
    let f: Vec<&str> = line.split_once(':')?.1.split_whitespace().collect();
    let level = f.get(2)?.trim_end_matches('.').parse::<f32>().ok()?;
    // values above 0 are on a 0..100 scale (quality), not dBm: not worth showing as dBm
    (level < 0.0).then_some(level.round() as i32)
}

/// Which network `iface` is joined to. Asks `iw` (a short-lived process, so call it from a background thread); the signal
/// falls back to `/proc/net/wireless` when `iw` is missing.
pub fn wifi_link(r: &Roots, iface: &str) -> Option<WifiLink> {
    if iface.is_empty() || !iface.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
        return None;
    }
    let out = std::process::Command::new("iw").args(["dev", iface, "link"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok().filter(|o| o.status.success());
    let mut link = out.and_then(|o| parse_iw_link(&String::from_utf8_lossy(&o.stdout)))?;
    if link.signal_dbm.is_none() {
        link.signal_dbm = read(r.proc_.join("net/wireless")).and_then(|t| parse_proc_wireless(&t, iface));
    }
    Some(link)
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
    /// The adapter's own (permanent) hardware address, when the driver tells.
    fn permanent_mac(&self, _name: &str) -> Option<String> {
        None
    }
    /// Give the adapter a new address (the link must be down). Drivers may refuse.
    fn set_mac(&self, _name: &str, _mac: &str) -> Result<(), String> {
        Err("changing the MAC address is not available here".into())
    }
    /// The Bluetooth controllers as the kernel's management interface reports them (works without an `address` file in sysfs).
    fn bt_controllers(&self) -> Result<Vec<btview::Controller>, String> {
        Err("not available here".into())
    }
    /// Power a Bluetooth controller on or off (management interface).
    fn bt_power(&self, _index: u16, _on: bool) -> Result<(), String> {
        Err("not available here".into())
    }
    /// Scan for `secs` seconds; the controller must be on. Never pairs.
    fn bt_scan(&self, _index: u16, _secs: u64) -> Result<Vec<btmgmt::Found>, String> {
        Err("not available here".into())
    }
    /// The patch file the kernel asks for (`brcm/BCM.hcd`) exists.
    fn bt_firmware_file(&self) -> bool {
        false
    }
    /// "Load the Bluetooth driver anyway": give the held drivers back and load them (a function the program registers, see [`set_bt_loader`]).
    fn bt_load_anyway(&self) -> Result<String, String> {
        Err("not available here".into())
    }
    /// The Bluetooth drivers are set aside until firmware exists (a T2 Mac), so "Load the Bluetooth driver anyway" has something to do.
    fn bt_held(&self) -> bool {
        false
    }
}

static BT_LOADER: std::sync::OnceLock<(fn() -> bool, fn() -> Result<String, String>)> = std::sync::OnceLock::new();

/// The program registers what "Load the Bluetooth driver anyway" does and how to tell that a driver is held back (the driver packs live
/// in a crate above this one).
pub fn set_bt_loader(held: fn() -> bool, load: fn() -> Result<String, String>) {
    let _ = BT_LOADER.set((held, load));
}

/// The real machine.
pub struct System;

impl Control for System {
    fn bt_controllers(&self) -> Result<Vec<btview::Controller>, String> {
        btview::controllers()
    }
    fn bt_power(&self, index: u16, on: bool) -> Result<(), String> {
        btview::set_powered(index, on)
    }
    fn bt_scan(&self, index: u16, secs: u64) -> Result<Vec<btmgmt::Found>, String> {
        btview::scan(index, secs)
    }
    fn bt_firmware_file(&self) -> bool {
        Path::new("/lib/firmware/brcm/BCM.hcd").exists()
    }
    fn bt_load_anyway(&self) -> Result<String, String> {
        BT_LOADER.get().map(|f| (f.1)()).unwrap_or_else(|| Err("not available here".into()))
    }
    fn bt_held(&self) -> bool {
        BT_LOADER.get().is_some_and(|f| (f.0)())
    }
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
    fn permanent_mac(&self, name: &str) -> Option<String> {
        hw::permanent(name)
    }
    fn set_mac(&self, name: &str, mac: &str) -> Result<(), String> {
        hw::set(name, mac)
    }
    fn dhcp(&self, name: &str) -> Result<(), String> {
        if name.is_empty() || name.len() > 15 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
            return Err(format!("not an interface name: {name:?}"));
        }
        std::process::Command::new("udhcpc").args(mac::dhcp_args(name, &format!("/run/udhcpc-{name}.pid"))).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ()).map_err(|e| format!("could not start DHCP on {name}: {e}"))
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
        assert_eq!(b.unstarted, vec!["hci0"], "no address file: the controller never answered");
        put(&r.sys, "class/bluetooth/hci0/address", "11:22:33:44:55:66\n");
        assert!(bluetooth(&r).unstarted.is_empty(), "an address means it started");
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

    /// A fake `amixer` for one card: hardware `Master` with a switch, or only IYAKKI's softvol (no switch).
    struct FakeMixer {
        softvol_only: bool,
        level: std::cell::Cell<u32>,
        on: std::cell::Cell<bool>,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl FakeMixer {
        fn new(softvol_only: bool, level: u32) -> FakeMixer {
            FakeMixer { softvol_only, level: level.into(), on: true.into(), calls: Default::default() }
        }
        fn run(&self, args: &[&str]) -> Option<String> {
            self.calls.borrow_mut().push(args.join(" "));
            let ctl = if self.softvol_only { "IYAKKI" } else { "Master" };
            if args.get(3) != Some(&ctl) {
                return None; // amixer: Unable to find simple control
            }
            if args[2] == "sset" {
                let a = args[4];
                if a == "toggle" {
                    self.on.set(!self.on.get());
                } else if let Some(p) = a.strip_suffix("%+") {
                    self.level.set((self.level.get() + p.parse::<u32>().unwrap()).min(100));
                } else if let Some(p) = a.strip_suffix("%-") {
                    self.level.set(self.level.get().saturating_sub(p.parse().unwrap()));
                } else {
                    self.level.set(a.trim_end_matches('%').parse().unwrap());
                }
            }
            let l = self.level.get();
            Some(if self.softvol_only {
                format!("Simple mixer control 'IYAKKI',0\n  Capabilities: volume volume-joined\n  Limits: 0 - 255\n  Mono: {} [{l}%] [-20.00dB]\n", l * 255 / 100)
            } else {
                format!("Simple mixer control 'Master',0\n  Capabilities: pvolume pswitch\n  Front Left: Playback 40 [{l}%] [-20.00dB] [{}]\n", if self.on.get() { "on" } else { "off" })
            })
        }
    }

    #[test]
    fn a_softvol_control_without_a_switch_is_found_and_muted_by_zero_percent_with_the_level_remembered() {
        let m = FakeMixer::new(true, 30);
        let run = |a: &[&str]| m.run(a);
        let mutes = Mutes::default();
        let v = volume_via(&run, &mutes, 1).unwrap();
        assert_eq!(v, Volume { control: "IYAKKI".into(), percent: 30, muted: false }, "the T2 card's only volume is IYAKKI's softvol");
        assert!(m.calls.borrow().iter().any(|c| c == "-c 1 sget Master"), "hardware controls are asked first");
        let v = change_volume_via(&run, &mutes, 1, None).unwrap();
        assert_eq!((v.percent, v.muted), (0, true), "mute is 0 %");
        assert!(m.calls.borrow().iter().any(|c| c == "-c 1 sset IYAKKI 0%"));
        assert!(volume_via(&run, &mutes, 1).unwrap().muted);
        let v = change_volume_via(&run, &mutes, 1, None).unwrap();
        assert_eq!((v.percent, v.muted), (30, false), "unmute restores the level");
        // louder while muted starts from the remembered level
        change_volume_via(&run, &mutes, 1, None).unwrap();
        let v = change_volume_via(&run, &mutes, 1, Some(5)).unwrap();
        assert_eq!((v.percent, v.muted), (35, false));
        // an exact level ends the mute too
        change_volume_via(&run, &mutes, 1, None).unwrap();
        let v = set_volume_via(&run, &mutes, 1, 60).unwrap();
        assert_eq!((v.percent, v.muted), (60, false));
        assert!(!m.calls.borrow().iter().any(|c| c.contains("toggle")), "a control without a switch is never toggled");
    }

    #[test]
    fn a_hardware_control_with_a_switch_is_toggled_as_before() {
        let m = FakeMixer::new(false, 62);
        let run = |a: &[&str]| m.run(a);
        let mutes = Mutes::default();
        let v = change_volume_via(&run, &mutes, 0, None).unwrap();
        assert_eq!(v, Volume { control: "Master".into(), percent: 62, muted: true });
        assert!(m.calls.borrow().iter().any(|c| c == "-c 0 sset Master toggle"));
        assert_eq!(change_volume_via(&run, &mutes, 0, Some(-10)).unwrap().percent, 52);
        assert!(m.calls.borrow().iter().any(|c| c == "-c 0 sset Master 10%-"));
        let none = |_: &[&str]| None;
        assert!(volume_via(&none, &mutes, 0).is_none() && change_volume_via(&none, &mutes, 0, None).is_err());
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

    #[test]
    fn host_names_are_checked_before_anything_is_changed() {
        for ok in ["iyakki", "kitchen-2", "a", "x1"] {
            assert!(hostname_ok(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "Mac mini", "UPPER", "under_score", "dot.ted", &"a".repeat(64)] {
            assert!(!hostname_ok(bad), "{bad}");
        }
        assert!(apply_hostname("Bad Name", Path::new("/nonexistent/hostname")).is_err());
        assert!(!current_hostname("fallback").is_empty());
    }

    #[test]
    fn wifi_link_text_is_read() {
        let text = "Connected to aa:bb:cc:00:00:01 (on wlan0)\n\tSSID: Home Net\n\tfreq: 5180\n\tsignal: -48 dBm\n\ttx bitrate: 866.7 MBit/s\n";
        assert_eq!(parse_iw_link(text), Some(WifiLink { ssid: "Home Net".into(), signal_dbm: Some(-48) }));
        assert_eq!(parse_iw_link("Not connected.\n"), None);
        assert_eq!(parse_iw_link(""), None);
        let w = "Inter-| sta-|   Quality        |   Discarded packets\n face | tus | link level noise |  nwid\n wlan0: 0000   70.  -40.  -256        0      0\n";
        assert_eq!(parse_proc_wireless(w, "wlan0"), Some(-40));
        assert_eq!(parse_proc_wireless(w, "wlan1"), None);
        assert!(wifi_link(&Roots { sys: "/x".into(), proc_: "/x".into() }, "bad name;").is_none());
    }
}
