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
