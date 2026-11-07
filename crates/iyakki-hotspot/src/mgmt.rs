//! IEEE 802.11 management frames for an access point that does its own authentication and association (open system
//! authentication, then WPA2 keys through the 4-way handshake): parse the client's authentication and association requests,
//! build the answers, the beacon pieces handed to the kernel (head and tail, the kernel adds the TIM), the probe response and
//! the de-authentication. All pure functions over bytes; short or damaged frames give `None`.

use crate::eapol;
use crate::Mac;

pub const SUBTYPE_ASSOC_REQ: u8 = 0x00;
pub const SUBTYPE_ASSOC_RESP: u8 = 0x10;
pub const SUBTYPE_PROBE_REQ: u8 = 0x40;
pub const SUBTYPE_PROBE_RESP: u8 = 0x50;
pub const SUBTYPE_BEACON: u8 = 0x80;
pub const SUBTYPE_DISASSOC: u8 = 0xa0;
pub const SUBTYPE_AUTH: u8 = 0xb0;
pub const SUBTYPE_DEAUTH: u8 = 0xc0;

pub const STATUS_OK: u16 = 0;
pub const STATUS_UNSPECIFIED: u16 = 1;
pub const STATUS_ALGO_UNSUPPORTED: u16 = 13;
pub const STATUS_AUTH_SEQ: u16 = 14;
pub const STATUS_AP_FULL: u16 = 17;
pub const STATUS_CIPHER_REJECTED: u16 = 24;
pub const REASON_UNSPECIFIED: u16 = 1;
pub const REASON_4WAY_TIMEOUT: u16 = 15;
pub const REASON_AP_STOPPING: u16 = 3;

pub const HEADER_LEN: usize = 24;
pub const BROADCAST: Mac = [0xff; 6];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    G24,
    A5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The first frame-control byte (type and subtype); management frames have the type bits 0.
    pub fc0: u8,
    pub da: Mac,
    pub sa: Mac,
    pub bssid: Mac,
}

pub fn parse_header(f: &[u8]) -> Option<Header> {
    if f.len() < HEADER_LEN || f[0] & 0x0c != 0 || f[0] & 0x03 != 0 {
        return None;
    }
    Some(Header { fc0: f[0] & 0xfc, da: f[4..10].try_into().ok()?, sa: f[10..16].try_into().ok()?, bssid: f[16..22].try_into().ok()? })
}

fn header(fc0: u8, da: Mac, sa: Mac, bssid: Mac) -> Vec<u8> {
    let mut h = vec![fc0, 0, 0, 0];
    h.extend_from_slice(&da);
    h.extend_from_slice(&sa);
    h.extend_from_slice(&bssid);
    h.extend_from_slice(&[0, 0]);
    h
}

/// Walk information elements; `None` if one runs past the end.
pub fn elements(mut b: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        let (id, len) = (*b.first()?, *b.get(1)? as usize);
        v.push((id, b.get(2..2 + len)?));
        b = &b[2 + len..];
    }
    Some(v)
}

fn rates(band: Band) -> (Vec<u8>, Vec<u8>) {
    match band {
        Band::G24 => (vec![0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24], vec![0x30, 0x48, 0x60, 0x6c]),
        Band::A5 => (vec![0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c], vec![]),
    }
}

fn capabilities(band: Band) -> u16 {
    match band {
        Band::G24 => 0x0431, // ESS, privacy, short preamble, short slot
        Band::A5 => 0x0411,
    }
}

fn ie(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    out.push(body.len() as u8);
    out.extend_from_slice(body);
}

/// Beacon up to (not including) the TIM: header, timestamp, interval, capabilities, SSID, rates, channel.
pub fn beacon_head(bssid: Mac, ssid: &[u8], interval_tu: u16, band: Band, channel: u8) -> Vec<u8> {
    let mut f = header(SUBTYPE_BEACON, BROADCAST, bssid, bssid);
    f.extend_from_slice(&[0u8; 8]);
    f.extend_from_slice(&interval_tu.to_le_bytes());
    f.extend_from_slice(&capabilities(band).to_le_bytes());
    ie(&mut f, 0, ssid);
    let (r, ext) = rates(band);
    ie(&mut f, 1, &r);
    if band == Band::G24 {
        ie(&mut f, 3, &[channel]);
    }
    let _ = ext;
    f
}

/// Beacon after the TIM: ERP and extended rates (2.4 GHz), then the RSN element.
pub fn beacon_tail(band: Band) -> Vec<u8> {
    let mut f = Vec::new();
    let (_, ext) = rates(band);
    if band == Band::G24 {
        ie(&mut f, 42, &[0]);
    }
    if !ext.is_empty() {
        ie(&mut f, 50, &ext);
    }
    f.extend_from_slice(&eapol::rsn_ie_ccmp_psk());
    f
}

/// The probe response template given to the kernel: the beacon without TIM, as a probe response (the receiver address is
/// filled in by the driver).
pub fn probe_response(bssid: Mac, ssid: &[u8], interval_tu: u16, band: Band, channel: u8) -> Vec<u8> {
    let mut f = beacon_head(bssid, ssid, interval_tu, band, channel);
    f[0] = SUBTYPE_PROBE_RESP;
    f.extend(beacon_tail(band));
    f
}

/// Authentication answer to an open-system request (algorithm 0, sequence 1). Anything else gets a refusal, still an answer.
pub fn auth_response(req: &[u8], bssid: Mac) -> Option<Vec<u8>> {
    let h = parse_header(req)?;
    if h.fc0 != SUBTYPE_AUTH || h.bssid != bssid || h.da != bssid || req.len() < HEADER_LEN + 6 {
        return None;
    }
    let b = &req[HEADER_LEN..];
    let (algo, seq) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
    let status = if algo != 0 {
        STATUS_ALGO_UNSUPPORTED
    } else if seq != 1 {
        STATUS_AUTH_SEQ
    } else {
        STATUS_OK
    };
    let mut f = header(SUBTYPE_AUTH, h.sa, bssid, bssid);
    f.extend_from_slice(&algo.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    Some(f)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocRequest {
    pub sta: Mac,
    pub capability: u16,
    pub listen_interval: u16,
    pub ssid: Vec<u8>,
    pub rates: Vec<u8>,
    pub rsn_ie: Option<Vec<u8>>,
}

pub fn parse_assoc_request(f: &[u8], bssid: Mac) -> Option<AssocRequest> {
    let h = parse_header(f)?;
    if h.fc0 != SUBTYPE_ASSOC_REQ || h.bssid != bssid || h.da != bssid || f.len() < HEADER_LEN + 4 {
        return None;
    }
    let b = &f[HEADER_LEN..];
    let mut r = AssocRequest { sta: h.sa, capability: u16::from_le_bytes([b[0], b[1]]), listen_interval: u16::from_le_bytes([b[2], b[3]]), ssid: vec![], rates: vec![], rsn_ie: None };
    let mut full = false;
    for (id, body) in elements(&b[4..])? {
        match id {
            0 => {
                r.ssid = body.to_vec();
                full = true;
            }
            1 | 50 => r.rates.extend_from_slice(body),
            48 => {
                let mut v = vec![48, body.len() as u8];
                v.extend_from_slice(body);
                r.rsn_ie = Some(v);
            }
            _ => {}
        }
    }
    full.then_some(r)
}

/// What to answer an association request: the status for it.
pub fn assoc_status(r: &AssocRequest, ssid: &[u8], stations_now: usize, max_clients: usize) -> u16 {
    if r.ssid != ssid {
        return STATUS_UNSPECIFIED;
    }
    if !r.rsn_ie.as_deref().is_some_and(eapol::rsn_ie_is_ccmp_psk) {
        return STATUS_CIPHER_REJECTED;
    }
    if stations_now >= max_clients {
        return STATUS_AP_FULL;
    }
    STATUS_OK
}

/// The association response. The association id is 1 to 2007; it is sent with the two top bits set.
pub fn assoc_response(sta: Mac, bssid: Mac, band: Band, status: u16, aid: u16) -> Vec<u8> {
    let mut f = header(SUBTYPE_ASSOC_RESP, sta, bssid, bssid);
    f.extend_from_slice(&capabilities(band).to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    f.extend_from_slice(&((aid & 0x3fff) | 0xc000).to_le_bytes());
    let (r, ext) = rates(band);
    ie(&mut f, 1, &r);
    if !ext.is_empty() {
        ie(&mut f, 50, &ext);
    }
    f
}

pub fn deauth(sta: Mac, bssid: Mac, reason: u16) -> Vec<u8> {
    let mut f = header(SUBTYPE_DEAUTH, sta, bssid, bssid);
    f.extend_from_slice(&reason.to_le_bytes());
    f
}

/// The lowest association id not in use (1-based), or `None` when there are 2007.
pub fn free_aid(used: &[u16]) -> Option<u16> {
    (1..=2007u16).find(|a| !used.contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    const AP: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    const ST: Mac = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];

    fn auth_req(algo: u16, seq: u16) -> Vec<u8> {
        let mut f = header(SUBTYPE_AUTH, AP, ST, AP);
        f.extend_from_slice(&algo.to_le_bytes());
        f.extend_from_slice(&seq.to_le_bytes());
        f.extend_from_slice(&[0, 0]);
        f
    }

    fn assoc_req(ssid: &[u8], rsn: Option<Vec<u8>>) -> Vec<u8> {
        let mut f = header(SUBTYPE_ASSOC_REQ, AP, ST, AP);
        f.extend_from_slice(&[0x31, 0x04, 10, 0]);
        ie(&mut f, 0, ssid);
        ie(&mut f, 1, &[0x82, 0x84]);
        ie(&mut f, 50, &[0x30]);
        if let Some(r) = rsn {
            f.extend(r);
        }
        f
    }

    #[test]
    fn the_beacon_pieces_have_the_documented_bytes() {
        let head = beacon_head(AP, b"IYAKKI-1A2B", 100, Band::G24, 6);
        // header | timestamp | interval 100 | capability 0x0431 | SSID | rates | DS parameter set
        let expect = "80000000 ffffffffffff 021122334455 021122334455 0000 0000000000000000 6400 3104 000b4959414b4b492d314132 42 010882848b960c121824 030106".replace(' ', "");
        assert_eq!(hex(&head), expect);
        // ERP | extended rates | RSN
        assert_eq!(hex(&beacon_tail(Band::G24)), "2a0100 32043048606c 30140100000fac040100000fac040100000fac020000".replace(' ', ""));
        // 5 GHz: no ERP, no DS parameter, no extended rates, no short preamble
        assert_eq!(hex(&beacon_tail(Band::A5)), "30140100000fac040100000fac040100000fac020000");
        let h5 = beacon_head(AP, b"x", 100, Band::A5, 36);
        assert_eq!(&h5[34..36], &[0x11, 0x04]);
        assert!(!h5.windows(2).any(|w| w == [3, 1]));
    }

    #[test]
    fn the_probe_response_is_the_beacon_without_a_tim_and_with_its_own_subtype() {
        let p = probe_response(AP, b"net", 100, Band::G24, 1);
        assert_eq!(p[0], SUBTYPE_PROBE_RESP);
        let mut b = beacon_head(AP, b"net", 100, Band::G24, 1);
        b.extend(beacon_tail(Band::G24));
        assert_eq!(&p[1..], &b[1..]);
        assert_eq!(parse_header(&p).unwrap().fc0, SUBTYPE_PROBE_RESP);
    }

    #[test]
    fn open_system_authentication_is_answered_and_anything_else_is_refused_with_a_status() {
        let r = auth_response(&auth_req(0, 1), AP).unwrap();
        assert_eq!(hex(&r), "b0000000 02aabbccddee 021122334455 021122334455 0000 0000 0200 0000".replace(' ', ""));
        let status = |r: &[u8]| u16::from_le_bytes([r[28], r[29]]);
        assert_eq!(status(&auth_response(&auth_req(3, 1), AP).unwrap()), STATUS_ALGO_UNSUPPORTED);
        assert_eq!(status(&auth_response(&auth_req(0, 3), AP).unwrap()), STATUS_AUTH_SEQ);
        // for another BSS, too short, not an authentication
        assert!(auth_response(&auth_req(0, 1), [9; 6]).is_none());
        assert!(auth_response(&auth_req(0, 1)[..28], AP).is_none());
        assert!(auth_response(&deauth(AP, ST, 1), AP).is_none());
    }

    #[test]
    fn an_association_request_is_parsed_and_judged() {
        let good = assoc_req(b"net", Some(eapol::rsn_ie_ccmp_psk()));
        let r = parse_assoc_request(&good, AP).unwrap();
        assert_eq!((r.sta, r.capability, r.listen_interval, r.ssid.as_slice()), (ST, 0x0431, 10, &b"net"[..]));
        assert_eq!(r.rates, vec![0x82, 0x84, 0x30]);
        assert_eq!(assoc_status(&r, b"net", 0, 8), STATUS_OK);
        assert_eq!(assoc_status(&r, b"other", 0, 8), STATUS_UNSPECIFIED);
        assert_eq!(assoc_status(&r, b"net", 8, 8), STATUS_AP_FULL);
        let open = parse_assoc_request(&assoc_req(b"net", None), AP).unwrap();
        assert_eq!(assoc_status(&open, b"net", 0, 8), STATUS_CIPHER_REJECTED, "no encryption is not allowed");
        let mut tkip = eapol::rsn_ie_ccmp_psk();
        tkip[13] = 2;
        let t = parse_assoc_request(&assoc_req(b"net", Some(tkip)), AP).unwrap();
        assert_eq!(assoc_status(&t, b"net", 0, 8), STATUS_CIPHER_REJECTED);
        // damaged: an element longer than the frame; no SSID at all; wrong BSS
        let mut bad = good.clone();
        bad.truncate(bad.len() - 3);
        assert!(parse_assoc_request(&bad, AP).is_none());
        let mut nossid = header(SUBTYPE_ASSOC_REQ, AP, ST, AP);
        nossid.extend_from_slice(&[0x31, 4, 10, 0]);
        assert!(parse_assoc_request(&nossid, AP).is_none());
        assert!(parse_assoc_request(&good, [1; 6]).is_none());
    }

    #[test]
    fn the_association_response_carries_the_status_and_the_aid_with_the_top_bits_set() {
        let r = assoc_response(ST, AP, Band::G24, STATUS_OK, 1);
        assert_eq!(r[0], SUBTYPE_ASSOC_RESP);
        assert_eq!(&r[4..10], &ST);
        assert_eq!(hex(&r[24..30]), "31040000" .to_string() + "01c0");
        assert_eq!(hex(&r[30..]), "010882848b960c121824" .to_string() + "32043048606c");
    }

    #[test]
    fn aids_deauth_and_junk() {
        assert_eq!(free_aid(&[]), Some(1));
        assert_eq!(free_aid(&[1, 2, 4]), Some(3));
        assert_eq!(free_aid(&(1..=2007).collect::<Vec<u16>>()), None);
        let d = deauth(ST, AP, REASON_4WAY_TIMEOUT);
        assert_eq!(hex(&d), "c0000000 02aabbccddee 021122334455 021122334455 0000 0f00".replace(' ', ""));
        let mut s = 5u32;
        for n in 0..4000usize {
            let v: Vec<u8> = (0..n % 90)
                .map(|_| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    (s >> 24) as u8
                })
                .collect();
            let _ = parse_header(&v);
            let _ = auth_response(&v, AP);
            let _ = parse_assoc_request(&v, AP);
            let _ = elements(&v);
        }
    }
}
