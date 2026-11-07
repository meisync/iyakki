//! A made-up machine for the sharing tests: records every call in order and can be told to fail one step.

use super::io::{self, Datagram, Ports, SocketMaker};
use super::manager::{Inbound, Io, Os};
use super::{IfaceInfo, LinkKind, Saved};
use iyakki_fw::nl::tablegen::TableSpec;
use iyakki_hotspot::dhcp::Dest;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct IoState {
    pub inbound: Vec<Inbound>,
    pub dhcp_out: Vec<(Vec<u8>, Dest)>,
    pub dns_client: Vec<(Ipv4Addr, u16, Vec<u8>)>,
    pub dns_up: Vec<Vec<u8>>,
    pub closed: bool,
}

#[derive(Default)]
pub struct State {
    pub ifs: Vec<IfaceInfo>,
    pub now: u64,
    pub forward: String,
    pub rules: Option<TableSpec>,
    pub zone: Vec<String>,
    pub addrs: Vec<(String, Ipv4Addr)>,
    pub saved: Option<Saved>,
    pub calls: Vec<String>,
    pub audit: Vec<String>,
    /// One of "addr", "io", "rules", "forward": that step fails.
    pub fail: Option<&'static str>,
    pub resolvers: Vec<Ipv4Addr>,
    pub wifi_ap: bool,
    /// `wifi_ap_start` fails with this line when set.
    pub wifi_ap_error: Option<&'static str>,
    pub io: Arc<Mutex<IoState>>,
    pub opened: Vec<(String, Ipv4Addr)>,
    /// When set, `open_io` makes real sockets on loopback with these ports.
    pub loop_ports: Option<Ports>,
    pub next_id: u16,
    pub policy: super::policy::Policy,
}

#[derive(Clone, Default)]
pub struct FakeOs(pub Arc<Mutex<State>>);

impl FakeOs {
    pub fn new(ifs: Vec<IfaceInfo>) -> FakeOs {
        let o = FakeOs::default();
        {
            let mut s = o.st();
            s.ifs = ifs;
            s.forward = "0".into();
            s.now = 1_000_000;
            s.next_id = 0x1234;
            s.policy = super::policy::Policy { wired: super::policy::Who::Users, wifi: super::policy::Who::Users, bluetooth: super::policy::Who::Users, ..Default::default() };
        }
        o
    }
    pub fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn advance_secs(&self, s: u64) {
        self.st().now += s * 1000;
    }
    pub fn io(&self) -> Arc<Mutex<IoState>> {
        self.st().io.clone()
    }
}

struct FakeIo(Arc<Mutex<IoState>>);

impl Io for FakeIo {
    fn poll(&mut self) -> Vec<Inbound> {
        std::mem::take(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()).inbound)
    }
    fn send_dhcp(&mut self, b: &[u8], d: Dest) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).dhcp_out.push((b.to_vec(), d));
    }
    fn send_dns_client(&mut self, ip: Ipv4Addr, port: u16, b: &[u8]) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).dns_client.push((ip, port, b.to_vec()));
    }
    fn send_dns_upstream(&mut self, b: &[u8]) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).dns_up.push(b.to_vec());
    }
}

impl Drop for FakeIo {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
    }
}

/// Real UDP sockets on 127.0.0.1 (the device is ignored); the ports come from [`Ports`].
pub struct LoopMaker;

struct LoopSock(std::net::UdpSocket);

impl Datagram for LoopSock {
    fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddrV4)> {
        match self.0.recv_from(buf) {
            Ok((n, std::net::SocketAddr::V4(a))) => Some((n, a)),
            _ => None,
        }
    }
    fn send_to(&mut self, d: &[u8], to: SocketAddrV4) -> bool {
        self.0.send_to(d, to).is_ok()
    }
}

impl SocketMaker for LoopMaker {
    fn bind(&mut self, addr: SocketAddrV4, _device: Option<&str>, _broadcast: bool) -> Result<Box<dyn Datagram>, String> {
        let s = std::net::UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, addr.port())).map_err(|e| e.to_string())?;
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Box::new(LoopSock(s)))
    }
}

/// Records what was bound to which device.
#[derive(Default)]
pub struct RecordingMaker {
    pub binds: Vec<(SocketAddrV4, Option<String>, bool)>,
    pub fail_port: Option<u16>,
}

struct Null;
impl Datagram for Null {
    fn recv(&mut self, _: &mut [u8]) -> Option<(usize, SocketAddrV4)> {
        None
    }
    fn send_to(&mut self, _: &[u8], _: SocketAddrV4) -> bool {
        true
    }
}

impl SocketMaker for RecordingMaker {
    fn bind(&mut self, addr: SocketAddrV4, device: Option<&str>, broadcast: bool) -> Result<Box<dyn Datagram>, String> {
        if self.fail_port == Some(addr.port()) {
            return Err("address in use".into());
        }
        self.binds.push((addr, device.map(String::from), broadcast));
        Ok(Box::new(Null))
    }
}

impl Os for FakeOs {
    fn now_ms(&self) -> u64 {
        self.st().now
    }
    fn random_u16(&mut self) -> u16 {
        self.st().next_id
    }
    fn interfaces(&mut self) -> Vec<IfaceInfo> {
        self.st().ifs.clone()
    }
    fn wifi_ap_available(&mut self) -> bool {
        self.st().wifi_ap
    }
    fn wifi_ap_start(&mut self) -> Result<String, String> {
        let mut s = self.st();
        s.calls.push("wifi_ap_start".into());
        if let Some(e) = s.wifi_ap_error {
            return Err(e.into());
        }
        let name = crate::wifi_ap::AP_IFACE.to_string();
        if !s.ifs.iter().any(|i| i.name == name) {
            s.ifs.push(IfaceInfo::new(&name, LinkKind::Wifi));
        }
        Ok(name)
    }
    fn wifi_ap_stop(&mut self) {
        let mut s = self.st();
        s.calls.push("wifi_ap_stop".into());
        let n = crate::wifi_ap::AP_IFACE;
        s.ifs.retain(|i| i.name != n);
    }
    fn resolvers(&mut self) -> Vec<Ipv4Addr> {
        self.st().resolvers.clone()
    }
    fn policy(&mut self) -> super::policy::Policy {
        self.st().policy.clone()
    }
    fn forward_get(&mut self) -> Result<String, String> {
        Ok(self.st().forward.clone())
    }
    fn forward_set(&mut self, v: &str) -> Result<(), String> {
        let mut s = self.st();
        s.calls.push(format!("forward={v}"));
        if v == "1" && s.fail == Some("forward") {
            return Err("read-only".into());
        }
        s.forward = v.into();
        Ok(())
    }
    fn link_up(&mut self, i: &str) -> Result<(), String> {
        self.st().calls.push(format!("up {i}"));
        Ok(())
    }
    fn add_address(&mut self, i: &str, ip: Ipv4Addr, p: u8) -> Result<(), String> {
        let mut s = self.st();
        s.calls.push(format!("addr+ {i} {ip}/{p}"));
        if s.fail == Some("addr") {
            return Err("busy".into());
        }
        s.addrs.push((i.into(), ip));
        Ok(())
    }
    fn remove_address(&mut self, i: &str, ip: Ipv4Addr, p: u8) -> Result<(), String> {
        let mut s = self.st();
        s.calls.push(format!("addr- {i} {ip}/{p}"));
        s.addrs.retain(|(n, a)| !(n == i && *a == ip));
        Ok(())
    }
    fn apply_rules(&mut self, spec: &TableSpec) -> Result<(), String> {
        let mut s = self.st();
        s.calls.push(format!("rules+ {}", spec.name));
        if s.fail == Some("rules") {
            return Err("kernel said no".into());
        }
        s.rules = Some(spec.clone());
        Ok(())
    }
    fn remove_rules(&mut self) -> Result<(), String> {
        let mut s = self.st();
        s.calls.push("rules-".into());
        s.rules = None;
        Ok(())
    }
    fn rules_present(&mut self) -> Result<bool, String> {
        Ok(self.st().rules.is_some())
    }
    fn set_firewall_zone(&mut self, z: Vec<String>) {
        let mut s = self.st();
        s.calls.push(format!("zone {}", z.join(",")));
        s.zone = z;
    }
    fn open_io(&mut self, device: &str, gw: Ipv4Addr, resolvers: &[Ipv4Addr]) -> Result<Box<dyn Io>, String> {
        let mut s = self.st();
        s.calls.push(format!("io {device} {gw}"));
        if s.fail == Some("io") {
            return Err("port 67 in use".into());
        }
        s.opened.push((device.into(), gw));
        s.io = Arc::new(Mutex::new(IoState::default()));
        if let Some(p) = s.loop_ports {
            let w = io::open(&mut LoopMaker, device, gw, resolvers, p)?;
            return Ok(Box::new(w));
        }
        Ok(Box::new(FakeIo(s.io.clone())))
    }
    fn load_saved(&mut self) -> Option<Saved> {
        self.st().saved.clone()
    }
    fn save_saved(&mut self, v: Option<&Saved>) {
        self.st().saved = v.cloned();
    }
    fn audit(&mut self, line: &str) {
        self.st().audit.push(line.into());
    }
}
