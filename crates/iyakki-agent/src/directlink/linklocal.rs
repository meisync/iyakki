//! IPv4 link-local addresses (RFC 3927) for a cable with no DHCP server: a candidate address in 169.254.1.0 to 169.254.254.255 is
//! made from the interface's MAC address (so a computer picks the same one again), probed with ARP before use, and another one is
//! tried when somebody else has it. Also the fixed private pair (10.77.0.1 and 10.77.0.2) the two ends can agree on without talking:
//! the lower MAC address gets the lower number. The ARP frames are built and read here; the raw socket is in [`super::real`].

use std::net::Ipv4Addr;

use super::Env;

/// ARP probes sent before an address is taken (RFC 3927 PROBE_NUM).
pub const PROBE_NUM: u32 = 3;
/// How many candidates are tried before giving up for this round (RFC 3927 MAX_CONFLICTS).
pub const MAX_ATTEMPTS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Free,
    /// Somebody else (this MAC) has the address or is probing for it.
    InUse([u8; 6]),
    /// The probe could not be sent (no socket, link went away).
    Failed,
}

pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let mut n = 0;
    for part in s.trim().split(':') {
        if n >= 6 || part.len() != 2 {
            return None;
        }
        out[n] = u8::from_str_radix(part, 16).ok()?;
        n += 1;
    }
    (n == 6).then_some(out)
}

pub fn mac_text(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// The `attempt`-th candidate for this MAC: the same MAC and attempt always give the same address, different attempts differ.
pub fn candidate(mac: &[u8; 6], attempt: u32) -> Ipv4Addr {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in mac.iter().copied().chain(attempt.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    // 254 third octets (1..=254) x 256 fourth octets, as RFC 3927 reserves the first and last 256 addresses
    let v = (h >> 16) % (254 * 256);
    Ipv4Addr::new(169, 254, 1 + (v / 256) as u8, (v % 256) as u8)
}

pub fn is_link_local(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 169 && ip.octets()[1] == 254
}

/// Pick an address nobody answers for. Err when every candidate was taken or the probe could not run.
pub fn acquire(env: &dyn Env, name: &str, mac: [u8; 6], max_attempts: u32) -> Result<Ipv4Addr, String> {
    let mut taken = 0;
    for attempt in 0..max_attempts.clamp(1, 64) {
        let ip = candidate(&mac, attempt);
        match env.arp_probe(name, mac, ip) {
            Probe::Free => return Ok(ip),
            Probe::InUse(_) => taken += 1,
            Probe::Failed => return Err(format!("could not probe for a free address on {name}")),
        }
    }
    Err(format!("{taken} link-local addresses were all taken on {name}"))
}

/// The private pair both ends can work out alone: (mine, the other one's). The lower MAC address gets 10.77.0.1. `None` when the
/// two MAC addresses are equal (a loop or a bad driver: nothing can be agreed).
pub fn private_pair(mine: &[u8; 6], peer: &[u8; 6]) -> Option<(Ipv4Addr, Ipv4Addr)> {
    use std::cmp::Ordering::*;
    let (a, b) = (Ipv4Addr::new(10, 77, 0, 1), Ipv4Addr::new(10, 77, 0, 2));
    match mine.cmp(peer) {
        Less => Some((a, b)),
        Greater => Some((b, a)),
        Equal => None,
    }
}

// ---- ARP frames ---------------------------------------------------------------------------------------------------

/// An ARP packet (Ethernet and IPv4 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arp {
    pub op: u16,
    pub sha: [u8; 6],
    pub spa: Ipv4Addr,
    pub tha: [u8; 6],
    pub tpa: Ipv4Addr,
}

/// A whole Ethernet frame (42 bytes) of an ARP request. A probe has sender address 0.0.0.0; an announcement has the address twice.
pub fn arp_frame(src: &[u8; 6], spa: Ipv4Addr, tpa: Ipv4Addr) -> [u8; 42] {
    let mut f = [0u8; 42];
    f[0..6].copy_from_slice(&[0xff; 6]);
    f[6..12].copy_from_slice(src);
    f[12..14].copy_from_slice(&[0x08, 0x06]);
    f[14..16].copy_from_slice(&[0, 1]); // Ethernet
    f[16..18].copy_from_slice(&[0x08, 0x00]); // IPv4
    f[18] = 6;
    f[19] = 4;
    f[20..22].copy_from_slice(&[0, 1]); // request
    f[22..28].copy_from_slice(src);
    f[28..32].copy_from_slice(&spa.octets());
    // target hardware address stays zero
    f[38..42].copy_from_slice(&tpa.octets());
    f
}

pub fn probe_frame(src: &[u8; 6], target: Ipv4Addr) -> [u8; 42] {
    arp_frame(src, Ipv4Addr::UNSPECIFIED, target)
}

/// Read an Ethernet frame as ARP; `None` for anything else or anything too short.
pub fn parse_arp(f: &[u8]) -> Option<Arp> {
    if f.len() < 42 || f[12..14] != [0x08, 0x06] || f[14..16] != [0, 1] || f[16..18] != [0x08, 0x00] || f[18] != 6 || f[19] != 4 {
        return None;
    }
    let mac = |o: usize| -> [u8; 6] { f[o..o + 6].try_into().unwrap_or([0; 6]) };
    let ip = |o: usize| Ipv4Addr::new(f[o], f[o + 1], f[o + 2], f[o + 3]);
    Some(Arp { op: u16::from_be_bytes([f[20], f[21]]), sha: mac(22), spa: ip(28), tha: mac(32), tpa: ip(38) })
}

/// Does this packet, seen while we probe for `ip`, mean the address is taken? Somebody uses it (sender address is `ip`), or somebody
/// else probes for it at the same moment (sender 0.0.0.0, target `ip`, another MAC). Our own packets never count.
pub fn conflicts(p: &Arp, our_mac: &[u8; 6], ip: Ipv4Addr) -> Option<[u8; 6]> {
    if p.sha == *our_mac {
        return None;
    }
    if p.spa == ip || (p.spa.is_unspecified() && p.tpa == ip && p.op == 1) {
        Some(p.sha)
    } else {
        None
    }
}
