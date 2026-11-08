//! Who a schedule runs as, and what it may use while nobody is signed in.
//!
//! * **Run identity.** A schedule is recorded with a delegated identity: (tenant, team, owner, the owner's role in the team at that
//!   moment) or, when a team admin chose it, the team's service identity. Every check uses the SNAPSHOT and the owner's CURRENT
//!   membership, whichever is lower: an owner who left the team or lost the role pauses their schedules ([`check`]) and the team's
//!   admins are told; nothing keeps running on stale rights, and a role gained later does not raise what the schedule may do.
//! * **No session.** A session token is never stored or reused. A run gets a short-lived token scoped to that run ([`RunTokens`]) that
//!   lives in memory only and is revoked when the run ends.
//! * **Secrets without a person.** A person's own vault is locked when they are away, so an unattended run may use only team vault
//!   entries a team admin marked in the tenant configuration (`flows.unattended`, one entry per secret, with an acknowledgement of
//!   the warning "this secret can be used by scheduled runs while nobody is signed in"). A locked or unmarked secret makes the run
//!   `blocked: needs secret` with a notification, never a silent failure ([`secret_gate`]).
//! * **Personal logins never run unattended.** A node that needs a person's own Claude or Copilot sign-in is refused when the schedule
//!   is saved ([`personal_logins`]); API-key integrations are allowed.
//!
//! How the tenant vault is open with nobody present is the vault link's business (`cloudvaultlink.rs`, `cloudvaultd.rs`: the pod holds
//! its key from the operator at start). This module only asks [`SecretStore::state`] and reports `Locked`.

use crate::cloudroles::{NsRole, Roles, TenantRole};
use crate::tenantconfig::schema::SecretRef;
use crate::tenantconfig::store::Store;
use crate::tenantconfig::vault::{SecretState, SecretStore};
use iyakki_flow::model::Flow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunAs {
    Owner,
    TeamService,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub owner: String,
    pub flow: String,
    pub team: String,
    pub environment: String,
    pub run_as: RunAs,
    /// The owner's role in the team when the schedule was saved: `viewer`, `editor`, `admin` (a tenant admin counts as `admin`).
    pub role_snapshot: String,
    /// For `team-service`: the team admin who chose it.
    pub chosen_by: String,
    pub created: u64,
    /// Why it is paused by the service (not the person's own pause).
    pub paused_reason: Option<String>,
}

impl Schedule {
    pub fn key(&self) -> String {
        format!("{}/{}", self.owner, self.flow)
    }
}

pub fn rank_of(role: &str) -> i32 {
    match role {
        "viewer" => 0,
        "editor" => 1,
        "admin" => 2,
        _ => -1,
    }
}

/// The caller's rank in a team now (a tenant admin counts as team admin); -1: none.
pub fn current_rank(roles: &Roles, team: &str, who: &str) -> i32 {
    if roles.tenant_role(who).is_none() {
        return -1;
    }
    let ns = match roles.ns_role(team, who) {
        Some(NsRole::Viewer) => 0,
        Some(NsRole::Editor) => 1,
        Some(NsRole::Admin) => 2,
        None => -1,
    };
    if roles.tenant_role(who) >= Some(TenantRole::Admin) && roles.namespaces.contains_key(team) { ns.max(2) } else { ns }
}

pub fn role_name(rank: i32) -> &'static str {
    match rank {
        0 => "viewer",
        1 => "editor",
        r if r >= 2 => "admin",
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerCheck {
    Ok,
    Pause(String),
}

/// May this schedule keep running? Uses the snapshot and the current lists.
pub fn check(roles: &Roles, s: &Schedule) -> OwnerCheck {
    if !roles.namespaces.contains_key(&s.team) {
        return OwnerCheck::Pause(format!("the team `{}` no longer exists", s.team));
    }
    match s.run_as {
        RunAs::Owner => {
            let now = current_rank(roles, &s.team, &s.owner);
            if now < 0 {
                return OwnerCheck::Pause(format!("{} is no longer in the team `{}`", s.owner, s.team));
            }
            if now < rank_of(&s.role_snapshot).max(1) {
                return OwnerCheck::Pause(format!("{}'s role in `{}` is lower than when the schedule was saved", s.owner, s.team));
            }
            OwnerCheck::Ok
        }
        RunAs::TeamService => {
            if current_rank(roles, &s.team, &s.chosen_by) < 2 {
                return OwnerCheck::Pause(format!("{} who chose the team identity is no longer an admin of `{}`", s.chosen_by, s.team));
            }
            OwnerCheck::Ok
        }
    }
}

/// Make the record for a new schedule; refused when the person may not own one.
pub fn make_schedule(roles: &Roles, owner: &str, flow: &str, team: &str, environment: &str, run_as: RunAs, now: u64) -> Result<Schedule, String> {
    let rank = current_rank(roles, team, owner);
    if rank < 1 {
        return Err(format!("you need at least the editor role in `{team}` to own a schedule"));
    }
    if run_as == RunAs::TeamService && rank < 2 {
        return Err("only a team admin may choose the team identity".into());
    }
    Ok(Schedule { owner: owner.into(), flow: flow.into(), team: team.into(), environment: environment.into(), run_as, role_snapshot: role_name(rank).into(), chosen_by: if run_as == RunAs::TeamService { owner.into() } else { String::new() }, created: now, paused_reason: None })
}

// ------------------------------------------------------------------------------------------------------------------ registry

pub struct Registry {
    file: std::path::PathBuf,
    lock: Mutex<()>,
}

impl Registry {
    pub fn new(flows_dir: &Path) -> Registry {
        Registry { file: flows_dir.join("schedules.json"), lock: Mutex::new(()) }
    }
    pub fn list(&self) -> Vec<Schedule> {
        super::load_json(&self.file)
    }
    pub fn get(&self, owner: &str, flow: &str) -> Option<Schedule> {
        self.list().into_iter().find(|s| s.owner == owner && s.flow == flow)
    }
    pub fn upsert(&self, s: Schedule) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.list();
        all.retain(|x| x.key() != s.key());
        all.push(s);
        super::save_json(&self.file, &all)
    }
    pub fn remove(&self, owner: &str, flow: &str) -> Result<bool, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.list();
        let n = all.len();
        all.retain(|x| !(x.owner == owner && x.flow == flow));
        super::save_json(&self.file, &all)?;
        Ok(all.len() != n)
    }
    pub fn set_paused(&self, owner: &str, flow: &str, reason: Option<String>) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.list();
        for s in all.iter_mut().filter(|s| s.owner == owner && s.flow == flow) {
            s.paused_reason = reason.clone();
        }
        super::save_json(&self.file, &all)
    }
}

// ------------------------------------------------------------------------------------------------------------------ run tokens

/// Short-lived tokens, each good for one run. Memory only: a restart ends them, and nothing is written to disk.
#[derive(Default)]
pub struct RunTokens {
    map: Mutex<HashMap<String, (String, u64)>>,
}

impl RunTokens {
    pub fn issue(&self, run: &str, now: u64, ttl_secs: u64) -> Result<String, String> {
        let t = format!("rt-{}", crate::cloudkeys::random::<16>().map(|b| crate::cloudkeys::hex(&b))?);
        self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(t.clone(), (run.to_string(), now + ttl_secs.min(86_400)));
        Ok(t)
    }
    pub fn verify(&self, token: &str, run: &str, now: u64) -> bool {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).get(token).is_some_and(|(r, exp)| r == run && *exp > now)
    }
    pub fn revoke_run(&self, run: &str) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, (r, _)| r != run);
    }
}

// ------------------------------------------------------------------------------------------------------------------ secrets

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretUse {
    Allowed,
    /// The run goes `blocked: needs secret`; the text says what to fix.
    Blocked(String),
}

/// The secret names a flow uses (`{{secret.NAME}}` anywhere in a node's settings).
pub fn secrets_in(flow: &Flow) -> Vec<String> {
    let mut out = Vec::new();
    for n in &flow.nodes {
        let t = n.config.to_string();
        let mut rest = t.as_str();
        while let Some(i) = rest.find("secret.") {
            let after = &rest[i + 7..];
            let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
            rest = after;
        }
    }
    out
}

/// May an unattended run of `team` use the secret `name` (a team vault entry)?
pub fn secret_gate(data: &Path, vault: &dyn SecretStore, team: &str, name: &str) -> SecretUse {
    let reference = format!("vault:team/{team}/{name}");
    let Some(r) = SecretRef::parse(&reference) else { return SecretUse::Blocked(format!("needs secret: `{name}` is not a valid secret name")) };
    let marked = Store::new(data).load().map(|cfg| cfg.entries().iter().any(|e| e.item.kind == "flows.unattended" && e.item.enabled && e.item.namespace.as_deref() == Some(team) && e.item.values.get("secret").and_then(|v| v.as_str()) == Some(reference.as_str()))).unwrap_or(false);
    if !marked {
        return SecretUse::Blocked(format!("needs secret: `{name}` is not marked for scheduled runs (a team admin marks it under flows.unattended)"));
    }
    match vault.state(&r) {
        SecretState::Set => SecretUse::Allowed,
        SecretState::Missing => SecretUse::Blocked(format!("needs secret: `{name}` is not in the team vault")),
        SecretState::Locked => SecretUse::Blocked("needs secret: the tenant vault is locked (the operator's key has not been supplied)".into()),
    }
}

/// A run asks for the secret `name`. `None`: a person started it, so the host's usual lookup (that person's own vault) applies. `Some`:
/// nobody is there, so only a team secret that a team admin marked for scheduled runs, of the team of the owner's schedule, is released;
/// a run with no schedule (an event, a hook, a failure trigger) has no team to take a secret from and gets none. The refusal text starts
/// with `needs secret`, which the engine treats as a stop only a person can lift and the service announces to the owner and the admins.
pub fn unattended_secret(data: &Path, vault: &dyn SecretStore, registry: &Registry, user: &str, run: &iyakki_flow::host::RunSecretCx, name: &str) -> Option<Result<String, String>> {
    if run.attended() {
        return None;
    }
    let Some(sched) = registry.get(user, &run.flow_id) else {
        return Some(Err(format!("needs secret: `{name}`: this run was not started by a person and has no schedule that names a team, so it cannot use a secret")));
    };
    match secret_gate(data, vault, &sched.team, name) {
        SecretUse::Blocked(why) => Some(Err(why)),
        SecretUse::Allowed => {
            let Some(r) = SecretRef::parse(&format!("vault:team/{}/{name}", sched.team)) else { return Some(Err(format!("needs secret: `{name}` is not a valid secret name"))) };
            Some(vault.use_secret(&r, "flow.unattended", &mut |v| Ok(v.to_string())))
        }
    }
}

/// Problems that stop a schedule from being saved: personal subscription logins and unmarked secrets.
pub fn unattended_problems(flow: &Flow, data: &Path, vault: &dyn SecretStore, team: &str) -> Vec<String> {
    let mut v = personal_logins(flow);
    for s in secrets_in(flow) {
        if let SecretUse::Blocked(why) = secret_gate(data, vault, team, &s) {
            if why.contains("not marked") || why.contains("not a valid") {
                v.push(why);
            }
        }
    }
    v
}

/// Nodes that need a person's own subscription login (Claude, Copilot) and name no API-key credential.
pub fn personal_logins(flow: &Flow) -> Vec<String> {
    flow.nodes
        .iter()
        .filter(|n| n.kind == "agent")
        .filter_map(|n| {
            let agent = n.config.get("agent").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
            let keyed = n.config.get("credential").and_then(|v| v.as_str()).is_some_and(|c| c.starts_with("vault:"));
            ((agent.contains("claude") || agent.contains("copilot")) && !keyed).then(|| format!("`{}` needs a person's own {} sign-in, which cannot run while nobody is signed in (use an API-key integration)", n.id, if agent.contains("copilot") { "Copilot" } else { "Claude" }))
        })
        .collect()
}
