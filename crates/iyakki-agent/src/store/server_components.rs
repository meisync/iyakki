//! The private store's system components: streamed blobs (`PUT`/`GET /v1/blobs/<sha256>`), the signed `components.json`, publishing,
//! withdrawing and the retention cleanup (docs/development/COMPONENT-UPDATES-PLAN.md, docs/development/COMPONENT-STORE-FINDINGS.md).
//!
//! This is a child of `server.rs` (it uses the server's own helpers) and does not touch the text/base64 package path or its caps.
//!
//! * A blob is stored as `blobs/<sha256>`, never rewritten. An upload goes to `incoming/<sha256>.part` through a 64 KiB buffer while the
//!   SHA-256 is computed; it moves to `blobs/` only when the bytes match the hash in the address. An upload can be split in pieces with
//!   `Content-Range: bytes a-b/total` (a cut connection resumes where the part file ends) and the pieces are hashed at the end.
//! * A download streams the file through the same buffer and answers `Range` / `If-Range`; the answer is immutable and cacheable.
//! * `POST /v1/components` publishes an entry for blobs that are already there; `GET /v1/components.json` is the signed list.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::super::compmanifest::{self as cm, Record};
use super::{atomic_write, clean, Req, Resp, StoreServer, Who};
use crate::devmode::update::compare_versions;

/// The buffer of every transfer: nothing larger is ever held for a blob.
pub const BUF: usize = 64 << 10;
/// A blob nobody refers to is removed by the cleanup after this long (an upload that was never published).
const ORPHAN_AGE: i64 = 86_400;
const PART_AGE: i64 = 7 * 86_400;

pub enum Body {
    None,
    Bytes(Vec<u8>),
    /// Already positioned; `StreamResp::len` bytes are sent.
    File(std::fs::File),
}

/// An answer that may be a file: sent by `server_http` in pieces.
pub struct StreamResp {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    /// The `Content-Length` (for a HEAD it is the length of the body that is not sent).
    pub len: u64,
    pub body: Body,
}

impl StreamResp {
    fn from_resp(r: Resp) -> StreamResp {
        StreamResp { status: r.status, content_type: r.content_type, len: r.body.len() as u64, headers: r.headers, body: Body::Bytes(r.body) }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct CState {
    serial: u64,
    records: Vec<Record>,
    /// The IYAKKI release the store's list is signed as (the newest a publisher named).
    #[serde(default)]
    release: Option<cm::ReleaseLabel>,
}

/// How a `Range` header applies to a body of `len` bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum Slice {
    Whole,
    Part(u64, u64),
    Unsatisfiable,
}

/// One byte range (`a-b`, `a-`, `-n`); several ranges or anything unknown is `Whole` (a full answer is always allowed).
pub fn parse_range(h: &str, len: u64) -> Slice {
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return Slice::Whole };
    if spec.contains(',') {
        return Slice::Whole;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return Slice::Whole };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        return match b.parse::<u64>() {
            Ok(n) if n > 0 && len > 0 => Slice::Part(len.saturating_sub(n), len - 1),
            Ok(_) => Slice::Unsatisfiable,
            Err(_) => Slice::Whole,
        };
    }
    let Ok(start) = a.parse::<u64>() else { return Slice::Whole };
    let end = if b.is_empty() { len.saturating_sub(1) } else { match b.parse::<u64>() { Ok(e) => e.min(len.saturating_sub(1)), Err(_) => return Slice::Whole } };
    if start >= len || end < start {
        return Slice::Unsatisfiable;
    }
    Slice::Part(start, end)
}

/// `Content-Range: bytes a-b/total` of an upload piece.
fn parse_content_range(h: &str) -> Option<(u64, u64, u64)> {
    let r = h.trim().strip_prefix("bytes ")?;
    let (span, total) = r.split_once('/')?;
    let (a, b) = span.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total.trim().parse().ok()?))
}

struct Claim<'a>(&'a StoreServer, String);

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.0.uploading.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.1);
    }
}

fn hash_file(p: &std::path::Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; BUF];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(crate::pkg::hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

impl StoreServer {
    fn cstate(&self) -> CState {
        std::fs::read(self.path("components-state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_cstate(&self, st: &CState) -> Result<(), String> {
        atomic_write(&self.path("components-state.json"), &serde_json::to_vec(st).unwrap_or_default())
    }

    fn component_cap(&self, component: &str) -> u64 {
        self.cfg.component_caps.get(component).copied().unwrap_or(self.cfg.max_component)
    }

    fn blob_path(&self, sha: &str) -> PathBuf {
        self.path("blobs").join(sha)
    }

    /// Who may change things here: an administrator or a publisher, and not on a read-only store. The owner id, or the answer to send.
    fn writer(&self, r: &Req, peer: &str) -> Result<String, Resp> {
        if self.locked_out(peer) {
            return Err(Resp::err(429, "too many failed sign-ins; wait a minute"));
        }
        let owner = match self.auth(r) {
            Some(Who::Admin) => "admin".to_string(),
            Some(Who::Publisher(p)) => p.id,
            None => return Err(self.unauthorised(peer, &r.path)),
        };
        if self.cfg.read_only {
            self.audit(peer, &owner, "component", &r.path, "403-read-only");
            return Err(Resp::err(403, "this store is read-only"));
        }
        Ok(owner)
    }

    // ---------------------------------------------------------------------------------------- blobs

    /// `PUT /v1/blobs/<sha256>?component=NAME`: `len` bytes from `body`, streamed. `Content-Range: bytes a-b/total` sends a piece.
    pub fn blob_put(&self, r: &Req, len: u64, body: &mut dyn Read, peer: &str) -> Resp {
        let owner = match self.writer(r, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if !self.allow(&format!("b:{peer}"), 240) {
            return Resp::err(429, "too many uploads from this address; wait a minute");
        }
        let sha = r.path.strip_prefix("/v1/blobs/").unwrap_or("");
        if !cm::is_sha(sha) {
            return Resp::err(400, "the address is /v1/blobs/<64 lowercase hex characters: the SHA-256 of the bytes>");
        }
        let comp = r.query.get("component").map(String::as_str).unwrap_or("");
        if !self.cfg.component_names.iter().any(|c| c == comp) {
            return Resp::err(400, &format!("say which component the blob is for (?component=): {}", self.cfg.component_names.join(", ")));
        }
        let (start, total) = match r.headers.get("content-range") {
            None => (0, len),
            Some(h) => match parse_content_range(h) {
                Some((a, b, t)) if b >= a && b - a + 1 == len => (a, t),
                _ => return Resp::err(400, "Content-Range: bytes <first>-<last>/<total>, and the body is that many bytes"),
            },
        };
        let cap = self.component_cap(comp);
        if total == 0 || len == 0 || start.checked_add(len).is_none_or(|e| e > total) {
            return Resp::err(400, "an empty blob, or a piece beyond the total");
        }
        if total > cap {
            self.audit(peer, &owner, "blob", &format!("{comp} {sha} {total}"), "413");
            return Resp::err(413, &format!("a {comp} blob is limited to {} MiB; this one is {} MiB", cap >> 20, total >> 20));
        }
        if self.blob_path(sha).is_file() {
            let _ = std::io::copy(&mut body.take(len), &mut std::io::sink());
            return Resp::json(200, json!({"ok": true, "existing": true, "sha256": sha, "size": std::fs::metadata(self.blob_path(sha)).map(|m| m.len()).unwrap_or(0)}));
        }
        if !self.uploading.lock().unwrap_or_else(|e| e.into_inner()).insert(sha.to_string()) {
            return Resp::err(409, "this blob is being uploaded right now");
        }
        let _claim = Claim(self, sha.to_string());
        let incoming = self.path("incoming");
        if let Err(e) = std::fs::create_dir_all(&incoming).and_then(|_| std::fs::create_dir_all(self.path("blobs"))) {
            return Resp::err(500, &format!("{}: {e}", incoming.display()));
        }
        let part = incoming.join(format!("{sha}.part"));
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if start != 0 && start != have {
            return Resp::json(409, json!({"ok": false, "error": format!("the store has the first {have} bytes of this blob: continue from there"), "have": have}));
        }
        let kept = if start == 0 { 0 } else { have };
        if let Some(free) = (self.free_space)(&self.cfg.data) {
            let need = (total - kept).saturating_add(self.cfg.disk_margin);
            if free < need {
                self.audit(peer, &owner, "blob", &format!("{comp} {sha} {total}"), "507");
                return Resp::err(507, &format!("not enough free disk on the store: {} MiB needed, {} MiB free", need >> 20, free >> 20));
            }
        }
        let file = if start == 0 { std::fs::File::create(&part) } else { std::fs::OpenOptions::new().append(true).open(&part) };
        let mut f = match file {
            Ok(f) => f,
            Err(e) => return Resp::err(500, &format!("{}: {e}", part.display())),
        };
        let mut hasher = (start == 0 && len == total).then(Sha256::new);
        let mut buf = vec![0u8; BUF];
        let mut left = len;
        while left > 0 {
            let want = (BUF as u64).min(left) as usize;
            match body.read(&mut buf[..want]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Err(e) = f.write_all(&buf[..n]) {
                        return Resp::err(507, &format!("could not write the upload: {e}"));
                    }
                    if let Some(h) = hasher.as_mut() {
                        h.update(&buf[..n]);
                    }
                    left -= n as u64;
                }
            }
        }
        let _ = f.flush();
        let now_have = kept + (len - left);
        if left > 0 {
            // the connection ended early: the part stays so the upload can continue
            return Resp::json(400, json!({"ok": false, "error": "the upload stopped early; send the rest with Content-Range from the byte named in `have`", "have": now_have}));
        }
        if now_have < total {
            return Resp::json(200, json!({"ok": true, "complete": false, "have": now_have}));
        }
        let _ = f.sync_all();
        drop(f);
        let got = match hasher {
            Some(h) => crate::pkg::hex(&h.finalize()),
            None => match hash_file(&part) {
                Ok(h) => h,
                Err(e) => return Resp::err(500, &e),
            },
        };
        if got != sha {
            let _ = std::fs::remove_file(&part);
            self.audit(peer, &owner, "blob", &format!("{comp} {sha}"), "422-hash");
            return Resp::err(422, "the bytes do not match the SHA-256 in the address: nothing was kept");
        }
        if let Err(e) = std::fs::rename(&part, self.blob_path(sha)) {
            return Resp::err(500, &format!("{e}"));
        }
        self.audit(peer, &owner, "blob", &format!("{comp} {sha} {total}"), "201");
        Resp::json(201, json!({"ok": true, "complete": true, "sha256": sha, "size": total}))
    }

    /// `GET`/`HEAD /v1/blobs/<sha256>`: the file in pieces, with `Range` and `If-Range`. Public (the address is the hash).
    pub fn blob_get(&self, r: &Req, peer: &str) -> StreamResp {
        if !self.allow(&format!("r:{peer}"), self.cfg.reads_per_min) {
            return StreamResp::from_resp(Resp::err(429, "too many requests; wait a minute"));
        }
        let sha = r.path.strip_prefix("/v1/blobs/").unwrap_or("");
        let none = |c: u16, m: &str| StreamResp::from_resp(Resp::err(c, m));
        // a status question: how much of an unfinished upload the store holds (publishers only)
        if let Some(s) = sha.strip_suffix("/part") {
            if !cm::is_sha(s) {
                return none(404, "no such blob");
            }
            return match self.writer(r, peer) {
                Err(e) => StreamResp::from_resp(e),
                Ok(_) => {
                    let have = std::fs::metadata(self.path("incoming").join(format!("{s}.part"))).map(|m| m.len()).unwrap_or(0);
                    StreamResp::from_resp(Resp::json(200, json!({"have": have, "exists": self.blob_path(s).is_file()})))
                }
            };
        }
        if !cm::is_sha(sha) {
            return none(404, "no such blob");
        }
        let Ok(mut f) = std::fs::File::open(self.blob_path(sha)) else { return none(404, "no such blob") };
        let Ok(len) = f.metadata().map(|m| m.len()) else { return none(404, "no such blob") };
        let etag = format!("\"{sha}\"");
        let mut headers = vec![("Accept-Ranges", "bytes".to_string()), ("ETag", etag.clone()), ("Cache-Control", "public, max-age=31536000, immutable".to_string()), ("X-Content-Type-Options", "nosniff".to_string())];
        let ranged = r.headers.get("range").filter(|_| r.headers.get("if-range").is_none_or(|v| v.trim() == etag));
        let slice = ranged.map_or(Slice::Whole, |h| parse_range(h, len));
        let head = r.method == "HEAD";
        match slice {
            Slice::Unsatisfiable => {
                headers.push(("Content-Range", format!("bytes */{len}")));
                StreamResp { status: 416, content_type: "application/json", headers, len: 0, body: Body::None }
            }
            Slice::Part(a, b) => {
                headers.push(("Content-Range", format!("bytes {a}-{b}/{len}")));
                let body = if head || f.seek(SeekFrom::Start(a)).is_err() { Body::None } else { Body::File(f) };
                StreamResp { status: 206, content_type: "application/octet-stream", headers, len: b - a + 1, body }
            }
            Slice::Whole => StreamResp { status: 200, content_type: "application/octet-stream", headers, len, body: if head { Body::None } else { Body::File(f) } },
        }
    }

    // ---------------------------------------------------------------------------------------- the list

    /// `GET /v1/components.json`: signed with the catalog key when asked, from the state.
    pub(super) fn components_doc(&self) -> Resp {
        let st = self.cstate();
        Resp::new(200, "application/json", cm::build_doc_with(&self.cfg.id, st.serial.max(1), self.now(), &st.records, st.release.as_ref(), &self.key).into_bytes())
    }

    /// `POST /v1/components`: publish an entry for blobs that are already uploaded.
    pub(super) fn component_publish(&self, r: &Req, peer: &str) -> Resp {
        let owner = match self.writer(r, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if r.body.len() > 64 << 10 {
            return Resp::err(413, "the entry is a small JSON document");
        }
        let mut e: cm::Entry = match serde_json::from_slice(&r.body) {
            Ok(e) => e,
            Err(err) => return Resp::err(400, &format!("not a component entry: {err}")),
        };
        let release: Option<cm::ReleaseLabel> = match serde_json::from_slice::<Value>(&r.body).ok().and_then(|v| v.get("release").cloned()).filter(|v| !v.is_null()) {
            None => None,
            Some(v) => match serde_json::from_value::<cm::ReleaseLabel>(v).map_err(|e| e.to_string()).and_then(|l| l.check().map(|_| l)) {
                Ok(l) => Some(l),
                Err(m) => return Resp::err(422, &format!("not accepted: {m}")),
            },
        };
        e.added = self.now();
        if e.install_path.is_empty() {
            match cm::default_install_path(&e.component) {
                Some(p) => e.install_path = p.to_string(),
                None if self.cfg.component_names.contains(&e.component) => return Resp::err(422, &format!("{} has no default install path: give install_path", e.component)),
                None => {}
            }
        }
        if !self.cfg.component_names.contains(&e.component) {
            return Resp::err(422, &format!("this store does not take a component named {:?} ({})", e.component, self.cfg.component_names.join(", ")));
        }
        if let Err(m) = cm::check_entry(&e) {
            return Resp::err(422, &format!("not accepted: {m}"));
        }
        let cap = self.component_cap(&e.component);
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.cstate();
        if st.records.iter().any(|x| x.entry.component == e.component && x.owner != owner) {
            return Resp::err(403, &format!("the component name {} belongs to another publisher", e.component));
        }
        let top = st.records.iter().filter(|x| x.entry.component == e.component && x.entry.channel == e.channel).map(|x| x.entry.version.as_str()).max_by(|a, b| compare_versions(a, b));
        if let Some(top) = top {
            if compare_versions(&e.version, top) != std::cmp::Ordering::Greater {
                return Resp::err(409, &format!("version {} is not newer than {top} in channel {}: versions only go up", e.version, e.channel));
            }
        }
        let blob_size = |sha: &str| std::fs::metadata(self.blob_path(sha)).ok().filter(|m| m.is_file()).map(|m| m.len());
        match blob_size(&e.sha256) {
            None => return Resp::err(422, &format!("the artifact is not on this store: PUT /v1/blobs/{}?component={} first", e.sha256, e.component)),
            Some(n) if n != e.size => return Resp::err(422, &format!("the artifact on the store is {n} bytes but the entry says {}", e.size)),
            Some(n) if n > cap => return Resp::err(413, &format!("a {} artifact is limited to {} MiB", e.component, cap >> 20)),
            _ => {}
        }
        for d in &e.delta {
            match blob_size(&d.sha256) {
                None => return Resp::err(422, &format!("the delta {} is not on this store: upload it first", d.sha256)),
                Some(n) if n != d.size => return Resp::err(422, &format!("the delta {} is {n} bytes but the entry says {}", d.sha256, d.size)),
                _ => {}
            }
            if !st.records.iter().any(|x| !x.purged && x.yanked.is_none() && x.entry.component == e.component && x.entry.sha256 == d.from_sha256) {
                return Resp::err(422, &format!("the delta starts from {}, which is not a full {} artifact this store holds", d.from_sha256, e.component));
            }
        }
        if let (Some(new), Some(cur)) = (&release, &st.release) {
            if compare_versions(&new.version, &cur.version) == std::cmp::Ordering::Less {
                return Resp::err(409, &format!("release {} is older than the {} this store's list is signed as: releases only go up", new.version, cur.version));
            }
        }
        st.records.push(Record { entry: e.clone(), owner: owner.clone(), yanked: None, purged: false });
        if release.is_some() {
            st.release = release;
        }
        st.serial += 1;
        if let Err(m) = self.save_cstate(&st) {
            return Resp::err(500, &m);
        }
        self.audit(peer, &owner, "component", &format!("{} {} {} {}", e.component, e.version, e.channel, e.sha256), "200");
        Resp::json(200, json!({"ok": true, "component": e.component, "version": e.version, "channel": e.channel, "sha256": e.sha256, "serial": st.serial}))
    }

    /// `DELETE /v1/components/<component>[/<version>]?channel=&reason=`: the administrator or the publisher who owns the name.
    pub(super) fn component_yank(&self, r: &Req, peer: &str) -> Resp {
        let owner = match self.writer(r, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        let rest = &r.path["/v1/components/".len()..];
        let (name, version) = match rest.split_once('/') {
            Some((n, v)) => (n, Some(v)),
            None => (rest, None),
        };
        if !cm::valid_component_name(name) || version.is_some_and(|v| !cm::valid_version(v)) {
            return Resp::err(400, "give a component name and optionally a version");
        }
        let channel = r.query.get("channel").map(String::as_str);
        let reason: String = clean(r.query.get("reason").map_or("withdrawn", String::as_str)).chars().take(120).collect();
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.cstate();
        if !st.records.iter().any(|x| x.entry.component == name) {
            return Resp::err(404, "no such component");
        }
        if owner != "admin" && st.records.iter().any(|x| x.entry.component == name && x.owner != owner) {
            return Resp::err(403, "only the administrator or the publisher who owns this component can withdraw it");
        }
        let mut n = 0;
        for x in st.records.iter_mut().filter(|x| x.entry.component == name && version.is_none_or(|v| v == x.entry.version) && channel.is_none_or(|c| c == x.entry.channel) && x.yanked.is_none()) {
            x.yanked = Some(reason.clone());
            n += 1;
        }
        if n == 0 {
            return Resp::err(404, "no such published version");
        }
        st.serial += 1;
        if let Err(m) = self.save_cstate(&st) {
            return Resp::err(500, &m);
        }
        self.audit(peer, &owner, "component-yank", &format!("{name} {} {reason}", version.unwrap_or("all")), "200");
        Resp::json(200, json!({"ok": true, "yanked": n, "serial": st.serial}))
    }

    /// `POST /v1/components/retention?keep=3[&apply=1]` (administrator): what a cleanup would remove, and with `apply=1` removes it.
    pub(super) fn component_retention(&self, r: &Req, peer: &str) -> Resp {
        match self.writer(r, peer) {
            Ok(o) if o == "admin" => {}
            Ok(_) => return Resp::err(403, "only the administrator runs the cleanup"),
            Err(e) => return e,
        }
        let keep: usize = r.query.get("keep").and_then(|k| k.parse().ok()).unwrap_or(3);
        if !(1..=50).contains(&keep) {
            return Resp::err(400, "keep is a number from 1 to 50");
        }
        let apply = r.query.get("apply").is_some_and(|v| v == "1" || v == "true");
        let _g = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = self.cstate();
        let plan = cm::plan_retention(&st.records, keep);
        let after = cm::apply_retention(&st.records, &plan);
        let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for x in after.iter().filter(|x| !x.purged) {
            referenced.insert(x.entry.sha256.clone());
            referenced.extend(x.entry.delta.iter().map(|d| d.sha256.clone()));
        }
        let (now, mut orphans, mut orphan_bytes) = (self.now(), vec![], 0u64);
        for f in std::fs::read_dir(self.path("blobs")).into_iter().flatten().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Ok(md) = f.metadata() else { continue };
            let age = md.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| now - d.as_secs() as i64);
            if cm::is_sha(&name) && !referenced.contains(&name) && !plan.remove_blobs.contains(&name) && age >= ORPHAN_AGE {
                orphan_bytes += md.len();
                orphans.push(name);
            }
        }
        let mut stale_parts = vec![];
        for f in std::fs::read_dir(self.path("incoming")).into_iter().flatten().flatten() {
            let age = f.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| now - d.as_secs() as i64);
            if age >= PART_AGE {
                stale_parts.push(f.path());
            }
        }
        let mut removed = 0usize;
        if apply && (!plan.is_empty() || !orphans.is_empty() || !stale_parts.is_empty()) {
            for h in plan.remove_blobs.iter().chain(&orphans) {
                if std::fs::remove_file(self.blob_path(h)).is_ok() {
                    removed += 1;
                }
            }
            for p in &stale_parts {
                let _ = std::fs::remove_file(p);
            }
            st.records = after;
            st.serial += 1;
            if let Err(m) = self.save_cstate(&st) {
                return Resp::err(500, &m);
            }
            self.audit(peer, "admin", "component-retention", &format!("keep {keep}: {} blobs, {} bytes", removed, plan.bytes + orphan_bytes), "200");
        }
        let row = |t: &(String, String, String)| json!({"component": t.0, "channel": t.1, "version": t.2});
        Resp::json(
            200,
            json!({
                "ok": true, "applied": apply, "keep": keep,
                "drop_entries": plan.drop_entries.iter().map(row).collect::<Vec<_>>(),
                "purge_yanked": plan.purge_yanked.iter().map(row).collect::<Vec<_>>(),
                "drop_deltas": plan.drop_deltas.iter().map(|d| json!({"component": d.0, "version": d.1, "sha256": d.2})).collect::<Vec<_>>(),
                "remove_blobs": plan.remove_blobs.len() + orphans.len(), "orphans": orphans.len(), "stale_uploads": stale_parts.len(),
                "bytes": plan.bytes + orphan_bytes, "removed": removed,
            }),
        )
    }
}
