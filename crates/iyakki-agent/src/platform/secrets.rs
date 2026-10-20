//! Generated secrets of a platform service. A value is created here, written to the person's vault first (if the vault is locked
//! nothing is created), then to a Kubernetes Secret through a 0600 temp file in RAM (`kubectl create secret generic --from-file`), so it
//! is never an argument, never in a log and never in a manifest. The caller gets it once as a [`Reveal`] for the masked dialog.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::devcluster::Kc;

/// The person's vault, as far as platform services need it.
pub trait VaultApi: Send + Sync {
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    fn has(&self, name: &str) -> bool;
}

/// The real vault of one signed-in person.
pub struct ServicesVault {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl VaultApi for ServicesVault {
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.svc.user_set(&self.user, name, value).map_err(|e| format!("your vault: {e}"))
    }
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        self.svc.user_get(&self.user, name).map_err(|e| format!("your vault: {e}"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.svc.user_delete(&self.user, name).map_err(|e| format!("your vault: {e}"))
    }
    fn has(&self, name: &str) -> bool {
        self.svc.user_names(&self.user).is_ok_and(|n| n.iter().any(|x| x == name))
    }
}

/// An in-memory vault for tests (and for hosts without one); `locked` makes every write fail like a locked vault.
#[derive(Default)]
pub struct MemVault {
    pub items: Mutex<BTreeMap<String, String>>,
    pub locked: std::sync::atomic::AtomicBool,
}

impl VaultApi for MemVault {
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("your vault: locked".into());
        }
        self.items.lock().unwrap_or_else(|e| e.into_inner()).insert(name.into(), value.into());
        Ok(())
    }
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("your vault: locked".into());
        }
        self.items.lock().unwrap_or_else(|e| e.into_inner()).get(name).map(|v| Zeroizing::new(v.clone())).ok_or_else(|| format!("your vault: no entry `{name}`"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.items.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        Ok(())
    }
    fn has(&self, name: &str) -> bool {
        self.items.lock().unwrap_or_else(|e| e.into_inner()).contains_key(name)
    }
}

/// A value to show once. Its `Debug` never prints the value; the memory is wiped on drop.
pub struct Reveal {
    pub label: String,
    pub vault: String,
    pub value: Zeroizing<String>,
}

impl std::fmt::Debug for Reveal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reveal({}, uvault:{}, {} characters)", self.label, self.vault, self.value.len())
    }
}

const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// A random password of `n` characters (no look-alike characters), from the system's random source.
pub fn random_secret(n: usize) -> String {
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut b = [0u8; 32];
        if iyakki_sec::random_fill(&mut b).is_err() {
            break;
        }
        for x in b {
            // 256 is not a multiple of 56: drop the top values so every letter is equally likely
            if (x as usize) < ALPHABET.len() * (256 / ALPHABET.len()) && out.len() < n {
                out.push(ALPHABET[x as usize % ALPHABET.len()] as char);
            }
        }
    }
    out
}

/// A 0600 file in `dir` holding `value`, removed when dropped.
pub struct SecretFile {
    pub path: std::path::PathBuf,
}

impl SecretFile {
    pub fn new(dir: &std::path::Path, value: &str) -> Result<SecretFile, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut r = [0u8; 8];
        iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
        let path = dir.join(format!("iyakki-svc-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(value.as_bytes()).map_err(|e| e.to_string())?;
        Ok(SecretFile { path })
    }
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Create the Secret `name` in `ns` from `(key, value)` pairs, labelled like the service's objects. The values travel in 0600 temp
/// files; the command line holds the file paths only. Returns kubectl's first line on failure (it never holds a value).
pub fn create_secret(kc: &Kc, tmp: &std::path::Path, ns: &str, name: &str, pairs: &[(String, Zeroizing<String>)], labels: &[(&str, &str)]) -> Result<(), String> {
    let files: Vec<(String, SecretFile)> = pairs.iter().map(|(k, v)| SecretFile::new(tmp, v).map(|f| (k.clone(), f))).collect::<Result<_, _>>()?;
    let froms: Vec<String> = files.iter().map(|(k, f)| format!("--from-file={k}={}", f.path.display())).collect();
    let mut args: Vec<&str> = vec!["create", "secret", "generic", name, "-n", ns];
    args.extend(froms.iter().map(String::as_str));
    let o = kc.kubectl(&args, 20, 40);
    drop(files);
    if !o.ok() {
        return Err(format!("could not create the Secret {name}: {}", o.combined().lines().next().unwrap_or("").trim()));
    }
    let l: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut la: Vec<&str> = vec!["label", "secret", name, "-n", ns, "--overwrite"];
    la.extend(l.iter().map(String::as_str));
    let o = kc.kubectl(&la, 20, 40);
    if !o.ok() {
        return Err(format!("created the Secret {name} but could not label it: {}", o.combined().lines().next().unwrap_or("").trim()));
    }
    Ok(())
}

pub fn secret_exists(kc: &Kc, ns: &str, name: &str) -> Result<bool, String> {
    let o = kc.kubectl(&["get", "secret", name, "-n", ns, "-o", "name"], 10, 20);
    if o.ok() {
        return Ok(true);
    }
    let t = o.combined();
    if t.contains("NotFound") || t.contains("not found") {
        return Ok(false);
    }
    Err(format!("could not check the Secret {name}: {}", t.lines().next().unwrap_or("").trim()))
}
