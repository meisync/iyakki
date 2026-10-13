//! Pictures: what kind a file is, and decoders for the kinds this engine reads itself (JPEG, GIF, WebP). PNG is decoded by the
//! renderer, which already has a PNG decoder; SVG is drawn by [`crate::svg`].

/// Straight (not premultiplied) RGBA, row after row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
    Unknown,
}

/// What a file is, from its first bytes.
pub fn sniff(b: &[u8]) -> Kind {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Kind::Png
    } else if b.starts_with(&[0xFF, 0xD8]) {
        Kind::Jpeg
    } else if b.starts_with(b"GIF8") {
        Kind::Gif
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Kind::Webp
    } else if crate::svg::looks_like_svg(b) {
        Kind::Svg
    } else {
        Kind::Unknown
    }
}

/// Decode a JPEG, GIF (first frame), WebP or SVG. Other kinds give an error that says why (the renderer handles PNG itself).
pub fn decode(bytes: &[u8]) -> Result<Bitmap, String> {
    match sniff(bytes) {
        Kind::Jpeg => crate::jpeg::decode(bytes),
        Kind::Gif => crate::gif::decode(bytes),
        Kind::Png => Err("PNG pictures are decoded by the renderer".into()),
        Kind::Webp => crate::webp::decode(bytes),
        Kind::Svg => crate::svg::decode(bytes),
        Kind::Unknown => Err("this is not a picture the browser can read".into()),
    }
}

/// `data:image/png;base64,...` (or percent-encoded text): the bytes and the declared type.
pub fn parse_data_url(u: &str) -> Option<(String, Vec<u8>)> {
    let rest = u.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let base64 = meta.ends_with(";base64");
    let mime = meta.trim_end_matches(";base64").split(';').next().unwrap_or("").to_ascii_lowercase();
    let bytes = if base64 { base64_decode(data)? } else { percent_decode(data) };
    Some((mime, bytes))
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
        if out.len() > 32 << 20 {
            return None;
        }
    }
    Some(out)
}
