//! The per-user device key: one Ed25519 key pair in the configuration folder (file `device-key`, mode 600, folder mode 700), made here
//! (`key new`) or imported from an unencrypted OpenSSH `ssh-ed25519` private key (`key import-ssh PATH`). The secret is never printed;
//! `key show` prints the public key as an `authorized_keys`-style line, which is what the tenant's owner enrols.
//! RSA and every other key type are refused. Signing is the `IYAKKI-SIG` scheme (`iyakki_link::sigscheme`).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use iyakki_link::sigscheme::{hex, parse_openssh_private, DeviceKey};

const MAGIC: &str = "iyakki-connect-key-v1";

pub struct KeyFile {
    file: PathBuf,
}

impl KeyFile {
    pub fn new(dir: &Path) -> KeyFile {
        KeyFile { file: dir.join("device-key") }
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
        f.write_all(format!("{MAGIC} {}\n", key.seed_hex()).as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
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
        return Err("that is a public key: signing needs the private key file (usually the same name without .pub). A key kept only in ssh-agent cannot be imported yet".into());
    }
    parse_openssh_private(&text)
}

pub fn show(key: &DeviceKey) -> String {
    format!("{}\nfingerprint: {}\npublic (hex): {}", key.ssh_public_line("iyakki-connect"), fingerprint(key), hex(&key.public()))
}
