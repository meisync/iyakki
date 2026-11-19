//! IYAKKI as the controller of a UPnP/DLNA renderer (F-MEDIA-11, casting): find the TVs and speakers on the local network, and tell one of
//! them to fetch a file from this machine's own media server and play it. Nothing here serves media (that is `upnp_serve`) and nothing is
//! sent beyond the local network: the renderer's control address must be a private or link-local address, and the file's address must be one
//! of this machine's own server (the caller builds it).
//!
//! The wire formats are those of UPnP AV: an `M-SEARCH` for `urn:schemas-upnp-org:device:MediaRenderer:1`, the device description (name and
//! the control address of its `AVTransport` service), and the SOAP actions `SetAVTransportURI`, `Play`, `Pause` and `Stop`.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::upnp::esc;

pub const RENDERER_ST: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub const AVTRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
/// The most renderers one search keeps.
pub const MAX_RENDERERS: usize = 32;

/// The search datagram (`mx` seconds is how long renderers may take to answer).
pub fn m_search(mx: u8) -> Vec<u8> {
    format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: {}\r\nST: {RENDERER_ST}\r\nUSER-AGENT: IYAKKI/1 UPnP/1.0\r\n\r\n", mx.clamp(1, 5)).into_bytes()
}

/// One answer to the search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub location: String,
    pub usn: String,
}

/// A datagram that answers the search with a renderer's description address (anything else is `None`).
pub fn parse_hit(datagram: &[u8]) -> Option<Hit> {
    if datagram.len() > 2048 {
        return None;
    }
    let text = std::str::from_utf8(datagram).ok()?;
    let mut lines = text.split("\r\n");
    if !lines.next()?.starts_with("HTTP/1.1 200") {
        return None;
    }
    let (mut st, mut location, mut usn) = (false, None, String::new());
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "st" => st = v == RENDERER_ST,
            "location" => location = Some(v.to_string()),
            "usn" => usn = v.chars().filter(|c| !c.is_control()).take(200).collect(),
            _ => {}
        }
    }
    let location = location.filter(|l| st && l.starts_with("http://") && l.len() <= 400 && !l.chars().any(|c| c.is_control() || c == ' '))?;
    Some(Hit { location, usn })
}

/// `http://host:port/path` taken apart: the socket address (a literal address, no name lookup) and the path.
pub fn split_http(url: &str) -> Option<(SocketAddr, String)> {
    let rest = url.strip_prefix("http://")?;
    let (auth, path) = rest.split_once('/').map(|(a, p)| (a, format!("/{p}"))).unwrap_or((rest, "/".into()));
    if auth.contains('@') {
        return None;
    }
    let with_port = if auth.contains(':') && !auth.starts_with('[') || auth.starts_with('[') && auth.contains("]:") { auth.to_string() } else { format!("{auth}:80") };
    let addr: SocketAddr = with_port.parse().ok()?;
    Some((addr, path))
}

/// May this renderer be talked to: a private or link-local address on the local network, never the internet.
pub fn on_local_network(a: IpAddr) -> bool {
    crate::upnp::may_answer(a)
}

/// A renderer, as its description says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renderer {
    pub name: String,
    pub udn: String,
    /// Where the AVTransport actions go (an address on the renderer's own host).
    pub control: SocketAddr,
    pub control_path: String,
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

/// Unescape the five XML entities a description uses.
fn unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Read a device description fetched from `location`: the friendly name, the unique name and the control address of the AVTransport service.
/// The control address must be on the same host as the description and on the local network (a description may not send actions elsewhere).
pub fn parse_description(xml: &str, location: &str) -> Result<Renderer, String> {
    if xml.len() > 200_000 {
        return Err("the description is too large".into());
    }
    let (host_addr, _) = split_http(location).ok_or("the description address is not usable")?;
    if !on_local_network(host_addr.ip()) {
        return Err("that device is not on the local network".into());
    }
    let name = between(xml, "<friendlyName>", "</friendlyName>").map(unesc).map(|n| n.chars().filter(|c| !c.is_control()).take(60).collect::<String>()).filter(|n| !n.trim().is_empty()).ok_or("the device has no name")?;
    let udn = between(xml, "<UDN>", "</UDN>").map(unesc).map(|n| n.chars().filter(|c| !c.is_control()).take(100).collect::<String>()).unwrap_or_default();
    // the service block that names AVTransport, and its controlURL
    let mut at = 0;
    let mut control = None;
    while let Some(i) = xml[at..].find("<service>") {
        let from = at + i;
        let end = xml[from..].find("</service>").map(|e| from + e).unwrap_or(xml.len());
        let block = &xml[from..end];
        if block.contains("AVTransport") {
            control = between(block, "<controlURL>", "</controlURL>").map(unesc);
            break;
        }
        at = end.max(from + 1);
    }
    let control = control.ok_or("the device cannot play media from the network (no AVTransport)")?;
    let (control, control_path) = if control.starts_with("http://") {
        split_http(&control).ok_or("the control address is not usable")?
    } else if control.starts_with('/') && !control.contains(' ') {
        (host_addr, control)
    } else {
        return Err("the control address is not usable".into());
    };
    if control.ip() != host_addr.ip() || !on_local_network(control.ip()) {
        return Err("the device sent its controls to another address".into());
    }
    Ok(Renderer { name, udn, control, control_path })
}

fn envelope(action: &str, args: &str) -> String {
    format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{AVTRANSPORT}\"><InstanceID>0</InstanceID>{args}</u:{action}></s:Body></s:Envelope>")
}

/// A title for a file as the renderer shows it.
pub fn didl_for(title: &str, mime: &str, url: &str, video: bool) -> String {
    let class = if video { "object.item.videoItem" } else { "object.item.audioItem.musicTrack" };
    format!("<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"><item id=\"1\" parentID=\"0\" restricted=\"1\"><dc:title>{}</dc:title><upnp:class>{class}</upnp:class><res protocolInfo=\"http-get:*:{}:*\">{}</res></item></DIDL-Lite>", esc(title), esc(mime), esc(url))
}

pub fn set_uri_body(url: &str, didl: &str) -> String {
    envelope("SetAVTransportURI", &format!("<CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>", esc(url), esc(didl)))
}

pub fn play_body() -> String {
    envelope("Play", "<Speed>1</Speed>")
}

pub fn stop_body() -> String {
    envelope("Stop", "")
}

pub fn pause_body() -> String {
    envelope("Pause", "")
}

/// The HTTP port: what the controller needs of the network (a test puts its own).
pub trait Wire {
    /// GET `path` from `addr`; the body (limited) and the status.
    fn get(&self, addr: SocketAddr, path: &str) -> Result<(u16, String), String>;
    /// POST a SOAP action; the status and the answer body.
    fn soap(&self, addr: SocketAddr, path: &str, action: &str, body: &str) -> Result<(u16, String), String>;
}

/// Tell `r` to fetch `media_url` and play it. `media_url` must be on this machine's own server: the caller built it and passes the
/// address the renderer should use; it is checked to be a local-network address here too.
pub fn cast(wire: &dyn Wire, r: &Renderer, media_url: &str, title: &str, mime: &str, video: bool) -> Result<(), String> {
    let (maddr, _) = split_http(media_url).ok_or("the media address is not usable")?;
    if !on_local_network(maddr.ip()) {
        return Err("a renderer is only given files from this computer's own server on the local network".into());
    }
    let act = |name: &str, body: String| -> Result<(), String> {
        let (st, ans) = wire.soap(r.control, &r.control_path, &format!("\"{AVTRANSPORT}#{name}\""), &body)?;
        if (200..300).contains(&st) {
            Ok(())
        } else {
            let why = between(&ans, "<errorDescription>", "</errorDescription>").map(unesc).unwrap_or_default();
            Err(format!("{} refused {name} ({st}{}{})", r.name, if why.is_empty() { "" } else { ": " }, why.chars().filter(|c| !c.is_control()).take(100).collect::<String>()))
        }
    };
    act("SetAVTransportURI", set_uri_body(media_url, &didl_for(title, mime, media_url, video)))?;
    act("Play", play_body())
}

pub fn stop(wire: &dyn Wire, r: &Renderer) -> Result<(), String> {
    let (st, _) = wire.soap(r.control, &r.control_path, &format!("\"{AVTRANSPORT}#Stop\""), &stop_body())?;
    if (200..300).contains(&st) { Ok(()) } else { Err(format!("{} refused Stop ({st})", r.name)) }
}

/// Look for renderers: send the search, take answers for `wait`, fetch each description. A device that does not parse or is not on the
/// local network is left out (`allow_loopback` is for tests); at most [`MAX_RENDERERS`] are kept.
pub fn discover(wire: &dyn Wire, sock: &UdpSocket, group: SocketAddr, wait: Duration, allow_loopback: bool) -> Vec<Renderer> {
    let _ = sock.send_to(&m_search(wait.as_secs().clamp(1, 5) as u8), group);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let end = Instant::now() + wait;
    let mut hits: Vec<Hit> = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < end && hits.len() < MAX_RENDERERS * 2 {
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if on_local_network(from.ip()) || (allow_loopback && from.ip().is_loopback()) {
                if let Some(h) = parse_hit(&buf[..n]) {
                    if !hits.iter().any(|x| x.location == h.location) {
                        hits.push(h);
                    }
                }
            }
        }
    }
    let mut out: Vec<Renderer> = Vec::new();
    for h in hits {
        let Some((addr, path)) = split_http(&h.location) else { continue };
        // a description is only fetched from the local network: a device may not make this computer call out to the internet
        if !on_local_network(addr.ip()) {
            continue;
        }
        if let Ok((200, xml)) = wire.get(addr, &path) {
            if let Ok(r) = parse_description(&xml, &h.location) {
                if !out.iter().any(|o| o.udn == r.udn && !r.udn.is_empty()) && out.len() < MAX_RENDERERS {
                    out.push(r);
                }
            }
        }
    }
    out
}

/// The real network: plain HTTP/1.1 over TCP with short timeouts and small bodies.
pub struct TcpWire;

impl TcpWire {
    fn exchange(addr: SocketAddr, req: &str) -> Result<(u16, String), String> {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("could not reach the device: {e}"))?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut all = Vec::new();
        let mut chunk = [0u8; 4096];
        while all.len() < 300_000 {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => all.extend_from_slice(&chunk[..n]),
            }
        }
        let text = String::from_utf8_lossy(&all).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or("the device did not answer in HTTP")?;
        Ok((status, text.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default()))
    }
}

impl Wire for TcpWire {
    fn get(&self, addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
        Self::exchange(addr, &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"))
    }
    fn soap(&self, addr: SocketAddr, path: &str, action: &str, body: &str) -> Result<(u16, String), String> {
        Self::exchange(addr, &format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nSOAPACTION: {action}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()))
    }
}

/// The multicast group of SSDP.
pub fn ssdp_group() -> SocketAddr {
    ("239.255.255.250", 1900).to_socket_addrs().ok().and_then(|mut a| a.next()).unwrap_or_else(|| SocketAddr::from(([239, 255, 255, 250], 1900)))
}

#[cfg(test)]
#[path = "upnp_cast_tests.rs"]
mod tests;
