//! What a scan finds.

use std::net::Ipv4Addr;

/// What kind of thing a service is, for grouping and icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Ollama.
    Ollama,
    /// Another server that answers the OpenAI chat API (LM Studio, llama.cpp, vLLM…).
    LlmServer,
    /// A camera, a recorder or a video stream.
    Camera,
    /// A media server, a smart TV, a speaker.
    Media,
    /// A printer or scanner.
    Printer,
    /// A home automation hub or a broker.
    SmartHome,
    /// A database, a search or vector store, a queue.
    Data,
    /// A router, a switch, a NAS or another box with an admin page.
    Device,
    /// A way to log in or share files (SSH, SMB, FTP, telnet).
    Access,
    /// A container or cluster API (Docker, Consul…).
    Platform,
    /// A web server.
    Web,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ollama => "Ollama",
            Kind::LlmServer => "Model server",
            Kind::Camera => "Camera",
            Kind::Media => "Media",
            Kind::Printer => "Printer",
            Kind::SmartHome => "Smart home",
            Kind::Data => "Data",
            Kind::Device => "Device",
            Kind::Access => "Remote access",
            Kind::Platform => "Platform",
            Kind::Web => "Web",
            Kind::Other => "Other",
        }
    }

    /// Is this something that can provide AI models?
    pub fn is_model_server(self) -> bool {
        matches!(self, Kind::Ollama | Kind::LlmServer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Only the port number says so.
    Low,
    /// The service answered like this kind of thing.
    Medium,
    /// The service named itself.
    High,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub port: u16,
    /// `tcp` or `udp`.
    pub proto: &'static str,
    pub kind: Kind,
    /// What it is: "Ollama 0.5.7", "Hikvision camera", "OpenSSH 9.6".
    pub name: String,
    /// More: the page title, the server header, the models it holds.
    pub detail: String,
    pub confidence: Confidence,
    /// A reason to look at it twice (an open Docker API, a Redis without a password).
    pub risk: Option<String>,
    /// For model servers: the model names it listed.
    pub models: Vec<String>,
    /// An address that opens it: `http://192.168.1.20:11434`, `rtsp://192.168.1.30:554/`.
    pub url: Option<String>,
}

impl Service {
    pub fn new(port: u16, kind: Kind, name: impl Into<String>) -> Service {
        Service { port, proto: "tcp", kind, name: name.into(), detail: String::new(), confidence: Confidence::Medium, risk: None, models: vec![], url: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub ip: Ipv4Addr,
    pub mac: Option<String>,
    /// A name the machine gave (mDNS or UPnP).
    pub name: Option<String>,
    pub services: Vec<Service>,
}

impl HostInfo {
    pub fn new(ip: Ipv4Addr) -> HostInfo {
        HostInfo { ip, mac: None, name: None, services: vec![] }
    }

    /// The most interesting kind on this host (model servers and cameras first).
    pub fn main_kind(&self) -> Kind {
        self.services.iter().map(|s| s.kind).filter(|k| !matches!(k, Kind::Web | Kind::Other)).min().or_else(|| self.services.first().map(|s| s.kind)).unwrap_or(Kind::Other)
    }

    pub fn has(&self, k: Kind) -> bool {
        self.services.iter().any(|s| s.kind == k)
    }

    pub fn risky(&self) -> bool {
        self.services.iter().any(|s| s.risk.is_some())
    }

    /// Add a service, or improve what is known about the same port (the better answer wins).
    pub fn add(&mut self, s: Service) {
        match self.services.iter_mut().find(|x| x.port == s.port && x.proto == s.proto) {
            Some(old) => {
                if s.confidence >= old.confidence {
                    *old = s;
                }
            }
            None => self.services.push(s),
        }
        self.services.sort_by_key(|s| (s.port, s.proto));
    }
}
