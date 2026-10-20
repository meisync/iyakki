//! Looking at one open port: what answers there? Only what the service says to anyone who asks (a banner, a home page, a
//! public information URL); no passwords are tried and nothing is changed. Content decides, the port number only
//! orders the guesses and names a service that stays silent.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::model::{Confidence, Kind, Service};

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub connect: Duration,
    pub io: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { connect: Duration::from_millis(400), io: Duration::from_millis(1500) }
    }
}

/// Ports tried by default, with what the port usually means (used for ordering and for services that say nothing).
pub const PORTS: &[(u16, Kind, &str)] = &[
    (21, Kind::Access, "FTP"),
    (22, Kind::Access, "SSH"),
    (23, Kind::Access, "Telnet"),
    (53, Kind::Device, "DNS"),
    (80, Kind::Web, "HTTP"),
    (81, Kind::Web, "HTTP"),
    (443, Kind::Web, "HTTPS"),
    (445, Kind::Access, "SMB file sharing"),
    (554, Kind::Camera, "RTSP video"),
    (631, Kind::Printer, "IPP printing"),
    (1234, Kind::LlmServer, "LM Studio (default port)"),
    (1883, Kind::SmartHome, "MQTT broker"),
    (2375, Kind::Platform, "Docker API (unencrypted)"),
    (3000, Kind::Web, "HTTP (3000)"),
    (3306, Kind::Data, "MySQL"),
    (5000, Kind::Web, "HTTP (5000)"),
    (5001, Kind::Web, "HTTPS (5001)"),
    (5432, Kind::Data, "PostgreSQL"),
    (5900, Kind::Access, "VNC"),
    (6333, Kind::Data, "Qdrant"),
    (6379, Kind::Data, "Redis"),
    (7547, Kind::Device, "TR-069 (router management)"),
    (8000, Kind::Web, "HTTP (8000)"),
    (8006, Kind::Platform, "Proxmox"),
    (8008, Kind::Media, "Chromecast"),
    (8080, Kind::Web, "HTTP (8080)"),
    (8081, Kind::Web, "HTTP (8081)"),
    (8096, Kind::Media, "Jellyfin"),
    (8123, Kind::SmartHome, "Home Assistant"),
    (8443, Kind::Web, "HTTPS (8443)"),
    (8500, Kind::Platform, "Consul"),
    (8554, Kind::Camera, "RTSP video"),
    (8888, Kind::Web, "HTTP (8888)"),
    (9100, Kind::Printer, "Raw printing (JetDirect)"),
    (9200, Kind::Data, "Elasticsearch"),
    (11434, Kind::Ollama, "Ollama (default port)"),
    (13305, Kind::LlmServer, "Lemonade (default port)"),
    (27017, Kind::Data, "MongoDB"),
    (32400, Kind::Media, "Plex"),
    (34567, Kind::Camera, "recorder (XM protocol)"),
    (37777, Kind::Camera, "Dahua recorder"),
];

pub fn default_ports() -> Vec<u16> {
    PORTS.iter().map(|p| p.0).collect()
}

fn port_hint(port: u16) -> Option<(Kind, &'static str)> {
    PORTS.iter().find(|p| p.0 == port).map(|p| (p.1, p.2))
}

pub fn connect(ip: Ipv4Addr, port: u16, timeout: Duration) -> Option<TcpStream> {
    TcpStream::connect_timeout(&SocketAddr::new(IpAddr::V4(ip), port), timeout).ok()
}

fn read_some(s: &mut TcpStream, limit: usize, wait: Duration) -> Vec<u8> {
    let _ = s.set_read_timeout(Some(wait));
    let mut buf = vec![0u8; limit];
    let n = s.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    buf
}

fn printable(b: &[u8]) -> String {
    String::from_utf8_lossy(b).chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// What an open port is. `None` only when the connection is refused straight after.
pub fn identify(ip: Ipv4Addr, port: u16, t: &Timing) -> Option<Service> {
    // services that speak first (SSH, FTP, SMTP, telnet, MySQL)
    let first = {
        let mut s = connect(ip, port, t.connect)?;
        read_some(&mut s, 256, Duration::from_millis(250))
    };
    if !first.is_empty() {
        if let Some(svc) = classify_banner(port, &first) {
            return Some(svc);
        }
    }
    let tls_first = matches!(port, 443 | 8443 | 5001 | 8006 | 9443 | 8883);
    let schemes: [&str; 2] = if tls_first { ["https", "http"] } else { ["http", "https"] };
    for scheme in schemes {
        if let Some(h) = http_get(&format!("{scheme}://{ip}:{port}/"), t.io) {
            return Some(classify_http(ip, port, scheme, &h, t));
        }
        // a plain-text port does not speak TLS: do not wait on the second scheme when the first said "not HTTP at all"
        if scheme == "http" && port_hint(port).is_some_and(|(k, _)| matches!(k, Kind::Camera | Kind::Data | Kind::Printer | Kind::SmartHome)) {
            break;
        }
    }
    if let Some(s) = rtsp_options(ip, port, t) {
        return Some(s);
    }
    if let Some(s) = redis_ping(ip, port, t) {
        return Some(s);
    }
    let (kind, name) = port_hint(port).unwrap_or((Kind::Other, "open port"));
    let mut svc = Service::new(port, kind, name);
    svc.confidence = Confidence::Low;
    svc.detail = if port_hint(port).is_some() { "the port number says so; nothing answered to confirm it".into() } else { "something listens here but did not answer a question".into() };
    if port == 23 {
        svc.risk = Some("telnet sends everything, passwords included, unencrypted".into());
    }
    Some(svc)
}

fn classify_banner(port: u16, b: &[u8]) -> Option<Service> {
    let text = printable(b);
    let line = text.lines().next().unwrap_or("").to_string();
    if line.starts_with("SSH-") {
        let mut s = Service::new(port, Kind::Access, format!("SSH ({})", line.trim_start_matches("SSH-2.0-").trim_start_matches("SSH-1.99-")));
        s.confidence = Confidence::High;
        s.detail = line;
        return Some(s);
    }
    if line.starts_with("220") {
        let lower = line.to_ascii_lowercase();
        let (kind, name) = if lower.contains("ftp") { (Kind::Access, "FTP server") } else if lower.contains("smtp") || lower.contains("esmtp") || lower.contains("mail") { (Kind::Data, "mail server (SMTP)") } else { (Kind::Access, "FTP or mail server") };
        let mut s = Service::new(port, kind, name);
        s.detail = line;
        s.confidence = Confidence::High;
        return Some(s);
    }
    if line.starts_with("RTSP/") {
        return None;
    }
    if port == 3306 && b.len() > 5 && b[4] == 10 {
        let mut s = Service::new(port, Kind::Data, "MySQL / MariaDB");
        s.detail = printable(&b[5..]).split(' ').next().unwrap_or("").to_string();
        s.confidence = Confidence::High;
        return Some(s);
    }
    if port == 23 || b.first() == Some(&0xff) {
        let mut s = Service::new(port, Kind::Access, "Telnet");
        s.risk = Some("telnet sends everything, passwords included, unencrypted".into());
        s.confidence = Confidence::High;
        return Some(s);
    }
    None
}

fn rtsp_options(ip: Ipv4Addr, port: u16, t: &Timing) -> Option<Service> {
    let mut s = connect(ip, port, t.connect)?;
    let _ = s.set_write_timeout(Some(t.io));
    s.write_all(format!("OPTIONS rtsp://{ip}:{port}/ RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: iyakki-scan\r\n\r\n").as_bytes()).ok()?;
    let resp = printable(&read_some(&mut s, 1024, t.io));
    if !resp.starts_with("RTSP/") {
        return None;
    }
    let header = |name: &str| resp.lines().find(|l| l.to_ascii_lowercase().starts_with(&format!("{name}:"))).map(|l| l.split_once(':').map_or("", |x| x.1).trim().to_string());
    let server = header("server").unwrap_or_default();
    let mut svc = Service::new(port, Kind::Camera, if server.is_empty() { "RTSP video stream".to_string() } else { format!("RTSP video stream ({server})") });
    svc.confidence = Confidence::High;
    svc.detail = resp.lines().next().unwrap_or("").to_string();
    svc.url = Some(format!("rtsp://{ip}:{port}/"));
    Some(svc)
}

fn redis_ping(ip: Ipv4Addr, port: u16, t: &Timing) -> Option<Service> {
    // only where Redis (or another data store) is expected: nothing is sent to other ports
    if port != 6379 && !port_hint(port).is_some_and(|(k, _)| k == Kind::Data) {
        return None;
    }
    let mut s = connect(ip, port, t.connect)?;
    s.write_all(b"PING\r\n").ok()?;
    let r = printable(&read_some(&mut s, 64, Duration::from_millis(600)));
    if r.starts_with("+PONG") {
        let mut svc = Service::new(port, Kind::Data, "Redis");
        svc.confidence = Confidence::High;
        svc.risk = Some("answers without a password: anyone on the network can read and change its data".into());
        return Some(svc);
    }
    if r.starts_with("-NOAUTH") || r.contains("Authentication required") {
        let mut svc = Service::new(port, Kind::Data, "Redis (password protected)");
        svc.confidence = Confidence::High;
        return Some(svc);
    }
    None
}

// ---------------------------------------------------------------------------------------------------------- HTTP

pub struct Http {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Http {
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map_or("", |(_, v)| v.as_str())
    }

    pub fn title(&self) -> String {
        let lower = self.body.to_ascii_lowercase();
        let Some(i) = lower.find("<title") else { return String::new() };
        let Some(j) = lower[i..].find('>') else { return String::new() };
        let rest = &self.body[i + j + 1..];
        let end = rest.to_ascii_lowercase().find("</title").unwrap_or(rest.len().min(200));
        rest[..end.min(rest.len())].split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect()
    }
}

/// One GET, no redirects, certificates not checked (home equipment is full of self-signed ones; nothing secret is sent).
pub fn http_get(url: &str, timeout: Duration) -> Option<Http> {
    let agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout))
        .tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build())
        .user_agent("iyakki-scan/0.1")
        .build()
        .new_agent();
    let mut resp = agent.get(url).call().ok()?;
    let status = resp.status().as_u16();
    let headers = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let mut buf = Vec::new();
    let _ = resp.body_mut().as_reader().take(16 * 1024).read_to_end(&mut buf);
    Some(Http { status, headers, body: String::from_utf8_lossy(&buf).into_owned() })
}

fn json(h: &Http) -> Option<serde_json::Value> {
    serde_json::from_str(h.body.trim()).ok()
}

const CAMERA_WORDS: &[&str] = &["ipcam", "ip camera", "network camera", "netcam", "hikvision", "dahua", "reolink", "amcrest", "foscam", "axis communications", "web service", "dvr", "nvr", "app-webs", "dnvrs-webs", "uc-httpd", "avtech", "vivotek", "geovision", "wyze", "tapo", "ezviz", "annke", "lorex", "swann", "camera"];

fn camera_hint(h: &Http) -> Option<String> {
    let hay = format!("{} {} {}", h.header("server"), h.header("www-authenticate"), h.title()).to_ascii_lowercase();
    CAMERA_WORDS.iter().find(|w| hay.contains(**w)).map(|w| w.to_string())
}

/// A Lemonade server at `base`: `GET /api/v1/health` answers `status` with `all_models_loaded` or `max_models`.
fn lemonade(base: &str, port: u16, t: &Timing) -> Option<Service> {
    let h = http_get(&format!("{base}/api/v1/health"), t.io).filter(|h| h.status == 200).and_then(|h| json(&h))?;
    h["status"].as_str()?;
    if !(h["all_models_loaded"].is_array() || h["max_models"].is_object()) {
        return None;
    }
    let mut svc = Service::new(port, Kind::LlmServer, "Lemonade Server");
    if let Some(v) = h["version"].as_str().filter(|v| !v.is_empty() && v.len() < 40) {
        svc.name = format!("Lemonade Server {v}");
    }
    svc.confidence = Confidence::High;
    if let Some(m) = http_get(&format!("{base}/api/v1/models"), t.io).filter(|m| m.status == 200).and_then(|m| json(&m)) {
        svc.models = m["data"].as_array().map(|a| a.iter().filter_map(|x| x["id"].as_str().map(str::to_string)).take(60).collect()).unwrap_or_default();
    }
    svc.detail = format!("{} model{} downloaded · chat, pictures, speech", svc.models.len(), if svc.models.len() == 1 { "" } else { "s" });
    svc.url = Some(format!("{base}/api/v1"));
    Some(svc)
}

fn classify_http(ip: Ipv4Addr, port: u16, scheme: &str, h: &Http, t: &Timing) -> Service {
    let base = format!("{scheme}://{ip}:{port}");
    let server = h.header("server").to_string();
    let title = h.title();
    let mut svc = Service::new(port, Kind::Web, "Web server");
    svc.url = Some(format!("{base}/"));
    svc.detail = [format!("HTTP {}", h.status), if server.is_empty() { String::new() } else { format!("server: {server}") }, if title.is_empty() { String::new() } else { format!("title: {title}") }].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");

    // Ollama says so on its home page
    if h.body.contains("Ollama is running") {
        svc.kind = Kind::Ollama;
        svc.confidence = Confidence::High;
        let version = http_get(&format!("{base}/api/version"), t.io).and_then(|v| json(&v)).and_then(|v| v["version"].as_str().map(str::to_string));
        svc.name = format!("Ollama{}", version.as_ref().map(|v| format!(" {v}")).unwrap_or_default());
        if let Some(tags) = http_get(&format!("{base}/api/tags"), t.io).and_then(|v| json(&v)) {
            svc.models = tags["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).take(60).collect()).unwrap_or_default();
        }
        svc.detail = format!("{} model{} installed", svc.models.len(), if svc.models.len() == 1 { "" } else { "s" });
        return svc;
    }
    // other model servers speak the OpenAI API
    let models_url = format!("{base}/v1/models");
    let looks_like_api = port == 1234 || h.header("content-type").contains("json") || h.status == 404 || h.body.trim().is_empty() || h.body.contains("llama") || h.body.to_ascii_lowercase().contains("openai");
    // AMD's Lemonade Server names itself in its health answer (it also answers /v1/models, so it is asked first)
    if looks_like_api || matches!(port, 8000 | 13305) || format!("{title} {}", h.body).to_ascii_lowercase().contains("lemonade") {
        if let Some(svc_l) = lemonade(&base, port, t) {
            return svc_l;
        }
    }
    if looks_like_api {
        if let Some(m) = http_get(&models_url, t.io).filter(|m| m.status == 200).and_then(|m| json(&m)) {
            if let Some(list) = m["data"].as_array() {
                svc.kind = Kind::LlmServer;
                svc.confidence = Confidence::High;
                svc.models = list.iter().filter_map(|x| x["id"].as_str().map(str::to_string)).take(60).collect();
                let props = http_get(&format!("{base}/props"), t.io).and_then(|p| json(&p));
                svc.name = if props.as_ref().is_some_and(|p| p.get("default_generation_settings").is_some() || p.get("model_path").is_some()) {
                    "llama.cpp server".into()
                } else if port == 1234 {
                    "LM Studio (or another OpenAI-compatible server)".into()
                } else if h.header("server").to_ascii_lowercase().contains("uvicorn") {
                    "OpenAI-compatible server (vLLM or similar)".into()
                } else {
                    "OpenAI-compatible model server".into()
                };
                svc.detail = format!("{} model{} listed", svc.models.len(), if svc.models.len() == 1 { "" } else { "s" });
                svc.url = Some(format!("{base}/v1"));
                return svc;
            }
        }
    }
    let lower_title = title.to_ascii_lowercase();
    let lower_body = h.body.to_ascii_lowercase();
    // named web applications
    let named: &[(&str, Kind, &str)] = &[
        ("home assistant", Kind::SmartHome, "Home Assistant"),
        ("jellyfin", Kind::Media, "Jellyfin media server"),
        ("plex", Kind::Media, "Plex media server"),
        ("emby", Kind::Media, "Emby media server"),
        ("pi-hole", Kind::Device, "Pi-hole (DNS ad blocker)"),
        ("grafana", Kind::Platform, "Grafana"),
        ("portainer", Kind::Platform, "Portainer (Docker)"),
        ("proxmox", Kind::Platform, "Proxmox"),
        ("synology", Kind::Device, "Synology NAS"),
        ("qnap", Kind::Device, "QNAP NAS"),
        ("truenas", Kind::Device, "TrueNAS"),
        ("openmediavault", Kind::Device, "OpenMediaVault NAS"),
        ("unifi", Kind::Device, "UniFi network controller"),
        ("openwrt", Kind::Device, "OpenWrt router"),
        ("pfsense", Kind::Device, "pfSense firewall"),
        ("mikrotik", Kind::Device, "MikroTik router"),
        ("tp-link", Kind::Device, "TP-Link device"),
        ("netgear", Kind::Device, "Netgear device"),
        ("cups", Kind::Printer, "CUPS print server"),
        ("octoprint", Kind::Printer, "OctoPrint (3D printer)"),
        ("qdrant", Kind::Data, "Qdrant vector database"),
        ("chroma", Kind::Data, "Chroma vector database"),
        ("open webui", Kind::LlmServer, "Open WebUI (a chat front end for model servers)"),
        ("nextcloud", Kind::Platform, "Nextcloud"),
        ("syncthing", Kind::Platform, "Syncthing"),
        ("node exporter", Kind::Platform, "Prometheus Node Exporter"),
        ("prometheus", Kind::Platform, "Prometheus"),
    ];
    let hay = format!("{lower_title} {} {}", server.to_ascii_lowercase(), lower_body.chars().take(4000).collect::<String>());
    if let Some((_, kind, name)) = named.iter().find(|(w, _, _)| hay.contains(w)) {
        svc.kind = *kind;
        svc.name = name.to_string();
        svc.confidence = Confidence::High;
        if *kind == Kind::Printer || *kind == Kind::Device {
            svc.confidence = Confidence::Medium;
        }
        return svc;
    }
    // other public information URLs of well-known services
    for (path, kind, name, needle) in [
        ("/identity", Kind::Media, "Plex media server", "MediaContainer"),
        ("/System/Info/Public", Kind::Media, "Jellyfin media server", "ServerName"),
        ("/v1/status/leader", Kind::Platform, "Consul", ":8300"),
        ("/version", Kind::Platform, "Docker API", "ApiVersion"),
    ] {
        let relevant = matches!((path, port), ("/identity", 32400) | ("/System/Info/Public", 8096) | ("/v1/status/leader", 8500) | ("/version", 2375 | 2376));
        if relevant {
            if let Some(x) = http_get(&format!("{base}{path}"), t.io).filter(|x| x.body.contains(needle)) {
                svc.kind = kind;
                svc.name = name.to_string();
                svc.confidence = Confidence::High;
                if path == "/version" {
                    svc.risk = Some("the Docker API is open without a password: anyone on the network can run programs on this machine".into());
                    svc.detail = json(&x).map(|v| format!("Docker {}", v["Version"].as_str().unwrap_or("?"))).unwrap_or_default();
                }
                return svc;
            }
        }
    }
    // cameras by what their web page and headers say, then by ONVIF
    let cam = camera_hint(h);
    let onvif = if cam.is_some() || matches!(port, 80 | 8000 | 8080 | 8899 | 2020) {
        http_get(&format!("{base}/onvif/device_service"), t.io).filter(|x| x.status != 404 && (x.body.to_ascii_lowercase().contains("soap") || x.body.to_ascii_lowercase().contains("onvif")))
    } else {
        None
    };
    if cam.is_some() || onvif.is_some() {
        svc.kind = Kind::Camera;
        svc.confidence = if onvif.is_some() { Confidence::High } else { Confidence::Medium };
        svc.name = match (&cam, onvif.is_some()) {
            (Some(w), true) => format!("IP camera / recorder with ONVIF ({w})"),
            (None, true) => "ONVIF camera or recorder".to_string(),
            (Some(w), false) => format!("probably a camera or recorder ({w})"),
            _ => unreachable!(),
        };
        if h.status == 401 || !h.header("www-authenticate").is_empty() {
            svc.detail = format!("{} · asks for a password", svc.detail);
        }
        return svc;
    }
    // a router or printer page behind a login
    if lower_title.contains("router") || lower_title.contains("gateway") || lower_title.contains("modem") {
        svc.kind = Kind::Device;
        svc.name = "router or modem admin page".into();
        return svc;
    }
    if lower_title.contains("printer") || lower_body.contains("print server") {
        svc.kind = Kind::Printer;
        svc.name = "printer web page".into();
        return svc;
    }
    // the port number only names a service that shows nothing of its own
    if let Some((k, n)) = port_hint(port).filter(|(k, _)| *k != Kind::Web).filter(|_| title.is_empty() && server.is_empty()) {
        svc.kind = k;
        svc.name = format!("{n} (by port number)");
        svc.confidence = Confidence::Low;
        return svc;
    }
    svc.name = if title.is_empty() { format!("Web server{}", if server.is_empty() { String::new() } else { format!(" ({server})") }) } else { format!("Web: {title}") };
    svc
}
