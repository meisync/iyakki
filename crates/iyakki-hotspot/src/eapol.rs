//! EAPOL-Key frames (IEEE 802.1X-2004 clause 7.6, IEEE 802.11i clause 8.5.2) for WPA2-personal with CCMP: encode, decode, the
//! MIC (HMAC-SHA1 cut to 16 bytes, key descriptor version 2), the RSN information element and the key data (RSN IE, GTK KDE).
//! Decoding never panics and never trusts a length it has not checked.

use iyakki_sec::wpa::hmac_sha1;

pub const EAPOL_TYPE_KEY: u8 = 3;
pub const DESCRIPTOR_RSN: u8 = 2;
/// Key descriptor version 2: HMAC-SHA1-128 MIC and AES key wrap.
pub const KEYVER_AES: u16 = 2;

pub const KI_VERSION_MASK: u16 = 0x0007;
pub const KI_PAIRWISE: u16 = 1 << 3;
pub const KI_INSTALL: u16 = 1 << 6;
pub const KI_ACK: u16 = 1 << 7;
pub const KI_MIC: u16 = 1 << 8;
pub const KI_SECURE: u16 = 1 << 9;
pub const KI_ERROR: u16 = 1 << 10;
pub const KI_REQUEST: u16 = 1 << 11;
pub const KI_ENCRYPTED: u16 = 1 << 12;

/// Message 1: authenticator to client (nonce).
pub const INFO_M1: u16 = KEYVER_AES | KI_PAIRWISE | KI_ACK;
/// Message 2: client to authenticator (nonce, RSN IE, MIC).
pub const INFO_M2: u16 = KEYVER_AES | KI_PAIRWISE | KI_MIC;
/// Message 3: authenticator to client (install, wrapped group key, MIC).
pub const INFO_M3: u16 = KEYVER_AES | KI_PAIRWISE | KI_INSTALL | KI_ACK | KI_MIC | KI_SECURE | KI_ENCRYPTED;
/// Message 4: client to authenticator (done, MIC).
pub const INFO_M4: u16 = KEYVER_AES | KI_PAIRWISE | KI_MIC | KI_SECURE;

const HEADER: usize = 4;
const FIXED: usize = 95;
const MIC_AT: usize = 81;
/// The smallest EAPOL-Key frame (no key data), header included.
pub const MIN_FRAME: usize = HEADER + FIXED;
/// Key data bigger than this is refused (a real RSN IE plus GTK is under 100 bytes).
pub const MAX_KEY_DATA: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EapolError {
    TooShort,
    NotKeyFrame,
    BadDescriptor,
    BadLength,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFrame {
    pub version: u8,
    pub key_info: u16,
    pub key_len: u16,
    pub replay: u64,
    pub nonce: [u8; 32],
    pub iv: [u8; 16],
    pub rsc: [u8; 8],
    pub mic: [u8; 16],
    pub data: Vec<u8>,
}

impl KeyFrame {
    pub fn new(key_info: u16, replay: u64) -> KeyFrame {
        KeyFrame { version: 2, key_info, key_len: 16, replay, nonce: [0; 32], iv: [0; 16], rsc: [0; 8], mic: [0; 16], data: Vec::new() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(MIN_FRAME + self.data.len());
        o.push(self.version);
        o.push(EAPOL_TYPE_KEY);
        o.extend_from_slice(&((FIXED + self.data.len()) as u16).to_be_bytes());
        o.push(DESCRIPTOR_RSN);
        o.extend_from_slice(&self.key_info.to_be_bytes());
        o.extend_from_slice(&self.key_len.to_be_bytes());
        o.extend_from_slice(&self.replay.to_be_bytes());
        o.extend_from_slice(&self.nonce);
        o.extend_from_slice(&self.iv);
        o.extend_from_slice(&self.rsc);
        o.extend_from_slice(&[0u8; 8]);
        o.extend_from_slice(&self.mic);
        o.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        o.extend_from_slice(&self.data);
        o
    }

    pub fn decode(b: &[u8]) -> Result<KeyFrame, EapolError> {
        if b.len() < MIN_FRAME {
            return Err(EapolError::TooShort);
        }
        if b[1] != EAPOL_TYPE_KEY {
            return Err(EapolError::NotKeyFrame);
        }
        let body = u16::from_be_bytes([b[2], b[3]]) as usize;
        if body < FIXED || HEADER + body > b.len() {
            return Err(EapolError::BadLength);
        }
        if b[4] != DESCRIPTOR_RSN {
            return Err(EapolError::BadDescriptor);
        }
        let dl = u16::from_be_bytes([b[97], b[98]]) as usize;
        if dl > MAX_KEY_DATA || FIXED + dl != body {
            return Err(EapolError::BadLength);
        }
        let t = |a: usize, n: usize| &b[a..a + n];
        Ok(KeyFrame {
            version: b[0],
            key_info: u16::from_be_bytes([b[5], b[6]]),
            key_len: u16::from_be_bytes([b[7], b[8]]),
            replay: u64::from_be_bytes(t(9, 8).try_into().map_err(|_| EapolError::TooShort)?),
            nonce: t(17, 32).try_into().map_err(|_| EapolError::TooShort)?,
            iv: t(49, 16).try_into().map_err(|_| EapolError::TooShort)?,
            rsc: t(65, 8).try_into().map_err(|_| EapolError::TooShort)?,
            mic: t(MIC_AT, 16).try_into().map_err(|_| EapolError::TooShort)?,
            data: b[99..99 + dl].to_vec(),
        })
    }
}

/// The MIC of an encoded frame: HMAC-SHA1 over the frame with the MIC field zeroed, first 16 bytes.
pub fn compute_mic(kck: &[u8; 16], frame: &[u8]) -> Option<[u8; 16]> {
    if frame.len() < MIN_FRAME {
        return None;
    }
    let mut z = frame.to_vec();
    z[MIC_AT..MIC_AT + 16].fill(0);
    let full = hmac_sha1(kck, &z);
    full[..16].try_into().ok()
}

/// Put the MIC into an encoded frame.
pub fn sign(kck: &[u8; 16], frame: &mut [u8]) {
    if let Some(m) = compute_mic(kck, frame) {
        frame[MIC_AT..MIC_AT + 16].copy_from_slice(&m);
    }
}

/// Is the MIC of this encoded frame right? Only the bytes the length field covers are hashed.
pub fn verify_mic(kck: &[u8; 16], frame: &[u8]) -> bool {
    if frame.len() < MIN_FRAME {
        return false;
    }
    let body = u16::from_be_bytes([frame[2], frame[3]]) as usize;
    let end = (HEADER + body).min(frame.len());
    match compute_mic(kck, &frame[..end]) {
        Some(m) => crate::ct_eq(&m, &frame[MIC_AT..MIC_AT + 16]),
        None => false,
    }
}

const OUI: [u8; 3] = [0x00, 0x0f, 0xac];

/// The RSN element for WPA2-personal with CCMP only: group CCMP, pairwise CCMP, key management PSK, no capabilities.
pub fn rsn_ie_ccmp_psk() -> Vec<u8> {
    let mut v = vec![0x30, 20, 1, 0];
    v.extend_from_slice(&[0, 0x0f, 0xac, 4]);
    v.extend_from_slice(&[1, 0, 0, 0x0f, 0xac, 4]);
    v.extend_from_slice(&[1, 0, 0, 0x0f, 0xac, 2]);
    v.extend_from_slice(&[0, 0]);
    v
}

/// Is `ie` an RSN element that offers exactly CCMP and PSK (what this hotspot speaks)? Extra capability bits are accepted.
pub fn rsn_ie_is_ccmp_psk(ie: &[u8]) -> bool {
    if ie.len() < 22 || ie[0] != 0x30 || ie[1] as usize + 2 > ie.len() || ie[1] < 20 {
        return false;
    }
    let ie = &ie[..ie[1] as usize + 2];
    let suite = |at: usize, t: u8| ie.get(at..at + 4) == Some(&[OUI[0], OUI[1], OUI[2], t][..]);
    ie[2..4] == [1, 0] && suite(4, 4) && ie[8..10] == [1, 0] && suite(10, 4) && ie[14..16] == [1, 0] && suite(16, 2)
}

/// The group key data item: dd len 00-0f-ac 01 | key id | 0 | key.
pub fn gtk_kde(key_id: u8, gtk: &[u8]) -> Vec<u8> {
    let mut v = vec![0xdd, (6 + gtk.len()) as u8, 0x00, 0x0f, 0xac, 1, key_id & 3, 0];
    v.extend_from_slice(gtk);
    v
}

/// What the key data of a handshake frame carries.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyData {
    pub rsn_ie: Option<Vec<u8>>,
    pub gtk: Option<(u8, Vec<u8>)>,
}

/// Walk the elements of key data. Padding (`dd 00 00 ...`) ends the walk; a damaged length gives `None`.
pub fn parse_key_data(d: &[u8]) -> Option<KeyData> {
    let mut out = KeyData::default();
    let mut i = 0;
    while i + 2 <= d.len() {
        let (t, l) = (d[i], d[i + 1] as usize);
        if t == 0xdd && l == 0 {
            // the padding of key data (RFC: a 0xdd followed by zeros)
            return if d[i..].iter().skip(1).all(|b| *b == 0) { Some(out) } else { None };
        }
        let body = d.get(i + 2..i + 2 + l)?;
        match t {
            0x30 => out.rsn_ie = Some(d[i..i + 2 + l].to_vec()),
            0xdd if l > 6 && body[..3] == OUI && body[3] == 1 => out.gtk = Some((body[4] & 3, body[6..].to_vec())),
            _ => {}
        }
        i += 2 + l;
    }
    // a single 0xdd at the very end is padding too
    if i == d.len() || (i + 1 == d.len() && d[i] == 0xdd) {
        Some(out)
    } else {
        None
    }
}

/// Pad key data to a multiple of 8 bytes, at least 16, with `dd 00 ...` as the standard says.
pub fn pad_key_data(mut d: Vec<u8>) -> Vec<u8> {
    if d.len() % 8 != 0 || d.len() < 16 {
        d.push(0xdd);
        while d.len() % 8 != 0 || d.len() < 16 {
            d.push(0);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> KeyFrame {
        let mut f = KeyFrame::new(INFO_M1, 7);
        f.nonce = std::array::from_fn(|i| i as u8);
        f.data = vec![1, 2, 3];
        f
    }

    #[test]
    fn a_frame_encodes_to_the_documented_layout_and_decodes_back() {
        let b = sample().encode();
        assert_eq!(b.len(), MIN_FRAME + 3);
        assert_eq!(&b[..9], &[2, 3, 0, 98, 2, 0x00, 0x8a, 0, 16]);
        assert_eq!(&b[9..17], &[0, 0, 0, 0, 0, 0, 0, 7]);
        assert_eq!(&b[17..21], &[0, 1, 2, 3]);
        assert_eq!(&b[97..], &[0, 3, 1, 2, 3]);
        assert_eq!(KeyFrame::decode(&b).unwrap(), sample());
    }

    #[test]
    fn the_four_message_flag_words_are_the_well_known_values() {
        assert_eq!((INFO_M1, INFO_M2, INFO_M3, INFO_M4), (0x008a, 0x010a, 0x13ca, 0x030a));
    }

    #[test]
    fn short_wrong_type_and_lying_lengths_are_refused() {
        let b = sample().encode();
        assert_eq!(KeyFrame::decode(&b[..50]), Err(EapolError::TooShort));
        let mut x = b.clone();
        x[1] = 0;
        assert_eq!(KeyFrame::decode(&x), Err(EapolError::NotKeyFrame));
        let mut x = b.clone();
        x[4] = 254;
        assert_eq!(KeyFrame::decode(&x), Err(EapolError::BadDescriptor));
        let mut x = b.clone();
        x[98] = 200; // key data length longer than the frame
        assert_eq!(KeyFrame::decode(&x), Err(EapolError::BadLength));
        let mut x = b.clone();
        x[3] = 200; // body length longer than the buffer
        assert_eq!(KeyFrame::decode(&x), Err(EapolError::BadLength));
        assert_eq!(KeyFrame::decode(&b[..b.len() - 1]), Err(EapolError::BadLength));
    }

    #[test]
    fn the_mic_covers_the_frame_and_a_changed_bit_or_a_wrong_key_fails() {
        let kck = [9u8; 16];
        let mut b = sample().encode();
        assert!(!verify_mic(&kck, &b));
        sign(&kck, &mut b);
        assert!(verify_mic(&kck, &b));
        assert!(!verify_mic(&[8u8; 16], &b));
        for at in [0usize, 5, 20, 90, 100] {
            let mut x = b.clone();
            x[at] ^= 1;
            assert!(!verify_mic(&kck, &x), "byte {at}");
        }
        // trailing bytes after the frame (link padding) are not part of the MIC
        let mut padded = b.clone();
        padded.extend_from_slice(&[0, 0, 0]);
        assert!(verify_mic(&kck, &padded));
    }

    #[test]
    fn the_rsn_element_is_the_standard_22_bytes_and_is_recognised() {
        let ie = rsn_ie_ccmp_psk();
        assert_eq!(crate::hex(&ie), "30140100000fac040100000fac040100000fac020000");
        assert!(rsn_ie_is_ccmp_psk(&ie));
        let mut tkip = ie.clone();
        tkip[13] = 2;
        assert!(!rsn_ie_is_ccmp_psk(&tkip));
        let mut sae = ie.clone();
        sae[19] = 8;
        assert!(!rsn_ie_is_ccmp_psk(&sae));
        assert!(!rsn_ie_is_ccmp_psk(&ie[..10]));
    }

    #[test]
    fn key_data_with_a_group_key_and_padding_parses_and_damage_is_noticed() {
        let mut d = rsn_ie_ccmp_psk();
        d.extend(gtk_kde(1, &[0xaa; 16]));
        assert_eq!(d.len(), 22 + 24);
        let d = pad_key_data(d);
        assert_eq!(d.len() % 8, 0);
        assert!(d.len() > 46);
        let k = parse_key_data(&d).unwrap();
        assert_eq!(k.rsn_ie.unwrap(), rsn_ie_ccmp_psk());
        assert_eq!(k.gtk.unwrap(), (1, vec![0xaa; 16]));
        assert_eq!(parse_key_data(&[0x30, 20, 1]), None);
        assert_eq!(parse_key_data(&[0xdd, 0, 1]), None);
        assert_eq!(pad_key_data(vec![1]).len(), 16);
        assert_eq!(pad_key_data(vec![1; 16]).len(), 16);
    }

    #[test]
    fn random_bytes_never_panic_the_decoder() {
        let mut s = 0x1234_5678u32;
        for n in 0..3000 {
            let len = (n % 140) as usize;
            let v: Vec<u8> = (0..len)
                .map(|_| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    (s >> 24) as u8
                })
                .collect();
            let _ = KeyFrame::decode(&v);
            let _ = parse_key_data(&v);
            let _ = rsn_ie_is_ccmp_psk(&v);
            let _ = verify_mic(&[0; 16], &v);
        }
    }
}
