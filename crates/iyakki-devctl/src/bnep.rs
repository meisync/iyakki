//! BNEP, the Bluetooth network encapsulation protocol phones use for tethering (the PAN profile), in our own code: the kernel of
//! this system has no `bnep` module. Pure packet handling, no sockets: the L2CAP and TAP calls live in `bt_pan.rs`.
//!
//! One BNEP packet is one L2CAP message: a type byte (bit 7: extension headers follow), then either an Ethernet frame in one of
//! four compressions or a control message. Control messages set the connection up (service UUIDs), and the phone may ask us to
//! filter network types and multicast addresses; we accept everything.

/// The L2CAP port BNEP listens on.
pub const PSM: u16 = 0x000F;
/// The smallest L2CAP MTU BNEP may use (an Ethernet frame plus the headers).
pub const MIN_MTU: u16 = 1691;
pub const UUID_PANU: u16 = 0x1115;
pub const UUID_NAP: u16 = 0x1116;
pub const UUID_GN: u16 = 0x1117;

const GENERAL_ETHERNET: u8 = 0x00;
const CONTROL: u8 = 0x01;
const COMPRESSED: u8 = 0x02;
const COMPRESSED_SRC_ONLY: u8 = 0x03;
const COMPRESSED_DST_ONLY: u8 = 0x04;

const CTRL_NOT_UNDERSTOOD: u8 = 0x00;
const CTRL_SETUP_REQUEST: u8 = 0x01;
const CTRL_SETUP_RESPONSE: u8 = 0x02;
const CTRL_FILTER_NET_SET: u8 = 0x03;
const CTRL_FILTER_NET_RESPONSE: u8 = 0x04;
const CTRL_FILTER_MULTI_SET: u8 = 0x05;
const CTRL_FILTER_MULTI_RESPONSE: u8 = 0x06;

pub type Mac = [u8; 6];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub dst: Mac,
    pub src: Mac,
    pub proto: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Destination and source service UUIDs (16-bit ones; the long forms are read but only the Bluetooth base is accepted).
    SetupRequest { dst: u16, src: u16 },
    SetupResponse(u16),
    FilterNetType,
    FilterNetResponse(u16),
    FilterMultiAddr,
    FilterMultiResponse(u16),
    NotUnderstood(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Ethernet(Frame),
    Control(Control),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Short,
    Unknown(u8),
}

/// The 128-bit form of a 16-bit UUID, big endian: `0000xxxx-0000-1000-8000-00805F9B34FB`.
fn long_uuid(short: u16) -> [u8; 16] {
    let mut u = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB];
    u[2..4].copy_from_slice(&short.to_be_bytes());
    u
}

fn short_uuid(b: &[u8]) -> Option<u16> {
    match b.len() {
        2 => Some(u16::from_be_bytes([b[0], b[1]])),
        4 if b[0] == 0 && b[1] == 0 => Some(u16::from_be_bytes([b[2], b[3]])),
        16 if b[..2] == [0, 0] && b[4..] == long_uuid(0)[4..] => Some(u16::from_be_bytes([b[2], b[3]])),
        _ => None,
    }
}

/// The request that opens a connection: we are `src` (PANU to use a phone, NAP to offer one), the phone is `dst`.
pub fn setup_request(dst: u16, src: u16) -> Vec<u8> {
    let mut v = vec![CONTROL, CTRL_SETUP_REQUEST, 2];
    v.extend_from_slice(&dst.to_be_bytes());
    v.extend_from_slice(&src.to_be_bytes());
    v
}

pub fn setup_response(code: u16) -> Vec<u8> {
    let mut v = vec![CONTROL, CTRL_SETUP_RESPONSE];
    v.extend_from_slice(&code.to_be_bytes());
    v
}

/// What a setup response code means, in a plain line.
pub fn setup_text(code: u16) -> &'static str {
    match code {
        0 => "connected",
        1 => "the phone does not offer that network service",
        2 => "the phone refused this computer's service",
        3 => "the phone does not support that service",
        4 => "the phone does not allow the connection: switch Bluetooth tethering on in its settings",
        _ => "the phone answered with an unknown code",
    }
}

/// An Ethernet frame as a general BNEP packet (every address given, no compression: always valid).
pub fn encode_frame(f: &Frame) -> Vec<u8> {
    let mut v = Vec::with_capacity(15 + f.payload.len());
    v.push(GENERAL_ETHERNET);
    v.extend_from_slice(&f.dst);
    v.extend_from_slice(&f.src);
    v.extend_from_slice(&f.proto.to_be_bytes());
    v.extend_from_slice(&f.payload);
    v
}

/// Skip the extension headers that follow a packet whose type byte had bit 7 set. Returns the offset after them.
fn skip_extensions(data: &[u8], mut at: usize) -> Result<usize, Error> {
    loop {
        let t = *data.get(at).ok_or(Error::Short)?;
        let len = *data.get(at + 1).ok_or(Error::Short)? as usize;
        at += 2 + len;
        if at > data.len() {
            return Err(Error::Short);
        }
        if t & 0x80 == 0 {
            return Ok(at);
        }
    }
}

/// Read one packet. `local` is our own address and `remote` the phone's: the compressed forms leave the addresses out.
pub fn parse(data: &[u8], local: Mac, remote: Mac) -> Result<Packet, Error> {
    let t = *data.first().ok_or(Error::Short)?;
    let ext = t & 0x80 != 0;
    let kind = t & 0x7f;
    let take = |from: usize, n: usize| data.get(from..from + n).ok_or(Error::Short);
    let mac = |from: usize| -> Result<Mac, Error> { Ok(take(from, 6)?.try_into().expect("six bytes")) };
    let proto = |from: usize| -> Result<u16, Error> {
        let p = take(from, 2)?;
        Ok(u16::from_be_bytes([p[0], p[1]]))
    };
    // (dst, src, protocol, where the payload starts) and where an extension chain would start
    let (dst, src, proto_at, head) = match kind {
        GENERAL_ETHERNET => (mac(1)?, mac(7)?, 13, 15),
        COMPRESSED => (local, remote, 1, 3),
        COMPRESSED_SRC_ONLY => (local, mac(1)?, 7, 9),
        COMPRESSED_DST_ONLY => (mac(1)?, remote, 7, 9),
        CONTROL => return parse_control(data, ext),
        other => return Err(Error::Unknown(other)),
    };
    let proto = proto(proto_at)?;
    let start = if ext { skip_extensions(data, head)? } else { head };
    Ok(Packet::Ethernet(Frame { dst, src, proto, payload: data.get(start..).ok_or(Error::Short)?.to_vec() }))
}

fn parse_control(data: &[u8], _ext: bool) -> Result<Packet, Error> {
    let ct = *data.get(1).ok_or(Error::Short)?;
    let body = &data[2..];
    let u16at = |b: &[u8]| -> Result<u16, Error> { Ok(u16::from_be_bytes([*b.first().ok_or(Error::Short)?, *b.get(1).ok_or(Error::Short)?])) };
    Ok(Packet::Control(match ct {
        CTRL_NOT_UNDERSTOOD => Control::NotUnderstood(*body.first().ok_or(Error::Short)?),
        CTRL_SETUP_REQUEST => {
            let n = *body.first().ok_or(Error::Short)? as usize;
            let ids = body.get(1..1 + 2 * n).ok_or(Error::Short)?;
            let (d, s) = ids.split_at(n);
            match (short_uuid(d), short_uuid(s)) {
                (Some(dst), Some(src)) => Control::SetupRequest { dst, src },
                _ => Control::SetupRequest { dst: 0, src: 0 },
            }
        }
        CTRL_SETUP_RESPONSE => Control::SetupResponse(u16at(body)?),
        CTRL_FILTER_NET_SET => Control::FilterNetType,
        CTRL_FILTER_NET_RESPONSE => Control::FilterNetResponse(u16at(body)?),
        CTRL_FILTER_MULTI_SET => Control::FilterMultiAddr,
        CTRL_FILTER_MULTI_RESPONSE => Control::FilterMultiResponse(u16at(body)?),
        other => return Err(Error::Unknown(other)),
    }))
}

/// The reply a control message needs, if any: filters are accepted (everything passes), unknown control messages are
/// answered with "not understood" so the phone does not wait.
pub fn answer(c: &Control) -> Option<Vec<u8>> {
    match c {
        Control::FilterNetType => Some(vec![CONTROL, CTRL_FILTER_NET_RESPONSE, 0, 0]),
        Control::FilterMultiAddr => Some(vec![CONTROL, CTRL_FILTER_MULTI_RESPONSE, 0, 0]),
        // a phone never opens a connection to a PANU, but if one does, it is told "not allowed" (0x0004) instead of being left waiting
        Control::SetupRequest { .. } => Some(setup_response(4)),
        _ => None,
    }
}

pub fn not_understood(control_type: u8) -> Vec<u8> {
    vec![CONTROL, CTRL_NOT_UNDERSTOOD, control_type]
}

/// The connection of one phone, seen from this computer as a PANU (we use the phone's internet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The request is sent, waiting for the phone's answer.
    Connecting,
    Up,
    Refused(u16),
}

/// What one received packet leads to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Packets to send back to the phone.
    pub replies: Vec<Vec<u8>>,
    /// An Ethernet frame to hand to the TAP interface.
    pub deliver: Option<Frame>,
}

/// The client side of one BNEP connection (PANU to a phone's NAP, or any of the three roles by the UUIDs given).
pub struct Panu {
    pub state: State,
    local: Mac,
    remote: Mac,
}

impl Panu {
    /// Start a connection to a phone that offers `dst` (the NAP service of a tethering phone). Returns the first packet to send.
    pub fn start(local: Mac, remote: Mac, dst: u16) -> (Panu, Vec<u8>) {
        (Panu { state: State::Connecting, local, remote }, setup_request(dst, UUID_PANU))
    }

    pub fn receive(&mut self, data: &[u8]) -> Result<Outcome, Error> {
        let mut out = Outcome::default();
        match parse(data, self.local, self.remote) {
            Ok(Packet::Ethernet(f)) => {
                // frames only count once the phone accepted the connection
                if self.state == State::Up {
                    out.deliver = Some(f);
                }
            }
            Ok(Packet::Control(Control::SetupResponse(code))) => {
                // only the answer to our own request counts; a late or repeated one cannot take a working link down
                if self.state == State::Connecting {
                    self.state = if code == 0 { State::Up } else { State::Refused(code) };
                }
            }
            Ok(Packet::Control(c)) => out.replies.extend(answer(&c)),
            Err(Error::Unknown(t)) if data.first().is_some_and(|b| b & 0x7f == CONTROL) => out.replies.push(not_understood(t)),
            Err(e) => return Err(e),
        }
        Ok(out)
    }

    /// The packet for a frame the computer wants to send; `None` before the connection is up.
    pub fn send(&self, f: &Frame) -> Option<Vec<u8>> {
        (self.state == State::Up).then(|| encode_frame(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Mac = [2, 0, 0, 0, 0, 1];
    const PHONE: Mac = [0xaa, 0xbb, 0xcc, 0, 0, 2];

    #[test]
    fn the_setup_request_names_the_phone_service_and_ours() {
        assert_eq!(setup_request(UUID_NAP, UUID_PANU), vec![1, 1, 2, 0x11, 0x16, 0x11, 0x15]);
        let p = parse(&setup_request(UUID_NAP, UUID_PANU), ME, PHONE).unwrap();
        assert_eq!(p, Packet::Control(Control::SetupRequest { dst: UUID_NAP, src: UUID_PANU }));
    }

    #[test]
    fn long_and_four_byte_uuids_are_read_as_the_same_services() {
        let mut v = vec![1, 1, 16];
        v.extend_from_slice(&long_uuid(UUID_NAP));
        v.extend_from_slice(&long_uuid(UUID_PANU));
        assert_eq!(parse(&v, ME, PHONE).unwrap(), Packet::Control(Control::SetupRequest { dst: UUID_NAP, src: UUID_PANU }));
        let w = vec![1, 1, 4, 0, 0, 0x11, 0x16, 0, 0, 0x11, 0x15];
        assert_eq!(parse(&w, ME, PHONE).unwrap(), Packet::Control(Control::SetupRequest { dst: UUID_NAP, src: UUID_PANU }));
    }

    #[test]
    fn a_general_frame_round_trips_and_the_compressed_forms_get_their_addresses() {
        let f = Frame { dst: PHONE, src: ME, proto: 0x0800, payload: vec![1, 2, 3] };
        let b = encode_frame(&f);
        assert_eq!(b.len(), 15 + 3);
        assert_eq!(parse(&b, ME, PHONE).unwrap(), Packet::Ethernet(f.clone()));
        let comp = [&[COMPRESSED][..], &0x0800u16.to_be_bytes(), &[1, 2, 3]].concat();
        assert_eq!(parse(&comp, ME, PHONE).unwrap(), Packet::Ethernet(Frame { dst: ME, src: PHONE, proto: 0x0800, payload: vec![1, 2, 3] }));
        let src_only = [&[COMPRESSED_SRC_ONLY][..], &[9, 9, 9, 9, 9, 9], &0x86ddu16.to_be_bytes(), &[7]].concat();
        assert_eq!(parse(&src_only, ME, PHONE).unwrap(), Packet::Ethernet(Frame { dst: ME, src: [9; 6], proto: 0x86dd, payload: vec![7] }));
        let dst_only = [&[COMPRESSED_DST_ONLY][..], &[8, 8, 8, 8, 8, 8], &0x0806u16.to_be_bytes(), &[5, 5]].concat();
        assert_eq!(parse(&dst_only, ME, PHONE).unwrap(), Packet::Ethernet(Frame { dst: [8; 6], src: PHONE, proto: 0x0806, payload: vec![5, 5] }));
    }

    #[test]
    fn extension_headers_are_skipped_and_short_packets_are_refused() {
        // compressed frame with one extension header (type 0 control extension, length 2) before the payload
        let b = [&[COMPRESSED | 0x80][..], &0x0800u16.to_be_bytes(), &[0x00, 2, 0xaa, 0xbb, 42]].concat();
        match parse(&b, ME, PHONE).unwrap() {
            Packet::Ethernet(f) => assert_eq!(f.payload, vec![42]),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse(&[], ME, PHONE), Err(Error::Short));
        assert_eq!(parse(&[GENERAL_ETHERNET, 1, 2], ME, PHONE), Err(Error::Short));
        assert_eq!(parse(&[0x55], ME, PHONE), Err(Error::Unknown(0x55)));
        assert_eq!(parse(&[CONTROL | 0x80, 1, 2, 0, 1, 1, 1, 1, 1, 1], ME, PHONE).map(|_| ()), Ok(()), "a control packet may carry the flag");
    }

    #[test]
    fn a_connection_comes_up_on_success_and_says_why_when_refused() {
        let (mut p, first) = Panu::start(ME, PHONE, UUID_NAP);
        assert_eq!(first, setup_request(UUID_NAP, UUID_PANU));
        let f = Frame { dst: PHONE, src: ME, proto: 0x0800, payload: vec![1] };
        assert!(p.send(&f).is_none(), "nothing is sent before the phone said yes");
        p.receive(&setup_response(0)).unwrap();
        assert_eq!(p.state, State::Up);
        assert_eq!(p.send(&f), Some(encode_frame(&f)));
        let (mut q, _) = Panu::start(ME, PHONE, UUID_NAP);
        q.receive(&setup_response(4)).unwrap();
        assert_eq!(q.state, State::Refused(4));
        assert!(setup_text(4).contains("tethering"));
    }

    #[test]
    fn frames_before_the_answer_are_dropped_and_after_it_delivered() {
        let (mut p, _) = Panu::start(ME, PHONE, UUID_NAP);
        let f = Frame { dst: ME, src: PHONE, proto: 0x0800, payload: vec![9] };
        assert_eq!(p.receive(&encode_frame(&f)).unwrap().deliver, None);
        p.receive(&setup_response(0)).unwrap();
        assert_eq!(p.receive(&encode_frame(&f)).unwrap().deliver, Some(f));
    }

    #[test]
    fn hostile_input_never_panics_and_never_reads_past_the_packet() {
        // every prefix of valid packets, every single-byte type and length, and a pseudo-random flood: errors or packets, never a panic
        let valid = [
            setup_request(UUID_NAP, UUID_PANU),
            setup_response(0),
            encode_frame(&Frame { dst: PHONE, src: ME, proto: 0x0800, payload: vec![1; 40] }),
            vec![COMPRESSED | 0x80, 8, 0, 0x80, 255, 1, 2, 3],
            vec![CONTROL, CTRL_FILTER_NET_SET, 0xff, 0xff, 1, 2],
        ];
        let (mut p, _) = Panu::start(ME, PHONE, UUID_NAP);
        for v in &valid {
            for n in 0..=v.len() {
                let _ = parse(&v[..n], ME, PHONE);
                let _ = p.receive(&v[..n]);
            }
        }
        for t in 0..=255u8 {
            for l in 0..=255u8 {
                let _ = parse(&[t, l, 255, l, t, 0, 0], ME, PHONE);
                let _ = parse(&[t | 0x80, 0x80, l, 0x80, 255], ME, PHONE);
            }
        }
        let mut x: u32 = 0x1234_5678;
        for _ in 0..4000 {
            let len = (x >> 24) as usize % 64;
            let data: Vec<u8> = (0..len).map(|_| { x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (x >> 16) as u8 }).collect();
            let _ = parse(&data, ME, PHONE);
            let _ = p.receive(&data);
        }
    }

    #[test]
    fn a_setup_request_from_the_phone_is_refused_and_a_late_response_cannot_end_a_working_link() {
        let (mut p, _) = Panu::start(ME, PHONE, UUID_NAP);
        let r = p.receive(&setup_request(UUID_PANU, UUID_NAP)).unwrap();
        assert_eq!(r.replies, vec![setup_response(4)]);
        p.receive(&setup_response(0)).unwrap();
        p.receive(&setup_response(4)).unwrap();
        assert_eq!(p.state, State::Up, "the first answer decides");
    }

    #[test]
    fn filters_are_accepted_and_unknown_control_messages_are_answered() {
        let (mut p, _) = Panu::start(ME, PHONE, UUID_NAP);
        let net = [CONTROL, CTRL_FILTER_NET_SET, 0, 4, 0x08, 0x00, 0x08, 0x00];
        assert_eq!(p.receive(&net).unwrap().replies, vec![vec![CONTROL, CTRL_FILTER_NET_RESPONSE, 0, 0]]);
        let multi = [CONTROL, CTRL_FILTER_MULTI_SET, 0, 0];
        assert_eq!(p.receive(&multi).unwrap().replies, vec![vec![CONTROL, CTRL_FILTER_MULTI_RESPONSE, 0, 0]]);
        assert_eq!(p.receive(&[CONTROL, 0x77]).unwrap().replies, vec![not_understood(0x77)]);
    }
}
