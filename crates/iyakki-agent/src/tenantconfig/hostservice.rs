//! A team's local model host as a cluster Service (F-TCFG-14): the YAML that lets pods of the team's namespace reach the host by a
//! stable in-cluster name, `llm-<name>` on the port of the entry.
//!
//! An address host (IPv4 or IPv6 literal) becomes a selector-less Service plus an `Endpoints` object that points at it; a DNS name becomes
//! an `ExternalName` Service. The text is built here and only here; nothing is applied (applying it needs a cluster and goes through the
//! deploy gate like every other change to a team's environment). Names are checked as DNS labels so no entry can inject YAML.
//! [`apply_host_service`] applies it through the [`ServiceApplier`] seam after the same deploy decision as every other change to a
//! team's cluster (`enforce::k3s_decide`); the real applier is the machine's own cluster (`Cluster::apply_yaml`).

use super::api::Env;
use super::resolve::resolve;
use super::schema;
use super::store::Store;

/// What the Service needs from the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostService {
    pub namespace: String,
    pub name: String,
    pub host: String,
    pub port: u16,
}

fn dns_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !s.starts_with('-') && !s.ends_with('-')
}

fn dns_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(dns_label)
}

/// The YAML (one document, `---` separated) for `h`.
pub fn service_yaml(h: &HostService) -> Result<String, String> {
    if !dns_label(&h.namespace) || !dns_label(&h.name) {
        return Err("the team or the entry name is not a valid cluster name".into());
    }
    if h.port == 0 {
        return Err("the entry has no port".into());
    }
    let svc = format!("llm-{}", h.name);
    let head = format!("apiVersion: v1\nkind: Service\nmetadata:\n  name: {svc}\n  namespace: {}\n  labels:\n    iyakki.io/managed-by: tenant-config\n    iyakki.io/local-model-host: {}\n", h.namespace, h.name);
    if h.host.parse::<std::net::IpAddr>().is_ok() {
        let ip = &h.host;
        return Ok(format!(
            "{head}spec:\n  ports:\n    - name: api\n      port: {p}\n      targetPort: {p}\n      protocol: TCP\n---\napiVersion: v1\nkind: Endpoints\nmetadata:\n  name: {svc}\n  namespace: {ns}\nsubsets:\n  - addresses:\n      - ip: \"{ip}\"\n    ports:\n      - name: api\n        port: {p}\n        protocol: TCP\n",
            p = h.port,
            ns = h.namespace
        ));
    }
    if !dns_name(&h.host.to_ascii_lowercase()) {
        return Err("the host of the entry is neither an address nor a valid DNS name".into());
    }
    Ok(format!("{head}spec:\n  type: ExternalName\n  externalName: {}\n  ports:\n    - name: api\n      port: {p}\n      targetPort: {p}\n      protocol: TCP\n", h.host.to_ascii_lowercase(), p = h.port))
}

/// The Service of the named `models.local` entry of a team: `Err` while the entry is missing or switched off.
pub fn host_service(env: &Env, team: &str, name: &str) -> Result<String, String> {
    let spec = schema::kind("models.local").ok_or("unknown kind")?;
    let cfg = Store::new(env.dir).load().map_err(|_| "the tenant configuration cannot be read")?;
    let entries = cfg.entries();
    let eff = resolve(spec, &cfg.layers(&entries, "models.local", Some(team), name));
    if !eff.exists || !eff.enabled {
        return Err("this local model host is not switched on".into());
    }
    let host = eff.values.get("host").and_then(|v| v.as_str()).unwrap_or("");
    let port = eff.values.get("port").and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok()).unwrap_or(0);
    service_yaml(&HostService { namespace: team.to_string(), name: name.to_string(), host: host.to_string(), port })
}

/// What applying a Service needs from a cluster; the machine's own cluster is one ([`crate::cluster::Cluster`]), tests use a recorder.
pub trait ServiceApplier {
    /// Apply `yaml` (the namespace is created first when it is missing); the text is what the cluster said.
    fn apply_yaml(&self, namespace: &str, yaml: &str) -> Result<String, String>;
}

impl ServiceApplier for crate::cluster::Cluster {
    fn apply_yaml(&self, namespace: &str, yaml: &str) -> Result<String, String> {
        let ns = format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {namespace}\n---\n");
        self.apply_yaml_text(&format!("{ns}{yaml}"))
    }
}

/// Publish the team's local model host as a Service in the cluster of `ask.environment`, through [`super::runtime::deploy_through`]: the deploy
/// decision first (allowed, or the plain reason it is not: no binding, a frozen environment, a person who may not deploy); when the binding
/// wants a second person the request is filed (or found) and the answer names its id and the role that can decide, and the Service is applied
/// only once that person has approved and the person asks again (an approval is spent by one apply); then the YAML of [`host_service`] goes
/// through `cluster`. Audited as `tenant.k3s.deploy` and `tenant.deploy.*`; the YAML holds an address and a port, never the token.
pub fn apply_host_service(env: &Env, ask: &super::enforce::DeployAsk, name: &str, cluster: &dyn ServiceApplier, now: u64) -> Result<String, String> {
    let yaml = host_service(env, ask.team, name)?;
    super::runtime::deploy_through(env, ask, now, &mut |_approved_by| cluster.apply_yaml(ask.team, &yaml)).map_err(|e| if e.starts_with("the Service was not applied") { e } else { format!("the Service was not applied: {e}") })
}
