//! HTTP/2 framing basics for the domain listener (RFC 9113 sections 4 and 6): the connection preface, the nine-byte frame header, the frame
//! types, the SETTINGS payload and the error codes, with the checks a receiver must make before it looks at a payload (frame size against
//! the limit, stream 0 or not for each type, fixed payload sizes). Pure bytes in, values out; no sockets and no header compression.
//!
//! This is the framing layer. The connection (HPACK from `iyakki-proxy`, stream state, flow control) is `cloudlisten_h2serve.rs`, which
//! builds on what is here and serves HTTP/2 on the listener.

/// The 24 bytes a client sends first.
pub const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// The size of a frame header.
pub const HEADER_LEN: usize = 9;
/// The payload size every peer must accept until a SETTINGS frame says more.
pub const DEFAULT_MAX_FRAME: u32 = 16_384;
/// The largest value SETTINGS_MAX_FRAME_SIZE may take.
pub const MAX_FRAME_LIMIT: u32 = 16_777_215;
/// The largest flow-control window.
pub const MAX_WINDOW: u32 = 2_147_483_647;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Data,
    Headers,
    Priority,
    RstStream,
    Settings,
    PushPromise,
    Ping,
    GoAway,
    WindowUpdate,
    Continuation,
    /// A type this code does not know: ignored by a receiver, never an error.
    Unknown(u8),
}

impl FrameType {
    pub fn from_u8(b: u8) -> FrameType {
        match b {
            0 => FrameType::Data,
            1 => FrameType::Headers,
            2 => FrameType::Priority,
            3 => FrameType::RstStream,
            4 => FrameType::Settings,
            5 => FrameType::PushPromise,
            6 => FrameType::Ping,
            7 => FrameType::GoAway,
            8 => FrameType::WindowUpdate,
            9 => FrameType::Continuation,
            n => FrameType::Unknown(n),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FrameType::Data => 0,
            FrameType::Headers => 1,
            FrameType::Priority => 2,
            FrameType::RstStream => 3,
            FrameType::Settings => 4,
            FrameType::PushPromise => 5,
            FrameType::Ping => 6,
            FrameType::GoAway => 7,
            FrameType::WindowUpdate => 8,
            FrameType::Continuation => 9,
            FrameType::Unknown(n) => n,
        }
    }
}

pub const FLAG_END_STREAM: u8 = 0x1;
pub const FLAG_ACK: u8 = 0x1;
pub const FLAG_END_HEADERS: u8 = 0x4;
pub const FLAG_PADDED: u8 = 0x8;
pub const FLAG_PRIORITY: u8 = 0x20;

/// Error codes of RST_STREAM and GOAWAY.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    NoError,
    ProtocolError,
    InternalError,
    FlowControlError,
    SettingsTimeout,
    StreamClosed,
    FrameSizeError,
    RefusedStream,
    Cancel,
    CompressionError,
    ConnectError,
    EnhanceYourCalm,
    InadequateSecurity,
    Http11Required,
    Unknown(u32),
}

impl ErrorCode {
    pub fn from_u32(n: u32) -> ErrorCode {
        match n {
            0 => ErrorCode::NoError,
            1 => ErrorCode::ProtocolError,
            2 => ErrorCode::InternalError,
            3 => ErrorCode::FlowControlError,
            4 => ErrorCode::SettingsTimeout,
            5 => ErrorCode::StreamClosed,
            6 => ErrorCode::FrameSizeError,
            7 => ErrorCode::RefusedStream,
            8 => ErrorCode::Cancel,
            9 => ErrorCode::CompressionError,
            10 => ErrorCode::ConnectError,
            11 => ErrorCode::EnhanceYourCalm,
            12 => ErrorCode::InadequateSecurity,
            13 => ErrorCode::Http11Required,
            n => ErrorCode::Unknown(n),
        }
    }

    pub fn to_u32(self) -> u32 {
        match self {
            ErrorCode::NoError => 0,
            ErrorCode::ProtocolError => 1,
            ErrorCode::InternalError => 2,
            ErrorCode::FlowControlError => 3,
            ErrorCode::SettingsTimeout => 4,
            ErrorCode::StreamClosed => 5,
            ErrorCode::FrameSizeError => 6,
            ErrorCode::RefusedStream => 7,
            ErrorCode::Cancel => 8,
            ErrorCode::CompressionError => 9,
            ErrorCode::ConnectError => 10,
            ErrorCode::EnhanceYourCalm => 11,
            ErrorCode::InadequateSecurity => 12,
            ErrorCode::Http11Required => 13,
            ErrorCode::Unknown(n) => n,
        }
    }
}

/// A frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub len: u32,
    pub ty: FrameType,
    pub flags: u8,
    /// 31 bits; the reserved top bit is cleared on reading.
    pub stream: u32,
}

/// What is wrong with a frame, and the error the receiver answers with (`conn` is true for a connection error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameError {
    pub code: ErrorCode,
    pub conn: bool,
}

fn conn(code: ErrorCode) -> FrameError {
    FrameError { code, conn: true }
}

/// Reads a frame header from the first nine bytes; `None` when fewer are there.
pub fn parse_header(b: &[u8]) -> Option<FrameHeader> {
    if b.len() < HEADER_LEN {
        return None;
    }
    Some(FrameHeader { len: u32::from_be_bytes([0, b[0], b[1], b[2]]), ty: FrameType::from_u8(b[3]), flags: b[4], stream: u32::from_be_bytes([b[5], b[6], b[7], b[8]]) & 0x7fff_ffff })
}

/// The nine bytes of a header. `len` over 2^24 - 1 or `stream` over 31 bits is refused.
pub fn encode_header(h: &FrameHeader) -> Option<[u8; HEADER_LEN]> {
    if h.len > MAX_FRAME_LIMIT || h.stream > 0x7fff_ffff {
        return None;
    }
    let l = h.len.to_be_bytes();
    let s = h.stream.to_be_bytes();
    Some([l[1], l[2], l[3], h.ty.to_u8(), h.flags, s[0], s[1], s[2], s[3]])
}

/// A whole frame as bytes.
pub fn encode_frame(ty: FrameType, flags: u8, stream: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut v = encode_header(&FrameHeader { len: u32::try_from(payload.len()).ok()?, ty, flags, stream })?.to_vec();
    v.extend_from_slice(payload);
    Some(v)
}

/// The checks a receiver makes on a header before it reads the payload (RFC 9113 section 4.2 and the per-type rules of section 6):
/// the size against the limit it announced, and the frame types that must (or must not) be on stream 0 with their fixed sizes.
pub fn check_header(h: &FrameHeader, max_frame: u32) -> Result<(), FrameError> {
    if h.len > max_frame {
        return Err(conn(ErrorCode::FrameSizeError));
    }
    let zero = h.stream == 0;
    match h.ty {
        FrameType::Data | FrameType::Headers | FrameType::Priority | FrameType::RstStream | FrameType::PushPromise | FrameType::Continuation if zero => Err(conn(ErrorCode::ProtocolError)),
        FrameType::Settings | FrameType::Ping | FrameType::GoAway if !zero => Err(conn(ErrorCode::ProtocolError)),
        FrameType::Priority if h.len != 5 => Err(FrameError { code: ErrorCode::FrameSizeError, conn: false }),
        FrameType::RstStream | FrameType::WindowUpdate if h.len != 4 => Err(conn(ErrorCode::FrameSizeError)),
        FrameType::Ping if h.len != 8 => Err(conn(ErrorCode::FrameSizeError)),
        FrameType::GoAway if h.len < 8 => Err(conn(ErrorCode::FrameSizeError)),
        FrameType::Settings if h.flags & FLAG_ACK != 0 && h.len != 0 => Err(conn(ErrorCode::FrameSizeError)),
        FrameType::Settings if h.len % 6 != 0 => Err(conn(ErrorCode::FrameSizeError)),
        _ => Ok(()),
    }
}

/// The settings of RFC 9113 section 6.5.2. Ones this code does not know are skipped, as the RFC says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub header_table_size: u32,
    pub enable_push: bool,
    pub max_concurrent_streams: Option<u32>,
    pub initial_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: Option<u32>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { header_table_size: 4096, enable_push: true, max_concurrent_streams: None, initial_window_size: 65_535, max_frame_size: DEFAULT_MAX_FRAME, max_header_list_size: None }
    }
}

impl Settings {
    /// Applies the pairs of a SETTINGS payload to a copy, in order, and refuses a value outside its range.
    pub fn apply(&self, payload: &[u8]) -> Result<Settings, FrameError> {
        if payload.len() % 6 != 0 {
            return Err(conn(ErrorCode::FrameSizeError));
        }
        let mut s = *self;
        for p in payload.chunks_exact(6) {
            let id = u16::from_be_bytes([p[0], p[1]]);
            let v = u32::from_be_bytes([p[2], p[3], p[4], p[5]]);
            match id {
                1 => s.header_table_size = v,
                2 => match v {
                    0 => s.enable_push = false,
                    1 => s.enable_push = true,
                    _ => return Err(conn(ErrorCode::ProtocolError)),
                },
                3 => s.max_concurrent_streams = Some(v),
                4 if v > MAX_WINDOW => return Err(conn(ErrorCode::FlowControlError)),
                4 => s.initial_window_size = v,
                5 if !(DEFAULT_MAX_FRAME..=MAX_FRAME_LIMIT).contains(&v) => return Err(conn(ErrorCode::ProtocolError)),
                5 => s.max_frame_size = v,
                6 => s.max_header_list_size = Some(v),
                _ => {}
            }
        }
        Ok(s)
    }

    /// The payload that announces the values that differ from the defaults.
    pub fn encode_changes(&self) -> Vec<u8> {
        let d = Settings::default();
        let mut v = Vec::new();
        let mut put = |id: u16, val: u32| {
            v.extend_from_slice(&id.to_be_bytes());
            v.extend_from_slice(&val.to_be_bytes());
        };
        if self.header_table_size != d.header_table_size {
            put(1, self.header_table_size);
        }
        if self.enable_push != d.enable_push {
            put(2, u32::from(self.enable_push));
        }
        if let Some(n) = self.max_concurrent_streams {
            put(3, n);
        }
        if self.initial_window_size != d.initial_window_size {
            put(4, self.initial_window_size);
        }
        if self.max_frame_size != d.max_frame_size {
            put(5, self.max_frame_size);
        }
        if let Some(n) = self.max_header_list_size {
            put(6, n);
        }
        v
    }
}

/// The part of a padded frame's payload that is data (RFC 9113 section 6.1): the pad length byte and the padding come off. A pad length
/// as large as the payload is a protocol error.
pub fn strip_padding(flags: u8, payload: &[u8]) -> Result<&[u8], FrameError> {
    if flags & FLAG_PADDED == 0 {
        return Ok(payload);
    }
    let Some((&pad, rest)) = payload.split_first() else { return Err(conn(ErrorCode::FrameSizeError)) };
    if usize::from(pad) >= payload.len() {
        return Err(conn(ErrorCode::ProtocolError));
    }
    Ok(&rest[..rest.len() - usize::from(pad)])
}

/// The increment of a WINDOW_UPDATE payload; zero is a protocol error (a connection error on stream 0, a stream error otherwise).
pub fn window_increment(payload: &[u8], stream: u32) -> Result<u32, FrameError> {
    let b: [u8; 4] = payload.try_into().map_err(|_| conn(ErrorCode::FrameSizeError))?;
    let n = u32::from_be_bytes(b) & 0x7fff_ffff;
    if n == 0 {
        return Err(FrameError { code: ErrorCode::ProtocolError, conn: stream == 0 });
    }
    Ok(n)
}

/// A GOAWAY payload: the last stream the sender processed, the code and the debug text.
pub fn parse_goaway(payload: &[u8]) -> Result<(u32, ErrorCode, &[u8]), FrameError> {
    if payload.len() < 8 {
        return Err(conn(ErrorCode::FrameSizeError));
    }
    Ok((u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7fff_ffff, ErrorCode::from_u32(u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]])), &payload[8..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preface_and_a_header_round_trip_with_the_reserved_bit_cleared() {
        assert_eq!(&PREFACE[..], b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        let h = FrameHeader { len: 0x01_02_03, ty: FrameType::Headers, flags: FLAG_END_HEADERS | FLAG_END_STREAM, stream: 7 };
        let b = encode_header(&h).unwrap();
        assert_eq!(b, [1, 2, 3, 1, 5, 0, 0, 0, 7]);
        assert_eq!(parse_header(&b), Some(h));
        let mut with_bit = b;
        with_bit[5] |= 0x80;
        assert_eq!(parse_header(&with_bit).unwrap().stream, 7, "the reserved bit is ignored");
        assert_eq!(parse_header(&b[..8]), None);
        assert!(encode_header(&FrameHeader { len: 1 << 24, ..h }).is_none() && encode_header(&FrameHeader { stream: 1 << 31, ..h }).is_none());
        assert_eq!(FrameType::from_u8(0xee), FrameType::Unknown(0xee));
        assert_eq!(FrameType::Unknown(0xee).to_u8(), 0xee);
        assert_eq!(ErrorCode::from_u32(13).to_u32(), 13);
        assert_eq!(ErrorCode::from_u32(99), ErrorCode::Unknown(99));
    }

    #[test]
    fn a_header_is_checked_against_the_size_limit_stream_zero_and_fixed_sizes() {
        let h = |ty, len, flags, stream| FrameHeader { len, ty, flags, stream };
        assert_eq!(check_header(&h(FrameType::Data, DEFAULT_MAX_FRAME + 1, 0, 1), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::FrameSizeError);
        assert!(check_header(&h(FrameType::Data, DEFAULT_MAX_FRAME, 0, 1), DEFAULT_MAX_FRAME).is_ok());
        assert_eq!(check_header(&h(FrameType::Data, 1, 0, 0), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::ProtocolError, "DATA on stream 0");
        assert_eq!(check_header(&h(FrameType::Settings, 0, 0, 3), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::ProtocolError, "SETTINGS off stream 0");
        assert_eq!(check_header(&h(FrameType::Settings, 7, 0, 0), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::FrameSizeError);
        assert_eq!(check_header(&h(FrameType::Settings, 6, FLAG_ACK, 0), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::FrameSizeError, "an ACK is empty");
        assert!(check_header(&h(FrameType::Settings, 0, FLAG_ACK, 0), DEFAULT_MAX_FRAME).is_ok());
        assert_eq!(check_header(&h(FrameType::Ping, 7, 0, 0), DEFAULT_MAX_FRAME).unwrap_err().code, ErrorCode::FrameSizeError);
        let p = check_header(&h(FrameType::Priority, 4, 0, 1), DEFAULT_MAX_FRAME).unwrap_err();
        assert!(!p.conn, "a wrong PRIORITY size is a stream error");
        assert!(check_header(&h(FrameType::Unknown(0xee), 100, 0, 0), DEFAULT_MAX_FRAME).is_ok(), "unknown types are passed over");
    }

    #[test]
    fn settings_are_applied_in_order_range_checked_and_announced_as_changes() {
        let mut p = Vec::new();
        for (id, v) in [(3u16, 100u32), (4, 1 << 20), (5, 32_768), (2, 0), (0x99, 5)] {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&v.to_be_bytes());
        }
        let s = Settings::default().apply(&p).unwrap();
        assert_eq!((s.max_concurrent_streams, s.initial_window_size, s.max_frame_size, s.enable_push), (Some(100), 1 << 20, 32_768, false));
        assert_eq!(Settings::default().apply(&s.encode_changes()).unwrap(), s, "what is announced is what is read back");
        assert_eq!(Settings::default().encode_changes(), Vec::<u8>::new());
        let bad = |id: u16, v: u32| {
            let mut p = id.to_be_bytes().to_vec();
            p.extend_from_slice(&v.to_be_bytes());
            Settings::default().apply(&p).unwrap_err().code
        };
        assert_eq!(bad(2, 2), ErrorCode::ProtocolError);
        assert_eq!(bad(4, 1 << 31), ErrorCode::FlowControlError);
        assert_eq!(bad(5, 100), ErrorCode::ProtocolError);
        assert_eq!(bad(5, 1 << 24), ErrorCode::ProtocolError);
        assert_eq!(Settings::default().apply(&[0, 1, 0]).unwrap_err().code, ErrorCode::FrameSizeError);
    }

    #[test]
    fn padding_window_updates_and_goaway_are_read_as_the_rfc_says() {
        assert_eq!(strip_padding(0, b"abc").unwrap(), b"abc");
        assert_eq!(strip_padding(FLAG_PADDED, &[2, b'a', b'b', b'c', 0, 0]).unwrap(), b"abc");
        assert_eq!(strip_padding(FLAG_PADDED, &[0]).unwrap(), b"");
        assert_eq!(strip_padding(FLAG_PADDED, &[3, 1, 2]).unwrap_err().code, ErrorCode::ProtocolError, "padding as long as the payload");
        assert_eq!(strip_padding(FLAG_PADDED, &[]).unwrap_err().code, ErrorCode::FrameSizeError);
        assert_eq!(window_increment(&[0, 0, 1, 0], 3), Ok(256));
        assert_eq!(window_increment(&[0, 0, 0, 0], 0), Err(FrameError { code: ErrorCode::ProtocolError, conn: true }));
        assert_eq!(window_increment(&[0, 0, 0, 0], 5), Err(FrameError { code: ErrorCode::ProtocolError, conn: false }));
        assert!(window_increment(&[0, 0, 1], 5).is_err());
        let (last, code, dbg) = parse_goaway(&[0, 0, 0, 9, 0, 0, 0, 2, b'h', b'i']).unwrap();
        assert_eq!((last, code, dbg), (9, ErrorCode::InternalError, &b"hi"[..]));
        assert!(parse_goaway(&[0; 7]).is_err());
        let f = encode_frame(FrameType::Ping, FLAG_ACK, 0, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(f.len(), 17);
        assert!(check_header(&parse_header(&f).unwrap(), DEFAULT_MAX_FRAME).is_ok());
    }
}
