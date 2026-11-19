//! Checking what a person typed. Only values that pass here ever reach the ruleset, so nothing typed can become part of a rule
//! text: addresses and ports are numbers, host names are letters, digits, dots and hyphens, comments never enter the kernel.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::model::{Kind, Proto, Rule};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortRange {
    pub lo: u16,
    pub hi: u16,
}

/// A rule's source after checking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Src {
    Ip(IpAddr),
    Range(IpAddr, IpAddr),
    Cidr(IpAddr, u8),
    Host(String),
}

/// A rule after checking, with warnings the page shows next to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub src: Src,
    pub ports: Vec<PortRange>,
    pub iface: Option<String>,
    pub warnings: Vec<String>,
}

pub const MAX_PORT_ITEMS: usize = 16;

fn usable(ip: IpAddr) -> Result<IpAddr, String> {
    if ip.is_unspecified() {
        return Err("0.0.0.0 and :: are not addresses of a computer. To allow everyone, use the CIDR 0.0.0.0/0 (and think twice).".into());
    }
    if ip.is_multicast() {
        return Err("A multicast address cannot open a connection to this machine.".into());
    }
    if ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return Err("The broadcast address cannot open a connection to this machine.".into());
    }
    Ok(ip)
}

pub fn parse_ip(s: &str) -> Result<IpAddr, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Type an IP address.".into());
    }
    if s.contains('%') {
        return Err("Leave out the interface (%name) after an IPv6 address; use the Interface field.".into());
    }
    let ip: IpAddr = s.parse().map_err(|_| format!("'{}' is not an IP address (for example 192.168.1.20 or fd00::5).", clip(s)))?;
    usable(ip)
}

fn clip(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(40).collect();
    t
}

fn bits(ip: IpAddr) -> u32 {
    if ip.is_ipv4() {
        32
    } else {
        128
    }
}

fn num(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(a) => u32::from(a) as u128,
        IpAddr::V6(a) => u128::from(a),
    }
}

pub fn parse_range(s: &str) -> Result<(IpAddr, IpAddr, Vec<String>), String> {
    let Some((a, b)) = s.split_once('-') else { return Err("An IP range is written first-last, for example 192.168.1.10-192.168.1.50.".into()) };
    let (a, b) = (parse_ip(a)?, parse_ip(b)?);
    if a.is_ipv4() != b.is_ipv4() {
        return Err("Both ends of a range must be the same kind of address (both IPv4 or both IPv6).".into());
    }
    if num(a) > num(b) {
        return Err("The range is reversed: the first address must be lower than the last.".into());
    }
    let mut w = Vec::new();
    let size = num(b) - num(a) + 1;
    if (a.is_ipv4() && size > 1 << 24) || (a.is_ipv6() && size > 1u128 << 80) {
        w.push("This range is very large: it allows a big part of the internet.".to_string());
    }
    Ok((a, b, w))
}

pub fn parse_cidr(s: &str) -> Result<(IpAddr, u8, Vec<String>), String> {
    let Some((a, p)) = s.split_once('/') else { return Err("A CIDR is an address and a length, for example 192.168.1.0/24.".into()) };
    let ip: IpAddr = a.trim().parse().map_err(|_| format!("'{}' is not an IP address.", clip(a)))?;
    let len: u8 = p.trim().parse().map_err(|_| "The number after / must be a whole number.".to_string())?;
    if u32::from(len) > bits(ip) {
        return Err(format!("The length after / is at most {} for this kind of address.", bits(ip)));
    }
    let host_bits = bits(ip) - u32::from(len);
    let host_mask: u128 = if host_bits >= 128 { u128::MAX } else { (1u128 << host_bits) - 1 };
    if num(ip) & host_mask != 0 {
        return Err(format!("{} has bits set after the /{len}; the network address is {}.", clip(a), network_of(ip, len)));
    }
    let mut w = Vec::new();
    if len == 0 {
        w.push("/0 allows everyone on the internet in.".to_string());
    } else if u32::from(len) <= bits(ip) / 4 {
        w.push(format!("/{len} is a very large network; it allows a big part of the internet."));
    }
    if ip.is_multicast() {
        return Err("A multicast network cannot open a connection to this machine.".into());
    }
    Ok((ip, len, w))
}

pub fn network_of(ip: IpAddr, len: u8) -> IpAddr {
    match ip {
        IpAddr::V4(a) => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - u32::from(len.min(32))) };
            IpAddr::V4(Ipv4Addr::from(u32::from(a) & mask))
        }
        IpAddr::V6(a) => {
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - u32::from(len.min(128))) };
            IpAddr::V6(Ipv6Addr::from(u128::from(a) & mask))
        }
    }
}

/// Lower case, one trailing dot removed.
pub fn normalize_host(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase();
    t.strip_suffix('.').unwrap_or(&t).to_string()
}

pub fn check_host(s: &str) -> Result<String, String> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err("Type a host name, for example laptop.local or files.example.com.".into());
    }
    if raw.contains('*') {
        return Err("Wildcard names (*.example.com) are not supported: a name must be one exact host.".into());
    }
    if !raw.is_ascii() {
        return Err("Write an international name in its xn-- form (for example xn--bcher-kva.example).".into());
    }
    let n = normalize_host(raw);
    if n.ends_with('.') || n.is_empty() {
        return Err("Only one dot may end a name.".into());
    }
    if n.len() > 253 {
        return Err("A host name is at most 253 characters.".into());
    }
    if n.parse::<IpAddr>().is_ok() {
        return Err("That is an IP address; choose the IP type.".into());
    }
    for label in n.split('.') {
        if label.is_empty() {
            return Err("A host name cannot have an empty part (two dots in a row, or a leading dot).".into());
        }
        if label.len() > 63 {
            return Err("Each part of a host name is at most 63 characters.".into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("A part of a host name cannot start or end with a hyphen.".into());
        }
        if let Some(c) = label.chars().find(|c| !(c.is_ascii_alphanumeric() || *c == '-')) {
            return Err(if c == '_' { "Underscores are not allowed in host names.".to_string() } else { format!("The character '{}' is not allowed in a host name (letters, digits, hyphens and dots only).", if c.is_control() { ' ' } else { c }) });
        }
    }
    if n.rsplit('.').next().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())) {
        return Err("The last part of a host name cannot be all digits.".into());
    }
    Ok(n)
}

pub fn parse_ports(s: &str) -> Result<Vec<PortRange>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for item in s.split(',') {
        let item = item.trim();
        let port = |t: &str| -> Result<u16, String> {
            let n: u32 = t.trim().parse().map_err(|_| format!("'{}' is not a port number.", clip(t)))?;
            if !(1..=65535).contains(&n) {
                return Err(format!("Port {n} is outside 1 to 65535."));
            }
            Ok(n as u16)
        };
        let (lo, hi) = match item.split_once('-') {
            Some((a, b)) => (port(a)?, port(b)?),
            None => {
                let p = port(item)?;
                (p, p)
            }
        };
        if lo > hi {
            return Err(format!("The port range {lo}-{hi} is reversed."));
        }
        out.push(PortRange { lo, hi });
    }
    if out.len() > MAX_PORT_ITEMS {
        return Err(format!("At most {MAX_PORT_ITEMS} ports or port ranges per rule."));
    }
    Ok(out)
}

pub fn check_iface(s: &str) -> Result<Option<String>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > 15 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err("An interface name is up to 15 letters, digits, - _ or . (for example eth0 or wlan0).".into());
    }
    Ok(Some(s.to_string()))
}

pub fn check_comment(s: &str) -> Result<(), String> {
    if s.chars().count() > 80 {
        return Err("The note is at most 80 characters.".into());
    }
    if s.chars().any(|c| c.is_control()) {
        return Err("The note cannot contain line breaks or control characters.".into());
    }
    Ok(())
}

/// Check one rule completely.
pub fn check_rule(r: &Rule) -> Result<Parsed, String> {
    check_comment(&r.comment)?;
    let mut warnings = Vec::new();
    let src = match r.kind {
        Kind::Ip => Src::Ip(parse_ip(&r.value)?),
        Kind::Range => {
            let (a, b, w) = parse_range(&r.value)?;
            warnings.extend(w);
            if a == b {
                Src::Ip(a)
            } else {
                Src::Range(a, b)
            }
        }
        Kind::Cidr => {
            let (a, l, w) = parse_cidr(&r.value)?;
            warnings.extend(w);
            Src::Cidr(a, l)
        }
        Kind::Host => Src::Host(check_host(&r.value)?),
    };
    let ports = parse_ports(&r.ports)?;
    let iface = check_iface(&r.iface)?;
    if ports.is_empty() && r.proto == Proto::Both {
        warnings.push("Every port and protocol from this source is allowed.".into());
    }
    if let Src::Host(_) = src {
        warnings.push("A name is only as trustworthy as the DNS server that answers for it; someone who controls your DNS can make a name point at their computer.".into());
    }
    Ok(Parsed { src, ports, iface, warnings })
}
