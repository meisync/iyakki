//! The enforcement points: the places that must ask the tenant's configuration before they act. Each function is small, pure
//! over the loaded configuration, and records one audit line for its decision (never a secret).
//!
//! | point | function | wired today |
//! |---|---|---|
//! | a member starts a shared coding agent | [`agent_gate`] | `webservice/handlers.rs` `runs_start` (provider `claude` / `copilot`) |
//! | a person's browser media | [`media_policy_for`] | `browsermedia::Policy::for_tenant`, `iyakki-remote` `screen.rs` |
//! | the egress proxy asks "may this host be reached" | [`egress_narrowing`] (over [`egress_allows`]) | `AiConfig::build_proxy` when `IYAKKI_TENANT_DATA_DIR` is set |
//! | a deploy to a cluster | [`k3s_decide`] via `runtime::deploy_gate` | no tenant deploy exists yet; see the plan, section 8 |
//! | a clone, a team provider or local host | [`git_remote_allows`], [`provider_models`] via `runtime::{git_clone_guard, provider_gate}` | the web clone route and run start |

use super::api::{emit, Env};
use super::resolve::{covers, resolve, state_of, Effective, State};
use super::schema::{self, k3s_ref_prefix, SecretRef};
use super::store::Store;
use super::Item;
use serde_json::Value;

fn effective(env: &Env, kind: &str, team: Option<&str>, name: &str) -> Option<(&'static schema::KindSpec, Effective)> {
    let spec = schema::kind(kind)?;
    let cfg = Store::new(env.dir).load().ok()?;
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, kind, team, name));
    Some((spec, eff))
}

fn strs(eff: &Effective, k: &str) -> Vec<String> {
    eff.values.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn int(eff: &Effective, k: &str) -> Option<u64> {
    eff.values.get(k).and_then(|v| v.as_u64())
}

fn text<'a>(eff: &'a Effective, k: &str) -> Option<&'a str> {
    eff.values.get(k).and_then(|v| v.as_str())
}

// ------------------------------------------------------------------------------------------------------------------ agents

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentUse {
    /// `shared`: the tenant's one login is used (counted); otherwise the person's own login.
    Allowed { shared: bool },
    Refused(String),
}

/// A member of the tenant starts a coding agent. Refuses when the agent is not switched on (the default), its secret is missing or
/// the vault is locked, the model is not allowed, or a shared agent's daily limit is reached. Counts and audits a shared use.
pub fn agent_gate(env: &Env, kind: &str, name: &str, team: Option<&str>, user: &str, model: Option<&str>, now: u64) -> AgentUse {
    let refuse = |why: &str| {
        emit(env, user, "tenant.agent.refused", &format!("kind={kind} name={name} model={} reason={why}", model.unwrap_or("-")));
        AgentUse::Refused(why.to_string())
    };
    if !kind.starts_with("agent.") {
        return refuse("not an agent kind");
    }
    let Some((spec, eff)) = effective(env, kind, team, name) else { return refuse("the tenant configuration cannot be read") };
    if !eff.exists {
        return refuse("this agent is not switched on for the tenant");
    }
    let (st, reasons) = state_of(spec, &eff, env.vault.as_ref());
    if st != State::Ready {
        return refuse(&format!("this agent is {} ({})", st.name(), reasons.join("; ")));
    }
    if let Some(why) = blocked_by_safety(env, kind, team, name, now) {
        return refuse(&format!("blocked by a safety rule ({why})"));
    }
    if let Some(m) = model {
        if !strs(&eff, "allowed_models").iter().any(|a| covers(a, m)) {
            return refuse("this model is not allowed for the agent");
        }
    }
    let shared = text(&eff, "mode") == Some("shared");
    if shared {
        let store = Store::new(env.dir);
        let scope = team.unwrap_or("-");
        match store.count_use(kind, scope, user, now, int(&eff, "daily_requests_per_person"), int(&eff, "daily_requests_tenant"), int(&eff, "monthly_requests_team")) {
            Ok((p, t, mo)) => emit(env, user, "tenant.agent.use", &format!("kind={kind} name={name} team={scope} model={} shared=true person_today={p} scope_today={t} team_period={mo}", model.unwrap_or("-"))),
            Err(_) => return refuse("the daily or monthly limit of the shared agent is reached"),
        }
    } else {
        emit(env, user, "tenant.agent.use", &format!("kind={kind} name={name} model={} shared=false", model.unwrap_or("-")));
    }
    AgentUse::Allowed { shared }
}

/// What the tenant allows teams to integrate, and the thresholds of the safety rules. Nothing configured: everything allowed with the
/// default thresholds; a policy item that is switched off allows nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct IntegrationPolicy {
    pub allow_team: bool,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub needs_approval: bool,
    pub allow_private: bool,
    pub warn_monthly: u64,
    pub warn_daily: u64,
    pub key_warn_days: u64,
    pub key_strong_days: u64,
    pub max_domains: usize,
    /// Only true when the operator's file manages the policy item and says so.
    pub allow_wildcard: bool,
    pub require_four_eyes_prod: bool,
    pub audit_enabled: bool,
}

pub fn integration_policy(cfg: &super::store::Config) -> IntegrationPolicy {
    let all = vec!["*".to_string()];
    let mut p = IntegrationPolicy { allow_team: true, providers: all.clone(), models: all, needs_approval: false, allow_private: false, warn_monthly: 100_000, warn_daily: 2_000, key_warn_days: 90, key_strong_days: 180, max_domains: 50, allow_wildcard: false, require_four_eyes_prod: false, audit_enabled: true };
    let Some(spec) = schema::kind("integration.policy") else { return p };
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "integration.policy", None, "default"));
    if !eff.exists {
        return p;
    }
    if !eff.enabled {
        p.allow_team = false;
        p.providers.clear();
        p.models.clear();
        return p;
    }
    let b = |k: &str| eff.values.get(k) == Some(&Value::Bool(true));
    p.allow_team = text(&eff, "team_integrations") != Some("deny");
    p.providers = strs(&eff, "allowed_providers");
    p.models = strs(&eff, "allowed_models");
    p.needs_approval = b("new_provider_needs_approval");
    p.allow_private = b("allow_private_endpoints");
    p.warn_monthly = int(&eff, "warn_monthly_cap").unwrap_or(p.warn_monthly);
    p.warn_daily = int(&eff, "warn_daily_per_person").unwrap_or(p.warn_daily);
    p.key_warn_days = int(&eff, "key_warn_days").unwrap_or(90);
    p.key_strong_days = int(&eff, "key_strong_days").unwrap_or(180);
    p.max_domains = int(&eff, "max_egress_domains").unwrap_or(50) as usize;
    p.allow_wildcard = b("allow_wildcard_egress") && cfg.is_managed("integration.policy", None, "default");
    p.require_four_eyes_prod = b("require_four_eyes_prod");
    p.audit_enabled = eff.values.get("audit_enabled") != Some(&Value::Bool(false));
    p
}

/// Hosts of the tenant's egress policy: `None` when it has none.
pub fn egress_allows_cfg(cfg: &super::store::Config, team: Option<&str>, host: &str) -> Option<bool> {
    let entries = cfg.entries();
    let eff = resolve(schema::kind("egress.policy")?, &cfg.layers(&entries, "egress.policy", team, "default"));
    if !eff.exists {
        return None;
    }
    if !eff.enabled || text(&eff, "mode") == Some("deny_all") {
        return Some(false);
    }
    let host = host.to_ascii_lowercase();
    Some(strs(&eff, "allowed_domains").iter().any(|p| covers(p, &host)))
}

/// The first BLOCK rule an item breaks (code and text), for the hooks that must not run a blocked item.
pub fn blocked_by_safety(env: &Env, kind: &str, team: Option<&str>, name: &str, now: u64) -> Option<String> {
    let cfg = Store::new(env.dir).load().ok()?;
    let spec = schema::kind(kind)?;
    let entries = cfg.entries();
    let layers = cfg.layers(&entries, kind, team, name);
    let eff = resolve(spec, &layers);
    let item = entries.iter().find(|e| e.item.kind == kind && e.item.namespace.as_deref() == team && e.item.name == name).map(|e| e.item.clone()).or_else(|| layers.last().map(|l| l.item.clone()))?;
    let meta = Store::new(env.dir).meta();
    super::safety::evaluate(&super::safety::Ctx { spec, item: &item, eff: &eff, cfg: &cfg, roles: None, meta: &meta, now }).into_iter().find(|w| w.severity == "block").map(|w| format!("{}: {}", w.code, w.text))
}

/// The token of a shared agent, handed to `f` for the named action only. The launcher calls this; the token never leaves `f`.
pub fn with_agent_token(env: &Env, kind: &str, name: &str, team: Option<&str>, user: &str, action: &str, f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
    let (_, eff) = effective(env, kind, team, name).ok_or("the tenant configuration cannot be read")?;
    let r = text(&eff, "token").and_then(SecretRef::parse).ok_or("no shared token is configured")?;
    emit(env, user, "tenant.secret.use", &format!("ref={} action={action}", r.text()));
    env.vault.use_secret(&r, action, f)
}

// ------------------------------------------------------------------------------------------------------------------ media

/// The browser-media policy of a person. A tenant `media.policy` item decides when there is one (switched off = nothing allowed);
/// otherwise `fallback` (the machine's setting) stays in force.
pub fn media_policy_for(dir: &std::path::Path, team: Option<&str>, user: &str, role: iyakki_sec::policy::Role, fallback: crate::browsermedia::Policy) -> crate::browsermedia::Policy {
    use crate::browsermedia::Policy;
    let Ok(cfg) = Store::new(dir).load() else { return Policy::deny_all() };
    let entries = cfg.entries();
    let Some(spec) = schema::kind("media.policy") else { return fallback };
    let eff = resolve(spec, &cfg.layers(&entries, "media.policy", team, "default"));
    if !eff.exists {
        return fallback;
    }
    if !eff.enabled || text(&eff, "media") != Some("allow") {
        return Policy::deny_all();
    }
    let mut p = Policy::from_sec(&iyakki_sec::media::MediaPolicy::tenant(true), user, role);
    for d in strs(&eff, "deny_devices") {
        match d.as_str() {
            "mic" => p.mic = false,
            "camera" => p.camera = false,
            "audio" => p.audio = false,
            "clipread" => p.clip_read = false,
            _ => {}
        }
    }
    p
}

/// Where a pod reads its tenant's configuration from (set by the manifest): the tenant's data folder.
pub fn data_dir_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os("IYAKKI_TENANT_DATA_DIR").map(std::path::PathBuf::from).filter(|p| p.is_absolute())
}

// ------------------------------------------------------------------------------------------------------------------ egress, git, models

/// `None`: the tenant has no egress policy (the caller keeps its own rule). `Some(false)` for a policy that is switched off.
pub fn egress_allows(dir: &std::path::Path, team: Option<&str>, host: &str) -> Option<bool> {
    egress_allows_cfg(&Store::new(dir).load().ok()?, team, host)
}

/// The tenant's egress policy as an extra, narrowing rule for the egress proxy (`iyakki_proxy::EgressProxy::with_narrowing`): the policy
/// is read on every call so an edit applies at once. A broken or unreadable configuration denies (the safe reading).
pub fn egress_narrowing(dir: std::path::PathBuf, team: Option<String>) -> iyakki_proxy::Narrowing {
    std::sync::Arc::new(move |host: &str| match Store::new(&dir).load() {
        Ok(cfg) => egress_allows_cfg(&cfg, team.as_deref(), host),
        Err(_) => Some(false),
    })
}

/// May `url` be used with the git remote entry `name`, and may it be pushed to? `(allowed, may_push)`; the credential helper calls this
/// and then [`super::vault::SecretStore::use_secret`] with the entry's `credential`.
pub fn git_remote_allows(env: &Env, team: Option<&str>, name: &str, url: &str) -> (bool, bool) {
    let Some((spec, eff)) = effective(env, "git.remote", team, name) else { return (false, false) };
    if state_of(spec, &eff, env.vault.as_ref()).0 != State::Ready || blocked_by_safety(env, "git.remote", team, name, super::now_secs()).is_some() {
        return (false, false);
    }
    let ok = strs(&eff, "url_prefixes").iter().any(|p| url.starts_with(p.as_str()));
    (ok, ok && text(&eff, "access") == Some("push"))
}

/// The models of a provider entry people may use, `None` when the provider is not ready.
pub fn provider_models(env: &Env, team: Option<&str>, name: &str) -> Option<Vec<String>> {
    let (spec, eff) = effective(env, "models.provider", team, name)?;
    (state_of(spec, &eff, env.vault.as_ref()).0 == State::Ready && blocked_by_safety(env, "models.provider", team, name, super::now_secs()).is_none()).then(|| strs(&eff, "allowed_models"))
}

// ------------------------------------------------------------------------------------------------------------------ k3s

pub struct DeployAsk<'a> {
    pub team: &'a str,
    pub environment: &'a str,
    /// Who wants the deploy and their rank in the team (`Caller::rank`).
    pub requester: &'a str,
    pub requester_rank: i32,
    /// An approval already given: who (checked against the router, not trusted).
    pub approval: Option<&'a str>,
    /// The environment this artifact was last deployed to successfully.
    pub promoted_from: Option<&'a str>,
    /// The registry of the image, when the deploy has one.
    pub registry: Option<&'a str>,
    /// The tenant's lists, for the approval router.
    pub roles: &'a crate::cloudroles::Roles,
    /// The requester's account role is guest: guests never request a deploy.
    pub guest: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deploy {
    Allow,
    NeedsApproval { approver_role: String },
    Deny(String),
}

fn role_rank(name: &str) -> i32 {
    schema::ROLES.iter().position(|r| *r == name).map_or(99, |i| i as i32)
}

/// The promotion chain (`k3s.environments`), default dev, preprod, prod.
pub fn promotion_order(env: &Env) -> Vec<String> {
    effective(env, "k3s.environments", None, "default").map(|(_, e)| strs(&e, "order")).filter(|o| !o.is_empty()).unwrap_or_else(|| vec!["dev".into(), "preprod".into(), "prod".into()])
}

fn approval_params(eff: &Effective, roles: &crate::cloudroles::Roles, team: &str, environment: &str, last: bool) -> (bool, bool) {
    let role_only = match text(eff, "approver_mode") {
        Some("role_only") => true,
        Some(_) => false,
        None => last && !roles.approver_holders(team, &[environment, "k3s.cluster"]).is_empty(),
    };
    (text(eff, "approver_role") == Some("tenant_admin"), role_only)
}

/// For the caller that files an approval request after `NeedsApproval`: (tenant_only, role_only) of this binding.
pub fn k3s_approval_params(env: &Env, roles: &crate::cloudroles::Roles, team: &str, environment: &str) -> (bool, bool) {
    let last = promotion_order(env).last().is_some_and(|l| l == environment);
    effective(env, "k3s.cluster", Some(team), environment).map_or((true, true), |(_, eff)| approval_params(&eff, roles, team, environment, last))
}

/// May this person deploy to (team, environment)? Allow, needs approval, or deny with the reason. Audited either way.
pub fn k3s_decide(env: &Env, ask: &DeployAsk) -> Deploy {
    let d = decide(env, ask);
    let (word, why) = match &d {
        Deploy::Allow => ("allow", String::new()),
        Deploy::NeedsApproval { approver_role } => ("needs_approval", format!("approver_role={approver_role}")),
        Deploy::Deny(r) => ("deny", r.clone()),
    };
    emit(env, ask.requester, "tenant.k3s.deploy", &format!("team={} environment={} decision={word} requested_by={} approved_by={} registry={} {why}", ask.team, ask.environment, ask.requester, ask.approval.unwrap_or("-"), ask.registry.unwrap_or("-")));
    d
}

fn decide(env: &Env, ask: &DeployAsk) -> Deploy {
    use Deploy::*;
    if ask.guest {
        return Deny("guests cannot deploy".into());
    }
    let Some((spec, eff)) = effective(env, "k3s.cluster", Some(ask.team), ask.environment) else { return Deny("the tenant configuration cannot be read".into()) };
    if !eff.exists {
        return Deny("no cluster binding for this team and environment".into());
    }
    if !eff.enabled {
        return Deny(format!("this environment is switched off by the {} level", eff.off_by.map_or("?", |l| l.name())));
    }
    if eff.values.get("frozen") == Some(&Value::Bool(true)) {
        return Deny("change freeze: the binding is frozen".into());
    }
    let (st, reasons) = state_of(spec, &eff, env.vault.as_ref());
    if st != State::Ready {
        return Deny(format!("the binding is {} ({})", st.name(), reasons.join("; ")));
    }
    if let Some(why) = blocked_by_safety(env, "k3s.cluster", Some(ask.team), ask.environment, super::now_secs()) {
        return Deny(format!("blocked by a safety rule ({why})"));
    }
    let probe = Item { kind: "k3s.cluster".into(), name: ask.environment.into(), namespace: Some(ask.team.into()), enabled: true, managed: false, values: Default::default() };
    if !text(&eff, "credential").and_then(SecretRef::parse).is_some_and(|r| r.name.starts_with(&k3s_ref_prefix(&probe))) {
        return Deny("the credential does not belong to this team and environment".into());
    }
    let order = promotion_order(env);
    let Some(idx) = order.iter().position(|o| o == ask.environment) else { return Deny("the environment is not in the promotion chain".into()) };
    if idx > 0 && ask.promoted_from != Some(order[idx - 1].as_str()) {
        return Deny(format!("promotion order: a deploy to {} comes from {}", ask.environment, order[idx - 1]));
    }
    let deploy_role = text(&eff, "deploy_role").map_or(if idx == 0 { "editor" } else { "admin" }, |s| s);
    if ask.requester_rank < role_rank(deploy_role) {
        return Deny(format!("deploying here needs the {deploy_role} role"));
    }
    if let Some(r) = ask.registry {
        if !strs(&eff, "allowed_registries").iter().any(|p| covers(p, r)) {
            return Deny("the image registry is not allowed for this binding".into());
        }
    }
    let four = match eff.values.get("four_eyes") {
        Some(Value::Bool(b)) => *b,
        _ => idx + 1 == order.len(),
    };
    if !four {
        return Allow;
    }
    // Owner decisions 2026-11-04: approval goes to an admin of the requester's team (never the requester, never another team's admin);
    // with none eligible it escalates to a tenant admin; the tenant may narrow it to tenant admins, a team cannot widen it.
    let (tenant_only, role_only) = approval_params(&eff, ask.roles, ask.team, ask.environment, idx + 1 == order.len());
    let r = super::approvals::route(ask.roles, ask.team, ask.requester, tenant_only, &[ask.environment, "k3s.cluster"], role_only);
    let note = if r.escalated { " (the team has no other admin: escalated to a tenant admin)" } else { "" };
    if r.approvers.is_empty() {
        return Deny(format!("nobody is eligible to approve this{note}"));
    }
    match ask.approval {
        None => NeedsApproval { approver_role: if r.escalated || r.tenant_only { "tenant_admin".into() } else { "admin".into() } },
        Some(by) if by == ask.requester => Deny("an approval cannot come from the person who asked".into()),
        Some(by) if !r.approvers.iter().any(|a| a == by) => Deny(format!("the approver must be an admin of the team{}{note}", if r.tenant_only { " and a tenant admin (the tenant requires tenant_admin)" } else { "" })),
        Some(_) => Allow,
    }
}
