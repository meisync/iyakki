//! Model host on your network: a computer on the same network (a Mac with Apple Silicon first, or any PC with a GPU) that runs
//! a model server and answers OpenAI-style requests. IYAKKI never installs anything on that machine: the person does, with
//! the commands this module writes out, and IYAKKI only talks HTTP to the address they type.
//!
//! * [`parse_target`]: the address and port the person typed, accepted only when it is on the local network (private,
//!   link-local or loopback addresses; `.local`, `.lan` and one-word names) unless the person confirms an outside address.
//! * [`test`]: `GET /v1/models`, then a tiny streamed chat, with short timeouts and bounded answers; tokens per second.
//! * [`add_to_models`] / [`Hosts`]: the provider `mac-<nickname>` (key in the vault) and the list of hosts kept for the page.
//! * [`scan_targets`] / [`scan`]: look for model servers on the local /24 on well-known ports, only when the person clicks.
//! * [`service_manifest`], [`publish`], [`test_from_cluster`]: a Service + Endpoints in the namespace `iyakki-ai` so pods
//!   call `http://mac-<nickname>:<port>`, and a pod that checks it (managed k3s only).
//!
//! Metal facts, the security text and the memory table are here too, so the page and `docs/MAC-MODEL-HOST.md` say the same.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::{Kind, ProviderCfg};
use crate::wsexec::Out;

/// The namespace of the models and AI services of IYAKKI's cluster (Lemonade lives here too).
pub const NAMESPACE: &str = "iyakki-ai";
/// The label a Mac host carries in Models.
pub const MAC_LABEL: &str = "remote GPU: Apple Silicon (Metal)";
pub const PC_LABEL: &str = "remote GPU: model host on your network";
/// The words a person types to allow an address outside their network.
pub const PUBLIC_WORD: &str = "PUBLIC";

// ------------------------------------------------------------------------------------------------ presets and servers

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    /// A Mac with Apple Silicon (M1 to M-series): the GPU is used through Metal.
    Mac,
    /// Any other computer with a GPU (NVIDIA, AMD, Intel) running a model server.
    Pc,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Mac, Preset::Pc];

    pub fn title(self) -> &'static str {
        match self {
            Preset::Mac => "A Mac (Apple Silicon)",
            Preset::Pc => "Another computer with a GPU",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Mac => MAC_LABEL,
            Preset::Pc => PC_LABEL,
        }
    }

    /// The start of the provider and Service name: `mac-studio`, `host-desktop`.
    pub fn prefix(self) -> &'static str {
        match self {
            Preset::Mac => "mac",
            Preset::Pc => "host",
        }
    }

    pub fn machine(self) -> &'static str {
        match self {
            Preset::Mac => "Mac",
            Preset::Pc => "computer",
        }
    }
}

/// The model servers the person can run on the other machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerApp {
    Ollama,
    LmStudio,
    LlamaCpp,
    MlxLm,
}

impl ServerApp {
    pub const ALL: [ServerApp; 4] = [ServerApp::Ollama, ServerApp::LmStudio, ServerApp::LlamaCpp, ServerApp::MlxLm];

    pub fn title(self) -> &'static str {
        match self {
            ServerApp::Ollama => "Ollama",
            ServerApp::LmStudio => "LM Studio (server mode)",
            ServerApp::LlamaCpp => "llama.cpp (llama-server)",
            ServerApp::MlxLm => "MLX-LM server",
        }
    }

    pub fn licence(self) -> &'static str {
        match self {
            ServerApp::Ollama => "MIT",
            ServerApp::LmStudio => "free to use, but proprietary (not open source): LM Studio's own terms apply",
            ServerApp::LlamaCpp => "MIT",
            ServerApp::MlxLm => "MIT (Apple's MLX framework: MIT)",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            ServerApp::Ollama => 11434,
            ServerApp::LmStudio => 1234,
            ServerApp::LlamaCpp | ServerApp::MlxLm => 8080,
        }
    }

    /// What to do on the other machine, step by step (the person does it; IYAKKI never installs on it).
    pub fn steps(self, preset: Preset) -> Vec<String> {
        let mac = preset == Preset::Mac;
        let port = self.default_port();
        let mut v: Vec<String> = Vec::new();
        match self {
            ServerApp::Ollama => {
                v.push(if mac { "1. Install Ollama from ollama.com (the app), or in Terminal: brew install ollama".into() } else { "1. Install Ollama from ollama.com (Windows, Linux or macOS installers)".into() });
                if mac {
                    v.push("2. Make it listen on your network, not only on this Mac. In Terminal, once: launchctl setenv OLLAMA_HOST \"0.0.0.0:11434\"  then quit and start the Ollama app again. Or without the app: OLLAMA_HOST=0.0.0.0:11434 ollama serve".into());
                } else {
                    v.push("2. Make it listen on your network: set OLLAMA_HOST=0.0.0.0:11434 (Linux: systemctl edit ollama, add Environment=\"OLLAMA_HOST=0.0.0.0:11434\"; Windows: a user environment variable), then restart Ollama".into());
                }
                v.push("3. Download a model that fits (see the memory table): ollama pull <model>, for example ollama pull llama3.2".into());
            }
            ServerApp::LmStudio => {
                v.push("1. Install LM Studio from lmstudio.ai (free to use; the program is proprietary) and download a model in it".into());
                v.push("2. Open the Developer tab, turn on \"Serve on Local Network\", then Start the server (port 1234). Or in Terminal: lms server start --port 1234 --bind 0.0.0.0".into());
                v.push("3. Load the model you want (the Developer tab, or lms load <model>); the server answers for the loaded models".into());
            }
            ServerApp::LlamaCpp => {
                v.push(if mac { "1. In Terminal: brew install llama.cpp (Metal is used automatically on Apple Silicon)".into() } else { "1. Install llama.cpp from github.com/ggml-org/llama.cpp (release builds: CUDA, HIP, Vulkan or CPU)".into() });
                v.push("2. Start the server on your network: llama-server -hf <user>/<repo>:<quant> --host 0.0.0.0 --port 8080 -ngl 99   (or -m /path/to/model.gguf instead of -hf)".into());
                v.push("3. Optional but recommended: add --api-key <a long secret> and type the same key here".into());
            }
            ServerApp::MlxLm => {
                v.push("1. In Terminal: python3 -m venv mlx-env && source mlx-env/bin/activate && pip install mlx-lm (Apple Silicon only)".into());
                v.push("2. Start the server on your network: mlx_lm.server --model mlx-community/<model> --host 0.0.0.0 --port 8080".into());
                v.push("3. mlx_lm.server has no login at all: use the macOS firewall or a reverse proxy with a key (see the security note)".into());
            }
        }
        v.push(if mac {
            format!("Then find the Mac's address: System Settings > Network (or Terminal: ipconfig getifaddr en0; its name is scutil --get LocalHostName, plus .local). Type it here with port {port}.")
        } else {
            format!("Then find the computer's address on your network (ip addr, ipconfig) and type it here with port {port}.")
        });
        v
    }

    /// A tighter command that binds to one address instead of every network card.
    pub fn bind_tip(self) -> &'static str {
        match self {
            ServerApp::Ollama => "Use OLLAMA_HOST=<the Mac's network address>:11434 instead of 0.0.0.0 to listen on that one network only.",
            ServerApp::LmStudio => "LM Studio listens on the local network only when \"Serve on Local Network\" is on; turn it off when you are away from home.",
            ServerApp::LlamaCpp | ServerApp::MlxLm => "Use --host <the Mac's network address> instead of 0.0.0.0 to listen on that one network only.",
        }
    }
}

/// The security warning shown beside the commands.
pub const SECURITY: &str = "These servers have no login by default: anyone on your network who can reach the port can use your Mac's GPU and read what you send. Bind them to your network address (not to every interface), switch on the macOS firewall (System Settings > Network > Firewall) and allow only the program, use an API key where the server has one (llama-server --api-key, LM Studio's API token), or put a reverse proxy with a key in front. Never forward the port on your router.";

/// What is true about the GPU path, in one place.
pub const METAL_FACTS: [&str; 5] = [
    "The model runs on the Mac's GPU through Metal on macOS; IYAKKI only talks to the server's HTTP address.",
    "Nothing leaves your network: the chat goes from IYAKKI to the Mac and back. The server may download models from its own source when you pull them.",
    "If IYAKKI runs as a virtual machine on that same Mac, use the address of the host as the virtual machine sees it (the VM's gateway address); the virtual machine itself cannot use Metal.",
    "Speed depends on the model, its quantisation, the chip and the memory bandwidth: the Test button measures tokens per second for this model.",
    "Unified memory limits the model size: model file + context (1 to 4 GB) must fit in the Mac's memory minus about 4 GB for macOS and your other programs.",
];

/// Guidance by memory: (GB of RAM, what usually fits). Quantised (4-bit) models, text only; a guide, not a promise.
pub const RAM_TABLE: [(u32, &str); 6] = [
    (8, "up to about 3 to 4 GB of model: 3B to 4B models (4-bit). Close other programs."),
    (16, "up to about 10 GB of model: 7B to 14B models (4-bit) with a normal context."),
    (24, "up to about 18 GB of model: 14B to 24B models (4-bit)."),
    (32, "up to about 26 GB of model: 30B-class models (4-bit) with a long context."),
    (64, "up to about 56 GB of model: 70B models (4-bit) or large mixture-of-experts models."),
    (128, "up to about 116 GB of model: 100B+ and the largest open models (4-bit)."),
];

/// Does a model of `model_gb` with `context_gb` of cache fit in a machine of `ram_gb`? (RAM minus 4 GB for the system.)
pub fn fits(ram_gb: f64, model_gb: f64, context_gb: f64) -> bool {
    model_gb + context_gb <= ram_gb - 4.0
}

/// The table as lines of text.
pub fn ram_table_lines() -> Vec<String> {
    RAM_TABLE.iter().map(|(gb, t)| format!("{gb} GB: {t}")).collect()
}

// ------------------------------------------------------------------------------------------------ addresses

/// Where a model host answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// An IP address or a name, as typed (lower case).
    pub host: String,
    pub port: u16,
}

impl Target {
    /// `http://host:port/v1` (IPv6 in brackets).
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.authority())
    }

    pub fn authority(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }
}

/// Is this address on the local network: private, link-local or loopback.
pub fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_link_local() || a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || (a.segments()[0] & 0xffc0) == 0xfe80 || (a.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// A name that normally stays on the local network: `.local` (Bonjour), `.lan`, `.home.arpa`, `.internal`, or one word.
pub fn is_local_name(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    !h.is_empty() && (!h.contains('.') || h.ends_with(".local") || h.ends_with(".lan") || h.ends_with(".home.arpa") || h.ends_with(".internal") || h.ends_with(".localdomain"))
}

fn valid_host_name(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Read what the person typed: `192.168.1.20`, `192.168.1.20:11434`, `http://studio.local:8080/v1`, `[fe80::1]:1234`. `default_port` is
/// used when none is typed. Only local-network addresses are accepted unless `confirm_public` (the person typed the confirmation).
pub fn parse_target(input: &str, default_port: u16, confirm_public: bool) -> Result<Target, String> {
    let mut t = input.trim();
    if t.is_empty() {
        return Err("Type the address of the machine, for example 192.168.1.20 or studio.local.".into());
    }
    if let Some(r) = t.strip_prefix("https://") {
        let _ = r;
        return Err("Use http:// (these servers speak plain HTTP on your own network). If you front it with a TLS proxy, add it in Settings > Models > Add instead.".into());
    }
    if let Some(r) = t.strip_prefix("http://") {
        t = r;
    }
    let t = t.split(['/', '?', '#']).next().unwrap_or("");
    if t.contains('@') {
        return Err("Type only the address, without a user name.".into());
    }
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or("A bracketed IPv6 address needs a closing ].")?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return Err("Nothing may follow the closing ] except :port.".into()),
        };
        (h.to_string(), port)
    } else if t.matches(':').count() > 1 {
        (t.to_string(), None) // a bare IPv6 address
    } else {
        match t.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (t.to_string(), None),
        }
    };
    let port = match port {
        None => default_port,
        Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or("The port is a number from 1 to 65535.")?,
    };
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_string();
    let local = if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_unspecified() || ip.is_multicast() {
            return Err("That is not the address of a machine (0.0.0.0 means every interface, here type the machine's own address).".into());
        }
        is_local_ip(ip)
    } else if valid_host_name(&host) {
        is_local_name(&host)
    } else {
        return Err("That does not look like an address or a machine name.".into());
    };
    if !local && !confirm_public {
        return Err(format!("{host} is not on your local network. IYAKKI only talks to machines on private networks unless you type {PUBLIC_WORD} in the confirmation field: your prompts would then cross the internet without encryption."));
    }
    Ok(Target { host, port })
}

/// The socket addresses of a target; a name must resolve to local addresses only (unless confirmed). Never resolves a numeric address.
pub fn resolve(t: &Target, confirm_public: bool) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = (t.host.as_str(), t.port).to_socket_addrs().map_err(|_| format!("{} did not resolve to an address. Check the name, or type the address.", t.host))?.collect();
    if addrs.is_empty() {
        return Err(format!("{} did not resolve to an address.", t.host));
    }
    if !confirm_public && addrs.iter().any(|a| !is_local_ip(a.ip())) {
        return Err(format!("{} resolves to an address outside your network. Type {PUBLIC_WORD} in the confirmation field to allow it.", t.host));
    }
    Ok(addrs)
}

// ------------------------------------------------------------------------------------------------ small HTTP client

#[derive(Clone, Debug)]
pub struct Limits {
    pub connect: Duration,
    /// For the whole request, answer included.
    pub total: Duration,
    pub max_body: usize,
}

impl Limits {
    pub fn models() -> Limits {
        Limits { connect: Duration::from_secs(3), total: Duration::from_secs(8), max_body: 1024 * 1024 }
    }
    pub fn chat() -> Limits {
        Limits { connect: Duration::from_secs(3), total: Duration::from_secs(120), max_body: 256 * 1024 }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NetErr {
    Refused,
    Timeout,
    TooLarge,
    Other(String),
}

#[derive(Debug)]
pub struct HttpResp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Milliseconds from the start of the request until the first body byte, and until the end.
    pub first_ms: u64,
    pub total_ms: u64,
}

impl HttpResp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Undo `Transfer-Encoding: chunked` (lenient: an unfinished last chunk is kept as far as it arrived).
pub fn decode_chunked(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let Some(nl) = raw[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = String::from_utf8_lossy(&raw[i..i + nl]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        i += nl + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(raw.len());
        out.extend_from_slice(&raw[i..end]);
        i = end + 2;
    }
    out
}

fn connect(addrs: &[SocketAddr], t: Duration) -> Result<TcpStream, NetErr> {
    let mut last = NetErr::Other("no address".into());
    for a in addrs {
        match TcpStream::connect_timeout(a, t) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => NetErr::Refused,
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetErr::Timeout,
                    _ => NetErr::Other(match e.raw_os_error() {
                        Some(113) | Some(101) => "no route to the machine (is it on this network and awake?)".to_string(),
                        _ => e.to_string(),
                    }),
                }
            }
        }
    }
    Err(last)
}

/// One HTTP/1.1 request (`Connection: close`), with every wait and the size of the answer bounded.
pub fn http(addrs: &[SocketAddr], host_header: &str, method: &str, path: &str, headers: &[(String, String)], body: Option<&[u8]>, lim: &Limits) -> Result<HttpResp, NetErr> {
    let start = Instant::now();
    let deadline = start + lim.total;
    let mut s = connect(addrs, lim.connect)?;
    let _ = s.set_nodelay(true);
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: iyakki-modelhost\r\nAccept: */*\r\nConnection: close\r\n");
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            return Err(NetErr::Other("bad header".into()));
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(req.as_bytes()).map_err(|e| NetErr::Other(e.to_string()))?;
    if let Some(b) = body {
        s.write_all(b).map_err(|e| NetErr::Other(e.to_string()))?;
    }
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut head_end: Option<usize> = None;
    let mut first_ms = 0u64;
    let mut status = 0u16;
    let mut hdrs: Vec<(String, String)> = Vec::new();
    let (mut chunked, mut clen) = (false, None::<usize>);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(NetErr::Timeout);
        }
        s.set_read_timeout(Some(left.max(Duration::from_millis(1)))).ok();
        let n = match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(NetErr::Timeout),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && head_end.is_some() => break,
            Err(e) => return Err(NetErr::Other(e.to_string())),
        };
        raw.extend_from_slice(&buf[..n]);
        if head_end.is_none() {
            if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..p]).into_owned();
                let mut lines = head.split("\r\n");
                let first = lines.next().unwrap_or("");
                if !first.starts_with("HTTP/") {
                    return Err(NetErr::Other("the answer is not HTTP".into()));
                }
                status = first.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| NetErr::Other("the answer is not HTTP".into()))?;
                for l in lines {
                    if let Some((k, v)) = l.split_once(':') {
                        hdrs.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                chunked = hdrs.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
                clen = hdrs.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok());
                if clen.is_some_and(|c| c > lim.max_body) {
                    return Err(NetErr::TooLarge);
                }
                head_end = Some(p + 4);
            }
        }
        match head_end {
            None => {
                if raw.len() > 64 * 1024 {
                    return Err(NetErr::TooLarge);
                }
            }
            Some(h) => {
                let have = raw.len() - h;
                if have > 0 && first_ms == 0 {
                    first_ms = start.elapsed().as_millis() as u64;
                }
                if have > lim.max_body + 64 * 1024 {
                    return Err(NetErr::TooLarge);
                }
                if let Some(c) = clen {
                    if have >= c {
                        break;
                    }
                } else if chunked && raw[h..].ends_with(b"0\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let Some(h) = head_end else { return Err(NetErr::Other("the connection closed before an answer".into())) };
    let mut body = if chunked { decode_chunked(&raw[h..]) } else { raw[h..].to_vec() };
    if let Some(c) = clen {
        body.truncate(c);
    }
    if body.len() > lim.max_body {
        return Err(NetErr::TooLarge);
    }
    Ok(HttpResp { status, headers: hdrs, body, first_ms, total_ms: start.elapsed().as_millis() as u64 })
}

// ------------------------------------------------------------------------------------------------ the test

#[derive(Debug, PartialEq, Eq)]
pub enum TestError {
    Refused(String),
    Timeout(String),
    /// 401 or 403.
    Unauthorized,
    NotOpenAi(String),
    BadJson,
    TooLarge,
    Other(String),
}

impl TestError {
    pub fn message(&self) -> String {
        match self {
            TestError::Refused(a) => format!("Nothing is listening at {a}. Is the server running, and does it listen on the network (not only 127.0.0.1)? Check the port and the macOS firewall."),
            TestError::Timeout(a) => format!("{a} did not answer in time. Is the machine awake and on this network? A firewall that drops packets looks like this."),
            TestError::Unauthorized => "The server answered 401: it wants an API key. Type the key in the Key field (it is stored in the system vault).".into(),
            TestError::NotOpenAi(w) => format!("Something answers there, but it is not an OpenAI-compatible model server ({w}). Check the port and that the server's OpenAI API is on."),
            TestError::BadJson => "The server's model list is not valid JSON. It may be a different program on that port.".into(),
            TestError::TooLarge => "The answer was far larger than a model list should be, so it was dropped. This is probably not a model server.".into(),
            TestError::Other(e) => e.clone(),
        }
    }
}

fn net_error(e: NetErr, t: &Target) -> TestError {
    match e {
        NetErr::Refused => TestError::Refused(t.authority()),
        NetErr::Timeout => TestError::Timeout(t.authority()),
        NetErr::TooLarge => TestError::TooLarge,
        NetErr::Other(m) => TestError::Other(format!("Could not talk to {}: {m}", t.authority())),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatStats {
    pub tokens: u64,
    /// The server counted them (usage); otherwise they are counted from the stream or estimated from the text.
    pub exact: bool,
    pub text: String,
    /// Milliseconds until the first answer bytes arrived, and until the end.
    pub first_ms: u64,
    pub total_ms: u64,
    pub tokens_per_s: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestReport {
    pub models: Vec<String>,
    pub default_model: Option<String>,
    pub list_ms: u64,
    pub chat: Option<ChatStats>,
    /// Why the chat did not work, when the model list did.
    pub chat_error: Option<String>,
}

impl TestReport {
    /// One or two lines for the page.
    pub fn summary(&self) -> String {
        let mut s = format!("Reachable: {} model(s) listed in {} ms", self.models.len(), self.list_ms);
        match (&self.chat, &self.chat_error) {
            (Some(c), _) => s.push_str(&format!("; {} answered {} tokens{} in {:.1} s, first words after {} ms{}", self.default_model.as_deref().unwrap_or("the model"), c.tokens, if c.exact { "" } else { " (counted)" }, c.total_ms as f64 / 1000.0, c.first_ms, c.tokens_per_s.map_or(String::new(), |t| format!(", about {t:.1} tokens/s")))),
            (None, Some(e)) => s.push_str(&format!("; the test chat failed: {e}")),
            (None, None) => {}
        }
        s
    }
}

/// Tokens per second over a window; `None` when the window is too short to mean anything.
pub fn tokens_per_second(tokens: u64, window_ms: u64) -> Option<f64> {
    (tokens > 0 && window_ms >= 5).then(|| tokens as f64 * 1000.0 / window_ms as f64)
}

/// The model ids of a `/v1/models` answer; `None` when it is not one.
pub fn parse_models(body: &[u8]) -> Result<Vec<String>, TestError> {
    let text = String::from_utf8_lossy(body);
    let t = text.trim_start();
    let v: Value = match serde_json::from_str(t) {
        Ok(v) => v,
        Err(_) if t.starts_with('<') || t.is_empty() => return Err(TestError::NotOpenAi("the answer is a web page, not JSON".into())),
        Err(_) => return Err(TestError::BadJson),
    };
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| TestError::NotOpenAi("no \"data\" list in the answer".into()))?;
    Ok(data.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).filter(|id| crate::providers::valid_model_name(id)).take(200).map(str::to_string).collect())
}

/// Prefer a chat model: the first whose name does not say embedding, reranker, whisper or speech.
pub fn pick_default(models: &[String]) -> Option<String> {
    let bad = |m: &str| {
        let l = m.to_ascii_lowercase();
        ["embed", "rerank", "whisper", "tts", "bge-", "minilm"].iter().any(|b| l.contains(b))
    };
    models.iter().find(|m| !bad(m)).or_else(|| models.first()).cloned()
}

/// Read a chat completion answer: a streamed one (`data: {...}` lines) or a single JSON object.
pub fn parse_chat(body: &[u8], first_ms: u64, total_ms: u64) -> Result<ChatStats, String> {
    let text = String::from_utf8_lossy(body);
    let t = text.trim();
    let (mut reply, mut events, mut usage) = (String::new(), 0u64, None::<u64>);
    let streamed = t.starts_with("data:") || t.starts_with(':') || t.starts_with("event:");
    if streamed {
        for line in t.lines() {
            let Some(d) = line.strip_prefix("data:") else { continue };
            let d = d.trim();
            if d == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(d) else { continue };
            if let Some(e) = v.get("error") {
                return Err(error_text(e));
            }
            if let Some(u) = v.pointer("/usage/completion_tokens").and_then(Value::as_u64) {
                usage = Some(u);
            }
            let delta = v.pointer("/choices/0/delta");
            let piece = delta.and_then(|d| d.get("content")).and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| delta.and_then(|d| d.get("reasoning_content").or_else(|| d.get("reasoning"))).and_then(Value::as_str).filter(|s| !s.is_empty()));
            if let Some(p) = piece {
                events += 1;
                if delta.and_then(|d| d.get("content")).and_then(Value::as_str).is_some_and(|c| !c.is_empty()) && reply.len() < 400 {
                    reply.push_str(p);
                }
            }
        }
    } else {
        let v: Value = serde_json::from_str(t).map_err(|_| "the chat answer is not JSON".to_string())?;
        if let Some(e) = v.get("error") {
            return Err(error_text(e));
        }
        let c = v.pointer("/choices/0/message/content").and_then(Value::as_str).or_else(|| v.pointer("/choices/0/text").and_then(Value::as_str));
        let c = c.ok_or("the chat answer has no message (not an OpenAI chat completion)")?;
        reply = c.chars().take(400).collect();
        usage = v.pointer("/usage/completion_tokens").and_then(Value::as_u64);
        events = (c.chars().count() as u64).div_ceil(4);
    }
    let tokens = usage.unwrap_or(events);
    if tokens == 0 {
        return Err("the model answered with nothing".into());
    }
    // a streamed answer: time the words after the first one; otherwise the whole request
    let window = if streamed && tokens > 1 && total_ms > first_ms { Some(((tokens - 1) * 1000) as f64 / (total_ms - first_ms).max(1) as f64) } else { tokens_per_second(tokens, total_ms) };
    Ok(ChatStats { tokens, exact: usage.is_some(), text: reply.trim().to_string(), first_ms, total_ms, tokens_per_s: window })
}

fn error_text(e: &Value) -> String {
    let m = e.get("message").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("the server reported an error");
    m.chars().take(300).collect()
}

fn auth(key: Option<&str>) -> Vec<(String, String)> {
    key.filter(|k| !k.is_empty()).map(|k| vec![("Authorization".to_string(), format!("Bearer {k}"))]).unwrap_or_default()
}

/// Test a model host: its model list, then one tiny chat. `model` overrides the one picked from the list.
pub fn test(t: &Target, key: Option<&str>, model: Option<&str>, confirm_public: bool, lim_models: &Limits, lim_chat: &Limits) -> Result<TestReport, TestError> {
    let addrs = resolve(t, confirm_public).map_err(TestError::Other)?;
    let host_hdr = t.authority();
    let r = http(&addrs, &host_hdr, "GET", "/v1/models", &auth(key), None, lim_models).map_err(|e| net_error(e, t))?;
    match r.status {
        401 | 403 => return Err(TestError::Unauthorized),
        200..=299 => {}
        s => return Err(TestError::NotOpenAi(format!("HTTP {s} for /v1/models"))),
    }
    let models = parse_models(&r.body)?;
    let default_model = model.map(str::to_string).or_else(|| pick_default(&models));
    let mut report = TestReport { models, default_model: default_model.clone(), list_ms: r.total_ms, chat: None, chat_error: None };
    let Some(m) = default_model else {
        report.chat_error = Some("the server lists no model; load or pull one first".into());
        return Ok(report);
    };
    let body = serde_json::json!({
        "model": m,
        "messages": [{"role": "user", "content": "Reply with one short sentence: what is 2 + 2?"}],
        "max_tokens": 48, "temperature": 0, "stream": true, "stream_options": {"include_usage": true}
    });
    match http(&addrs, &host_hdr, "POST", "/v1/chat/completions", &auth(key), Some(body.to_string().as_bytes()), lim_chat) {
        Ok(c) if (200..300).contains(&c.status) => match parse_chat(&c.body, c.first_ms, c.total_ms) {
            Ok(s) => report.chat = Some(s),
            Err(e) => report.chat_error = Some(e),
        },
        Ok(c) if c.status == 401 || c.status == 403 => return Err(TestError::Unauthorized),
        Ok(c) => {
            let why = serde_json::from_slice::<Value>(&c.body).ok().and_then(|v| v.get("error").map(error_text)).unwrap_or_default();
            report.chat_error = Some(format!("HTTP {}{}", c.status, if why.is_empty() { String::new() } else { format!(": {why}") }));
        }
        Err(NetErr::Timeout) => report.chat_error = Some("the model did not answer in time (a big model can need a minute to load the first time: try again)".into()),
        Err(e) => report.chat_error = Some(format!("{:?}", e)),
    }
    Ok(report)
}

// ------------------------------------------------------------------------------------------------ the list of hosts

/// A model host the person added. Kept in `model-hosts.json` in the data folder; the provider itself is in Models.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// The provider id and the Service name: `mac-studio`.
    pub id: String,
    /// The nickname as typed.
    pub nickname: String,
    pub preset: Preset,
    pub address: String,
    pub port: u16,
    /// A Service and Endpoints for pods exist in the cluster (or were asked for).
    #[serde(default)]
    pub in_cluster: bool,
    /// The person confirmed an address outside the local network.
    #[serde(default)]
    pub public_ok: bool,
}

impl Host {
    pub fn target(&self) -> Target {
        Target { host: self.address.clone(), port: self.port }
    }
    pub fn label(&self) -> &'static str {
        self.preset.label()
    }
    /// What pods call.
    pub fn cluster_url(&self) -> String {
        format!("http://{}:{}/v1", self.id, self.port)
    }
}

pub struct Hosts {
    path: PathBuf,
}

impl Hosts {
    pub fn new(data_dir: &Path) -> Hosts {
        Hosts { path: data_dir.join("model-hosts.json") }
    }

    pub fn list(&self) -> Vec<Host> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Host> {
        self.list().into_iter().find(|h| h.id == id)
    }

    pub fn upsert(&self, h: Host) -> Result<(), String> {
        let mut all = self.list();
        match all.iter_mut().find(|x| x.id == h.id) {
            Some(s) => *s = h,
            None => all.push(h),
        }
        self.save(&all)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut all = self.list();
        all.retain(|h| h.id != id);
        self.save(&all)
    }

    fn save(&self, all: &[Host]) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

/// The provider id for a nickname: `mac-<nickname>` with letters, digits and `-`; at most 32 characters.
pub fn provider_id(preset: Preset, nickname: &str) -> Result<String, String> {
    let mut slug = String::new();
    for c in nickname.trim().to_ascii_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if c == '-' && (slug.is_empty() || slug.ends_with('-')) {
            continue;
        }
        slug.push(c);
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        return Err("Give the machine a nickname with letters or digits, for example studio.".into());
    }
    let id: String = format!("{}-{slug}", preset.prefix()).chars().take(32).collect();
    let id = id.trim_end_matches('-').to_string();
    if !crate::providers::valid_provider_id(&id) {
        return Err("The nickname makes an invalid provider name; use letters and digits.".into());
    }
    Ok(id)
}

/// What the person filled in.
#[derive(Clone, Debug)]
pub struct NewHost {
    pub nickname: String,
    pub preset: Preset,
    pub address: String,
    pub default_port: u16,
    pub key: String,
    pub confirm: String,
    /// A model to use instead of the first one listed.
    pub model: Option<String>,
}

impl NewHost {
    pub fn confirm_public(&self) -> bool {
        self.confirm.trim() == PUBLIC_WORD
    }
    pub fn target(&self) -> Result<Target, String> {
        parse_target(&self.address, self.default_port, self.confirm_public())
    }
}

/// Register the host in Models as `mac-<nickname>` (an OpenAI-compatible provider) with its key in the system vault, and keep it in
/// the list of hosts. An existing provider of that name that is not a host of this list is never overwritten. `report` is the
/// result of a successful test (the models and the default one).
pub fn add_to_models(svc: &crate::services::Services, n: &NewHost, report: &TestReport) -> Result<Host, String> {
    let target = n.target()?;
    let id = provider_id(n.preset, &n.nickname)?;
    let hosts = Hosts::new(svc.dir());
    let existing = hosts.get(&id);
    {
        let st = svc.settings.lock().unwrap();
        if st.get().provider(&id).is_some() && existing.is_none() {
            return Err(format!("A provider named {id} already exists in Models and is not a model host; pick another nickname."));
        }
    }
    let mut cfg = ProviderCfg::new(&id, Kind::Compat);
    cfg.base_url = Some(target.base_url());
    cfg.models = report.models.clone();
    cfg.default_model = n.model.clone().or_else(|| report.default_model.clone());
    if !n.key.is_empty() {
        cfg.key_ref = Some(format!("vault:{id}-key"));
    } else if let Some(old) = svc.settings.lock().unwrap().get().provider(&id) {
        cfg.key_ref = old.key_ref.clone(); // saving again without typing the key keeps the stored one
    }
    cfg.validate()?;
    if !n.key.is_empty() {
        svc.system_set(&format!("{id}-key"), &n.key).map_err(|e| match e {
            crate::vault::VaultError::Locked => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            other => other.to_string(),
        })?;
    }
    svc.settings.lock().unwrap().upsert_provider(cfg).map_err(|e| e.to_string())?;
    let host = Host { id: id.clone(), nickname: n.nickname.trim().to_string(), preset: n.preset, address: target.host.clone(), port: target.port, in_cluster: existing.as_ref().is_some_and(|h| h.in_cluster), public_ok: n.confirm_public() };
    hosts.upsert(host.clone())?;
    Ok(host)
}

/// Take the host out of Models and the list (the vault key stays until the person deletes it there).
pub fn remove_from_models(svc: &crate::services::Services, id: &str) -> Result<(), String> {
    let hosts = Hosts::new(svc.dir());
    if hosts.get(id).is_none() {
        return Err(format!("{id} is not a model host."));
    }
    let _ = svc.settings.lock().unwrap().remove_provider(id);
    hosts.remove(id)
}

// ------------------------------------------------------------------------------------------------ scan

/// The ports model servers use by default: Ollama, llama.cpp / MLX-LM, LM Studio, vLLM and others.
pub const SCAN_PORTS: [u16; 4] = [11434, 8080, 1234, 8000];

/// A name for what usually listens on a port.
pub fn port_hint(port: u16) -> &'static str {
    match port {
        11434 => "Ollama",
        8080 => "llama.cpp or MLX-LM",
        1234 => "LM Studio",
        8000 => "vLLM or another server",
        _ => "a model server",
    }
}

/// The hosts one scan may touch: the /24 around this machine's address (a smaller network only as far as it goes), without this
/// machine, at most 254. A public address gives nothing. Never the network or broadcast address.
pub fn scan_targets(own: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if !(own.is_private() || own.is_link_local()) {
        return Vec::new();
    }
    let prefix = prefix.clamp(24, 30);
    let mask: u32 = u32::MAX << (32 - prefix as u32);
    let net = u32::from(own) & mask;
    let size = !mask;
    (1..size).map(|i| Ipv4Addr::from(net + i)).filter(|ip| *ip != own).collect()
}

/// This machine's network cards that look like a LAN (not container bridges), as (address, prefix).
pub fn lan_interfaces() -> Vec<(Ipv4Addr, u8, String)> {
    iyakki_netscan::net::local_subnets().into_iter().filter(|s| !["cni", "flannel", "veth", "docker", "br-", "podman", "virbr", "cali", "kube", "lo"].iter().any(|p| s.iface.starts_with(p))).map(|s| (s.addr, s.prefix, s.iface)).collect()
}

/// What a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub ip: IpAddr,
    pub port: u16,
    pub hint: &'static str,
    /// The OpenAI model list answered (true) or the server wants a key (false).
    pub open: bool,
    pub models: usize,
}

impl Found {
    pub fn line(&self) -> String {
        format!("{}:{}  {}  {}", self.ip, self.port, self.hint, if self.open { format!("{} model(s)", self.models) } else { "wants an API key".into() })
    }
}

#[derive(Clone, Debug)]
pub struct ScanOpts {
    pub ports: Vec<u16>,
    pub connect: Duration,
    pub read: Duration,
    /// At most this many hosts are probed at once.
    pub concurrency: usize,
}

impl Default for ScanOpts {
    fn default() -> ScanOpts {
        ScanOpts { ports: SCAN_PORTS.to_vec(), connect: Duration::from_millis(300), read: Duration::from_millis(1500), concurrency: 32 }
    }
}

/// Probe `targets` on the ports of `opts`: a connection, then `GET /v1/models` on the ports that opened. Only private, link-local and
/// loopback addresses are touched (anything else is skipped and never connected to). Stops early when `cancel` is set.
pub fn scan(targets: &[IpAddr], opts: &ScanOpts, cancel: &AtomicBool) -> Vec<Found> {
    let work: Vec<IpAddr> = targets.iter().copied().filter(|ip| is_local_ip(*ip)).take(254).collect();
    let next = AtomicUsize::new(0);
    let found = std::sync::Mutex::new(Vec::<Found>::new());
    let workers = worker_count(opts, work.len());
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(ip) = work.get(i) else { break };
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                for port in &opts.ports {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Some(f) = probe(*ip, *port, opts) {
                        found.lock().unwrap().push(f);
                    }
                }
            });
        }
    });
    let mut v = found.into_inner().unwrap_or_default();
    v.sort_by_key(|f| (f.ip, f.port));
    v
}

/// How many hosts are probed at once: the option, at most 64, at most one per host.
pub fn worker_count(opts: &ScanOpts, hosts: usize) -> usize {
    opts.concurrency.clamp(1, 64).min(hosts.max(1))
}

fn probe(ip: IpAddr, port: u16, opts: &ScanOpts) -> Option<Found> {
    let addr = SocketAddr::new(ip, port);
    // a closed or filtered port costs at most the connect timeout; nothing is sent to it
    drop(TcpStream::connect_timeout(&addr, opts.connect).ok()?);
    let lim = Limits { connect: opts.connect.max(Duration::from_millis(500)), total: opts.read, max_body: 256 * 1024 };
    let r = http(&[addr], &Target { host: ip.to_string(), port }.authority(), "GET", "/v1/models", &[], None, &lim).ok()?;
    match r.status {
        401 | 403 => Some(Found { ip, port, hint: port_hint(port), open: false, models: 0 }),
        200..=299 => parse_models(&r.body).ok().map(|m| Found { ip, port, hint: port_hint(port), open: true, models: m.len() }),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ the cluster

/// Runs `kubectl` against the cluster IYAKKI manages (and no other).
pub trait Kube {
    fn kubectl(&self, args: &[&str]) -> Out;
}

impl Kube for crate::k3sdev::K3sDev {
    fn kubectl(&self, args: &[&str]) -> Out {
        crate::k3sdev::K3sDev::kubectl(self, args)
    }
}

fn is_ip(s: &str) -> Option<IpAddr> {
    s.parse().ok()
}

/// The objects that let pods call the host as `http://mac-<nickname>:<port>`: the namespace, and for an address a Service without a
/// selector with Endpoints that point at it; for a name an ExternalName Service (pods then use the host's own port).
pub fn service_manifest(h: &Host) -> String {
    let labels = format!("    app.kubernetes.io/managed-by: iyakki\n    iyakki.dev/model-host: {}\n", h.id);
    let ns = format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NAMESPACE}\n  labels:\n    app.kubernetes.io/managed-by: iyakki\n");
    match is_ip(&h.address) {
        Some(ip) => {
            let fam = if ip.is_ipv6() { "\n  ipFamilies: [IPv6]" } else { "" };
            format!(
                "{ns}---\napiVersion: v1\nkind: Service\nmetadata:\n  name: {id}\n  namespace: {NAMESPACE}\n  labels:\n{labels}  annotations:\n    iyakki.dev/note: \"{note}\"\nspec:\n  type: ClusterIP{fam}\n  ports:\n    - name: http\n      port: {port}\n      targetPort: {port}\n      protocol: TCP\n---\napiVersion: v1\nkind: Endpoints\nmetadata:\n  name: {id}\n  namespace: {NAMESPACE}\n  labels:\n{labels}subsets:\n  - addresses:\n      - ip: {ip}\n    ports:\n      - name: http\n        port: {port}\n        protocol: TCP\n",
                id = h.id,
                port = h.port,
                note = h.label(),
            )
        }
        None => format!(
            "{ns}---\napiVersion: v1\nkind: Service\nmetadata:\n  name: {id}\n  namespace: {NAMESPACE}\n  labels:\n{labels}  annotations:\n    iyakki.dev/note: \"{note}\"\nspec:\n  type: ExternalName\n  externalName: {name}\n",
            id = h.id,
            name = h.address,
            note = h.label(),
        ),
    }
}

/// An egress rule for a cluster that has a default-deny policy in `iyakki-ai` (IYAKKI does not apply it).
pub fn network_policy_example(h: &Host) -> String {
    let to = match is_ip(&h.address) {
        Some(ip) => format!("        - ipBlock:\n            cidr: {ip}/{}", if ip.is_ipv6() { 128 } else { 32 }),
        None => "        - ipBlock:\n            cidr: <the host's address>/32".to_string(),
    };
    format!("apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: allow-{id}\n  namespace: {NAMESPACE}\nspec:\n  podSelector: {{}}\n  policyTypes: [Egress]\n  egress:\n    - to:\n{to}\n      ports:\n        - protocol: TCP\n          port: {port}\n", id = h.id, port = h.port)
}

pub const NETWORK_POLICY_NOTE: &str = "k3s starts with no network policy, so pods may call any address their node can reach. If you add a default-deny egress policy to the namespace iyakki-ai, allow the host's address and port too (docs/MAC-MODEL-HOST.md shows the rule). Pods reach a machine on your network through the node (flannel masquerades their traffic), so the machine only sees the node's address.";

/// Create or update the Service and Endpoints of a host (the cluster must be running). Returns what kubectl printed.
pub fn publish(k: &dyn Kube, data_dir: &Path, h: &Host) -> Result<String, String> {
    let dir = data_dir.join("model-hosts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.yaml", h.id));
    std::fs::write(&file, service_manifest(h)).map_err(|e| e.to_string())?;
    let out = k.kubectl(&["apply", "-f", &file.display().to_string()]);
    if out.ok() {
        Ok(out.stdout.trim().to_string())
    } else if out.missing || out.code.is_none() {
        Err("The cluster is not running here (Settings > Dev cluster, or Store > Setup > k3s).".into())
    } else {
        Err(format!("kubectl apply failed: {}", out.combined().trim()))
    }
}

/// Take the Service and Endpoints away again.
pub fn unpublish(k: &dyn Kube, id: &str) -> Result<(), String> {
    let out = k.kubectl(&["-n", NAMESPACE, "delete", "service,endpoints", "-l", &format!("iyakki.dev/model-host={id}"), "--ignore-not-found"]);
    if out.ok() { Ok(()) } else { Err(format!("kubectl delete failed: {}", out.combined().trim())) }
}

/// The pod of "Test from the cluster": busybox looks the name up and fetches the model list; it prints the lines [`parse_cluster_log`] reads.
pub fn test_pod_manifest(h: &Host) -> (String, String) {
    let name = format!("{}-check", h.id);
    let url = format!("http://{}:{}/v1/models", h.id, h.port);
    let script = format!("if nslookup {id} >/tmp/ns 2>&1; then echo DNS:ok; else echo DNS:fail; fi; if wget -q -T 8 -O /tmp/m {url} 2>/tmp/e; then echo HTTP:ok; head -c 200 /tmp/m; echo; else echo HTTP:fail; head -c 200 /tmp/e; fi", id = h.id);
    let m = format!(
        "apiVersion: v1\nkind: Pod\nmetadata:\n  name: {name}\n  namespace: {NAMESPACE}\n  labels:\n    app.kubernetes.io/managed-by: iyakki\n    iyakki.dev/model-host-check: {id}\nspec:\n  restartPolicy: Never\n  automountServiceAccountToken: false\n  containers:\n    - name: check\n      image: docker.io/library/busybox:1.36\n      command: [\"sh\", \"-c\", \"{script}\"]\n      securityContext:\n        allowPrivilegeEscalation: false\n        runAsNonRoot: true\n        runAsUser: 65534\n        capabilities:\n          drop: [ALL]\n        seccompProfile:\n          type: RuntimeDefault\n",
        id = h.id,
        script = script.replace('\\', "\\\\").replace('"', "\\\""),
    );
    (name, m)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterCheck {
    pub dns: bool,
    pub http: bool,
    pub detail: String,
}

impl ClusterCheck {
    pub fn message(&self, h: &Host) -> String {
        match (self.dns, self.http) {
            (true, true) => format!("A pod in the cluster reached {} ({}). Pods call {}.", h.id, h.target().authority(), h.cluster_url()),
            (true, false) => format!("The name {} resolves in the cluster, but the pod could not fetch the model list from {}: {}. Check the machine's firewall and that the server listens on the network, not only on 127.0.0.1.", h.id, h.target().authority(), self.detail),
            (false, _) => format!("The name {} does not resolve in the cluster. Publish the host to the cluster first (the Service is missing).", h.id),
        }
    }
}

/// Read the log of the check pod.
pub fn parse_cluster_log(log: &str) -> ClusterCheck {
    let dns = log.lines().any(|l| l.trim() == "DNS:ok");
    let http = log.lines().any(|l| l.trim() == "HTTP:ok");
    let after = log.lines().skip_while(|l| !l.starts_with("HTTP:")).skip(1).collect::<Vec<_>>().join(" ");
    ClusterCheck { dns, http, detail: after.trim().chars().take(160).collect::<String>() }
}

/// Run the check pod in the managed cluster and read its log. `poll` and `timeout` bound the wait (the first run pulls busybox).
pub fn test_from_cluster(k: &dyn Kube, data_dir: &Path, h: &Host, timeout: Duration, poll: Duration, sleep: &dyn Fn(Duration)) -> Result<ClusterCheck, String> {
    let (name, manifest) = test_pod_manifest(h);
    let _ = k.kubectl(&["-n", NAMESPACE, "delete", "pod", &name, "--ignore-not-found", "--wait=true"]);
    let dir = data_dir.join("model-hosts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{name}.yaml"));
    std::fs::write(&file, manifest).map_err(|e| e.to_string())?;
    let out = k.kubectl(&["apply", "-f", &file.display().to_string()]);
    let _ = std::fs::remove_file(&file);
    if !out.ok() {
        return Err(format!("could not start the check pod: {}", out.combined().trim()));
    }
    let start = Instant::now();
    let result = loop {
        let ph = k.kubectl(&["-n", NAMESPACE, "get", "pod", &name, "-o", "jsonpath={.status.phase}"]);
        let phase = ph.stdout.trim().to_string();
        if phase == "Succeeded" || phase == "Failed" {
            let log = k.kubectl(&["-n", NAMESPACE, "logs", &name]);
            break Ok(parse_cluster_log(&log.stdout));
        }
        if start.elapsed() >= timeout {
            let d = k.kubectl(&["-n", NAMESPACE, "describe", "pod", &name]);
            let tail: Vec<&str> = d.stdout.lines().rev().take(3).collect();
            break Err(format!("the check pod did not finish in {} s (phase {}); its last events: {}. The first run pulls the busybox image, which needs internet access for the cluster.", timeout.as_secs(), if phase.is_empty() { "unknown" } else { &phase }, tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
        }
        sleep(poll);
    };
    let _ = k.kubectl(&["-n", NAMESPACE, "delete", "pod", &name, "--ignore-not-found", "--wait=false"]);
    result
}

#[cfg(test)]
#[path = "modelhost_tests.rs"]
mod tests;
