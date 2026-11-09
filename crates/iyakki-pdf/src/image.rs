//! Image XObjects and inline images: raw samples at 1, 2, 4, 8 or 16 bits in the device, indexed, ICC and separation spaces, Flate/LZW
//! (through the stream filters) and DCT (the browser's JPEG decoder). Image masks give a stencil. JPX, CCITT and JBIG2 are not
//! decoded: the viewer draws a grey box and says so.

use crate::color::Cs;
use crate::file::File;
use crate::obj::*;

pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;

pub struct Image {
    pub w: u32,
    pub h: u32,
    /// RGBA, or for a stencil only the alpha channel is used (colour comes from the fill colour).
    pub rgba: Vec<u8>,
    pub stencil: bool,
}

pub enum Failure {
    Unsupported(String),
    Bad(String),
}

fn sample(data: &[u8], bit_pos: usize, bpc: usize) -> u32 {
    match bpc {
        8 => data.get(bit_pos / 8).copied().unwrap_or(0) as u32,
        16 => ((data.get(bit_pos / 8).copied().unwrap_or(0) as u32) << 8 | data.get(bit_pos / 8 + 1).copied().unwrap_or(0) as u32) >> 8,
        b => {
            let byte = data.get(bit_pos / 8).copied().unwrap_or(0) as u32;
            let shift = 8 - b - (bit_pos % 8);
            (byte >> shift) & ((1 << b) - 1)
        }
    }
}

/// `d`: the image dictionary; `raw` the already-decoded sample bytes (or a DCT stream via `image_filter`).
pub fn decode(f: &File, d: &Dict, src: &Obj, inline: Option<&[u8]>, res: &Dict) -> Result<Image, Failure> {
    let w = f.get(d, "Width").int().unwrap_or(0);
    let h = f.get(d, "Height").int().unwrap_or(0);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_IMAGE_PIXELS {
        return Err(Failure::Bad("image size".into()));
    }
    let (w, h) = (w as usize, h as usize);
    let is_mask = matches!(f.get(d, "ImageMask"), Obj::Bool(true));
    let decoded = match inline {
        Some(raw) => {
            // inline images carry their filters in the dictionary: run them through a temporary stream view
            inline_decode(f, d, raw)
        }
        None => f.stream_data(src).map_err(|e| Failure::Bad(e.to_string())),
    }?;
    let decode_arr: Vec<f64> = f.get(d, "Decode").arr().map(|a| a.iter().filter_map(|x| f.resolve(x).num()).collect()).unwrap_or_default();
    if let Some((name, _)) = &decoded.image_filter {
        if name == "DCTDecode" || name == "DCT" {
            let bm = iyakki_web::jpeg::decode(&decoded.data).map_err(Failure::Bad)?;
            let mut rgba = bm.rgba;
            if decode_arr.len() >= 2 && decode_arr[0] > decode_arr[1] {
                for px in rgba.chunks_mut(4) {
                    for c in px.iter_mut().take(3) {
                        *c = 255 - *c;
                    }
                }
            }
            return Ok(Image { w: bm.width, h: bm.height, rgba, stencil: false });
        }
        return Err(Failure::Unsupported(match name.as_str() {
            "JPXDecode" => "JPEG 2000 pictures".into(),
            "CCITTFaxDecode" | "CCF" => "fax (CCITT) pictures".into(),
            "JBIG2Decode" => "JBIG2 pictures".into(),
            other => format!("{other} pictures"),
        }));
    }
    let data = decoded.data;
    if is_mask {
        let inv = decode_arr.first().copied().unwrap_or(0.0) >= 1.0;
        let mut rgba = vec![0u8; w * h * 4];
        let row_bytes = w.div_ceil(8);
        for y in 0..h {
            for x in 0..w {
                let bit = sample(&data, y * row_bytes * 8 + x, 1);
                // sample 0 paints (unless Decode is [1 0])
                let paint = (bit == 0) != inv;
                rgba[(y * w + x) * 4 + 3] = if paint { 255 } else { 0 };
            }
        }
        return Ok(Image { w: w as u32, h: h as u32, rgba, stencil: true });
    }
    let bpc = f.get(d, "BitsPerComponent").int().unwrap_or(8) as usize;
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return Err(Failure::Bad("bits per component".into()));
    }
    let cs = match d.get("ColorSpace") {
        Some(c) => Cs::parse(f, c, f.get(res, "ColorSpace").dict().map(|x| &**x), 0),
        None => Cs::Gray,
    };
    let n = cs.n().max(1);
    let row_bits = w * n * bpc;
    let row_bytes = row_bits.div_ceil(8);
    let maxv = ((1u32 << bpc.min(8)) - 1) as f64;
    let mut rgba = vec![255u8; w * h * 4];
    // a small colour cache makes indexed and separation images fast
    let indexed = matches!(cs, Cs::Indexed { .. });
    let mut cache: std::collections::HashMap<u32, [f32; 3]> = std::collections::HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let base = y * row_bytes * 8 + x * n * bpc;
            let mut comps = [0f64; 8];
            let mut key = 0u32;
            for (c, slot) in comps.iter_mut().enumerate().take(n.min(8)) {
                let s = sample(&data, base + c * bpc, bpc);
                key = key.wrapping_mul(257).wrapping_add(s);
                let lo = decode_arr.get(c * 2).copied();
                let hi = decode_arr.get(c * 2 + 1).copied();
                *slot = if indexed {
                    match (lo, hi) {
                        (Some(l), Some(hh)) => l + s as f64 * (hh - l) / maxv,
                        _ => s as f64,
                    }
                } else {
                    let v = s as f64 / maxv;
                    match (lo, hi) {
                        (Some(l), Some(hh)) => l + v * (hh - l),
                        _ => v,
                    }
                };
            }
            let rgb = match cache.get(&key) {
                Some(c) if n <= 4 => *c,
                _ => {
                    let c = if matches!(cs, Cs::Lab) {
                        cs.rgb(&[comps[0] * 100.0, comps[1] * 255.0 - 128.0, comps[2] * 255.0 - 128.0])
                    } else {
                        cs.rgb(&comps[..n.min(8)])
                    };
                    if cache.len() < 100_000 && n <= 4 {
                        cache.insert(key, c);
                    }
                    c
                }
            };
            let i = (y * w + x) * 4;
            rgba[i] = (rgb[0] * 255.0 + 0.5) as u8;
            rgba[i + 1] = (rgb[1] * 255.0 + 0.5) as u8;
            rgba[i + 2] = (rgb[2] * 255.0 + 0.5) as u8;
        }
    }
    // a soft mask or a colour-key mask
    if let Obj::Stream(md, _, _) = f.get(d, "SMask") {
        let mo = f.get(d, "SMask");
        if let Ok(Image { w: mw, h: mh, rgba: mrgba, .. }) = decode(f, &md, &mo, None, res) {
            for y in 0..h {
                for x in 0..w {
                    let (mx, my) = (x * mw as usize / w, y * mh as usize / h);
                    rgba[(y * w + x) * 4 + 3] = mrgba[(my * mw as usize + mx) * 4];
                }
            }
        }
    }
    Ok(Image { w: w as u32, h: h as u32, rgba, stencil: false })
}

fn inline_decode(f: &File, d: &Dict, raw: &[u8]) -> Result<crate::file::Decoded, Failure> {
    // Build a throw-away stream object over `raw` by decoding with the same filter code
    let names: Vec<String> = match d.get("Filter") {
        Some(Obj::Name(n)) => vec![n.to_string()],
        Some(Obj::Arr(a)) => a.iter().filter_map(|x| x.name().map(|s| s.to_string())).collect(),
        _ => vec![],
    };
    let parms = match d.get("DecodeParms") {
        Some(Obj::Dict(p)) => Some(p.clone()),
        Some(Obj::Arr(a)) => a.first().and_then(|x| x.dict().cloned()),
        _ => None,
    };
    let geti = |k: &str, dflt: i64| parms.as_ref().and_then(|p| f.get(p, k).int()).unwrap_or(dflt);
    let mut data = raw.to_vec();
    for name in names.iter().take(6) {
        data = match name.as_str() {
            "FlateDecode" | "Fl" | "LZWDecode" | "LZW" => {
                let out = if name.starts_with('F') { crate::filter::inflate(&data, crate::filter::MAX_STREAM) } else { crate::filter::lzw(&data, geti("EarlyChange", 1) != 0, crate::filter::MAX_STREAM) };
                let pr = geti("Predictor", 1);
                if pr > 1 { crate::filter::predictor(out, pr, geti("Colors", 1) as usize, geti("BitsPerComponent", 8) as usize, geti("Columns", 1) as usize) } else { out }
            }
            "ASCIIHexDecode" | "AHx" => crate::filter::ascii_hex(&data),
            "ASCII85Decode" | "A85" => crate::filter::ascii85(&data),
            "RunLengthDecode" | "RL" => crate::filter::run_length(&data, crate::filter::MAX_STREAM),
            "DCTDecode" | "DCT" => return Ok(crate::file::Decoded { data, image_filter: Some(("DCTDecode".into(), Obj::Null)) }),
            other => return Err(Failure::Unsupported(format!("{other} pictures"))),
        };
    }
    Ok(crate::file::Decoded { data, image_filter: None })
}
