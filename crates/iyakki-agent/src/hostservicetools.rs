//! The cloud assistant's way to publish a team's local model host as a cluster Service (F-TCFG-14): `deploy_team_host` asks on a card
//! every time (the Service YAML is on the card) and then calls [`crate::tenantconfig::hostservice::apply_host_service`], which makes the
//! deploy decision first (a second person, a frozen environment, no binding: the plain reason comes back) and only then applies the
//! YAML. The YAML holds an address and a port, never the token. The tool is offered in the cloud profile only.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const TOOL: &str = "deploy_team_host";

pub const PROMPT_RULE: &str = "\nTEAM MODEL HOST: deploy_team_host publishes a team's local model host (a models.local entry) as a Service in the cluster, so team runs can reach it by name. It asks the person on a card that shows the Service, and the deploy rules of the team apply (a second person may have to approve; the plain reason is given back).";

/// What the tool needs (a fake in tests).
pub trait Backend: Send + Sync {
    /// The Service the host would get, as YAML (no side effect); the plain reason when there is none.
    fn preview(&self, team: &str, name: &str) -> Result<String, String>;
    /// Apply it for `environment`; the cluster's answer, or the plain reason it was not applied.
    fn publish(&self, team: &str, name: &str, environment: &str) -> Result<String, String>;
}

struct Args {
    team: String,
    name: String,
    environment: String,
}

fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn args(a: &Value) -> Result<Args, String> {
    let get = |k: &str| a.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
    let (team, name) = (get("team"), get("name"));
    let environment = Some(get("environment")).filter(|e| !e.is_empty()).unwrap_or_else(|| "dev".into());
    for (k, v) in [("team", &team), ("name", &name), ("environment", &environment)] {
        if !label_ok(v) {
            return Err(format!("{k} must be a short name of letters, digits, '-', '_' or '.'"));
        }
    }
    Ok(Args { team, name, environment })
}

pub struct TeamHostTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl TeamHostTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> TeamHostTools {
        TeamHostTools { inner, backend }
    }

    fn card(&self, a: &Value) -> Result<String, String> {
        let a = args(a)?;
        let yaml = self.backend.preview(&a.team, &a.name)?;
        Ok(format!("Publish the local model host \"{}\" of team {} as a Service in the cluster (environment {}). The deploy rules of the team decide whether it is applied.\n{}", a.name, a.team, a.environment, yaml))
    }
}

impl Tools for TeamHostTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != TOOL);
        v.push(ToolDef {
            spec: ToolSpec {
                name: TOOL.into(),
                description: "Publish a team's local model host (a models.local entry) as a Service in the cluster. The person approves a card every time; the team's deploy rules apply.".into(),
                schema: json!({"type": "object", "properties": {
                    "team": {"type": "string", "description": "The team (namespace) that owns the host."},
                    "name": {"type": "string", "description": "The name of the models.local entry."},
                    "environment": {"type": "string", "description": "The environment to deploy to (default dev)."}
                }, "required": ["team", "name"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, a: &Value) -> Result<String, String> {
        if name != TOOL {
            return self.inner.call(name, a);
        }
        let p = args(a)?;
        self.backend.publish(&p.team, &p.name, &p.environment)
    }

    fn describe(&self, name: &str, a: &Value) -> String {
        if name != TOOL {
            return self.inner.describe(name, a);
        }
        match args(a) {
            Ok(p) => format!("Publish the model host {} of team {} as a Service ({})", p.name, p.team, p.environment),
            Err(e) => format!("{name} (invalid: {e})"),
        }
    }

    fn preview(&self, name: &str, a: &Value) -> Option<String> {
        if name == TOOL { self.card(a).ok() } else { self.inner.preview(name, a) }
    }

    fn refusal(&self, name: &str, a: &Value) -> Option<String> {
        if name == TOOL { self.card(a).err() } else { self.inner.refusal(name, a) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        name != TOOL && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, a: &Value) -> Option<String> {
        if name == TOOL { Some("publishing a model host as a Service asks every time".into()) } else { self.inner.grant_blocker(name, a) }
    }
    fn grant_scope(&self, name: &str, a: &Value) -> String {
        self.inner.grant_scope(name, a)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, a: &Value, ok: bool) {
        self.inner.note_external(name, a, ok)
    }
}

struct Sink(Arc<dyn iyakki_core_api::AuditSink>);

impl crate::tenantconfig::api::ConfigAudit for Sink {
    fn record(&self, who: &str, event: &str, detail: &str) {
        self.0.record(who, event, detail);
    }
}

/// The real backend: the tenant data folder, the person who asks and the machine's cluster.
pub struct RealBackend {
    pub dir: PathBuf,
    pub user: String,
    pub audit: Arc<dyn iyakki_core_api::AuditSink>,
    pub applier: Arc<dyn crate::tenantconfig::hostservice::ServiceApplier + Send + Sync>,
}

impl RealBackend {
    /// The roles of the tenant, and whether the person belongs to the team (or administers the tenant).
    fn roles_for(&self, team: &str) -> Result<crate::cloudroles::Roles, String> {
        let roles = crate::cloudaccess::Access::new(&self.dir).roles().ok().flatten().ok_or("this machine has no teams set up")?;
        let admin = roles.tenant_role(&self.user).is_some_and(|x| x >= crate::cloudroles::TenantRole::Admin);
        if roles.ns_role(team, &self.user).is_none() && !admin {
            return Err("no such team".into());
        }
        Ok(roles)
    }
}

impl Backend for RealBackend {
    fn preview(&self, team: &str, name: &str) -> Result<String, String> {
        self.roles_for(team)?;
        let sink = Sink(self.audit.clone());
        let env = crate::tenantconfig::api::Env { dir: &self.dir, vault: crate::tenantconfig::vault::for_user(&self.dir, &self.user), audit: &sink };
        crate::tenantconfig::hostservice::host_service(&env, team, name)
    }

    fn publish(&self, team: &str, name: &str, environment: &str) -> Result<String, String> {
        let roles = self.roles_for(team)?;
        let sink = Sink(self.audit.clone());
        let env = crate::tenantconfig::api::Env { dir: &self.dir, vault: crate::tenantconfig::vault::for_user(&self.dir, &self.user), audit: &sink };
        let rank = crate::tenantconfig::api::Caller { user: &self.user, roles: Some(&roles) }.rank(team);
        let guest = roles.tenant_role(&self.user).is_none();
        let ask = crate::tenantconfig::enforce::DeployAsk { team, environment, requester: &self.user, requester_rank: rank, approval: None, promoted_from: None, registry: None, roles: &roles, guest };
        crate::tenantconfig::hostservice::apply_host_service(&env, &ask, name, self.applier.as_ref(), crate::clock::now_unix().max(0) as u64)
    }
}

#[cfg(test)]
#[path = "hostservicetools_tests.rs"]
mod tests;
