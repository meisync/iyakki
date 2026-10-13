//! A JPEG decoder: baseline, extended sequential and progressive, Huffman coding, 8 bits, grey, YCbCr, RGB and CMYK.
//! Written for untrusted input: every index is checked, sizes are capped, and a damaged file gives what could be read
//! (the rest grey) or an error, never a panic.

use crate::image::Bitmap;

const MAX_PIXELS: u64 = 16_000_000;
const MAX_SIDE: u32 = 16_384;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct Huff {
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [usize; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(counts: &[u8; 16], vals: Vec<u8>) -> Huff {
        let mut h = Huff { vals, ..Huff::default() };
        let (mut code, mut k) = (0i32, 0usize);
        for l in 1..=16 {
            let n = counts[l - 1] as i32;
            h.valptr[l] = k;
            h.mincode[l] = code;
            code += n;
            k += n as usize;
            h.maxcode[l] = if n > 0 { code - 1 } else { -1 };
            code <<= 1;
        }
        h
    }
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
    /// A marker was reached; no more entropy-coded bytes until it is consumed.
    marker: Option<u8>,
    /// Zero bits supplied after the data ran out (a truncated file).
    padded: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Bits<'a> {
        Bits { data, pos, acc: 0, n: 0, marker: None, padded: false }
    }

    fn fill(&mut self) {
        while self.n <= 24 {
            let byte = if self.marker.is_some() {
                self.padded = true;
                0
            } else if let Some(&b) = self.data.get(self.pos) {
                if b == 0xFF {
                    match self.data.get(self.pos + 1) {
                        Some(0) => {
                            self.pos += 2;
                            0xFF
                        }
                        Some(&m) if m != 0xFF => {
                            self.marker = Some(m);
                            0
                        }
                        Some(_) => {
                            self.pos += 1; // fill byte
                            continue;
                        }
                        None => {
                            self.marker = Some(0xD9);
                            0
                        }
                    }
                } else {
                    self.pos += 1;
                    b
                }
            } else {
                self.marker = Some(0xD9);
                self.padded = true;
                0
            };
            self.acc |= (byte as u32) << (24 - self.n);
            self.n += 8;
        }
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.n < n {
            self.fill();
        }
        let v = self.acc >> (32 - n);
        self.acc <<= n;
        self.n -= n;
        v
    }

    fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    fn decode(&mut self, h: &Huff) -> u8 {
        let mut code = 0i32;
        for l in 1..=16 {
            code = (code << 1) | self.bit() as i32;
            if h.maxcode[l] >= 0 && code <= h.maxcode[l] && code >= h.mincode[l] {
                return h.vals.get(h.valptr[l] + (code - h.mincode[l]) as usize).copied().unwrap_or(0);
            }
        }
        0 // a bad code: treat as zero, keep going
    }

    /// Throw away what is left of the current byte and any padding up to the marker, then consume `RSTn` if it is next.
    fn restart(&mut self) -> bool {
        self.acc = 0;
        self.n = 0;
        // find the marker if we have not reached it yet
        while self.marker.is_none() {
            match (self.data.get(self.pos), self.data.get(self.pos + 1)) {
                (Some(0xFF), Some(&m)) if m != 0 && m != 0xFF => self.marker = Some(m),
                (Some(_), _) => self.pos += 1,
                (None, _) => return false,
            }
        }
        if let Some(m) = self.marker {
            if (0xD0..=0xD7).contains(&m) {
                self.pos += 2;
                self.marker = None;
                return true;
            }
        }
        false
    }
}

fn extend(v: u32, t: u32) -> i32 {
    if t == 0 {
        0
    } else if v < (1 << (t - 1)) {
        v as i32 - (1 << t) + 1
    } else {
        v as i32
    }
}

#[derive(Clone, Default)]
struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    dc: usize,
    ac: usize,
    bw: usize,
    bh: usize,
    coefs: Vec<i16>,
    pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    comps: Vec<Comp>,
    hmax: usize,
    vmax: usize,
    mcus_x: usize,
    mcus_y: usize,
}

pub fn decode(data: &[u8]) -> Result<Bitmap, String> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let mut pos = 2;
    let mut qt = [[0u16; 64]; 4];
    let mut dc_tables: [Option<Huff>; 4] = Default::default();
    let mut ac_tables: [Option<Huff>; 4] = Default::default();
    let mut frame: Option<Frame> = None;
    let mut restart_interval = 0usize;
    let mut adobe: Option<u8> = None;
    let mut scans = 0;
    let mut damaged = false;
    loop {
        // next marker
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        let Some(&m) = data.get(pos) else { break };
        pos += 1;
        if m == 0xD9 {
            break;
        }
        if m == 0x00 || m == 0x01 || (0xD0..=0xD8).contains(&m) {
            continue; // stray restart or padding
        }
        let len = match (data.get(pos), data.get(pos + 1)) {
            (Some(&a), Some(&b)) => ((a as usize) << 8) | b as usize,
            _ => break,
        };
        if len < 2 || pos + len > data.len() {
            damaged = true;
            if scans == 0 {
                return Err("the JPEG is cut short".into());
            }
            break;
        }
        let seg = &data[pos + 2..pos + len];
        pos += len;
        match m {
            0xDB => {
                let mut s = seg;
                while !s.is_empty() {
                    let (prec, id) = (s[0] >> 4, (s[0] & 15) as usize);
                    let n = if prec == 0 { 64 } else { 128 };
                    if id > 3 || s.len() < 1 + n {
                        return Err("bad quantisation table".into());
                    }
                    for i in 0..64 {
                        qt[id][i] = if prec == 0 { s[1 + i] as u16 } else { ((s[1 + 2 * i] as u16) << 8) | s[2 + 2 * i] as u16 };
                    }
                    s = &s[1 + n..];
                }
            }
            0xC4 => {
                let mut s = seg;
                while s.len() >= 17 {
                    let (class, id) = (s[0] >> 4, (s[0] & 15) as usize);
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&s[1..17]);
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    if id > 3 || class > 1 || total > 256 || s.len() < 17 + total {
                        return Err("bad Huffman table".into());
                    }
                    let h = Huff::new(&counts, s[17..17 + total].to_vec());
                    if class == 0 { dc_tables[id] = Some(h) } else { ac_tables[id] = Some(h) }
                    s = &s[17 + total..];
                }
            }
            0xDD => {
                if seg.len() >= 2 {
                    restart_interval = ((seg[0] as usize) << 8) | seg[1] as usize;
                }
            }
            0xEE => {
                if seg.len() >= 12 && &seg[..5] == b"Adobe" {
                    adobe = Some(seg[11]);
                }
            }
            0xC0 | 0xC1 | 0xC2 => {
                if frame.is_some() {
                    return Err("more than one frame".into());
                }
                if seg.len() < 6 || seg[0] != 8 {
                    return Err("only 8-bit JPEGs are supported".into());
                }
                let (h, w) = ((((seg[1] as usize) << 8) | seg[2] as usize), (((seg[3] as usize) << 8) | seg[4] as usize));
                let nc = seg[5] as usize;
                if w == 0 || h == 0 || w as u32 > MAX_SIDE || h as u32 > MAX_SIDE || (w as u64) * (h as u64) > MAX_PIXELS {
                    return Err(format!("the picture is too large ({w}x{h})"));
                }
                if !matches!(nc, 1 | 3 | 4) || seg.len() < 6 + 3 * nc {
                    return Err("unsupported number of colour components".into());
                }
                let mut comps = Vec::new();
                for i in 0..nc {
                    let b = &seg[6 + 3 * i..9 + 3 * i];
                    let (hh, vv) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
                    if !(1..=4).contains(&hh) || !(1..=4).contains(&vv) || b[2] > 3 {
                        return Err("bad sampling factors".into());
                    }
                    comps.push(Comp { id: b[0], h: hh, v: vv, tq: b[2] as usize, ..Comp::default() });
                }
                let (hmax, vmax) = (comps.iter().map(|c| c.h).max().unwrap_or(1), comps.iter().map(|c| c.v).max().unwrap_or(1));
                let (mcus_x, mcus_y) = (w.div_ceil(8 * hmax), h.div_ceil(8 * vmax));
                for c in &mut comps {
                    c.bw = mcus_x * c.h;
                    c.bh = mcus_y * c.v;
                    c.coefs = vec![0i16; c.bw * c.bh * 64];
                }
                frame = Some(Frame { width: w, height: h, progressive: m == 0xC2, comps, hmax, vmax, mcus_x, mcus_y });
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Err("this kind of JPEG (lossless, arithmetic or hierarchical) is not supported".into()),
            0xDA => {
                let Some(f) = frame.as_mut() else { return Err("a scan before the frame".into()) };
                let ns = *seg.first().ok_or("bad scan header")? as usize;
                if ns == 0 || ns > 4 || seg.len() < 1 + 2 * ns + 3 {
                    return Err("bad scan header".into());
                }
                let mut idx = Vec::new();
                for i in 0..ns {
                    let (cid, tables) = (seg[1 + 2 * i], seg[2 + 2 * i]);
                    let ci = f.comps.iter().position(|c| c.id == cid).ok_or("a scan refers to an unknown component")?;
                    f.comps[ci].dc = (tables >> 4) as usize & 3;
                    f.comps[ci].ac = (tables & 15) as usize & 3;
                    idx.push(ci);
                }
                let (ss, se, ahl) = (seg[1 + 2 * ns] as usize, seg[2 + 2 * ns] as usize, seg[3 + 2 * ns]);
                let (ah, al) = ((ahl >> 4) as u32, (ahl & 15) as u32);
                let mut bits = Bits::new(data, pos);
                let scan = Scan { comps: idx, ss, se, ah, al, ri: restart_interval };
                decode_scan(f, &scan, &dc_tables, &ac_tables, &mut bits)?;
                damaged |= bits.padded;
                pos = bits.pos;
                scans += 1;
            }
            _ => {}
        }
    }
    let f = frame.ok_or("no picture in the file")?;
    if scans == 0 {
        return Err("no picture data in the file".into());
    }
    let _ = damaged;
    finish(&f, &qt, adobe)
}

struct Scan {
    comps: Vec<usize>,
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
    ri: usize,
}

fn decode_scan(f: &mut Frame, sc: &Scan, dc_t: &[Option<Huff>; 4], ac_t: &[Option<Huff>; 4], bits: &mut Bits) -> Result<(), String> {
    let progressive = f.progressive;
    if progressive {
        if sc.ss > sc.se || sc.se > 63 || (sc.ss == 0 && sc.se != 0) || (sc.ss > 0 && sc.comps.len() != 1) {
            return Err("bad progressive scan".into());
        }
    }
    for &ci in &sc.comps {
        f.comps[ci].pred = 0;
    }
    let mut eobrun = 0u32;
    let (cols, rows, interleaved) = if sc.comps.len() == 1 {
        let c = &f.comps[sc.comps[0]];
        let cw = (f.width * c.h).div_ceil(f.hmax);
        let ch = (f.height * c.v).div_ceil(f.vmax);
        (cw.div_ceil(8), ch.div_ceil(8), false)
    } else {
        (f.mcus_x, f.mcus_y, true)
    };
    let total = cols * rows;
    let mut since_restart = 0usize;
    for n in 0..total {
        if sc.ri > 0 && since_restart == sc.ri {
            bits.restart();
            for &ci in &sc.comps {
                f.comps[ci].pred = 0;
            }
            eobrun = 0;
            since_restart = 0;
        }
        since_restart += 1;
        let (mx, my) = (n % cols, n / cols);
        if interleaved {
            for &ci in &sc.comps {
                let (h, v) = (f.comps[ci].h, f.comps[ci].v);
                for dy in 0..v {
                    for dx in 0..h {
                        block(f, ci, mx * h + dx, my * v + dy, sc, dc_t, ac_t, bits, &mut eobrun, progressive)?;
                    }
                }
            }
        } else {
            block(f, sc.comps[0], mx, my, sc, dc_t, ac_t, bits, &mut eobrun, progressive)?;
        }
        if bits.padded && bits.n == 0 && bits.marker == Some(0xD9) && n + 1 < total && bits.pos >= bits.data.len() {
            break; // the file ends here
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn block(f: &mut Frame, ci: usize, bx: usize, by: usize, sc: &Scan, dc_t: &[Option<Huff>; 4], ac_t: &[Option<Huff>; 4], bits: &mut Bits, eobrun: &mut u32, progressive: bool) -> Result<(), String> {
    let c = &mut f.comps[ci];
    if bx >= c.bw || by >= c.bh {
        return Ok(());
    }
    let base = (by * c.bw + bx) * 64;
    let coefs = &mut c.coefs[base..base + 64];
    if !progressive {
        let dc = dc_t[c.dc].as_ref().ok_or("a missing DC table")?;
        let ac = ac_t[c.ac].as_ref().ok_or("a missing AC table")?;
        let t = bits.decode(dc) as u32;
        let diff = extend(bits.bits(t.min(16)), t.min(16));
        c.pred = c.pred.wrapping_add(diff);
        coefs[0] = c.pred as i16;
        let mut k = 1;
        while k < 64 {
            let rs = bits.decode(ac);
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if r == 15 {
                    k += 16;
                    continue;
                }
                break;
            }
            k += r;
            if k > 63 {
                break;
            }
            coefs[ZIGZAG[k]] = extend(bits.bits(s), s) as i16;
            k += 1;
        }
        return Ok(());
    }
    // progressive
    if sc.ss == 0 {
        if sc.ah == 0 {
            let dc = dc_t[c.dc].as_ref().ok_or("a missing DC table")?;
            let t = bits.decode(dc) as u32;
            let diff = extend(bits.bits(t.min(16)), t.min(16));
            c.pred = c.pred.wrapping_add(diff);
            coefs[0] = (c.pred << sc.al) as i16;
        } else if bits.bit() == 1 {
            coefs[0] |= (1i32 << sc.al) as i16;
        }
        return Ok(());
    }
    let ac = ac_t[c.ac].as_ref().ok_or("a missing AC table")?;
    if sc.ah == 0 {
        if *eobrun > 0 {
            *eobrun -= 1;
            return Ok(());
        }
        let mut k = sc.ss;
        while k <= sc.se {
            let rs = bits.decode(ac);
            let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            if s == 0 {
                if r < 15 {
                    *eobrun = (1 << r) - 1;
                    if r > 0 {
                        *eobrun += bits.bits(r);
                    }
                    break;
                }
                k += 16;
                continue;
            }
            k += r as usize;
            if k > 63 {
                break;
            }
            coefs[ZIGZAG[k]] = (extend(bits.bits(s), s) << sc.al) as i16;
            k += 1;
        }
        return Ok(());
    }
    // refinement of AC coefficients
    let (p1, m1) = (1i16 << sc.al, -(1i16 << sc.al));
    let mut k = sc.ss;
    if *eobrun == 0 {
        while k <= sc.se {
            let rs = bits.decode(ac);
            let (mut r, s) = ((rs >> 4) as i32, (rs & 15) as u32);
            let mut val = 0i16;
            if s != 0 {
                val = if bits.bit() == 1 { p1 } else { m1 };
            } else if r != 15 {
                *eobrun = 1 << r;
                if r > 0 {
                    *eobrun += bits.bits(r as u32);
                }
                break;
            }
            while k <= sc.se {
                let z = ZIGZAG[k];
                if coefs[z] != 0 {
                    if bits.bit() == 1 && (coefs[z] & p1) == 0 {
                        coefs[z] = coefs[z].wrapping_add(if coefs[z] >= 0 { p1 } else { m1 });
                    }
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if val != 0 && k <= sc.se {
                coefs[ZIGZAG[k]] = val;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= sc.se {
            let z = ZIGZAG[k];
            if coefs[z] != 0 && bits.bit() == 1 && (coefs[z] & p1) == 0 {
                coefs[z] = coefs[z].wrapping_add(if coefs[z] >= 0 { p1 } else { m1 });
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

/// Dequantise and inverse-transform every block into one plane per component, then upsample and convert to RGBA.
fn finish(f: &Frame, qt: &[[u16; 64]; 4], adobe: Option<u8>) -> Result<Bitmap, String> {
    // cos table: T[u][x] = c(u) * cos((2x+1) u pi / 16) / 2
    let mut t = [[0f32; 8]; 8];
    for (u, row) in t.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            *v = cu * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos() / 2.0;
        }
    }
    let mut planes: Vec<(Vec<u8>, usize)> = Vec::new();
    for c in &f.comps {
        let (pw, ph) = (c.bw * 8, c.bh * 8);
        let mut plane = vec![0u8; pw * ph];
        let q = &qt[c.tq];
        for by in 0..c.bh {
            for bx in 0..c.bw {
                let base = (by * c.bw + bx) * 64;
                let mut deq = [0f32; 64];
                for k in 0..64 {
                    let nat = ZIGZAG[k];
                    deq[nat] = c.coefs[base + nat] as f32 * q[k] as f32;
                }
                // rows then columns
                let mut tmp = [0f32; 64];
                for v in 0..8 {
                    for x in 0..8 {
                        let mut s = 0.0;
                        for u in 0..8 {
                            s += deq[v * 8 + u] * t[u][x];
                        }
                        tmp[v * 8 + x] = s;
                    }
                }
                for x in 0..8 {
                    for y in 0..8 {
                        let mut s = 0.0;
                        for v in 0..8 {
                            s += tmp[v * 8 + x] * t[v][y];
                        }
                        plane[(by * 8 + y) * pw + bx * 8 + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        planes.push((plane, pw));
    }
    let (w, h) = (f.width, f.height);
    let mut rgba = vec![255u8; w * h * 4];
    // Samples of a subsampled component sit at the centre of the pixels they cover, so a pixel takes a weighted mix
    // of the nearest samples (what libjpeg's "fancy upsampling" does), not just the one it falls in.
    let sample = |ci: usize, x: usize, y: usize| -> f32 {
        let c = &f.comps[ci];
        let (pw, ph) = (c.bw * 8, c.bh * 8);
        let plane = &planes[ci].0;
        if c.h == f.hmax && c.v == f.vmax {
            return plane[y.min(ph - 1) * pw + x.min(pw - 1)] as f32;
        }
        let fx = ((x as f32 + 0.5) * c.h as f32 / f.hmax as f32 - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) * c.v as f32 / f.vmax as f32 - 0.5).max(0.0);
        let (x0, y0) = ((fx as usize).min(pw - 1), (fy as usize).min(ph - 1));
        let (x1, y1) = ((x0 + 1).min(pw - 1), (y0 + 1).min(ph - 1));
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let top = plane[y0 * pw + x0] as f32 * (1.0 - ax) + plane[y0 * pw + x1] as f32 * ax;
        let bottom = plane[y1 * pw + x0] as f32 * (1.0 - ax) + plane[y1 * pw + x1] as f32 * ax;
        top * (1.0 - ay) + bottom * ay
    };
    let ycc = |y: f32, cb: f32, cr: f32| -> (f32, f32, f32) { (y + 1.402 * (cr - 128.0), y - 0.344136 * (cb - 128.0) - 0.714136 * (cr - 128.0), y + 1.772 * (cb - 128.0)) };
    let rgb_ids = f.comps.len() == 3 && f.comps[0].id == b'R' && f.comps[1].id == b'G' && f.comps[2].id == b'B';
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let (r, g, b) = match f.comps.len() {
                1 => {
                    let v = sample(0, x, y);
                    (v, v, v)
                }
                3 => {
                    let (a, b2, c) = (sample(0, x, y), sample(1, x, y), sample(2, x, y));
                    if adobe == Some(0) || rgb_ids { (a, b2, c) } else { ycc(a, b2, c) }
                }
                _ => {
                    let (a, b2, c, k) = (sample(0, x, y), sample(1, x, y), sample(2, x, y), sample(3, x, y));
                    let (c1, m1, y1) = if adobe == Some(2) { ycc(a, b2, c) } else { (a, b2, c) };
                    // Adobe stores CMYK inverted: 255 means no ink
                    (c1.clamp(0.0, 255.0) * k / 255.0, m1.clamp(0.0, 255.0) * k / 255.0, y1.clamp(0.0, 255.0) * k / 255.0)
                }
            };
            rgba[o] = r.round().clamp(0.0, 255.0) as u8;
            rgba[o + 1] = g.round().clamp(0.0, 255.0) as u8;
            rgba[o + 2] = b.round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(Bitmap { width: w as u32, height: h as u32, rgba })
}
