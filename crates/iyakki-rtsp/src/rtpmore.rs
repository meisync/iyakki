//! More RTP depacketisers for the camera client (F-CAM-20): H.265 (RFC 7798: single NAL unit, aggregation packet, fragmentation unit)
//! into access units, and the sound of a camera: G.711 (RFC 3551) and AAC `mpeg4-generic` (RFC 3640, the AAC-hbr style). Sound is handed
//! out as frames of the camera's own coding; decoding AAC or H.265 needs the add-on or a decoder and is not done here.

use crate::rtp::{AccessUnit, Rtp, MAX_FRAME};
use crate::sdp::AudioCodec;

#[derive(Default)]
pub struct H265Depay {
    au: AccessUnit,
    size: usize,
    fu: Option<Vec<u8>>,
    last_seq: Option<u16>,
    pub dropped: u64,
}

impl H265Depay {
    pub fn new() -> H265Depay {
        H265Depay::default()
    }

    fn add(&mut self, nal: Vec<u8>) {
        if nal.len() < 2 {
            return;
        }
        if self.size + nal.len() > MAX_FRAME {
            self.au.nals.clear();
            self.size = 0;
            self.dropped += 1;
            return;
        }
        self.size += nal.len();
        self.au.nals.push(nal);
    }

    /// The access unit this packet completes (the marker bit, or a new timestamp ending the previous unit).
    pub fn push(&mut self, p: &Rtp) -> Option<AccessUnit> {
        let mut finished = None;
        if !self.au.nals.is_empty() && self.au.timestamp != p.timestamp {
            finished = Some(std::mem::take(&mut self.au));
            self.size = 0;
        }
        if self.au.nals.is_empty() {
            self.au.timestamp = p.timestamp;
        }
        if let Some(l) = self.last_seq {
            if p.seq != l.wrapping_add(1) && self.fu.take().is_some() {
                self.dropped += 1;
            }
        }
        self.last_seq = Some(p.seq);
        let d = p.payload;
        if d.len() < 3 {
            return finished;
        }
        match (d[0] >> 1) & 0x3f {
            0..=47 => self.add(d.to_vec()),
            48 => {
                let mut i = 2;
                while i + 2 <= d.len() {
                    let n = u16::from_be_bytes([d[i], d[i + 1]]) as usize;
                    i += 2;
                    if n < 2 || i + n > d.len() {
                        self.dropped += 1;
                        break;
                    }
                    self.add(d[i..i + n].to_vec());
                    i += n;
                }
            }
            49 => {
                let (start, end, ntype) = (d[2] & 0x80 != 0, d[2] & 0x40 != 0, d[2] & 0x3f);
                if start {
                    let mut v = vec![(d[0] & 0x81) | (ntype << 1), d[1]];
                    v.extend_from_slice(&d[3..]);
                    self.fu = Some(v);
                } else if let Some(v) = self.fu.as_mut() {
                    if v.len() + d.len() > MAX_FRAME {
                        self.fu = None;
                        self.dropped += 1;
                    } else {
                        v.extend_from_slice(&d[3..]);
                    }
                }
                if end {
                    if let Some(v) = self.fu.take() {
                        self.add(v);
                    }
                }
            }
            // PACI and the reserved types are not used by cameras
            _ => self.dropped += 1,
        }
        if p.marker && !self.au.nals.is_empty() {
            let done = std::mem::take(&mut self.au);
            self.size = 0;
            if finished.is_some() {
                self.au = done;
                self.size = self.au.nals.iter().map(Vec::len).sum();
                return finished;
            }
            return Some(done);
        }
        finished
    }
}

/// One piece of sound as the camera coded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFrame {
    pub timestamp: u32,
    pub data: Vec<u8>,
}

#[derive(Default)]
pub struct AudioDepay {
    pub dropped: u64,
}

fn bits(d: &[u8], pos: usize, n: usize) -> Option<u32> {
    if n == 0 {
        return Some(0);
    }
    if pos + n > d.len() * 8 || n > 24 {
        return None;
    }
    let mut v = 0u32;
    for i in 0..n {
        let b = pos + i;
        v = (v << 1) | u32::from((d[b / 8] >> (7 - b % 8)) & 1);
    }
    Some(v)
}

impl AudioDepay {
    pub fn new() -> AudioDepay {
        AudioDepay::default()
    }

    /// The frames in one packet (G.711: the whole payload; AAC: each access unit, its timestamp advanced by 1024 samples per unit).
    pub fn push(&mut self, codec: &AudioCodec, p: &Rtp) -> Vec<AudioFrame> {
        match codec {
            AudioCodec::Pcmu | AudioCodec::Pcma if !p.payload.is_empty() && p.payload.len() <= 8192 => vec![AudioFrame { timestamp: p.timestamp, data: p.payload.to_vec() }],
            AudioCodec::Aac { size_length, index_length, index_delta_length, .. } => self.aac(p, *size_length as usize, *index_length as usize, *index_delta_length as usize),
            _ => {
                self.dropped += 1;
                Vec::new()
            }
        }
    }

    fn aac(&mut self, p: &Rtp, size_len: usize, index_len: usize, delta_len: usize) -> Vec<AudioFrame> {
        let d = p.payload;
        let per = size_len + index_len;
        if d.len() < 2 || per == 0 || size_len == 0 {
            self.dropped += 1;
            return Vec::new();
        }
        let hdr_bits = u16::from_be_bytes([d[0], d[1]]) as usize;
        let hdr_bytes = hdr_bits.div_ceil(8);
        if hdr_bits < per || 2 + hdr_bytes > d.len() {
            self.dropped += 1;
            return Vec::new();
        }
        let mut sizes = Vec::new();
        let mut pos = 16;
        let end = 16 + hdr_bits;
        let mut first = true;
        while pos + if first { per } else { size_len + delta_len } <= end && sizes.len() < 64 {
            let Some(sz) = bits(d, pos, size_len) else { break };
            pos += size_len + if first { index_len } else { delta_len };
            first = false;
            sizes.push(sz as usize);
        }
        let mut at = 2 + hdr_bytes;
        let mut out = Vec::new();
        for (i, sz) in sizes.iter().enumerate() {
            if *sz == 0 || at + sz > d.len() {
                // a fragmented access unit (it spans packets) or a cut one: not built, counted
                self.dropped += 1;
                break;
            }
            out.push(AudioFrame { timestamp: p.timestamp.wrapping_add(1024 * i as u32), data: d[at..at + sz].to_vec() });
            at += sz;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        crate::packetize::rtp_packet(98, marker, seq, ts, 7, payload)
    }
    fn run(d: &mut H265Depay, seq: u16, ts: u32, m: bool, payload: &[u8]) -> Option<AccessUnit> {
        let b = rtp(seq, ts, m, payload);
        d.push(&crate::rtp::parse_rtp(&b).unwrap())
    }

    #[test]
    fn single_nals_aggregation_and_fragments_become_one_access_unit() {
        let mut d = H265Depay::new();
        let vps = [32 << 1, 1, 0xAA];
        let sps = [33 << 1, 1, 0xBB, 0xBB];
        // an aggregation packet: type 48, two NAL units
        let mut ap = vec![48 << 1, 1];
        for n in [&vps[..], &sps[..]] {
            ap.extend_from_slice(&(n.len() as u16).to_be_bytes());
            ap.extend_from_slice(n);
        }
        assert!(run(&mut d, 1, 100, false, &ap).is_none());
        // an IDR slice (type 19) in three fragments
        let nal_hdr = [19u8 << 1, 1];
        let body: Vec<u8> = (0..30).collect();
        let frag = |s: bool, e: bool, chunk: &[u8]| {
            let mut v = vec![49 << 1, 1, (if s { 0x80 } else { 0 }) | (if e { 0x40 } else { 0 }) | 19];
            v.extend_from_slice(chunk);
            v
        };
        assert!(run(&mut d, 2, 100, false, &frag(true, false, &body[..10])).is_none());
        assert!(run(&mut d, 3, 100, false, &frag(false, false, &body[10..20])).is_none());
        let au = run(&mut d, 4, 100, true, &frag(false, true, &body[20..])).unwrap();
        assert_eq!(au.nals.len(), 3);
        assert_eq!(au.nals[0], vps);
        let mut want = nal_hdr.to_vec();
        want.extend_from_slice(&body);
        assert_eq!(au.nals[2], want, "the rebuilt NAL header has the fragment's type");
        assert_eq!(au.timestamp, 100);
    }

    #[test]
    fn a_lost_fragment_drops_the_unit_and_junk_does_not_panic() {
        let mut d = H265Depay::new();
        let f = |s: bool, e: bool| vec![49 << 1, 1, (if s { 0x80 } else { 0 }) | (if e { 0x40 } else { 0 }) | 1, 9, 9];
        run(&mut d, 1, 5, false, &f(true, false));
        assert!(run(&mut d, 5, 5, true, &f(false, true)).is_none() || d.dropped > 0);
        assert!(d.dropped >= 1);
        for junk in [&[][..], &[0x62][..], &[48 << 1, 1, 0xFF, 0xFF, 1][..], &[63 << 1, 1, 1][..]] {
            let _ = run(&mut d, 9, 6, true, junk);
        }
    }

    #[test]
    fn g711_is_passed_through_and_aac_access_units_are_split_by_their_headers() {
        let mut a = AudioDepay::new();
        let b = rtp(1, 8000, false, &[1, 2, 3]);
        let p = crate::rtp::parse_rtp(&b).unwrap();
        assert_eq!(a.push(&AudioCodec::Pcmu, &p), vec![AudioFrame { timestamp: 8000, data: vec![1, 2, 3] }]);
        // AAC-hbr: sizeLength 13, indexLength 3: two access units of 3 and 2 bytes
        let aac = AudioCodec::Aac { size_length: 13, index_length: 3, index_delta_length: 3, config: vec![] };
        let h1 = (3u16 << 3) | 0; // size 3, index 0
        let h2 = (2u16 << 3) | 0; // size 2, delta 0
        let mut payload = vec![0, 32]; // 32 header bits
        payload.extend_from_slice(&h1.to_be_bytes());
        payload.extend_from_slice(&h2.to_be_bytes());
        payload.extend_from_slice(&[10, 11, 12, 20, 21]);
        let b = rtp(2, 16000, true, &payload);
        let f = a.push(&aac, &crate::rtp::parse_rtp(&b).unwrap());
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].data.as_slice(), f[0].timestamp, f[1].data.as_slice(), f[1].timestamp), (&[10u8, 11, 12][..], 16000, &[20u8, 21][..], 17024));
        // a header that claims more bytes than there are
        let mut bad = vec![0, 16];
        bad.extend_from_slice(&((200u16) << 3).to_be_bytes());
        bad.extend_from_slice(&[1, 2]);
        let b = rtp(3, 0, true, &bad);
        assert!(a.push(&aac, &crate::rtp::parse_rtp(&b).unwrap()).is_empty());
        assert!(a.dropped >= 1);
        assert!(a.push(&AudioCodec::Other("OPUS".into()), &crate::rtp::parse_rtp(&rtp(4, 0, true, &[1])).unwrap()).is_empty());
    }
}
