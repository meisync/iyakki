//! "Launch cloud mode" on a chosen cluster (docs/development/CLOUD-MULTITENANT-PLAN.md, step 2 and section 3a): everything the button does,
//! as a [`Plan`] that is shown first and applied only when it has no [`Plan::blockers`].
//!
//! The cluster is whatever the K8S tile has selected (the local dev k3s or a remote profile); the caller describes it in a
//! [`ClusterRef`] and brings the way to run `kubectl` against it, so this module starts nothing itself and tests use a fake.

use std::path::Path;

use crate::cloudroute::RouteTable;
use crate::cloudtenant::TenantConfig;
use crate::cloudvault::{self, VaultService};

/// Where the image comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageStep {
    /// The cluster can already pull it (a registry the person runs, or an image imported before).
    Existing,
    /// IYAKKI makes its one-file image and pushes it to a plain-HTTP registry (`HOST:PORT/REPO:TAG`; the tenant's image must be that name).
    Push { dest: String },
    /// Only a tar file is made, with the command that loads it on the cluster's node (nothing is imported from here).
    Tar { path: String },
}

/// Which vault the tenant uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultPlan {
    /// None: the vaults stay on the tenant's own volume.
    None,
    /// An address the person gave (`https://...`).
    Remote(String),
    /// The cluster's own vault service is made (`cloudvault.rs`).
    Own,
}

/// The cluster as the K8S tile knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterRef {
    /// Its name as shown (`local k3s`, a context name).
    pub name: String,
    /// The tile's own judgement (`devcluster::ClusterProfile::looks_like_production`).
    pub looks_like_production: bool,
    /// Why changes are blocked on it right now (`devcluster::read_only_reason`), when they are.
    pub read_only: Option<String>,
    /// The person allowed changes on a production-looking cluster for this session.
    pub changes_allowed: bool,
}

#[derive(Clone, Debug)]
pub struct Launch {
    pub tenant: TenantConfig,
    pub vault: VaultPlan,
    pub routes: RouteTable,
    pub image: ImageStep,
    pub cluster: ClusterRef,
}

/// What would be done, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// (file name, YAML) applied in this order.
    pub documents: Vec<(String, String)>,
    /// Things to know; they do not stop the launch.
    pub notes: Vec<String>,
    /// Things that stop it.
    pub blockers: Vec<String>,
    pub image: Option<ImageStep>,
}

pub fn plan(l: &Launch) -> Plan {
    let mut p = Plan { image: Some(l.image.clone()), ..Plan::default() };
    let mut tenant = l.tenant.clone();
    match &l.vault {
        VaultPlan::None => p.notes.push("No vault service: the tenant's vaults stay on its own volume.".into()),
        VaultPlan::Remote(u) => tenant.vault_url = Some(u.clone()),
        VaultPlan::Own => {
            tenant.vault_url = Some(cloudvault::endpoint());
            match cloudvault::manifests(&VaultService { image: tenant.image.clone(), ..Default::default() }) {
                Ok(y) => p.documents.push(("00-vault-service.yaml".into(), y)),
                Err(e) => p.blockers.push(format!("vault service: {e}")),
            }
            if !cloudvault::SERVICE_PROGRAM_BUILT {
                p.blockers.push("the cluster's own vault service cannot be started yet: its program (`iyakki vault serve`) is not built. Give a remote vault address, or launch without a vault service.".into());
            }
        }
    }
    match tenant.check().and_then(|_| crate::cloudtenant::manifests(&tenant)) {
        Ok(y) => p.documents.push(("10-tenant.yaml".into(), y)),
        Err(e) => p.blockers.push(format!("tenant: {e}")),
    }
    if !l.routes.routes.is_empty() {
        match crate::cloudroute::ingress(&tenant.namespace(), &l.routes) {
            Ok(y) => p.documents.push(("20-routes.yaml".into(), y)),
            Err(e) => p.blockers.push(format!("routes: {e}")),
        }
    }
    if let Some(why) = &l.cluster.read_only {
        p.blockers.push(format!("{} does not accept changes now: {why}", l.cluster.name));
    } else if l.cluster.looks_like_production && !l.cluster.changes_allowed {
        p.blockers.push(format!("{} looks like a production cluster: allow changes in the K8S tile first, then launch again.", l.cluster.name));
    }
    match &l.image {
        ImageStep::Existing => p.notes.push(format!("The image {} must already be pullable by the cluster.", tenant.image)),
        ImageStep::Push { dest } => {
            if dest != &tenant.image {
                p.blockers.push(format!("the image pushed ({dest}) is not the one the tenant runs ({}); make them the same", tenant.image));
            }
        }
        ImageStep::Tar { path } => p.notes.push(format!("{path} is written, not loaded: load it on the cluster's node (for k3s: `k3s ctr images import {path}`) before the pod can start.")),
    }
    p.notes.push(format!("The accounts' first passwords are not in the manifests: {}", crate::cloudtenant::secret_command(&tenant)));
    p
}

/// The text the confirmation dialog shows: blockers first, then what will be made, then the notes.
pub fn preview(l: &Launch, p: &Plan) -> String {
    let mut s = format!("Launch cloud mode \"{}\" on {}\n\n", l.tenant.tenant, l.cluster.name);
    for b in &p.blockers {
        s.push_str(&format!("BLOCKED: {b}\n"));
    }
    if !p.blockers.is_empty() {
        s.push('\n');
    }
    for (n, y) in &p.documents {
        s.push_str(&format!("--- {n} ---\n{y}\n"));
    }
    for n in &p.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

/// Run the plan: refuse when blocked; write each document to `work` and `kubectl apply -f` it through `kubectl` (which the caller binds to
/// the chosen cluster). The image step runs first for `Push` (through `push`). Returns the lines to show.
pub fn execute(l: &Launch, p: &Plan, work: &Path, kubectl: &mut dyn FnMut(&[String]) -> Result<String, String>, push: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<Vec<String>, String> {
    if !p.blockers.is_empty() {
        return Err(format!("not launched: {}", p.blockers.join("; ")));
    }
    let mut said: Vec<String> = Vec::new();
    if let Some(ImageStep::Push { dest }) = &p.image {
        said.push(push(dest)?);
    }
    std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    for (name, y) in &p.documents {
        let f = work.join(name);
        std::fs::write(&f, y).map_err(|e| format!("{}: {e}", f.display()))?;
        let out = kubectl(&["apply".into(), "-f".into(), f.display().to_string()]).map_err(|e| format!("{name}: {e}"))?;
        said.push(out.trim_end().to_string());
    }
    said.push(crate::cloudtenant::reach_text(&l.tenant));
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudroute::{Access, Route, Scheme, Target};

    fn cluster() -> ClusterRef {
        ClusterRef { name: "local k3s".into(), looks_like_production: false, read_only: None, changes_allowed: true }
    }
    fn launch() -> Launch {
        Launch { tenant: TenantConfig::example("acme"), vault: VaultPlan::None, routes: RouteTable::default(), image: ImageStep::Existing, cluster: cluster() }
    }

    #[test]
    fn a_plain_launch_makes_the_tenant_and_blocks_nothing() {
        let l = launch();
        let p = plan(&l);
        assert!(p.blockers.is_empty(), "{:?}", p.blockers);
        assert_eq!(p.documents.len(), 1);
        assert!(p.documents[0].1.contains("namespace: iyakki-acme") || p.documents[0].1.contains("name: iyakki-acme"));
        let t = preview(&l, &p);
        assert!(t.contains("Launch cloud mode \"acme\" on local k3s") && t.contains("--- 10-tenant.yaml ---") && t.contains("create secret generic iyakki-accounts"), "{t}");
    }

    #[test]
    fn a_production_looking_cluster_waits_until_changes_are_allowed_and_a_read_only_one_never_launches() {
        let mut l = launch();
        l.cluster = ClusterRef { name: "prod-eu".into(), looks_like_production: true, read_only: None, changes_allowed: false };
        assert!(plan(&l).blockers.iter().any(|b| b.contains("production")));
        l.cluster.changes_allowed = true;
        assert!(plan(&l).blockers.is_empty());
        l.cluster.read_only = Some("it is shared read-only".into());
        let p = plan(&l);
        assert!(p.blockers.iter().any(|b| b.contains("does not accept changes") && b.contains("shared read-only")));
        let e = execute(&l, &p, &std::env::temp_dir().join("iyakki-launch-ro"), &mut |_| panic!("nothing is applied"), &mut |_| panic!("nothing is pushed")).unwrap_err();
        assert!(e.starts_with("not launched"), "{e}");
    }

    #[test]
    fn the_cluster_s_own_vault_goes_first_and_no_longer_waits_for_a_program() {
        let mut l = launch();
        l.vault = VaultPlan::Own;
        let p = plan(&l);
        assert_eq!(p.documents[0].0, "00-vault-service.yaml", "the vault goes first");
        assert!(p.documents[1].1.contains("IYAKKI_VAULT_URL"), "the tenant is told where it is");
        assert!(!p.blockers.iter().any(|b| b.contains("vault")), "{:?}", p.blockers);
        assert!(p.documents[0].1.contains("\"vault\", \"serve\""), "the pod runs the program that exists");
    }

    #[test]
    fn a_remote_vault_address_goes_to_the_pod_and_a_bad_one_blocks() {
        let mut l = launch();
        l.vault = VaultPlan::Remote("https://vault.example.org".into());
        let p = plan(&l);
        assert!(p.blockers.is_empty() && p.documents[0].1.contains("https://vault.example.org"), "{:?}", p.blockers);
        l.vault = VaultPlan::Remote("http://nope".into());
        assert!(plan(&l).blockers.iter().any(|b| b.starts_with("tenant:")));
    }

    #[test]
    fn routes_become_an_ingress_document_and_an_image_push_must_name_the_tenants_image() {
        let mut l = launch();
        l.routes = RouteTable { routes: vec![Route { host: "app.example.org".into(), path_prefix: "/".into(), target: Target::Service { name: "iyakki".into(), port: 8080 }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None }], ingress_class: None };
        l.image = ImageStep::Push { dest: "10.0.0.5:5000/iyakki:1".into() };
        let p = plan(&l);
        assert!(p.documents.iter().any(|d| d.0 == "20-routes.yaml"));
        assert!(p.blockers.iter().any(|b| b.contains("not the one the tenant runs")), "{:?}", p.blockers);
        l.tenant.image = "10.0.0.5:5000/iyakki:1".into();
        assert!(plan(&l).blockers.is_empty());
        l.image = ImageStep::Tar { path: "iyakki-image.tar".into() };
        assert!(plan(&l).notes.iter().any(|n| n.contains("k3s ctr images import")));
    }

    #[test]
    fn execute_pushes_then_applies_each_document_in_order_through_the_given_cluster() {
        let mut l = launch();
        l.tenant.image = "10.0.0.5:5000/iyakki:1".into();
        l.image = ImageStep::Push { dest: "10.0.0.5:5000/iyakki:1".into() };
        l.routes = RouteTable { routes: vec![Route { host: "app.example.org".into(), path_prefix: "/".into(), target: Target::Service { name: "iyakki".into(), port: 8080 }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None }], ingress_class: None };
        let p = plan(&l);
        let work = std::env::temp_dir().join(format!("iyakki-launch-{}", std::process::id()));
        let mut calls: Vec<String> = Vec::new();
        let mut pushed: Vec<String> = Vec::new();
        let said = execute(&l, &p, &work, &mut |a| { calls.push(a.join(" ")); Ok("created".into()) }, &mut |d| { pushed.push(d.into()); Ok(format!("pushed {d}")) }).unwrap();
        assert_eq!(pushed, vec!["10.0.0.5:5000/iyakki:1"]);
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].contains("10-tenant.yaml") && calls[1].contains("20-routes.yaml") && calls.iter().all(|c| c.starts_with("apply -f ")));
        assert!(said[0].starts_with("pushed") && said.last().unwrap().len() > 10);
        // a failing apply stops with the document's name
        let e = execute(&l, &p, &work, &mut |_| Err("forbidden".into()), &mut |_| Ok(String::new())).unwrap_err();
        assert!(e.contains("10-tenant.yaml") && e.contains("forbidden"), "{e}");
    }
}
