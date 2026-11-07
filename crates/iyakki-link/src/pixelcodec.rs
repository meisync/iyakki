//! The picture codec of protocol version 2 (`docs/REMOTE-PROTOCOL.md`): the screen is cut into 64x64 tiles, only tiles that
//! differ from what the page already has are sent, each tile is coded by what is in it (one colour, a few colours, many
//! colours, photo-like), and the tiles of one message are deflated together (raw deflate, which every current browser
//! decodes itself with `DecompressionStream("deflate-raw")`).
//!
//! Written here, no codec library: the filters are PNG's (None, Sub, Up, Paeth), the compressor is `miniz_oxide`
//! (MIT or Apache-2.0 or Zlib). The lossy modes only touch tiles that look like photographs or video (many colours); text and
//! user-interface tiles are always exact, and a lossy tile is sent again exactly once the picture has been still for a moment.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;

pub const TILE: usize = 64;
pub const KIND_FRAME: u8 = 2;
pub const HEADER: usize = 18;
/// Largest picture either side accepts, in pixels per side (after scale-down).
pub const MAX_SIDE: usize = 4096;
/// Largest message body (all tiles, before deflate) the decoder accepts.
pub const MAX_BODY: usize = 48 * 1024 * 1024;

pub const T_SOLID: u8 = 0;
pub const T_RAW: u8 = 1;
pub const T_PALETTE: u8 = 2;
pub const T_PNG: u8 = 3;
pub const T_YC: u8 = 4;

/// How much picture quality is traded for bytes. Chosen automatically from the measured link, or by the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Lossless = 0,
    Balanced = 1,
    Low = 2,
    VeryLow = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lossy {
    Off,
    /// Each colour channel keeps `levels` values (32 means a step of about 8).
    Quant { levels: u16 },
    /// Luma step `ys`, chroma step `cs` (a power of two up to 128), chroma at half resolution.
    Chroma { ys: u8, cs: u8 },
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Lossless, Level::Balanced, Level::Low, Level::VeryLow];

    pub fn name(self) -> &'static str {
        match self {
            Level::Lossless => "lossless",
            Level::Balanced => "balanced",
            Level::Low => "low",
            Level::VeryLow => "verylow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::Lossless => "Lossless",
            Level::Balanced => "Balanced",
            Level::Low => "Low bandwidth",
            Level::VeryLow => "Very low",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.name() == s)
    }

    pub fn from_u8(n: u8) -> Option<Level> {
        Level::ALL.get(n as usize).copied()
    }

    /// 1 = full size; 2 = the picture is halved before it is coded and the page stretches it back.
    pub fn scale(self) -> u8 {
        if self == Level::VeryLow {
            2
        } else {
            1
        }
    }

    pub fn lossy(self) -> Lossy {
        match self {
            Level::Lossless => Lossy::Off,
            Level::Balanced => Lossy::Quant { levels: 32 },
            Level::Low => Lossy::Chroma { ys: 4, cs: 16 },
            Level::VeryLow => Lossy::Chroma { ys: 12, cs: 32 },
        }
    }

    /// Shortest time between two pictures, in milliseconds (a key or a click sets it aside for one picture).
    pub fn min_interval_ms(self) -> u64 {
        [16, 33, 100, 200][self as usize]
    }

    /// How long a lossy tile must be unchanged before it is sent again exactly.
    pub fn refine_ms(self) -> u64 {
        [0, 250, 400, 600][self as usize]
    }

    fn deflate_level(self) -> u8 {
        [3, 4, 5, 6][self as usize]
    }

    /// The next level with fewer bytes (or this one at the end).
    pub fn lower(self) -> Level {
        Level::from_u8(self as u8 + 1).unwrap_or(self)
    }

    pub fn higher(self) -> Level {
        if self == Level::Lossless {
            self
        } else {
            Level::from_u8(self as u8 - 1).unwrap_or(self)
        }
    }
}

// ---------------------------------------------------------------- filters

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i32, b as i32, c as i32);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn predict(f: u8, cur: &[u8], up: Option<&[u8]>, i: usize, bpp: usize) -> u8 {
    let a = if i >= bpp { cur[i - bpp] } else { 0 };
    let b = up.map_or(0, |u| u[i]);
    let c = if i >= bpp { up.map_or(0, |u| u[i - bpp]) } else { 0 };
    match f {
        1 => a,
        2 => b,
        3 => paeth(a, b, c),
        _ => 0,
    }
}

/// Every row gets a filter byte (0 none, 1 left, 2 up, 3 Paeth) followed by the row; the filter that leaves the smallest
/// sum of absolute (signed) residuals is used, as PNG encoders do.
pub fn filter_rows(data: &[u8], row_len: usize, rows: usize, bpp: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows * (row_len + 1));
    let mut cand = vec![0u8; row_len];
    for y in 0..rows {
        let cur = &data[y * row_len..(y + 1) * row_len];
        let up = (y > 0).then(|| &data[(y - 1) * row_len..y * row_len]);
        let (mut best, mut best_cost) = (0u8, u64::MAX);
        for f in 0..4u8 {
            if f == 2 && up.is_none() {
                continue;
            }
            let mut cost = 0u64;
            for i in 0..row_len {
                let r = cur[i].wrapping_sub(predict(f, cur, up, i, bpp));
                cost += (r as i8).unsigned_abs() as u64;
            }
            if cost < best_cost {
                best_cost = cost;
                best = f;
            }
        }
        for i in 0..row_len {
            cand[i] = cur[i].wrapping_sub(predict(best, cur, up, i, bpp));
        }
        out.push(best);
        out.extend_from_slice(&cand);
    }
    out
}

/// The reverse of [`filter_rows`]; `None` when the data is the wrong length or names a filter that does not exist.
pub fn unfilter_rows(data: &[u8], row_len: usize, rows: usize, bpp: usize) -> Option<Vec<u8>> {
    if data.len() != rows * (row_len + 1) {
        return None;
    }
    let mut out = vec![0u8; rows * row_len];
    for y in 0..rows {
        let f = data[y * (row_len + 1)];
        if f > 3 {
            return None;
        }
        let src = &data[y * (row_len + 1) + 1..(y + 1) * (row_len + 1)];
        let (done, rest) = out.split_at_mut(y * row_len);
        let cur = &mut rest[..row_len];
        let up = (y > 0).then(|| &done[(y - 1) * row_len..]);
        for i in 0..row_len {
            let p = predict(f, cur, up, i, bpp);
            cur[i] = src[i].wrapping_add(p);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- small palette

struct Pal {
    keys: [u32; 1024],
    idx: [u16; 1024],
    colors: Vec<u32>,
}

impl Pal {
    fn new() -> Pal {
        Pal { keys: [u32::MAX; 1024], idx: [0; 1024], colors: Vec::new() }
    }

    /// The index of colour `c`, adding it; `None` once there would be more than 256.
    fn get(&mut self, c: u32) -> Option<u16> {
        let mut h = (c.wrapping_mul(0x9E37_79B1) >> 22) as usize & 1023;
        loop {
            if self.keys[h] == c {
                return Some(self.idx[h]);
            }
            if self.keys[h] == u32::MAX {
                if self.colors.len() >= 256 {
                    return None;
                }
                self.keys[h] = c;
                self.idx[h] = self.colors.len() as u16;
                self.colors.push(c);
                return Some(self.idx[h]);
            }
            h = (h + 1) & 1023;
        }
    }
}

enum Class {
    Solid(u32),
    Few(Vec<u32>, Vec<u8>),
    /// More than 64 colours; the number is capped at 257 (so "257" means "more than 256").
    Many(usize),
}

const FEW: usize = 256;
/// Tiles with more colours than this look like photographs or video and may be coded lossily.
const PHOTO: usize = 192;

fn classify(rgb: &[u8]) -> Class {
    let mut pal = Pal::new();
    let mut idx = Vec::with_capacity(rgb.len() / 3);
    for p in rgb.chunks_exact(3) {
        let c = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        match pal.get(c) {
            Some(i) => idx.push(i as u8),
            None => return Class::Many(257),
        }
    }
    match pal.colors.len() {
        1 => Class::Solid(pal.colors[0]),
        n if n <= FEW => Class::Few(pal.colors, idx),
        n => Class::Many(n),
    }
}

fn bits_for(n: usize) -> usize {
    match n {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    }
}

fn put_palette(out: &mut Vec<u8>, colors: &[u32], idx: &[u8], w: usize, h: usize) {
    out.push((colors.len() - 1) as u8);
    for c in colors {
        out.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, *c as u8]);
    }
    let bits = bits_for(colors.len());
    for y in 0..h {
        let row = &idx[y * w..(y + 1) * w];
        if bits == 8 {
            out.extend_from_slice(row);
            continue;
        }
        let per = 8 / bits;
        for chunk in row.chunks(per) {
            let mut b = 0u8;
            for (k, v) in chunk.iter().enumerate() {
                b |= v << (8 - bits * (k + 1));
            }
            out.push(b);
        }
    }
}

// ---------------------------------------------------------------- lossy

fn quant_table(levels: u16) -> [u8; 256] {
    let l = levels.clamp(2, 256) as f32 - 1.0;
    let mut t = [0u8; 256];
    for (v, o) in t.iter_mut().enumerate() {
        let k = (v as f32 * l / 255.0).round();
        *o = (k * 255.0 / l).round().min(255.0) as u8;
    }
    t
}

fn quant_step(v: f32, step: u32) -> u8 {
    let s = step.max(1) as f32;
    ((v / s).round() * s).clamp(0.0, 255.0) as u8
}

/// Luma plane at full size, and the two chroma planes (green-minus-average and red-minus-blue, centred on 128) at half size.
fn to_yc(rgb: &[u8], w: usize, h: usize, ys: u32, cs: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y_plane = vec![0u8; w * h];
    let mut cg_sum = vec![0f32; cw * ch];
    let mut co_sum = vec![0f32; cw * ch];
    let mut cnt = vec![0f32; cw * ch];
    for y in 0..h {
        for x in 0..w {
            let p = &rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            y_plane[y * w + x] = quant_step((r + 2.0 * g + b) / 4.0, ys);
            let k = (y / 2) * cw + x / 2;
            cg_sum[k] += (2.0 * g - r - b) / 4.0;
            co_sum[k] += (r - b) / 2.0;
            cnt[k] += 1.0;
        }
    }
    let q = |v: f32| quant_step(v + 128.0, cs);
    let cg = cg_sum.iter().zip(&cnt).map(|(s, n)| q(s / n)).collect();
    let co = co_sum.iter().zip(&cnt).map(|(s, n)| q(s / n)).collect();
    (y_plane, cg, co, cw, ch)
}

fn from_yc(y: f32, cg: f32, co: f32) -> [u8; 3] {
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [c(y - cg + co), c(y + cg), c(y - cg - co)]
}

// ---------------------------------------------------------------- tiles

fn gather(src: &[u8], sw: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        for p in src[(row * sw + x) * 4..(row * sw + x + w) * 4].chunks_exact(4) {
            rgb.extend_from_slice(&p[..3]);
        }
    }
    rgb
}

fn png_of(rgb: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w as u32, h as u32)?)?.encode_png().ok()
}

/// What the encoder knows about the page: it can inflate (otherwise tiles are PNG and nothing is deflated).
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub deflate: bool,
}

/// Code one tile. Returns the tile's bytes (header and payload), and whether it was coded lossily.
pub fn encode_tile(out: &mut Vec<u8>, src: &[u8], sw: usize, x: usize, y: usize, w: usize, h: usize, lossy: Lossy, caps: Caps, exact: bool) -> bool {
    let mut rgb = gather(src, sw, x, y, w, h);
    out.extend_from_slice(&(x as u16).to_le_bytes());
    out.extend_from_slice(&(y as u16).to_le_bytes());
    out.push(w as u8);
    out.push(h as u8);
    let class = classify(&rgb);
    match class {
        Class::Solid(c) => {
            out.extend_from_slice(&[T_SOLID, (c >> 16) as u8, (c >> 8) as u8, c as u8]);
            return false;
        }
        Class::Few(colors, idx) if caps.deflate => {
            out.push(T_PALETTE);
            put_palette(out, &colors, &idx, w, h);
            return false;
        }
        Class::Few(..) => {
            put_png(out, &rgb, w, h);
            return false;
        }
        Class::Many(n) => {
            let photo = n > PHOTO && !exact;
            match lossy {
                Lossy::Chroma { ys, cs } if photo && caps.deflate => {
                    let (yp, cg, co, cw, ch) = to_yc(&rgb, w, h, ys as u32, cs as u32);
                    out.push(T_YC);
                    out.extend_from_slice(&filter_rows(&yp, w, h, 1));
                    out.extend_from_slice(&filter_rows(&cg, cw, ch, 1));
                    out.extend_from_slice(&filter_rows(&co, cw, ch, 1));
                    return true;
                }
                Lossy::Chroma { .. } | Lossy::Quant { .. } if photo => {
                    let levels = match lossy {
                        Lossy::Quant { levels } => levels,
                        _ => 32,
                    };
                    let t = quant_table(levels);
                    for v in rgb.iter_mut() {
                        *v = t[*v as usize];
                    }
                    put_plain(out, &rgb, w, h, caps);
                    return true;
                }
                _ => {}
            }
        }
    }
    put_plain(out, &rgb, w, h, caps);
    false
}

fn put_png(out: &mut Vec<u8>, rgb: &[u8], w: usize, h: usize) {
    let png = png_of(rgb, w, h).unwrap_or_default();
    out.push(T_PNG);
    out.extend_from_slice(&(png.len() as u32).to_le_bytes());
    out.extend_from_slice(&png);
}

fn put_plain(out: &mut Vec<u8>, rgb: &[u8], w: usize, h: usize, caps: Caps) {
    if caps.deflate {
        out.push(T_RAW);
        out.extend_from_slice(&filter_rows(rgb, w * 3, h, 3));
    } else {
        put_png(out, rgb, w, h);
    }
}

// ---------------------------------------------------------------- frames

/// A frame message: the 18-byte header and the (usually deflated) tiles.
///
/// `[0]` kind 2, `[1]` flags (bit 0: body is deflated), `[2..6]` sequence (u32), `[6..8]` and `[8..10]` the screen size,
/// `[10]` scale, `[11]` level, `[12..14]` tile count, `[14..18]` length of the body before deflate. All little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    pub scale: u8,
    pub level: Level,
    pub tiles: u16,
}

pub fn build_message(h: &FrameHeader, body: &[u8], caps: Caps, level: Level) -> Vec<u8> {
    let packed = if caps.deflate && body.len() > 64 { Some(compress_to_vec(body, level.deflate_level())) } else { None };
    let (flags, payload): (u8, &[u8]) = match &packed {
        Some(p) if p.len() < body.len() => (1, p),
        _ => (0, body),
    };
    let mut m = Vec::with_capacity(HEADER + payload.len());
    m.push(KIND_FRAME);
    m.push(flags);
    m.extend_from_slice(&h.seq.to_le_bytes());
    m.extend_from_slice(&h.screen_w.to_le_bytes());
    m.extend_from_slice(&h.screen_h.to_le_bytes());
    m.push(h.scale);
    m.push(h.level as u8);
    m.extend_from_slice(&h.tiles.to_le_bytes());
    m.extend_from_slice(&(body.len() as u32).to_le_bytes());
    m.extend_from_slice(payload);
    m
}

// ---------------------------------------------------------------- decoder (the page's job; kept here for tests and tools)

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Short,
    Kind,
    Size,
    Body,
    Tile(&'static str),
}

/// The page's picture, as the decoder rebuilds it.
#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

impl Canvas {
    fn ensure(&mut self, w: usize, h: usize) {
        if (self.w, self.h) != (w, h) {
            *self = Canvas { w, h, rgba: vec![0; w * h * 4] };
        }
    }
}

struct Rd<'a>(&'a [u8], usize);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.1.checked_add(n).filter(|e| *e <= self.0.len()).ok_or(DecodeError::Tile("truncated"))?;
        let s = &self.0[self.1..end];
        self.1 = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
}

/// Decode one frame message onto `canvas` (which takes the scaled size of the picture). Never panics on bad data.
pub fn decode_frame(msg: &[u8], canvas: &mut Canvas) -> Result<FrameHeader, DecodeError> {
    if msg.len() < HEADER {
        return Err(DecodeError::Short);
    }
    if msg[0] != KIND_FRAME {
        return Err(DecodeError::Kind);
    }
    let flags = msg[1];
    let u16at = |i: usize| u16::from_le_bytes([msg[i], msg[i + 1]]);
    let hd = FrameHeader {
        seq: u32::from_le_bytes([msg[2], msg[3], msg[4], msg[5]]),
        screen_w: u16at(6),
        screen_h: u16at(8),
        scale: msg[10],
        level: Level::from_u8(msg[11]).ok_or(DecodeError::Kind)?,
        tiles: u16at(12),
    };
    let body_len = u32::from_le_bytes([msg[14], msg[15], msg[16], msg[17]]) as usize;
    if hd.scale == 0 || hd.scale > 4 || hd.screen_w == 0 || hd.screen_h == 0 {
        return Err(DecodeError::Size);
    }
    let (sw, sh) = ((hd.screen_w as usize).div_ceil(hd.scale as usize), (hd.screen_h as usize).div_ceil(hd.scale as usize));
    if sw > MAX_SIDE || sh > MAX_SIDE || body_len > MAX_BODY {
        return Err(DecodeError::Size);
    }
    let body: Vec<u8> = if flags & 1 != 0 {
        let b = decompress_to_vec_with_limit(&msg[HEADER..], body_len).map_err(|_| DecodeError::Body)?;
        if b.len() != body_len {
            return Err(DecodeError::Body);
        }
        b
    } else {
        if msg.len() - HEADER != body_len {
            return Err(DecodeError::Body);
        }
        msg[HEADER..].to_vec()
    };
    canvas.ensure(sw, sh);
    let mut r = Rd(&body, 0);
    for _ in 0..hd.tiles {
        let hdr = r.take(7)?;
        let (x, y) = (u16::from_le_bytes([hdr[0], hdr[1]]) as usize, u16::from_le_bytes([hdr[2], hdr[3]]) as usize);
        let (w, h, ty) = (hdr[4] as usize, hdr[5] as usize, hdr[6]);
        if w == 0 || h == 0 || x + w > sw || y + h > sh {
            return Err(DecodeError::Tile("outside the picture"));
        }
        let rgb = decode_tile(&mut r, ty, w, h)?;
        for row in 0..h {
            for col in 0..w {
                let s = (row * w + col) * 3;
                let d = ((y + row) * sw + x + col) * 4;
                canvas.rgba[d..d + 4].copy_from_slice(&[rgb[s], rgb[s + 1], rgb[s + 2], 255]);
            }
        }
    }
    if r.1 != body.len() {
        return Err(DecodeError::Body);
    }
    Ok(hd)
}

fn decode_tile(r: &mut Rd, ty: u8, w: usize, h: usize) -> Result<Vec<u8>, DecodeError> {
    match ty {
        T_SOLID => {
            let c = r.take(3)?;
            Ok(c.iter().copied().cycle().take(w * h * 3).collect())
        }
        T_RAW => {
            let data = r.take(h * (w * 3 + 1))?;
            unfilter_rows(data, w * 3, h, 3).ok_or(DecodeError::Tile("bad filter"))
        }
        T_PALETTE => {
            let n = r.u8()? as usize + 1;
            let pal = r.take(n * 3)?;
            let bits = bits_for(n);
            let row_bytes = (w * bits).div_ceil(8);
            let data = r.take(row_bytes * h)?;
            let mut rgb = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                let row = &data[y * row_bytes..(y + 1) * row_bytes];
                for x in 0..w {
                    let i = if bits == 8 { row[x] as usize } else { ((row[x * bits / 8] >> (8 - bits - (x * bits) % 8)) & ((1 << bits) - 1)) as usize };
                    if i >= n {
                        return Err(DecodeError::Tile("palette index"));
                    }
                    rgb.extend_from_slice(&pal[i * 3..i * 3 + 3]);
                }
            }
            Ok(rgb)
        }
        T_PNG => {
            let len = u32::from_le_bytes(r.take(4)?.try_into().expect("4")) as usize;
            let png = r.take(len)?;
            let pm = tiny_skia::Pixmap::decode_png(png).map_err(|_| DecodeError::Tile("png"))?;
            if (pm.width() as usize, pm.height() as usize) != (w, h) {
                return Err(DecodeError::Tile("png size"));
            }
            Ok(pm.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect())
        }
        T_YC => {
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let yp = unfilter_rows(r.take(h * (w + 1))?, w, h, 1).ok_or(DecodeError::Tile("bad filter"))?;
            let cg = unfilter_rows(r.take(ch * (cw + 1))?, cw, ch, 1).ok_or(DecodeError::Tile("bad filter"))?;
            let co = unfilter_rows(r.take(ch * (cw + 1))?, cw, ch, 1).ok_or(DecodeError::Tile("bad filter"))?;
            let mut rgb = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    let k = (y / 2) * cw + x / 2;
                    rgb.extend_from_slice(&from_yc(yp[y * w + x] as f32, cg[k] as f32 - 128.0, co[k] as f32 - 128.0));
                }
            }
            Ok(rgb)
        }
        _ => Err(DecodeError::Tile("unknown tile type")),
    }
}

// ---------------------------------------------------------------- the encoder's memory of what the page has

/// Halve a picture by averaging 2x2 blocks (the last row and column of an odd size are averaged over what exists).
pub fn halve(src: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![255u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let mut s = [0u32; 3];
            let mut n = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                    if sx < w && sy < h {
                        let p = &src[(sy * w + sx) * 4..];
                        s[0] += p[0] as u32;
                        s[1] += p[1] as u32;
                        s[2] += p[2] as u32;
                        n += 1;
                    }
                }
            }
            let o = (y * nw + x) * 4;
            for c in 0..3 {
                out[o + c] = ((s[c] + n / 2) / n) as u8;
            }
        }
    }
    (out, nw, nh)
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub level: Level,
    pub caps: Caps,
    /// Stop adding tiles once the body has this many bytes (the rest follows in the next message).
    pub budget: usize,
    pub now_ms: u64,
}

#[derive(Debug)]
pub struct Encoded {
    pub msg: Vec<u8>,
    pub seq: u32,
    pub tiles: usize,
    pub lossy_tiles: usize,
    /// Tiles sent again exactly after having been lossy.
    pub refined: usize,
    pub body_len: usize,
}

/// Remembers what the page has (per tile: the source pixels last sent, whether that was lossy, when the tile last changed) and
/// builds the next message from the newest picture. Dropping frames costs nothing: the next message is always the difference
/// between the newest picture and what the page has.
pub struct Streamer {
    src: Vec<u8>,
    src_w: usize,
    src_h: usize,
    fresh: bool,
    scaled: Option<(u8, Vec<u8>, usize, usize)>,
    prev: Vec<u8>,
    prev_dims: (usize, usize, u8),
    lossy: Vec<bool>,
    changed_ms: Vec<u64>,
    more: bool,
    refine_due: Option<u64>,
    seq: u32,
}

impl Default for Streamer {
    fn default() -> Self {
        Streamer::new()
    }
}

impl Streamer {
    pub fn new() -> Streamer {
        Streamer { src: Vec::new(), src_w: 0, src_h: 0, fresh: false, scaled: None, prev: Vec::new(), prev_dims: (0, 0, 0), lossy: Vec::new(), changed_ms: Vec::new(), more: false, refine_due: None, seq: 0 }
    }

    /// The page lost its picture (or just connected): the next message holds everything.
    pub fn reset(&mut self) {
        self.prev_dims = (0, 0, 0);
        self.fresh = true;
    }

    /// The newest picture (RGBA, `w * h * 4` bytes; anything else is ignored).
    pub fn set_source(&mut self, w: usize, h: usize, rgba: Vec<u8>) {
        if w == 0 || h == 0 || w > MAX_SIDE * 2 || h > MAX_SIDE * 2 || rgba.len() != w * h * 4 {
            return;
        }
        self.src = rgba;
        self.src_w = w;
        self.src_h = h;
        self.scaled = None;
        self.fresh = true;
    }

    /// Is there something worth trying to encode now (a new picture, tiles left over, a lossy tile due to be sent exactly)?
    pub fn wants_encode(&self, now_ms: u64) -> bool {
        !self.src.is_empty() && (self.fresh || self.more || self.refine_due.is_some_and(|d| now_ms >= d))
    }

    /// Anything the page does not have exactly yet.
    pub fn backlog(&self) -> bool {
        self.more || self.refine_due.is_some()
    }

    /// The message for the difference between the newest picture and what the page has, or `None` when there is none (or
    /// nothing is due). Unsent tiles stay different, so they are found again next time.
    pub fn encode(&mut self, o: &Opts) -> Option<Encoded> {
        if self.src.is_empty() {
            return None;
        }
        self.fresh = false;
        let scale = o.level.scale();
        if self.scaled.as_ref().map(|s| s.0) != Some(scale) {
            self.scaled = Some(if scale == 1 { (1, Vec::new(), self.src_w, self.src_h) } else { let (d, w, h) = halve(&self.src, self.src_w, self.src_h); (scale, d, w, h) });
        }
        let (_, ref sc, sw, sh) = *self.scaled.as_ref().expect("set above");
        let img: &[u8] = if scale == 1 { &self.src } else { sc };
        let dims = (sw, sh, scale);
        let (tx, ty) = (sw.div_ceil(TILE), sh.div_ceil(TILE));
        let full = self.prev_dims != dims;
        if full {
            self.prev = vec![0; sw * sh * 4];
            self.lossy = vec![false; tx * ty];
            self.changed_ms = vec![o.now_ms; tx * ty];
            self.prev_dims = dims;
        }
        let lossy_mode = o.level.lossy();
        let refine_after = o.level.refine_ms();
        let mut body = Vec::new();
        let (mut n, mut n_lossy, mut n_refined) = (0usize, 0usize, 0usize);
        let mut leftover = false;
        let diff: Vec<bool> = (0..tx * ty)
            .map(|k| {
                let (x, y) = ((k % tx) * TILE, (k / tx) * TILE);
                let (w, h) = (TILE.min(sw - x), TILE.min(sh - y));
                full || (0..h).any(|row| {
                    let a = ((y + row) * sw + x) * 4;
                    img[a..a + w * 4] != self.prev[a..a + w * 4]
                })
            })
            .collect();
        // pass 0: tiles that differ; pass 1: lossy tiles that have been still long enough
        for pass in 0..2 {
            for k in 0..tx * ty {
                let (x, y) = ((k % tx) * TILE, (k / tx) * TILE);
                let (w, h) = (TILE.min(sw - x), TILE.min(sh - y));
                let differs = diff[k];
                let take = if pass == 0 { differs } else { !differs && self.lossy[k] && o.now_ms.saturating_sub(self.changed_ms[k]) >= refine_after };
                if !take {
                    continue;
                }
                if body.len() >= o.budget && n > 0 {
                    leftover = true;
                    continue;
                }
                let was_lossy = encode_tile(&mut body, img, sw, x, y, w, h, lossy_mode, o.caps, pass == 1);
                n += 1;
                for row in 0..h {
                    let a = ((y + row) * sw + x) * 4;
                    self.prev[a..a + w * 4].copy_from_slice(&img[a..a + w * 4]);
                }
                if differs {
                    self.changed_ms[k] = o.now_ms;
                }
                if was_lossy {
                    n_lossy += 1;
                }
                if pass == 1 {
                    n_refined += 1;
                }
                self.lossy[k] = was_lossy;
            }
        }
        self.more = leftover;
        self.refine_due = (0..self.lossy.len()).filter(|k| self.lossy[*k]).map(|k| self.changed_ms[k] + refine_after.max(1)).min();
        if n == 0 {
            return None;
        }
        self.seq = self.seq.wrapping_add(1);
        let hd = FrameHeader { seq: self.seq, screen_w: self.src_w as u16, screen_h: self.src_h as u16, scale, level: o.level, tiles: n as u16 };
        let msg = build_message(&hd, &body, o.caps, o.level);
        Some(Encoded { msg, seq: self.seq, tiles: n, lossy_tiles: n_lossy, refined: n_refined, body_len: body.len() })
    }
}
