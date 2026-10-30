//! The system layer of the VPN manager ([`crate::vpn_mgr`]): the only place that touches the machine. Everything the manager
//! needs from the operating system is the trait [`Sys`]; [`OsSys`] is the real one (the image's `ip`, a generic-netlink socket
//! for WireGuard, `/etc/resolv.conf`, and the Firewall's own nf_tables netlink writer for the guard and DNS tables; no `nft`) and [`FakeSys`] keeps the same state in memory so the manager's
//! tests can assert that a stop leaves nothing behind.
//!
//! Nothing here has run on a device (see `docs/development/VPN-CONNECTOR-PLAN.md`, Status).

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;

use iyakki_vpn::net::Cidr;
use iyakki_vpn::plan::{DnsCaptureSpec, KillSwitchSpec, DNS_TABLE, KS_TABLE};
use iyakki_vpn::wgnl::{DeviceSet, DeviceStatus, GenlTransport, WgClient};
use iyakki_vpn::VpnError;

/// What the manager needs to know about the connected network.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetInfo {
    /// The interface of the default route (the one the VPN server is reached through).
    pub physical_if: String,
    /// Every interface that carries real traffic (the default one first); the kill switch lets the endpoint through on all of them.
    pub physical_ifs: Vec<String>,
    pub gateway4: Option<Ipv4Addr>,
    pub gateway6: Option<Ipv6Addr>,
    /// Connected subnets of the physical interfaces.
    pub lan: Vec<Cidr>,
}

pub trait Sys: Send + Sync {
    fn net_info(&self) -> Result<NetInfo, String>;
    /// Resolve a server name once (the manager checks and pins the answers).
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String>;
    /// `ip` with a fixed argument list (never a shell).
    fn ip(&self, args: &[String]) -> Result<(), String>;
    fn link_exists(&self, ifname: &str) -> bool;
    fn wg_configure(&self, d: &DeviceSet) -> Result<(), String>;
    fn wg_status(&self, ifname: &str) -> Result<DeviceStatus, String>;
    fn read_resolv(&self) -> Option<String>;
    fn write_resolv(&self, text: &str) -> Result<(), String>;
    /// The saved original of `/etc/resolv.conf` (kept on tmpfs while the VPN's DNS is in place).
    fn read_backup(&self) -> Option<String>;
    fn write_backup(&self, text: &str) -> Result<(), String>;
    fn drop_backup(&self);
    /// Install (or replace) the guard table. Fails when the system cannot: the manager then refuses to connect.
    fn install_guard(&self, spec: &KillSwitchSpec) -> Result<(), String>;
    fn remove_guard(&self);
    fn guard_present(&self) -> bool;
    fn install_dns_capture(&self, spec: &DnsCaptureSpec) -> Result<(), String>;
    fn remove_dns_capture(&self);
    /// Add or remove an address in the guard's learned-destination sets.
    fn guard_set(&self, ip: IpAddr, expires_in: Option<u64>, add: bool) -> Result<(), String>;
}

// ------------------------------------------------------------------------------------------------ parsing `ip` output

/// `default via 192.168.1.1 dev eth0 proto dhcp` into (device, gateway text).
pub fn parse_default_route(text: &str) -> Option<(String, Option<String>)> {
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.first() != Some(&"default") {
            continue;
        }
        let at = |k: &str| t.iter().position(|w| *w == k).and_then(|i| t.get(i + 1)).map(|s| s.to_string());
        if let Some(dev) = at("dev") {
            return Some((dev, at("via")));
        }
    }
    None
}

/// `ip -o addr show scope global` into (interface, network). The tunnel and the cluster's own devices are skipped.
pub fn parse_addrs(text: &str, skip_prefixes: &[&str]) -> Vec<(String, Cidr)> {
    let mut out = Vec::new();
    for l in text.lines() {
        let t: Vec<&str> = l.split_whitespace().collect();
        let (Some(dev), Some(fam)) = (t.get(1), t.iter().position(|w| *w == "inet" || *w == "inet6")) else { continue };
        let dev = dev.trim_end_matches(':');
        if dev == "lo" || skip_prefixes.iter().any(|p| dev.starts_with(p)) {
            continue;
        }
        if let Some(c) = t.get(fam + 1).and_then(|c| Cidr::parse(c)) {
            out.push((dev.to_string(), c.network()));
        }
    }
    out
}

/// Interface name prefixes that are not "the local network": the cluster's devices, containers, tunnels.
pub const NOT_LAN_PREFIXES: [&str; 8] = ["cni", "flannel", "veth", "docker", "br-", "wg", "iyvpn", "tun"];

// ------------------------------------------------------------------------------------------------ the real one

/// The real system. Needs root (or `CAP_NET_ADMIN`); without it every call returns the kernel's reason in words.
pub struct OsSys {
    pub resolv_path: String,
    pub backup_path: String,
}

impl Default for OsSys {
    fn default() -> OsSys {
        OsSys { resolv_path: iyakki_vpn::plan::RESOLV_CONF.into(), backup_path: iyakki_vpn::plan::RESOLV_BACKUP.into() }
    }
}

fn find_program(name: &str) -> Option<std::path::PathBuf> {
    ["/usr/sbin", "/sbin", "/usr/bin", "/bin"].iter().map(|d| std::path::Path::new(d).join(name)).find(|p| p.is_file())
}

fn run(prog: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let p = find_program(prog).ok_or_else(|| format!("the program `{prog}` is not on this system"))?;
    let mut c = Command::new(p);
    c.args(args).env_clear().env("PATH", "/usr/sbin:/sbin:/usr/bin:/bin").stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = c.spawn().map_err(|e| format!("could not start {prog}: {e}"))?;
    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        si.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{prog} {}: {}", args.first().copied().unwrap_or(""), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

impl Sys for OsSys {
    fn net_info(&self) -> Result<NetInfo, String> {
        let r4 = run("ip", &["-4", "route", "show", "default"], None).unwrap_or_default();
        let r6 = run("ip", &["-6", "route", "show", "default"], None).unwrap_or_default();
        let d4 = parse_default_route(&r4);
        let d6 = parse_default_route(&r6);
        let physical_if = d4.as_ref().or(d6.as_ref()).map(|d| d.0.clone()).ok_or("there is no default route: connect to a network first")?;
        let mut physical_ifs = vec![physical_if.clone()];
        let addrs = run("ip", &["-o", "addr", "show", "scope", "global"], None)?;
        let mut lan = Vec::new();
        for (dev, net) in parse_addrs(&addrs, &NOT_LAN_PREFIXES) {
            if !physical_ifs.contains(&dev) {
                physical_ifs.push(dev);
            }
            if !lan.contains(&net) {
                lan.push(net);
            }
        }
        Ok(NetInfo {
            physical_if,
            physical_ifs,
            gateway4: d4.and_then(|d| d.1).and_then(|g| g.parse().ok()),
            gateway6: d6.and_then(|d| d.1).and_then(|g| g.parse().ok()),
            lan,
        })
    }

    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        use std::net::ToSocketAddrs;
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let mut v: Vec<IpAddr> = (host, 0u16).to_socket_addrs().map_err(|e| format!("could not resolve {host}: {e}"))?.map(|a| a.ip()).collect();
        v.dedup();
        if v.is_empty() {
            return Err(format!("{host} has no address"));
        }
        Ok(v)
    }

    fn ip(&self, args: &[String]) -> Result<(), String> {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        run("ip", &a, None).map(|_| ())
    }

    fn link_exists(&self, ifname: &str) -> bool {
        std::path::Path::new("/sys/class/net").join(ifname).exists()
    }

    fn wg_configure(&self, d: &DeviceSet) -> Result<(), String> {
        let t = NlTransport::open().map_err(|e| e.to_string())?;
        WgClient::new(t).configure(d).map_err(|e| e.to_string())
    }

    fn wg_status(&self, ifname: &str) -> Result<DeviceStatus, String> {
        let t = NlTransport::open().map_err(|e| e.to_string())?;
        WgClient::new(t).status(ifname).map_err(|e| e.to_string())
    }

    fn read_resolv(&self) -> Option<String> {
        std::fs::read_to_string(&self.resolv_path).ok()
    }

    fn write_resolv(&self, text: &str) -> Result<(), String> {
        let tmp = format!("{}.iyakki-tmp", self.resolv_path);
        std::fs::write(&tmp, text).map_err(|e| format!("could not write {}: {e}", self.resolv_path))?;
        std::fs::rename(&tmp, &self.resolv_path).map_err(|e| format!("could not replace {}: {e}", self.resolv_path))
    }

    fn read_backup(&self) -> Option<String> {
        std::fs::read_to_string(&self.backup_path).ok()
    }

    fn write_backup(&self, text: &str) -> Result<(), String> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(d) = std::path::Path::new(&self.backup_path).parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&self.backup_path).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }

    fn drop_backup(&self) {
        let _ = std::fs::remove_file(&self.backup_path);
    }

    fn install_guard(&self, spec: &KillSwitchSpec) -> Result<(), String> {
        let t = crate::vpn_nft::guard_table(spec)?;
        nf_apply(&t).map_err(|e| format!("the guard table could not be installed: {e}"))
    }

    fn remove_guard(&self) {
        let _ = nf_remove(KS_TABLE);
    }

    fn guard_present(&self) -> bool {
        nf_exists(KS_TABLE)
    }

    fn install_dns_capture(&self, spec: &DnsCaptureSpec) -> Result<(), String> {
        let t = crate::vpn_nft::dns_table(spec)?;
        nf_apply(&t).map_err(|e| format!("the DNS capture rule could not be installed: {e}"))
    }

    fn remove_dns_capture(&self) {
        let _ = nf_remove(DNS_TABLE);
    }

    fn guard_set(&self, ip: IpAddr, expires_in: Option<u64>, add: bool) -> Result<(), String> {
        let set = if ip.is_ipv4() { iyakki_vpn::plan::SET_V4 } else { iyakki_vpn::plan::SET_V6 };
        nf_elem(set, ip, expires_in, add)
    }
}

// ------------------------------------------------------------------------------------------------ nf_tables (our own writer)

#[cfg(target_os = "linux")]
fn nf_socket() -> Result<iyakki_fw::nl::NfSocket, String> {
    use std::sync::Once;
    static LOAD: Once = Once::new();
    LOAD.call_once(|| {
        for m in crate::vpn_nft::MODULES {
            let _ = std::process::Command::new("modprobe").arg(m).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    });
    iyakki_fw::nl::NfSocket::open(5).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn nf_apply(t: &iyakki_fw::nl::tablegen::TableSpec) -> Result<(), String> {
    nf_socket()?.apply_table(t).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn nf_remove(name: &str) -> Result<(), String> {
    nf_socket()?.remove_table(name).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn nf_exists(name: &str) -> bool {
    nf_socket().and_then(|mut s| s.table_exists(name).map_err(|e| e.to_string())).unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn nf_elem(set: &str, ip: IpAddr, timeout: Option<u64>, add: bool) -> Result<(), String> {
    nf_socket()?.set_elem(KS_TABLE, set, ip, timeout, add).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn nf_apply(_: &iyakki_fw::nl::tablegen::TableSpec) -> Result<(), String> {
    Err("nf_tables exists on Linux only".into())
}
#[cfg(not(target_os = "linux"))]
fn nf_remove(_: &str) -> Result<(), String> {
    Err("nf_tables exists on Linux only".into())
}
#[cfg(not(target_os = "linux"))]
fn nf_exists(_: &str) -> bool {
    false
}
#[cfg(not(target_os = "linux"))]
fn nf_elem(_: &str, _: IpAddr, _: Option<u64>, _: bool) -> Result<(), String> {
    Err("nf_tables exists on Linux only".into())
}

/// The generic-netlink socket for WireGuard (family number is asked by [`WgClient`]).
pub struct NlTransport {
    fd: i32,
}

impl NlTransport {
    pub fn open() -> Result<NlTransport, VpnError> {
        let err = || VpnError::Netlink(errno_words(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
        // SAFETY: plain syscalls with valid arguments; the descriptor is closed in Drop.
        let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_GENERIC) };
        if fd < 0 {
            return Err(err());
        }
        let t = NlTransport { fd };
        let tv = libc::timeval { tv_sec: 5, tv_usec: 0 };
        // SAFETY: tv outlives the call and the size matches.
        unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&tv as *const libc::timeval).cast(), std::mem::size_of::<libc::timeval>() as u32) };
        // SAFETY: a zeroed sockaddr_nl with the family set is valid.
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: sa is a valid sockaddr_nl of the given size.
        if unsafe { libc::bind(fd, (&sa as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) } < 0 {
            return Err(err());
        }
        Ok(t)
    }
}

impl Drop for NlTransport {
    fn drop(&mut self) {
        // SAFETY: the descriptor was opened by us and is closed once.
        unsafe { libc::close(self.fd) };
    }
}

fn errno_words(e: i32) -> String {
    match e {
        1 | 13 => "operation not permitted (this needs root)".into(),
        93 | 97 => "generic netlink is not available in this kernel".into(),
        n => format!("system error {n}"),
    }
}

impl GenlTransport for NlTransport {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<Vec<u8>>, VpnError> {
        // SAFETY: request is a valid buffer for its length.
        let n = unsafe { libc::send(self.fd, request.as_ptr().cast(), request.len(), 0) };
        if n < 0 || n as usize != request.len() {
            return Err(VpnError::Netlink(errno_words(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))));
        }
        let mut out = Vec::new();
        for _ in 0..64 {
            let mut buf = vec![0u8; 65536];
            // SAFETY: buf is valid for its length.
            let n = unsafe { libc::recv(self.fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(VpnError::Netlink("the kernel did not answer in time".into()));
            }
            buf.truncate(n as usize);
            let mut done = false;
            for m in iyakki_vpn::wgnl::split_messages(&buf)? {
                let ty = u16::from_ne_bytes([m[4], m[5]]);
                // NLMSG_ERROR (2) is the acknowledgement or the failure, NLMSG_DONE (3) ends a dump
                done |= ty == 2 || ty == 3;
                out.push(m.to_vec());
            }
            if done {
                return Ok(out);
            }
        }
        Err(VpnError::Netlink("the kernel's answer did not end".into()))
    }
}

// ------------------------------------------------------------------------------------------------ the fake

/// In-memory stand-in. `calls` lists every action in order; `fail_on` makes a call fail when its text contains the word
/// (`"link add"`, `"route add"`, `"guard"`, `"wg"`, `"resolve"`, `"resolv"`, `"capture"`).
#[derive(Default)]
pub struct FakeSys {
    pub st: Mutex<FakeState>,
}

#[derive(Default)]
pub struct FakeState {
    pub net: NetInfo,
    pub no_network: bool,
    pub names: Vec<(String, Vec<IpAddr>)>,
    pub links: BTreeSet<String>,
    pub routes: BTreeSet<String>,
    pub resolv: Option<String>,
    pub backup: Option<String>,
    pub guard: Option<KillSwitchSpec>,
    pub guard_members: BTreeSet<String>,
    pub capture: Option<DnsCaptureSpec>,
    pub devices: Vec<(String, bool)>,
    pub configured: Vec<DeviceStatus>,
    pub handshake: Option<i64>,
    pub fail_on: Vec<String>,
    pub calls: Vec<String>,
    /// Seen keys: the private key bytes of the last configure (for the canary test).
    pub last_private: Option<Vec<u8>>,
}

impl FakeSys {
    pub fn new() -> FakeSys {
        let s = FakeSys::default();
        {
            let mut st = s.st.lock().unwrap();
            st.net = NetInfo {
                physical_if: "eth0".into(),
                physical_ifs: vec!["eth0".into()],
                gateway4: Some("192.168.1.1".parse().unwrap()),
                gateway6: None,
                lan: vec![Cidr::parse("192.168.1.0/24").unwrap()],
            };
            st.resolv = Some("nameserver 192.168.1.1\n".into());
        }
        s
    }

    pub fn name(&self, host: &str, ips: &[&str]) {
        self.st.lock().unwrap().names.push((host.into(), ips.iter().map(|i| i.parse().unwrap()).collect()));
    }

    pub fn fail(&self, word: &str) {
        self.st.lock().unwrap().fail_on.push(word.into());
    }

    pub fn clear_failures(&self) {
        self.st.lock().unwrap().fail_on.clear();
    }

    pub fn calls(&self) -> Vec<String> {
        self.st.lock().unwrap().calls.clone()
    }

    /// True when nothing the VPN made is left: link, routes, guard, capture, resolv marker, backup.
    pub fn clean(&self) -> bool {
        let s = self.st.lock().unwrap();
        s.links.is_empty() && s.routes.is_empty() && s.guard.is_none() && s.capture.is_none() && s.backup.is_none() && !s.resolv.as_deref().unwrap_or("").contains("IYAKKI-VPN-DNS") && s.guard_members.is_empty()
    }

    fn step(&self, what: String) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(what.clone());
        if s.fail_on.iter().any(|w| what.contains(w.as_str())) {
            return Err(format!("fake failure: {what}"));
        }
        Ok(())
    }
}

impl Sys for FakeSys {
    fn net_info(&self) -> Result<NetInfo, String> {
        self.step("net_info".into())?;
        let s = self.st.lock().unwrap();
        if s.no_network {
            return Err("there is no default route: connect to a network first".into());
        }
        Ok(s.net.clone())
    }

    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        self.step(format!("resolve {host}"))?;
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        self.st.lock().unwrap().names.iter().find(|(h, _)| h == host).map(|(_, v)| v.clone()).ok_or_else(|| format!("could not resolve {host}"))
    }

    fn ip(&self, args: &[String]) -> Result<(), String> {
        self.step(format!("ip {}", args.join(" ")))?;
        let mut s = self.st.lock().unwrap();
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["link", "add", "dev", n, ..] => {
                if !s.links.insert(n.to_string()) {
                    return Err("file exists".into());
                }
            }
            ["link", "del", "dev", n] => {
                if !s.links.remove(*n) {
                    return Err("cannot find device".into());
                }
                s.routes.retain(|r| !r.ends_with(&format!("dev {n}")));
            }
            ["route", "add", rest @ ..] => {
                if !s.routes.insert(rest.join(" ")) {
                    return Err("file exists".into());
                }
            }
            ["route", "del", rest @ ..] => {
                if !s.routes.remove(&rest.join(" ")) {
                    return Err("no such process".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn link_exists(&self, ifname: &str) -> bool {
        self.st.lock().unwrap().links.contains(ifname)
    }

    fn wg_configure(&self, d: &DeviceSet) -> Result<(), String> {
        self.step(format!("wg configure {}", d.ifname))?;
        let mut s = self.st.lock().unwrap();
        s.last_private = d.private_key.map(|k| k.expose().to_vec());
        let peers = d
            .peers
            .iter()
            .map(|p| iyakki_vpn::wgnl::PeerStatus { public_key: *p.public_key, endpoint: p.endpoint, keepalive: p.keepalive.unwrap_or(0), last_handshake: None, rx_bytes: 0, tx_bytes: 0, allowed_ips: p.allowed_ips.to_vec() })
            .collect();
        s.configured.retain(|c| c.ifname != d.ifname);
        s.configured.push(DeviceStatus { ifname: d.ifname.to_string(), public_key: None, listen_port: d.listen_port.unwrap_or(0), peers });
        Ok(())
    }

    fn wg_status(&self, ifname: &str) -> Result<DeviceStatus, String> {
        self.step(format!("wg status {ifname}"))?;
        let s = self.st.lock().unwrap();
        if !s.links.contains(ifname) {
            return Err("no such device".into());
        }
        let mut d = s.configured.iter().find(|c| c.ifname == ifname).cloned().ok_or("no such device")?;
        for p in &mut d.peers {
            p.last_handshake = s.handshake;
        }
        Ok(d)
    }

    fn read_resolv(&self) -> Option<String> {
        self.st.lock().unwrap().resolv.clone()
    }

    fn write_resolv(&self, text: &str) -> Result<(), String> {
        self.step("resolv write".into())?;
        self.st.lock().unwrap().resolv = Some(text.to_string());
        Ok(())
    }

    fn read_backup(&self) -> Option<String> {
        self.st.lock().unwrap().backup.clone()
    }

    fn write_backup(&self, text: &str) -> Result<(), String> {
        self.step("resolv backup".into())?;
        self.st.lock().unwrap().backup = Some(text.to_string());
        Ok(())
    }

    fn drop_backup(&self) {
        self.st.lock().unwrap().backup = None;
    }

    fn install_guard(&self, spec: &KillSwitchSpec) -> Result<(), String> {
        self.step("guard install".into())?;
        self.st.lock().unwrap().guard = Some(spec.clone());
        Ok(())
    }

    fn remove_guard(&self) {
        let mut s = self.st.lock().unwrap();
        s.calls.push("guard remove".into());
        s.guard = None;
        s.guard_members.clear();
    }

    fn guard_present(&self) -> bool {
        self.st.lock().unwrap().guard.is_some()
    }

    fn install_dns_capture(&self, spec: &DnsCaptureSpec) -> Result<(), String> {
        self.step("capture install".into())?;
        self.st.lock().unwrap().capture = Some(spec.clone());
        Ok(())
    }

    fn remove_dns_capture(&self) {
        let mut s = self.st.lock().unwrap();
        s.calls.push("capture remove".into());
        s.capture = None;
    }

    fn guard_set(&self, ip: IpAddr, _expires_in: Option<u64>, add: bool) -> Result<(), String> {
        self.step(format!("guard set {} {ip}", if add { "add" } else { "del" }))?;
        let mut s = self.st.lock().unwrap();
        if add {
            s.guard_members.insert(ip.to_string());
        } else {
            s.guard_members.remove(&ip.to_string());
        }
        Ok(())
    }
}
