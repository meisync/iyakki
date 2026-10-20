//! Push an image to a registry with no docker, podman or curl: this device builds a one-file image (a `FROM scratch` image whose only
//! layer holds one static program, which is all the private store's server is) and speaks the registry v2 HTTP API itself.
//!
//! * [`build_image`] makes the three parts of an OCI image: one uncompressed layer (a tar with one file), the image config (user, entry
//!   point, port, CPU architecture) and the manifest. The same input always gives the same bytes (fixed times), so the same program is
//!   the same digest and an unchanged layer is not sent twice.
//! * [`push`] sends them to `host:port/<repo>:<tag>` over plain HTTP: blobs the registry has are skipped (HEAD), the others are
//!   uploaded in one request each, the manifest last. [`catalog`] lists what a registry holds (for "used by N images").
//! * HTTP goes through [`RegistryHttp`]: [`TcpHttp`] is the real client (HTTP/1.1 over a TCP socket, one request per connection, no TLS:
//!   the in-cluster registry is plain HTTP on a NodePort, see `regsetup.rs`), the tests give a fake registry. Nothing here starts a
//!   process.
//!
//! The image is pulled by the cluster's containerd as `localhost:<nodePort>/<repo>:<tag>`; containerd allows plain HTTP for
//! `localhost`, so no node needs configuring.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const MEDIA_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const MEDIA_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
pub const MEDIA_LAYER: &str = "application/vnd.oci.image.layer.v1.tar";

/// One reply of a registry.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).chars().take(300).collect()
    }
}

/// The one way this module speaks HTTP. `host` is `name:port`; `path` starts with `/` and may carry a query.
pub trait RegistryHttp: Send + Sync {
    fn request(&self, method: &str, host: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Reply, String>;
}

// ------------------------------------------------------------------------------------------------------------------ the real client

/// HTTP/1.1 over a TCP socket. `Connection: close`, so the reply ends where the connection does (or at its Content-Length / chunks).
pub struct TcpHttp {
    pub connect: Duration,
    pub io: Duration,
}

impl Default for TcpHttp {
    fn default() -> TcpHttp {
        TcpHttp { connect: Duration::from_secs(4), io: Duration::from_secs(120) }
    }
}

const MAX_REPLY: usize = 8 * 1024 * 1024;

impl RegistryHttp for TcpHttp {
    fn request(&self, method: &str, host: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Reply, String> {
        if host.is_empty() || host.contains('/') || host.contains(' ') || !path.starts_with('/') || path.contains(['\r', '\n', ' ']) {
            return Err(format!("not a registry address: {host}{path}"));
        }
        let addr = host.to_socket_addrs().map_err(|e| format!("cannot resolve {host}: {e}"))?.next().ok_or_else(|| format!("cannot resolve {host}"))?;
        let mut s = TcpStream::connect_timeout(&addr, self.connect).map_err(|e| format!("cannot connect to {host}: {e}"))?;
        let _ = s.set_read_timeout(Some(self.io));
        let _ = s.set_write_timeout(Some(self.io));
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: iyakki\r\n");
        for (k, v) in headers {
            if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
                return Err("bad header".into());
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() || matches!(method, "PUT" | "POST") {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("sending to {host} failed: {e}"))?;
        let mut raw = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if raw.len() > MAX_REPLY {
                        return Err("the registry's reply is too large".into());
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(format!("{host} stopped answering")),
                Err(e) => return Err(format!("reading from {host} failed: {e}")),
            }
        }
        parse_reply(&raw, method == "HEAD")
    }
}

/// Parse a complete HTTP/1.1 reply (status line, headers, body with Content-Length, chunks or until close).
pub fn parse_reply(raw: &[u8], head_only: bool) -> Result<Reply, String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("the reply has no header end")?;
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.lines();
    let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).ok_or("the reply has no status line")?;
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    let rest = &raw[end + 4..];
    let find = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let body = if head_only {
        Vec::new()
    } else if find("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked")) {
        dechunk(rest)?
    } else if let Some(n) = find("content-length").and_then(|v| v.parse::<usize>().ok()) {
        rest[..n.min(rest.len())].to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Reply { status, headers, body })
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let nl = b.windows(2).position(|w| w == b"\r\n").ok_or("a chunked reply ended early")?;
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..nl]).split(';').next().unwrap_or("").trim(), 16).map_err(|_| "a chunk size is not a number")?;
        b = &b[nl + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            return Err("a chunked reply ended early".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

// ------------------------------------------------------------------------------------------------------------------ the image

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// What the one-file image holds and how it starts.
#[derive(Clone, Debug)]
pub struct ImageSpec<'a> {
    /// The file's name in the image's root (`iyakki`): the entry point is `/<file>`.
    pub file: &'a str,
    pub contents: &'a [u8],
    /// `amd64` or `arm64`.
    pub arch: &'a str,
    /// `10001:10001`.
    pub user: &'a str,
    /// Arguments after the program (the entry point is `["/<file>", args...]`).
    pub args: Vec<String>,
    pub port: u16,
}

pub struct Image {
    pub layer: Vec<u8>,
    pub layer_digest: String,
    pub config: Vec<u8>,
    pub config_digest: String,
    pub manifest: Vec<u8>,
}

fn octal(field: &mut [u8], v: u64) {
    let s = format!("{:0width$o}\0", v, width = field.len() - 1);
    field.copy_from_slice(s.as_bytes());
}

/// A tar archive with one regular file (mode 0555, owner root, time 0) at the archive's root.
pub fn tar_one_file(name: &str, contents: &[u8]) -> Result<Vec<u8>, String> {
    if name.is_empty() || name.len() > 99 || name.contains(['/', '\0']) {
        return Err(format!("`{name}` is not a file name for an image's root"));
    }
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], 0o555);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], contents.len() as u64);
    octal(&mut h[136..148], 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let c = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(c.as_bytes());
    let mut out = Vec::with_capacity(512 + contents.len() + 1536);
    out.extend_from_slice(&h);
    out.extend_from_slice(contents);
    out.resize(out.len() + (512 - contents.len() % 512) % 512, 0);
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}

pub fn build_image(spec: &ImageSpec) -> Result<Image, String> {
    if !matches!(spec.arch, "amd64" | "arm64") {
        return Err(format!("`{}` is not a CPU architecture an image can be built for (amd64 or arm64)", spec.arch));
    }
    if spec.contents.is_empty() {
        return Err("the program to put in the image is empty".into());
    }
    let layer = tar_one_file(spec.file, spec.contents)?;
    let layer_digest = format!("sha256:{}", sha256_hex(&layer));
    let mut entry = vec![format!("/{}", spec.file)];
    entry.extend(spec.args.iter().cloned());
    let config = serde_json::json!({
        "architecture": spec.arch,
        "os": "linux",
        "config": { "User": spec.user, "Entrypoint": entry, "ExposedPorts": { format!("{}/tcp", spec.port): {} }, "Env": ["PATH=/"] },
        "rootfs": { "type": "layers", "diff_ids": [layer_digest.clone()] },
        "history": [{ "created_by": "iyakki: one-file image" }],
    });
    let config = serde_json::to_vec(&config).map_err(|e| e.to_string())?;
    let config_digest = format!("sha256:{}", sha256_hex(&config));
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MEDIA_MANIFEST,
        "config": { "mediaType": MEDIA_CONFIG, "digest": config_digest, "size": config.len() },
        "layers": [{ "mediaType": MEDIA_LAYER, "digest": layer_digest, "size": layer.len() }],
    });
    let manifest = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    Ok(Image { layer, layer_digest, config, config_digest, manifest })
}

// ------------------------------------------------------------------------------------------------------------------ the registry API

fn valid_repo(r: &str) -> bool {
    !r.is_empty() && r.len() <= 200 && r.split('/').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')) && !p.starts_with(['.', '-', '_']))
}

fn valid_tag(t: &str) -> bool {
    !t.is_empty() && t.len() <= 128 && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) && !t.starts_with(['.', '-'])
}

/// The path part of an upload `Location` (absolute or relative), with `digest` added.
fn upload_url(location: &str, digest: &str) -> String {
    let path = match location.find("://") {
        Some(i) => location[i + 3..].find('/').map_or("/", |j| &location[i + 3 + j..]),
        None => location,
    };
    format!("{path}{}digest={digest}", if path.contains('?') { '&' } else { '?' })
}

fn refused(what: &str, host: &str, r: &Reply) -> String {
    let hint = match r.status {
        401 | 403 => " (the registry wants a sign-in, which IYAKKI's registry does not)",
        404 => " (the registry has no such repository or does not speak the v2 API)",
        413 => " (the registry refused a body this large)",
        500..=599 => " (the registry itself failed; look at its pod's log)",
        _ => "",
    };
    let detail = r.text();
    format!("{what} failed at {host}: HTTP {}{hint}{}", r.status, if detail.trim().is_empty() { String::new() } else { format!(" · {}", detail.trim()) })
}

/// Send `img` to `registry` (`host:port`) as `<repo>:<tag>`. Returns the manifest digest. `say` gets one line per step.
pub fn push(http: &dyn RegistryHttp, registry: &str, repo: &str, tag: &str, img: &Image, say: &mut dyn FnMut(&str)) -> Result<String, String> {
    if !valid_repo(repo) {
        return Err(format!("`{repo}` is not a repository name (lower-case letters, digits and . _ - , with / between parts)"));
    }
    if !valid_tag(tag) {
        return Err(format!("`{tag}` is not an image tag"));
    }
    let ping = http.request("GET", registry, "/v2/", &[], &[])?;
    if !matches!(ping.status, 200 | 401) {
        return Err(format!("{registry} does not look like a registry (GET /v2/ answered HTTP {})", ping.status));
    }
    let blobs: [(&str, &[u8], &str); 2] = [("the image layer", &img.layer, &img.layer_digest), ("the image config", &img.config, &img.config_digest)];
    for (what, bytes, digest) in blobs {
        let have = http.request("HEAD", registry, &format!("/v2/{repo}/blobs/{digest}"), &[], &[])?;
        if have.status == 200 {
            say(&format!("{what} is in the registry already ({} bytes)", bytes.len()));
            continue;
        }
        say(&format!("sending {what} ({} bytes)", bytes.len()));
        let start = http.request("POST", registry, &format!("/v2/{repo}/blobs/uploads/"), &[], &[])?;
        if start.status != 202 {
            return Err(refused("starting the upload", registry, &start));
        }
        let loc = start.header("location").ok_or("the registry gave no upload address")?;
        let put = http.request("PUT", registry, &upload_url(loc, digest), &[("Content-Type".into(), "application/octet-stream".into())], bytes)?;
        if put.status != 201 {
            return Err(refused("the upload", registry, &put));
        }
    }
    say("sending the manifest");
    let m = http.request("PUT", registry, &format!("/v2/{repo}/manifests/{tag}"), &[("Content-Type".into(), MEDIA_MANIFEST.into())], &img.manifest)?;
    if m.status != 201 {
        return Err(refused("the manifest", registry, &m));
    }
    Ok(format!("sha256:{}", sha256_hex(&img.manifest)))
}

/// What a registry holds: `(repository, tags)` for each repository (at most 200).
pub fn catalog(http: &dyn RegistryHttp, registry: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let r = http.request("GET", registry, "/v2/_catalog?n=200", &[], &[])?;
    if r.status != 200 {
        return Err(refused("listing the repositories", registry, &r));
    }
    let j: serde_json::Value = serde_json::from_slice(&r.body).map_err(|e| format!("the repository list is not JSON: {e}"))?;
    let mut out = Vec::new();
    for repo in j["repositories"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str()).filter(|r| valid_repo(r)) {
        let t = http.request("GET", registry, &format!("/v2/{repo}/tags/list"), &[], &[])?;
        let tags: Vec<String> = if t.status == 200 {
            serde_json::from_slice::<serde_json::Value>(&t.body).ok().and_then(|j| j["tags"].as_array().cloned()).unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        } else {
            Vec::new()
        };
        out.push((repo.to_string(), tags));
    }
    Ok(out)
}

/// Does `registry` answer `GET /v2/`?
pub fn answers(http: &dyn RegistryHttp, registry: &str) -> bool {
    http.request("GET", registry, "/v2/", &[], &[]).is_ok_and(|r| matches!(r.status, 200 | 401))
}

#[cfg(test)]
#[path = "ocipush_tests.rs"]
pub(crate) mod tests;
