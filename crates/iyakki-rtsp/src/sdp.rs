//! The parts of an SDP description (RFC 4566) a camera client needs: the media sections with their payload types, `rtpmap`, `fmtp` and
//! `control` attributes.

use crate::hash::base64_decode;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Media {
    /// `video`, `audio`, `application` ...
    pub kind: String,
    pub proto: String,
    pub payload_types: Vec<u8>,
    pub control: Option<String>,
    /// payload type -> (encoding name in upper case, clock rate)
    pub rtpmap: Vec<(u8, String, u32)>,
    pub fmtp: Vec<(u8, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Sdp {
    pub session_control: Option<String>,
    pub media: Vec<Media>,
}

pub const MAX_MEDIA: usize = 16;

pub fn parse(text: &str) -> Sdp {
    let mut out = Sdp::default();
    let mut cur: Option<Media> = None;
    for line in text.lines().take(500) {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "m" => {
                if let Some(m) = cur.take() {
                    out.media.push(m);
                }
                if out.media.len() >= MAX_MEDIA {
                    break;
                }
                let mut it = v.split_whitespace();
                let kind = it.next().unwrap_or("").to_ascii_lowercase();
                let _port = it.next();
                let proto = it.next().unwrap_or("").to_string();
                let payload_types = it.filter_map(|p| p.parse::<u8>().ok().filter(|p| *p < 128)).collect();
                cur = Some(Media { kind, proto, payload_types, ..Default::default() });
            }
            "a" => {
                let (name, val) = v.split_once(':').unwrap_or((v, ""));
                let val = val.trim();
                match (name, cur.as_mut()) {
                    ("control", Some(m)) => m.control = Some(val.to_string()),
                    ("control", None) => out.session_control = Some(val.to_string()),
                    ("rtpmap", Some(m)) => {
                        let (pt, rest) = val.split_once(' ').unwrap_or((val, ""));
                        let mut p = rest.trim().split('/');
                        if let (Ok(pt), Some(enc)) = (pt.parse::<u8>(), p.next()) {
                            let clock = p.next().and_then(|c| c.parse().ok()).unwrap_or(90000);
                            m.rtpmap.push((pt, enc.to_ascii_uppercase(), clock));
                        }
                    }
                    ("fmtp", Some(m)) => {
                        if let Some((pt, rest)) = val.split_once(' ') {
                            if let Ok(pt) = pt.parse::<u8>() {
                                m.fmtp.push((pt, rest.trim().to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Some(m) = cur {
        out.media.push(m);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Payload type 26 or a dynamic `JPEG` rtpmap.
    Mjpeg,
    H264 { sps: Vec<u8>, pps: Vec<u8>, packetization_mode: u8 },
    /// Anything else, by its encoding name.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoTrack {
    pub payload_type: u8,
    pub codec: Codec,
    pub control: Option<String>,
}

impl Sdp {
    /// The first video track, with its codec worked out. Static payload type 26 is Motion-JPEG without any rtpmap line.
    pub fn video(&self) -> Option<VideoTrack> {
        let m = self.media.iter().find(|m| m.kind == "video")?;
        let pt = *m.payload_types.first()?;
        let map = m.rtpmap.iter().find(|r| r.0 == pt);
        let codec = match (pt, map.map(|r| r.1.as_str())) {
            (26, _) | (_, Some("JPEG")) => Codec::Mjpeg,
            (_, Some("H264")) => {
                let f = m.fmtp.iter().find(|f| f.0 == pt).map(|f| f.1.as_str()).unwrap_or("");
                let mut sps = vec![];
                let mut pps = vec![];
                let mut mode = 0u8;
                for kv in f.split(';') {
                    let (k, v) = kv.trim().split_once('=').unwrap_or((kv.trim(), ""));
                    match k {
                        "sprop-parameter-sets" => {
                            let mut parts = v.split(',').filter_map(base64_decode);
                            sps = parts.next().unwrap_or_default();
                            pps = parts.next().unwrap_or_default();
                        }
                        "packetization-mode" => mode = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
                Codec::H264 { sps, pps, packetization_mode: mode }
            }
            (_, Some(name)) => Codec::Other(name.chars().take(20).collect()),
            _ => Codec::Other(format!("payload type {pt}")),
        };
        Some(VideoTrack { payload_type: pt, codec, control: m.control.clone().or_else(|| self.session_control.clone()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H264: &str = "v=0\r\no=- 0 0 IN IP4 10.0.0.2\r\ns=cam\r\na=control:*\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;profile-level-id=64001F;sprop-parameter-sets=Z2QAH6zZQFAFuhAAAAMAEAAAAwPA8YMZYA==,aOvjyyLA\r\na=control:trackID=1\r\nm=audio 0 RTP/AVP 0\r\na=control:trackID=2\r\n";

    #[test]
    fn an_h264_camera_is_described_with_its_parameter_sets() {
        let v = parse(H264).video().unwrap();
        assert_eq!((v.payload_type, v.control.as_deref()), (96, Some("trackID=1")));
        match v.codec {
            Codec::H264 { sps, pps, packetization_mode } => {
                assert_eq!(sps[0] & 0x1f, 7, "an SPS NAL");
                assert_eq!(pps[0] & 0x1f, 8, "a PPS NAL");
                assert_eq!(packetization_mode, 1);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn motion_jpeg_is_payload_26_with_or_without_an_rtpmap() {
        assert_eq!(parse("m=video 0 RTP/AVP 26\r\na=control:track1\r\n").video().unwrap().codec, Codec::Mjpeg);
        assert_eq!(parse("m=video 0 RTP/AVP 97\r\na=rtpmap:97 JPEG/90000\r\n").video().unwrap().codec, Codec::Mjpeg);
        assert_eq!(parse("m=video 0 RTP/AVP 98\r\na=rtpmap:98 H265/90000\r\n").video().unwrap().codec, Codec::Other("H265".into()));
        assert!(parse("m=audio 0 RTP/AVP 0\r\n").video().is_none());
        assert!(parse("").video().is_none());
    }

    #[test]
    fn junk_and_floods_do_not_panic() {
        let junk = "m=\r\nm=video\r\na=\r\na=rtpmap:\r\na=rtpmap:x y\r\na=fmtp:96\r\n=\r\nm=video 0 RTP/AVP 300 x\r\n";
        let _ = parse(junk).video();
        let flood: String = (0..100).map(|_| "m=video 0 RTP/AVP 96\r\n").collect();
        assert!(parse(&flood).media.len() <= MAX_MEDIA);
        assert_eq!(parse("m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 sprop-parameter-sets=!!!\r\n").video().unwrap().codec, Codec::H264 { sps: vec![], pps: vec![], packetization_mode: 0 });
    }
}
