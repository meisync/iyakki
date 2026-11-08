//! "Hardware firmware" (Setup item `fw-hardware`): the vendor firmware files THIS computer asks for and does not have, fetched from the
//! upstream linux-firmware release at the person's click. The images carry none of them (`dist/buildroot/configs/iyakki_no_firmware.fragment`,
//! `docs/FIRMWARE-LICENCES.md`); nothing is downloaded, copied or installed before the person has read the licence list and clicked.
//!
//! 1. **Detect** ([`parse_log`], [`read_devs`], [`build_plan`]): the kernel log says which files the drivers asked for ("Direct firmware load
//!    for X failed", "firmware: failed to load X", ...); PCI and USB ids say which card that is. Pure functions with fixtures.
//! 2. **Source**: a pinned release of the linux-firmware repository ([`manifest`]: tag, commit, size and SHA-256 of every file, the SHA-256 of
//!    the whole snapshot tarball; made by `dist/scripts/firmware-notes/pin.sh`). Only the needed files and their licence texts are fetched,
//!    one by one from `git.kernel.org` at the pinned commit (https only, redirects to other hosts refused, resumable, every file checked).
//!    Other ways to get the same bytes: a folder or USB stick (a `firmware-extra` folder, an unpacked release, or the release tarball),
//!    the staging folder the developer-mode link fills, or the internet of another computer over a cable (`fwextra_net.rs`).
//! 3. **Install** ([`run`]): the files, their licence texts and the WHENCE entries become the driver pack `fw-hardware` on the data volume
//!    (`iyakki_drivers::apply`: linked into `/lib/firmware` at every boot, the mechanism the other firmware packs use). Drivers that are
//!    safe to reload are reloaded (never the graphics driver); otherwise the result says "restart needed" and the tile offers a Restart
//!    button. Nothing restarts by itself. [`remove`] takes the pack away.
//!
//! The world is reached only through [`Env`]; the tests give it a fake.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use iyakki_drivers::pack::{self, sha256_hex};
use iyakki_drivers::{apply, b64};

use crate::fwpack::{ModState, Outcome};

/// The pinned manifest (`dist/scripts/firmware-notes/pin.sh` writes it).
pub const MANIFEST: &str = include_str!("../../../dist/firmware/manifest.txt");
/// The id of the Setup item, the job (`iyakki quick-install fw-hardware`) and the driver pack.
pub const PACK_ID: &str = "fw-hardware";
/// The release this build is pinned to. `manifest_matches_the_constants` checks the manifest against these.
pub const TAG: &str = "20261019";
pub const COMMIT: &str = "a2dcb924f0897368b0b4f305cbf9808cdda90ce0";
/// SHA-256 of `https://cdn.kernel.org/pub/linux/kernel/firmware/linux-firmware-20261019.tar.gz` (1021468302 bytes), computed from the real download.
pub const SNAPSHOT_SHA256: &str = "f37337e66c1af8ab488972f09ef17efb9b1a2663ed339fc73860936d1af04831";
/// Free space wanted on the data volume besides the files themselves.
pub const NEEDED_FREE: u64 = 64 << 20;
/// A plan bigger than this is refused (the biggest realistic need is a few tens of MB).
pub const MAX_PLAN_BYTES: u64 = 300 << 20;
/// Drivers that may drive the screen: never reloaded, a restart is asked for instead.
pub const GPU_DRIVERS: [&str; 6] = ["i915", "xe", "amdgpu", "radeon", "nouveau", "nvidia"];
/// The words the job prints first.
pub const NOTICE: &str = "These files are third-party firmware under the vendors' own licences (listed per file). They are downloaded from the upstream linux-firmware repository for you, stay on this computer and are not part of any IYAKKI image.";

// ------------------------------------------------------------------------------------------------ the manifest

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub size: u64,
    pub sha256: String,
    /// The licence texts WHENCE names for the file (`LICENCE.iwlwifi_firmware`, `LICENSE.qcom+NOTICE.qcom`, `GPL-2.0`, ...).
    pub tokens: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub tag: String,
    pub commit: String,
    pub base: String,
    pub snapshot_size: u64,
    pub snapshot_sha256: String,
    pub snapshot_url: String,
    /// Licence texts and WHENCE: path -> (size, sha256).
    pub docs: BTreeMap<String, (u64, String)>,
    pub files: BTreeMap<String, FileEntry>,
    pub links: BTreeMap<String, String>,
}

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A path the manifest may name: relative, no dot components, plain characters.
pub fn safe_name(p: &str) -> bool {
    !p.is_empty() && p.len() <= 200 && !p.starts_with('/') && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.starts_with('.') && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.,+".contains(&b)))
}

/// Read a manifest text. Strict about what it keeps: a malformed line is an error; a path with odd characters is left out.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let mut m = Manifest::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        let bad = |why: &str| format!("firmware manifest, line {}: {why}", n + 1);
        match w.as_slice() {
            ["tag", t] if t.bytes().all(|b| b.is_ascii_digit()) => m.tag = t.to_string(),
            ["commit", c] if hex(c, 40) => m.commit = c.to_string(),
            ["base", b] if b.starts_with("https://") => m.base = b.trim_end_matches('/').to_string(),
            ["snapshot", size, sha, url] if hex(sha, 64) && url.starts_with("https://") => {
                m.snapshot_size = size.parse().map_err(|_| bad("the snapshot size is not a number"))?;
                m.snapshot_sha256 = sha.to_string();
                m.snapshot_url = url.to_string();
            }
            ["doc", size, sha, path] => {
                if !hex(sha, 64) {
                    return Err(bad("a sha256 is not valid"));
                }
                if safe_name(path) {
                    m.docs.insert(path.to_string(), (size.parse().map_err(|_| bad("a size is not a number"))?, sha.to_string()));
                }
            }
            ["file", size, sha, tokens, path] => {
                let size: u64 = size.parse().map_err(|_| bad("a size is not a number"))?;
                if !hex(sha, 64) || size == 0 {
                    return Err(bad("a size or a sha256 is not valid"));
                }
                if safe_name(path) {
                    m.files.insert(path.to_string(), FileEntry { size, sha256: sha.to_string(), tokens: tokens.split('+').map(str::to_string).collect() });
                }
            }
            ["link", path, target] => {
                if safe_name(path) && !target.starts_with('/') {
                    m.links.insert(path.to_string(), target.to_string());
                }
            }
            _ => return Err(bad("not understood")),
        }
    }
    if m.tag.is_empty() || m.commit.is_empty() || m.base.is_empty() || m.snapshot_sha256.is_empty() || m.files.is_empty() || !m.docs.contains_key("WHENCE") {
        return Err("firmware manifest: a tag, a commit, a base address, the snapshot hash, WHENCE and files are required".into());
    }
    Ok(m)
}

pub fn manifest() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| parse_manifest(MANIFEST).expect("the pinned firmware manifest is valid"))
}

/// Where a requested name really lives in the release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The name the driver asked for.
    pub requested: String,
    /// The file in the release (the same name unless the request is a link).
    pub path: String,
}

fn normalise(p: &str) -> Option<String> {
    let mut v: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                v.pop()?;
            }
            c => v.push(c),
        }
    }
    (!v.is_empty()).then(|| v.join("/"))
}

impl Manifest {
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}?id={}", self.base, path, self.commit)
    }

    /// Follow links (at most four) to a file of the release.
    pub fn resolve(&self, name: &str) -> Option<Resolved> {
        if !safe_name(name) {
            return None;
        }
        let mut cur = name.to_string();
        for _ in 0..5 {
            if self.files.contains_key(&cur) {
                return Some(Resolved { requested: name.to_string(), path: cur });
            }
            let t = self.links.get(&cur)?;
            let dir = cur.rsplit_once('/').map_or("", |x| x.0);
            cur = normalise(&if dir.is_empty() { t.clone() } else { format!("{dir}/{t}") })?;
        }
        None
    }
}

// ------------------------------------------------------------------------------------------------ detection: the kernel log

/// A file a driver asked for and did not get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    /// The driver that asked (the first word of the log line), empty when unknown.
    pub driver: String,
    /// The PCI address or USB path in the line, empty when none.
    pub device: String,
}

fn line_body(line: &str) -> &str {
    if let Some(i) = line.find("kernel: ") {
        return &line[i + 8..];
    }
    let l = line.trim_start();
    if l.starts_with('[') {
        if let Some(i) = l.find("] ") {
            return &l[i + 2..];
        }
    }
    l
}

fn driver_and_device(body: &str) -> (String, String) {
    let mut w = body.split_whitespace();
    let (Some(a), b) = (w.next(), w.next()) else { return (String::new(), String::new()) };
    let drv = a.trim_end_matches(':');
    if drv.is_empty() || drv.contains('/') || drv.len() > 40 || a.starts_with("Direct") || a.starts_with("firmware") {
        return (String::new(), String::new());
    }
    let dev = b.map(|x| x.trim_end_matches(':')).filter(|x| x.contains(':') && (x.contains('.') || x.contains('-'))).unwrap_or("");
    (drv.to_string(), dev.to_string())
}

fn clean_token(t: &str) -> &str {
    t.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']' | '<' | '>'))
}

fn token_after<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let i = body.find(key)?;
    let t = clean_token(body[i + key.len()..].split_whitespace().next()?);
    (!t.is_empty()).then_some(t)
}

/// The files the kernel log says drivers could not get, in the order of the log, each once. Names that are not in the release are kept
/// when the line says so plainly ("Direct firmware load for X failed"); a looser line counts only when a word of it is a file of the release.
pub fn parse_log(text: &str, m: &Manifest) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::new();
    let mut add = |name: &str, drv: &str, dev: &str| {
        if safe_name(name) && !out.iter().any(|r| r.name == name) {
            out.push(Request { name: name.to_string(), driver: drv.to_string(), device: dev.to_string() });
        }
    };
    for line in text.lines() {
        let body = line_body(line);
        let (drv, dev) = driver_and_device(body);
        if let Some(t) = token_after(body, "Direct firmware load for ") {
            if body.contains("failed") {
                add(t, &drv, &dev);
            }
            continue;
        }
        if let Some(t) = token_after(body, "firmware: failed to load ") {
            add(t, &drv, &dev);
            continue;
        }
        if let Some(t) = token_after(body, "Falling back to sysfs fallback for: ") {
            add(t, &drv, &dev);
            continue;
        }
        let low = body.to_lowercase();
        if low.contains("firmware") && ["fail", "missing", "can't load", "cannot load", "unable to", "not found", "error -2"].iter().any(|k| low.contains(k)) {
            for w in body.split_whitespace() {
                let t = clean_token(w);
                if t.contains('.') && m.resolve(t).is_some() {
                    add(t, &drv, &dev);
                }
            }
        }
    }
    out
}

/// Versions of one file the driver tries from the newest down (`.ucode` files): the family of a name, `None` when it has none.
pub fn family_key(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".ucode")?;
    let digits = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    (digits > 0 && digits < stem.len() && matches!(stem.as_bytes()[stem.len() - digits - 1], b'-' | b'_')).then(|| format!("{}*.ucode", &stem[..stem.len() - digits]))
}

/// A request with what the release has for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub request: Request,
    pub resolved: Option<Resolved>,
    /// The name as the card shows it (`iwlwifi-cc-a0-*.ucode` for a family).
    pub shown: String,
}

/// Which of the requests are needs: for a family of versions only the first request the release can serve (the driver tries the newest first
/// and uses the first it finds); the others are alternatives and are not installed. Requests the release has no file for are listed with
/// `resolved: None`.
pub fn needs(reqs: &[Request], m: &Manifest) -> (Vec<Need>, Vec<String>) {
    let mut out: Vec<Need> = Vec::new();
    let mut alternatives = Vec::new();
    let mut taken: BTreeSet<(String, String)> = BTreeSet::new();
    for r in reqs {
        let resolved = m.resolve(&r.name);
        match (family_key(&r.name), &resolved) {
            (Some(k), Some(_)) => {
                if !taken.insert((r.driver.clone(), k.clone())) {
                    alternatives.push(r.name.clone());
                    continue;
                }
                out.push(Need { request: r.clone(), resolved, shown: k });
            }
            (Some(k), None) => {
                // a version the release does not have: it only counts when no other version of the family is served
                if reqs.iter().any(|o| o.name != r.name && family_key(&o.name).as_deref() == Some(&k) && m.resolve(&o.name).is_some()) {
                    alternatives.push(r.name.clone());
                } else if taken.insert((r.driver.clone(), k.clone())) {
                    out.push(Need { request: r.clone(), resolved, shown: k });
                }
            }
            (None, _) => out.push(Need { request: r.clone(), resolved, shown: r.name.clone() }),
        }
    }
    (out, alternatives)
}

// ------------------------------------------------------------------------------------------------ detection: PCI and USB ids

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dev {
    /// "pci" or "usb".
    pub bus: &'static str,
    /// The sysfs name (`0000:00:14.3`, `1-2`).
    pub id: String,
    pub vendor: u16,
    pub device: u16,
    /// The PCI class code (`0x028000`), 0 for USB.
    pub class: u32,
    /// The bound driver's name, empty when none.
    pub driver: String,
}

fn rd(d: &Path, f: &str) -> String {
    std::fs::read_to_string(d.join(f)).unwrap_or_default().trim().to_string()
}

fn hex_u(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

fn driver_of(d: &Path) -> String {
    std::fs::read_link(d.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()
}

/// PCI and USB devices under a `/sys` folder.
pub fn read_devs(sys: &Path) -> Vec<Dev> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten() {
        let d = e.path();
        if let (Some(ven), Some(dev)) = (hex_u(&rd(&d, "vendor")), hex_u(&rd(&d, "device"))) {
            v.push(Dev { bus: "pci", id: e.file_name().to_string_lossy().into_owned(), vendor: ven as u16, device: dev as u16, class: hex_u(&rd(&d, "class")).unwrap_or(0), driver: driver_of(&d) });
        }
    }
    for e in std::fs::read_dir(sys.join("bus/usb/devices")).into_iter().flatten().flatten() {
        let d = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.contains(':') {
            continue; // an interface
        }
        if let (Some(ven), Some(prod)) = (hex_u(&rd(&d, "idVendor")), hex_u(&rd(&d, "idProduct"))) {
            let mut driver = driver_of(&d);
            if driver.is_empty() || driver == "usb" {
                driver = std::fs::read_dir(&d).into_iter().flatten().flatten().filter(|x| x.file_name().to_string_lossy().starts_with(&format!("{name}:"))).map(|x| driver_of(&x.path())).find(|s| !s.is_empty()).unwrap_or_default();
            }
            let class = hex_u(&rd(&d, "bDeviceClass")).unwrap_or(0) << 16;
            v.push(Dev { bus: "usb", id: name, vendor: ven as u16, device: prod as u16, class, driver });
        }
    }
    v.sort_by(|a, b| (a.bus, &a.id).cmp(&(b.bus, &b.id)));
    v
}

pub fn vendor_name(v: u16) -> &'static str {
    match v {
        0x8086 | 0x8087 => "Intel",
        0x10ec | 0x0bda => "Realtek",
        0x14c3 => "MediaTek",
        0x168c | 0x0cf3 => "Qualcomm Atheros",
        0x17cb => "Qualcomm",
        0x14e4 | 0x0a5c => "Broadcom",
        0x1002 => "AMD",
        0x10de => "NVIDIA",
        0x1814 => "Ralink",
        0x11ab => "Marvell",
        _ => "",
    }
}

/// Marketing names of a few common cards (from the Linux PCI id database; a hint for the card, never used to decide anything).
pub fn known_name(vendor: u16, device: u16) -> Option<&'static str> {
    Some(match (vendor, device) {
        (0x8086, 0x2723) => "Wi-Fi 6 AX200",
        (0x8086, 0x2725) => "Wi-Fi 6E AX210",
        (0x8086, 0xa0f0) => "Wi-Fi 6 AX201",
        (0x8086, 0x2526) => "Wireless-AC 9260",
        (0x8086, 0x24fd) => "Wireless-AC 8265",
        (0x8086, 0x095a) => "Wireless 7265",
        (0x14c3, 0x7961) => "MT7921 Wi-Fi",
        (0x14c3, 0x0616) => "MT7922 Wi-Fi",
        (0x168c, 0x003e) => "QCA6174 Wi-Fi",
        (0x17cb, 0x1103) => "WCN6855 Wi-Fi",
        (0x10ec, 0xc821) => "RTL8821CE Wi-Fi",
        (0x10ec, 0x8852) => "RTL8852AE Wi-Fi",
        _ => return None,
    })
}

pub fn class_label(d: &Dev) -> &'static str {
    match d.class >> 8 {
        0x0280 => "Wi-Fi card",
        0x0200 => "Ethernet card",
        0x0300 | 0x0302 | 0x0380 => "graphics card",
        0x0403 => "audio device",
        _ if d.bus == "usb" && d.class >> 16 == 0xe0 => "Bluetooth adapter",
        _ => "device",
    }
}

/// "your Wi-Fi card Intel Wi-Fi 6 AX200 (8086:2723)" for a device.
pub fn dev_phrase(d: &Dev) -> String {
    let name = known_name(d.vendor, d.device).map(|n| format!("{} {n}", vendor_name(d.vendor))).unwrap_or_else(|| vendor_name(d.vendor).to_string());
    format!("your {}{} ({:04x}:{:04x})", class_label(d), if name.trim().is_empty() { String::new() } else { format!(" {}", name.trim()) }, d.vendor, d.device)
}

/// The device a request belongs to: the address in the log line, else a device bound to the driver that asked.
pub fn owner<'a>(r: &Request, devs: &'a [Dev]) -> Option<&'a Dev> {
    if !r.device.is_empty() {
        if let Some(d) = devs.iter().find(|d| d.id == r.device) {
            return Some(d);
        }
    }
    if !r.driver.is_empty() {
        return devs.iter().find(|d| d.driver == r.driver);
    }
    None
}

// ------------------------------------------------------------------------------------------------ vendors and licences

/// (path prefix, group key, group title). The first match wins.
const FAMILIES: &[(&str, &str, &str)] = &[
    ("intel/iwlwifi/", "intel-wifi", "Intel Wi-Fi"),
    ("iwlwifi-", "intel-wifi", "Intel Wi-Fi"),
    ("intel/ibt-", "intel-bt", "Intel Bluetooth"),
    ("i915/", "intel-gpu", "Intel graphics"),
    ("xe/", "intel-gpu", "Intel graphics"),
    ("intel/ice/", "intel-eth", "Intel Ethernet"),
    ("e100/", "intel-eth", "Intel Ethernet"),
    ("intel/", "intel-other", "Intel (other)"),
    ("amdgpu/", "amd-gpu", "AMD graphics"),
    ("radeon/", "amd-gpu", "AMD graphics"),
    ("amd/", "amd-other", "AMD (other)"),
    ("nvidia/", "nvidia", "NVIDIA graphics"),
    ("rtw88/", "realtek-wifi", "Realtek Wi-Fi"),
    ("rtw89/", "realtek-wifi", "Realtek Wi-Fi"),
    ("rtlwifi/", "realtek-wifi", "Realtek Wi-Fi"),
    ("rtl_bt/", "realtek-bt", "Realtek Bluetooth"),
    ("rtl_nic/", "realtek-eth", "Realtek Ethernet"),
    ("mediatek/", "mediatek", "MediaTek Wi-Fi and Bluetooth"),
    ("brcm/", "broadcom-wifi", "Broadcom Wi-Fi and Bluetooth"),
    ("cypress/", "broadcom-wifi", "Broadcom Wi-Fi and Bluetooth"),
    ("ath9k_htc/", "atheros", "Atheros Wi-Fi"),
    ("ath10k/", "qca-wifi", "Qualcomm Atheros Wi-Fi"),
    ("ath11k/", "qca-wifi", "Qualcomm Atheros Wi-Fi"),
    ("ath12k/", "qca-wifi", "Qualcomm Atheros Wi-Fi"),
    ("ar3k/", "qca-bt", "Qualcomm Atheros Bluetooth"),
    ("ath3k", "qca-bt", "Qualcomm Atheros Bluetooth"),
    ("ath6k/", "qca-wifi", "Qualcomm Atheros Wi-Fi"),
    ("ti-connectivity/", "ti-wireless", "Texas Instruments Wi-Fi and Bluetooth"),
    ("qed/", "server-eth", "Server network cards (QLogic, Chelsio, Mellanox, Netronome)"),
    ("cxgb3/", "server-eth", "Server network cards (QLogic, Chelsio, Mellanox, Netronome)"),
    ("cxgb4/", "server-eth", "Server network cards (QLogic, Chelsio, Mellanox, Netronome)"),
    ("mellanox/", "server-eth", "Server network cards (QLogic, Chelsio, Mellanox, Netronome)"),
    ("netronome/", "server-eth", "Server network cards (QLogic, Chelsio, Mellanox, Netronome)"),
    ("qca/", "qca-bt", "Qualcomm Atheros Bluetooth"),
    ("tigon/", "broadcom-eth", "Broadcom Ethernet"),
    ("bnx2x/", "broadcom-eth", "Broadcom Ethernet"),
    ("bnx2/", "broadcom-eth", "Broadcom Ethernet"),
    ("mrvl/", "marvell", "Marvell"),
    ("libertas/", "marvell", "Marvell"),
    ("regulatory.db", "regdb", "Wireless regulatory database"),
    ("rt2", "ralink", "Ralink Wi-Fi"),
    ("ar9", "atheros", "Atheros Wi-Fi"),
    ("carl9170", "atheros", "Atheros Wi-Fi"),
    ("cirrus/", "audio", "Audio codecs"),
    ("ti/", "audio", "Audio codecs"),
    ("sof", "audio", "Audio codecs"),
];

/// (group key, title) of a release path.
pub fn family_of(path: &str) -> (String, String) {
    for (p, k, t) in FAMILIES {
        if path.starts_with(p) {
            return (k.to_string(), t.to_string());
        }
    }
    let top = path.split('/').next().unwrap_or(path);
    (format!("other-{top}"), format!("Other ({top})"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Free to redistribute with notice (binary, unmodified, copyright text kept, no reverse engineering).
    Free,
    /// An old two-line permission ("hexadecimal or equivalent format, with this notice").
    OldWording,
    /// A vendor agreement: read the full text.
    Agreement,
    /// Restricted to the vendor's hardware.
    VendorOnly,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Free => "Free to redistribute with notice",
            Class::OldWording => "Old permission: firmware data may be distributed with the copyright notice",
            Class::Agreement => "A vendor agreement: read the full text",
            Class::VendorOnly => "Restricted to the vendor's hardware",
        }
    }
}

/// What a licence token says, in one line, and its class. Read from the texts of the pinned release; the full text is kept next to the files.
pub fn token_note(token: &str) -> (Class, &'static str) {
    const FREE_BIN: &str = "binary redistribution without modification, copyright and disclaimer kept with the files, no reverse engineering";
    match token {
        "LICENCE.mediatek" => (Class::VendorOnly, "for devices with MediaTek chipsets only, and not as part of the Linux kernel image or under the GPL"),
        "LICENSE.airoha" => (Class::VendorOnly, "for devices with Airoha chipsets only, and not as part of the Linux kernel image or under the GPL"),
        "LICENCE.Marvell" => (Class::VendorOnly, "only with Marvell silicon products, no reverse engineering"),
        "LICENSE.QualcommAtheros_ath10k" | "LICENSE.QualcommAtheros_ar3k" => (Class::VendorOnly, "solely in conjunction with a Qualcomm Atheros chipset, unmodified, notice kept, no reverse engineering"),
        "LICENSE.qcom" | "LICENSE.qcom-2" | "LICENSE.dell" => (Class::VendorOnly, "a Qualcomm (or Dell) agreement: only with their chipsets, and the files may not be put under third-party (for example open-source) licence terms"),
        "LICENCE.broadcom_bcm43xx" => (Class::VendorOnly, "solely for Broadcom integrated circuits, complete and unmodified, the agreement text must accompany the files"),
        "LICENCE.cypress" => (Class::VendorOnly, "solely in connection with Cypress products, unmodified, the agreement terminates on breach"),
        "LICENCE.nvidia" => (Class::VendorOnly, "only with processors designed by NVIDIA; redistribution only for operating systems under an OSI-approved open-source licence, binaries unmodified, a copy of this licence with the files; no reverse engineering"),
        "LICENCE.tigon" | "LICENCE.bnx2" | "LICENCE.bnx2x" | "LICENSE.r8169" => (Class::OldWording, "firmware data derived from the vendor's proprietary source, distribution permitted in hexadecimal or equivalent format with the copyright notice"),
        "LICENSE.amdgpu" | "LICENSE.radeon" | "LICENSE.amd-ucode" | "LICENSE.amd-sev" => (Class::Free, "free of licence fees, binary only, notice reproduced, no reverse engineering"),
        "LICENCE.iwlwifi_firmware" | "LICENCE.ibt_firmware" | "LICENSE.i915" | "LICENSE.xe" | "LICENSE.ice" | "LICENSE.intel" | "LICENCE.rtlwifi_firmware.txt" | "LICENCE.atheros_firmware" | "LICENCE.ralink-firmware.txt" | "LICENCE.ralink_a_mediatek_company_firmware" | "LICENCE.ti-tspa" => (Class::Free, FREE_BIN),
        "LICENCE.e100" | "LICENSE.mellanox" | "LICENCE.open-ath9k-htc-firmware" => (Class::Free, "BSD-style licence: notice kept"),
        t if t.starts_with("GPL") || t == "MIT" || t == "Apache-2.0" || t == "ISC" || t.starts_with("BSD") => (Class::Free, "an open-source licence for this file (see the licence text)"),
        _ => (Class::Agreement, "a vendor licence: the full text is kept next to the files"),
    }
}

/// The licence text paths in the release for a file's tokens (`LICENSES/<token>`, only those the manifest holds).
pub fn doc_paths(tokens: &[String], m: &Manifest) -> Vec<String> {
    let mut v = Vec::new();
    for t in tokens {
        let p = format!("LICENSES/{t}");
        if m.docs.contains_key(&p) {
            v.push(p);
        } else if let Some(g) = t.strip_prefix("GPLv2").map(|_| "LICENSES/GPL-2.0".to_string()).filter(|g| m.docs.contains_key(g)) {
            v.push(g);
        }
    }
    v
}

// ------------------------------------------------------------------------------------------------ the plan

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub requested: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub tokens: Vec<String>,
    pub driver: String,
    /// "your Wi-Fi card Intel Wi-Fi 6 AX200 (8086:2723) needs iwlwifi-cc-a0-*.ucode"
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub title: String,
    /// Shown and accepted on its own (NVIDIA): installed only when the person includes it.
    pub separate: bool,
    pub files: Vec<PlanFile>,
    /// What the group's licences say: (token, class, line), one per token.
    pub licences: Vec<(String, Class, &'static str)>,
    /// What the hardware of the group needs besides the file (shown for NVIDIA).
    pub needs: String,
}

impl Group {
    pub fn class(&self) -> Class {
        self.licences.iter().map(|l| l.1).max().unwrap_or(Class::Agreement)
    }
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub groups: Vec<Group>,
    /// Requested names the pinned release has no file for.
    pub unknown: Vec<String>,
    /// Older or newer versions of a family that the driver tries after the first and that are not needed.
    pub alternatives: Vec<String>,
}

pub const NVIDIA_NEEDS: &str = "An NVIDIA graphics card of the Turing generation (GeForce GTX 16xx / RTX 20xx) or newer, and the open nouveau driver using the GSP firmware. The files are licensed only for use with NVIDIA processors, only for open-source operating systems, unmodified and with the licence text. Without them the card runs on the basic screen driver or a slower fallback.";

/// Group the needs by vendor, with the licence of every file and a plain reason per file.
pub fn build_plan(reqs: &[Request], devs: &[Dev], m: &Manifest) -> Plan {
    let (ns, alternatives) = needs(reqs, m);
    let mut plan = Plan { alternatives, ..Default::default() };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for n in ns {
        let Some(r) = &n.resolved else {
            plan.unknown.push(n.request.name.clone());
            continue;
        };
        let (key, title) = family_of(&r.path);
        let e = &m.files[&r.path];
        let why = match owner(&n.request, devs) {
            Some(d) => format!("{} needs {}", dev_phrase(d), n.shown),
            None if !n.request.driver.is_empty() => format!("the {} driver asked for {}", n.request.driver, n.shown),
            None => format!("a driver asked for {}", n.shown),
        };
        if !seen.insert(format!("{}|{}", r.requested, r.path)) {
            continue;
        }
        let f = PlanFile { requested: r.requested.clone(), path: r.path.clone(), size: e.size, sha256: e.sha256.clone(), tokens: e.tokens.clone(), driver: n.request.driver.clone(), why };
        let gi = match plan.groups.iter().position(|g| g.key == key) {
            Some(i) => i,
            None => {
                plan.groups.push(Group { key: key.clone(), title, separate: key == "nvidia", files: Vec::new(), licences: Vec::new(), needs: if key == "nvidia" { NVIDIA_NEEDS.to_string() } else { String::new() } });
                plan.groups.len() - 1
            }
        };
        let g = &mut plan.groups[gi];
        for t in &f.tokens {
            if !g.licences.iter().any(|l| &l.0 == t) {
                let (c, line) = token_note(t);
                g.licences.push((t.clone(), c, line));
            }
        }
        g.files.push(f);
    }
    plan.groups.sort_by(|a, b| (a.separate, &a.title).cmp(&(b.separate, &b.title)));
    plan
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The files of the groups that are not separate, plus the separate groups named in `with`.
    pub fn select(&self, with: &[String]) -> Selection {
        let mut s = Selection::default();
        for g in self.groups.iter().filter(|g| !g.separate || with.contains(&g.key)) {
            for f in &g.files {
                s.files.push(f.clone());
            }
        }
        s
    }

    /// The lines the card and the CLI list before the click: per vendor the licence class, one line per licence, then the files.
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        for g in &self.groups {
            v.push(format!("{}{}: {} file(s), {} KB. {}", g.title, if g.separate { " (separate, you choose it on its own)" } else { "" }, g.files.len(), g.bytes() >> 10, g.class().label()));
            for (t, c, line) in &g.licences {
                v.push(format!("    licence {t} ({}): {line}", c.label()));
            }
            if !g.needs.is_empty() {
                v.push(format!("    it needs: {}", g.needs));
            }
            let mut shown: BTreeSet<&str> = BTreeSet::new();
            for f in &g.files {
                if shown.insert(f.why.as_str()) {
                    v.push(format!("    {}", f.why));
                }
            }
        }
        for u in &self.unknown {
            v.push(format!("not in linux-firmware {TAG}: {u} (nothing to download for it)"));
        }
        v
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub files: Vec<PlanFile>,
}

impl Selection {
    /// The distinct files in the release (a link and its target are one file).
    pub fn unique(&self) -> BTreeMap<String, (u64, String)> {
        self.files.iter().map(|f| (f.path.clone(), (f.size, f.sha256.clone()))).collect()
    }

    /// Licence texts of the selected files plus WHENCE.
    pub fn docs(&self, m: &Manifest) -> BTreeMap<String, (u64, String)> {
        let mut d = BTreeMap::new();
        for f in &self.files {
            for p in doc_paths(&f.tokens, m) {
                if let Some(e) = m.docs.get(&p) {
                    d.insert(p, e.clone());
                }
            }
            for t in &f.tokens {
                // NOTICE.* files named next to a licence
                let p = format!("LICENSES/{t}");
                if let Some(e) = m.docs.get(&p) {
                    d.insert(p, e.clone());
                }
            }
        }
        if let Some(e) = m.docs.get("WHENCE") {
            d.insert("WHENCE".into(), e.clone());
        }
        d
    }

    pub fn bytes(&self, m: &Manifest) -> u64 {
        self.unique().values().map(|e| e.0).sum::<u64>() + self.docs(m).values().map(|e| e.0).sum::<u64>()
    }

    /// What the person agrees to: the release, every file with its hash and the licence texts. A new list or a new pin asks again.
    pub fn digest(&self, m: &Manifest) -> String {
        let mut s = format!("fw-hardware {} {}\n", m.tag, m.commit);
        for (p, (size, sha)) in self.unique() {
            s.push_str(&format!("file {p} {size} {sha}\n"));
        }
        for (p, (size, sha)) in self.docs(m) {
            s.push_str(&format!("doc {p} {size} {sha}\n"));
        }
        sha256_hex(s.as_bytes())
    }
}

/// The blocks of WHENCE that name any of the paths (as File, RawFile, or as the link or its target), with a header.
pub fn whence_excerpt(whence: &str, paths: &[String]) -> String {
    let mut out = format!("WHENCE entries of linux-firmware {TAG} for the files installed here (the whole file: WHENCE in the release)\n\n");
    let mut block: Vec<&str> = Vec::new();
    let flush = |block: &mut Vec<&str>, out: &mut String| {
        let hit = block.iter().any(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("File:").or_else(|| l.strip_prefix("RawFile:")).or_else(|| l.strip_prefix("Link:"));
            rest.is_some_and(|r| r.split_whitespace().any(|w| paths.iter().any(|p| p == w)))
        });
        if hit {
            out.push_str(&block.join("\n"));
            out.push_str("\n\n");
        }
        block.clear();
    };
    for l in whence.lines() {
        if l.starts_with("Driver:") || l.starts_with("-----") {
            flush(&mut block, &mut out);
            if l.starts_with("-----") {
                continue;
            }
        }
        block.push(l);
    }
    flush(&mut block, &mut out);
    out
}

// ------------------------------------------------------------------------------------------------ acceptance

fn work_dir(data: &Path) -> PathBuf {
    data.join("fwextra")
}
pub fn result_path(data: &Path) -> PathBuf {
    work_dir(data).join("last.json")
}
fn accepted_path(data: &Path) -> PathBuf {
    work_dir(data).join("accepted.json")
}
fn download_dir(data: &Path) -> PathBuf {
    work_dir(data).join("download")
}
fn seen_path(data: &Path) -> PathBuf {
    work_dir(data).join("seen.txt")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Acceptance {
    pub by: String,
    pub at: i64,
    pub digest: String,
    pub tag: String,
}

/// Record that `by` read the licence list of this selection and accepts it.
pub fn accept(data: &Path, sel: &Selection, by: &str, now: i64) -> Result<(), String> {
    accept_in(manifest(), data, sel, by, now)
}

pub fn accept_in(m: &Manifest, data: &Path, sel: &Selection, by: &str, now: i64) -> Result<(), String> {
    let a = Acceptance { by: by.to_string(), at: now, digest: sel.digest(m), tag: m.tag.clone() };
    std::fs::create_dir_all(work_dir(data)).map_err(|e| format!("{}: {e}", work_dir(data).display()))?;
    std::fs::write(accepted_path(data), serde_json::to_string_pretty(&a).unwrap_or_default()).map_err(|e| e.to_string())
}

/// The acceptance on record, when it is for exactly this selection of the current pin.
pub fn accepted(data: &Path, sel: &Selection) -> Option<Acceptance> {
    accepted_in(manifest(), data, sel)
}

pub fn accepted_in(m: &Manifest, data: &Path, sel: &Selection) -> Option<Acceptance> {
    let a: Acceptance = serde_json::from_str(&std::fs::read_to_string(accepted_path(data)).ok()?).ok()?;
    (a.digest == sel.digest(m)).then_some(a)
}

pub fn read_outcome(data: &Path) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(result_path(data)).ok()?).ok()
}

fn write_outcome(data: &Path, o: &Outcome) {
    let _ = std::fs::create_dir_all(work_dir(data));
    let _ = std::fs::write(result_path(data), serde_json::to_string_pretty(o).unwrap_or_default());
}

// ------------------------------------------------------------------------------------------------ folder and tarball sources

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Download,
    /// A folder or stick: a `firmware-extra` folder, an unpacked release, the staging folder, or a folder with the release tarball.
    Folder(PathBuf),
}

impl Source {
    pub fn name(&self) -> &'static str {
        match self {
            Source::Download => "download",
            Source::Folder(_) => "folder",
        }
    }
}

/// Where the staging folder is: the developer-mode link pushes into `<home>/firmware-extra`.
pub fn staging_dir(home: &Path) -> PathBuf {
    home.join("firmware-extra")
}

fn is_plain_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_file()).unwrap_or(false)
}

/// The roots a folder is searched under.
fn folder_roots(dir: &Path) -> Vec<PathBuf> {
    vec![dir.to_path_buf(), dir.join("firmware-extra"), dir.join(format!("linux-firmware-{TAG}")), dir.join("linux-firmware"), dir.join("firmware")]
}

pub fn find_in_folder(dir: &Path, rel: &str) -> Option<PathBuf> {
    if !safe_name(rel) {
        return None;
    }
    folder_roots(dir).into_iter().map(|r| r.join(rel)).find(|p| is_plain_file(p))
}

/// What is wrong with a folder before anything is read from it: links (never followed), odd names, oversized files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub entries: usize,
    pub rejected: Vec<(String, String)>,
    pub tarball: Option<PathBuf>,
}

pub fn scan_folder(dir: &Path) -> Scan {
    let mut s = Scan::default();
    fn walk(d: &Path, rel: &str, depth: usize, s: &mut Scan) {
        if depth > 8 || s.entries > 60_000 {
            return;
        }
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            s.entries += 1;
            let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
            let ft = md.file_type();
            if ft.is_symlink() {
                s.rejected.push((r, "a link: links are never followed".into()));
            } else if !(name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.,+ ".contains(&b)) && !name.starts_with("..")) {
                s.rejected.push((r, "an unusual name".into()));
            } else if ft.is_dir() {
                walk(&e.path(), &r, depth + 1, s);
            } else if md.len() > (1 << 30) + (1 << 29) && !name.ends_with(".tar.gz") {
                s.rejected.push((r, "far larger than any firmware file".into()));
            } else if name == format!("linux-firmware-{TAG}.tar.gz") && depth == 0 {
                s.tarball = Some(e.path());
            }
        }
    }
    walk(dir, "", 0, &mut s);
    s.rejected.sort();
    s
}

/// A `Write` that reads a tar stream and keeps only the wanted files (so the 1 GB release tarball is never unpacked).
struct TarPick<'a> {
    prefix: String,
    want: &'a dyn Fn(&str) -> bool,
    max: u64,
    head: Vec<u8>,
    // current entry
    left: u64,
    pad: u64,
    kind: u8,
    name: String,
    capture: Option<Vec<u8>>,
    long: Option<String>,
    pax: Option<String>,
    got: BTreeMap<String, Vec<u8>>,
    failed: Option<String>,
}

fn octal(b: &[u8]) -> u64 {
    let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
    u64::from_str_radix(s.trim(), 8).unwrap_or(0)
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned()
}

impl TarPick<'_> {
    fn header(&mut self) {
        let b = std::mem::take(&mut self.head);
        if b.iter().all(|x| *x == 0) {
            return;
        }
        let size = octal(&b[124..136]);
        self.kind = b[156];
        let mut name = cstr(&b[0..100]);
        if &b[257..262] == b"ustar" {
            let p = cstr(&b[345..500]);
            if !p.is_empty() {
                name = format!("{p}/{name}");
            }
        }
        if let Some(n) = self.long.take().or_else(|| self.pax.take()) {
            name = n;
        }
        self.left = size;
        self.pad = size.div_ceil(512) * 512 - size;
        self.name = name;
        self.capture = None;
        match self.kind {
            b'L' | b'x' | b'g' => {
                if size > 1 << 20 {
                    self.failed = Some("an oversized header in the archive".into());
                } else {
                    self.capture = Some(Vec::new());
                }
            }
            b'0' | 0 | b'7' => {
                if let Some(rel) = self.name.strip_prefix(&self.prefix).map(str::to_string) {
                    if (self.want)(&rel) {
                        if size > self.max {
                            self.failed = Some(format!("{rel} is larger than allowed"));
                        } else {
                            self.name = rel;
                            self.capture = Some(Vec::with_capacity(size as usize));
                        }
                    }
                }
            }
            _ => {}
        }
        if self.left == 0 {
            self.finish_entry();
        }
    }

    fn finish_entry(&mut self) {
        if let Some(buf) = self.capture.take() {
            match self.kind {
                b'L' => self.long = Some(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string()),
                b'x' => self.pax = String::from_utf8_lossy(&buf).lines().find_map(|l| l.split_once(' ').and_then(|(_, kv)| kv.strip_prefix("path=")).map(str::to_string)).or(self.pax.take()),
                b'g' => {}
                _ => {
                    self.got.insert(self.name.clone(), buf);
                }
            }
        }
    }
}

impl Write for TarPick<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let total = data.len();
        let mut d = data;
        while !d.is_empty() {
            if let Some(e) = &self.failed {
                return Err(std::io::Error::other(e.clone()));
            }
            if self.left > 0 {
                let n = (self.left as usize).min(d.len());
                if let Some(c) = &mut self.capture {
                    c.extend_from_slice(&d[..n]);
                }
                self.left -= n as u64;
                d = &d[n..];
                if self.left == 0 {
                    self.finish_entry();
                }
            } else if self.pad > 0 {
                let n = (self.pad as usize).min(d.len());
                self.pad -= n as u64;
                d = &d[n..];
            } else {
                let need = 512 - self.head.len();
                let n = need.min(d.len());
                self.head.extend_from_slice(&d[..n]);
                d = &d[n..];
                if self.head.len() == 512 {
                    self.header();
                }
            }
        }
        Ok(total)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Take the wanted files out of the release tarball `tar_gz` while it is decompressed. `want` gets the path inside the release.
pub fn pick_from_tarball(tar_gz: &Path, want: &dyn Fn(&str) -> bool) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut p = TarPick { prefix: format!("linux-firmware-{TAG}/"), want, max: 64 << 20, head: Vec::new(), left: 0, pad: 0, kind: 0, name: String::new(), capture: None, long: None, pax: None, got: BTreeMap::new(), failed: None };
    crate::archive::gunzip_to_writer(tar_gz, &mut p, 8 << 30)?;
    Ok(p.got)
}

// ------------------------------------------------------------------------------------------------ the job

/// Everything the job does to the world.
pub trait Env {
    fn say(&mut self, line: &str);
    fn audit(&mut self, line: &str);
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
    fn data_in_memory(&self) -> bool;
    /// The kernel log text.
    fn kernel_log(&self) -> String;
    fn devs(&self) -> Vec<Dev>;
    /// Fetch `url` into `dest`, resuming; the caller checks the SHA-256.
    fn fetch(&mut self, url: &str, want: u64, dest: &Path) -> Result<(), String>;
    fn install(&mut self, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String>;
    fn module_state(&self, name: &str) -> ModState;
    fn now(&self) -> i64;
}

/// The plan for this computer: the kernel log plus the requests kept from earlier runs, the devices from sysfs.
pub fn plan_now(env: &dyn Env, data: &Path) -> Plan {
    plan_in(manifest(), env, data)
}

pub fn plan_in(m: &Manifest, env: &dyn Env, data: &Path) -> Plan {
    let mut text = env.kernel_log();
    text.push('\n');
    text.push_str(&std::fs::read_to_string(seen_path(data)).unwrap_or_default());
    build_plan(&parse_log(&text, m), &env.devs(), m)
}

/// Keep the names asked for (the kernel log can roll over before the person gets here).
pub fn remember(data: &Path, reqs: &[Request]) {
    let mut have: BTreeSet<String> = std::fs::read_to_string(seen_path(data)).unwrap_or_default().lines().map(str::to_string).collect();
    let before = have.len();
    for r in reqs {
        have.insert(format!("{}: Direct firmware load for {} failed with error -2", if r.driver.is_empty() { "kernel" } else { &r.driver }, r.name));
    }
    if have.len() != before {
        let _ = std::fs::create_dir_all(work_dir(data));
        let _ = std::fs::write(seen_path(data), have.into_iter().collect::<Vec<_>>().join("\n") + "\n");
    }
}

#[derive(Clone, Debug, Default)]
pub struct Opts {
    /// Separate groups the person included (`nvidia`).
    pub with: Vec<String>,
}

fn check_bytes(path: &str, size: u64, sha: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 != size {
        return Err(format!("{path} has {} bytes, not the expected {size}", bytes.len()));
    }
    let got = sha256_hex(bytes);
    if got != sha {
        return Err(format!("{path} has sha256 {got}, not the expected {sha}"));
    }
    Ok(())
}

fn note(env: &mut dyn Env, lines: &mut Vec<String>, l: String) {
    env.say(&l);
    lines.push(l);
}

/// Run the job: detect, check the acceptance, get the bytes from `source`, verify, install, decide about the restart.
pub fn run(env: &mut dyn Env, data: &Path, source: &Source, opts: &Opts) -> Result<Outcome, String> {
    run_with(env, manifest(), data, source, opts)
}

pub fn run_with(env: &mut dyn Env, m: &Manifest, data: &Path, source: &Source, opts: &Opts) -> Result<Outcome, String> {
    let mut lines = Vec::new();
    match run_inner(env, m, data, source, opts, &mut lines) {
        Ok(o) => Ok(o),
        Err(e) => {
            let o = Outcome { at: env.now(), headline: format!("Not done: {e}"), lines, source: source.name().into(), ..Default::default() };
            write_outcome(data, &o);
            env.audit(&format!("firmware.fail {e}"));
            Err(e)
        }
    }
}

fn run_inner(env: &mut dyn Env, m: &Manifest, data: &Path, source: &Source, opts: &Opts, lines: &mut Vec<String>) -> Result<Outcome, String> {
    note(env, lines, format!("[0/6] Hardware firmware, linux-firmware {} (commit {}). {NOTICE}", m.tag, &m.commit[..10]));
    let plan = plan_in(m, env, data);
    for l in plan.lines() {
        note(env, lines, l);
    }
    let sel = plan.select(&opts.with);
    if sel.files.is_empty() {
        return Err("no missing firmware file was found in the kernel log (or only a separate group, which you did not include); nothing to do".into());
    }
    note(env, lines, format!("[1/6] {} file(s) in the release, {} KB with the licence texts", sel.unique().len(), sel.bytes(m) >> 10));
    if sel.bytes(m) > MAX_PLAN_BYTES {
        return Err(format!("the list is {} MB, more than the {} MB this job allows; nothing was fetched", sel.bytes(m) >> 20, MAX_PLAN_BYTES >> 20));
    }
    let Some(acc) = accepted_in(m, data, &sel) else {
        return Err("the licence list was not accepted (or the list changed since you read it). Open the card, read the licences and accept; nothing was downloaded or installed".into());
    };
    note(env, lines, format!("[2/6] licences accepted by {} (list {})", acc.by, &acc.digest[..16]));
    env.audit(&format!("firmware.start source={} files={} list={}", source.name(), sel.unique().len(), &acc.digest[..16]));
    if env.data_in_memory() {
        return Err("the data volume is in memory (a live session): the firmware would be gone after a restart. Start from the installed system".into());
    }
    let dl = download_dir(data);
    std::fs::create_dir_all(&dl).map_err(|e| format!("{}: {e}", dl.display()))?;
    let need = sel.bytes(m).saturating_mul(3) + NEEDED_FREE;
    match env.free_bytes(data) {
        Some(f) if f < need => return Err(format!("the data volume has {} MB free; this needs about {} MB", f >> 20, need >> 20)),
        Some(f) => note(env, lines, format!("free space on the data volume: {} MB", f >> 20)),
        None => note(env, lines, "note: the free space of the data volume could not be read".into()),
    }

    // 3. the bytes, every file checked against the pinned size and sha256
    let uniq = sel.unique();
    let docs = sel.docs(m);
    let mut got: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    match source {
        Source::Download => {
            note(env, lines, format!("[3/6] downloading {} file(s) and {} licence text(s) from the linux-firmware repository (git.kernel.org), commit {}. A stopped download resumes", uniq.len(), docs.len(), &m.commit[..10]));
            let all: Vec<(&String, &(u64, String))> = docs.iter().chain(uniq.iter()).collect();
            for (p, (size, sha)) in all {
                let dest = dl.join(p);
                if let Some(d) = dest.parent() {
                    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                }
                env.fetch(&m.url(p), *size, &dest)?;
                let bytes = std::fs::read(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                if let Err(e) = check_bytes(p, *size, sha, &bytes) {
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!("{e}. The file was deleted; the upstream file may have changed or the download was damaged. Nothing was installed"));
                }
                got.insert(p.clone(), bytes);
            }
            note(env, lines, format!("  all {} files match the pinned checksums", got.len()));
        }
        Source::Folder(dir) => {
            note(env, lines, format!("[3/6] reading the files from {}", dir.display()));
            let scan = scan_folder(dir);
            for (p, why) in scan.rejected.iter().take(10) {
                note(env, lines, format!("  ignored {p}: {why}"));
            }
            let mut missing: Vec<String> = Vec::new();
            for (p, (size, sha)) in docs.iter().chain(uniq.iter()) {
                match find_in_folder(dir, p) {
                    Some(f) => {
                        let bytes = std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                        check_bytes(p, *size, sha, &bytes).map_err(|e| format!("{}: {e}. Nothing was installed (only the exact files of linux-firmware {} are accepted)", f.display(), m.tag))?;
                        got.insert(p.clone(), bytes);
                    }
                    None => missing.push(p.clone()),
                }
            }
            if !missing.is_empty() {
                if let Some(t) = &scan.tarball {
                    note(env, lines, format!("  reading {} (checking the whole file first: this takes a while)", t.display()));
                    let sum = crate::fwpack::file_sha256(t)?;
                    if sum != m.snapshot_sha256 {
                        return Err(format!("{} has sha256 {sum}, not the pinned {}; it is not the release this version expects. Nothing was installed", t.display(), m.snapshot_sha256));
                    }
                    let want: BTreeSet<&String> = missing.iter().collect();
                    let picked = pick_from_tarball(t, &|p| want.contains(&p.to_string()))?;
                    for (p, bytes) in picked {
                        let (size, sha) = docs.get(&p).or_else(|| uniq.get(&p)).cloned().ok_or("an unexpected file in the tarball")?;
                        check_bytes(&p, size, &sha, &bytes)?;
                        got.insert(p, bytes);
                    }
                    missing.retain(|p| !got.contains_key(p));
                }
            }
            if !missing.is_empty() {
                let only_docs = missing.iter().all(|p| docs.contains_key(p));
                if only_docs {
                    note(env, lines, format!("  note: {} licence text(s) are not in the folder ({}); they stay on the vendor's repository", missing.len(), missing.join(", ")));
                } else {
                    return Err(format!("{} file(s) the list needs are not in {} (for example {}); nothing was installed. Make the folder with dist/scripts/firmware-notes/fetch.sh on a computer that has network", missing.len(), dir.display(), missing[0]));
                }
            }
            note(env, lines, format!("  {} files found and checked", got.len()));
        }
    }

    // 4. the pack: the files under the names the drivers ask for (and the release path), licence texts, the WHENCE entries
    note(env, lines, "[4/6] installing as a driver pack on the data volume (linked into /lib/firmware at every start)".into());
    let paths: Vec<String> = sel.files.iter().flat_map(|f| [f.requested.clone(), f.path.clone()]).collect();
    let whence = got.get("WHENCE").map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    let p = build_pack(m, &sel, &got, &whence_excerpt(&whence, &paths), &reload_list(env, &sel));
    let src = match source {
        Source::Download => format!("linux-firmware {} (commit {}, git.kernel.org), downloaded on request", m.tag, m.commit),
        Source::Folder(_) => format!("a folder, files checked against linux-firmware {} (commit {})", m.tag, m.commit),
    };
    let rep = env.install(&format!("linux-firmware {}", m.tag), &src, &p)?;
    for l in &rep.lines {
        note(env, lines, format!("  {l}"));
    }
    let _ = std::fs::remove_dir_all(&dl);

    // 5. restart?
    note(env, lines, "[5/6] checking whether the drivers picked the files up".into());
    let mut why: Vec<String> = Vec::new();
    for f in &sel.files {
        if GPU_DRIVERS.contains(&f.driver.as_str()) && !why.iter().any(|w| w.contains(&f.driver)) {
            why.push(format!("{} may drive the screen, so it is not reloaded while you use it", f.driver));
        } else if f.driver.is_empty() && matches!(family_of(&f.path).0.as_str(), "intel-gpu" | "amd-gpu" | "nvidia") && !why.iter().any(|w| w.contains("graphics")) {
            why.push("graphics firmware is read when the graphics driver starts".into());
        }
    }
    for d in sel.files.iter().map(|f| f.driver.as_str()).filter(|d| !d.is_empty()).collect::<BTreeSet<_>>() {
        if env.module_state(d) == ModState::Builtin {
            why.push(format!("{d} is built into the kernel and reads its firmware only at start"));
        }
    }
    if rep.lines.iter().any(|l| l.contains("did not load")) {
        why.push("a driver did not load again".into());
    }
    let reboot = !why.is_empty();
    let headline = if reboot { "Hardware firmware installed. Restart to finish.".to_string() } else { "Hardware firmware installed and the drivers were loaded again.".to_string() };
    let mut all = lines.clone();
    for w in &why {
        all.push(format!("restart wanted: {w}"));
    }
    let o = Outcome { at: env.now(), installed: true, files: uniq.len(), source: source.name().into(), reboot_needed: reboot, headline, lines: all };
    write_outcome(data, &o);
    env.audit(&format!("firmware.installed files={} restart={}", uniq.len(), reboot));
    env.say(&format!("[6/6] done: {}", o.headline));
    Ok(o)
}

/// The modules that are safe to unload and load again: the drivers that asked, loaded as modules, and not a graphics driver.
fn reload_list(env: &dyn Env, sel: &Selection) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for f in &sel.files {
        let d = f.driver.as_str();
        if !d.is_empty() && !GPU_DRIVERS.contains(&d) && pack::valid_module_name(d) && env.module_state(d) == ModState::Loaded && !v.iter().any(|x| x == d) {
            v.push(d.to_string());
        }
    }
    v
}

/// The pack made of verified files: each file under the name asked for and under its release path, the licence texts under
/// `licences/fw-hardware/`, and the WHENCE entries.
pub fn build_pack(m: &Manifest, sel: &Selection, got: &BTreeMap<String, Vec<u8>>, whence: &str, reload: &[String]) -> pack::Pack {
    let mut p = pack::Pack { v: 1, why: format!("Vendor firmware this computer asked for, installed on the person's request from linux-firmware {} (commit {}). Not part of any IYAKKI image.", m.tag, &m.commit[..10]), reload: reload.to_vec(), ..Default::default() };
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut add = |p: &mut pack::Pack, path: String, bytes: &[u8]| {
        if done.insert(path.clone()) {
            p.files.push(pack::FwFile { path, data: b64::encode(bytes), sha256: sha256_hex(bytes) });
        }
    };
    for f in &sel.files {
        if let Some(b) = got.get(&f.path) {
            add(&mut p, f.path.clone(), b);
            if f.requested != f.path {
                add(&mut p, f.requested.clone(), b);
            }
        }
    }
    for (path, bytes) in got {
        if path.starts_with("LICENSES/") {
            add(&mut p, format!("licences/{PACK_ID}/{}", path.trim_start_matches("LICENSES/")), bytes);
        }
    }
    add(&mut p, format!("licences/{PACK_ID}/WHENCE.excerpt"), whence.as_bytes());
    p
}

// ------------------------------------------------------------------------------------------------ remove

pub fn installed(data: &Path) -> Option<apply::Record> {
    let (fs, _) = apply::Fs::for_data(data);
    apply::read_record(&fs, PACK_ID)
}

/// Take the extra firmware off the machine: the pack, its result and the downloaded pieces. The acceptance stays.
pub fn remove(data: &Path) -> Result<apply::Report, String> {
    let (fs, _) = apply::Fs::for_data(data);
    let r = apply::remove_pack(&fs, PACK_ID)?;
    let _ = std::fs::remove_file(result_path(data));
    let _ = std::fs::remove_dir_all(download_dir(data));
    Ok(r)
}

pub fn leftovers(data: &Path) -> u64 {
    fn walk(d: &Path) -> u64 {
        std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => walk(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        }).sum()
    }
    walk(&download_dir(data))
}

pub fn delete_leftovers(data: &Path) -> u64 {
    let n = leftovers(data);
    let _ = std::fs::remove_dir_all(download_dir(data));
    n
}

// ------------------------------------------------------------------------------------------------ the real machine

pub struct RealEnv {
    pub data: PathBuf,
    pub streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
    pub sys: PathBuf,
}

impl RealEnv {
    pub fn new(data: &Path, streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>) -> RealEnv {
        RealEnv { data: data.to_path_buf(), streamer, sys: "/sys".into() }
    }
}

impl Env for RealEnv {
    fn say(&mut self, line: &str) {
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    fn audit(&mut self, line: &str) {
        // the job runs as a process of its own: the audit line goes to the job log (the tile copies it to the audit log)
        crate::qprintln!("audit: {line}");
    }
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        crate::k3sdev::space(dir).map(|s| s.0)
    }
    fn data_in_memory(&self) -> bool {
        crate::macfw::in_memory(&iyakki_storage::read_mounts().unwrap_or_default(), &self.data)
    }
    fn kernel_log(&self) -> String {
        std::process::Command::new("dmesg").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    }
    fn devs(&self) -> Vec<Dev> {
        read_devs(&self.sys)
    }
    fn fetch(&mut self, url: &str, want: u64, dest: &Path) -> Result<(), String> {
        let st = self.streamer.clone();
        crate::fwpack::fetch_resume(&*st, url, want, dest, 5, std::time::Duration::from_secs(3), &mut |l| self.say(l), &|| false)
    }
    fn install(&mut self, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        let (fs, real) = apply::Fs::for_data(&self.data);
        let run: Box<dyn iyakki_drivers::run::Runner> = if real { Box::new(iyakki_drivers::run::System) } else { Box::new(iyakki_drivers::run::Dry) };
        crate::fwpack::install_checked(&fs, run.as_ref(), PACK_ID, version, "LicenseRef-linux-firmware-various-per-file", source, p)
    }
    fn module_state(&self, name: &str) -> ModState {
        crate::fwpack::module_state_in(&self.sys, name)
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

#[cfg(test)]
#[path = "fwextra_tests.rs"]
mod tests;
