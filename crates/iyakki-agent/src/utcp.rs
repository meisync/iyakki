//! UTCP (Universal Tool Calling Protocol): a client for UTCP *manuals* and their `http` tools (docs/development/UTCP-PLAN.md, step 1).
//!
//! A manual is a JSON document (`utcp_version`, `manual_version`, `info`, `tools`); each tool carries a call template that says how to call
//! the tool's own endpoint directly. This module reads manuals, builds the HTTP request for a tool call, and sends it. Written from the
//! specification text; no UTCP library. Not here yet: `cli`, `text`, `sse`, `websocket`, `mcp` templates and OAuth2 (each is refused with
//! a plain sentence, never half-done).
//!
//! Safety rules (each has a test):
//! * `https` only; plain `http` only for the machine itself (loopback);
//! * a manual that came from a remote address may not name loopback or local-network tool URLs;
//! * no redirects are followed (the transport refuses them), so a credential never travels to another host;
//! * path values are percent-encoded; a path value cannot add a `/` or a `?`;
//! * credentials are never written in a manual: `${NAME}` is looked up in the vault by the caller, a literal secret is refused;
//! * the manual and every reply have a size limit.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use iyakki_proxy::Upstream;

/// The biggest manual read (bytes).
pub const MAX_MANUAL: usize = 1024 * 1024;
/// The biggest reply returned to the assistant (bytes); the rest is cut and says so.
pub const MAX_REPLY: usize = 64 * 1024;
/// Tools one manual may offer.
pub const MAX_TOOLS: usize = 200;

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "DELETE", "PATCH"];

/// Where an API key goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyPlace {
    Header,
    Query,
    Cookie,
}

/// How a call proves who it is. Values hold `${NAME}` placeholders for the vault, never the secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    ApiKey { value: String, var_name: String, place: KeyPlace },
    Basic { username: String, password: String },
}

/// The `http` call template of one tool.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpTemplate {
    pub url: String,
    pub method: String,
    pub content_type: String,
    pub headers: BTreeMap<String, String>,
    pub body_field: Option<String>,
    pub header_fields: Vec<String>,
    pub auth: Option<Auth>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UtcpTool {
    pub name: String,
    pub description: String,
    pub inputs: Value,
    pub outputs: Value,
    pub tags: Vec<String>,
    pub template: HttpTemplate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manual {
    pub utcp_version: String,
    pub manual_version: String,
    pub title: String,
    pub description: String,
    pub tools: Vec<UtcpTool>,
    /// Tools the manual lists that this version cannot use, each with the reason (shown to the person, never silent).
    pub skipped: Vec<String>,
    /// The manual came from this machine or its own network: its tools may name local addresses too.
    pub source_local: bool,
}

// -------------------------------------------------------------------------------------------------------- URLs

/// The host part of an `http(s)` URL; `None` when the URL has user info, no host, or another scheme.
pub fn url_host(url: &str) -> Option<(&str, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    if auth.is_empty() || auth.contains('@') {
        return None;
    }
    let host = if let Some(r) = auth.strip_prefix('[') {
        r.split(']').next()?.to_string()
    } else {
        auth.rsplit_once(':').map_or(auth, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { auth }).to_string()
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme, host.to_ascii_lowercase()))
}

/// This machine itself.
pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// This machine or its own network (loopback, private, link-local, `.local`).
pub fn is_local_host(host: &str) -> bool {
    if is_loopback_host(host) || host.ends_with(".local") || host.ends_with(".internal") || !host.contains('.') && host.parse::<IpAddr>().is_err() {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_broadcast(),
        Ok(IpAddr::V6(a)) => {
            let s = a.segments()[0];
            a.is_unspecified() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// The rules for a URL a tool will call (see the module notes).
pub fn check_tool_url(url: &str, source_local: bool) -> Result<(), String> {
    let (scheme, host) = url_host(url).ok_or_else(|| format!("`{url}` is not an http(s) address (user names in the address are refused)"))?;
    if scheme == "http" && !is_loopback_host(&host) {
        return Err(format!("`{url}` is not https; plain http is allowed only for this machine"));
    }
    if is_local_host(&host) && !source_local {
        return Err(format!("`{url}` is an address on this machine or its network, and the manual came from the internet"));
    }
    Ok(())
}

/// Percent-encoding of a value that goes into a URL path or query.
pub fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

// ------------------------------------------------------------------------------------------------------ manual

fn valid_tool_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A secret placeholder only: `${NAME}` (alone or inside a longer text such as `Bearer ${NAME}`).
fn has_placeholder(s: &str) -> bool {
    s.find("${").is_some_and(|i| s[i..].contains('}'))
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn parse_auth(v: &Value) -> Result<Option<Auth>, String> {
    if v.is_null() {
        return Ok(None);
    }
    match str_of(v, "auth_type").unwrap_or("") {
        "api_key" => {
            let value = str_of(v, "api_key").ok_or("auth api_key: the `api_key` value is missing")?.to_string();
            if !has_placeholder(&value) {
                return Err("auth api_key: a key written into a manual is refused; use ${NAME} and keep the key in the vault".into());
            }
            let place = match str_of(v, "location").unwrap_or("header") {
                "header" => KeyPlace::Header,
                "query" => KeyPlace::Query,
                "cookie" => KeyPlace::Cookie,
                o => return Err(format!("auth api_key: location `{o}` is not header, query or cookie")),
            };
            Ok(Some(Auth::ApiKey { value, var_name: str_of(v, "var_name").unwrap_or("Authorization").to_string(), place }))
        }
        "basic" => {
            let (u, p) = (str_of(v, "username").unwrap_or(""), str_of(v, "password").unwrap_or(""));
            if !has_placeholder(u) && !has_placeholder(p) || u.is_empty() || p.is_empty() {
                return Err("auth basic: user name and password must both be given as ${NAME} (the vault holds the real values)".into());
            }
            Ok(Some(Auth::Basic { username: u.into(), password: p.into() }))
        }
        "oauth2" => Err("auth oauth2 is not supported yet".into()),
        o => Err(format!("auth type `{o}` is not supported")),
    }
}

fn parse_template(v: &Value, allowed: &[String], source_local: bool) -> Result<HttpTemplate, String> {
    let kind = str_of(v, "call_template_type").ok_or("no call_template_type")?;
    if !allowed.iter().any(|a| a == kind) {
        return Err(format!("the template type `{kind}` is not in this manual's allowed_communication_protocols"));
    }
    if kind != "http" {
        return Err(format!("the template type `{kind}` is not supported yet (only http)"));
    }
    let url = str_of(v, "url").ok_or("the http template has no url")?.to_string();
    // the placeholders in the path are replaced before the call; the rest of the address must already pass
    check_tool_url(&url.replace('{', "x").replace('}', "x"), source_local)?;
    let method = str_of(v, "http_method").unwrap_or("GET").to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(format!("http_method `{method}` is not one of GET, POST, PUT, DELETE, PATCH"));
    }
    let headers = v.get("headers").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
    let header_fields = v.get("header_fields").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok(HttpTemplate {
        url,
        method,
        content_type: str_of(v, "content_type").unwrap_or("application/json").to_string(),
        headers,
        body_field: str_of(v, "body_field").map(str::to_string),
        header_fields,
        auth: parse_auth(v.get("auth").unwrap_or(&Value::Null))?,
    })
}

/// Read a manual. `source_local`: it came from this machine or its own network.
pub fn parse_manual(text: &str, source_local: bool) -> Result<Manual, String> {
    if text.len() > MAX_MANUAL {
        return Err(format!("the manual is bigger than {} KB", MAX_MANUAL / 1024));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("this is not a UTCP manual (not JSON: {e})"))?;
    let utcp_version = str_of(&v, "utcp_version").ok_or("this is not a UTCP manual: `utcp_version` is missing (an OpenAPI file is not read yet)")?.to_string();
    let tools_v = v.get("tools").and_then(Value::as_array).ok_or("the manual has no `tools` list")?;
    if tools_v.len() > MAX_TOOLS {
        return Err(format!("the manual lists {} tools; at most {MAX_TOOLS} are read", tools_v.len()));
    }
    let allowed: Vec<String> = v.get("allowed_communication_protocols").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_else(|| vec!["http".into()]);
    let info = v.get("info").cloned().unwrap_or(Value::Null);
    let mut tools = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in tools_v {
        let name = str_of(t, "name").unwrap_or("").to_string();
        if !valid_tool_name(&name) {
            skipped.push(format!("a tool called `{name}`: the name must be letters, digits, _ . - (up to 64)"));
            continue;
        }
        if !seen.insert(name.clone()) {
            skipped.push(format!("{name}: listed twice"));
            continue;
        }
        let tpl = t.get("tool_call_template").or_else(|| t.get("provider")).ok_or(()).map_err(|_| "no tool_call_template".to_string()).and_then(|x| parse_template(x, &allowed, source_local));
        match tpl {
            Ok(template) => tools.push(UtcpTool {
                name,
                description: str_of(t, "description").unwrap_or("").chars().take(500).collect(),
                inputs: t.get("inputs").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                outputs: t.get("outputs").cloned().unwrap_or(Value::Null),
                tags: t.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                template,
            }),
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
    }
    Ok(Manual {
        utcp_version,
        manual_version: str_of(&v, "manual_version").unwrap_or("").to_string(),
        title: str_of(&info, "title").unwrap_or("").to_string(),
        description: str_of(&info, "description").unwrap_or("").chars().take(500).collect(),
        tools,
        skipped,
        source_local,
    })
}

/// Fetch a manual from `url` (GET) and read it. 10 s is the transport's limit for this call.
pub fn fetch_manual(up: &dyn Upstream, url: &str) -> Result<Manual, String> {
    let (_, host) = url_host(url).ok_or_else(|| format!("`{url}` is not an http(s) address"))?;
    check_tool_url(url, true)?; // scheme rule; the local rule is about the tools' addresses, decided below
    let r = up.request("GET", url, &[("Accept".into(), "application/json".into())], &[])?;
    if (300..400).contains(&r.status) {
        return Err(format!("the address answered with a redirect ({}); use the final address of the manual", r.status));
    }
    if r.status != 200 {
        return Err(format!("the address answered {} instead of the manual", r.status));
    }
    if r.body.len() > MAX_MANUAL {
        return Err(format!("the manual is bigger than {} KB", MAX_MANUAL / 1024));
    }
    parse_manual(&String::from_utf8_lossy(&r.body), is_local_host(&host))
}

// ------------------------------------------------------------------------------------------------------- calls

/// The request a tool call becomes (before it is sent).
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `${NAME}` placeholders replaced by what `resolve` finds in the vault.
pub fn expand(s: &str, resolve: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let j = after.find('}').ok_or("a ${ in the manual is not closed")?;
        let name = &after[..j];
        out.push_str(&resolve(name).ok_or_else(|| format!("the vault has no secret called {name} (add it in Settings → Vault)"))?);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Turn a tool call into the request: path values, then the body field, then the header fields; what is left goes in the query.
pub fn prepare(tool: &UtcpTool, args: &Value, resolve: &dyn Fn(&str) -> Option<String>) -> Result<Prepared, String> {
    let obj = match args {
        Value::Null => serde_json::Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err("the arguments must be a JSON object".into()),
    };
    if let Some(req) = tool.inputs.get("required").and_then(Value::as_array) {
        for r in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(r) {
                return Err(format!("the argument `{r}` is required"));
            }
        }
    }
    let t = &tool.template;
    let mut rest = obj;
    // path placeholders
    let mut url = String::new();
    let mut tail = t.url.as_str();
    while let Some(i) = tail.find('{') {
        url.push_str(&tail[..i]);
        let j = tail[i..].find('}').ok_or("the address has a { that is not closed")? + i;
        let key = &tail[i + 1..j];
        let val = rest.remove(key).ok_or_else(|| format!("the argument `{key}` (in the address) is missing"))?;
        url.push_str(&pct(&scalar(&val)));
        tail = &tail[j + 1..];
    }
    url.push_str(tail);
    let mut headers: Vec<(String, String)> = vec![("Accept".into(), "application/json".into())];
    for (k, v) in &t.headers {
        headers.push((k.clone(), expand(v, resolve)?));
    }
    let mut body = Vec::new();
    if let Some(f) = &t.body_field {
        if let Some(v) = rest.remove(f) {
            body = match (&v, t.content_type.contains("json")) {
                (Value::String(s), false) => s.clone().into_bytes(),
                _ => serde_json::to_vec(&v).map_err(|e| e.to_string())?,
            };
            headers.push(("Content-Type".into(), t.content_type.clone()));
        }
    }
    for f in &t.header_fields {
        if let Some(v) = rest.remove(f) {
            let s = scalar(&v);
            if s.contains(['\r', '\n']) {
                return Err(format!("the value of `{f}` has a line break and cannot be a header"));
            }
            headers.push((f.clone(), s));
        }
    }
    let mut query: Vec<(String, String)> = Vec::new();
    for (k, v) in &rest {
        match v {
            Value::Array(a) => a.iter().for_each(|x| query.push((k.clone(), scalar(x)))),
            Value::Object(_) => query.push((k.clone(), v.to_string())),
            _ => query.push((k.clone(), scalar(v))),
        }
    }
    match &t.auth {
        Some(Auth::ApiKey { value, var_name, place }) => {
            let v = expand(value, resolve)?;
            match place {
                KeyPlace::Header => headers.push((var_name.clone(), v)),
                KeyPlace::Query => query.push((var_name.clone(), v)),
                KeyPlace::Cookie => headers.push(("Cookie".into(), format!("{var_name}={v}"))),
            }
        }
        Some(Auth::Basic { username, password }) => {
            let pair = format!("{}:{}", expand(username, resolve)?, expand(password, resolve)?);
            headers.push(("Authorization".into(), format!("Basic {}", b64(pair.as_bytes()))));
        }
        None => {}
    }
    if !query.is_empty() {
        let q: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect();
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&q.join("&"));
    }
    Ok(Prepared { method: t.method.clone(), url, headers, body })
}

/// Call a tool and return the reply as text (JSON is kept as JSON). A non-2xx status is an error that says the status and the start of the reply.
pub fn call(up: &dyn Upstream, manual: &Manual, tool: &str, args: &Value, resolve: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let t = manual.tools.iter().find(|t| t.name == tool).ok_or_else(|| format!("the manual has no tool called {tool}"))?;
    let p = prepare(t, args, resolve)?;
    check_tool_url(&p.url, manual.source_local)?;
    let r = up.request(&p.method, &p.url, &p.headers, &p.body)?;
    let mut text = String::from_utf8_lossy(&r.body).into_owned();
    let cut = text.len() > MAX_REPLY;
    if cut {
        let mut n = MAX_REPLY;
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        text.truncate(n);
    }
    if (300..400).contains(&r.status) {
        return Err(format!("the tool's address answered with a redirect ({}); redirects are not followed", r.status));
    }
    if !(200..300).contains(&r.status) {
        return Err(format!("the tool answered {}: {}", r.status, text.chars().take(300).collect::<String>()));
    }
    if cut {
        text.push_str("\n[reply cut: it was longer than 64 KB]");
    }
    Ok(text)
}

// ---------------------------------------------------------------------------------------------- manuals added here

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredManual {
    /// The short name the tools are listed under (`<name>.<tool>`).
    pub name: String,
    /// Where the manual is read from (https, or loopback for this machine).
    pub url: String,
}

/// UTCP manuals added from the UTCP tile.
pub struct UtcpStore {
    path: std::path::PathBuf,
    lock: std::sync::Mutex<()>,
}

impl UtcpStore {
    pub fn new(path: &std::path::Path) -> UtcpStore {
        UtcpStore { path: path.into(), lock: std::sync::Mutex::new(()) }
    }

    fn load(&self) -> Vec<StoredManual> {
        std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, v: &[StoredManual]) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Vec<StoredManual> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load()
    }

    pub fn add(&self, m: StoredManual) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if !crate::mcp::valid_server_name(&m.name) {
            return Err("the name: letters, digits, - and _ (up to 32)".into());
        }
        check_tool_url(&m.url, true).map_err(|e| format!("the manual address: {e}"))?;
        let mut v = self.load();
        if v.iter().any(|x| x.name == m.name) {
            return Err(format!("a manual called {} is already there", m.name));
        }
        if v.len() >= 50 {
            return Err("50 manuals are the most; remove one first".into());
        }
        v.push(m);
        self.save(&v)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.load();
        let n = v.len();
        v.retain(|x| x.name != name);
        if v.len() == n {
            return Err(format!("no manual called {name}"));
        }
        self.save(&v)
    }
}

/// The tools of a manual in the shape the assistant lists tools in (the name is `<manual>.<tool>`).
pub fn as_tools(manual_name: &str, m: &Manual) -> Vec<crate::mcp::Tool> {
    m.tools.iter().map(|t| crate::mcp::Tool { name: format!("{manual_name}.{}", t.name), description: t.description.clone(), schema: t.inputs.clone() }).collect()
}
