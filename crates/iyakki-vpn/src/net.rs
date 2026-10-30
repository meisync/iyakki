//! Addresses and networks, and the checks of the threat list ("Route hijack", "DNS hijack", "Endpoint abuse").
//! All pure: the cluster CIDRs and the LAN subnets are inputs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::error::VpnError;

/// An address with a prefix length, stored as given (a host address with its network, as in `Address = 10.8.0.2/24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    pub fn new(addr: IpAddr, prefix: u8) -> Option<Cidr> {
        let max = if addr.is_ipv4() { 32 } else { 128 };
        (prefix <= max).then_some(Cidr { addr, prefix })
    }
    /// `a.b.c.d/n`, `x::y/n` or a bare address (a host: /32 or /128).
    pub fn parse(s: &str) -> Option<Cidr> {
        let s = s.trim();
        match s.split_once('/') {
            Some((a, p)) => {
                if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Cidr::new(a.parse().ok()?, p.parse().ok()?)
            }
            None => {
                let a: IpAddr = s.parse().ok()?;
                Cidr::new(a, if a.is_ipv4() { 32 } else { 128 })
            }
        }
    }
    pub fn is_v4(&self) -> bool {
        self.addr.is_ipv4()
    }
    fn width(&self) -> u8 {
        if self.is_v4() {
            32
        } else {
            128
        }
    }
    fn value(&self) -> u128 {
        match self.addr {
            IpAddr::V4(a) => u32::from(a) as u128,
            IpAddr::V6(a) => u128::from(a),
        }
    }
    fn mask(&self) -> u128 {
        let (w, p) = (self.width() as u32, self.prefix as u32);
        if p == 0 {
            0
        } else {
            (!0u128 >> (128 - p)) << (w - p)
        }
    }
    /// The network address (host bits cleared).
    pub fn network(&self) -> Cidr {
        let v = self.value() & self.mask();
        Cidr { addr: from_value(v, self.is_v4()), prefix: self.prefix }
    }
    /// True when every address of `other` is inside `self`.
    pub fn contains(&self, other: &Cidr) -> bool {
        self.is_v4() == other.is_v4() && other.prefix >= self.prefix && (other.value() & self.mask()) == (self.value() & self.mask())
    }
    pub fn contains_ip(&self, ip: IpAddr) -> bool {
        Cidr::new(ip, if ip.is_ipv4() { 32 } else { 128 }).is_some_and(|c| self.contains(&c))
    }
    /// True when the two share at least one address (one contains the other).
    pub fn overlaps(&self, other: &Cidr) -> bool {
        self.contains(other) || other.contains(self)
    }
    /// `0.0.0.0/0` or `::/0`.
    pub fn is_default(&self) -> bool {
        self.prefix == 0
    }
    /// The four halves used instead of a default route.
    pub fn def1_halves(v4: bool) -> [Cidr; 2] {
        if v4 {
            [Cidr::parse("0.0.0.0/1").unwrap(), Cidr::parse("128.0.0.0/1").unwrap()]
        } else {
            [Cidr::parse("::/1").unwrap(), Cidr::parse("8000::/1").unwrap()]
        }
    }
    pub fn is_half(&self) -> bool {
        self.prefix == 1 && self.value() & self.mask() == self.value()
    }
}

fn from_value(v: u128, v4: bool) -> IpAddr {
    if v4 {
        IpAddr::V4(Ipv4Addr::from(v as u32))
    } else {
        IpAddr::V6(Ipv6Addr::from(v))
    }
}

/// Addresses only (no scope ids, no brackets).
pub fn parse_ip(s: &str) -> Option<IpAddr> {
    s.trim().parse::<IpAddr>().ok()
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

impl From<Cidr> for String {
    fn from(c: Cidr) -> String {
        c.to_string()
    }
}

impl TryFrom<String> for Cidr {
    type Error = String;
    fn try_from(s: String) -> Result<Cidr, String> {
        Cidr::parse(&s).ok_or_else(|| "not an address with a prefix".to_string())
    }
}

/// The k3s defaults, used when no cluster exists to ask (`k3sdev.rs`).
pub fn default_cluster_cidrs() -> Vec<Cidr> {
    vec![Cidr::parse("10.42.0.0/16").unwrap(), Cidr::parse("10.43.0.0/16").unwrap()]
}

/// What the checks need to know about this machine.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub cluster: Vec<Cidr>,
    /// Connected subnets of the physical interfaces.
    pub lan: Vec<Cidr>,
    /// The VPN's own tunnel addresses (`Address =`), as networks.
    pub tunnel: Vec<Cidr>,
}

fn special_no_go() -> Vec<(Cidr, &'static str)> {
    vec![
        (Cidr::parse("127.0.0.0/8").unwrap(), "loopback"),
        (Cidr::parse("::1/128").unwrap(), "loopback"),
        (Cidr::parse("169.254.0.0/16").unwrap(), "link-local (cloud metadata services live here)"),
        (Cidr::parse("fe80::/10").unwrap(), "link-local"),
    ]
}

fn private_ranges() -> Vec<Cidr> {
    ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"].iter().map(|s| Cidr::parse(s).unwrap()).collect()
}

/// Result of checking one route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteVerdict {
    Ok,
    /// Allowed, but the person must see it before it is applied.
    Warn(String),
    Refuse(String),
}

/// One check for every route: profile `AllowedIPs`, OpenVPN `route`, pushed routes. Halves (`/1`) and defaults (`/0`) are the
/// full-tunnel forms and are always fine because every longer route (cluster, LAN) still wins over them.
pub fn check_route(r: &Cidr, ctx: &Context) -> RouteVerdict {
    if r.prefix <= 1 {
        return RouteVerdict::Ok;
    }
    for (bad, why) in special_no_go() {
        if r.overlaps(&bad) {
            return RouteVerdict::Refuse(format!("{r} would take over {why} traffic"));
        }
    }
    for c in &ctx.cluster {
        if r.overlaps(c) {
            return RouteVerdict::Refuse(format!("{r} overlaps the cluster network {c}"));
        }
    }
    for l in &ctx.lan {
        if r.overlaps(l) {
            return RouteVerdict::Refuse(format!("{r} overlaps the local network {l} and would take it away"));
        }
    }
    if ctx.tunnel.iter().any(|t| t.contains(r)) {
        return RouteVerdict::Ok;
    }
    if private_ranges().iter().any(|p| p.overlaps(r)) {
        return RouteVerdict::Warn(format!("{r} is a private range that is not the VPN's own network; traffic to it will leave through the tunnel"));
    }
    RouteVerdict::Ok
}

/// DNS servers: addresses only, at most 3 (extra ones are dropped with a warning), never loopback, unspecified or inside the
/// cluster. Returns the accepted list and the warnings.
pub fn check_dns(servers: &[String], ctx: &Context) -> Result<(Vec<IpAddr>, Vec<String>), VpnError> {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for s in servers {
        let ip = parse_ip(s).ok_or_else(|| VpnError::Unsafe("a DNS server must be an address, not a name".into()))?;
        if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
            return Err(VpnError::Unsafe(format!("DNS server {ip} is not allowed (loopback, unspecified or multicast)")));
        }
        if ctx.cluster.iter().any(|c| c.contains_ip(ip)) {
            return Err(VpnError::Unsafe(format!("DNS server {ip} is inside the cluster network")));
        }
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    if out.len() > 3 {
        warnings.push(format!("{} DNS servers were given; only the first 3 are used", out.len()));
        out.truncate(3);
    }
    Ok((out, warnings))
}

/// What the endpoint check decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointVerdict {
    Ok,
    /// On the local network: allowed only after the administrator confirms.
    NeedsConfirm(String),
    Refuse(String),
}

/// The resolved endpoint address (resolve once, then check, then pin).
pub fn check_endpoint(ip: IpAddr, ctx: &Context) -> EndpointVerdict {
    if ip.is_unspecified() || ip.is_multicast() {
        return EndpointVerdict::Refuse(format!("{ip} cannot be a VPN server"));
    }
    for (bad, why) in special_no_go() {
        if bad.contains_ip(ip) {
            return EndpointVerdict::Refuse(format!("{ip} is {why}"));
        }
    }
    if ctx.cluster.iter().any(|c| c.contains_ip(ip)) {
        return EndpointVerdict::Refuse(format!("{ip} is inside the cluster network"));
    }
    if ctx.lan.iter().any(|c| c.contains_ip(ip)) || private_ranges().iter().any(|p| p.contains_ip(ip)) {
        return EndpointVerdict::NeedsConfirm(format!("{ip} is an address on your local network"));
    }
    EndpointVerdict::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        Cidr::parse(s).unwrap()
    }
    fn ctx() -> Context {
        Context { cluster: default_cluster_cidrs(), lan: vec![c("192.168.1.0/24")], tunnel: vec![c("10.8.0.2/24")] }
    }

    #[test]
    fn parsing_and_masking() {
        assert_eq!(c("10.8.0.2/24").network(), c("10.8.0.0/24"));
        assert_eq!(c("1.2.3.4"), c("1.2.3.4/32"));
        assert_eq!(c("fd00::5").prefix, 128);
        assert_eq!(c("fd00::5/64").network().to_string(), "fd00::/64");
        assert_eq!(c("0.0.0.0/0").network().to_string(), "0.0.0.0/0");
        for bad in ["", "1.2.3.4/33", "::/129", "1.2.3/24", "1.2.3.4/", "1.2.3.4/-1", "a/8", "1.2.3.4/8/8", "1.2.3.4 /8"] {
            assert!(Cidr::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(serde_json::to_string(&c("10.0.0.0/8")).unwrap(), "\"10.0.0.0/8\"");
        assert!(serde_json::from_str::<Cidr>("\"nope\"").is_err());
    }

    #[test]
    fn containment_and_overlap_are_per_family() {
        assert!(c("10.0.0.0/8").contains(&c("10.42.0.0/16")));
        assert!(!c("10.42.0.0/16").contains(&c("10.0.0.0/8")));
        assert!(c("10.42.0.0/16").overlaps(&c("10.0.0.0/8")));
        assert!(!c("10.42.0.0/16").overlaps(&c("10.43.0.0/16")));
        assert!(!c("::/0").overlaps(&c("10.0.0.0/8")));
        assert!(c("0.0.0.0/0").contains(&c("8.8.8.8")));
        assert!(c("::/1").is_half() && c("128.0.0.0/1").is_half() && c("8000::/1").is_half());
        assert!(!c("64.0.0.0/1").is_half() || c("64.0.0.0/1").network() == c("0.0.0.0/1"));
    }

    #[test]
    fn routes_follow_the_threat_table() {
        let x = ctx();
        for refused in ["10.42.0.0/24", "10.43.0.0/16", "10.0.0.0/8", "0.0.0.0/2", "8.0.0.0/5", "127.0.0.0/8", "127.0.0.1/32", "169.254.169.254/32", "192.168.1.0/24", "192.168.1.50/32", "192.168.0.0/16", "::1/128", "fe80::/10"] {
            assert!(matches!(check_route(&c(refused), &x), RouteVerdict::Refuse(_)), "{refused}");
        }
        // the full-tunnel forms are fine
        for ok in ["0.0.0.0/0", "0.0.0.0/1", "128.0.0.0/1", "::/0", "8000::/1", "8.8.8.0/24", "203.0.113.0/24"] {
            assert_eq!(check_route(&c(ok), &x), RouteVerdict::Ok, "{ok}");
        }
        // the VPN's own network is fine, another private range asks
        assert_eq!(check_route(&c("10.8.0.0/24"), &x), RouteVerdict::Ok);
        assert!(matches!(check_route(&c("172.20.0.0/16"), &x), RouteVerdict::Warn(_)));
        assert!(matches!(check_route(&c("10.9.0.0/16"), &x), RouteVerdict::Warn(_)));
        assert!(matches!(check_route(&c("fd12::/48"), &x), RouteVerdict::Warn(_)));
    }

    #[test]
    fn dns_rules() {
        let x = ctx();
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(check_dns(&s(&["10.8.0.1", "1.1.1.1", "10.8.0.1"]), &x).unwrap().0.len(), 2);
        for bad in ["127.0.0.1", "::1", "10.43.0.10", "0.0.0.0", "dns.example.com", "1.1.1.1:53", "224.0.0.1"] {
            assert!(check_dns(&s(&[bad]), &x).is_err(), "{bad}");
        }
        let many: Vec<String> = (1..=10).map(|i| format!("9.9.9.{i}")).collect();
        let (ok, w) = check_dns(&many, &x).unwrap();
        assert_eq!(ok.len(), 3);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn endpoint_rules() {
        let x = ctx();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(check_endpoint(ip("203.0.113.7"), &x), EndpointVerdict::Ok);
        assert_eq!(check_endpoint(ip("2001:db8::1"), &x), EndpointVerdict::Ok);
        for bad in ["127.0.0.1", "10.42.0.5", "169.254.169.254", "0.0.0.0", "::1", "fe80::1", "224.0.0.1"] {
            assert!(matches!(check_endpoint(ip(bad), &x), EndpointVerdict::Refuse(_)), "{bad}");
        }
        for ask in ["192.168.1.20", "10.1.2.3", "172.16.5.5", "fd00::7"] {
            assert!(matches!(check_endpoint(ip(ask), &x), EndpointVerdict::NeedsConfirm(_)), "{ask}");
        }
    }
}
