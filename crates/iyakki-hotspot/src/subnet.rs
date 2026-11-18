//! Choosing the hotspot's private network: a /24 that overlaps nothing the machine already uses (the uplink, a VPN, containers).

use std::net::Ipv4Addr;

/// An IPv4 network: address and prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl Cidr {
    pub fn new(addr: Ipv4Addr, prefix: u8) -> Option<Cidr> {
        (prefix <= 32).then_some(Cidr { addr, prefix })
    }

    /// Parse `a.b.c.d/n`.
    pub fn parse(s: &str) -> Option<Cidr> {
        let (a, p) = s.trim().split_once('/')?;
        Cidr::new(a.parse().ok()?, p.parse().ok()?)
    }

    fn mask(&self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix as u32)
        }
    }

    pub fn network(&self) -> u32 {
        u32::from(self.addr) & self.mask()
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & self.mask() == self.network()
    }

    /// Do two networks share any address?
    pub fn overlaps(&self, other: &Cidr) -> bool {
        let m = self.mask() & other.mask();
        self.network() & m == other.network() & m
    }
}

/// The hotspot network: a /24, the machine is `.1`, clients get `.10` upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotspotNet {
    pub net: Cidr,
}

impl HotspotNet {
    pub const MASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);

    fn host(&self, n: u8) -> Ipv4Addr {
        Ipv4Addr::from((self.net.network() & 0xffff_ff00) | n as u32)
    }

    /// The machine's own address on the hotspot (gateway, DHCP and DNS server).
    pub fn gateway(&self) -> Ipv4Addr {
        self.host(1)
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        self.host(255)
    }

    /// First address handed out.
    pub fn pool_start(&self) -> Ipv4Addr {
        self.host(10)
    }

    /// Last address handed out (`.10` plus `size - 1`, at most `.250`).
    pub fn pool_end(&self, size: u8) -> Ipv4Addr {
        self.host(10u8.saturating_add(size.max(1) - 1).min(250))
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        self.net.contains(ip)
    }
}

/// The networks tried first: the ones other phones and routers use for a hotspot, so people recognise them.
const PREFERRED: [[u8; 3]; 5] = [[10, 42, 0], [192, 168, 43], [192, 168, 137], [10, 43, 0], [172, 20, 10]];

/// A /24 that overlaps none of `avoid`. Tries the usual hotspot networks, then `10.64.0.0` to `10.127.255.0`. `None` only when
/// everything is taken, which is not realistic.
pub fn choose_subnet(avoid: &[Cidr]) -> Option<HotspotNet> {
    let free = |c: &Cidr| !avoid.iter().any(|a| a.overlaps(c));
    for p in PREFERRED {
        let c = Cidr { addr: Ipv4Addr::new(p[0], p[1], p[2], 0), prefix: 24 };
        if free(&c) {
            return Some(HotspotNet { net: c });
        }
    }
    for b in 64u8..128 {
        for c3 in 0u8..=255 {
            let c = Cidr { addr: Ipv4Addr::new(10, b, c3, 0), prefix: 24 };
            if free(&c) {
                return Some(HotspotNet { net: c });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        Cidr::parse(s).unwrap()
    }

    #[test]
    fn cidr_parsing_membership_and_overlap() {
        assert_eq!(Cidr::parse("10.0.0.5/24").unwrap().network(), u32::from(Ipv4Addr::new(10, 0, 0, 0)));
        assert!(Cidr::parse("10.0.0.0/33").is_none());
        assert!(Cidr::parse("nonsense").is_none());
        assert!(Cidr::parse("10.0.0.0").is_none());
        assert!(c("10.0.0.0/8").contains(Ipv4Addr::new(10, 200, 1, 1)));
        assert!(!c("10.0.0.0/24").contains(Ipv4Addr::new(10, 0, 1, 1)));
        assert!(c("10.0.0.0/8").overlaps(&c("10.42.0.0/24")));
        assert!(c("10.42.0.0/24").overlaps(&c("10.0.0.0/8")), "overlap is symmetric");
        assert!(!c("10.42.0.0/24").overlaps(&c("10.43.0.0/24")));
        assert!(c("0.0.0.0/0").overlaps(&c("192.168.1.0/24")));
        assert!(c("192.168.1.7/32").overlaps(&c("192.168.1.0/24")));
    }

    #[test]
    fn with_nothing_in_the_way_the_first_usual_network_is_chosen() {
        let n = choose_subnet(&[]).unwrap();
        assert_eq!(n.gateway(), Ipv4Addr::new(10, 42, 0, 1));
        assert_eq!(n.broadcast(), Ipv4Addr::new(10, 42, 0, 255));
        assert_eq!(n.pool_start(), Ipv4Addr::new(10, 42, 0, 10));
        assert_eq!(n.pool_end(8), Ipv4Addr::new(10, 42, 0, 17));
        assert_eq!(n.pool_end(255), Ipv4Addr::new(10, 42, 0, 250));
    }

    #[test]
    fn the_uplink_a_vpn_and_a_container_network_are_all_avoided() {
        let avoid = [c("10.42.0.0/16"), c("192.168.43.0/24"), c("192.168.137.0/24"), c("10.43.0.0/16"), c("172.16.0.0/12")];
        let n = choose_subnet(&avoid).unwrap();
        assert_eq!(n.net.addr, Ipv4Addr::new(10, 64, 0, 0));
        assert!(avoid.iter().all(|a| !a.overlaps(&n.net)));
        // a big 10/8 corporate network leaves the 192.168 candidates
        let n = choose_subnet(&[c("10.0.0.0/8")]).unwrap();
        assert_eq!(n.net.addr, Ipv4Addr::new(192, 168, 43, 0));
        // 10/8 and 192.168/16 and 172.16/12 all in use: nothing left
        assert!(choose_subnet(&[c("10.0.0.0/8"), c("192.168.0.0/16"), c("172.16.0.0/12")]).is_none());
        assert!(choose_subnet(&[c("0.0.0.0/0")]).is_none());
    }
}
