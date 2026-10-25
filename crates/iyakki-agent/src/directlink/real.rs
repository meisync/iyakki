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
}

impl RealEnv {
    pub fn new(data: &Path, home: &Path) -> RealEnv {
        RealEnv { sys: "/sys".into(), proc_: "/proc".into(), data: data.into(), home: home.into(), sockets: Mutex::new(Vec::new()) }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
    }

    fn ipv6_ll(&self, name: &str) -> Vec<String> {
        let t = Self::read(&self.proc_.join("net/if_inet6"));
        t.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() >= 6 && f[5] == name && f[0].len() == 32 && f[0].starts_with("fe80") {
                    let g: Vec<String> = (0..8).map(|i| f[0][i * 4..i * 4 + 4].trim_start_matches('0').to_string()).map(|s| if s.is_empty() { "0".into() } else { s }).collect();
                    Some(format!("{}%{name}", g.join(":")))
                } else {
                    None
                }
            })
            .collect()
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

fn valid_ifname(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl Env for RealEnv {
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }

    fn net_ifaces(&self) -> Vec<NetIf> {
        let v4 = iyakki_devctl::ipv4_addresses();
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.sys.join("class/net")) else { return out };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !valid_ifname(&name) || name == "lo" {
                continue;
            }
            let p = e.path();
            let driver = std::fs::read_link(p.join("device/driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
            let flags = u32::from_str_radix(Self::read(&p.join("flags")).trim_start_matches("0x"), 16).unwrap_or(0);
            let speed = Self::read(&p.join("speed")).parse::<i64>().ok().filter(|s| *s > 0).map(|s| s as u32);
            let ipv4 = v4.iter().filter(|(n, _)| n.split(':').next() == Some(name.as_str())).filter_map(|(_, a)| a.parse().ok()).collect();
            out.push(NetIf { driver, mac: Self::read(&p.join("address")), up: flags & 1 != 0, carrier: Self::read(&p.join("carrier")) == "1", speed_mbps: speed, ipv4, ipv6_ll: self.ipv6_ll(&name), name });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn udc_present(&self) -> bool {
        std::fs::read_dir(self.sys.join("class/udc")).is_ok_and(|mut r| r.next().is_some())
    }

    fn tb(&self) -> TbView {
        tb::read_view(&self.sys)
    }

    fn tb_write_authorized(&self, device: &str, level: u8, key: Option<&str>) -> Result<(), String> {
        if !device.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')) || device.is_empty() {
            return Err("not a Thunderbolt device name".into());
        }
        let dir = self.sys.join("bus/thunderbolt/devices").join(device);
        if let Some(k) = key {
            if k.len() != 64 || !k.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("the key must be 64 hex characters".into());
            }
            std::fs::write(dir.join("key"), k).map_err(|e| format!("{device}: writing the key: {e}"))?;
        }
        std::fs::write(dir.join("authorized"), level.to_string()).map_err(|e| format!("{device}: approving: {e}"))
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
        use std::io::Read;
        let mut b = [0u8; 32];
        if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
            // never a weak key: with no randomness the caller's write is refused by its length check
            return String::new();
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn set_up(&self, name: &str) -> Result<(), String> {
        iyakki_devctl::set_link(name, true)
    }

    fn add_ipv4(&self, name: &str, label: u8, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        sys::set_addr(&label_name(name, label), ip, prefix)
    }

    fn del_ipv4(&self, name: &str, label: u8) -> Result<(), String> {
        sys::set_addr(&label_name(name, label), Ipv4Addr::UNSPECIFIED, 0)
    }

    fn no_forwarding(&self, name: &str) -> Result<(), String> {
        if !valid_ifname(name) {
            return Err("not an interface name".into());
        }
        let a = std::fs::write(format!("/proc/sys/net/ipv4/conf/{name}/forwarding"), "0");
        let b = std::fs::write(format!("/proc/sys/net/ipv6/conf/{name}/forwarding"), "0");
        a.and(b).map_err(|e| format!("{name}: {e}"))
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
                            found.push(Peer { iface: i.name.clone(), addr: *a.ip(), beacon: b, last_seen: self.now() });
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
        Beacon { host, version: env!("CARGO_PKG_VERSION").to_string(), role: if on { "target" } else { "host" }.into(), dev_on: on, pairing_open: on && dm.pairing().is_some(), mac: String::new(), id: id.clone(), port: DEV_PORT }
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

#[cfg(target_os = "linux")]
mod sys {
    use super::super::linklocal::{self, Probe};
    use std::net::Ipv4Addr;

    const SIOCSIFADDR: libc::c_ulong = 0x8916;
    const SIOCSIFNETMASK: libc::c_ulong = 0x891c;
    const SIOCSIFBRDADDR: libc::c_ulong = 0x891a;

    #[repr(C)]
    struct IfReqAddr {
        name: [u8; 16],
        family: libc::sa_family_t,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
        pad: [u8; 8],
    }

    fn req(name: &str, ip: Ipv4Addr) -> Result<IfReqAddr, String> {
        if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
            return Err(format!("not an interface name: {name:?}"));
        }
        let mut r = IfReqAddr { name: [0; 16], family: libc::AF_INET as libc::sa_family_t, port: 0, addr: ip.octets(), zero: [0; 8], pad: [0; 8] };
        r.name[..name.len()].copy_from_slice(name.as_bytes());
        Ok(r)
    }

    /// Set (or, with 0.0.0.0, remove) the IPv4 address of `name` (`eth0` or an alias `eth0:1`) with its netmask and broadcast address.
    pub fn set_addr(name: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        if prefix > 32 {
            return Err("bad prefix".into());
        }
        let mask = if prefix == 0 { 0u32 } else { u32::MAX << (32 - u32::from(prefix)) };
        let bcast = u32::from(ip) | !mask;
        // SAFETY: a datagram socket used for three interface requests; every `IfReqAddr` matches the kernel's `struct ifreq`
        // (16-byte name, then a `sockaddr_in` in the union, padded to 40 bytes) and outlives its call.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(format!("socket: {}", std::io::Error::last_os_error()));
            }
            let res = (|| {
                let mut a = req(name, ip)?;
                if libc::ioctl(fd, SIOCSIFADDR as _, &mut a as *mut IfReqAddr) < 0 {
                    return Err(format!("{name}: {}", std::io::Error::last_os_error()));
                }
                if ip.is_unspecified() {
                    return Ok(());
                }
                let mut m = req(name, Ipv4Addr::from(mask))?;
                if libc::ioctl(fd, SIOCSIFNETMASK as _, &mut m as *mut IfReqAddr) < 0 {
                    return Err(format!("{name}: netmask: {}", std::io::Error::last_os_error()));
                }
                let mut b = req(name, Ipv4Addr::from(bcast))?;
                let _ = libc::ioctl(fd, SIOCSIFBRDADDR as _, &mut b as *mut IfReqAddr);
                Ok(())
            })();
            libc::close(fd);
            res
        }
    }

    fn ifindex(name: &str) -> Option<i32> {
        let c = std::ffi::CString::new(name).ok()?;
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
        let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
        (i != 0).then_some(i as i32)
    }

    fn packet_socket(name: &str) -> Result<i32, String> {
        let idx = ifindex(name).ok_or_else(|| format!("{name}: no such interface"))?;
        // SAFETY: a raw packet socket for ARP only (protocol 0x0806 in network order), bound to one interface; `sll` is a fully
        // initialised `sockaddr_ll` and the length passed is its size. The descriptor is closed by the callers.
        unsafe {
            let fd = libc::socket(libc::AF_PACKET, libc::SOCK_RAW | libc::SOCK_CLOEXEC, i32::from((libc::ETH_P_ARP as u16).to_be()));
            if fd < 0 {
                return Err(format!("packet socket: {}", std::io::Error::last_os_error()));
            }
            let mut sll: libc::sockaddr_ll = std::mem::zeroed();
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_protocol = (libc::ETH_P_ARP as u16).to_be();
            sll.sll_ifindex = idx;
            if libc::bind(fd, &sll as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_ll>() as u32) < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(format!("bind: {e}"));
            }
            Ok(fd)
        }
    }

    fn send(fd: i32, name: &str, frame: &[u8]) -> bool {
        let Some(idx) = ifindex(name) else { return false };
        // SAFETY: `frame` is valid for its length; `sll` is a fully initialised `sockaddr_ll` of the right size.
        unsafe {
            let mut sll: libc::sockaddr_ll = std::mem::zeroed();
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_ifindex = idx;
            sll.sll_halen = 6;
            sll.sll_addr[..6].copy_from_slice(&[0xff; 6]);
            libc::sendto(fd, frame.as_ptr() as *const libc::c_void, frame.len(), 0, &sll as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_ll>() as u32) == frame.len() as isize
        }
    }

    /// Wait up to `ms` for a frame; the frame's bytes.
    fn recv(fd: i32, ms: i32, buf: &mut [u8]) -> Option<usize> {
        // SAFETY: `pfd` is one valid pollfd; `buf` is valid for its length.
        unsafe {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if libc::poll(&mut pfd, 1, ms) <= 0 {
                return None;
            }
            let n = libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            (n > 0).then_some(n as usize)
        }
    }

    /// RFC 3927: PROBE_NUM probes about a second apart (a little different per MAC), then ANNOUNCE_WAIT; any packet that shows the
    /// address in use ends it at once.
    pub fn arp_probe(name: &str, mac: [u8; 6], ip: Ipv4Addr) -> Probe {
        let Ok(fd) = packet_socket(name) else { return Probe::Failed };
        let frame = linklocal::probe_frame(&mac, ip);
        let jitter = 200 + i32::from(mac[5]) * 2; // 200..710 ms of spread, so two computers do not probe in step
        let mut buf = [0u8; 128];
        let mut result = Probe::Free;
        'outer: for round in 0..=linklocal::PROBE_NUM {
            if round < linklocal::PROBE_NUM && !send(fd, name, &frame) {
                result = Probe::Failed;
                break;
            }
            let wait_ms = if round < linklocal::PROBE_NUM { 600 + jitter } else { 1500 };
            let end = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms as u64);
            while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
                if let Some(n) = recv(fd, left.as_millis().min(i32::MAX as u128) as i32, &mut buf) {
                    if let Some(p) = linklocal::parse_arp(&buf[..n]) {
                        if let Some(other) = linklocal::conflicts(&p, &mac, ip) {
                            result = Probe::InUse(other);
                            break 'outer;
                        }
                    }
                }
            }
        }
        // SAFETY: closing the descriptor this function opened.
        unsafe { libc::close(fd) };
        result
    }

    pub fn arp_announce(name: &str, mac: [u8; 6], ip: Ipv4Addr) {
        if let Ok(fd) = packet_socket(name) {
            let f = linklocal::arp_frame(&mac, ip, ip);
            send(fd, name, &f);
            // SAFETY: closing the descriptor this function opened.
            unsafe { libc::close(fd) };
        }
    }

    pub fn beacon_socket(name: &str) -> Result<std::net::UdpSocket, String> {
        use std::os::fd::AsRawFd;
        if name.is_empty() || name.len() > 15 || name.contains('/') || name.bytes().any(|b| b == 0) {
            return Err("not an interface name".into());
        }
        // every socket of ours shares the port (SO_REUSEADDR), so the manager and a one-off scan can both listen
        let s = reuse_bind().map_err(|e| format!("beacon port {}: {e}", super::super::BEACON_PORT))?;
        // SAFETY: setsockopt on a valid descriptor with a pointer and length of a byte string that lives through the call.
        let r = unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, name.as_ptr() as *const libc::c_void, name.len() as u32) };
        if r < 0 {
            return Err(format!("{name}: bind to the interface: {}", std::io::Error::last_os_error()));
        }
        s.set_broadcast(true).map_err(|e| e.to_string())?;
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(s)
    }

    fn reuse_bind() -> Result<std::net::UdpSocket, std::io::Error> {
        use std::os::fd::FromRawFd;
        // SAFETY: a new UDP socket with SO_REUSEADDR and SO_REUSEPORT set, bound to 0.0.0.0:BEACON_PORT with a fully initialised
        // `sockaddr_in`; on failure the descriptor is closed, on success ownership passes to the returned `UdpSocket`.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let one: libc::c_int = 1;
            for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
                libc::setsockopt(fd, libc::SOL_SOCKET, opt, &one as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32);
            }
            let mut sin: libc::sockaddr_in = std::mem::zeroed();
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = super::super::BEACON_PORT.to_be();
            if libc::bind(fd, &sin as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            Ok(std::net::UdpSocket::from_raw_fd(fd))
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod sys {
    use super::super::linklocal::Probe;
    use std::net::Ipv4Addr;
    pub fn set_addr(_: &str, _: Ipv4Addr, _: u8) -> Result<(), String> {
        Err("not available on this OS".into())
    }
    pub fn arp_probe(_: &str, _: [u8; 6], _: Ipv4Addr) -> Probe {
        Probe::Failed
    }
    pub fn arp_announce(_: &str, _: [u8; 6], _: Ipv4Addr) {}
    pub fn beacon_socket(_: &str) -> Result<std::net::UdpSocket, String> {
        Err("not available on this OS".into())
    }
}
