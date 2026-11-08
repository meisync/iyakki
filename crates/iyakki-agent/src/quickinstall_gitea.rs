//! The Quick install item "Gitea (versioning for your work)": Platform services' Gitea (`platform/gitea.rs`) put into the managed
//! cluster by a Quick install job, with the two things a job cannot do by itself done by the shell afterwards.
//!
//! * The objects are the ones Platform services builds ([`platform::gitea::Gitea::build`]): this file only chooses the options (namespace
//!   `iyakki-gitea`, ClusterIP, a port on this machine's loopback, the pinned image) and adds the namespace, the Secrets and the record.
//!   Nothing is copied from it.
//! * The image is pinned by tag AND digest ([`IMAGE`], checked on Docker Hub's registry API for the multi-arch index of that tag).
//! * Reaching it: the Service stays ClusterIP; the pod publishes its web port as a `hostPort` bound to 127.0.0.1 of the node
//!   (`http://127.0.0.1:3300`). This machine (git, the IDE, the browser) can use it; no other computer can. Git over HTTP with a token
//!   for the first cut; SSH is not published.
//! * The administrator password is generated in the job, goes into the Secret `gitea-admin` through a 0600 file, and is never printed.
//!   The job also makes the API token IYAKKI uses (scopes `write:repository,read:user`) inside the pod and parks it in the Secret
//!   [`TOKEN_SECRET`]. A job has no vault, so the shell ([`complete`]) moves both into the person's vault (`svc.<instance>.admin-password`,
//!   `svc.<instance>.token`: the names Platform services uses, so its tools find them), deletes the token Secret, and adds the Git
//!   connection [`CONNECTION_NAME`] (once; a second run changes nothing) whose token is the same vault entry.
//! * The record `installed-services-<user>.json` is Platform services' own, so `platform_list`, `git_service_status`, Settings → Dev
//!   cluster and Connect workspace see the instance, and Remove works on it.

use std::sync::Arc;

use zeroize::Zeroizing;

use super::*;
use crate::gitconn::{Conns, Connection, Kind, Method};
use crate::platform::gitea::{self as pg, Gitea, LOOPBACK_OPTION};
use crate::platform::installed::{self, ClusterRef, Installed};
use crate::platform::{BuildCtx, Built, Options, ServiceDef, LABEL_MANAGED, LABEL_SERVICE};

pub const NAMESPACE: &str = "iyakki-gitea";
pub const VERSION: &str = "1.24.5";
/// Tag and digest of the image (the multi-arch OCI index of `gitea/gitea:1.24.5-rootless`, amd64 and arm64, read from Docker Hub's
/// registry API on 2026-10-25). The cluster pulls it when the person clicks Install; IYAKKI does not ship it.
pub const IMAGE: &str = "docker.io/gitea/gitea:1.24.5-rootless@sha256:35951356de5321229c41405d6f1dcbe5d51883ca4538ea334d989298b8286611";
/// The loopback ports tried, in order, for the first install.
pub const PORT_FIRST: u16 = 3300;
pub const PORT_LAST: u16 = 3309;
/// Holds the API token between the job and the shell's handoff into the vault.
pub const TOKEN_SECRET: &str = "gitea-iyakki-token";
/// The request file the job leaves for the shell.
pub const REQUEST: &str = "gitea-vault";
/// The Git connection (Settings → Git, the GIT tile, the AI IDE) this item adds.
pub const CONNECTION_NAME: &str = "Local Gitea";
/// The IDE's offer when the project has no remote.
pub const OFFER_LABEL: &str = "Put this project under version control on Local Gitea";
pub const NOT_INSTALLED: &str = "Gitea is not installed yet. Open Store \u{2192} Setup, install \"Gitea (versioning for your work)\", then press this again.";

// ------------------------------------------------------------------------------------------------ what it deploys

/// The options of this install, checked by Platform services' own rules.
pub fn options(port: u16) -> Result<Options, String> {
    let mut o = Options::default();
    o.set("namespace", NAMESPACE);
    o.set("version", VERSION);
    o.set("exposure", "ClusterIP");
    o.set(LOOPBACK_OPTION, &port.to_string());
    crate::platform::validate_options(&Gitea, &o)
}

/// Platform services' objects for this install (without the namespace, which has no `namespace:` fields yet: see [`with_namespace`]).
pub fn build(port: u16, instance: &str) -> Result<(Built, Options), String> {
    let opts = options(port)?;
    let ctx = BuildCtx { namespace: NAMESPACE.into(), opts: opts.clone(), version: VERSION.into(), images: vec![IMAGE.into()], storage_class: None, host: "localhost".into(), http_port: 0, ssh_port: 0, vault_prefix: format!("svc.{instance}"), instance: instance.into() };
    Ok((Gitea.build(&ctx)?, opts))
}

/// The namespace, labelled as IYAKKI's and as Gitea's (what Platform services' guards look for).
pub fn namespace_manifest() -> String {
    format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NAMESPACE}\n  labels:\n    {LABEL_MANAGED}: iyakki\n    {LABEL_SERVICE}: gitea\n")
}

/// Put `namespace: <ns>` into every object of a multi-document manifest (Platform services applies with `-n`; a Quick install job
/// applies the file as it is).
pub fn with_namespace(manifest: &str, ns: &str) -> String {
    let mut out = String::new();
    for doc in manifest.split("---\n").filter(|d| !d.trim().is_empty()) {
        let mut done = false;
        for line in doc.lines() {
            out.push_str(line);
            out.push('\n');
            if !done && line == "metadata:" {
                out.push_str(&format!("  namespace: {ns}\n"));
                done = true;
            }
        }
        out.push_str("---\n");
    }
    out.trim_end_matches("---\n").to_string()
}

pub fn web_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The line that goes into the job's result, the Setup row and the marker: the addresses, never a token.
pub fn detail(port: u16, admin: &str) -> String {
    format!("web {} \u{b7} clone http://127.0.0.1:{port}/{admin}/<repo>.git (git over HTTP with a token) \u{b7} this machine only", web_url(port))
}

/// A Gitea of this install: the record carries a loopback port.
pub fn ours(r: &Installed) -> bool {
    r.service == "gitea" && !r.connected && r.namespace == NAMESPACE && Gitea::loopback_of(&r.options).is_some()
}

pub fn admin_vault(rec: &Installed) -> String {
    rec.vault.iter().find(|v| v.ends_with(".admin-password")).cloned().unwrap_or_else(|| format!("svc.{}.admin-password", rec.id))
}

pub fn token_vault(rec: &Installed) -> String {
    rec.token_vault().map(str::to_string).unwrap_or_else(|| format!("svc.{}.token", rec.id))
}

/// Setup row: installed when the marker is there and a Gitea record is (a Remove in Platform services takes the record away); a Gitea
/// deployed from Platform services alone counts too.
pub fn presence(data: &Path) -> Presence {
    let mut any = false;
    for e in std::fs::read_dir(data).into_iter().flatten().flatten() {
        if !e.file_name().to_string_lossy().starts_with("installed-services-") {
            continue;
        }
        let recs: Vec<Installed> = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        any |= recs.iter().any(|r| r.service == "gitea" && !r.connected);
    }
    match (marked(data, Item::Gitea), any) {
        (Some(d), true) => Presence::Installed(d),
        (None, true) => Presence::Installed("deployed in the cluster (Platform services)".into()),
        _ => Presence::Missing,
    }
}

// ------------------------------------------------------------------------------------------------ the job

fn show(env: &mut dyn StepEnv, out: &str) {
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(8) {
        env.say(&format!("  {}", l.trim()));
    }
}

fn first_line(t: &str) -> String {
    t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// Run a command in the pod, trying again while the service is still starting (its database is made on the first start).
fn exec_retrying(env: &mut dyn StepEnv, script: &str, arg0: &str, ok_if: &dyn Fn(&str) -> bool) -> Result<Option<String>, String> {
    let mut last = String::new();
    for n in 0..4 {
        if n > 0 {
            env.sleep(Duration::from_secs(5));
        }
        match env.pod_exec(NAMESPACE, "gitea", "gitea", script, arg0) {
            Ok(out) => return Ok(Some(out)),
            Err(e) if ok_if(&e) => return Ok(None),
            Err(e) => last = first_line(&e),
        }
    }
    Err(last)
}

/// Step 8: a Gitea that Platform services deployed (reachable from this machine) is used as it is; one that is only reachable inside the
/// cluster is explained.
fn use_existing(env: &mut dyn StepEnv, other: &Installed) -> Result<(), String> {
    let web = other.web_url().unwrap_or("").to_string();
    if other.options.get("exposure") == "ClusterIP" || web.is_empty() {
        return Err(format!("a Gitea from Platform services is installed already (namespace {}, instance {}) but it is only reachable inside the cluster. Remove it in Settings \u{2192} Dev cluster \u{2192} Platform services (its data is kept unless you ask), or deploy it again with the NodePort exposure, then press Install again", other.namespace, other.id));
    }
    env.say(&format!("[1/2] a Gitea from Platform services is installed already ({web}, instance {}): it is used as it is, nothing was changed in the cluster", other.id));
    env.request_after(REQUEST)?;
    env.say("[2/2] asked IYAKKI to add it as the Git connection \"Local Gitea\" (its token is already in your vault)");
    let detail = format!("web {web} (from Platform services)");
    crate::quickinstall::mark_verified(env, Item::Gitea, &detail)?;
    env.say(&format!("done: {detail}"));
    Ok(())
}

/// What the cluster holds of a Gitea, whatever the records say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveGitea {
    /// The type of the Service `gitea-http`: `NodePort`, `ClusterIP`, ... (empty when the Service is missing).
    pub exposure: String,
    pub http_node_port: u16,
    pub ssh_node_port: u16,
}

/// A Gitea that Platform services deployed (same namespace, Deployment and volume) whose record is missing, and that is open on the
/// network (NodePort): its record is written from what the cluster holds, nothing in the cluster is changed (so nothing is widened or
/// narrowed), and the API token is made for the vault handoff as usual.
fn adopt_nodeport(env: &mut dyn StepEnv, cluster: ClusterRef, records: &[Installed], live: &LiveGitea) -> Result<(), String> {
    let instance = installed::instance_id("gitea", NAMESPACE, &format!("{}#{}", cluster.kubeconfig, cluster.context), records);
    let mut o = Options::default();
    o.set("namespace", NAMESPACE);
    o.set("version", VERSION);
    o.set("exposure", "NodePort");
    o.set("http_node_port", &live.http_node_port.to_string());
    if live.ssh_node_port != 0 {
        o.set("ssh_node_port", &live.ssh_node_port.to_string());
    }
    let opts = crate::platform::validate_options(&Gitea, &o)?;
    let ctx = BuildCtx { namespace: NAMESPACE.into(), opts: opts.clone(), version: VERSION.into(), images: vec![IMAGE.into()], storage_class: None, host: "localhost".into(), http_port: live.http_node_port, ssh_port: live.ssh_node_port, vault_prefix: format!("svc.{instance}"), instance: instance.clone() };
    let built = Gitea.build(&ctx)?;
    env.say(&format!("[1/3] a Gitea from Platform services is in the cluster already (namespace {NAMESPACE}, open on the network at port {}) but no record of it exists here: it is adopted as it is, nothing is changed in the cluster", live.http_node_port));
    let name = format!("iyakki-{}", crate::clock::now_unix());
    let out = exec_retrying(env, pg::TOKEN_SCRIPT, &name, &|_| false).map_err(|e| format!("creating the API token failed: {e}"))?.unwrap_or_default();
    let token: Zeroizing<String> = pg::token_from_output(&out)?;
    env.put_secret(NAMESPACE, TOKEN_SECRET, "token", &token, &instance)?;
    env.say("[2/3] made an API token for IYAKKI (it is not shown; it goes into your vault)");
    let now = crate::clock::now_unix();
    let rec = Installed { id: instance, service: "gitea".into(), title: "Gitea".into(), version: VERSION.into(), kind: "git".into(), cluster: Some(cluster), namespace: NAMESPACE.into(), created_namespace: false, connected: false, options: opts, urls: built.urls, vault: vec![], installed_at: now, updated_at: now, workspaces: vec![], note: "adopted from the cluster: the version in this record is the one Quick install pins, not read from the pod".into() };
    let web = rec.web_url().unwrap_or("").to_string();
    env.save_service(rec)?;
    env.request_after(REQUEST)?;
    let detail = format!("web {web} (adopted from Platform services)");
    crate::quickinstall::mark_verified(env, Item::Gitea, &detail)?;
    env.say(&format!("[3/3] done: {detail}"));
    Ok(())
}

pub fn run(env: &mut dyn StepEnv, wait: Duration, poll: Duration) -> Result<(), String> {
    env.say(&format!("[0/9] Gitea runs in k3s: checking the cluster first (licence: {})", Item::Gitea.licence()));
    wait_for_cluster(env, wait, poll)?;
    let cluster: ClusterRef = env.cluster_ref()?;
    let records = env.service_records();
    if let Some(other) = records.iter().find(|r| r.service == "gitea" && !r.connected && !ours(r)) {
        return use_existing(env, other);
    }
    let prev = records.iter().find(|r| ours(r)).cloned();
    if prev.is_none() && records.iter().all(|r| !(r.service == "gitea" && !r.connected)) {
        if let Some(live) = env.live_gitea() {
            if live.exposure == "NodePort" && live.http_node_port != 0 {
                return adopt_nodeport(env, cluster, &records, &live);
            }
            env.say("a Gitea (Deployment gitea, volume gitea-data) is in the cluster already without a record: its volume and secrets are kept and it is recorded below");
        }
    }
    let instance = prev.as_ref().map(|r| r.id.clone()).unwrap_or_else(|| installed::instance_id("gitea", NAMESPACE, &format!("{}#{}", cluster.kubeconfig, cluster.context), &records));
    let port = match prev.as_ref().and_then(|r| r.options.get(LOOPBACK_OPTION).parse::<u16>().ok()) {
        Some(p) => p,
        None => (PORT_FIRST..=PORT_LAST).find(|p| env.port_free(*p)).ok_or_else(|| format!("the ports {PORT_FIRST} to {PORT_LAST} on this machine are all in use, so Gitea has no address to be reached at. Stop the program that listens on {PORT_FIRST}, then press Install again"))?,
    };
    let (built, opts) = build(port, &instance)?;
    let sec = built.secrets.first().ok_or("Platform services' Gitea names no administrator Secret")?;
    let entry = sec.entries.first().ok_or("Platform services' Gitea names no administrator password")?;
    let admin = opts.get("admin_user").to_string();
    let web = web_url(port);

    env.say(&format!("[1/9] k3s is Ready; making the namespace {NAMESPACE}"));
    let out = env.kubectl_apply("gitea-namespace", &namespace_manifest())?;
    show(env, &out);
    match env.generated_secret(NAMESPACE, &sec.name, &entry.key, &instance)? {
        true => env.say(&format!("[2/9] made the administrator password and the Secret {} (the password is not shown; it goes into your vault)", sec.name)),
        false => env.say(&format!("[2/9] the Secret {} is there already; its password is kept", sec.name)),
    }
    env.say(&format!("[3/9] applying Platform services' Gitea objects (rootless, unprivileged, ClusterIP; the web port is published on 127.0.0.1:{port} of this machine only; image pinned by digest, 10 GB volume)"));
    let out = env.kubectl_apply("gitea", &with_namespace(&built.manifest, NAMESPACE))?;
    show(env, &out);
    env.say(&format!("[4/9] waiting for the pod (the first start pulls {} from Docker Hub, about 110 MB)", IMAGE.split('@').next().unwrap_or(IMAGE)));
    env.rollout(NAMESPACE, "gitea", Duration::from_secs(900))?;
    let mut answered = None;
    for n in 0..12 {
        if n > 0 {
            env.sleep(Duration::from_secs(5));
        }
        match env.http_status(&format!("{web}/api/healthz")) {
            Ok(200) => {
                answered = Some(());
                break;
            }
            Ok(s) => env.say(&format!("  {web} answered HTTP {s}, trying again")),
            Err(_) => {}
        }
    }
    if answered.is_none() {
        return Err(format!("the Gitea pod is running but {web} does not answer from this machine. Another program may hold port {port}, or this cluster's network does not publish host ports on the loopback address. Settings \u{2192} Dev cluster shows the pod; press Install again after freeing the port"));
    }
    env.say(&format!("[5/9] Gitea answers at {web}"));
    match exec_retrying(env, pg::ADMIN_CREATE_SCRIPT, "", &|e| pg::admin_exists(e)) {
        Ok(Some(_)) => env.say(&format!("[6/9] created the administrator {admin} (sign-in is required, registration is closed)")),
        Ok(None) => env.say(&format!("[6/9] the administrator {admin} exists already")),
        Err(e) => return Err(format!("creating the administrator failed: {e}")),
    }
    let tvault = format!("svc.{instance}.token");
    if prev.as_ref().is_some_and(|r| r.vault.contains(&tvault)) {
        env.say("[7/9] the IYAKKI API token is in your vault already; it is kept");
    } else {
        let name = format!("iyakki-{}", crate::clock::now_unix());
        let out = exec_retrying(env, pg::TOKEN_SCRIPT, &name, &|_| false).map_err(|e| format!("creating the API token failed: {e}"))?.unwrap_or_default();
        let token: Zeroizing<String> = pg::token_from_output(&out)?;
        env.put_secret(NAMESPACE, TOKEN_SECRET, "token", &token, &instance)?;
        env.say("[7/9] made an API token for IYAKKI with the scopes write:repository and read:user (it is not shown; it goes into your vault)");
    }
    let now = crate::clock::now_unix();
    let rec = Installed {
        id: instance.clone(),
        service: "gitea".into(),
        title: "Gitea".into(),
        version: VERSION.into(),
        kind: "git".into(),
        cluster: Some(cluster),
        namespace: NAMESPACE.into(),
        created_namespace: prev.as_ref().is_none_or(|p| p.created_namespace),
        connected: false,
        options: opts,
        urls: built.urls.clone(),
        vault: prev.as_ref().map(|p| p.vault.clone()).unwrap_or_default(),
        installed_at: prev.as_ref().map_or(now, |p| p.installed_at),
        updated_at: now,
        workspaces: prev.map(|p| p.workspaces).unwrap_or_default(),
        note: String::new(),
    };
    env.save_service(rec)?;
    env.request_after(REQUEST)?;
    let detail = detail(port, &admin);
    crate::quickinstall::mark_verified(env, Item::Gitea, &detail)?;
    env.say("[8/9] recorded it in your installed services (Settings \u{2192} Dev cluster, the assistant's git_service_status and Connect workspace see it); IYAKKI is putting the password and token into your vault and adding the Git connection \"Local Gitea\"");
    env.say(&format!("[9/9] done: {detail}"));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the shell's half

/// What the handoff needs from the world; tests give fakes.
pub struct Handoff<'a> {
    /// `(secret, key)` -> the value, `None` when the Secret is not there.
    pub read: &'a dyn Fn(&str, &str) -> Result<Option<Zeroizing<String>>, String>,
    pub store: &'a dyn Fn(&str, &str) -> Result<(), String>,
    pub has: &'a dyn Fn(&str) -> bool,
    pub drop_secret: &'a dyn Fn(&str) -> Result<(), String>,
    pub sleep: &'a dyn Fn(Duration),
    pub tries: u32,
    pub wait: Duration,
}

/// Move the administrator password and the API token from the cluster's Secrets into the vault (retried while the vault is locked or the
/// cluster busy), then delete the token Secret. Neither returned line holds a value.
pub fn handoff(h: &Handoff, admin_vault: &str, token_vault: &str) -> Result<String, String> {
    let mut last = String::from("not tried");
    'tries: for n in 0..h.tries.max(1) {
        if n > 0 {
            (h.sleep)(h.wait);
        }
        for (secret, key, vault, label) in [(pg::ADMIN_SECRET, "password", admin_vault, "administrator password"), (TOKEN_SECRET, "token", token_vault, "API token")] {
            match (h.read)(secret, key) {
                Ok(Some(v)) if !v.trim().is_empty() => {
                    if let Err(e) = (h.store)(vault, v.trim()) {
                        last = format!("could not put the {label} in your vault: {e}");
                        continue 'tries;
                    }
                    if secret == TOKEN_SECRET {
                        // the vault has it now: the cluster does not need to keep a second copy of the token
                        let _ = (h.drop_secret)(secret);
                    }
                }
                Ok(_) if (h.has)(vault) => {}
                Ok(_) => {
                    last = format!("the Secret {secret} holds no {label} and your vault has none");
                    continue 'tries;
                }
                Err(e) => {
                    last = format!("could not read the {label} from the cluster: {e}");
                    continue 'tries;
                }
            }
        }
        return Ok(format!("the administrator password ({admin_vault}) and the IYAKKI API token ({token_vault}) are in your vault"));
    }
    Err(format!("{last}. Gitea works; unlock your vault (sign in again) and press Install on Gitea again"))
}

/// Add the Git connection for this Gitea once: found by the instance it names, so a second run changes nothing but a moved address.
/// Returns the connection and whether it was made now.
pub fn register_connection(conns: &Conns, rec: &Installed) -> Result<(Connection, bool), String> {
    let url = rec.web_url().ok_or("the Gitea record has no web address")?.trim_end_matches('/').to_string();
    let tvault = token_vault(rec);
    if let Some(mut c) = conns.list().into_iter().find(|c| c.cluster == rec.id) {
        if c.url != url || c.token_vault != tvault {
            c.url = url;
            c.token_vault = tvault;
            conns.update(&c, None)?;
        }
        return Ok((c, false));
    }
    let taken = conns.list().iter().any(|c| c.name.eq_ignore_ascii_case(CONNECTION_NAME));
    let name = if taken { format!("{CONNECTION_NAME} (IYAKKI)") } else { CONNECTION_NAME.to_string() };
    let c = Connection { name, kind: Kind::Gitea, url, method: Method::Token, token_vault: tvault, cluster: rec.id.clone(), ..Default::default() };
    let c = conns.add(c, None)?;
    Ok((c, true))
}

/// The record picked for the handoff: this install's, else any Gitea Platform services deployed.
pub fn record_for(dir: &Path, user: &str) -> Option<Installed> {
    let all = installed::load(dir, user);
    all.iter().find(|r| ours(r)).or_else(|| all.iter().find(|r| r.service == "gitea" && !r.connected)).cloned()
}

fn remember_vault(dir: &Path, user: &str, id: &str, names: &[String]) -> Result<(), String> {
    let mut r = installed::find(dir, user, id).ok_or("the Gitea record is gone")?;
    for n in names {
        if !r.vault.contains(n) {
            r.vault.push(n.clone());
        }
    }
    installed::upsert(dir, user, r)
}

/// The shell's whole half: handoff into the vault, the names noted in the record, the Git connection added. One plain line.
pub fn complete(h: &Handoff, conns: &Conns, dir: &Path, user: &str) -> Result<String, String> {
    let rec = record_for(dir, user).ok_or("no Gitea is recorded in your installed services")?;
    let (a, t) = (admin_vault(&rec), token_vault(&rec));
    let moved = handoff(h, &a, &t)?;
    remember_vault(dir, user, &rec.id, &[a, t])?;
    let rec = installed::find(dir, user, &rec.id).unwrap_or(rec);
    let (c, made) = register_connection(conns, &rec)?;
    Ok(format!("{moved}; {} the Git connection \"{}\" at {} (GIT tile, AI IDE \u{2192} Git)", if made { "added" } else { "kept" }, c.name, c.url))
}

/// The real thing for one person: the managed cluster's Secrets, their vault, their Git connections.
pub fn finish(svc: &Arc<crate::services::Services>, user: &str) -> Result<String, String> {
    let ns = record_for(svc.dir(), user).map(|r| r.namespace).unwrap_or_else(|| NAMESPACE.into());
    let read = |secret: &str, key: &str| -> Result<Option<Zeroizing<String>>, String> {
        let out = svc.k3sdev.kubectl(&["-n", &ns, "get", "secret", secret, "-o", &format!("go-template={{{{index .data \"{key}\" | base64decode}}}}")]);
        let v = Zeroizing::new(out.stdout.clone());
        if out.code == Some(0) {
            Ok(Some(v))
        } else if out.stderr.contains("NotFound") || out.stderr.contains("not found") {
            Ok(None)
        } else {
            Err(first_line(&out.stderr))
        }
    };
    let store = |name: &str, value: &str| svc.user_set(user, name, value).map_err(|e| e.to_string());
    let has = |name: &str| svc.user_names(user).is_ok_and(|n| n.iter().any(|x| x == name));
    let drop_secret = |name: &str| {
        let o = svc.k3sdev.kubectl(&["-n", &ns, "delete", "secret", name, "--ignore-not-found"]);
        if o.code == Some(0) { Ok(()) } else { Err(first_line(&o.stderr)) }
    };
    let h = Handoff { read: &read, store: &store, has: &has, drop_secret: &drop_secret, sleep: &|d| std::thread::sleep(d), tries: 40, wait: Duration::from_secs(3) };
    let env = crate::ssh::Env::for_services(svc, user);
    let conns = Conns::new(&env.dir, Arc::new(crate::gitkeys::ServicesStore::new(svc, user)), Arc::new(|_, _| {}));
    complete(&h, &conns, svc.dir(), user)
}

// ------------------------------------------------------------------------------------------------ the IDE's offer

/// What the AI IDE's Git panel shows for a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// The project has a remote already: nothing is offered.
    None,
    /// No remote and Gitea is not installed (no connection yet): the plain message that names Store \u{2192} Setup.
    Install(String),
    /// No remote and the connection exists: its id (the flow starts on it).
    Ready(String),
}

pub fn offer(has_remote: bool, conns: &[Connection]) -> Offer {
    if has_remote {
        return Offer::None;
    }
    match conns.iter().find(|c| !c.cluster.is_empty() && c.kind == Kind::Gitea && c.name.starts_with(CONNECTION_NAME)) {
        Some(c) => Offer::Ready(c.id.clone()),
        None => Offer::Install(NOT_INSTALLED.to_string()),
    }
}
