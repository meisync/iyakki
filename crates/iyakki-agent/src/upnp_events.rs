//! Event subscriptions of the media server (F-MEDIA-02): a player sends `SUBSCRIBE` with a call-back address and is told, with a
//! `NOTIFY` message, when the content changes (`SystemUpdateID`), so it refreshes its lists without polling.
//!
//! The table is small and strict: at most 32 subscriptions, 4 for one player; the call-back must be an `http` address of the
//! player's own machine (the same address the request came from, a numeric host, no user part), so a subscription cannot make the
//! server send messages to a third machine; a subscription lasts 60 to 1800 seconds and ends unless renewed. Messages are sent one
//! at a time with short time-outs and are never repeated; an unreachable player is dropped after three failures.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::upnp::{esc, CD_TYPE, CM_TYPE, SOURCE_PROTOCOLS};

pub const MAX_SUBSCRIPTIONS: usize = 32;
pub const MAX_PER_PLAYER: usize = 4;
pub const MIN_TIMEOUT: u32 = 60;
pub const MAX_TIMEOUT: u32 = 1800;
const MAX_FAILURES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    ContentDirectory,
    ConnectionManager,
}

impl Service {
    /// The service for an event address of the device description.
    pub fn from_path(path: &str) -> Option<Service> {
        match path {
            "/upnp/cd/event" => Some(Service::ContentDirectory),
            "/upnp/cm/event" => Some(Service::ConnectionManager),
            _ => None,
        }
    }
    /// The service in plain words.
    pub fn label(self) -> &'static str {
        match self {
            Service::ContentDirectory => "the file list",
            Service::ConnectionManager => "the connection",
        }
    }
    fn urn(self) -> &'static str {
        match self {
            Service::ContentDirectory => CD_TYPE,
            Service::ConnectionManager => CM_TYPE,
        }
    }
}

#[derive(Clone, Debug)]
struct Sub {
    sid: String,
    service: Service,
    callback: SocketAddr,
    path: String,
    peer: IpAddr,
    expires: i64,
    seq: u32,
    failures: u8,
}

/// A message to send: where, and the bytes of the whole HTTP request.
/// One subscription as the person sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubInfo {
    pub service: &'static str,
    pub peer: IpAddr,
    pub left_secs: i64,
}

#[derive(Clone, Debug)]
pub struct Notify {
    pub sid: String,
    pub to: SocketAddr,
    pub bytes: Vec<u8>,
}

/// What `subscribe` and `unsubscribe` answer: the status line, and for a new or renewed subscription its `SID` and `TIMEOUT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub sid: Option<String>,
    pub timeout: Option<u32>,
}

fn fail(status: u16) -> Answer {
    Answer { status, sid: None, timeout: None }
}

#[derive(Default)]
pub struct Subscriptions {
    subs: Vec<Sub>,
    counter: u64,
}

/// `<http://192.168.1.5:5000/path>` to the address and path; only a numeric host that is `peer` (no names, no user part) is accepted.
fn callback_of(header: &str, peer: IpAddr) -> Option<(SocketAddr, String)> {
    let h = header.trim();
    let inner = h.strip_prefix('<')?.strip_suffix('>')?;
    if inner.contains('<') || inner.contains('>') {
        return None;
    }
    let rest = inner.strip_prefix("http://")?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if hostport.contains('@') || path.len() > 256 || !path.bytes().all(|b| (0x21..0x7f).contains(&b)) {
        return None;
    }
    let addr: SocketAddr = hostport.parse().ok()?;
    (addr.ip() == peer && addr.port() != 0).then(|| (addr, path.to_string()))
}

fn timeout_of(header: Option<&str>) -> u32 {
    let n = header.and_then(|h| h.trim().strip_prefix("Second-")).and_then(|v| v.trim().parse::<u32>().ok());
    n.unwrap_or(MAX_TIMEOUT).clamp(MIN_TIMEOUT, MAX_TIMEOUT)
}

impl Subscriptions {
    pub fn new() -> Subscriptions {
        Subscriptions::default()
    }

    pub fn len(&self) -> usize {
        self.subs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Who is subscribed now, for the Settings page and the chat tool: the service, the player's address and the seconds left. No
    /// session id and no call-back path is given out (they could renew or end another player's subscription).
    pub fn listing(&self, now: i64) -> Vec<SubInfo> {
        self.subs.iter().filter(|s| s.expires > now).map(|s| SubInfo { service: s.service.label(), peer: s.peer, left_secs: s.expires - now }).collect()
    }

    /// Forget the subscriptions that ran out.
    pub fn expire(&mut self, now: i64) {
        self.subs.retain(|s| s.expires > now);
    }

    fn new_sid(&mut self, now: i64) -> String {
        self.counter += 1;
        // the id only has to be unique; it is not a secret that grants anything (it renews or ends one subscription)
        format!("uuid:{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", now as u32, self.counter as u16, (self.counter >> 16) as u16 & 0xfff, (self.counter >> 8) as u16 & 0xfff, self.counter.wrapping_mul(0x9e37_79b9_7f4a_7c15) & 0xffff_ffff_ffff)
    }

    /// A `SUBSCRIBE` request: a new subscription (`CALLBACK` and `NT: upnp:event`) or a renewal (`SID`). Status codes are those of the
    /// UPnP specification: 400 for a request with both or neither, 412 for a precondition that failed (bad call-back, unknown `SID`),
    /// 503 when the table is full.
    pub fn subscribe(&mut self, service: Service, callback: Option<&str>, nt: Option<&str>, sid: Option<&str>, timeout: Option<&str>, peer: IpAddr, now: i64) -> Answer {
        self.expire(now);
        let timeout = timeout_of(timeout);
        match (sid, callback, nt) {
            (Some(sid), None, None) => match self.subs.iter_mut().find(|s| s.sid == sid.trim() && s.service == service && s.peer == peer) {
                Some(s) => {
                    s.expires = now + i64::from(timeout);
                    Answer { status: 200, sid: Some(s.sid.clone()), timeout: Some(timeout) }
                }
                None => fail(412),
            },
            (None, Some(cb), Some(nt)) => {
                if nt.trim() != "upnp:event" {
                    return fail(412);
                }
                let Some((callback, path)) = callback_of(cb, peer) else { return fail(412) };
                if self.subs.len() >= MAX_SUBSCRIPTIONS || self.subs.iter().filter(|s| s.peer == peer).count() >= MAX_PER_PLAYER {
                    return fail(503);
                }
                let sid = self.new_sid(now);
                self.subs.push(Sub { sid: sid.clone(), service, callback, path, peer, expires: now + i64::from(timeout), seq: 0, failures: 0 });
                Answer { status: 200, sid: Some(sid), timeout: Some(timeout) }
            }
            _ => fail(if sid.is_some() || callback.is_some() || nt.is_some() { 400 } else { 412 }),
        }
    }

    /// An `UNSUBSCRIBE` request.
    pub fn unsubscribe(&mut self, service: Service, sid: Option<&str>, peer: IpAddr) -> Answer {
        let Some(sid) = sid.map(str::trim) else { return fail(412) };
        let before = self.subs.len();
        self.subs.retain(|s| !(s.sid == sid && s.service == service && s.peer == peer));
        fail(if self.subs.len() < before { 200 } else { 412 })
    }

    /// The messages to send for a service whose variables are `vars` (name and value), one per subscriber; sequence numbers move on.
    /// The first message of a new subscription (`SEQ` 0) carries all variables; call this right after answering the request.
    pub fn messages(&mut self, service: Service, vars: &[(&str, String)], only: Option<&str>) -> Vec<Notify> {
        let body = property_set(vars);
        let mut out = Vec::new();
        for s in self.subs.iter_mut().filter(|s| s.service == service && only.is_none_or(|o| o == s.sid)) {
            let head = format!(
                "NOTIFY {} HTTP/1.1\r\nHOST: {}\r\nCONTENT-TYPE: text/xml; charset=\"utf-8\"\r\nNT: upnp:event\r\nNTS: upnp:propchange\r\nSID: {}\r\nSEQ: {}\r\nCONTENT-LENGTH: {}\r\nConnection: close\r\n\r\n",
                s.path,
                s.callback,
                s.sid,
                s.seq,
                body.len()
            );
            s.seq = s.seq.wrapping_add(1);
            let mut bytes = head.into_bytes();
            bytes.extend_from_slice(body.as_bytes());
            out.push(Notify { sid: s.sid.clone(), to: s.callback, bytes });
        }
        out
    }

    /// Record how a delivery went; a player that failed three times in a row is dropped.
    pub fn delivered(&mut self, sid: &str, ok: bool) {
        if let Some(s) = self.subs.iter_mut().find(|s| s.sid == sid) {
            s.failures = if ok { 0 } else { s.failures + 1 };
        }
        self.subs.retain(|s| s.failures < MAX_FAILURES);
    }
}

/// The event message body for the given variables.
pub fn property_set(vars: &[(&str, String)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?>\n<e:propertyset xmlns:e=\"urn:schemas-upnp-org:event-1-0\">");
    for (k, v) in vars {
        s.push_str(&format!("<e:property><{k}>{}</{k}></e:property>", esc(v)));
    }
    s.push_str("</e:propertyset>");
    s
}

/// The variables of a service that are sent as a whole in the first message.
pub fn all_vars(service: Service, update_id: u32) -> Vec<(&'static str, String)> {
    match service {
        Service::ContentDirectory => vec![("SystemUpdateID", update_id.to_string()), ("ContainerUpdateIDs", String::new())],
        Service::ConnectionManager => vec![("SourceProtocolInfo", SOURCE_PROTOCOLS.to_string()), ("SinkProtocolInfo", String::new()), ("CurrentConnectionIDs", "0".to_string())],
    }
}

/// The urn of the service, for logs and tests.
pub fn urn(service: Service) -> &'static str {
    service.urn()
}

/// Send one message: connect, write, read the status line, close. `true` when the player answered 200. Never waits more than a few
/// seconds and never follows anything.
pub fn deliver(n: &Notify) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&n.to, Duration::from_secs(2)) else { return false };
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    if s.write_all(&n.bytes).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let got = s.read(&mut buf).unwrap_or(0);
    std::str::from_utf8(&buf[..got]).is_ok_and(|t| t.starts_with("HTTP/1.1 200") || t.starts_with("HTTP/1.0 200"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn sub(t: &mut Subscriptions, cb: &str, peer: &str, now: i64) -> Answer {
        t.subscribe(Service::ContentDirectory, Some(cb), Some("upnp:event"), None, Some("Second-300"), ip(peer), now)
    }

    #[test]
    fn the_listing_names_the_player_and_the_time_left_and_never_the_session_id() {
        let mut t = Subscriptions::new();
        assert!(t.listing(1000).is_empty());
        assert_eq!(sub(&mut t, "<http://192.0.2.5:5000/e>", "192.0.2.5", 1000).status, 200);
        let l = t.listing(1100);
        assert_eq!(l, vec![SubInfo { service: "the file list", peer: ip("192.0.2.5"), left_secs: 200 }]);
        assert!(!format!("{l:?}").contains("uuid:"), "no session id is given out");
        assert!(t.listing(1400).is_empty(), "a subscription that ran out is not listed");
        assert!(crate::upnp_serve::subscribers_text(&[]).contains("No player"));
        assert!(crate::upnp_serve::subscribers_text(&l).contains("1 subscription(s) from 1 player(s): 192.0.2.5"));
    }

    #[test]
    fn a_subscription_is_made_renewed_ended_and_runs_out() {
        let mut t = Subscriptions::new();
        let a = sub(&mut t, "<http://192.168.1.9:4000/ev>", "192.168.1.9", 1000);
        assert_eq!((a.status, a.timeout), (200, Some(300)));
        let sid = a.sid.unwrap();
        assert!(sid.starts_with("uuid:"));
        let r = t.subscribe(Service::ContentDirectory, None, None, Some(&sid), Some("Second-5000"), ip("192.168.1.9"), 1100);
        assert_eq!((r.status, r.timeout), (200, Some(MAX_TIMEOUT)), "renewed, and the time is capped");
        assert_eq!(t.subscribe(Service::ContentDirectory, None, None, Some(&sid), None, ip("192.168.1.77"), 1100).status, 412, "another machine cannot renew it");
        assert_eq!(t.subscribe(Service::ConnectionManager, None, None, Some(&sid), None, ip("192.168.1.9"), 1100).status, 412, "nor on another service");
        assert_eq!(t.unsubscribe(Service::ContentDirectory, Some(&sid), ip("192.168.1.77")).status, 412);
        assert_eq!(t.unsubscribe(Service::ContentDirectory, Some(&sid), ip("192.168.1.9")).status, 200);
        assert_eq!(t.unsubscribe(Service::ContentDirectory, Some(&sid), ip("192.168.1.9")).status, 412, "twice");
        let a = t.subscribe(Service::ContentDirectory, Some("<http://192.168.1.9:4000/ev>"), Some("upnp:event"), None, Some("Second-5"), ip("192.168.1.9"), 1000);
        assert_eq!(a.timeout, Some(MIN_TIMEOUT));
        t.expire(1000 + 59);
        assert_eq!(t.len(), 1);
        t.expire(1000 + 61);
        assert_eq!(t.len(), 0, "the shortest time is 60 seconds");
    }

    #[test]
    fn the_call_back_must_be_the_players_own_numeric_http_address() {
        let mut t = Subscriptions::new();
        for (cb, peer) in [
            ("<http://192.168.1.10:4000/ev>", "192.168.1.9"),
            ("<http://example.org:4000/ev>", "192.168.1.9"),
            ("<https://192.168.1.9:4000/ev>", "192.168.1.9"),
            ("<http://user@192.168.1.9:4000/ev>", "192.168.1.9"),
            ("<http://192.168.1.9:0/ev>", "192.168.1.9"),
            ("<http://192.168.1.9/ev>", "192.168.1.9"),
            ("http://192.168.1.9:4000/ev", "192.168.1.9"),
            ("<http://192.168.1.9:4000/a b>", "192.168.1.9"),
            ("<http://192.168.1.9:4000/ev><http://10.0.0.1/x>", "192.168.1.9"),
        ] {
            assert_eq!(sub(&mut t, cb, peer, 0).status, 412, "{cb} from {peer}");
        }
        assert_eq!(t.subscribe(Service::ContentDirectory, Some("<http://192.168.1.9:4000/ev>"), Some("upnp:other"), None, None, ip("192.168.1.9"), 0).status, 412);
        assert_eq!(t.subscribe(Service::ContentDirectory, Some("<http://192.168.1.9:4000/ev>"), Some("upnp:event"), Some("uuid:x"), None, ip("192.168.1.9"), 0).status, 400, "both");
        assert_eq!(t.subscribe(Service::ContentDirectory, None, None, None, None, ip("192.168.1.9"), 0).status, 412, "neither");
        assert!(t.is_empty());
        assert_eq!(sub(&mut t, "<http://[fe80::1]:4000/ev>", "fe80::1", 0).status, 200, "an IPv6 player works too");
    }

    #[test]
    fn the_table_is_limited_overall_and_per_player() {
        let mut t = Subscriptions::new();
        for _ in 0..MAX_PER_PLAYER {
            assert_eq!(sub(&mut t, "<http://10.0.0.2:1/e>", "10.0.0.2", 0).status, 200);
        }
        assert_eq!(sub(&mut t, "<http://10.0.0.2:1/e>", "10.0.0.2", 0).status, 503);
        for i in 0..(MAX_SUBSCRIPTIONS - MAX_PER_PLAYER) {
            let p = format!("10.0.1.{i}");
            assert_eq!(sub(&mut t, &format!("<http://{p}:1/e>"), &p, 0).status, 200);
        }
        assert_eq!(sub(&mut t, "<http://10.0.9.9:1/e>", "10.0.9.9", 0).status, 503, "the whole table is full");
        let mut sids: Vec<_> = t.subs.iter().map(|s| s.sid.clone()).collect();
        sids.sort();
        sids.dedup();
        assert_eq!(sids.len(), MAX_SUBSCRIPTIONS, "every subscription has its own id");
    }

    #[test]
    fn messages_carry_the_variables_a_growing_sequence_and_the_id_and_a_dead_player_is_dropped() {
        let mut t = Subscriptions::new();
        let sid = sub(&mut t, "<http://192.168.1.9:4000/ev>", "192.168.1.9", 0).sid.unwrap();
        let other = t.subscribe(Service::ConnectionManager, Some("<http://192.168.1.9:4000/cm>"), Some("upnp:event"), None, None, ip("192.168.1.9"), 0).sid.unwrap();
        let m = t.messages(Service::ContentDirectory, &all_vars(Service::ContentDirectory, 7), None);
        assert_eq!(m.len(), 1, "only the subscribers of that service");
        let text = String::from_utf8(m[0].bytes.clone()).unwrap();
        assert!(text.starts_with("NOTIFY /ev HTTP/1.1\r\n") && text.contains(&format!("SID: {sid}\r\n")) && text.contains("SEQ: 0\r\n") && text.contains("NTS: upnp:propchange"), "{text}");
        assert!(text.contains("<SystemUpdateID>7</SystemUpdateID>") && text.contains("<ContainerUpdateIDs></ContainerUpdateIDs>"), "{text}");
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        assert!(text.contains(&format!("CONTENT-LENGTH: {}\r\n", body.len())));
        let m2 = t.messages(Service::ContentDirectory, &[("SystemUpdateID", "8".into())], None);
        assert!(String::from_utf8_lossy(&m2[0].bytes).contains("SEQ: 1\r\n"));
        assert_eq!(t.messages(Service::ConnectionManager, &all_vars(Service::ConnectionManager, 0), Some(&other)).len(), 1);
        assert!(t.messages(Service::ConnectionManager, &[], Some("uuid:nobody")).is_empty());
        for _ in 0..MAX_FAILURES - 1 {
            t.delivered(&sid, false);
        }
        assert_eq!(t.len(), 2);
        t.delivered(&sid, true);
        for _ in 0..MAX_FAILURES {
            t.delivered(&sid, false);
        }
        assert_eq!(t.len(), 1, "three failures in a row end it");
        assert!(property_set(&[("A", "<&>".into())]).contains("&lt;&amp;&gt;"));
    }

    #[test]
    fn a_message_reaches_a_player_that_listens_and_a_silent_one_is_reported_as_failed() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut got = vec![0u8; 2048];
            let n = s.read(&mut got).unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
            String::from_utf8_lossy(&got[..n]).to_string()
        });
        let mut subs = Subscriptions::new();
        let cb = format!("<http://{addr}/ev>");
        let sid = sub(&mut subs, &cb, "127.0.0.1", 0).sid.unwrap();
        let m = subs.messages(Service::ContentDirectory, &all_vars(Service::ContentDirectory, 1), Some(&sid));
        assert!(deliver(&m[0]));
        assert!(t.join().unwrap().contains("SEQ: 0"));
        // nobody listens any more
        assert!(!deliver(&m[0]));
        assert_eq!(urn(Service::ContentDirectory), CD_TYPE);
        assert_eq!(Service::from_path("/upnp/cd/event"), Some(Service::ContentDirectory));
        assert_eq!(Service::from_path("/x"), None);
    }
}
