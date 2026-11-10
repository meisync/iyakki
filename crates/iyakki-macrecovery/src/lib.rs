//! The client of Apple's macOS Recovery download service (`osrecovery.apple.com`), the protocol Apple's own Internet Recovery
//! speaks and the OpenCore project's `macrecovery` tool documents. IYAKKI uses it only to fetch the recovery image of the
//! person's own Mac model, at their request (Mac drivers page), to read Apple's Wi-Fi and Bluetooth firmware out of it
//! (`macfw.rs`). Nothing of Apple's is shipped, uploaded or kept after the job.
//!
//! The protocol, in three steps:
//! 1. `GET http://osrecovery.apple.com/` answers with a `Set-Cookie` session cookie.
//! 2. `POST http://osrecovery.apple.com/InstallationPayload/RecoveryImage` with the cookie and a text body of `key=value`
//!    lines (`cid`, `sn` = the serial's board part, here all zeros, `bid` = the board id of the Mac, `k`, `fg`, `os`)
//!    answers with `key: value` lines: `AU` (image address), `AT` (its token), `AH` (a hash), `CU`/`CT`/`CH` (the same for the
//!    chunk list), `AP` (the product).
//! 3. `GET <AU>` with the cookie `AssetToken=<AT>` downloads the image (`BaseSystem.dmg`, a UDIF disk image); the chunk list
//!    (`BaseSystem.chunklist`) from `CU` with `CT` gives the SHA-256 of every chunk of the image.
//!
//! Safety: every address Apple's service names must be on an `apple.com` host (the policy is a field so tests can point at a
//! local fake server); a redirect is followed only to such a host; sizes are checked against what the server announces; the
//! image is checked against the chunk list chunk by chunk (the list's own signature is not verified here); the download
//! resumes after an interruption from what is on disk. The wire details are from the OpenCore tool and the field names of
//! the answer are checked, but the whole path has NOT yet been run against Apple's real service by IYAKKI.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use iyakki_proxy::Streamer;
use sha2::{Digest, Sha256};

/// Apple's service.
pub const SERVICE: &str = "http://osrecovery.apple.com";
pub const USER_AGENT: &str = "InternetRecovery/1.0";
/// The serial's board part that asks for the newest image for the board (all zeros).
pub const MLB_ZERO: &str = "00000000000000000";
/// Largest image accepted (the real ones are under 2 GB; a server that announces more is refused).
pub const MAX_IMAGE: u64 = 6 << 30;
/// Largest chunk list read.
pub const MAX_CHUNKLIST: u64 = 8 << 20;

/// Where Apple's answers may point.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Host names (and their sub-domains) that addresses may have.
    pub hosts: Vec<String>,
    /// Redirects followed per request.
    pub max_redirects: u32,
    /// How long to wait before asking again after a failure (times the attempt number).
    pub retry_pause: std::time::Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { hosts: vec!["apple.com".into()], max_redirects: 5, retry_pause: std::time::Duration::from_secs(3) }
    }
}

impl Policy {
    pub fn allows(&self, url: &str) -> bool {
        match host_of(url) {
            Some(h) => self.hosts.iter().any(|a| h == *a || h.ends_with(&format!(".{a}"))),
            None => false,
        }
    }
}

/// The host of an `http(s)` address, lower case, without port or user info.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let auth = rest.split(['/', '?', '#']).next()?;
    if auth.contains('@') {
        return None;
    }
    let host = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(auth, |(h, _)| h);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Random hex of `n` characters from the system's random source.
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n.div_ceil(2)];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        // no random device: time and address-space noise is enough for an id that only has to look like one
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let h = Sha256::digest(format!("{t}{:p}", &buf).as_bytes());
        for (i, b) in buf.iter_mut().enumerate() {
            *b = h[i % 32];
        }
    }
    buf.iter().map(|b| format!("{b:02X}")).collect::<String>().chars().take(n).collect()
}

/// The ids a request carries. `cid` (16 hex), `k` and `fg` (64 hex each) are random per request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ids {
    pub cid: String,
    pub k: String,
    pub fg: String,
}

impl Ids {
    pub fn random() -> Ids {
        Ids { cid: random_hex(16), k: random_hex(64), fg: random_hex(64) }
    }
}

/// The body of the image request: `key=value` lines joined by newlines, in the tool's order.
pub fn request_body(ids: &Ids, mlb: &str, board_id: &str, os: &str) -> String {
    ["cid", "sn", "bid", "k", "fg", "os"].iter().zip([ids.cid.as_str(), mlb, board_id, ids.k.as_str(), ids.fg.as_str(), os]).map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")
}

/// What Apple's service answered for a board.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageInfo {
    pub product: String,
    pub image_url: String,
    pub image_hash: String,
    pub image_token: String,
    pub chunklist_url: String,
    pub chunklist_hash: String,
    pub chunklist_token: String,
}

/// Parse the `key: value` lines of the answer. Every field the download needs must be there.
pub fn parse_info(text: &str) -> Result<ImageInfo, String> {
    let mut m = std::collections::BTreeMap::new();
    for l in text.lines() {
        if let Some((k, v)) = l.split_once(": ") {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let get = |k: &str| m.get(k).filter(|v| !v.is_empty()).cloned().ok_or_else(|| format!("Apple's service did not send {k}"));
    Ok(ImageInfo { product: get("AP")?, image_url: get("AU")?, image_hash: get("AH")?, image_token: get("AT")?, chunklist_url: get("CU")?, chunklist_hash: get("CH")?, chunklist_token: get("CT")? })
}

fn base_headers() -> Vec<(String, String)> {
    vec![("User-Agent".into(), USER_AGENT.into()), ("Connection".into(), "close".into())]
}

fn read_text(mut head: iyakki_proxy::StreamHead, cap: u64) -> Result<String, String> {
    let mut v = Vec::new();
    head.body.by_ref().take(cap).read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&v).into_owned())
}

/// Step 1: the session cookie (`name=value`, as it must be sent back).
pub fn get_session(st: &dyn Streamer, base: &str) -> Result<String, String> {
    let head = st.open(&format!("{base}/"), &base_headers()).map_err(|e| format!("could not reach Apple's recovery service: {e}"))?;
    if head.status / 100 != 2 {
        return Err(format!("Apple's recovery service answered HTTP {}", head.status));
    }
    head.headers.iter().filter(|(k, _)| k == "set-cookie").find_map(|(_, v)| v.split(';').next().map(|c| c.trim().to_string()).filter(|c| c.contains('='))).ok_or_else(|| "Apple's recovery service sent no session cookie".to_string())
}

/// Step 2: ask for the recovery image of a board. `os` is `default` (the newest the board can run) or `latest`.
pub fn get_image_info(st: &dyn Streamer, base: &str, cookie: &str, board_id: &str, mlb: &str, os: &str, ids: &Ids) -> Result<ImageInfo, String> {
    if !iyakki_drivers::macboard::valid_board_id(board_id) {
        return Err(format!("`{board_id}` is not a Mac board id"));
    }
    let mut h = base_headers();
    h.push(("Cookie".into(), cookie.into()));
    h.push(("Content-Type".into(), "text/plain".into()));
    let head = st.post(&format!("{base}/InstallationPayload/RecoveryImage"), &h, request_body(ids, mlb, board_id, os).as_bytes()).map_err(|e| format!("could not ask Apple's recovery service: {e}"))?;
    let status = head.status;
    let text = read_text(head, 64 * 1024)?;
    if status / 100 != 2 {
        return Err(match status {
            404 | 400 | 403 => format!("Apple's service has no recovery image for board {board_id} (HTTP {status}); the board id may be wrong for this Mac"),
            s => format!("Apple's recovery service answered HTTP {s}"),
        });
    }
    parse_info(&text)
}

/// Open `url` with the asset token as the tool does (`Cookie: AssetToken=<token>`), following redirects only to allowed hosts.
fn open_asset(st: &dyn Streamer, policy: &Policy, url: &str, token: &str, range_from: u64) -> Result<iyakki_proxy::StreamHead, String> {
    let mut url = url.to_string();
    for _ in 0..=policy.max_redirects {
        if !policy.allows(&url) {
            return Err(format!("{url} is not an Apple address; refused"));
        }
        let mut h = base_headers();
        h.push(("Cookie".into(), format!("AssetToken={token}")));
        if range_from > 0 {
            h.push(("Range".into(), format!("bytes={range_from}-")));
        }
        let head = st.open(&url, &h)?;
        if matches!(head.status, 301 | 302 | 303 | 307 | 308) {
            let loc = head.header("location").ok_or("a redirect without a place to go")?.to_string();
            url = if loc.starts_with('/') {
                let (scheme, rest) = url.split_once("://").unwrap_or(("http", ""));
                format!("{scheme}://{}{loc}", rest.split('/').next().unwrap_or(""))
            } else {
                loc
            };
            continue;
        }
        return Ok(head);
    }
    Err("too many redirects".into())
}

/// "bytes 100-199/200" -> (100, 199, Some(200)).
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let r = v.trim().strip_prefix("bytes ")?;
    let (range, total) = r.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?, total.parse().ok()))
}

/// How a download went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub bytes: u64,
    /// The download started from bytes already on disk.
    pub resumed_from: u64,
}

/// Download `url` into `dest`, resuming from `dest` when it is there already (a partial file from an interrupted job). Writes
/// to `dest` itself while it runs; it is complete when this returns `Ok`. `progress(done, total)` is called about every 4 MiB
/// (total 0 when unknown); `cancel()` true stops with an error and keeps the partial file. A connection that drops is resumed up to
/// `retries` times.
#[allow(clippy::too_many_arguments)]
pub fn download(st: &dyn Streamer, policy: &Policy, url: &str, token: &str, dest: &Path, max: u64, retries: u32, progress: &mut dyn FnMut(u64, u64), cancel: &dyn Fn() -> bool) -> Result<Downloaded, String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let first = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut last_err = String::new();
    for attempt in 0..=retries {
        if attempt > 0 {
            std::thread::sleep(policy.retry_pause * attempt);
        }
        let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        let head = match open_asset(st, policy, url, token, have) {
            Ok(h) => h,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let (mut file, total, mut done) = match head.status {
            206 => {
                let (from, _to, total) = head.header("content-range").and_then(parse_content_range).ok_or("the server answered a partial download without saying which part")?;
                if from != have {
                    return Err(format!("the server resumed at byte {from}, not {have}"));
                }
                (std::fs::OpenOptions::new().append(true).open(dest).map_err(|e| format!("{}: {e}", dest.display()))?, total.unwrap_or(0), have)
            }
            200 => {
                let total = head.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                (std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?, total, 0)
            }
            416 => {
                // asked to resume past the end: the file may be complete already
                let total = head.header("content-range").and_then(|v| v.trim().strip_prefix("bytes */")?.parse::<u64>().ok());
                if total == Some(have) {
                    progress(have, have);
                    return Ok(Downloaded { bytes: have, resumed_from: first });
                }
                let _ = std::fs::remove_file(dest);
                last_err = "the part on disk does not fit what the server has; starting over".into();
                continue;
            }
            404 | 403 => return Err(format!("Apple's server refused the download (HTTP {}); the token may have expired: start the job again", head.status)),
            s => {
                last_err = format!("HTTP {s}");
                continue;
            }
        };
        if total > max || done > max {
            return Err(format!("the file is {} bytes, more than the {} allowed", total.max(done), max));
        }
        let mut body = head.body;
        let mut buf = vec![0u8; 256 * 1024];
        let mut since = 0u64;
        let mut broke = false;
        loop {
            if cancel() {
                let _ = file.flush();
                return Err("stopped".into());
            }
            match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    file.write_all(&buf[..n]).map_err(|e| format!("{}: {e} (is the disk full?)", dest.display()))?;
                    done += n as u64;
                    since += n as u64;
                    if done > max {
                        return Err(format!("the download is larger than the {max} bytes allowed"));
                    }
                    if since >= 4 << 20 {
                        progress(done, total);
                        since = 0;
                    }
                }
                Err(e) => {
                    last_err = format!("the connection dropped after {done} bytes: {e}");
                    broke = true;
                    break;
                }
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        if broke {
            if attempt == retries {
                break;
            }
            continue;
        }
        if total > 0 && done != total {
            last_err = format!("got {done} of {total} bytes");
            if attempt == retries {
                break;
            }
            continue;
        }
        progress(done, total.max(done));
        return Ok(Downloaded { bytes: done, resumed_from: first });
    }
    Err(format!("the download did not finish: {last_err}"))
}

// ------------------------------------------------------------------------------------------------ the chunk list

/// The chunk list: the SHA-256 of every chunk of the image (header `CNKL`, 36 bytes, then 36-byte entries: a little-endian u32
/// size and the 32-byte hash).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkList {
    pub chunks: Vec<(u32, [u8; 32])>,
}

pub fn parse_chunklist(b: &[u8]) -> Result<ChunkList, String> {
    if b.len() < 36 || &b[0..4] != b"CNKL" {
        return Err("the chunk list is not in the format Apple publishes (no CNKL header)".into());
    }
    let u32le = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let u64le = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));
    let (header_size, method) = (u32le(4), b[9]);
    let (count, offset) = (u64le(12), u64le(20));
    if header_size != 36 || method != 1 {
        return Err(format!("the chunk list has a header size {header_size} and method {method}; only 36 and 1 are known"));
    }
    if count == 0 || count > 1 << 22 || offset < 36 || offset.saturating_add(count * 36) > b.len() as u64 {
        return Err("the chunk list is cut short or does not add up".into());
    }
    let mut chunks = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let o = offset as usize + i * 36;
        let mut h = [0u8; 32];
        h.copy_from_slice(&b[o + 4..o + 36]);
        chunks.push((u32le(o), h));
    }
    Ok(ChunkList { chunks })
}

impl ChunkList {
    pub fn total(&self) -> u64 {
        self.chunks.iter().map(|c| c.0 as u64).sum()
    }
}

/// Check the image file against the chunk list: its size and every chunk's SHA-256, reading it once. `progress(done, total)`
/// is called now and then. Returns what is wrong in plain words.
pub fn verify_image(image: &Path, list: &ChunkList, progress: &mut dyn FnMut(u64, u64), cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let len = std::fs::metadata(image).map_err(|e| format!("{}: {e}", image.display()))?.len();
    if len != list.total() {
        return Err(format!("the image is {len} bytes but Apple's chunk list says {}: the download is incomplete or wrong", list.total()));
    }
    let mut f = std::fs::File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let mut done = 0u64;
    let mut buf = Vec::new();
    for (i, (size, want)) in list.chunks.iter().enumerate() {
        if cancel() {
            return Err("stopped".into());
        }
        buf.resize(*size as usize, 0);
        f.read_exact(&mut buf).map_err(|e| format!("{}: {e}", image.display()))?;
        if Sha256::digest(&buf).as_slice() != want {
            return Err(format!("chunk {} of {} does not match Apple's chunk list (at byte {done}): the download is damaged; start the job again{}", i + 1, list.chunks.len(), if i == 0 { start_of(&buf) } else { String::new() }));
        }
        done += *size as u64;
        if i % 64 == 0 {
            progress(done, len);
        }
    }
    progress(len, len);
    Ok(())
}

/// What the start of a file that failed its first chunk looks like, to tell a page or an error text (the server did not send the
/// image) from damaged bytes: " (the file starts with ...)".
pub fn start_of(buf: &[u8]) -> String {
    let head = &buf[..buf.len().min(48)];
    if !head.is_empty() && head.iter().all(|b| (0x20..0x7f).contains(b) || *b == b'\n' || *b == b'\r') {
        format!(" (the file starts with the text \"{}\": the server sent a message, not the image)", String::from_utf8_lossy(&head[..head.len().min(40)]).replace(['\n', '\r'], " "))
    } else {
        format!(" (the file starts with bytes {})", head.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "))
    }
}

/// Download the chunk list into memory (it is a few hundred kilobytes).
pub fn fetch_chunklist(st: &dyn Streamer, policy: &Policy, info: &ImageInfo) -> Result<ChunkList, String> {
    let mut head = open_asset(st, policy, &info.chunklist_url, &info.chunklist_token, 0)?;
    if head.status / 100 != 2 {
        return Err(format!("Apple's server answered HTTP {} for the chunk list", head.status));
    }
    let mut v = Vec::new();
    head.body.by_ref().take(MAX_CHUNKLIST + 1).read_to_end(&mut v).map_err(|e| e.to_string())?;
    if v.len() as u64 > MAX_CHUNKLIST {
        return Err("the chunk list is larger than expected; refused".into());
    }
    parse_chunklist(&v)
}

/// SHA-256 of a whole file, as lower-case hex (the `AH` value is compared with it when it looks like one).
pub fn file_sha256(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Which image Apple would send for a board and `os` (`default` or `latest`), without downloading it: the product and image address,
/// so two requests can be compared.
pub fn image_identity(st: &dyn Streamer, base: &str, policy: &Policy, board_id: &str, os: &str) -> Result<String, String> {
    image_identity_for(st, base, policy, board_id, MLB_ZERO, os)
}

/// [`image_identity`] with the serial's board part (`mlb`): the service picks the macOS version from the board id AND this part (the
/// all-zero part asks for the newest version the board can run; a serial of an older year asks for the version of that time).
pub fn image_identity_for(st: &dyn Streamer, base: &str, policy: &Policy, board_id: &str, mlb: &str, os: &str) -> Result<String, String> {
    let cookie = get_session(st, base)?;
    let info = get_image_info(st, base, &cookie, board_id, mlb, os, &Ids::random())?;
    if !policy.allows(&info.image_url) {
        return Err(format!("Apple's service named {}, which is not an Apple address; refused", info.image_url));
    }
    Ok(format!("{} {}", info.product, info.image_url))
}

/// Everything in one call (`os` = `default`): see [`fetch_recovery_image_os`].
#[allow(clippy::too_many_arguments)]
pub fn fetch_recovery_image(st: &dyn Streamer, base: &str, policy: &Policy, board_id: &str, dir: &Path, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool, retries: u32) -> Result<PathBuf, String> {
    fetch_recovery_image_os(st, base, policy, board_id, "default", dir, say, cancel, retries).map(|r| r.0)
}

/// Everything in one call: session, image info for `os`, chunk list, download of the image into `dir/BaseSystem.dmg`, verification.
/// Says each step through `say`. Returns the file and the image's identity ([`image_identity`]).
#[allow(clippy::too_many_arguments)]
pub fn fetch_recovery_image_os(st: &dyn Streamer, base: &str, policy: &Policy, board_id: &str, os: &str, dir: &Path, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool, retries: u32) -> Result<(PathBuf, String), String> {
    fetch_recovery_image_for(st, base, policy, board_id, MLB_ZERO, os, dir, say, cancel, retries)
}

/// [`fetch_recovery_image_os`] with the serial's board part (see [`image_identity_for`]). Says which host every request goes to and
/// how many bytes came, so a failure can be diagnosed from the log alone (no tokens are logged).
#[allow(clippy::too_many_arguments)]
pub fn fetch_recovery_image_for(st: &dyn Streamer, base: &str, policy: &Policy, board_id: &str, mlb: &str, os: &str, dir: &Path, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool, retries: u32) -> Result<(PathBuf, String), String> {
    say(&format!("asking Apple's recovery service ({}) for a session", host_of(base).unwrap_or_default()));
    let cookie = get_session(st, base)?;
    say(&format!("asking for the recovery image of board {board_id} (os={os}{})", if mlb == MLB_ZERO { String::new() } else { format!(", serial part {mlb}") }));
    let info = get_image_info(st, base, &cookie, board_id, mlb, os, &Ids::random())?;
    let identity = format!("{} {}", info.product, info.image_url);
    for u in [&info.image_url, &info.chunklist_url] {
        if !policy.allows(u) {
            return Err(format!("Apple's service named {u}, which is not an Apple address; refused"));
        }
    }
    say(&format!("product {}; image host {}; fetching the chunk list", info.product, host_of(&info.image_url).unwrap_or_default()));
    let list = fetch_chunklist(st, policy, &info)?;
    say(&format!("the image is {} bytes in {} chunks", list.total(), list.chunks.len()));
    let dest = dir.join("BaseSystem.dmg");
    // A part whose marker names another image (another address or hash) is a different build and would only be appended to. A part
    // with no marker (from an older start of the job) is resumed; if it is wrong the check below catches it and the whole image is
    // fetched once more.
    let marker = dir.join("BaseSystem.dmg.id");
    let id = format!("{} {}", info.image_url, info.image_hash);
    if dest.exists() && std::fs::read_to_string(&marker).ok().is_some_and(|m| m != id) {
        say("a part of an image from an earlier try is not this image; starting the download over");
        let _ = std::fs::remove_file(&dest);
    }
    let _ = std::fs::write(&marker, &id);
    let mut verified = Err(String::new());
    for pass in 0..2 {
        let mut last_pct = u64::MAX;
        let got = download(st, policy, &info.image_url, &info.image_token, &dest, MAX_IMAGE, retries, &mut |d, t| {
            let pct = if t > 0 { d * 100 / t } else { 0 };
            if pct != last_pct {
                say(&format!("downloading {pct}% ({} of {} MB)", d >> 20, t >> 20));
                last_pct = pct;
            }
        }, cancel)?;
        if got.resumed_from > 0 {
            say(&format!("resumed after {} MB already on disk", got.resumed_from >> 20));
        }
        say(&format!("downloaded: {} bytes on disk ({} bytes came over the network)", got.bytes, got.bytes.saturating_sub(got.resumed_from)));
        say("checking the image against Apple's chunk list");
        let mut last = u64::MAX;
        verified = verify_image(&dest, &list, &mut |d, t| {
            let pct = if t > 0 { d * 100 / t } else { 0 };
            if pct / 10 != last {
                say(&format!("checking {pct}%"));
                last = pct / 10;
            }
        }, cancel);
        let Err(why) = &verified else { break };
        // a damaged file must not be resumed from: remove it
        let _ = std::fs::remove_file(&dest);
        if why == "stopped" || pass == 1 {
            break;
        }
        say(&format!("{why}; downloading the whole image once more"));
    }
    verified?;
    if info.image_hash.len() == 64 && info.image_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        match file_sha256(&dest) {
            Ok(h) if h.eq_ignore_ascii_case(&info.image_hash) => say("the image also matches the hash Apple sent (AH)"),
            Ok(_) => say("note: the image does not match the AH value Apple sent; what AH covers is not documented, the chunk list check passed"),
            Err(_) => {}
        }
    }
    say("the image is complete and every chunk matches");
    Ok((dest, identity))
}

#[cfg(test)]
#[path = "macrecovery_tests.rs"]
mod tests;
