//! `sandbox.policy` of the tenant configuration, read into one struct. Fail closed: no item, a switched-off item or an unreadable
//! configuration is [`SandboxPolicy::off`], which refuses every run.

use super::spec::{Limits, NetMode, Tier};
use crate::tenantconfig::api::Env;
use crate::tenantconfig::resolve::resolve;
use crate::tenantconfig::schema;
use crate::tenantconfig::store::Store;
use serde_json::Value;
use std::collections::BTreeMap;

/// Where the egress proxy is, so a run's NetworkPolicy can let traffic reach it and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyTarget {
    /// Label `app.kubernetes.io/name` of the proxy pods (same namespace as the runs).
    pub app_label: String,
    pub service: String,
    pub port: u16,
}

impl Default for ProxyTarget {
    fn default() -> Self {
        ProxyTarget { app_label: "iyakki-egress".into(), service: "iyakki-egress".into(), port: 3128 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub enabled: bool,
    pub tiers: Vec<Tier>,
    pub browser: bool,
    pub userns_ack: bool,
    pub allowed_registries: Vec<String>,
    pub require_digest: bool,
    pub max: Limits,
    pub network_modes: Vec<NetMode>,
    pub runtime_class: Option<String>,
    /// Least team role (`viewer`, `editor`, `admin`, `tenant_admin`) that may run commands.
    pub cli_role: String,
    pub approval_patterns: Vec<String>,
    pub per_person: u32,
    pub per_team: u32,
    pub queue_timeout_secs: u64,
    pub audit_retention_days: u32,
    pub k3s_environment: String,
    pub browser_domains: Vec<String>,
    pub browser_named_profile: bool,
    pub browser_download_mib: u32,
    pub browser_session_secs: u32,
    pub browser_image: String,
    /// The image of the command-line tools `run_command` offers.
    pub cli_image: String,
    pub proxy: ProxyTarget,
    /// Where the Jobs run (a namespace of the tenant); empty: the manifest builder's default.
    pub namespace: String,
}

impl SandboxPolicy {
    /// Everything refused.
    pub fn off() -> SandboxPolicy {
        SandboxPolicy {
            enabled: false,
            tiers: vec![],
            browser: false,
            userns_ack: false,
            allowed_registries: vec![],
            require_digest: false,
            max: Limits { cpu_m: 0, mem_mib: 0, pids: 0, seconds: 0, workspace_mib: 0, output_kib: 0 },
            network_modes: vec![],
            runtime_class: None,
            cli_role: "tenant_admin".into(),
            approval_patterns: vec![],
            per_person: 0,
            per_team: 0,
            queue_timeout_secs: 0,
            audit_retention_days: 90,
            k3s_environment: "dev".into(),
            browser_domains: vec![],
            browser_named_profile: false,
            browser_download_mib: 0,
            browser_session_secs: 0,
            browser_image: String::new(),
            cli_image: String::new(),
            proxy: ProxyTarget::default(),
            namespace: String::new(),
        }
    }

    /// The policy of an effective item (defaults applied by the resolver). `direct` in `network_modes` is dropped here as well: the
    /// safety rule blocks saving it, and nothing downstream can see it.
    pub fn from_values(enabled: bool, v: &BTreeMap<String, Value>) -> SandboxPolicy {
        let strs = |k: &str| -> Vec<String> { v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
        let int = |k: &str, d: u64| v.get(k).and_then(|x| x.as_u64()).unwrap_or(d);
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
        let mut tiers: Vec<Tier> = strs("tiers").iter().filter_map(|t| Tier::parse(t)).collect();
        if !flag("browser") {
            tiers.retain(|t| *t != Tier::Browser);
        }
        if !flag("userns_ack") {
            tiers.retain(|t| *t != Tier::Namespace);
        }
        SandboxPolicy {
            enabled,
            tiers,
            browser: flag("browser"),
            userns_ack: flag("userns_ack"),
            allowed_registries: strs("allowed_registries"),
            require_digest: flag("require_digest"),
            max: Limits {
                cpu_m: int("max_cpu_m", 0) as u32,
                mem_mib: int("max_mem_mib", 0) as u32,
                pids: int("max_pids", 0) as u32,
                seconds: int("max_seconds", 0) as u32,
                workspace_mib: int("max_workspace_mib", 0) as u32,
                output_kib: 256,
            },
            network_modes: strs("network_modes").iter().filter_map(|m| NetMode::parse(m)).filter(|m| *m != NetMode::Direct).collect(),
            runtime_class: text("runtime_class"),
            cli_role: text("cli_role").unwrap_or_else(|| "tenant_admin".into()),
            approval_patterns: strs("approval_patterns"),
            per_person: int("per_person_concurrency", 1) as u32,
            per_team: int("per_team_concurrency", 1) as u32,
            queue_timeout_secs: int("queue_timeout_secs", 1),
            audit_retention_days: int("audit_retention_days", 90) as u32,
            k3s_environment: text("k3s_environment").unwrap_or_else(|| "dev".into()),
            browser_domains: strs("browser_domains"),
            browser_named_profile: text("browser_profile").as_deref() == Some("named"),
            browser_download_mib: int("browser_download_mib", 64) as u32,
            browser_session_secs: int("browser_session_secs", 300) as u32,
            browser_image: text("browser_image").unwrap_or_default(),
            cli_image: text("cli_image").unwrap_or_default(),
            proxy: ProxyTarget::default(),
            namespace: String::new(),
        }
    }

    /// The tenant's policy for `team`, from the stored configuration. Anything unreadable is [`SandboxPolicy::off`].
    pub fn load(env: &Env, team: &str) -> SandboxPolicy {
        let Some(spec) = schema::kind("sandbox.policy") else { return SandboxPolicy::off() };
        let Ok(cfg) = Store::new(env.dir).load() else { return SandboxPolicy::off() };
        let entries = cfg.entries();
        let eff = resolve(spec, &cfg.layers(&entries, "sandbox.policy", Some(team), "default"));
        if !eff.exists || !eff.enabled {
            return SandboxPolicy::off();
        }
        SandboxPolicy::from_values(true, &eff.values)
    }

    pub fn allows_tier(&self, t: Tier) -> bool {
        self.tiers.contains(&t)
    }
}
