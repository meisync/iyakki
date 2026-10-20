//! The sockets under the private store server: one request per connection (`Connection: close`), hard limits on the head,
//! the body and the time a client may take. No TLS here: put an ingress or a sidecar in front (docs/STORE.md, section 13).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::server::{Req, Resp, StoreServer};

const MAX_HEAD: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 64;

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(if b[i] == b'+' { b' ' } else { b[i] });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn say(s: &str) {
    let _ = writeln!(std::io::stdout(), "{s}");
}

fn status_text(c: u16) -> &'static str {
    match c {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    }
}

fn write_resp(s: &mut TcpStream, r: &Resp) {
    let mut h = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, status_text(r.status), r.content_type, r.body.len());
    for (k, v) in &r.headers {
        if !v.contains(['\r', '\n']) {
            h.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    h.push_str("\r\n");
    let _ = s.write_all(h.as_bytes()).and_then(|_| s.write_all(&r.body)).and_then(|_| s.flush());
}

fn fail(s: &mut TcpStream, code: u16, msg: &str) {
    write_resp(s, &Resp { status: code, content_type: "application/json", body: format!("{{\"ok\":false,\"error\":{msg:?}}}").into_bytes(), headers: vec![] });
}

/// Read one request. Errors are answered on the socket and turned into `None`.
fn read_request(s: &mut TcpStream, max_body: usize) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            fail(s, 431, "request headers too large");
            return None;
        }
        match s.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => {
                fail(s, 408, "timed out");
                return None;
            }
        }
    };
    let Ok(head) = std::str::from_utf8(&buf[..head_end]) else {
        fail(s, 400, "bad request");
        return None;
    };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version), None) = (first.next(), first.next(), first.next(), first.next()) else {
        fail(s, 400, "bad request line");
        return None;
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') || target.contains(['\0', '\\']) {
        fail(s, 400, "bad request");
        return None;
    }
    let (raw_path, raw_query) = target.split_once('?').unwrap_or((target, ""));
    let path = percent_decode(raw_path);
    if path.split('/').any(|p| p == ".." || p == ".") || path.contains("//") {
        fail(s, 400, "bad path");
        return None;
    }
    let query: HashMap<String, String> = raw_query.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map_or((percent_decode(p), String::new()), |(k, v)| (percent_decode(k), percent_decode(v)))).collect();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if headers.contains_key("transfer-encoding") {
        fail(s, 400, "chunked bodies are not accepted; send a Content-Length");
        return None;
    }
    let len: usize = match headers.get("content-length") {
        None => 0,
        Some(v) => match v.parse() {
            Ok(n) => n,
            Err(_) => {
                fail(s, 400, "bad Content-Length");
                return None;
            }
        },
    };
    if len > max_body + 1024 {
        fail(s, 413, "the body is too large");
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => {
                fail(s, 408, "timed out");
                return None;
            }
        }
    }
    body.truncate(len);
    Some(Req { method: method.to_string(), path, query, headers, body })
}

/// Serve on `listener` until `stop` is set. Returns when the listener is closed or stopped.
pub fn serve(server: Arc<StoreServer>, listener: TcpListener, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    let live = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut s, peer)) => {
                if live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    fail(&mut s, 429, "busy");
                    continue;
                }
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                live.fetch_add(1, Ordering::Relaxed);
                let (srv, live2) = (server.clone(), live.clone());
                std::thread::spawn(move || {
                    if let Some(req) = read_request(&mut s, srv.config().max_body) {
                        let resp = srv.handle(&req, &peer.ip().to_string());
                        write_resp(&mut s, &resp);
                    }
                    live2.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// `iyakki store-server`: the command line. Returns the text to print on success.
pub fn run_cli(args: &[String]) -> Result<(), String> {
    let mut flags: HashMap<String, String> = HashMap::new();
    let mut bools = Vec::new();
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.strip_prefix("--") {
            Some(f @ ("read-only" | "require-trusted")) => bools.push(f.to_string()),
            Some(f) => {
                flags.insert(f.to_string(), it.next().ok_or_else(|| format!("--{f} needs a value"))?.clone());
            }
            None => pos.push(a.clone()),
        }
    }
    let data = std::path::PathBuf::from(flags.get("data").ok_or("--data <folder> is required")?);
    let key_text = match flags.get("catalog-seed-file") {
        Some(f) => super::server::key_from_seed(&std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?)?,
        None => {
            let key_file = flags.get("catalog-key-file").cloned().unwrap_or_else(|| data.join("catalog.key").display().to_string());
            std::fs::read_to_string(&key_file).map_err(|e| format!("{key_file}: {e} (make one with `iyakki pkg keygen` and mount it as a Secret)"))?
        }
    };
    let mut cfg = super::server::Config::new(&data);
    if let Some(f) = flags.get("publish-token-file") {
        let t = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        if t.trim().len() < 24 {
            return Err("the admin token must be at least 24 characters".into());
        }
        cfg.admin_token_hash = Some(super::server::hash_token(&t));
    }
    if let Some(v) = flags.get("id") {
        cfg.id = v.clone();
    }
    if let Some(v) = flags.get("name") {
        cfg.name = v.chars().take(60).collect();
    }
    if let Some(v) = flags.get("max-body-mib") {
        cfg.max_body = v.parse::<usize>().map_err(|_| "--max-body-mib is a number")?.clamp(1, 10) << 20;
    }
    cfg.read_only = bools.iter().any(|b| b == "read-only");
    cfg.allow_community = !bools.iter().any(|b| b == "require-trusted");
    let server = Arc::new(super::server::StoreServer::open(cfg, &key_text)?);
    if pos.first().map(String::as_str) == Some("publisher-add") {
        let (id, name) = (flags.get("pub-id").ok_or("--pub-id is required")?, flags.get("pub-name").cloned().unwrap_or_default());
        let tok = server.create_publisher(id, if name.is_empty() { id } else { &name })?;
        say(&format!("publisher {id} token (shown once, give it to the publisher): {tok}"));
        return Ok(());
    }
    let listen = flags.get("listen").cloned().unwrap_or_else(|| "0.0.0.0:8080".into());
    let l = TcpListener::bind(&listen).map_err(|e| format!("{listen}: {e}"))?;
    say(&format!("store server {} on {listen}; catalog key {}; fingerprint {}", server.config().name, server.catalog_key(), super::server::fingerprint(&server.catalog_key())));
    serve(server, l, Arc::new(AtomicBool::new(false)));
    Ok(())
}
