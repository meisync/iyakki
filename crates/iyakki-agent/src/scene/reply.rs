//! What a model's answer can hold as a picture, in whatever form it comes: an `<svg>` block, a `data:image/...;base64,` address,
//! a base64 blob in a code fence or on its own, a markdown image link to a file the agent wrote in its own empty working
//! folder (only that folder), or a link to somewhere on the web (never fetched here: the person must approve, and the host is
//! shown). Every candidate is judged by its first bytes, never by what the reply claims, then checked: size caps, a pixel-count
//! cap read from the header before anything is decoded (decompression bombs), a real decode with the in-house decoders,
//! metadata stripped, nothing ever executed. A short video (mp4, webm) is accepted only when the person allowed clips.

use std::path::Path;

use iyakki_web::image::{self, Kind as ImgKind};

/// The largest picture accepted from a model, in bytes.
pub const MAX_IMAGE_BYTES: usize = 8 << 20;
/// The largest video clip accepted, in bytes.
pub const MAX_VIDEO_BYTES: usize = 50 << 20;
/// The most pixels a picture may have (about 6000 by 6000).
pub const MAX_PIXELS: u64 = 36_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicKind {
    Svg,
    Raster,
    Video,
}

/// A picture accepted from a reply.
#[derive(Clone, Debug)]
pub struct Found {
    pub kind: PicKind,
    pub ext: &'static str,
    pub bytes: Vec<u8>,
    /// Where in the reply it came from (for the log).
    pub origin: &'static str,
}

/// What the first bytes say a file is.
pub fn sniff(b: &[u8]) -> Option<(PicKind, &'static str)> {
    match image::sniff(b) {
        ImgKind::Png => Some((PicKind::Raster, "png")),
        ImgKind::Jpeg => Some((PicKind::Raster, "jpg")),
        ImgKind::Gif => Some((PicKind::Raster, "gif")),
        ImgKind::Webp => Some((PicKind::Raster, "webp")),
        ImgKind::Svg => Some((PicKind::Svg, "svg")),
        ImgKind::Unknown => {
            if b.len() > 12 && &b[4..8] == b"ftyp" {
                Some((PicKind::Video, "mp4"))
            } else if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
                Some((PicKind::Video, "webm"))
            } else {
                None
            }
        }
    }
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// The width and height a raster file declares in its header, without decoding it.
pub fn dimensions(b: &[u8], ext: &str) -> Option<(u32, u32)> {
    match ext {
        "png" => {
            if b.get(12..16)? != b"IHDR" {
                return None;
            }
            Some((be32(b, 16)?, be32(b, 20)?))
        }
        "gif" => Some((u16::from_le_bytes([*b.get(6)?, *b.get(7)?]) as u32, u16::from_le_bytes([*b.get(8)?, *b.get(9)?]) as u32)),
        "jpg" => {
            let mut i = 2;
            while i + 9 < b.len() {
                if b[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let m = b[i + 1];
                if m == 0xFF {
                    i += 1;
                    continue;
                }
                if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                    return Some((u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32, u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32));
                }
                if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                    i += 2;
                    continue;
                }
                let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
                i += 2 + len.max(2);
            }
            None
        }
        "webp" => match b.get(12..16)? {
            b"VP8 " => Some(((u16::from_le_bytes([*b.get(26)?, *b.get(27)?]) & 0x3FFF) as u32, (u16::from_le_bytes([*b.get(28)?, *b.get(29)?]) & 0x3FFF) as u32)),
            b"VP8L" => {
                let v = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => {
                let w = u32::from_le_bytes([*b.get(24)?, *b.get(25)?, *b.get(26)?, 0]) + 1;
                let h = u32::from_le_bytes([*b.get(27)?, *b.get(28)?, *b.get(29)?, 0]) + 1;
                Some((w, h))
            }
            _ => None,
        },
        _ => None,
    }
}

/// PNG without its text and metadata chunks, JPEG without comments and APPn segments (colour profile kept in APP2 is dropped
/// too), WebP without EXIF and XMP: what a picture says about where and how it was made does not travel with it.
pub fn strip_metadata(b: &[u8], ext: &str) -> Vec<u8> {
    match ext {
        "png" => {
            let mut out = b[..8].to_vec();
            let mut i = 8;
            while i + 12 <= b.len() {
                let len = be32(b, i).unwrap_or(0) as usize;
                let end = i + 12 + len;
                if end > b.len() {
                    return b.to_vec();
                }
                let ty = &b[i + 4..i + 8];
                if !matches!(ty, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME" | b"pHYs") {
                    out.extend_from_slice(&b[i..end]);
                }
                i = end;
            }
            out
        }
        "jpg" => {
            let mut out = b[..2].to_vec();
            let mut i = 2;
            while i + 4 <= b.len() {
                if b[i] != 0xFF {
                    out.extend_from_slice(&b[i..]);
                    return out;
                }
                let m = b[i + 1];
                if m == 0xDA {
                    out.extend_from_slice(&b[i..]);
                    return out;
                }
                if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
                    out.extend_from_slice(&b[i..i + 2]);
                    i += 2;
                    continue;
                }
                let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
                let end = (i + 2 + len).min(b.len());
                if !((0xE1..=0xEF).contains(&m) || m == 0xFE) {
                    out.extend_from_slice(&b[i..end]);
                }
                i = end;
            }
            out.extend_from_slice(&b[i.min(b.len())..]);
            out
        }
        "webp" => {
            if b.len() < 20 || &b[12..16] != b"VP8X" {
                return b.to_vec();
            }
            let mut out = b[..12].to_vec();
            let mut i = 12;
            while i + 8 <= b.len() {
                let len = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
                let end = (i + 8 + len + (len & 1)).min(b.len());
                let ty = &b[i..i + 4];
                if !matches!(ty, b"EXIF" | b"XMP ") {
                    let mut chunk = b[i..end].to_vec();
                    if ty == b"VP8X" && chunk.len() > 8 {
                        chunk[8] &= !0x0C;
                    }
                    out.extend_from_slice(&chunk);
                }
                i = end;
            }
            let size = (out.len() - 8) as u32;
            out[4..8].copy_from_slice(&size.to_le_bytes());
            out
        }
        _ => b.to_vec(),
    }
}

/// Judge bytes: what they are by their first bytes, then the caps and a real decode. SVG goes through the sanitiser. A picture
/// that passes comes back with its metadata stripped.
pub fn validate(bytes: Vec<u8>, allow_video: bool, origin: &'static str) -> Result<Found, String> {
    let (kind, ext) = sniff(&bytes).ok_or("it is not a picture or video format this program reads (png, jpeg, webp, gif, svg, mp4, webm)")?;
    match kind {
        PicKind::Svg => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let svg = crate::story::sanitize_svg(&text)?;
            Ok(Found { kind, ext, bytes: svg.into_bytes(), origin })
        }
        PicKind::Raster => {
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err(format!("the picture is too large ({} bytes, at most {MAX_IMAGE_BYTES})", bytes.len()));
            }
            let (w, h) = dimensions(&bytes, ext).ok_or("the picture's header cannot be read")?;
            if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
                return Err(format!("the picture claims {w}x{h} pixels, more than the {MAX_PIXELS} allowed"));
            }
            if ext == "png" {
                // the shell decodes PNG when it draws; here the chunk structure must be whole, ending with IEND
                let mut i = 8;
                let mut end = false;
                while i + 12 <= bytes.len() {
                    let len = be32(&bytes, i).unwrap_or(u32::MAX) as usize;
                    if len > bytes.len() || i + 12 + len > bytes.len() {
                        break;
                    }
                    if &bytes[i + 4..i + 8] == b"IEND" {
                        end = true;
                        break;
                    }
                    i += 12 + len;
                }
                if !end {
                    return Err("the PNG is cut off or damaged".into());
                }
            } else {
                image::decode(&bytes).map_err(|e| format!("the {ext} picture cannot be decoded ({e})"))?;
            }
            Ok(Found { kind, ext, bytes: strip_metadata(&bytes, ext), origin })
        }
        PicKind::Video => {
            if !allow_video {
                return Err("it is a video clip, and short video clips are switched off (Options)".into());
            }
            if bytes.len() > MAX_VIDEO_BYTES {
                return Err(format!("the video is too large ({} bytes, at most {MAX_VIDEO_BYTES})", bytes.len()));
            }
            Ok(Found { kind, ext, bytes, origin })
        }
    }
}

/// One thing in a reply that might be a picture.
#[derive(Clone, Debug)]
enum Cand {
    Bytes(Vec<u8>, &'static str),
    Link(String),
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("").to_string()
}

fn is_b64_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

fn candidates(reply: &str, workdir: Option<&Path>) -> Vec<Cand> {
    let l = reply.to_ascii_lowercase();
    let reply = l.rfind("</think>").map_or(reply, |i| &reply[i + 8..]);
    let mut out: Vec<(usize, Cand)> = Vec::new();
    // data: addresses
    for key in ["data:image/", "data:video/"] {
        let mut from = 0;
        while let Some(i) = reply[from..].find(key) {
            let start = from + i;
            let rest = &reply[start..];
            let end = rest.find(|c: char| matches!(c, '"' | '\'' | ')' | '>' | '<' | ' ' | '\n' | '`')).unwrap_or(rest.len());
            if let Some((_, bytes)) = image::parse_data_url(&rest[..end]) {
                out.push((start, Cand::Bytes(bytes, "a data address")));
            }
            from = start + end.max(1);
        }
    }
    // code fences and bare long lines of base64
    let mut pos = 0;
    let mut fence: Option<(usize, String)> = None;
    for line in reply.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("```") {
            match fence.take() {
                Some((at, body)) => {
                    let blob: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                    if blob.len() >= 40 && blob.chars().all(is_b64_char) {
                        if let Some(b) = image::base64_decode(&blob) {
                            out.push((at, Cand::Bytes(b, "a base64 block")));
                        }
                    }
                }
                None => fence = Some((pos, String::new())),
            }
        } else if let Some((_, body)) = fence.as_mut() {
            body.push_str(t);
        } else if t.len() >= 400 && t.chars().all(is_b64_char) {
            if let Some(b) = image::base64_decode(t) {
                out.push((pos, Cand::Bytes(b, "a base64 line")));
            }
        }
        pos += line.len();
    }
    // markdown links: files in the working folder, or addresses
    let mut from = 0;
    while let Some(i) = reply[from..].find("](") {
        let start = from + i + 2;
        let Some(j) = reply[start..].find(')') else { break };
        let target = reply[start..start + j].trim().trim_matches(['<', '>']).split_whitespace().next().unwrap_or("").to_string();
        from = start + j;
        if target.starts_with("http://") || target.starts_with("https://") {
            out.push((start, Cand::Link(target)));
        } else if !target.is_empty() && !target.contains("://") && !target.starts_with("data:") {
            if let Some(wd) = workdir.and_then(|w| std::fs::canonicalize(w).ok()) {
                let p = if Path::new(&target).is_absolute() { Path::new(&target).to_path_buf() } else { wd.join(&target) };
                if let Ok(real) = std::fs::canonicalize(&p) {
                    if real.starts_with(&wd) && real.is_file() && std::fs::metadata(&real).map_or(false, |m| m.len() as usize <= MAX_VIDEO_BYTES) {
                        if let Ok(b) = std::fs::read(&real) {
                            out.push((start, Cand::Bytes(b, "a file in the agent's folder")));
                        }
                    }
                }
            }
        }
    }
    // an <svg> block
    if let (Some(a), Some(b)) = (reply.to_ascii_lowercase().find("<svg"), reply.to_ascii_lowercase().rfind("</svg>")) {
        if b > a {
            out.push((a, Cand::Bytes(reply[a..b + 6].as_bytes().to_vec(), "an svg block")));
        }
    }
    out.sort_by_key(|c| c.0);
    out.into_iter().map(|c| c.1).collect()
}

/// The first usable picture in a reply, or why none was: each candidate that failed is named. Addresses on the web are not
/// fetched; the error names the host so the person can decide.
pub fn accept(reply: &str, workdir: Option<&Path>, allow_video: bool) -> Result<Found, String> {
    let mut why: Vec<String> = Vec::new();
    for c in candidates(reply, workdir) {
        match c {
            Cand::Bytes(b, origin) => match validate(b, allow_video, origin) {
                Ok(f) => return Ok(f),
                Err(e) => why.push(format!("{origin}: {e}")),
            },
            Cand::Link(u) => why.push(format!("the reply links to {} ({u}), which is not fetched without your approval", host_of(&u))),
        }
    }
    if why.is_empty() {
        Err("the reply holds no picture (no image, no data address, no file in the working folder, no svg)".into())
    } else {
        Err(why.join("; "))
    }
}
