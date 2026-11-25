//! The kubeconfig profiles held in a vault entry (`devcluster-vault-<user>.json`); re-exported by `iyakki_shared::kubeprofiles`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub fn is_vault_ref(s: &str) -> bool {
    s.starts_with("vault:") || s.starts_with("uvault:")
}

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
    if !crate::ids::valid_secret_name(entry) {
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
mod tests {
    use super::*;

    #[test]
    fn only_vault_and_uvault_prefixed_references_are_vault_references() {
        assert!(is_vault_ref("vault:prod-key"));
        assert!(is_vault_ref("uvault:kube.one"));
        assert!(!is_vault_ref("/etc/kube/kubeconfig"));
        assert!(!is_vault_ref("Vault:upper"));
        assert!(!is_vault_ref(""));
    }

    #[test]
    fn the_profile_file_is_named_after_the_user_with_unsafe_characters_removed() {
        assert_eq!(vault_path(Path::new("/d"), "admin.team"), PathBuf::from("/d/devcluster-vault-adminteam.json"));
    }

    #[test]
    fn a_profile_serialises_its_name_and_reference_in_that_order() {
        let p = VaultProfile { name: "Lab".into(), reference: "uvault:kube".into() };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"name":"Lab","reference":"uvault:kube"}"#);
    }

    #[test]
    fn adding_a_profile_refuses_an_empty_or_blank_name_before_touching_the_store() {
        let dir = Path::new("/nonexistent-iyakki-profiles");
        assert!(vault_add(dir, "admin", "", "vault:kube").unwrap_err().starts_with("a profile name is letters"));
        assert!(vault_add(dir, "admin", "   ", "vault:kube").is_err());
    }

    #[test]
    fn adding_a_profile_refuses_a_name_with_a_slash_or_longer_than_40_characters() {
        let dir = Path::new("/nonexistent-iyakki-profiles");
        assert!(vault_add(dir, "admin", "bad/name", "vault:kube").is_err());
        assert!(vault_add(dir, "admin", &"a".repeat(41), "vault:kube").is_err());
    }

    #[test]
    fn adding_a_profile_refuses_a_vault_entry_name_that_is_not_a_secret_name() {
        let dir = Path::new("/nonexistent-iyakki-profiles");
        let err = vault_add(dir, "admin", "Lab", "vault:Bad Name").unwrap_err();
        assert!(err.starts_with("a vault entry name is lowercase"));
        assert!(vault_add(dir, "admin", "Lab", "uvault:").is_err());
    }
}
