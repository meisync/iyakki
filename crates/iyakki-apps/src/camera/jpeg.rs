//! JPEG for the camera: a small baseline encoder (4:2:0, the standard tables of the JPEG specification, Annex K) and the
//! fix-up most USB cameras need: their MJPEG frames leave out the Huffman tables (the "AVI1" convention), so a frame is
//! only a picture file once the standard tables are put back. Decoding is the browser's decoder (`iyakki_web::jpeg`).

/// Code counts per length (1..=16) and the values, for the four standard tables.
pub const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
pub const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
pub const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
pub const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
pub const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
pub const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
pub const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// The quantisation tables of Annex K, row by row.
const Q_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const Q_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The DHT segment (marker included) with the four standard tables.
pub fn standard_dht() -> Vec<u8> {
    let tables: [(u8, &[u8; 16], &[u8]); 4] = [(0x00, &DC_LUMA_BITS, &DC_VALS), (0x10, &AC_LUMA_BITS, &AC_LUMA_VALS), (0x01, &DC_CHROMA_BITS, &DC_VALS), (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS)];
    let mut body = Vec::new();
    for (class_id, bits, vals) in tables {
        body.push(class_id);
        body.extend_from_slice(bits);
        body.extend_from_slice(vals);
    }
    let mut out = vec![0xFF, 0xC4];
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Walk the marker segments up to the first SOS: (marker, offset of the 0xFF, total length with the marker).
fn segments(j: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= j.len() {
        if j[i] != 0xFF {
            break;
        }
        let m = j[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            out.push((m, i, 2));
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        out.push((m, i, len + 2));
        if m == 0xDA {
            break;
        }
        i += 2 + len;
    }
    out
}

/// Whether a JPEG brings its own Huffman tables before the first scan.
pub fn has_huffman(j: &[u8]) -> bool {
    segments(j).iter().any(|s| s.0 == 0xC4)
}

/// A camera frame made into a complete JPEG: the standard Huffman tables are put in before the first scan when the frame
/// has none (frames that have them come back unchanged). `None` when it is not a JPEG at all.
pub fn with_huffman(j: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if j.len() < 4 || j[0] != 0xFF || j[1] != 0xD8 {
        return None;
    }
    let segs = segments(j);
    if segs.iter().any(|s| s.0 == 0xC4) {
        return Some(std::borrow::Cow::Borrowed(j));
    }
    let sos = segs.iter().find(|s| s.0 == 0xDA)?.1;
    let mut out = Vec::with_capacity(j.len() + 432);
    out.extend_from_slice(&j[..sos]);
    out.extend_from_slice(&standard_dht());
    out.extend_from_slice(&j[sos..]);
    Some(std::borrow::Cow::Owned(out))
}

/// The same JPEG without its Huffman tables (as a camera sends it); for tests and for checking the fix-up.
pub fn strip_huffman(j: &[u8]) -> Vec<u8> {
    let mut out = j[..2.min(j.len())].to_vec();
    let segs = segments(j);
    let mut pos = 2;
    for (m, at, len) in &segs {
        if *at > pos {
            out.extend_from_slice(&j[pos..*at]);
        }
        if *m != 0xC4 {
            out.extend_from_slice(&j[*at..(*at + len).min(j.len())]);
        }
        pos = at + len;
        if *m == 0xDA {
            break;
        }
    }
    if pos < j.len() {
        out.extend_from_slice(&j[pos..]);
    }
    out
}

/// Decode a camera frame (tables put back when missing) to straight RGBA.
pub fn decode(j: &[u8]) -> Result<iyakki_web::image::Bitmap, String> {
    let full = with_huffman(j).ok_or("not a JPEG frame")?;
    iyakki_web::jpeg::decode(&full)
}

/// Canonical codes from code counts: (code, length) per value.
fn codes(bits: &[u8; 16], vals: &[u8]) -> [(u16, u8); 256] {
    let mut out = [(0u16, 0u8); 256];
    let (mut code, mut k) = (0u16, 0usize);
    for (l, &n) in bits.iter().enumerate() {
        for _ in 0..n {
            if let Some(&v) = vals.get(k) {
                out[v as usize] = (code, l as u8 + 1);
            }
            code = code.wrapping_add(1);
            k += 1;
        }
        code <<= 1;
    }
    out
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code as u32 & ((1u32 << len) - 1));
        self.n += len as u32;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0);
            }
            self.n -= 8;
        }
        self.acc &= (1u32 << self.n) - 1;
    }
    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n as u8;
            self.put((1u16 << pad) - 1, pad);
        }
    }
}

/// The libjpeg quality scaling (1..=100) of a base table, in zig-zag order as DQT stores it.
fn scaled(base: &[u16; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as u32;
    let s = if q < 50 { 5000 / q } else { 200 - q * 2 };
    let mut out = [0u16; 64];
    for (k, o) in out.iter_mut().enumerate() {
        *o = ((base[ZIGZAG[k]] as u32 * s + 50) / 100).clamp(1, 255) as u16;
    }
    out
}

fn cos_table() -> [[f32; 8]; 8] {
    let mut c = [[0f32; 8]; 8];
    for (u, row) in c.iter_mut().enumerate() {
        let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
        for (x, v) in row.iter_mut().enumerate() {
            *v = 0.5 * cu * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos();
        }
    }
    c
}

struct Huffs {
    dc: [[(u16, u8); 256]; 2],
    ac: [[(u16, u8); 256]; 2],
}

fn category(v: i32) -> (u8, u16) {
    let a = v.unsigned_abs();
    let n = 32 - a.leading_zeros();
    let bits = if v < 0 { (v - 1) as u32 & ((1u32 << n) - 1) } else { v as u32 };
    (n as u8, bits as u16)
}

/// Transform, quantise and code one 8x8 block (samples already level-shifted).
fn block(w: &mut BitWriter, px: &[f32; 64], q: &[u16; 64], c: &[[f32; 8]; 8], h: &Huffs, t: usize, prev_dc: &mut i32) {
    // rows then columns
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for x in 0..8 {
                s += c[u][x] * px[y * 8 + x];
            }
            tmp[y * 8 + u] = s;
        }
    }
    let mut coef = [0f32; 64];
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0.0;
            for y in 0..8 {
                s += c[v][y] * tmp[y * 8 + u];
            }
            coef[v * 8 + u] = s;
        }
    }
    let mut zz = [0i32; 64];
    for k in 0..64 {
        zz[k] = (coef[ZIGZAG[k]] / q[k] as f32).round() as i32;
    }
    let diff = zz[0] - *prev_dc;
    *prev_dc = zz[0];
    let (n, bits) = category(diff);
    let (code, len) = h.dc[t][n as usize];
    w.put(code, len);
    w.put(bits, n);
    let mut run = 0;
    for &v in &zz[1..] {
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            let (code, len) = h.ac[t][0xF0];
            w.put(code, len);
            run -= 16;
        }
        let (n, bits) = category(v);
        let (code, len) = h.ac[t][(run << 4) | n as usize];
        w.put(code, len);
        w.put(bits, n);
        run = 0;
    }
    if run > 0 {
        let (code, len) = h.ac[t][0x00];
        w.put(code, len);
    }
}

/// Encode straight RGBA (`w`×`h`) as a baseline JPEG, 4:2:0, quality 1..=100.
pub fn encode_rgba(w: u32, h: u32, rgba: &[u8], quality: u8) -> Vec<u8> {
    let (wu, hu) = (w.max(1) as usize, h.max(1) as usize);
    let px = |x: usize, y: usize| -> (f32, f32, f32) {
        let i = (y.min(hu - 1) * wu + x.min(wu - 1)) * 4;
        match rgba.get(i..i + 3) {
            Some(p) => (p[0] as f32, p[1] as f32, p[2] as f32),
            None => (0.0, 0.0, 0.0),
        }
    };
    let ql = scaled(&Q_LUMA, quality);
    let qc = scaled(&Q_CHROMA, quality);
    let mut out = vec![0xFF, 0xD8];
    // JFIF
    out.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
    for (id, q) in [(0u8, &ql), (1u8, &qc)] {
        out.extend_from_slice(&[0xFF, 0xDB, 0, 67, id]);
        out.extend(q.iter().map(|v| *v as u8));
    }
    out.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8]);
    out.extend_from_slice(&(hu as u16).to_be_bytes());
    out.extend_from_slice(&(wu as u16).to_be_bytes());
    out.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    out.extend_from_slice(&standard_dht());
    out.extend_from_slice(&[0xFF, 0xDA, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    let h_t = Huffs { dc: [codes(&DC_LUMA_BITS, &DC_VALS), codes(&DC_CHROMA_BITS, &DC_VALS)], ac: [codes(&AC_LUMA_BITS, &AC_LUMA_VALS), codes(&AC_CHROMA_BITS, &AC_CHROMA_VALS)] };
    let c = cos_table();
    let mut bw = BitWriter { out: Vec::with_capacity(wu * hu / 4), acc: 0, n: 0 };
    let (mut dy, mut dcb, mut dcr) = (0i32, 0i32, 0i32);
    let ycc = |(r, g, b): (f32, f32, f32)| (0.299 * r + 0.587 * g + 0.114 * b, -0.168_736 * r - 0.331_264 * g + 0.5 * b + 128.0, 0.5 * r - 0.418_688 * g - 0.081_312 * b + 128.0);
    for my in (0..hu).step_by(16) {
        for mx in (0..wu).step_by(16) {
            let mut yb = [[0f32; 64]; 4];
            let mut cb = [0f32; 64];
            let mut cr = [0f32; 64];
            for yy in 0..16 {
                for xx in 0..16 {
                    let (yv, u, v) = ycc(px(mx + xx, my + yy));
                    yb[(yy / 8) * 2 + xx / 8][(yy % 8) * 8 + xx % 8] = yv - 128.0;
                    let k = (yy / 2) * 8 + xx / 2;
                    cb[k] += (u - 128.0) / 4.0;
                    cr[k] += (v - 128.0) / 4.0;
                }
            }
            for b in &yb {
                block(&mut bw, b, &ql, &c, &h_t, 0, &mut dy);
            }
            block(&mut bw, &cb, &qc, &c, &h_t, 1, &mut dcb);
            block(&mut bw, &cr, &qc, &c, &h_t, 1, &mut dcr);
        }
    }
    bw.flush();
    out.extend_from_slice(&bw.out);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIL: &[u8] = include_bytes!("testdata/pil-q90.jpg");

    #[test]
    fn the_standard_tables_hold_every_code_once_and_match_libjpeg_byte_for_byte() {
        for (bits, vals) in [(&AC_LUMA_BITS, &AC_LUMA_VALS), (&AC_CHROMA_BITS, &AC_CHROMA_VALS)] {
            assert_eq!(bits.iter().map(|b| *b as usize).sum::<usize>(), 162);
            let mut want: Vec<u8> = vec![0x00, 0xF0];
            for r in 0..16u8 {
                for s in 1..=10u8 {
                    want.push(r << 4 | s);
                }
            }
            let mut got = vals.to_vec();
            got.sort_unstable();
            want.sort_unstable();
            assert_eq!(got, want);
        }
        // the four DHT tables of a file written by libjpeg (Pillow, quality 90, not optimised) are ours
        let mut theirs = Vec::new();
        for (m, at, len) in segments(PIL) {
            if m == 0xC4 {
                theirs.extend_from_slice(&PIL[at + 4..at + len]);
            }
        }
        assert_eq!(theirs, standard_dht()[4..].to_vec());
    }

    #[test]
    fn a_camera_frame_without_tables_decodes_once_they_are_put_back() {
        let bare = strip_huffman(PIL);
        assert!(!has_huffman(&bare) && has_huffman(PIL));
        assert!(bare.len() < PIL.len() - 400);
        let a = iyakki_web::jpeg::decode(PIL).unwrap();
        assert!(iyakki_web::jpeg::decode(&bare).map_or(true, |b| b.rgba != a.rgba), "without tables there is no real picture");
        let fixed = with_huffman(&bare).unwrap();
        let b = iyakki_web::jpeg::decode(&fixed).unwrap();
        assert_eq!((b.width, b.height), (48, 32));
        assert_eq!(a.rgba, b.rgba, "the same picture as the original file");
        // a frame that has tables is left alone; something else is not a JPEG
        assert!(matches!(with_huffman(PIL), Some(std::borrow::Cow::Borrowed(_))));
        assert!(with_huffman(b"RIFF....").is_none());
        assert_eq!(decode(&bare).unwrap().rgba, a.rgba);
    }

    #[test]
    fn the_encoder_writes_a_jpeg_the_decoder_reads_back_close_to_the_original() {
        let (w, h) = (37u32, 21u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 6) as u8, (y * 11) as u8, 200 - (x + y) as u8, 255]);
            }
        }
        let j = encode_rgba(w, h, &rgba, 92);
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        assert_eq!(&j[j.len() - 2..], &[0xFF, 0xD9]);
        let b = iyakki_web::jpeg::decode(&j).unwrap();
        assert_eq!((b.width, b.height), (w, h));
        let err: f64 = rgba.iter().zip(&b.rgba).map(|(a, b)| (*a as f64 - *b as f64).abs()).sum::<f64>() / rgba.len() as f64;
        assert!(err < 6.0, "mean error {err}");
        // a stripped copy of our own file decodes through the fix-up too
        assert_eq!(decode(&strip_huffman(&j)).unwrap().rgba, b.rgba);
        // a flat grey picture: tiny file
        let grey = vec![128u8; 64 * 64 * 4];
        assert!(encode_rgba(64, 64, &grey, 80).len() < 1200);
    }
}
