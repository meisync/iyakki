//! Private MAC addresses: Off (the hardware's own address), Per network (a stable address per WiFi network or Ethernet
//! link) and Random (a new one every time the link is brought up), like macOS "Private Wi-Fi address".
//!
//! Everything that decides something is a plain function (mode choice, address derivation, `net.conf` text, DHCP options,
//! the order of the steps) and is tested against a fake machine ([`Roots`] under a temp folder and a fake [`Control`]).
//! Only [`crate::System`] touches a real adapter, with `SIOCSIFHWADDR` while the link is down.
//!
//! * The address is derived with HMAC-SHA1 keyed by a per-device secret (`/var/lib/iyakki/mac-secret`, owner-only), the
//!   adapter's permanent address, the kind of link and the network name (the SSID, or the interface name for Ethernet), so
//!   it is stable for one network on one device, differs between networks and between devices, and cannot be linked back
//!   to the hardware address without the secret. The first byte always has the locally-administered bit set and the
//!   multicast bit clear.
//! * Settings live in `net.conf` as `mac.ethernet`, `mac.wifi`, `mac.iface.<name>` and `mac.ssid.<network>`. When
//!   `/etc/iyakki` is read-only (the live image) the same lines go to `/var/lib/iyakki/net-mac.conf`, which wins.
//! * DHCP: the client sends no host name (never `-x hostname`, `-F`, `-H`) in any mode, and its client identifier is the
//!   adapter's *current* address, which the steps below set before the client starts, so nothing carries the hardware
//!   address to the router when the mode is not Off.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{links, Control, Link, LinkKind, Roots};

// ------------------------------------------------------------------------------------------------ modes and settings

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacMode {
    /// The hardware's own address.
    Off,
    /// A stable private address per network.
    PerNetwork,
    /// A new private address on every connection.
    Random,
}

impl MacMode {
    pub const ALL: [MacMode; 3] = [MacMode::Off, MacMode::PerNetwork, MacMode::Random];

    /// The word used in `net.conf` and on the command line.
    pub fn key(self) -> &'static str {
        match self {
            MacMode::Off => "off",
            MacMode::PerNetwork => "per-network",
            MacMode::Random => "random",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MacMode::Off => "Off (hardware address)",
            MacMode::PerNetwork => "Per network (stable private address)",
            MacMode::Random => "Random each connection",
        }
    }

    pub fn parse(s: &str) -> Option<MacMode> {
        match s.trim().to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
            "off" | "no" | "false" | "0" | "hardware" => Some(MacMode::Off),
            "per-network" | "pernetwork" | "network" | "stable" | "on" => Some(MacMode::PerNetwork),
            "random" | "each-connection" | "random-each-connection" => Some(MacMode::Random),
            _ => None,
        }
    }

    pub fn is_private(self) -> bool {
        self != MacMode::Off
    }
}

/// The choice for each kind of adapter, with optional exceptions for one interface or one WiFi network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacConfig {
    pub ethernet: MacMode,
    pub wifi: MacMode,
    pub ifaces: BTreeMap<String, MacMode>,
    pub ssids: BTreeMap<String, MacMode>,
}

impl Default for MacConfig {
    /// Ethernet keeps the hardware address so DHCP reservations and router rules keep working; WiFi is private per network.
    fn default() -> MacConfig {
        MacConfig { ethernet: MacMode::Off, wifi: MacMode::PerNetwork, ifaces: BTreeMap::new(), ssids: BTreeMap::new() }
    }
}

fn iface_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn ssid_key_ok(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.chars().all(|c| !c.is_control() && c != '=' && c != '"' && c != '\\')
}

impl MacConfig {
    pub fn type_mode(&self, kind: LinkKind) -> MacMode {
        match kind {
            LinkKind::Ethernet => self.ethernet,
            LinkKind::Wifi => self.wifi,
        }
    }

    /// The mode that applies: a network's own choice, then the interface's, then the kind's.
    pub fn mode_for(&self, kind: LinkKind, iface: &str, ssid: Option<&str>) -> MacMode {
        if kind == LinkKind::Wifi {
            if let Some(m) = ssid.and_then(|s| self.ssids.get(s)) {
                return *m;
            }
        }
        self.ifaces.get(iface).copied().unwrap_or_else(|| self.type_mode(kind))
    }

    /// Take one `key = value` line. `Ok(false)`: not a `mac.` key. `Err`: a `mac.` key with a bad name or value.
    pub fn set_key(&mut self, key: &str, value: &str) -> Result<bool, String> {
        let Some(rest) = key.strip_prefix("mac.") else { return Ok(false) };
        let mode = MacMode::parse(value).ok_or_else(|| format!("{key}: expected off, per-network or random, not `{value}`"))?;
        match rest {
            "ethernet" => self.ethernet = mode,
            "wifi" => self.wifi = mode,
            _ => {
                if let Some(n) = rest.strip_prefix("iface.") {
                    if !iface_name_ok(n) {
                        return Err(format!("{key}: not an interface name"));
                    }
                    self.ifaces.insert(n.to_string(), mode);
                } else if let Some(s) = rest.strip_prefix("ssid.") {
                    if !ssid_key_ok(s) {
                        return Err(format!("{key}: not a usable WiFi network name"));
                    }
                    self.ssids.insert(s.to_string(), mode);
                } else {
                    return Err(format!("unknown key `{key}`"));
                }
            }
        }
        Ok(true)
    }

    /// The lines that go into `net.conf`.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!("mac.ethernet = {}", self.ethernet.key()), format!("mac.wifi = {}", self.wifi.key())];
        v.extend(self.ifaces.iter().map(|(n, m)| format!("mac.iface.{n} = {}", m.key())));
        v.extend(self.ssids.iter().map(|(n, m)| format!("mac.ssid.{n} = {}", m.key())));
        v
    }

    /// Read the `mac.` lines of a `net.conf` text. Everything else is ignored and so are bad `mac.` lines (an old or
    /// hand-edited file must never stop the network coming up); the returned strings say what was skipped.
    pub fn parse_lenient(text: &str) -> (MacConfig, Vec<String>) {
        let mut c = MacConfig::default();
        let mut skipped = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if let Err(e) = c.set_key(k.trim(), v.trim()) {
                skipped.push(e);
            }
        }
        (c, skipped)
    }
}

const BLOCK_HEAD: &str = "# Private MAC address (Settings -> Network, `iyakki net mac`): off | per-network | random";

/// `text` with its `mac.` lines replaced by `cfg`'s. Other lines and comments stay as they are.
pub fn merge_into_text(text: &str, cfg: &MacConfig) -> String {
    let mut kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("mac.") && l.trim() != BLOCK_HEAD).collect();
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(BLOCK_HEAD);
    out.push('\n');
    for l in cfg.lines() {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Write a file so that a power cut leaves either the old or the new contents: a temp file next to it, then a rename.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let dir = path.parent().ok_or("no folder")?;
    let tmp = dir.join(format!(".{}.tmp{}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), std::process::id()));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("{}: {e}", path.display()))
}

/// Where the settings are kept.
#[derive(Clone, Debug)]
pub struct MacStore {
    /// `/etc/iyakki/net.conf`.
    pub primary: PathBuf,
    /// Used when the primary cannot be written (read-only image).
    pub fallback: PathBuf,
}

impl MacStore {
    pub fn real() -> MacStore {
        MacStore { primary: "/etc/iyakki/net.conf".into(), fallback: "/var/lib/iyakki/net-mac.conf".into() }
    }

    pub fn load(&self) -> MacConfig {
        let mut text = std::fs::read_to_string(&self.primary).unwrap_or_default();
        // the fallback's lines come last so they win
        text.push('\n');
        text.push_str(&std::fs::read_to_string(&self.fallback).unwrap_or_default());
        MacConfig::parse_lenient(&text).0
    }

    /// Save. Returns the file that now holds the settings.
    pub fn save(&self, cfg: &MacConfig) -> Result<PathBuf, String> {
        let old = std::fs::read_to_string(&self.primary).unwrap_or_default();
        match write_atomic(&self.primary, &merge_into_text(&old, cfg)) {
            Ok(()) => {
                // a stale override in the fallback would win over what was just saved
                let _ = std::fs::remove_file(&self.fallback);
                Ok(self.primary.clone())
            }
            Err(first) => {
                if let Some(d) = self.fallback.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let old = std::fs::read_to_string(&self.fallback).unwrap_or_default();
                write_atomic(&self.fallback, &merge_into_text(&old, cfg)).map(|_| self.fallback.clone()).map_err(|second| format!("could not save the setting ({first}; {second})"))
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ addresses

pub type Mac = [u8; 6];

pub fn parse_mac(s: &str) -> Option<Mac> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        m[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(m)
}

pub fn fmt_mac(m: &Mac) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

/// A unicast address that is not all zeros: the only kind an interface may be given.
pub fn valid_unicast(m: &Mac) -> bool {
    m[0] & 1 == 0 && m.iter().any(|b| *b != 0)
}

/// The locally-administered bit (second-lowest bit of the first byte): set on private and made-up addresses.
pub fn locally_administered(m: &Mac) -> bool {
    m[0] & 2 != 0
}

/// Make `m` a locally administered unicast address.
fn private_form(mut m: Mac) -> Mac {
    m[0] = (m[0] | 0x02) & 0xFE;
    m
}

/// The stable private address for `scope` (the network name, or the interface name for Ethernet) on this device.
pub fn derive(secret: &[u8], kind: LinkKind, permanent: &str, scope: &str) -> Mac {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"iyakki-mac-v1\0");
    msg.extend_from_slice(match kind {
        LinkKind::Ethernet => b"ethernet",
        LinkKind::Wifi => b"wifi",
    });
    msg.push(0);
    msg.extend_from_slice(permanent.to_ascii_lowercase().as_bytes());
    msg.push(0);
    msg.extend_from_slice(scope.as_bytes());
    let h = iyakki_sec::wpa::hmac_sha1(secret, &msg);
    private_form([h[0], h[1], h[2], h[3], h[4], h[5]])
}

pub fn random_mac() -> Result<Mac, String> {
    let mut b = [0u8; 6];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no random numbers available: {e}"))?;
    Ok(private_form(b))
}

/// The per-device secret, made on first use. The flag says whether it is kept (`false`: this boot only, so "per network"
/// is stable only until the next boot).
pub fn load_secret(path: &Path) -> (Vec<u8>, bool) {
    if let Ok(b) = std::fs::read(path) {
        if b.len() >= 32 {
            return (b, true);
        }
    }
    let mut b = vec![0u8; 32];
    if iyakki_sec::random_fill(&mut b).is_err() {
        // never a guessable key: mix what is at hand
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
        b = iyakki_sec::wpa::hmac_sha1(&t.to_le_bytes(), format!("{:?}", std::thread::current().id()).as_bytes()).repeat(2);
    }
    let kept = (|| -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?.write_all(&b)?;
        std::fs::rename(&tmp, path)
    })()
    .is_ok();
    (b, kept)
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// The address the kernel shows now.
pub fn current_mac(roots: &Roots, iface: &str) -> Option<String> {
    read_trim(roots.sys.join("class/net").join(iface).join("address"))
}

/// How the kernel got the address: 0 permanent, 1 random, 2 taken from another device, 3 set by a program.
pub fn assign_type(roots: &Roots, iface: &str) -> Option<u8> {
    read_trim(roots.sys.join("class/net").join(iface).join("addr_assign_type")).and_then(|s| s.parse().ok())
}

/// The adapter's own address: from the driver when it says (Linux `ETHTOOL_GPERMADDR`), else the current one when the kernel
/// says it was never changed.
pub fn permanent_mac(ctl: &dyn Control, roots: &Roots, iface: &str) -> Option<String> {
    ctl.permanent_mac(iface).filter(|m| parse_mac(m).is_some()).or_else(|| if assign_type(roots, iface) == Some(0) { current_mac(roots, iface) } else { None })
}

// ------------------------------------------------------------------------------------------------ what Settings shows

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacInfo {
    pub iface: String,
    pub kind: LinkKind,
    pub permanent: Option<String>,
    pub current: String,
    pub locally_administered: bool,
    /// The address is not the hardware's own.
    pub private: bool,
    /// The mode that applies to this interface (without a network name).
    pub mode: MacMode,
}

impl MacInfo {
    /// "aa:bb:cc:dd:ee:ff (private)".
    pub fn short(&self) -> String {
        format!("{} ({})", self.current, if self.private { "private" } else { "hardware" })
    }
}

fn is_private(current: &str, assign: Option<u8>, permanent: Option<&str>) -> bool {
    match (permanent, parse_mac(current)) {
        (Some(p), _) if p.eq_ignore_ascii_case(current) => false,
        (Some(_), Some(_)) => true,
        // the hardware address is not known: trust the kernel's note on how the address came about (random, or set by a program)
        (None, Some(_)) => matches!(assign, Some(1) | Some(3)),
        _ => false,
    }
}

fn info_of(l: Link, permanent: Option<String>, cfg: &MacConfig, assign: Option<u8>) -> MacInfo {
    let private = is_private(&l.mac, assign, permanent.as_deref());
    MacInfo { locally_administered: parse_mac(&l.mac).is_some_and(|m| locally_administered(&m)), mode: cfg.mode_for(l.kind, &l.name, None), private, permanent, current: l.mac, kind: l.kind, iface: l.name }
}

/// What every adapter has, for the Network page.
pub fn infos(ctl: &dyn Control, roots: &Roots, cfg: &MacConfig) -> Vec<MacInfo> {
    links(roots)
        .into_iter()
        .map(|l| {
            let permanent = permanent_mac(ctl, roots, &l.name);
            let assign = assign_type(roots, &l.name);
            info_of(l, permanent, cfg, assign)
        })
        .collect()
}

/// The same without asking the driver (files only), cheap enough for a tile or popover redrawn often.
pub fn brief(roots: &Roots, cfg: &MacConfig) -> Vec<MacInfo> {
    links(roots)
        .into_iter()
        .map(|l| {
            let assign = assign_type(roots, &l.name);
            let permanent = if assign == Some(0) { Some(l.mac.clone()) } else { None };
            info_of(l, permanent, cfg, assign)
        })
        .collect()
}

/// One line for the System tile and the Devices popover: the address of the adapter in use, e.g.
/// `MAC: aa:bb:cc:dd:ee:ff (private)`. Prefers an adapter with a cable or association.
pub fn summary_line(roots: &Roots) -> Option<String> {
    let ls = links(roots);
    let l: &Link = ls.iter().find(|l| l.up && l.carrier).or_else(|| ls.first())?;
    let i = brief(roots, &MacConfig::default()).into_iter().find(|i| i.iface == l.name)?;
    Some(format!("MAC: {}", i.short()))
}

/// `MAC: aa:bb:cc:dd:ee:ff (private)` for one named adapter (the Devices popover's network lines), files only.
pub fn line_for(roots: &Roots, iface: &str) -> Option<String> {
    brief(roots, &MacConfig::default()).into_iter().find(|i| i.iface == iface).map(|i| format!("MAC: {}", i.short()))
}

/// Lines for `iyakki net mac` (read-only), asking the driver for the hardware address.
pub fn status_lines(ctl: &dyn Control, roots: &Roots, cfg: &MacConfig) -> Vec<String> {
    format_status(infos(ctl, roots, cfg), cfg)
}

/// The same from files alone (no driver request), for `iyakki net` and the assistant's network tool.
pub fn status_lines_from_files(roots: &Roots, cfg: &MacConfig) -> Vec<String> {
    format_status(brief(roots, cfg), cfg)
}

fn format_status(infos: Vec<MacInfo>, cfg: &MacConfig) -> Vec<String> {
    let mut v = vec![format!("Private MAC address: Ethernet {}, WiFi {} (net.conf: mac.ethernet, mac.wifi)", cfg.ethernet.key(), cfg.wifi.key())];
    for i in infos {
        v.push(format!(
            "{:<10} {:<9} mode {:<11} hardware {:<17} current {:<17} {}{}",
            i.iface,
            if i.kind == LinkKind::Wifi { "WiFi" } else { "Ethernet" },
            i.mode.key(),
            i.permanent.as_deref().unwrap_or("unknown"),
            i.current,
            if i.private { "private" } else { "hardware address" },
            if i.locally_administered { ", locally administered" } else { "" }
        ));
    }
    v
}

// ------------------------------------------------------------------------------------------------ DHCP

/// Arguments of the DHCP client for one interface. In every mode no host name is sent and the client identifier is the
/// interface's current address (BusyBox's default), which the steps in [`apply_mac`] set before the client starts.
pub fn dhcp_args(iface: &str, pid_file: &str) -> Vec<String> {
    ["-b", "-R", "-i", iface, "-p", pid_file].iter().map(|s| s.to_string()).collect()
}

/// Lines for the supplicant: when the address is private, scans and service queries before joining also use random
/// addresses (the driver may refuse; the supplicant then carries on).
pub fn supplicant_lines(mode: MacMode) -> &'static str {
    if mode.is_private() { "preassoc_mac_addr=1\ngas_rand_mac_addr=1\n" } else { "" }
}

// ------------------------------------------------------------------------------------------------ applying

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Already that address.
    Unchanged(String),
    Changed { from: String, to: String },
}

fn restore(ctl: &dyn Control, roots: &Roots, iface: &str, original: &str, permanent: Option<&str>) -> String {
    let back = permanent.unwrap_or(original);
    let which = if permanent.is_some() { "hardware" } else { "previous" };
    match ctl.set_mac(iface, back) {
        Ok(()) if current_mac(roots, iface).is_some_and(|c| c.eq_ignore_ascii_case(back)) => format!("the {which} address {back} was put back"),
        Ok(()) => format!("putting back the {which} address {back} could not be confirmed"),
        Err(e) => format!("putting back the {which} address {back} failed too: {e}"),
    }
}

/// Give `iface` the address `target`: link down, set, check, link up. Never leaves an invalid address: the target is checked
/// first, and if anything fails after the change the hardware address is put back and the error says so.
pub fn apply_mac(ctl: &dyn Control, roots: &Roots, iface: &str, target: &Mac, permanent: Option<&str>) -> Result<Applied, String> {
    if !valid_unicast(target) {
        return Err(format!("{}: refused: not a valid unicast address", fmt_mac(target)));
    }
    let link = links(roots).into_iter().find(|l| l.name == iface).ok_or_else(|| format!("{iface}: no such network adapter"))?;
    let want = fmt_mac(target);
    let original = link.mac.clone();
    if original.eq_ignore_ascii_case(&want) {
        return Ok(Applied::Unchanged(want));
    }
    if link.up {
        ctl.set_link(iface, false).map_err(|e| format!("{iface}: could not take the link down to change its address: {e}"))?;
    }
    let bring_up = |ctl: &dyn Control| if link.up { ctl.set_link(iface, true) } else { Ok(()) };
    if let Err(e) = ctl.set_mac(iface, &want) {
        let _ = bring_up(ctl);
        return Err(format!("{iface}: the address was not changed ({e}); it keeps {original}"));
    }
    let now = current_mac(roots, iface).unwrap_or_default();
    if !now.eq_ignore_ascii_case(&want) {
        let said = restore(ctl, roots, iface, &original, permanent);
        let _ = bring_up(ctl);
        return Err(format!("{iface}: the driver did not keep the new address (it shows {now}); {said}"));
    }
    if let Err(e) = bring_up(ctl) {
        let said = restore(ctl, roots, iface, &original, permanent);
        let _ = bring_up(ctl);
        return Err(format!("{iface}: the link did not come back with the new address ({e}); {said}"));
    }
    Ok(Applied::Changed { from: original, to: want })
}

/// The address `mode` calls for, `None` when nothing should change (Off and the hardware address is not known).
pub fn target_for(mode: MacMode, kind: LinkKind, secret: &[u8], permanent: Option<&str>, current: &str, iface: &str, ssid: Option<&str>) -> Result<Option<Mac>, String> {
    Ok(match mode {
        MacMode::Off => permanent.and_then(parse_mac),
        MacMode::PerNetwork => {
            let hw = permanent.unwrap_or(current);
            let scope = match (kind, ssid) {
                (LinkKind::Wifi, Some(s)) => format!("ssid:{s}"),
                _ => format!("iface:{iface}"),
            };
            Some(derive(secret, kind, hw, &scope))
        }
        MacMode::Random => Some(random_mac()?),
    })
}

/// Set the address `cfg` calls for on one interface (the whole decision and the change).
pub fn apply_config(ctl: &dyn Control, roots: &Roots, cfg: &MacConfig, secret: &[u8], iface: &str, ssid: Option<&str>) -> Result<Applied, String> {
    let link = links(roots).into_iter().find(|l| l.name == iface).ok_or_else(|| format!("{iface}: no such network adapter"))?;
    apply_mode(ctl, roots, cfg.mode_for(link.kind, iface, ssid), secret, iface, ssid)
}

/// Set the address `mode` calls for on one interface.
pub fn apply_mode(ctl: &dyn Control, roots: &Roots, mode: MacMode, secret: &[u8], iface: &str, ssid: Option<&str>) -> Result<Applied, String> {
    let link = links(roots).into_iter().find(|l| l.name == iface).ok_or_else(|| format!("{iface}: no such network adapter"))?;
    let permanent = permanent_mac(ctl, roots, iface);
    match target_for(mode, link.kind, secret, permanent.as_deref(), &link.mac, iface, ssid)? {
        Some(t) => apply_mac(ctl, roots, iface, &t, permanent.as_deref()),
        None => Ok(Applied::Unchanged(link.mac)),
    }
}

/// Established connections that came in to this machine on `ip` (SSH, the remote screen): dropping the link drops them.
pub fn inbound_sessions(roots: &Roots, ip: &str) -> usize {
    let Some(want) = ip.split('.').map(|p| p.parse::<u8>().ok()).collect::<Option<Vec<u8>>>().filter(|v| v.len() == 4) else { return 0 };
    // /proc/net/tcp shows the address as the little-endian hex of the 32-bit value
    let hex = format!("{:02X}{:02X}{:02X}{:02X}", want[3], want[2], want[1], want[0]);
    let text = std::fs::read_to_string(roots.proc_.join("net/tcp")).unwrap_or_default();
    let rows: Vec<Vec<&str>> = text.lines().skip(1).map(|l| l.split_whitespace().collect()).collect();
    let listening: Vec<&str> = rows.iter().filter(|r| r.get(3) == Some(&"0A")).filter_map(|r| r.get(1)?.split(':').nth(1)).collect();
    rows.iter()
        .filter(|r| r.get(3) == Some(&"01"))
        .filter(|r| {
            let (Some(local), Some(remote)) = (r.get(1), r.get(2)) else { return false };
            let (lip, lport) = local.split_once(':').unwrap_or_default();
            let rip = remote.split(':').next().unwrap_or_default();
            lip == hex && rip != "0100007F" && listening.contains(&lport)
        })
        .count()
}

/// Everything a settings change or `iyakki net mac` acts on. Real in production; tests use a fake machine.
#[derive(Clone)]
pub struct MacEnv {
    pub store: MacStore,
    pub secret_path: PathBuf,
    pub ctl: Arc<dyn Control>,
    pub roots: Roots,
    /// `SSH_CONNECTION` of this process, if it was started over SSH.
    pub ssh_connection: Option<String>,
    /// The desktop window on someone else's computer: the host OS owns the addresses.
    pub host_managed: bool,
    /// The saved WiFi networks (newest first); the first is the one "per network" is made for before anything is joined.
    pub known_wifi: PathBuf,
}

impl MacEnv {
    pub fn real() -> MacEnv {
        MacEnv { store: MacStore::real(), secret_path: "/var/lib/iyakki/mac-secret".into(), ctl: Arc::new(crate::System), roots: Roots::real(), ssh_connection: std::env::var("SSH_CONNECTION").ok(), host_managed: false, known_wifi: "/var/lib/iyakki/wifi.json".into() }
    }

    /// A stand-in that sees and changes nothing (unit tests of other crates, where the real machine must not be touched).
    pub fn nowhere(ctl: Arc<dyn Control>) -> MacEnv {
        let gone = PathBuf::from("/nonexistent");
        MacEnv { store: MacStore { primary: gone.join("net.conf"), fallback: gone.join("net-mac.conf") }, secret_path: gone.join("mac-secret"), ctl, roots: Roots { sys: gone.join("sys"), proc_: gone.join("proc") }, ssh_connection: None, host_managed: false, known_wifi: gone.join("wifi.json") }
    }

    /// The WiFi network a private address would be made for when none is joined yet: the newest saved one.
    pub fn preferred_ssid(&self) -> Option<String> {
        crate::wifi::KnownStore::new(&self.known_wifi).list().first().map(|k| k.ssid.clone())
    }

    pub fn config(&self) -> MacConfig {
        self.store.load()
    }

    pub fn infos(&self) -> Vec<MacInfo> {
        infos(&*self.ctl, &self.roots, &self.config())
    }

    /// Choose a mode for every adapter of one kind (`iface` None) or for one interface; saved for the next connection.
    pub fn set_mode(&self, kind: Option<LinkKind>, iface: Option<&str>, mode: MacMode) -> Result<PathBuf, String> {
        if self.host_managed {
            return Err("The addresses are managed by the host operating system here.".into());
        }
        let mut cfg = self.config();
        let ls = links(&self.roots);
        match (iface, kind) {
            (Some(n), _) => {
                if !iface_name_ok(n) {
                    return Err(format!("not an interface name: {n:?}"));
                }
                if !ls.iter().any(|l| l.name == n) {
                    return Err(format!("{n}: no such network adapter"));
                }
                cfg.ifaces.insert(n.to_string(), mode);
            }
            (None, Some(k)) => {
                match k {
                    LinkKind::Ethernet => cfg.ethernet = mode,
                    LinkKind::Wifi => cfg.wifi = mode,
                }
                // choosing for the whole kind clears that kind's exceptions
                cfg.ifaces.retain(|n, _| ls.iter().any(|l| &l.name == n && l.kind != k));
            }
            (None, None) => {
                cfg.ethernet = mode;
                cfg.wifi = mode;
                cfg.ifaces.clear();
            }
        }
        self.store.save(&cfg)
    }

    /// The interface carries the session this program was reached through (SSH, or an inbound connection such as the remote
    /// screen), so changing its address would cut it off.
    pub fn carries_session(&self, iface: &str) -> bool {
        let ips: Vec<String> = self.ctl.ipv4().into_iter().filter(|(n, _)| n == iface).map(|(_, a)| a).collect();
        let ssh_ip = self.ssh_connection.as_deref().and_then(|c| c.split_whitespace().nth(2));
        ips.iter().any(|ip| ssh_ip == Some(ip.as_str()) || inbound_sessions(&self.roots, ip) > 0)
    }

    /// Change the address now (the link goes down for a moment). `confirmed` must be true when the interface carries the
    /// session in use. `ssid`: the WiFi network about to be joined, for "per network".
    pub fn apply_now(&self, iface: &str, ssid: Option<&str>, confirmed: bool) -> Result<String, String> {
        if self.host_managed {
            return Err("The addresses are managed by the host operating system here.".into());
        }
        if !iface_name_ok(iface) {
            return Err(format!("not an interface name: {iface:?}"));
        }
        if self.carries_session(iface) && !confirmed {
            return Err(format!("{iface} carries the connection you are using now (SSH or the remote screen); changing its address would drop it. Confirm to go ahead."));
        }
        let (secret, kept) = load_secret(&self.secret_path);
        let cfg = self.config();
        let is_wifi = links(&self.roots).iter().any(|l| l.name == iface && l.kind == LinkKind::Wifi);
        let preferred = if ssid.is_none() && is_wifi { self.preferred_ssid() } else { None };
        match apply_config(&*self.ctl, &self.roots, &cfg, &secret, iface, ssid.or(preferred.as_deref()))? {
            Applied::Unchanged(m) => Ok(format!("{iface} already has {m}.")),
            Applied::Changed { from, to } => Ok(format!("{iface}: {from} -> {to}.{}", if kept { "" } else { " (The device secret could not be saved, so this address is only stable until the next restart.)" })),
        }
    }
}

// ------------------------------------------------------------------------------------------------ command line

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacCmd {
    Show { iface: Option<String>, kind: Option<LinkKind> },
    Set { mode: MacMode, iface: Option<String>, kind: Option<LinkKind> },
    Apply { iface: Option<String>, kind: Option<LinkKind>, yes: bool },
}

pub const USAGE: &str = "usage: iyakki net mac [show|off|per-network|random|apply] [--iface NAME] [--wifi|--ethernet] [--yes]";

/// Parse what follows `iyakki net mac`.
pub fn parse_cmd(args: &[String]) -> Result<MacCmd, String> {
    let mut verb: Option<String> = None;
    let (mut iface, mut kind, mut yes) = (None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--iface" | "-i" => iface = Some(it.next().ok_or("--iface needs an interface name")?.clone()),
            "--wifi" => kind = Some(LinkKind::Wifi),
            "--ethernet" => kind = Some(LinkKind::Ethernet),
            "--yes" | "-y" => yes = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            s => {
                if verb.replace(s.to_string()).is_some() {
                    return Err(USAGE.into());
                }
            }
        }
    }
    if let Some(i) = &iface {
        if !iface_name_ok(i) {
            return Err(format!("not an interface name: {i:?}"));
        }
    }
    match verb.as_deref().unwrap_or("show") {
        "show" | "status" => Ok(MacCmd::Show { iface, kind }),
        "apply" => Ok(MacCmd::Apply { iface, kind, yes }),
        v => MacMode::parse(v).map(|mode| MacCmd::Set { mode, iface, kind }).ok_or_else(|| format!("unknown mode `{v}`\n{USAGE}")),
    }
}

fn wanted(i: &MacInfo, iface: &Option<String>, kind: Option<LinkKind>) -> bool {
    iface.as_ref().is_none_or(|n| *n == i.iface) && kind.is_none_or(|k| k == i.kind)
}

/// Run a parsed command. `admin`: the caller may change settings (root on the machine).
pub fn run_cmd(cmd: &MacCmd, env: &MacEnv, admin: bool) -> Result<String, String> {
    match cmd {
        MacCmd::Show { iface, kind } => {
            let cfg = env.config();
            let mut lines = status_lines(&*env.ctl, &env.roots, &cfg);
            let head = lines.remove(0);
            let rows: Vec<String> = env.infos().iter().zip(lines).filter(|(i, _)| wanted(i, iface, *kind)).map(|(_, l)| l).collect();
            if rows.is_empty() && (iface.is_some() || kind.is_some()) {
                return Err("no such network adapter".into());
            }
            let mut out = vec![head];
            out.extend(rows);
            if env.host_managed {
                out.push("Managed by the host operating system here.".into());
            }
            Ok(out.join("\n"))
        }
        MacCmd::Set { mode, iface, kind } => {
            if !admin {
                return Err("Changing the MAC address setting needs an administrator (run as root).".into());
            }
            let path = env.set_mode(*kind, iface.as_deref(), *mode)?;
            Ok(format!("Private MAC address set to {} ({}); it applies the next time the link is brought up, or run `iyakki net mac apply`.", mode.key(), path.display()))
        }
        MacCmd::Apply { iface, kind, yes } => {
            if !admin {
                return Err("Changing the MAC address needs an administrator (run as root).".into());
            }
            let targets: Vec<MacInfo> = env.infos().into_iter().filter(|i| wanted(i, iface, *kind)).collect();
            if targets.is_empty() {
                return Err("no such network adapter".into());
            }
            let mut out = Vec::new();
            for t in targets {
                out.push(env.apply_now(&t.iface, None, *yes).unwrap_or_else(|e| format!("{}: {e}", t.iface)));
            }
            Ok(out.join("\n"))
        }
    }
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-mac-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// A machine under a temp folder with an Ethernet card and a WiFi adapter, and a fake driver that records what it is asked.
    struct Fake {
        sys: PathBuf,
        calls: Mutex<Vec<String>>,
        perm: Vec<(String, String)>,
        /// Interfaces whose driver refuses a new address.
        refuse: Vec<String>,
        /// Interfaces whose driver accepts the call but keeps the old address.
        ignore: Vec<String>,
        /// Interfaces that will not come back up right after the first change.
        no_up: Vec<String>,
        ips: Vec<(String, String)>,
    }

    impl Fake {
        fn new(d: &Path) -> Fake {
            let sys = d.join("sys");
            for (n, wifi, mac) in [("eno1", false, "00:11:22:33:44:55"), ("wlan0", true, "aa:bb:cc:dd:ee:ff")] {
                put(&sys, &format!("class/net/{n}/type"), "1\n");
                put(&sys, &format!("class/net/{n}/flags"), "0x1003\n");
                put(&sys, &format!("class/net/{n}/carrier"), "1\n");
                put(&sys, &format!("class/net/{n}/address"), &format!("{mac}\n"));
                put(&sys, &format!("class/net/{n}/addr_assign_type"), "0\n");
                put(&sys, &format!("class/net/{n}/device/x"), "");
                if wifi {
                    put(&sys, &format!("class/net/{n}/wireless/x"), "");
                }
            }
            Fake { sys, calls: Mutex::new(vec![]), perm: vec![("eno1".into(), "00:11:22:33:44:55".into()), ("wlan0".into(), "aa:bb:cc:dd:ee:ff".into())], refuse: vec![], ignore: vec![], no_up: vec![], ips: vec![("eno1".into(), "192.168.1.100".into())] }
        }
        fn roots(&self) -> Roots {
            Roots { sys: self.sys.clone(), proc_: self.sys.parent().unwrap().join("proc") }
        }
        fn log(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Control for Fake {
        fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
            let mac_calls = self.calls.lock().unwrap().iter().filter(|c| c.starts_with(&format!("mac {n} "))).count();
            self.calls.lock().unwrap().push(format!("link {n} {}", if up { "up" } else { "down" }));
            if up && self.no_up.iter().any(|x| x == n) && mac_calls == 1 {
                return Err("would not come up".into());
            }
            put(&self.sys, &format!("class/net/{n}/flags"), if up { "0x1003\n" } else { "0x1002\n" });
            Ok(())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            self.ips.clone()
        }
        fn volume(&self, _: u32) -> Option<crate::Volume> {
            None
        }
        fn change_volume(&self, _: u32, _: Option<i32>) -> Result<crate::Volume, String> {
            Err("no".into())
        }
        fn set_volume(&self, _: u32, _: u32) -> Result<crate::Volume, String> {
            Err("no".into())
        }
        fn dhcp(&self, n: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("dhcp {n}"));
            Ok(())
        }
        fn permanent_mac(&self, n: &str) -> Option<String> {
            self.perm.iter().find(|(i, _)| i == n).map(|(_, m)| m.clone())
        }
        fn set_mac(&self, n: &str, mac: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("mac {n} {mac}"));
            if self.refuse.iter().any(|x| x == n) {
                return Err("not supported by this driver".into());
            }
            // a real kernel refuses while the link is up
            let flags = std::fs::read_to_string(self.sys.join("class/net").join(n).join("flags")).unwrap();
            assert!(flags.trim() == "0x1002", "the address must only be changed with the link down");
            if !self.ignore.iter().any(|x| x == n) {
                put(&self.sys, &format!("class/net/{n}/address"), &format!("{mac}\n"));
                put(&self.sys, &format!("class/net/{n}/addr_assign_type"), "3\n");
            }
            Ok(())
        }
    }

    fn secret() -> Vec<u8> {
        (0..32u8).collect()
    }

    fn env_of(d: &Path, ssh: Option<&str>, host: bool) -> MacEnv {
        let f = Fake::new(d);
        MacEnv { store: MacStore { primary: d.join("net.conf"), fallback: d.join("m.conf") }, secret_path: d.join("secret"), roots: f.roots(), ctl: Arc::new(f), ssh_connection: ssh.map(str::to_string), host_managed: host, known_wifi: d.join("wifi.json") }
    }

    #[test]
    fn derived_addresses_are_stable_per_network_differ_across_networks_and_devices_and_are_locally_administered_unicast() {
        let s = secret();
        let a = derive(&s, LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Home");
        assert_eq!(a, derive(&s, LinkKind::Wifi, "AA:BB:CC:DD:EE:FF", "ssid:Home"), "stable (and case of the hardware address does not matter)");
        assert_ne!(a, derive(&s, LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Cafe"), "another network, another address");
        assert_ne!(a, derive(&s, LinkKind::Wifi, "aa:bb:cc:dd:ee:00", "ssid:Home"), "another adapter, another address");
        assert_ne!(a, derive(&[7u8; 32], LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Home"), "another device secret, another address");
        assert_ne!(a, derive(&s, LinkKind::Ethernet, "aa:bb:cc:dd:ee:ff", "ssid:Home"), "the kind of link is part of it");
        let mut seen = std::collections::HashSet::new();
        for i in 0..2000u32 {
            let m = derive(&[i as u8, (i >> 8) as u8, 3, 4], if i % 2 == 0 { LinkKind::Wifi } else { LinkKind::Ethernet }, &format!("00:00:00:00:{:02x}:{:02x}", i >> 8, i & 255), &format!("n{i}"));
            assert!(valid_unicast(&m) && locally_administered(&m) && m[0] & 1 == 0, "{}", fmt_mac(&m));
            seen.insert(m);
        }
        assert!(seen.len() > 1990, "no clustering: {}", seen.len());
        for _ in 0..500 {
            let m = random_mac().unwrap();
            assert!(valid_unicast(&m) && locally_administered(&m) && m[0] & 1 == 0);
        }
        assert_ne!(random_mac().unwrap(), random_mac().unwrap());
        assert!(!valid_unicast(&[0; 6]) && !valid_unicast(&[0xff; 6]) && !valid_unicast(&[1, 2, 3, 4, 5, 6]) && valid_unicast(&[2, 0, 0, 0, 0, 1]) && valid_unicast(&[0, 0x11, 0x22, 0x33, 0x44, 0x55]));
    }

    #[test]
    fn mac_text_is_parsed_strictly_and_printed_in_lower_case() {
        assert_eq!(parse_mac("00:11:22:33:44:5F"), Some([0, 0x11, 0x22, 0x33, 0x44, 0x5f]));
        assert_eq!(fmt_mac(&[0xAB, 1, 2, 3, 4, 5]), "ab:01:02:03:04:05");
        for bad in ["", "00:11:22:33:44", "00:11:22:33:44:55:66", "0:1:2:3:4:5", "zz:11:22:33:44:55", "00-11-22-33-44-55"] {
            assert_eq!(parse_mac(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_secret_is_made_once_kept_owner_only_and_a_failure_to_keep_it_is_reported() {
        let d = tmp("secret");
        let p = d.join("var/mac-secret");
        let (a, kept) = load_secret(&p);
        assert!(kept && a.len() >= 32);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(load_secret(&p).0, a, "the same secret on the next boot");
        // a path that cannot be written
        put(&d, "file", "x");
        let (b, kept) = load_secret(&d.join("file/sub/secret"));
        assert!(!kept && b.len() >= 32, "an unsaved secret still works for this boot");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn modes_parse_and_the_most_specific_choice_wins() {
        assert_eq!(MacMode::parse("Per-Network"), Some(MacMode::PerNetwork));
        assert_eq!(MacMode::parse("per_network"), Some(MacMode::PerNetwork));
        assert_eq!(MacMode::parse("random"), Some(MacMode::Random));
        assert_eq!(MacMode::parse("off"), Some(MacMode::Off));
        assert_eq!(MacMode::parse("maybe"), None);
        let mut c = MacConfig::default();
        assert_eq!((c.ethernet, c.wifi), (MacMode::Off, MacMode::PerNetwork), "defaults");
        c.set_key("mac.iface.wlan1", "random").unwrap();
        c.set_key("mac.ssid.Work Net", "off").unwrap();
        assert_eq!(c.mode_for(LinkKind::Wifi, "wlan0", Some("Home")), MacMode::PerNetwork);
        assert_eq!(c.mode_for(LinkKind::Wifi, "wlan1", Some("Home")), MacMode::Random);
        assert_eq!(c.mode_for(LinkKind::Wifi, "wlan1", Some("Work Net")), MacMode::Off, "the network's own choice beats the interface's");
        assert_eq!(c.mode_for(LinkKind::Ethernet, "eno1", Some("Work Net")), MacMode::Off, "networks only matter for WiFi");
        assert_eq!(c.set_key("wifi.ssid", "x"), Ok(false), "not a mac key");
    }

    #[test]
    fn net_conf_lines_are_read_leniently_written_back_atomically_and_old_files_still_work() {
        // an old file with no mac keys keeps the defaults and everything else is untouched when saving
        let old = "# IYAKKI network configuration.\nwifi.ssid = Home\nwifi.psk = hunter2hunter2\nstatic.eth0 = 10.0.0.2/24 gw 10.0.0.1\ndns = 1.1.1.1\n";
        let (c, skipped) = MacConfig::parse_lenient(old);
        assert_eq!(c, MacConfig::default());
        assert!(skipped.is_empty());
        let mut c = MacConfig::default();
        c.ethernet = MacMode::Random;
        c.ifaces.insert("eth1".into(), MacMode::PerNetwork);
        c.ssids.insert("Cafe".into(), MacMode::Off);
        let text = merge_into_text(old, &c);
        assert!(text.starts_with(old.trim_end()), "{text}");
        assert!(text.contains("mac.ethernet = random\nmac.wifi = per-network\nmac.iface.eth1 = per-network\nmac.ssid.Cafe = off\n"));
        assert_eq!(MacConfig::parse_lenient(&text).0, c, "round trip");
        // saving again replaces rather than appends
        let again = merge_into_text(&text, &c);
        assert_eq!(again, text);
        assert_eq!(again.matches("mac.ethernet").count(), 1);
        // bad lines are skipped and reported, never fatal
        let (c, skipped) = MacConfig::parse_lenient("mac.wifi = sometimes\nmac.iface.a/b = off\nmac.ssid. = off\nmac.nonsense = off\nmac.ethernet = random\nnot a line\n");
        assert_eq!((c.ethernet, c.wifi), (MacMode::Random, MacMode::PerNetwork));
        assert_eq!(skipped.len(), 4, "{skipped:?}");
    }

    #[test]
    fn the_store_writes_net_conf_atomically_falls_back_when_it_is_read_only_and_a_stale_fallback_never_wins() {
        let d = tmp("store");
        let store = MacStore { primary: d.join("etc/net.conf"), fallback: d.join("var/net-mac.conf") };
        put(&d, "etc/net.conf", "wifi.ssid = Home\n");
        assert_eq!(store.load(), MacConfig::default());
        let mut c = MacConfig::default();
        c.wifi = MacMode::Random;
        assert_eq!(store.save(&c).unwrap(), d.join("etc/net.conf"));
        assert_eq!(store.load().wifi, MacMode::Random);
        assert!(std::fs::read_to_string(d.join("etc/net.conf")).unwrap().contains("wifi.ssid = Home"), "other settings kept");
        assert!(std::fs::read_dir(d.join("etc")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("tmp")), "no temp file left");
        // read-only /etc: the primary is a directory, which cannot be replaced by a file
        let ro = MacStore { primary: d.join("ro"), fallback: d.join("var2/net-mac.conf") };
        std::fs::create_dir_all(d.join("ro/x")).unwrap();
        c.ethernet = MacMode::PerNetwork;
        assert_eq!(ro.save(&c).unwrap(), d.join("var2/net-mac.conf"));
        assert_eq!(ro.load().ethernet, MacMode::PerNetwork);
        // after a successful primary save the fallback is dropped
        std::fs::remove_dir_all(d.join("ro")).unwrap();
        c.ethernet = MacMode::Off;
        assert_eq!(ro.save(&c).unwrap(), d.join("ro"));
        assert_eq!(ro.load().ethernet, MacMode::Off);
        assert!(!d.join("var2/net-mac.conf").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_address_is_changed_with_the_link_down_in_order_and_the_link_comes_back() {
        let d = tmp("order");
        let f = Fake::new(&d);
        let t = derive(&secret(), LinkKind::Ethernet, "00:11:22:33:44:55", "iface:eno1");
        let r = apply_mac(&f, &f.roots(), "eno1", &t, Some("00:11:22:33:44:55")).unwrap();
        assert_eq!(r, Applied::Changed { from: "00:11:22:33:44:55".into(), to: fmt_mac(&t) });
        assert_eq!(f.log(), vec!["link eno1 down".to_string(), format!("mac eno1 {}", fmt_mac(&t)), "link eno1 up".to_string()]);
        // the same address again does nothing
        let before = f.log().len();
        assert_eq!(apply_mac(&f, &f.roots(), "eno1", &t, None).unwrap(), Applied::Unchanged(fmt_mac(&t)));
        assert_eq!(f.log().len(), before);
        // a link that was down stays down
        put(&f.sys, "class/net/wlan0/flags", "0x1002\n");
        apply_mac(&f, &f.roots(), "wlan0", &t, None).unwrap();
        assert!(!f.log().iter().any(|c| c == "link wlan0 up" || c == "link wlan0 down"), "{:?}", f.log());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_invalid_address_is_never_set() {
        let d = tmp("invalid");
        let f = Fake::new(&d);
        for bad in [[0u8; 6], [0xff; 6], [1, 2, 3, 4, 5, 6], [0x33, 0, 0, 0, 0, 1]] {
            let e = apply_mac(&f, &f.roots(), "eno1", &bad, None).unwrap_err();
            assert!(e.contains("not a valid unicast"), "{e}");
        }
        assert!(f.log().is_empty(), "the driver was never called: {:?}", f.log());
        assert!(apply_mac(&f, &f.roots(), "nope0", &[2, 0, 0, 0, 0, 1], None).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_driver_that_refuses_leaves_the_address_alone_and_the_link_up_with_a_clear_message() {
        let d = tmp("refuse");
        let mut f = Fake::new(&d);
        f.refuse.push("wlan0".into());
        let e = apply_mac(&f, &f.roots(), "wlan0", &[2, 0, 0, 0, 0, 1], Some("aa:bb:cc:dd:ee:ff")).unwrap_err();
        assert!(e.contains("not supported by this driver") && e.contains("keeps aa:bb:cc:dd:ee:ff"), "{e}");
        assert_eq!(f.log().last().unwrap(), "link wlan0 up", "the link is brought back: {:?}", f.log());
        assert_eq!(current_mac(&f.roots(), "wlan0").unwrap(), "aa:bb:cc:dd:ee:ff");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_driver_that_does_not_keep_the_address_or_a_link_that_will_not_come_up_gets_the_hardware_address_back() {
        let d = tmp("restore");
        let mut f = Fake::new(&d);
        f.ignore.push("eno1".into());
        let e = apply_mac(&f, &f.roots(), "eno1", &[2, 0, 0, 0, 0, 1], Some("00:11:22:33:44:55")).unwrap_err();
        assert!(e.contains("did not keep") && e.contains("hardware address"), "{e}");
        assert!(f.log().contains(&"mac eno1 00:11:22:33:44:55".to_string()), "the permanent address was written back: {:?}", f.log());
        let d2 = tmp("restore2");
        let mut f = Fake::new(&d2);
        f.no_up.push("wlan0".into());
        let e = apply_mac(&f, &f.roots(), "wlan0", &[2, 0, 0, 0, 0, 7], Some("aa:bb:cc:dd:ee:ff")).unwrap_err();
        assert!(e.contains("did not come back") && e.contains("hardware address aa:bb:cc:dd:ee:ff was put back"), "{e}");
        assert_eq!(current_mac(&f.roots(), "wlan0").unwrap(), "aa:bb:cc:dd:ee:ff", "restored");
        assert_eq!(f.log().last().unwrap(), "link wlan0 up");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn the_configured_mode_decides_the_address_off_goes_back_to_hardware_and_overrides_apply() {
        let d = tmp("modes");
        let f = Fake::new(&d);
        let s = secret();
        let mut cfg = MacConfig::default();
        // WiFi default: per network, so the address depends on the SSID and is stable
        let a = apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Home")).unwrap();
        let Applied::Changed { to, .. } = a else { panic!() };
        assert_eq!(to, fmt_mac(&derive(&s, LinkKind::Wifi, "aa:bb:cc:dd:ee:ff", "ssid:Home")));
        // the same network again: nothing to do
        assert!(matches!(apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Home")).unwrap(), Applied::Unchanged(_)));
        // another network: another address
        let Applied::Changed { to: to2, .. } = apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Cafe")).unwrap() else { panic!() };
        assert_ne!(to, to2);
        // Ethernet defaults to Off: the hardware address stays
        assert!(matches!(apply_config(&f, &f.roots(), &cfg, &s, "eno1", None).unwrap(), Applied::Unchanged(m) if m == "00:11:22:33:44:55"));
        // a per-network "off" brings the hardware address back
        cfg.ssids.insert("Work".into(), MacMode::Off);
        let Applied::Changed { to: back, .. } = apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Work")).unwrap() else { panic!() };
        assert_eq!(back, "aa:bb:cc:dd:ee:ff");
        // random changes every time
        cfg.wifi = MacMode::Random;
        let Applied::Changed { to: r1, .. } = apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Home")).unwrap() else { panic!() };
        let Applied::Changed { to: r2, .. } = apply_config(&f, &f.roots(), &cfg, &s, "wlan0", Some("Home")).unwrap() else { panic!() };
        assert_ne!(r1, r2);
        assert!(locally_administered(&parse_mac(&r1).unwrap()));
        // an interface override
        cfg.ifaces.insert("eno1".into(), MacMode::PerNetwork);
        let Applied::Changed { to: e, .. } = apply_config(&f, &f.roots(), &cfg, &s, "eno1", None).unwrap() else { panic!() };
        assert_eq!(e, fmt_mac(&derive(&s, LinkKind::Ethernet, "00:11:22:33:44:55", "iface:eno1")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn what_is_shown_distinguishes_hardware_and_private_addresses() {
        let d = tmp("infos");
        let f = Fake::new(&d);
        let cfg = MacConfig::default();
        let i = infos(&f, &f.roots(), &cfg);
        assert_eq!(i.len(), 2);
        assert!(!i[0].private && i[0].permanent.as_deref() == Some("00:11:22:33:44:55") && !i[0].locally_administered && i[0].short() == "00:11:22:33:44:55 (hardware)");
        assert_eq!(i[0].mode, MacMode::Off);
        assert_eq!(i[1].mode, MacMode::PerNetwork);
        apply_mac(&f, &f.roots(), "wlan0", &[0x02, 1, 2, 3, 4, 5], Some("aa:bb:cc:dd:ee:ff")).unwrap();
        let i = infos(&f, &f.roots(), &cfg);
        assert!(i[1].private && i[1].locally_administered && i[1].short() == "02:01:02:03:04:05 (private)" && i[1].permanent.as_deref() == Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(summary_line(&f.roots()).unwrap(), "MAC: 00:11:22:33:44:55 (hardware)", "the first adapter with a cable is shown");
        assert!(brief(&f.roots(), &cfg)[1].private, "files alone are enough to tell");
        let lines = status_lines(&f, &f.roots(), &cfg);
        assert!(lines[0].contains("Ethernet off") && lines[0].contains("WiFi per-network") && lines[2].contains("private") && lines[2].contains("locally administered"), "{lines:?}");
        let from_files = status_lines_from_files(&f.roots(), &cfg);
        assert_eq!(from_files.len(), 3);
        assert!(from_files[2].contains("wlan0") && from_files[2].contains("private") && from_files[1].contains("hardware address"), "{from_files:?}");
        // a VM's made-up address that the kernel never changed is not called private
        let d2 = tmp("infos2");
        let g = Fake::new(&d2);
        put(&g.sys, "class/net/eno1/address", "52:54:00:12:34:56\n");
        assert!(!brief(&g.roots(), &cfg)[0].private);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn dhcp_never_sends_a_host_name_in_any_mode() {
        let a = dhcp_args("eth0", "/run/udhcpc-eth0.pid");
        assert_eq!(a, vec!["-b", "-R", "-i", "eth0", "-p", "/run/udhcpc-eth0.pid"]);
        for forbidden in ["-x", "-F", "-H", "-h", "-V"] {
            assert!(!a.iter().any(|x| x == forbidden), "{forbidden}");
        }
        assert_eq!(supplicant_lines(MacMode::Off), "");
        assert!(supplicant_lines(MacMode::PerNetwork).contains("preassoc_mac_addr=1") && supplicant_lines(MacMode::Random).contains("gas_rand_mac_addr=1"));
    }

    #[test]
    fn the_session_in_use_is_found_from_ssh_and_from_inbound_connections() {
        let d = tmp("session");
        assert!(!env_of(&d, None, false).carries_session("eno1"), "no session");
        assert!(env_of(&d, Some("192.168.1.5 51000 192.168.1.100 22"), false).carries_session("eno1"), "SSH to this address");
        assert!(!env_of(&d, Some("192.168.1.5 51000 192.168.1.100 22"), false).carries_session("wlan0"), "another adapter");
        // an established connection to a listening port of this address (the remote screen)
        let tcp = "  sl  local_address rem_address   st\n   0: 00000000:1F90 00000000:0000 0A 0 0\n   1: 6401A8C0:1F90 0501A8C0:C738 01 0 0\n   2: 6401A8C0:C000 0901A8C0:0050 01 0 0\n";
        put(&d, "proc/net/tcp", tcp);
        assert_eq!(inbound_sessions(&Roots { sys: d.join("sys"), proc_: d.join("proc") }, "192.168.1.100"), 1, "port 8080 is listening, 49152 is an outgoing connection");
        assert!(env_of(&d, None, false).carries_session("eno1"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn apply_now_asks_first_when_it_would_cut_the_session_and_not_in_a_host_window() {
        let d = tmp("applynow");
        let e = env_of(&d, None, false);
        e.set_mode(Some(LinkKind::Ethernet), None, MacMode::Random).unwrap();
        let ssh = env_of(&d, Some("192.168.1.5 51000 192.168.1.100 22"), false);
        let err = ssh.apply_now("eno1", None, false).unwrap_err();
        assert!(err.contains("carries the connection you are using"), "{err}");
        assert!(ssh.apply_now("eno1", None, true).unwrap().contains("->"));
        // WiFi is not the session's adapter
        assert!(env_of(&d, Some("192.168.1.5 51000 192.168.1.100 22"), false).apply_now("wlan0", Some("Home"), false).unwrap().contains("->"));
        let h = env_of(&d, None, true);
        assert!(h.apply_now("eno1", None, true).unwrap_err().contains("host operating system"));
        assert!(h.set_mode(None, None, MacMode::Off).unwrap_err().contains("host operating system"));
        assert!(e.apply_now("../x", None, true).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn setting_a_mode_for_a_kind_an_interface_or_everything_is_saved() {
        let d = tmp("setmode");
        let env = env_of(&d, None, false);
        env.set_mode(Some(LinkKind::Wifi), None, MacMode::Random).unwrap();
        assert_eq!(env.config().wifi, MacMode::Random);
        env.set_mode(None, Some("eno1"), MacMode::PerNetwork).unwrap();
        assert_eq!(env.config().ifaces.get("eno1"), Some(&MacMode::PerNetwork));
        env.set_mode(Some(LinkKind::Ethernet), None, MacMode::Off).unwrap();
        assert!(env.config().ifaces.is_empty(), "choosing for all Ethernet clears its exceptions");
        env.set_mode(None, None, MacMode::Off).unwrap();
        assert_eq!((env.config().ethernet, env.config().wifi), (MacMode::Off, MacMode::Off));
        assert!(env.set_mode(None, Some("nope0"), MacMode::Off).is_err());
        assert!(env.set_mode(None, Some("a/b"), MacMode::Off).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_command_line_is_parsed() {
        assert_eq!(parse_cmd(&args("")).unwrap(), MacCmd::Show { iface: None, kind: None });
        assert_eq!(parse_cmd(&args("show --wifi")).unwrap(), MacCmd::Show { iface: None, kind: Some(LinkKind::Wifi) });
        assert_eq!(parse_cmd(&args("per-network --iface wlan0")).unwrap(), MacCmd::Set { mode: MacMode::PerNetwork, iface: Some("wlan0".into()), kind: None });
        assert_eq!(parse_cmd(&args("random --ethernet")).unwrap(), MacCmd::Set { mode: MacMode::Random, iface: None, kind: Some(LinkKind::Ethernet) });
        assert_eq!(parse_cmd(&args("off")).unwrap(), MacCmd::Set { mode: MacMode::Off, iface: None, kind: None });
        assert_eq!(parse_cmd(&args("apply --iface eth0 --yes")).unwrap(), MacCmd::Apply { iface: Some("eth0".into()), kind: None, yes: true });
        for bad in ["sometimes", "off on", "--iface", "off --iface a/b", "off --bogus"] {
            assert!(parse_cmd(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_command_shows_sets_and_applies_and_only_for_administrators() {
        let d = tmp("cmd");
        let env = env_of(&d, None, false);
        let shown = run_cmd(&parse_cmd(&args("show")).unwrap(), &env, false).unwrap();
        assert!(shown.contains("eno1") && shown.contains("wlan0") && shown.contains("Ethernet off"), "{shown}");
        let wifi_only = run_cmd(&parse_cmd(&args("show --wifi")).unwrap(), &env, false).unwrap();
        assert!(wifi_only.contains("wlan0") && !wifi_only.contains("eno1"), "{wifi_only}");
        assert!(run_cmd(&parse_cmd(&args("show --iface nope0")).unwrap(), &env, false).is_err());
        assert!(run_cmd(&parse_cmd(&args("random --wifi")).unwrap(), &env, false).unwrap_err().contains("administrator"));
        assert!(!d.join("net.conf").exists(), "nothing was written");
        let set = run_cmd(&parse_cmd(&args("random --wifi")).unwrap(), &env, true).unwrap();
        assert!(set.contains("random") && env.config().wifi == MacMode::Random, "{set}");
        assert!(run_cmd(&parse_cmd(&args("apply")).unwrap(), &env, false).is_err());
        let applied = run_cmd(&parse_cmd(&args("apply --wifi")).unwrap(), &env, true).unwrap();
        assert!(applied.contains("wlan0: aa:bb:cc:dd:ee:ff ->"), "{applied}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
