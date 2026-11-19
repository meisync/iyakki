//! The common vault of a cloud instance (docs/development/CLOUD-MULTITENANT-PLAN.md, step 3).
//!
//! Three scopes of secrets, by reference scheme: `uvault:NAME` is one person's (unlocked by their sign-in password), `vault:NAME` is the
//! machine's, which in a cloud tenant is **the tenant's** (a tenant is one pod; each administrator has a key slot and so shares it without
//! sharing a password), and `cvault:NAME` is **common to every tenant** of the instance: it holds what the operator wants all of them to
//! use (a model provider's key, a registry token) and is read-only for them.
//!
//! The common vault is an ordinary vault file made and kept by the operator, in one folder: `vault.json` and `key`. The key is a random
//! password for the slot `tenants`; the operator puts the folder in a cluster-wide Secret (`iyakki-common`) that each tenant's pod mounts
//! read-only at `/etc/iyakki/common` (`cloudtenant.rs`, `common_vault: true`). At start the pod opens it with that key. Nothing in the
//! tenant can write it, and a tenant that is not given the Secret has no `cvault:` at all.
//!
//! Operator commands: `iyakki cloud common <init|set|get-names|delete> --dir DIR` (see `cloudcli.rs`).

use std::path::{Path, PathBuf};

use crate::vault::{LocalVault, VaultError};

pub const DEFAULT_DIR: &str = "/etc/iyakki/common";
pub const SLOT: &str = "tenants";
const VAULT_FILE: &str = "vault.json";
const KEY_FILE: &str = "key";

/// The folder of the common vault (`IYAKKI_COMMON_VAULT_DIR` beats the default).
pub fn dir() -> PathBuf {
    std::env::var_os("IYAKKI_COMMON_VAULT_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_DIR))
}

fn read_key(dir: &Path) -> Result<String, String> {
    let k = std::fs::read_to_string(dir.join(KEY_FILE)).map_err(|e| format!("{}: {e}", dir.join(KEY_FILE).display()))?;
    let k = k.trim().to_string();
    if k.len() < 32 {
        return Err("the common vault's key file is too short".into());
    }
    Ok(k)
}

/// Open and unlock the common vault of `dir`. `None` when there is none (the normal case outside a cloud instance) or it cannot be
/// opened (said in the log, never the key).
pub fn load(dir: &Path) -> Option<LocalVault> {
    let path = dir.join(VAULT_FILE);
    if !LocalVault::exists(&path) {
        return None;
    }
    let opened = read_key(dir).and_then(|key| {
        let mut v = LocalVault::open(&path).map_err(|e| e.to_string())?;
        v.unlock(SLOT, &key).map_err(|e| format!("the key does not open the common vault ({e})"))?;
        Ok(v)
    });
    match opened {
        Ok(v) => {
            log::info!("common vault: opened {} ({} secrets)", path.display(), v.names().map(|n| n.len()).unwrap_or(0));
            Some(v)
        }
        Err(e) => {
            log::warn!("common vault: {e}");
            None
        }
    }
}

/// Make a new common vault in `dir` (the folder is created, the key file is 0600). Refuses to replace one.
pub fn init(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(VAULT_FILE);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let mut raw = [0u8; 32];
    iyakki_sec::random_fill(&mut raw).map_err(|e| format!("no entropy: {e}"))?;
    let key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    LocalVault::create(&path, SLOT, &key).map_err(|e| e.to_string())?;
    let kp = dir.join(KEY_FILE);
    std::fs::write(&kp, format!("{key}\n")).map_err(|e| format!("{}: {e}", kp.display()))?;
    std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(())
}

fn open_for_operator(dir: &Path) -> Result<LocalVault, String> {
    let mut v = LocalVault::open(&dir.join(VAULT_FILE)).map_err(|e| format!("{}: {e} (iyakki cloud common init --dir DIR makes one)", dir.join(VAULT_FILE).display()))?;
    v.unlock(SLOT, &read_key(dir)?).map_err(|e: VaultError| e.to_string())?;
    Ok(v)
}

pub fn set(dir: &Path, name: &str, value: &str) -> Result<(), String> {
    open_for_operator(dir)?.set(name, value).map_err(|e| e.to_string())
}

pub fn names(dir: &Path) -> Result<Vec<String>, String> {
    open_for_operator(dir)?.names().map_err(|e| e.to_string())
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    open_for_operator(dir)?.delete(name).map_err(|e| e.to_string())
}

/// The command that puts the folder into the cluster-wide Secret every tenant mounts.
pub fn secret_command(dir: &Path, namespace: &str) -> String {
    format!("kubectl -n {namespace} create secret generic iyakki-common --from-file={}/{VAULT_FILE} --from-file={}/{KEY_FILE}", dir.display(), dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{SecretError, Secrets};
    use std::sync::Arc;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-common-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    struct NoNet;
    impl iyakki_proxy::Upstream for NoNet {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("no network".into())
        }
    }

    #[test]
    fn the_operator_keeps_it_and_a_tenant_reads_it_by_cvault_and_cannot_write() {
        let d = tmp("a");
        assert!(load(&d).is_none(), "no folder, no common vault");
        init(&d).unwrap();
        assert!(init(&d).is_err(), "never replaces one");
        set(&d, "model.key", "sk-common").unwrap();
        set(&d, "registry.token", "t0ken").unwrap();
        assert_eq!(names(&d).unwrap(), vec!["model.key".to_string(), "registry.token".to_string()]);
        let mode = std::fs::metadata(d.join("key")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);
        // a tenant pod: the folder is mounted, the vault opens at start
        let s = Secrets::new(None, Arc::new(NoNet));
        assert_eq!(s.resolve("cvault:model.key", None), Err(SecretError::Locked), "nothing mounted: no cvault");
        assert!(!s.common_unlocked());
        s.set_common(load(&d));
        assert!(s.common_unlocked());
        assert_eq!(&*s.resolve("cvault:model.key", None).unwrap(), "sk-common");
        assert!(matches!(s.resolve("cvault:missing", None), Err(SecretError::NotFound(_))));
        // the other scopes are separate: the same name in the tenant's own vault is not the common one
        assert_eq!(s.resolve("vault:model.key", None), Err(SecretError::Locked));
        delete(&d, "registry.token").unwrap();
        assert_eq!(names(&d).unwrap(), vec!["model.key".to_string()]);
    }

    #[test]
    fn a_wrong_or_missing_key_gives_no_vault_and_never_a_panic() {
        let d = tmp("b");
        init(&d).unwrap();
        std::fs::write(d.join("key"), "short\n").unwrap();
        assert!(load(&d).is_none());
        std::fs::write(d.join("key"), format!("{}\n", "ab".repeat(32))).unwrap();
        assert!(load(&d).is_none(), "the wrong key");
        std::fs::remove_file(d.join("key")).unwrap();
        assert!(load(&d).is_none(), "no key file");
        assert!(set(&d, "x", "y").is_err());
    }

    #[test]
    fn the_secret_command_names_both_files() {
        let c = secret_command(Path::new("/ops/common"), "iyakki-platform");
        assert!(c.contains("-n iyakki-platform") && c.contains("/ops/common/vault.json") && c.contains("/ops/common/key"));
    }
}
