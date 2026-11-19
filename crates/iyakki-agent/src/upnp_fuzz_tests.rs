//! Fuzz tests (deterministic, seeded) for the UPnP/DLNA media server: SSDP datagrams, SOAP bodies and the DIDL answers, the throttle, and
//! the HTTP side over a real loopback socket (mutated requests, `Range` headers, hostile paths). A panic, a hang, a case over the heap bound
//! or a byte of a file outside the share fails the test and prints the seed and case.
//! `IYAKKI_FUZZ_CASES=20000 cargo test -p iyakki-agent upnp_fuzz` runs a larger set.

use crate::fuzzkit::{cases, mutate, run, Rng};
use crate::upnp::{self, Identity, ListCatalog, Media, Throttle};
use crate::upnp_serve::{self, ServeCfg};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

fn catalog(n: usize) -> ListCatalog {
    let m = |i: usize| Media {
        title: format!("T{i} <script>INJECT</script> & \"q\" '"),
        artist: "Ar\u{1}\u{2}tist </upnp:artist><x>".into(),
        album: "Al\r\nInjected: header".into(),
        video: i % 3 == 0,
        mime: if i % 3 == 0 { "video/mp4" } else { "audio/mpeg" }.into(),
        size: u64::MAX - i as u64,
        seconds: u32::MAX - i as u32,
    };
    ListCatalog { items: (0..n).map(m).collect(), update: u32::MAX, url_for: Box::new(|id| format!("http://192.168.1.5:8200/upnp/media/{id}?<>\"")), alt: None }
}

fn soap(action: &str, args: &str) -> String {
    format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action} xmlns:u=\"{}\">{args}</u:{action}></s:Body></s:Envelope>", upnp::CD_TYPE)
}

fn ident() -> Identity {
    Identity { friendly_name: "Ada's <media> & more".into(), uuid: "6f1c1c0a-1111-4222-8333-444455556666".into() }
}

#[test]
fn upnp_fuzz_ssdp_datagrams_and_the_throttle() {
    let corpus: Vec<Vec<u8>> = [
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: ssdp:all\r\n\r\n",
        "M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nMX: 99999999999\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n",
        "NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\n\r\n",
        "M-SEARCH * HTTP/1.1\r\nST: upnp:rootdevice\r\nMAN:ssdp:discover\r\nmx:-1\r\n\r\n",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    let n = run("ssdp", 0x55D, &corpus, 8000, Duration::from_secs(1), 16 << 20, |input| {
        let Some(s) = upnp::parse_search(input) else { return };
        assert!((1..=5).contains(&s.mx) && !s.st.is_empty() && s.st.len() <= 200 && !s.st.chars().any(|c| c.is_control()));
        let loc = "http://192.168.1.5:8200/upnp/device.xml";
        let replies = upnp::search_replies(&s, &ident().uuid, loc, "IYAKKI/1", "date");
        assert!(replies.len() <= 5, "an amplifier: {} replies", replies.len());
        for r in replies {
            assert!(r.len() < 1024, "a reply of {} bytes", r.len());
            let t = String::from_utf8(r).unwrap();
            assert!(t.ends_with("\r\n\r\n") && t.matches("\r\n\r\n").count() == 1, "reply is not one message: {t:?}");
        }
    });
    assert!(n >= cases(8000));
    // the throttle: never more than the limit per source per second, and its table stays bounded
    let mut r = Rng::new(0x55E);
    let mut t = Throttle::new();
    let mut now = 1_700_000_000i64;
    let mut allowed: std::collections::HashMap<(IpAddr, i64), u32> = std::collections::HashMap::new();
    for _ in 0..cases(40_000) {
        if r.chance(6) {
            now += r.below(3) as i64 - if r.chance(20) { 5 } else { 0 };
        }
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, r.below(40) as u8, r.below(40) as u8));
        if t.allow(ip, now) {
            let c = allowed.entry((ip, now)).or_default();
            *c += 1;
            assert!(*c <= upnp::MAX_PER_SECOND, "{ip} was allowed {} times in second {now}", *c);
        }
    }
}

#[test]
fn upnp_fuzz_soap_bodies_and_didl_answers_never_panic_or_balloon() {
    let cat = catalog(500);
    let id = ident();
    let corpus: Vec<Vec<u8>> = vec![
        soap("Browse", "<ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>10</RequestedCount><SortCriteria></SortCriteria>").into_bytes(),
        soap("Browse", "<ObjectID>a3</ObjectID><BrowseFlag>BrowseMetadata</BrowseFlag><StartingIndex>4294967295</StartingIndex><RequestedCount>4294967295</RequestedCount>").into_bytes(),
        soap("Browse", "<ObjectID>1</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><StartingIndex>-1</StartingIndex><RequestedCount>99999999999999999999</RequestedCount>").into_bytes(),
        soap("GetSystemUpdateID", "").into_bytes(),
        soap("GetProtocolInfo", "<a/><b/><c></c>").into_bytes(),
        format!("<s:Envelope><s:Body>{}", "<x>".repeat(2000)).into_bytes(),
        b"<Body><Browse><ObjectID>&lt;&amp;lt;&#0;&#x110000;</ObjectID></Browse></Body>".to_vec(),
        "<Body>\u{4e2d}<\u{4e2d}:Browse><\u{4e2d}/></Body>".as_bytes().to_vec(),
    ];
    let mut k = 0;
    let n = run("soap", 0x50A, &corpus, 8000, Duration::from_secs(2), 64 << 20, |input| {
        k += 1;
        let (path, action) = match k % 4 {
            0 => ("/upnp/cm/control", Some("\"urn:schemas-upnp-org:service:ConnectionManager:1#GetProtocolInfo\"")),
            1 => ("/upnp/cd/control", Some("\"urn:schemas-upnp-org:service:ContentDirectory:1#Browse\"")),
            _ => ("/upnp/cd/control", None),
        };
        let Some(rep) = upnp::handle("POST", path, action, input, &id, &cat) else { panic!("a control address was not handled") };
        assert!(matches!(rep.status, 200 | 400 | 500), "status {}", rep.status);
        // one page is bounded: at most MAX_PAGE entries, whatever was asked
        let body = String::from_utf8(rep.body).expect("the answer is UTF-8");
        assert!(body.matches("<item ").count() <= upnp::MAX_PAGE as usize, "{} items in one answer", body.matches("<item ").count());
        assert!(body.len() < 400_000, "an answer of {} bytes", body.len());
        // hostile catalogue text is escaped (the markup in a title is data); the answer holds no raw script tag and no control characters
        assert!(!body.contains("<script>") && !body.contains("<x>") && !body.chars().any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')), "unescaped catalogue text in the answer");
        let _ = upnp::parse_action(&String::from_utf8_lossy(input));
    });
    assert!(n >= cases(8000));
}

// ---- the HTTP side, over a real socket ----

struct Site {
    dir: std::path::PathBuf,
    server: Option<upnp_serve::Running>,
    addr: SocketAddr,
    song: Vec<u8>,
    ids: Vec<(String, Vec<u8>)>,
}

impl Site {
    fn new() -> Site {
        let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let dir = std::path::PathBuf::from(base).join(format!("iyakki-upnp-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("share/sub")).unwrap();
        std::fs::create_dir_all(dir.join("outside")).unwrap();
        let song: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("share/song.mp3"), &song).unwrap();
        std::fs::write(dir.join("share/sub/film.mp4"), vec![7u8; 1000]).unwrap();
        std::fs::write(dir.join("share/empty.mp3"), b"").unwrap();
        std::fs::write(dir.join("share/.secret.mp3"), b"HIDDEN-SECRET").unwrap();
        std::fs::write(dir.join("outside/leak.mp3"), b"OUTSIDE-SECRET").unwrap();
        std::os::unix::fs::symlink(dir.join("outside/leak.mp3"), dir.join("share/link.mp3")).unwrap();
        let cfg = ServeCfg { name: "Fuzz".into(), uuid: "6f1c1c0a-1111-4222-8333-444455556666".into(), http_bind: "127.0.0.1:0".parse().unwrap(), ssdp_bind: "127.0.0.1:0".parse().unwrap(), multicast: false, folders: vec![dir.join("share")], deny: vec![], allow_loopback: true, head_deadline: std::time::Duration::from_secs(10), audit: Arc::new(|_, _| {}), transcoder: None };
        let lib = upnp_serve::scan(&cfg.folders, &cfg.deny);
        let ids: Vec<(String, Vec<u8>)> = lib.paths.iter().enumerate().map(|(i, p)| (format!("{}{i}", if lib.items[i].video { 'v' } else { 'a' }), std::fs::read(p).unwrap())).collect();
        let server = upnp_serve::start(cfg).unwrap();
        let addr = server.http_addr();
        Site { dir, server: Some(server), addr, song, ids }
    }

    fn send(&self, raw: &[u8]) -> Vec<u8> {
        let mut s = TcpStream::connect(self.addr).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = s.write_all(raw);
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut out = Vec::new();
        let mut chunk = [0u8; 16384];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&chunk[..n]);
                    assert!(out.len() < 4 << 20, "a response of {} bytes", out.len());
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe) => break,
                Err(e) => panic!("the server did not answer or close within 5 s: {e}"),
            }
        }
        out
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        if let Some(s) = self.server.take() {
            s.stop();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn request(path: &str, extra: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:8200\r\n{extra}\r\n").into_bytes()
}

#[test]
fn upnp_fuzz_http_requests_ranges_and_hostile_paths() {
    let site = Site::new();
    let song_id = site.ids.iter().find(|(_, d)| *d == site.song).map(|(i, _)| i.clone()).unwrap();
    let body = soap("Browse", "<ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><StartingIndex>0</StartingIndex><RequestedCount>5</RequestedCount>");
    let post = format!("POST /upnp/cd/control HTTP/1.1\r\nHost: 127.0.0.1:8200\r\nSOAPACTION: \"{}#Browse\"\r\nContent-Length: {}\r\n\r\n{body}", upnp::CD_TYPE, body.len());
    let corpus: Vec<Vec<u8>> = vec![
        request(&format!("/upnp/media/{song_id}"), "Range: bytes=10-99\r\n"),
        request(&format!("/upnp/media/{song_id}"), "Range: bytes=-100\r\n"),
        request(&format!("/upnp/media/{song_id}"), "Range: bytes=4990-\r\n"),
        request(&format!("/upnp/media/{song_id}"), "Range: bytes=18446744073709551615-18446744073709551615\r\n"),
        request(&format!("/upnp/media/{song_id}"), "Range: bytes=0-0,5-9\r\nConnection: keep-alive\r\n"),
        request("/upnp/media/../../outside/leak.mp3", ""),
        request("/upnp/media/%2e%2e%2foutside%2fleak.mp3", ""),
        request("/upnp/media/a1/../../x", ""),
        request("/upnp/device.xml", ""),
        request("/upnp/cd.xml", "X-A: b\r\n"),
        post.into_bytes(),
        format!("HEAD /upnp/media/{song_id} HTTP/1.1\r\nHost: 192.168.1.5\r\nRange: bytes=0-3\r\n\r\n").into_bytes(),
        b"SUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: 127.0.0.1\r\nCALLBACK: <http://x/>\r\nNT: upnp:event\r\n\r\n".to_vec(),
        b"GET /upnp/device.xml HTTP/1.1\r\nHost: evil.example\r\n\r\n".to_vec(),
        b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 99999999\r\n\r\n".to_vec(),
        format!("GET /upnp/media/{} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", "a".repeat(9000)).into_bytes(),
    ];
    let mut served = 0usize;
    // a connection costs the server's accept poll (20 ms), so this test samples: a dozen cuts of each request, then the mutations
    let mut inputs: Vec<Vec<u8>> = Vec::new();
    for c in &corpus {
        inputs.push(c.clone());
        for k in 1..12 {
            inputs.push(c[..c.len() * k / 12].to_vec());
        }
    }
    let mut r = Rng::new(0x4B7);
    for _ in 0..cases(150) {
        let b = r.pick(&corpus).clone();
        inputs.push(mutate(&mut r, &b));
    }
    let n = inputs.len();
    for (case, input) in inputs.iter().enumerate() {
        let _g = crate::fuzzkit::Report::new(format!("upnp http case {case}: {:?}", String::from_utf8_lossy(&input[..input.len().min(200)])));
        let input: &[u8] = input;
        let out = site.send(input);
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("OUTSIDE-SECRET") && !text.contains("HIDDEN-SECRET"), "a file outside the share leaked");
        if out.is_empty() {
            continue;
        }
        assert!(out.starts_with(b"HTTP/1.1 "), "not an HTTP answer: {:?}", &out[..out.len().min(40)]);
        let at = out.windows(4).position(|w| w == b"\r\n\r\n").expect("a head");
        let head = String::from_utf8_lossy(&out[..at]).into_owned();
        let body = &out[at + 4..];
        let status: u16 = head.split(' ').nth(1).and_then(|s| s.parse().ok()).expect("a status");
        assert!(head.lines().skip(1).all(|l| l.contains(": ") || l.ends_with(':')), "a malformed header line in {head:?}");
        let is_head = input.starts_with(b"HEAD ");
        if let Some(len) = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.parse::<usize>().ok()) {
            if !is_head {
                assert_eq!(body.len(), len, "Content-Length disagrees with the body in {head:?}");
            }
        }
        if status == 206 && !is_head {
            // a range answer is exactly that range of a real file
            let cr = head.lines().find_map(|l| l.strip_prefix("Content-Range: bytes ")).expect("Content-Range");
            let (range, size) = cr.split_once('/').unwrap();
            let (a, b) = range.split_once('-').unwrap();
            let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
            assert!(site.ids.iter().any(|(_, d)| d.len().to_string() == size && d.len() > b && body == &d[a..=b]), "the bytes of a range answer are not that range of a shared file");
            served += 1;
        }
    }
    assert!(n >= cases(200) && served > 0, "the corpus must reach the range code ({served})");
}
