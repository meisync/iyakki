//! The other direction: a picture cut into RFC 2435 RTP payloads and wrapped in RTP / interleaved frames. For camera simulators and
//! tests (the Cameras tile's fake RTSP server uses it); the client never calls it.

/// A whole RTP packet.
pub fn rtp_packet(pt: u8, marker: bool, seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x80, (pt & 0x7f) | if marker { 0x80 } else { 0 }];
    v.extend_from_slice(&seq.to_be_bytes());
    v.extend_from_slice(&ts.to_be_bytes());
    v.extend_from_slice(&ssrc.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// `$` + channel + length + data.
pub fn interleave(channel: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![b'$', channel];
    v.extend_from_slice(&(data.len() as u16).to_be_bytes());
    v.extend_from_slice(data);
    v
}

/// RFC 2435 payloads for one picture's entropy-coded `scan`. `ftype` is 0 (4:2:2) or 1 (4:2:0); `q` 1..=99 uses the standard scaling,
/// `q` >= 128 sends `tables` (128 bytes: luma then chroma, zig-zag order) in the first packet, or none (reuse). Each payload comes with
/// whether it is the last (the RTP marker bit).
pub fn packetize_jpeg(width: u16, height: u16, ftype: u8, q: u8, tables: Option<&[u8]>, scan: &[u8], max_payload: usize) -> Vec<(bool, Vec<u8>)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let first_extra = if q >= 128 { 4 + tables.map_or(0, <[u8]>::len) } else { 0 };
    loop {
        let mut p = vec![0u8];
        p.extend_from_slice(&(off as u32).to_be_bytes()[1..]);
        p.extend_from_slice(&[ftype, q, (width / 8) as u8, (height / 8) as u8]);
        let room = max_payload.saturating_sub(8 + if off == 0 { first_extra } else { 0 }).max(1);
        if off == 0 && q >= 128 {
            let t = tables.unwrap_or(&[]);
            p.extend_from_slice(&[0, 0]);
            p.extend_from_slice(&(t.len() as u16).to_be_bytes());
            p.extend_from_slice(t);
        }
        let take = room.min(scan.len() - off);
        p.extend_from_slice(&scan[off..off + take]);
        off += take;
        let last = off >= scan.len();
        out.push((last, p));
        if last {
            return out;
        }
    }
}

/// Split a baseline JPEG (the camera module's encoder writes 4:2:0 with the standard tables) into its quantisation tables and scan,
/// as RFC 2435 wants them. `None` for anything else (other sampling, other Huffman tables, progressive).
pub fn split_jpeg(j: &[u8]) -> Option<(u16, u16, Vec<u8>, Vec<u8>)> {
    let (mut i, mut w, mut h) = (2usize, 0u16, 0u16);
    let mut tables = vec![vec![0u8; 64]; 2];
    let mut sampling_ok = false;
    if j.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    while i + 4 <= j.len() {
        if j[i] != 0xFF {
            return None;
        }
        let m = j[i + 1];
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        let seg = j.get(i + 4..i + 2 + len)?;
        match m {
            0xDB => {
                let mut k = 0;
                while k + 65 <= seg.len() {
                    let id = (seg[k] & 0x0f) as usize;
                    if seg[k] >> 4 != 0 || id > 1 {
                        return None;
                    }
                    tables[id] = seg[k + 1..k + 65].to_vec();
                    k += 65;
                }
            }
            0xC0 => {
                h = u16::from_be_bytes([seg[1], seg[2]]);
                w = u16::from_be_bytes([seg[3], seg[4]]);
                sampling_ok = seg[5] == 3 && seg[7] == 0x22 && seg[10] == 0x11 && seg[13] == 0x11;
            }
            0xC1 | 0xC2 => return None,
            0xDA => {
                if !sampling_ok || w % 8 != 0 || h % 8 != 0 {
                    return None;
                }
                let scan = &j[i + 2 + len..];
                let scan = scan.strip_suffix(&[0xFF, 0xD9]).unwrap_or(scan);
                let mut t = tables[0].clone();
                t.extend_from_slice(&tables[1]);
                return Some((w, h, t, scan.to_vec()));
            }
            _ => {}
        }
        i += 2 + len;
    }
    None
}
