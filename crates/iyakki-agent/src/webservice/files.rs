//! The cloud profile's file and clone routes (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b). DENY BY DEFAULT.
//!
//! Every route does the same three things in the same order, and nothing else is possible:
//! 1. the person is the signed-in account (never a name from the request: unknown fields are refused);
//! 2. the right for the scope is checked with `cloudaccess::Access::check` (`cloudfiles::action`): viewers read, editors write, a person
//!    outside the namespace gets the answer of a namespace that is not there, a tenant without role lists allows nothing;
//! 3. the folder comes from `cloudstorage::resolve` (locate + mount + ensure + jail): `..`, absolute paths and links are refused.
//!
//! What a person can reach: `ns/<ns>/people/<person>` (scope `person`), `ns/<ns>/shared` (scope `shared`) and `personal/<person>`
//! (scope `personal`, the default without a namespace). Text files only (see `cloudfiles`).

use super::handlers::{Ctx, Out};
use super::server::{json_body, Fail};
use crate::cloudaccess::{Access, Denial};
use crate::cloudfiles as fs;
use crate::cloudstorage::{resolve, StorageLayout};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

fn line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// (folder of the scope, the path asked for inside it)
fn target(c: &Ctx, scope: Option<&str>, ns: Option<&str>, path: &str, write: bool) -> Result<(PathBuf, PathBuf), Fail> {
    let scope = fs::parse_scope(scope, ns).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    let me = &c.who().user;
    let acc = Access::new(std::path::Path::new(&c.sh.cfg.data_dir));
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

fn io_fail(e: String) -> Fail {
    if e.contains("no such") {
        Fail::new(404, "not_found", &e)
    } else if e.contains("larger than") || e.contains("more than") {
        Fail::new(413, "too_large", &e)
    } else {
        Fail::new(400, "invalid_input", &e)
    }
}

fn q<'a>(c: &'a Ctx, k: &str) -> Option<&'a str> {
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
    fs::git_clone(&r.url, &dest, "https", r.branch.as_deref(), r.depth, Duration::from_secs(fs::CLONE_SECS)).map_err(|e| Fail::new(400, "invalid_input", &e))?;
    // a repository can hold links; with core.symlinks=false there should be none, and whatever leaves the folder is removed anyway
    let acc = Access::new(std::path::Path::new(&c.sh.cfg.data_dir));
    let removed = acc.after_checkout(&base).map_err(|e| Fail::new(500, "io", &e))?;
    Ok((200, json!({"folder": folder, "removed_links": removed.len()}), line(&format!("folder={folder}"))))
}

pub fn clones(c: &Ctx) -> Out {
    let (base, _) = target(c, q(c, "scope"), q(c, "namespace"), "", false)?;
    let v: Vec<Value> = fs::list_clones(&base).iter().map(|k| json!({"folder": k.folder, "branch": k.branch})).collect();
    Ok((200, json!({"clones": v}), String::new()))
}
