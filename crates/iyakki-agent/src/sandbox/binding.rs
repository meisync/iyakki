//! The team's cluster binding as a `KubeApi` (F-SBX-03): the binding's vault credential becomes a kubeconfig for `kubectl`.
//!
//! The credential is either a bearer token (a service account token) or a complete kubeconfig. Per call the vault hands the value to
//! this module only for the duration of the call ([`SecretStore::use_secret`]); a kubeconfig file is written to a private folder with
//! mode 0600 (`create_new`, never overwritten), used for the one `kubectl` call and removed again, also when the call fails. The value
//! never goes on a command line, into an environment variable or into an error message.
//!
//! A kubeconfig the person stored is refused when it could run a program or read another file (`exec`, `auth-provider`, `tokenFile`,
//! key and certificate paths) or when its server is not the binding's `api_server`: the credential cannot send the run somewhere else.

use super::kube::{JobState, KubeApi, Kubectl};
use crate::tenantconfig::schema::SecretRef;
use crate::tenantconfig::vault::SecretStore;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Builds the cluster door for one kubeconfig file (the real one is `kubectl`; tests give a fake).
pub type Maker = Box<dyn Fn(&Path) -> Box<dyn KubeApi> + Send + Sync>;

/// Keys that make `kubectl` run a program or read a file named by the kubeconfig.
const FORBIDDEN_KEYS: [&str; 9] = ["exec", "auth-provider", "tokenFile", "client-certificate", "client-key", "certificate-authority", "proxy-url", "insecure-skip-tls-verify", "username"];

fn same_server(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/').eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

/// The keys written in a YAML or JSON kubeconfig text, as far as a line scan sees them.
fn keys_of(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in text.lines() {
        // JSON can put several pairs on one line: split on commas and braces first
        for part in line.split([',', '{', '}']) {
            let p = part.trim().trim_start_matches("- ").trim();
            if let Some((k, val)) = p.split_once(':') {
                let k = k.trim().trim_matches(['"', '\'']);
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    v.push((k.to_string(), val.trim().trim_matches(['"', '\'']).to_string()));
                }
            }
        }
    }
    v
}

/// The kubeconfig text for this binding and credential value. The error never contains the value.
pub fn kubeconfig_for(server: &str, credential: &str) -> Result<String, String> {
    if !server.starts_with("https://") || server.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"') {
        return Err("the binding's API address is not an https address".into());
    }
    let c = credential.trim();
    if c.is_empty() {
        return Err("the binding's credential is empty".into());
    }
    if c.contains("apiVersion") || c.starts_with('{') {
        let keys = keys_of(c);
        if let Some((k, _)) = keys.iter().find(|(k, _)| FORBIDDEN_KEYS.contains(&k.as_str())) {
            return Err(format!("the stored kubeconfig uses `{k}`, which could run a program or read a file; store a token or a kubeconfig with embedded data"));
        }
        let servers: Vec<&String> = keys.iter().filter(|(k, _)| k == "server").map(|(_, v)| v).collect();
        if servers.is_empty() || !servers.iter().all(|s| same_server(s, server)) {
            return Err("the stored kubeconfig does not point at the binding's API address".into());
        }
        return Ok(format!("{c}\n"));
    }
    if c.len() < 16 || c.len() > 8192 || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || "._~+/=-".contains(ch)) {
        return Err("the binding's credential is neither a token nor a kubeconfig".into());
    }
    let cfg: Value = json!({
        "apiVersion": "v1", "kind": "Config",
        "clusters": [{"name": "binding", "cluster": {"server": server}}],
        "users": [{"name": "binding", "user": {"token": c}}],
        "contexts": [{"name": "binding", "context": {"cluster": "binding", "user": "binding"}}],
        "current-context": "binding"
    });
    Ok(format!("{cfg}\n"))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A kubeconfig file that exists for one call.
struct TempConfig(PathBuf);

impl TempConfig {
    fn write(dir: &Path, text: &str) -> Result<TempConfig, String> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("kc-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let t = TempConfig(path);
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        Ok(t)
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The cluster door of one team and environment.
pub struct BindingKube {
    server: String,
    credential: SecretRef,
    vault: Arc<dyn SecretStore>,
    dir: PathBuf,
    make: Maker,
}

impl BindingKube {
    /// `dir` is a private folder for the short-lived kubeconfig files (on a tenant pod an `emptyDir` such as `/tmp`).
    pub fn new(server: &str, credential: SecretRef, vault: Arc<dyn SecretStore>, dir: &Path, make: Maker) -> BindingKube {
        BindingKube { server: server.into(), credential, vault, dir: dir.into(), make }
    }

    /// The real `kubectl` for the binding of `team` and `environment`; `None` when the binding is missing, off or not the team's.
    pub fn from_binding(env: &crate::tenantconfig::api::Env, team: &str, environment: &str, dir: &Path) -> Option<BindingKube> {
        let (server, credential) = crate::tenantconfig::enforce::k3s_binding(env, team, environment)?;
        let make: Maker = Box::new(|p: &Path| Box::new(Kubectl::new(Some(p.to_string_lossy().into_owned()))) as Box<dyn KubeApi>);
        Some(BindingKube::new(&server, credential, env.vault.clone(), dir, make))
    }

    fn with_kube<T>(&self, f: impl FnOnce(&dyn KubeApi) -> Result<T, String>) -> Result<T, String> {
        let mut f = Some(f);
        let mut out: Option<Result<T, String>> = None;
        let used = self.vault.use_secret(&self.credential, "sandbox.cluster", &mut |value: &str| {
            let text = kubeconfig_for(&self.server, value)?;
            let file = TempConfig::write(&self.dir, &text)?;
            let kube = (self.make)(&file.0);
            if let Some(f) = f.take() {
                out = Some(f(kube.as_ref()));
            }
            Ok(String::new())
        });
        used?;
        out.unwrap_or_else(|| Err("the cluster door was not used".into()))
    }
}

impl KubeApi for BindingKube {
    fn apply(&self, manifest: &Value) -> Result<(), String> {
        self.with_kube(|k| k.apply(manifest))
    }
    fn state(&self, ns: &str, run: &str) -> Result<JobState, String> {
        self.with_kube(|k| k.state(ns, run))
    }
    fn logs(&self, ns: &str, run: &str, limit: usize) -> Result<Vec<u8>, String> {
        self.with_kube(|k| k.logs(ns, run, limit))
    }
    fn artifacts(&self, ns: &str, run: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.with_kube(|k| k.artifacts(ns, run))
    }
    fn delete_run(&self, ns: &str, run: &str) -> Result<(), String> {
        self.with_kube(|k| k.delete_run(ns, run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenantconfig::vault::SecretState;
    use std::sync::Mutex;

    const SERVER: &str = "https://10.0.0.5:6443";
    const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6Im9uZSJ9.abc-def_ghi";

    #[derive(Default)]
    struct Vault {
        value: String,
        used: Mutex<Vec<String>>,
    }
    impl SecretStore for Vault {
        fn state(&self, _: &SecretRef) -> SecretState {
            SecretState::Locked
        }
        fn put(&self, _: &SecretRef, _: &str) -> Result<(), String> {
            Err("no".into())
        }
        fn remove(&self, _: &SecretRef) -> Result<(), String> {
            Err("no".into())
        }
        fn use_secret(&self, r: &SecretRef, action: &str, f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
            self.used.lock().unwrap().push(format!("{} {action}", r.text()));
            if self.value.is_empty() {
                return Err("the tenant vault is locked".into());
            }
            f(&self.value)
        }
    }

    /// Records what the kubeconfig file looked like while the call ran.
    struct Seen {
        files: Arc<Mutex<Vec<(PathBuf, String, u32)>>>,
    }
    struct Fake {
        path: PathBuf,
        seen: Arc<Mutex<Vec<(PathBuf, String, u32)>>>,
        fail: bool,
    }
    impl KubeApi for Fake {
        fn apply(&self, _: &Value) -> Result<(), String> {
            use std::os::unix::fs::PermissionsExt;
            let text = std::fs::read_to_string(&self.path).unwrap();
            let mode = std::fs::metadata(&self.path).unwrap().permissions().mode() & 0o777;
            self.seen.lock().unwrap().push((self.path.clone(), text, mode));
            if self.fail { Err("boom".into()) } else { Ok(()) }
        }
        fn state(&self, _: &str, _: &str) -> Result<JobState, String> {
            Ok(JobState { succeeded: true, ..Default::default() })
        }
        fn logs(&self, _: &str, _: &str, _: usize) -> Result<Vec<u8>, String> {
            Ok(b"hi".to_vec())
        }
        fn delete_run(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn rig(tag: &str, value: &str, fail: bool) -> (BindingKube, Arc<Vault>, Seen, PathBuf) {
        let dir = std::env::temp_dir().join(format!("iyakki-bindkube-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let vault = Arc::new(Vault { value: value.into(), ..Default::default() });
        let seen = Seen { files: Arc::new(Mutex::new(vec![])) };
        let (files, f2) = (seen.files.clone(), fail);
        let make: Maker = Box::new(move |p: &Path| Box::new(Fake { path: p.to_path_buf(), seen: files.clone(), fail: f2 }) as Box<dyn KubeApi>);
        let k = BindingKube::new(SERVER, SecretRef::parse("vault:team/dev/dev-cluster").unwrap(), vault.clone(), &dir.join("kc"), make);
        (k, vault, seen, dir)
    }

    #[test]
    fn a_token_becomes_a_kubeconfig_for_the_bindings_server_only() {
        let t = kubeconfig_for(SERVER, TOKEN).unwrap();
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["clusters"][0]["cluster"]["server"], SERVER);
        assert_eq!(v["users"][0]["user"]["token"], TOKEN);
        assert_eq!(v["current-context"], "binding");
        for bad in ["", "short", "has space in it and is long enough", "a\"b'c;rm -rf /xxxxxxxxxxxxx"] {
            let e = kubeconfig_for(SERVER, bad).unwrap_err();
            assert!(!e.contains(bad) || bad.is_empty(), "the value is not echoed: {e}");
        }
        assert!(kubeconfig_for("http://10.0.0.5:6443", TOKEN).is_err());
        assert!(kubeconfig_for("https://a\"b", TOKEN).is_err());
    }

    #[test]
    fn a_stored_kubeconfig_is_used_only_when_it_cannot_run_a_program_read_a_file_or_go_elsewhere() {
        let ok = "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster:\n    server: https://10.0.0.5:6443/\n    certificate-authority-data: AAAA\nusers:\n- name: u\n  user:\n    token: abcdefabcdefabcdefabcd\n";
        assert!(kubeconfig_for(SERVER, ok).is_ok());
        for (what, bad) in [
            ("exec", ok.replace("    token: abcdefabcdefabcdefabcd", "    exec:\n      command: sh")),
            ("auth-provider", ok.replace("    token:", "    auth-provider:\n      name: x\n    t:")),
            ("tokenFile", ok.replace("    token: abcdefabcdefabcdefabcd", "    tokenFile: /etc/shadow")),
            ("client-key", ok.replace("    token: abcdefabcdefabcdefabcd", "    client-key: /root/key")),
            ("certificate-authority", ok.replace("certificate-authority-data: AAAA", "certificate-authority: /etc/passwd")),
        ] {
            assert!(kubeconfig_for(SERVER, &bad).unwrap_err().contains(what), "{what}");
        }
        assert!(kubeconfig_for(SERVER, &ok.replace("10.0.0.5", "evil.example")).unwrap_err().contains("does not point"));
        assert!(kubeconfig_for(SERVER, &ok.replace("    server: https://10.0.0.5:6443/\n", "")).is_err());
        let json = r#"{"apiVersion":"v1","clusters":[{"name":"c","cluster":{"server":"https://10.0.0.5:6443"}}],"users":[{"name":"u","user":{"exec":{"command":"sh"}}}]}"#;
        assert!(kubeconfig_for(SERVER, json).unwrap_err().contains("exec"));
    }

    #[test]
    fn each_call_writes_a_private_kubeconfig_uses_it_once_and_removes_it() {
        let (k, vault, seen, dir) = rig("ok", TOKEN, false);
        k.apply(&json!({})).unwrap();
        k.apply(&json!({})).unwrap();
        let files = seen.files.lock().unwrap();
        assert_eq!(files.len(), 2);
        assert_ne!(files[0].0, files[1].0, "a file of its own per call");
        for (path, text, mode) in files.iter() {
            assert_eq!(*mode, 0o600);
            assert!(text.contains(TOKEN) && text.contains(SERVER));
            assert!(!path.exists(), "removed after the call");
        }
        assert_eq!(std::fs::read_dir(dir.join("kc")).unwrap().count(), 0);
        assert!(vault.used.lock().unwrap().iter().all(|u| u == "vault:team/dev/dev-cluster sandbox.cluster"), "{:?}", vault.used.lock().unwrap());
        assert_eq!(k.state("ns", "r").unwrap().succeeded, true);
        assert_eq!(k.logs("ns", "r", 10).unwrap(), b"hi");
        assert!(k.delete_run("ns", "r").is_ok());
        assert!(k.artifacts("ns", "r").unwrap().is_empty());
    }

    #[test]
    fn a_failing_call_still_removes_the_file_and_a_locked_vault_or_bad_value_writes_nothing() {
        let (k, _, seen, dir) = rig("fail", TOKEN, true);
        assert_eq!(k.apply(&json!({})).unwrap_err(), "boom");
        assert!(!seen.files.lock().unwrap()[0].0.exists());
        let (k, _, seen, dir2) = rig("locked", "", false);
        assert!(k.apply(&json!({})).unwrap_err().contains("locked"));
        assert!(seen.files.lock().unwrap().is_empty());
        let (k, _, seen, dir3) = rig("bad", "not a token at all, sorry", false);
        let e = k.apply(&json!({})).unwrap_err();
        assert!(!e.contains("not a token at all") && seen.files.lock().unwrap().is_empty(), "{e}");
        for d in [dir, dir2, dir3] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
