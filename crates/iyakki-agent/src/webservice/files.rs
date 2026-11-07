//! The cloud profile's file and clone routes (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b). DENY BY DEFAULT.
//!
//! Every route does the same three things in the same order, and nothing else is possible:
//! 1. the person is the signed-in account (never a name from the request: unknown fields are refused);
//! 2. the right for the scope is checked with `cloudaccess::Access::check` (`cloudfiles::action`): viewers read, editors write, a person
//!    outside the namespace gets the answer of a namespace that is not there, a tenant without role lists allows nothing;
//! 3. the folder comes from `cloudstorage::resolve` (locate + mount + ensure + jail): `..`, absolute paths and links are refused.
//!
//! What a person can reach: `ns/<ns>/people/<person>` (scope `person`), `ns/<ns>/shared` (scope `shared`) and `personal/<person>`
//! (scope `personal`, the default without a namespace). The JSON routes carry text only (see `cloudfiles`); `PUT /api/v1/files/upload` and
//! `GET /api/v1/files/download` carry any bytes and are STREAMED: the connection hands them to [`serve_stream`] after the head has been read, so
//! a body is copied between the socket and a file in 64 KiB pieces (limit `cloudfiles::MAX_BINARY`, `Content-Length` required and checked
//! first, a short body is an error and leaves nothing behind). Both go through the same [`target`] as the text routes: same rights, same
//! jail. The ordinary pipeline (`handle`) also answers an upload whose whole body is already in memory (at most the service's body limit).

use super::handlers::{Ctx, Out};
use super::server::{json_body, Fail};
use crate::cloudaccess::{Access, Denial};
use crate::cloudfiles as fs;
use crate::httpmini::Response;
use crate::cloudstorage::{resolve, StorageLayout};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// (folder of the scope, the path asked for inside it)
pub(super) fn target(c: &Ctx, scope: Option<&str>, ns: Option<&str>, path: &str, write: bool) -> Result<(PathBuf, PathBuf), Fail> {
    let scope = fs::parse_scope(scope, ns).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    let me = &c.who().user;
    let acc = Access::new(&c.data_dir());
    let v = acc.check(me, &fs::action(&scope, write));
    c.sh.deps.audit.record(me, "access", &line(&v.audit));
    match &v.denial {
        None => {}
        Some(Denial::Forbidden(m)) => return Err(Fail::new(403, "denied", m)),
        Some(Denial::NotFound(m)) => return Err(Fail::new(404, "not_found", m)),
    }
    let (layout, root) = (StorageLayout::default(), acc.files_root());
    let bad = |e: String| Fail::new(400, "invalid_input", &e);
    let base = resolve(&layout, &root, me, &scope, "").map_err(bad)?;
    let full = resolve(&layout, &root, me, &scope, path).map_err(bad)?;
    Ok((base, full))
}

pub(super) fn io_fail(e: String) -> Fail {
    if e.contains("no such") {
        Fail::new(404, "not_found", &e)
    } else if e.contains("already exists") {
        Fail::new(409, "exists", &e)
    } else if e.contains("larger than") || e.contains("more than") {
        Fail::new(413, "too_large", &e)
    } else {
        Fail::new(400, "invalid_input", &e)
    }
}

pub(super) fn q<'a>(c: &'a Ctx, k: &str) -> Option<&'a str> {
    c.req.query.get(k).map(String::as_str)
}

pub fn list(c: &Ctx) -> Out {
    let path = q(c, "path").unwrap_or("");
    let (_, full) = target(c, q(c, "scope"), q(c, "namespace"), path, false)?;
    let entries = fs::list(&full).map_err(io_fail)?;
    let v: Vec<Value> = entries.iter().map(|e| json!({"name": e.name, "kind": e.kind, "size": e.size})).collect();
    Ok((200, json!({"path": path, "entries": v}), String::new()))
}

pub fn read(c: &Ctx) -> Out {
    let path = q(c, "path").ok_or_else(|| Fail::new(400, "invalid_input", "give ?path="))?;
    let (_, full) = target(c, q(c, "scope"), q(c, "namespace"), path, false)?;
    let text = fs::read_text(&full).map_err(io_fail)?;
    Ok((200, json!({"path": path, "content": text}), String::new()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteReq {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    path: String,
    content: String,
}

pub fn write(c: &Ctx) -> Out {
    let r: WriteReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"path\":..., \"content\":..., optional \"scope\", \"namespace\"}"))?;
    if r.path.is_empty() {
        return Err(Fail::new(400, "invalid_input", "give a file path"));
    }
    let (_, full) = target(c, r.scope.as_deref(), r.namespace.as_deref(), &r.path, true)?;
    fs::write_text(&full, &r.content).map_err(io_fail)?;
    Ok((200, json!({"ok": true}), line(&format!("path={} bytes={}", r.path, r.content.len()))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteReq {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    path: String,
    #[serde(default)]
    recursive: bool,
}

pub fn delete(c: &Ctx) -> Out {
    let r: DeleteReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"path\":..., optional \"recursive\", \"scope\", \"namespace\"}"))?;
    let (base, full) = target(c, r.scope.as_deref(), r.namespace.as_deref(), &r.path, true)?;
    fs::delete(&base, &full, r.recursive).map_err(io_fail)?;
    Ok((200, json!({"ok": true}), line(&format!("path={} recursive={}", r.path, r.recursive))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveReq {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    from: String,
    to: String,
    #[serde(default)]
    overwrite: bool,
}

/// `POST /api/v1/files/move`: rename or move inside ONE space (same scope and namespace for both paths). Needs the write right, like a write.
pub fn move_(c: &Ctx) -> Out {
    let r: MoveReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"from\":..., \"to\":..., optional \"overwrite\", \"scope\", \"namespace\"}"))?;
    // both paths go through the same jail check as every other route, then the move itself opens each folder without following links
    let (base, _) = target(c, r.scope.as_deref(), r.namespace.as_deref(), &r.from, true)?;
    target(c, r.scope.as_deref(), r.namespace.as_deref(), &r.to, true)?;
    fs::rename(&base, &r.from, &r.to, r.overwrite).map_err(io_fail)?;
    Ok((200, json!({"ok": true}), line(&format!("from={} to={} overwrite={}", r.from, r.to, r.overwrite))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MkdirReq {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    path: String,
}

/// `POST /api/v1/files/mkdir`: make a folder (and the folders above it). A name that exists is a 409.
pub fn mkdir(c: &Ctx) -> Out {
    let r: MkdirReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"path\":..., optional \"scope\", \"namespace\"}"))?;
    let (base, _) = target(c, r.scope.as_deref(), r.namespace.as_deref(), &r.path, true)?;
    fs::mkdir(&base, &r.path).map_err(io_fail)?;
    Ok((200, json!({"ok": true}), line(&format!("path={}", r.path))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloneReq {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    url: String,
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    depth: u32,
}

pub fn clone(c: &Ctx) -> Out {
    let r: CloneReq = serde_json::from_value(json_body(c.req)?).map_err(|_| Fail::new(400, "invalid_input", "send {\"url\":\"https://...\", optional \"folder\", \"branch\", \"depth\", \"scope\", \"namespace\"}"))?;
    fs::check_clone_url(&r.url).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    let folder = match &r.folder {
        Some(f) => f.clone(),
        None => fs::default_folder(&r.url).ok_or_else(|| Fail::new(400, "invalid_input", "name the folder"))?,
    };
    if !crate::cloudstorage::plain_name(&folder) {
        return Err(Fail::new(400, "invalid_input", "the folder is a plain name: letters, digits, - and _"));
    }
    let (base, dest) = target(c, r.scope.as_deref(), r.namespace.as_deref(), &folder, true)?;
    if dest.parent() != Some(base.as_path()) {
        return Err(Fail::new(400, "invalid_input", "the folder is a plain name"));
    }
    tenant_git_guard(c, r.namespace.as_deref(), &r.url)?;
    fs::git_clone(&r.url, &dest, "https", r.branch.as_deref(), r.depth, Duration::from_secs(fs::CLONE_SECS)).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    // a repository can hold links; with core.symlinks=false there should be none, and whatever leaves the folder is removed anyway
    let acc = Access::new(&c.data_dir());
    let removed = acc.after_checkout(&base).map_err(|e| Fail::new(500, "io", &e))?;
    Ok((200, json!({"folder": folder, "removed_links": removed.len()}), line(&format!("folder={folder}"))))
}

pub fn clones(c: &Ctx) -> Out {
    let (base, _) = target(c, q(c, "scope"), q(c, "namespace"), "", false)?;
    let v: Vec<Value> = fs::list_clones(&base).iter().map(|k| json!({"folder": k.folder, "branch": k.branch})).collect();
    Ok((200, json!({"clones": v}), String::new()))
}

// ------------------------------------------------------------------------------------------------------------------ binary

fn require_path(c: &Ctx) -> Result<String, Fail> {
    q(c, "path").filter(|p| !p.is_empty()).map(str::to_string).ok_or_else(|| Fail::new(400, "invalid_input", "give ?path= (the file)"))
}

/// The upload itself, for a body of `declared` bytes that `src` yields.
fn do_upload(c: &Ctx, declared: u64, src: &mut dyn Read) -> Result<(Value, String), Fail> {
    let path = require_path(c)?;
    let sha = c.req.header("x-content-sha256").map(str::to_string);
    let (_, full) = target(c, q(c, "scope"), q(c, "namespace"), &path, true)?;
    let (bytes, sum) = fs::write_stream(&full, src, declared, fs::MAX_BINARY, sha.as_deref()).map_err(io_fail)?;
    Ok((json!({"ok": true, "bytes": bytes, "sha256": sum}), line(&format!("path={path} bytes={bytes}"))))
}

/// `PUT /api/v1/files/upload` when the whole body is in memory already (the ordinary pipeline). Big bodies take [`serve_stream`].
pub fn upload(c: &Ctx) -> Out {
    let body = &c.req.body;
    let (v, detail) = do_upload(c, body.len() as u64, &mut &body[..])?;
    Ok((200, v, detail))
}

/// `GET /api/v1/files/download` is answered by [`serve_stream`] on the connection; the pipeline's JSON answer cannot carry bytes.
pub fn download(c: &Ctx) -> Out {
    let path = require_path(c)?;
    target(c, q(c, "scope"), q(c, "namespace"), &path, false)?;
    Err(Fail::new(400, "invalid_input", "a download is streamed: ask for it on a connection to the service"))
}

/// Is this head (the bytes read so far) one of the two streamed routes? Only a look at the request line.
pub(super) fn is_stream_head(head: &[u8]) -> bool {
    ["PUT /api/v1/files/upload", "GET /api/v1/files/download"].iter().any(|p| head.starts_with(p.as_bytes()) && matches!(head.get(p.len()), Some(b'?' | b' ')))
}

fn send<S: Write>(s: &mut S, r: &crate::httpmini::Response) {
    let _ = s.write_all(&r.to_bytes());
    let _ = s.flush();
}

/// Serve one streamed file request on `io`, whose head (and maybe the start of the body) is `buf`. Returns false when this is not a streamed
/// route of the running profile (the caller then runs the ordinary pipeline, which answers 404 and the like).
pub(super) fn serve_stream<S: Read + Write>(sh: &super::server::Shared, io: &mut S, buf: &[u8], peer: IpAddr) -> bool {
    use super::routes::{self, Found};
    use super::server::{authenticate_user, error_answer, with_origin};
    let head = match crate::httpmini::parse_head(buf) {
        Ok(Some(h)) => h,
        Ok(None) => return false,
        Err((code, msg)) => {
            send(io, &error_answer(code, "invalid_input", msg));
            return true;
        }
    };
    let Found::Route(route, _) = routes::find(sh.profile, &head.req.method, &head.req.path) else { return false };
    if route.id != "files_upload" && route.id != "files_download" {
        return false;
    }
    let t0 = std::time::Instant::now();
    let req = &head.req;
    let (p, origin) = match authenticate_user(sh, req, peer, route) {
        Ok(x) => x,
        Err(r) => {
            send(io, &r);
            return true;
        }
    };
    let ip = peer.to_string();
    let ctx = Ctx { sh, req, params: &[], principal: Some(&p), token: super::server::bearer(req), ip: &ip };
    let hsts = |r: crate::httpmini::Response| with_origin(r.header("Strict-Transport-Security", "max-age=31536000"), &origin);
    let fail = |f: Fail| hsts(error_answer(f.status, f.code, &f.message));
    let (status, detail) = if route.id == "files_upload" {
        let r = (|| -> Result<(Value, String), Fail> {
            let declared = head.content_length.ok_or_else(|| Fail::new(411, "length_required", "send Content-Length: the size of the file in bytes"))?;
            if declared > fs::MAX_BINARY {
                return Err(Fail::new(413, "too_large", &format!("the file is larger than {} MiB", fs::MAX_BINARY / (1024 * 1024))));
            }
            // the rights and the folder first, so a refused upload costs no bytes
            let path = require_path(&ctx)?;
            target(&ctx, q(&ctx, "scope"), q(&ctx, "namespace"), &path, true)?;
            if req.header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
                let _ = io.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
                let _ = io.flush();
            }
            let early = &buf[head.body_at.min(buf.len())..];
            let early = &early[..early.len().min(declared as usize)];
            let mut body = early.chain(&mut *io).take(declared);
            do_upload(&ctx, declared, &mut body)
        })();
        match r {
            Ok((v, detail)) => {
                send(io, &hsts(Response::json(200, &v)));
                (200, detail)
            }
            Err(f) => {
                let st = f.status;
                send(io, &fail(f));
                (st, "refused".to_string())
            }
        }
    } else {
        let r = (|| -> Result<(std::fs::File, u64, String), Fail> {
            let path = require_path(&ctx)?;
            let (_, full) = target(&ctx, q(&ctx, "scope"), q(&ctx, "namespace"), &path, false)?;
            let (f, len) = fs::open_for_download(&full).map_err(io_fail)?;
            Ok((f, len, fs::download_name(&full)))
        })();
        match r {
            Ok((mut f, len, name)) => {
                let resp = hsts(Response::new(200, "application/octet-stream", Vec::new()).header("Content-Disposition", &format!("attachment; filename=\"{name}\"")));
                let _ = io.write_all(&resp.to_head_bytes(len));
                match fs::copy_exact(&mut f, io, len) {
                    Ok(()) => {
                        let _ = io.flush();
                        (200, format!("bytes={len}"))
                    }
                    Err(e) => {
                        log::warn!("service: a download stopped: {e}");
                        (500, "interrupted".to_string())
                    }
                }
            }
            Err(f) => {
                let st = f.status;
                send(io, &fail(f));
                (st, "refused".to_string())
            }
        }
    };
    if route.audit {
        let detail: String = format!("{detail} status={status} addr={ip}").chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(120).collect();
        sh.deps.audit.record(&p.user, &format!("api.{}", route.id), &detail);
    }
    log::info!("service: {} {} {} {}ms user={}", req.method, route.id, status, t0.elapsed().as_millis(), p.user);
    true
}

/// The tenant's `git.remote` entries narrow what may be cloned (`tenantconfig::runtime::git_clone_guard`); a tenant with none changes nothing.
fn tenant_git_guard(c: &Ctx, team: Option<&str>, url: &str) -> Result<(), Fail> {
    if !c.sh.profile.is_cloud() {
        return Ok(());
    }
    struct Sink<'a>(&'a dyn iyakki_core_api::AuditSink);
    impl crate::tenantconfig::api::ConfigAudit for Sink<'_> {
        fn record(&self, who: &str, event: &str, detail: &str) {
            self.0.record(who, event, detail);
        }
    }
    let dir = c.data_dir();
    let sink = Sink(c.sh.deps.audit.as_ref());
    let env = crate::tenantconfig::api::Env { dir: &dir, vault: crate::tenantconfig::vault::for_user(&dir, &c.who().user), audit: &sink };
    crate::tenantconfig::runtime::git_clone_guard(&env, team, &c.who().user, url).map_err(|e| Fail::new(403, "denied", &e))
}
