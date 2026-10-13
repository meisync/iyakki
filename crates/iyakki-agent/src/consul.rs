//! A small client for HashiCorp Consul's HTTP API: what is registered (services and their health), which machines are
//! in the cluster (nodes, leader, peers) and the key/value store. It speaks plain HTTP(S) through [`Upstream`], sends
//! the ACL token only in the `X-Consul-Token` header, and never builds a request from unchecked text: addresses and
//! keys are validated and encoded first.

use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde_json::Value;

pub const MAX_KEY: usize = 512;
pub const MAX_VALUE: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Passing,
    Warning,
    Critical,
}

impl Health {
    fn parse(s: &str) -> Health {
        match s {
            "passing" => Health::Passing,
            "warning" => Health::Warning,
            _ => Health::Critical,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Health::Passing => "passing",
            Health::Warning => "warning",
            Health::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Health,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub node: String,
    pub address: String,
    pub port: u16,
    pub status: Health,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterInfo {
    pub leader: String,
    pub peers: Vec<String>,
    pub version: String,
    pub datacenter: String,
}

pub struct Consul {
    base: String,
    token: Option<String>,
    up: Arc<dyn Upstream>,
}

/// `http://host:8500` or `https://host`: a scheme, a host and perhaps a port, nothing else.
pub fn valid_address(a: &str) -> Result<String, String> {
    let a = a.trim().trim_end_matches('/');
    let (scheme, rest) = a.split_once("://").ok_or("the address starts with http:// or https://")?;
    if scheme != "http" && scheme != "https" {
        return Err("the address starts with http:// or https://".into());
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, Some(p)),
        _ => (rest, None),
    };
    let host_ok = !host.is_empty() && host.len() < 254 && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '[' | ']' | ':'));
    if !host_ok || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err("the address is just a host and an optional port, like http://consul.local:8500".into());
    }
    if let Some(p) = port {
        if p.parse::<u16>().is_err() {
            return Err("the port is not a number".into());
        }
    }
    Ok(format!("{scheme}://{rest}"))
}

/// The host part of a validated address (for the account's host policy).
pub fn host_of(address: &str) -> String {
    let rest = address.split_once("://").map_or(address, |x| x.1);
    let host = rest.rsplit_once(':').filter(|(h, p)| p.parse::<u16>().is_ok() && !h.is_empty()).map_or(rest, |x| x.0);
    host.trim_matches(['[', ']']).to_ascii_lowercase()
}

fn valid_key(k: &str) -> Result<(), String> {
    if k.len() > MAX_KEY {
        return Err(format!("keys are limited to {MAX_KEY} characters"));
    }
    if k.starts_with('/') || k.contains("//") || k.split('/').any(|s| s == "..") || k.chars().any(|c| c.is_control()) {
        return Err("a key has no leading slash, no `..` and no control characters".into());
    }
    Ok(())
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

impl Consul {
    pub fn new(address: &str, token: Option<String>, up: Arc<dyn Upstream>) -> Result<Consul, String> {
        Ok(Consul { base: valid_address(address)?, token: token.filter(|t| !t.is_empty()), up })
    }

    fn call(&self, method: &str, path: &str, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
        let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
        if let Some(t) = &self.token {
            headers.push(("X-Consul-Token".into(), t.clone()));
        }
        let r = self.up.request(method, &format!("{}{}", self.base, path), &headers, body).map_err(|e| format!("could not reach Consul: {e}"))?;
        match r.status {
            403 => Err("Consul refused: the token has no permission for this".into()),
            401 => Err("Consul needs an ACL token".into()),
            s if s >= 500 => Err(format!("Consul answered {s}: {}", String::from_utf8_lossy(&r.body).chars().take(120).collect::<String>())),
            s => Ok((s, r.body)),
        }
    }

    fn json(&self, path: &str) -> Result<Value, String> {
        let (status, body) = self.call("GET", path, &[])?;
        if status >= 400 {
            return Err(format!("Consul answered {status}"));
        }
        serde_json::from_slice(&body).map_err(|_| "Consul sent something that is not JSON".to_string())
    }

    pub fn cluster(&self) -> Result<ClusterInfo, String> {
        let agent = self.json("/v1/agent/self")?;
        let leader: String = self.json("/v1/status/leader").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let peers: Vec<String> = self.json("/v1/status/peers").ok().and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())).unwrap_or_default();
        Ok(ClusterInfo { leader, peers, version: text(&agent["Config"], "Version"), datacenter: text(&agent["Config"], "Datacenter") })
    }

    pub fn services(&self) -> Result<Vec<ServiceInfo>, String> {
        let v = self.json("/v1/catalog/services")?;
        let mut out: Vec<ServiceInfo> = v.as_object().ok_or("unexpected answer")?.iter().map(|(n, t)| ServiceInfo { name: n.clone(), tags: t.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() }).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn instances(&self, service: &str) -> Result<Vec<Instance>, String> {
        valid_key(service)?;
        let v = self.json(&format!("/v1/health/service/{}", enc(service)))?;
        let mut out = Vec::new();
        for item in v.as_array().ok_or("unexpected answer")? {
            let checks: Vec<Check> = item["Checks"].as_array().map(|a| a.iter().map(|c| Check { name: text(c, "Name"), status: Health::parse(c["Status"].as_str().unwrap_or("")), output: text(c, "Output").chars().take(300).collect() }).collect()).unwrap_or_default();
            let status = checks.iter().map(|c| c.status).max().unwrap_or(Health::Passing);
            let svc = &item["Service"];
            let address = if text(svc, "Address").is_empty() { text(&item["Node"], "Address") } else { text(svc, "Address") };
            out.push(Instance { node: text(&item["Node"], "Node"), address, port: svc["Port"].as_u64().unwrap_or(0) as u16, status, checks });
        }
        Ok(out)
    }

    pub fn nodes(&self) -> Result<Vec<NodeInfo>, String> {
        let v = self.json("/v1/catalog/nodes")?;
        let mut out: Vec<NodeInfo> = v.as_array().ok_or("unexpected answer")?.iter().map(|n| NodeInfo { name: text(n, "Node"), address: text(n, "Address") }).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Keys directly under `prefix` (a trailing `/` marks a folder). A prefix with nothing under it is empty, not an error.
    pub fn kv_list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let p = prefix.trim_start_matches('/');
        valid_key(p.trim_end_matches('/'))?;
        let (status, body) = self.call("GET", &format!("/v1/kv/{}?keys&separator=%2F", enc(p)), &[])?;
        if status == 404 {
            return Ok(Vec::new());
        }
        if status >= 400 {
            return Err(format!("Consul answered {status}"));
        }
        let v: Value = serde_json::from_slice(&body).map_err(|_| "Consul sent something that is not JSON".to_string())?;
        let mut keys: Vec<String> = v.as_array().ok_or("unexpected answer")?.iter().filter_map(|k| k.as_str().map(str::to_string)).filter(|k| k != p).collect();
        keys.sort();
        keys.truncate(2000);
        Ok(keys)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>, String> {
        valid_key(key)?;
        let (status, body) = self.call("GET", &format!("/v1/kv/{}?raw", enc(key)), &[])?;
        match status {
            404 => Ok(None),
            s if s >= 400 => Err(format!("Consul answered {s}")),
            _ if body.len() > MAX_VALUE => Err("the value is larger than this app shows".into()),
            _ => Ok(Some(String::from_utf8_lossy(&body).into_owned())),
        }
    }

    pub fn kv_put(&self, key: &str, value: &str) -> Result<(), String> {
        valid_key(key)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("give the key a name".into());
        }
        if value.len() > MAX_VALUE {
            return Err("the value is too large".into());
        }
        let (status, body) = self.call("PUT", &format!("/v1/kv/{}", enc(key)), value.as_bytes())?;
        if status >= 400 || String::from_utf8_lossy(&body).trim() == "false" {
            return Err(format!("Consul did not store the value ({status})"));
        }
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<(), String> {
        valid_key(key)?;
        if key.is_empty() || key.ends_with('/') {
            return Err("pick a single key to delete".into());
        }
        let (status, _) = self.call("DELETE", &format!("/v1/kv/{}", enc(key)), &[])?;
        if status >= 400 {
            return Err(format!("Consul answered {status}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;
    use std::sync::Mutex;

    /// A fake Consul: answers by method and path, remembers what it was asked and with which token.
    #[derive(Default)]
    struct Fake {
        seen: Mutex<Vec<(String, String, Option<String>, Vec<u8>)>>,
        kv: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }

    impl Upstream for Fake {
        fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            let path = url.strip_prefix("http://consul.local:8500").ok_or_else(|| format!("wrong host {url}"))?.to_string();
            let token = headers.iter().find(|(k, _)| k == "X-Consul-Token").map(|(_, v)| v.clone());
            self.seen.lock().unwrap().push((method.into(), path.clone(), token.clone(), body.to_vec()));
            let ok = |s: &str| Ok(UpstreamResponse { status: 200, body: s.as_bytes().to_vec() });
            if token.as_deref() != Some("s3cret") && !path.starts_with("/v1/status") {
                return Ok(UpstreamResponse { status: 403, body: b"ACL not found".to_vec() });
            }
            match (method, path.split('?').next().unwrap_or("")) {
                ("GET", "/v1/agent/self") => ok(r#"{"Config":{"Version":"1.19.0","Datacenter":"dc1"}}"#),
                ("GET", "/v1/status/leader") => ok("\"10.0.0.1:8300\""),
                ("GET", "/v1/status/peers") => ok(r#"["10.0.0.1:8300","10.0.0.2:8300"]"#),
                ("GET", "/v1/catalog/services") => ok(r#"{"web":["v1","edge"],"consul":[],"api":["v2"]}"#),
                ("GET", "/v1/health/service/web") => ok(r#"[{"Node":{"Node":"n1","Address":"10.0.0.5"},"Service":{"Address":"","Port":8080},"Checks":[{"Name":"serf","Status":"passing","Output":"ok"},{"Name":"http","Status":"warning","Output":"slow"}]},{"Node":{"Node":"n2","Address":"10.0.0.6"},"Service":{"Address":"10.1.1.1","Port":8080},"Checks":[]}]"#),
                ("GET", "/v1/catalog/nodes") => ok(r#"[{"Node":"n2","Address":"10.0.0.6"},{"Node":"n1","Address":"10.0.0.5"}]"#),
                ("GET", "/v1/kv/") => ok(r#"["app/","motd"]"#),
                ("GET", p) if p.starts_with("/v1/kv/app/") && path.contains("keys") => ok(r#"["app/","app/color","app/limits/"]"#),
                ("GET", p) if p.starts_with("/v1/kv/") => match self.kv.lock().unwrap().get(&p["/v1/kv/".len()..].replace("%20", " ")) {
                    Some(v) => Ok(UpstreamResponse { status: 200, body: v.clone() }),
                    None => Ok(UpstreamResponse { status: 404, body: vec![] }),
                },
                ("PUT", p) if p.starts_with("/v1/kv/") => {
                    self.kv.lock().unwrap().insert(p["/v1/kv/".len()..].replace("%20", " "), body.to_vec());
                    ok("true")
                }
                ("DELETE", p) if p.starts_with("/v1/kv/") => {
                    self.kv.lock().unwrap().remove(&p["/v1/kv/".len()..].replace("%20", " "));
                    ok("true")
                }
                _ => Ok(UpstreamResponse { status: 404, body: vec![] }),
            }
        }
    }

    fn client() -> (Consul, Arc<Fake>) {
        let f = Arc::new(Fake::default());
        (Consul::new("http://consul.local:8500/", Some("s3cret".into()), f.clone()).unwrap(), f)
    }

    #[test]
    fn addresses_are_checked_and_the_host_is_found() {
        assert_eq!(valid_address(" http://consul.local:8500/ ").unwrap(), "http://consul.local:8500");
        assert_eq!(valid_address("https://10.0.0.1").unwrap(), "https://10.0.0.1");
        for bad in ["consul.local", "ftp://x", "http://", "http://a/b", "http://a?x=1", "http://u:p@h", "http://h:99999", "http://h :80", "http://h:ab"] {
            assert!(valid_address(bad).is_err(), "{bad}");
        }
        assert_eq!(host_of("http://Consul.Local:8500"), "consul.local");
        assert_eq!(host_of("https://10.0.0.1"), "10.0.0.1");
    }

    #[test]
    fn the_cluster_services_health_and_nodes_are_read() {
        let (c, f) = client();
        let info = c.cluster().unwrap();
        assert_eq!((info.leader.as_str(), info.peers.len(), info.version.as_str(), info.datacenter.as_str()), ("10.0.0.1:8300", 2, "1.19.0", "dc1"));
        let svcs = c.services().unwrap();
        assert_eq!(svcs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["api", "consul", "web"]);
        assert_eq!(svcs[2].tags, vec!["v1".to_string(), "edge".to_string()]);
        let inst = c.instances("web").unwrap();
        assert_eq!(inst.len(), 2);
        assert_eq!((inst[0].node.as_str(), inst[0].address.as_str(), inst[0].port, inst[0].status), ("n1", "10.0.0.5", 8080, Health::Warning), "the worst check decides; the node's address stands in");
        assert_eq!((inst[1].address.as_str(), inst[1].status), ("10.1.1.1", Health::Passing));
        assert_eq!(inst[0].checks[1].output, "slow");
        assert_eq!(c.nodes().unwrap().iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["n1", "n2"]);
        // the token travels only in its header, never in an address
        assert!(f.seen.lock().unwrap().iter().all(|(_, path, t, _)| !path.contains("s3cret") && t.as_deref() == Some("s3cret")));
    }

    #[test]
    fn a_missing_or_wrong_token_is_explained() {
        let f = Arc::new(Fake::default());
        let c = Consul::new("http://consul.local:8500", None, f.clone()).unwrap();
        assert!(c.services().unwrap_err().contains("no permission"), "{:?}", c.services());
        assert!(c.cluster().is_err());
        let c = Consul::new("http://consul.local:8500", Some("wrong".into()), f).unwrap();
        assert!(c.kv_put("a", "b").is_err());
    }

    #[test]
    fn keys_are_listed_read_written_and_deleted() {
        let (c, f) = client();
        assert_eq!(c.kv_list("").unwrap(), vec!["app/", "motd"]);
        assert_eq!(c.kv_list("app/").unwrap(), vec!["app/color", "app/limits/"], "the folder itself is not listed");
        assert_eq!(c.kv_get("app/color").unwrap(), None);
        c.kv_put("app/color", "blue").unwrap();
        c.kv_put("my key", "with space").unwrap();
        assert_eq!(c.kv_get("app/color").unwrap().as_deref(), Some("blue"));
        assert_eq!(c.kv_get("my key").unwrap().as_deref(), Some("with space"));
        c.kv_delete("app/color").unwrap();
        assert_eq!(c.kv_get("app/color").unwrap(), None);
        let seen = f.seen.lock().unwrap();
        assert!(seen.iter().any(|(m, p, _, b)| m == "PUT" && p == "/v1/kv/my%20key" && b == b"with space"), "keys are percent-encoded: {seen:?}");
    }

    #[test]
    fn unsafe_keys_and_values_never_leave_the_machine() {
        let (c, f) = client();
        for bad in ["/abs", "a//b", "a/../b", "a\nb", &"k".repeat(MAX_KEY + 1)] {
            assert!(c.kv_get(bad).is_err(), "{bad:?}");
            assert!(c.kv_put(bad, "x").is_err());
        }
        assert!(c.kv_put("", "x").is_err() && c.kv_put("folder/", "x").is_err() && c.kv_delete("folder/").is_err());
        assert!(c.kv_put("big", &"x".repeat(MAX_VALUE + 1)).is_err());
        // a key cannot smuggle a query string into the request
        let _ = c.kv_get("a?token=x#y");
        assert!(f.seen.lock().unwrap().iter().all(|(_, p, _, _)| !p.contains("token=x")), "{:?}", f.seen.lock().unwrap());
        assert!(c.instances("../agent").is_err());
    }
}
