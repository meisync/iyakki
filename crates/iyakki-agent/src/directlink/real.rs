//! The real machine for [`Env`]: `/sys` and `/proc` as plain files, interface addresses through ioctls (`SIOCSIFADDR` and friends,
//! no `ip` program), ARP probes on a raw `AF_PACKET` socket, the beacon on a UDP socket bound to the one interface
//! (`SO_BINDTODEVICE`), and the developer-mode client for hello, pairing and the speed check. Only the system calls listed in
//! `BOUNDARY.md` are used here. None of it runs in a test (the tests use [`super::fake::FakeEnv`]); the parts that need no
//! privileges (sysfs reading) are tested against a copy of `/sys` in a temporary folder.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::beacon::Beacon;
use super::linklocal::Probe;
use iyakki_core_local::linksys::{self, sys};
use super::tb::{self, TbView};
use super::{classify, Config, Env, Hello, NetIf, Peer, Snapshot, SpeedSample, DEV_PORT};
use crate::devmode::client::{self, DevClient, Home};

pub struct RealEnv {
    pub sys: PathBuf,
    pub proc_: PathBuf,
    pub data: PathBuf,
    /// The developer-mode client folder (keys and paired devices) used for pairing and the speed check.
    pub home: PathBuf,
    sockets: Mutex<Vec<(String, std::net::UdpSocket)>>,
    sockets6: Mutex<Vec<(String, std::net::UdpSocket)>>,
}

impl RealEnv {
    pub fn new(data: &Path, home: &Path) -> RealEnv {
        RealEnv { sys: "/sys".into(), proc_: "/proc".into(), data: data.into(), home: home.into(), sockets: Mutex::new(Vec::new()), sockets6: Mutex::new(Vec::new()) }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
    }

    fn state_path(&self) -> PathBuf {
        if self.sys == Path::new("/sys") { PathBuf::from(super::STATE_FILE) } else { self.data.join("directlink-state.json") }
    }

    fn config_path(&self) -> PathBuf {
        self.data.join("directlink.json")
    }

    fn client(&self, ip: Ipv4Addr) -> Result<DevClient, String> {
        let home = Home::at(&self.home);
        let d = home.devices().into_iter().find(|d| d.host == ip.to_string()).ok_or("not paired with that address")?;
        DevClient::new(&home, d)
    }

    fn computer_name(&self) -> String {
        let h = self.me().host;
        if crate::devmode::valid_name(&h) { h } else { "computer".into() }
    }
}

impl Env for RealEnv {
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }

    fn net_ifaces(&self) -> Vec<NetIf> {
        linksys::net_ifaces(&self.sys, &self.proc_, &iyakki_core_local::devctl::ipv4_addresses())
    }

    fn udc_present(&self) -> bool {
        linksys::udc_present(&self.sys)
    }

    fn tb(&self) -> TbView {
        let mut v = tb::read_view(&self.sys);
        v.irqs = linksys::interrupts_text(&self.proc_).and_then(|t| tb::irq_total(&t));
        v
    }

    fn tb_lanes(&self) -> Option<Vec<tb::Lane>> {
        // every read makes the kernel fetch all registers of all ports: at most one read in LANES_EVERY, the rest get the last answer
        static LAST: std::sync::Mutex<Option<(std::time::Instant, Option<Vec<tb::Lane>>)>> = std::sync::Mutex::new(None);
        const LANES_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
        let real = self.sys == Path::new("/sys");
        if real {
            if let Some((at, kept)) = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                if at.elapsed() < LANES_EVERY {
                    return kept.clone();
                }
            }
        }
        let lanes = tb::read_lanes(&self.sys, &tb::read_view(&self.sys));
        if real {
            *LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((std::time::Instant::now(), lanes.clone()));
        }
        lanes
    }

    fn kernel_log(&self, grep: &str) -> Vec<String> {
        if self.sys != Path::new("/sys") {
            return Vec::new();
        }
        crate::devmode::debug::kmsg(&crate::devmode::debug::Paths::real(), 0, grep, 20).into_iter().map(|k| k.msg).collect()
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(5000)));
    }

    fn tb_write_authorized(&self, device: &str, level: u8, key: Option<&str>) -> Result<(), String> {
        linksys::tb_write_authorized(&self.sys, device, level, key)
    }

    fn tb_key(&self, unique_id: &str) -> Option<String> {
        let k = Self::read(&self.data.join("directlink/tb-keys").join(key_file(unique_id)?));
        (k.len() == 64).then_some(k)
    }

    fn tb_store_key(&self, unique_id: &str, key: &str) -> Result<(), String> {
        let f = key_file(unique_id).ok_or("the device has no usable id")?;
        let dir = self.data.join("directlink/tb-keys");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(f);
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true).mode(0o600);
            o.open(&path).and_then(|mut f| f.write_all(key.as_bytes())).map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        std::fs::write(&path, key).map_err(|e| e.to_string())
    }

    fn random_hex32(&self) -> String {
        let mut b = [0u8; 32];
        if iyakki_sec::random_fill(&mut b).is_err() {
            // never a weak key: with no randomness the caller's write is refused by its length check
            return String::new();
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn set_up(&self, name: &str) -> Result<(), String> {
        iyakki_core_local::devctl::set_link(name, true)
    }

    fn add_ipv4(&self, name: &str, label: u8, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        sys::set_addr(&label_name(name, label), ip, prefix)
    }

    fn del_ipv4(&self, name: &str, label: u8) -> Result<(), String> {
        sys::set_addr(&label_name(name, label), Ipv4Addr::UNSPECIFIED, 0)
    }

    fn no_forwarding(&self, name: &str) -> Result<(), String> {
        if !linksys::valid_ifname(name) {
            return Err("not an interface name".into());
        }
        iyakki_core_local::net::forwarding_off(name)
    }

    fn arp_probe(&self, name: &str, mac: [u8; 6], ip: Ipv4Addr) -> Probe {
        sys::arp_probe(name, mac, ip)
    }

    fn arp_announce(&self, name: &str, mac: [u8; 6], ip: Ipv4Addr) {
        sys::arp_announce(name, mac, ip);
    }

    fn beacon_send(&self, name: &str, _local: Ipv4Addr, payload: &[u8]) -> Result<(), String> {
        let mut socks = self.sockets.lock().unwrap_or_else(|e| e.into_inner());
        if !socks.iter().any(|(n, _)| n == name) {
            socks.push((name.to_string(), sys::beacon_socket(name)?));
        }
        let s = &socks.iter().find(|(n, _)| n == name).ok_or("no socket")?.1;
        s.send_to(payload, (Ipv4Addr::BROADCAST, super::BEACON_PORT)).map(|_| ()).map_err(|e| format!("{name}: {e}"))
    }

    fn beacon_recv(&self, name: &str) -> Vec<(Ipv4Addr, Vec<u8>)> {
        let mut socks = self.sockets.lock().unwrap_or_else(|e| e.into_inner());
        if !socks.iter().any(|(n, _)| n == name) {
            match sys::beacon_socket(name) {
                Ok(s) => socks.push((name.to_string(), s)),
                Err(_) => return Vec::new(),
            }
        }
        let mut out = Vec::new();
        if let Some((_, s)) = socks.iter().find(|(n, _)| n == name) {
            let mut buf = [0u8; super::beacon::MAX_LEN + 1];
            while out.len() < 32 {
                match s.recv_from(&mut buf) {
                    Ok((n, std::net::SocketAddr::V4(a))) => out.push((*a.ip(), buf[..n.min(buf.len())].to_vec())),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        out
    }

    fn beacon6_send(&self, name: &str, payload: &[u8]) -> Result<(), String> {
        let mut socks = self.sockets6.lock().unwrap_or_else(|e| e.into_inner());
        if !socks.iter().any(|(n, _)| n == name) {
            socks.push((name.to_string(), sys::beacon_socket6(name)?));
        }
        let idx = sys::ifindex6(name).ok_or_else(|| format!("{name}: no such interface"))?;
        let s = &socks.iter().find(|(n, _)| n == name).ok_or("no socket")?.1;
        s.send_to(payload, std::net::SocketAddrV6::new(super::linklocal::BEACON_GROUP6, super::BEACON_PORT, 0, idx)).map(|_| ()).map_err(|e| format!("{name}: {e}"))
    }

    fn beacon6_recv(&self, name: &str) -> Vec<(std::net::Ipv6Addr, Vec<u8>)> {
        let mut socks = self.sockets6.lock().unwrap_or_else(|e| e.into_inner());
        if !socks.iter().any(|(n, _)| n == name) {
            match sys::beacon_socket6(name) {
                Ok(s) => socks.push((name.to_string(), s)),
                Err(_) => return Vec::new(),
            }
        }
        let mut out = Vec::new();
        if let Some((_, s)) = socks.iter().find(|(n, _)| n == name) {
            let mut buf = [0u8; super::beacon::MAX_LEN + 1];
            while out.len() < 32 {
                match s.recv_from(&mut buf) {
                    Ok((n, std::net::SocketAddr::V6(a))) => out.push((*a.ip(), buf[..n.min(buf.len())].to_vec())),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        out
    }

    fn discover(&self, ms: u64) -> Vec<Peer> {
        let udc = self.udc_present();
        let me = self.me();
        let mut found: Vec<Peer> = Vec::new();
        let mut socks = Vec::new();
        for i in self.net_ifaces().into_iter().filter(|i| i.carrier && classify(i, udc).is_some()) {
            if let Ok(s) = sys::beacon_socket(&i.name) {
                let mut b = me.clone();
                b.mac = i.mac.clone();
                let _ = s.send_to(&b.encode(), (Ipv4Addr::BROADCAST, super::BEACON_PORT));
                socks.push((i, s));
            }
        }
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms.min(5000));
        let mut buf = [0u8; super::beacon::MAX_LEN + 1];
        while std::time::Instant::now() < end && found.len() < 8 {
            for (i, s) in &socks {
                while let Ok((n, std::net::SocketAddr::V4(a))) = s.recv_from(&mut buf) {
                    if let Ok(b) = Beacon::parse(&buf[..n.min(buf.len())]) {
                        if b.id != me.id && b.mac != i.mac && !found.iter().any(|p| p.beacon.id == b.id) {
                            found.push(Peer { iface: i.name.clone(), addr: *a.ip(), addr6: None, beacon: b, last_seen: self.now() });
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        found
    }

    fn snapshot(&self) -> Option<Snapshot> {
        let s: Snapshot = serde_json::from_str(&std::fs::read_to_string(self.state_path()).ok()?).ok()?;
        (self.now() - s.at <= 10).then_some(s)
    }

    fn write_snapshot(&self, s: &Snapshot) {
        let p = self.state_path();
        let tmp = p.with_extension("tmp");
        if let Ok(t) = serde_json::to_string(s) {
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }

    fn config(&self) -> Config {
        std::fs::read_to_string(self.config_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_config(&self, c: &Config) -> Result<(), String> {
        std::fs::create_dir_all(&self.data).map_err(|e| e.to_string())?;
        std::fs::write(self.config_path(), serde_json::to_string_pretty(c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    fn me(&self) -> Beacon {
        static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let id = ID.get_or_init(|| {
            let r = self.random_hex32();
            if r.len() >= 32 { r[..32].to_string() } else { format!("{:032x}", std::process::id()) }
        });
        let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).ok().filter(|h| !h.is_empty() && h.len() <= 63 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))).unwrap_or_else(|| "iyakki".into());
        let dm = crate::devmode::DevMode::open(&self.data);
        let on = dm.is_on();
        Beacon { host, version: env!("CARGO_PKG_VERSION").to_string(), role: if on { "target" } else { "host" }.into(), dev_on: on, pairing_open: on && dm.pairing().is_some(), mac: String::new(), id: id.clone(), port: DEV_PORT, ip6: None }
    }

    fn dev_hello(&self, ip: Ipv4Addr) -> Result<Hello, String> {
        let (fp, v) = client::hello(&ip.to_string(), DEV_PORT)?;
        Ok(Hello { name: v["name"].as_str().unwrap_or("?").to_string(), version: v["version"].as_str().unwrap_or("?").to_string(), fingerprint: fp, pairing_open: v["pairing"].as_bool() == Some(true) })
    }

    fn paired_as(&self, ip: Ipv4Addr) -> Option<String> {
        let devs = Home::at(&self.home).devices();
        if let Some(d) = devs.iter().find(|d| d.host == ip.to_string()) {
            return Some(d.name.clone());
        }
        // the link-local address may have changed since pairing: the certificate fingerprint still names the device
        let (fp, _) = client::hello(&ip.to_string(), DEV_PORT).ok()?;
        devs.into_iter().find(|d| crate::devmode::auth::norm_fp(&d.fingerprint) == crate::devmode::auth::norm_fp(&fp)).map(|d| d.name)
    }

    fn pair(&self, ip: Ipv4Addr, code: &str, expect_fingerprint: &str) -> Result<String, String> {
        let home = Home::at(&self.home);
        let name = self.computer_name();
        let d = client::pair(&home, &ip.to_string(), DEV_PORT, &name, code, Some(expect_fingerprint), None)?;
        Ok(format!("Paired: this computer is \"{name}\" on {ip} and acts as the account {}. Saved as device \"{}\".", d.account, d.name))
    }

    fn tcp_rtt_ms(&self, ip: Ipv4Addr, port: u16) -> Result<f64, String> {
        let mut best: Option<f64> = None;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from((ip, port)), std::time::Duration::from_secs(2)).map_err(|e| e.to_string())?;
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            best = Some(best.map_or(ms, |b| b.min(ms)));
        }
        best.ok_or_else(|| "no measurement".into())
    }

    fn speed(&self, ip: Ipv4Addr) -> Result<SpeedSample, String> {
        let c = self.client(ip)?;
        let body = vec![0x5au8; super::SPEED_CHUNK];
        // one warm-up request so the first TLS handshake is not counted against the link
        c.call("POST", "/v1/dev/speed", &[], &body[..1024])?;
        let t = std::time::Instant::now();
        let mut sent = 0u64;
        for _ in 0..super::SPEED_CHUNKS {
            let v = c.call("POST", "/v1/dev/speed", &[], &body)?;
            sent += v["received"].as_u64().unwrap_or(0);
        }
        Ok(SpeedSample { bytes: sent, secs: t.elapsed().as_secs_f64() })
    }
}

fn label_name(name: &str, label: u8) -> String {
    if label == 0 { name.to_string() } else { format!("{name}:{label}") }
}

/// A file name for a device key: only what the kernel's `unique_id` looks like (hex digits and dashes).
fn key_file(unique_id: &str) -> Option<String> {
    (!unique_id.is_empty() && unique_id.len() <= 64 && unique_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then(|| unique_id.to_string())
}
