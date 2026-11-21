//! The glue between the enforcement functions of [`super::enforce`] and the places that act: the git clone guard, the gate for team
//! provider and local-model calls (with counters), the fair-share limiter at the local-model call site, and the k3s deploy gate that
//! files and consumes approvals. Every function here is small and testable with fakes; the callers are named in each doc comment.

use super::api::{emit, Env};
use super::enforce::{self, AgentUse, Deploy, DeployAsk};
use super::fairshare::{self, Admit, Limits, Running};
use super::resolve::{covers, resolve, state_of, State};
use super::schema;
use super::store::Store;
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

// ------------------------------------------------------------------------------------------------------------------ git

/// The git clone guard (`webservice/files.rs` `clone`). A tenant that has configured no `git.remote` entry for this team changes nothing;
/// once it has configured any (its own or the tenant's), a clone must match the `url_prefixes` of a READY entry (not blocked by a safety
/// rule). The answer to a refusal never repeats the address. Pushing is [`git_push_guard`].
pub fn git_clone_guard(env: &Env, team: Option<&str>, user: &str, url: &str) -> Result<(), String> {
    let Ok(cfg) = Store::new(env.dir).load() else { return Err("the tenant configuration cannot be read".into()) };
    let mut names: Vec<String> = cfg.entries().iter().filter(|e| e.item.kind == "git.remote" && (e.item.namespace.is_none() || e.item.namespace.as_deref() == team)).map(|e| e.item.name.clone()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(());
    }
    for n in &names {
        if enforce::git_remote_allows(env, team, n, url).0 {
            emit(env, user, "tenant.git.clone", &format!("remote={n} team={}", team.unwrap_or("-")));
            return Ok(());
        }
    }
    emit(env, user, "tenant.git.refused", &format!("team={} reason=no_matching_remote", team.unwrap_or("-")));
    Err("the tenant's git settings do not allow this repository address".into())
}

/// The git push guard (`platform/connect.rs` `plan_push` and `push`). Same rule as the clone guard, with one more condition: the READY entry
/// that matches the address must say `access: push` (the second value of [`enforce::git_remote_allows`]); an entry that only reads does not
/// allow a push. A tenant with no `git.remote` entry changes nothing. The refusal never repeats the address.
pub fn git_push_guard(env: &Env, team: Option<&str>, user: &str, url: &str) -> Result<(), String> {
    let Ok(cfg) = Store::new(env.dir).load() else { return Err("the tenant configuration cannot be read".into()) };
    let mut names: Vec<String> = cfg.entries().iter().filter(|e| e.item.kind == "git.remote" && (e.item.namespace.is_none() || e.item.namespace.as_deref() == team)).map(|e| e.item.name.clone()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(());
    }
    let mut read_only = false;
    for n in &names {
        match enforce::git_remote_allows(env, team, n, url) {
            (true, true) => {
                emit(env, user, "tenant.git.push", &format!("remote={n} team={}", team.unwrap_or("-")));
                return Ok(());
            }
            (true, false) => read_only = true,
            _ => {}
        }
    }
    emit(env, user, "tenant.git.push.refused", &format!("team={} reason={}", team.unwrap_or("-"), if read_only { "read_only" } else { "no_matching_remote" }));
    Err(if read_only { "the tenant's git settings allow reading this repository, not pushing to it".into() } else { "the tenant's git settings do not allow this repository address".into() })
}

// ------------------------------------------------------------------------------------------------------------------ providers

fn role_rank(name: &str) -> i32 {
    schema::ROLES.iter().position(|r| *r == name).map_or(99, |i| i as i32)
}

/// A person uses a team's model provider, API-key integration or local model host (`kind`: `models.provider`, `integration.apikey`,
/// `models.local`). Refuses unless the entry is ready and not blocked, the model is on its allow-list (`provider_models` for providers),
/// the person's team role is high enough (`use_role` of a local host) and the per-person daily and team monthly request counters
/// have room; counts the use and writes `tenant.provider.use` / `tenant.provider.refused` (never a key).
/// Callers: `webservice/handlers.rs` `runs_start` when the request names a team entry, and [`call_local`].
#[allow(clippy::too_many_arguments)]
pub fn provider_gate(env: &Env, kind: &str, team: Option<&str>, name: &str, user: &str, user_rank: i32, model: &str, now: u64) -> AgentUse {
    provider_gate_for(env, kind, team, name, user, user_rank, Some(model), now)
}

/// [`provider_gate`] for a use that names no model (a git host, a search service: `model` is `None` and only the entry, the role and the
/// counters decide).
#[allow(clippy::too_many_arguments)]
pub fn provider_gate_for(env: &Env, kind: &str, team: Option<&str>, name: &str, user: &str, user_rank: i32, model: Option<&str>, now: u64) -> AgentUse {
    let model_text = model.unwrap_or("-");
    let refuse = |why: &str| {
        emit(env, user, "tenant.provider.refused", &format!("kind={kind} name={name} model={model_text} reason={why}"));
        AgentUse::Refused(why.to_string())
    };
    if !matches!(kind, "models.provider" | "integration.apikey" | "models.local") {
        return refuse("not a provider kind");
    }
    let Some(spec) = schema::kind(kind) else { return refuse("unknown kind") };
    let Ok(cfg) = Store::new(env.dir).load() else { return refuse("the tenant configuration cannot be read") };
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, kind, team, name));
    if !eff.exists {
        return refuse("this provider is not configured");
    }
    let (st, reasons) = state_of(spec, &eff, env.vault.as_ref());
    if st != State::Ready {
        return refuse(&format!("this provider is {} ({})", st.name(), reasons.join("; ")));
    }
    if let Some(why) = enforce::blocked_by_safety(env, kind, team, name, now) {
        return refuse(&format!("blocked by a safety rule ({why})"));
    }
    let allowed: Vec<String> = if kind == "models.provider" {
        match enforce::provider_models(env, team, name) {
            Some(m) => m,
            None => return refuse("this provider is not ready"),
        }
    } else {
        eff.values.get("allowed_models").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    if model.is_some_and(|m| !allowed.iter().any(|a| covers(a, m))) {
        return refuse("this model is not allowed for the provider");
    }
    if let Some(r) = eff.values.get("use_role").and_then(|v| v.as_str()) {
        if user_rank < role_rank(r) {
            return refuse("your team role does not allow using this host");
        }
    }
    let num = |k: &str| eff.values.get(k).and_then(|v| v.as_u64());
    let scope = team.unwrap_or("-");
    // tokens: the answer's cost is added by [`record_tokens`] after the call; here the next request is refused once the day's count is over the limit
    if let Some(limit) = num("daily_tokens_per_person") {
        if Store::new(env.dir).tokens_today(kind, &format!("{scope}:{name}"), user, now) >= limit {
            return refuse("the daily token limit of this provider is reached");
        }
    }
    match Store::new(env.dir).count_use(kind, &format!("{scope}:{name}"), user, now, num("daily_requests_per_person"), None, num("monthly_requests_team")) {
        Ok((p, _, mo)) => {
            emit(env, user, "tenant.provider.use", &format!("kind={kind} name={name} team={scope} model={model_text} person_today={p} team_period={mo}"));
            AgentUse::Allowed { shared: true }
        }
        Err(_) => refuse("the daily or monthly request limit of this provider is reached"),
    }
}

/// What a use of a team's API-key integration (`integration.apikey`) is handed: the entry's fields and the key for this call only.
pub struct IntegrationTarget<'a> {
    /// The provider of the catalog (`github`, `openai`, ...).
    pub provider: &'a str,
    /// The entry's base URL as written; empty when the provider's own address is used.
    pub base_url: &'a str,
    pub deployment: &'a str,
    pub api_version: &'a str,
    pub key: &'a str,
}

/// A team's API-key integration used by anything other than a run start (a flow step, a tool, a git host call): the gate of
/// [`provider_gate_for`] (entry ready and not blocked, the model on the list when the use names one, the person's daily and the team's
/// monthly counters, audit `tenant.provider.use` per person), then the vault hands the key to `f` for this call only (named action
/// `action`; the vault's audit line names the action and the reference, never the value). The person must belong to the team: the caller
/// passes the rank from the roles (`user_rank`, 0 for a stranger is refused by the team check of the caller).
#[allow(clippy::too_many_arguments)]
pub fn with_integration_key<T>(env: &Env, team: &str, name: &str, user: &str, user_rank: i32, model: Option<&str>, action: &str, now: u64, f: &mut dyn FnMut(&IntegrationTarget) -> Result<T, String>) -> Result<T, String> {
    if let AgentUse::Refused(why) = provider_gate_for(env, "integration.apikey", Some(team), name, user, user_rank, model, now) {
        return Err(why);
    }
    let spec = schema::kind("integration.apikey").ok_or("unknown kind")?;
    let cfg = Store::new(env.dir).load().map_err(|_| "the tenant configuration cannot be read")?;
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "integration.apikey", Some(team), name));
    let text = |k: &str| eff.values.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (provider, base, deployment, version) = (text("provider"), text("base_url"), text("deployment"), text("api_version"));
    let r = eff.values.get("api_key").and_then(|v| v.as_str()).and_then(schema::SecretRef::parse).ok_or("this integration has no key")?;
    let mut slot: Option<T> = None;
    env.vault.use_secret(&r, action, &mut |k: &str| {
        slot = Some(f(&IntegrationTarget { provider: &provider, base_url: &base, deployment: &deployment, api_version: &version, key: k })?);
        Ok(String::new())
    })?;
    slot.take().ok_or_else(|| "the call produced no result".to_string())
}

/// Adds the tokens an answer cost to the person's count for today on the entry (`daily_tokens_per_person` is checked by [`provider_gate`]
/// on the next request). Audited by count only.
pub fn record_tokens(env: &Env, kind: &str, team: Option<&str>, name: &str, user: &str, now: u64, tokens: u64) {
    let scope = team.unwrap_or("-");
    let total = Store::new(env.dir).add_tokens(kind, &format!("{scope}:{name}"), user, now, tokens);
    emit(env, user, "tenant.provider.tokens", &format!("kind={kind} name={name} team={scope} added={tokens} today={total}"));
}

/// What a call to a team's model provider (`models.provider`) is handed. The key is valid only inside the call.
pub struct ProviderTarget<'a> {
    /// `openai`, `ollama`, `anthropic` or `other`.
    pub api: &'a str,
    /// The entry's base URL as written (may carry a path such as `/v1`).
    pub base_url: &'a str,
    pub key: Option<&'a str>,
}

/// The provider call through the gate (entry ready, model on the list, counters, daily tokens) and the vault: `f` gets the entry and the
/// key for this call only (named action `provider-call`).
#[allow(clippy::too_many_arguments)]
pub fn with_provider_target<T>(env: &Env, team: &str, name: &str, user: &str, user_rank: i32, model: &str, now: u64, f: &mut dyn FnMut(&ProviderTarget) -> Result<T, String>) -> Result<T, String> {
    if let AgentUse::Refused(why) = provider_gate(env, "models.provider", Some(team), name, user, user_rank, model, now) {
        return Err(why);
    }
    let spec = schema::kind("models.provider").ok_or("unknown kind")?;
    let cfg = Store::new(env.dir).load().map_err(|_| "the tenant configuration cannot be read")?;
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "models.provider", Some(team), name));
    let text = |k: &str| eff.values.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (api, base) = (text("api"), text("base_url"));
    let mut slot: Option<T> = None;
    let out = match eff.values.get("api_key").and_then(|v| v.as_str()).and_then(schema::SecretRef::parse) {
        Some(r) => env.vault.use_secret(&r, "provider-call", &mut |k: &str| {
            slot = Some(f(&ProviderTarget { api: &api, base_url: &base, key: Some(k) })?);
            Ok(String::new())
        }),
        None => f(&ProviderTarget { api: &api, base_url: &base, key: None }).map(|v| {
            slot = Some(v);
            String::new()
        }),
    };
    out.and_then(|_| slot.take().ok_or_else(|| "the call produced no result".to_string()))
}

// ------------------------------------------------------------------------------------------------------------------ fair share

#[derive(Default)]
struct HostState {
    running: Running,
    /// (ticket, user, since) in arrival order.
    queue: Vec<(u64, String, u64)>,
    next_ticket: u64,
}

type Hosts = Mutex<BTreeMap<String, HostState>>;
fn hosts() -> &'static (Hosts, Condvar) {
    static H: OnceLock<(Hosts, Condvar)> = OnceLock::new();
    H.get_or_init(|| (Mutex::new(BTreeMap::new()), Condvar::new()))
}

/// A slot on a local model host; dropping it gives the slot back and wakes the waiters.
pub struct Permit {
    key: String,
    user: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (m, cv) = hosts();
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = g.get_mut(&self.key) {
            if let Some(n) = h.running.by_person.get_mut(&self.user) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    h.running.by_person.remove(&self.user);
                }
            }
        }
        cv.notify_all();
    }
}

/// Wait for a slot on host `key` (tenant dir + team + name) under `limits`: [`fairshare::admit`] decides whether the person may go, and
/// [`fairshare::next_to_run`] chooses among the waiting people (fewest running first, then longest waiting). Gives up after the queue
/// timeout. `clock` returns seconds; the wait itself is real time.
pub fn acquire(key: &str, user: &str, limits: &Limits, clock: &dyn Fn() -> u64) -> Result<Permit, String> {
    let (m, cv) = hosts();
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    let since = clock();
    let ticket = {
        let h = g.entry(key.to_string()).or_default();
        h.next_ticket += 1;
        let t = h.next_ticket;
        h.queue.push((t, user.to_string(), since));
        t
    };
    loop {
        let now = clock();
        let h = g.get_mut(key).expect("host state");
        let q: Vec<(String, u64)> = h.queue.iter().map(|(_, u, s)| (u.clone(), *s)).collect();
        let verdict = fairshare::admit(limits, &h.running, user, Some(since), now);
        let mine_next = fairshare::next_to_run(limits, &h.running, &q, now).is_some_and(|i| h.queue[i].0 == ticket);
        match verdict {
            Admit::Run if mine_next => {
                h.queue.retain(|(t, _, _)| *t != ticket);
                *h.running.by_person.entry(user.to_string()).or_insert(0) += 1;
                return Ok(Permit { key: key.to_string(), user: user.to_string() });
            }
            Admit::Reject(why) => {
                h.queue.retain(|(t, _, _)| *t != ticket);
                cv.notify_all();
                return Err(why);
            }
            _ => {}
        }
        if now >= since + limits.queue_timeout_secs {
            h.queue.retain(|(t, _, _)| *t != ticket);
            cv.notify_all();
            return Err("the queue timed out".into());
        }
        g = cv.wait_timeout(g, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
    }
}

/// What a call to a team's local model host is handed: where it goes, how, with which token, and the limits of the entry.
pub struct LocalTarget<'a> {
    /// `scheme://host:port`, no path.
    pub base: &'a str,
    /// `ollama` or `openai_compatible`.
    pub style: &'a str,
    /// The bearer token when the host has one; valid only inside the call.
    pub token: Option<&'a str>,
    /// `max_request_kb` of the entry (in bytes): the largest request body sent.
    pub max_request_bytes: usize,
    /// `max_request_secs` of the entry: the longest the whole call may take.
    pub max_secs: u64,
}

/// Where a call to a team's local model host goes through the gate, the limiter and the vault. `f` gets the base URL
/// (`scheme://host:port`), the API style (`ollama` / `openai_compatible`) and the bearer token when the host has one (only for the
/// call, by "use a secret for a named action"). The slot is held until `f` returns. Refusals: the gate's reasons, or the queue timeout.
/// The chat backend that makes the call is [`super::localchat`] (it uses [`call_local_target`], which also hands over the entry's limits).
#[allow(clippy::too_many_arguments)]
pub fn call_local(env: &Env, team: &str, name: &str, user: &str, user_rank: i32, model: &str, now: u64, clock: &dyn Fn() -> u64, f: &mut dyn FnMut(&str, &str, Option<&str>) -> Result<String, String>) -> Result<String, String> {
    call_local_target(env, team, name, user, user_rank, model, now, clock, &mut |t| f(t.base, t.style, t.token))
}

/// As [`call_local`], handing over a [`LocalTarget`] (the entry's request size and time limits as well).
#[allow(clippy::too_many_arguments)]
pub fn call_local_target(env: &Env, team: &str, name: &str, user: &str, user_rank: i32, model: &str, now: u64, clock: &dyn Fn() -> u64, f: &mut dyn FnMut(&LocalTarget) -> Result<String, String>) -> Result<String, String> {
    with_local_target(env, team, name, user, user_rank, model, now, clock, f)
}

/// As [`call_local_target`] for a call whose result is not a string (a model turn with tool calls, for example).
#[allow(clippy::too_many_arguments)]
pub fn with_local_target<T>(env: &Env, team: &str, name: &str, user: &str, user_rank: i32, model: &str, now: u64, clock: &dyn Fn() -> u64, f: &mut dyn FnMut(&LocalTarget) -> Result<T, String>) -> Result<T, String> {
    if let AgentUse::Refused(why) = provider_gate(env, "models.local", Some(team), name, user, user_rank, model, now) {
        return Err(why);
    }
    let spec = schema::kind("models.local").ok_or("unknown kind")?;
    let cfg = Store::new(env.dir).load().map_err(|_| "the tenant configuration cannot be read")?;
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "models.local", Some(team), name));
    let num = |k: &str| eff.values.get(k).and_then(|v| v.as_u64());
    let text = |k: &str| eff.values.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let limits = Limits { per_person: num("max_concurrent_per_person").unwrap_or(1) as u32, per_team: num("max_concurrent_team").unwrap_or(1) as u32, queue_timeout_secs: num("queue_timeout_secs").unwrap_or(30) };
    let base = format!("{}://{}:{}", if text("scheme").is_empty() { "http" } else { text("scheme") }, text("host"), num("port").unwrap_or(0));
    let style = text("api_style").to_string();
    let max_request_bytes = (num("max_request_kb").unwrap_or(1024) as usize).saturating_mul(1024);
    let max_secs = num("max_request_secs").unwrap_or(120);
    let key = format!("{}|{team}|{name}", env.dir.display());
    let permit = acquire(&key, user, &limits, clock).inspect_err(|why| {
        emit(env, user, "tenant.provider.refused", &format!("kind=models.local name={name} model={model} reason={why}"));
    })?;
    let mut slot: Option<T> = None;
    let out = match eff.values.get("token").and_then(|v| v.as_str()).and_then(schema::SecretRef::parse) {
        Some(r) => env.vault.use_secret(&r, "local-model-call", &mut |t: &str| {
            slot = Some(f(&LocalTarget { base: &base, style: &style, token: Some(t), max_request_bytes, max_secs })?);
            Ok(String::new())
        }),
        None => f(&LocalTarget { base: &base, style: &style, token: None, max_request_bytes, max_secs }).map(|v| {
            slot = Some(v);
            String::new()
        }),
    };
    drop(permit);
    out.and_then(|_| slot.take().ok_or_else(|| "the call produced no result".to_string()))
}

// ------------------------------------------------------------------------------------------------------------------ k3s deploy

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeployStep {
    /// Go ahead; `approved_by` is set when a second person's approval was consumed.
    Go { approved_by: Option<String> },
    /// A request is filed (or already waiting); nothing runs. The requester asks again after the approval.
    Pending { id: String, approver_role: String },
    Denied(String),
}

/// The deploy entry point's one call. There is no tenant deploy in `cluster.rs` / `clusters.rs` / `k3sdev.rs` yet (those manage a
/// person's own local and remote clusters, not a team's environment bindings), so the function a future team deploy must call before it
/// touches a cluster is this one: it decides ([`enforce::k3s_decide`]); when four eyes asks for an approval it first TAKES one that is
/// already granted (single use, [`super::approvals::take_approval`]) and decides again with it, otherwise FILES a request
/// ([`super::approvals::request`], routed to the team's approver-role holders, then its admins, then the tenant admins). An approval
/// is only ever made by a person's own click on `POST /api/v1/tenant/approvals/{id}/approve`.
pub fn deploy_gate(env: &Env, ask: &DeployAsk, now: u64) -> DeployStep {
    match enforce::k3s_decide(env, ask) {
        Deploy::Allow => DeployStep::Go { approved_by: ask.approval.map(String::from) },
        Deploy::Deny(why) => DeployStep::Denied(why),
        Deploy::NeedsApproval { approver_role } => {
            if let Some(by) = super::approvals::take_approval(env, "k3s.cluster", ask.team, ask.environment, ask.requester, now) {
                let again = DeployAsk { team: ask.team, environment: ask.environment, requester: ask.requester, requester_rank: ask.requester_rank, approval: Some(&by), promoted_from: ask.promoted_from, registry: ask.registry, roles: ask.roles, guest: ask.guest };
                return match enforce::k3s_decide(env, &again) {
                    Deploy::Allow => DeployStep::Go { approved_by: Some(by) },
                    Deploy::Deny(why) => DeployStep::Denied(why),
                    Deploy::NeedsApproval { .. } => DeployStep::Denied("the approval does not satisfy the binding".into()),
                };
            }
            let (tenant_only, role_only) = enforce::k3s_approval_params(env, ask.roles, ask.team, ask.environment);
            match super::approvals::request(env, ask.roles, "k3s.cluster", ask.team, ask.environment, ask.requester, tenant_only, role_only, now) {
                Ok(p) => DeployStep::Pending { id: p.id, approver_role },
                Err(m) => DeployStep::Denied(m),
            }
        }
    }
}

/// What a cluster manager does around a change to a team's environment: ask [`deploy_gate`], and only on `Go` run `apply` (handed the name
/// of the person whose approval was consumed, when one was). `Pending` files (or finds) the request and answers with its id and the role
/// that can decide; nothing is applied until that person has approved and the manager asks again. An approval is spent when `apply`
/// starts, so an `apply` that fails needs a new approval. Every outcome is audited by team, environment and request id, never by content.
pub fn deploy_through<T>(env: &Env, ask: &DeployAsk, now: u64, apply: &mut dyn FnMut(Option<&str>) -> Result<T, String>) -> Result<T, String> {
    let line = |what: &str, extra: &str| emit(env, ask.requester, what, &format!("team={} environment={}{extra}", ask.team, ask.environment));
    match deploy_gate(env, ask, now) {
        DeployStep::Go { approved_by } => {
            line("tenant.deploy.go", &format!(" approved_by={}", approved_by.as_deref().unwrap_or("-")));
            let r = apply(approved_by.as_deref());
            line(if r.is_ok() { "tenant.deploy.applied" } else { "tenant.deploy.failed" }, "");
            r
        }
        DeployStep::Pending { id, approver_role } => {
            line("tenant.deploy.waiting", &format!(" request={id} approver_role={approver_role}"));
            Err(format!("waiting for approval {id} by a {approver_role}; nothing was changed"))
        }
        DeployStep::Denied(why) => {
            line("tenant.deploy.denied", "");
            Err(why)
        }
    }
}
