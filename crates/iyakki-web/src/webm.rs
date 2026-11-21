//! The poster frame of a WebM clip (F-TALES-18): the first key frame of a VP8 video track, decoded with the lossy WebP decoder (a VP8 key
//! frame and the picture of a lossy WebP file are the same bytes, RFC 6386). Written from the public Matroska/EBML description: element
//! ids and sizes are variable-length numbers, a Segment holds Tracks and Clusters, a Cluster holds SimpleBlocks (and BlockGroups with a
//! Block). Only that frame is read: no inter frame, no sound, no other codec (VP9, AV1 and H.264 clips give a plain sentence). Hostile
//! input is bounded: depth, element count and frame size are capped and every read is checked.

use crate::image::Bitmap;

const MAX_ELEMENTS: usize = 200_000;
const MAX_DEPTH: usize = 6;
/// The largest frame handed to the decoder (a poster of a short clip is far smaller).
const MAX_FRAME: usize = 8 << 20;

const ID_SEGMENT: u32 = 0x1853_8067;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;

/// A variable-length number at `at`: (value without the length marker, the id with it, bytes used). `all_ones` is true for an unknown size.
fn vint(b: &[u8], at: usize, keep_marker: bool) -> Option<(u64, usize, bool)> {
    let first = *b.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let bytes = b.get(at..at + len)?;
    let mut v = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
    let mut ones = (first as u64) & ((1u64 << (8 - len)) - 1) == (1u64 << (8 - len)) - 1;
    for &x in &bytes[1..] {
        v = (v << 8) | x as u64;
        ones &= x == 0xFF;
    }
    Some((v, len, ones))
}

struct Elem<'a> {
    id: u32,
    body: &'a [u8],
}

/// The elements one level down in `b` (a child of unknown size runs to the end of its parent).
fn children<'a>(b: &'a [u8], budget: &mut usize) -> Vec<Elem<'a>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() && *budget > 0 {
        let Some((id, il, _)) = vint(b, at, true) else { break };
        let Some((size, sl, unknown)) = vint(b, at + il, false) else { break };
        *budget -= 1;
        let start = at + il + sl;
        let end = if unknown { b.len() } else { (start as u64).saturating_add(size).min(b.len() as u64) as usize };
        if start > end || id > u32::MAX as u64 {
            break;
        }
        out.push(Elem { id: id as u32, body: &b[start..end] });
        at = end;
    }
    out
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |v, &x| (v << 8) | x as u64)
}

/// The track number of the first video track whose codec is VP8; an error that names the codec when there is a video track of another one.
fn vp8_track(tracks: &[u8], budget: &mut usize) -> Result<u64, String> {
    let mut other: Option<String> = None;
    for te in children(tracks, budget).into_iter().filter(|e| e.id == ID_TRACK_ENTRY) {
        let (mut num, mut codec, mut kind) = (None, String::new(), 0u64);
        for f in children(te.body, budget) {
            match f.id {
                ID_TRACK_NUMBER => num = Some(uint(f.body)),
                ID_TRACK_TYPE => kind = uint(f.body),
                ID_CODEC_ID => codec = String::from_utf8_lossy(f.body).trim_end_matches('\0').to_string(),
                _ => {}
            }
        }
        if kind == 1 || codec.starts_with("V_") {
            if codec == "V_VP8" {
                if let Some(n) = num {
                    return Ok(n);
                }
            } else if other.is_none() {
                other = Some(codec);
            }
        }
    }
    Err(match other {
        Some(c) if !c.is_empty() => format!("this clip's video is {} and only VP8 clips get a poster frame", c.trim_start_matches("V_")),
        _ => "this WebM clip has no VP8 video track".into(),
    })
}

/// The frame inside a (Simple)Block of track `track`, when it is a whole frame (no lacing).
fn block_frame(body: &[u8], track: u64) -> Option<&[u8]> {
    let (n, l, _) = vint(body, 0, false)?;
    if n != track {
        return None;
    }
    let flags = *body.get(l + 2)?;
    if flags & 0x06 != 0 {
        return None; // laced: several frames in one block
    }
    body.get(l + 3..)
}

/// The first VP8 key frame of the clip, as the bytes of a VP8 frame.
fn first_key_frame(data: &[u8]) -> Result<&[u8], String> {
    if data.len() < 4 || data[..4] != [0x1A, 0x45, 0xDF, 0xA3] {
        return Err("this is not a WebM clip".into());
    }
    let mut budget = MAX_ELEMENTS;
    let top = children(data, &mut budget);
    let seg = top.iter().find(|e| e.id == ID_SEGMENT).ok_or("this WebM clip has no segment")?;
    let parts = children(seg.body, &mut budget);
    let track = vp8_track(parts.iter().find(|e| e.id == ID_TRACKS).ok_or("this WebM clip lists no tracks")?.body, &mut budget)?;
    let mut depth = 0;
    for cl in parts.iter().filter(|e| e.id == ID_CLUSTER) {
        depth += 1;
        if depth > MAX_DEPTH * 1000 {
            break;
        }
        for blk in children(cl.body, &mut budget) {
            let frame = match blk.id {
                ID_SIMPLE_BLOCK => block_frame(blk.body, track),
                ID_BLOCK_GROUP => children(blk.body, &mut budget).into_iter().find(|e| e.id == ID_BLOCK).and_then(|e| block_frame(e.body, track)),
                _ => None,
            };
            // bit 0 of the first byte of a VP8 frame is 0 for a key frame
            if let Some(f) = frame.filter(|f| f.len() > 10 && f[0] & 1 == 0) {
                if f.len() > MAX_FRAME {
                    return Err("the first picture of this clip is too large".into());
                }
                return Ok(f);
            }
        }
    }
    Err("this WebM clip holds no key frame to show".into())
}

/// The poster frame of a WebM clip with a VP8 video track.
pub fn poster(data: &[u8]) -> Result<Bitmap, String> {
    let frame = first_key_frame(data)?;
    let pad = frame.len() & 1;
    let mut webp = Vec::with_capacity(frame.len() + 32);
    webp.extend_from_slice(b"RIFF");
    webp.extend_from_slice(&((4 + 8 + frame.len() + pad) as u32).to_le_bytes());
    webp.extend_from_slice(b"WEBPVP8 ");
    webp.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    webp.extend_from_slice(frame);
    if pad == 1 {
        webp.push(0);
    }
    crate::webp::decode(&webp).map_err(|e| format!("the first picture of this clip cannot be shown: {e}"))
}

/// A one-frame WebM clip around a VP8 key frame: what a test of another crate needs to give [`poster`] something real.
#[doc(hidden)]
pub fn one_frame_clip_for_test(vp8_key_frame: &[u8]) -> Vec<u8> {
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x01);
        v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        v.extend_from_slice(body);
        v
    }
    let mut entry = el(&[0xD7], &[1]);
    entry.extend(el(&[0x83], &[1]));
    entry.extend(el(&[0x86], b"V_VP8"));
    let mut seg = el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &entry));
    let mut blk = vec![0x81, 0, 0, 0x80];
    blk.extend_from_slice(vp8_key_frame);
    seg.extend(el(&[0x1F, 0x43, 0xB6, 0x75], &el(&[0xA3], &blk)));
    let mut file = el(&[0x1A, 0x45, 0xDF, 0xA3], &el(&[0x42, 0x82], b"webm"));
    file.extend(el(&[0x18, 0x53, 0x80, 0x67], &seg));
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(n: usize) -> Vec<u8> {
        // always the eight-byte form: simple and valid
        let mut v = vec![0x01];
        v.extend_from_slice(&(n as u64).to_be_bytes()[1..]);
        v
    }
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend(size(body.len()));
        v.extend_from_slice(body);
        v
    }
    fn vp8_of(webp: &[u8]) -> Vec<u8> {
        let at = webp.windows(4).position(|w| w == b"VP8 ").unwrap();
        let n = u32::from_le_bytes([webp[at + 4], webp[at + 5], webp[at + 6], webp[at + 7]]) as usize;
        webp[at + 8..at + 8 + n].to_vec()
    }
    fn clip(codec: &str, frames: &[(Vec<u8>, bool)], simple: bool) -> Vec<u8> {
        let mut entry = el(&[0xD7], &[1]);
        entry.extend(el(&[0x83], &[1]));
        entry.extend(el(&[0x86], codec.as_bytes()));
        let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &entry));
        let mut cluster = el(&[0xE7], &[0]);
        for (f, _) in frames {
            let mut blk = vec![0x81, 0, 0, 0x80];
            blk.extend_from_slice(f);
            cluster.extend(if simple { el(&[0xA3], &blk) } else { el(&[0xA0], &el(&[0xA1], &blk)) });
        }
        let mut seg = tracks;
        seg.extend(el(&[0x1F, 0x43, 0xB6, 0x75], &cluster));
        let mut file = el(&[0x1A, 0x45, 0xDF, 0xA3], &el(&[0x42, 0x82], b"webm"));
        file.extend(el(&[0x18, 0x53, 0x80, 0x67], &seg));
        file
    }

    #[test]
    fn the_first_key_frame_of_a_vp8_track_is_decoded_as_the_poster_and_matches_the_same_frame_as_a_webp_file() {
        let webp = include_bytes!("../tests/data/webp_lossy.webp");
        let key = vp8_of(webp);
        let inter = vec![0x01u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for simple in [true, false] {
            // an inter frame comes first and is skipped
            let file = clip("V_VP8", &[(inter.clone(), false), (key.clone(), true)], simple);
            let p = poster(&file).unwrap();
            let want = crate::webp::decode(webp).unwrap();
            assert_eq!((p.width, p.height), (want.width, want.height));
            assert_eq!(p.rgba, want.rgba, "the same picture, bit for bit");
        }
    }

    #[test]
    fn other_codecs_missing_tracks_damaged_and_hostile_files_give_a_sentence_and_never_a_panic() {
        let key = vp8_of(include_bytes!("../tests/data/webp_lossy.webp"));
        let vp9 = clip("V_VP9", &[(key.clone(), true)], true);
        assert!(poster(&vp9).unwrap_err().contains("VP9"), "{:?}", poster(&vp9));
        assert!(poster(b"RIFF....WEBP").unwrap_err().contains("not a WebM"));
        assert!(poster(&clip("V_VP8", &[], true)).unwrap_err().contains("no key frame"));
        assert!(poster(&[]).is_err());
        // every cut of a good file and every flipped byte: an error or a picture, no panic
        let good = clip("V_VP8", &[(key, true)], true);
        for n in 0..good.len() {
            let _ = poster(&good[..n]);
        }
        for i in (0..good.len()).step_by(3) {
            let mut bad = good.clone();
            bad[i] ^= 0xA5;
            let _ = poster(&bad);
        }
        // a size that runs past the end and an unknown-size segment are bounded
        let mut unknown = good.clone();
        let at = unknown.windows(4).position(|w| w == [0x18, 0x53, 0x80, 0x67]).unwrap() + 4;
        unknown[at..at + 8].copy_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(poster(&unknown).is_ok());
    }

    #[test]
    fn variable_length_numbers_follow_the_matroska_rules() {
        assert_eq!(vint(&[0x81], 0, false), Some((1, 1, false)));
        assert_eq!(vint(&[0x40, 0x02], 0, false), Some((2, 2, false)));
        assert_eq!(vint(&[0xFF], 0, false), Some((0x7F, 1, true)), "all ones is an unknown size");
        assert_eq!(vint(&[0x1A, 0x45, 0xDF, 0xA3], 0, true), Some((0x1A45DFA3, 4, false)));
        assert_eq!(vint(&[0x00], 0, false), None);
        assert_eq!(vint(&[0x40], 0, false), None, "cut short");
    }
}
