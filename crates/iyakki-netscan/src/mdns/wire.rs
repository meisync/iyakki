//! The mDNS packet format (RFC 1035 messages as used by RFC 6762), the subset the device name needs: questions, and the A, AAAA, PTR,
//! SRV and TXT records. Decoding never trusts a packet: sizes are capped, a compression pointer must point backwards (so a loop
//! cannot happen), labels are at most 63 bytes, a name at most 255, and every read is bounds-checked. A packet that breaks a rule is
//! an error, never a panic.

use std::net::{Ipv4Addr, Ipv6Addr};

pub const TYPE_A: u16 = 1;
pub const TYPE_PTR: u16 = 12;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_SRV: u16 = 33;
pub const TYPE_ANY: u16 = 255;
pub const CLASS_IN: u16 = 1;

/// Largest packet looked at (RFC 6762 allows 9000; nothing here needs more).
pub const MAX_PACKET: usize = 9000;
/// Largest packet this code sends (fits one Ethernet frame without fragmenting).
pub const MAX_SEND: usize = 1400;
const MAX_RECORDS: usize = 64;
const MAX_LABEL: usize = 63;
const MAX_NAME: usize = 255;
const MAX_JUMPS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Ptr(String),
    Srv { priority: u16, weight: u16, port: u16, target: String },
    Txt(Vec<String>),
    /// A record type this code does not use: kept as bytes (at most 256), never interpreted.
    Other(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Lower-case, labels joined by dots, no trailing dot (`iyakki-a1b2c3.local`).
    pub name: String,
    pub rtype: u16,
    /// The cache-flush bit (RFC 6762 section 10.2).
    pub flush: bool,
    pub ttl: u32,
    pub data: Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub name: String,
    pub qtype: u16,
    /// The "unicast response wanted" bit (QU, RFC 6762 section 5.4).
    pub unicast: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub id: u16,
    /// Bit 15 is QR (1: a response), bit 10 AA.
    pub flags: u16,
    pub questions: Vec<Question>,
    pub answers: Vec<Record>,
    pub authority: Vec<Record>,
    pub additional: Vec<Record>,
}

impl Message {
    pub fn is_response(&self) -> bool {
        self.flags & 0x8000 != 0
    }

    pub fn query(questions: Vec<Question>) -> Message {
        Message { questions, ..Default::default() }
    }

    pub fn response(answers: Vec<Record>, additional: Vec<Record>) -> Message {
        Message { flags: 0x8400, answers, additional, ..Default::default() }
    }
}

/// A lower-case copy of a name without its trailing dot (names are compared this way).
pub fn norm(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

// --------------------------------------------------------------------------------------------------------------- encode

fn put_name(out: &mut Vec<u8>, name: &str) -> Result<(), String> {
    let name = name.trim_end_matches('.');
    if name.len() > MAX_NAME {
        return Err("name too long".into());
    }
    if !name.is_empty() {
        for label in name.split('.') {
            if label.is_empty() || label.len() > MAX_LABEL {
                return Err(format!("bad label in {name}"));
            }
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
    }
    out.push(0);
    Ok(())
}

fn put_record(out: &mut Vec<u8>, r: &Record) -> Result<(), String> {
    put_name(out, &r.name)?;
    out.extend_from_slice(&r.rtype.to_be_bytes());
    out.extend_from_slice(&(CLASS_IN | if r.flush { 0x8000 } else { 0 }).to_be_bytes());
    out.extend_from_slice(&r.ttl.to_be_bytes());
    let at = out.len();
    out.extend_from_slice(&[0, 0]);
    match &r.data {
        Data::A(a) => out.extend_from_slice(&a.octets()),
        Data::Aaaa(a) => out.extend_from_slice(&a.octets()),
        Data::Ptr(n) => put_name(out, n)?,
        Data::Srv { priority, weight, port, target } => {
            for v in [priority, weight, port] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            put_name(out, target)?;
        }
        Data::Txt(items) => {
            if items.is_empty() {
                out.push(0);
            }
            for s in items {
                if s.len() > 255 {
                    return Err("TXT item too long".into());
                }
                out.push(s.len() as u8);
                out.extend_from_slice(s.as_bytes());
            }
        }
        Data::Other(b) => out.extend_from_slice(b),
    }
    let len = out.len() - at - 2;
    if len > u16::MAX as usize {
        return Err("record too long".into());
    }
    out[at..at + 2].copy_from_slice(&(len as u16).to_be_bytes());
    Ok(())
}

/// The bytes of a message (no name compression). An error when a name is malformed or the packet would pass [`MAX_SEND`].
pub fn encode(m: &Message) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&m.id.to_be_bytes());
    out.extend_from_slice(&m.flags.to_be_bytes());
    for n in [m.questions.len(), m.answers.len(), m.authority.len(), m.additional.len()] {
        out.extend_from_slice(&u16::try_from(n).map_err(|_| "too many records")?.to_be_bytes());
    }
    for q in &m.questions {
        put_name(&mut out, &q.name)?;
        out.extend_from_slice(&q.qtype.to_be_bytes());
        out.extend_from_slice(&(CLASS_IN | if q.unicast { 0x8000 } else { 0 }).to_be_bytes());
    }
    for r in m.answers.iter().chain(&m.authority).chain(&m.additional) {
        put_record(&mut out, r)?;
    }
    if out.len() > MAX_SEND {
        return Err(format!("packet of {} bytes is over {MAX_SEND}", out.len()));
    }
    Ok(out)
}

// --------------------------------------------------------------------------------------------------------------- decode

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("truncated packet")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn name(&mut self) -> Result<String, String> {
        let (n, next) = name_at(self.b, self.at)?;
        self.at = next;
        Ok(n)
    }
}

/// The name that starts at `at` and the offset just after it (in the place it started, not after a jump).
fn name_at(b: &[u8], start: usize) -> Result<(String, usize), String> {
    let mut at = start;
    let mut end: Option<usize> = None;
    let mut jumps = 0;
    let mut name = String::new();
    loop {
        let len = *b.get(at).ok_or("truncated name")? as usize;
        match len & 0xC0 {
            0 if len == 0 => {
                return Ok((name, end.unwrap_or(at + 1)));
            }
            0 => {
                let label = b.get(at + 1..at + 1 + len).ok_or("truncated label")?;
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
                if name.len() > MAX_NAME {
                    return Err("name too long".into());
                }
                at += 1 + len;
            }
            0xC0 => {
                let target = ((len & 0x3F) << 8) | *b.get(at + 1).ok_or("truncated pointer")? as usize;
                // a pointer may only go back to something before itself: a loop is then impossible, and the jump count is capped too
                if target >= at {
                    return Err("compression pointer does not point backwards".into());
                }
                jumps += 1;
                if jumps > MAX_JUMPS {
                    return Err("too many compression pointers".into());
                }
                end.get_or_insert(at + 2);
                at = target;
            }
            _ => return Err("unsupported label type".into()),
        }
    }
}

fn read_record(r: &mut Reader) -> Result<Record, String> {
    let name = r.name()?;
    let rtype = r.u16()?;
    let class = r.u16()?;
    let ttl = r.u32()?;
    let len = r.u16()? as usize;
    let start = r.at;
    let body = r.take(len)?;
    let data = match rtype {
        TYPE_A if len == 4 => Data::A(Ipv4Addr::new(body[0], body[1], body[2], body[3])),
        TYPE_AAAA if len == 16 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(body);
            Data::Aaaa(Ipv6Addr::from(o))
        }
        TYPE_PTR => Data::Ptr(name_at(r.b, start)?.0),
        TYPE_SRV if len >= 7 => {
            let u = |i: usize| u16::from_be_bytes([body[i], body[i + 1]]);
            Data::Srv { priority: u(0), weight: u(2), port: u(4), target: name_at(r.b, start + 6)?.0 }
        }
        TYPE_TXT => {
            let mut items = Vec::new();
            let mut i = 0;
            while i < body.len() {
                let n = body[i] as usize;
                let s = body.get(i + 1..i + 1 + n).ok_or("truncated TXT")?;
                if n > 0 {
                    items.push(String::from_utf8_lossy(s).into_owned());
                }
                i += 1 + n;
            }
            Data::Txt(items)
        }
        _ => Data::Other(body[..body.len().min(256)].to_vec()),
    };
    Ok(Record { name, rtype, flush: class & 0x8000 != 0, ttl, data })
}

/// Read a packet. Errors (never panics) on anything malformed.
pub fn decode(b: &[u8]) -> Result<Message, String> {
    if b.len() < 12 {
        return Err("shorter than a header".into());
    }
    if b.len() > MAX_PACKET {
        return Err("packet too large".into());
    }
    let mut r = Reader { b, at: 0 };
    let id = r.u16()?;
    let flags = r.u16()?;
    let counts = [r.u16()? as usize, r.u16()? as usize, r.u16()? as usize, r.u16()? as usize];
    if counts.iter().sum::<usize>() > MAX_RECORDS * 2 || counts.iter().any(|c| *c > MAX_RECORDS) {
        return Err("too many records".into());
    }
    let mut m = Message { id, flags, ..Default::default() };
    for _ in 0..counts[0] {
        let name = r.name()?;
        let qtype = r.u16()?;
        let class = r.u16()?;
        m.questions.push(Question { name, qtype, unicast: class & 0x8000 != 0 });
    }
    for (i, n) in counts[1..].iter().enumerate() {
        for _ in 0..*n {
            let rec = read_record(&mut r)?;
            [&mut m.answers, &mut m.authority, &mut m.additional][i].push(rec);
        }
    }
    Ok(m)
}
