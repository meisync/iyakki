//! The picture decoder of the client engine: the tile codec of protocol version 2 (`docs/REMOTE-PROTOCOL.md`), decoding side only.
//!
//! This is a copy of the decoder in `iyakki_link::pixelcodec`, kept free of the heavy parts of that crate (TLS, PNG, the encoder) so it
//! builds for wasm32. The test `tests/agree.rs` encodes real pictures with the instance's encoder and checks that both decoders give the
//! same pixels, so the copy cannot drift unnoticed. One difference on purpose: PNG tiles (`T_PNG`) are not decoded. The engine always says
//! `deflate: true` when it opens the picture stream, and an encoder that knows the client can inflate never sends PNG tiles.

use miniz_oxide::inflate::decompress_to_vec_with_limit;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    pub scale: u8,
    pub level: u8,
    pub tiles: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Short,
    Kind,
    Size,
    Body,
    Tile(&'static str),
}

impl DecodeError {
    pub fn words(&self) -> &'static str {
        match self {
            DecodeError::Short => "a picture message was cut short",
            DecodeError::Kind => "a picture message of an unknown kind",
            DecodeError::Size => "a picture of a size this client does not accept",
            DecodeError::Body => "a picture message that does not add up",
            DecodeError::Tile(t) => t,
        }
    }
}

/// The picture, as the decoder rebuilds it (RGBA, alpha always 255 where drawn).
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

/// The reverse of the encoder's row filter; `None` when the data is the wrong length or names a filter that does not exist.
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

fn bits_for(n: usize) -> usize {
    match n {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    }
}

fn from_yc(y: f32, cg: f32, co: f32) -> [u8; 3] {
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [c(y - cg + co), c(y + cg), c(y - cg - co)]
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
    if msg[11] > 3 {
        return Err(DecodeError::Kind);
    }
    let hd = FrameHeader {
        seq: u32::from_le_bytes([msg[2], msg[3], msg[4], msg[5]]),
        screen_w: u16at(6),
        screen_h: u16at(8),
        scale: msg[10],
        level: msg[11],
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
        T_PNG => Err(DecodeError::Tile("png tiles are not decoded by this client")),
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
