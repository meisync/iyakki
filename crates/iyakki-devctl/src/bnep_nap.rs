//! The sharing side of Bluetooth tethering (NAP, F-HOTSPOT-15): phones connect to this computer and get its internet. Only the
//! pure parts are here: the server end of one BNEP connection, the learning bridge that moves Ethernet frames between the phones
//! and the machine's TAP interface, and the SDP record that announces the service. No sockets: the L2CAP listener on PSM 15, the
//! TAP device, the SDP registration and the pairing are not written (they need a real phone and kernel options).
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use crate::bnep::{self, Control, Error, Frame, Mac, Outcome, Packet, UUID_NAP, UUID_PANU};

/// Most phones one Bluetooth piconet carries.
pub const MAX_PHONES: usize = 7;
const TABLE_MAX: usize = 256;
const AGE_OUT_SECS: u64 = 300;

// BNEP setup response codes (Bluetooth BNEP specification, section 2.6.2).
pub const RESP_OK: u16 = 0x0000;
pub const RESP_BAD_DST: u16 = 0x0001;
pub const RESP_BAD_SRC: u16 = 0x0002;
pub const RESP_NOT_ALLOWED: u16 = 0x0004;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NapState {
    Waiting,
    Up,
}

/// One phone's connection seen from the server side.
pub struct NapSession {
    pub state: NapState,
    local: Mac,
    remote: Mac,
    allowed: bool,
}

impl NapSession {
    /// `allowed` is false while sharing over Bluetooth is switched off or the phone is not a trusted (paired) one: every request
    /// is then answered "not allowed".
    pub fn new(local: Mac, remote: Mac, allowed: bool) -> NapSession {
        NapSession { state: NapState::Waiting, local, remote, allowed }
    }

    pub fn receive(&mut self, data: &[u8]) -> Result<Outcome, Error> {
        let mut out = Outcome::default();
        match bnep::parse(data, self.local, self.remote) {
            Ok(Packet::Ethernet(f)) => {
                if self.state == NapState::Up {
                    out.deliver = Some(f);
                }
            }
            Ok(Packet::Control(Control::SetupRequest { dst, src })) => {
                let code = if !self.allowed {
                    RESP_NOT_ALLOWED
                } else if dst != UUID_NAP {
                    RESP_BAD_DST
                } else if src != UUID_PANU {
                    RESP_BAD_SRC
                } else {
                    RESP_OK
                };
                if code == RESP_OK {
                    self.state = NapState::Up;
                }
                out.replies.push(bnep::setup_response(code));
            }
            // a phone that asks for the setup only answers ours when we asked: never here
            Ok(Packet::Control(Control::SetupResponse(_))) => {}
            Ok(Packet::Control(c)) => out.replies.extend(bnep::answer(&c)),
            Err(Error::Unknown(t)) if data.first().is_some_and(|b| b & 0x7f == 0x01) => out.replies.push(bnep::not_understood(t)),
            Err(e) => return Err(e),
        }
        Ok(out)
    }

    /// The packet for a frame to send to this phone; `None` until it has been accepted.
    pub fn send(&self, f: &Frame) -> Option<Vec<u8>> {
        (self.state == NapState::Up).then(|| bnep::encode_frame(f))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Port {
    /// The machine's TAP interface (towards the rest of the computer and the internet).
    Tap,
    Phone(u8),
}

/// A learning bridge: remembers which port each Ethernet address was last seen on and sends a frame only where it belongs, or to
/// every other port when the address is not known (or is a broadcast). Phones never talk to each other through it unless asked.
pub struct Bridge {
    phones: Vec<u8>,
    table: HashMap<Mac, (Port, u64)>,
    pub phones_see_each_other: bool,
}

fn multicast(m: &Mac) -> bool {
    m[0] & 1 == 1
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge::new()
    }
}

impl Bridge {
    pub fn new() -> Bridge {
        Bridge { phones: Vec::new(), table: HashMap::new(), phones_see_each_other: false }
    }

    /// A phone connected: its port number, or `None` when seven are already connected.
    pub fn add_phone(&mut self) -> Option<u8> {
        if self.phones.len() >= MAX_PHONES {
            return None;
        }
        let id = (0..=u8::MAX).find(|n| !self.phones.contains(n))?;
        self.phones.push(id);
        Some(id)
    }

    /// A phone left: its port and every address learned on it are forgotten.
    pub fn remove_phone(&mut self, id: u8) {
        self.phones.retain(|p| *p != id);
        self.table.retain(|_, (p, _)| *p != Port::Phone(id));
    }

    fn ports_except(&self, from: Port) -> Vec<Port> {
        let mut v = Vec::new();
        if from != Port::Tap {
            v.push(Port::Tap);
        }
        for p in &self.phones {
            let port = Port::Phone(*p);
            if port != from && (self.phones_see_each_other || from == Port::Tap) {
                v.push(port);
            }
        }
        v
    }

    /// Decide where a frame that arrived on `from` goes. `now` is in seconds (any steady clock).
    pub fn forward(&mut self, from: Port, f: &Frame, now: u64) -> Vec<Port> {
        if let Port::Phone(p) = from {
            if !self.phones.contains(&p) {
                return Vec::new();
            }
        }
        self.table.retain(|_, (_, t)| now.saturating_sub(*t) < AGE_OUT_SECS);
        if !multicast(&f.src) {
            if self.table.len() >= TABLE_MAX && !self.table.contains_key(&f.src) {
                if let Some(oldest) = self.table.iter().min_by_key(|(_, (_, t))| *t).map(|(m, _)| *m) {
                    self.table.remove(&oldest);
                }
            }
            self.table.insert(f.src, (from, now));
        }
        if !multicast(&f.dst) {
            if let Some((p, _)) = self.table.get(&f.dst).copied() {
                if p == from {
                    return Vec::new();
                }
                let allowed = from == Port::Tap || p == Port::Tap || self.phones_see_each_other;
                return if allowed { vec![p] } else { Vec::new() };
            }
        }
        self.ports_except(from)
    }
}

const PSM_BNEP: u16 = 0x000f;
/// The record handle the SDP server gives the NAP record.
pub const SDP_NAP_HANDLE: u32 = 0x0001_0004;

/// The SDP record that announces this computer as a network access point (service class NAP, L2CAP PSM 15, BNEP 1.0 carrying IPv4,
/// ARP and IPv6). Attributes in id order. Not yet compared with a phone's view of it.
pub fn sdp_record(handle: u32, name: &str, description: &str) -> Vec<(u16, Vec<u8>)> {
    use crate::a2dp::{sdp_seq as seq, sdp_text, sdp_uint16 as u16e, sdp_uint32 as u32e, sdp_uuid16 as uuid};
    vec![
        (0x0000, u32e(handle)),
        (0x0001, seq(&[uuid(UUID_NAP)])),
        (0x0004, seq(&[seq(&[uuid(0x0100), u16e(PSM_BNEP)]), seq(&[uuid(0x000f), u16e(0x0100), seq(&[u16e(0x0800), u16e(0x0806), u16e(0x86dd)])])])),
        (0x0005, seq(&[uuid(0x1002)])),
        (0x0008, vec![0x08, 0xff]),
        (0x0009, seq(&[seq(&[uuid(UUID_NAP), u16e(0x0100)])])),
        (0x0100, sdp_text(name)),
        (0x0101, sdp_text(description)),
        // net access type: 0xfffe (other), maximum rate 0 (unknown)
        (0x030a, u16e(0xfffe)),
        (0x030b, u32e(0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Mac = [2, 0, 0, 0, 0, 1];
    const P1: Mac = [0xaa, 0, 0, 0, 0, 1];
    const P2: Mac = [0xaa, 0, 0, 0, 0, 2];
    const HOST: Mac = [2, 0, 0, 0, 0, 9];

    fn fr(dst: Mac, src: Mac) -> Frame {
        Frame { dst, src, proto: 0x0800, payload: vec![1, 2, 3] }
    }

    #[test]
    fn a_phone_asking_for_the_access_point_service_is_accepted_and_every_other_request_is_answered_with_a_reason() {
        let mut s = NapSession::new(ME, P1, true);
        assert!(s.send(&fr(P1, HOST)).is_none() && s.receive(&bnep::encode_frame(&fr(HOST, P1))).unwrap().deliver.is_none(), "nothing flows before the setup");
        assert_eq!(s.receive(&bnep::setup_request(UUID_NAP, UUID_PANU)).unwrap().replies, vec![vec![1, 2, 0, 0]]);
        assert_eq!(s.state, NapState::Up);
        assert_eq!(s.receive(&bnep::encode_frame(&fr(HOST, P1))).unwrap().deliver, Some(fr(HOST, P1)));
        assert_eq!(s.send(&fr(P1, HOST)), Some(bnep::encode_frame(&fr(P1, HOST))));
        for (dst, src, code) in [(UUID_PANU, UUID_PANU, RESP_BAD_DST), (UUID_NAP, UUID_NAP, RESP_BAD_SRC)] {
            let mut t = NapSession::new(ME, P1, true);
            assert_eq!(t.receive(&bnep::setup_request(dst, src)).unwrap().replies, vec![bnep::setup_response(code)]);
            assert_eq!(t.state, NapState::Waiting);
        }
        let mut off = NapSession::new(ME, P1, false);
        assert_eq!(off.receive(&bnep::setup_request(UUID_NAP, UUID_PANU)).unwrap().replies, vec![vec![1, 2, 0, 4]]);
        assert_eq!(off.state, NapState::Waiting);
        // filters are accepted, an unknown control message is answered, a stray setup response is ignored, a short packet is an error
        assert_eq!(s.receive(&[1, 3, 0, 0]).unwrap().replies.len(), 1);
        assert_eq!(s.receive(&[1, 0x7e]).unwrap().replies, vec![bnep::not_understood(0x7e)]);
        assert!(s.receive(&bnep::setup_response(0)).unwrap().replies.is_empty());
        assert_eq!(s.receive(&[]), Err(Error::Short));
    }

    #[test]
    fn the_bridge_learns_addresses_and_phones_are_kept_apart() {
        let mut b = Bridge::new();
        let (a, c) = (b.add_phone().unwrap(), b.add_phone().unwrap());
        assert_eq!((a, c), (0, 1));
        // an unknown destination goes everywhere except where it came from; phones do not see each other
        assert_eq!(b.forward(Port::Phone(a), &fr(HOST, P1), 10), vec![Port::Tap]);
        assert_eq!(b.forward(Port::Tap, &fr(P1, HOST), 11), vec![Port::Phone(a)], "now known");
        assert_eq!(b.forward(Port::Tap, &fr(P2, HOST), 12), vec![Port::Phone(a), Port::Phone(c)], "unknown: flooded from the machine");
        assert_eq!(b.forward(Port::Phone(c), &fr(P1, P2), 13), Vec::<Port>::new(), "phone to phone is refused");
        assert_eq!(b.forward(Port::Phone(a), &fr([0xff; 6], P1), 14), vec![Port::Tap], "a broadcast from a phone reaches the machine only");
        b.phones_see_each_other = true;
        assert_eq!(b.forward(Port::Phone(c), &fr(P1, P2), 15), vec![Port::Phone(a)]);
        // a frame to the port it came from is dropped; a multicast source is never learned
        assert!(b.forward(Port::Phone(a), &fr(P1, P1), 16).is_empty());
        let _ = b.forward(Port::Phone(a), &fr(HOST, [0x01, 0, 0, 0, 0, 5]), 17);
        assert!(!b.table.contains_key(&[0x01, 0, 0, 0, 0, 5]));
    }

    #[test]
    fn at_most_seven_phones_gone_phones_are_forgotten_and_old_addresses_age_out() {
        let mut b = Bridge::new();
        let ids: Vec<u8> = (0..7).map(|_| b.add_phone().unwrap()).collect();
        assert!(b.add_phone().is_none());
        b.forward(Port::Phone(ids[2]), &fr(HOST, P1), 0);
        b.remove_phone(ids[2]);
        assert!(b.table.is_empty());
        assert_eq!(b.add_phone(), Some(2), "a free number is reused");
        assert!(b.forward(Port::Phone(99), &fr(HOST, P1), 1).is_empty(), "frames from an unknown port are dropped");
        b.forward(Port::Phone(0), &fr(HOST, P1), 100);
        b.forward(Port::Tap, &fr(P2, HOST), 100 + AGE_OUT_SECS);
        assert!(!b.table.contains_key(&P1), "the old entry aged out");
        for n in 0..300u32 {
            let m = [2, 0, 0, (n >> 8) as u8, n as u8, 7];
            b.forward(Port::Tap, &fr(P1, m), 1000 + u64::from(n));
        }
        assert!(b.table.len() <= TABLE_MAX);
    }

    #[test]
    fn the_sdp_record_names_the_service_the_transport_and_the_network_types() {
        let r = sdp_record(SDP_NAP_HANDLE, "IYAKKI network", "Internet through this computer");
        let ids: Vec<u16> = r.iter().map(|(i, _)| *i).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "attributes in id order");
        assert_eq!(r[1].1, vec![0x35, 0x03, 0x19, 0x11, 0x16]);
        let proto = &r[2].1;
        assert!(proto.windows(4).any(|w| w == [0x19, 0x01, 0x00, 0x09]) && proto.windows(3).any(|w| w == [0x09, 0x00, 0x0f]), "L2CAP with PSM 15");
        assert!(proto.windows(9).any(|w| w == [0x09, 0x08, 0x00, 0x09, 0x08, 0x06, 0x09, 0x86, 0xdd]), "IPv4, ARP and IPv6");
        assert_eq!(r[4].1, vec![0x08, 0xff]);
    }
}
