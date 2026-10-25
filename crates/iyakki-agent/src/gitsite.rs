//! Git sites and repositories: reading a clone URL (HTTPS, `ssh://`, scp-like `git@host:path`), choosing a folder for it inside a
//! workspace, and the `repos` list of `.iyakki/project.json`.
//!
//! Rules that hold everywhere:
//! * **No credentials in a URL.** `https://user:token@host/…` is refused with a pointer to Settings → Git and SSH keys; the user
//!   name of an SSH URL (`git@`) is not a secret and is kept. What the person typed is never echoed back with a password in it
//!   ([`redact_url`] is used for every message).
//! * **Only network transports.** `file://`, local paths, `ext::`, `fd::` and anything that starts with `-` are refused (a clone
//!   URL must never become an option or a program to run); tests may allow local paths with [`ParseOptions::allow_local`].
//! * **A folder never leaves the workspace.** [`folder_for`] gives a plain name (letters, digits, `.`, `_`, `-`), never `..`, never
//!   an existing non-empty folder, de-duplicated against the other repositories of the same batch.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Https,
    Http,
    Ssh,
    /// Only with [`ParseOptions::allow_local`] (tests).
    Local,
}

/// A clone URL taken apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoUrl {
    pub scheme: Scheme,
    /// Lower-case host name; empty for a local path.
    pub host: String,
    pub port: Option<u16>,
    /// SSH user (`git`); for HTTPS the user name only when the URL carried one without a password (not a secret).
    pub user: Option<String>,
    /// The path on the server, without a leading slash for scp-like URLs.
    pub path: String,
    /// The URL as git should get it (no password can be in it).
    pub url: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions {
    pub allow_local: bool,
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['-', '.']) && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
}

fn valid_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 512 && !p.starts_with('-') && !p.split('/').any(|s| s == "..") && !p.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '\'' | '"' | '`' | '$' | ';' | '|' | '&' | '<' | '>'))
}

fn valid_user(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// The URL with any `user:password@` reduced to `user@` (for messages and logs).
pub fn redact_url(s: &str) -> String {
    if let Some((scheme, rest)) = s.split_once("://") {
        let (auth, tail) = rest.split_once('/').map_or((rest, ""), |x| x);
        if let Some((cred, host)) = auth.rsplit_once('@') {
            let user = cred.split(':').next().unwrap_or("");
            let sep = if rest.contains('/') { "/" } else { "" };
            return format!("{scheme}://{user}@{host}{sep}{tail}");
        }
    }
    s.to_string()
}

pub fn parse(input: &str, opts: ParseOptions) -> Result<RepoUrl, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("the address is empty".into());
    }
    if s.starts_with('-') {
        return Err("an address cannot start with a dash".into());
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("an address has no spaces or control characters".into());
    }
    let low = s.to_ascii_lowercase();
    if low.starts_with("ext::") || low.starts_with("fd::") || low.contains("::") && !s.contains("://") && !s.contains('@') {
        return Err("that kind of address runs a program; only https:// and ssh addresses are allowed".into());
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        let sch = match scheme.to_ascii_lowercase().as_str() {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            "ssh" => Scheme::Ssh,
            "file" if opts.allow_local => {
                return Ok(RepoUrl { scheme: Scheme::Local, host: String::new(), port: None, user: None, path: rest.to_string(), url: s.to_string() });
            }
            "git" => return Err("git:// is not encrypted and not supported; use https:// or ssh".into()),
            other => return Err(format!("`{other}://` is not supported; use https:// or ssh://")),
        };
        let (auth, path) = rest.split_once('/').ok_or("the address has no repository path after the host")?;
        let (cred, hostport) = match auth.rsplit_once('@') {
            Some((c, h)) => (Some(c), h),
            None => (None, auth),
        };
        let mut user = None;
        if let Some(c) = cred {
            if c.contains(':') {
                return Err("a password or token cannot be part of an address: remove it and add it in Settings → Git and SSH keys".into());
            }
            if !valid_user(c) {
                return Err("the user name in the address is not valid".into());
            }
            user = Some(c.to_string());
        }
        let (host, port) = split_host_port(hostport)?;
        if !valid_path(path) {
            return Err("the repository path in the address is not valid".into());
        }
        let url = format!("{}://{}{}{}/{}", scheme.to_ascii_lowercase(), user.as_ref().map_or(String::new(), |u| format!("{u}@")), host, port.map_or(String::new(), |p| format!(":{p}")), path);
        return Ok(RepoUrl { scheme: sch, host, port, user, path: path.to_string(), url });
    }
    // scp-like: [user@]host:path
    if let Some((left, path)) = s.split_once(':') {
        if !left.contains('/') && !path.is_empty() {
            let (user, host) = match left.rsplit_once('@') {
                Some((u, h)) => (Some(u), h),
                None => (None, left),
            };
            if let Some(u) = user {
                if !valid_user(u) {
                    return Err("the user name in the address is not valid".into());
                }
            }
            if host.len() == 1 && opts.allow_local {
                // a drive letter, not a host
            } else {
                if !valid_host(host) || host.contains(':') {
                    return Err("the host name in the address is not valid".into());
                }
                let path = path.trim_start_matches('/');
                if !valid_path(path) {
                    return Err("the repository path in the address is not valid".into());
                }
                let host = host.to_ascii_lowercase();
                let url = format!("{}{}:{}", user.map_or(String::new(), |u| format!("{u}@")), host, path);
                return Ok(RepoUrl { scheme: Scheme::Ssh, host, port: None, user: user.map(String::from), path: path.to_string(), url });
            }
        }
    }
    if opts.allow_local && s.starts_with('/') {
        return Ok(RepoUrl { scheme: Scheme::Local, host: String::new(), port: None, user: None, path: s.to_string(), url: s.to_string() });
    }
    Err("that is not a repository address: use https://host/owner/repo.git or git@host:owner/repo.git".into())
}

fn split_host_port(hp: &str) -> Result<(String, Option<u16>), String> {
    let (h, p) = match hp.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, Some(p)),
        _ => (hp, None),
    };
    let port = match p {
        Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p > 0).ok_or("the port in the address is not valid")?),
        None => None,
    };
    if !valid_host(h) {
        return Err("the host name in the address is not valid".into());
    }
    Ok((h.to_ascii_lowercase(), port))
}

impl RepoUrl {
    /// The key for the host table: the host name (`github.com`); the port is not part of it, so one rule serves a server's
    /// SSH and HTTPS addresses.
    pub fn site(&self) -> String {
        self.host.clone()
    }
    /// The port for the SSH connection (22 unless the address says otherwise).
    pub fn ssh_port(&self) -> u16 {
        self.port.unwrap_or(22)
    }
    pub fn is_ssh(&self) -> bool {
        self.scheme == Scheme::Ssh
    }
    pub fn is_https(&self) -> bool {
        matches!(self.scheme, Scheme::Https | Scheme::Http)
    }
    /// `owner/repo` (the last two path segments, `.git` removed) for titles.
    pub fn short(&self) -> String {
        let p = self.path.trim_end_matches('/').trim_end_matches(".git");
        let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        match segs.len() {
            0 => self.host.clone(),
            1 => segs[0].to_string(),
            n => format!("{}/{}", segs[n - 2], segs[n - 1]),
        }
    }
}

// ------------------------------------------------------------------ the lines of the Clone dialog

/// One line of the dialog's text area: `URL`, optionally followed by `#branch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneLine {
    pub url: String,
    pub branch: String,
}

/// Splits the text area into lines: blank lines and `//`-free comment lines starting with `#` are skipped; `url #branch` and
/// `url#branch` both name a branch. A line keeps its text when it is not a valid address (the dialog reports it by number).
pub fn split_lines(text: &str) -> Vec<CloneLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.split_once('#') {
            Some((u, b)) => CloneLine { url: u.trim().to_string(), branch: b.trim().to_string() },
            None => CloneLine { url: l.to_string(), branch: String::new() },
        })
        .collect()
}

pub fn valid_branch(b: &str) -> bool {
    b.is_empty() || (b.len() <= 120 && !b.starts_with(['-', '/']) && !b.ends_with(['/', '.']) && !b.contains("..") && !b.contains("@{") && !b.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')))
}

// ------------------------------------------------------------------ folders

/// A plain folder name from free text: letters, digits, `.`, `_`, `-`; nothing else survives, and never `.`/`..`.
pub fn clean_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    out.chars().take(80).collect()
}

/// The name a URL suggests: its last path segment without `.git`.
pub fn suggested_name(url: &RepoUrl) -> String {
    let p = url.path.trim_end_matches('/');
    let last = p.rsplit('/').next().unwrap_or("");
    let n = clean_name(last.strip_suffix(".git").unwrap_or(last));
    if n.is_empty() { "repo".into() } else { n }
}

fn is_empty_dir(p: &Path) -> bool {
    std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false)
}

/// Whether `name` can be used under `root`: it does not exist, or is an empty folder, and is not taken by another repository of
/// the batch. A symbolic link is never usable (it could point out of the workspace).
pub fn folder_free(root: &Path, name: &str, taken: &HashSet<String>) -> bool {
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') || taken.contains(&name.to_ascii_lowercase()) {
        return false;
    }
    let p = root.join(name);
    match std::fs::symlink_metadata(&p) {
        Err(_) => true,
        Ok(m) => m.is_dir() && !m.file_type().is_symlink() && is_empty_dir(&p),
    }
}

/// The folder a repository goes to. `wanted` is the person's choice (an error if it cannot be used); without one the name comes from
/// the URL and a number is added (`app`, `app-2`, …) until it is free. The result is a single plain name inside `root`.
pub fn folder_for(root: &Path, url: &RepoUrl, wanted: &str, taken: &HashSet<String>) -> Result<String, String> {
    if !wanted.trim().is_empty() {
        let w = wanted.trim();
        if w.contains('/') || w.contains('\\') || w == "." || w == ".." || w.starts_with('.') {
            return Err(format!("`{w}` is not a folder name: use a plain name; the repository is cloned inside the workspace"));
        }
        let n = clean_name(w);
        if n != w {
            return Err(format!("`{w}` has characters a folder name should not have; use letters, digits, `.`, `_` and `-` (for example `{n}`)"));
        }
        if !folder_free(root, &n, taken) {
            return Err(format!("the folder `{n}` already exists with files in it (or is used by another repository of this list); choose another name"));
        }
        return Ok(n);
    }
    let base = suggested_name(url);
    if folder_free(root, &base, taken) {
        return Ok(base);
    }
    for i in 2..1000 {
        let c = format!("{base}-{i}");
        if folder_free(root, &c, taken) {
            return Ok(c);
        }
    }
    Err("no free folder name was found".into())
}

// ------------------------------------------------------------------ project.json: the repos list

/// One repository of a workspace (`repos` in `.iyakki/project.json`). No secret is in it: `key` is the id of the SSH key used, not the key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoEntry {
    pub url: String,
    /// The folder, relative to the workspace root.
    pub path: String,
    pub branch: String,
    /// The SSH key id or `token` that was used (empty: none needed).
    pub key: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub depth: u32,
    /// Test and run commands for this repository (run inside its folder); empty: the workspace's own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub test: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub run: String,
    /// `cloned`, `failed` or `pending`: where the last clone stands.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Adds or replaces (same `path`) an entry. The list stays in the order it was made.
pub fn upsert(repos: &mut Vec<RepoEntry>, e: RepoEntry) {
    if let Some(x) = repos.iter_mut().find(|r| r.path == e.path) {
        // commands the person set survive a re-clone
        let (t, r) = (x.test.clone(), x.run.clone());
        *x = e;
        if x.test.is_empty() {
            x.test = t;
        }
        if x.run.is_empty() {
            x.run = r;
        }
    } else {
        repos.push(e);
    }
}

/// The lines about the repositories for the assistant and the external agents ("Repositories in this workspace: …").
pub fn describe(repos: &[RepoEntry]) -> String {
    if repos.is_empty() {
        return String::new();
    }
    let mut s = String::from("Repositories in this workspace (each in its own folder; run git inside the folder):\n");
    for r in repos {
        let mut line = format!("- {}/", r.path.trim_end_matches('/'));
        let url = redact_url(&r.url);
        line.push_str(&format!(" <- {url}"));
        if !r.branch.is_empty() {
            line.push_str(&format!(" (branch {})", r.branch));
        }
        if !r.test.is_empty() {
            line.push_str(&format!("; test: {}", r.test));
        }
        if !r.run.is_empty() {
            line.push_str(&format!("; run: {}", r.run));
        }
        s.push_str(&line);
        s.push('\n');
    }
    s
}

#[cfg(test)]
#[path = "gitsite_tests.rs"]
mod tests;
