//! The cloud profile's READ-ONLY git routes on a clone inside a person's folder: `status`, `branches`, `log` (paged), `diff`.
//! The clone is reached exactly like every file route (`files::target`: the signed-in person, the right for the scope, the jail), and
//! the folder is a plain name directly inside the person's place. Git runs through `gitbrowse::LocalRepo` (the same guarded reader the GIT
//! tile uses: argument arrays, no shell, a 15 s timeout, output caps) in its `confined` form.
//!
//! A person can write into their own clone's `.git` with the file routes, so the clone is NOT trusted: before git runs, `.git` must be a
//! real folder and its `config` may hold only the sections and keys a clone makes (anything that could run a program or point outside the
//! folder, such as `core.fsmonitor`, `core.worktree`, `filter.*`, `include.*`, `diff.*.textconv`, is a refusal), and git is started with
//! those hooks switched off as well. Nothing is written, nothing touches the network, and no address leaves with a user name or password in it.

use super::files::{q, target};
use super::handlers::{Ctx, Out};
use super::server::Fail;
use crate::gitbrowse::{self as gb, BrowseErr, LocalRepo, LineKind};
use crate::wsexec::System;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Most commits in one page of the log.
pub const LOG_MAX: usize = 200;
const LOG_DEFAULT: usize = 50;
/// The furthest page that is served (a guard against absurd offsets).
const PAGE_MAX: usize = 10_000;
const LINE_MAX: usize = 2000;

/// `text` with the user name and password of every `scheme://user:pass@host` taken out.
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail.find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'').unwrap_or(tail.len());
        let auth = &tail[..end];
        match auth.rfind('@') {
            Some(at) => out.push_str(&tail[at + 1..end]),
            None => out.push_str(auth),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn cut(s: &str, n: usize) -> String {
    scrub(&s.chars().take(n).collect::<String>())
}

/// The sections and keys a clone's `config` may hold. Anything else could run a program or leave the folder.
fn config_allows(text: &str) -> Result<(), String> {
    let mut section = String::new();
    for raw in text.lines() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
            continue;
        }
        if let Some(h) = l.strip_prefix('[') {
            let name = h.trim_end_matches(']').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            if !matches!(name.as_str(), "core" | "remote" | "branch" | "extensions") {
                return Err(format!("the clone's configuration has a [{name}] section, which this view does not read"));
            }
            section = name;
            continue;
        }
        let key = l.split(['=', ' ', '\t']).next().unwrap_or("").to_ascii_lowercase();
        let ok = match section.as_str() {
            "core" => matches!(key.as_str(), "repositoryformatversion" | "filemode" | "bare" | "logallrefupdates" | "symlinks" | "ignorecase" | "precomposeunicode"),
            "remote" => matches!(key.as_str(), "url" | "fetch" | "tagopt" | "promisor" | "partialclonefilter"),
            "branch" => matches!(key.as_str(), "remote" | "merge" | "rebase"),
            "extensions" => matches!(key.as_str(), "objectformat" | "partialclone"),
            _ => false,
        };
        if !ok {
            return Err(format!("the clone's configuration sets \"{key}\", which this view does not read"));
        }
    }
    Ok(())
}

/// Is `dir` a clone this view may run git in? (`.git` a real folder, plain pieces, a plain configuration, no borrowed objects.)
pub fn clone_ok(dir: &Path) -> Result<(), String> {
    let git = dir.join(".git");
    let real = |p: &Path, folder: bool| std::fs::symlink_metadata(p).map(|m| !m.file_type().is_symlink() && if folder { m.is_dir() } else { m.is_file() });
    if !matches!(real(&git, true), Ok(true)) {
        return Err("not a clone".into());
    }
    for (p, folder) in [("HEAD", false), ("config", false), ("objects", true), ("refs", true)] {
        if !matches!(real(&git.join(p), folder), Ok(true)) {
            return Err("not a clone".into());
        }
    }
    for p in ["commondir", "gitdir", "objects/info/alternates", "info/grafts", "replace"] {
        if git.join(p).symlink_metadata().is_ok() {
            return Err(format!("the clone has \"{p}\", which this view does not read"));
        }
    }
    let cfg = std::fs::read_to_string(git.join("config")).map_err(|_| "not a clone".to_string())?;
    if cfg.len() > 64 * 1024 {
        return Err("the clone's configuration is too large".into());
    }
    config_allows(&cfg)
}

fn browse_fail(e: BrowseErr) -> Fail {
    let t = cut(&e.text(), 200);
    match e {
        BrowseErr::Transient(_) => Fail::new(504, "timeout", &t),
        BrowseErr::Rate(_) => Fail::new(429, "rate_limited", &t),
        BrowseErr::TooBig(_) => Fail::new(413, "too_large", &t),
        BrowseErr::Persistent(_) if t.contains("not a git repository") => Fail::new(404, "not_found", "not a clone"),
        BrowseErr::Persistent(_) => Fail::new(400, "invalid_input", &t),
    }
}

/// The reader for `?folder=` of the caller's space; read right only.
fn repo(c: &Ctx) -> Result<LocalRepo, Fail> {
    let folder = q(c, "folder").filter(|f| crate::cloudstorage::plain_name(f)).ok_or_else(|| Fail::new(400, "invalid_input", "give ?folder= (the name of a clone, see /api/v1/git/clones)"))?;
    let (base, dir) = target(c, q(c, "scope"), q(c, "namespace"), folder, false)?;
    if dir.parent() != Some(base.as_path()) {
        return Err(Fail::new(400, "invalid_input", "the folder is a plain name"));
    }
    clone_ok(&dir).map_err(|e| if e == "not a clone" { Fail::new(404, "not_found", &e) } else { Fail::new(400, "invalid_input", &e) })?;
    Ok(LocalRepo::confined(Arc::new(System), &dir, &std::env::temp_dir()))
}

pub fn status(c: &Ctx) -> Out {
    let r = repo(c)?;
    let w = r.working().map_err(browse_fail)?;
    let s = &w.status;
    let changes: Vec<Value> = w.changes.iter().map(|x| json!({"path": cut(&x.path, 1024), "from": cut(&x.from, 1024), "kind": x.kind.word(), "letter": x.kind.letter().to_string(), "staged": x.staged, "unstaged": x.unstaged})).collect();
    let origin = r.origin().map(|o| cut(&o, 300));
    Ok((200, json!({"folder": q(c, "folder"), "branch": cut(&s.branch, 200), "detached": s.detached, "upstream": cut(&s.upstream, 200), "ahead": s.ahead, "behind": s.behind,
        "changed": s.changed, "untracked": s.untracked, "conflicted": s.conflicted, "dirty": s.dirty(), "origin": origin, "changes": changes, "more": w.more}), String::new()))
}

pub fn branches(c: &Ctx) -> Out {
    let r = repo(c)?;
    let list = <LocalRepo as gb::RepoReader>::branches(&r).map_err(browse_fail)?;
    let v: Vec<Value> = list.iter().map(|b| json!({"name": cut(&b.name, 200), "sha": b.sha.chars().take(64).collect::<String>(), "subject": cut(&b.subject, 200), "default": b.default, "when": b.when})).collect();
    Ok((200, json!({"current": r.branch().map(|b| cut(&b, 200)), "branches": v}), String::new()))
}

fn num(c: &Ctx, k: &str, default: usize, max: usize) -> Result<usize, Fail> {
    match q(c, k) {
        None | Some("") => Ok(default),
        Some(v) => v.parse::<usize>().map(|n| n.min(max)).map_err(|_| Fail::new(400, "invalid_input", &format!("{k} is a number"))),
    }
}

pub fn log(c: &Ctx) -> Out {
    let r = repo(c)?;
    let per = num(c, "per", LOG_DEFAULT, LOG_MAX)?.max(1);
    let page = num(c, "page", 0, PAGE_MAX)?;
    let rf = q(c, "ref").unwrap_or("");
    let (list, more) = r.commits_page(rf, page * per, per).map_err(browse_fail)?;
    let v: Vec<Value> = list.iter().map(|k| {
        let subject = k.message.lines().next().unwrap_or("");
        json!({"sha": k.sha, "short": k.short(), "author": cut(&k.author, 120), "when": k.when, "subject": cut(subject, 200), "message": cut(&k.message, 2000)})
    }).collect();
    Ok((200, json!({"page": page, "per": per, "more": more, "commits": v}), String::new()))
}

fn diff_json(d: &gb::Diff) -> Value {
    let files: Vec<Value> = d.files.iter().map(|f| {
        let lines: Vec<Value> = if f.binary { vec![] } else {
            f.lines.iter().map(|l| json!({"kind": match l.kind { LineKind::Meta => "meta", LineKind::Hunk => "hunk", LineKind::Add => "add", LineKind::Del => "del", LineKind::Ctx => "ctx" }, "text": cut(&l.text, LINE_MAX)})).collect()
        };
        json!({"old": cut(&f.old, 1024), "new": cut(&f.new, 1024), "status": f.status.to_string(), "added": f.added, "removed": f.removed, "binary": f.binary, "lines": lines})
    }).collect();
    json!({"files": files, "too_large": d.too_large})
}

/// `?path=` the working copy's changes in one file against the last commit (a binary file is refused, 415); `?commit=` one commit's patch
/// (binary files are listed without content). A diff above 256 KiB is a 413, never a partial list.
pub fn diff(c: &Ctx) -> Out {
    let r = repo(c)?;
    let d = match (q(c, "path").filter(|p| !p.is_empty()), q(c, "commit").filter(|p| !p.is_empty())) {
        (Some(p), None) => {
            if p == ".git" || p.starts_with(".git/") || p.contains("/.git/") {
                return Err(Fail::new(400, "invalid_input", "that path is not one this view reads"));
            }
            let d = r.diff_head(p).map_err(browse_fail)?;
            if d.files.iter().any(|f| f.binary) {
                return Err(Fail::new(415, "binary", "that file is binary; a diff of it is not shown"));
            }
            d
        }
        (None, Some(sha)) => <LocalRepo as gb::RepoReader>::diff(&r, sha).map_err(browse_fail)?,
        _ => return Err(Fail::new(400, "invalid_input", "give ?path= (a changed file) or ?commit= (a commit id), not both")),
    };
    if d.too_large {
        return Err(Fail::new(413, "too_large", &format!("the diff is larger than {} KiB", gb::DIFF_MAX / 1024)));
    }
    Ok((200, diff_json(&d), String::new()))
}
