//! HPACK, the header compression of HTTP/2 (RFC 7541): the static table, a dynamic table with its size rules, integers with prefixes,
//! Huffman strings, and an encoder and a decoder. Pure: bytes in, headers out, and back. Written from the RFC; the Huffman code is its
//! Appendix B (see [`crate::hpack_table`]) and the examples of its Appendix C are tests. Everything the decoder reads is untrusted:
//! string, table and header-list sizes are capped, and every malformed input is an error, never a panic.

use crate::hpack_table::{CODES, EOS_BITS, EOS_CODE};
use std::collections::VecDeque;
use std::sync::OnceLock;

/// A header: lower-case name and value (a value that is not UTF-8 is kept with the bad bytes replaced).
pub type Header = (String, String);

/// The most bytes of one string literal, of all the headers of one block (name, value and 32 each, as the RFC counts) and the biggest table.
pub const MAX_STRING: usize = 64 * 1024;
pub const MAX_LIST: usize = 256 * 1024;
pub const DEFAULT_TABLE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HpackError {
    Truncated,
    BadIndex(usize),
    BadHuffman,
    TooLarge,
    BadSizeUpdate,
    IntegerOverflow,
}

impl std::fmt::Display for HpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HpackError::Truncated => write!(f, "the header block ends in the middle of a value"),
            HpackError::BadIndex(i) => write!(f, "the header block names table entry {i}, which does not exist"),
            HpackError::BadHuffman => write!(f, "a header string is not valid Huffman code"),
            HpackError::TooLarge => write!(f, "the headers are bigger than the limit"),
            HpackError::BadSizeUpdate => write!(f, "the header table size change is not allowed here"),
            HpackError::IntegerOverflow => write!(f, "a number in the header block is too large"),
        }
    }
}

impl std::error::Error for HpackError {}

/// RFC 7541 Appendix A.
pub const STATIC_TABLE: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

// ------------------------------------------------------------------------------------------------ integers

pub fn encode_int(out: &mut Vec<u8>, flags: u8, prefix: u8, mut v: usize) {
    let max = (1usize << prefix) - 1;
    if v < max {
        out.push(flags | v as u8);
        return;
    }
    out.push(flags | max as u8);
    v -= max;
    while v >= 128 {
        out.push((v % 128) as u8 | 128);
        v /= 128;
    }
    out.push(v as u8);
}

/// The integer with a `prefix`-bit start at `pos`, and the position after it.
pub fn decode_int(b: &[u8], mut pos: usize, prefix: u8) -> Result<(usize, usize), HpackError> {
    let max = (1usize << prefix) - 1;
    let first = *b.get(pos).ok_or(HpackError::Truncated)? as usize & max;
    pos += 1;
    if first < max {
        return Ok((first, pos));
    }
    let (mut v, mut shift) = (max, 0u32);
    loop {
        let byte = *b.get(pos).ok_or(HpackError::Truncated)? as usize;
        pos += 1;
        if shift > 28 {
            return Err(HpackError::IntegerOverflow);
        }
        v = v.checked_add((byte & 127) << shift).ok_or(HpackError::IntegerOverflow)?;
        shift += 7;
        if byte & 128 == 0 {
            return Ok((v, pos));
        }
    }
}

// ------------------------------------------------------------------------------------------------ Huffman

pub fn huffman_len(s: &[u8]) -> usize {
    s.iter().map(|&b| CODES[b as usize].1 as usize).sum::<usize>().div_ceil(8)
}

pub fn huffman_encode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(huffman_len(s));
    let (mut acc, mut bits) = (0u64, 0u32);
    for &b in s {
        let (code, len) = CODES[b as usize];
        acc = (acc << len) | code as u64;
        bits += len as u32;
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits > 0 {
        // the rest of the last byte is the start of the end-of-string code: ones
        out.push(((acc << (8 - bits)) as u8) | (0xffu8 >> bits));
    }
    out
}

/// Nodes of the decoding tree: `[child for 0, child for 1]`; a leaf holds the byte as `-(byte + 1)`, 0 is "none".
fn tree() -> &'static Vec<[i32; 2]> {
    static T: OnceLock<Vec<[i32; 2]>> = OnceLock::new();
    T.get_or_init(|| {
        let mut nodes: Vec<[i32; 2]> = vec![[0, 0]];
        let mut add = |code: u32, len: u8, leaf: i32| {
            let mut at = 0usize;
            for i in (0..len).rev() {
                let bit = ((code >> i) & 1) as usize;
                if i == 0 {
                    nodes[at][bit] = leaf;
                } else {
                    if nodes[at][bit] <= 0 {
                        nodes.push([0, 0]);
                        let n = nodes.len() as i32 - 1;
                        nodes[at][bit] = n;
                    }
                    at = nodes[at][bit] as usize;
                }
            }
        };
        for (b, (code, len)) in CODES.iter().enumerate() {
            add(*code, *len, -(b as i32 + 1));
        }
        add(EOS_CODE, EOS_BITS, -257);
        nodes
    })
}

pub fn huffman_decode(s: &[u8]) -> Result<Vec<u8>, HpackError> {
    let t = tree();
    let (mut out, mut at, mut run_of_ones, mut bits_since) = (Vec::with_capacity(s.len() * 2), 0usize, true, 0u32);
    for &byte in s {
        for i in (0..8).rev() {
            let bit = ((byte >> i) & 1) as usize;
            let next = t[at][bit];
            if bit == 0 {
                run_of_ones = false;
            }
            bits_since += 1;
            if next < 0 {
                if next == -257 {
                    return Err(HpackError::BadHuffman); // the end-of-string symbol inside a string
                }
                out.push((-next - 1) as u8);
                (at, run_of_ones, bits_since) = (0, true, 0);
                if out.len() > MAX_STRING * 2 {
                    return Err(HpackError::TooLarge);
                }
            } else if next == 0 {
                return Err(HpackError::BadHuffman);
            } else {
                at = next as usize;
            }
        }
    }
    // what is left over is padding: fewer than 8 bits, all ones (the start of the end-of-string code)
    if at != 0 && (bits_since > 7 || !run_of_ones) {
        return Err(HpackError::BadHuffman);
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ the dynamic table

#[derive(Debug, Default)]
struct Table {
    entries: VecDeque<Header>,
    size: usize,
    max: usize,
}

fn entry_size(h: &Header) -> usize {
    h.0.len() + h.1.len() + 32
}

impl Table {
    fn new(max: usize) -> Table {
        Table { entries: VecDeque::new(), size: 0, max }
    }

    fn evict(&mut self) {
        while self.size > self.max {
            match self.entries.pop_back() {
                Some(e) => self.size -= entry_size(&e),
                None => break,
            }
        }
    }

    fn add(&mut self, h: Header) {
        let s = entry_size(&h);
        if s > self.max {
            self.entries.clear();
            self.size = 0;
            return;
        }
        self.size += s;
        self.entries.push_front(h);
        self.evict();
    }

    fn resize(&mut self, max: usize) {
        self.max = max;
        self.evict();
    }

    /// The entry with this 1-based index over the static and then the dynamic table.
    fn get(&self, index: usize) -> Option<Header> {
        if index == 0 {
            None
        } else if index <= STATIC_TABLE.len() {
            let (n, v) = STATIC_TABLE[index - 1];
            Some((n.to_string(), v.to_string()))
        } else {
            self.entries.get(index - STATIC_TABLE.len() - 1).cloned()
        }
    }

    /// (index of an entry with this name and value, index of one with this name).
    fn find(&self, name: &str, value: &str) -> (Option<usize>, Option<usize>) {
        let (mut full, mut named) = (None, None);
        for (i, (n, v)) in STATIC_TABLE.iter().enumerate() {
            if *n == name {
                named.get_or_insert(i + 1);
                if *v == value {
                    full = Some(i + 1);
                    break;
                }
            }
        }
        if full.is_none() {
            for (i, (n, v)) in self.entries.iter().enumerate() {
                if n == name {
                    named.get_or_insert(STATIC_TABLE.len() + i + 1);
                    if v == value {
                        full = Some(STATIC_TABLE.len() + i + 1);
                        break;
                    }
                }
            }
        }
        (full, named)
    }
}

// ------------------------------------------------------------------------------------------------ the decoder

pub struct Decoder {
    table: Table,
    /// The largest table size this side allows the peer to use (what it announced in its SETTINGS).
    allowed: usize,
    /// A literal name with a capital letter was seen since the last [`Decoder::take_uppercase_name`] (a client may not send one; the name is lower-cased here either way).
    uppercase: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new(DEFAULT_TABLE)
    }
}

fn string_literal(b: &[u8], pos: usize) -> Result<(Vec<u8>, usize), HpackError> {
    let huff = b.get(pos).ok_or(HpackError::Truncated)? & 0x80 != 0;
    let (len, p) = decode_int(b, pos, 7)?;
    if len > MAX_STRING {
        return Err(HpackError::TooLarge);
    }
    let raw = b.get(p..p.checked_add(len).ok_or(HpackError::Truncated)?).ok_or(HpackError::Truncated)?;
    Ok((if huff { huffman_decode(raw)? } else { raw.to_vec() }, p + len))
}

impl Decoder {
    pub fn new(allowed: usize) -> Decoder {
        Decoder { table: Table::new(allowed), allowed, uppercase: false }
    }

    /// Whether a block decoded since the last call held a literal name with a capital letter (RFC 9113 8.2.1: a malformed request).
    pub fn take_uppercase_name(&mut self) -> bool {
        std::mem::take(&mut self.uppercase)
    }

    /// The headers of one complete header block (HEADERS and its CONTINUATIONs put together).
    pub fn decode(&mut self, b: &[u8]) -> Result<Vec<Header>, HpackError> {
        let (mut pos, mut out, mut total, mut head) = (0usize, Vec::new(), 0usize, true);
        while pos < b.len() {
            let first = b[pos];
            if first & 0x80 != 0 {
                let (i, p) = decode_int(b, pos, 7)?;
                let h = self.table.get(i).ok_or(HpackError::BadIndex(i))?;
                pos = p;
                total += entry_size(&h);
                out.push(h);
                head = false;
            } else if first & 0xe0 == 0x20 {
                // a table size change: only before the first header, and never above what was allowed
                let (n, p) = decode_int(b, pos, 5)?;
                if !head || n > self.allowed {
                    return Err(HpackError::BadSizeUpdate);
                }
                self.table.resize(n);
                pos = p;
            } else {
                let (prefix, index_it) = if first & 0x40 != 0 { (6, true) } else { (4, false) };
                let (i, p) = decode_int(b, pos, prefix)?;
                let (name, p) = if i == 0 {
                    let (n, p2) = string_literal(b, p)?;
                    self.uppercase |= n.iter().any(u8::is_ascii_uppercase);
                    (String::from_utf8_lossy(&n).to_ascii_lowercase(), p2)
                } else {
                    (self.table.get(i).ok_or(HpackError::BadIndex(i))?.0, p)
                };
                let (v, p) = string_literal(b, p)?;
                pos = p;
                let h: Header = (name, String::from_utf8_lossy(&v).into_owned());
                total += entry_size(&h);
                if index_it {
                    self.table.add(h.clone());
                }
                out.push(h);
                head = false;
            }
            if total > MAX_LIST {
                return Err(HpackError::TooLarge);
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------------------------------------ the encoder

pub struct Encoder {
    table: Table,
    /// A size change to announce at the start of the next block (the peer's SETTINGS lowered it).
    pending: Option<usize>,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder::new(DEFAULT_TABLE)
    }
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    if huffman_len(s) < s.len() {
        let h = huffman_encode(s);
        encode_int(out, 0x80, 7, h.len());
        out.extend_from_slice(&h);
    } else {
        encode_int(out, 0, 7, s.len());
        out.extend_from_slice(s);
    }
}

/// Names whose values are never put in a table (nor by a proxy on the way): credentials and cookies.
fn sensitive(name: &str) -> bool {
    matches!(name, "authorization" | "proxy-authorization" | "cookie" | "set-cookie")
}

impl Encoder {
    pub fn new(max: usize) -> Encoder {
        Encoder { table: Table::new(max), pending: None }
    }

    /// The peer allows a table of at most `n` bytes (its SETTINGS_HEADER_TABLE_SIZE): the next block says so.
    pub fn set_peer_limit(&mut self, n: usize) {
        if n < self.table.max {
            self.table.resize(n);
            self.pending = Some(n);
        }
    }

    pub fn encode(&mut self, headers: &[Header]) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(n) = self.pending.take() {
            encode_int(&mut out, 0x20, 5, n);
        }
        for (name, value) in headers {
            let (full, named) = self.table.find(name, value);
            if let (Some(i), false) = (full, sensitive(name)) {
                encode_int(&mut out, 0x80, 7, i);
            } else if sensitive(name) {
                // a literal that no table keeps
                match named {
                    Some(i) => encode_int(&mut out, 0x10, 4, i),
                    None => {
                        encode_int(&mut out, 0x10, 4, 0);
                        put_string(&mut out, name.as_bytes());
                    }
                }
                put_string(&mut out, value.as_bytes());
            } else {
                match named {
                    Some(i) => encode_int(&mut out, 0x40, 6, i),
                    None => {
                        encode_int(&mut out, 0x40, 6, 0);
                        put_string(&mut out, name.as_bytes());
                    }
                }
                put_string(&mut out, value.as_bytes());
                self.table.add((name.clone(), value.clone()));
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "hpack_tests.rs"]
mod tests;
