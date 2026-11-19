//! The serving half of the UPnP/DLNA media server (F-MEDIA-02): an SSDP responder on a UDP socket, the description and SOAP pages over
//! HTTP, and the media files with `Range` requests. The protocol is in [`crate::upnp`]; this file owns the sockets and the files.
//!
//! Nothing here starts by itself: a person's server runs only after [`start_for`] (the chat tool asks first), shows only audio and
//! video files inside that person's own shared folders (hidden files, links that leave the folder and the deny list left out, the same
//! rules as the shared-folder server), answers only private and link-local addresses, needs a numeric `Host` (a web page cannot reach
//! it through a rebound name), and stops at sign-out. The sockets can be bound anywhere, so the tests use the loopback address.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::sharedsite::{parse_head, Head};
use crate::upnp::{self, Identity, ListCatalog, Media, Throttle};
use crate::upnp_events::{self, Notify, Service, Subscriptions};

pub const SSDP_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const MAX_CONNECTIONS: usize = 16;
const MAX_BODY: usize = 16 * 1024;
const MAX_ITEMS: usize = 5000;
const MAX_DEPTH: usize = 8;
const REFRESH: Duration = Duration::from_secs(30);
const SERVER: &str = "Linux/1.0 UPnP/1.0 IYAKKI/1";

/// What a server needs.
#[derive(Clone)]
pub struct ServeCfg {
    pub name: String,
    pub uuid: String,
    pub http_bind: SocketAddr,
    pub ssdp_bind: SocketAddr,
    /// Join the SSDP multicast group and announce (real use); the tests talk to the socket directly.
    pub multicast: bool,
    pub folders: Vec<PathBuf>,
    pub deny: Vec<PathBuf>,
    /// Answer loopback too (tests only).
    pub allow_loopback: bool,
    /// The whole request (head and body) must arrive within this time, or the connection is dropped and its place is free again.
    pub head_deadline: Duration,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// The converter for files a player cannot play (the person's own ffmpeg); `None`: files are served as they are.
    pub transcoder: Option<Arc<dyn crate::upnp_transcode::Transcoder>>,
}

// ---------------------------------------------------------------------------------------------------------------- the library

/// The media files of the shared folders.
#[derive(Clone, Default)]
pub struct Library {
    pub items: Vec<Media>,
    pub paths: Vec<PathBuf>,
    pub truncated: bool,
}

/// MIME type and whether it is video, by extension (`None`: not a file this server offers).
pub fn kind_of(name: &str) -> Option<(&'static str, bool)> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp3" => ("audio/mpeg", false),
        "flac" => ("audio/flac", false),
        "ogg" | "opus" => ("audio/ogg", false),
        "wav" => ("audio/wav", false),
        "m4a" => ("audio/mp4", false),
        "mp4" => ("video/mp4", true),
        "mkv" => ("video/x-matroska", true),
        "webm" => ("video/webm", true),
        "avi" => ("video/x-msvideo", true),
        _ => return None,
    })
}

/// Look through the folders. A folder that does not exist is skipped.
pub fn scan(folders: &[PathBuf], deny: &[PathBuf]) -> Library {
    let mut lib = Library::default();
    for root in folders {
        let Ok(root) = std::fs::canonicalize(root) else { continue };
        let mut todo = vec![(root.clone(), 0usize)];
        let mut seen = std::collections::HashSet::new();
        while let Some((dir, depth)) = todo.pop() {
            if !seen.insert(dir.clone()) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut ents: Vec<_> = rd.flatten().collect();
            ents.sort_by_key(|e| e.file_name());
            for e in ents {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(real) = std::fs::canonicalize(e.path()) else { continue };
                let inside = real.strip_prefix(&root).ok();
                if inside.is_none_or(|r| r.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))) || deny.iter().any(|d| real.starts_with(d)) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&real) else { continue };
                if md.is_dir() {
                    if depth < MAX_DEPTH {
                        todo.push((real, depth + 1));
                    }
                } else if md.is_file() {
                    let Some((mime, video)) = kind_of(&name) else { continue };
                    if lib.items.len() >= MAX_ITEMS {
                        lib.truncated = true;
                        return lib;
                    }
                    let mut title = name.rsplit_once('.').map_or(name.clone(), |(s, _)| s.to_string());
                    let (mut artist, mut album, mut seconds) = (String::new(), String::new(), 0);
                    // the tags and the length are read from the file's own bytes (the same reader the media library uses); a file it cannot read
                    // is still offered under its name
                    if let Some(t) = crate::media::describe_path(&real.to_string_lossy()) {
                        if !t.title.trim().is_empty() {
                            title = t.title;
                        }
                        if t.artist != crate::media::UNKNOWN_ARTIST {
                            artist = t.artist;
                        }
                        if t.album != crate::media::UNKNOWN_ALBUM {
                            album = t.album;
                        }
                        seconds = t.seconds.unwrap_or(0);
                    }
                    lib.items.push(Media { title, artist, album, video, mime: mime.into(), size: md.len(), seconds });
                    lib.paths.push(real);
                }
            }
        }
    }
    lib
}

/// `a12` or `v3` to a position of the library; the letter has to agree with the file's kind.
fn index_of(lib: &Library, id: &str) -> Option<usize> {
    let (video, n) = match id.split_at_checked(1)? {
        ("a", n) => (false, n),
        ("v", n) => (true, n),
        _ => return None,
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || n.len() > 6 {
        return None;
    }
    let i: usize = n.parse().ok()?;
    (lib.items.get(i)?.video == video).then_some(i)
}

// -------------------------------------------------------------------------------------------------------------------- server

struct Shared {
    cfg: ServeCfg,
    id: Identity,
    stop: AtomicBool,
    lib: Mutex<(Instant, Arc<Library>, u32)>,
    open: AtomicUsize,
    http_port: u16,
    /// Players that asked to be told when the content changes (F-MEDIA-02).
    subs: Mutex<Subscriptions>,
    /// The update number the subscribers were last told.
    told: Mutex<u32>,
    /// Messages being sent at the moment (few, so a slow player cannot pile up threads).
    sending: AtomicUsize,
}

impl Shared {
    fn library(&self) -> (Arc<Library>, u32) {
        let mut g = self.lib.lock().unwrap_or_else(|e| e.into_inner());
        if g.0.elapsed() > REFRESH {
            let fresh = scan(&self.cfg.folders, &self.cfg.deny);
            if fresh.paths != g.1.paths {
                g.2 = g.2.wrapping_add(1);
            }
            *g = (Instant::now(), Arc::new(fresh), g.2);
        }
        (g.1.clone(), g.2)
    }

    /// Send messages to subscribers on a thread of their own, a few at a time at most.
    fn send(self: &Arc<Self>, msgs: Vec<Notify>) {
        if msgs.is_empty() || self.sending.load(Ordering::Relaxed) >= 8 {
            return;
        }
        self.sending.fetch_add(1, Ordering::SeqCst);
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("iyakki-upnp-event".into()).spawn(move || {
            for n in msgs {
                let ok = upnp_events::deliver(&n);
                me.subs.lock().unwrap_or_else(|e| e.into_inner()).delivered(&n.sid, ok);
            }
            me.sending.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.sending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Tell the subscribers when the content changed, and forget the ones that ran out. Called from the SSDP thread every 100 ms.
    fn pump(self: &Arc<Self>) {
        let now = now_secs();
        {
            let mut subs = self.subs.lock().unwrap_or_else(|e| e.into_inner());
            subs.expire(now);
            if subs.is_empty() {
                return;
            }
        }
        let (_, update) = self.library();
        let mut told = self.told.lock().unwrap_or_else(|e| e.into_inner());
        if *told != update {
            *told = update;
            let msgs = self.subs.lock().unwrap_or_else(|e| e.into_inner()).messages(Service::ContentDirectory, &[("SystemUpdateID", update.to_string())], None);
            drop(told);
            self.send(msgs);
        }
    }

    fn allowed(&self, ip: IpAddr) -> bool {
        upnp::may_answer(ip) || (self.cfg.allow_loopback && ip.is_loopback())
    }
}

pub struct Running {
    shared: Arc<Shared>,
    http: SocketAddr,
    ssdp: SocketAddr,
    threads: Vec<JoinHandle<()>>,
}

impl Running {
    pub fn http_addr(&self) -> SocketAddr {
        self.http
    }
    pub fn ssdp_addr(&self) -> SocketAddr {
        self.ssdp
    }
    /// Look at the shared folders again now instead of at the next 30-second turn (subscribers are told if anything changed).
    pub fn refresh(&self) {
        let mut g = self.shared.lib.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = scan(&self.shared.cfg.folders, &self.shared.cfg.deny);
        if fresh.paths != g.1.paths {
            g.2 = g.2.wrapping_add(1);
        }
        *g = (Instant::now(), Arc::new(fresh), g.2);
    }
    /// How many players are subscribed to changes.
    pub fn subscribers(&self) -> usize {
        self.shared.subs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    /// The players subscribed to changes now (address, what they follow, seconds left).
    pub fn subscription_list(&self) -> Vec<crate::upnp_events::SubInfo> {
        self.shared.subs.lock().unwrap_or_else(|e| e.into_inner()).listing(now_secs())
    }
    pub fn items(&self) -> usize {
        self.shared.library().0.items.len()
    }
    /// Say goodbye on the network, close the sockets and wait for the threads.
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start(cfg: ServeCfg) -> Result<Running, String> {
    let listener = TcpListener::bind(cfg.http_bind).map_err(|e| format!("cannot listen on {}: {e}", cfg.http_bind))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let http = listener.local_addr().map_err(|e| e.to_string())?;
    let udp = UdpSocket::bind(cfg.ssdp_bind).map_err(|e| format!("cannot use {} for finding players ({e}); another media server may be running", cfg.ssdp_bind))?;
    if cfg.multicast {
        udp.join_multicast_v4(&SSDP_GROUP, &Ipv4Addr::UNSPECIFIED).map_err(|e| format!("could not join the multicast group of UPnP: {e}"))?;
    }
    udp.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
    let ssdp = udp.local_addr().map_err(|e| e.to_string())?;
    let lib = scan(&cfg.folders, &cfg.deny);
    let shared = Arc::new(Shared { id: Identity { friendly_name: cfg.name.clone(), uuid: cfg.uuid.clone() }, cfg, stop: AtomicBool::new(false), lib: Mutex::new((Instant::now(), Arc::new(lib), 1)), open: AtomicUsize::new(0), http_port: http.port(), subs: Mutex::new(Subscriptions::new()), told: Mutex::new(1), sending: AtomicUsize::new(0) });
    (shared.cfg.audit)("media_server.start", &format!("http={http} items={}", shared.library().0.items.len()));
    let (s1, s2) = (shared.clone(), shared.clone());
    let t1 = std::thread::Builder::new().name("iyakki-upnp-http".into()).spawn(move || http_loop(listener, s1)).map_err(|e| e.to_string())?;
    let t2 = std::thread::Builder::new().name("iyakki-upnp-ssdp".into()).spawn(move || ssdp_loop(udp, s2)).map_err(|e| e.to_string())?;
    Ok(Running { shared, http, ssdp, threads: vec![t1, t2] })
}

fn local_ip_toward(peer: IpAddr) -> Option<IpAddr> {
    let s = UdpSocket::bind(if peer.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).ok()?;
    s.connect((peer, 9)).ok()?;
    Some(s.local_addr().ok()?.ip())
}

fn now_secs() -> i64 {
    crate::clock::now_unix()
}

fn ssdp_loop(sock: UdpSocket, sh: Arc<Shared>) {
    let mut throttle = Throttle::new();
    let mut buf = [0u8; 2048];
    let announce = |alive: bool, sock: &UdpSocket| {
        if !sh.cfg.multicast {
            return;
        }
        let loc = |ip: IpAddr| format!("http://{ip}:{}/upnp/device.xml", sh.http_port);
        let ip = local_ip_toward(IpAddr::V4(SSDP_GROUP)).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        for d in upnp::notifications(alive, &sh.id.uuid, &loc(ip), SERVER) {
            let _ = sock.send_to(&d, (SSDP_GROUP, 1900));
        }
    };
    announce(true, &sock);
    while !sh.stop.load(Ordering::Relaxed) {
        let (n, src) = match sock.recv_from(&mut buf) {
            Ok(x) => x,
            Err(_) => {
                sh.pump();
                continue;
            }
        };
        if !sh.allowed(src.ip()) || !throttle.allow(src.ip(), now_secs()) {
            continue;
        }
        let Some(search) = upnp::parse_search(&buf[..n]) else { continue };
        let Some(ip) = local_ip_toward(src.ip()) else { continue };
        let location = match ip {
            IpAddr::V4(a) => format!("http://{a}:{}/upnp/device.xml", sh.http_port),
            IpAddr::V6(a) => format!("http://[{a}]:{}/upnp/device.xml", sh.http_port),
        };
        for reply in upnp::search_replies(&search, &sh.id.uuid, &location, SERVER, &crate::sharedsite::http_date(now_secs())) {
            let _ = sock.send_to(&reply, src);
        }
    }
    announce(false, &sock);
}

fn http_loop(listener: TcpListener, sh: Arc<Shared>) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !sh.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !sh.allowed(peer.ip()) || sh.open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue; // dropped without a word
                }
                sh.open.fetch_add(1, Ordering::SeqCst);
                let s = sh.clone();
                match std::thread::Builder::new().name("iyakki-upnp-conn".into()).spawn(move || {
                    connection(stream, &s);
                    s.open.fetch_sub(1, Ordering::SeqCst);
                }) {
                    Ok(h) => {
                        workers.retain(|w| !w.is_finished());
                        workers.push(h);
                    }
                    Err(_) => {
                        sh.open.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    for w in workers {
        let _ = w.join();
    }
}

fn reply(stream: &mut TcpStream, status: u16, ctype: &str, extra: &[(&str, String)], body: &[u8], head_only: bool) {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        412 => "Precondition Failed",
        503 => "Service Unavailable",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Error",
    };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nServer: {SERVER}\r\nX-Content-Type-Options: nosniff\r\nEXT:\r\n", body.len());
    for (k, v) in extra {
        h.push_str(&format!("{k}: {}\r\n", v.replace(['\r', '\n'], "")));
    }
    h.push_str("\r\n");
    let _ = stream.write_all(h.as_bytes());
    if !head_only {
        let _ = stream.write_all(body);
    }
}

/// `192.168.1.5:8200` or `[fe80::1]:8200` or a bare number address: the host part must be an address, not a name.
fn numeric_host(h: &str) -> bool {
    let host = if let Some(rest) = h.strip_prefix('[') { rest.split(']').next().unwrap_or("") } else { h.rsplit_once(':').map_or(h, |(a, _)| a) };
    host.parse::<IpAddr>().is_ok()
}

fn connection(mut stream: TcpStream, sh: &Arc<Shared>) {
    // an accepted socket inherits the listener's non-blocking mode on macOS and the BSDs (not on Linux): a read would end the request at once
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let started = Instant::now();
    let (mut req, mut at) = loop {
        if started.elapsed() > sh.cfg.head_deadline {
            return;
        }
        match parse_head(&buf) {
            Head::Done(r, n) => break (r, n),
            Head::Bad(s, _) => return reply(&mut stream, s, "text/plain", &[], b"", false),
            Head::Incomplete => {}
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let want: usize = req.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if want > MAX_BODY {
        return reply(&mut stream, 400, "text/plain", &[], b"", false);
    }
    while buf.len() < at + want {
        if started.elapsed() > sh.cfg.head_deadline {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    req.body = buf[at..at + want].to_vec();
    at += want;
    let _ = at;
    let Some(host) = req.header("host").map(str::to_string).filter(|h| numeric_host(h)) else {
        return reply(&mut stream, 400, "text/plain", &[], b"", false);
    };
    let head_only = req.method == "HEAD";
    if matches!(req.method.as_str(), "SUBSCRIBE" | "UNSUBSCRIBE") {
        if let (Some(service), Ok(peer)) = (Service::from_path(&req.raw_path), stream.peer_addr()) {
            return events(&mut stream, sh, service, &req, peer.ip());
        }
    }
    let (lib, update) = sh.library();
    if let Some(id) = req.raw_path.strip_prefix("/upnp/media/") {
        if req.method != "GET" && req.method != "HEAD" {
            return reply(&mut stream, 405, "text/plain", &[], b"", false);
        }
        // `?t=mp3` asks for the file converted (only when a converter is set up and the file is one it can convert)
        let want = req.query.split('&').find_map(|p| p.strip_prefix("t=")).and_then(crate::upnp_transcode::target).filter(|_| sh.cfg.transcoder.is_some());
        return match want {
            Some(t) => media_converted(&mut stream, sh, &lib, id, &t, head_only),
            None => media(&mut stream, sh, &lib, id, &req, head_only),
        };
    }
    let base = format!("http://{host}/upnp/media/");
    let alt = sh.cfg.transcoder.clone();
    let cat = ListCatalog { items: lib.items.clone(), update, url_for: Box::new(move |id| format!("{base}{id}")), alt: alt.map(|t| -> Box<dyn Fn(&upnp::Media) -> Option<(String, String)> + Send + Sync> { Box::new(move |m| match crate::upnp_transcode::plan(&m.mime, m.video, &|x| t.can(x)) { Some(crate::upnp_transcode::Plan::Transcode(x)) => Some((x.mime.to_string(), x.name.to_string())), _ => None }) }) };
    match upnp::handle(&req.method, &req.raw_path, req.header("soapaction"), &req.body, &sh.id, &cat) {
        Some(r) => reply(&mut stream, r.status, &r.content_type, &[], &r.body, head_only),
        None => reply(&mut stream, 404, "text/plain", &[], b"", false),
    }
}

/// `SUBSCRIBE` and `UNSUBSCRIBE` (see [`crate::upnp_events`]). A new subscriber is sent all variables once the answer is out.
fn events(stream: &mut TcpStream, sh: &Arc<Shared>, service: Service, req: &crate::sharedsite::Req, peer: IpAddr) {
    let now = now_secs();
    let new = req.method == "SUBSCRIBE" && req.header("callback").is_some();
    let ans = {
        let mut subs = sh.subs.lock().unwrap_or_else(|e| e.into_inner());
        if req.method == "SUBSCRIBE" {
            subs.subscribe(service, req.header("callback"), req.header("nt"), req.header("sid"), req.header("timeout"), peer, now)
        } else {
            subs.unsubscribe(service, req.header("sid"), peer)
        }
    };
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let (Some(sid), Some(t)) = (&ans.sid, ans.timeout) {
        extra.push(("SID", sid.clone()));
        extra.push(("TIMEOUT", format!("Second-{t}")));
    }
    reply(stream, ans.status, "text/plain", &extra, b"", false);
    if new && ans.status == 200 {
        let update = sh.library().1;
        let msgs = sh.subs.lock().unwrap_or_else(|e| e.into_inner()).messages(service, &upnp_events::all_vars(service, update), ans.sid.as_deref());
        sh.send(msgs);
    }
}

/// The file converted on the fly. The file is opened and checked exactly as for a plain download (inside a shared folder, no link, no
/// pipe); a player gets a stream without a length or ranges, and the conversion stops when the player does.
fn media_converted(stream: &mut TcpStream, sh: &Shared, lib: &Library, id: &str, t: &crate::upnp_transcode::Target, head_only: bool) {
    let (Some(i), Some(tc)) = (index_of(lib, id), sh.cfg.transcoder.clone()) else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let item = &lib.items[i];
    // only what the converter is for: a video target for a video, an audio target for sound
    if item.video != t.video && t.video || !tc.can(t) {
        return reply(stream, 404, "text/plain", &[], b"", false);
    }
    let path = &lib.paths[i];
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let Ok(file) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let Ok(md) = file.metadata() else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let inside = std::fs::canonicalize(path).ok().is_some_and(|real| sh.cfg.folders.iter().filter_map(|f| std::fs::canonicalize(f).ok()).any(|root| real.starts_with(&root)) && !sh.cfg.deny.iter().any(|d| real.starts_with(d)) && std::fs::metadata(&real).is_ok_and(|m| (m.dev(), m.ino()) == (md.dev(), md.ino())));
    if !inside || !md.is_file() {
        return reply(stream, 404, "text/plain", &[], b"", false);
    }
    drop(file);
    (sh.cfg.audit)("upnp.transcode", t.name);
    let h = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nConnection: close\r\nServer: {SERVER}\r\nX-Content-Type-Options: nosniff\r\nAccept-Ranges: none\r\ntransferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: DLNA.ORG_OP=00;DLNA.ORG_CI=1\r\nCache-Control: no-store\r\n\r\n", t.mime);
    if stream.write_all(h.as_bytes()).is_err() || head_only {
        return;
    }
    let Ok(mut src) = tc.start(t, path) else { return };
    let mut buf = vec![0u8; 64 * 1024];
    while !sh.stop.load(Ordering::Relaxed) {
        match src.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

fn media(stream: &mut TcpStream, sh: &Shared, lib: &Library, id: &str, req: &crate::sharedsite::Req, head_only: bool) {
    let Some(i) = index_of(lib, id) else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let path = &lib.paths[i];
    // the file is opened without following a link (and without blocking on a pipe put in its place), then looked at again: it must be
    // the very file the folder's own path leads to now, inside one of the shared folders
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let Ok(mut file) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let Ok(md) = file.metadata() else { return reply(stream, 404, "text/plain", &[], b"", false) };
    let same_inside = std::fs::canonicalize(path).ok().is_some_and(|real| sh.cfg.folders.iter().filter_map(|f| std::fs::canonicalize(f).ok()).any(|root| real.starts_with(&root)) && !sh.cfg.deny.iter().any(|d| real.starts_with(d)) && std::fs::metadata(&real).is_ok_and(|m| (m.dev(), m.ino()) == (md.dev(), md.ino())));
    if !same_inside || !md.is_file() {
        return reply(stream, 404, "text/plain", &[], b"", false);
    }
    let size = md.len();
    let mime = &lib.items[i].mime;
    let mut extra = vec![("Accept-Ranges", "bytes".to_string()), ("transferMode.dlna.org", "Streaming".to_string()), ("contentFeatures.dlna.org", "DLNA.ORG_OP=01;DLNA.ORG_CI=0".to_string()), ("Cache-Control", "no-store".to_string())];
    let (status, start, len) = match req.header("range") {
        Some(r) => match crate::mediaserve::parse_range(r, size) {
            Some((a, b)) => {
                extra.push(("Content-Range", format!("bytes {a}-{b}/{size}")));
                (206, a, b - a + 1)
            }
            None => {
                extra.push(("Content-Range", format!("bytes */{size}")));
                return reply(stream, 416, "text/plain", &extra, b"", false);
            }
        },
        None => (200, 0, size),
    };
    let reason = if status == 206 { "Partial Content" } else { "OK" };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\nConnection: close\r\nServer: {SERVER}\r\nX-Content-Type-Options: nosniff\r\n");
    for (k, v) in &extra {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    if stream.write_all(h.as_bytes()).is_err() || head_only {
        return;
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 && !sh.stop.load(Ordering::Relaxed) {
        let want = (left as usize).min(buf.len());
        match file.read(&mut buf[..want]) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
        }
    }
}

// ----------------------------------------------------------------------------------------------- one server per signed-in person

fn registry() -> &'static Mutex<HashMap<String, Running>> {
    static R: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The folders the person shares in Files mode (not the package drop): the only place this server reads from.
pub fn share_folders(data_dir: &Path, user: &str) -> Vec<PathBuf> {
    let cfgs: Vec<crate::sharedfolder::ShareCfg> = std::fs::read_to_string(crate::sharedfolder::share_file(data_dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    cfgs.into_iter().filter(|c| c.mode == crate::sharedfolder::Mode::Files && !c.package_drop && !c.folder.is_empty()).map(|c| PathBuf::from(c.folder)).collect()
}

/// A name for the server that stays the same for a person on this machine (random, kept in the data folder; no serial number).
fn uuid_for(data_dir: &Path, user: &str) -> Result<String, String> {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
    let p = data_dir.join("upnp").join(format!("{safe}.uuid"));
    if let Some(t) = std::fs::read_to_string(&p).ok().map(|t| t.trim().to_string()).filter(|t| t.len() == 36) {
        return Ok(t);
    }
    let mut b = [0u8; 16];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no randomness: {e:?}"))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let u = format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]);
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, &u).map_err(|e| e.to_string())?;
    Ok(u)
}

/// Switch the person's media server on: players on the local network find it and play the audio and video of the person's shared
/// folders. Returns what to tell the person. Fails when there is no shared folder, or the finding port is busy.
/// The person's own ffmpeg as the converter, when there is one that runs (the audio and video add-on or a downloaded ffmpeg); otherwise none.
fn real_transcoder() -> Option<Arc<dyn crate::upnp_transcode::Transcoder>> {
    let p = crate::player::find_program("ffmpeg").filter(|p| crate::player::runs(p))?;
    crate::upnp_transcode::Ffmpeg::probe(Path::new(&p)).map(|f| Arc::new(f) as Arc<dyn crate::upnp_transcode::Transcoder>)
}

pub fn start_for(data_dir: &Path, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Result<String, String> {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = r.get(user) {
        return Ok(format!("The media server is already on ({} files).", run.items()));
    }
    let folders = share_folders(data_dir, user);
    if folders.is_empty() {
        return Err("There is no shared folder to offer: share a folder first (Settings -> Shared folders).".into());
    }
    let cfg = ServeCfg { name: format!("IYAKKI ({user})"), uuid: uuid_for(data_dir, user)?, http_bind: "0.0.0.0:0".parse().unwrap(), ssdp_bind: "0.0.0.0:1900".parse().unwrap(), multicast: true, folders, deny: crate::sharedfolder::deny_list(data_dir), allow_loopback: false, head_deadline: Duration::from_secs(10), audit, transcoder: real_transcoder() };
    let run = start(cfg)?;
    let msg = format!("The media server is on: {} audio and video files of your shared folders are visible to players on the local network (port {}). It stops when you sign out or switch it off.", run.items(), run.http_addr().port());
    r.insert(user.to_string(), run);
    Ok(msg)
}

/// What casting needs for an item of a person's running media server: its title, type, whether it is video and the address a renderer on
/// the local network fetches it from (this computer's address toward the renderer, the server's port). `None` when the server is not on
/// or has no such item.
pub fn cast_source(user: &str, id: &str, toward: IpAddr) -> Option<(String, String, bool, String)> {
    let r = registry().lock().unwrap_or_else(|e| e.into_inner());
    let run = r.get(user)?;
    let (lib, _) = run.shared.library();
    let i = index_of(&lib, id)?;
    let m = &lib.items[i];
    let me = local_ip_toward(toward)?;
    Some((m.title.clone(), m.mime.clone(), m.video, format!("http://{}/upnp/media/{id}", SocketAddr::new(me, run.http.port()))))
}

/// Switch it off; false when it was not on.
pub fn stop_user(user: &str) -> bool {
    let run = registry().lock().unwrap_or_else(|e| e.into_inner()).remove(user);
    match run {
        Some(r) => {
            r.stop();
            true
        }
        None => false,
    }
}

/// (port, files) while the person's server is on.
pub fn status_for(user: &str) -> Option<(u16, usize)> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(user).map(|r| (r.http_addr().port(), r.items()))
}

/// The players subscribed to the person's server (empty when it is off).
pub fn subscriptions_for(user: &str) -> Vec<crate::upnp_events::SubInfo> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(user).map(|r| r.subscription_list()).unwrap_or_default()
}

/// One line for the Settings page and the chat: how many players follow the server and which.
pub fn subscribers_text(list: &[crate::upnp_events::SubInfo]) -> String {
    if list.is_empty() {
        return "No player follows the server for changes.".into();
    }
    let mut peers: Vec<String> = Vec::new();
    for s in list {
        let p = s.peer.to_string();
        if !peers.contains(&p) {
            peers.push(p);
        }
    }
    format!("{} subscription(s) from {} player(s): {}.", list.len(), peers.len(), peers.iter().take(6).cloned().collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-upserve-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("share/sub")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        std::fs::create_dir_all(d.join("share/.hid")).unwrap();
        let song: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(d.join("share/song.mp3"), &song).unwrap();
        std::fs::write(d.join("share/sub/film.mp4"), vec![7u8; 1000]).unwrap();
        std::fs::write(d.join("share/notes.txt"), "not media").unwrap();
        std::fs::write(d.join("share/.secret.mp3"), "hidden").unwrap();
        std::fs::write(d.join("share/.hid/x.mp3"), "hidden dir").unwrap();
        std::fs::write(d.join("outside/leak.mp3"), "outside the share").unwrap();
        std::os::unix::fs::symlink(d.join("outside/leak.mp3"), d.join("share/link.mp3")).unwrap();
        std::os::unix::fs::symlink(d.join("outside"), d.join("share/linkdir")).unwrap();
        d
    }

    fn cfg(d: &Path, loopback: bool) -> ServeCfg {
        ServeCfg { name: "Test <server>".into(), uuid: "6f1c1c0a-1111-4222-8333-444455556666".into(), http_bind: "127.0.0.1:0".parse().unwrap(), ssdp_bind: "127.0.0.1:0".parse().unwrap(), multicast: false, folders: vec![d.join("share")], deny: vec![], allow_loopback: loopback, head_deadline: Duration::from_secs(10), audit: Arc::new(|_, _| {}), transcoder: None }
    }

    fn http(addr: SocketAddr, raw: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let at = out.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(out.len());
        let head = String::from_utf8_lossy(&out[..at]).into_owned();
        let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        (status, head, out[at..].to_vec())
    }

    fn get(addr: SocketAddr, path: &str, extra: &str) -> (u16, String, Vec<u8>) {
        http(addr, &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}\r\n"))
    }

    fn soap(addr: SocketAddr, action: &str, args: &str) -> String {
        let body = format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action} xmlns:u=\"{}\">{args}</u:{action}></s:Body></s:Envelope>", upnp::CD_TYPE);
        let raw = format!("POST /upnp/cd/control HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/xml\r\nSOAPACTION: \"{}#{action}\"\r\nContent-Length: {}\r\n\r\n{body}", upnp::CD_TYPE, body.len());
        String::from_utf8_lossy(&http(addr, &raw).2).into_owned()
    }

    /// A WAV file of `secs` seconds of silence (8 kHz, 16 bit, mono).
    fn wav(secs: u32) -> Vec<u8> {
        let data = secs * 16_000;
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 1, 0]);
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16_000u32.to_le_bytes());
        v.extend_from_slice(&[2, 0, 16, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data as usize));
        v
    }

    #[test]
    fn the_library_reads_the_length_of_a_file_and_a_player_is_told_it_in_the_listing() {
        let d = tree("dur");
        std::fs::write(d.join("share/Ada - Hello.wav"), wav(3)).unwrap();
        let lib = scan(&[d.join("share")], &[]);
        let m = lib.items.iter().find(|m| m.mime.contains("wav")).expect("the wav file is offered");
        assert_eq!((m.seconds, m.title.as_str(), m.artist.as_str()), (3, "Hello", "Ada"), "{m:?}");
        // a file whose bytes say nothing is offered under its name with no length, as before
        let junk = lib.items.iter().find(|m| m.title == "song").unwrap();
        assert_eq!(junk.seconds, 0);
        let run = start(cfg(&d, true)).unwrap();
        let xml = soap(run.http_addr(), "Browse", "<ObjectID>1</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>50</RequestedCount><SortCriteria></SortCriteria>");
        assert!(xml.contains("duration=&quot;0:00:03&quot;"), "{xml}");
        run.stop();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_scan_offers_media_of_the_shared_folders_and_nothing_hidden_or_outside() {
        let d = tree("scan");
        let lib = scan(&[d.join("share"), d.join("nowhere")], &[]);
        let mut names: Vec<_> = lib.paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["film.mp4", "song.mp3"], "no text file, no hidden file or folder, no link that leaves the folder");
        assert!(lib.items.iter().any(|m| m.video && m.mime == "video/mp4") && lib.items.iter().any(|m| !m.video && m.title == "song"));
        // the deny list is kept out as well
        let lib = scan(&[d.join("share")], &[std::fs::canonicalize(d.join("share/sub")).unwrap()]);
        assert_eq!(lib.items.len(), 1);
        assert_eq!(kind_of("A.MP3"), Some(("audio/mpeg", false)));
        assert_eq!(kind_of("x.exe"), None);
        assert!(index_of(&lib, "a0").is_some() && index_of(&lib, "v0").is_none() && index_of(&lib, "a99").is_none() && index_of(&lib, "../a0").is_none() && index_of(&lib, "a").is_none() && index_of(&lib, "a-1").is_none());
    }

    #[test]
    fn a_player_finds_the_server_reads_its_pages_browses_and_plays_with_ranges() {
        let d = tree("serve");
        let run = start(cfg(&d, true)).unwrap();
        // finding: an M-SEARCH to the socket is answered with the address of the description
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.send_to(b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n", run.ssdp_addr()).unwrap();
        let mut b = [0u8; 2048];
        let (n, _) = c.recv_from(&mut b).unwrap();
        let text = String::from_utf8_lossy(&b[..n]).into_owned();
        assert!(text.starts_with("HTTP/1.1 200 OK") && text.contains(&format!("LOCATION: http://127.0.0.1:{}/upnp/device.xml", run.http_addr().port())), "{text}");
        // a search for something else, and junk, get no answer
        c.set_read_timeout(Some(Duration::from_millis(400))).unwrap();
        c.send_to(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:other:device:Printer:1\r\n\r\n", run.ssdp_addr()).unwrap();
        c.send_to(b"junk", run.ssdp_addr()).unwrap();
        assert!(c.recv_from(&mut b).is_err());
        // the pages
        let a = run.http_addr();
        let (s, _, body) = get(a, "/upnp/device.xml", "");
        assert_eq!(s, 200);
        assert!(String::from_utf8_lossy(&body).contains("Test &lt;server&gt;"));
        assert_eq!(get(a, "/upnp/cd.xml", "").0, 200);
        // browsing: the root, the music folder, and the address of a song as the player gets it
        let root = soap(a, "Browse", "<ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount>");
        assert!(root.contains("<NumberReturned>2</NumberReturned>"), "{root}");
        let music = soap(a, "Browse", "<ObjectID>1</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag>");
        assert!(music.contains("song") && !music.contains("film") && !music.contains("leak") && !music.contains("secret") && !music.contains("notes"), "{music}");
        assert!(music.contains(&format!("http://127.0.0.1:{}/upnp/media/a0", a.port())) || music.contains(&format!("http://{a}/upnp/media/a0")), "{music}");
        // playing: the whole file, a range, a bad range, a head request
        let song: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let (s, h, body) = get(a, "/upnp/media/a0", "");
        assert_eq!((s, body.as_slice()), (200, song.as_slice()));
        assert!(h.contains("Content-Type: audio/mpeg") && h.contains("Accept-Ranges: bytes") && h.contains("transferMode.dlna.org: Streaming"), "{h}");
        let (s, h, body) = get(a, "/upnp/media/a0", "Range: bytes=100-199\r\n");
        assert_eq!((s, body.as_slice()), (206, &song[100..200]));
        assert!(h.contains("Content-Range: bytes 100-199/5000"));
        let (s, _, body) = get(a, "/upnp/media/a0", "Range: bytes=-10\r\n");
        assert_eq!((s, body.as_slice()), (206, &song[4990..]));
        assert_eq!(get(a, "/upnp/media/a0", "Range: bytes=9999-\r\n").0, 416);
        let (s, h, body) = http(a, &format!("HEAD /upnp/media/a0 HTTP/1.1\r\nHost: {a}\r\n\r\n"));
        assert!(s == 200 && body.is_empty() && h.contains("Content-Length: 5000"));
        assert_eq!(get(a, "/upnp/media/v1", "").0, 200, "the video is item 1: ids are positions, with the letter of the kind");
        assert_eq!(get(a, "/upnp/media/v0", "").0, 404, "item 0 is a song, not a video");
        // nothing outside the list can be named
        for bad in ["/upnp/media/a1", "/upnp/media/a../../etc/passwd", "/upnp/media/..%2f..%2fetc", "/upnp/media/", "/upnp/media/v9", "/etc/passwd", "/upnp/../upnp/media/a0"] {
            let s = get(a, bad, "").0;
            assert!(s == 404 || s == 400, "{bad}: {s}");
        }
        assert_eq!(http(a, &format!("POST /upnp/media/a0 HTTP/1.1\r\nHost: {a}\r\nContent-Length: 0\r\n\r\n")).0, 405);
        // a name instead of a number in Host (a rebinding page) is refused
        assert_eq!(http(a, "GET /upnp/device.xml HTTP/1.1\r\nHost: evil.example:80\r\n\r\n").0, 400);
        let a = run.http_addr();
        run.stop();
        // the accept loop notices the stop on its next wake-up: wait (wall clock) until the port refuses
        let t0 = std::time::Instant::now();
        while TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_ok() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_err(), "the port is closed after stopping");
    }

    #[test]
    fn a_server_that_does_not_trust_the_address_answers_nobody() {
        let d = tree("deny");
        let run = start(cfg(&d, false)).unwrap();
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        c.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        c.send_to(b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: ssdp:all\r\n\r\n", run.ssdp_addr()).unwrap();
        let mut b = [0u8; 2048];
        assert!(c.recv_from(&mut b).is_err(), "a public or loopback source is not answered");
        let (s, _, body) = get(run.http_addr(), "/upnp/device.xml", "");
        assert!(s == 0 && body.is_empty(), "the connection is dropped without a word: {s}");
        run.stop();
    }

    #[test]
    fn a_request_that_trickles_in_is_dropped_at_the_deadline_and_frees_its_place() {
        let d = tree("slow");
        let mut c = cfg(&d, true);
        c.head_deadline = Duration::from_millis(900);
        let run = start(c).unwrap();
        let a = run.http_addr();
        let mut slow = TcpStream::connect(a).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let t0 = Instant::now();
        // a byte every 300 ms is never "too slow" for the per-read timeout of 5 s, only for the deadline of the whole request
        let mut dropped = false;
        for b in b"GET /upnp/device.xml HTTP/1.1\r\nHost: 127.0.0.1\r\nX-A: aaaaaaaaaaaaaaaaaaaaaaaa".iter() {
            if slow.write_all(&[*b]).is_err() {
                dropped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
            if t0.elapsed() > Duration::from_secs(3) {
                break;
            }
        }
        let mut out = Vec::new();
        let _ = slow.read_to_end(&mut out);
        assert!(dropped || out.is_empty(), "no answer for a request that did not arrive in time");
        assert!(t0.elapsed() < Duration::from_secs(4), "the connection ended at the deadline: {:?}", t0.elapsed());
        assert_eq!(get(a, "/upnp/media/a0", "").0, 200, "the server still serves");
        run.stop();
    }

    #[test]
    fn a_file_that_turns_into_a_link_out_of_the_folder_is_not_served_and_hostile_requests_do_not_hang_the_server() {
        let d = tree("toctou");
        let run = start(cfg(&d, true)).unwrap();
        let a = run.http_addr();
        assert_eq!(get(a, "/upnp/media/a0", "").0, 200);
        // after the scan the song is replaced by a link to a file outside the folder
        std::fs::remove_file(d.join("share/song.mp3")).unwrap();
        std::os::unix::fs::symlink(d.join("outside/leak.mp3"), d.join("share/song.mp3")).unwrap();
        assert_eq!(get(a, "/upnp/media/a0", "").0, 404, "looked at again at every request");
        // junk, half requests and a huge body
        for raw in ["", "GARBAGE\r\n\r\n", "GET / HTTP/1.1\r\n", "POST /upnp/cd/control HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 99999999\r\n\r\n"] {
            let mut s = TcpStream::connect(a).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(7))).unwrap();
            let _ = s.write_all(raw.as_bytes());
            let mut out = Vec::new();
            let _ = s.read_to_end(&mut out);
        }
        assert_eq!(get(a, "/upnp/device.xml", "").0, 200, "still serving");
        run.stop();
    }

    #[test]
    fn one_person_one_server_on_their_own_shares_only_switched_off_at_sign_out() {
        let d = tree("person");
        let data = d.join("data");
        let sf = crate::sharedfolder::share_file(&data, "ann");
        std::fs::create_dir_all(sf.parent().unwrap()).unwrap();
        let cfgs = vec![crate::sharedfolder::ShareCfg { id: "a".into(), name: "music".into(), folder: d.join("share").display().to_string(), ..Default::default() }, crate::sharedfolder::ShareCfg { id: "b".into(), name: "drop".into(), folder: d.join("outside").display().to_string(), package_drop: true, ..Default::default() }];
        std::fs::write(&sf, serde_json::to_string(&cfgs).unwrap()).unwrap();
        assert_eq!(share_folders(&data, "ann"), vec![d.join("share")], "the package drop is not offered");
        assert!(share_folders(&data, "bob").is_empty());
        assert!(start_for(&data, "bob", Arc::new(|_, _| {})).unwrap_err().contains("no shared folder"));
        let u1 = uuid_for(&data, "ann").unwrap();
        assert_eq!(u1, uuid_for(&data, "ann").unwrap(), "the same name every time");
        assert_ne!(u1, uuid_for(&data, "bob").unwrap());
        assert_eq!(u1.len(), 36);
        assert!(status_for("ann").is_none() && !stop_user("ann"));
    }

    /// The next message a fake player receives on its call-back socket (answered with 200), or `None` after five seconds.
    fn next_notify(l: &TcpListener) -> Option<String> {
        l.set_nonblocking(true).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Ok((mut s, _)) = l.accept() {
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut got = Vec::new();
                let mut chunk = [0u8; 1024];
                while !String::from_utf8_lossy(&got).contains("</e:propertyset>") {
                    match s.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return Some(String::from_utf8_lossy(&got).into_owned());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn a_player_is_told_at_once_and_again_when_the_content_changes_and_can_renew_and_leave() {
        let d = tree("events");
        let run = start(cfg(&d, true)).unwrap();
        let a = run.http_addr();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let cb = l.local_addr().unwrap();
        // a call-back that is not the player's own address is refused
        let (st, _, _) = http(a, &format!("SUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: {a}\r\nCALLBACK: <http://10.1.2.3:80/x>\r\nNT: upnp:event\r\n\r\n"));
        assert_eq!(st, 412);
        assert_eq!(run.subscribers(), 0);
        let (st, head, _) = http(a, &format!("SUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: {a}\r\nCALLBACK: <http://{cb}/ev>\r\nNT: upnp:event\r\nTIMEOUT: Second-120\r\n\r\n"));
        assert_eq!(st, 200, "{head}");
        assert!(head.contains("TIMEOUT: Second-120"), "{head}");
        let sid = head.lines().find_map(|l| l.strip_prefix("SID: ")).expect("a SID").to_string();
        assert_eq!(run.subscribers(), 1);
        let first = next_notify(&l).expect("the first message");
        assert!(first.starts_with("NOTIFY /ev HTTP/1.1") && first.contains("SEQ: 0") && first.contains(&format!("SID: {sid}")), "{first}");
        assert!(first.contains("<SystemUpdateID>1</SystemUpdateID>"), "{first}");
        // a new file: the next look at the folders finds it and the player is told
        std::fs::write(d.join("share/new.mp3"), vec![1u8; 300]).unwrap();
        run.refresh();
        let second = next_notify(&l).expect("a message about the change");
        assert!(second.contains("SEQ: 1") && second.contains("<SystemUpdateID>2</SystemUpdateID>"), "{second}");
        // renewing needs the right id; leaving ends it
        assert_eq!(http(a, &format!("SUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: {a}\r\nSID: {sid}\r\nTIMEOUT: Second-300\r\n\r\n")).0, 200);
        assert_eq!(http(a, &format!("SUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: {a}\r\nSID: uuid:nobody\r\n\r\n")).0, 412);
        assert_eq!(http(a, &format!("UNSUBSCRIBE /upnp/cd/event HTTP/1.1\r\nHost: {a}\r\nSID: {sid}\r\n\r\n")).0, 200);
        assert_eq!(run.subscribers(), 0);
        run.stop();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_player_can_search_by_title_and_class_and_asks_what_is_searchable() {
        let d = tree("search");
        let run = start(cfg(&d, true)).unwrap();
        let a = run.http_addr();
        let caps = soap(a, "GetSearchCapabilities", "");
        assert!(caps.contains("dc:title") && caps.contains("upnp:class"), "{caps}");
        let hit = soap(a, "Search", "<ContainerID>0</ContainerID><SearchCriteria>dc:title contains \"film\"</SearchCriteria><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria>");
        assert!(hit.contains("<TotalMatches>1</TotalMatches>") && hit.contains("film"), "{hit}");
        let videos = soap(a, "Search", "<ContainerID>0</ContainerID><SearchCriteria>upnp:class derivedfrom \"object.item.videoItem\"</SearchCriteria><StartingIndex>0</StartingIndex><RequestedCount>10</RequestedCount>");
        assert!(videos.contains("<TotalMatches>1</TotalMatches>"), "{videos}");
        let bad = soap(a, "Search", "<ContainerID>0</ContainerID><SearchCriteria>dc:title like x</SearchCriteria>");
        assert!(bad.contains("<errorCode>708</errorCode>"), "{bad}");
        // nothing outside the shared folders can be found
        let leak = soap(a, "Search", "<ContainerID>0</ContainerID><SearchCriteria>dc:title contains \"leak\"</SearchCriteria>");
        assert!(leak.contains("<TotalMatches>0</TotalMatches>"), "{leak}");
        run.stop();
        let _ = std::fs::remove_dir_all(&d);
    }
}
