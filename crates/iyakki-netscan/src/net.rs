//! Which networks this machine is on, and which addresses a scan may touch. A scan stays on the local network: private
//! (RFC 1918) and link-local addresses, plus this machine itself. Anything else is refused before a packet is sent.

use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub iface: String,
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl Subnet {
    pub fn cidr(&self) -> String {
        let mask = prefix_mask(self.prefix);
        format!("{}/{}", Ipv4Addr::from(u32::from(self.addr) & mask), self.prefix)
    }
}

fn prefix_mask(p: u8) -> u32 {
    if p == 0 { 0 } else { u32::MAX << (32 - p.min(32) as u32) }
}

/// Addresses on the local network: 10/8, 172.16/12, 192.168/16, 169.254/16.
pub fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// May a scan touch this address? Private ones and this machine's own loopback.
pub fn allowed(ip: Ipv4Addr) -> bool {
    is_private(ip) || ip.is_loopback()
}

/// The IPv4 networks of the interfaces that are up (not loopback).
pub fn local_subnets() -> Vec<Subnet> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs allocates a list that freeifaddrs releases; the entries are only read.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut cur = list;
        while !cur.is_null() {
            let a = &*cur;
            cur = a.ifa_next;
            if a.ifa_addr.is_null() || a.ifa_netmask.is_null() || a.ifa_flags & (libc::IFF_UP as u32) == 0 || a.ifa_flags & (libc::IFF_LOOPBACK as u32) != 0 {
                continue;
            }
            if i32::from((*a.ifa_addr).sa_family) != libc::AF_INET {
                continue;
            }
            let ip = Ipv4Addr::from(u32::from_be((*(a.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr));
            let mask = u32::from_be((*(a.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr);
            if !is_private(ip) {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned();
            out.push(Subnet { iface: name, addr: ip, prefix: mask.count_ones() as u8 });
        }
        libc::freeifaddrs(list);
    }
    out
}

pub const MAX_HOSTS: usize = 1024;

/// The host addresses of a subnet (without the network and broadcast addresses). A network bigger than a /22 is cut to
/// the /24 around this machine, so one click never means sixty thousand probes.
pub fn hosts_of(s: &Subnet) -> Vec<Ipv4Addr> {
    let prefix = if s.prefix < 22 { 24 } else { s.prefix };
    if prefix >= 31 {
        return vec![s.addr];
    }
    let mask = prefix_mask(prefix);
    let net = u32::from(s.addr) & mask;
    let size = !mask;
    (1..size).map(|i| Ipv4Addr::from(net + i)).filter(|ip| *ip != s.addr).collect()
}

/// What a person typed: `192.168.1.0/24`, `192.168.1.20`, `192.168.1.10-50`, or several separated by spaces or commas.
pub fn parse_targets(text: &str) -> Result<Vec<Ipv4Addr>, String> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for part in text.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()) {
        let ips: Vec<Ipv4Addr> = if let Some((a, p)) = part.split_once('/') {
            let ip: Ipv4Addr = a.parse().map_err(|_| format!("{a} is not an IPv4 address"))?;
            let prefix: u8 = p.parse().map_err(|_| format!("{p} is not a network size"))?;
            if prefix > 32 || prefix < 22 {
                return Err(format!("/{prefix} is too big or too small: use /22 (1022 addresses) to /32"));
            }
            let mask = prefix_mask(prefix);
            let (net, size) = (u32::from(ip) & mask, !mask);
            // every host address of the network that was typed, this machine's own included
            (1..size).map(|i| Ipv4Addr::from(net + i)).collect()
        } else if let Some((a, last)) = part.split_once('-') {
            let ip: Ipv4Addr = a.parse().map_err(|_| format!("{a} is not an IPv4 address"))?;
            let end: u8 = last.parse().map_err(|_| format!("{last} is not the end of a range"))?;
            let start = ip.octets()[3];
            if end < start {
                return Err("the end of the range is before its start".into());
            }
            (start..=end).map(|d| Ipv4Addr::new(ip.octets()[0], ip.octets()[1], ip.octets()[2], d)).collect()
        } else {
            vec![part.parse().map_err(|_| format!("{part} is not an IPv4 address"))?]
        };
        for ip in ips {
            if !allowed(ip) {
                return Err(format!("{ip} is not on the local network: scans stay on private addresses (10.x, 172.16-31.x, 192.168.x, 169.254.x)"));
            }
            if !out.contains(&ip) {
                out.push(ip);
            }
            if out.len() > MAX_HOSTS {
                return Err(format!("more than {MAX_HOSTS} addresses: scan a smaller range"));
            }
        }
    }
    if out.is_empty() {
        return Err("no address to scan".into());
    }
    Ok(out)
}

/// The hardware address of a neighbour this machine has talked to recently (`/proc/net/arp`).
pub fn arp_table() -> Vec<(Ipv4Addr, String)> {
    let text = std::fs::read_to_string("/proc/net/arp").unwrap_or_default();
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let ip: Ipv4Addr = f.first()?.parse().ok()?;
            let mac = f.get(3)?.to_string();
            (mac != "00:00:00:00:00:00" && f.get(2).is_some_and(|flags| *flags != "0x0")).then_some((ip, mac))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_addresses_are_allowed() {
        for ok in ["10.0.0.5", "172.16.3.4", "172.31.255.1", "192.168.1.1", "169.254.1.1", "127.0.0.1"] {
            assert!(allowed(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "1.1.1.1", "100.64.0.1", "192.169.0.1", "224.0.0.1"] {
            assert!(!allowed(bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn targets_parse_ranges_and_networks_and_refuse_the_internet() {
        assert_eq!(parse_targets("192.168.1.20").unwrap(), vec![Ipv4Addr::new(192, 168, 1, 20)]);
        assert_eq!(parse_targets("192.168.1.10-12, 192.168.1.11").unwrap().len(), 3, "duplicates are dropped");
        let net = parse_targets("192.168.1.0/24").unwrap();
        assert_eq!((net.len(), net.first().copied(), net.last().copied()), (254, Some(Ipv4Addr::new(192, 168, 1, 1)), Some(Ipv4Addr::new(192, 168, 1, 254))));
        assert_eq!(parse_targets("192.168.1.77/24").unwrap().len(), 254, "any address of the network names it");
        assert_eq!(parse_targets("10.0.0.0/22").unwrap().len(), 1022);
        assert!(parse_targets("10.0.0.0/16").is_err() && parse_targets("10.0.0.0/8").is_err(), "too big");
        for bad in ["8.8.8.8", "192.168.1.0/24 1.1.1.1", "192.168.1.5-3", "nonsense", "", "192.168.1.300", "192.168.1.1/x"] {
            assert!(parse_targets(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_targets("127.0.0.1").unwrap().len(), 1, "this machine itself");
    }

    #[test]
    fn a_subnet_gives_its_hosts_without_this_machine() {
        let s = Subnet { iface: "eth0".into(), addr: Ipv4Addr::new(192, 168, 1, 7), prefix: 24 };
        let h = hosts_of(&s);
        assert_eq!(h.len(), 253);
        assert!(!h.contains(&s.addr) && h.contains(&Ipv4Addr::new(192, 168, 1, 1)) && !h.contains(&Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(s.cidr(), "192.168.1.0/24");
        // a /16 is cut to the /24 around us
        let big = Subnet { iface: "x".into(), addr: Ipv4Addr::new(10, 1, 2, 3), prefix: 16 };
        assert_eq!(hosts_of(&big).len(), 253);
        assert!(hosts_of(&big).iter().all(|ip| ip.octets()[..3] == [10, 1, 2]));
    }

    #[test]
    fn the_machines_own_networks_are_found_without_panicking() {
        for s in local_subnets() {
            assert!(is_private(s.addr) && s.prefix <= 32, "{s:?}");
        }
        let _ = arp_table();
    }
}
