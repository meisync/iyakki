//! The sockets of a sharing session: DHCP (UDP 67) bound to the shared interface only, DNS (UDP 53) bound to the gateway address on that
//! interface only, and one ordinary socket that asks the resolvers. The code is generic over a [`SocketMaker`] so that the tests use
//! fakes (which record what was bound to which device) and loopback sockets; the real maker is `real::SysSockets` (`SO_BINDTODEVICE`).

use super::manager::{Inbound, Io};
use iyakki_hotspot::dhcp::Dest;
use std::net::{Ipv4Addr, SocketAddrV4};

/// One datagram socket that never blocks.
pub trait Datagram {
    fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddrV4)>;
    fn send_to(&mut self, data: &[u8], to: SocketAddrV4) -> bool;
}

pub trait SocketMaker {
    /// Bind a UDP socket to `addr`; with `device` it only ever sees (and sends on) that interface. `broadcast` allows sending to the broadcast address.
    fn bind(&mut self, addr: SocketAddrV4, device: Option<&str>, broadcast: bool) -> Result<Box<dyn Datagram>, String>;
}

/// The port numbers (changed only by the tests, which cannot bind below 1024).
#[derive(Clone, Copy, Debug)]
pub struct Ports {
    pub dhcp_server: u16,
    pub dhcp_client: u16,
    pub dns: u16,
    pub upstream_dns: u16,
    pub broadcast: Ipv4Addr,
}

impl Default for Ports {
    fn default() -> Ports {
        Ports { dhcp_server: 67, dhcp_client: 68, dns: 53, upstream_dns: 53, broadcast: Ipv4Addr::BROADCAST }
    }
}

pub struct Wire {
    dhcp: Box<dyn Datagram>,
    dns: Box<dyn Datagram>,
    up: Box<dyn Datagram>,
    resolvers: Vec<Ipv4Addr>,
    ports: Ports,
}

/// Open the three sockets. DHCP listens on every address but only on `device` (a client with no address yet writes to the broadcast
/// address); DNS listens on the gateway address on `device`; the resolver socket is not tied to any device (it follows the routes).
pub fn open(maker: &mut dyn SocketMaker, device: &str, gateway: Ipv4Addr, resolvers: &[Ipv4Addr], ports: Ports) -> Result<Wire, String> {
    let dhcp = maker.bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, ports.dhcp_server), Some(device), true).map_err(|e| format!("DHCP port {}: {e}", ports.dhcp_server))?;
    let dns = maker.bind(SocketAddrV4::new(gateway, ports.dns), Some(device), false).map_err(|e| format!("DNS port {}: {e}", ports.dns))?;
    let up = maker.bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), None, false).map_err(|e| format!("resolver socket: {e}"))?;
    Ok(Wire { dhcp, dns, up, resolvers: resolvers.to_vec(), ports })
}

const BATCH: usize = 32;

impl Io for Wire {
    fn poll(&mut self) -> Vec<Inbound> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4608];
        for _ in 0..BATCH {
            match self.dhcp.recv(&mut buf) {
                Some((n, _)) => out.push(Inbound::Dhcp(buf[..n].to_vec())),
                None => break,
            }
        }
        for _ in 0..BATCH {
            match self.dns.recv(&mut buf) {
                Some((n, from)) => out.push(Inbound::DnsQuery { src: *from.ip(), sport: from.port(), data: buf[..n].to_vec() }),
                None => break,
            }
        }
        for _ in 0..BATCH {
            match self.up.recv(&mut buf) {
                // only a resolver we asked may answer
                Some((n, from)) if self.resolvers.contains(from.ip()) && from.port() == self.ports.upstream_dns => out.push(Inbound::DnsAnswer(buf[..n].to_vec())),
                Some(_) => {}
                None => break,
            }
        }
        out
    }

    fn send_dhcp(&mut self, bytes: &[u8], dest: Dest) {
        let ip = match dest {
            Dest::Broadcast => self.ports.broadcast,
            Dest::Unicast(ip) => ip,
        };
        self.dhcp.send_to(bytes, SocketAddrV4::new(ip, self.ports.dhcp_client));
    }

    fn send_dns_client(&mut self, ip: Ipv4Addr, port: u16, bytes: &[u8]) {
        self.dns.send_to(bytes, SocketAddrV4::new(ip, port));
    }

    fn send_dns_upstream(&mut self, bytes: &[u8]) {
        for r in self.resolvers.iter().take(2) {
            self.up.send_to(bytes, SocketAddrV4::new(*r, self.ports.upstream_dns));
        }
    }
}
