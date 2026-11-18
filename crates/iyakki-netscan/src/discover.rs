//! Machines that announce themselves: SSDP/UPnP (media servers, TVs, routers), WS-Discovery (ONVIF cameras) and mDNS
//! (printers, speakers, Apple and Google devices). One question each is sent to the local network and the answers are
//! listened to for a couple of seconds. The answers are matched to the addresses being scanned only.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::model::{Confidence, Kind, Service};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub ip: Ipv4Addr,
    pub name: Option<String>,
    pub service: Service,
}

fn listen(sock: &UdpSocket, wait: Duration, mut each: impl FnMut(&[u8], Ipv4Addr)) {
    let end = Instant::now() + wait;
    let mut buf = vec![0u8; 8192];
    while Instant::now() < end {
        let _ = sock.set_read_timeout(Some(end.saturating_duration_since(Instant::now()).max(Duration::from_millis(10))));
        if let Ok((n, SocketAddr::V4(from))) = sock.recv_from(&mut buf) {
            each(&buf[..n], *from.ip());
        }
    }
}

fn socket() -> Option<UdpSocket> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    let _ = s.set_multicast_ttl_v4(2);
    Some(s)
}

// ---------------------------------------------------------------------------------------------------------- SSDP

pub fn ssdp(wait: Duration) -> Vec<Announcement> {
    let Some(sock) = socket() else { return vec![] };
    let q = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n";
    let _ = sock.send_to(q.as_bytes(), "239.255.255.250:1900");
    let mut out = Vec::new();
    listen(&sock, wait, |data, ip| out.extend(parse_ssdp(data, ip)));
    out
}

pub fn parse_ssdp(data: &[u8], ip: Ipv4Addr) -> Option<Announcement> {
    let text = String::from_utf8_lossy(data);
    if !text.starts_with("HTTP/1.1 200") && !text.starts_with("NOTIFY") {
        return None;
    }
    let h = |name: &str| text.lines().find(|l| l.to_ascii_lowercase().starts_with(&format!("{name}:"))).map(|l| l.split_once(':').map_or("", |x| x.1).trim().to_string()).unwrap_or_default();
    let (server, st, location) = (h("server"), h("st"), h("location"));
    let hay = format!("{server} {st}").to_ascii_lowercase();
    let (kind, name) = if hay.contains("mediaserver") || hay.contains("roku") || hay.contains("dlna") || hay.contains("sonos") || hay.contains("chromecast") || hay.contains("mediarenderer") {
        (Kind::Media, "UPnP media device")
    } else if hay.contains("internetgatewaydevice") || hay.contains("wanipconnection") {
        (Kind::Device, "router (UPnP)")
    } else if hay.contains("printer") {
        (Kind::Printer, "printer (UPnP)")
    } else if hay.contains("camera") || hay.contains("networkvideo") {
        (Kind::Camera, "camera (UPnP)")
    } else {
        (Kind::Other, "UPnP device")
    };
    let mut s = Service::new(1900, kind, name);
    s.proto = "udp";
    s.confidence = Confidence::Medium;
    s.detail = [server, st].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    s.url = (!location.is_empty()).then_some(location);
    Some(Announcement { ip, name: None, service: s })
}

// ---------------------------------------------------------------------------------------------------- WS-Discovery

pub fn ws_discovery(wait: Duration) -> Vec<Announcement> {
    let Some(sock) = socket() else { return vec![] };
    let probe = "<?xml version=\"1.0\" encoding=\"utf-8\"?><e:Envelope xmlns:e=\"http://www.w3.org/2003/05/soap-envelope\" xmlns:w=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" xmlns:d=\"http://schemas.xmlsoap.org/ws/2005/04/discovery\" xmlns:dn=\"http://www.onvif.org/ver10/network/wsdl\"><e:Header><w:MessageID>uuid:84ede3de-7dec-11d0-c360-f01234567890</w:MessageID><w:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header><e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>";
    let _ = sock.send_to(probe.as_bytes(), "239.255.255.250:3702");
    let mut out = Vec::new();
    listen(&sock, wait, |data, ip| out.extend(parse_ws_discovery(data, ip)));
    out
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = xml.find(&format!(":{name}>")).or_else(|| xml.find(&format!("<{name}>")))?;
    let start = xml[open..].find('>')? + open + 1;
    let end = xml[start..].find("</")? + start;
    Some(xml[start..end].trim())
}

pub fn parse_ws_discovery(data: &[u8], ip: Ipv4Addr) -> Option<Announcement> {
    let xml = String::from_utf8_lossy(data);
    if !xml.contains("ProbeMatch") {
        return None;
    }
    let scopes = tag(&xml, "Scopes").unwrap_or("");
    let scope = |key: &str| scopes.split_whitespace().find_map(|s| s.split_once(&format!("onvif://www.onvif.org/{key}/")).map(|x| x.1.replace("%20", " ")));
    let (name, hardware) = (scope("name"), scope("hardware"));
    let xaddrs = tag(&xml, "XAddrs").unwrap_or("").split_whitespace().next().unwrap_or("").to_string();
    let mut s = Service::new(3702, Kind::Camera, format!("ONVIF camera{}", name.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()));
    s.proto = "udp";
    s.confidence = Confidence::High;
    s.detail = [hardware.map(|h| format!("hardware {h}")), (!xaddrs.is_empty()).then(|| format!("control at {xaddrs}"))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    s.url = (!xaddrs.is_empty()).then_some(xaddrs);
    Some(Announcement { ip, name, service: s })
}

// ---------------------------------------------------------------------------------------------------------- mDNS

const MDNS_NAMES: &[&str] = &["_services._dns-sd._udp.local", "_rtsp._tcp.local", "_ipp._tcp.local", "_printer._tcp.local", "_googlecast._tcp.local", "_airplay._tcp.local", "_http._tcp.local", "_hap._tcp.local", "_homekit._tcp.local", "_smb._tcp.local", "_ssh._tcp.local", "_iyakki-dev._tcp.local", "_iyakki-web._tcp.local"];

fn dns_query(name: &str) -> Vec<u8> {
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 12, 0, 1]); // PTR, class IN
    q
}

pub fn mdns(wait: Duration) -> Vec<Announcement> {
    let Some(sock) = socket() else { return vec![] };
    for n in MDNS_NAMES {
        let _ = sock.send_to(&dns_query(n), "224.0.0.251:5353");
    }
    let mut out = Vec::new();
    listen(&sock, wait, |data, ip| out.extend(parse_mdns(data, ip)));
    out
}

fn read_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut parts = Vec::new();
    let mut end = None;
    for _ in 0..40 {
        let len = *msg.get(at)? as usize;
        if len == 0 {
            return Some((parts.join("."), end.unwrap_or(at + 1)));
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *msg.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            at = ptr;
            continue;
        }
        parts.push(String::from_utf8_lossy(msg.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
    None
}

/// The service names announced in an mDNS answer.
pub fn parse_mdns(msg: &[u8], ip: Ipv4Addr) -> Option<Announcement> {
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None; // not an answer
    }
    let (qd, an) = (u16::from_be_bytes([msg[4], msg[5]]) as usize, u16::from_be_bytes([msg[6], msg[7]]) as usize + u16::from_be_bytes([msg[8], msg[9]]) as usize + u16::from_be_bytes([msg[10], msg[11]]) as usize);
    let mut at = 12;
    for _ in 0..qd {
        at = read_name(msg, at)?.1 + 4;
    }
    let (mut ptrs, mut srv_port, mut host) = (Vec::<String>::new(), None, None);
    for _ in 0..an.min(40) {
        let (name, next) = read_name(msg, at)?;
        let ty = u16::from_be_bytes([*msg.get(next)?, *msg.get(next + 1)?]);
        let len = u16::from_be_bytes([*msg.get(next + 8)?, *msg.get(next + 9)?]) as usize;
        let data = next + 10;
        match ty {
            12 => ptrs.push(read_name(msg, data).map(|x| x.0).unwrap_or_default()),
            33 => {
                srv_port = msg.get(data + 4..data + 6).map(|p| u16::from_be_bytes([p[0], p[1]]));
                host = read_name(msg, data + 6).map(|x| x.0);
            }
            _ => {}
        }
        let _ = name;
        at = data + len;
    }
    let all = ptrs.join(" ").to_ascii_lowercase();
    if all.is_empty() {
        return None;
    }
    let (kind, label) = if all.contains("_iyakki-dev.") || all.contains("_iyakki-web.") { (Kind::Device, "IYAKKI device (developer mode / web service)") } else if all.contains("_rtsp.") || all.contains("camera") { (Kind::Camera, "RTSP video") } else if all.contains("_ipp.") || all.contains("_printer.") { (Kind::Printer, "printer (mDNS)") } else if all.contains("_googlecast.") || all.contains("_airplay.") { (Kind::Media, "casting device (mDNS)") } else if all.contains("_hap.") || all.contains("_homekit.") { (Kind::SmartHome, "HomeKit device (mDNS)") } else if all.contains("_smb.") { (Kind::Access, "file sharing (mDNS)") } else if all.contains("_ssh.") { (Kind::Access, "SSH (mDNS)") } else { (Kind::Other, "device announcing itself (mDNS)") };
    let mut s = Service::new(srv_port.unwrap_or(5353), kind, label);
    s.proto = "udp";
    s.confidence = Confidence::Medium;
    s.detail = ptrs.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    Some(Announcement { ip, name: host.map(|h| h.trim_end_matches(".local").to_string()), service: s })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssdp_answers_are_classified() {
        let ip = Ipv4Addr::new(192, 168, 1, 9);
        let tv = b"HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: http://192.168.1.9:8060/\r\nSERVER: Roku/12.0 UPnP/1.0\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        let a = parse_ssdp(tv, ip).unwrap();
        assert_eq!((a.service.kind, a.service.url.as_deref()), (Kind::Media, Some("http://192.168.1.9:8060/")));
        let gw = b"HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nSERVER: Linux UPnP/1.0\r\n\r\n";
        assert_eq!(parse_ssdp(gw, ip).unwrap().service.kind, Kind::Device);
        assert!(parse_ssdp(b"M-SEARCH * HTTP/1.1\r\n", ip).is_none() && parse_ssdp(b"garbage", ip).is_none());
    }

    #[test]
    fn onvif_cameras_answer_ws_discovery_with_a_name_and_an_address() {
        let xml = "<s:Envelope><s:Body><d:ProbeMatches><d:ProbeMatch><d:Types>dn:NetworkVideoTransmitter</d:Types><d:Scopes>onvif://www.onvif.org/type/video_encoder onvif://www.onvif.org/name/Front%20Door onvif://www.onvif.org/hardware/IPC-B1</d:Scopes><d:XAddrs>http://192.168.1.30:80/onvif/device_service</d:XAddrs></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>";
        let a = parse_ws_discovery(xml.as_bytes(), Ipv4Addr::new(192, 168, 1, 30)).unwrap();
        assert_eq!((a.service.kind, a.name.as_deref()), (Kind::Camera, Some("Front Door")));
        assert!(a.service.name.contains("Front Door") && a.service.detail.contains("IPC-B1") && a.service.url.as_deref() == Some("http://192.168.1.30:80/onvif/device_service"));
        assert!(parse_ws_discovery(b"<x/>", Ipv4Addr::LOCALHOST).is_none());
    }

    #[test]
    fn mdns_answers_name_what_a_device_offers_and_bad_packets_are_ignored() {
        // an answer with one PTR record: _ipp._tcp.local -> Office Printer._ipp._tcp.local
        let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let name = |n: &str| {
            let mut v = Vec::new();
            for l in n.split('.') {
                v.push(l.len() as u8);
                v.extend_from_slice(l.as_bytes());
            }
            v.push(0);
            v
        };
        m.extend(name("_ipp._tcp.local"));
        m.extend([0, 12, 0, 1, 0, 0, 0x11, 0x94]);
        let target = name("Office Printer._ipp._tcp.local");
        m.extend((target.len() as u16).to_be_bytes());
        m.extend(target);
        let a = parse_mdns(&m, Ipv4Addr::new(192, 168, 1, 40)).unwrap();
        assert_eq!(a.service.kind, Kind::Printer);
        assert!(a.service.detail.contains("Office Printer"));
        // an IYAKKI device announcing its developer port
        let mut d = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        d.extend(name("_iyakki-dev._tcp.local"));
        d.extend([0, 12, 0, 1, 0, 0, 0x11, 0x94]);
        let inst = name("iyakki-0a1b2c._iyakki-dev._tcp.local");
        d.extend((inst.len() as u16).to_be_bytes());
        d.extend(inst);
        let found = parse_mdns(&d, Ipv4Addr::new(192, 168, 1, 41)).unwrap();
        assert_eq!(found.service.kind, Kind::Device);
        assert!(found.service.name.contains("IYAKKI") && found.service.detail.contains("iyakki-0a1b2c"));
        // a query (not an answer), truncated and random data
        assert!(parse_mdns(&dns_query("_ipp._tcp.local"), Ipv4Addr::LOCALHOST).is_none());
        for cut in 0..m.len() {
            let _ = parse_mdns(&m[..cut], Ipv4Addr::LOCALHOST);
        }
        let mut looped = m.clone();
        looped[12] = 0xC0;
        looped[13] = 12; // a name that points at itself
        assert!(parse_mdns(&looped, Ipv4Addr::LOCALHOST).is_none(), "compression loops end");
    }
}
