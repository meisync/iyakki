//! The person's saved machines for the `remote_flow` node (F-FLOW-16): a short name, the address of another IYAKKI's web service,
//! and the NAME of the vault entry that holds the sign-in token there. The token itself is never in this file, in a flow or in a
//! chat tool call: only its vault name. A flow refers to a machine by name only, so a flow that came from outside cannot name an
//! address of its own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::remote::RemoteTarget;

pub const MAX_MACHINES: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    pub url: String,
    /// The vault entry holding the token (never the token).
    pub token: String,
}

pub struct Machines {
    path: PathBuf,
}

/// A vault entry name: letters, digits, `.`, `_`, `-`, optionally with the `uvault:` prefix.
pub fn vault_name(s: &str) -> Option<String> {
    let n = s.trim().strip_prefix("uvault:").unwrap_or(s.trim());
    (!n.is_empty() && n.len() <= 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))).then(|| n.to_string())
}

impl Machines {
    pub fn new(data: &Path) -> Machines {
        Machines { path: data.join("remote-machines.json") }
    }

    pub fn list(&self) -> Vec<Machine> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn get(&self, name: &str) -> Option<Machine> {
        self.list().into_iter().find(|m| m.name == name)
    }

    fn save(&self, v: &[Machine]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot save: {e}"))?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("cannot save: {e}"))?;
        f.write_all(serde_json::to_string_pretty(v).unwrap_or_default().as_bytes()).map_err(|e| format!("cannot save: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot save: {e}"))
    }

    /// Save (or replace) a machine. The address is checked like a [`RemoteTarget`] (https, host and port only).
    pub fn add(&self, name: &str, url: &str, token_vault: &str) -> Result<Machine, String> {
        if !iyakki_flow::kinds::remote_flow::valid_machine(name) {
            return Err("a machine name is letters, digits, - and _ (up to 40)".into());
        }
        RemoteTarget::new(url, Zeroizing::new("placeholder-token".into()))?;
        let token = vault_name(token_vault).ok_or("give the NAME of the vault entry that holds the token (never the token itself)")?;
        let m = Machine { name: name.into(), url: url.trim().trim_end_matches('/').into(), token };
        let mut v = self.list();
        v.retain(|x| x.name != m.name);
        if v.len() >= MAX_MACHINES {
            return Err(format!("at most {MAX_MACHINES} machines are kept"));
        }
        v.push(m.clone());
        v.sort_by(|a, b| a.name.cmp(&b.name));
        self.save(&v)?;
        Ok(m)
    }

    pub fn remove(&self, name: &str) -> Result<bool, String> {
        let mut v = self.list();
        let n = v.len();
        v.retain(|x| x.name != name);
        if v.len() == n {
            return Ok(false);
        }
        self.save(&v)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(t: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("machines-{t}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn machines_are_saved_by_name_with_a_vault_name_and_never_a_token() {
        let d = dir("a");
        let m = Machines::new(&d);
        assert!(m.list().is_empty());
        let a = m.add("lab", "https://lab.local:8443/", "uvault:lab-token").unwrap();
        assert_eq!((a.url.as_str(), a.token.as_str()), ("https://lab.local:8443", "lab-token"));
        m.add("attic", "https://attic.local:8443", "attic.token").unwrap();
        assert_eq!(m.list().iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["attic", "lab"]);
        // saving the same name replaces it
        m.add("lab", "https://lab2.local:8443", "lab-token").unwrap();
        assert_eq!(m.list().len(), 2);
        assert_eq!(m.get("lab").unwrap().url, "https://lab2.local:8443");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("remote-machines.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(m.remove("lab").unwrap() && !m.remove("lab").unwrap());
        assert_eq!(m.list().len(), 1);
    }

    #[test]
    fn bad_names_addresses_and_token_names_are_refused_without_echo() {
        let m = Machines::new(&dir("b"));
        assert!(m.add("a b", "https://x.local", "t").is_err());
        assert!(m.add("lab", "http://x.local", "t").is_err());
        assert!(m.add("lab", "https://u:p@x.local", "t").is_err());
        assert!(m.add("lab", "https://x.local/path", "t").is_err());
        // text that is not a vault entry name (it has spaces or other characters, as a pasted token may)
        let e = m.add("lab", "https://x.local", "eyJhbGciOi secret value!").unwrap_err();
        assert!(!e.contains("eyJ") && !e.contains("secret value"), "{e}");
        assert!(m.list().is_empty());
        for i in 0..MAX_MACHINES {
            m.add(&format!("m{i}"), "https://x.local", "t").unwrap();
        }
        assert!(m.add("one-more", "https://x.local", "t").is_err());
    }
}
