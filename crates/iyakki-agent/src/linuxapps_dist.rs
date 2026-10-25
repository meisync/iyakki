//! Getting a Debian or Ubuntu base root file system for the Linux apps sandbox, at the person's click, from the distribution's own
//! servers. Nothing is shipped in the IYAKKI image. Two routes, the simplest verifiable one for each:
//!
//! * **Ubuntu**: the `ubuntu-base` root tarball (`cdimage.ubuntu.com/ubuntu-base/releases/<version>/release/`), a tarball made for
//!   `chroot` (about 30 MB, no cloud-init or snap). The file name carries the point release, so it is read from the `SHA256SUMS`
//!   file of the same folder, fetched over HTTPS; the download must hash to exactly the value listed there.
//! * **Debian**: the official `debian:<suite>-slim` image of Docker Hub's library (the Debian project's own `debuerreotype` build),
//!   pulled with the plain registry protocol: the tag is resolved to a manifest, the manifest's SHA-256 is computed here (and
//!   compared with a pin when the release table has one), and each layer blob must hash to the digest the manifest names.
//!
//! What this verifies, in plain words: the bytes are exactly the ones the distribution's server listed, over a certificate-checked
//! HTTPS connection to the distribution's host. It does **not** verify the distribution's GPG signature (`SHA256SUMS.gpg`,
//! the image signatures): that would need a GPG verifier and the Ubuntu/Debian archive keys, which IYAKKI does not carry. `apt`
//! inside the sandbox does verify the signed package lists with the distribution's keys (they come in the base).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use iyakki_proxy::{StreamHead, Streamer};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Distro {
    Debian,
    Ubuntu,
}

impl Distro {
    pub fn title(self) -> &'static str {
        match self {
            Distro::Debian => "Debian",
            Distro::Ubuntu => "Ubuntu",
        }
    }
}

/// One base the person can choose.
#[derive(Clone, Copy, Debug)]
pub struct Release {
    pub distro: Distro,
    /// Short id used on the command line and in files: `debian-12`, `ubuntu-24.04`.
    pub id: &'static str,
    pub title: &'static str,
    /// Debian: the image tag; Ubuntu: the release folder (`24.04`).
    pub tag: &'static str,
    /// A SHA-256 manifest digest this release must resolve to (Debian). None: the tag is followed and the digest it resolved to is recorded.
    pub pin: Option<&'static str>,
    /// About how much disk the base takes unpacked (MB), for the space check and the page.
    pub unpacked_mb: u64,
}

pub const RELEASES: [Release; 4] = [
    Release { distro: Distro::Debian, id: "debian-12", title: "Debian 12 (bookworm)", tag: "bookworm-slim", pin: None, unpacked_mb: 120 },
    Release { distro: Distro::Debian, id: "debian-13", title: "Debian 13 (trixie)", tag: "trixie-slim", pin: None, unpacked_mb: 120 },
    Release { distro: Distro::Ubuntu, id: "ubuntu-22.04", title: "Ubuntu 22.04 LTS (jammy)", tag: "22.04", pin: None, unpacked_mb: 120 },
    Release { distro: Distro::Ubuntu, id: "ubuntu-24.04", title: "Ubuntu 24.04 LTS (noble)", tag: "24.04", pin: None, unpacked_mb: 120 },
];

pub fn release(id: &str) -> Option<&'static Release> {
    RELEASES.iter().find(|r| r.id == id)
}

/// The licence notice shown before a download.
pub const LICENCE_NOTE: &str = "Debian and Ubuntu are free software. The base system is downloaded from the distribution's servers (nothing is shipped in IYAKKI); every package keeps its own licence, which you find under /usr/share/doc/<package>/copyright inside the sandbox. \"Ubuntu\" and its logo are trademarks of Canonical Ltd; \"Debian\" is a registered trademark of Software in the Public Interest, Inc. IYAKKI does not rebrand them and is not endorsed by either.";

/// The processor name the distributions use.
pub fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" }
}

/// Hosts a download may be redirected to.
const HOSTS: &[&str] = &["ubuntu.com", "docker.io", "docker.com", "cloudflarestorage.com", "cloudflare.docker.com"];
/// Never more than this many bytes of compressed download.
pub const MAX_DOWNLOAD: u64 = 2 << 30;

/// What was fetched and how it was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// What was verified, one sentence for the sandbox's record and the page.
    pub verified: String,
    /// Where it came from.
    pub source: String,
}

// ------------------------------------------------------------------------------------------------ fetching

fn resolve(base: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.starts_with("https://") || loc.starts_with("http://") {
        return Some(loc.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split('/').next()?;
    if loc.starts_with('/') { Some(format!("{scheme}://{authority}{loc}")) } else { None }
}

fn host_of(url: &str) -> String {
    url.strip_prefix("https://").unwrap_or(url).split(['/', ':', '?', '#']).next().unwrap_or("").to_ascii_lowercase()
}

/// Open `url`, following redirects (each hop must be https and on the allow-list; the authorization header is dropped when the host changes).
pub fn open(st: &dyn Streamer, url: &str, headers: &[(String, String)]) -> Result<(StreamHead, String), String> {
    let mut url = url.to_string();
    let first_host = host_of(&url);
    for _ in 0..6 {
        crate::clis::host_ok(&url, HOSTS)?;
        let hdrs: Vec<(String, String)> = if host_of(&url) == first_host { headers.to_vec() } else { headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("authorization")).cloned().collect() };
        let head = st.open(&url, &hdrs).map_err(|e| format!("could not reach {url}: {e}"))?;
        if (300..400).contains(&head.status) {
            let loc = head.header("location").ok_or("a redirect without a place to go")?;
            url = resolve(&url, loc).ok_or("an unusable redirect")?;
            continue;
        }
        if head.status != 200 {
            return Err(format!("{url} answered {}", head.status));
        }
        return Ok((head, url));
    }
    Err("too many redirects".into())
}

fn small(st: &dyn Streamer, url: &str, headers: &[(String, String)], max: u64) -> Result<Vec<u8>, String> {
    let (head, _) = open(st, url, headers)?;
    let mut v = Vec::new();
    head.body.take(max + 1).read_to_end(&mut v).map_err(|e| e.to_string())?;
    if v.len() as u64 > max {
        return Err(format!("{url} is larger than expected"));
    }
    Ok(v)
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Download to `dest`, hashing as it goes. Returns the SHA-256 (hex). `limit` stops a download that is longer than it should be.
fn to_file(st: &dyn Streamer, url: &str, headers: &[(String, String)], dest: &Path, limit: u64, free: &dyn Fn(u64) -> Result<(), String>, progress: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    let (head, _) = open(st, url, headers)?;
    let total: u64 = head.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if total > limit {
        return Err(format!("{url} is {} MB, more than the {} MB allowed", total >> 20, limit >> 20));
    }
    free(total)?;
    let mut f = File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let (mut h, mut done, mut buf, mut last) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024], 0u64);
    let mut body = head.body;
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > limit {
            return Err(format!("{url} is longer than the {} MB allowed", limit >> 20));
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        if done - last >= 4 << 20 {
            last = done;
            progress(done, total);
        }
    }
    if total > 0 && done != total {
        return Err(format!("the download ended early ({done} of {total} bytes)"));
    }
    f.flush().map_err(|e| e.to_string())?;
    progress(done, total.max(done));
    Ok(h.finalize().iter().map(|x| format!("{x:02x}")).collect())
}

// ------------------------------------------------------------------------------------------------ Ubuntu

/// The line of a `SHA256SUMS` file for the base tarball of `version` and `arch`: `(file name, hash)`. When several point releases are
/// listed the highest one wins.
pub fn ubuntu_pick(sums: &str, version: &str, arch: &str) -> Option<(String, String)> {
    let want_end = format!("-base-{arch}.tar.gz");
    let want_start = format!("ubuntu-base-{version}");
    let mut found: Vec<(String, String)> = sums
        .lines()
        .filter_map(|l| {
            let (h, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            let ok_hash = h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit());
            (ok_hash && name.starts_with(&want_start) && name.ends_with(&want_end) && !name.contains('/')).then(|| (name.to_string(), h.to_ascii_lowercase()))
        })
        .collect();
    found.sort_by(|a, b| version_key(&a.0).cmp(&version_key(&b.0)));
    found.pop()
}

fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().unwrap_or(0)).collect()
}

fn fetch_ubuntu(st: &dyn Streamer, r: &Release, dir: &Path, free: &dyn Fn(u64) -> Result<(), String>, progress: &mut dyn FnMut(u64, u64)) -> Result<(std::path::PathBuf, Fetched), String> {
    let base = format!("https://cdimage.ubuntu.com/ubuntu-base/releases/{}/release/", r.tag);
    let sums_url = format!("{base}SHA256SUMS");
    let sums = String::from_utf8_lossy(&small(st, &sums_url, &[], 1 << 20)?).into_owned();
    let (name, want) = ubuntu_pick(&sums, r.tag, arch()).ok_or_else(|| format!("{sums_url} lists no ubuntu-base tarball for {} on {}", r.tag, arch()))?;
    let file = dir.join(&name);
    let got = to_file(st, &format!("{base}{name}"), &[], &file, MAX_DOWNLOAD, free, progress)?;
    if got != want {
        let _ = std::fs::remove_file(&file);
        return Err(format!("the checksum of {name} does not match the one in SHA256SUMS (got {got}, listed {want}): the download was discarded"));
    }
    Ok((file, Fetched { verified: format!("SHA-256 {want} of {name}, as listed in {sums_url} (fetched over HTTPS; the GPG signature of that list is not checked)"), source: format!("{base}{name}") }))
}

// ------------------------------------------------------------------------------------------------ Debian (OCI)

const OCI_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
const REGISTRY: &str = "https://registry-1.docker.io";

fn digest_ok(d: &str) -> bool {
    d.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
}

/// The manifest to pull for `arch` out of an image index (or the manifest itself when it is not an index): `Ok(Some(digest))` to fetch
/// that one next, `Ok(None)` when `doc` already is the manifest.
pub fn oci_pick(doc: &Value, arch: &str) -> Result<Option<String>, String> {
    let Some(list) = doc.get("manifests").and_then(Value::as_array) else { return Ok(None) };
    list.iter()
        .find(|m| {
            let p = m.get("platform");
            p.and_then(|p| p.get("os")).and_then(Value::as_str) == Some("linux") && p.and_then(|p| p.get("architecture")).and_then(Value::as_str) == Some(arch)
        })
        .and_then(|m| m.get("digest").and_then(Value::as_str))
        .filter(|d| digest_ok(d))
        .map(|d| Some(d.to_string()))
        .ok_or_else(|| format!("the image has no linux/{arch} build"))
}

/// The layers of a manifest: `(digest, size)` in order. Anything but plain, gzip or zstd tar layers is refused.
pub fn oci_layers(doc: &Value) -> Result<Vec<(String, u64)>, String> {
    let layers = doc.get("layers").and_then(Value::as_array).ok_or("the manifest has no layers")?;
    if layers.is_empty() || layers.len() > 64 {
        return Err("the manifest has an unusable number of layers".into());
    }
    layers
        .iter()
        .map(|l| {
            let mt = l.get("mediaType").and_then(Value::as_str).unwrap_or("");
            if !(mt.contains("tar") || mt.contains("layer")) || mt.contains("foreign") || mt.contains("nondistributable") {
                return Err(format!("a layer of a kind IYAKKI does not unpack ({mt})"));
            }
            let d = l.get("digest").and_then(Value::as_str).filter(|d| digest_ok(d)).ok_or("a layer without a SHA-256 digest")?;
            Ok((d.to_string(), l.get("size").and_then(Value::as_u64).unwrap_or(0)))
        })
        .collect()
}

fn fetch_debian(st: &dyn Streamer, r: &Release, dir: &Path, free: &dyn Fn(u64) -> Result<(), String>, progress: &mut dyn FnMut(u64, u64)) -> Result<(Vec<std::path::PathBuf>, Fetched), String> {
    let repo = "library/debian";
    let token_url = format!("https://auth.docker.io/token?service=registry.docker.io&scope=repository:{repo}:pull");
    let tok: Value = serde_json::from_slice(&small(st, &token_url, &[], 64 * 1024)?).map_err(|_| "the registry's token answer was not understood")?;
    let token = tok.get("token").and_then(Value::as_str).ok_or("the registry gave no token")?.to_string();
    let hdrs = |accept: bool| -> Vec<(String, String)> {
        let mut h = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        if accept {
            h.push(("Accept".to_string(), OCI_ACCEPT.to_string()));
        }
        h
    };
    let top_url = format!("{REGISTRY}/v2/{repo}/manifests/{}", r.tag);
    let top = small(st, &top_url, &hdrs(true), 4 << 20)?;
    let top_digest = format!("sha256:{}", sha256_hex(&top));
    if let Some(pin) = r.pin {
        if pin != top_digest {
            return Err(format!("{} resolved to {top_digest}, not the pinned {pin}: refused", r.tag));
        }
    }
    let top_doc: Value = serde_json::from_slice(&top).map_err(|_| "the manifest was not understood")?;
    let (manifest_digest, doc) = match oci_pick(&top_doc, arch())? {
        None => (top_digest.clone(), top_doc),
        Some(d) => {
            let body = small(st, &format!("{REGISTRY}/v2/{repo}/manifests/{d}"), &hdrs(true), 4 << 20)?;
            let got = format!("sha256:{}", sha256_hex(&body));
            if got != d {
                return Err(format!("the manifest for {arch} hashes to {got}, not the {d} the index names: refused", arch = arch()));
            }
            (d, serde_json::from_slice(&body).map_err(|_| "the manifest was not understood")?)
        }
    };
    let layers = oci_layers(&doc)?;
    let total: u64 = layers.iter().map(|l| l.1).sum();
    if total > MAX_DOWNLOAD {
        return Err("the image is larger than allowed".into());
    }
    free(total)?;
    let mut files = Vec::new();
    for (i, (digest, size)) in layers.iter().enumerate() {
        let file = dir.join(format!("layer{i}.blob"));
        let url = format!("{REGISTRY}/v2/{repo}/blobs/{digest}");
        let got = to_file(st, &url, &hdrs(false), &file, (*size).max(1).saturating_add(1 << 20).min(MAX_DOWNLOAD), &|_| Ok(()), progress)?;
        if format!("sha256:{got}") != *digest {
            let _ = std::fs::remove_file(&file);
            return Err(format!("layer {} hashes to sha256:{got}, not the {digest} the manifest names: the download was discarded", i + 1));
        }
        files.push(file);
    }
    Ok((files, Fetched { verified: format!("manifest {manifest_digest} (tag {}, resolved by IYAKKI from registry-1.docker.io); each of the {} layer(s) hashes to the SHA-256 the manifest names; image signatures are not checked", r.tag, layers.len()), source: format!("docker.io/library/debian:{} @ {manifest_digest}", r.tag) }))
}

/// Download the base of `r` into `dir` and check it. The files come back in the order they must be unpacked (Ubuntu: one tarball;
/// Debian: the image's layers). `free(bytes)` is asked before the big transfer starts.
pub fn fetch(st: &dyn Streamer, r: &Release, dir: &Path, free: &dyn Fn(u64) -> Result<(), String>, progress: &mut dyn FnMut(u64, u64)) -> Result<(Vec<std::path::PathBuf>, Fetched), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match r.distro {
        Distro::Ubuntu => fetch_ubuntu(st, r, dir, free, progress).map(|(f, v)| (vec![f], v)),
        Distro::Debian => fetch_debian(st, r, dir, free, progress),
    }
}
