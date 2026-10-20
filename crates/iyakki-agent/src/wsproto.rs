//! WebSocket (RFC 6455), server side: the opening handshake and the frame format. No dependencies; SHA-1 and Base64 are
//! written out here because the handshake needs exactly those two and nothing else.

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// The `Sec-WebSocket-Accept` value for a client's `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{}{GUID}", key.trim()).as_bytes()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WsError {
    /// The client broke the protocol (unmasked frame, reserved bits, oversized message, bad text).
    Protocol(&'static str),
}

/// Collects bytes as they arrive and hands out whole messages; a frame can arrive in pieces.
pub struct Parser {
    buf: Vec<u8>,
    partial: Option<(bool, Vec<u8>)>, // (is text, data so far) of a fragmented message
    max_message: usize,
}

impl Parser {
    pub fn new(max_message: usize) -> Parser {
        Parser { buf: Vec::new(), partial: None, max_message }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, if one has arrived.
    pub fn next(&mut self) -> Result<Option<Message>, WsError> {
        loop {
            let b = &self.buf;
            if b.len() < 2 {
                return Ok(None);
            }
            let (fin, rsv, op) = (b[0] & 0x80 != 0, b[0] & 0x70, b[0] & 0x0f);
            let (masked, len7) = (b[1] & 0x80 != 0, (b[1] & 0x7f) as usize);
            if rsv != 0 {
                return Err(WsError::Protocol("reserved bits set"));
            }
            if !masked {
                return Err(WsError::Protocol("client frames must be masked"));
            }
            let (len, mut at) = match len7 {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().expect("8 bytes")) as usize, 10),
                126 | 127 => return Ok(None),
                n => (n, 2),
            };
            if len > self.max_message {
                return Err(WsError::Protocol("message too large"));
            }
            if b.len() < at + 4 + len {
                return Ok(None);
            }
            let mask = [b[at], b[at + 1], b[at + 2], b[at + 3]];
            at += 4;
            let payload: Vec<u8> = b[at..at + len].iter().enumerate().map(|(i, x)| x ^ mask[i % 4]).collect();
            self.buf.drain(..at + len);
            match op {
                0x8 => return Ok(Some(Message::Close)),
                0x9 => return Ok(Some(Message::Ping(payload))),
                0xA => return Ok(Some(Message::Pong)),
                0x0 | 0x1 | 0x2 => {
                    let (text, mut data) = match (op, self.partial.take()) {
                        (0x0, Some(p)) => p,
                        (0x0, None) => return Err(WsError::Protocol("continuation without a start")),
                        (_, Some(_)) => return Err(WsError::Protocol("new message inside a fragmented one")),
                        (o, None) => (o == 0x1, Vec::new()),
                    };
                    data.extend_from_slice(&payload);
                    if data.len() > self.max_message {
                        return Err(WsError::Protocol("message too large"));
                    }
                    if !fin {
                        self.partial = Some((text, data));
                        continue;
                    }
                    return Ok(Some(if text {
                        Message::Text(String::from_utf8(data).map_err(|_| WsError::Protocol("text is not UTF-8"))?)
                    } else {
                        Message::Binary(data)
                    }));
                }
                _ => return Err(WsError::Protocol("unknown opcode")),
            }
        }
    }
}

/// One unmasked server frame.
pub fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => f.push(n as u8),
        n if n <= 0xffff => {
            f.push(126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            f.push(127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    f.extend_from_slice(payload);
    f
}

pub fn text_frame(s: &str) -> Vec<u8> {
    frame(0x1, s.as_bytes())
}
pub fn binary_frame(b: &[u8]) -> Vec<u8> {
    frame(0x2, b)
}
pub fn pong_frame(b: &[u8]) -> Vec<u8> {
    frame(0xA, b)
}
pub fn close_frame() -> Vec<u8> {
    frame(0x8, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut f = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
        }
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn the_handshake_answer_matches_the_example_in_the_standard() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn sha1_and_base64_match_known_values() {
        let hex: String = sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        let long: String = sha1(&[b'a'; 1000]).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(long, "291e9a6c66994949b57ba5e650361e98fc36b1ba");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn a_masked_text_frame_is_read_even_when_it_arrives_in_pieces() {
        let mut p = Parser::new(1000);
        let f = masked(0x1, true, b"Hello");
        for b in &f[..f.len() - 1] {
            p.push(&[*b]);
            assert_eq!(p.next(), Ok(None));
        }
        p.push(&f[f.len() - 1..]);
        assert_eq!(p.next(), Ok(Some(Message::Text("Hello".into()))));
        assert_eq!(p.next(), Ok(None));
    }

    #[test]
    fn long_frames_fragments_and_control_frames_in_between_are_handled() {
        let mut p = Parser::new(100_000);
        let big = vec![b'x'; 300];
        p.push(&masked(0x1, false, &big[..100]));
        p.push(&masked(0x9, true, b"hi")); // a ping between the fragments
        p.push(&masked(0x0, true, &big[100..]));
        assert_eq!(p.next(), Ok(Some(Message::Ping(b"hi".to_vec()))));
        assert_eq!(p.next(), Ok(Some(Message::Text(String::from_utf8(big).unwrap()))));
    }

    #[test]
    fn broken_clients_are_refused() {
        let unmasked = [0x81, 0x01, b'x'];
        let mut p = Parser::new(100);
        p.push(&unmasked);
        assert!(p.next().is_err(), "unmasked");
        let mut p = Parser::new(10);
        p.push(&masked(0x1, true, &[b'a'; 50]));
        assert!(p.next().is_err(), "too large");
        let mut p = Parser::new(100);
        p.push(&[0xC1, 0x80, 0, 0, 0, 0]);
        assert!(p.next().is_err(), "reserved bits");
        let mut p = Parser::new(100);
        p.push(&masked(0x1, true, &[0xff, 0xfe]));
        assert!(p.next().is_err(), "not UTF-8");
        let mut p = Parser::new(100);
        p.push(&masked(0x0, true, b"x"));
        assert!(p.next().is_err(), "continuation of nothing");
    }

    #[test]
    fn server_frames_have_the_right_length_forms() {
        assert_eq!(text_frame("hi"), vec![0x81, 2, b'h', b'i']);
        let f = binary_frame(&[7u8; 200]);
        assert_eq!(&f[..4], &[0x82, 126, 0, 200]);
        let f = binary_frame(&vec![7u8; 70_000]);
        assert_eq!(f[1], 127);
        assert_eq!(u64::from_be_bytes(f[2..10].try_into().unwrap()), 70_000);
        assert_eq!(close_frame(), vec![0x88, 0]);
    }
}
