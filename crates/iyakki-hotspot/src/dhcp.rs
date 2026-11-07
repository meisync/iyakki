//! A small DHCP server (RFC 2131 subset, options RFC 2132) as a state machine over byte buffers: DISCOVER, OFFER, REQUEST, ACK,
//! NAK, DECLINE, RELEASE and INFORM. It owns a lease table over one pool and nothing else: no sockets, no clock (time is passed
//! in as seconds). Malformed packets are ignored, never a panic.
//!
//! Only stations the access point has authenticated (`station_joined`) are served, and each holds at most one address, so a
//! flood of made-up client addresses cannot use up the pool. A station that leaves gives its address back.

use crate::Mac;
use std::collections::HashMap;
use std::net::Ipv4Addr;

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const BOOTP_LEN: usize = 236;
pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const DECLINE: u8 = 4;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;
pub const RELEASE: u8 = 7;
pub const INFORM: u8 = 8;

/// An offer that is not taken up is forgotten after this many seconds.
pub const OFFER_HOLD_SECS: u64 = 30;
/// An address a client declined (someone else uses it) is not offered again for this long.
pub const DECLINE_QUARANTINE_SECS: u64 = 600;
/// A packet bigger than this is not read.
pub const MAX_PACKET: usize = 1500;

#[derive(Debug, Clone)]
pub struct DhcpConfig {
    pub server_ip: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub broadcast: Ipv4Addr,
    pub pool_start: Ipv4Addr,
    pub pool_end: Ipv4Addr,
    pub lease_secs: u32,
    /// The DNS server given to clients (normally the hotspot's own address).
    pub dns: Ipv4Addr,
}

/// Where the reply goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// Limited broadcast, to the client's hardware address (the client has no address yet).
    Broadcast,
    /// The client's own address.
    Unicast(Ipv4Addr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub bytes: Vec<u8>,
    pub dest: Dest,
    pub client_mac: Mac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Offered,
    Leased,
}

#[derive(Debug, Clone)]
struct Lease {
    ip: Ipv4Addr,
    kind: Kind,
    expires: u64,
    hostname: String,
}

/// What a person sees in the clients list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseInfo {
    pub mac: Mac,
    pub ip: Ipv4Addr,
    pub hostname: String,
    pub expires: u64,
}

struct Parsed<'a> {
    op: u8,
    xid: [u8; 4],
    flags: [u8; 2],
    ciaddr: Ipv4Addr,
    giaddr: Ipv4Addr,
    mac: Mac,
    msg_type: u8,
    requested: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
    hostname: Option<&'a [u8]>,
}

fn ip4(b: &[u8]) -> Option<Ipv4Addr> {
    let a: [u8; 4] = b.try_into().ok()?;
    Some(Ipv4Addr::from(a))
}

fn parse(p: &[u8]) -> Option<Parsed<'_>> {
    if p.len() < BOOTP_LEN + 4 || p.len() > MAX_PACKET || p[BOOTP_LEN..BOOTP_LEN + 4] != MAGIC {
        return None;
    }
    if p[1] != 1 || p[2] != 6 {
        return None; // Ethernet hardware addresses only
    }
    let mut r = Parsed {
        op: p[0],
        xid: p[4..8].try_into().ok()?,
        flags: p[10..12].try_into().ok()?,
        ciaddr: ip4(&p[12..16])?,
        giaddr: ip4(&p[24..28])?,
        mac: p[28..34].try_into().ok()?,
        msg_type: 0,
        requested: None,
        server_id: None,
        hostname: None,
    };
    let o = &p[BOOTP_LEN + 4..];
    let mut i = 0;
    while i < o.len() {
        let code = o[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = *o.get(i + 1)? as usize;
        let v = o.get(i + 2..i + 2 + len)?;
        match code {
            53 if len == 1 => r.msg_type = v[0],
            50 if len == 4 => r.requested = ip4(v),
            54 if len == 4 => r.server_id = ip4(v),
            12 => r.hostname = Some(v),
            _ => {}
        }
        i += 2 + len;
    }
    (r.op == 1 && r.msg_type != 0).then_some(r)
}

fn clean_hostname(h: Option<&[u8]>) -> String {
    h.map(|b| b.iter().take(63).filter(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b'.' || **c == b'_').map(|c| *c as char).collect()).unwrap_or_default()
}

pub struct DhcpServer {
    cfg: DhcpConfig,
    leases: HashMap<Mac, Lease>,
    allowed: std::collections::HashSet<Mac>,
    /// Wired sharing: there is no radio handshake to vouch for a station, so every hardware address on the cable may ask (the pool is still bounded).
    open: bool,
    bad: HashMap<Ipv4Addr, u64>,
}

impl DhcpServer {
    pub fn new(cfg: DhcpConfig) -> DhcpServer {
        DhcpServer { cfg, leases: HashMap::new(), allowed: Default::default(), open: false, bad: HashMap::new() }
    }

    pub fn pool_size(&self) -> u32 {
        u32::from(self.cfg.pool_end).saturating_sub(u32::from(self.cfg.pool_start)) + 1
    }

    /// Serve every station on the link (wired and USB sharing), not only those the access point authenticated.
    pub fn allow_any_station(&mut self) {
        self.open = true;
    }

    /// The access point finished authenticating this station: it may now be given an address.
    pub fn station_joined(&mut self, mac: Mac) {
        self.allowed.insert(mac);
    }

    /// The station left (or was removed): its address is free again.
    pub fn station_left(&mut self, mac: &Mac) {
        self.allowed.remove(mac);
        self.leases.remove(mac);
    }

    /// The leases in force at `now`, for the clients list.
    pub fn leases(&self, now: u64) -> Vec<LeaseInfo> {
        let mut v: Vec<LeaseInfo> = self.leases.iter().filter(|(_, l)| l.kind == Kind::Leased && l.expires > now).map(|(m, l)| LeaseInfo { mac: *m, ip: l.ip, hostname: l.hostname.clone(), expires: l.expires }).collect();
        v.sort_by_key(|l| u32::from(l.ip));
        v
    }

    fn expire(&mut self, now: u64) {
        self.leases.retain(|_, l| l.expires > now);
        self.bad.retain(|_, until| *until > now);
    }

    fn in_pool(&self, ip: Ipv4Addr) -> bool {
        ip >= self.cfg.pool_start && ip <= self.cfg.pool_end
    }

    fn taken_by_other(&self, ip: Ipv4Addr, mac: &Mac) -> bool {
        self.bad.contains_key(&ip) || self.leases.iter().any(|(m, l)| l.ip == ip && m != mac)
    }

    fn pick(&self, mac: &Mac, wanted: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
        if let Some(l) = self.leases.get(mac) {
            return Some(l.ip);
        }
        if let Some(w) = wanted.filter(|w| self.in_pool(*w) && !self.taken_by_other(*w, mac)) {
            return Some(w);
        }
        let size = self.pool_size();
        let start = mac.iter().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(*b as u32)) % size.max(1);
        (0..size).map(|k| Ipv4Addr::from(u32::from(self.cfg.pool_start) + (start + k) % size)).find(|ip| !self.taken_by_other(*ip, mac))
    }

    /// Handle one packet from a client at `now` (seconds). `None` means: say nothing.
    pub fn handle(&mut self, packet: &[u8], now: u64) -> Option<Reply> {
        let p = parse(packet)?;
        self.expire(now);
        if p.giaddr != Ipv4Addr::UNSPECIFIED || (!self.open && !self.allowed.contains(&p.mac)) {
            return None;
        }
        match p.msg_type {
            DISCOVER => {
                let ip = self.pick(&p.mac, p.requested)?;
                let host = clean_hostname(p.hostname);
                let keep = self.leases.get(&p.mac).filter(|l| l.kind == Kind::Leased).map(|l| (l.kind, l.expires));
                let (kind, expires) = keep.unwrap_or((Kind::Offered, now + OFFER_HOLD_SECS));
                self.leases.insert(p.mac, Lease { ip, kind, expires, hostname: host });
                Some(self.reply(&p, OFFER, ip, true))
            }
            REQUEST => {
                if p.server_id.is_some_and(|s| s != self.cfg.server_ip) {
                    // the client chose another server: forget our offer
                    if self.leases.get(&p.mac).is_some_and(|l| l.kind == Kind::Offered) {
                        self.leases.remove(&p.mac);
                    }
                    return None;
                }
                let renewing = p.ciaddr != Ipv4Addr::UNSPECIFIED;
                let wanted = if renewing { Some(p.ciaddr) } else { p.requested };
                let ok = match (wanted, self.leases.get(&p.mac)) {
                    (Some(w), Some(l)) => l.ip == w,
                    // INIT-REBOOT for an address we know nothing about: say no if it is ours to say, else stay silent
                    (Some(w), None) if p.server_id.is_none() && !renewing => {
                        if !self.in_pool(w) || self.taken_by_other(w, &p.mac) {
                            return Some(self.nak(&p));
                        }
                        // free and in our pool: grant it
                        self.leases.insert(p.mac, Lease { ip: w, kind: Kind::Offered, expires: now, hostname: String::new() });
                        true
                    }
                    _ => false,
                };
                if !ok {
                    return Some(self.nak(&p));
                }
                let ip = wanted?;
                let host = clean_hostname(p.hostname);
                let host = if host.is_empty() { self.leases.get(&p.mac).map(|l| l.hostname.clone()).unwrap_or_default() } else { host };
                self.leases.insert(p.mac, Lease { ip, kind: Kind::Leased, expires: now + self.cfg.lease_secs as u64, hostname: host });
                Some(self.reply(&p, ACK, ip, !renewing))
            }
            DECLINE => {
                if let Some(l) = self.leases.remove(&p.mac) {
                    self.bad.insert(l.ip, now + DECLINE_QUARANTINE_SECS);
                }
                None
            }
            RELEASE => {
                if self.leases.get(&p.mac).is_some_and(|l| l.ip == p.ciaddr) {
                    self.leases.remove(&p.mac);
                }
                None
            }
            INFORM => {
                let mut r = self.build(&p, ACK, Ipv4Addr::UNSPECIFIED, false);
                r.dest = if p.ciaddr == Ipv4Addr::UNSPECIFIED { Dest::Broadcast } else { Dest::Unicast(p.ciaddr) };
                Some(r)
            }
            _ => None,
        }
    }

    fn reply(&self, p: &Parsed<'_>, t: u8, ip: Ipv4Addr, broadcast: bool) -> Reply {
        let mut r = self.build(p, t, ip, true);
        r.dest = if broadcast || p.ciaddr == Ipv4Addr::UNSPECIFIED { Dest::Broadcast } else { Dest::Unicast(p.ciaddr) };
        r
    }

    fn nak(&self, p: &Parsed<'_>) -> Reply {
        let mut r = self.build(p, NAK, Ipv4Addr::UNSPECIFIED, false);
        r.dest = Dest::Broadcast;
        r
    }

    fn build(&self, p: &Parsed<'_>, t: u8, yiaddr: Ipv4Addr, with_lease: bool) -> Reply {
        let mut b = vec![0u8; BOOTP_LEN];
        b[0] = 2;
        b[1] = 1;
        b[2] = 6;
        b[4..8].copy_from_slice(&p.xid);
        b[10..12].copy_from_slice(&p.flags);
        if t == ACK && !with_lease {
            b[12..16].copy_from_slice(&p.ciaddr.octets()); // INFORM answers echo ciaddr
        }
        b[16..20].copy_from_slice(&yiaddr.octets());
        b[20..24].copy_from_slice(&self.cfg.server_ip.octets());
        b[28..34].copy_from_slice(&p.mac);
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&[53, 1, t, 54, 4]);
        b.extend_from_slice(&self.cfg.server_ip.octets());
        if t != NAK {
            if with_lease {
                let l = self.cfg.lease_secs;
                b.extend_from_slice(&[51, 4]);
                b.extend_from_slice(&l.to_be_bytes());
                b.extend_from_slice(&[58, 4]);
                b.extend_from_slice(&(l / 2).to_be_bytes());
                b.extend_from_slice(&[59, 4]);
                b.extend_from_slice(&(l / 8 * 7).to_be_bytes());
            }
            b.extend_from_slice(&[1, 4]);
            b.extend_from_slice(&self.cfg.mask.octets());
            b.extend_from_slice(&[3, 4]);
            b.extend_from_slice(&self.cfg.server_ip.octets());
            b.extend_from_slice(&[6, 4]);
            b.extend_from_slice(&self.cfg.dns.octets());
            b.extend_from_slice(&[28, 4]);
            b.extend_from_slice(&self.cfg.broadcast.octets());
        }
        b.push(255);
        while b.len() < 300 {
            b.push(0); // old clients want a packet of at least 300 bytes
        }
        Reply { bytes: b, dest: Dest::Broadcast, client_mac: p.mac }
    }
}

/// A client packet for the tests and for tools: build a request of the given type.
pub fn client_packet(t: u8, mac: Mac, xid: u32, ciaddr: Ipv4Addr, requested: Option<Ipv4Addr>, server_id: Option<Ipv4Addr>, hostname: Option<&str>) -> Vec<u8> {
    let mut b = vec![0u8; BOOTP_LEN];
    b[0] = 1;
    b[1] = 1;
    b[2] = 6;
    b[4..8].copy_from_slice(&xid.to_be_bytes());
    b[12..16].copy_from_slice(&ciaddr.octets());
    b[28..34].copy_from_slice(&mac);
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&[53, 1, t]);
    if let Some(r) = requested {
        b.extend_from_slice(&[50, 4]);
        b.extend_from_slice(&r.octets());
    }
    if let Some(s) = server_id {
        b.extend_from_slice(&[54, 4]);
        b.extend_from_slice(&s.octets());
    }
    if let Some(h) = hostname {
        b.extend_from_slice(&[12, h.len() as u8]);
        b.extend_from_slice(h.as_bytes());
    }
    b.push(255);
    b
}

/// Read the message type and the offered address out of a server reply (tests and the clients list).
pub fn reply_summary(b: &[u8]) -> Option<(u8, Ipv4Addr)> {
    let p = b.get(..BOOTP_LEN + 4)?;
    if p[0] != 2 || p[BOOTP_LEN..] != MAGIC {
        return None;
    }
    let o = &b[BOOTP_LEN + 4..];
    let mut i = 0;
    while i + 2 < o.len() {
        if o[i] == 255 {
            break;
        }
        if o[i] == 53 {
            return Some((*o.get(i + 2)?, ip4(&b[16..20])?));
        }
        i += 2 + o[i + 1] as usize;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRV: Ipv4Addr = Ipv4Addr::new(10, 42, 0, 1);

    fn server(pool: u8) -> DhcpServer {
        DhcpServer::new(DhcpConfig { server_ip: SRV, mask: Ipv4Addr::new(255, 255, 255, 0), broadcast: Ipv4Addr::new(10, 42, 0, 255), pool_start: Ipv4Addr::new(10, 42, 0, 10), pool_end: Ipv4Addr::new(10, 42, 0, 10 + pool - 1), lease_secs: 3600, dns: SRV })
    }

    fn mac(n: u8) -> Mac {
        [2, 0, 0, 0, 0, n]
    }

    fn join(s: &mut DhcpServer, n: u8) {
        s.station_joined(mac(n));
    }

    fn discover(s: &mut DhcpServer, n: u8, now: u64) -> Option<Reply> {
        s.handle(&client_packet(DISCOVER, mac(n), n as u32, Ipv4Addr::UNSPECIFIED, None, None, Some("phone")), now)
    }

    fn request(s: &mut DhcpServer, n: u8, ip: Ipv4Addr, now: u64) -> Option<Reply> {
        s.handle(&client_packet(REQUEST, mac(n), n as u32, Ipv4Addr::UNSPECIFIED, Some(ip), Some(SRV), None), now)
    }

    fn opt(b: &[u8], code: u8) -> Option<Vec<u8>> {
        let o = &b[BOOTP_LEN + 4..];
        let mut i = 0;
        while i + 1 < o.len() && o[i] != 255 {
            if o[i] == code {
                return Some(o[i + 2..i + 2 + o[i + 1] as usize].to_vec());
            }
            i += 2 + o[i + 1] as usize;
        }
        None
    }

    #[test]
    fn wired_sharing_serves_any_station_but_still_has_a_bounded_pool() {
        let mut s = server(2);
        assert!(discover(&mut s, 1, 100).is_none(), "strangers are ignored until the server is opened");
        s.allow_any_station();
        for n in 1..=2 {
            let o = discover(&mut s, n, 100).unwrap();
            let (_, ip) = reply_summary(&o.bytes).unwrap();
            assert_eq!(reply_summary(&request(&mut s, n, ip, 101).unwrap().bytes), Some((ACK, ip)));
        }
        assert!(discover(&mut s, 3, 102).is_none(), "a third device finds the pool empty");
    }

    #[test]
    fn discover_offer_request_ack_gives_the_first_address_with_the_options() {
        let mut s = server(8);
        join(&mut s, 1);
        let o = discover(&mut s, 1, 100).unwrap();
        let (t, ip) = reply_summary(&o.bytes).unwrap();
        assert_eq!(t, OFFER);
        assert!(ip >= Ipv4Addr::new(10, 42, 0, 10) && ip <= Ipv4Addr::new(10, 42, 0, 17));
        assert_eq!(o.dest, Dest::Broadcast);
        assert_eq!(&o.bytes[4..8], &1u32.to_be_bytes(), "the transaction id is echoed");
        assert_eq!(opt(&o.bytes, 54).unwrap(), SRV.octets());
        assert_eq!(opt(&o.bytes, 1).unwrap(), [255, 255, 255, 0]);
        assert_eq!(opt(&o.bytes, 3).unwrap(), SRV.octets());
        assert_eq!(opt(&o.bytes, 6).unwrap(), SRV.octets());
        assert_eq!(opt(&o.bytes, 51).unwrap(), 3600u32.to_be_bytes());
        assert!(o.bytes.len() >= 300);
        let a = request(&mut s, 1, ip, 101).unwrap();
        assert_eq!(reply_summary(&a.bytes), Some((ACK, ip)));
        let l = s.leases(102);
        assert_eq!((l.len(), l[0].ip, l[0].hostname.as_str()), (1, ip, "phone"));
        // asking again gets the same address
        let again = discover(&mut s, 1, 200).unwrap();
        assert_eq!(reply_summary(&again.bytes), Some((OFFER, ip)));
    }

    #[test]
    fn a_requested_address_in_the_pool_is_honoured_and_one_outside_is_not() {
        let mut s = server(8);
        join(&mut s, 1);
        let p = client_packet(DISCOVER, mac(1), 5, Ipv4Addr::UNSPECIFIED, Some(Ipv4Addr::new(10, 42, 0, 14)), None, None);
        assert_eq!(reply_summary(&s.handle(&p, 0).unwrap().bytes).unwrap().1, Ipv4Addr::new(10, 42, 0, 14));
        join(&mut s, 2);
        let p = client_packet(DISCOVER, mac(2), 6, Ipv4Addr::UNSPECIFIED, Some(Ipv4Addr::new(10, 42, 0, 200)), None, None);
        let ip = reply_summary(&s.handle(&p, 0).unwrap().bytes).unwrap().1;
        assert_ne!(ip, Ipv4Addr::new(10, 42, 0, 200));
        let p = client_packet(DISCOVER, mac(2), 7, Ipv4Addr::UNSPECIFIED, Some(Ipv4Addr::new(10, 42, 0, 14)), None, None);
        assert_ne!(reply_summary(&s.handle(&p, 0).unwrap().bytes).unwrap().1, Ipv4Addr::new(10, 42, 0, 14), "taken by the other station");
    }

    #[test]
    fn two_stations_never_get_the_same_address_and_a_full_pool_stays_silent() {
        let mut s = server(4);
        let mut got = std::collections::BTreeSet::new();
        for n in 1..=4 {
            join(&mut s, n);
            let ip = reply_summary(&discover(&mut s, n, 0).unwrap().bytes).unwrap().1;
            assert!(got.insert(ip), "{ip} given twice");
            request(&mut s, n, ip, 1).unwrap();
        }
        join(&mut s, 5);
        assert!(discover(&mut s, 5, 2).is_none());
        // one leaves, the fifth gets its address
        let freed = s.leases(2)[0].ip;
        let m = s.leases(2).iter().find(|l| l.ip == freed).unwrap().mac;
        s.station_left(&m);
        assert_eq!(reply_summary(&discover(&mut s, 5, 3).unwrap().bytes).unwrap().1, freed);
    }

    #[test]
    fn unconfirmed_offers_expire_and_a_station_holds_only_one_address() {
        let mut s = server(2);
        join(&mut s, 1);
        join(&mut s, 2);
        let a = reply_summary(&discover(&mut s, 1, 0).unwrap().bytes).unwrap().1;
        for xid in 0..20u32 {
            // the same station discovering over and over does not use more addresses
            let p = client_packet(DISCOVER, mac(1), xid, Ipv4Addr::UNSPECIFIED, None, None, None);
            assert_eq!(reply_summary(&s.handle(&p, 1).unwrap().bytes).unwrap().1, a);
        }
        assert!(discover(&mut s, 2, 2).is_some(), "the second address is still free");
        // nobody confirmed: after the hold time everything is free again
        join(&mut s, 3);
        assert!(discover(&mut s, 3, 3).is_none());
        assert!(discover(&mut s, 3, OFFER_HOLD_SECS + 5).is_some());
    }

    #[test]
    fn a_stranger_who_is_not_an_authenticated_station_is_not_served() {
        let mut s = server(8);
        assert!(discover(&mut s, 9, 0).is_none());
        join(&mut s, 9);
        assert!(discover(&mut s, 9, 0).is_some());
        s.station_left(&mac(9));
        assert!(discover(&mut s, 9, 1).is_none());
        // relayed packets are ignored
        join(&mut s, 9);
        let mut p = client_packet(DISCOVER, mac(9), 1, Ipv4Addr::UNSPECIFIED, None, None, None);
        p[24..28].copy_from_slice(&[10, 0, 0, 9]);
        assert!(s.handle(&p, 2).is_none());
    }

    #[test]
    fn wrong_requests_get_a_nak_and_requests_for_another_server_are_ignored() {
        let mut s = server(8);
        join(&mut s, 1);
        let ip = reply_summary(&discover(&mut s, 1, 0).unwrap().bytes).unwrap().1;
        // selecting an address we did not offer
        let bad = request(&mut s, 1, Ipv4Addr::new(10, 42, 0, 99), 1).unwrap();
        assert_eq!(reply_summary(&bad.bytes).unwrap().0, NAK);
        assert_eq!(bad.dest, Dest::Broadcast);
        assert_eq!(reply_summary(&request(&mut s, 1, ip, 2).unwrap().bytes).unwrap().0, ACK);
        // the client picked a different server
        let other = client_packet(REQUEST, mac(1), 9, Ipv4Addr::UNSPECIFIED, Some(ip), Some(Ipv4Addr::new(1, 2, 3, 4)), None);
        assert!(s.handle(&other, 3).is_none());
        // INIT-REBOOT with an address from elsewhere gets a NAK; a free pool address is granted
        join(&mut s, 2);
        let reboot = client_packet(REQUEST, mac(2), 3, Ipv4Addr::UNSPECIFIED, Some(Ipv4Addr::new(192, 168, 1, 5)), None, None);
        assert_eq!(reply_summary(&s.handle(&reboot, 4).unwrap().bytes).unwrap().0, NAK);
        let reboot = client_packet(REQUEST, mac(2), 3, Ipv4Addr::UNSPECIFIED, Some(Ipv4Addr::new(10, 42, 0, 16)), None, None);
        assert_eq!(reply_summary(&s.handle(&reboot, 5).unwrap().bytes), Some((ACK, Ipv4Addr::new(10, 42, 0, 16))));
        // INIT-REBOOT for the address of someone else
        join(&mut s, 3);
        let steal = client_packet(REQUEST, mac(3), 3, Ipv4Addr::UNSPECIFIED, Some(ip), None, None);
        assert_eq!(reply_summary(&s.handle(&steal, 6).unwrap().bytes).unwrap().0, NAK);
    }

    #[test]
    fn renewing_goes_unicast_and_extends_the_lease_and_release_and_expiry_free_the_address() {
        let mut s = server(8);
        join(&mut s, 1);
        let ip = reply_summary(&discover(&mut s, 1, 0).unwrap().bytes).unwrap().1;
        request(&mut s, 1, ip, 1).unwrap();
        let renew = client_packet(REQUEST, mac(1), 4, ip, None, None, None);
        let r = s.handle(&renew, 1800).unwrap();
        assert_eq!((reply_summary(&r.bytes), r.dest), (Some((ACK, ip)), Dest::Unicast(ip)));
        assert_eq!(s.leases(1801)[0].expires, 1800 + 3600);
        let rel = client_packet(RELEASE, mac(1), 5, ip, None, None, None);
        assert!(s.handle(&rel, 1900).is_none());
        assert!(s.leases(1901).is_empty());
        // expiry
        assert_eq!(reply_summary(&request(&mut s, 1, ip, 2000).unwrap().bytes).unwrap().0, NAK, "the lease is gone");
        let _ = discover(&mut s, 1, 2000);
        request(&mut s, 1, ip, 2001);
        assert_eq!(s.leases(2002).len(), 1);
        assert!(s.leases(2001 + 3601).is_empty());
    }

    #[test]
    fn a_declined_address_is_not_offered_again_for_a_while() {
        let mut s = server(2);
        join(&mut s, 1);
        let ip = reply_summary(&discover(&mut s, 1, 0).unwrap().bytes).unwrap().1;
        request(&mut s, 1, ip, 1).unwrap();
        let dec = client_packet(DECLINE, mac(1), 6, Ipv4Addr::UNSPECIFIED, Some(ip), Some(SRV), None);
        assert!(s.handle(&dec, 2).is_none());
        let next = reply_summary(&discover(&mut s, 1, 3).unwrap().bytes).unwrap().1;
        assert_ne!(next, ip);
        // after the quarantine it can come back
        s.station_left(&mac(1));
        join(&mut s, 1);
        let late = DECLINE_QUARANTINE_SECS + 10;
        let again: Vec<Ipv4Addr> = (0..1).map(|_| reply_summary(&discover(&mut s, 1, late).unwrap().bytes).unwrap().1).collect();
        assert!(again[0] >= Ipv4Addr::new(10, 42, 0, 10));
    }

    #[test]
    fn inform_gets_the_options_without_an_address_or_a_lease_time() {
        let mut s = server(8);
        join(&mut s, 1);
        let p = client_packet(INFORM, mac(1), 7, Ipv4Addr::new(10, 42, 0, 12), None, None, None);
        let r = s.handle(&p, 0).unwrap();
        assert_eq!(reply_summary(&r.bytes), Some((ACK, Ipv4Addr::UNSPECIFIED)));
        assert_eq!(r.dest, Dest::Unicast(Ipv4Addr::new(10, 42, 0, 12)));
        assert!(opt(&r.bytes, 51).is_none());
        assert_eq!(opt(&r.bytes, 6).unwrap(), SRV.octets());
        assert!(s.leases(1).is_empty(), "no lease is made for an inform");
    }

    #[test]
    fn a_hostname_with_odd_characters_is_cleaned_before_it_is_shown() {
        let mut s = server(8);
        join(&mut s, 1);
        let p = client_packet(DISCOVER, mac(1), 1, Ipv4Addr::UNSPECIFIED, None, None, Some("<b>x</b>\u{7}ok"));
        s.handle(&p, 0).unwrap();
        let ip = s.leases.get(&mac(1)).unwrap().ip;
        request(&mut s, 1, ip, 1).unwrap();
        assert_eq!(s.leases(2)[0].hostname, "bxbok");
    }

    #[test]
    fn malformed_packets_are_ignored_and_random_bytes_never_panic() {
        let mut s = server(8);
        join(&mut s, 1);
        let good = client_packet(DISCOVER, mac(1), 1, Ipv4Addr::UNSPECIFIED, None, None, None);
        assert!(s.handle(&[], 0).is_none());
        assert!(s.handle(&good[..100], 0).is_none());
        assert!(s.handle(&good[..BOOTP_LEN + 2], 0).is_none());
        let mut x = good.clone();
        x[BOOTP_LEN] = 0; // no magic cookie
        assert!(s.handle(&x, 0).is_none());
        let mut x = good.clone();
        x[0] = 2; // a reply, not a request
        assert!(s.handle(&x, 0).is_none());
        let mut x = good.clone();
        x[2] = 8; // not 6-byte hardware addresses
        assert!(s.handle(&x, 0).is_none());
        let mut x = good.clone();
        x.truncate(BOOTP_LEN + 4);
        x.extend_from_slice(&[53, 9, 1]); // option longer than the packet
        assert!(s.handle(&x, 0).is_none());
        assert!(s.handle(&vec![1u8; 4000], 0).is_none());
        let mut seed = 7u32;
        for n in 0..5000usize {
            let mut v = good.clone();
            for _ in 0..(1 + n % 6) {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let at = (seed >> 8) as usize % v.len();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                v[at] = (seed >> 24) as u8;
            }
            if n % 7 == 0 {
                v.truncate((seed >> 4) as usize % v.len());
            }
            let _ = s.handle(&v, n as u64);
            let pure: Vec<u8> = (0..(n % 400)).map(|i| (seed.wrapping_add(i as u32).wrapping_mul(2654435761) >> 13) as u8).collect();
            let _ = s.handle(&pure, n as u64);
        }
        assert!(s.leases(10).len() <= 8);
    }
}
