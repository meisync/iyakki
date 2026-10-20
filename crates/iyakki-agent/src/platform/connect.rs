//! Connect a workspace to a git service (Gitea): create the repository through the API (private by default, with IYAKKI's own token from
//! the vault), set `origin` in the workspace's git configuration (the URL never holds a credential), and push the current branch after a
//! second approval. The token travels only in a 0600 temp file for curl (`-K`) and in environment variables for git; never in an
//! argument, a URL, a log or a result.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::engine::first_line;
use super::installed::{self, Installed, WsLink};
use super::secrets::SecretFile;
use super::*;
use crate::wsexec::Runner;

// ------------------------------------------------------------------------------------------------ HTTP

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResp {
    pub status: u16,
    pub body: String,
}

/// The one way platform code speaks HTTP to a service. The token (if any) is sent as `Authorization: token <t>`.
pub trait Http: Send + Sync {
    fn request(&self, method: &str, url: &str, token: Option<&str>, body: Option<&str>) -> Result<HttpResp, String>;
}

/// curl through the [`Runner`], with everything that may hold a secret in a private temp config file.
pub struct CurlHttp {
    pub run: Arc<dyn Runner>,
    pub tmp: PathBuf,
}

fn curl_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}

impl Http for CurlHttp {
    fn request(&self, method: &str, url: &str, token: Option<&str>, body: Option<&str>) -> Result<HttpResp, String> {
        if !self.run.has("curl") {
            return Err("curl is not installed".into());
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("only http and https addresses are used".into());
        }
        let mut cfg = format!("silent\nshow-error\nmax-time = 20\nurl = {}\nrequest = {}\nwrite-out = \"\\n%{{http_code}}\"\n", curl_quote(url), curl_quote(method));
        if let Some(t) = token {
            cfg.push_str(&format!("header = {}\n", curl_quote(&format!("Authorization: token {t}"))));
        }
        cfg.push_str("header = \"Accept: application/json\"\n");
        if let Some(b) = body {
            cfg.push_str(&format!("header = \"Content-Type: application/json\"\ndata = {}\n", curl_quote(b)));
        }
        let f = SecretFile::new(&self.tmp, &cfg)?;
        let o = self.run.run(&std::env::temp_dir(), &["curl".into(), "-K".into(), f.path.display().to_string()], &[], Duration::from_secs(30), 256 * 1024, None);
        drop(f);
        if o.missing {
            return Err("curl is not installed".into());
        }
        let text = o.stdout;
        let (b, code) = text.rsplit_once('\n').ok_or_else(|| if o.stderr.trim().is_empty() { "no answer".to_string() } else { first_line(&o.stderr) })?;
        let status: u16 = code.trim().parse().map_err(|_| first_line(&o.stderr))?;
        if status == 0 {
            return Err(if o.stderr.trim().is_empty() { "could not connect".into() } else { first_line(&o.stderr) });
        }
        Ok(HttpResp { status, body: b.to_string() })
    }
}

/// A scripted [`Http`] for tests: the first rule whose `"METHOD url-part"` is contained in the request answers.
#[derive(Default)]
pub struct FakeHttp {
    rules: Mutex<Vec<(String, u16, String)>>,
    pub calls: Mutex<Vec<(String, String, Option<String>, Option<String>)>>,
}

impl FakeHttp {
    pub fn answer(&self, rule: &str, status: u16, body: &str) -> &Self {
        self.rules.lock().unwrap_or_else(|e| e.into_inner()).push((rule.into(), status, body.into()));
        self
    }
    pub fn lines(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|c| format!("{} {}", c.0, c.1)).collect()
    }
}

impl Http for FakeHttp {
    fn request(&self, method: &str, url: &str, token: Option<&str>, body: Option<&str>) -> Result<HttpResp, String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push((method.into(), url.into(), token.map(str::to_string), body.map(str::to_string)));
        let line = format!("{method} {url}");
        let r = self.rules.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _, _)| line.contains(k.as_str())).cloned();
        match r {
            Some((_, status, body)) => Ok(HttpResp { status, body }),
            None => Err("could not connect".into()),
        }
    }
}

// ------------------------------------------------------------------------------------------------ workspace

#[derive(Clone, Debug)]
pub struct WsInfo {
    pub name: String,
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ssh,
}

#[derive(Clone, Debug)]
pub struct ConnectOpts {
    /// Empty: from the workspace name.
    pub repo: String,
    pub private: bool,
    pub transport: Transport,
    /// Replace an `origin` that points somewhere else (shown in the plan).
    pub replace_origin: bool,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts { repo: String::new(), private: true, transport: Transport::Https, replace_origin: false }
    }
}

/// A repository name from a workspace name: letters, digits, `-`, `_`, `.`.
pub fn repo_name(ws: &str) -> String {
    let mut s: String = ws.trim().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c.to_ascii_lowercase() } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches(|c| c == '-' || c == '.').trim_end_matches(".git").to_string();
    if s.is_empty() { "workspace".into() } else { s.chars().take(100).collect() }
}

fn git(run: &dyn Runner, root: &Path, args: &[&str], env: &[(String, String)]) -> crate::wsexec::Out {
    let mut argv: Vec<String> = vec!["git".into(), "-C".into(), root.display().to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    run.run(root, &argv, env, Duration::from_secs(120), 64 * 1024, None)
}

#[derive(Clone, Debug)]
pub struct ConnectPlan {
    pub inst: Installed,
    pub ws: WsInfo,
    pub repo: String,
    pub private: bool,
    pub transport: Transport,
    pub branch: String,
    pub origin: Option<String>,
    pub replace_origin: bool,
    pub commits: usize,
    pub base: String,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

fn base_url(inst: &Installed) -> Result<String, String> {
    inst.web_url().map(|u| u.trim_end_matches('/').to_string()).ok_or_else(|| "this service has no web address".to_string())
}

/// Read the workspace's git state and the service's record (no network, no change).
pub fn plan_connect(pl: &Platform, inst: &Installed, ws: &WsInfo, o: &ConnectOpts) -> Result<ConnectPlan, String> {
    if inst.kind != "git" {
        return Err(format!("{} is not a git service", inst.title));
    }
    let base = base_url(inst)?;
    let repo = if o.repo.trim().is_empty() { repo_name(&ws.name) } else { o.repo.trim().to_string() };
    if repo.is_empty() || !repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || repo == "." || repo == ".." || repo.ends_with(".git") {
        return Err(format!("`{repo}` is not a repository name (letters, digits, - _ . only)"));
    }
    let mut p = ConnectPlan { inst: inst.clone(), ws: ws.clone(), repo, private: o.private, transport: o.transport, branch: String::new(), origin: None, replace_origin: o.replace_origin, commits: 0, base: base.clone(), blockers: vec![], warnings: vec![] };
    let inside = git(&*pl.run, &ws.root, &["rev-parse", "--is-inside-work-tree"], &[]);
    if !inside.ok() || inside.stdout.trim() != "true" {
        p.blockers.push(format!("{} is not a git repository: run `git init` in it first (the IDE offers it)", ws.root.display()));
        return Ok(p);
    }
    let br = git(&*pl.run, &ws.root, &["symbolic-ref", "--short", "HEAD"], &[]);
    p.branch = br.stdout.trim().to_string();
    if p.branch.is_empty() {
        p.blockers.push("HEAD is detached: check out a branch first".into());
    }
    let n = git(&*pl.run, &ws.root, &["rev-list", "--count", "HEAD"], &[]);
    p.commits = n.stdout.trim().parse().unwrap_or(0);
    if p.commits == 0 {
        p.blockers.push("the branch has no commit yet: commit something first (a push needs at least one commit)".into());
    }
    let ou = git(&*pl.run, &ws.root, &["remote", "get-url", "origin"], &[]);
    if ou.ok() && !ou.stdout.trim().is_empty() {
        p.origin = Some(ou.stdout.trim().to_string());
    }
    if !pl.vault.has(inst.token_vault().unwrap_or("-")) {
        p.blockers.push("the IYAKKI token of this service is not in your vault (unlock the vault, or connect again)".into());
    }
    if inst.options.get("exposure") == "ClusterIP" {
        p.blockers.push("this Gitea is only reachable inside the cluster (ClusterIP); expose it with NodePort or Ingress to connect a workspace".into());
    }
    if base.starts_with("http://") {
        p.warnings.push("the service speaks plain http: the token and your code travel unencrypted on the network between this machine and the service".into());
    }
    Ok(p)
}

pub fn preview_connect(p: &ConnectPlan) -> String {
    let mut s = format!("CONNECT the workspace {} to {} at {}\n\n", p.ws.name, p.inst.title, p.base);
    for b in &p.blockers {
        s.push_str(&format!("CANNOT BE APPLIED: {b}\n"));
    }
    for w in &p.warnings {
        s.push_str(&format!("warning: {w}\n"));
    }
    s.push_str(&format!("1. Ask the service who the IYAKKI token belongs to: GET {}/api/v1/user (the token comes from your vault entry uvault:{}; it is never shown)\n", p.base, p.inst.token_vault().unwrap_or("?")));
    s.push_str(&format!("2. Create the repository {} ({}): POST {}/api/v1/user/repos; if it already exists it is reused\n", p.repo, if p.private { "PRIVATE" } else { "PUBLIC: anyone who can reach the service may read it" }, p.base));
    s.push_str(&format!("3. In {}: git remote {} origin <the repository's {} address, with no credential in it>\n", p.ws.root.display(), if p.origin.is_some() { "set-url" } else { "add" }, if p.transport == Transport::Ssh { "SSH" } else { "HTTPS" }));
    if let Some(o) = &p.origin {
        if p.replace_origin {
            s.push_str(&format!("   origin currently is {o}: it will be REPLACED\n"));
        } else {
            s.push_str(&format!("   origin already is {o}: nothing is replaced; if it differs from the new repository this step is refused\n"));
        }
    }
    s.push_str(&format!("\nNothing is pushed by this step: pushing the branch {} ({} commit(s)) is a separate approval afterwards.\n", p.branch, p.commits));
    if p.transport == Transport::Ssh {
        s.push_str("SSH needs your public key added to your account in the service; IYAKKI does not manage keys.\n");
    }
    s.trim_end().to_string()
}

#[derive(Clone, Debug)]
pub struct Connected {
    pub owner: String,
    pub repo_url: String,
    pub remote_url: String,
    pub created: bool,
    pub origin_set: bool,
}

fn json_str(v: &str, path: &[&str]) -> Option<String> {
    let mut j: serde_json::Value = serde_json::from_str(v).ok()?;
    for k in path {
        j = j.get(k)?.clone();
    }
    j.as_str().map(str::to_string)
}

fn token_of(pl: &Platform, inst: &Installed) -> Result<zeroize::Zeroizing<String>, String> {
    let name = inst.token_vault().ok_or("this service has no IYAKKI token")?;
    pl.vault.get(name)
}

/// Create the repository and set `origin`. `approved` must be true.
pub fn connect(pl: &Platform, p: &ConnectPlan, approved: bool) -> Result<Connected, String> {
    if !approved {
        return Err("connecting the workspace was not approved".into());
    }
    if !p.blockers.is_empty() {
        return Err(format!("not connected: {}", p.blockers.join("; ")));
    }
    let token = token_of(pl, &p.inst)?;
    let api = format!("{}/api/v1", p.base);
    let me = pl.http.request("GET", &format!("{api}/user"), Some(&token), None).map_err(|e| format!("could not reach {}: {e}", p.base))?;
    if me.status != 200 {
        return Err(format!("the service refused the IYAKKI token (HTTP {}); connect the service again", me.status));
    }
    let owner = json_str(&me.body, &["login"]).ok_or("the service did not say who the token belongs to")?;
    let body = serde_json::json!({"name": p.repo, "private": p.private, "description": format!("Workspace {} (IYAKKI)", p.ws.name), "auto_init": false, "default_branch": p.branch}).to_string();
    let r = pl.http.request("POST", &format!("{api}/user/repos"), Some(&token), Some(&body)).map_err(|e| format!("could not create the repository: {e}"))?;
    let (created, repo_json) = match r.status {
        201 => (true, r.body),
        409 | 422 => {
            let g = pl.http.request("GET", &format!("{api}/repos/{owner}/{}", p.repo), Some(&token), None).map_err(|e| e.to_string())?;
            if g.status != 200 {
                return Err(format!("the repository {} could not be created (HTTP {}) and does not exist either", p.repo, r.status));
            }
            (false, g.body)
        }
        401 | 403 => return Err(format!("the service refused to create the repository (HTTP {}): the token lacks the repository write scope", r.status)),
        s => return Err(format!("creating the repository failed (HTTP {s}): {}", first_line(&r.body))),
    };
    let remote = match p.transport {
        Transport::Https => json_str(&repo_json, &["clone_url"]).unwrap_or_else(|| format!("{}/{owner}/{}.git", p.base, p.repo)),
        Transport::Ssh => json_str(&repo_json, &["ssh_url"]).ok_or("the service did not give an SSH address")?,
    };
    if p.transport == Transport::Https && (remote.contains('@') || !remote.starts_with("http")) {
        return Err(format!("refused: the repository address {remote} holds a credential or is not http(s)"));
    }
    let repo_url = json_str(&repo_json, &["html_url"]).unwrap_or_else(|| format!("{}/{owner}/{}", p.base, p.repo));
    let mut origin_set = false;
    match &p.origin {
        Some(o) if *o == remote => {}
        Some(o) if !p.replace_origin => return Err(format!("the repository exists but origin is already {o}, which is not this repository; nothing was changed. Choose to replace origin if you mean it")),
        Some(_) => {
            let o = git(&*pl.run, &p.ws.root, &["remote", "set-url", "origin", &remote], &[]);
            if !o.ok() {
                return Err(format!("git remote set-url failed: {}", first_line(&o.combined())));
            }
            origin_set = true;
        }
        None => {
            let o = git(&*pl.run, &p.ws.root, &["remote", "add", "origin", &remote], &[]);
            if !o.ok() {
                return Err(format!("git remote add failed: {}", first_line(&o.combined())));
            }
            origin_set = true;
        }
    }
    let mut rec = installed::find(&pl.dir, &pl.user, &p.inst.id).unwrap_or_else(|| p.inst.clone());
    rec.workspaces.retain(|w| w.workspace != p.ws.name);
    rec.workspaces.push(WsLink { workspace: p.ws.name.clone(), repo_url: repo_url.clone() });
    rec.updated_at = crate::clock::now_unix();
    installed::upsert(&pl.dir, &pl.user, rec)?;
    pl.audit("connect_workspace", &format!("{}:{}:{} ok", p.inst.id, p.ws.name, p.repo));
    Ok(Connected { owner, repo_url, remote_url: remote, created, origin_set })
}

// ------------------------------------------------------------------------------------------------ push

#[derive(Clone, Debug)]
pub struct PushPlan {
    pub inst: Installed,
    pub ws: WsInfo,
    pub branch: String,
    pub remote: String,
    pub commits: usize,
    pub transport: Transport,
    pub blockers: Vec<String>,
}

/// What a push would do: the branch, where, how many commits. Read-only.
pub fn plan_push(pl: &Platform, inst: &Installed, ws: &WsInfo) -> Result<PushPlan, String> {
    let br = git(&*pl.run, &ws.root, &["symbolic-ref", "--short", "HEAD"], &[]);
    let branch = br.stdout.trim().to_string();
    let ou = git(&*pl.run, &ws.root, &["remote", "get-url", "origin"], &[]);
    let remote = ou.stdout.trim().to_string();
    let n = git(&*pl.run, &ws.root, &["rev-list", "--count", "HEAD"], &[]);
    let mut p = PushPlan { inst: inst.clone(), ws: ws.clone(), branch, remote: remote.clone(), commits: n.stdout.trim().parse().unwrap_or(0), transport: if remote.starts_with("http") { Transport::Https } else { Transport::Ssh }, blockers: vec![] };
    if p.branch.is_empty() {
        p.blockers.push("HEAD is detached: check out a branch first".into());
    }
    if remote.is_empty() {
        p.blockers.push("this workspace has no origin: connect it first".into());
    } else if let Ok(base) = base_url(inst) {
        let host = |u: &str| u.split("://").nth(1).unwrap_or(u).split('/').next().unwrap_or("").to_string();
        if p.transport == Transport::Https && host(&remote) != host(&base) {
            p.blockers.push(format!("origin ({remote}) is not on this service ({base}): IYAKKI only sends its token to the service it belongs to"));
        }
    }
    if p.remote.contains('@') && p.transport == Transport::Https {
        p.blockers.push("origin holds a credential in its address: remove it (git remote set-url origin <address without credentials>) so it cannot leak".into());
    }
    if p.transport == Transport::Https && !pl.vault.has(inst.token_vault().unwrap_or("-")) {
        p.blockers.push("the IYAKKI token is not in your vault".into());
    }
    Ok(p)
}

pub fn preview_push(p: &PushPlan) -> String {
    let mut s = format!("PUSH the branch {} ({} commit(s)) of {} to {}\n\ngit push --set-upstream origin {}\n", p.branch, p.commits, p.ws.name, p.remote, p.branch);
    s.push_str(if p.transport == Transport::Https { "The IYAKKI token is sent for this one command through git's environment (never in the address, an argument or a file in the repository).\n" } else { "Over SSH with your own key; IYAKKI sends no token.\n" });
    for b in &p.blockers {
        s.push_str(&format!("CANNOT BE APPLIED: {b}\n"));
    }
    s.trim_end().to_string()
}

pub fn push(pl: &Platform, p: &PushPlan, approved: bool) -> Result<String, String> {
    if !approved {
        return Err("the push was not approved".into());
    }
    if !p.blockers.is_empty() {
        return Err(format!("not pushed: {}", p.blockers.join("; ")));
    }
    let mut env: Vec<(String, String)> = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
    let token = if p.transport == Transport::Https { Some(token_of(pl, &p.inst)?) } else { None };
    if let Some(t) = &token {
        // the login is needed for basic auth: ask the service (the token is only in the header)
        let base = base_url(&p.inst)?;
        let me = pl.http.request("GET", &format!("{base}/api/v1/user"), Some(t), None).map_err(|e| format!("could not reach {base}: {e}"))?;
        let login = json_str(&me.body, &["login"]).filter(|_| me.status == 200).ok_or("the service refused the IYAKKI token")?;
        let basic = crate::story::base64_encode(format!("{login}:{}", t.as_str()).as_bytes());
        env.extend([
            ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
            ("GIT_CONFIG_KEY_0".into(), "credential.helper".into()),
            ("GIT_CONFIG_VALUE_0".into(), String::new()),
            ("GIT_CONFIG_KEY_1".into(), "http.extraHeader".into()),
            ("GIT_CONFIG_VALUE_1".into(), format!("Authorization: Basic {basic}")),
        ]);
    }
    let o = git(&*pl.run, &p.ws.root, &["push", "--set-upstream", "origin", &p.branch], &env);
    let mut text = o.combined();
    if let Some(t) = &token {
        text = text.replace(t.as_str(), "***");
    }
    if !o.ok() {
        return Err(format!("git push failed: {}", text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim()));
    }
    pl.audit("push", &format!("{}:{}:{} ok", p.inst.id, p.ws.name, p.branch));
    Ok(format!("pushed {} to {}", p.branch, p.remote))
}
