//! A small PNG reader (non-interlaced, 8 bits a sample: grey, grey with alpha, RGB, RGBA and palette), in-house; the only library is
//! `miniz_oxide` (zlib). It is the one PNG reader of the program: the browser QA pictures (`iyakki-agent`'s `qaimage`) and the wallpaper
//! picture of the shell both call it.

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Reads a non-interlaced 8-bit PNG (grey, grey+alpha, RGB, RGBA, palette): what this program and common tools write.
pub fn decode(bytes: &[u8]) -> Result<crate::image::Bitmap, String> {
    if bytes.len() < 33 || bytes[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Err("this is not a PNG file".into());
    }
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut idat: Vec<u8> = Vec::new();
    let mut plte: Vec<u8> = Vec::new();
    let mut pos = 8;
    while pos + 8 <= bytes.len() {
        let len = be32(&bytes[pos..]) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let data = bytes.get(pos + 8..pos + 8 + len).ok_or("the PNG file is cut short")?;
        match kind {
            b"IHDR" if len >= 13 => {
                w = be32(data);
                h = be32(&data[4..]);
                depth = data[8];
                ctype = data[9];
                interlace = data[12];
            }
            b"PLTE" => plte = data.to_vec(),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if w == 0 || h == 0 || w > 16384 || h > 32768 {
        return Err("the PNG file has a size this program will not read".into());
    }
    if depth != 8 || interlace != 0 {
        return Err("only 8-bit PNG files without interlacing can be compared".into());
    }
    let channels = match ctype {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("this PNG colour type is not supported".into()),
    };
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, 1 << 30).map_err(|_| "the PNG data is damaged".to_string())?;
    let stride = w as usize * channels;
    if raw.len() < (stride + 1) * h as usize {
        return Err("the PNG data is too short".into());
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let f = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for i in 0..stride {
            let a = if i >= channels { cur[i - channels] } else { 0 } as i32;
            let b = prev[i] as i32;
            let c = if i >= channels { prev[i - channels] } else { 0 } as i32;
            let x = line[i];
            cur[i] = match f {
                0 => x,
                1 => x.wrapping_add(a as u8),
                2 => x.wrapping_add(b as u8),
                3 => x.wrapping_add(((a + b) / 2) as u8),
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    let pr = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                    x.wrapping_add(pr as u8)
                }
                _ => return Err("the PNG data is damaged (unknown filter)".into()),
            };
        }
        for px in cur.chunks_exact(channels) {
            match ctype {
                0 => rgba.extend_from_slice(&[px[0], px[0], px[0], 255]),
                2 => rgba.extend_from_slice(&[px[0], px[1], px[2], 255]),
                3 => {
                    let i = px[0] as usize * 3;
                    let p = plte.get(i..i + 3).unwrap_or(&[0, 0, 0]);
                    rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
                4 => rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]),
                _ => rgba.extend_from_slice(&[px[0], px[1], px[2], px[3]]),
            }
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Ok(crate::image::Bitmap { width: w, height: h, rgba })
}
