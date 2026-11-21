use super::*;
use crate::upnp::{Catalog, Entry, ListCatalog, Media};
use std::io::{Read, Write};
use std::sync::Arc;

struct Fake {
    makes: Vec<&'static str>,
}

impl Transcoder for Fake {
    fn can(&self, t: &Target) -> bool {
        self.makes.contains(&t.name)
    }
    fn start(&self, t: &Target, input: &Path) -> Result<Box<dyn Read + Send>, String> {
        if !self.can(t) {
            return Err("cannot".into());
        }
        // what the converter would have made: the target's name and the input's file name
        let body = format!("{}:{}", t.name, input.file_name().and_then(|n| n.to_str()).unwrap_or(""));
        Ok(Box::new(std::io::Cursor::new(body.into_bytes())))
    }
}

#[test]
fn a_file_players_play_goes_as_it_is_and_the_others_get_the_target_the_converter_can_make() {
    let all = |_: &Target| true;
    assert_eq!(plan("audio/mpeg", false, &all), Some(Plan::Direct));
    assert_eq!(plan("video/mp4", true, &all), Some(Plan::Direct));
    assert_eq!(plan("audio/flac", false, &all), Some(Plan::Transcode(target("mp3").unwrap())));
    assert_eq!(plan("audio/flac", false, &|t: &Target| t.name == "wav"), Some(Plan::Transcode(target("wav").unwrap())), "without the mp3 encoder, WAV");
    assert_eq!(plan("video/x-matroska", true, &all), Some(Plan::Transcode(target("mpegts").unwrap())));
    assert_eq!(plan("video/x-matroska", true, &|_| false), None, "no converter, nothing offered");
    assert!(target("../../etc").is_none() && target("mp3;rm").is_none(), "a player chooses a name from the list, never a command");
}

#[test]
fn the_command_line_is_fixed_has_no_shell_and_keeps_the_input_a_single_argument() {
    let evil = Path::new("/share/a; rm -rf ~ $(x) \"q\".flac");
    for t in TARGETS {
        let a = ffmpeg_args(&t, evil);
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert_eq!(a[i + 1], evil.display().to_string(), "the file is one argument whatever its name holds");
        assert!(a.contains(&"-protocol_whitelist".into()) && a[a.iter().position(|x| x == "-protocol_whitelist").unwrap() + 1] == "file", "no network input");
        assert!(a.contains(&"-nostdin".into()) && a.contains(&"-map_metadata".into()) && a.last().unwrap() == "pipe:1");
        assert_eq!(a.iter().filter(|x| x.as_str() == "-i").count(), 1);
    }
    assert!(ffmpeg_args(&target("mp3").unwrap(), evil).contains(&"-vn".into()), "a cover picture is dropped from audio");
}

#[test]
fn the_encoder_list_of_ffmpeg_is_read_and_a_target_needs_its_own_encoders() {
    let text = " V..... = Video\n ------\n V....D mpeg2video           MPEG-2 video\n A....D mp2                  MP2 (MPEG audio layer 2)\n A....D pcm_s16le            PCM signed 16-bit little-endian\n";
    let names = encoder_names(text);
    assert_eq!(names, vec!["mpeg2video", "mp2", "pcm_s16le"]);
    let f = Ffmpeg { exe: "/x".into(), encoders: names };
    assert!(f.can(&target("wav").unwrap()) && f.can(&target("mpegts").unwrap()));
    assert!(!f.can(&target("mp3").unwrap()), "no libmp3lame in this build");
    assert!(f.start(&target("mp3").unwrap(), Path::new("/x.flac")).is_err());
}

#[test]
fn a_catalog_offers_the_converted_copy_beside_the_file_only_when_it_may_not_play() {
    let m = |title: &str, mime: &str, video: bool| Media { title: title.into(), artist: String::new(), album: String::new(), video, mime: mime.into(), size: 10, seconds: 0 };
    let tc: Arc<dyn Transcoder> = Arc::new(Fake { makes: vec!["mp3"] });
    let cat = ListCatalog {
        items: vec![m("plain", "audio/mpeg", false), m("lossless", "audio/flac", false), m("film", "video/x-matroska", true)],
        update: 1,
        url_for: Box::new(|id| format!("http://10.0.0.5/upnp/media/{id}")),
        alt: Some(Box::new(move |m| match plan(&m.mime, m.video, &|t| tc.can(t)) { Some(Plan::Transcode(t)) => Some((t.mime.to_string(), t.name.to_string())), _ => None })),
    };
    let (kids, _) = cat.children("1", 0, 10).unwrap();
    let alt = |e: &Entry| if let Entry::Item { alt, .. } = e { alt.clone() } else { None };
    assert_eq!(alt(&kids[0]), None, "an mp3 plays as it is");
    assert_eq!(alt(&kids[1]), Some(("audio/mpeg".into(), "http://10.0.0.5/upnp/media/a1?t=mp3".into())));
    let (films, _) = cat.children("2", 0, 10).unwrap();
    assert_eq!(alt(&films[0]), None, "this converter cannot make the video target");
    let xml = crate::upnp::didl(&kids);
    assert_eq!(xml.matches("<res ").count(), 3, "two items, one with a second res: {xml}");
    assert!(xml.contains("DLNA.ORG_OP=00;DLNA.ORG_CI=1\">http://10.0.0.5/upnp/media/a1?t=mp3</res>"));
}

fn tree(tag: &str) -> PathBuf {
    let d = crate::testdir::outside_project("upnptc", tag);
    std::fs::create_dir_all(d.join("share")).unwrap();
    d
}

fn raw_get(addr: std::net::SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", addr.port()).as_bytes()).unwrap();
    let mut all = Vec::new();
    s.read_to_end(&mut all).unwrap();
    let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&all[..split]).to_string();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head, all[split + 4..].to_vec())
}

#[test]
fn the_server_streams_a_converted_copy_of_a_shared_file_and_refuses_anything_else() {
    let d = tree("serve");
    std::fs::write(d.join("share/song.flac"), b"not really flac").unwrap();
    std::fs::write(d.join("share/clip.mp3"), b"not really mp3").unwrap();
    let mut cfg = crate::upnp_serve::ServeCfg { name: "T".into(), uuid: "6f1c1c0a-1111-4222-8333-444455556666".into(), http_bind: "127.0.0.1:0".parse().unwrap(), ssdp_bind: "127.0.0.1:0".parse().unwrap(), multicast: false, folders: vec![d.join("share")], deny: vec![], allow_loopback: true, head_deadline: std::time::Duration::from_secs(10), audit: Arc::new(|_, _| {}), transcoder: None, lan: None };
    // without a converter the query is ignored and the file is served as it is
    let plain = crate::upnp_serve::start(cfg.clone()).unwrap();
    let lib_ids = |a| {
        let (_, _, body) = raw_get(a, "/upnp/media/a0");
        body
    };
    assert!(!lib_ids(plain.http_addr()).is_empty());
    let (st, _, body) = raw_get(plain.http_addr(), "/upnp/media/a0?t=mp3");
    assert_eq!(st, 200);
    assert!(!String::from_utf8_lossy(&body).starts_with("mp3:"), "no converter: the file itself");
    plain.stop();
    cfg.transcoder = Some(Arc::new(Fake { makes: vec!["mp3", "wav"] }));
    let run = crate::upnp_serve::start(cfg).unwrap();
    // find the flac: items are sorted by the library, so look at both
    let mut found = None;
    for i in 0..2 {
        let (st, head, body) = raw_get(run.http_addr(), &format!("/upnp/media/a{i}?t=mp3"));
        if st == 200 && body.starts_with(b"mp3:song.flac") {
            assert!(head.contains("Content-Type: audio/mpeg") && !head.contains("Content-Length") && head.contains("Accept-Ranges: none"), "{head}");
            found = Some(i);
        }
    }
    assert!(found.is_some(), "the flac came out converted");
    // a target the converter cannot make, an unknown name and an id that is not there
    assert_eq!(raw_get(run.http_addr(), "/upnp/media/a0?t=mpegts").0, 404);
    let (_, _, plain_body) = raw_get(run.http_addr(), "/upnp/media/a0?t=nonsense");
    assert!(!String::from_utf8_lossy(&plain_body).starts_with("nonsense:"), "an unknown name is not a converter name: the file is served as it is");
    assert_eq!(raw_get(run.http_addr(), "/upnp/media/a99?t=mp3").0, 404);
    run.stop();
}
