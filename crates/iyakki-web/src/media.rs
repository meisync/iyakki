//! What a media file is, from its first bytes: container, codecs, picture size and length. Only enough to describe it
//! on a page; nothing here decodes audio or video.

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|x| u64::from_be_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]]))
}

/// The boxes of an MP4/QuickTime region: `(type, content)`. A box that runs past the data we have is cut short.
fn boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= b.len() && out.len() < 400 {
        let (mut size, ty) = (be32(b, at).unwrap_or(0) as u64, [b[at + 4], b[at + 5], b[at + 6], b[at + 7]]);
        let mut head = 8;
        if size == 1 {
            size = be64(b, at + 8).unwrap_or(0);
            head = 16;
        } else if size == 0 {
            size = (b.len() - at) as u64;
        }
        if size < head as u64 {
            break;
        }
        let end = (at as u64 + size).min(b.len() as u64) as usize;
        if at + head <= end {
            out.push((ty, &b[at + head..end]));
        }
        at = end;
    }
    out
}

fn find<'a>(list: &[([u8; 4], &'a [u8])], ty: &[u8; 4]) -> Option<&'a [u8]> {
    list.iter().find(|(t, _)| t == ty).map(|(_, c)| *c)
}

fn codec_name(f: &[u8]) -> String {
    match f {
        b"avc1" | b"avc3" => "H.264".into(),
        b"hvc1" | b"hev1" => "H.265".into(),
        b"vp09" => "VP9".into(),
        b"av01" => "AV1".into(),
        b"mp4a" => "AAC".into(),
        b"Opus" => "Opus".into(),
        b"ac-3" => "AC-3".into(),
        b"mp4v" => "MPEG-4 video".into(),
        other => String::from_utf8_lossy(other).trim().to_string(),
    }
}

#[derive(Default)]
struct Track {
    kind: Option<[u8; 4]>,
    codec: String,
    width: u32,
    height: u32,
}

fn duration_text(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

fn mp4(b: &[u8]) -> Option<String> {
    let top = boxes(b);
    let ftyp = find(&top, b"ftyp")?;
    let brand = String::from_utf8_lossy(ftyp.get(..4)?).trim().to_string();
    let mut tracks: Vec<Track> = Vec::new();
    let mut duration = None;
    if let Some(moov) = find(&top, b"moov") {
        let m = boxes(moov);
        if let Some(mvhd) = find(&m, b"mvhd") {
            let v = *mvhd.first()?;
            let (scale, dur) = if v == 1 { (be32(mvhd, 20)?, be64(mvhd, 24)?) } else { (be32(mvhd, 12)?, be32(mvhd, 16)? as u64) };
            if scale > 0 {
                duration = Some(dur as f64 / scale as f64);
            }
        }
        for (ty, trak) in &m {
            if ty != b"trak" {
                continue;
            }
            let t = boxes(trak);
            let mut tr = Track::default();
            if let Some(tkhd) = find(&t, b"tkhd") {
                let off = if tkhd.first() == Some(&1) { 88 } else { 76 };
                tr.width = be32(tkhd, off).unwrap_or(0) >> 16;
                tr.height = be32(tkhd, off + 4).unwrap_or(0) >> 16;
            }
            if let Some(mdia) = find(&t, b"mdia") {
                let md = boxes(mdia);
                if let Some(hdlr) = find(&md, b"hdlr") {
                    tr.kind = hdlr.get(8..12).map(|k| [k[0], k[1], k[2], k[3]]);
                }
                if let Some(stsd) = find(&boxes(find(&boxes(find(&md, b"minf").unwrap_or(&[])), b"stbl").unwrap_or(&[])), b"stsd") {
                    if let Some(f) = stsd.get(12..16) {
                        tr.codec = codec_name(f);
                    }
                }
            }
            tracks.push(tr);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = tracks.iter().find(|t| t.kind == Some(*b"vide")) {
        parts.push(format!("{} video, {}×{}", if v.codec.is_empty() { "?".to_string() } else { v.codec.clone() }, v.width, v.height));
    }
    if let Some(a) = tracks.iter().find(|t| t.kind == Some(*b"soun")) {
        parts.push(format!("{} audio", if a.codec.is_empty() { "?".to_string() } else { a.codec.clone() }));
    }
    if let Some(d) = duration {
        parts.push(duration_text(d));
    }
    if parts.is_empty() {
        parts.push(format!("MP4 ({brand}); the details are at the end of the file"));
    }
    Some(parts.join(" · "))
}

fn webm(b: &[u8]) -> Option<String> {
    // EBML header magic; read just the DocType and the first video size/codec we can find by scanning for known ids
    if b.get(..4) != Some(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return None;
    }
    let text = |needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);
    let codec = if text(b"V_VP9") { "VP9" } else if text(b"V_VP8") { "VP8" } else if text(b"V_AV1") { "AV1" } else if text(b"V_MPEG4/ISO/AVC") { "H.264" } else { "" };
    let audio = if text(b"A_OPUS") { "Opus" } else if text(b"A_VORBIS") { "Vorbis" } else if text(b"A_AAC") { "AAC" } else { "" };
    let mut parts = vec![if text(b"webm") { "WebM".to_string() } else { "Matroska".to_string() }];
    if !codec.is_empty() {
        parts.push(format!("{codec} video"));
    }
    if !audio.is_empty() {
        parts.push(format!("{audio} audio"));
    }
    Some(parts.join(" · "))
}

/// A short description of a media file from its start, or an empty string when it is not one this knows.
pub fn describe(content_type: &str, head: &[u8]) -> String {
    let ct = content_type.to_ascii_lowercase();
    if !(ct.starts_with("video/") || ct.starts_with("audio/") || ct.starts_with("application/ogg") || ct == "application/octet-stream") {
        return String::new();
    }
    if let Some(d) = mp4(head) {
        return d;
    }
    if let Some(d) = webm(head) {
        return d;
    }
    if head.starts_with(b"OggS") {
        return "Ogg".into();
    }
    if head.starts_with(b"ID3") || (head.len() > 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        return "MP3 audio".into();
    }
    String::new()
}

/// Is this content type something a player should be offered for?
pub fn is_media(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("video/") || ct.starts_with("audio/") || ct.starts_with("application/ogg") || ct == "application/vnd.apple.mpegurl" || ct == "application/x-mpegurl"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(ty: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut v = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(content);
        v
    }

    fn sample() -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41");
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&5000u32.to_be_bytes());
        let mut tkhd = vec![0u8; 84];
        tkhd[76..80].copy_from_slice(&(640u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(360u32 << 16).to_be_bytes());
        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(b"vide");
        let mut stsd = vec![0u8; 8];
        stsd.extend_from_slice(&bx(b"avc1", &[0u8; 20]));
        let stsd_box = bx(b"stsd", &{
            let mut x = vec![0u8; 4];
            x.extend_from_slice(&1u32.to_be_bytes());
            x.extend_from_slice(&bx(b"avc1", &[0u8; 20]));
            x
        });
        let stbl = bx(b"stbl", &stsd_box);
        let minf = bx(b"minf", &stbl);
        let mdia = bx(b"mdia", &[bx(b"hdlr", &hdlr), minf].concat());
        let trak = bx(b"trak", &[bx(b"tkhd", &tkhd), mdia].concat());
        let moov = bx(b"moov", &[bx(b"mvhd", &mvhd), trak].concat());
        [ftyp, moov, bx(b"mdat", &[0u8; 64])].concat()
    }

    #[test]
    fn an_mp4_is_described_from_its_boxes() {
        assert_eq!(describe("video/mp4", &sample()), "H.264 video, 640×360 · 5 s");
        // the details at the end of a file that was only read in part
        let ftyp_only = bx(b"ftyp", b"isom\0\0\x02\0");
        assert!(describe("video/mp4", &ftyp_only).contains("at the end"), "{}", describe("video/mp4", &ftyp_only));
    }

    #[test]
    fn other_containers_and_junk_are_handled() {
        assert!(describe("video/webm", &[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0, b'w', b'e', b'b', b'm', b'V', b'_', b'V', b'P', b'9']).contains("VP9 video"));
        assert_eq!(describe("audio/ogg", b"OggS\0\0"), "Ogg");
        assert_eq!(describe("audio/mpeg", b"ID3\x04\0"), "MP3 audio");
        assert_eq!(describe("image/png", &sample()), "", "not a media type");
        for junk in [&[][..], &[0xff; 3], &[0u8; 16], b"ftypftypftypftyp"] {
            let _ = describe("video/mp4", junk);
        }
        // sizes that lie must not loop or panic
        let mut lie = sample();
        lie[0..4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        let _ = describe("video/mp4", &lie);
        assert!(is_media("video/mp4") && is_media("audio/mpeg") && !is_media("text/html"));
    }
}
