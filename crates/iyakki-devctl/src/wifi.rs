//! WiFi: finding networks (`iw` scan), remembering the ones a person joined, and writing the supplicant's configuration for
//! all of them so the machine joins whichever is in range. Parsing and writing are plain functions (tested with real `iw`
//! output); only [`Wifi::scan`] and [`Wifi::apply`] run programs, and they look in a folder the tests can put stand-ins in.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Security {
    Open,
    /// WEP (obsolete and broken: shown, never joined from here).
    Wep,
    /// The first WPA (TKIP).
    Wpa,
    Wpa2,
    Wpa2Wpa3,
    Wpa3,
    /// A company or school network that needs a user name and certificate.
    Enterprise,
}

impl Security {
    pub fn label(self) -> &'static str {
        match self {
            Security::Open => "open",
            Security::Wep => "WEP",
            Security::Wpa => "WPA",
            Security::Wpa2 => "WPA2",
            Security::Wpa2Wpa3 => "WPA2/WPA3",
            Security::Wpa3 => "WPA3",
            Security::Enterprise => "enterprise",
        }
    }

    /// Can be joined with just a passphrase (or nothing).
    pub fn joinable(self) -> bool {
        !matches!(self, Security::Wep | Security::Enterprise)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ap {
    pub ssid: String,
    pub bssid: String,
    pub freq_mhz: u32,
    pub signal_dbm: f32,
    pub security: Security,
    /// This is the network the machine is joined to now.
    pub connected: bool,
}

impl Ap {
    /// 0 to 4 bars from the signal strength.
    pub fn bars(&self) -> usize {
        match self.signal_dbm {
            s if s >= -55.0 => 4,
            s if s >= -67.0 => 3,
            s if s >= -75.0 => 2,
            s if s >= -85.0 => 1,
            _ => 0,
        }
    }

    pub fn band(&self) -> &'static str {
        if self.freq_mhz >= 5925 { "6 GHz" } else if self.freq_mhz >= 4900 { "5 GHz" } else { "2.4 GHz" }
    }
}

/// `iw` writes names it cannot show as `\xNN`; turn them back into characters.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() + 0 && b[i + 1] == b'x' && b[i + 2].is_ascii_hexdigit() && b[i + 3].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The networks in the output of `iw dev <interface> scan`: one entry per network name (the strongest access point of it),
/// the best signal first. Hidden networks (no name) are left out.
pub fn parse_scan(text: &str) -> Vec<Ap> {
    struct Cur {
        bssid: String,
        connected: bool,
        freq: u32,
        signal: f32,
        ssid: String,
        privacy: bool,
        in_rsn: bool,
        rsn: bool,
        wpa: bool,
        psk: bool,
        sae: bool,
        eap: bool,
    }
    fn finish(c: Cur, out: &mut Vec<Ap>) {
        let ssid = c.ssid.trim_matches('\0').to_string();
        if ssid.is_empty() || ssid.chars().all(|ch| ch == '\0') {
            return;
        }
        let security = if c.eap {
            Security::Enterprise
        } else if c.rsn && c.sae && c.psk {
            Security::Wpa2Wpa3
        } else if c.rsn && c.sae {
            Security::Wpa3
        } else if c.rsn {
            Security::Wpa2
        } else if c.wpa {
            Security::Wpa
        } else if c.privacy {
            Security::Wep
        } else {
            Security::Open
        };
        out.push(Ap { ssid, bssid: c.bssid, freq_mhz: c.freq, signal_dbm: c.signal, security, connected: c.connected });
    }
    let mut all: Vec<Ap> = Vec::new();
    let mut cur: Option<Cur> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("BSS ") {
            if let Some(c) = cur.take() {
                finish(c, &mut all);
            }
            let bssid: String = rest.chars().take_while(|c| c.is_ascii_hexdigit() || *c == ':').collect();
            cur = Some(Cur { bssid, connected: rest.contains("-- associated"), freq: 0, signal: -100.0, ssid: String::new(), privacy: false, in_rsn: false, rsn: false, wpa: false, psk: false, sae: false, eap: false });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        let t = line.trim();
        if let Some(v) = t.strip_prefix("freq:") {
            c.freq = v.trim().parse::<f32>().unwrap_or(0.0) as u32;
        } else if let Some(v) = t.strip_prefix("signal:") {
            c.signal = v.trim().split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(-100.0);
        } else if let Some(v) = t.strip_prefix("SSID:") {
            c.ssid = unescape(v.trim_start_matches(' '));
        } else if t.starts_with("capability:") {
            c.privacy = t.contains("Privacy");
        } else if t.starts_with("RSN:") {
            c.rsn = true;
            c.in_rsn = true;
        } else if t.starts_with("WPA:") {
            c.wpa = true;
            c.in_rsn = true;
        } else if t.contains("Authentication suites:") {
            if c.in_rsn {
                c.psk |= t.contains("PSK");
                c.sae |= t.contains("SAE");
                c.eap |= t.contains("IEEE 802.1X") || t.contains("EAP");
            }
        } else if !line.starts_with('\t') || (t.ends_with(':') && !t.starts_with('*')) {
            // another information element starts: leave the security block
            if !(t.starts_with("RSN:") || t.starts_with("WPA:")) && !t.starts_with('*') && !t.is_empty() && line.starts_with('\t') && !line.starts_with("\t\t") {
                c.in_rsn = false;
            }
        }
    }
    if let Some(c) = cur.take() {
        finish(c, &mut all);
    }
    // one entry per name: the strongest
    let mut best: Vec<Ap> = Vec::new();
    for ap in all {
        match best.iter_mut().find(|b| b.ssid == ap.ssid) {
            Some(b) => {
                let connected = b.connected || ap.connected;
                if ap.signal_dbm > b.signal_dbm {
                    *b = ap;
                }
                b.connected = connected;
            }
            None => best.push(ap),
        }
    }
    best.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal_dbm.partial_cmp(&a.signal_dbm).unwrap_or(std::cmp::Ordering::Equal)));
    best
}

/// A name the supplicant's file can hold: 1 to 32 bytes of printable text with no quotes, backslashes or line breaks.
pub fn valid_ssid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && !s.chars().any(|c| c.is_control() || c == '"' || c == '\\')
}

pub fn valid_passphrase(s: &str) -> bool {
    (8..=63).contains(&s.chars().count()) && s.is_ascii() && !s.chars().any(|c| c.is_control() || c == '"' || c == '\\')
}

/// A network the machine knows how to join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub ssid: String,
    /// The key as 64 hexadecimal characters (made from the passphrase, which is not kept) for WPA and WPA2 networks.
    #[serde(default)]
    pub psk: Option<String>,
    /// The passphrase itself, kept only for WPA3 networks (they need it as it is).
    #[serde(default)]
    pub passphrase: Option<String>,
    /// The network does not announce its name (the supplicant has to ask for it).
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub added: i64,
}

impl Known {
    /// Build the entry for joining `ssid` with `passphrase` (empty for an open network) given what the network offers.
    pub fn new(ssid: &str, passphrase: &str, security: Security, hidden: bool, now: i64) -> Result<Known, String> {
        if !valid_ssid(ssid) {
            return Err("the network name must be 1 to 32 characters without quotes or backslashes".into());
        }
        if !security.joinable() {
            return Err(format!("{} networks cannot be joined from here", security.label()));
        }
        let (psk, pass) = match security {
            Security::Open => (None, None),
            Security::Wpa3 => {
                if !valid_passphrase(passphrase) {
                    return Err("the passphrase must be 8 to 63 characters (letters, digits and symbols, no quotes or backslashes)".into());
                }
                (None, Some(passphrase.to_string()))
            }
            _ => {
                if !valid_passphrase(passphrase) {
                    return Err("the passphrase must be 8 to 63 characters (letters, digits and symbols, no quotes or backslashes)".into());
                }
                (Some(iyakki_sec::wpa::wpa_psk_hex(passphrase, ssid)), None)
            }
        };
        Ok(Known { ssid: ssid.to_string(), psk, passphrase: pass, hidden, added: now })
    }
}

/// The networks a person has joined, kept in one file (owner-only, in the data folder).
pub struct KnownStore {
    path: PathBuf,
}

impl KnownStore {
    pub fn new(path: &Path) -> KnownStore {
        KnownStore { path: path.into() }
    }

    pub fn list(&self) -> Vec<Known> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, v: &[Known]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("tmp");
        let json = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(json.as_bytes())).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Remember a network (replacing an older entry of the same name); the newest is tried first.
    pub fn add(&self, k: Known) -> Result<(), String> {
        let mut v = self.list();
        v.retain(|o| o.ssid != k.ssid);
        v.insert(0, k);
        v.truncate(50);
        self.save(&v)
    }

    pub fn forget(&self, ssid: &str) -> Result<bool, String> {
        let mut v = self.list();
        let n = v.len();
        v.retain(|o| o.ssid != ssid);
        self.save(&v)?;
        Ok(v.len() != n)
    }
}

/// The supplicant's configuration for all the known networks, the newest with the highest priority.
pub fn wpa_conf_for(known: &[Known]) -> String {
    let mut s = String::from("ctrl_interface=/run/wpa_supplicant\nupdate_config=0\ncountry=00\n");
    for (i, k) in known.iter().enumerate() {
        if !valid_ssid(&k.ssid) {
            continue;
        }
        s.push_str(&format!("network={{\n    ssid=\"{}\"\n    priority={}\n", k.ssid, known.len() - i));
        if k.hidden {
            s.push_str("    scan_ssid=1\n");
        }
        match (&k.psk, &k.passphrase) {
            (Some(p), _) if p.len() == 64 && p.chars().all(|c| c.is_ascii_hexdigit()) => s.push_str(&format!("    psk={p}\n    key_mgmt=WPA-PSK\n")),
            (_, Some(p)) if valid_passphrase(p) => s.push_str(&format!("    sae_password=\"{p}\"\n    psk=\"{p}\"\n    key_mgmt=SAE WPA-PSK\n    ieee80211w=2\n")),
            _ => s.push_str("    key_mgmt=NONE\n"),
        }
        s.push_str("}\n");
    }
    s
}

/// Runs the programs that scan and join. `path` is a folder to look in before the usual ones (tests put stand-ins there).
pub struct Wifi {
    pub run_dir: PathBuf,
    pub path: Option<PathBuf>,
}

fn run(w: &Wifi, program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null());
    let base = std::env::var("PATH").unwrap_or_else(|_| "/usr/sbin:/usr/bin:/sbin:/bin".into());
    cmd.env("PATH", match &w.path {
        Some(p) => format!("{}:{base}", p.display()),
        None => base,
    });
    let o = cmd.output().map_err(|e| format!("{program}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("{program}: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("it failed").trim()))
    }
}

impl Wifi {
    pub fn real() -> Wifi {
        Wifi { run_dir: "/run".into(), path: None }
    }

    /// Look for networks with this interface (the radio is switched on first by the caller).
    pub fn scan(&self, iface: &str) -> Result<Vec<Ap>, String> {
        if iface.is_empty() || !iface.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return Err("not an interface name".into());
        }
        // the interface has to be up to scan
        let _ = run(self, "ip", &["link", "set", iface, "up"]);
        run(self, "iw", &["dev", iface, "scan"]).map(|t| parse_scan(&t)).map_err(|e| format!("could not scan: {e}"))
    }

    /// Write the configuration for every known network and make the supplicant use it (starting it, and the DHCP client, if
    /// they are not running).
    pub fn apply(&self, iface: &str, known: &[Known]) -> Result<(), String> {
        if iface.is_empty() || !iface.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return Err("not an interface name".into());
        }
        let conf = self.run_dir.join(format!("wpa_supplicant-{iface}.conf"));
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::create_dir_all(&self.run_dir).map_err(|e| e.to_string())?;
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&conf).and_then(|mut f| f.write_all(wpa_conf_for(known).as_bytes())).map_err(|e| format!("{}: {e}", conf.display()))?;
        }
        let _ = run(self, "ip", &["link", "set", iface, "up"]);
        if run(self, "wpa_cli", &["-i", iface, "status"]).is_ok() {
            run(self, "wpa_cli", &["-i", iface, "reconfigure"]).map_err(|e| format!("could not make the supplicant read the new networks: {e}"))?;
        } else {
            run(self, "wpa_supplicant", &["-B", "-i", iface, "-c", &conf.display().to_string()]).map_err(|e| format!("could not start the WiFi supplicant: {e}"))?;
        }
        // an address for the link (the client keeps trying until the network is joined)
        let pid = self.run_dir.join(format!("udhcpc-{iface}.pid"));
        let running = std::fs::read_to_string(&pid).ok().and_then(|p| p.trim().parse::<u32>().ok()).is_some_and(|p| Path::new(&format!("/proc/{p}")).exists());
        if !running {
            run(self, "udhcpc", &["-b", "-R", "-i", iface, "-p", &pid.display().to_string()]).map_err(|e| format!("could not ask for an address: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `iw dev wlan0 scan` prints (shortened but in its real shape).
    const SCAN: &str = "\
BSS aa:bb:cc:00:00:01(on wlan0) -- associated
\tlast seen: 4143.352s [boottime]
\tfreq: 5180.0
\tbeacon interval: 100 TUs
\tcapability: ESS Privacy SpectrumMgmt RadioMeasure (0x1111)
\tsignal: -48.00 dBm
\tSSID: HomeNet
\tSupported rates: 6.0* 9.0 12.0* 18.0
\tRSN:\t * Version: 1
\t\t * Group cipher: CCMP
\t\t * Pairwise ciphers: CCMP
\t\t * Authentication suites: PSK SAE
\t\t * Capabilities: 1-PTKSA-RC 1-GTKSA-RC MFP-capable (0x008c)
\tHT capabilities:
\t\tCapabilities: 0x19ef
BSS aa:bb:cc:00:00:02(on wlan0)
\tfreq: 2437.0
\tcapability: ESS Privacy ShortSlotTime (0x0411)
\tsignal: -62.00 dBm
\tSSID: HomeNet
\tRSN:\t * Version: 1
\t\t * Authentication suites: PSK
BSS aa:bb:cc:00:00:03(on wlan0)
\tfreq: 2412.0
\tcapability: ESS (0x0401)
\tsignal: -80.00 dBm
\tSSID: Cafe Guest
BSS aa:bb:cc:00:00:04(on wlan0)
\tfreq: 2462.0
\tcapability: ESS Privacy (0x0411)
\tsignal: -70.50 dBm
\tSSID: OldRouter
BSS aa:bb:cc:00:00:05(on wlan0)
\tfreq: 2447.0
\tcapability: ESS Privacy (0x0411)
\tsignal: -71.00 dBm
\tSSID: Office
\tRSN:\t * Version: 1
\t\t * Authentication suites: IEEE 802.1X
BSS aa:bb:cc:00:00:06(on wlan0)
\tfreq: 2452.0
\tcapability: ESS Privacy (0x0411)
\tsignal: -55.00 dBm
\tSSID: Modern3
\tRSN:\t * Version: 1
\t\t * Authentication suites: SAE
BSS aa:bb:cc:00:00:07(on wlan0)
\tfreq: 2417.0
\tcapability: ESS Privacy (0x0411)
\tsignal: -60.00 dBm
\tSSID: \\x00\\x00\\x00\\x00
BSS aa:bb:cc:00:00:08(on wlan0)
\tfreq: 2422.0
\tcapability: ESS Privacy (0x0411)
\tsignal: -66.00 dBm
\tSSID: Caf\\xc3\\xa9 Wi-Fi
\tWPA:\t * Version: 1
\t\t * Authentication suites: PSK
";

    fn by<'a>(v: &'a [Ap], ssid: &str) -> &'a Ap {
        v.iter().find(|a| a.ssid == ssid).unwrap_or_else(|| panic!("no {ssid} in {:?}", v.iter().map(|a| &a.ssid).collect::<Vec<_>>()))
    }

    #[test]
    fn a_real_looking_scan_is_read_with_security_signal_and_the_connected_network_first() {
        let v = parse_scan(SCAN);
        assert_eq!(v.iter().map(|a| a.ssid.as_str()).collect::<Vec<_>>(), vec!["HomeNet", "Modern3", "Café Wi-Fi", "OldRouter", "Office", "Cafe Guest"], "connected first, then the strongest; a hidden name is left out");
        let home = by(&v, "HomeNet");
        assert_eq!((home.security, home.signal_dbm, home.freq_mhz, home.connected, home.bssid.as_str()), (Security::Wpa2Wpa3, -48.0, 5180, true, "aa:bb:cc:00:00:01"), "two access points of one name: the stronger one");
        assert_eq!((home.bars(), home.band()), (4, "5 GHz"));
        assert_eq!(by(&v, "Modern3").security, Security::Wpa3);
        assert_eq!(by(&v, "Cafe Guest").security, Security::Open);
        assert_eq!(by(&v, "OldRouter").security, Security::Wep);
        assert_eq!(by(&v, "Office").security, Security::Enterprise);
        assert_eq!(by(&v, "Café Wi-Fi").security, Security::Wpa, "escaped bytes become the characters they spell");
        assert!(!by(&v, "OldRouter").security.joinable() && !by(&v, "Office").security.joinable() && by(&v, "Cafe Guest").security.joinable());
        assert_eq!((by(&v, "Cafe Guest").bars(), by(&v, "Cafe Guest").band()), (1, "2.4 GHz"));
        assert!(parse_scan("").is_empty() && parse_scan("garbage\nmore").is_empty());
    }

    #[test]
    fn what_a_person_types_is_checked_and_turned_into_the_entry_to_keep() {
        let k = Known::new("HomeNet", "password", Security::Wpa2, false, 5).unwrap();
        assert_eq!(k.psk.as_deref().map(str::len), Some(64));
        assert!(k.passphrase.is_none(), "for WPA2 the passphrase itself is not kept");
        assert_eq!(Known::new("IEEE", "password", Security::Wpa2, false, 0).unwrap().psk.as_deref(), Some("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"));
        let k3 = Known::new("Modern3", "a long passphrase", Security::Wpa3, false, 5).unwrap();
        assert_eq!((k3.psk, k3.passphrase.as_deref()), (None, Some("a long passphrase")), "WPA3 needs the passphrase as it is");
        assert_eq!(Known::new("Cafe", "", Security::Open, false, 5).unwrap().psk, None);
        assert!(Known::new("HomeNet", "short", Security::Wpa2, false, 0).is_err());
        assert!(Known::new("Home\"Net", "password", Security::Wpa2, false, 0).is_err());
        assert!(Known::new(&"x".repeat(33), "password", Security::Wpa2, false, 0).is_err());
        assert!(Known::new("Old", "password", Security::Wep, false, 0).unwrap_err().contains("WEP"));
        assert!(Known::new("Office", "password", Security::Enterprise, false, 0).is_err());
        assert!(Known::new("Home", "pass\\\"word", Security::Wpa2, false, 0).is_err(), "no quotes or backslashes that would break the file");
        assert!(Known::new("Home", "pässwörd12", Security::Wpa2, false, 0).is_err(), "ASCII only");
    }

    #[test]
    fn networks_are_remembered_newest_first_forgotten_and_written_for_the_supplicant() {
        let d = std::env::temp_dir().join(format!("iyakki-wifi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = KnownStore::new(&d.join("wifi.json"));
        assert!(store.list().is_empty());
        store.add(Known::new("HomeNet", "password", Security::Wpa2, false, 1).unwrap()).unwrap();
        store.add(Known::new("Cafe", "", Security::Open, false, 2).unwrap()).unwrap();
        store.add(Known::new("Modern3", "a long passphrase", Security::Wpa3, true, 3).unwrap()).unwrap();
        store.add(Known::new("Cafe", "", Security::Open, false, 4).unwrap()).unwrap(); // joined again: moves to the front
        assert_eq!(store.list().iter().map(|k| k.ssid.as_str()).collect::<Vec<_>>(), vec!["Cafe", "Modern3", "HomeNet"]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("wifi.json")).unwrap().permissions().mode() & 0o777, 0o600);
        let conf = wpa_conf_for(&store.list());
        assert!(conf.starts_with("ctrl_interface=/run/wpa_supplicant\n"));
        assert!(conf.contains("ssid=\"Cafe\"\n    priority=3\n    key_mgmt=NONE"), "{conf}");
        assert!(conf.contains("ssid=\"Modern3\"\n    priority=2\n    scan_ssid=1\n    sae_password=\"a long passphrase\""), "{conf}");
        assert!(conf.contains("ssid=\"HomeNet\"\n    priority=1\n    psk=") && conf.contains("key_mgmt=WPA-PSK\n"), "{conf}");
        assert!(!conf.contains("=\"password\""), "the WPA2 passphrase is not in the file: {conf}");
        assert!(store.forget("Modern3").unwrap() && !store.forget("Modern3").unwrap());
        assert_eq!(store.list().len(), 2);
        // a damaged entry never breaks the file
        assert!(!wpa_conf_for(&[Known { ssid: "x\"y".into(), psk: None, passphrase: None, hidden: false, added: 0 }]).contains("network"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fake_tools(d: &Path, rec: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = |name: &str, body: &str| {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", rec.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script("ip", "exit 0");
        script("iw", "cat <<'EOT'\nBSS aa:bb:cc:00:00:01(on wlan0)\n\tfreq: 2437.0\n\tcapability: ESS Privacy (0x0411)\n\tsignal: -50.00 dBm\n\tSSID: Seen\n\tRSN:\t * Version: 1\n\t\t * Authentication suites: PSK\nEOT");
        script("wpa_cli", "exit 1"); // the supplicant is not running yet
        script("wpa_supplicant", "exit 0");
        script("udhcpc", "exit 0");
        bin
    }

    #[test]
    fn scanning_and_joining_run_the_right_programs_with_the_conf_in_place_and_refuse_odd_interface_names() {
        let d = std::env::temp_dir().join(format!("iyakki-wifi-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let rec = d.join("rec.log");
        let w = Wifi { run_dir: d.join("run"), path: Some(fake_tools(&d, &rec)) };
        let aps = w.scan("wlan0").unwrap();
        assert_eq!(aps.len(), 1);
        assert_eq!((aps[0].ssid.as_str(), aps[0].security), ("Seen", Security::Wpa2));
        let known = vec![Known::new("Seen", "password", Security::Wpa2, false, 1).unwrap()];
        w.apply("wlan0", &known).unwrap();
        let log = std::fs::read_to_string(&rec).unwrap();
        assert!(log.contains("iw dev wlan0 scan") && log.contains("ip link set wlan0 up"), "{log}");
        assert!(log.contains("wpa_cli -i wlan0 status") && log.contains(&format!("wpa_supplicant -B -i wlan0 -c {}", d.join("run/wpa_supplicant-wlan0.conf").display())), "a supplicant is started when none runs: {log}");
        assert!(log.contains("udhcpc -b -R -i wlan0 -p"), "{log}");
        assert!(std::fs::read_to_string(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().contains("ssid=\"Seen\""));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("run/wpa_supplicant-wlan0.conf")).unwrap().permissions().mode() & 0o777, 0o600);
        // with a supplicant running it is only told to read the file again
        std::fs::write(d.join("bin/wpa_cli"), format!("#!/bin/sh\necho \"wpa_cli $*\" >> {}\nexit 0\n", rec.display())).unwrap();
        std::fs::write(&rec, "").unwrap();
        w.apply("wlan0", &known).unwrap();
        let log = std::fs::read_to_string(&rec).unwrap();
        assert!(log.contains("wpa_cli -i wlan0 reconfigure") && !log.contains("wpa_supplicant -B"), "{log}");
        assert!(w.scan("wlan0; reboot").is_err() && w.apply("../x", &known).is_err() && w.scan("").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
