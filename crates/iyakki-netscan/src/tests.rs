//! The scanner against small fake services on this machine's loopback address.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

/// Run `handler` for every connection on a fresh port; the listener lives as long as the test process.
fn serve(handler: impl Fn(std::net::TcpStream) + Send + Sync + 'static) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = Arc::new(handler);
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let h = h.clone();
            std::thread::spawn(move || h(s));
        }
    });
    port
}

fn request_path(s: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = [0u8; 2048];
    let mut got = Vec::new();
    loop {
        let n = s.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        got.extend_from_slice(&buf[..n]);
        if got.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let line = String::from_utf8_lossy(&got).lines().next()?.to_string();
    line.split_whitespace().nth(1).map(str::to_string)
}

fn reply(s: &mut std::net::TcpStream, status: &str, headers: &str, body: &str) {
    let _ = s.write_all(format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
}

fn http(routes: &'static [(&'static str, &'static str, &'static str)], headers: &'static str) -> u16 {
    serve(move |mut s| {
        let path = request_path(&mut s).unwrap_or_default();
        match routes.iter().find(|r| r.0 == path) {
            Some(r) => reply(&mut s, "200 OK", &format!("{headers}Content-Type: {}\r\n", r.1), r.2),
            None => reply(&mut s, "404 Not Found", headers, "not found"),
        }
    })
}

fn quick() -> Options {
    Options { timing: Timing { connect: Duration::from_millis(300), io: Duration::from_millis(1200) }, announce_wait: None, workers: 8, ..Options::default() }
}

fn scan_ports(ports: &[u16]) -> HostInfo {
    let hosts = scan_blocking(vec![Ipv4Addr::LOCALHOST], Options { ports: ports.to_vec(), ..quick() });
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    hosts.into_iter().next().unwrap()
}

fn svc(h: &HostInfo, port: u16) -> &Service {
    h.services.iter().find(|s| s.port == port).unwrap_or_else(|| panic!("no service on {port}: {:?}", h.services))
}

#[test]
fn ollama_is_found_with_its_version_and_models() {
    let port = http(&[("/", "text/plain", "Ollama is running"), ("/api/version", "application/json", "{\"version\":\"0.5.7\"}"), ("/api/tags", "application/json", "{\"models\":[{\"name\":\"llama3.2:3b\"},{\"name\":\"qwen2.5:7b\"}]}")], "");
    let h = scan_ports(&[port]);
    let s = svc(&h, port);
    assert_eq!((s.kind, s.name.as_str(), s.confidence), (Kind::Ollama, "Ollama 0.5.7", Confidence::High));
    assert_eq!(s.models, vec!["llama3.2:3b", "qwen2.5:7b"]);
    assert!(s.detail.contains("2 models") && s.url.as_deref() == Some(&format!("http://127.0.0.1:{port}/")[..]));
    assert!(h.main_kind() == Kind::Ollama && s.kind.is_model_server());
}

#[test]
fn openai_compatible_servers_are_found_by_their_models_url() {
    let port = http(&[("/v1/models", "application/json", "{\"object\":\"list\",\"data\":[{\"id\":\"qwen2-7b-instruct\"},{\"id\":\"nomic-embed\"}]}"), ("/props", "application/json", "{\"default_generation_settings\":{}}")], "Content-Type: application/json\r\n");
    let s = scan_ports(&[port]).services.remove(0);
    assert_eq!((s.kind, s.name.as_str()), (Kind::LlmServer, "llama.cpp server"));
    assert_eq!(s.models.len(), 2);
    assert_eq!(s.url.as_deref(), Some(&format!("http://127.0.0.1:{port}/v1")[..]));
}

#[test]
fn lemonade_is_found_by_its_health_answer_before_the_openai_check() {
    let port = http(&[("/api/v1/health", "application/json", "{\"status\":\"ok\",\"version\":\"2026.42.1\",\"model_loaded\":null,\"all_models_loaded\":[],\"max_models\":{\"llm\":1}}"), ("/api/v1/models", "application/json", "{\"data\":[{\"id\":\"Qwen3-0.6B-GGUF\",\"recipe\":\"llamacpp\"},{\"id\":\"SD-Turbo\",\"recipe\":\"sd-cpp\"}]}"), ("/v1/models", "application/json", "{\"data\":[{\"id\":\"Qwen3-0.6B-GGUF\"}]}")], "Content-Type: application/json\r\n");
    let s = scan_ports(&[port]).services.remove(0);
    assert_eq!((s.kind, s.name.as_str(), s.confidence), (Kind::LlmServer, "Lemonade Server 2026.42.1", Confidence::High));
    assert_eq!(s.models, vec!["Qwen3-0.6B-GGUF", "SD-Turbo"]);
    assert_eq!(s.url.as_deref(), Some(&format!("http://127.0.0.1:{port}/api/v1")[..]));
}

#[test]
fn cameras_are_found_by_rtsp_by_their_web_page_and_by_onvif() {
    let rtsp = serve(|mut s| {
        let mut b = [0u8; 512];
        if s.read(&mut b).unwrap_or(0) > 0 {
            let _ = s.write_all(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nServer: Fake Camera RTSP 1.0\r\nPublic: DESCRIBE, SETUP, PLAY\r\n\r\n");
        }
    });
    let web = serve(|mut s| {
        let path = request_path(&mut s).unwrap_or_default();
        if path == "/onvif/device_service" {
            reply(&mut s, "400 Bad Request", "Content-Type: application/soap+xml\r\n", "<SOAP-ENV:Envelope><SOAP-ENV:Fault>onvif</SOAP-ENV:Fault></SOAP-ENV:Envelope>");
        } else {
            reply(&mut s, "401 Unauthorized", "Server: App-webs/\r\nWWW-Authenticate: Basic realm=\"IPCamera\"\r\n", "<html><head><title>Network Camera</title></head></html>");
        }
    });
    let h = scan_ports(&[rtsp, web]);
    let r = svc(&h, rtsp);
    assert_eq!((r.kind, r.confidence), (Kind::Camera, Confidence::High));
    assert!(r.name.contains("Fake Camera RTSP") && r.url.as_deref() == Some(&format!("rtsp://127.0.0.1:{rtsp}/")[..]), "{r:?}");
    let w = svc(&h, web);
    assert_eq!((w.kind, w.confidence), (Kind::Camera, Confidence::High), "{w:?}");
    assert!(w.name.contains("ONVIF") && w.detail.contains("asks for a password"), "{w:?}");
    assert!(h.has(Kind::Camera));
}

#[test]
fn banners_ssh_redis_and_silent_ports() {
    let ssh = serve(|mut s| {
        let _ = s.write_all(b"SSH-2.0-OpenSSH_9.6p1 Ubuntu\r\n");
    });
    let redis = serve(|mut s| {
        let mut b = [0u8; 64];
        if s.read(&mut b).unwrap_or(0) > 0 {
            let _ = s.write_all(b"+PONG\r\n");
        }
    });
    let silent = serve(|s| {
        std::thread::sleep(Duration::from_secs(4));
        drop(s);
    });
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let h = scan_ports(&[ssh, redis, silent, closed]);
    let s = svc(&h, ssh);
    assert_eq!((s.kind, s.confidence), (Kind::Access, Confidence::High));
    assert!(s.name.contains("OpenSSH_9.6p1"));
    // Redis is only recognised on its own port or on a Data port; on a random port nothing is sent to it
    let r = svc(&h, redis);
    assert_eq!(r.kind, Kind::Other, "{r:?}");
    let q = svc(&h, silent);
    assert_eq!((q.kind, q.confidence), (Kind::Other, Confidence::Low));
    assert!(h.services.iter().all(|s| s.port != closed), "a closed port is not a service");
}

#[test]
fn risks_are_named_but_nothing_is_tried() {
    // an exposed Docker API (2375 only if it is free here) and telnet's banner
    let telnet = serve(|mut s| {
        let _ = s.write_all(&[0xff, 0xfd, 0x18]);
    });
    let h = scan_ports(&[telnet]);
    let t = svc(&h, telnet);
    assert_eq!(t.kind, Kind::Access);
    assert!(t.risk.as_deref().unwrap().contains("unencrypted") && h.risky());
}

#[test]
fn only_local_addresses_are_scanned_and_cancel_stops_the_scan() {
    // a public address is dropped before anything is sent
    let (tx, rx) = std::sync::mpsc::channel();
    scan(vec![Ipv4Addr::new(8, 8, 8, 8), Ipv4Addr::new(1, 1, 1, 1)], quick(), Arc::new(AtomicBool::new(false)), tx);
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(matches!(events[0], Event::Started { hosts: 0, .. }), "{events:?}");
    assert!(matches!(events.last(), Some(Event::Finished { hosts: 0, services: 0, cancelled: false })));
    // a scan that is cancelled at once finishes at once
    let cancel = Arc::new(AtomicBool::new(true));
    let (tx, rx) = std::sync::mpsc::channel();
    let t0 = Instant::now();
    scan(net::parse_targets("127.0.0.1").unwrap(), quick(), cancel, tx);
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert!(matches!(rx.try_iter().last(), Some(Event::Finished { cancelled: true, .. })));
}

#[test]
fn progress_and_host_events_arrive_in_order() {
    let a = http(&[("/", "text/plain", "Ollama is running")], "");
    let (tx, rx) = std::sync::mpsc::channel();
    scan(vec![Ipv4Addr::LOCALHOST], Options { ports: vec![a], ..quick() }, Arc::new(AtomicBool::new(false)), tx);
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(matches!(events.first(), Some(Event::Started { hosts: 1, ports: 1 })));
    assert!(events.iter().any(|e| matches!(e, Event::Host(h) if h.has(Kind::Ollama))));
    assert!(events.iter().any(|e| matches!(e, Event::Progress { done: 1, total: 1 })));
    assert!(matches!(events.last(), Some(Event::Finished { hosts: 1, services: 1, cancelled: false })));
}
