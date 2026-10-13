//! WebP pictures, decoded from scratch: the RIFF container (plain, extended `VP8X`, and the first frame of an animation
//! that covers the whole canvas), lossy pictures (`VP8 `, the key frame of VP8 as in RFC 6386, with the loop filter
//! and the smooth "fancy" chroma upsampling libwebp uses), lossless pictures (`VP8L`) and the `ALPH` alpha plane (raw or
//! lossless, with the horizontal, vertical and gradient filters).
//!
//! Not supported: animations whose first frame does not cover the whole canvas (the decoder then says so), and
//! anything with a size beyond the same caps the JPEG decoder uses. The code never panics on bad input; every
//! allocation is bounded by the declared picture size, which is capped.
//!
//! The probability and quantiser tables in the lossy decoder are the constants fixed by the VP8 specification.

use crate::image::Bitmap;

const MAX_PIXELS: u64 = 16_000_000;
const MAX_SIDE: u32 = 16_384;

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || (w as u64) * (h as u64) > MAX_PIXELS {
        return Err("this WebP picture is too large".into());
    }
    Ok(())
}

fn le24(b: &[u8]) -> u32 {
    b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16
}

fn le32(b: &[u8]) -> u32 {
    le24(b) | (b[3] as u32) << 24
}

#[derive(Clone, Copy, PartialEq)]
enum Codec {
    Lossy,
    Lossless,
}

struct Image<'a> {
    codec: Codec,
    data: &'a [u8],
    alpha: Option<&'a [u8]>,
}

/// Pull the picture chunks out of a run of chunks (the body of a frame, or the top level of the file).
fn find_image<'a>(chunks: &'a [u8]) -> Result<(Option<Image<'a>>, Option<(u32, u32, u8)>), String> {
    let mut alpha: Option<&[u8]> = None;
    let mut pos = 0usize;
    let mut canvas: Option<(u32, u32, u8)> = None;
    while pos + 8 <= chunks.len() {
        let tag = &chunks[pos..pos + 4];
        let size = le32(&chunks[pos + 4..]) as usize;
        let start = pos + 8;
        let end = start.checked_add(size).ok_or("this WebP file is damaged")?;
        if end > chunks.len() {
            return Err("this WebP file is cut short".into());
        }
        let body = &chunks[start..end];
        match tag {
            b"VP8X" => {
                if body.len() < 10 {
                    return Err("this WebP file is damaged".into());
                }
                canvas = Some((1 + le24(&body[4..]), 1 + le24(&body[7..]), body[0]));
            }
            b"ALPH" => {
                if alpha.is_none() {
                    alpha = Some(body);
                }
            }
            b"VP8 " => return Ok((Some(Image { codec: Codec::Lossy, data: body, alpha }), canvas)),
            b"VP8L" => return Ok((Some(Image { codec: Codec::Lossless, data: body, alpha: None }), canvas)),
            b"ANMF" => {
                if body.len() < 16 {
                    return Err("this WebP file is damaged".into());
                }
                let (fx, fy) = (2 * le24(&body[0..]), 2 * le24(&body[3..]));
                let (fw, fh) = (1 + le24(&body[6..]), 1 + le24(&body[9..]));
                let (inner, _) = find_image(&body[16..])?;
                let inner = inner.ok_or("this animated WebP has no picture data")?;
                if fx != 0 || fy != 0 || canvas.map(|c| (c.0, c.1)) != Some((fw, fh)) {
                    return Err("animated WebP pictures whose first frame is smaller than the canvas are not supported".into());
                }
                return Ok((Some(inner), canvas));
            }
            _ => {}
        }
        pos = end + (size & 1);
    }
    Ok((None, canvas))
}

/// Decode a WebP picture (the first frame, for an animation) to straight RGBA.
pub fn decode(data: &[u8]) -> Result<Bitmap, String> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err("this is not a WebP picture".into());
    }
    let riff = le32(&data[4..]) as usize;
    if riff < 4 {
        return Err("this WebP file is damaged".into());
    }
    let end = (8 + riff).min(data.len());
    let (img, canvas) = find_image(&data[12..end])?;
    let img = img.ok_or("this WebP file holds no picture")?;
    let animated = canvas.map_or(false, |c| c.2 & 2 != 0);
    let bm = match img.codec {
        Codec::Lossless => {
            let (w, h, px, has_alpha) = decode_vp8l(img.data)?;
            let mut rgba = Vec::with_capacity(px.len() * 4);
            for p in px {
                let a = if has_alpha { (p >> 24) as u8 } else { 255 };
                rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, a]);
            }
            Bitmap { width: w, height: h, rgba }
        }
        Codec::Lossy => {
            let f = decode_vp8(img.data)?;
            let alpha = match img.alpha {
                Some(a) => Some(decode_alpha(a, f.w, f.h)?),
                None => None,
            };
            yuv_to_rgba(&f, alpha.as_deref())
        }
    };
    if let Some((cw, ch, _)) = canvas {
        if (cw, ch) != (bm.width, bm.height) && !animated {
            return Err("this WebP file's sizes disagree".into());
        }
    }
    Ok(bm)
}

// ---------------------------------------------------------------------------------------------------------------
// Alpha plane

fn decode_alpha(a: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    if a.is_empty() {
        return Err("the WebP alpha data is empty".into());
    }
    let method = a[0] & 3;
    let filter = (a[0] >> 2) & 3;
    let body = &a[1..];
    let mut plane = match method {
        0 => {
            if body.len() < w * h {
                return Err("the WebP alpha data is cut short".into());
            }
            body[..w * h].to_vec()
        }
        1 => {
            let mut br = BitReader::new(body);
            let px = decode_stream(&mut br, w, h, true)?;
            px.iter().map(|p| (p >> 8) as u8).collect()
        }
        _ => return Err("the WebP alpha data uses an unknown compression".into()),
    };
    if filter != 0 {
        for y in 0..h {
            let row = y * w;
            if y == 0 {
                for x in 1..w {
                    plane[x] = plane[x].wrapping_add(plane[x - 1]);
                }
                continue;
            }
            // first pixel of every later row is predicted from the pixel above
            plane[row] = plane[row].wrapping_add(plane[row - w]);
            match filter {
                1 => {
                    for x in 1..w {
                        plane[row + x] = plane[row + x].wrapping_add(plane[row + x - 1]);
                    }
                }
                2 => {
                    for x in 1..w {
                        plane[row + x] = plane[row + x].wrapping_add(plane[row + x - w]);
                    }
                }
                _ => {
                    for x in 1..w {
                        let (l, t, tl) = (plane[row + x - 1] as i32, plane[row + x - w] as i32, plane[row + x - w - 1] as i32);
                        let pred = (l + t - tl).clamp(0, 255) as u8;
                        plane[row + x] = plane[row + x].wrapping_add(pred);
                    }
                }
            }
        }
    }
    Ok(plane)
}

// ---------------------------------------------------------------------------------------------------------------
// VP8L (lossless)

struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
    val: u64,
    nbits: u32,
    eos: bool,
}

impl<'a> BitReader<'a> {
    fn new(d: &'a [u8]) -> Self {
        BitReader { d, pos: 0, val: 0, nbits: 0, eos: false }
    }

    fn fill(&mut self) {
        while self.nbits <= 56 && self.pos < self.d.len() {
            self.val |= (self.d[self.pos] as u64) << self.nbits;
            self.pos += 1;
            self.nbits += 8;
        }
    }

    /// Read `n` (at most 32) bits, least significant first. Past the end this gives zeros and flags `eos`.
    fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.nbits < n {
            self.fill();
            if self.nbits < n {
                self.eos = true;
                self.nbits = 0;
                self.val = 0;
                return 0;
            }
        }
        let v = (self.val & ((1u64 << n) - 1)) as u32;
        self.val >>= n;
        self.nbits -= n;
        v
    }
}

struct Huff {
    count: [u16; 16],
    symbols: Vec<u16>,
    single: Option<u16>,
}

impl Huff {
    fn build(lengths: &[u8]) -> Result<Huff, String> {
        let mut count = [0u16; 16];
        let mut used = 0usize;
        let mut last = 0usize;
        for (s, &l) in lengths.iter().enumerate() {
            if l > 15 {
                return Err("the WebP picture has a bad code table".into());
            }
            if l != 0 {
                count[l as usize] += 1;
                used += 1;
                last = s;
            }
        }
        if used == 0 {
            return Err("the WebP picture has an empty code table".into());
        }
        if used == 1 {
            return Ok(Huff { count, symbols: Vec::new(), single: Some(last as u16) });
        }
        let mut left: i32 = 1;
        for l in 1..16 {
            left = (left << 1) - count[l] as i32;
            if left < 0 {
                return Err("the WebP picture has a bad code table".into());
            }
        }
        if left != 0 {
            return Err("the WebP picture has a bad code table".into());
        }
        let mut offs = [0usize; 17];
        for l in 1..16 {
            offs[l + 1] = offs[l] + count[l] as usize;
        }
        let mut symbols = vec![0u16; used];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize]] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huff { count, symbols, single: None })
    }

    #[inline]
    fn decode(&self, br: &mut BitReader) -> u32 {
        if let Some(s) = self.single {
            return s as u32;
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for l in 1..16 {
            code |= br.read(1) as i32;
            let c = self.count[l] as i32;
            if code - c < first {
                return self.symbols[(index + (code - first)) as usize] as u32;
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        br.eos = true;
        0
    }
}

const CODE_LENGTH_ORDER: [usize; 19] = [17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

fn read_huffman_code(br: &mut BitReader, alphabet: usize) -> Result<Huff, String> {
    let mut lengths = vec![0u8; alphabet];
    if br.read(1) == 1 {
        let n = br.read(1) + 1;
        let wide = br.read(1);
        let s0 = br.read(if wide == 1 { 8 } else { 1 }) as usize;
        if s0 < alphabet {
            lengths[s0] = 1;
        }
        if n == 2 {
            let s1 = br.read(8) as usize;
            if s1 < alphabet {
                lengths[s1] = 1;
            }
        }
    } else {
        let n = br.read(4) as usize + 4;
        let mut cl = [0u8; 19];
        for &o in CODE_LENGTH_ORDER.iter().take(n) {
            cl[o] = br.read(3) as u8;
        }
        let h = Huff::build(&cl)?;
        let mut max_symbol = alphabet;
        if br.read(1) == 1 {
            let nb = 2 + 2 * br.read(3);
            max_symbol = 2 + br.read(nb) as usize;
            if max_symbol > alphabet {
                return Err("the WebP picture has a bad code table".into());
            }
        }
        let (mut sym, mut prev) = (0usize, 8u8);
        while sym < alphabet {
            if max_symbol == 0 {
                break;
            }
            max_symbol -= 1;
            let c = h.decode(br);
            if br.eos {
                return Err("this WebP picture is cut short".into());
            }
            if c < 16 {
                lengths[sym] = c as u8;
                sym += 1;
                if c != 0 {
                    prev = c as u8;
                }
            } else {
                let (extra, off) = match c {
                    16 => (2, 3),
                    17 => (3, 3),
                    _ => (7, 11),
                };
                let rep = br.read(extra) as usize + off;
                if sym + rep > alphabet {
                    return Err("the WebP picture has a bad code table".into());
                }
                let l = if c == 16 { prev } else { 0 };
                for x in &mut lengths[sym..sym + rep] {
                    *x = l;
                }
                sym += rep;
            }
        }
    }
    if br.eos {
        return Err("this WebP picture is cut short".into());
    }
    Huff::build(&lengths)
}

enum Transform {
    Predictor { bits: u32, width: usize, data: Vec<u32> },
    Cross { bits: u32, width: usize, data: Vec<u32> },
    SubGreen,
    Indexed { bits: u32, width: usize, table: Vec<u32> },
}

fn div_round_up(a: usize, b: u32) -> usize {
    (a + (1usize << b) - 1) >> b
}

#[inline]
fn add_px(a: u32, b: u32) -> u32 {
    let ag = (a & 0xff00_ff00).wrapping_add(b & 0xff00_ff00) & 0xff00_ff00;
    let rb = (a & 0x00ff_00ff).wrapping_add(b & 0x00ff_00ff) & 0x00ff_00ff;
    ag | rb
}

#[inline]
fn avg2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xfefe_fefe) >> 1).wrapping_add(a & b)
}

#[inline]
fn chans(a: u32) -> [i32; 4] {
    [(a >> 24) as i32, ((a >> 16) & 255) as i32, ((a >> 8) & 255) as i32, (a & 255) as i32]
}

#[inline]
fn pack(c: [i32; 4]) -> u32 {
    (c[0].clamp(0, 255) as u32) << 24 | (c[1].clamp(0, 255) as u32) << 16 | (c[2].clamp(0, 255) as u32) << 8 | c[3].clamp(0, 255) as u32
}

fn select(l: u32, t: u32, tl: u32) -> u32 {
    let (cl, ct, ctl) = (chans(l), chans(t), chans(tl));
    let mut pl = 0;
    let mut pt = 0;
    for i in 0..4 {
        pl += (ct[i] - ctl[i]).abs();
        pt += (cl[i] - ctl[i]).abs();
    }
    if pl < pt {
        l
    } else {
        t
    }
}

fn predict(mode: u32, l: u32, t: u32, tl: u32, tr: u32) -> u32 {
    match mode {
        1 => l,
        2 => t,
        3 => tr,
        4 => tl,
        5 => avg2(avg2(l, tr), t),
        6 => avg2(l, tl),
        7 => avg2(l, t),
        8 => avg2(tl, t),
        9 => avg2(t, tr),
        10 => avg2(avg2(l, tl), avg2(t, tr)),
        11 => select(l, t, tl),
        12 => {
            let (a, b, c) = (chans(l), chans(t), chans(tl));
            pack([a[0] + b[0] - c[0], a[1] + b[1] - c[1], a[2] + b[2] - c[2], a[3] + b[3] - c[3]])
        }
        13 => {
            let a = chans(avg2(l, t));
            let c = chans(tl);
            pack([a[0] + (a[0] - c[0]) / 2, a[1] + (a[1] - c[1]) / 2, a[2] + (a[2] - c[2]) / 2, a[3] + (a[3] - c[3]) / 2])
        }
        _ => 0xff00_0000,
    }
}

fn inverse_predictor(px: &mut [u32], w: usize, h: usize, bits: u32, data: &[u32]) {
    let bw = div_round_up(w, bits);
    px[0] = add_px(px[0], 0xff00_0000);
    for x in 1..w {
        px[x] = add_px(px[x], px[x - 1]);
    }
    for y in 1..h {
        let row = y * w;
        px[row] = add_px(px[row], px[row - w]);
        for x in 1..w {
            let mode = (data[(y >> bits) * bw + (x >> bits)] >> 8) & 15;
            let i = row + x;
            let p = predict(mode, px[i - 1], px[i - w], px[i - w - 1], px[i - w + 1]);
            px[i] = add_px(px[i], p);
        }
    }
}

fn inverse_cross(px: &mut [u32], w: usize, h: usize, bits: u32, data: &[u32]) {
    let bw = div_round_up(w, bits);
    for y in 0..h {
        for x in 0..w {
            let code = data[(y >> bits) * bw + (x >> bits)];
            let g2r = (code & 255) as u8 as i8 as i32;
            let g2b = ((code >> 8) & 255) as u8 as i8 as i32;
            let r2b = ((code >> 16) & 255) as u8 as i8 as i32;
            let p = px[y * w + x];
            let g = ((p >> 8) & 255) as u8 as i8 as i32;
            let r = ((p >> 16) & 255) as i32;
            let b = (p & 255) as i32;
            let nr = (r + ((g2r * g) >> 5)) & 255;
            let nb = (b + ((g2b * g) >> 5) + ((r2b * (nr as u8 as i8 as i32)) >> 5)) & 255;
            px[y * w + x] = (p & 0xff00_ff00) | (nr as u32) << 16 | nb as u32;
        }
    }
}

fn read_transform(br: &mut BitReader, xsize: &mut usize, ysize: usize, used: &mut u32) -> Result<Transform, String> {
    let t = br.read(2);
    if *used & (1 << t) != 0 {
        return Err("the WebP picture repeats a transform".into());
    }
    *used |= 1 << t;
    Ok(match t {
        0 | 1 => {
            let bits = br.read(3) + 2;
            let data = decode_stream(br, div_round_up(*xsize, bits), div_round_up(ysize, bits), false)?;
            let width = *xsize;
            if t == 0 {
                Transform::Predictor { bits, width, data }
            } else {
                Transform::Cross { bits, width, data }
            }
        }
        2 => Transform::SubGreen,
        _ => {
            let n = br.read(8) as usize + 1;
            let bits = if n > 16 {
                0
            } else if n > 4 {
                1
            } else if n > 2 {
                2
            } else {
                3
            };
            let mut table = decode_stream(br, n, 1, false)?;
            for i in 1..n {
                table[i] = add_px(table[i], table[i - 1]);
            }
            table.resize(1usize << (8 >> bits), 0);
            let width = *xsize;
            *xsize = div_round_up(*xsize, bits);
            Transform::Indexed { bits, width, table }
        }
    })
}

/// One entropy-coded image of `xsize` by `ysize` pixels. At the top level (`level0`) it also reads and undoes the
/// transforms and may use several prefix-code groups.
fn decode_stream(br: &mut BitReader, xsize: usize, ysize: usize, level0: bool) -> Result<Vec<u32>, String> {
    let orig_xsize = xsize;
    let mut xsize = xsize;
    let mut transforms = Vec::new();
    if level0 {
        let mut used = 0u32;
        while br.read(1) == 1 {
            let t = read_transform(br, &mut xsize, ysize, &mut used)?;
            transforms.push(t);
            if br.eos {
                return Err("this WebP picture is cut short".into());
            }
        }
    }
    let mut cache_bits = 0u32;
    if br.read(1) == 1 {
        cache_bits = br.read(4);
        if !(1..=11).contains(&cache_bits) {
            return Err("the WebP picture has a bad colour cache".into());
        }
    }
    let mut meta: Vec<u32> = Vec::new();
    let (mut pbits, mut pw) = (0u32, 0usize);
    let mut groups_n = 1usize;
    if level0 && br.read(1) == 1 {
        pbits = br.read(3) + 2;
        pw = div_round_up(xsize, pbits);
        meta = decode_stream(br, pw, div_round_up(ysize, pbits), false)?;
        groups_n = meta.iter().map(|&p| ((p >> 8) & 0xffff) as usize).max().unwrap_or(0) + 1;
    }
    if br.eos {
        return Err("this WebP picture is cut short".into());
    }
    let cache_size = if cache_bits > 0 { 1usize << cache_bits } else { 0 };
    let mut groups: Vec<[Huff; 5]> = Vec::new();
    for _ in 0..groups_n {
        let g = read_huffman_code(br, 256 + 24 + cache_size)?;
        let r = read_huffman_code(br, 256)?;
        let b = read_huffman_code(br, 256)?;
        let a = read_huffman_code(br, 256)?;
        let d = read_huffman_code(br, 40)?;
        groups.push([g, r, b, a, d]);
    }
    let total = xsize * ysize;
    let mut px = vec![0u32; total];
    let mut cache = vec![0u32; cache_size];
    let (mut pos, mut x, mut y) = (0usize, 0usize, 0usize);
    while pos < total {
        let gi = if meta.is_empty() { 0 } else { ((meta[(y >> pbits) * pw + (x >> pbits)] >> 8) & 0xffff) as usize };
        let g = &groups[gi];
        let s = g[0].decode(br) as usize;
        if br.eos {
            return Err("this WebP picture is cut short".into());
        }
        if s < 256 {
            let r = g[1].decode(br);
            let b = g[2].decode(br);
            let a = g[3].decode(br);
            let p = a << 24 | r << 16 | (s as u32) << 8 | b;
            px[pos] = p;
            if cache_bits > 0 {
                cache[(0x1e35_a7bdu32.wrapping_mul(p) >> (32 - cache_bits)) as usize] = p;
            }
            pos += 1;
            x += 1;
            if x == xsize {
                x = 0;
                y += 1;
            }
        } else if s < 280 {
            let length = copy_value(s as u32 - 256, br) as usize;
            let dsym = g[4].decode(br);
            let dcode = copy_value(dsym, br) as usize;
            if br.eos {
                return Err("this WebP picture is cut short".into());
            }
            let dist = plane_to_dist(xsize, dcode);
            if dist > pos || length > total - pos {
                return Err("the WebP picture has a bad back-reference".into());
            }
            for i in 0..length {
                let p = px[pos + i - dist];
                px[pos + i] = p;
                if cache_bits > 0 {
                    cache[(0x1e35_a7bdu32.wrapping_mul(p) >> (32 - cache_bits)) as usize] = p;
                }
            }
            pos += length;
            x += length;
            while x >= xsize {
                x -= xsize;
                y += 1;
            }
        } else {
            let key = s - 280;
            if key >= cache_size {
                return Err("the WebP picture has a bad colour cache reference".into());
            }
            px[pos] = cache[key];
            pos += 1;
            x += 1;
            if x == xsize {
                x = 0;
                y += 1;
            }
        }
    }
    if br.eos {
        return Err("this WebP picture is cut short".into());
    }
    // undo the transforms, last one first
    let mut width = xsize;
    for t in transforms.iter().rev() {
        match t {
            Transform::Predictor { bits, width: w, data } => {
                inverse_predictor(&mut px, *w, ysize, *bits, data);
            }
            Transform::Cross { bits, width: w, data } => {
                inverse_cross(&mut px, *w, ysize, *bits, data);
            }
            Transform::SubGreen => {
                for p in px.iter_mut() {
                    let g = (*p >> 8) & 255;
                    let rb = ((*p & 0x00ff_00ff).wrapping_add(g << 16 | g)) & 0x00ff_00ff;
                    *p = (*p & 0xff00_ff00) | rb;
                }
            }
            Transform::Indexed { bits, width: w, table } => {
                let mut out = vec![0u32; *w * ysize];
                let mask = (1u32 << (8 >> bits)) - 1;
                for yy in 0..ysize {
                    for xx in 0..*w {
                        let packed = (px[yy * width + (xx >> bits)] >> 8) & 255;
                        let idx = if *bits == 0 { packed } else { (packed >> ((xx as u32 & ((1 << bits) - 1)) * (8 >> bits))) & mask };
                        out[yy * *w + xx] = table[idx as usize];
                    }
                }
                px = out;
                width = *w;
            }
        }
    }
    debug_assert!(!level0 || px.len() == orig_xsize * ysize);
    Ok(px)
}

fn copy_value(sym: u32, br: &mut BitReader) -> u32 {
    if sym < 4 {
        return sym + 1;
    }
    let extra = (sym - 2) >> 1;
    let offset = (2 + (sym & 1)) << extra;
    offset + br.read(extra) + 1
}

const CODE_TO_PLANE: [u8; 120] = [
    0x18, 0x07, 0x17, 0x19, 0x28, 0x06, 0x27, 0x29, 0x16, 0x1a, 0x26, 0x2a, 0x38, 0x05, 0x37, 0x39, 0x15, 0x1b, 0x36, 0x3a, 0x25, 0x2b,
    0x48, 0x04, 0x47, 0x49, 0x14, 0x1c, 0x35, 0x3b, 0x46, 0x4a, 0x24, 0x2c, 0x58, 0x45, 0x4b, 0x34, 0x3c, 0x03, 0x57, 0x59, 0x13, 0x1d,
    0x56, 0x5a, 0x23, 0x2d, 0x44, 0x4c, 0x55, 0x5b, 0x33, 0x3d, 0x68, 0x02, 0x67, 0x69, 0x12, 0x1e, 0x66, 0x6a, 0x22, 0x2e, 0x54, 0x5c,
    0x43, 0x4d, 0x65, 0x6b, 0x32, 0x3e, 0x78, 0x01, 0x77, 0x79, 0x53, 0x5d, 0x11, 0x1f, 0x64, 0x6c, 0x42, 0x4e, 0x76, 0x7a, 0x21, 0x2f,
    0x75, 0x7b, 0x31, 0x3f, 0x63, 0x6d, 0x52, 0x5e, 0x00, 0x74, 0x7c, 0x41, 0x4f, 0x10, 0x20, 0x62, 0x6e, 0x30, 0x73, 0x7d, 0x51, 0x5f,
    0x40, 0x72, 0x7e, 0x61, 0x6f, 0x50, 0x71, 0x7f, 0x60, 0x70,
];

fn plane_to_dist(xsize: usize, code: usize) -> usize {
    if code > 120 {
        return code - 120;
    }
    let dc = CODE_TO_PLANE[code - 1] as i64;
    let yoff = dc >> 4;
    let xoff = 8 - (dc & 15);
    let d = yoff * xsize as i64 + xoff;
    if d >= 1 {
        d as usize
    } else {
        1
    }
}

fn decode_vp8l(d: &[u8]) -> Result<(u32, u32, Vec<u32>, bool), String> {
    let mut br = BitReader::new(d);
    if br.read(8) != 0x2f {
        return Err("this is not a lossless WebP picture".into());
    }
    let w = br.read(14) + 1;
    let h = br.read(14) + 1;
    let alpha = br.read(1) == 1;
    if br.read(3) != 0 {
        return Err("this lossless WebP version is not supported".into());
    }
    if br.eos {
        return Err("this WebP picture is cut short".into());
    }
    check_size(w, h)?;
    let px = decode_stream(&mut br, w as usize, h as usize, true)?;
    Ok((w, h, px, alpha))
}

// ---------------------------------------------------------------------------------------------------------------
// VP8 (lossy)

struct Bool<'a> {
    d: &'a [u8],
    pos: usize,
    value: u64,
    range: u32, // the real range minus one
    bits: i32,
    eof: bool,
}

impl<'a> Bool<'a> {
    fn new(d: &'a [u8]) -> Self {
        let mut b = Bool { d, pos: 0, value: 0, range: 254, bits: -8, eof: false };
        b.load();
        b
    }

    fn load(&mut self) {
        if self.pos < self.d.len() {
            self.value = (self.value << 8) | self.d[self.pos] as u64;
            self.pos += 1;
            self.bits += 8;
        } else if !self.eof {
            self.value <<= 8;
            self.bits += 8;
            self.eof = true;
        } else {
            self.bits = 0;
        }
    }

    #[inline]
    fn get(&mut self, prob: u8) -> u32 {
        if self.bits < 0 {
            self.load();
        }
        let pos = self.bits;
        let split = (self.range * prob as u32) >> 8;
        let value = (self.value >> pos) as u32;
        let mut range;
        let bit;
        if value > split {
            range = self.range - split;
            self.value -= ((split + 1) as u64) << pos;
            bit = 1;
        } else {
            range = split + 1;
            bit = 0;
        }
        let shift = 7 ^ (31 - range.leading_zeros());
        range <<= shift;
        self.bits -= shift as i32;
        self.range = range - 1;
        bit
    }

    fn flag(&mut self) -> bool {
        self.get(0x80) == 1
    }

    fn value(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in (0..n).rev() {
            v |= self.get(0x80) << i;
        }
        v
    }

    fn signed_value(&mut self, n: u32) -> i32 {
        let v = self.value(n) as i32;
        if self.flag() {
            -v
        } else {
            v
        }
    }

    fn signed(&mut self, v: i32) -> i32 {
        if self.get(0x80) == 1 {
            -v
        } else {
            v
        }
    }
}

const BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const CAT3: [u8; 3] = [173, 148, 140];
const CAT4: [u8; 4] = [176, 155, 140, 135];
const CAT5: [u8; 5] = [180, 157, 141, 134, 130];
const CAT6: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

// Sub-block modes, in the order the probability table uses.
const B_DC: u8 = 0;
const B_TM: u8 = 1;
const B_VE: u8 = 2;
const B_HE: u8 = 3;
const B_RD: u8 = 4;
const B_VR: u8 = 5;
const B_LD: u8 = 6;
const B_VL: u8 = 7;
const B_HD: u8 = 8;
const B_HU: u8 = 9;

#[inline]
fn pidx(t: usize, b: usize, c: usize) -> usize {
    ((t * 8 + b) * 3 + c) * 11
}

fn large_value(br: &mut Bool, p: &[u8]) -> i32 {
    if br.get(p[3]) == 0 {
        if br.get(p[4]) == 0 {
            2
        } else {
            3 + br.get(p[5]) as i32
        }
    } else if br.get(p[6]) == 0 {
        if br.get(p[7]) == 0 {
            5 + br.get(159) as i32
        } else {
            let mut v = 7 + 2 * br.get(165) as i32;
            v += br.get(145) as i32;
            v
        }
    } else {
        let bit1 = br.get(p[8]) as usize;
        let bit0 = br.get(p[9 + bit1]) as usize;
        let cat = 2 * bit1 + bit0;
        let tab: &[u8] = match cat {
            0 => &CAT3,
            1 => &CAT4,
            2 => &CAT5,
            _ => &CAT6,
        };
        let mut v = 0i32;
        for &pr in tab {
            v += v + br.get(pr) as i32;
        }
        v + 3 + (8 << cat)
    }
}

/// Read the coefficients of one block into `out` (dequantised, in raster order). Returns the position after the
/// last non-zero coefficient (or `first` when there is none).
fn get_coeffs(br: &mut Bool, probs: &[u8], t: usize, ctx: usize, dq: [i32; 2], first: usize, out: &mut [i16]) -> usize {
    let mut n = first;
    let mut p = pidx(t, BANDS[n], ctx);
    while n < 16 {
        if br.get(probs[p]) == 0 {
            return n;
        }
        while br.get(probs[p + 1]) == 0 {
            n += 1;
            p = pidx(t, BANDS[n], 0);
            if n == 16 {
                return 16;
            }
        }
        let v;
        if br.get(probs[p + 2]) == 0 {
            v = 1;
            p = pidx(t, BANDS[n + 1], 1);
        } else {
            v = large_value(br, &probs[p..p + 11]);
            p = pidx(t, BANDS[n + 1], 2);
        }
        let s = br.signed(v);
        out[ZIGZAG[n]] = (s * dq[(n > 0) as usize]) as i16;
        n += 1;
    }
    16
}

fn wht(inp: &[i16; 16], out: &mut [i16]) {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a0 = inp[i] as i32 + inp[12 + i] as i32;
        let a1 = inp[4 + i] as i32 + inp[8 + i] as i32;
        let a2 = inp[4 + i] as i32 - inp[8 + i] as i32;
        let a3 = inp[i] as i32 - inp[12 + i] as i32;
        tmp[i] = a0 + a1;
        tmp[8 + i] = a0 - a1;
        tmp[4 + i] = a3 + a2;
        tmp[12 + i] = a3 - a2;
    }
    for i in 0..4 {
        let dc = tmp[i * 4] + 3;
        let a0 = dc + tmp[3 + i * 4];
        let a1 = tmp[1 + i * 4] + tmp[2 + i * 4];
        let a2 = tmp[1 + i * 4] - tmp[2 + i * 4];
        let a3 = dc - tmp[3 + i * 4];
        out[(i * 4) * 16] = ((a0 + a1) >> 3) as i16;
        out[(i * 4 + 1) * 16] = ((a3 + a2) >> 3) as i16;
        out[(i * 4 + 2) * 16] = ((a0 - a1) >> 3) as i16;
        out[(i * 4 + 3) * 16] = ((a3 - a2) >> 3) as i16;
    }
}

#[inline]
fn mul1(a: i64) -> i64 {
    ((a * 20091) >> 16) + a
}

#[inline]
fn mul2(a: i64) -> i64 {
    (a * 35468) >> 16
}

const BPS: usize = 32;

#[inline]
fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn idct_add(inp: &[i16], buf: &mut [u8], o: usize) {
    let mut tmp = [0i64; 16];
    for i in 0..4 {
        let (i0, i4, i8, i12) = (inp[i] as i64, inp[4 + i] as i64, inp[8 + i] as i64, inp[12 + i] as i64);
        let a = i0 + i8;
        let b = i0 - i8;
        let c = mul2(i4) - mul1(i12);
        let d = mul1(i4) + mul2(i12);
        tmp[4 * i] = a + d;
        tmp[4 * i + 1] = b + c;
        tmp[4 * i + 2] = b - c;
        tmp[4 * i + 3] = a - d;
    }
    for i in 0..4 {
        let dc = tmp[i] + 4;
        let a = dc + tmp[8 + i];
        let b = dc - tmp[8 + i];
        let c = mul2(tmp[4 + i]) - mul1(tmp[12 + i]);
        let d = mul1(tmp[4 + i]) + mul2(tmp[12 + i]);
        let r = o + i * BPS;
        let put = |v: u8, x: i64| (v as i64 + (x >> 3)).clamp(0, 255) as u8;
        buf[r] = put(buf[r], a + d);
        buf[r + 1] = put(buf[r + 1], b + c);
        buf[r + 2] = put(buf[r + 2], b - c);
        buf[r + 3] = put(buf[r + 3], a - d);
    }
}

#[inline]
fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((a as u32 + 2 * b as u32 + c as u32 + 2) >> 2) as u8
}

#[inline]
fn avg2b(a: u8, b: u8) -> u8 {
    ((a as u32 + b as u32 + 1) >> 1) as u8
}

fn pred4(buf: &mut [u8], o: usize, mode: u8) {
    let mut t = [0u8; 9]; // top-left, then 8 above (4 above, 4 above-right)
    for (i, v) in t.iter_mut().enumerate() {
        *v = buf[o - BPS - 1 + i];
    }
    let (i, j, k, l) = (buf[o - 1], buf[o + BPS - 1], buf[o + 2 * BPS - 1], buf[o + 3 * BPS - 1]);
    let (x, a, b, c, d, e, f, g, h) = (t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8]);
    let mut dd = [[0u8; 4]; 4]; // dd[y][x]
    macro_rules! set {
        ($(($px:expr, $py:expr)),+ = $v:expr) => {{
            let v = $v;
            $( dd[$py][$px] = v; )+
        }};
    }
    match mode {
        B_DC => {
            let s = 4 + [a, b, c, d, i, j, k, l].iter().map(|&v| v as u32).sum::<u32>();
            dd = [[(s >> 3) as u8; 4]; 4];
        }
        B_TM => {
            let lf = [i, j, k, l];
            for yy in 0..4 {
                for xx in 0..4 {
                    dd[yy][xx] = clip8(lf[yy] as i32 + t[xx + 1] as i32 - x as i32);
                }
            }
        }
        B_VE => {
            let v = [avg3(x, a, b), avg3(a, b, c), avg3(b, c, d), avg3(c, d, e)];
            dd = [v; 4];
        }
        B_HE => {
            dd[0] = [avg3(x, i, j); 4];
            dd[1] = [avg3(i, j, k); 4];
            dd[2] = [avg3(j, k, l); 4];
            dd[3] = [avg3(k, l, l); 4];
        }
        B_RD => {
            set!((0, 3) = avg3(j, k, l));
            set!((1, 3), (0, 2) = avg3(i, j, k));
            set!((2, 3), (1, 2), (0, 1) = avg3(x, i, j));
            set!((3, 3), (2, 2), (1, 1), (0, 0) = avg3(a, x, i));
            set!((3, 2), (2, 1), (1, 0) = avg3(b, a, x));
            set!((3, 1), (2, 0) = avg3(c, b, a));
            set!((3, 0) = avg3(d, c, b));
        }
        B_VR => {
            set!((0, 0), (1, 2) = avg2b(x, a));
            set!((1, 0), (2, 2) = avg2b(a, b));
            set!((2, 0), (3, 2) = avg2b(b, c));
            set!((3, 0) = avg2b(c, d));
            set!((0, 3) = avg3(k, j, i));
            set!((0, 2) = avg3(j, i, x));
            set!((0, 1), (1, 3) = avg3(i, x, a));
            set!((1, 1), (2, 3) = avg3(x, a, b));
            set!((2, 1), (3, 3) = avg3(a, b, c));
            set!((3, 1) = avg3(b, c, d));
        }
        B_LD => {
            set!((0, 0) = avg3(a, b, c));
            set!((1, 0), (0, 1) = avg3(b, c, d));
            set!((2, 0), (1, 1), (0, 2) = avg3(c, d, e));
            set!((3, 0), (2, 1), (1, 2), (0, 3) = avg3(d, e, f));
            set!((3, 1), (2, 2), (1, 3) = avg3(e, f, g));
            set!((3, 2), (2, 3) = avg3(f, g, h));
            set!((3, 3) = avg3(g, h, h));
        }
        B_VL => {
            set!((0, 0) = avg2b(a, b));
            set!((1, 0), (0, 2) = avg2b(b, c));
            set!((2, 0), (1, 2) = avg2b(c, d));
            set!((3, 0), (2, 2) = avg2b(d, e));
            set!((0, 1) = avg3(a, b, c));
            set!((1, 1), (0, 3) = avg3(b, c, d));
            set!((2, 1), (1, 3) = avg3(c, d, e));
            set!((3, 1), (2, 3) = avg3(d, e, f));
            set!((3, 2) = avg3(e, f, g));
            set!((3, 3) = avg3(f, g, h));
        }
        B_HD => {
            set!((0, 0), (2, 1) = avg2b(i, x));
            set!((0, 1), (2, 2) = avg2b(j, i));
            set!((0, 2), (2, 3) = avg2b(k, j));
            set!((0, 3) = avg2b(l, k));
            set!((3, 0) = avg3(a, b, c));
            set!((2, 0) = avg3(x, a, b));
            set!((1, 0), (3, 1) = avg3(i, x, a));
            set!((1, 1), (3, 2) = avg3(j, i, x));
            set!((1, 2), (3, 3) = avg3(k, j, i));
            set!((1, 3) = avg3(l, k, j));
        }
        _ => {
            // B_HU
            set!((0, 0) = avg2b(i, j));
            set!((2, 0), (0, 1) = avg2b(j, k));
            set!((2, 1), (0, 2) = avg2b(k, l));
            set!((1, 0) = avg3(i, j, k));
            set!((3, 0), (1, 1) = avg3(j, k, l));
            set!((3, 1), (1, 2) = avg3(k, l, l));
            set!((3, 2), (2, 2), (0, 3), (1, 3), (2, 3), (3, 3) = l);
        }
    }
    for (yy, row) in dd.iter().enumerate() {
        buf[o + yy * BPS..o + yy * BPS + 4].copy_from_slice(row);
    }
}

/// Whole-block predictors: 16x16 luma (`n` = 16) and 8x8 chroma (`n` = 8). Modes: 0 DC, 1 TM, 2 vertical, 3 horizontal.
fn pred_block(buf: &mut [u8], o: usize, n: usize, mode: u8, have_top: bool, have_left: bool) {
    match mode {
        1 => {
            let tl = buf[o - BPS - 1] as i32;
            for y in 0..n {
                let l = buf[o + y * BPS - 1] as i32;
                for x in 0..n {
                    buf[o + y * BPS + x] = clip8(l + buf[o - BPS + x] as i32 - tl);
                }
            }
        }
        2 => {
            for y in 0..n {
                for x in 0..n {
                    buf[o + y * BPS + x] = buf[o - BPS + x];
                }
            }
        }
        3 => {
            for y in 0..n {
                let l = buf[o + y * BPS - 1];
                for x in 0..n {
                    buf[o + y * BPS + x] = l;
                }
            }
        }
        _ => {
            let sum_top: u32 = (0..n).map(|x| buf[o - BPS + x] as u32).sum();
            let sum_left: u32 = (0..n).map(|y| buf[o + y * BPS - 1] as u32).sum();
            let shift = if n == 16 { 4 } else { 3 };
            let dc = match (have_top, have_left) {
                (true, true) => (sum_top + sum_left + n as u32) >> (shift + 1),
                (true, false) => (sum_top + (n as u32 >> 1)) >> shift,
                (false, true) => (sum_left + (n as u32 >> 1)) >> shift,
                (false, false) => 0x80,
            } as u8;
            for y in 0..n {
                for x in 0..n {
                    buf[o + y * BPS + x] = dc;
                }
            }
        }
    }
}

struct Quant {
    y1: [i32; 2],
    y2: [i32; 2],
    uv: [i32; 2],
}

#[derive(Clone, Copy, Default)]
struct FilterInfo {
    limit: u8,
    ilevel: u8,
    hev: u8,
    inner: bool,
}

struct Frame {
    w: usize,
    h: usize,
    ys: usize,
    uvs: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

fn decode_vp8(d: &[u8]) -> Result<Frame, String> {
    const CUT: &str = "this WebP picture is cut short";
    if d.len() < 10 {
        return Err(CUT.into());
    }
    let tag = le24(d);
    if tag & 1 != 0 {
        return Err("this WebP picture is not a key frame".into());
    }
    if (tag >> 1) & 7 > 3 {
        return Err("this WebP picture uses an unknown VP8 version".into());
    }
    if (tag >> 4) & 1 == 0 {
        return Err("this WebP frame is not meant to be shown".into());
    }
    let part0 = (tag >> 5) as usize;
    if d[3..6] != [0x9d, 0x01, 0x2a] {
        return Err("this WebP picture has a bad start code".into());
    }
    let w = (d[6] as u32 | (d[7] as u32) << 8) & 0x3fff;
    let h = (d[8] as u32 | (d[9] as u32) << 8) & 0x3fff;
    check_size(w, h)?;
    let (w, h) = (w as usize, h as usize);
    let rest = &d[10..];
    if part0 > rest.len() {
        return Err(CUT.into());
    }
    let mut br = Bool::new(&rest[..part0]);

    br.flag(); // colour space
    br.flag(); // clamping type (we always clamp)

    // segments
    let use_segment = br.flag();
    let (mut update_map, mut absolute) = (false, false);
    let mut seg_quant = [0i32; 4];
    let mut seg_filter = [0i32; 4];
    let mut seg_probs = [255u8; 3];
    if use_segment {
        update_map = br.flag();
        if br.flag() {
            absolute = br.flag();
            for q in seg_quant.iter_mut() {
                *q = if br.flag() { br.signed_value(7) } else { 0 };
            }
            for f in seg_filter.iter_mut() {
                *f = if br.flag() { br.signed_value(6) } else { 0 };
            }
        }
        if update_map {
            for p in seg_probs.iter_mut() {
                *p = if br.flag() { br.value(8) as u8 } else { 255 };
            }
        }
    }

    // loop filter
    let simple = br.flag();
    let level = br.value(6) as i32;
    let sharpness = br.value(3) as i32;
    let use_lf_delta = br.flag();
    let (mut ref_delta, mut mode_delta) = ([0i32; 4], [0i32; 4]);
    if use_lf_delta && br.flag() {
        for x in ref_delta.iter_mut() {
            if br.flag() {
                *x = br.signed_value(6);
            }
        }
        for x in mode_delta.iter_mut() {
            if br.flag() {
                *x = br.signed_value(6);
            }
        }
    }
    let filter_type = if level == 0 {
        0
    } else if simple {
        1
    } else {
        2
    };

    // token partitions
    let nparts = 1usize << br.value(2);
    let after = &rest[part0..];
    let table = 3 * (nparts - 1);
    if after.len() < table {
        return Err(CUT.into());
    }
    let mut data = &after[table..];
    if data.is_empty() {
        return Err(CUT.into());
    }
    let mut parts: Vec<Bool> = Vec::with_capacity(nparts);
    for i in 0..nparts {
        let sz = if i + 1 < nparts { (le24(&after[3 * i..]) as usize).min(data.len()) } else { data.len() };
        parts.push(Bool::new(&data[..sz]));
        data = &data[sz..];
    }

    // quantisers
    let base_q = br.value(7) as i32;
    let mut dq = [0i32; 5];
    for x in dq.iter_mut() {
        *x = if br.flag() { br.signed_value(4) } else { 0 };
    }
    let mut quants: Vec<Quant> = Vec::new();
    for s in 0..4 {
        let mut q = base_q;
        if use_segment {
            q = seg_quant[s];
            if !absolute {
                q += base_q;
            }
        } else if s > 0 {
            let (y1, y2, uv) = (quants[0].y1, quants[0].y2, quants[0].uv);
            quants.push(Quant { y1, y2, uv });
            continue;
        }
        let idx = |extra: i32, max: i32| (q + extra).clamp(0, max) as usize;
        let y2ac = (AC_TABLE[idx(dq[2], 127)] as i32 * 101581) >> 16;
        quants.push(Quant {
            y1: [DC_TABLE[idx(dq[0], 127)] as i32, AC_TABLE[idx(0, 127)] as i32],
            y2: [DC_TABLE[idx(dq[1], 127)] as i32 * 2, y2ac.max(8)],
            uv: [DC_TABLE[idx(dq[3], 117)] as i32, AC_TABLE[idx(dq[4], 127)] as i32],
        });
    }

    br.flag(); // whether to refresh probabilities: irrelevant for one frame

    let mut probs = COEFF_PROBS;
    for i in 0..1056 {
        if br.get(COEFF_UPDATE[i]) == 1 {
            probs[i] = br.value(8) as u8;
        }
    }
    let use_skip = br.flag();
    let skip_p = if use_skip { br.value(8) as u8 } else { 0 };
    if br.eof {
        return Err(CUT.into());
    }

    // filter strengths for each segment and mode
    let mut fstr = [[FilterInfo::default(); 2]; 4];
    for s in 0..4 {
        let base = if use_segment {
            let mut b = seg_filter[s];
            if !absolute {
                b += level;
            }
            b
        } else {
            level
        };
        for i4 in 0..2 {
            let mut lv = base;
            if use_lf_delta {
                lv += ref_delta[0];
                if i4 == 1 {
                    lv += mode_delta[0];
                }
            }
            let lv = lv.clamp(0, 63);
            let mut info = FilterInfo { inner: i4 == 1, ..Default::default() };
            if lv > 0 {
                let mut il = lv;
                if sharpness > 0 {
                    il >>= if sharpness > 4 { 2 } else { 1 };
                    il = il.min(9 - sharpness);
                }
                il = il.max(1);
                info.ilevel = il as u8;
                info.limit = (2 * lv + il) as u8;
                info.hev = if lv >= 40 {
                    2
                } else if lv >= 15 {
                    1
                } else {
                    0
                };
            }
            fstr[s][i4] = info;
        }
    }

    let (mbw, mbh) = ((w + 15) / 16, (h + 15) / 16);
    let (ys, uvs) = (mbw * 16, mbw * 8);
    let mut fy = vec![0u8; ys * mbh * 16];
    let mut fu = vec![0u8; uvs * mbh * 8];
    let mut fv = vec![0u8; uvs * mbh * 8];
    let mut finfo = vec![FilterInfo::default(); mbw * mbh];

    let mut top_modes = vec![B_DC; mbw * 4];
    let mut top_y = vec![false; mbw * 4];
    let mut top_u = vec![false; mbw * 2];
    let mut top_v = vec![false; mbw * 2];
    let mut top_dc = vec![false; mbw];

    for mb_y in 0..mbh {
        let mut left_modes = [B_DC; 4];
        let mut left_y = [false; 4];
        let mut left_u = [false; 2];
        let mut left_v = [false; 2];
        let mut left_dc = false;
        let tb = &mut parts[mb_y & (nparts - 1)];
        for mb_x in 0..mbw {
            // modes
            let segment = if update_map {
                if br.get(seg_probs[0]) == 0 {
                    br.get(seg_probs[1]) as usize
                } else {
                    br.get(seg_probs[2]) as usize + 2
                }
            } else {
                0
            };
            let skip = if use_skip { br.get(skip_p) == 1 } else { false };
            let is_i4 = br.get(145) == 0;
            let mut imodes = [0u8; 16];
            let mut ymode = 0u8;
            if !is_i4 {
                ymode = if br.get(156) == 1 {
                    if br.get(128) == 1 {
                        1
                    } else {
                        3
                    }
                } else if br.get(163) == 1 {
                    2
                } else {
                    0
                };
                for i in 0..4 {
                    top_modes[mb_x * 4 + i] = ymode;
                }
                left_modes = [ymode; 4];
            } else {
                for by in 0..4 {
                    let mut m = left_modes[by];
                    for bx in 0..4 {
                        let pr = &BMODE_PROBS[(top_modes[mb_x * 4 + bx] as usize * 10 + m as usize) * 9..][..9];
                        m = read_bmode(&mut br, pr);
                        top_modes[mb_x * 4 + bx] = m;
                        imodes[by * 4 + bx] = m;
                    }
                    left_modes[by] = m;
                }
            }
            let uvmode = if br.get(142) == 0 {
                0
            } else if br.get(114) == 0 {
                2
            } else if br.get(183) == 1 {
                1
            } else {
                3
            };

            // residuals
            let mut coeffs = [0i16; 384];
            let mut any_nz = false;
            if !skip {
                let q = &quants[segment];
                let mut first = 0;
                let mut t_ac = 3;
                if !is_i4 {
                    let mut dc = [0i16; 16];
                    let ctx = top_dc[mb_x] as usize + left_dc as usize;
                    let nz = get_coeffs(tb, &probs, 1, ctx, q.y2, 0, &mut dc);
                    top_dc[mb_x] = nz > 0;
                    left_dc = nz > 0;
                    wht(&dc, &mut coeffs);
                    first = 1;
                    t_ac = 0;
                }
                for by in 0..4 {
                    for bx in 0..4 {
                        let ctx = left_y[by] as usize + top_y[mb_x * 4 + bx] as usize;
                        let blk = by * 4 + bx;
                        let nz = get_coeffs(tb, &probs, t_ac, ctx, q.y1, first, &mut coeffs[blk * 16..blk * 16 + 16]);
                        let f = nz > first;
                        left_y[by] = f;
                        top_y[mb_x * 4 + bx] = f;
                        any_nz |= nz > 1 || coeffs[blk * 16] != 0;
                    }
                }
                for ch in 0..2 {
                    let (top, left) = if ch == 0 { (&mut top_u, &mut left_u) } else { (&mut top_v, &mut left_v) };
                    for by in 0..2 {
                        for bx in 0..2 {
                            let ctx = left[by] as usize + top[mb_x * 2 + bx] as usize;
                            let o = 256 + ch * 64 + (by * 2 + bx) * 16;
                            let nz = get_coeffs(tb, &probs, 2, ctx, q.uv, 0, &mut coeffs[o..o + 16]);
                            left[by] = nz > 0;
                            top[mb_x * 2 + bx] = nz > 0;
                            any_nz |= nz > 1 || coeffs[o] != 0;
                        }
                    }
                }
            } else {
                for i in 0..4 {
                    top_y[mb_x * 4 + i] = false;
                }
                top_u[mb_x * 2] = false;
                top_u[mb_x * 2 + 1] = false;
                top_v[mb_x * 2] = false;
                top_v[mb_x * 2 + 1] = false;
                left_y = [false; 4];
                left_u = [false; 2];
                left_v = [false; 2];
                if !is_i4 {
                    top_dc[mb_x] = false;
                    left_dc = false;
                }
            }
            if filter_type > 0 {
                let mut fi = fstr[segment][is_i4 as usize];
                fi.inner |= any_nz;
                finfo[mb_y * mbw + mb_x] = fi;
            }

            // reconstruction in a work area with a border row above and a border column to the left
            let mut yw = [0u8; 17 * BPS];
            let mut uw = [0u8; 9 * BPS];
            let mut vw = [0u8; 9 * BPS];
            if mb_y == 0 {
                yw[..21].fill(127);
                uw[..10].fill(127);
                vw[..10].fill(127);
            } else {
                let base = (mb_y * 16 - 1) * ys + mb_x * 16;
                yw[0] = if mb_x == 0 { 129 } else { fy[base - 1] };
                yw[1..17].copy_from_slice(&fy[base..base + 16]);
                for i in 0..4 {
                    yw[17 + i] = if mb_x + 1 < mbw { fy[base + 16 + i] } else { fy[base + 15] };
                }
                let cb = (mb_y * 8 - 1) * uvs + mb_x * 8;
                uw[0] = if mb_x == 0 { 129 } else { fu[cb - 1] };
                vw[0] = if mb_x == 0 { 129 } else { fv[cb - 1] };
                uw[1..9].copy_from_slice(&fu[cb..cb + 8]);
                vw[1..9].copy_from_slice(&fv[cb..cb + 8]);
            }
            for r in 0..16 {
                yw[(r + 1) * BPS] = if mb_x == 0 { 129 } else { fy[(mb_y * 16 + r) * ys + mb_x * 16 - 1] };
            }
            for r in 0..8 {
                let (a, b) = if mb_x == 0 { (129, 129) } else { (fu[(mb_y * 8 + r) * uvs + mb_x * 8 - 1], fv[(mb_y * 8 + r) * uvs + mb_x * 8 - 1]) };
                uw[(r + 1) * BPS] = a;
                vw[(r + 1) * BPS] = b;
            }
            for r in [3usize, 7, 11] {
                for i in 0..4 {
                    yw[(r + 1) * BPS + 17 + i] = yw[17 + i];
                }
            }
            let o = BPS + 1;
            if is_i4 {
                for by in 0..4 {
                    for bx in 0..4 {
                        let ob = o + by * 4 * BPS + bx * 4;
                        pred4(&mut yw, ob, imodes[by * 4 + bx]);
                        let blk = by * 4 + bx;
                        idct_add(&coeffs[blk * 16..blk * 16 + 16], &mut yw, ob);
                    }
                }
            } else {
                pred_block(&mut yw, o, 16, ymode, mb_y > 0, mb_x > 0);
                for blk in 0..16 {
                    let ob = o + (blk / 4) * 4 * BPS + (blk % 4) * 4;
                    idct_add(&coeffs[blk * 16..blk * 16 + 16], &mut yw, ob);
                }
            }
            for (ch, wbuf) in [&mut uw, &mut vw].into_iter().enumerate() {
                pred_block(wbuf, o, 8, uvmode, mb_y > 0, mb_x > 0);
                for blk in 0..4 {
                    let ob = o + (blk / 2) * 4 * BPS + (blk % 2) * 4;
                    let co = 256 + ch * 64 + blk * 16;
                    idct_add(&coeffs[co..co + 16], wbuf, ob);
                }
            }
            for r in 0..16 {
                let dst = (mb_y * 16 + r) * ys + mb_x * 16;
                fy[dst..dst + 16].copy_from_slice(&yw[(r + 1) * BPS + 1..(r + 1) * BPS + 17]);
            }
            for r in 0..8 {
                let dst = (mb_y * 8 + r) * uvs + mb_x * 8;
                fu[dst..dst + 8].copy_from_slice(&uw[(r + 1) * BPS + 1..(r + 1) * BPS + 9]);
                fv[dst..dst + 8].copy_from_slice(&vw[(r + 1) * BPS + 1..(r + 1) * BPS + 9]);
            }
        }
        if br.eof || tb.eof {
            return Err(CUT.into());
        }
    }

    if filter_type > 0 {
        for mb_y in 0..mbh {
            for mb_x in 0..mbw {
                let fi = finfo[mb_y * mbw + mb_x];
                if fi.limit == 0 {
                    continue;
                }
                filter_mb(&mut fy, &mut fu, &mut fv, ys, uvs, mb_x, mb_y, fi, filter_type == 1);
            }
        }
    }
    Ok(Frame { w, h, ys, uvs, y: fy, u: fu, v: fv })
}

fn read_bmode(br: &mut Bool, p: &[u8]) -> u8 {
    if br.get(p[0]) == 0 {
        return B_DC;
    }
    if br.get(p[1]) == 0 {
        return B_TM;
    }
    if br.get(p[2]) == 0 {
        return B_VE;
    }
    if br.get(p[3]) == 0 {
        if br.get(p[4]) == 0 {
            return B_HE;
        }
        return if br.get(p[5]) == 0 { B_RD } else { B_VR };
    }
    if br.get(p[6]) == 0 {
        return B_LD;
    }
    if br.get(p[7]) == 0 {
        return B_VL;
    }
    if br.get(p[8]) == 0 {
        B_HD
    } else {
        B_HU
    }
}

// loop filter -------------------------------------------------------------------------------------------------

#[inline]
fn px(b: &[u8], p: usize, s: usize, k: isize) -> i32 {
    b[(p as isize + k * s as isize) as usize] as i32
}

fn do_filter2(b: &mut [u8], p: usize, s: usize) {
    let (p1, p0, q0, q1) = (px(b, p, s, -2), px(b, p, s, -1), px(b, p, s, 0), px(b, p, s, 1));
    let a = 3 * (q0 - p0) + (p1 - q1).clamp(-128, 127);
    let a1 = ((a + 4) >> 3).clamp(-16, 15);
    let a2 = ((a + 3) >> 3).clamp(-16, 15);
    b[p - s] = clip8(p0 + a2);
    b[p] = clip8(q0 - a1);
}

fn do_filter4(b: &mut [u8], p: usize, s: usize) {
    let (p1, p0, q0, q1) = (px(b, p, s, -2), px(b, p, s, -1), px(b, p, s, 0), px(b, p, s, 1));
    let a = 3 * (q0 - p0);
    let a1 = ((a + 4) >> 3).clamp(-16, 15);
    let a2 = ((a + 3) >> 3).clamp(-16, 15);
    let a3 = (a1 + 1) >> 1;
    b[p - 2 * s] = clip8(p1 + a3);
    b[p - s] = clip8(p0 + a2);
    b[p] = clip8(q0 - a1);
    b[p + s] = clip8(q1 - a3);
}

fn do_filter6(b: &mut [u8], p: usize, s: usize) {
    let (p2, p1, p0) = (px(b, p, s, -3), px(b, p, s, -2), px(b, p, s, -1));
    let (q0, q1, q2) = (px(b, p, s, 0), px(b, p, s, 1), px(b, p, s, 2));
    let a = (3 * (q0 - p0) + (p1 - q1).clamp(-128, 127)).clamp(-128, 127);
    let a1 = (27 * a + 63) >> 7;
    let a2 = (18 * a + 63) >> 7;
    let a3 = (9 * a + 63) >> 7;
    b[p - 3 * s] = clip8(p2 + a3);
    b[p - 2 * s] = clip8(p1 + a2);
    b[p - s] = clip8(p0 + a1);
    b[p] = clip8(q0 - a1);
    b[p + s] = clip8(q1 - a2);
    b[p + 2 * s] = clip8(q2 - a3);
}

fn hev(b: &[u8], p: usize, s: usize, thresh: i32) -> bool {
    (px(b, p, s, -2) - px(b, p, s, -1)).abs() > thresh || (px(b, p, s, 1) - px(b, p, s, 0)).abs() > thresh
}

fn needs_filter(b: &[u8], p: usize, s: usize, t: i32) -> bool {
    4 * (px(b, p, s, -1) - px(b, p, s, 0)).abs() + (px(b, p, s, -2) - px(b, p, s, 1)).abs() <= t
}

fn needs_filter2(b: &[u8], p: usize, s: usize, t: i32, it: i32) -> bool {
    if !needs_filter(b, p, s, t) {
        return false;
    }
    let (p3, p2, p1, p0) = (px(b, p, s, -4), px(b, p, s, -3), px(b, p, s, -2), px(b, p, s, -1));
    let (q0, q1, q2, q3) = (px(b, p, s, 0), px(b, p, s, 1), px(b, p, s, 2), px(b, p, s, 3));
    (p3 - p2).abs() <= it && (p2 - p1).abs() <= it && (p1 - p0).abs() <= it && (q3 - q2).abs() <= it && (q2 - q1).abs() <= it && (q1 - q0).abs() <= it
}

fn simple_loop(b: &mut [u8], p: usize, hs: usize, vs: usize, thresh: i32) {
    let t2 = 2 * thresh + 1;
    for i in 0..16 {
        let pp = p + i * vs;
        if needs_filter(b, pp, hs, t2) {
            do_filter2(b, pp, hs);
        }
    }
}

fn loop26(b: &mut [u8], p: usize, hs: usize, vs: usize, size: usize, thresh: i32, ithresh: i32, hev_t: i32) {
    let t2 = 2 * thresh + 1;
    for i in 0..size {
        let pp = p + i * vs;
        if needs_filter2(b, pp, hs, t2, ithresh) {
            if hev(b, pp, hs, hev_t) {
                do_filter2(b, pp, hs);
            } else {
                do_filter6(b, pp, hs);
            }
        }
    }
}

fn loop24(b: &mut [u8], p: usize, hs: usize, vs: usize, size: usize, thresh: i32, ithresh: i32, hev_t: i32) {
    let t2 = 2 * thresh + 1;
    for i in 0..size {
        let pp = p + i * vs;
        if needs_filter2(b, pp, hs, t2, ithresh) {
            if hev(b, pp, hs, hev_t) {
                do_filter2(b, pp, hs);
            } else {
                do_filter4(b, pp, hs);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn filter_mb(fy: &mut [u8], fu: &mut [u8], fv: &mut [u8], ys: usize, uvs: usize, mb_x: usize, mb_y: usize, fi: FilterInfo, simple: bool) {
    let limit = fi.limit as i32;
    let il = fi.ilevel as i32;
    let hv = fi.hev as i32;
    let yo = mb_y * 16 * ys + mb_x * 16;
    let co = mb_y * 8 * uvs + mb_x * 8;
    if simple {
        if mb_x > 0 {
            simple_loop(fy, yo, 1, ys, limit + 4);
        }
        if fi.inner {
            for k in 1..4 {
                simple_loop(fy, yo + 4 * k, 1, ys, limit);
            }
        }
        if mb_y > 0 {
            simple_loop(fy, yo, ys, 1, limit + 4);
        }
        if fi.inner {
            for k in 1..4 {
                simple_loop(fy, yo + 4 * k * ys, ys, 1, limit);
            }
        }
        return;
    }
    if mb_x > 0 {
        loop26(fy, yo, 1, ys, 16, limit + 4, il, hv);
        loop26(fu, co, 1, uvs, 8, limit + 4, il, hv);
        loop26(fv, co, 1, uvs, 8, limit + 4, il, hv);
    }
    if fi.inner {
        for k in 1..4 {
            loop24(fy, yo + 4 * k, 1, ys, 16, limit, il, hv);
        }
        loop24(fu, co + 4, 1, uvs, 8, limit, il, hv);
        loop24(fv, co + 4, 1, uvs, 8, limit, il, hv);
    }
    if mb_y > 0 {
        loop26(fy, yo, ys, 1, 16, limit + 4, il, hv);
        loop26(fu, co, uvs, 1, 8, limit + 4, il, hv);
        loop26(fv, co, uvs, 1, 8, limit + 4, il, hv);
    }
    if fi.inner {
        for k in 1..4 {
            loop24(fy, yo + 4 * k * ys, ys, 1, 16, limit, il, hv);
        }
        loop24(fu, co + 4 * uvs, uvs, 1, 8, limit, il, hv);
        loop24(fv, co + 4 * uvs, uvs, 1, 8, limit, il, hv);
    }
}

// colour conversion ----------------------------------------------------------------------------------------------

#[inline]
fn mult_hi(v: i32, c: i32) -> i32 {
    (v * c) >> 8
}

#[inline]
fn clip_yuv(v: i32) -> u8 {
    if v & !16383 == 0 {
        (v >> 6) as u8
    } else if v < 0 {
        0
    } else {
        255
    }
}

/// One output row from a row of luma and two rows of chroma, `a` the nearer one: libwebp's "fancy" upsampling.
fn upsample_row(a: &[u8], b: &[u8], w: usize, out: &mut [u8]) {
    out[0] = ((3 * a[0] as u32 + b[0] as u32 + 2) >> 2) as u8;
    let last = (w - 1) >> 1;
    for x in 1..=last {
        let (tl, t, l, uv) = (a[x - 1] as u32, a[x] as u32, b[x - 1] as u32, b[x] as u32);
        let avg = tl + t + l + uv + 8;
        let d12 = (avg + 2 * (t + l)) >> 3;
        let d03 = (avg + 2 * (tl + uv)) >> 3;
        out[2 * x - 1] = ((d12 + tl) >> 1) as u8;
        out[2 * x] = ((d03 + t) >> 1) as u8;
    }
    if w & 1 == 0 {
        out[w - 1] = ((3 * a[last] as u32 + b[last] as u32 + 2) >> 2) as u8;
    }
}

fn yuv_to_rgba(f: &Frame, alpha: Option<&[u8]>) -> Bitmap {
    let (w, h) = (f.w, f.h);
    let cw = (w + 1) / 2;
    let ch = (h + 1) / 2;
    let mut rgba = vec![255u8; w * h * 4];
    let mut ur = vec![0u8; w];
    let mut vr = vec![0u8; w];
    for r in 0..h {
        let near = r >> 1;
        let far = if r & 1 == 1 { (near + 1).min(ch - 1) } else { near.saturating_sub(1) };
        let (nu, fu_) = (&f.u[near * f.uvs..near * f.uvs + cw], &f.u[far * f.uvs..far * f.uvs + cw]);
        let (nv, fv_) = (&f.v[near * f.uvs..near * f.uvs + cw], &f.v[far * f.uvs..far * f.uvs + cw]);
        upsample_row(nu, fu_, w, &mut ur);
        upsample_row(nv, fv_, w, &mut vr);
        let yrow = &f.y[r * f.ys..r * f.ys + w];
        let out = &mut rgba[r * w * 4..(r + 1) * w * 4];
        for x in 0..w {
            let (y, u, v) = (yrow[x] as i32, ur[x] as i32, vr[x] as i32);
            let yy = mult_hi(y, 19077);
            out[x * 4] = clip_yuv(yy + mult_hi(v, 26149) - 14234);
            out[x * 4 + 1] = clip_yuv(yy - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708);
            out[x * 4 + 2] = clip_yuv(yy + mult_hi(u, 33050) - 17685);
            if let Some(al) = alpha {
                out[x * 4 + 3] = al[r * w + x];
            }
        }
    }
    Bitmap { width: w as u32, height: h as u32, rgba }
}

// the constant tables of the VP8 specification

const DC_TABLE: [u8; 128] = [
    4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17,
    18, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 25, 25, 26, 27, 28,
    29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43,
    44, 45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58,
    59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74,
    75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
    91, 93, 95, 96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118,
    122, 124, 126, 128, 130, 132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
];

const AC_TABLE: [u16; 128] = [
    4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
    36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51,
    52, 53, 54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76,
    78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108,
    110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140, 143, 146, 149, 152,
    155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205, 209,
    213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
];

const COEFF_PROBS: [u8; 1056] = [
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    253, 136, 254, 255, 228, 219, 128, 128, 128, 128, 128,
    189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128,
    106, 126, 227, 252, 214, 209, 255, 255, 128, 128, 128,
    1, 98, 248, 255, 236, 226, 255, 255, 128, 128, 128,
    181, 133, 238, 254, 221, 234, 255, 154, 128, 128, 128,
    78, 134, 202, 247, 198, 180, 255, 219, 128, 128, 128,
    1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128,
    184, 150, 247, 255, 236, 224, 128, 128, 128, 128, 128,
    77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128,
    1, 101, 251, 255, 241, 255, 128, 128, 128, 128, 128,
    170, 139, 241, 252, 236, 209, 255, 255, 128, 128, 128,
    37, 116, 196, 243, 228, 255, 255, 255, 128, 128, 128,
    1, 204, 254, 255, 245, 255, 128, 128, 128, 128, 128,
    207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128,
    102, 103, 231, 255, 211, 171, 128, 128, 128, 128, 128,
    1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128,
    177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128,
    80, 129, 211, 255, 194, 224, 128, 128, 128, 128, 128,
    1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    246, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    198, 35, 237, 223, 193, 187, 162, 160, 145, 155, 62,
    131, 45, 198, 221, 172, 176, 220, 157, 252, 221, 1,
    68, 47, 146, 208, 149, 167, 221, 162, 255, 223, 128,
    1, 149, 241, 255, 221, 224, 255, 255, 128, 128, 128,
    184, 141, 234, 253, 222, 220, 255, 199, 128, 128, 128,
    81, 99, 181, 242, 176, 190, 249, 202, 255, 255, 128,
    1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128,
    99, 121, 210, 250, 201, 198, 255, 202, 128, 128, 128,
    23, 91, 163, 242, 170, 187, 247, 210, 255, 255, 128,
    1, 200, 246, 255, 234, 255, 128, 128, 128, 128, 128,
    109, 178, 241, 255, 231, 245, 255, 255, 128, 128, 128,
    44, 130, 201, 253, 205, 192, 255, 255, 128, 128, 128,
    1, 132, 239, 251, 219, 209, 255, 165, 128, 128, 128,
    94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128,
    22, 100, 174, 245, 186, 161, 255, 199, 128, 128, 128,
    1, 182, 249, 255, 232, 235, 128, 128, 128, 128, 128,
    124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128,
    35, 77, 181, 251, 193, 211, 255, 205, 128, 128, 128,
    1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128,
    121, 141, 235, 255, 225, 227, 255, 255, 128, 128, 128,
    45, 99, 188, 251, 195, 217, 255, 224, 128, 128, 128,
    1, 1, 251, 255, 213, 255, 128, 128, 128, 128, 128,
    203, 1, 248, 255, 255, 128, 128, 128, 128, 128, 128,
    137, 1, 177, 255, 224, 255, 128, 128, 128, 128, 128,
    253, 9, 248, 251, 207, 208, 255, 192, 128, 128, 128,
    175, 13, 224, 243, 193, 185, 249, 198, 255, 255, 128,
    73, 17, 171, 221, 161, 179, 236, 167, 255, 234, 128,
    1, 95, 247, 253, 212, 183, 255, 255, 128, 128, 128,
    239, 90, 244, 250, 211, 209, 255, 255, 128, 128, 128,
    155, 77, 195, 248, 188, 195, 255, 255, 128, 128, 128,
    1, 24, 239, 251, 218, 219, 255, 205, 128, 128, 128,
    201, 51, 219, 255, 196, 186, 128, 128, 128, 128, 128,
    69, 46, 190, 239, 201, 218, 255, 228, 128, 128, 128,
    1, 191, 251, 255, 255, 128, 128, 128, 128, 128, 128,
    223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128,
    141, 124, 248, 255, 255, 128, 128, 128, 128, 128, 128,
    1, 16, 248, 255, 255, 128, 128, 128, 128, 128, 128,
    190, 36, 230, 255, 236, 255, 128, 128, 128, 128, 128,
    149, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    1, 226, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    247, 192, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    1, 134, 252, 255, 255, 128, 128, 128, 128, 128, 128,
    213, 62, 250, 255, 255, 128, 128, 128, 128, 128, 128,
    55, 93, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    202, 24, 213, 235, 186, 191, 220, 160, 240, 175, 255,
    126, 38, 182, 232, 169, 184, 228, 174, 255, 187, 128,
    61, 46, 138, 219, 151, 178, 240, 170, 255, 216, 128,
    1, 112, 230, 250, 199, 191, 247, 159, 255, 255, 128,
    166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128,
    39, 77, 162, 232, 172, 180, 245, 178, 255, 255, 128,
    1, 52, 220, 246, 198, 199, 249, 220, 255, 255, 128,
    124, 74, 191, 243, 183, 193, 250, 221, 255, 255, 128,
    24, 71, 130, 219, 154, 170, 243, 182, 255, 255, 128,
    1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128,
    149, 150, 226, 252, 216, 205, 255, 171, 128, 128, 128,
    28, 108, 170, 242, 183, 194, 254, 223, 255, 255, 128,
    1, 81, 230, 252, 204, 203, 255, 192, 128, 128, 128,
    123, 102, 209, 247, 188, 196, 255, 233, 128, 128, 128,
    20, 95, 153, 243, 164, 173, 255, 203, 128, 128, 128,
    1, 222, 248, 255, 216, 213, 128, 128, 128, 128, 128,
    168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128,
    47, 116, 215, 255, 211, 212, 255, 255, 128, 128, 128,
    1, 121, 236, 253, 212, 214, 255, 255, 128, 128, 128,
    141, 84, 213, 252, 201, 202, 255, 219, 128, 128, 128,
    42, 80, 160, 240, 162, 185, 255, 205, 128, 128, 128,
    1, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    244, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
    238, 1, 255, 128, 128, 128, 128, 128, 128, 128, 128,
];

const COEFF_UPDATE: [u8; 1056] = [
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    176, 246, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255,
    249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255,
    234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    239, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255,
    250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    217, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255,
    234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255,
    255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255,
    255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    249, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255,
    234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255,
    251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255,
    255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    251, 253, 253, 254, 254, 255, 255, 255, 255, 255, 255,
    255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255,
    248, 254, 249, 253, 255, 255, 255, 255, 255, 255, 255,
    255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255,
    255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255,
    248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255,
    255, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    249, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255,
    250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
];

const BMODE_PROBS: [u8; 900] = [
    231, 120, 48, 89, 115, 113, 120, 152, 112,
    152, 179, 64, 126, 170, 118, 46, 70, 95,
    175, 69, 143, 80, 85, 82, 72, 155, 103,
    56, 58, 10, 171, 218, 189, 17, 13, 152,
    114, 26, 17, 163, 44, 195, 21, 10, 173,
    121, 24, 80, 195, 26, 62, 44, 64, 85,
    144, 71, 10, 38, 171, 213, 144, 34, 26,
    170, 46, 55, 19, 136, 160, 33, 206, 71,
    63, 20, 8, 114, 114, 208, 12, 9, 226,
    81, 40, 11, 96, 182, 84, 29, 16, 36,
    134, 183, 89, 137, 98, 101, 106, 165, 148,
    72, 187, 100, 130, 157, 111, 32, 75, 80,
    66, 102, 167, 99, 74, 62, 40, 234, 128,
    41, 53, 9, 178, 241, 141, 26, 8, 107,
    74, 43, 26, 146, 73, 166, 49, 23, 157,
    65, 38, 105, 160, 51, 52, 31, 115, 128,
    104, 79, 12, 27, 217, 255, 87, 17, 7,
    87, 68, 71, 44, 114, 51, 15, 186, 23,
    47, 41, 14, 110, 182, 183, 21, 17, 194,
    66, 45, 25, 102, 197, 189, 23, 18, 22,
    88, 88, 147, 150, 42, 46, 45, 196, 205,
    43, 97, 183, 117, 85, 38, 35, 179, 61,
    39, 53, 200, 87, 26, 21, 43, 232, 171,
    56, 34, 51, 104, 114, 102, 29, 93, 77,
    39, 28, 85, 171, 58, 165, 90, 98, 64,
    34, 22, 116, 206, 23, 34, 43, 166, 73,
    107, 54, 32, 26, 51, 1, 81, 43, 31,
    68, 25, 106, 22, 64, 171, 36, 225, 114,
    34, 19, 21, 102, 132, 188, 16, 76, 124,
    62, 18, 78, 95, 85, 57, 50, 48, 51,
    193, 101, 35, 159, 215, 111, 89, 46, 111,
    60, 148, 31, 172, 219, 228, 21, 18, 111,
    112, 113, 77, 85, 179, 255, 38, 120, 114,
    40, 42, 1, 196, 245, 209, 10, 25, 109,
    88, 43, 29, 140, 166, 213, 37, 43, 154,
    61, 63, 30, 155, 67, 45, 68, 1, 209,
    100, 80, 8, 43, 154, 1, 51, 26, 71,
    142, 78, 78, 16, 255, 128, 34, 197, 171,
    41, 40, 5, 102, 211, 183, 4, 1, 221,
    51, 50, 17, 168, 209, 192, 23, 25, 82,
    138, 31, 36, 171, 27, 166, 38, 44, 229,
    67, 87, 58, 169, 82, 115, 26, 59, 179,
    63, 59, 90, 180, 59, 166, 93, 73, 154,
    40, 40, 21, 116, 143, 209, 34, 39, 175,
    47, 15, 16, 183, 34, 223, 49, 45, 183,
    46, 17, 33, 183, 6, 98, 15, 32, 183,
    57, 46, 22, 24, 128, 1, 54, 17, 37,
    65, 32, 73, 115, 28, 128, 23, 128, 205,
    40, 3, 9, 115, 51, 192, 18, 6, 223,
    87, 37, 9, 115, 59, 77, 64, 21, 47,
    104, 55, 44, 218, 9, 54, 53, 130, 226,
    64, 90, 70, 205, 40, 41, 23, 26, 57,
    54, 57, 112, 184, 5, 41, 38, 166, 213,
    30, 34, 26, 133, 152, 116, 10, 32, 134,
    39, 19, 53, 221, 26, 114, 32, 73, 255,
    31, 9, 65, 234, 2, 15, 1, 118, 73,
    75, 32, 12, 51, 192, 255, 160, 43, 51,
    88, 31, 35, 67, 102, 85, 55, 186, 85,
    56, 21, 23, 111, 59, 205, 45, 37, 192,
    55, 38, 70, 124, 73, 102, 1, 34, 98,
    125, 98, 42, 88, 104, 85, 117, 175, 82,
    95, 84, 53, 89, 128, 100, 113, 101, 45,
    75, 79, 123, 47, 51, 128, 81, 171, 1,
    57, 17, 5, 71, 102, 57, 53, 41, 49,
    38, 33, 13, 121, 57, 73, 26, 1, 85,
    41, 10, 67, 138, 77, 110, 90, 47, 114,
    115, 21, 2, 10, 102, 255, 166, 23, 6,
    101, 29, 16, 10, 85, 128, 101, 196, 26,
    57, 18, 10, 102, 102, 213, 34, 20, 43,
    117, 20, 15, 36, 163, 128, 68, 1, 26,
    102, 61, 71, 37, 34, 53, 31, 243, 192,
    69, 60, 71, 38, 73, 119, 28, 222, 37,
    68, 45, 128, 34, 1, 47, 11, 245, 171,
    62, 17, 19, 70, 146, 85, 55, 62, 70,
    37, 43, 37, 154, 100, 163, 85, 160, 1,
    63, 9, 92, 136, 28, 64, 32, 201, 85,
    75, 15, 9, 9, 64, 255, 184, 119, 16,
    86, 6, 28, 5, 64, 255, 25, 248, 1,
    56, 8, 17, 132, 137, 255, 55, 116, 128,
    58, 15, 20, 82, 135, 57, 26, 121, 40,
    164, 50, 31, 137, 154, 133, 25, 35, 218,
    51, 103, 44, 131, 131, 123, 31, 6, 158,
    86, 40, 64, 135, 148, 224, 45, 183, 128,
    22, 26, 17, 131, 240, 154, 14, 1, 209,
    45, 16, 21, 91, 64, 222, 7, 1, 197,
    56, 21, 39, 155, 60, 138, 23, 102, 213,
    83, 12, 13, 54, 192, 255, 68, 47, 28,
    85, 26, 85, 85, 128, 128, 32, 146, 171,
    18, 11, 7, 63, 144, 171, 4, 4, 246,
    35, 27, 10, 146, 174, 171, 12, 26, 128,
    190, 80, 35, 99, 180, 80, 126, 54, 45,
    85, 126, 47, 87, 176, 51, 41, 20, 32,
    101, 75, 128, 139, 118, 146, 116, 128, 85,
    56, 41, 15, 176, 236, 85, 37, 9, 62,
    71, 30, 17, 119, 118, 255, 17, 18, 138,
    101, 38, 60, 138, 55, 70, 43, 26, 142,
    146, 36, 19, 30, 171, 255, 97, 27, 20,
    138, 45, 61, 62, 219, 1, 81, 188, 64,
    32, 41, 20, 117, 151, 142, 20, 21, 163,
    112, 19, 12, 61, 195, 128, 48, 4, 24,
];


#[cfg(test)]
mod tests {
    use super::*;

    const FILES: [(&str, &[u8]); 10] = [
        ("lossy", include_bytes!("../tests/data/webp_lossy.webp")),
        ("simple filter", include_bytes!("../tests/data/webp_lossy_simple_filter.webp")),
        ("alpha lossless", include_bytes!("../tests/data/webp_lossy_alpha_lossless.webp")),
        ("alpha raw", include_bytes!("../tests/data/webp_lossy_alpha_raw_gradient.webp")),
        ("1x1", include_bytes!("../tests/data/webp_lossy_1x1.webp")),
        ("big", include_bytes!("../tests/data/webp_lossy_big.webp")),
        ("lossless", include_bytes!("../tests/data/webp_lossless.webp")),
        ("lossless opaque", include_bytes!("../tests/data/webp_lossless_opaque.webp")),
        ("palette", include_bytes!("../tests/data/webp_lossless_palette.webp")),
        ("animated", include_bytes!("../tests/data/webp_anim.webp")),
    ];

    #[test]
    fn fixtures_decode() {
        for (name, f) in FILES {
            let b = decode(f).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(b.rgba.len(), (b.width * b.height * 4) as usize, "{name}");
        }
    }

    #[test]
    fn every_truncation_is_an_error_or_a_picture_and_never_a_panic() {
        for (name, f) in FILES {
            for len in 0..f.len() {
                // cutting a RIFF file short must be noticed (the chunk sizes no longer fit) or give a picture
                let r = decode(&f[..len]);
                if len < 12 {
                    assert!(r.is_err(), "{name} at {len}");
                }
            }
        }
    }

    #[test]
    fn flipped_bytes_never_panic() {
        for (_, f) in FILES {
            let mut v = f.to_vec();
            for i in 0..v.len() {
                let x = [0x01u8, 0x80, 0xff][i % 3];
                v[i] ^= x;
                let _ = decode(&v);
                v[i] ^= x;
            }
        }
    }

    #[test]
    fn garbage_is_refused() {
        assert!(decode(b"").is_err());
        assert!(decode(b"RIFF\0\0\0\0WEBP").is_err());
        assert!(decode(b"RIFF\x04\0\0\0WEBPVP8 \xff\xff\xff\xff").is_err());
        let mut x = 12345u32;
        for n in [10usize, 40, 200, 3000] {
            let mut v = b"RIFF".to_vec();
            v.extend_from_slice(&(n as u32 + 4).to_le_bytes());
            v.extend_from_slice(b"WEBPVP8L");
            v.extend_from_slice(&(n as u32).to_le_bytes());
            v.push(0x2f);
            for _ in 0..n {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                v.push((x >> 24) as u8);
            }
            let _ = decode(&v);
        }
    }

    #[test]
    fn huge_declared_sizes_are_refused_without_allocating() {
        // VP8L header: 16384 x 16384 is over the pixel cap
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&18u32.to_le_bytes());
        v.extend_from_slice(b"WEBPVP8L");
        v.extend_from_slice(&5u32.to_le_bytes());
        v.extend_from_slice(&[0x2f, 0xff, 0xff, 0xff, 0x0f, 0]);
        assert!(decode(&v).unwrap_err().contains("too large"));
        // VP8 key frame header with the same size
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&30u32.to_le_bytes());
        v.extend_from_slice(b"WEBPVP8 ");
        v.extend_from_slice(&18u32.to_le_bytes());
        v.extend_from_slice(&[0x10, 0, 0, 0x9d, 0x01, 0x2a, 0xff, 0x3f, 0xff, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(decode(&v).unwrap_err().contains("too large"));
    }

    #[test]
    fn plane_codes_follow_the_specification() {
        assert_eq!(plane_to_dist(100, 1), 100); // directly above
        assert_eq!(plane_to_dist(100, 2), 1); // directly to the left
        assert_eq!(plane_to_dist(100, 121), 1);
        assert_eq!(plane_to_dist(100, 130), 10);
    }
}
