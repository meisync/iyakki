//! The per-user device key: one Ed25519 key pair in the configuration folder (file `device-key`, mode 600, folder mode 700), made here
//! (`key new`) or imported from an unencrypted OpenSSH `ssh-ed25519` private key (`key import-ssh PATH`). The secret is never printed;
//! `key show` prints the public key as an `authorized_keys`-style line, which is what the tenant's owner enrols.
//! RSA and every other key type are refused. Signing is the `IYAKKI-SIG` scheme (`iyakki_link::sigscheme`).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use iyakki_link::sigscheme::{hex, parse_openssh_private, DeviceKey};

const MAGIC: &str = "iyakki-connect-key-v1";
/// A key that only an ssh-agent holds: the file keeps the public key (hex) and nothing secret; the agent is found through `SSH_AUTH_SOCK`.
const AGENT_MAGIC: &str = "iyakki-connect-agent-v1";

pub struct KeyFile {
    file: PathBuf,
    /// The system's key store, when the person chose it (`IYAKKI_CONNECT_KEYSTORE=os`): the secret goes there and the file keeps a pointer.
    os: Option<(std::sync::Arc<crate::keystore::OsStore>, String)>,
}

/// A key whose secret is in the system's key store: the file holds this word, the account and the public key (hex), and nothing secret.
const OS_MAGIC: &str = "iyakki-connect-os-v1";

impl KeyFile {
    pub fn new(dir: &Path) -> KeyFile {
        KeyFile { file: dir.join("device-key"), os: None }
    }

    /// A key file at an exact path (the window program keeps one per instance).
    pub fn at(file: PathBuf) -> KeyFile {
        KeyFile { file, os: None }
    }

    /// Keep the secret in the system's key store under `account` instead of in the file.
    pub fn with_os_store(mut self, store: std::sync::Arc<crate::keystore::OsStore>, account: &str) -> KeyFile {
        self.os = Some((store, account.to_string()));
        self
    }

    pub fn load(&self) -> Result<Option<DeviceKey>, String> {
        let meta = match std::fs::metadata(&self.file) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", self.file.display())),
        };
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!("{} can be read by other people (mode {:o}): run `chmod 600 {}` or make a new key", self.file.display(), meta.permissions().mode() & 0o777, self.file.display()));
        }
        let text = std::fs::read_to_string(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))?;
        if let Some(hexpub) = text.trim().strip_prefix(AGENT_MAGIC) {
            let public = iyakki_link::sigscheme::unhex(hexpub.trim()).and_then(|b| <[u8; 32]>::try_from(b).ok()).ok_or_else(|| format!("{} is damaged", self.file.display()))?;
            let agent = iyakki_link::sshagent::Agent::from_env_value(std::env::var("SSH_AUTH_SOCK").ok().as_deref())?;
            return Ok(Some(DeviceKey::from_agent(public, agent.sock().to_path_buf())));
        }
        if let Some(rest) = text.trim().strip_prefix(OS_MAGIC) {
            let account = rest.split_whitespace().next().ok_or_else(|| format!("{} is damaged", self.file.display()))?;
            let (store, _) = self.os.as_ref().ok_or_else(|| format!("{} keeps its key in the system key store: set IYAKKI_CONNECT_KEYSTORE=os", self.file.display()))?;
            let seed = store.get(account)?.ok_or_else(|| "the system key store has no key for this device any more: enrol it again".to_string())?;
            return DeviceKey::from_hex(&seed).map(Some);
        }
        let seed = text.trim().strip_prefix(MAGIC).ok_or_else(|| format!("{} is not a device key file", self.file.display()))?;
        DeviceKey::from_hex(seed).map(Some)
    }

    pub fn store(&self, key: &DeviceKey, force: bool) -> Result<(), String> {
        let dir = self.file.parent().ok_or("no folder")?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if self.file.exists() && !force {
            return Err(format!("there is already a device key ({}): use --force to replace it (the old one is lost)", self.file.display()));
        }
        let tmp = self.file.with_extension("new");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let line = match key.seed_hex() {
            Some(seed) if self.os.is_some() => {
                let (store, account) = self.os.as_ref().expect("checked");
                store.set(account, &seed)?;
                format!("{OS_MAGIC} {account} {}\n", hex(&key.public()))
            }
            Some(seed) => format!("{MAGIC} {seed}\n"),
            None => format!("{AGENT_MAGIC} {}\n", hex(&key.public())),
        };
        f.write_all(line.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

pub fn fingerprint(key: &DeviceKey) -> String {
    iyakki_link::sigscheme::fingerprint(&key.public())
}

/// The key from an OpenSSH file the person names. A public key file is explained, not accepted: signing needs the private half.
pub fn import_ssh(path: &Path) -> Result<DeviceKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.trim_start().starts_with("ssh-") && !text.contains("PRIVATE KEY") {
        return Err("that is a public key: signing needs the private key file (usually the same name without .pub). A key kept only in ssh-agent: use `key import-agent`".into());
    }
    parse_openssh_private(&text)
}

pub fn show(key: &DeviceKey) -> String {
    format!("{}\nfingerprint: {}\npublic (hex): {}", key.ssh_public_line("iyakki-connect"), fingerprint(key), hex(&key.public()))
}

/// Use a key an ssh-agent holds (`key import-agent [COMMENT|SHA256:FINGERPRINT]`). `sock` is `SSH_AUTH_SOCK`. With no selector the agent must
/// hold exactly one ssh-ed25519 key. The secret never leaves the agent; only the public key is kept.
pub fn import_agent(sock: Option<&str>, selector: Option<&str>) -> Result<DeviceKey, String> {
    let agent = iyakki_link::sshagent::Agent::from_env_value(sock)?;
    let ids = agent.ed25519_keys()?;
    let pick: Vec<_> = ids
        .iter()
        .filter(|i| match selector {
            None => true,
            Some(s) => i.comment == s || iyakki_link::sigscheme::fingerprint(&i.public) == s,
        })
        .collect();
    match pick.as_slice() {
        [one] => Ok(DeviceKey::from_agent(one.public, agent.sock().to_path_buf())),
        [] if ids.is_empty() => Err("the ssh-agent holds no ssh-ed25519 key (ssh-add one; RSA and ECDSA are not supported)".into()),
        [] => Err(format!("no key of the ssh-agent matches that name; it holds: {}", ids.iter().map(|i| format!("{} ({})", i.comment, iyakki_link::sigscheme::fingerprint(&i.public))).collect::<Vec<_>>().join(", "))),
        many => Err(format!("the ssh-agent holds several keys; name one: {}", many.iter().map(|i| format!("{} ({})", i.comment, iyakki_link::sigscheme::fingerprint(&i.public))).collect::<Vec<_>>().join(", "))),
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn an_agent_key_is_stored_as_a_public_pointer_with_no_secret_and_a_missing_agent_is_explained() {
        let d = std::env::temp_dir().join(format!("iyakki-devkey-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let public = DeviceKey::from_seed([3; 32]).public();
        let key = DeviceKey::from_agent(public, d.join("agent.sock"));
        let kf = KeyFile::new(&d);
        kf.store(&key, false).unwrap();
        let text = std::fs::read_to_string(d.join("device-key")).unwrap();
        assert_eq!(text.trim(), format!("iyakki-connect-agent-v1 {}", hex(&public)));
        assert!(!text.contains(&DeviceKey::from_seed([3; 32]).seed_hex().unwrap()));
        assert!(import_agent(None, None).unwrap_err().contains("SSH_AUTH_SOCK"));
        assert!(import_agent(Some(d.join("none.sock").to_str().unwrap()), None).unwrap_err().contains("did not answer"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3").join(name)
    }

    /// `tests/compat/alpha3/connect-agent-key.txt` is what alpha.3 wrote for a key that only an ssh-agent holds.
    #[test]
    fn the_agent_key_file_of_alpha3_is_still_read_and_a_newer_or_damaged_one_is_refused_in_words() {
        let _shared = iyakki_testkit::lock(iyakki_testkit::names::ENV); // flaky: the process-wide state below is shared with the other tests of this run
        let d = std::env::temp_dir().join(format!("iyakki-devkey-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let put = |text: &str| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(d.join("device-key"), text).unwrap();
            std::fs::set_permissions(d.join("device-key"), std::fs::Permissions::from_mode(0o600)).unwrap();
            KeyFile::new(&d).load()
        };
        // the pointer is recognised (the failure is about the missing ssh-agent, not about the file)
        let saved = std::env::var("SSH_AUTH_SOCK").ok();
        std::env::remove_var("SSH_AUTH_SOCK");
        let err = put(&std::fs::read_to_string(fixture("connect-agent-key.txt")).unwrap()).map(|_| ()).unwrap_err();
        assert!(err.contains("SSH_AUTH_SOCK") && !err.contains("damaged") && !err.contains("not a device key file"), "{err}");
        // a newer word is not guessed at; a cut public key is called damaged
        assert!(put("iyakki-connect-agent-v2 00\n").map(|_| ()).unwrap_err().contains("not a device key file"));
        assert!(put("iyakki-connect-agent-v1 0011\n").map(|_| ()).unwrap_err().contains("damaged"));
        if let Some(v) = saved {
            std::env::set_var("SSH_AUTH_SOCK", v);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod os_tests {
    use super::*;
    use crate::keystore::{Flavour, OsStore, Ran, Runner};
    use std::sync::{Arc, Mutex};

    /// A key store in memory: `store` keeps the secret from standard input, `lookup` returns it.
    #[derive(Default)]
    struct Mem(Mutex<Option<String>>);
    impl Runner for Arc<Mem> {
        fn run(&self, _: &str, a: &[String], i: Option<&str>) -> Result<Ran, String> {
            match a[0].as_str() {
                "store" => *self.0.lock().unwrap() = i.map(str::to_string),
                "clear" => *self.0.lock().unwrap() = None,
                _ => {}
            }
            Ok(Ran { ok: true, stdout: if a[0] == "lookup" { self.0.lock().unwrap().clone().unwrap_or_default() } else { String::new() } })
        }
    }

    #[test]
    fn with_the_system_key_store_the_file_holds_no_secret_and_the_key_comes_back_from_the_store() {
        let d = std::env::temp_dir().join(format!("iyakki-devkey-os-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mem = Arc::new(Mem::default());
        let store = Arc::new(OsStore::new(Flavour::SecretService, Box::new(mem.clone())));
        let key = DeviceKey::from_seed([9; 32]);
        let seed = key.seed_hex().unwrap();
        let kf = KeyFile::new(&d).with_os_store(store.clone(), "acct1");
        kf.store(&key, false).unwrap();
        let text = std::fs::read_to_string(d.join("device-key")).unwrap();
        assert!(text.starts_with("iyakki-connect-os-v1 acct1 ") && !text.contains(&seed), "{text}");
        assert_eq!(kf.load().unwrap().unwrap().public(), key.public());
        // without the store setting the file cannot be used, and says why
        assert!(KeyFile::new(&d).load().unwrap_err().contains("IYAKKI_CONNECT_KEYSTORE"));
        // the store lost the key (the keychain was reset): a clear message, not a crash
        store.delete("acct1").unwrap();
        assert!(kf.load().unwrap_err().contains("enrol it again"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
