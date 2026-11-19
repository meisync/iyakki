//! Safety rules and warnings (plan section "Safety rules and warnings"): a pure layer that looks at an item, its effective values,
//! the rest of the tenant's configuration and a few facts (when a key was last set, the last connection test) and returns warnings
//! with stable codes. `block` rules refuse to enable or apply an item, `warn` rules must be acknowledged by the admin on enabling,
//! `info` rules only explain. No text ever contains a setting value that could be a secret: texts name providers, domains, fields and numbers.

use super::catalog;
use super::enforce::{egress_allows_cfg, integration_policy};
use super::resolve::Effective;
use super::schema::{KindSpec, SecretRef, Ty};
use super::store::{Config, Meta};
use super::Item;
use crate::cloudroles::{NsRole, Roles};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    /// `info`, `warn` or `block`.
    pub severity: &'static str,
    pub field: String,
    pub text: String,
}

impl Warning {
    pub fn json(&self) -> Value {
        json!({"code": self.code, "severity": self.severity, "field": self.field, "text": self.text, "doc": format!("TENANT-CONFIG-PLAN.md#safety-{}", self.code.replace('.', "-"))})
    }
}

pub struct Ctx<'a> {
    pub spec: &'static KindSpec,
    pub item: &'a Item,
    pub eff: &'a Effective,
    pub cfg: &'a Config,
    pub roles: Option<&'a Roles>,
    pub meta: &'a BTreeMap<String, Meta>,
    pub now: u64,
}

const PAID: &[&str] = &["anthropic", "openai", "gemini", "openrouter", "azure_openai", "mistral"];
const KEY_PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "ghs_", "github_pat_", "AIza", "xoxb-", "xoxp-", "AKIA", "glpat-"];

pub fn highest(w: &[Warning]) -> &'static str {
    if w.iter().any(|x| x.severity == "block") {
        "block"
    } else if w.iter().any(|x| x.severity == "warn") {
        "warn"
    } else if w.is_empty() {
        "none"
    } else {
        "info"
    }
}

fn looks_like_key(s: &str) -> bool {
    if s.starts_with("http://") || s.starts_with("https://") {
        let rest = s.split_once("://").map_or("", |x| x.1);
        let auth = rest.split('/').next().unwrap_or("");
        let q = rest.split_once('?').map_or("", |x| x.1).to_ascii_lowercase();
        return auth.contains('@') || ["key=", "token=", "secret=", "password=", "apikey=", "api_key=", "access_token="].iter().any(|k| q.contains(k) || q.contains(&format!("&{k}")));
    }
    s.len() >= 16 && KEY_PREFIXES.iter().any(|p| s.starts_with(p))
}

fn leaves<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
        Value::Object(m) => m.values().for_each(|x| leaves(x, out)),
        _ => {}
    }
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn num(e: &Effective, k: &str) -> Option<u64> {
    e.values.get(k).and_then(|v| v.as_u64())
}

fn s<'a>(e: &'a Effective, k: &str) -> Option<&'a str> {
    e.values.get(k).and_then(|v| v.as_str())
}

/// Host of an `http(s)://` URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split(['/', '?', '#']).next()?;
    Some(auth.rsplit_once(':').map_or(auth, |x| x.0).to_string())
}

/// `pat` (an exact host, a `*.domain` pattern, an IPv4 address or an IPv4 CIDR) covers `host`.
pub fn range_covers(pat: &str, host: &str) -> bool {
    if let Some((net, len)) = pat.split_once('/') {
        let (Some(n), Some(h), Ok(l)) = (ip4(net), ip4(host), len.parse::<u32>()) else { return false };
        return l <= 32 && (l == 0 || (n >> (32 - l)) == (h >> (32 - l)));
    }
    super::resolve::covers(pat, host)
}

fn ip4(s: &str) -> Option<u32> {
    let p: Vec<u32> = s.split('.').map(|x| x.parse::<u32>().ok().filter(|v| *v < 256)).collect::<Option<_>>()?;
    (p.len() == 4).then(|| (p[0] << 24) | (p[1] << 16) | (p[2] << 8) | p[3])
}

/// Why a local-host address must be refused (SSRF): metadata and link-local addresses always, the cluster API servers the tenant lists,
/// what the tenant denies, loopback unless the tenant allows the same host, and anything not on the tenant's internal-hosts list.
pub fn ssrf_problem(host: &str, cfg: &Config, internal: &[String], deny: &[String], same_host: bool) -> Option<String> {
    let h = host.to_ascii_lowercase();
    let link_local = ip4(&h).is_some_and(|a| a >> 16 == 0xA9FE) || h == "metadata.google.internal" || h == "metadata" || h == "0.0.0.0";
    if link_local {
        return Some("metadata and link-local addresses are never allowed".into());
    }
    if cfg.all_items().iter().any(|i| i.kind == "k3s.cluster" && i.values.get("api_server").and_then(|v| v.as_str()).and_then(host_of).is_some_and(|a| a == h)) {
        return Some("this is a cluster API server address".into());
    }
    if deny.iter().any(|d| range_covers(d, &h)) {
        return Some("the tenant denies this address range".into());
    }
    if h == "localhost" || ip4(&h).is_some_and(|a| a >> 24 == 127) {
        return if same_host { None } else { Some("loopback is only for development when the tenant allows the same host".into()) };
    }
    if internal.iter().any(|r| range_covers(r, &h)) {
        None
    } else {
        Some("the address is not on the tenant's list of allowed internal hosts".into())
    }
}

/// All rules for one item. Disabled items get only the two rules about where a secret may be.
pub fn evaluate(c: &Ctx) -> Vec<Warning> {
    let mut out: Vec<Warning> = Vec::new();
    let mut add = |code: &'static str, sev: &'static str, field: &str, text: String| out.push(Warning { code, severity: sev, field: field.into(), text });
    let (it, e) = (c.item, c.eff);
    let pol = integration_policy(c.cfg);
    let kind = it.kind.as_str();
    let fields = c.spec.all_fields();
    // ---- always: where secrets may be
    for (k, v) in &it.values {
        let secret = fields.iter().any(|f| f.name == k && matches!(f.ty, Ty::SecretRef(_)));
        let enumerated = fields.iter().any(|f| f.name == k && matches!(f.ty, Ty::Enum(_)));
        if secret {
            if let Some(r) = v.as_str().and_then(SecretRef::parse) {
                if r.ns == "team" && r.name.split('/').next() != it.namespace.as_deref() {
                    add("secret.foreign", "block", k, "this reference points into another team's part of the vault".into());
                }
            }
        } else if !enumerated {
            let mut ls = Vec::new();
            leaves(v, &mut ls);
            if ls.iter().any(|x| looks_like_key(x)) {
                add("secret.inline", "block", k, "this looks like a key or a URL with a key in it; keys go in the secret box only".into());
            }
        }
    }
    if !it.enabled {
        return out;
    }
    // ---- facts
    let team = it.namespace.as_deref();
    let uses_key = fields.iter().any(|f| matches!(f.ty, Ty::SecretRef(_)) && e.values.contains_key(f.name));
    let shared_agent = kind.starts_with("agent.") && s(e, "mode") == Some("shared");
    let provider_id = if kind == "integration.apikey" { s(e, "provider").unwrap_or("other").to_string() } else if kind == "agent.claude" { "anthropic".into() } else { String::new() };
    let paid = (kind == "integration.apikey" && PAID.contains(&provider_id.as_str())) || (shared_agent && matches!(kind, "agent.claude" | "agent.copilot"));
    let local = kind == "models.local";
    let key_kind = matches!(kind, "integration.apikey" | "models.provider" | "models.local") || shared_agent;
    let provider_name = match kind {
        "agent.claude" => "Anthropic".to_string(),
        "agent.copilot" => "GitHub (Copilot)".to_string(),
        "integration.apikey" => catalog::provider(&provider_id).map_or("the provider".into(), |p| p.name.to_string()),
        "models.provider" => s(e, "base_url").and_then(host_of).unwrap_or_else(|| "the provider".into()),
        _ => String::new(),
    };
    // ---- BLOCK
    if (paid || shared_agent) && (num(e, "monthly_requests_team").is_none() || num(e, "daily_requests_per_person").is_none()) {
        let (code, text) = if shared_agent { ("agent.shared.limits", "a shared agent needs a daily limit per person and a monthly cap for the team") } else { ("spend.cap.missing", "a paid provider needs both a monthly cap for the team and a daily cap per person") };
        add(code, "block", "monthly_requests_team", text.into());
    }
    if key_kind && !local && uses_key && it.namespace.is_some() && e.values.get("terms_ack") != Some(&Value::Bool(true)) {
        add("terms.ack.missing", "block", "terms_ack", "the team admin must confirm that billing goes to the key's owner and that use is audited per person".into());
    }
    if let Some(url) = s(e, "base_url") {
        let private = host_of(url).is_some_and(|h| catalog::is_private_host(&h));
        if url.starts_with("http://") && !private {
            add("endpoint.http", "block", "base_url", "plain http to a host outside the private ranges is not allowed".into());
        } else if kind == "integration.apikey" && private && !pol.allow_private {
            add("endpoint.private", "block", "base_url", "a private or loopback endpoint needs the tenant policy to allow private endpoints".into());
        }
    }
    let domains: Vec<String> = match kind {
        "integration.apikey" => catalog::needed_domains(&provider_id, &e.values),
        "agent.claude" if shared_agent => vec!["api.anthropic.com".into()],
        "agent.copilot" if shared_agent => vec!["api.githubcopilot.com".into()],
        "notify.channel" => notify_hosts(e),
        "models.provider" => s(e, "base_url").and_then(host_of).filter(|h| !catalog::is_private_host(h.as_str())).into_iter().collect(),
        _ => vec![],
    };
    for d in domains {
        if egress_allows_cfg(c.cfg, team, &d) == Some(false) {
            add("egress.domain.blocked", "block", "egress", format!("disabled by policy: the tenant's egress policy does not allow {d}"));
        }
    }
    if kind == "k3s.cluster" && it.namespace.is_some() && (it.name == "prod") && pol.require_four_eyes_prod && e.values.get("four_eyes") == Some(&Value::Bool(false)) {
        add("k3s.prod.four_eyes", "block", "four_eyes", "the tenant requires four-eyes approval for prod".into());
    }
    if kind == "egress.policy" {
        let doms = strs(e.values.get("allowed_domains"));
        if !pol.allow_wildcard && doms.iter().any(|d| d == "*" || (d.starts_with("*.") && !d[2..].contains('.'))) {
            add("egress.wildcard", "block", "allowed_domains", "'*' or a whole top-level domain opens the internet; only the operator can allow that".into());
        }
        if doms.len() > pol.max_domains {
            add("egress.many", "warn", "allowed_domains", format!("the allow-list has {} domains (more than {}); long lists are hard to review", doms.len(), pol.max_domains));
        }
    }
    if !pol.audit_enabled && uses_key {
        add("audit.disabled", "block", "audit", "audit cannot be switched off for an item that handles a secret".into());
    }
    if local {
        let missing = ["max_concurrent_per_person", "max_concurrent_team", "queue_timeout_secs"].iter().any(|k| num(e, k).is_none());
        if missing {
            add("local.limits.missing", "block", "max_concurrent_per_person", "a shared local model host needs a concurrency limit per person, one for the team and a queue timeout".into());
        }
        if let Some(h) = s(e, "host") {
            let internal = strs(c.cfg.entries().iter().find(|x| x.item.kind == "integration.policy").and_then(|x| x.item.values.get("internal_hosts")));
            let deny = strs(c.cfg.entries().iter().find(|x| x.item.kind == "integration.policy").and_then(|x| x.item.values.get("deny_hosts")));
            let same = c.cfg.entries().iter().find(|x| x.item.kind == "integration.policy").is_some_and(|x| x.item.values.get("allow_same_host") == Some(&Value::Bool(true)));
            if let Some(why) = ssrf_problem(h, c.cfg, &internal, &deny, same) {
                add("local.ssrf", "block", "host", why);
            }
        }
    }
    // ---- sandbox.policy (docs/development/CLOUD-SANDBOX-PLAN.md)
    if kind == "sandbox.policy" {
        for (field, what) in [("allow_privileged", "privileged containers"), ("allow_host_network", "the host network"), ("allow_host_path", "host folders"), ("allow_root", "running as root"), ("allow_cap_add", "added Linux capabilities")] {
            if e.values.get(field) == Some(&Value::Bool(true)) {
                add("sandbox.privileged", "block", field, format!("{what} would break the isolation between runs and tenants; no tenant setting allows it"));
            }
        }
        if strs(e.values.get("network_modes")).iter().any(|m| m == "direct") {
            add("sandbox.net.direct", "block", "network_modes", "a run with direct internet access bypasses the allow-list and the key injection; use proxy".into());
        }
        if strs(e.values.get("network_modes")).iter().any(|m| m != "none" && m != "proxy" && m != "direct") {
            add("sandbox.net.unknown", "block", "network_modes", "unknown network mode (none or proxy)".into());
        }
        let tiers = strs(e.values.get("tiers"));
        if tiers.iter().any(|t| crate::sandbox::spec::Tier::parse(t).is_none()) {
            add("sandbox.tier.unknown", "block", "tiers", "unknown tier (wasm, namespace, job, browser)".into());
        }
        if tiers.iter().any(|t| t == "namespace") && e.values.get("userns_ack") != Some(&Value::Bool(true)) {
            add("sandbox.userns.ack", "block", "userns_ack", "the namespace tier needs pods that allow user namespaces; the admin must confirm that first".into());
        }
        if tiers.iter().any(|t| t == "browser") && e.values.get("browser") != Some(&Value::Bool(true)) {
            add("sandbox.browser.off", "block", "browser", "the browser tier is listed but the browser sandbox is not switched on".into());
        }
        if s(e, "runtime_class").is_none() && tiers.iter().any(|t| matches!(t.as_str(), "job" | "browser")) {
            add("sandbox.runtime_class.missing", "warn", "runtime_class", "no runtime class (gVisor, Kata): runs share the node's kernel; on shared nodes set one".into());
        }
        if strs(e.values.get("allowed_registries")).iter().any(|r| r == "*") {
            add("sandbox.registry.any", "warn", "allowed_registries", "any registry: a run may start any image on the internet".into());
        }
        if (tiers.iter().any(|t| t == "job" || t == "browser")) && strs(e.values.get("allowed_registries")).is_empty() {
            add("sandbox.registry.none", "warn", "allowed_registries", "no image registry is allowed: no command can start until one is listed".into());
        }
        if e.values.get("browser") == Some(&Value::Bool(true)) && strs(e.values.get("browser_domains")).is_empty() {
            add("sandbox.browser.domains", "warn", "browser_domains", "no domain is allowed for the browser sandbox: sessions can open nothing".into());
        }
        if s(e, "browser_profile") == Some("named") {
            add("sandbox.browser.profile", "warn", "browser_profile", "a named profile keeps cookies between sessions; a leak in one session reaches the next".into());
        }
    }
    if kind == "notify.channel" {
        if e.values.get("include_details") == Some(&Value::Bool(true)) && e.values.get("details_ack") != Some(&Value::Bool(true)) {
            add("notify.details.ack.missing", "block", "details_ack", "titles and texts can only leave the tenant after the admin confirms that they may".into());
        }
        if s(e, "type") == Some("email") && !matches!(s(e, "tls").unwrap_or("starttls"), "starttls" | "implicit") {
            add("notify.plaintext", "block", "tls", "plain SMTP is not allowed".into());
        }
    }
    // ---- WARN
    if kind == "notify.channel" {
        let host = notify_hosts(e).into_iter().next().unwrap_or_default();
        add("notify.data.leaves", "warn", "type", format!("data leaves your tenant: every message of this channel is sent to {host}, outside the tenant, and cannot be taken back"));
        if e.values.get("include_details") == Some(&Value::Bool(true)) {
            add("notify.details.on", "warn", "include_details", "titles and texts of notifications will be sent out; they can hold names and business data".into());
        }
        if s(e, "type") == Some("email") && strs(e.values.get("addresses")).is_empty() {
            add("notify.addresses.missing", "warn", "addresses", "no person has an address: this channel sends nothing".into());
        }
        if num(e, "max_per_hour").is_none_or(|v| v > 600) {
            add("notify.rate.high", "warn", "max_per_hour", "no hourly limit below 600: a flow that fires all night could send a flood".into());
        }
        if egress_allows_cfg(c.cfg, team, &host).is_none() {
            add("notify.egress.none", "warn", "egress", "the tenant has no egress policy, so nothing is sent until one allows this host".into());
        }
    }
    if key_kind && !local && !provider_name.is_empty() {
        add("data.leaves", "warn", "provider", format!("prompts, files and tool results sent to this agent are processed by {provider_name}, outside your tenant"));
    }
    if (key_kind || local) && !kind.starts_with("agent.") || shared_agent {
        let models = strs(e.values.get("allowed_models"));
        if models.is_empty() || models.iter().any(|m| m == "*") {
            add("models.unrestricted", "warn", "allowed_models", "no model allow-list: any model, including expensive ones, may be used".into());
        }
    }
    if num(e, "monthly_requests_team").is_some_and(|v| v > pol.warn_monthly) {
        add("cap.high", "warn", "monthly_requests_team", format!("the team cap is above {} requests per 30 days", pol.warn_monthly));
    }
    if num(e, "daily_requests_per_person").is_some_and(|v| v > pol.warn_daily) {
        add("cap.high", "warn", "daily_requests_per_person", format!("the per-person daily cap is above {}", pol.warn_daily));
    }
    if uses_key && key_kind {
        if let Some(t) = c.meta.get(&super::store::meta_key(kind, team, &it.name)).and_then(|m| m.last_rotated) {
            let days = c.now.saturating_sub(t) / 86_400;
            if days >= pol.key_strong_days {
                add("key.age.strong", "warn", "api_key", format!("the key has not been rotated for {days} days; rotate it now"));
            } else if days >= pol.key_warn_days {
                add("key.age", "warn", "api_key", format!("the key has not been rotated for {days} days"));
            }
        }
        let mine: Vec<String> = fields.iter().filter(|f| matches!(f.ty, Ty::SecretRef(_))).filter_map(|f| it.values.get(f.name)).filter_map(|v| v.as_str().map(String::from)).collect();
        if c.cfg.all_items().iter().any(|o| o.key() != it.key() && o.values.values().any(|v| v.as_str().is_some_and(|r| mine.iter().any(|m| m == r)))) {
            add("key.shared", "warn", "api_key", "the same key is used by more than one entry; a leak or a rotation affects all of them".into());
        }
    }
    if matches!(kind, "integration.apikey" | "models.local") {
        match c.meta.get(&super::store::meta_key(kind, team, &it.name)).and_then(|m| m.test) {
            None => add("test.never", "warn", "test", "the connection has never been tested".into()),
            Some((_, false, _)) => add("test.failed", "warn", "test", "the last connection test failed".into()),
            _ => {}
        }
    }
    if (key_kind || local) && !paid && !shared_agent && num(e, "daily_requests_per_person").is_none() && num(e, "max_concurrent_per_person").is_none() && !kind.starts_with("agent.") {
        add("limits.person.missing", "warn", "daily_requests_per_person", "no per-person limit: guests and normal users can use it without a cap".into());
    }
    if kind == "k3s.cluster" && it.namespace.is_some() {
        if strs(e.values.get("allowed_registries")).is_empty() {
            add("k3s.registry.missing", "warn", "allowed_registries", "no image registry allow-list: nothing can be deployed until one is set, and a broad one would allow any image".into());
        }
        if num(e, "quota_cpu_m").is_none() && num(e, "quota_mem_mib").is_none() && num(e, "quota_pods").is_none() {
            add("k3s.quota.missing", "warn", "quota_pods", "no namespace quota: one deploy can take the whole cluster".into());
        }
        if let Some(r) = c.roles {
            let admins = r.namespaces.get(it.namespace.as_deref().unwrap_or("")).map_or(0, |m| m.values().filter(|x| **x == NsRole::Admin).count());
            let eyes = e.values.get("four_eyes") == Some(&Value::Bool(true)) || it.name == "prod";
            if eyes && admins == 1 && r.approver_holders(it.namespace.as_deref().unwrap_or(""), &[it.name.as_str(), "k3s.cluster"]).is_empty() {
                add("approver.sole", "warn", "approver_mode", "the team has one admin and no approver role holders: that admin's own deploys go to a tenant admin".into());
            }
        }
        if e.values.get("four_eyes") == Some(&Value::Bool(false)) && it.name != "prod" {
            add("info.nonprod.approval", "info", "four_eyes", "no approval is needed for this environment".into());
        }
    }
    if kind == "git.remote" && s(e, "access") == Some("push") && strs(e.values.get("protected_branches")).is_empty() {
        add("git.push.unprotected", "warn", "protected_branches", "pushing is allowed and no branch is protected".into());
    }
    if local {
        if !e.values.contains_key("token") {
            add("local.noauth", "warn", "token", "this server has no authentication: everyone who can reach that network address can use it; bind it to a private interface or put a token or reverse proxy in front".into());
        }
        if s(e, "scheme").unwrap_or("http") == "http" {
            add("local.http", "warn", "scheme", "plain http inside the network: traffic is not encrypted on the wire".into());
        }
        add("local.model_files", "warn", "allowed_models", "model files and their licences are the team's responsibility".into());
        add("info.local.sleep", "info", "host", "a laptop or desktop host may sleep or disappear; the health state then shows unreachable, which is not an error".into());
    }
    // ---- INFO
    if uses_key && key_kind {
        add("info.key.scope", "info", "api_key", "the key is stored write-only in the team's part of the vault, used only for requests members start in this team, and never shown again".into());
        add("info.usage.audit", "info", "usage", "every use is audited per person; team admins and tenant admins can see the counts".into());
    }
    out
}

/// Warnings of `warn` severity that the request did not acknowledge.
pub fn unacknowledged(w: &[Warning], ack: &[String]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = w.iter().filter(|x| x.severity == "warn" && !ack.iter().any(|a| a == x.code)).map(|x| x.code).collect();
    v.dedup();
    v
}

/// The host a notification channel talks to: its webhook's or its mail server's.
fn notify_hosts(e: &Effective) -> Vec<String> {
    match s(e, "type") {
        Some("email") => s(e, "host").map(|h| vec![h.to_ascii_lowercase()]).unwrap_or_default(),
        _ => s(e, "url").and_then(host_of).into_iter().collect(),
    }
}

pub fn rules_table() -> Value {
    json!(["notify.details.ack.missing", "notify.plaintext", "notify.data.leaves", "notify.details.on", "notify.addresses.missing", "notify.rate.high", "notify.egress.none", "spend.cap.missing", "agent.shared.limits", "terms.ack.missing", "secret.inline", "secret.foreign", "endpoint.http", "endpoint.private", "egress.domain.blocked", "k3s.prod.four_eyes", "egress.wildcard", "audit.disabled", "local.limits.missing", "local.ssrf", "data.leaves", "models.unrestricted", "cap.high", "key.age", "key.age.strong", "key.shared", "test.never", "test.failed", "limits.person.missing", "k3s.registry.missing", "k3s.quota.missing", "git.push.unprotected", "egress.many", "approver.sole", "local.noauth", "local.http", "local.model_files", "info.key.scope", "info.usage.audit", "info.nonprod.approval", "info.local.sleep", "sandbox.privileged", "sandbox.net.direct", "sandbox.net.unknown", "sandbox.tier.unknown", "sandbox.userns.ack", "sandbox.browser.off", "sandbox.runtime_class.missing", "sandbox.registry.any", "sandbox.registry.none", "sandbox.browser.domains", "sandbox.browser.profile"])
}
