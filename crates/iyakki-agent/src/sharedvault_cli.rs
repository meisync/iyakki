//! The command-line side of the shared-vault provider (F-CLOUD-15): `iyakki vault secret put | get | list | delete` and the snapshot
//! keyring over the shared vault, so both follow the provider the setting names (built-in, or the person's own external system) instead
//! of always using the built-in client.
//!
//! * The setting is read as the pod reads it ([`Setting::load`]: the file a person saved, else the launch configuration, else built-in).
//!   With `external` nothing is read from or written to the built-in service and nothing falls back to it.
//! * The token for the external system is the NAME in the setting; the CLI reads its value from `--token-file FILE` (kept by the
//!   operator, mode 0600, never an option value). The built-in side needs `--vault-key FILE --as USER` (and the vault address options).
//! * `get` writes the value to `--out FILE` (a new 0600 file) and never prints it. `put` reads `--value-file FILE`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_shared::cloudvaultd::Scope;
use iyakki_shared::sharedvault::{Backend, Provider, SharedVault, Wiring};

use crate::cloudsnapshot::{key_id_ok, KeySource, SnapKey};

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

const CURRENT: &str = "snapshot-key.current";

/// Everything `open_vault` needs besides the data folder; tests pass fakes, the command line passes the real ones.
pub struct Deps {
    pub upstream: Arc<dyn iyakki_proxy::Upstream>,
    pub builtin: Option<Box<dyn Backend>>,
    /// The token of the external system (read from the operator's file).
    pub token: Option<zeroize::Zeroizing<String>>,
    pub actor: String,
}

/// The shared vault of `data` for the provider its setting names.
pub fn open_vault(data: &Path, d: Deps) -> Result<SharedVault, String> {
    let tok = d.token;
    SharedVault::open(Wiring {
        data_dir: data.into(),
        actor: d.actor,
        builtin: d.builtin,
        upstream: d.upstream,
        token: Box::new(move |_name| tok.clone().ok_or_else(|| "no token file was given".to_string())),
    })
}

/// The snapshot keys in the tenant scope of the shared vault: the built-in provider keeps the 32 raw bytes as before, an external one keeps
/// text, so the key is stored as 64 hex digits there. Both forms are read.
pub struct SharedKeyring<'a> {
    pub vault: &'a SharedVault,
}

impl SharedKeyring<'_> {
    fn value(&self, k: &SnapKey) -> Vec<u8> {
        match self.vault.provider() {
            Provider::Builtin => crate::cloudkeys::unhex(&k.to_hex()).unwrap_or_default(),
            Provider::External => k.to_hex().into_bytes(),
        }
    }

    pub fn rotate(&self) -> Result<SnapKey, String> {
        let k = SnapKey::generate()?;
        self.vault.put(&Scope::Tenant, &format!("snapshot-key.{}", k.id), &self.value(&k)).map_err(|e| format!("the vault refused the key: {e}"))?;
        self.vault.put(&Scope::Tenant, CURRENT, k.id.as_bytes()).map_err(|e| format!("the vault refused the key: {e}"))?;
        Ok(k)
    }
}

impl KeySource for SharedKeyring<'_> {
    fn current(&self) -> Result<SnapKey, String> {
        let id = self.vault.get(&Scope::Tenant, CURRENT).map_err(|e| format!("no current snapshot key in the vault ({e}); make one with: iyakki cloud snapshot key-rotate"))?;
        self.get(std::str::from_utf8(&id).map_err(|_| "the current key's id is damaged")?.trim())
    }
    fn get(&self, id: &str) -> Result<SnapKey, String> {
        if !key_id_ok(id) {
            return Err("that is not a key id".into());
        }
        let b = self.vault.get(&Scope::Tenant, &format!("snapshot-key.{id}")).map_err(|e| format!("key {id} is not in the vault ({e})"))?;
        let k = if b.len() == 32 { SnapKey::from_bytes(&b)? } else { SnapKey::from_hex(std::str::from_utf8(&b).map_err(|_| "the stored key is damaged")?)? };
        if k.id != id {
            return Err(format!("the vault's key {id} does not match its id"));
        }
        Ok(k)
    }
}

/// The real dependencies from the command line: `--token-file`, `--as`, and for the built-in provider `--vault-key` with the vault address.
pub fn deps_from_args(a: &[String]) -> Result<Deps, String> {
    let token = match opt(a, "--token-file") {
        Some(f) => Some(zeroize::Zeroizing::new(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?.trim().to_string())),
        None => None,
    };
    let actor = opt(a, "--as").unwrap_or_else(|| "operator".into());
    let builtin: Option<Box<dyn Backend>> = match opt(a, "--vault-key") {
        None => None,
        Some(kf) => {
            let env = |k: &str| std::env::var(k).ok();
            let link = crate::cloudvaultlink::Link::from_values(
                opt(a, "--vault").or_else(|| env(crate::cloudvaultlink::ENV_URL)).as_deref(),
                opt(a, "--vault-fingerprint").or_else(|| env(crate::cloudvaultlink::ENV_FINGERPRINT)).as_deref(),
                opt(a, "--tenant").or_else(|| env(crate::cloudvaultlink::ENV_TENANT)).as_deref(),
            );
            let keys = crate::cloudkeys::MemberKeys::from_hex(&std::fs::read_to_string(&kf).map_err(|e| format!("{kf}: {e}"))?)?;
            match link {
                crate::cloudvaultlink::Link::Target(t) => Some(Box::new(crate::sharedvault_builtin::Builtin(t.client(&actor, keys)?))),
                crate::cloudvaultlink::Link::NotConfigured => return Err("no vault address (--vault URL or IYAKKI_VAULT_URL)".into()),
                crate::cloudvaultlink::Link::Invalid(e) => return Err(format!("the vault address is not usable: {e}")),
            }
        }
    };
    Ok(Deps { upstream: Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(20))), builtin, token, actor })
}

/// `iyakki vault secret put | get | list | delete --scope S [--name N] ...` (the arguments after `secret`).
pub fn secret(a: &[String], d: Deps, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let usage = "usage: iyakki vault secret put|get|list|delete --data DIR --scope tenant|common|namespace:NAME [--name N] [--value-file F | --out F] [--token-file F] [--vault-key F --as USER]";
    let data = PathBuf::from(opt(a, "--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let scope = Scope::parse(&opt(a, "--scope").ok_or(usage)?).ok_or("the scope is tenant, common or namespace:NAME")?;
    let name = || opt(a, "--name").ok_or_else(|| "--name NAME is needed".to_string());
    let v = open_vault(&data, d)?;
    match a.first().map(String::as_str) {
        Some("put") => {
            let f = opt(a, "--value-file").ok_or("--value-file FILE is needed (a value is never taken from the command line)")?;
            let value = zeroize::Zeroizing::new(std::fs::read(&f).map_err(|e| format!("{f}: {e}"))?);
            v.put(&scope, &name()?, &value)?;
            out(&format!("{} stored in {} ({} shared vault)", name()?, scope.name(), v.provider().word()));
            Ok(())
        }
        Some("get") => {
            let f = opt(a, "--out").ok_or("--out FILE is needed (the value is written to a new file, never printed)")?;
            let value = zeroize::Zeroizing::new(v.get(&scope, &name()?)?);
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&f).map_err(|e| format!("{f}: {e}"))?;
            file.write_all(&value).map_err(|e| format!("{f}: {e}"))?;
            out(&format!("{} written to {f}", name()?));
            Ok(())
        }
        Some("list") => {
            let names = v.list(&scope)?;
            if names.is_empty() {
                out("no secrets in this scope");
            }
            for n in names {
                out(&n);
            }
            Ok(())
        }
        Some("delete") => {
            v.delete(&scope, &name()?)?;
            out(&format!("{} deleted from {}", name()?, scope.name()));
            Ok(())
        }
        _ => Err(usage.into()),
    }
}
