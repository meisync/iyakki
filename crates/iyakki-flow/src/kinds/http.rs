//! `http` (any HTTP call) and `remote` (a call to another IYAKKI or a service with credentials from the vault).

use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::*;
use crate::expr::get_path;
use crate::host::{HttpRequest, TlsOpts};
use crate::retry::{http_status_retryable, parse_retry_after};

pub fn register(r: &mut Registry) {
    r.register(Arc::new(HttpKind { remote: false }));
    r.register(Arc::new(HttpKind { remote: true }));
}

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const RESERVED: [&str; 5] = ["status", "ok", "headers", "body", "text"];
pub const MAX_TEXT: usize = 64 * 1024;

struct HttpKind {
    remote: bool,
}

pub fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Does `status` satisfy an `expect_status` setting: a number, `2xx`, or a list of them. Empty means any 2xx.
pub fn status_ok(expect: &Value, status: u16) -> bool {
    let one = |v: &Value| -> bool {
        match v {
            Value::Number(n) => n.as_u64() == Some(status as u64),
            Value::String(t) => {
                let t = t.trim().to_ascii_lowercase();
                if let Some(d) = t.strip_suffix("xx") {
                    d.parse::<u16>().is_ok_and(|d| status / 100 == d)
                } else {
                    t.parse::<u16>().is_ok_and(|n| n == status)
                }
            }
            _ => false,
        }
    };
    match expect {
        Value::Null => (200..300).contains(&status),
        Value::Array(a) if a.is_empty() => (200..300).contains(&status),
        Value::Array(a) => a.iter().any(one),
        Value::String(t) if t.is_empty() => (200..300).contains(&status),
        other => one(other),
    }
}

fn http_schema(remote: bool) -> Vec<Field> {
    let mut v = vec![
        Field::new("method", "Method", FieldKind::Choice(&METHODS), "").default(json!("GET")),
        Field::new("url", "Address", FieldKind::Text, "http:// or https://, may use {{ templates }}").required(),
        Field::new("headers", "Headers", FieldKind::Json, "name: value; a credential only as {{secret.NAME}} (a vault secret)"),
    ];
    if remote {
        v.push(Field::new("auth", "Sign-in", FieldKind::Json, "{\"kind\": \"bearer\" | \"basic\" | \"header\", \"secret\": \"vault name\", \"user\": \"for basic\", \"header\": \"for header\"}"));
    }
    v.push(Field::new("body", "Body", FieldKind::Multiline, "text, or a JSON object (sent as JSON)"));
    v.push(Field::new("expect_status", "Success status", FieldKind::Json, "200, \"2xx\" or [200, 204]; default any 2xx"));
    v.push(Field::new("extract", "Extract", FieldKind::Json, "{\"name\": \"path.in.json.body\"} becomes `nodes.<id>.output.name`"));
    v.push(Field::new("tls", "TLS", FieldKind::Json, "{\"ca_file\": \"/path/ca.pem\", \"insecure\": false}"));
    v.push(Field::new("follow_redirects", "Follow redirects", FieldKind::Bool, "").default(json!(true)));
    v.push(Field::new("idempotency_key", "Send an Idempotency-Key", FieldKind::Bool, "stable across attempts (POST, PUT, PATCH, DELETE)").default(json!(true)));
    v
}

fn headers_of(v: &Value) -> Vec<(String, String)> {
    match v {
        Value::Object(o) => o.iter().map(|(k, x)| (k.clone(), crate::expr::text(x))).collect(),
        _ => vec![],
    }
}

impl NodeKind for HttpKind {
    fn id(&self) -> &'static str {
        if self.remote {
            "remote"
        } else {
            "http"
        }
    }
    fn label(&self) -> &'static str {
        if self.remote {
            "Remote call"
        } else {
            "HTTP"
        }
    }
    fn blurb(&self) -> &'static str {
        if self.remote {
            "Calls another IYAKKI or a service endpoint with a sign-in from the vault."
        } else {
            "Calls a web address and reads the answer."
        }
    }
    fn category(&self) -> &'static str {
        "Call"
    }
    fn schema(&self) -> Vec<Field> {
        http_schema(self.remote)
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["headers"]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let url = s(c, "url");
        if url.is_empty() {
            v.push(Issue::error(&node.id, "give an address"));
        } else if !url.contains("{{") && !(url.starts_with("http://") || url.starts_with("https://")) {
            v.push(Issue::error(&node.id, "the address must start with http:// or https://"));
        }
        let m = s(c, "method");
        if !m.is_empty() && !m.contains("{{") && !METHODS.contains(&m.to_ascii_uppercase().as_str()) {
            v.push(Issue::error(&node.id, format!("`{m}` is not a method ({})", METHODS.join(", "))));
        }
        for (k, val) in headers_of(c.get("headers").unwrap_or(&Value::Null)) {
            if credential_header(&k) && !val.is_empty() && !val.contains("{{secret.") {
                v.push(Issue::error(&node.id, format!("header `{k}` looks like a credential: write it as {{{{secret.NAME}}}} and keep the value in your vault")));
            }
        }
        if let Some(a) = c.get("auth") {
            if self.remote {
                match s(a, "kind") {
                    "bearer" | "header" | "basic" => {
                        if s(a, "secret").is_empty() {
                            v.push(Issue::error(&node.id, "sign-in needs the name of a vault secret"));
                        }
                    }
                    "" => {}
                    other => v.push(Issue::error(&node.id, format!("sign-in kind `{other}` is not bearer, basic or header"))),
                }
            }
        } else if self.remote && !c.get("no_auth").and_then(Value::as_bool).unwrap_or(false) {
            v.push(Issue::warn(&node.id, "a remote call without a sign-in (set `no_auth: true` to silence this)"));
        }
        if let Some(Value::Object(ex)) = c.get("extract") {
            for k in ex.keys() {
                if RESERVED.contains(&k.as_str()) {
                    v.push(Issue::error(&node.id, format!("`{k}` is already an output of this node: choose another name for the extract")));
                }
            }
        }
        if c.get("tls").and_then(|t| t.get("insecure")).and_then(Value::as_bool).unwrap_or(false) {
            v.push(Issue::warn(&node.id, "certificate checks are off for this call (not safe)"));
        }
        v
    }
    fn idempotent(&self, node: &Node) -> bool {
        matches!(s(&node.config, "method").to_ascii_uppercase().as_str(), "" | "GET" | "HEAD" | "OPTIONS")
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let method = if s(cfg, "method").is_empty() { "GET".to_string() } else { s(cfg, "method").to_ascii_uppercase() };
        if !METHODS.contains(&method.as_str()) {
            return Err(NodeError::config(format!("`{method}` is not an HTTP method")));
        }
        let url = s(cfg, "url").trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(NodeError::config("the address must start with http:// or https://"));
        }
        let mut headers = headers_of(cfg.get("headers").unwrap_or(&Value::Null));
        if self.remote {
            if let Some(a) = cfg.get("auth").filter(|a| a.is_object()) {
                let name = s(a, "secret");
                let secret = cx.host.secret(name).map_err(NodeError::config)?;
                cx.redactor.add(&secret);
                match s(a, "kind") {
                    "bearer" => headers.push(("Authorization".into(), format!("Bearer {secret}"))),
                    "basic" => headers.push(("Authorization".into(), format!("Basic {}", b64(format!("{}:{secret}", s(a, "user")).as_bytes())))),
                    "header" => headers.push((if s(a, "header").is_empty() { "X-Api-Key".to_string() } else { s(a, "header").to_string() }, secret.clone())),
                    other => return Err(NodeError::config(format!("sign-in kind `{other}` is not bearer, basic or header"))),
                }
            }
        }
        let body = match cfg.get("body") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(t)) => t.clone().into_bytes(),
            Some(other) => {
                if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                    headers.push(("Content-Type".into(), "application/json".into()));
                }
                other.to_string().into_bytes()
            }
        };
        if !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") && b(cfg, "idempotency_key", true) && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("idempotency-key")) {
            headers.push(("Idempotency-Key".into(), format!("{}-{}", cx.run_id, cx.node_id)));
        }
        let tls = cfg.get("tls").map(|t| TlsOpts { ca_file: s(t, "ca_file").to_string(), insecure: t.get("insecure").and_then(Value::as_bool).unwrap_or(false) }).unwrap_or_default();
        let req = HttpRequest { method: method.clone(), url: url.clone(), headers, body, timeout_ms: cx.timeout_ms, tls, follow_redirects: b(cfg, "follow_redirects", true), run_id: cx.run_id.to_string(), node_id: cx.node_id.to_string() };
        cx.log(format!("{method} {url} (attempt {}, limit {} ms)", cx.attempt, cx.timeout_ms));
        let resp = cx.host.http(&req, cx.cancel)?;
        cx.log(format!("-> {} in {} ms, {} bytes", resp.status, resp.elapsed_ms, resp.body.len()));
        let expect = cfg.get("expect_status").cloned().unwrap_or(Value::Null);
        if !status_ok(&expect, resp.status) {
            let retry_after_ms = resp.header("retry-after").and_then(|v| parse_retry_after(v, cx.clock.now_ms().div_euclid(1000)));
            return Err(NodeError { message: format!("HTTP {} (expected {}): {}", resp.status, if expect.is_null() { "2xx".to_string() } else { expect.to_string() }, preview(&json!(resp.body.trim()), 200)), http_status: Some(resp.status), retry_after_ms, output: preview(&json!(resp.body), 2000), ..Default::default() });
        }
        let t = resp.body.trim_start();
        let parsed: Value = if t.starts_with('{') || t.starts_with('[') { serde_json::from_str(&resp.body).unwrap_or_else(|_| json!(resp.body)) } else { json!(resp.body) };
        let hmap: Map<String, Value> = resp.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), json!(v))).collect();
        let mut out = Map::new();
        out.insert("status".into(), json!(resp.status));
        out.insert("ok".into(), json!(true));
        out.insert("headers".into(), Value::Object(hmap));
        out.insert("body".into(), parsed.clone());
        out.insert("text".into(), json!(resp.body.chars().take(MAX_TEXT).collect::<String>()));
        if let Some(Value::Object(ex)) = cfg.get("extract") {
            let root = json!({"status": resp.status, "headers": out["headers"], "body": parsed});
            for (name, path) in ex {
                let p = path.as_str().unwrap_or("");
                let first = p.split(['.', '[']).next().unwrap_or("");
                let v = if matches!(first, "status" | "headers" | "body") { get_path(&root, p).clone() } else { get_path(&root["body"], p).clone() };
                if RESERVED.contains(&name.as_str()) {
                    return Err(NodeError::config(format!("extract name `{name}` is reserved")));
                }
                out.insert(name.clone(), v);
            }
        }
        let _ = node;
        Ok(NodeOutput::out(Value::Object(out)))
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        if e.cancelled || e.config {
            return ErrClass::Permanent;
        }
        match e.http_status {
            Some(s) => {
                if http_status_retryable(s) {
                    ErrClass::Retryable
                } else {
                    ErrClass::Permanent
                }
            }
            None => default_class(e),
        }
    }
    fn describe(&self, node: &Node) -> String {
        format!("{} {}", if s(&node.config, "method").is_empty() { "GET" } else { s(&node.config, "method") }, preview(&json!(s(&node.config, "url")), 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_status_expectations() {
        assert_eq!(b64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(b64(b"a"), "YQ==");
        assert_eq!(b64(b"ab"), "YWI=");
        assert!(status_ok(&Value::Null, 204) && !status_ok(&Value::Null, 301) && !status_ok(&Value::Null, 404));
        assert!(status_ok(&json!("2xx"), 201) && status_ok(&json!(200), 200) && !status_ok(&json!(200), 201));
        assert!(status_ok(&json!([200, "404"]), 404) && status_ok(&json!(["3xx", 200]), 302));
        assert!(status_ok(&json!([]), 200));
    }
}
