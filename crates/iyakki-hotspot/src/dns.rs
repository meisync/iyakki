//! The packet handling of the hotspot's DNS forwarder (UDP only, nothing cached). The socket code lives elsewhere; this module
//! decides, for each packet, what to do:
//!
//! * a query from a client inside the hotspot subnet is checked, then either answered here (the machine's own name), refused
//!   with a short error, dropped, or forwarded to the uplink's resolver under a fresh random id;
//! * an answer from the resolver is matched to the pending query by id **and** by the question it repeats, and goes back to the
//!   client with the client's own id.
//!
//! It never panics on junk, never reads past a length it has not checked, drops queries over 512 bytes, forwards nothing from
//! outside the subnet, and keeps a bounded table of pending queries.

use crate::subnet::Cidr;
use std::net::Ipv4Addr;

/// Largest query read (the classic UDP limit; a query is small).
pub const MAX_QUERY: usize = 512;
/// Largest answer passed back to a client.
pub const MAX_RESPONSE: usize = 4096;
/// A pending query is forgotten after this long.
pub const PENDING_MS: u64 = 5000;

const RCODE_FORMERR: u8 = 1;
const RCODE_NOTIMP: u8 = 4;
const RCODE_REFUSED: u8 = 5;

#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// The hotspot's address (what the gateway name resolves to).
    pub server_ip: Ipv4Addr,
    /// Only clients inside this network are served.
    pub subnet: Cidr,
    /// The name of the machine on the hotspot, lower case, without a trailing dot.
    pub gateway_name: String,
    pub max_pending: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientDecision {
    /// Say nothing.
    Drop,
    /// Send these bytes back to the client.
    Reply(Vec<u8>),
    /// Send these bytes to the resolver.
    Forward(Vec<u8>),
}

#[derive(Debug)]
struct Pending {
    up_id: u16,
    client: (Ipv4Addr, u16),
    orig_id: u16,
    question: Vec<u8>,
    born: u64,
}

pub struct Forwarder {
    cfg: DnsConfig,
    pending: Vec<Pending>,
}

/// The question of a packet: lower-cased name labels joined by dots, type, class, and the offset where the section ends.
struct Question {
    name: String,
    qtype: u16,
    qclass: u16,
    end: usize,
}

fn read_question(p: &[u8]) -> Option<Question> {
    let mut i = 12;
    let mut name = String::new();
    loop {
        let l = *p.get(i)? as usize;
        i += 1;
        if l == 0 {
            break;
        }
        if l & 0xc0 != 0 {
            return None; // compression is not allowed in a question we forward or answer
        }
        let label = p.get(i..i + l)?;
        i += l;
        if !name.is_empty() {
            name.push('.');
        }
        name.extend(label.iter().map(|b| b.to_ascii_lowercase() as char));
        if name.len() > 253 {
            return None;
        }
    }
    let t = p.get(i..i + 4)?;
    Some(Question { name, qtype: u16::from_be_bytes([t[0], t[1]]), qclass: u16::from_be_bytes([t[2], t[3]]), end: i + 4 })
}

/// The question bytes in a canonical (lower-case) form, for comparing a response with its query.
fn canon_question(p: &[u8]) -> Option<Vec<u8>> {
    let q = read_question(p)?;
    Some(p[12..q.end].to_ascii_lowercase())
}

fn short_reply(id: [u8; 2], query_flags: u16, rcode: u8) -> Vec<u8> {
    let flags = 0x8000 | (query_flags & 0x7900) | 0x0080 | rcode as u16;
    let mut r = vec![id[0], id[1]];
    r.extend_from_slice(&flags.to_be_bytes());
    r.extend_from_slice(&[0; 8]);
    r
}

impl Forwarder {
    pub fn new(cfg: DnsConfig) -> Forwarder {
        Forwarder { cfg, pending: Vec::new() }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn purge(&mut self, now_ms: u64) {
        self.pending.retain(|p| now_ms.saturating_sub(p.born) < PENDING_MS);
    }

    /// A UDP datagram arrived from `src:sport` on the hotspot. `fresh_id` is a random 16-bit value for the resolver leg.
    pub fn handle_client(&mut self, src: Ipv4Addr, sport: u16, pkt: &[u8], fresh_id: u16, now_ms: u64) -> ClientDecision {
        self.purge(now_ms);
        if sport == 0 || src == self.cfg.server_ip || !self.cfg.subnet.contains(src) || pkt.len() < 12 || pkt.len() > MAX_QUERY {
            return ClientDecision::Drop;
        }
        let id = [pkt[0], pkt[1]];
        let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
        if flags & 0x8000 != 0 {
            return ClientDecision::Drop; // a response, not a query
        }
        let opcode = (flags >> 11) & 15;
        if opcode != 0 {
            return ClientDecision::Reply(short_reply(id, flags, RCODE_NOTIMP));
        }
        let (qd, an, ns, ar) = (u16::from_be_bytes([pkt[4], pkt[5]]), u16::from_be_bytes([pkt[6], pkt[7]]), u16::from_be_bytes([pkt[8], pkt[9]]), u16::from_be_bytes([pkt[10], pkt[11]]));
        let Some(q) = read_question(pkt).filter(|_| qd == 1 && an == 0 && ns == 0 && ar <= 1) else {
            return ClientDecision::Reply(short_reply(id, flags, RCODE_FORMERR));
        };
        if q.name == self.cfg.gateway_name {
            return ClientDecision::Reply(self.own_answer(pkt, flags, &q));
        }
        if self.pending.len() >= self.cfg.max_pending {
            return ClientDecision::Reply(short_reply(id, flags, RCODE_REFUSED));
        }
        let mut up_id = fresh_id;
        for _ in 0..=self.pending.len() {
            if self.pending.iter().all(|p| p.up_id != up_id) {
                break;
            }
            up_id = up_id.wrapping_add(1);
        }
        let mut out = pkt.to_vec();
        out[..2].copy_from_slice(&up_id.to_be_bytes());
        self.pending.push(Pending { up_id, client: (src, sport), orig_id: u16::from_be_bytes(id), question: pkt[12..q.end].to_ascii_lowercase(), born: now_ms });
        ClientDecision::Forward(out)
    }

    fn own_answer(&self, pkt: &[u8], qflags: u16, q: &Question) -> Vec<u8> {
        let is_a = q.qtype == 1 && q.qclass == 1;
        let flags: u16 = 0x8000 | 0x0400 | (qflags & 0x0100) | 0x0080;
        let mut r = vec![pkt[0], pkt[1]];
        r.extend_from_slice(&flags.to_be_bytes());
        r.extend_from_slice(&[0, 1, 0, if is_a { 1 } else { 0 }, 0, 0, 0, 0]);
        r.extend_from_slice(&pkt[12..q.end]);
        if is_a {
            r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
            r.extend_from_slice(&self.cfg.server_ip.octets());
        }
        r
    }

    /// A datagram arrived from the resolver. Returns where to send it and the bytes (the client's own id restored).
    pub fn handle_upstream(&mut self, pkt: &[u8], now_ms: u64) -> Option<(Ipv4Addr, u16, Vec<u8>)> {
        self.purge(now_ms);
        if pkt.len() < 12 || pkt.len() > MAX_RESPONSE || pkt[2] & 0x80 == 0 {
            return None;
        }
        let id = u16::from_be_bytes([pkt[0], pkt[1]]);
        let at = self.pending.iter().position(|p| p.up_id == id)?;
        // the answer must repeat the question that was asked (a forged answer has to guess it as well as the id)
        if canon_question(pkt)? != self.pending[at].question {
            return None;
        }
        let p = self.pending.swap_remove(at);
        let mut out = pkt.to_vec();
        out[..2].copy_from_slice(&p.orig_id.to_be_bytes());
        Some((p.client.0, p.client.1, out))
    }
}

/// Build a query packet (for tests and for a self-test of the uplink resolver).
pub fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut q = id.to_be_bytes().to_vec();
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.').filter(|l| !l.is_empty()) {
        q.push(label.len().min(63) as u8);
        q.extend_from_slice(&label.as_bytes()[..label.len().min(63)]);
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&[0, 1]);
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRV: Ipv4Addr = Ipv4Addr::new(10, 42, 0, 1);
    const CL: Ipv4Addr = Ipv4Addr::new(10, 42, 0, 12);

    fn fw(max_pending: usize) -> Forwarder {
        Forwarder::new(DnsConfig { server_ip: SRV, subnet: Cidr::parse("10.42.0.0/24").unwrap(), gateway_name: "iyakki.lan".into(), max_pending })
    }

    /// A resolver's answer to `q`: same question, one A record.
    fn answer_to(q: &[u8]) -> Vec<u8> {
        let mut r = q.to_vec();
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 1;
        r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
        r
    }

    #[test]
    fn a_query_is_forwarded_under_a_new_id_and_the_answer_returns_with_the_clients_id() {
        let mut f = fw(16);
        let q = build_query(0x1234, "Example.COM", 1);
        let ClientDecision::Forward(up) = f.handle_client(CL, 5353, &q, 0xbeef, 0) else { panic!("not forwarded") };
        assert_eq!(&up[..2], &[0xbe, 0xef]);
        assert_eq!(&up[2..], &q[2..]);
        assert_eq!(f.pending_count(), 1);
        let (ip, port, back) = f.handle_upstream(&answer_to(&up), 10).unwrap();
        assert_eq!((ip, port), (CL, 5353));
        assert_eq!(&back[..2], &[0x12, 0x34]);
        assert_eq!(f.pending_count(), 0);
        assert!(f.handle_upstream(&answer_to(&up), 11).is_none(), "an answer is used once");
    }

    #[test]
    fn answers_that_do_not_match_are_dropped() {
        let mut f = fw(16);
        let q = build_query(1, "example.com", 1);
        let ClientDecision::Forward(up) = f.handle_client(CL, 4000, &q, 77, 0) else { panic!() };
        // a different question under the right id
        let other = build_query(77, "evil.example", 1);
        assert!(f.handle_upstream(&answer_to(&other), 1).is_none());
        // the wrong id
        let mut wrong = answer_to(&up);
        wrong[1] ^= 1;
        assert!(f.handle_upstream(&wrong, 1).is_none());
        // a query is not an answer; junk is not an answer
        assert!(f.handle_upstream(&up, 1).is_none());
        assert!(f.handle_upstream(&[0u8; 5], 1).is_none());
        assert!(f.handle_upstream(&vec![0x80u8; 5000], 1).is_none());
        // the real one still works afterwards, with the name in any letter case
        let mut real = answer_to(&up);
        real[13] = b'E';
        assert!(f.handle_upstream(&real, 2).is_some());
    }

    #[test]
    fn only_clients_of_the_hotspot_subnet_are_served() {
        let mut f = fw(16);
        let q = build_query(1, "example.com", 1);
        for src in [Ipv4Addr::new(192, 168, 1, 5), Ipv4Addr::new(10, 42, 1, 5), Ipv4Addr::new(127, 0, 0, 1), SRV, Ipv4Addr::UNSPECIFIED] {
            assert_eq!(f.handle_client(src, 4000, &q, 1, 0), ClientDecision::Drop, "{src}");
        }
        assert_eq!(f.handle_client(CL, 0, &q, 1, 0), ClientDecision::Drop, "port 0");
        assert!(matches!(f.handle_client(CL, 4000, &q, 1, 0), ClientDecision::Forward(_)));
    }

    #[test]
    fn oversize_junk_responses_and_odd_opcodes_are_handled_without_forwarding() {
        let mut f = fw(16);
        let mut big = build_query(1, "example.com", 1);
        big.resize(513, 0);
        assert_eq!(f.handle_client(CL, 4000, &big, 1, 0), ClientDecision::Drop);
        assert_eq!(f.handle_client(CL, 4000, &[1, 2, 3], 1, 0), ClientDecision::Drop);
        let mut resp = build_query(1, "example.com", 1);
        resp[2] |= 0x80;
        assert_eq!(f.handle_client(CL, 4000, &resp, 1, 0), ClientDecision::Drop, "no reply to a reply");
        let mut notify = build_query(9, "example.com", 1);
        notify[2] |= 4 << 3; // opcode 4
        let ClientDecision::Reply(r) = f.handle_client(CL, 4000, &notify, 1, 0) else { panic!() };
        assert_eq!((r[0], r[1], r[3] & 15), (0, 9, 4));
        // two questions, a compression pointer in the question, a name that never ends
        let mut two = build_query(3, "a.b", 1);
        two[5] = 2;
        let mut ptr = build_query(3, "a.b", 1);
        ptr[12] = 0xc0;
        let mut endless = vec![0u8; 12];
        endless[5] = 1;
        endless.extend_from_slice(&[5, b'a', b'b']);
        for bad in [two, ptr, endless] {
            let ClientDecision::Reply(r) = f.handle_client(CL, 4000, &bad, 1, 0) else { panic!() };
            assert_eq!(r[3] & 15, 1, "format error");
            assert_eq!(r.len(), 12, "short, no amplification");
        }
        assert_eq!(f.pending_count(), 0);
    }

    #[test]
    fn the_machines_own_name_is_answered_here_and_never_forwarded() {
        let mut f = fw(16);
        let ClientDecision::Reply(r) = f.handle_client(CL, 4000, &build_query(0xabcd, "IYAKKI.lan", 1), 5, 0) else { panic!() };
        assert_eq!(&r[..2], &[0xab, 0xcd]);
        assert_eq!((r[2], r[3]), (0x85, 0x80), "response, authoritative, recursion desired and available");
        assert_eq!((r[5], r[7]), (1, 1));
        assert_eq!(&r[r.len() - 4..], &SRV.octets());
        // AAAA: the name exists but has no such record
        let ClientDecision::Reply(r) = f.handle_client(CL, 4000, &build_query(1, "iyakki.lan", 28), 5, 0) else { panic!() };
        assert_eq!((r[3] & 15, r[7]), (0, 0));
        assert_eq!(f.pending_count(), 0);
    }

    #[test]
    fn the_pending_table_is_bounded_expires_and_never_reuses_an_id_in_flight() {
        let mut f = fw(3);
        let mut ids = std::collections::BTreeSet::new();
        for n in 0..3u16 {
            let ClientDecision::Forward(up) = f.handle_client(CL, 4000 + n, &build_query(n, "a.example", 1), 100, 0) else { panic!() };
            assert!(ids.insert(u16::from_be_bytes([up[0], up[1]])), "id reused while in flight");
        }
        let ClientDecision::Reply(r) = f.handle_client(CL, 4010, &build_query(9, "b.example", 1), 100, 1) else { panic!("full table") };
        assert_eq!(r[3] & 15, 5, "refused");
        assert!(matches!(f.handle_client(CL, 4010, &build_query(9, "b.example", 1), 100, PENDING_MS + 1), ClientDecision::Forward(_)));
        assert_eq!(f.pending_count(), 1);
    }

    #[test]
    fn random_bytes_and_mutated_queries_never_panic() {
        let mut f = fw(64);
        let base = build_query(5, "www.example.org", 1);
        let mut s = 42u32;
        let mut next = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (s >> 16) as usize
        };
        for n in 0..8000usize {
            let mut v = base.clone();
            for _ in 0..(1 + n % 5) {
                let at = next() % v.len();
                v[at] = next() as u8;
            }
            if n % 5 == 0 {
                let l = next() % v.len();
                v.truncate(l);
            }
            let junk: Vec<u8> = (0..next() % 700).map(|_| next() as u8).collect();
            for p in [&v, &junk] {
                let d = f.handle_client(CL, 4000, p, next() as u16, n as u64);
                if let ClientDecision::Forward(up) = d {
                    let _ = f.handle_upstream(&answer_to(&up), n as u64);
                }
                let _ = f.handle_upstream(p, n as u64);
            }
        }
        assert!(f.pending_count() <= 64);
    }
}
