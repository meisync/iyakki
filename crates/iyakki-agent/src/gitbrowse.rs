//! Browsing the repositories of a git host, read-only (the GIT tile, `docs/GIT-TILE.md`). Two readers answer the same questions
//! (a directory, a file, the commits, one commit's diff, the branches and tags, the issues and pull requests):
//!
//! * [`RemoteRepo`] asks the host's own API through [`GitHttp`] (Gitea and Forgejo, GitHub, GitLab; each kind has its own paths and
//!   answers, all turned into the same small types here). Every request is a GET. The token comes from the vault through the
//!   connection and is sent only in the request's header; every error text is scrubbed; the network policy is asked before every
//!   request ([`Egress`]); a certificate the person trusted for the host is used as in `gitconn.rs`.
//! * [`LocalRepo`] runs the git program through the [`Runner`] in a clone on this computer (`ls-tree`, `show`, `log`,
//!   `for-each-ref`), never touching the working tree or the index of the clone.
//!
//! Limits that hold for both: a page of repositories, commits or issues is 50; a file shown is at most 512 KB; a diff at most 256 KB
//! (more gives a fallback, not a partial list); a directory shows at most 500 entries; a request waits 15 s. Failures are told apart as
//! [`BrowseErr`]: worth retrying (the host is busy or down), not worth retrying (refused, not found), told to slow down (rate limit),
//! or too large.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use zeroize::Zeroizing;

use crate::gitconn::{self, Connection, GitHttp, Kind, NetErr, Req, Resp, TOO_BIG};
use crate::wsexec::Runner;

pub const PAGE: usize = 50;
pub const COMMITS: usize = 50;
pub const ISSUES: usize = 50;
/// The largest file the viewer opens.
pub const FILE_MAX: u64 = 512 * 1024;
/// The largest diff shown.
pub const DIFF_MAX: usize = 256 * 1024;
/// Most entries of one directory.
pub const DIR_MAX: usize = 500;
pub const TIMEOUT_S: u64 = 15;
/// What a rate-limited host is waited for when it does not say.
pub const DEFAULT_WAIT_S: u64 = 60;
/// The longest wait honoured; a host asking for more shows its own text.
pub const MAX_WAIT_S: u64 = 3600;

// ------------------------------------------------------------------ errors

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseErr {
    /// Trying again may work: the host did not answer, or has a problem of its own.
    Transient(String),
    /// Trying again will not help until something is fixed.
    Persistent(String),
    /// The host asked us to slow down: retry in this many seconds.
    Rate(u64),
    /// More than the tile shows.
    TooBig(String),
}

impl BrowseErr {
    pub fn text(&self) -> String {
        match self {
            BrowseErr::Transient(t) | BrowseErr::Persistent(t) | BrowseErr::TooBig(t) => t.clone(),
            BrowseErr::Rate(n) => format!("The host asked us to slow down: retry in {n} s."),
        }
    }
    pub fn verdict(&self) -> crate::k8sretry::Verdict {
        match self {
            BrowseErr::Transient(_) | BrowseErr::Rate(_) => crate::k8sretry::Verdict::Transient,
            _ => crate::k8sretry::Verdict::Persistent,
        }
    }
}

/// May this account reach this host at all (the network policy)? `Err` says why not.
pub type Egress = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

// ------------------------------------------------------------------ the types

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoItem {
    pub full_name: String,
    pub description: String,
    pub default_branch: String,
    pub private: bool,
    /// Seconds since 1970 of the last update (0: not known).
    pub updated: u64,
    pub stars: Option<u64>,
    pub size_kb: Option<u64>,
    pub https_url: String,
    pub ssh_url: String,
    pub web_url: String,
    pub license: String,
    pub archived: bool,
    pub fork: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The host has more after these.
    pub more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Link,
    Submodule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: String,
    /// From the repository's root, without a leading slash.
    pub path: String,
    pub kind: EntryKind,
    /// Bytes (0: not known, GitLab's tree does not say).
    pub size: u64,
}

/// A directory listing: directories first, then files, by name; cut at [`DIR_MAX`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dir {
    pub entries: Vec<TreeEntry>,
    pub cut: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileData {
    /// The size the host or git reported (or the bytes read).
    pub size: u64,
    pub bytes: Vec<u8>,
    /// Larger than [`FILE_MAX`]: nothing was read.
    pub too_big: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub when: u64,
}

impl CommitInfo {
    pub fn short(&self) -> String {
        self.sha.chars().take(7).collect()
    }
    pub fn subject(&self) -> String {
        self.message.lines().next().unwrap_or("").trim().to_string()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefInfo {
    pub name: String,
    pub sha: String,
    pub subject: String,
    pub default: bool,
    pub tag: bool,
    pub when: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: String,
    pub when: u64,
    pub labels: Vec<String>,
    pub body: String,
    pub pr: bool,
    pub comments: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `diff --git`, `index`, `---`, `+++`, mode lines.
    Meta,
    Hunk,
    Add,
    Del,
    Ctx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffFile {
    pub old: String,
    pub new: String,
    /// `A` added, `D` deleted, `M` changed, `R` renamed.
    pub status: char,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    pub lines: Vec<DiffLine>,
}

impl DiffFile {
    pub fn name(&self) -> &str {
        if self.status == 'D' || self.new.is_empty() { &self.old } else { &self.new }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub files: Vec<DiffFile>,
    /// The diff was larger than [`DIFF_MAX`]: no lines are kept.
    pub too_large: bool,
}

// ------------------------------------------------------------------ the reader

/// What the tile asks of a repository, whether it is on a host or on this computer.
pub trait RepoReader: Send + Sync {
    fn dir(&self, rf: &str, path: &str) -> Result<Dir, BrowseErr>;
    /// `size_hint` is what the listing said (0: unknown); a file over [`FILE_MAX`] is not read.
    fn file(&self, rf: &str, path: &str, size_hint: u64) -> Result<FileData, BrowseErr>;
    fn commits(&self, rf: &str) -> Result<Vec<CommitInfo>, BrowseErr>;
    fn diff(&self, sha: &str) -> Result<Diff, BrowseErr>;
    fn branches(&self) -> Result<Vec<RefInfo>, BrowseErr>;
    fn tags(&self) -> Result<Vec<RefInfo>, BrowseErr>;
    fn issues(&self, pulls: bool) -> Result<Vec<IssueInfo>, BrowseErr>;
}

// ------------------------------------------------------------------ small helpers

/// Percent-encoding of a path or a value (`/` kept when `keep_slash`).
pub fn pct(s: &str, keep_slash: bool) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (keep_slash && b == b'/') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn js(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    let mut c = v;
    for p in path {
        c = &c[*p];
    }
    c
}

fn tx(v: &Value, path: &[&str]) -> String {
    at(v, path).as_str().unwrap_or("").to_string()
}

fn num(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64())
}

/// Seconds since 1970 of `2026-10-22T10:00:00Z`, `...+02:00` or `...-0500` (fractions ignored); 0 when it cannot be read.
pub fn ts_of(s: &str) -> u64 {
    let base = crate::k8s::parse_time(s);
    if base == 0 || s.len() < 20 {
        return base;
    }
    let rest = &s[19..];
    let rest = rest.strip_prefix('.').map_or(rest, |f| f.trim_start_matches(|c: char| c.is_ascii_digit()));
    let sign = match rest.chars().next() {
        Some('+') => 1i64,
        Some('-') => -1i64,
        _ => return base,
    };
    let d: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
    if d.len() < 4 {
        return base;
    }
    let off = d[..2].parse::<i64>().unwrap_or(0) * 3600 + d[2..4].parse::<i64>().unwrap_or(0) * 60;
    (base as i64 - sign * off).max(0) as u64
}

/// A ref name or a path that may be given to git or the host: no control characters, no leading dash for a ref, no `..` step in a path.
pub fn safe_ref(r: &str) -> bool {
    !r.is_empty() && r.len() <= 200 && !r.starts_with('-') && !r.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '"' | '\'' | '`' | '$' | ';' | '|' | '&' | '<' | '>' | '*' | '?' | '[' | ']' | ':'))
}

pub fn safe_path(p: &str) -> bool {
    p.len() <= 1024 && !p.starts_with('-') && !p.starts_with('/') && !p.chars().any(|c| c.is_control()) && !p.split('/').any(|s| s == "..")
}

fn sort_dir(mut v: Vec<TreeEntry>) -> Dir {
    v.sort_by(|a, b| {
        let da = a.kind == EntryKind::Dir;
        let db = b.kind == EntryKind::Dir;
        db.cmp(&da).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let cut = v.len() > DIR_MAX;
    v.truncate(DIR_MAX);
    Dir { entries: v, cut }
}

// ------------------------------------------------------------------ what a file is

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// png, jpeg, gif or webp, by its first bytes.
    Image,
    /// A name for the kind of file (`ZIP archive`) shown with its size.
    Binary(String),
}

/// Text (valid UTF-8 without NUL), a picture, or something else.
pub fn classify(name: &str, bytes: &[u8]) -> Content {
    use iyakki_web::image::{sniff, Kind as Img};
    if matches!(sniff(bytes), Img::Png | Img::Jpeg | Img::Gif | Img::Webp) {
        return Content::Image;
    }
    let magic = if bytes.starts_with(b"PK\x03\x04") {
        Some("ZIP archive")
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        Some("gzip data")
    } else if bytes.starts_with(b"%PDF") {
        Some("PDF document")
    } else if bytes.starts_with(b"\x7fELF") {
        Some("ELF program")
    } else if bytes.starts_with(b"SQLite format 3") {
        Some("SQLite database")
    } else {
        None
    };
    if let Some(m) = magic {
        return Content::Binary(m.to_string());
    }
    let head = &bytes[..bytes.len().min(8000)];
    if !head.contains(&0) {
        if let Ok(t) = std::str::from_utf8(bytes) {
            return Content::Text(t.to_string());
        }
    }
    let ext = name.rsplit_once('.').map(|x| x.1.to_ascii_lowercase()).unwrap_or_default();
    let label = if ext == "zip" {
        "ZIP archive"
    } else if matches!(ext.as_str(), "ico" | "bmp" | "tif" | "tiff" | "svgz") {
        "picture (a format the viewer does not draw)"
    } else if matches!(ext.as_str(), "mp3" | "ogg" | "wav" | "flac") {
        "audio"
    } else if matches!(ext.as_str(), "mp4" | "mkv" | "webm" | "mov") {
        "video"
    } else if matches!(ext.as_str(), "woff" | "woff2" | "ttf" | "otf") {
        "font"
    } else {
        "binary data"
    };
    Content::Binary(label.to_string())
}

/// A name that is the repository's front page: README, README.md, readme.rst ...
pub fn is_readme(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "readme" || l.starts_with("readme.")
}

/// The README of a listing (Markdown first when several), if any.
pub fn find_readme(d: &Dir) -> Option<&TreeEntry> {
    let mut all: Vec<&TreeEntry> = d.entries.iter().filter(|e| e.kind == EntryKind::File && is_readme(&e.name)).collect();
    all.sort_by_key(|e| (!e.name.to_ascii_lowercase().ends_with(".md"), e.name.len()));
    all.into_iter().next()
}

pub fn is_markdown(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".md") || l.ends_with(".markdown")
}

// ------------------------------------------------------------------ a remote clone address as a key

/// `https://git.example.org:3000/Team/App.git`, `git@git.example.org:team/app.git` and `ssh://git@git.example.org:2222/team/app` all give
/// `git.example.org/team/app`: how a clone on this computer is matched with a repository of a host.
pub fn remote_key(url: &str) -> String {
    let u = url.trim();
    let (host, path) = if let Some(rest) = u.split_once("://").map(|x| x.1) {
        let auth_end = rest.find('/').unwrap_or(rest.len());
        let auth = rest[..auth_end].rsplit('@').next().unwrap_or("");
        let host = auth.split(':').next().unwrap_or("");
        (host.to_string(), rest[auth_end..].to_string())
    } else if let Some((l, r)) = u.split_once(':') {
        (l.rsplit('@').next().unwrap_or("").to_string(), r.to_string())
    } else {
        return u.to_ascii_lowercase();
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{}/{}", host.to_ascii_lowercase(), path.to_ascii_lowercase())
}

// ------------------------------------------------------------------ the diff

/// Reads a unified diff (`git diff`, `git show`, a host's `.diff`). More than [`DIFF_MAX`] bytes gives `too_large` and the names of the
/// files only.
pub fn parse_unified(text: &str) -> Diff {
    let too_large = text.len() > DIFF_MAX;
    let mut files: Vec<DiffFile> = Vec::new();
    let mut in_hunk = false;
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let (a, b) = split_git_names(rest);
            files.push(DiffFile { old: a, new: b, status: 'M', ..Default::default() });
            in_hunk = false;
            if !too_large {
                files.last_mut().unwrap().lines.push(DiffLine { kind: LineKind::Meta, text: l.to_string() });
            }
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if !in_hunk {
            match l {
                _ if l.starts_with("new file mode") => f.status = 'A',
                _ if l.starts_with("deleted file mode") => f.status = 'D',
                _ if l.starts_with("rename from ") => {
                    f.status = 'R';
                    f.old = l["rename from ".len()..].to_string();
                }
                _ if l.starts_with("rename to ") => f.new = l["rename to ".len()..].to_string(),
                _ if l.starts_with("Binary files ") || l.starts_with("GIT binary patch") => f.binary = true,
                _ if l.starts_with("--- ") => {
                    if l == "--- /dev/null" {
                        f.status = 'A';
                    }
                }
                _ if l.starts_with("+++ ") => {
                    if l == "+++ /dev/null" {
                        f.status = 'D';
                    }
                }
                _ => {}
            }
            if l.starts_with("@@") {
                in_hunk = true;
            } else {
                if !too_large {
                    f.lines.push(DiffLine { kind: LineKind::Meta, text: l.to_string() });
                }
                continue;
            }
        }
        let (kind, counted) = match l.as_bytes().first() {
            Some(b'@') if l.starts_with("@@") => (LineKind::Hunk, false),
            Some(b'+') => (LineKind::Add, true),
            Some(b'-') => (LineKind::Del, true),
            Some(b'\\') => (LineKind::Meta, false),
            _ => (LineKind::Ctx, false),
        };
        if counted {
            if kind == LineKind::Add {
                f.added += 1;
            } else {
                f.removed += 1;
            }
        }
        if !too_large {
            f.lines.push(DiffLine { kind, text: l.to_string() });
        }
    }
    Diff { files, too_large }
}

/// `a/old/path b/new/path` -> (`old/path`, `new/path`). Paths with spaces are told apart by the repeated half.
fn split_git_names(rest: &str) -> (String, String) {
    let strip = |s: &str| -> String { s.trim().trim_matches('"').trim_start_matches("a/").trim_start_matches("b/").to_string() };
    if let Some((a, b)) = rest.split_once(" b/") {
        return (strip(a), strip(b));
    }
    let mut it = rest.splitn(2, ' ');
    (strip(it.next().unwrap_or("")), strip(it.next().unwrap_or("")))
}

// ------------------------------------------------------------------ a host

/// The host's API for one connection. Cheap to clone.
#[derive(Clone)]
pub struct Browser {
    pub http: Arc<dyn GitHttp>,
    pub conn: Connection,
    token: Arc<Zeroizing<String>>,
    cert: Option<PathBuf>,
    egress: Egress,
    /// Seconds since 1970 now (the reset time of a rate limit is a date).
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Browser {
    /// `token` is the connection's token from the vault (empty: public repositories only).
    pub fn new(http: Arc<dyn GitHttp>, conn: Connection, token: &str, cert: Option<PathBuf>, egress: Egress) -> Browser {
        Browser { http, conn, token: Arc::new(Zeroizing::new(token.to_string())), cert, egress, now: Arc::new(|| crate::clock::now_unix().max(0) as u64) }
    }

    pub fn with_clock(mut self, now: Arc<dyn Fn() -> u64 + Send + Sync>) -> Browser {
        self.now = now;
        self
    }

    /// One GET. `path` starts with `/` and is under the API base.
    fn get(&self, path: &str, accept: Option<&str>, max_bytes: usize) -> Result<Resp, BrowseErr> {
        if !self.conn.kind.has_api() {
            return Err(BrowseErr::Persistent("This kind of host has no API to browse: clone the repository, then browse the clone.".into()));
        }
        (self.egress)(&self.conn.host()).map_err(BrowseErr::Persistent)?;
        let req = Req {
            method: "GET".into(),
            url: format!("{}{path}", gitconn::api_base(self.conn.kind, &self.conn.url)),
            token: Some(self.token.to_string()).filter(|t| !t.is_empty()),
            style: gitconn::style_of(self.conn.kind),
            body: None,
            cert: self.cert.clone(),
            accept: accept.map(String::from),
            max_bytes,
            timeout_s: TIMEOUT_S,
        };
        let r = match self.http.send(&req) {
            Ok(r) => r,
            Err(NetErr::Other(m)) if m == TOO_BIG => return Err(BrowseErr::TooBig("That is larger than the tile shows.".into())),
            Err(e @ NetErr::Unreachable(_)) => return Err(BrowseErr::Transient(gitconn::net_message(&e, &self.token))),
            Err(e) => return Err(BrowseErr::Persistent(gitconn::net_message(&e, &self.token))),
        };
        self.check(r)
    }

    fn check(&self, r: Resp) -> Result<Resp, BrowseErr> {
        match r.status {
            200..=299 => Ok(r),
            429 => Err(BrowseErr::Rate(self.wait_of(&r))),
            403 if self.limited(&r) => Err(BrowseErr::Rate(self.wait_of(&r))),
            404 => Err(BrowseErr::Persistent("The host does not have that (404): the repository, branch or file may have been renamed or removed, or the token cannot see it.".into())),
            s @ (500..=599 | 408) => Err(BrowseErr::Transient(gitconn::status_text(s, self.conn.kind))),
            s => Err(BrowseErr::Persistent(gitconn::status_text(s, self.conn.kind))),
        }
    }

    fn limited(&self, r: &Resp) -> bool {
        r.header("retry-after").is_some() || r.header("x-ratelimit-remaining") == Some("0") || r.header("ratelimit-remaining") == Some("0") || r.body.to_ascii_lowercase().contains("rate limit")
    }

    fn wait_of(&self, r: &Resp) -> u64 {
        let secs = if let Some(n) = r.header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()) {
            n
        } else if let Some(t) = r.header("x-ratelimit-reset").or_else(|| r.header("ratelimit-reset")).and_then(|v| v.trim().parse::<u64>().ok()) {
            t.saturating_sub((self.now)())
        } else {
            DEFAULT_WAIT_S
        };
        secs.clamp(1, MAX_WAIT_S)
    }

    fn json(&self, path: &str) -> Result<(Value, Resp), BrowseErr> {
        let r = self.get(path, None, 0)?;
        let v = serde_json::from_str(&r.body).map_err(|_| BrowseErr::Persistent(format!("The host's answer could not be read: is {} a {} server?", self.conn.host(), self.conn.kind.label())))?;
        Ok((v, r))
    }

    // ---------------------------------------------------------------- repositories

    /// One page (from 1) of the repositories the account can see, most recently used first where the host can sort.
    pub fn repos(&self, page: usize) -> Result<Page<RepoItem>, BrowseErr> {
        let page = page.max(1);
        let path = match self.conn.kind {
            Kind::GitHub => format!("/user/repos?per_page={PAGE}&sort=updated&page={page}"),
            Kind::GitLab => format!("/projects?membership=true&per_page={PAGE}&order_by=last_activity_at&page={page}"),
            _ => format!("/user/repos?limit={PAGE}&page={page}"),
        };
        let (v, r) = self.json(&path)?;
        let arr = v.as_array().ok_or_else(|| BrowseErr::Persistent("The host's list of repositories could not be read.".into()))?;
        let items: Vec<RepoItem> = arr.iter().filter_map(|x| parse_repo(x, self.conn.kind)).collect();
        let link = r.header("link");
        let more = match (link, r.header("x-next-page")) {
            (Some(l), _) => l.contains("rel=\"next\""),
            (None, Some(n)) => !n.trim().is_empty(),
            _ => arr.len() >= PAGE,
        };
        Ok(Page { items, more })
    }

    pub fn repo(&self, full: &str) -> RemoteRepo {
        RemoteRepo { b: self.clone(), full: full.to_string(), default_branch: String::new() }
    }
}

/// A repository object in the shape of each host.
pub fn parse_repo(v: &Value, kind: Kind) -> Option<RepoItem> {
    let r = match kind {
        Kind::GitLab => RepoItem {
            full_name: js(v, "path_with_namespace"),
            description: js(v, "description"),
            default_branch: js(v, "default_branch"),
            private: v.get("visibility").and_then(|x| x.as_str()) != Some("public"),
            updated: ts_of(&js(v, "last_activity_at")),
            stars: num(v, "star_count"),
            size_kb: None,
            https_url: js(v, "http_url_to_repo"),
            ssh_url: js(v, "ssh_url_to_repo"),
            web_url: js(v, "web_url"),
            license: String::new(),
            archived: v.get("archived").and_then(|x| x.as_bool()).unwrap_or(false),
            fork: !v["forked_from_project"].is_null(),
        },
        _ => RepoItem {
            full_name: js(v, "full_name"),
            description: js(v, "description"),
            default_branch: js(v, "default_branch"),
            private: v.get("private").and_then(|x| x.as_bool()).unwrap_or(true),
            updated: ts_of(&js(v, if kind == Kind::GitHub && v.get("pushed_at").is_some_and(|x| x.is_string()) { "pushed_at" } else { "updated_at" })),
            stars: num(v, if kind == Kind::GitHub { "stargazers_count" } else { "stars_count" }),
            size_kb: num(v, "size"),
            https_url: js(v, "clone_url"),
            ssh_url: js(v, "ssh_url"),
            web_url: js(v, "html_url"),
            license: {
                let l = tx(v, &["license", "spdx_id"]);
                if l.is_empty() || l == "NOASSERTION" { tx(v, &["license", "name"]) } else { l }
            },
            archived: v.get("archived").and_then(|x| x.as_bool()).unwrap_or(false),
            fork: v.get("fork").and_then(|x| x.as_bool()).unwrap_or(false),
        },
    };
    // an address with a credential in it is never kept
    if r.full_name.is_empty() || r.https_url.contains('@') {
        return None;
    }
    Some(r)
}

// ------------------------------------------------------------------ one repository on a host

pub struct RemoteRepo {
    b: Browser,
    pub full: String,
    pub default_branch: String,
}

impl RemoteRepo {
    pub fn with_default(mut self, branch: &str) -> RemoteRepo {
        self.default_branch = branch.to_string();
        self
    }

    /// `/repos/o/r` for Gitea and GitHub, `/projects/o%2Fr` for GitLab.
    fn base(&self) -> String {
        match self.b.conn.kind {
            Kind::GitLab => format!("/projects/{}", pct(&self.full, false)),
            _ => format!("/repos/{}", pct(&self.full, true)),
        }
    }

    fn get_json(&self, tail: &str) -> Result<Value, BrowseErr> {
        Ok(self.b.json(&format!("{}{tail}", self.base()))?.0)
    }
}

fn check_ref_path(rf: &str, path: &str) -> Result<(), BrowseErr> {
    if !rf.is_empty() && !safe_ref(rf) {
        return Err(BrowseErr::Persistent(format!("`{rf}` is not a branch, tag or commit name this tile will ask for.")));
    }
    if !safe_path(path) {
        return Err(BrowseErr::Persistent("That path is not one this tile will ask for.".into()));
    }
    Ok(())
}

impl RepoReader for RemoteRepo {
    fn dir(&self, rf: &str, path: &str) -> Result<Dir, BrowseErr> {
        check_ref_path(rf, path)?;
        let path = path.trim_matches('/');
        let refq = if rf.is_empty() { String::new() } else { format!("ref={}", pct(rf, false)) };
        let v = if self.b.conn.kind == Kind::GitLab {
            let mut q = format!("per_page=100{}", if refq.is_empty() { String::new() } else { format!("&{refq}") });
            if !path.is_empty() {
                q.push_str(&format!("&path={}", pct(path, false)));
            }
            self.get_json(&format!("/repository/tree?{q}"))?
        } else {
            let p = if path.is_empty() { "/contents".to_string() } else { format!("/contents/{}", pct(path, true)) };
            self.get_json(&format!("{p}{}", if refq.is_empty() { String::new() } else { format!("?{refq}") }))?
        };
        let arr = v.as_array().ok_or_else(|| BrowseErr::Persistent("That is a file, not a folder.".into()))?;
        let mut out = Vec::new();
        for e in arr {
            let name = js(e, "name");
            let kind = match (self.b.conn.kind, js(e, "type").as_str()) {
                (Kind::GitLab, "tree") | (_, "dir") => EntryKind::Dir,
                (Kind::GitLab, "commit") | (_, "submodule") => EntryKind::Submodule,
                (_, "symlink") => EntryKind::Link,
                _ => EntryKind::File,
            };
            if name.is_empty() {
                continue;
            }
            let p = { let s = js(e, "path"); if s.is_empty() { if path.is_empty() { name.clone() } else { format!("{path}/{name}") } } else { s } };
            out.push(TreeEntry { name, path: p, kind, size: num(e, "size").unwrap_or(0) });
        }
        Ok(sort_dir(out))
    }

    fn file(&self, rf: &str, path: &str, size_hint: u64) -> Result<FileData, BrowseErr> {
        check_ref_path(rf, path)?;
        if size_hint > FILE_MAX {
            return Ok(FileData { size: size_hint, bytes: vec![], too_big: true });
        }
        let refq = if rf.is_empty() { String::new() } else { format!("?ref={}", pct(rf, false)) };
        let tail = match self.b.conn.kind {
            Kind::GitLab => format!("/repository/files/{}{}", pct(path, false), if rf.is_empty() { "?ref=HEAD".to_string() } else { refq }),
            _ => format!("/contents/{}{refq}", pct(path, true)),
        };
        // base64 grows a file by a third; the limit leaves room for the JSON around it
        let r = match self.b.get(&format!("{}{tail}", self.base()), None, (FILE_MAX as usize) * 3 / 2 + 8192) {
            Ok(r) => r,
            Err(BrowseErr::TooBig(_)) => return Ok(FileData { size: size_hint.max(FILE_MAX + 1), bytes: vec![], too_big: true }),
            Err(e) => return Err(e),
        };
        let v: Value = serde_json::from_str(&r.body).map_err(|_| BrowseErr::Persistent("The host's answer could not be read.".into()))?;
        if v.is_array() {
            return Err(BrowseErr::Persistent("That is a folder, not a file.".into()));
        }
        let size = num(&v, "size").unwrap_or(0);
        if size > FILE_MAX {
            return Ok(FileData { size, bytes: vec![], too_big: true });
        }
        let content = js(&v, "content");
        if js(&v, "encoding") == "none" || (content.is_empty() && size > 0) {
            // GitHub leaves big files out of the contents answer
            return Ok(FileData { size, bytes: vec![], too_big: true });
        }
        let bytes = if js(&v, "encoding") == "base64" || js(&v, "encoding").is_empty() { crate::sshkey::unb64(&content).ok_or_else(|| BrowseErr::Persistent("The file's content could not be decoded.".into()))? } else { content.into_bytes() };
        if bytes.len() as u64 > FILE_MAX {
            return Ok(FileData { size: bytes.len() as u64, bytes: vec![], too_big: true });
        }
        Ok(FileData { size: bytes.len() as u64, bytes, too_big: false })
    }

    fn commits(&self, rf: &str) -> Result<Vec<CommitInfo>, BrowseErr> {
        check_ref_path(rf, "")?;
        let rq = |name: &str| if rf.is_empty() { String::new() } else { format!("&{name}={}", pct(rf, false)) };
        let tail = match self.b.conn.kind {
            Kind::GitLab => format!("/repository/commits?per_page={COMMITS}{}", rq("ref_name")),
            Kind::GitHub => format!("/commits?per_page={COMMITS}{}", rq("sha")),
            _ => format!("/commits?limit={COMMITS}&stat=false&verification=false&files=false{}", rq("sha")),
        };
        let v = self.get_json(&tail)?;
        let arr = v.as_array().ok_or_else(|| BrowseErr::Persistent("The host's list of commits could not be read.".into()))?;
        Ok(arr.iter().take(COMMITS).map(|c| parse_commit(c, self.b.conn.kind)).filter(|c| !c.sha.is_empty()).collect())
    }

    fn diff(&self, sha: &str) -> Result<Diff, BrowseErr> {
        if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.is_empty() || sha.len() > 64 {
            return Err(BrowseErr::Persistent("That is not a commit id.".into()));
        }
        let cap = DIFF_MAX + 64 * 1024;
        let text = match self.b.conn.kind {
            Kind::GitLab => {
                let r = self.b.get(&format!("{}/repository/commits/{sha}/diff?per_page=100", self.base()), None, cap + cap / 2);
                match r {
                    Ok(r) => gitlab_diff_text(&r.body)?,
                    Err(BrowseErr::TooBig(_)) => return Ok(Diff { files: vec![], too_large: true }),
                    Err(e) => return Err(e),
                }
            }
            Kind::GitHub => match self.b.get(&format!("{}/commits/{sha}", self.base()), Some("application/vnd.github.diff"), cap) {
                Ok(r) => r.body,
                Err(BrowseErr::TooBig(_)) => return Ok(Diff { files: vec![], too_large: true }),
                Err(e) => return Err(e),
            },
            _ => {
                let first = self.b.get(&format!("{}/git/commits/{sha}.diff", self.base()), Some("text/plain"), cap);
                let first = match first {
                    Err(BrowseErr::Persistent(_)) => self.b.get(&format!("{}/commits/{sha}.diff", self.base()), Some("text/plain"), cap),
                    other => other,
                };
                match first {
                    Ok(r) => r.body,
                    Err(BrowseErr::TooBig(_)) => return Ok(Diff { files: vec![], too_large: true }),
                    Err(e) => return Err(e),
                }
            }
        };
        Ok(parse_unified(&text))
    }

    fn branches(&self) -> Result<Vec<RefInfo>, BrowseErr> {
        let tail = match self.b.conn.kind {
            Kind::GitLab => format!("/repository/branches?per_page={PAGE}"),
            Kind::GitHub => format!("/branches?per_page={PAGE}"),
            _ => format!("/branches?limit={PAGE}"),
        };
        let v = self.get_json(&tail)?;
        let mut out: Vec<RefInfo> = v.as_array().into_iter().flatten().filter_map(|b| parse_ref(b, self.b.conn.kind, false)).collect();
        for r in out.iter_mut() {
            r.default = r.default || (!self.default_branch.is_empty() && r.name == self.default_branch);
        }
        out.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    fn tags(&self) -> Result<Vec<RefInfo>, BrowseErr> {
        let tail = match self.b.conn.kind {
            Kind::GitLab => format!("/repository/tags?per_page={PAGE}"),
            Kind::GitHub => format!("/tags?per_page={PAGE}"),
            _ => format!("/tags?limit={PAGE}"),
        };
        let v = self.get_json(&tail)?;
        Ok(v.as_array().into_iter().flatten().filter_map(|b| parse_ref(b, self.b.conn.kind, true)).collect())
    }

    fn issues(&self, pulls: bool) -> Result<Vec<IssueInfo>, BrowseErr> {
        let tail = match (self.b.conn.kind, pulls) {
            (Kind::GitLab, false) => format!("/issues?state=all&per_page={ISSUES}&order_by=updated_at"),
            (Kind::GitLab, true) => format!("/merge_requests?state=all&per_page={ISSUES}&order_by=updated_at"),
            (Kind::GitHub, false) => format!("/issues?state=all&per_page={ISSUES}&sort=updated"),
            (Kind::GitHub, true) => format!("/pulls?state=all&per_page={ISSUES}&sort=updated"),
            (_, p) => format!("/issues?type={}&state=all&limit={ISSUES}", if p { "pulls" } else { "issues" }),
        };
        let v = self.get_json(&tail)?;
        let kind = self.b.conn.kind;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            // GitHub's issue list holds the pull requests too
            .filter(|i| !(kind == Kind::GitHub && !pulls && i.get("pull_request").is_some()))
            .filter_map(|i| parse_issue(i, kind, pulls))
            .take(ISSUES)
            .collect())
    }
}

fn gitlab_diff_text(body: &str) -> Result<String, BrowseErr> {
    let v: Value = serde_json::from_str(body).map_err(|_| BrowseErr::Persistent("The host's diff could not be read.".into()))?;
    let mut t = String::new();
    for f in v.as_array().into_iter().flatten() {
        let (old, new) = (js(f, "old_path"), js(f, "new_path"));
        t.push_str(&format!("diff --git a/{old} b/{new}\n"));
        if f.get("new_file").and_then(|x| x.as_bool()) == Some(true) {
            t.push_str("new file mode 100644\n");
        } else if f.get("deleted_file").and_then(|x| x.as_bool()) == Some(true) {
            t.push_str("deleted file mode 100644\n");
        } else if f.get("renamed_file").and_then(|x| x.as_bool()) == Some(true) {
            t.push_str(&format!("rename from {old}\nrename to {new}\n"));
        }
        let d = js(f, "diff");
        if d.is_empty() {
            t.push_str(&format!("Binary files a/{old} and b/{new} differ\n"));
        } else {
            t.push_str(&format!("--- a/{old}\n+++ b/{new}\n{d}"));
            if !d.ends_with('\n') {
                t.push('\n');
            }
        }
    }
    Ok(t)
}

pub fn parse_commit(c: &Value, kind: Kind) -> CommitInfo {
    match kind {
        Kind::GitLab => CommitInfo { sha: js(c, "id"), message: { let m = js(c, "message"); if m.is_empty() { js(c, "title") } else { m } }, author: js(c, "author_name"), when: ts_of(&js(c, "authored_date")) },
        _ => CommitInfo {
            sha: js(c, "sha"),
            message: tx(c, &["commit", "message"]),
            author: { let n = tx(c, &["commit", "author", "name"]); if n.is_empty() { tx(c, &["author", "login"]) } else { n } },
            when: ts_of(&tx(c, &["commit", "author", "date"])),
        },
    }
}

fn parse_ref(b: &Value, kind: Kind, tag: bool) -> Option<RefInfo> {
    let name = js(b, "name");
    if name.is_empty() {
        return None;
    }
    Some(match kind {
        Kind::GitLab => RefInfo { name, sha: tx(b, &["commit", "id"]), subject: tx(b, &["commit", "title"]), default: b.get("default").and_then(|x| x.as_bool()).unwrap_or(false), tag, when: ts_of(&tx(b, &["commit", "committed_date"])) },
        Kind::GitHub => RefInfo { name, sha: tx(b, &["commit", "sha"]), subject: String::new(), default: false, tag, when: 0 },
        _ => RefInfo {
            name,
            sha: { let s = tx(b, &["commit", "id"]); if s.is_empty() { tx(b, &["commit", "sha"]) } else { s } },
            subject: tx(b, &["commit", "message"]).lines().next().unwrap_or("").to_string(),
            default: false,
            tag,
            when: ts_of(&tx(b, &["commit", "timestamp"])),
        },
    })
}

fn parse_issue(i: &Value, kind: Kind, pr: bool) -> Option<IssueInfo> {
    let (number, author, when, labels, body, state) = match kind {
        Kind::GitLab => (
            num(i, "iid")?,
            tx(i, &["author", "username"]),
            ts_of(&js(i, "updated_at")),
            i["labels"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)).collect::<Vec<_>>(),
            js(i, "description"),
            { let s = js(i, "state"); if s == "opened" { "open".to_string() } else { s } },
        ),
        _ => (
            num(i, "number")?,
            tx(i, &["user", "login"]),
            ts_of(&js(i, "updated_at")),
            i["labels"].as_array().into_iter().flatten().filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(String::from)).collect(),
            js(i, "body"),
            { let s = js(i, "state"); if i.get("merged_at").is_some_and(|m| m.is_string()) || i.get("merged").and_then(|m| m.as_bool()) == Some(true) { "merged".to_string() } else { s } },
        ),
    };
    let comments = num(i, "comments").or_else(|| num(i, "user_notes_count")).unwrap_or(0);
    Some(IssueInfo { number, title: js(i, "title"), state, author, when, labels, body, pr, comments })
}

// ------------------------------------------------------------------ a clone on this computer

/// A repository on this computer, read through git. Read-only: `ls-tree`, `show`, `log`, `for-each-ref`, `diff`; a picture is
/// written to a private temporary folder through a temporary index (`checkout-index --prefix`), never to the working tree.
pub struct LocalRepo {
    pub run: Arc<dyn Runner>,
    pub dir: PathBuf,
    /// A private folder for pictures read out of git.
    pub tmp: PathBuf,
    /// The clone belongs to someone else (a person's folder on a shared volume, reached by the web service): the repository's own
    /// configuration is not trusted. No fsmonitor, hook, pager, external diff or text filter of the clone runs, and the user's and the
    /// system's git configuration are not read.
    pub confined: bool,
}

fn lerr(o: &crate::wsexec::Out) -> BrowseErr {
    if o.missing {
        return BrowseErr::Persistent(crate::gitjobs::missing_message());
    }
    if o.timed_out {
        return BrowseErr::Transient("git took too long.".into());
    }
    let t = crate::bugs::redact(o.combined().trim());
    let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").chars().take(200).collect::<String>();
    BrowseErr::Persistent(first)
}

impl LocalRepo {
    pub fn new(run: Arc<dyn Runner>, dir: &Path, tmp: &Path) -> LocalRepo {
        LocalRepo { run, dir: dir.to_path_buf(), tmp: tmp.to_path_buf(), confined: false }
    }

    /// A reader for a clone that is not trusted (see [`LocalRepo::confined`]).
    pub fn confined(run: Arc<dyn Runner>, dir: &Path, tmp: &Path) -> LocalRepo {
        LocalRepo { confined: true, ..LocalRepo::new(run, dir, tmp) }
    }

    fn git(&self, args: &[&str], cap: usize) -> crate::wsexec::Out {
        let mut argv: Vec<String> = vec!["git".into(), "--no-pager".into(), "-c".into(), "core.quotepath=off".into(), "-c".into(), "color.ui=never".into()];
        if self.confined {
            for c in ["core.fsmonitor=false", "core.hooksPath=/dev/null", "core.pager=cat", "diff.external=", "core.untrackedCache=false", "core.sshCommand=false", "credential.helper="] {
                argv.extend(["-c".to_string(), c.to_string()]);
            }
        }
        argv.extend(["-C".to_string(), self.dir.display().to_string()]);
        argv.extend(args.iter().map(|s| s.to_string()));
        let mut env = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string()), ("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())];
        if self.confined {
            env.extend([("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()), ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()), ("GIT_ALLOW_PROTOCOL".to_string(), "none".to_string())]);
        }
        self.run.run(&self.dir, &argv, &env, Duration::from_secs(TIMEOUT_S), cap, None)
    }

    /// The address of `origin`, with any credential taken out.
    pub fn origin(&self) -> Option<String> {
        let o = self.git(&["remote", "get-url", "origin"], 4096);
        o.ok().then(|| crate::gitsite::redact_url(o.stdout.trim())).filter(|u| !u.is_empty())
    }

    /// The branch checked out (none when detached or empty).
    pub fn branch(&self) -> Option<String> {
        let o = self.git(&["symbolic-ref", "--short", "-q", "HEAD"], 4096);
        o.ok().then(|| o.stdout.trim().to_string()).filter(|b| !b.is_empty())
    }

    /// The branch the remote calls its default, else the one checked out.
    pub fn default_branch(&self) -> Option<String> {
        let o = self.git(&["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"], 4096);
        let d = o.ok().then(|| o.stdout.trim().trim_start_matches("origin/").to_string()).filter(|b| !b.is_empty());
        d.or_else(|| self.branch())
    }

    /// Branch, ahead and behind, changed files (`git status --porcelain=v2 --branch`; takes no lock, writes nothing).
    pub fn status(&self) -> Result<crate::gitjobs::RepoStatus, BrowseErr> {
        let o = self.git(&["status", "--porcelain=v2", "--branch"], 256 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        Ok(crate::gitjobs::parse_status(&o.stdout))
    }

    /// The state of the working copy: branch, ahead and behind, and the changed files with their status (one `git status
    /// --porcelain=v2 --branch`; takes no lock, writes nothing, never touches the network).
    pub fn working(&self) -> Result<Working, BrowseErr> {
        let o = self.git(&["status", "--porcelain=v2", "--branch"], 1024 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        let mut changes = parse_changes(&o.stdout);
        let more = changes.len().saturating_sub(CHANGES_MAX);
        changes.truncate(CHANGES_MAX);
        Ok(Working { status: crate::gitjobs::parse_status(&o.stdout), changes, more })
    }

    /// What one changed file differs by against the last commit (staged and not staged together). Read-only.
    pub fn diff_head(&self, path: &str) -> Result<Diff, BrowseErr> {
        if !safe_path(path) || path.is_empty() {
            return Err(BrowseErr::Persistent("That path is not one this tile will ask for.".into()));
        }
        let o = self.git(&["diff", "HEAD", "--no-color", "--no-ext-diff", "--no-textconv", "-M", "--", path], DIFF_MAX + 64 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        if o.truncated {
            return Ok(Diff { files: parse_unified(&o.stdout).files.into_iter().map(|mut f| { f.lines.clear(); f }).collect(), too_large: true });
        }
        Ok(parse_unified(&o.stdout))
    }

    /// `n` commits of `rf` after skipping `skip` (newest first), plus whether more follow. Used by the web service's paged log.
    pub fn commits_page(&self, rf: &str, skip: usize, n: usize) -> Result<(Vec<CommitInfo>, bool), BrowseErr> {
        let rf = self.rf(rf)?;
        let (sk, take) = (format!("--skip={skip}"), format!("-n{}", n + 1));
        let o = self.git(&["log", &sk, &take, "--format=%H%x1f%an%x1f%at%x1f%B%x1e", rf, "--"], 2 * 1024 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        let mut v: Vec<CommitInfo> = o.stdout.split('\u{1e}').filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let mut it = rec.splitn(4, '\u{1f}');
            let (sha, author, at, msg) = (it.next()?.trim(), it.next()?, it.next()?, it.next().unwrap_or(""));
            (!sha.is_empty()).then(|| CommitInfo { sha: sha.to_string(), author: author.to_string(), when: at.trim().parse().unwrap_or(0), message: msg.trim_end().to_string() })
        }).collect();
        let more = v.len() > n;
        v.truncate(n);
        Ok((v, more))
    }

    fn rf<'a>(&self, rf: &'a str) -> Result<&'a str, BrowseErr> {
        let rf = if rf.is_empty() { "HEAD" } else { rf };
        if safe_ref(rf) { Ok(rf) } else { Err(BrowseErr::Persistent(format!("`{rf}` is not a branch, tag or commit name this tile will ask for."))) }
    }

    fn blob_size(&self, rf: &str, path: &str) -> Option<u64> {
        let o = self.git(&["cat-file", "-s", &format!("{rf}:{path}")], 4096);
        o.ok().then(|| o.stdout.trim().parse().ok()).flatten()
    }

    /// The bytes of a file at `rf`, through a temporary index and `checkout-index --prefix`: nothing of the clone changes.
    fn blob_bytes(&self, rf: &str, path: &str) -> Result<Vec<u8>, BrowseErr> {
        let tag: String = { let mut r = [0u8; 4]; let _ = iyakki_sec::random_fill(&mut r); r.iter().map(|b| format!("{b:02x}")).collect() };
        let work = self.tmp.join(format!("blob-{tag}"));
        std::fs::create_dir_all(&work).map_err(|e| BrowseErr::Persistent(e.to_string()))?;
        let index = work.join("index");
        let prefix = format!("{}/out/", work.display());
        let run = |args: &[&str]| -> crate::wsexec::Out {
            let mut argv: Vec<String> = vec!["git".into(), "-C".into(), self.dir.display().to_string()];
            argv.extend(args.iter().map(|s| s.to_string()));
            let env = vec![("GIT_INDEX_FILE".to_string(), index.display().to_string()), ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("LC_ALL".to_string(), "C".to_string())];
            self.run.run(&self.dir, &argv, &env, Duration::from_secs(TIMEOUT_S), 64 * 1024, None)
        };
        let result = (|| {
            let a = run(&["read-tree", rf]);
            if !a.ok() {
                return Err(lerr(&a));
            }
            let b = run(&["checkout-index", &format!("--prefix={prefix}"), "--", path]);
            if !b.ok() {
                return Err(lerr(&b));
            }
            std::fs::read(Path::new(&prefix).join(path)).map_err(|e| BrowseErr::Persistent(format!("the file could not be read from git ({e})")))
        })();
        let _ = std::fs::remove_dir_all(&work);
        result
    }
}

impl RepoReader for LocalRepo {
    fn dir(&self, rf: &str, path: &str) -> Result<Dir, BrowseErr> {
        let rf = self.rf(rf)?;
        if !safe_path(path) {
            return Err(BrowseErr::Persistent("That path is not one this tile will ask for.".into()));
        }
        let path = path.trim_matches('/');
        let spec = if path.is_empty() { String::new() } else { format!("{path}/") };
        let mut args = vec!["ls-tree", "-l", "-z", rf];
        if !spec.is_empty() {
            args.push("--");
            args.push(&spec);
        }
        let o = self.git(&args, 2 * 1024 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        let mut out = Vec::new();
        for rec in o.stdout.split('\0').filter(|r| !r.is_empty()) {
            // <mode> SP <type> SP <object> SP <size> TAB <path>
            let Some((meta, p)) = rec.split_once('\t') else { continue };
            let mut it = meta.split_whitespace();
            let (mode, typ, _obj, size) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or("-"));
            let kind = match (typ, mode) {
                ("tree", _) => EntryKind::Dir,
                ("commit", _) => EntryKind::Submodule,
                (_, "120000") => EntryKind::Link,
                _ => EntryKind::File,
            };
            let name = p.rsplit('/').next().unwrap_or(p).to_string();
            out.push(TreeEntry { name, path: p.to_string(), kind, size: size.parse().unwrap_or(0) });
        }
        Ok(sort_dir(out))
    }

    fn file(&self, rf: &str, path: &str, size_hint: u64) -> Result<FileData, BrowseErr> {
        let rf = self.rf(rf)?;
        if !safe_path(path) || path.is_empty() {
            return Err(BrowseErr::Persistent("That path is not one this tile will ask for.".into()));
        }
        let size = if size_hint > 0 { size_hint } else { self.blob_size(rf, path).unwrap_or(0) };
        if size > FILE_MAX {
            return Ok(FileData { size, bytes: vec![], too_big: true });
        }
        // text through `git show`; the bytes (pictures, binaries) through a temporary index
        let o = self.git(&["show", &format!("{rf}:{path}")], FILE_MAX as usize + 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        if o.truncated {
            return Ok(FileData { size: size.max(FILE_MAX + 1), bytes: vec![], too_big: true });
        }
        let probe = classify(path, o.stdout.as_bytes());
        let looks_binary = o.stdout.contains('\u{FFFD}') || o.stdout.as_bytes().contains(&0);
        if looks_binary || matches!(probe, Content::Image) {
            let b = self.blob_bytes(rf, path)?;
            return Ok(FileData { size: b.len() as u64, bytes: b, too_big: false });
        }
        Ok(FileData { size: o.stdout.len() as u64, bytes: o.stdout.into_bytes(), too_big: false })
    }

    fn commits(&self, rf: &str) -> Result<Vec<CommitInfo>, BrowseErr> {
        let rf = self.rf(rf)?;
        let n = format!("-n{COMMITS}");
        let o = self.git(&["log", &n, "--format=%H%x1f%an%x1f%at%x1f%B%x1e", rf, "--"], 1024 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        Ok(o.stdout
            .split('\u{1e}')
            .filter_map(|rec| {
                let rec = rec.trim_start_matches('\n');
                let mut it = rec.splitn(4, '\u{1f}');
                let (sha, author, at, msg) = (it.next()?.trim(), it.next()?, it.next()?, it.next().unwrap_or(""));
                (!sha.is_empty()).then(|| CommitInfo { sha: sha.to_string(), author: author.to_string(), when: at.trim().parse().unwrap_or(0), message: msg.trim_end().to_string() })
            })
            .take(COMMITS)
            .collect())
    }

    fn diff(&self, sha: &str) -> Result<Diff, BrowseErr> {
        if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.is_empty() || sha.len() > 64 {
            return Err(BrowseErr::Persistent("That is not a commit id.".into()));
        }
        let o = self.git(&["show", "--format=", "--patch", "--no-color", "--no-ext-diff", "--no-textconv", "-M", sha, "--"], DIFF_MAX + 64 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        if o.truncated {
            return Ok(Diff { files: parse_unified(&o.stdout).files.into_iter().map(|mut f| { f.lines.clear(); f }).collect(), too_large: true });
        }
        Ok(parse_unified(&o.stdout))
    }

    fn branches(&self) -> Result<Vec<RefInfo>, BrowseErr> {
        let o = self.git(&["for-each-ref", "--format=%(refname:short)%1f%(objectname)%1f%(contents:subject)%1f%(committerdate:unix)", "refs/heads"], 256 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        let def = self.default_branch().unwrap_or_default();
        let mut v: Vec<RefInfo> = o.stdout.lines().filter_map(|l| parse_ref_line(l, false)).collect();
        for r in v.iter_mut() {
            r.default = r.name == def;
        }
        v.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.name.cmp(&b.name)));
        Ok(v)
    }

    fn tags(&self) -> Result<Vec<RefInfo>, BrowseErr> {
        let o = self.git(&["for-each-ref", "--sort=-creatordate", "--format=%(refname:short)%1f%(objectname)%1f%(contents:subject)%1f%(creatordate:unix)", "refs/tags"], 256 * 1024);
        if !o.ok() {
            return Err(lerr(&o));
        }
        Ok(o.stdout.lines().filter_map(|l| parse_ref_line(l, true)).take(PAGE).collect())
    }

    fn issues(&self, _pulls: bool) -> Result<Vec<IssueInfo>, BrowseErr> {
        Err(BrowseErr::Persistent("Issues and pull requests live on the host: choose its connection to read them.".into()))
    }
}

fn parse_ref_line(l: &str, tag: bool) -> Option<RefInfo> {
    let mut it = l.split('\u{1f}');
    let name = it.next()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(RefInfo { name, sha: it.next().unwrap_or("").to_string(), subject: it.next().unwrap_or("").to_string(), default: false, tag, when: it.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0) })
}

// ------------------------------------------------------------------ the working copy

/// Changed files shown at most.
pub const CHANGES_MAX: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// The letter a status list shows.
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Modified => 'M',
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::Untracked => '?',
            ChangeKind::Conflicted => 'U',
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            ChangeKind::Modified => "changed",
            ChangeKind::Added => "added",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Renamed => "renamed",
            ChangeKind::Untracked => "new, not tracked",
            ChangeKind::Conflicted => "conflict",
        }
    }
}

/// One changed file of the working copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// Where a renamed file came from.
    pub from: String,
    pub kind: ChangeKind,
    /// Part of it is staged.
    pub staged: bool,
    /// Part of it is not staged.
    pub unstaged: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Working {
    pub status: crate::gitjobs::RepoStatus,
    pub changes: Vec<Change>,
    /// Changed files beyond [`CHANGES_MAX`].
    pub more: usize,
}

/// Reads the file lines of `git status --porcelain=v2` (ordinary, renamed, unmerged, untracked).
pub fn parse_changes(out: &str) -> Vec<Change> {
    let mut v = Vec::new();
    for l in out.lines() {
        let kind_of = |x: char, y: char| {
            if x == 'R' || y == 'R' || x == 'C' || y == 'C' {
                ChangeKind::Renamed
            } else if x == 'A' || y == 'A' {
                ChangeKind::Added
            } else if x == 'D' || y == 'D' {
                ChangeKind::Deleted
            } else {
                ChangeKind::Modified
            }
        };
        if let Some(r) = l.strip_prefix("1 ") {
            // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            let mut it = r.splitn(8, ' ');
            let xy: Vec<char> = it.next().unwrap_or("..").chars().collect();
            let path = it.nth(6).unwrap_or("");
            if xy.len() == 2 && !path.is_empty() {
                v.push(Change { path: path.into(), from: String::new(), kind: kind_of(xy[0], xy[1]), staged: xy[0] != '.', unstaged: xy[1] != '.' });
            }
        } else if let Some(r) = l.strip_prefix("2 ") {
            // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path><TAB><origPath>
            let mut it = r.splitn(9, ' ');
            let xy: Vec<char> = it.next().unwrap_or("..").chars().collect();
            let rest = it.nth(7).unwrap_or("");
            let (path, from) = rest.split_once('\t').unwrap_or((rest, ""));
            if xy.len() == 2 && !path.is_empty() {
                v.push(Change { path: path.into(), from: from.into(), kind: ChangeKind::Renamed, staged: xy[0] != '.', unstaged: xy[1] != '.' });
            }
        } else if let Some(r) = l.strip_prefix("u ") {
            // <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            let path = r.splitn(10, ' ').nth(9).unwrap_or("");
            if !path.is_empty() {
                v.push(Change { path: path.into(), from: String::new(), kind: ChangeKind::Conflicted, staged: false, unstaged: true });
            }
        } else if let Some(p) = l.strip_prefix("? ") {
            v.push(Change { path: p.into(), from: String::new(), kind: ChangeKind::Untracked, staged: false, unstaged: true });
        }
    }
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

// ------------------------------------------------------------------ clones on this computer

/// A clone found in the Repositories folder or listed by a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalClone {
    pub name: String,
    pub dir: PathBuf,
    /// `origin`, credentials removed.
    pub remote: Option<String>,
    pub branch: String,
}

impl LocalClone {
    pub fn key(&self) -> Option<String> {
        self.remote.as_deref().map(remote_key)
    }
}

/// The folders under `root` that are git repositories (a `.git` folder or file), and the `extra` folders (the repositories that
/// project.json lists). Sorted by name; at most 200.
pub fn find_clones(run: &Arc<dyn Runner>, root: &Path, extra: &[PathBuf]) -> Vec<LocalClone> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) && p.join(".git").exists() {
            dirs.push(p);
        }
    }
    for p in extra {
        if p.join(".git").exists() && !dirs.contains(p) {
            dirs.push(p.clone());
        }
    }
    dirs.sort();
    dirs.truncate(200);
    dirs.into_iter()
        .map(|d| {
            let l = LocalRepo::new(run.clone(), &d, &std::env::temp_dir());
            LocalClone { name: d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), remote: l.origin(), branch: l.branch().unwrap_or_default(), dir: d }
        })
        .collect()
}

#[cfg(test)]
#[path = "gitbrowse_tests.rs"]
mod tests;
