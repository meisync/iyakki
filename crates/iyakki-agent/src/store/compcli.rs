//! `iyakki store component ...`: publish system components to a private store (the publisher's side), list what a store offers, plan an
//! update from it (the device's side, nothing is applied) and run the retention cleanup. See docs/development/COMPONENT-UPDATES-PLAN.md.
//!
//! Publishing is "upload the blob(s), then post the entry": a blob goes up in pieces of at most [`CHUNK`] bytes (`Content-Range`), so only one
//! piece is in memory and a cut connection resumes from what the store has. The token is read from an environment variable, never from the line.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::compmanifest as cm;
use super::components::{self as comps, human, PinnedStore, Release};

pub const USAGE: &str = "iyakki store component <command>
  publish --server URL --token-env NAME --component C --version V --file F [--channel nightly] [--requires k=range]... [--built-from TEXT]
          [--install-path P] [--release VERSION --release-variant NAME] [--delta-from OLD_FILE]...
                                   upload the artifact (and a delta from each OLD_FILE, which must already be published) and list it;
                                   --release says which IYAKKI release (version and image variant) the store's list is signed as
  list --server URL [--key PUBHEX]   what the store offers (verified when the store's public key is given)
  plan --server URL --key PUBHEX [--channel stable] [--data DIR] [--stage DIR [--rest-from BOOTFILES] [--version V] [--variant NAME]]
                                   what this device would take (installed versions from DIR/components-installed.json, corrected from the running system); with --stage the
                                   files are downloaded and checked and an update payload or patch pack is made, labelled with the release the store's
                                   signed list names (--version/--variant override it). Nothing is applied.
  yank --server URL --token-env NAME --component C [--version V] [--channel CH] [--reason TEXT]
  retention --server URL --token-env NAME [--keep 3] [--apply]
                                   what a cleanup would remove (newest N per component and channel, and the deltas between them); --apply does it";

/// The size of one uploaded piece.
pub const CHUNK: u64 = 32 << 20;

struct Args {
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse(a: &[String]) -> Result<Args, String> {
        let mut flags = BTreeMap::<String, Vec<String>>::new();
        let mut it = a.iter();
        while let Some(x) = it.next() {
            match x.strip_prefix("--") {
                Some(f @ "apply") => flags.entry(f.into()).or_default().push("1".into()),
                Some(f) => flags.entry(f.into()).or_default().push(it.next().ok_or_else(|| format!("--{f} needs a value"))?.clone()),
                None => return Err(format!("unexpected {x:?}\n{USAGE}")),
            }
        }
        Ok(Args { flags })
    }

    fn one(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(String::as_str)
    }

    fn need(&self, k: &str) -> Result<&str, String> {
        self.one(k).ok_or_else(|| format!("--{k} is required"))
    }

    fn many(&self, k: &str) -> Vec<String> {
        self.flags.get(k).cloned().unwrap_or_default()
    }
}

fn token(a: &Args) -> Result<String, String> {
    let var = a.need("token-env")?;
    std::env::var(var).map_err(|_| format!("the environment variable {var} is not set (it holds your publish token; never put the token on the command line)"))
}

fn base_of(a: &Args) -> Result<String, String> {
    let base = super::author::server_base(a.need("server")?)?;
    if base.starts_with("http://") && !super::author::private_host(&base) {
        return Err("the token would travel unencrypted: use an https address, or a loopback or private-network address".into());
    }
    Ok(base)
}

/// `args` are the words after `iyakki store`.
pub fn run(args: &[String]) -> Result<String, String> {
    if args.first().map(String::as_str) != Some("component") {
        return Err(format!("usage: iyakki store component ...\n{USAGE}"));
    }
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
    let a = Args::parse(args.get(2..).unwrap_or(&[]))?;
    let up = iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(900));
    match cmd {
        "help" | "--help" | "-h" => Ok(USAGE.to_string()),
        "publish" => {
            let base = base_of(&a)?;
            let mut requires = BTreeMap::new();
            for r in a.many("requires") {
                let (k, v) = r.split_once('=').ok_or_else(|| format!("--requires {r:?}: write name=range, for example kernel=>=6.12"))?;
                requires.insert(k.trim().to_string(), v.trim().to_string());
            }
            let req = PublishReq {
                component: a.need("component")?.into(),
                version: a.need("version")?.into(),
                channel: a.one("channel").unwrap_or("nightly").into(),
                file: PathBuf::from(a.need("file")?),
                requires,
                built_from: a.one("built-from").unwrap_or("").into(),
                install_path: a.one("install-path").unwrap_or("").into(),
                release: match (a.one("release"), a.one("release-variant")) {
                    (Some(v), Some(variant)) => Some(cm::ReleaseLabel { version: v.into(), variant: variant.into() }),
                    (None, None) => None,
                    _ => return Err("--release and --release-variant go together".into()),
                },
                delta_from: a.many("delta-from").into_iter().map(PathBuf::from).collect(),
            };
            let mut lines = vec![];
            let r = publish_component(&up, &base, &token(&a)?, &req, &mut |s| lines.push(s.to_string()))?;
            lines.push(r);
            Ok(lines.join("\n"))
        }
        "list" => list(&up, a.need("server")?, a.one("key")),
        "plan" => device_plan(&a),
        "yank" => {
            let (base, tok) = (base_of(&a)?, token(&a)?);
            let c = a.need("component")?;
            let path = match a.one("version") {
                Some(v) => format!("{base}/v1/components/{c}/{v}"),
                None => format!("{base}/v1/components/{c}"),
            };
            let mut q = vec![];
            if let Some(ch) = a.one("channel") {
                q.push(format!("channel={ch}"));
            }
            q.push(format!("reason={}", enc(a.one("reason").unwrap_or("withdrawn"))));
            let r = call(&up, "DELETE", &format!("{path}?{}", q.join("&")), &tok, &[], &[])?;
            Ok(format!("withdrawn {} version(s); the list is now serial {}", r["yanked"], r["serial"]))
        }
        "retention" => {
            let (base, tok) = (base_of(&a)?, token(&a)?);
            let apply = a.one("apply").is_some();
            let r = call(&up, "POST", &format!("{base}/v1/components/retention?keep={}{}", a.one("keep").unwrap_or("3"), if apply { "&apply=1" } else { "" }), &tok, &[], &[])?;
            Ok(retention_text(&r))
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    }
}

pub fn retention_text(r: &Value) -> String {
    let names = |k: &str| r[k].as_array().map(|a| a.iter().map(|x| format!("{} {} ({})", x["component"].as_str().unwrap_or("?"), x["version"].as_str().unwrap_or("?"), x["channel"].as_str().unwrap_or("-"))).collect::<Vec<_>>()).unwrap_or_default();
    let applied = r["applied"].as_bool() == Some(true);
    let mut out = vec![format!("Keep the newest {} per component and channel: {} version(s) leave the list, {} withdrawn version(s) lose their files, {} file(s) ({}) {}.", r["keep"], names("drop_entries").len(), names("purge_yanked").len(), r["remove_blobs"], human(r["bytes"].as_u64().unwrap_or(0)), if applied { "were removed" } else { "would be removed" })];
    for n in names("drop_entries") {
        out.push(format!("  leaves: {n}"));
    }
    for n in names("purge_yanked") {
        out.push(format!("  files removed (withdrawal notice stays): {n}"));
    }
    if r["orphans"].as_u64().unwrap_or(0) > 0 {
        out.push(format!("  {} uploaded file(s) that no entry uses", r["orphans"]));
    }
    if !applied {
        out.push("Nothing was removed: add --apply to do it.".into());
    }
    out.join("\n")
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn call(up: &dyn iyakki_proxy::Upstream, method: &str, url: &str, tok: &str, extra: &[(String, String)], body: &[u8]) -> Result<Value, String> {
    let mut h = vec![("Authorization".to_string(), format!("Bearer {}", tok.trim()))];
    h.extend(extra.iter().cloned());
    let r = up.request(method, url, &h, body).map_err(|e| format!("could not reach the store: {e}"))?;
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    if (200..300).contains(&r.status) {
        return Ok(v);
    }
    let said = v["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default();
    Err(match r.status {
        401 | 403 => format!("the store did not accept the token ({}{said})", r.status),
        _ => format!("the store answered {}{said}", r.status),
    })
}

// ------------------------------------------------------------------------------------------------ publishing

#[derive(Clone, Debug)]
pub struct PublishReq {
    pub component: String,
    pub version: String,
    pub channel: String,
    pub file: PathBuf,
    pub requires: BTreeMap<String, String>,
    pub built_from: String,
    pub install_path: String,
    /// The IYAKKI release this build belongs to; the store signs the newest one it was told in its list.
    pub release: Option<cm::ReleaseLabel>,
    pub delta_from: Vec<PathBuf>,
}

enum Source {
    File(PathBuf),
    Mem(Vec<u8>),
}

impl Source {
    fn read_at(&self, off: u64, n: u64) -> Result<Vec<u8>, String> {
        match self {
            Source::Mem(b) => Ok(b[off as usize..(off + n) as usize].to_vec()),
            Source::File(p) => {
                let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
                f.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; n as usize];
                f.read_exact(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
                Ok(buf)
            }
        }
    }
}

/// SHA-256 and size of a file, read through a small buffer.
pub fn hash_file(p: &Path) -> Result<(String, u64), String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let (mut h, mut buf, mut n) = (Sha256::new(), vec![0u8; 1 << 20], 0u64);
    loop {
        let k = f.read(&mut buf).map_err(|e| e.to_string())?;
        if k == 0 {
            return Ok((crate::pkg::hex(&h.finalize()), n));
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
}

/// Upload one blob in pieces; skips what the store has and continues what it holds of an unfinished upload.
fn upload(up: &dyn iyakki_proxy::Upstream, base: &str, tok: &str, component: &str, sha: &str, total: u64, src: &Source, chunk: u64, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let status = call(up, "GET", &format!("{base}/v1/blobs/{sha}/part"), tok, &[], &[])?;
    if status["exists"].as_bool() == Some(true) {
        out(&format!("  {sha:.12}: already on the store"));
        return Ok(());
    }
    let mut off = status["have"].as_u64().unwrap_or(0).min(total);
    if off > 0 {
        out(&format!("  {sha:.12}: the store has the first {}; continuing", human(off)));
    }
    let mut tries = 0;
    while off < total {
        let n = chunk.min(total - off);
        let body = src.read_at(off, n)?;
        let h = vec![("Content-Type".to_string(), "application/octet-stream".to_string()), ("Content-Range".to_string(), format!("bytes {off}-{}/{total}", off + n - 1))];
        let r = up.request("PUT", &format!("{base}/v1/blobs/{sha}?component={component}"), &[vec![("Authorization".to_string(), format!("Bearer {}", tok.trim()))], h].concat(), &body);
        match r {
            Ok(r) if (200..300).contains(&r.status) => {
                off += n;
                tries = 0;
                out(&format!("  {sha:.12}: {} of {}", human(off), human(total)));
            }
            Ok(r) => {
                let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
                if r.status == 409 && v["have"].is_u64() && tries < 3 {
                    off = v["have"].as_u64().unwrap_or(0).min(total);
                    tries += 1;
                    continue;
                }
                return Err(match r.status {
                    401 | 403 => format!("the store did not accept the token ({})", r.status),
                    413 => format!("the store refuses a {component} file this large: {}", v["error"].as_str().unwrap_or("")),
                    422 => format!("the store says the bytes do not match their SHA-256: {}", v["error"].as_str().unwrap_or("")),
                    507 => format!("the store has no room: {}", v["error"].as_str().unwrap_or("")),
                    s => format!("the store answered {s}: {}", v["error"].as_str().unwrap_or("")),
                });
            }
            Err(e) if tries < 3 => {
                tries += 1;
                let again = call(up, "GET", &format!("{base}/v1/blobs/{sha}/part"), tok, &[], &[]).map_err(|x| format!("{e}; {x}"))?;
                off = again["have"].as_u64().unwrap_or(0).min(total);
            }
            Err(e) => return Err(format!("the upload keeps failing: {e}; run the same command again to continue from {}", human(off))),
        }
    }
    Ok(())
}

/// Upload the artifact (and the deltas), then post the entry.
pub fn publish_component(up: &dyn iyakki_proxy::Upstream, base: &str, tok: &str, req: &PublishReq, out: &mut dyn FnMut(&str)) -> Result<String, String> {
    publish_with_chunk(up, base, tok, req, CHUNK, out)
}

pub fn publish_with_chunk(up: &dyn iyakki_proxy::Upstream, base: &str, tok: &str, req: &PublishReq, chunk: u64, out: &mut dyn FnMut(&str)) -> Result<String, String> {
    if tok.trim().is_empty() {
        return Err("no publish token".into());
    }
    let (sha, size) = hash_file(&req.file)?;
    if size == 0 {
        return Err(format!("{} is empty", req.file.display()));
    }
    out(&format!("{} {} ({}), SHA-256 {}", req.component, req.version, human(size), &sha[..12]));
    let mut deltas = vec![];
    for old in &req.delta_from {
        let (old_sha, _) = hash_file(old)?;
        let (a, b) = (std::fs::read(old).map_err(|e| format!("{}: {e}", old.display()))?, std::fs::read(&req.file).map_err(|e| format!("{}: {e}", req.file.display()))?);
        let d = crate::bindelta::make_delta(&a, &b);
        drop((a, b));
        let dsha = crate::pkg::hex(&Sha256::digest(&d));
        out(&format!("delta from {:.12}: {} (the whole file is {})", old_sha, human(d.len() as u64), human(size)));
        deltas.push((cm::Delta { from_sha256: old_sha, sha256: dsha, size: d.len() as u64 }, d));
    }
    upload(up, base, tok, &req.component, &sha, size, &Source::File(req.file.clone()), chunk, out)?;
    for (d, bytes) in deltas.iter() {
        upload(up, base, tok, &req.component, &d.sha256, d.size, &Source::Mem(bytes.clone()), chunk, out)?;
    }
    let entry = json!({"component": req.component, "version": req.version, "sha256": sha, "size": size, "requires": req.requires, "channel": req.channel, "built_from": req.built_from, "install_path": req.install_path, "delta": deltas.iter().map(|d| &d.0).collect::<Vec<_>>()});
    let mut entry = entry;
    if let Some(r) = &req.release {
        entry["release"] = json!({"version": r.version, "variant": r.variant});
    }
    let r = call(up, "POST", &format!("{base}/v1/components"), tok, &[("Content-Type".to_string(), "application/json".to_string())], entry.to_string().as_bytes())?;
    Ok(format!("published {} {} to channel {} (list serial {})", req.component, req.version, req.channel, r["serial"]))
}

// ------------------------------------------------------------------------------------------------ list and plan

fn list(up: &dyn iyakki_proxy::Upstream, server: &str, key: Option<&str>) -> Result<String, String> {
    let url = cm::url_for_catalog(server);
    let r = up.request("GET", &url, &[], &[]).map_err(|e| format!("could not reach the store: {e}"))?;
    if r.status != 200 {
        return Err(format!("the store answered {} for its component list", r.status));
    }
    let (doc, note) = match key {
        Some(k) => (cm::verify(&r.body, k)?, "signature checked against the key you gave"),
        None => {
            let v: Value = serde_json::from_slice(&r.body).map_err(|_| "not a component list".to_string())?;
            let entries: Vec<cm::Entry> = serde_json::from_value(v["components"].clone()).map_err(|e| e.to_string())?;
            (cm::Doc { id: v["id"].as_str().unwrap_or("").into(), serial: v["serial"].as_u64().unwrap_or(0), issued: 0, entries, yanked: serde_json::from_value(v["yanked"].clone()).unwrap_or_default(), key: v["key"].as_str().unwrap_or("").into(), release: serde_json::from_value(v["release"].clone()).ok() }, "NOT VERIFIED: give --key with the store's public key to check the signature")
        }
    };
    let mut out = vec![format!("{} (list serial {}, key {}): {note}", doc.id, doc.serial, doc.key.get(..12).unwrap_or(""))];
    for e in &doc.entries {
        out.push(format!("  {:<13} {:<12} {:<8} {:>10}  {}{}{}", e.component, e.version, e.channel, human(e.size), &e.sha256[..12], if e.delta.is_empty() { String::new() } else { format!("  deltas from {}", e.delta.iter().map(|d| format!("{:.8} ({})", d.from_sha256, human(d.size))).collect::<Vec<_>>().join(", ")) }, if e.requires.is_empty() { String::new() } else { format!("  needs {}", e.requires.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")) }));
    }
    if doc.entries.is_empty() {
        out.push("  (no components published)".into());
    }
    for y in &doc.yanked {
        out.push(format!("  withdrawn: {} {} ({}): {}", y.component, y.version, y.channel, y.reason));
    }
    Ok(out.join("\n"))
}

fn device_plan(a: &Args) -> Result<String, String> {
    let server = a.need("server")?;
    let store = PinnedStore { name: server.to_string(), base: cm::base_of(server), pin: a.need("key")?.to_ascii_lowercase() };
    let up = iyakki_proxy::https::UreqUpstream::default();
    let doc = comps::fetch_doc(&up, &store)?;
    let inst = a.one("data").map(|d| super::compinstalled::current(Path::new(d), &super::compinstalled::Facts::real())).unwrap_or_default();
    let p = comps::plan(&doc, &store.name, a.one("channel").unwrap_or("stable"), &inst, None)?;
    let mut out = comps::plan_text(&p);
    if let Some(dir) = a.one("stage") {
        if p.steps.is_empty() {
            return Ok(out);
        }
        let st = iyakki_proxy::https::download_streamer();
        let mut last = (String::new(), 101u64);
        let staged = comps::stage(&p, &store.base, st.as_ref(), &inst, &Path::new(dir).join("staging"), 2, &mut |c, have, total| {
            let pct = if total == 0 { 100 } else { have * 100 / total };
            if (c != last.0 || pct / 10 != last.1 / 10) && total > (4 << 20) {
                eprintln!("  {c}: {pct}%");
                last = (c.to_string(), pct);
            }
        }, &|| false)?;
        // the label the store signed; the flags override it
        let from_doc = doc.release.as_ref();
        let release = Release {
            version: a.one("version").map(str::to_string).or_else(|| from_doc.map(|r| r.version.clone())).ok_or("the store's list carries no release label: give --version and --variant")?,
            variant: a.one("variant").map(str::to_string).or_else(|| from_doc.map(|r| r.variant.clone())).ok_or("the store's list carries no release label: give --version and --variant")?,
        };
        let h = comps::handover(&staged, Path::new(dir), &release, a.one("rest-from").map(Path::new), &inst)?;
        out = format!("{out}\n\n{}", comps::staged_text(&staged, &h));
    }
    Ok(out)
}
