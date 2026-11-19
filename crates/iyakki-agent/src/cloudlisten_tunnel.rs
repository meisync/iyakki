//! Tunnel targets for the local domain listener (F-CLOUD-77). A route can point at
//!
//! * `tunnel`: an SSH tunnel the person made in Settings -> SSH tunnels (`sshtunnel.rs`). The tunnel listens on this machine (127.0.0.1)
//!   and the listener connects to that port; the tunnel must exist, be a one-page (local forward) tunnel and be enabled. The listener
//!   never starts or changes a tunnel: the supervisor of the tunnels does.
//! * `peer`: an address on the other side of a WireGuard (or other VPN) interface, reached through the machine's own routes. The
//!   address must be inside one of the networks the person named for the listener (`--peer-net`, normally the VPN's allowed IPs) and must
//!   be a private address (RFC 1918, carrier-grade NAT 100.64/10, unique local fc00::/7, never loopback or link-local), so a route can
//!   never be pointed at the open internet or at this machine's own services by this target.
//!
//! Nothing here opens a tunnel or an interface and nothing is sent anywhere else.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

/// How a `tunnel` target finds the local port of an SSH tunnel.
pub trait TunnelPorts: Send + Sync {
    /// The port 127.0.0.1 listens on for the tunnel `id`, or why it cannot be used.
    fn local_port(&self, id: &str) -> Result<u16, String>;
    /// The one-page tunnels there are, as (id, name), for a page that offers them as targets. Empty by default.
    fn choices(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// The tunnels of one person's `tunnels.json`, read afresh at every connection (so a tunnel the person disabled stops being reachable).
pub struct StoreTunnels {
    path: PathBuf,
}

impl StoreTunnels {
    pub fn new(path: PathBuf) -> StoreTunnels {
        StoreTunnels { path }
    }
}

impl TunnelPorts for StoreTunnels {
    fn choices(&self) -> Vec<(String, String)> {
        crate::sshtunnel::Store::new(&self.path).list().into_iter().filter(|t| t.kind == crate::sshtunnel::Kind::LocalForward).map(|t| (t.id, t.name)).collect()
    }

    fn local_port(&self, id: &str) -> Result<u16, String> {
        let t = crate::sshtunnel::Store::new(&self.path).get(id).ok_or_else(|| format!("there is no SSH tunnel {id}"))?;
        if t.kind != crate::sshtunnel::Kind::LocalForward {
            return Err(format!("the SSH tunnel {} is a SOCKS proxy, not a one-page tunnel", t.name));
        }
        if !t.enabled {
            return Err(format!("the SSH tunnel {} is switched off", t.name));
        }
        if t.local_port == 0 {
            return Err(format!("the SSH tunnel {} has no local port yet", t.name));
        }
        Ok(t.local_port)
    }
}

/// Which addresses a `peer` target may name.
pub trait PeerGate: Send + Sync {
    fn allowed(&self, addr: IpAddr) -> bool;
}

/// The networks named for the listener: an address is allowed when it is private and inside one of them.
pub struct PeerNets {
    nets: Vec<(IpAddr, u8)>,
}

impl PeerNets {
    /// `10.8.0.0/24`, `fd00:1::/64` or a single address; at least one, at most 32.
    pub fn parse(list: &[String]) -> Result<PeerNets, String> {
        let mut nets = Vec::new();
        for t in list {
            let (a, b) = t.split_once('/').map_or((t.as_str(), None), |(a, b)| (a, Some(b)));
            let ip: IpAddr = a.trim().parse().map_err(|_| format!("\"{t}\" is not a network (write it like 10.8.0.0/24)"))?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let bits: u8 = match b {
                Some(b) => b.trim().parse().ok().filter(|n| *n <= max).ok_or_else(|| format!("\"{t}\": the prefix is 0 to {max}"))?,
                None => max,
            };
            if bits == 0 {
                return Err(format!("\"{t}\" would allow every address"));
            }
            if !is_private(ip) {
                return Err(format!("\"{t}\" is not a private network"));
            }
            nets.push((ip, bits));
        }
        if nets.is_empty() || nets.len() > 32 {
            return Err("name one to 32 networks".into());
        }
        Ok(PeerNets { nets })
    }
}

impl PeerGate for PeerNets {
    fn allowed(&self, addr: IpAddr) -> bool {
        is_private(addr) && self.nets.iter().any(|(n, bits)| contains(*n, *bits, addr))
    }
}

pub use iyakki_shared::cloudnames::is_private;

fn contains(net: IpAddr, bits: u8, a: IpAddr) -> bool {
    match (net, a) {
        (IpAddr::V4(n), IpAddr::V4(a)) => mask4(n, bits) == mask4(a, bits),
        (IpAddr::V6(n), IpAddr::V6(a)) => mask6(n, bits) == mask6(a, bits),
        _ => false,
    }
}

fn mask4(a: Ipv4Addr, bits: u8) -> u32 {
    let m = if bits == 0 { 0 } else { u32::MAX << (32 - u32::from(bits.min(32))) };
    u32::from(a) & m
}

fn mask6(a: Ipv6Addr, bits: u8) -> u128 {
    let m = if bits == 0 { 0 } else { u128::MAX << (128 - u32::from(bits.min(128))) };
    u128::from(a) & m
}

/// Connect to a `peer` target: the gate decides first, then the machine's own routes carry the connection.
pub fn dial_peer(gate: Option<&dyn PeerGate>, addr: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let ip: IpAddr = addr.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the peer address is not an address"))?;
    match gate {
        Some(g) if g.allowed(ip) => {}
        Some(_) => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "that address is not inside the networks named for this listener")),
        None => return Err(io::Error::new(io::ErrorKind::Unsupported, "no VPN network is named for this listener (--peer-net)")),
    }
    let s = TcpStream::connect_timeout(&SocketAddr::new(ip, port), timeout)?;
    let _ = s.set_nodelay(true);
    Ok(s)
}

pub use iyakki_shared::cloudnames::check_peer;

/// The networks of the person's enabled WireGuard profiles: the addresses their peers are allowed to carry (never a default route and
/// never a network that is not private), read afresh at every connection, so switching a VPN off closes the routes that depend on it.
pub struct VpnPeers {
    nets: Box<dyn Fn() -> Vec<iyakki_filter::net::Cidr> + Send + Sync>,
}

impl VpnPeers {
    pub fn new(nets: Box<dyn Fn() -> Vec<iyakki_filter::net::Cidr> + Send + Sync>) -> VpnPeers {
        VpnPeers { nets }
    }

    /// Over the profiles of a VPN manager: those that are switched on and WireGuard.
    pub fn of_manager(mgr: std::sync::Arc<crate::vpn_mgr::Manager>) -> VpnPeers {
        VpnPeers::new(Box::new(move || {
            mgr.rows()
                .into_iter()
                .filter(|r| r.enabled)
                .filter_map(|r| match r.profile.body {
                    iyakki_vpn::profile::Body::Wireguard(c) => Some(c.peers.into_iter().flat_map(|p| p.allowed_ips).collect::<Vec<_>>()),
                    _ => None,
                })
                .flatten()
                .collect()
        }))
    }
}

impl PeerGate for VpnPeers {
    fn allowed(&self, addr: IpAddr) -> bool {
        is_private(addr) && (self.nets)().iter().any(|c| !c.is_default() && is_private(c.addr) && c.contains_ip(addr))
    }
}
