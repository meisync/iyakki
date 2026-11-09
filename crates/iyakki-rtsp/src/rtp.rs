//! RTP (RFC 3550) packet parsing and the two depacketisers: Motion-JPEG (RFC 2435) into complete JPEG files, and H.264 (RFC 6184:
//! single NAL unit, STAP-A, FU-A) into access units. Pure functions over bytes; every length is checked and every frame is bounded.

use crate::jpegtab::{scaled_zigzag, standard_dht, Q_CHROMA, Q_LUMA};

/// A frame (JPEG or access unit) larger than this is dropped.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rtp<'a> {
    pub payload_type: u8,
    pub marker: bool,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: &'a [u8],
}

pub fn parse_rtp(d: &[u8]) -> Result<Rtp<'_>, String> {
    if d.len() < 12 {
        return Err("RTP packet shorter than its header".into());
    }
    if d[0] >> 6 != 2 {
        return Err("not RTP version 2".into());
    }
    let (padding, ext, cc) = (d[0] & 0x20 != 0, d[0] & 0x10 != 0, (d[0] & 0x0f) as usize);
    let mut off = 12 + cc * 4;
    if ext {
        if d.len() < off + 4 {
            return Err("RTP extension cut short".into());
        }
        off += 4 + u16::from_be_bytes([d[off + 2], d[off + 3]]) as usize * 4;
    }
    let mut end = d.len();
    if padding {
        let p = *d.last().unwrap_or(&0) as usize;
        if p == 0 || p > end {
            return Err("bad RTP padding".into());
        }
        end -= p;
    }
    if off > end {
        return Err("RTP header longer than the packet".into());
    }
    Ok(Rtp { payload_type: d[1] & 0x7f, marker: d[1] & 0x80 != 0, seq: u16::from_be_bytes([d[2], d[3]]), timestamp: u32::from_be_bytes([d[4], d[5], d[6], d[7]]), ssrc: u32::from_be_bytes([d[8], d[9], d[10], d[11]]), payload: &d[off..end] })
}

// ------------------------------------------------------------------------------------------------ Motion-JPEG

/// Builds JPEG files from RFC 2435 packets. Feed packets in order; a finished picture comes out at the packet with the marker bit.
#[derive(Default)]
pub struct MjpegDepay {
    cur: Option<Cur>,
    /// Quantisation tables sent for Q 128..=255, kept for the frames that leave them out.
    tables: Vec<Option<Vec<u8>>>,
    /// Frames dropped because of a gap or bad data (for the status line).
    pub dropped: u64,
}

struct Cur {
    ts: u32,
    ftype: u8,
    q: u8,
    w: u16,
    h: u16,
    restart: Option<u16>,
    qt: Option<Vec<u8>>,
    data: Vec<u8>,
    next_seq: u16,
}

impl MjpegDepay {
    pub fn new() -> MjpegDepay {
        MjpegDepay { cur: None, tables: vec![None; 128], dropped: 0 }
    }

    pub fn push(&mut self, p: &Rtp) -> Option<Vec<u8>> {
        let d = p.payload;
        if d.len() < 8 {
            self.drop_frame();
            return None;
        }
        let off = u32::from_be_bytes([0, d[1], d[2], d[3]]) as usize;
        let (ftype, q) = (d[4], d[5]);
        let (w, h) = (d[6] as u16 * 8, d[7] as u16 * 8);
        let mut i = 8;
        let mut restart = None;
        if (64..=127).contains(&ftype) {
            if d.len() < i + 4 {
                self.drop_frame();
                return None;
            }
            restart = Some(u16::from_be_bytes([d[i], d[i + 1]]));
            i += 4;
        }
        let mut qt = None;
        if off == 0 {
            // a new picture starts: whatever was unfinished is lost
            if self.cur.is_some() {
                self.drop_frame();
            }
            if q >= 128 {
                if d.len() < i + 4 {
                    self.drop_frame();
                    return None;
                }
                let (prec, len) = (d[i + 1], u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize);
                i += 4;
                if len > 0 {
                    if prec != 0 || len != 128 || d.len() < i + len {
                        // 16-bit tables are not used by cameras and not supported here
                        self.dropped += 1;
                        return None;
                    }
                    qt = Some(d[i..i + len].to_vec());
                    self.tables[(q - 128) as usize] = qt.clone();
                    i += len;
                } else {
                    qt = self.tables[(q - 128) as usize].clone();
                }
            }
            if w == 0 || h == 0 || !matches!(ftype & 0x3f, 0 | 1) || q == 0 || (100..128).contains(&q) {
                self.dropped += 1;
                return None;
            }
            self.cur = Some(Cur { ts: p.timestamp, ftype, q, w, h, restart, qt, data: Vec::new(), next_seq: p.seq });
        }
        let Some(cur) = self.cur.as_mut() else { return None };
        if p.timestamp != cur.ts || p.seq != cur.next_seq || off != cur.data.len() {
            self.drop_frame();
            return None;
        }
        if cur.data.len() + (d.len() - i) > MAX_FRAME {
            self.drop_frame();
            return None;
        }
        cur.data.extend_from_slice(&d[i..]);
        cur.next_seq = p.seq.wrapping_add(1);
        if !p.marker {
            return None;
        }
        let c = self.cur.take()?;
        build_jpeg(&c)
    }

    fn drop_frame(&mut self) {
        if self.cur.take().is_some() {
            self.dropped += 1;
        }
    }
}

fn build_jpeg(c: &Cur) -> Option<Vec<u8>> {
    if c.data.is_empty() {
        return None;
    }
    let (lq, cq): (Vec<u8>, Vec<u8>) = match (&c.qt, c.q) {
        (Some(t), _) if t.len() == 128 => (t[..64].to_vec(), t[64..].to_vec()),
        (_, q) if q < 100 => (scaled_zigzag(&Q_LUMA, q).to_vec(), scaled_zigzag(&Q_CHROMA, q).to_vec()),
        _ => return None,
    };
    let mut out = Vec::with_capacity(c.data.len() + 700);
    out.extend_from_slice(&[0xFF, 0xD8]);
    for (id, t) in [(0u8, &lq), (1u8, &cq)] {
        out.extend_from_slice(&[0xFF, 0xDB, 0, 67, id]);
        out.extend_from_slice(t);
    }
    // SOF0: type 0 is 4:2:2 (2x1), type 1 is 4:2:0 (2x2)
    out.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8]);
    out.extend_from_slice(&c.h.to_be_bytes());
    out.extend_from_slice(&c.w.to_be_bytes());
    out.extend_from_slice(&[3, 1, if c.ftype & 0x3f == 0 { 0x21 } else { 0x22 }, 0, 2, 0x11, 1, 3, 0x11, 1]);
    out.extend_from_slice(&standard_dht());
    if let Some(r) = c.restart {
        out.extend_from_slice(&[0xFF, 0xDD, 0, 4]);
        out.extend_from_slice(&r.to_be_bytes());
    }
    out.extend_from_slice(&[0xFF, 0xDA, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    out.extend_from_slice(&c.data);
    if !out.ends_with(&[0xFF, 0xD9]) {
        out.extend_from_slice(&[0xFF, 0xD9]);
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------ H.264

/// One picture's NAL units (without start codes).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AccessUnit {
    pub nals: Vec<Vec<u8>>,
    pub timestamp: u32,
}

impl AccessUnit {
    /// An IDR picture (a NAL unit of type 5): decoding can start here.
    pub fn is_keyframe(&self) -> bool {
        self.nals.iter().any(|n| n.first().is_some_and(|b| b & 0x1f == 5))
    }
    /// The NAL units with `00 00 00 01` start codes, the form decoders read from a file or pipe.
    pub fn annex_b(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for n in &self.nals {
            o.extend_from_slice(&[0, 0, 0, 1]);
            o.extend_from_slice(n);
        }
        o
    }
}

#[derive(Default)]
pub struct H264Depay {
    au: AccessUnit,
    size: usize,
    fu: Option<Vec<u8>>,
    last_seq: Option<u16>,
    pub dropped: u64,
}

impl H264Depay {
    pub fn new() -> H264Depay {
        H264Depay::default()
    }

    fn add(&mut self, nal: Vec<u8>) {
        if nal.is_empty() {
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

    /// Returns the access unit that this packet completes (the RTP marker bit), if any. A new timestamp also ends the previous unit.
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
                self.dropped += 1; // a fragment lost: the half-built NAL unit is useless
            }
        }
        self.last_seq = Some(p.seq);
        let d = p.payload;
        if d.is_empty() {
            return finished;
        }
        match d[0] & 0x1f {
            1..=23 => self.add(d.to_vec()),
            24 => {
                let mut i = 1;
                while i + 2 <= d.len() {
                    let n = u16::from_be_bytes([d[i], d[i + 1]]) as usize;
                    i += 2;
                    if n == 0 || i + n > d.len() {
                        self.dropped += 1;
                        break;
                    }
                    self.add(d[i..i + n].to_vec());
                    i += n;
                }
            }
            28 if d.len() >= 3 => {
                let (start, end, ntype) = (d[1] & 0x80 != 0, d[1] & 0x40 != 0, d[1] & 0x1f);
                if start {
                    let mut v = vec![(d[0] & 0xe0) | ntype];
                    v.extend_from_slice(&d[2..]);
                    self.fu = Some(v);
                } else if let Some(v) = self.fu.as_mut() {
                    if v.len() + d.len() > MAX_FRAME {
                        self.fu = None;
                        self.dropped += 1;
                    } else {
                        v.extend_from_slice(&d[2..]);
                    }
                }
                if end {
                    if let Some(v) = self.fu.take() {
                        self.add(v);
                    }
                }
            }
            // STAP-B, MTAP, FU-B and the reserved types are not used by cameras
            _ => self.dropped += 1,
        }
        if p.marker && !self.au.nals.is_empty() {
            let done = std::mem::take(&mut self.au);
            self.size = 0;
            // when a previous unit was also finished by the timestamp change, hand out the older one first and keep this one's
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packetize::{packetize_jpeg, rtp_packet};

    fn p(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        rtp_packet(26, marker, seq, ts, 7, payload)
    }

    #[test]
    fn rtp_headers_with_csrc_extension_and_padding_are_read_and_bad_ones_refused() {
        let mut d = vec![0x80 | 0x20 | 0x10 | 2, 0x80 | 96, 0, 5, 0, 0, 0, 9, 0, 0, 0, 1];
        d.extend_from_slice(&[0; 8]); // 2 CSRC
        d.extend_from_slice(&[0xBE, 0xDE, 0, 1, 1, 2, 3, 4]); // one extension word
        d.extend_from_slice(&[0xAA, 0xBB, 0, 0, 3]); // payload + 3 padding bytes
        let r = parse_rtp(&d).unwrap();
        assert_eq!((r.payload_type, r.marker, r.seq, r.timestamp, r.payload), (96, true, 5, 9, &[0xAA, 0xBB][..]));
        assert!(parse_rtp(&d[..11]).is_err());
        assert!(parse_rtp(&[0x40; 20]).is_err(), "version 1");
        let mut bad = d.clone();
        *bad.last_mut().unwrap() = 200;
        assert!(parse_rtp(&bad).is_err(), "padding longer than the packet");
        assert!(parse_rtp(&[0x8f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err(), "CSRC list past the end");
        assert!(parse_rtp(&[0x90, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xBE]).is_err(), "extension cut short");
    }

    #[test]
    fn a_jpeg_cut_into_packets_comes_back_with_the_same_scan_and_valid_markers() {
        // a made-up scan (the depacketiser does not decode)
        let scan: Vec<u8> = (0..5000u32).map(|i| (i * 31 % 251) as u8).filter(|b| *b != 0xFF).collect();
        let pkts = packetize_jpeg(640, 480, 1, 80, None, &scan, 1000);
        assert!(pkts.len() >= 5);
        let mut dp = MjpegDepay::new();
        let mut out = None;
        for (i, pk) in pkts.iter().enumerate() {
            let bytes = p(i as u16 + 100, 4242, pk.0, &pk.1);
            if let Some(j) = dp.push(&parse_rtp(&bytes).unwrap()) {
                out = Some(j);
            }
        }
        let j = out.expect("a picture at the marker packet");
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        assert_eq!(&j[j.len() - 2..], &[0xFF, 0xD9]);
        let sof = j.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        assert_eq!(u16::from_be_bytes([j[sof + 5], j[sof + 6]]), 480);
        assert_eq!(u16::from_be_bytes([j[sof + 7], j[sof + 8]]), 640);
        assert_eq!(j[sof + 11], 0x22, "type 1 is 4:2:0");
        let sos = j.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap() + 14;
        assert_eq!(&j[sos..j.len() - 2], scan.as_slice());
        assert_eq!(dp.dropped, 0);
    }

    #[test]
    fn q_over_127_carries_its_tables_and_a_later_frame_may_reuse_them() {
        let qt: Vec<u8> = (1..=128u8).collect();
        let scan = vec![1u8, 2, 3, 4];
        let first = packetize_jpeg(64, 64, 0, 200, Some(&qt), &scan, 1000);
        let again = packetize_jpeg(64, 64, 0, 200, None, &scan, 1000);
        let mut dp = MjpegDepay::new();
        let mut got = vec![];
        for (n, set) in [first, again].iter().enumerate() {
            for (i, pk) in set.iter().enumerate() {
                if let Some(j) = dp.push(&parse_rtp(&p((n * 10 + i) as u16, 100 + n as u32, pk.0, &pk.1)).unwrap()) {
                    got.push(j);
                }
            }
        }
        assert_eq!(got.len(), 2);
        let dqt = got[1].windows(2).position(|w| w == [0xFF, 0xDB]).unwrap();
        assert_eq!(&got[1][dqt + 5..dqt + 9], &[1, 2, 3, 4], "the cached luma table");
        assert!(got[0].windows(2).any(|w| w == [0xFF, 0xDD]) == false);
        // type 0 -> 2x1 sampling
        assert_eq!(got[1][got[1].windows(2).position(|w| w == [0xFF, 0xC0]).unwrap() + 11], 0x21);
    }

    #[test]
    fn a_missing_fragment_drops_that_picture_and_the_next_one_is_fine() {
        let scan = vec![7u8; 3000];
        let a = packetize_jpeg(64, 64, 1, 50, None, &scan, 700);
        let b = packetize_jpeg(64, 64, 1, 50, None, &scan, 700);
        let mut dp = MjpegDepay::new();
        let mut got = 0;
        for (i, pk) in a.iter().enumerate() {
            if i == 2 {
                continue; // lost
            }
            if dp.push(&parse_rtp(&p(i as u16, 1, pk.0, &pk.1)).unwrap()).is_some() {
                got += 1;
            }
        }
        assert_eq!(got, 0);
        for (i, pk) in b.iter().enumerate() {
            if dp.push(&parse_rtp(&p(100 + i as u16, 2, pk.0, &pk.1)).unwrap()).is_some() {
                got += 1;
            }
        }
        assert_eq!(got, 1);
        assert!(dp.dropped >= 1);
    }

    #[test]
    fn hostile_mjpeg_payloads_are_dropped_without_a_panic() {
        let mut dp = MjpegDepay::new();
        let hdr = |off: u32, t: u8, q: u8, w: u8, h: u8| {
            let o = off.to_be_bytes();
            vec![0, o[1], o[2], o[3], t, q, w, h]
        };
        let mut seq = 0u16;
        let mut feed = |dp: &mut MjpegDepay, payload: Vec<u8>, marker: bool| {
            seq = seq.wrapping_add(1);
            let bytes = p(seq, 5, marker, &payload);
            dp.push(&parse_rtp(&bytes).unwrap())
        };
        assert!(feed(&mut dp, vec![1, 2, 3], true).is_none(), "shorter than the header");
        assert!(feed(&mut dp, hdr(0, 1, 50, 0, 0), true).is_none(), "zero size");
        assert!(feed(&mut dp, hdr(0, 5, 50, 8, 8), true).is_none(), "unknown type");
        assert!(feed(&mut dp, hdr(0, 1, 0, 8, 8), true).is_none(), "Q 0");
        assert!(feed(&mut dp, hdr(0, 1, 110, 8, 8), true).is_none(), "reserved Q");
        let mut x = hdr(0, 1, 200, 8, 8);
        x.extend_from_slice(&[0, 0, 0xFF, 0xFF]); // table length 65535 but no data
        assert!(feed(&mut dp, x, true).is_none());
        let mut y = hdr(0, 1, 200, 8, 8);
        y.extend_from_slice(&[0, 0, 0, 0]); // length 0 and nothing cached
        y.extend_from_slice(&[1, 2]);
        assert!(feed(&mut dp, y, true).is_none(), "no table known for this Q");
        assert!(feed(&mut dp, hdr(0xFF_FFFF, 1, 50, 8, 8), true).is_none(), "a continuation with no start");
        let mut z = hdr(0, 65, 50, 8, 8);
        z.extend_from_slice(&[0, 4]); // restart header cut short
        assert!(feed(&mut dp, z, true).is_none());
        // a never-ending picture is cut at the size limit
        let mut big = hdr(0, 1, 50, 8, 8);
        big.extend_from_slice(&vec![0u8; 1400]);
        feed(&mut dp, big, false);
        let mut off = 1400u32;
        let mut res = None;
        for _ in 0..7000 {
            let mut c = hdr(off, 1, 50, 8, 8);
            c.extend_from_slice(&vec![0u8; 1400]);
            off += 1400;
            res = feed(&mut dp, c, false);
        }
        assert!(res.is_none());
        assert!(dp.dropped > 0);
    }

    fn h(seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
        rtp_packet(96, marker, seq, ts, 7, payload)
    }

    #[test]
    fn h264_single_nals_stap_a_and_fu_a_make_access_units() {
        let sps = vec![0x67, 1, 2, 3];
        let pps = vec![0x68, 9];
        let idr: Vec<u8> = std::iter::once(0x65).chain((0..3000u32).map(|i| (i % 200) as u8 + 1)).collect();
        let mut stap = vec![24u8];
        for n in [&sps, &pps] {
            stap.extend_from_slice(&(n.len() as u16).to_be_bytes());
            stap.extend_from_slice(n);
        }
        // FU-A fragments of the IDR NAL
        let body = &idr[1..];
        let chunks: Vec<&[u8]> = body.chunks(1000).collect();
        let mut pk = vec![h(1, 90, false, &stap)];
        for (i, c) in chunks.iter().enumerate() {
            let mut v = vec![(idr[0] & 0xe0) | 28, (if i == 0 { 0x80 } else { 0 }) | (if i == chunks.len() - 1 { 0x40 } else { 0 }) | (idr[0] & 0x1f)];
            v.extend_from_slice(c);
            pk.push(h(2 + i as u16, 90, i == chunks.len() - 1, &v));
        }
        let slice = vec![0x41u8, 5, 6];
        pk.push(h(10, 180, true, &slice));
        let mut dp = H264Depay::new();
        let mut aus = vec![];
        for b in &pk {
            if let Some(a) = dp.push(&parse_rtp(b).unwrap()) {
                aus.push(a);
            }
        }
        assert_eq!(aus.len(), 2);
        assert_eq!(aus[0].nals, vec![sps.clone(), pps.clone(), idr.clone()]);
        assert!(aus[0].is_keyframe() && !aus[1].is_keyframe());
        assert_eq!(aus[1].nals, vec![slice]);
        assert!(aus[0].annex_b().starts_with(&[0, 0, 0, 1, 0x67]));
        assert_eq!(dp.dropped, 0);
    }

    #[test]
    fn h264_losses_and_hostile_payloads_are_dropped_cleanly() {
        let mut dp = H264Depay::new();
        let mut seq = 0u16;
        let mut feed = |dp: &mut H264Depay, ts: u32, m: bool, pl: &[u8]| {
            seq = seq.wrapping_add(1);
            dp.push(&parse_rtp(&h(seq, ts, m, pl)).unwrap())
        };
        // an FU-A continuation with no start, a truncated one, an oversized STAP-A entry, a STAP-B
        assert!(feed(&mut dp, 1, false, &[0x7c, 0x05, 1, 2]).is_none());
        assert!(feed(&mut dp, 1, false, &[0x7c]).is_none());
        assert!(feed(&mut dp, 1, true, &[24, 0xFF, 0xFF, 1]).is_none());
        assert!(feed(&mut dp, 1, true, &[25, 0, 1, 1]).is_none());
        assert!(feed(&mut dp, 1, true, &[]).is_none());
        assert!(dp.dropped >= 2);
        // a start fragment, a gap, then an end fragment: no NAL comes out of it
        feed(&mut dp, 2, false, &[0x7c, 0x85, 1, 2, 3]);
        let mut gap = H264Depay::new();
        gap.push(&parse_rtp(&h(1, 3, false, &[0x7c, 0x85, 1, 2, 3])).unwrap());
        let out = gap.push(&parse_rtp(&h(5, 3, true, &[0x7c, 0x45, 4, 5])).unwrap());
        assert!(out.is_none());
        assert!(gap.dropped >= 1);
        // a unit that grows without end is cut
        let mut big = H264Depay::new();
        let nal: Vec<u8> = std::iter::once(0x41).chain(std::iter::repeat_n(1u8, 60_000)).collect();
        for i in 0..200u16 {
            big.push(&parse_rtp(&h(i, 7, false, &nal)).unwrap());
        }
        assert!(big.dropped > 0);
    }
}
