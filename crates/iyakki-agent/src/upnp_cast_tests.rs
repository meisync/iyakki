use super::*;
use std::cell::RefCell;

const DESC: &str = "<?xml version=\"1.0\"?><root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType><friendlyName>Living &amp; Room TV</friendlyName><UDN>uuid:abc-123</UDN><serviceList>\
    <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/rc/control</controlURL></service>\
    <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/avt/control</controlURL></service></serviceList></device></root>";

fn reply(loc: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: {loc}\r\nST: {RENDERER_ST}\r\nUSN: uuid:abc-123::{RENDERER_ST}\r\n\r\n").into_bytes()
}

#[test]
fn the_search_and_its_answers_are_the_ones_of_upnp() {
    let s = String::from_utf8(m_search(9)).unwrap();
    assert!(s.starts_with("M-SEARCH * HTTP/1.1\r\n") && s.contains("MX: 5\r\n") && s.contains(&format!("ST: {RENDERER_ST}")) && s.contains("MAN: \"ssdp:discover\""));
    // and our own server reads it as a search (the two ends agree)
    assert!(crate::upnp::parse_search(&m_search(2)).is_some());
    let h = parse_hit(&reply("http://192.168.1.20:49152/desc.xml")).unwrap();
    assert_eq!(h.location, "http://192.168.1.20:49152/desc.xml");
    assert!(h.usn.starts_with("uuid:abc-123"));
    for bad in [&b"NOTIFY * HTTP/1.1\r\n\r\n"[..], &b"HTTP/1.1 404 Not Found\r\n\r\n"[..], &reply("https://x/desc.xml")[..], &reply("http://a b/c")[..], &[0xFF; 20][..]] {
        assert!(parse_hit(bad).is_none());
    }
    let other = format!("HTTP/1.1 200 OK\r\nLOCATION: http://192.168.1.2/d.xml\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n");
    assert!(parse_hit(other.as_bytes()).is_none(), "a media server is not a renderer");
    assert!(parse_hit(&vec![b'a'; 3000]).is_none());
}

#[test]
fn a_description_gives_the_name_and_the_control_address_of_its_own_host_only() {
    let r = parse_description(DESC, "http://192.168.1.20:49152/desc.xml").unwrap();
    assert_eq!((r.name.as_str(), r.udn.as_str(), r.control, r.control_path.as_str()), ("Living & Room TV", "uuid:abc-123", "192.168.1.20:49152".parse().unwrap(), "/avt/control"));
    // an absolute control address on the same host is taken; on another host it is not
    let abs = DESC.replace("/avt/control", "http://192.168.1.20:5000/ctl");
    assert_eq!(parse_description(&abs, "http://192.168.1.20:49152/d.xml").unwrap().control.port(), 5000);
    let away = DESC.replace("/avt/control", "http://10.9.9.9:5000/ctl");
    assert!(parse_description(&away, "http://192.168.1.20:49152/d.xml").unwrap_err().contains("another address"));
    // not on the local network, no AVTransport, no name, too large
    assert!(parse_description(DESC, "http://93.184.216.34/d.xml").unwrap_err().contains("local network"));
    assert!(parse_description(&DESC.replace("AVTransport", "Other"), "http://192.168.1.20/d.xml").unwrap_err().contains("AVTransport"));
    assert!(parse_description(&DESC.replace("Living &amp; Room TV", " "), "http://192.168.1.20/d.xml").is_err());
    assert!(parse_description(&"x".repeat(300_000), "http://192.168.1.20/d.xml").is_err());
}

struct Fake {
    log: RefCell<Vec<String>>,
    status: u16,
    fault: Option<&'static str>,
}

impl Wire for Fake {
    fn get(&self, _: SocketAddr, path: &str) -> Result<(u16, String), String> {
        self.log.borrow_mut().push(format!("GET {path}"));
        Ok((200, DESC.to_string()))
    }
    fn soap(&self, addr: SocketAddr, path: &str, action: &str, body: &str) -> Result<(u16, String), String> {
        self.log.borrow_mut().push(format!("POST {addr}{path} {action} {}", body.len()));
        self.log.borrow_mut().push(body.to_string());
        Ok((self.status, self.fault.map(|f| format!("<errorDescription>{f}</errorDescription>")).unwrap_or_default()))
    }
}

fn renderer() -> Renderer {
    parse_description(DESC, "http://192.168.1.20:49152/desc.xml").unwrap()
}

#[test]
fn a_file_is_cast_by_naming_it_and_then_playing_it_and_hostile_titles_stay_text() {
    let w = Fake { log: RefCell::new(vec![]), status: 200, fault: None };
    cast(&w, &renderer(), "http://192.168.1.5:8200/upnp/media/a3", "Song <b>&\"q\"", "audio/flac", false).unwrap();
    let log = w.log.borrow().clone();
    assert!(log[0].contains("192.168.1.20:49152/avt/control") && log[0].contains("SetAVTransportURI"));
    assert!(log[1].contains("<CurrentURI>http://192.168.1.5:8200/upnp/media/a3</CurrentURI>"));
    assert!(log[1].contains("Song &amp;lt;b&amp;gt;"), "the title is escaped once inside the metadata and once more as an element: {}", log[1]);
    assert!(log[2].contains("#Play") && log[3].contains("<Speed>1</Speed>"));
    assert_eq!(log.len(), 4);
    // a media address outside the local network is refused before anything is sent
    let w = Fake { log: RefCell::new(vec![]), status: 200, fault: None };
    assert!(cast(&w, &renderer(), "http://93.184.216.34/x.mp3", "t", "audio/mpeg", false).unwrap_err().contains("local network"));
    assert!(cast(&w, &renderer(), "https://192.168.1.5/x", "t", "audio/mpeg", false).is_err());
    assert!(w.log.borrow().is_empty());
}

#[test]
fn a_refusal_says_which_step_and_why_and_stop_works() {
    let w = Fake { log: RefCell::new(vec![]), status: 500, fault: Some("Transition not available") };
    let e = cast(&w, &renderer(), "http://192.168.1.5:8200/upnp/media/v1", "Film", "video/mp4", true).unwrap_err();
    assert!(e.contains("Living & Room TV refused SetAVTransportURI") && e.contains("Transition not available"), "{e}");
    assert_eq!(w.log.borrow().len(), 2, "Play was not tried after the refusal");
    let ok = Fake { log: RefCell::new(vec![]), status: 200, fault: None };
    stop(&ok, &renderer()).unwrap();
    assert!(ok.log.borrow()[0].contains("#Stop"));
    assert!(stop(&w, &renderer()).is_err());
}

#[test]
fn discovery_finds_a_renderer_over_a_real_socket_and_leaves_out_strangers_and_duplicates() {
    // the "network": one socket that answers the search the way a TV does, and a second description that is not on the local network
    let tv = UdpSocket::bind("127.0.0.1:0").unwrap();
    let tv_addr = tv.local_addr().unwrap();
    let me = UdpSocket::bind("127.0.0.1:0").unwrap();
    let me_addr = me.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let mut b = [0u8; 2048];
        let (n, _) = tv.recv_from(&mut b).unwrap();
        assert!(crate::upnp::parse_search(&b[..n]).is_some_and(|s| s.st == RENDERER_ST));
        // two answers with the same description (a repeat), one pointing away from the local network
        let loc = "http://192.168.1.20:49152/desc.xml";
        tv.send_to(&reply(loc), me_addr).unwrap();
        tv.send_to(&reply(loc), me_addr).unwrap();
        tv.send_to(&reply("http://93.184.216.34/d.xml"), me_addr).unwrap();
    });
    let w = Fake { log: RefCell::new(vec![]), status: 200, fault: None };
    let found = discover(&w, &me, tv_addr, Duration::from_secs(1), true);
    t.join().unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "Living & Room TV");
    assert_eq!(w.log.borrow().iter().filter(|l| l.starts_with("GET")).count(), 1, "the description was fetched once");
    assert_eq!(ssdp_group().port(), 1900);
}

/// A renderer that answers each action from a table.
struct Scripted {
    log: RefCell<Vec<String>>,
    answers: Vec<(&'static str, u16, String)>,
}

impl Wire for Scripted {
    fn get(&self, _: SocketAddr, _: &str) -> Result<(u16, String), String> {
        Ok((200, DESC.to_string()))
    }
    fn soap(&self, _: SocketAddr, _: &str, action: &str, body: &str) -> Result<(u16, String), String> {
        self.log.borrow_mut().push(format!("{action} {body}"));
        let name = action.trim_matches('"').rsplit('#').next().unwrap_or("");
        match self.answers.iter().find(|(n, _, _)| *n == name) {
            Some((_, st, b)) => Ok((*st, b.clone())),
            None => Ok((200, String::new())),
        }
    }
}

fn scripted(answers: Vec<(&'static str, u16, String)>) -> Scripted {
    Scripted { log: RefCell::new(vec![]), answers }
}

#[test]
fn times_are_written_and_read_the_way_renderers_do() {
    assert_eq!((hms(0), hms(75), hms(3723), hms(90_000)), ("0:00:00".to_string(), "0:01:15".to_string(), "1:02:03".to_string(), "25:00:00".to_string()));
    assert_eq!((parse_hms("0:01:23"), parse_hms("1:02:03.500"), parse_hms(" 0:00:00 ")), (Some(83), Some(3723), Some(0)));
    for bad in ["", "NOT_IMPLEMENTED", "1:02", "1:2:3:4", "-0:01:00", "0:61:00", "0:00:61", "a:b:c", "0:00:00000000", "+1:00:00"] {
        assert_eq!(parse_hms(bad), None, "{bad:?}");
    }
    for s in [0u64, 1, 59, 60, 3599, 3600, 86_399] {
        assert_eq!(parse_hms(&hms(s)), Some(s));
    }
}

#[test]
fn seek_pause_and_play_on_send_the_actions_of_upnp_and_a_refusal_says_which() {
    let w = scripted(vec![]);
    seek(&w, &renderer(), 3723).unwrap();
    pause(&w, &renderer()).unwrap();
    resume(&w, &renderer()).unwrap();
    let log = w.log.borrow().clone();
    assert!(log[0].contains("#Seek") && log[0].contains("<Unit>REL_TIME</Unit><Target>1:02:03</Target>"), "{}", log[0]);
    assert!(log[1].contains("#Pause") && log[2].contains("#Play") && log[2].contains("<Speed>1</Speed>"));
    // a jump is never longer than a day
    assert!(seek_body(10_000_000).contains("<Target>23:59:59</Target>"));
    let no = scripted(vec![("Seek", 500, "<errorDescription>Seek mode not supported</errorDescription>".into())]);
    let e = seek(&no, &renderer(), 5).unwrap_err();
    assert!(e.contains("Living & Room TV refused Seek") && e.contains("Seek mode not supported"), "{e}");
}

#[test]
fn progress_reads_state_position_and_length_and_says_it_in_one_line() {
    let info = "<s:Envelope><s:Body><u:GetTransportInfoResponse><CurrentTransportState>PLAYING</CurrentTransportState><CurrentTransportStatus>OK</CurrentTransportStatus></u:GetTransportInfoResponse></s:Body></s:Envelope>".to_string();
    let pos = "<u:GetPositionInfoResponse><Track>1</Track><TrackDuration>1:30:00</TrackDuration><RelTime>1:02:03</RelTime><AbsTime>NOT_IMPLEMENTED</AbsTime></u:GetPositionInfoResponse>".to_string();
    let w = scripted(vec![("GetTransportInfo", 200, info), ("GetPositionInfo", 200, pos)]);
    let p = progress(&w, &renderer()).unwrap();
    assert_eq!(p, Progress { state: "PLAYING".into(), position_secs: Some(3723), duration_secs: Some(5400) });
    assert_eq!(p.line(), "playing 1:02:03 of 1:30:00 (68 %)");
    // a stream with no known length, a paused one, one with nothing loaded, and a state that is not one of the usual
    assert_eq!(Progress { state: "PLAYING".into(), position_secs: Some(65), duration_secs: None }.line(), "playing 0:01:05");
    assert_eq!(Progress { state: "PAUSED_PLAYBACK".into(), position_secs: Some(1), duration_secs: Some(0) }.line(), "paused 0:00:01");
    assert_eq!(Progress { state: "NO_MEDIA_PRESENT".into(), position_secs: None, duration_secs: None }.line(), "nothing loaded");
    assert_eq!(Progress { state: "SOMETHING_NEW".into(), position_secs: None, duration_secs: None }.line(), "state unknown");
    // a position past the end is shown as the end; hostile text in the state is cut down to letters
    assert_eq!(Progress { state: "PLAYING".into(), position_secs: Some(9000), duration_secs: Some(100) }.line(), "playing 2:30:00 of 0:01:40 (100 %)");
    let hostile = scripted(vec![("GetTransportInfo", 200, "<CurrentTransportState>PLAY<script>1</script>ING\r\n</CurrentTransportState>".into())]);
    assert_eq!(progress(&hostile, &renderer()).unwrap().state, "PLAYING");
    // one question refused: the other still answers; both refused: the error
    let half = scripted(vec![("GetPositionInfo", 500, String::new()), ("GetTransportInfo", 200, "<CurrentTransportState>STOPPED</CurrentTransportState>".into())]);
    assert_eq!(progress(&half, &renderer()).unwrap(), Progress { state: "STOPPED".into(), position_secs: None, duration_secs: None });
    let none = scripted(vec![("GetPositionInfo", 500, String::new()), ("GetTransportInfo", 500, String::new())]);
    assert!(progress(&none, &renderer()).unwrap_err().contains("refused GetTransportInfo"));
}
