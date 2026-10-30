//! The little of DNS wire format the forwarder needs (RFC 1035, with the AAAA type of RFC 3596): read a query's one question,
//! read the A and AAAA answers of a reply, build an error reply. Bounds-checked everywhere, compression pointers followed at
//! most 16 times, nothing is allocated from a length the packet claims.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const MAX_PACKET: usize = 4096;
pub const TYPE_A: u16 = 1;
pub const TYPE_AAAA: u16 = 28;
pub const RCODE_SERVFAIL: u8 = 2;
pub const RCODE_REFUSED: u8 = 5;
const MAX_ANSWERS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsError {
    Short,
    TooBig,
    Malformed(&'static str),
    NotAQuery,
    NotAResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// Lower case, no trailing dot; bytes outside letters, digits, `-` and `_` become `?` (such a name matches no rule).
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    /// Offset of the first byte after the question.
    pub end: usize,
}

fn u16_at(p: &[u8], i: usize) -> Result<u16, DnsError> {
    Ok(u16::from_be_bytes(p.get(i..i + 2).ok_or(DnsError::Short)?.try_into().unwrap()))
}

/// Read a (possibly compressed) name at `pos`; returns the name and the offset after it in the original position.
fn read_name(p: &[u8], pos: usize) -> Result<(String, usize), DnsError> {
    let mut name = String::new();
    let mut i = pos;
    let mut after: Option<usize> = None;
    let mut jumps = 0;
    loop {
        let len = *p.get(i).ok_or(DnsError::Short)? as usize;
        match len & 0xc0 {
            0x00 => {
                if len == 0 {
                    return Ok((name, after.unwrap_or(i + 1)));
                }
                let label = p.get(i + 1..i + 1 + len).ok_or(DnsError::Short)?;
                if !name.is_empty() {
                    name.push('.');
                }
                for &b in label {
                    name.push(if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { b.to_ascii_lowercase() as char } else { '?' });
                }
                if name.len() > 253 {
                    return Err(DnsError::Malformed("name too long"));
                }
                i += 1 + len;
            }
            0xc0 => {
                let lo = *p.get(i + 1).ok_or(DnsError::Short)? as usize;
                if after.is_none() {
                    after = Some(i + 2);
                }
                jumps += 1;
                if jumps > 16 {
                    return Err(DnsError::Malformed("too many compression jumps"));
                }
                i = ((len & 0x3f) << 8) | lo;
            }
            _ => return Err(DnsError::Malformed("bad label type")),
        }
    }
}

fn header(p: &[u8]) -> Result<(u16, u16, u16, u16, u16, u16), DnsError> {
    if p.len() > MAX_PACKET {
        return Err(DnsError::TooBig);
    }
    if p.len() < 12 {
        return Err(DnsError::Short);
    }
    Ok((u16_at(p, 0)?, u16_at(p, 2)?, u16_at(p, 4)?, u16_at(p, 6)?, u16_at(p, 8)?, u16_at(p, 10)?))
}

fn read_question(p: &[u8]) -> Result<Question, DnsError> {
    let (name, i) = read_name(p, 12)?;
    let qtype = u16_at(p, i)?;
    let qclass = u16_at(p, i + 2)?;
    Ok(Question { name, qtype, qclass, end: i + 4 })
}

/// A standard query with exactly one question and nothing else but an optional additional section.
pub fn parse_query(p: &[u8]) -> Result<(u16, Question), DnsError> {
    let (id, flags, qd, an, ns, _ar) = header(p)?;
    if flags & 0x8000 != 0 || (flags >> 11) & 0xf != 0 {
        return Err(DnsError::NotAQuery);
    }
    if qd != 1 || an != 0 || ns != 0 {
        return Err(DnsError::Malformed("expected exactly one question"));
    }
    Ok((id, read_question(p)?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub ip: IpAddr,
    pub ttl: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub id: u16,
    pub rcode: u8,
    pub truncated: bool,
    pub question: Question,
    pub answers: Vec<Answer>,
}

/// A reply: its question and the A / AAAA records of the answer section (other records are skipped).
pub fn parse_response(p: &[u8]) -> Result<Response, DnsError> {
    let (id, flags, qd, an, _ns, _ar) = header(p)?;
    if flags & 0x8000 == 0 {
        return Err(DnsError::NotAResponse);
    }
    if qd != 1 {
        return Err(DnsError::Malformed("expected exactly one question"));
    }
    let q = read_question(p)?;
    let mut i = q.end;
    let mut answers = Vec::new();
    for _ in 0..(an as usize).min(MAX_ANSWERS) {
        let (_n, j) = read_name(p, i)?;
        let ty = u16_at(p, j)?;
        let ttl = u32::from_be_bytes(p.get(j + 4..j + 8).ok_or(DnsError::Short)?.try_into().unwrap());
        let rdlen = u16_at(p, j + 8)? as usize;
        let rd = p.get(j + 10..j + 10 + rdlen).ok_or(DnsError::Short)?;
        match (ty, rdlen) {
            (TYPE_A, 4) => answers.push(Answer { ip: IpAddr::V4(Ipv4Addr::new(rd[0], rd[1], rd[2], rd[3])), ttl }),
            (TYPE_AAAA, 16) => answers.push(Answer { ip: IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(rd).unwrap())), ttl }),
            (TYPE_A, _) | (TYPE_AAAA, _) => return Err(DnsError::Malformed("address record of the wrong size")),
            _ => {}
        }
        i = j + 10 + rdlen;
    }
    if an as usize > MAX_ANSWERS {
        return Err(DnsError::Malformed("too many answers"));
    }
    Ok(Response { id, rcode: (flags & 0xf) as u8, truncated: flags & 0x0200 != 0, question: q, answers })
}

/// Set the transaction id of a packet (a copy).
pub fn with_id(p: &[u8], id: u16) -> Vec<u8> {
    let mut v = p.to_vec();
    if v.len() >= 2 {
        v[0..2].copy_from_slice(&id.to_be_bytes());
    }
    v
}

/// An error reply for a query: same id and question, recursion bits as asked, `rcode` set.
pub fn error_response(query: &[u8], rcode: u8) -> Result<Vec<u8>, DnsError> {
    let (id, q) = parse_query(query)?;
    let rd = u16_at(query, 2)? & 0x0100;
    let mut v = Vec::with_capacity(q.end);
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&(0x8000 | rd | 0x0080 | rcode as u16).to_be_bytes());
    v.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    v.extend_from_slice(&query[12..q.end]);
    Ok(v)
}

// ---- builders, used by tests and by the executor's own probes ---------------------------------------------------------

/// A plain query for `name`.
pub fn build_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&0x0100u16.to_be_bytes());
    v.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    for l in name.split('.').filter(|l| !l.is_empty()) {
        v.push(l.len().min(63) as u8);
        v.extend_from_slice(&l.as_bytes()[..l.len().min(63)]);
    }
    v.push(0);
    v.extend_from_slice(&qtype.to_be_bytes());
    v.extend_from_slice(&1u16.to_be_bytes());
    v
}

/// A reply to `query` with the given address answers (each with a pointer to the question name) and `rcode`.
pub fn build_response(query: &[u8], rcode: u8, answers: &[(IpAddr, u32)]) -> Result<Vec<u8>, DnsError> {
    let mut v = error_response(query, rcode)?;
    v[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());
    for (ip, ttl) in answers {
        v.extend_from_slice(&[0xc0, 12]);
        v.extend_from_slice(&(if ip.is_ipv4() { TYPE_A } else { TYPE_AAAA }).to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&ttl.to_be_bytes());
        match ip {
            IpAddr::V4(a) => {
                v.extend_from_slice(&4u16.to_be_bytes());
                v.extend_from_slice(&a.octets());
            }
            IpAddr::V6(a) => {
                v.extend_from_slice(&16u16.to_be_bytes());
                v.extend_from_slice(&a.octets());
            }
        }
    }
    Ok(v)
}
