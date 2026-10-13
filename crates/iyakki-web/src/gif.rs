//! A GIF decoder: palettes, transparency, interlacing, LZW with all code sizes; the first frame on its own ([`decode`]) or
//! every frame with delays and disposal methods ([`decode_animated`]).

use crate::image::Bitmap;

const MAX_PIXELS: u64 = 16_000_000;

/// Most frames and most pixels (frames times width times height) an animation may have; later frames are dropped.
pub const MAX_FRAMES: usize = 300;
pub const MAX_ANIMATION_PIXELS: u64 = 24_000_000;

/// One step of an animation: the whole picture as it looks during this step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rgba: Vec<u8>,
    /// How long it stays, in milliseconds (never under 20; 0 and 1 hundredths count as 100 ms, as browsers do).
    pub delay_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Animation {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
}

/// The first frame of a GIF, on a transparent canvas the size of the logical screen.
pub fn decode(d: &[u8]) -> Result<Bitmap, String> {
    let mut a = decode_frames(d, 1)?;
    let f = a.frames.swap_remove(0);
    Ok(Bitmap { width: a.width, height: a.height, rgba: f.rgba })
}

/// Every frame of a GIF, composited with the disposal methods and transparency, with its delay. A GIF that breaks after
/// some frames gives the frames before the break; one that breaks before the first is an error.
pub fn decode_animated(d: &[u8]) -> Result<Animation, String> {
    decode_frames(d, MAX_FRAMES)
}

fn decode_frames(d: &[u8], max_frames: usize) -> Result<Animation, String> {
    if d.len() < 13 || &d[..3] != b"GIF" {
        return Err("not a GIF".into());
    }
    let (sw, sh) = (u16::from_le_bytes([d[6], d[7]]) as usize, u16::from_le_bytes([d[8], d[9]]) as usize);
    if sw == 0 || sh == 0 || (sw as u64) * (sh as u64) > MAX_PIXELS {
        return Err("the GIF has a bad or too large size".into());
    }
    let max_frames = max_frames.min((MAX_ANIMATION_PIXELS / (sw as u64 * sh as u64)).max(1) as usize);
    let flags = d[10];
    let mut pos = 13;
    let mut global: Vec<[u8; 3]> = Vec::new();
    if flags & 0x80 != 0 {
        let n = 2usize << (flags & 7);
        global = palette(d, pos, n)?;
        pos += 3 * n;
    }
    let mut transparent: Option<u8> = None;
    let (mut delay_ms, mut disposal) = (100u32, 0u8);
    let mut canvas = vec![0u8; sw * sh * 4];
    let mut frames: Vec<Frame> = Vec::new();
    let result: Result<(), String> = (|| loop {
        match *d.get(pos).ok_or("the GIF is cut short")? {
            0x21 => {
                let label = *d.get(pos + 1).ok_or("the GIF is cut short")?;
                pos += 2;
                if label == 0xF9 && d.get(pos) == Some(&4) && pos + 5 < d.len() {
                    disposal = (d[pos + 1] >> 2) & 7;
                    transparent = if d[pos + 1] & 1 == 1 { Some(d[pos + 4]) } else { None };
                    let hundredths = u16::from_le_bytes([d[pos + 2], d[pos + 3]]) as u32;
                    delay_ms = if hundredths < 2 { 100 } else { hundredths * 10 };
                }
                pos = skip_blocks(d, pos)?;
            }
            0x2C => {
                if pos + 10 > d.len() {
                    return Err("the GIF is cut short".into());
                }
                let (ix, iy, iw, ih) = (u16::from_le_bytes([d[pos + 1], d[pos + 2]]) as usize, u16::from_le_bytes([d[pos + 3], d[pos + 4]]) as usize, u16::from_le_bytes([d[pos + 5], d[pos + 6]]) as usize, u16::from_le_bytes([d[pos + 7], d[pos + 8]]) as usize);
                let lf = d[pos + 9];
                pos += 10;
                if iw == 0 || ih == 0 || (iw as u64) * (ih as u64) > MAX_PIXELS {
                    return Err("the GIF frame has a bad or too large size".into());
                }
                let mut table = global.clone();
                if lf & 0x80 != 0 {
                    let n = 2usize << (lf & 7);
                    table = palette(d, pos, n)?;
                    pos += 3 * n;
                }
                let interlaced = lf & 0x40 != 0;
                let min_code = *d.get(pos).ok_or("the GIF is cut short")?;
                pos += 1;
                if !(2..=11).contains(&min_code) {
                    return Err("bad LZW code size".into());
                }
                let mut data = Vec::new();
                loop {
                    let n = *d.get(pos).ok_or("the GIF is cut short")? as usize;
                    pos += 1;
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(d.get(pos..pos + n).ok_or("the GIF is cut short")?);
                    pos += n;
                }
                let indices = lzw(&data, min_code, iw * ih);
                let rows: Vec<usize> = if interlaced {
                    [(0, 8), (4, 8), (2, 4), (1, 2)].iter().flat_map(|&(start, step)| (start..ih).step_by(step)).collect()
                } else {
                    (0..ih).collect()
                };
                let before = if disposal == 3 { Some(canvas.clone()) } else { None };
                for (i, &ix_val) in indices.iter().enumerate() {
                    let (row, col) = (rows.get(i / iw.max(1)).copied().unwrap_or(usize::MAX), i % iw.max(1));
                    let (x, y) = (ix + col, iy + row);
                    if x >= sw || y >= sh {
                        continue;
                    }
                    let o = (y * sw + x) * 4;
                    if Some(ix_val) == transparent {
                        continue;
                    }
                    let c = table.get(ix_val as usize).copied().unwrap_or([0, 0, 0]);
                    canvas[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
                frames.push(Frame { rgba: canvas.clone(), delay_ms });
                if frames.len() >= max_frames {
                    return Ok(());
                }
                match disposal {
                    2 => {
                        for y in iy.min(sh)..(iy + ih).min(sh) {
                            for x in ix.min(sw)..(ix + iw).min(sw) {
                                let o = (y * sw + x) * 4;
                                canvas[o..o + 4].fill(0);
                            }
                        }
                    }
                    3 => canvas = before.unwrap_or(canvas),
                    _ => {}
                }
                transparent = None;
                disposal = 0;
                delay_ms = 100;
            }
            0x3B => return Ok(()),
            _ => return Err("the GIF is damaged".into()),
        }
    })();
    if frames.is_empty() {
        return Err(match result {
            Err(e) => e,
            Ok(()) => "the GIF has no picture".into(),
        });
    }
    Ok(Animation { width: sw as u32, height: sh as u32, frames })
}

fn palette(d: &[u8], pos: usize, n: usize) -> Result<Vec<[u8; 3]>, String> {
    let bytes = d.get(pos..pos + 3 * n).ok_or("the GIF is cut short")?;
    Ok(bytes.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

fn skip_blocks(d: &[u8], mut pos: usize) -> Result<usize, String> {
    loop {
        let n = *d.get(pos).ok_or("the GIF is cut short")? as usize;
        pos += 1;
        if n == 0 {
            return Ok(pos);
        }
        pos += n;
    }
}

/// LZW decode into at most `want` indices; short data leaves the rest zero.
fn lzw(data: &[u8], min_code: u8, want: usize) -> Vec<u8> {
    let clear = 1usize << min_code;
    let end = clear + 1;
    let mut prefix = vec![0u16; 4096];
    let mut suffix = vec![0u8; 4096];
    let mut first = vec![0u8; 4096];
    let mut next = end + 1;
    let mut size = min_code as usize + 1;
    let mut out: Vec<u8> = Vec::with_capacity(want.min(1 << 20));
    let (mut acc, mut nbits, mut pos) = (0u32, 0usize, 0usize);
    let mut prev: Option<usize> = None;
    let mut stack: Vec<u8> = Vec::new();
    while out.len() < want {
        while nbits < size {
            let Some(&b) = data.get(pos) else {
                out.resize(want, 0);
                return out;
            };
            acc |= (b as u32) << nbits;
            nbits += 8;
            pos += 1;
        }
        let code = (acc & ((1 << size) - 1)) as usize;
        acc >>= size;
        nbits -= size;
        if code == clear {
            next = end + 1;
            size = min_code as usize + 1;
            prev = None;
            continue;
        }
        if code == end {
            break;
        }
        let Some(p) = prev else {
            if code < clear {
                out.push(code as u8);
                prev = Some(code);
                first[code] = code as u8;
            }
            continue;
        };
        if code > next {
            break; // a code that cannot exist: the data is damaged, keep what was decoded
        }
        let mut cur = code;
        stack.clear();
        if code >= next {
            // the KwKwK case: previous string plus its own first byte
            stack.push(first[p]);
            cur = p;
        }
        while cur >= clear {
            // a chain longer than the table is a loop in damaged data
            if cur >= 4096 || stack.len() > 4096 {
                out.resize(want, 0);
                return out;
            }
            stack.push(suffix[cur]);
            cur = prefix[cur] as usize;
        }
        stack.push(cur as u8);
        let first_byte = *stack.last().unwrap_or(&0);
        while let Some(b) = stack.pop() {
            if out.len() < want {
                out.push(b);
            }
        }
        if next < 4096 {
            prefix[next] = p as u16;
            suffix[next] = first_byte;
            first[next] = first[p];
            next += 1;
            if next == (1 << size) && size < 12 {
                size += 1;
            }
        }
        prev = Some(code.min(4095));
        if code < clear {
            first[code] = code as u8;
        }
    }
    out.resize(want, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LZW with no compression: every pixel as a literal code, a clear code every few pixels so the code size never grows.
    fn lzw_literals(indices: &[u8], min_code: u8) -> Vec<u8> {
        let clear = 1u32 << min_code;
        let size = min_code as u32 + 1;
        let (mut out, mut acc, mut nbits) = (Vec::new(), 0u32, 0u32);
        let mut put = |code: u32, out: &mut Vec<u8>| {
            acc |= code << nbits;
            nbits += size;
            while nbits >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                nbits -= 8;
            }
        };
        put(clear, &mut out);
        for (i, &v) in indices.iter().enumerate() {
            if i > 0 && i % 2 == 0 {
                put(clear, &mut out);
            }
            put(v as u32, &mut out);
        }
        put(clear + 1, &mut out);
        if nbits > 0 {
            out.push(acc as u8);
        }
        out
    }

    /// A 4x4 GIF whose frames are given as (x, y, w, h, colour index, delay in hundredths, disposal, transparent index).
    fn build(frames: &[(u16, u16, u16, u16, u8, u16, u8, Option<u8>)]) -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[4, 0, 4, 0, 0x81, 0, 0]);
        // palette of 4: red, green, blue, white
        g.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        g.extend_from_slice(&[0x21, 0xFF, 11]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[3, 1, 0, 0, 0]);
        for &(x, y, w, h, c, delay, disp, tr) in frames {
            g.extend_from_slice(&[0x21, 0xF9, 4, (disp << 2) | tr.is_some() as u8, delay as u8, (delay >> 8) as u8, tr.unwrap_or(0), 0]);
            g.push(0x2C);
            for v in [x, y, w, h] {
                g.extend_from_slice(&v.to_le_bytes());
            }
            g.push(0);
            g.push(2);
            let data = lzw_literals(&vec![c; w as usize * h as usize], 2);
            for chunk in data.chunks(255) {
                g.push(chunk.len() as u8);
                g.extend_from_slice(chunk);
            }
            g.push(0);
        }
        g.push(0x3B);
        g
    }

    fn at(f: &Frame, x: usize, y: usize) -> [u8; 4] {
        let o = (y * 4 + x) * 4;
        [f.rgba[o], f.rgba[o + 1], f.rgba[o + 2], f.rgba[o + 3]]
    }

    #[test]
    fn frames_keep_their_delays_and_disposal_methods() {
        // 1: red everywhere, kept (disposal 1); 2: a green 2x2 block at (1,1), restored to the previous picture (disposal 3);
        // 3: a blue 2x2 block at (0,0), then cleared to transparent (disposal 2); 4: a white pixel only
        let g = build(&[(0, 0, 4, 4, 0, 10, 1, None), (1, 1, 2, 2, 1, 5, 3, None), (0, 0, 2, 2, 2, 30, 2, None), (3, 3, 1, 1, 3, 0, 0, None)]);
        let a = decode_animated(&g).unwrap();
        assert_eq!((a.width, a.height, a.frames.len()), (4, 4, 4));
        assert_eq!(a.frames.iter().map(|f| f.delay_ms).collect::<Vec<_>>(), vec![100, 50, 300, 100]);
    }

    #[test]
    fn compositing_follows_disposal() {
        let g = build(&[(0, 0, 4, 4, 0, 10, 1, None), (1, 1, 2, 2, 1, 5, 3, None), (0, 0, 2, 2, 2, 30, 2, None), (3, 3, 1, 1, 3, 0, 0, None)]);
        let a = decode_animated(&g).unwrap();
        let f = &a.frames;
        assert_eq!(at(&f[0], 3, 3), [255, 0, 0, 255]);
        assert_eq!(at(&f[1], 1, 1), [0, 255, 0, 255], "the green block");
        assert_eq!(at(&f[1], 0, 0), [255, 0, 0, 255], "red stays under it");
        assert_eq!(at(&f[2], 1, 1), [0, 0, 255, 255], "restore-to-previous removed the green block before frame 3 drew");
        assert_eq!(at(&f[2], 2, 2), [255, 0, 0, 255]);
        assert_eq!(at(&f[3], 0, 0), [0, 0, 0, 0], "frame 3's block was cleared to transparent (the red is cleared there too)");
        assert_eq!(at(&f[3], 3, 3), [255, 255, 255, 255]);
        assert_eq!(at(&f[3], 3, 0), [255, 0, 0, 255], "pixels outside the cleared block keep the red");
    }

    #[test]
    fn delays_are_in_milliseconds_with_browser_minimums() {
        let g = build(&[(0, 0, 1, 1, 0, 0, 0, None), (0, 0, 1, 1, 0, 1, 0, None), (0, 0, 1, 1, 0, 2, 0, None), (0, 0, 1, 1, 0, 25, 0, None)]);
        let a = decode_animated(&g).unwrap();
        assert_eq!(a.frames.iter().map(|f| f.delay_ms).collect::<Vec<_>>(), vec![100, 100, 20, 250]);
    }

    #[test]
    fn transparent_index_leaves_earlier_pixels_showing() {
        let g = build(&[(0, 0, 4, 4, 0, 10, 1, None), (0, 0, 2, 2, 3, 10, 1, Some(3))]);
        let a = decode_animated(&g).unwrap();
        assert_eq!(at(&a.frames[1], 0, 0), [255, 0, 0, 255], "the transparent colour index does not paint");
    }

    #[test]
    fn the_first_frame_api_is_unchanged_and_broken_files_keep_their_good_frames() {
        let g = build(&[(0, 0, 4, 4, 1, 10, 1, None), (0, 0, 4, 4, 2, 10, 1, None)]);
        let b = decode(&g).unwrap();
        assert_eq!((b.width, b.height), (4, 4));
        assert_eq!(&b.rgba[..4], &[0, 255, 0, 255]);
        // cut inside the second frame: the first survives
        let cut = &g[..g.len() - 8];
        let a = decode_animated(cut).unwrap();
        assert_eq!(a.frames.len(), 1);
        assert!(decode_animated(&g[..30]).is_err());
        for n in 0..g.len() {
            let _ = decode_animated(&g[..n]);
        }
    }

    #[test]
    fn frame_count_and_pixel_caps_hold() {
        let one = (0, 0, 4, 4, 0, 10, 0, None);
        let g = build(&vec![one; MAX_FRAMES + 50]);
        assert_eq!(decode_animated(&g).unwrap().frames.len(), MAX_FRAMES);
        // a big screen only gets as many frames as the pixel budget allows
        let mut big = build(&vec![one; 40]);
        big[6..10].copy_from_slice(&[0xE8, 0x03, 0xE8, 0x03]); // 1000 x 1000
        let a = decode_animated(&big).unwrap();
        assert!(a.frames.len() as u64 * 1_000_000 <= MAX_ANIMATION_PIXELS, "{} frames", a.frames.len());
        assert!(!a.frames.is_empty());
    }

    #[test]
    fn real_single_frame_files_are_one_frame() {
        let g = std::fs::read(format!("{}/tests/data/pal.gif", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let a = decode_animated(&g).unwrap();
        assert_eq!(a.frames.len(), 1);
        assert_eq!(a.frames[0].rgba, decode(&g).unwrap().rgba);
    }
}
