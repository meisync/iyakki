//! Cloud mode (docs/K3S-DEV.md, phase 5): clusters that are not a file on disk.
//!
//! * **This pod's service account.** When IYAKKI runs in a pod (`KUBERNETES_SERVICE_HOST` set and a service account token mounted),
//!   [`in_cluster_kubeconfig`] writes a kubeconfig that points at the API server and names the token and CA *files* (the token itself
//!   is never copied), and the cluster resolver offers it as the "In-cluster" profile ([`crate::devcluster::Source::InCluster`]).
//!   What the account may do is shown from `kubectl auth can-i --list` ([`can_i_list`]); an account that can do everything everywhere
//!   (`*.*` with `*`) is treated as read-only for the assistant ([`cluster_admin_like`]). Writes still ask first and stay in the
//!   workspace's namespace.
//! * **Kubeconfigs held in a vault.** A profile whose kubeconfig is `vault:<name>` (system vault) or `uvault:<name>` (the person's
//!   vault) is resolved only when a program is started with it: [`prepare`] writes the secret to a new 0600 file in a RAM folder,
//!   points the child's `--kubeconfig` / `KUBECONFIG` at it, and the file is removed as soon as the child has finished (also on a
//!   panic, by `Drop`). The secret is never logged, never in the audit log and never passed through the model.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::devcluster::ClusterProfile;
use crate::wsexec::Runner;

pub fn is_vault_ref(s: &str) -> bool {
    s.starts_with("vault:") || s.starts_with("uvault:")
}

// ------------------------------------------------------------------------------------------------ this pod's service account

const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

/// Where a pod's environment and service account files are. [`PodEnv::real`] is this process; tests give their own.
#[derive(Clone, Debug)]
pub struct PodEnv {
    pub host: Option<String>,
    pub port: Option<String>,
    pub token: PathBuf,
    pub ca: PathBuf,
    pub namespace: PathBuf,
    /// Where the generated kubeconfig is written.
    pub out: PathBuf,
}

impl PodEnv {
    pub fn real() -> PodEnv {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        PodEnv {
            host: std::env::var("KUBERNETES_SERVICE_HOST").ok().filter(|s| !s.is_empty()),
            port: std::env::var("KUBERNETES_SERVICE_PORT").ok().filter(|s| !s.is_empty()),
            token: Path::new(SA_DIR).join("token"),
            ca: Path::new(SA_DIR).join("ca.crt"),
            namespace: Path::new(SA_DIR).join("namespace"),
            out: std::env::temp_dir().join(format!("iyakki-incluster-{uid}.yaml")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InCluster {
    pub server: String,
    pub namespace: String,
    pub token: PathBuf,
    pub ca: PathBuf,
}

/// Are we in a pod with a service account? Needs the environment variable, a non-empty token file and the CA file.
pub fn detect_in_cluster(e: &PodEnv) -> Option<InCluster> {
    let host = e.host.as_deref()?.trim();
    if host.is_empty() || !std::fs::read_to_string(&e.token).is_ok_and(|t| !t.trim().is_empty()) || !e.ca.is_file() {
        return None;
    }
    let port = e.port.as_deref().unwrap_or("443");
    let h = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    let namespace = std::fs::read_to_string(&e.namespace).map(|s| s.trim().to_string()).unwrap_or_default();
    Some(InCluster { server: format!("https://{h}:{port}"), namespace: if namespace.is_empty() { "default".into() } else { namespace }, token: e.token.clone(), ca: e.ca.clone() })
}

pub fn kubeconfig_text(ic: &InCluster) -> String {
    format!(
        "apiVersion: v1\nkind: Config\nclusters:\n- name: in-cluster\n  cluster:\n    server: {}\n    certificate-authority: {}\nusers:\n- name: serviceaccount\n  user:\n    tokenFile: {}\ncontexts:\n- name: in-cluster\n  context:\n    cluster: in-cluster\n    user: serviceaccount\n    namespace: {}\ncurrent-context: in-cluster\n",
        ic.server,
        ic.ca.display(),
        ic.token.display(),
        ic.namespace
    )
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

/// The kubeconfig of this pod's service account, written (mode 0600; it holds no secret, only the paths) when IYAKKI runs in a pod.
pub fn in_cluster_kubeconfig() -> Option<PathBuf> {
    in_cluster_kubeconfig_for(&PodEnv::real())
}

pub fn in_cluster_kubeconfig_for(e: &PodEnv) -> Option<PathBuf> {
    let ic = detect_in_cluster(e)?;
    write_private(&e.out, &kubeconfig_text(&ic)).ok()?;
    Some(e.out.clone())
}

/// `kubectl auth can-i --list` for the profile in `ns`, as readable lines (`pods: get list watch`), one per resource.
pub fn can_i_list(run: &Arc<dyn Runner>, p: &ClusterProfile, ns: &str) -> Vec<String> {
    let kc = crate::devcluster::Kc::new(run.clone(), &p.kubeconfig, &p.context);
    let o = kc.kubectl(&["auth", "can-i", "--list", "-n", ns], 6, 14);
    if !o.ok() {
        return Vec::new();
    }
    summarize_can_i(&o.stdout)
}

/// The table `kubectl auth can-i --list` prints, as `resource: verbs` lines (the non-resource URLs are left out).
pub fn summarize_can_i(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    for l in text.lines().skip(1) {
        let Some(open) = l.rfind('[') else { continue };
        let verbs = l[open..].trim_matches(|c| c == '[' || c == ']').trim().to_string();
        let first = l.split_whitespace().next().unwrap_or("");
        // rows with only non-resource URLs start with "[" (an empty resource column is printed as spaces)
        if first.is_empty() || first.starts_with('[') || verbs.is_empty() {
            continue;
        }
        v.push(format!("{first}: {verbs}"));
    }
    v
}

/// Can the account do everything on everything? (`*.*` with `*`, or `*` on `*` in the rows.)
pub fn cluster_admin_like(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        let (r, v) = l.split_once(": ").unwrap_or((l, ""));
        (r == "*.*" || r == "*") && v.split_whitespace().any(|x| x == "*")
    })
}

/// The `can_i` text of a profile: for the in-cluster account the permissions in the workspace namespace.
pub fn can_i_text(lines: &[String]) -> String {
    if lines.is_empty() {
        return "no permissions listed".into();
    }
    let mut t: Vec<String> = lines.iter().filter(|l| !l.starts_with("selfsubject") && !l.starts_with("selfsubjectaccessreviews")).take(8).cloned().collect();
    if lines.len() > 8 {
        t.push(format!("and {} more", lines.len() - 8));
    }
    if cluster_admin_like(lines) {
        t.push("cluster-admin like: the assistant only reads".into());
    }
    t.join(" · ")
}

// ------------------------------------------------------------------------------------------------ kubeconfigs held in a vault

/// Looks up a vault reference (`vault:name`, `uvault:name`) for the signed-in person.
pub type Resolver = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

static RESOLVER: Mutex<Option<Resolver>> = Mutex::new(None);

pub fn set_resolver(r: Option<Resolver>) {
    *RESOLVER.lock().unwrap_or_else(|e| e.into_inner()) = r;
    if let Some(d) = temp_dir() {
        sweep_stale(&d, Duration::from_secs(3600));
    }
}

/// The resolver for one person: `uvault:` reads their personal vault, `vault:` the system vault.
pub fn resolver_for(svc: &Arc<crate::services::Services>, user: &str) -> Resolver {
    let (svc, user) = (svc.clone(), user.to_string());
    Arc::new(move |r: &str| {
        if let Some(name) = r.strip_prefix("uvault:") {
            svc.user_get(&user, name).map(|z| z.to_string()).map_err(|e| format!("your vault: {e}"))
        } else {
            svc.secrets.resolve(r, None).map(|z| z.to_string()).map_err(|e| format!("the system vault: {e}"))
        }
    })
}

/// A folder in RAM for the temporary files (`/dev/shm`, else the runtime folder, else the temp folder).
pub fn temp_dir() -> Option<PathBuf> {
    ["/dev/shm"].iter().map(PathBuf::from).chain(std::env::var("XDG_RUNTIME_DIR").ok().map(PathBuf::from)).chain(Some(std::env::temp_dir())).find(|d| d.is_dir())
}

const TEMP_PREFIX: &str = "iyakki-kc-";

/// Remove temp kubeconfigs left by a process that died before it could clean up.
pub fn sweep_stale(dir: &Path, older: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(TEMP_PREFIX) && e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= older) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A kubeconfig written for one child process; the file is removed when this is dropped.
#[derive(Debug)]
pub struct TempKube {
    pub path: PathBuf,
}

impl Drop for TempKube {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write the secret `reference` points at to a new 0600 file in `dir`.
pub fn materialize_in(reference: &str, dir: &Path, resolve: &Resolver) -> Result<TempKube, String> {
    let text = resolve(reference).map_err(|e| format!("the kubeconfig {reference} could not be read from the vault ({e}); sign in again or check the entry"))?;
    if text.trim().is_empty() {
        return Err(format!("the vault entry {reference} is empty"));
    }
    let mut r = [0u8; 8];
    iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{TEMP_PREFIX}{}.yaml", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    write_private(&path, &text)?;
    Ok(TempKube { path })
}

/// The command line and environment for a child, with vault references replaced by temp files. `None`: no reference, run as given.
/// The returned guards must live until the child has finished.
pub type Prepared = (Vec<String>, Vec<(String, String)>, Vec<TempKube>);

pub fn prepare_in(argv: &[String], env: &[(String, String)], dir: &Path, resolve: Option<&Resolver>) -> Result<Option<Prepared>, String> {
    let has = argv.iter().any(|a| is_vault_ref(a) || a.split_once('=').is_some_and(|(_, v)| is_vault_ref(v))) || env.iter().any(|(k, v)| k == "KUBECONFIG" && is_vault_ref(v));
    if !has {
        return Ok(None);
    }
    let resolve = resolve.ok_or("a kubeconfig held in a vault cannot be used: nobody is signed in, or the vault is locked")?;
    let mut guards: Vec<TempKube> = Vec::new();
    let mut map = |reference: &str| -> Result<String, String> {
        let t = materialize_in(reference, dir, resolve)?;
        let p = t.path.display().to_string();
        guards.push(t);
        Ok(p)
    };
    let mut out = Vec::with_capacity(argv.len());
    let mut prev_kubeconfig = false;
    for a in argv {
        if prev_kubeconfig && is_vault_ref(a) {
            out.push(map(a)?);
        } else if let Some(v) = a.strip_prefix("--kubeconfig=").filter(|v| is_vault_ref(v)) {
            out.push(format!("--kubeconfig={}", map(v)?));
        } else {
            out.push(a.clone());
        }
        prev_kubeconfig = a == "--kubeconfig";
    }
    let mut env2 = Vec::with_capacity(env.len());
    for (k, v) in env {
        if k == "KUBECONFIG" && is_vault_ref(v) {
            env2.push((k.clone(), map(v)?));
        } else {
            env2.push((k.clone(), v.clone()));
        }
    }
    Ok(Some((out, env2, guards)))
}

/// [`prepare_in`] with this process's resolver and temp folder (what `wsexec::System` calls before starting a child).
pub fn prepare(argv: &[String], env: &[(String, String)]) -> Result<Option<Prepared>, String> {
    let r = RESOLVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let dir = temp_dir().ok_or("no folder for a temporary kubeconfig")?;
    prepare_in(argv, env, &dir, r.as_ref())
}

// ------------------------------------------------------------------------------------------------ the person's vault profiles

/// A named kubeconfig held in a vault entry, kept per person (`devcluster-vault-<user>.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultProfile {
    pub name: String,
    /// `uvault:<entry>` or `vault:<entry>`.
    pub reference: String,
}

fn vault_path(dir: &Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    dir.join(format!("devcluster-vault-{safe}.json"))
}

pub fn vault_load(dir: &Path, user: &str) -> Vec<VaultProfile> {
    std::fs::read_to_string(vault_path(dir, user)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn vault_save(dir: &Path, user: &str, v: &[VaultProfile]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(vault_path(dir, user), serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Add (or replace) a profile. The entry name is checked; the secret itself is never read here.
pub fn vault_add(dir: &Path, user: &str, name: &str, reference: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || !name.chars().all(|c| c.is_ascii_alphanumeric() || " -_.".contains(c)) {
        return Err("a profile name is letters, digits, spaces and - _ . (up to 40)".into());
    }
    let entry = reference.trim().strip_prefix("uvault:").or_else(|| reference.trim().strip_prefix("vault:")).unwrap_or(reference.trim());
    if !crate::vault::valid_secret_name(entry) {
        return Err("a vault entry name is lowercase letters, digits, . _ - (up to 64)".into());
    }
    let reference = if reference.trim().starts_with("vault:") { format!("vault:{entry}") } else { format!("uvault:{entry}") };
    let mut v = vault_load(dir, user);
    v.retain(|p| p.name != name);
    v.push(VaultProfile { name: name.into(), reference });
    vault_save(dir, user, &v)
}

pub fn vault_remove(dir: &Path, user: &str, name: &str) -> Result<(), String> {
    let mut v = vault_load(dir, user);
    v.retain(|p| p.name != name);
    vault_save(dir, user, &v)
}

#[cfg(test)]
#[path = "kubeprofiles_tests.rs"]
mod tests;
