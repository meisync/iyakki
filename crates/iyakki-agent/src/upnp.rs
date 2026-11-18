//! UPnP / DLNA media server, the protocol half (F-MEDIA-02): finding the server (SSDP), describing it (device and service
//! documents), and answering a player's browse requests (SOAP, DIDL-Lite). Everything here works on bytes and strings: no socket is
//! opened, no folder is read and nothing is started by itself. A caller that opens the multicast socket and the HTTP port (later,
//! with the person's approval) feeds datagrams and requests in and sends what comes out.
//!
//! Rules built in: only searches from the local network are answered, a source may ask only a few times a second, a reply is one
//! small datagram per matching target, only the read-only browse actions exist (no search, no upload, no subscription), and a
//! player can only name items the catalog gives it, never a path.

use std::collections::BTreeMap;
use std::net::IpAddr;

pub const SSDP_ADDR: &str = "239.255.255.250:1900";
pub const DEVICE_TYPE: &str = "urn:schemas-upnp-org:device:MediaServer:1";
pub const CD_TYPE: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
pub const CM_TYPE: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";
pub const MAX_AGE: u32 = 1800;

// ---------------------------------------------------------------------------------------------------------------- xml helpers

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

// -------------------------------------------------------------------------------------------------------------------- SSDP

/// What a player asked for with M-SEARCH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub st: String,
    /// Seconds the player is willing to wait (1 to 5): the caller delays the reply by a random part of it.
    pub mx: u8,
}

/// A datagram that is an M-SEARCH for discovery; anything else (notifications of other devices, junk) is `None`.
pub fn parse_search(datagram: &[u8]) -> Option<Search> {
    if datagram.len() > 2048 {
        return None;
    }
    let text = std::str::from_utf8(datagram).ok()?;
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    if !first.eq_ignore_ascii_case("M-SEARCH * HTTP/1.1") {
        return None;
    }
    let (mut man, mut st, mut mx) = (false, None, 2u8);
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "man" => man = v.trim_matches('"').eq_ignore_ascii_case("ssdp:discover"),
            "st" => st = Some(v.to_string()),
            "mx" => mx = v.parse::<u8>().unwrap_or(2).clamp(1, 5),
            _ => {}
        }
    }
    let st = st.filter(|s| !s.is_empty() && s.len() <= 200 && !s.chars().any(|c| c.is_control()))?;
    man.then_some(Search { st, mx })
}

/// The (NT, USN) pairs this server is known by.
pub fn targets(uuid: &str) -> Vec<(String, String)> {
    let u = format!("uuid:{uuid}");
    vec![
        ("upnp:rootdevice".into(), format!("{u}::upnp:rootdevice")),
        (u.clone(), u.clone()),
        (DEVICE_TYPE.into(), format!("{u}::{DEVICE_TYPE}")),
        (CD_TYPE.into(), format!("{u}::{CD_TYPE}")),
        (CM_TYPE.into(), format!("{u}::{CM_TYPE}")),
    ]
}

/// The replies to a search: one datagram for every target it matches. `location` is the address of the device description.
pub fn search_replies(s: &Search, uuid: &str, location: &str, server: &str, date: &str) -> Vec<Vec<u8>> {
    targets(uuid)
        .into_iter()
        .filter(|(nt, _)| {
            // `ssdp:all` gets the root device only (a player reads the services from its description): one small answer, not five; the
            // connection manager is found through the root device and is not answered on its own
            if s.st == "ssdp:all" { nt == "upnp:rootdevice" } else { nt != CM_TYPE && s.st.eq_ignore_ascii_case(nt) }
        })
        .map(|(nt, usn)| format!("HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nDATE: {date}\r\nEXT:\r\nLOCATION: {location}\r\nSERVER: {server}\r\nST: {nt}\r\nUSN: {usn}\r\n\r\n").into_bytes())
        .collect()
}

/// `ssdp:alive` (when the server starts and now and then) or `ssdp:byebye` (when it stops), one datagram per target.
pub fn notifications(alive: bool, uuid: &str, location: &str, server: &str) -> Vec<Vec<u8>> {
    targets(uuid)
        .into_iter()
        .map(|(nt, usn)| {
            if alive {
                format!("NOTIFY * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nLOCATION: {location}\r\nNT: {nt}\r\nNTS: ssdp:alive\r\nSERVER: {server}\r\nUSN: {usn}\r\n\r\n").into_bytes()
            } else {
                format!("NOTIFY * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nNT: {nt}\r\nNTS: ssdp:byebye\r\nUSN: {usn}\r\n\r\n").into_bytes()
            }
        })
        .collect()
}

/// Only private and link-local sources are answered (the same meaning of "local network" as the shared folders).
pub fn may_answer(src: IpAddr) -> bool {
    match src {
        IpAddr::V4(a) => a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => (a.segments()[0] & 0xffc0) == 0xfe80 || (a.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// A source may ask this many times in a second; more are ignored (a multicast search must not become an amplifier).
pub struct Throttle {
    seen: BTreeMap<IpAddr, (i64, u32)>,
}

pub const MAX_PER_SECOND: u32 = 5;
const MAX_SOURCES: usize = 512;

impl Default for Throttle {
    fn default() -> Throttle {
        Throttle::new()
    }
}

impl Throttle {
    pub fn new() -> Throttle {
        Throttle { seen: BTreeMap::new() }
    }
    pub fn allow(&mut self, src: IpAddr, now: i64) -> bool {
        if self.seen.len() >= MAX_SOURCES && !self.seen.contains_key(&src) {
            self.seen.retain(|_, (t, _)| *t >= now);
            if self.seen.len() >= MAX_SOURCES {
                return false;
            }
        }
        let e = self.seen.entry(src).or_insert((now, 0));
        if e.0 != now {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 <= MAX_PER_SECOND
    }
}

// ---------------------------------------------------------------------------------------------------------- the documents

/// What the server tells about itself.
#[derive(Clone, Debug)]
pub struct Identity {
    /// Shown on the player's list.
    pub friendly_name: String,
    /// A stable uuid for this machine and person (the caller derives it; it must not leak a serial number).
    pub uuid: String,
}

pub fn device_xml(id: &Identity) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<root xmlns=\"urn:schemas-upnp-org:device-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><device><deviceType>{DEVICE_TYPE}</deviceType><friendlyName>{}</friendlyName><manufacturer>IYAKKI</manufacturer><modelName>IYAKKI media server</modelName><UDN>uuid:{}</UDN><serviceList><service><serviceType>{CD_TYPE}</serviceType><serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId><SCPDURL>/upnp/cd.xml</SCPDURL><controlURL>/upnp/cd/control</controlURL><eventSubURL>/upnp/cd/event</eventSubURL></service><service><serviceType>{CM_TYPE}</serviceType><serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId><SCPDURL>/upnp/cm.xml</SCPDURL><controlURL>/upnp/cm/control</controlURL><eventSubURL>/upnp/cm/event</eventSubURL></service></serviceList></device></root>",
        esc(&id.friendly_name),
        esc(&id.uuid)
    )
}

fn scpd(actions: &[(&str, &[(&str, &str, &str)])], vars: &[(&str, &str, bool)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?>\n<scpd xmlns=\"urn:schemas-upnp-org:service-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><actionList>");
    for (name, args) in actions {
        s.push_str(&format!("<action><name>{name}</name><argumentList>"));
        for (arg, dir, var) in *args {
            s.push_str(&format!("<argument><name>{arg}</name><direction>{dir}</direction><relatedStateVariable>{var}</relatedStateVariable></argument>"));
        }
        s.push_str("</argumentList></action>");
    }
    s.push_str("</actionList><serviceStateTable>");
    for (name, ty, events) in vars {
        s.push_str(&format!("<stateVariable sendEvents=\"{}\"><name>{name}</name><dataType>{ty}</dataType></stateVariable>", if *events { "yes" } else { "no" }));
    }
    s.push_str("</serviceStateTable></scpd>");
    s
}

pub fn content_directory_scpd() -> String {
    scpd(
        &[
            ("Browse", &[("ObjectID", "in", "A_ARG_TYPE_ObjectID"), ("BrowseFlag", "in", "A_ARG_TYPE_BrowseFlag"), ("Filter", "in", "A_ARG_TYPE_Filter"), ("StartingIndex", "in", "A_ARG_TYPE_Index"), ("RequestedCount", "in", "A_ARG_TYPE_Count"), ("SortCriteria", "in", "A_ARG_TYPE_SortCriteria"), ("Result", "out", "A_ARG_TYPE_Result"), ("NumberReturned", "out", "A_ARG_TYPE_Count"), ("TotalMatches", "out", "A_ARG_TYPE_Count"), ("UpdateID", "out", "A_ARG_TYPE_UpdateID")]),
            ("Search", &[("ContainerID", "in", "A_ARG_TYPE_ObjectID"), ("SearchCriteria", "in", "A_ARG_TYPE_SearchCriteria"), ("Filter", "in", "A_ARG_TYPE_Filter"), ("StartingIndex", "in", "A_ARG_TYPE_Index"), ("RequestedCount", "in", "A_ARG_TYPE_Count"), ("SortCriteria", "in", "A_ARG_TYPE_SortCriteria"), ("Result", "out", "A_ARG_TYPE_Result"), ("NumberReturned", "out", "A_ARG_TYPE_Count"), ("TotalMatches", "out", "A_ARG_TYPE_Count"), ("UpdateID", "out", "A_ARG_TYPE_UpdateID")]),
            ("GetSearchCapabilities", &[("SearchCaps", "out", "SearchCapabilities")]),
            ("GetSortCapabilities", &[("SortCaps", "out", "SortCapabilities")]),
            ("GetSystemUpdateID", &[("Id", "out", "SystemUpdateID")]),
        ],
        &[("A_ARG_TYPE_ObjectID", "string", false), ("A_ARG_TYPE_Result", "string", false), ("A_ARG_TYPE_BrowseFlag", "string", false), ("A_ARG_TYPE_Filter", "string", false), ("A_ARG_TYPE_SearchCriteria", "string", false), ("A_ARG_TYPE_SortCriteria", "string", false), ("A_ARG_TYPE_Index", "ui4", false), ("A_ARG_TYPE_Count", "ui4", false), ("A_ARG_TYPE_UpdateID", "ui4", false), ("SearchCapabilities", "string", false), ("SortCapabilities", "string", false), ("SystemUpdateID", "ui4", true)],
    )
}

pub fn connection_manager_scpd() -> String {
    scpd(
        &[
            ("GetProtocolInfo", &[("Source", "out", "SourceProtocolInfo"), ("Sink", "out", "SinkProtocolInfo")]),
            ("GetCurrentConnectionIDs", &[("ConnectionIDs", "out", "CurrentConnectionIDs")]),
            ("GetCurrentConnectionInfo", &[("ConnectionID", "in", "A_ARG_TYPE_ConnectionID"), ("RcsID", "out", "A_ARG_TYPE_RcsID"), ("AVTransportID", "out", "A_ARG_TYPE_AVTransportID"), ("ProtocolInfo", "out", "A_ARG_TYPE_ProtocolInfo"), ("PeerConnectionManager", "out", "A_ARG_TYPE_ConnectionManager"), ("PeerConnectionID", "out", "A_ARG_TYPE_ConnectionID"), ("Direction", "out", "A_ARG_TYPE_Direction"), ("Status", "out", "A_ARG_TYPE_ConnectionStatus")]),
        ],
        &[("SourceProtocolInfo", "string", true), ("SinkProtocolInfo", "string", true), ("CurrentConnectionIDs", "string", true), ("A_ARG_TYPE_ConnectionStatus", "string", false), ("A_ARG_TYPE_ConnectionManager", "string", false), ("A_ARG_TYPE_Direction", "string", false), ("A_ARG_TYPE_ProtocolInfo", "string", false), ("A_ARG_TYPE_ConnectionID", "i4", false), ("A_ARG_TYPE_AVTransportID", "i4", false), ("A_ARG_TYPE_RcsID", "i4", false)],
    )
}

// -------------------------------------------------------------------------------------------------------------- the catalog

/// One thing a player can see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Container { id: String, parent: String, title: String, children: u32 },
    Item { id: String, parent: String, title: String, artist: String, album: String, /// `object.item.audioItem.musicTrack` or `object.item.videoItem`
        class: String, mime: String, size: u64, seconds: u32, /// The address the player fetches the file from (made by the caller).
        url: String },
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Entry::Container { id, .. } | Entry::Item { id, .. } => id,
        }
    }
}

/// What the server lets players see. The caller decides what is behind it (a person's library, for example).
pub trait Catalog {
    /// The entry with this id, or `None`. Id `0` is the root.
    fn get(&self, id: &str) -> Option<Entry>;
    /// The children of a container, `count` of them from `start`, and how many there are in all. `None` for an unknown or non-container id.
    fn children(&self, id: &str, start: u32, count: u32) -> Option<(Vec<Entry>, u32)>;
    /// Changes whenever the content changes, so players refresh.
    fn update_id(&self) -> u32;
}

/// Most entries sent in one answer.
pub const MAX_PAGE: u32 = 200;

pub(crate) fn didl(entries: &[Entry]) -> String {
    let mut s = String::from("<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">");
    for e in entries {
        match e {
            Entry::Container { id, parent, title, children } => s.push_str(&format!("<container id=\"{}\" parentID=\"{}\" restricted=\"1\" childCount=\"{children}\"><dc:title>{}</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>", esc(id), esc(parent), esc(title))),
            Entry::Item { id, parent, title, artist, album, class, mime, size, seconds, url } => {
                s.push_str(&format!("<item id=\"{}\" parentID=\"{}\" restricted=\"1\"><dc:title>{}</dc:title>", esc(id), esc(parent), esc(title)));
                if !artist.is_empty() {
                    s.push_str(&format!("<upnp:artist>{}</upnp:artist>", esc(artist)));
                }
                if !album.is_empty() {
                    s.push_str(&format!("<upnp:album>{}</upnp:album>", esc(album)));
                }
                s.push_str(&format!("<upnp:class>{}</upnp:class><res protocolInfo=\"http-get:*:{}:*\" size=\"{size}\"", esc(class), esc(mime)));
                if *seconds > 0 {
                    s.push_str(&format!(" duration=\"{}:{:02}:{:02}\"", seconds / 3600, seconds / 60 % 60, seconds % 60));
                }
                s.push_str(&format!(">{}</res></item>", esc(url)));
            }
        }
    }
    s.push_str("</DIDL-Lite>");
    s
}

// ------------------------------------------------------------------------------------------------------------------- SOAP

/// A SOAP action: its name and arguments (element text, namespaces ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    pub args: BTreeMap<String, String>,
}

/// Read the first element inside `<Body>` of a SOAP request. Hostile input (deep nesting, huge text) is cut off, never followed.
pub fn parse_action(body: &str) -> Option<Action> {
    if body.len() > 16 * 1024 {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    let b = lower.find("body>")?;
    let rest = &body[b + 5..];
    let open = rest.find('<')?;
    let rest = &rest[open + 1..];
    let end = rest.find(['>', ' ', '/'])?;
    let tag = &rest[..end];
    let name = tag.rsplit(':').next()?.to_string();
    if name.is_empty() || name.starts_with('/') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let after = &rest[rest.find('>')? + 1..];
    let close = after.find(&format!("</{tag}>")).unwrap_or(after.len());
    let mut inner = &after[..close];
    let mut args = BTreeMap::new();
    while let Some(i) = inner.find('<') {
        let t = &inner[i + 1..];
        let Some(e) = t.find('>') else { break };
        let el = t[..e].trim_end_matches('/').trim();
        let el_name = el.split_whitespace().next().unwrap_or("");
        if el_name.starts_with('/') || el_name.is_empty() {
            inner = &t[e + 1..];
            continue;
        }
        let key = el_name.rsplit(':').next().unwrap_or(el_name).to_string();
        if t[..e].ends_with('/') {
            args.insert(key, String::new());
            inner = &t[e + 1..];
            continue;
        }
        let val_start = e + 1;
        let Some(c) = t[val_start..].find("</") else { break };
        args.insert(key, unesc(&t[val_start..val_start + c]));
        let skip = t[val_start + c..].find('>').map_or(t.len(), |x| val_start + c + x + 1);
        inner = &t[skip.min(t.len())..];
        if args.len() > 32 {
            return None;
        }
    }
    Some(Action { name, args })
}

fn envelope(inner: &str) -> String {
    format!("<?xml version=\"1.0\"?>\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>{inner}</s:Body></s:Envelope>")
}

pub fn soap_response(service: &str, action: &str, out: &[(&str, String)]) -> String {
    let mut s = format!("<u:{action}Response xmlns:u=\"{service}\">");
    for (k, v) in out {
        s.push_str(&format!("<{k}>{}</{k}>", esc(v)));
    }
    s.push_str(&format!("</u:{action}Response>"));
    envelope(&s)
}

pub fn soap_fault(code: u16, text: &str) -> String {
    envelope(&format!("<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>{code}</errorCode><errorDescription>{}</errorDescription></UPnPError></detail></s:Fault>", esc(text)))
}

fn browse(cat: &dyn Catalog, a: &Action) -> Result<String, (u16, &'static str)> {
    let id = a.args.get("ObjectID").map(String::as_str).unwrap_or("0");
    let flag = a.args.get("BrowseFlag").map(String::as_str).unwrap_or("");
    let start = a.args.get("StartingIndex").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
    let want = a.args.get("RequestedCount").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
    let count = if want == 0 { MAX_PAGE } else { want.min(MAX_PAGE) };
    let (entries, total) = match flag {
        "BrowseMetadata" => (vec![cat.get(id).ok_or((701, "No such object"))?], 1),
        "BrowseDirectChildren" => cat.children(id, start, count).ok_or((701, "No such object"))?,
        _ => return Err((402, "Invalid args")),
    };
    Ok(soap_response(CD_TYPE, "Browse", &[("Result", didl(&entries)), ("NumberReturned", entries.len().to_string()), ("TotalMatches", total.to_string()), ("UpdateID", cat.update_id().to_string())]))
}

/// What the server answers over HTTP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

fn xml(status: u16, body: String) -> Reply {
    Reply { status, content_type: "text/xml; charset=\"utf-8\"".into(), body: body.into_bytes() }
}

fn plain(status: u16) -> Reply {
    Reply { status, content_type: "text/plain".into(), body: Vec::new() }
}

/// The MIME types the server can offer, for `GetProtocolInfo`.
pub const SOURCE_PROTOCOLS: &str = "http-get:*:audio/mpeg:*,http-get:*:audio/flac:*,http-get:*:audio/ogg:*,http-get:*:audio/wav:*,http-get:*:audio/mp4:*,http-get:*:video/mp4:*,http-get:*:video/webm:*,http-get:*:video/x-matroska:*";

/// Answer one HTTP request addressed to the UPnP part of the server. `None`: not a UPnP address (the caller serves its own).
/// Only `GET` for documents and `POST` for control; subscriptions are answered by the serving half (`upnp_serve`), which knows who asks (501 here).
pub fn handle(method: &str, path: &str, soap_action: Option<&str>, body: &[u8], id: &Identity, cat: &dyn Catalog) -> Option<Reply> {
    let m = method.to_ascii_uppercase();
    match (m.as_str(), path) {
        ("GET" | "HEAD", "/upnp/device.xml") => Some(xml(200, device_xml(id))),
        ("GET" | "HEAD", "/upnp/cd.xml") => Some(xml(200, content_directory_scpd())),
        ("GET" | "HEAD", "/upnp/cm.xml") => Some(xml(200, connection_manager_scpd())),
        ("SUBSCRIBE" | "UNSUBSCRIBE", "/upnp/cd/event" | "/upnp/cm/event") => Some(plain(501)),
        ("POST", "/upnp/cd/control" | "/upnp/cm/control") => {
            let cd = path == "/upnp/cd/control";
            let service = if cd { CD_TYPE } else { CM_TYPE };
            let text = String::from_utf8_lossy(body);
            let Some(a) = parse_action(&text) else { return Some(xml(400, soap_fault(401, "Invalid action"))) };
            // the SOAPACTION header, when sent, has to name the same service and action
            if let Some(h) = soap_action {
                let h = h.trim().trim_matches('"');
                if !h.is_empty() && h != format!("{service}#{}", a.name) {
                    return Some(xml(500, soap_fault(401, "Invalid action")));
                }
            }
            let fault = |c: u16, t: &str| xml(500, soap_fault(c, t));
            Some(match (cd, a.name.as_str()) {
                (true, "Browse") => match browse(cat, &a) {
                    Ok(x) => xml(200, x),
                    Err((c, t)) => fault(c, t),
                },
                (true, "Search") => match crate::upnp_search::search(cat, &a) {
                    Ok(x) => xml(200, x),
                    Err((c, t)) => fault(c, t),
                },
                (true, "GetSearchCapabilities") => xml(200, soap_response(service, &a.name, &[("SearchCaps", crate::upnp_search::CAPABILITIES.to_string())])),
                (true, "GetSortCapabilities") => xml(200, soap_response(service, &a.name, &[("SortCaps", String::new())])),
                (true, "GetSystemUpdateID") => xml(200, soap_response(service, &a.name, &[("Id", cat.update_id().to_string())])),
                (false, "GetProtocolInfo") => xml(200, soap_response(service, &a.name, &[("Source", SOURCE_PROTOCOLS.to_string()), ("Sink", String::new())])),
                (false, "GetCurrentConnectionIDs") => xml(200, soap_response(service, &a.name, &[("ConnectionIDs", "0".to_string())])),
                (false, "GetCurrentConnectionInfo") => xml(200, soap_response(service, &a.name, &[("RcsID", "-1".into()), ("AVTransportID", "-1".into()), ("ProtocolInfo", String::new()), ("PeerConnectionManager", String::new()), ("PeerConnectionID", "-1".into()), ("Direction", "Output".into()), ("Status", "OK".into())])),
                _ => fault(401, "Invalid action"),
            })
        }
        (_, "/upnp/device.xml" | "/upnp/cd.xml" | "/upnp/cm.xml" | "/upnp/cd/control" | "/upnp/cm/control" | "/upnp/cd/event" | "/upnp/cm/event") => Some(plain(405)),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------------ a catalog of a library

/// One file of a library as the catalog sees it.
#[derive(Clone, Debug)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub video: bool,
    pub mime: String,
    pub size: u64,
    pub seconds: u32,
}

/// A flat catalog: the root holds two folders, Music and Videos; ids are positions (`a12`, `v3`), so a player can only ask for
/// what the list holds. `url_for` makes the address a player fetches a file from.
pub struct ListCatalog {
    pub items: Vec<Media>,
    pub update: u32,
    pub url_for: Box<dyn Fn(&str) -> String + Send + Sync>,
}

impl ListCatalog {
    fn idx(&self, video: bool) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, m)| m.video == video).map(|(i, _)| i).collect()
    }
    fn item(&self, i: usize) -> Entry {
        let m = &self.items[i];
        let id = format!("{}{i}", if m.video { 'v' } else { 'a' });
        Entry::Item { id: id.clone(), parent: if m.video { "2" } else { "1" }.into(), title: m.title.clone(), artist: m.artist.clone(), album: m.album.clone(), class: if m.video { "object.item.videoItem" } else { "object.item.audioItem.musicTrack" }.into(), mime: m.mime.clone(), size: m.size, seconds: m.seconds, url: (self.url_for)(&id) }
    }
}

impl Catalog for ListCatalog {
    fn get(&self, id: &str) -> Option<Entry> {
        match id {
            "0" => Some(Entry::Container { id: "0".into(), parent: "-1".into(), title: "IYAKKI".into(), children: 2 }),
            "1" => Some(Entry::Container { id: "1".into(), parent: "0".into(), title: "Music".into(), children: self.idx(false).len() as u32 }),
            "2" => Some(Entry::Container { id: "2".into(), parent: "0".into(), title: "Videos".into(), children: self.idx(true).len() as u32 }),
            _ => {
                let (video, n) = match id.split_at_checked(1)? {
                    ("a", n) => (false, n),
                    ("v", n) => (true, n),
                    _ => return None,
                };
                let i: usize = n.parse().ok()?;
                (self.items.get(i)?.video == video).then(|| self.item(i))
            }
        }
    }
    fn children(&self, id: &str, start: u32, count: u32) -> Option<(Vec<Entry>, u32)> {
        let list: Vec<Entry> = match id {
            "0" => vec![self.get("1")?, self.get("2")?],
            "1" => self.idx(false).into_iter().map(|i| self.item(i)).collect(),
            "2" => self.idx(true).into_iter().map(|i| self.item(i)).collect(),
            _ => return None,
        };
        let total = list.len() as u32;
        Some((list.into_iter().skip(start as usize).take(count as usize).collect(), total))
    }
    fn update_id(&self) -> u32 {
        self.update
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Identity {
        Identity { friendly_name: "Ada's <media> & more".into(), uuid: "6f1c1c0a-1111-4222-8333-444455556666".into() }
    }

    fn cat() -> ListCatalog {
        let m = |t: &str, v: bool| Media { title: t.into(), artist: "Ar & Tist".into(), album: "Al".into(), video: v, mime: if v { "video/mp4" } else { "audio/mpeg" }.into(), size: 1234, seconds: 3725 };
        ListCatalog { items: vec![m("Song one", false), m("Film", true), m("Song <two>", false)], update: 7, url_for: Box::new(|id| format!("http://192.168.1.5:8200/upnp/media/{id}")) }
    }

    fn msearch(st: &str) -> Vec<u8> {
        format!("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: {st}\r\n\r\n").into_bytes()
    }

    #[test]
    fn a_search_is_recognised_and_junk_and_other_peoples_announcements_are_ignored() {
        assert_eq!(parse_search(&msearch("ssdp:all")), Some(Search { st: "ssdp:all".into(), mx: 3 }));
        assert_eq!(parse_search(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nMX: 99\r\nST: upnp:rootdevice\r\n\r\n").unwrap().mx, 5, "MX is limited to 5");
        assert_eq!(parse_search(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nMX: 0\r\nST: x\r\n\r\n").unwrap().mx, 1);
        assert!(parse_search(b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\n\r\n").is_none());
        assert!(parse_search(b"M-SEARCH * HTTP/1.1\r\nST: ssdp:all\r\n\r\n").is_none(), "no MAN, no answer");
        assert!(parse_search(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\n\r\n").is_none(), "no ST");
        assert!(parse_search(&[0xff, 0xfe, 0x00]).is_none());
        assert!(parse_search(&vec![b'a'; 5000]).is_none());
        assert!(parse_search(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nST: a\x01b\r\n\r\n").is_none());
    }

    #[test]
    fn a_search_gets_one_small_reply_per_matching_target_and_only_from_the_local_network() {
        let loc = "http://192.168.1.5:8200/upnp/device.xml";
        let all = search_replies(&Search { st: "ssdp:all".into(), mx: 2 }, &id().uuid, loc, "IYAKKI/1 UPnP/1.0", "Thu, 12 Nov 2026 10:00:00 GMT");
        assert_eq!(all.len(), 1, "ssdp:all is one answer, the root device");
        assert!(String::from_utf8_lossy(&all[0]).contains("ST: upnp:rootdevice"));
        assert!(search_replies(&Search { st: CM_TYPE.into(), mx: 1 }, &id().uuid, loc, "s", "d").is_empty(), "the connection manager is not answered on its own");
        assert_eq!(search_replies(&Search { st: CD_TYPE.into(), mx: 1 }, &id().uuid, loc, "s", "d").len(), 1);
        let one = search_replies(&Search { st: DEVICE_TYPE.into(), mx: 2 }, &id().uuid, loc, "IYAKKI/1 UPnP/1.0", "d");
        assert_eq!(one.len(), 1);
        let t = String::from_utf8(one[0].clone()).unwrap();
        assert!(t.starts_with("HTTP/1.1 200 OK\r\n") && t.contains(&format!("LOCATION: {loc}\r\n")) && t.contains(&format!("ST: {DEVICE_TYPE}\r\n")) && t.contains("USN: uuid:6f1c1c0a-1111-4222-8333-444455556666::urn:schemas-upnp-org:device:MediaServer:1\r\n"), "{t}");
        assert!(one[0].len() < 500, "small replies: a search is not an amplifier");
        assert!(search_replies(&Search { st: "urn:schemas-upnp-org:device:Printer:1".into(), mx: 1 }, &id().uuid, loc, "s", "d").is_empty());
        let root = search_replies(&Search { st: "UPNP:ROOTDEVICE".into(), mx: 1 }, &id().uuid, loc, "s", "d");
        assert_eq!(root.len(), 1, "the match ignores case");
        assert!(may_answer("192.168.1.9".parse().unwrap()) && may_answer("10.1.2.3".parse().unwrap()) && may_answer("169.254.1.1".parse().unwrap()) && may_answer("fe80::1".parse().unwrap()) && may_answer("fd00::5".parse().unwrap()));
        assert!(!may_answer("8.8.8.8".parse().unwrap()) && !may_answer("2001:db8::1".parse().unwrap()) && !may_answer("127.0.0.1".parse().unwrap()));
        let alive = notifications(true, &id().uuid, loc, "s");
        let bye = notifications(false, &id().uuid, loc, "s");
        assert_eq!((alive.len(), bye.len()), (5, 5));
        assert!(String::from_utf8_lossy(&alive[0]).contains("NTS: ssdp:alive") && String::from_utf8_lossy(&bye[0]).contains("NTS: ssdp:byebye") && !String::from_utf8_lossy(&bye[0]).contains("LOCATION"));
    }

    #[test]
    fn one_source_cannot_make_the_server_chatty() {
        let mut t = Throttle::new();
        let a: IpAddr = "192.168.1.9".parse().unwrap();
        let b: IpAddr = "192.168.1.10".parse().unwrap();
        let ok: Vec<bool> = (0..8).map(|_| t.allow(a, 100)).collect();
        assert_eq!(ok.iter().filter(|x| **x).count(), MAX_PER_SECOND as usize);
        assert!(t.allow(b, 100), "another source is unaffected");
        assert!(t.allow(a, 101), "the next second starts again");
        // a flood of addresses cannot grow the table without bound
        let mut t = Throttle::new();
        for i in 0..2000u32 {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            t.allow(ip, 5);
        }
        assert!(t.seen.len() <= MAX_SOURCES);
    }

    #[test]
    fn the_documents_name_the_server_and_escape_what_a_person_typed() {
        let d = device_xml(&id());
        assert!(d.contains("<friendlyName>Ada&apos;s &lt;media&gt; &amp; more</friendlyName>") && d.contains("<UDN>uuid:6f1c1c0a-1111-4222-8333-444455556666</UDN>") && d.contains(DEVICE_TYPE));
        assert!(d.contains("/upnp/cd/control") && d.contains("/upnp/cm.xml"));
        for x in [content_directory_scpd(), connection_manager_scpd(), d] {
            assert_eq!(x.matches('<').count(), x.matches('>').count());
            assert!(x.starts_with("<?xml"));
        }
        assert!(content_directory_scpd().contains("<name>Browse</name>") && content_directory_scpd().contains("<name>Search</name>"), "browsing and searching");
    }

    fn soap(action: &str, args: &str) -> Vec<u8> {
        format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action} xmlns:u=\"{CD_TYPE}\">{args}</u:{action}></s:Body></s:Envelope>").into_bytes()
    }

    fn post(action: &str, args: &str, c: &ListCatalog) -> Reply {
        handle("POST", "/upnp/cd/control", Some(&format!("\"{CD_TYPE}#{action}\"")), &soap(action, args), &id(), c).unwrap()
    }

    #[test]
    fn a_player_browses_the_root_a_folder_and_one_item_and_gets_escaped_didl() {
        let c = cat();
        let r = post("Browse", "<ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria>", &c);
        assert_eq!(r.status, 200);
        let t = String::from_utf8(r.body).unwrap();
        assert!(t.contains("<NumberReturned>2</NumberReturned>") && t.contains("<TotalMatches>2</TotalMatches>") && t.contains("<UpdateID>7</UpdateID>"), "{t}");
        assert!(t.contains("&lt;container id=&quot;1&quot;") && t.contains("Music") && t.contains("Videos"), "the DIDL is a text value, escaped once: {t}");
        let r = post("Browse", "<ObjectID>1</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><StartingIndex>1</StartingIndex><RequestedCount>5</RequestedCount>", &c);
        let t = String::from_utf8(r.body).unwrap();
        assert!(t.contains("<NumberReturned>1</NumberReturned>") && t.contains("<TotalMatches>2</TotalMatches>"), "paging: {t}");
        assert!(t.contains("Song &amp;lt;two&amp;gt;") && t.contains("Ar &amp;amp; Tist"), "titles are escaped inside the DIDL and again outside: {t}");
        assert!(t.contains("protocolInfo=&quot;http-get:*:audio/mpeg:*&quot;") && t.contains("duration=&quot;1:02:05&quot;") && t.contains("http://192.168.1.5:8200/upnp/media/a2"), "{t}");
        let r = post("Browse", "<ObjectID>v1</ObjectID><BrowseFlag>BrowseMetadata</BrowseFlag>", &c);
        assert!(String::from_utf8(r.body).unwrap().contains("object.item.videoItem"));
        // the answer is never more than a page
        let big = ListCatalog { items: (0..500).map(|i| Media { title: format!("t{i}"), artist: String::new(), album: String::new(), video: false, mime: "audio/mpeg".into(), size: 1, seconds: 0 }).collect(), update: 1, url_for: Box::new(|i| i.to_string()) };
        let t = String::from_utf8(post("Browse", "<ObjectID>1</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><RequestedCount>100000</RequestedCount>", &big).body).unwrap();
        assert!(t.contains(&format!("<NumberReturned>{MAX_PAGE}</NumberReturned>")) && t.contains("<TotalMatches>500</TotalMatches>"));
    }

    #[test]
    fn unknown_things_and_wrong_calls_get_the_standard_errors_and_nothing_else_happens() {
        let c = cat();
        let f = |r: Reply| (r.status, String::from_utf8(r.body).unwrap());
        let (s, t) = f(post("Browse", "<ObjectID>../../etc</ObjectID><BrowseFlag>BrowseMetadata</BrowseFlag>", &c));
        assert!(s == 500 && t.contains("<errorCode>701</errorCode>"), "a player can name only ids from the list: {t}");
        let (s, t) = f(post("Browse", "<ObjectID>a1</ObjectID><BrowseFlag>BrowseMetadata</BrowseFlag>", &c));
        assert!(s == 500 && t.contains("701"), "a1 is a video, not music");
        let (_, t) = f(post("Browse", "<ObjectID>0</ObjectID><BrowseFlag>Nonsense</BrowseFlag>", &c));
        assert!(t.contains("<errorCode>402</errorCode>"));
        let (s, t) = f(post("Search", "<ContainerID>0</ContainerID><SearchCriteria>dc:title like \"x\"</SearchCriteria>", &c));
        assert!(s == 500 && t.contains("708"), "bad criteria are an error");
        let (s, t) = f(post("Search", "<ContainerID>nope</ContainerID><SearchCriteria>*</SearchCriteria>", &c));
        assert!(s == 500 && t.contains("710"), "no such container");
        let (s, _) = f(post("DestroyObject", "<ObjectID>a0</ObjectID>", &c));
        assert_eq!(s, 500, "nothing can be changed");
        // the header must agree with the body
        let r = handle("POST", "/upnp/cd/control", Some("\"urn:evil#Browse\""), &soap("Browse", ""), &id(), &c).unwrap();
        assert_eq!(r.status, 500);
        assert_eq!(handle("POST", "/upnp/cd/control", None, b"<junk", &id(), &c).unwrap().status, 400);
        assert_eq!(handle("PUT", "/upnp/cd/control", None, b"", &id(), &c).unwrap().status, 405);
        assert_eq!(handle("SUBSCRIBE", "/upnp/cd/event", None, b"", &id(), &c).unwrap().status, 501);
        assert!(handle("GET", "/etc/passwd", None, b"", &id(), &c).is_none(), "other addresses are the caller's");
        let (_, t) = f(handle("POST", "/upnp/cd/control", None, &soap("GetSystemUpdateID", ""), &id(), &c).unwrap());
        assert!(t.contains("<Id>7</Id>"));
        let cm = handle("POST", "/upnp/cm/control", None, &soap("GetProtocolInfo", ""), &id(), &c).unwrap();
        assert!(String::from_utf8(cm.body).unwrap().contains("audio/mpeg"));
    }

    #[test]
    fn hostile_requests_do_not_panic_or_run_away() {
        let c = cat();
        for body in ["", "<Body>", "<s:Body><", "<s:Body><u:Browse", "<s:Body><u:Browse xmlns:u='x'><ObjectID", "<s:Body><u:Browse><A>1</A><B></B><C/><D>", &"<s:Body><u:Browse>".repeat(2000), &format!("<s:Body><u:Browse>{}", "<a>x</a>".repeat(5000))] {
            let _ = handle("POST", "/upnp/cd/control", None, body.as_bytes(), &id(), &c);
        }
        let mut x: u64 = 12345;
        for _ in 0..500 {
            let mut b = Vec::new();
            for _ in 0..80 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                b.push(b"<>/ =\"&;:BodyBrowse"[(x >> 33) as usize % 19]);
            }
            let _ = handle("POST", "/upnp/cd/control", None, &b, &id(), &c);
            let _ = parse_search(&b);
        }
        assert_eq!(esc("a\u{1}b<"), "ab&lt;", "control characters are dropped from what goes into a document");
    }
}
