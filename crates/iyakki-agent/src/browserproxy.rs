//! The browser's HTTP proxy setting (F-BROX-22): one text, `host:port`, kept in `<data>/browser-proxy.json`; the shell reads it when it
//! builds the browser's network (`BrowserFetch::with_proxy`: HTTP/2 and HTTP/1.1 both go through a `CONNECT` tunnel, TLS stays end to
//! end). Chat tools `browser_proxy_status` and `browser_proxy_set` (asks; an empty text turns the proxy off).
//!
//! No sign-in is stored (D-BROXY-AUTH, docs/development/ALPHA-7-DECISIONS.md): a proxy that wants a name and a password is not
//! supported by this setting until the owner decides where such a secret lives; a secret is never typed into the chat.

use std::path::Path;

use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;

pub use iyakki_proxy::proxyset::{load, save, text};

pub fn is_proxy_tool(name: &str) -> bool {
    matches!(name, "browser_proxy_status" | "browser_proxy_set")
}

fn offered(p: &Policy) -> bool {
    p.apps.contains(&App::Settings) && p.apps.contains(&App::Browser)
}

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !offered(p) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    let tool = |name: &str, desc: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false };
    vec![
        tool("browser_proxy_status", "The HTTP proxy the browser's pages go through (host:port), or that there is none. Read-only.", obj(json!({}), &[]), Risk::Safe),
        tool("browser_proxy_set", "Set the HTTP proxy the browser's pages go through, as `proxy` = host:port (an empty text turns it off). Pages are fetched through a CONNECT tunnel, so the proxy sees which sites are visited but not what is in them (TLS stays end to end). The change applies the next time the browser starts. A proxy that needs a name and password is not supported here; never type a password into the chat. ALWAYS asks.", obj(json!({"proxy": {"type": "string"}}), &["proxy"]), Risk::Ask),
    ]
}

pub fn describe(name: &str, args: &Value) -> Option<String> {
    match name {
        "browser_proxy_status" => Some("look at the browser's proxy setting".into()),
        "browser_proxy_set" => {
            let t = args.get("proxy").and_then(Value::as_str).unwrap_or("").trim();
            Some(if t.is_empty() { "turn the browser's proxy off".to_string() } else { format!("send the browser's pages through the proxy {}: it will see which sites you visit, not their content", t.chars().take(80).collect::<String>()) })
        }
        _ => None,
    }
}

pub fn run(name: &str, args: &Value, data: &Path) -> Result<String, String> {
    match name {
        "browser_proxy_status" => {
            let t = text(data);
            Ok(if t.is_empty() { "The browser connects to sites directly (no proxy).".to_string() } else { format!("The browser's pages go through the proxy {t}.") })
        }
        "browser_proxy_set" => {
            let t = args.get("proxy").and_then(Value::as_str).ok_or("give `proxy` as host:port (empty to turn it off)")?;
            let kept = save(data, t)?;
            Ok(if kept.is_empty() { "The browser's proxy is off; it applies the next time the browser starts.".to_string() } else { format!("The browser will use the proxy {kept} from the next time it starts.") })
        }
        _ => Err(format!("unknown tool `{name}`")),
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy) -> Result<String, String> {
    if !offered(p) {
        return Err("your account may not change the browser's proxy".into());
    }
    run(name, args, svc.dir())
}

#[cfg(test)]
#[path = "browserproxy_tests.rs"]
mod tests;
