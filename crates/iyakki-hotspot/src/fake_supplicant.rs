//! A client (supplicant) side of the 4-way handshake, written from the same standard but not from the authenticator's code:
//! it reads and writes frames at raw byte offsets and checks the MIC with its own zero-and-hash routine. The authenticator tests
//! run against it, so a mistake common to the encoder and the decoder of the real code would show up as a disagreement.

use crate::kdf::{derive_ptk, Ptk};
use crate::{keywrap, Mac};
use iyakki_sec::wpa::hmac_sha1;

pub struct Supplicant {
    pmk: [u8; 32],
    aa: Mac,
    spa: Mac,
    snonce: [u8; 32],
    my_rsn_ie: Vec<u8>,
    ptk: Option<Ptk>,
    anonce: [u8; 32],
    last_replay: Option<u64>,
    installed: Option<([u8; 16], [u8; 16], u8)>,
    rsn_match: bool,
    beacon_rsn_ie: Vec<u8>,
}

fn mic_of(kck: &[u8; 16], frame: &[u8]) -> [u8; 16] {
    let mut z = frame.to_vec();
    z[81..97].fill(0);
    let h = hmac_sha1(kck, &z);
    let mut o = [0u8; 16];
    o.copy_from_slice(&h[..16]);
    o
}

impl Supplicant {
    /// `my_rsn_ie` is what the client puts in message 2; the real beacon element is assumed to be the standard one.
    pub fn new(pmk: [u8; 32], aa: Mac, spa: Mac, snonce: [u8; 32], my_rsn_ie: Vec<u8>) -> Supplicant {
        Supplicant { pmk, aa, spa, snonce, my_rsn_ie, ptk: None, anonce: [0; 32], last_replay: None, installed: None, rsn_match: false, beacon_rsn_ie: crate::eapol::rsn_ie_ccmp_psk() }
    }

    pub fn installed(&self) -> Option<([u8; 16], [u8; 16], u8)> {
        self.installed
    }

    pub fn saw_rsn_ie_match(&self) -> bool {
        self.rsn_match
    }

    fn frame(&self, info: u16, replay: u64, nonce: &[u8; 32], data: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; 99];
        f[0] = 1; // a client may answer with version 1
        f[1] = 3;
        f[2..4].copy_from_slice(&((95 + data.len()) as u16).to_be_bytes());
        f[4] = 2;
        f[5..7].copy_from_slice(&info.to_be_bytes());
        f[7..9].copy_from_slice(&16u16.to_be_bytes());
        f[9..17].copy_from_slice(&replay.to_be_bytes());
        f[17..49].copy_from_slice(nonce);
        f[97..99].copy_from_slice(&(data.len() as u16).to_be_bytes());
        f.extend_from_slice(data);
        if info & 0x100 != 0 {
            let kck = self.ptk.as_ref().map(|p| p.kck).unwrap_or([0; 16]);
            let m = mic_of(&kck, &f);
            f[81..97].copy_from_slice(&m);
        }
        f
    }

    /// Feed a frame from the authenticator; the answer, if any.
    pub fn on_frame(&mut self, b: &[u8]) -> Option<Vec<u8>> {
        if b.len() < 99 || b[1] != 3 || b[4] != 2 {
            return None;
        }
        let info = u16::from_be_bytes([b[5], b[6]]);
        let replay = u64::from_be_bytes(b[9..17].try_into().ok()?);
        let dlen = u16::from_be_bytes([b[97], b[98]]) as usize;
        let body = u16::from_be_bytes([b[2], b[3]]) as usize;
        if b.len() < 99 + dlen || body != 95 + dlen {
            return None;
        }
        let ack = info & 0x80 != 0;
        let has_mic = info & 0x100 != 0;
        if !ack {
            return None;
        }
        if self.last_replay.is_some_and(|l| replay <= l) {
            return None; // replay protection: only ever go up
        }
        if !has_mic {
            // message 1
            self.anonce.copy_from_slice(&b[17..49]);
            self.ptk = Some(derive_ptk(&self.pmk, &self.aa, &self.spa, &self.anonce, &self.snonce));
            self.last_replay = Some(replay);
            let ie = self.my_rsn_ie.clone();
            return Some(self.frame(0x010a, replay, &self.snonce.clone(), &ie));
        }
        // message 3
        let ptk = self.ptk.clone()?;
        if b[17..49] != self.anonce || mic_of(&ptk.kck, &b[..99 + dlen]) != b[81..97] {
            return None;
        }
        let kd = keywrap::unwrap(&ptk.kek, &b[99..99 + dlen])?;
        // walk the elements by hand
        let (mut i, mut ie, mut gtk) = (0usize, None, None);
        while i + 2 <= kd.len() {
            let (t, l) = (kd[i], kd[i + 1] as usize);
            if t == 0xdd && l == 0 {
                break;
            }
            let e = kd.get(i + 2..i + 2 + l)?;
            if t == 0x30 {
                ie = Some(kd[i..i + 2 + l].to_vec());
            } else if t == 0xdd && l == 22 && e[..4] == [0, 0x0f, 0xac, 1] {
                gtk = Some((e[4] & 3, e[6..22].to_vec()));
            }
            i += 2 + l;
        }
        self.rsn_match = ie.as_deref() == Some(&self.beacon_rsn_ie[..]);
        if !self.rsn_match {
            return None; // downgrade: the element differs from the beacon's
        }
        let (gid, g) = gtk?;
        self.last_replay = Some(replay);
        self.installed = Some((ptk.tk, g.try_into().ok()?, gid));
        Some(self.frame(0x030a, replay, &[0u8; 32], &[]))
    }
}
