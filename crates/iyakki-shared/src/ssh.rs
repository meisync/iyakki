//! Types moved out of `iyakki_agent::ssh` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Who is asking. The assistant gets the strictest rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Assistant,
    /// Typed by the person themselves (the terminal, the Connections tab).
    Person,
}


use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

pub const KEY_PREFIX: &str = "ssh.";

/// One line of a `known_hosts` file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostKey {
    /// `host`, or `[host]:port` when the port is not 22.
    pub pattern: String,
    pub kind: String,
    pub b64: String,
}

impl HostKey {
    /// `SHA256:…`, as OpenSSH prints it.
    pub fn fingerprint(&self) -> String {
        let blob = crate::b64::b64_decode(&self.b64).unwrap_or_default();
        let digest = Sha256::digest(&blob);
        format!("SHA256:{}", crate::b64::base64_encode(&digest).trim_end_matches('='))
    }

    pub fn line(&self) -> String {
        format!("{} {} {}", self.pattern, self.kind, self.b64)
    }
}

pub fn pattern(host: &str, port: u16) -> String {
    if port == 22 { host.to_ascii_lowercase() } else { format!("[{}]:{port}", host.to_ascii_lowercase()) }
}


fn default_port() -> u16 {
    22
}

/// A saved host. No secret is stored: the key is a vault name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Connection {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// The vault name of the private key (`ssh.<x>`), or empty.
    #[serde(default)]
    pub key: String,
    /// What to run when connecting; empty runs a quick check (`true`).
    #[serde(default)]
    pub command: String,
}

impl Connection {
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || n.len() > 40 || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')) {
            return Err("the name is 1 to 40 letters, digits, spaces, `.`, `_` or `-`".into());
        }
        if !valid_host(&self.host) {
            return Err(format!("`{}` is not a host name or address", self.host));
        }
        if self.port == 0 {
            return Err("the port is 1 to 65535".into());
        }
        if !self.user.is_empty() && !valid_user(&self.user) {
            return Err(format!("`{}` is not a user name", self.user));
        }
        if !self.key.is_empty() && !valid_key_name(&self.key) {
            return Err(format!("`{}` is not a vault key name (ssh.<name>)", self.key));
        }
        if self.command.len() > 500 || self.command.contains(|c: char| c.is_control()) {
            return Err("the default command is up to 500 characters on one line".into());
        }
        crate::exec::split_words(&self.command)?;
        Ok(())
    }

    /// `user@host`, or just the host.
    pub fn target(&self) -> String {
        if self.user.is_empty() { self.host.clone() } else { format!("{}@{}", self.user, self.host) }
    }

    /// The arguments of an interactive login: a terminal is asked for (`-t`, so also a saved default command gets one) and, without a
    /// default command, the remote side gives the person its login shell. [`Connection::args`] is the other one: it ends in `true`.
    pub fn login_args(&self) -> Vec<String> {
        let mut a = vec!["-p".to_string(), self.port.to_string()];
        if !self.key.is_empty() {
            a.extend(["-i".to_string(), format!("uvault:{}", self.key)]);
        }
        a.push("-t".into());
        a.push(self.target());
        a.extend(crate::exec::split_words(&self.command).unwrap_or_default());
        a
    }

    /// The arguments of the `ssh` command that connects (the default command, or `true` for a quick check).
    pub fn args(&self) -> Vec<String> {
        let mut a = vec!["-p".to_string(), self.port.to_string()];
        if !self.key.is_empty() {
            a.extend(["-i".to_string(), format!("uvault:{}", self.key)]);
        }
        a.push(self.target());
        let cmd = crate::exec::split_words(&self.command).unwrap_or_default();
        if cmd.is_empty() {
            a.push("true".into());
        } else {
            a.extend(cmd);
        }
        a
    }
}


pub fn valid_key_name(n: &str) -> bool {
    n.len() > KEY_PREFIX.len() && n.starts_with(KEY_PREFIX) && !n.ends_with(".passphrase") && crate::vault::valid_secret_name(n)
}

pub fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['-', '.']) && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

pub fn valid_user(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}


/// A one-line fingerprint list for the screen and the terminal.
pub fn describe_keys(keys: &[HostKey]) -> Vec<String> {
    keys.iter().map(|k| format!("{} {}", k.kind, k.fingerprint())).collect()
}


pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    pub fn new(path: &Path) -> KnownHosts {
        KnownHosts { path: path.into() }
    }

    pub fn read(&self) -> Vec<HostKey> {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        text.lines().filter_map(parse_known_line).collect()
    }

    pub fn for_host(&self, host: &str, port: u16) -> Vec<HostKey> {
        let want = pattern(host, port);
        self.read().into_iter().filter(|k| k.pattern == want).collect()
    }

    fn write(&self, keys: &[HostKey]) -> Result<(), String> {
        let mut text = String::new();
        for k in keys {
            text.push_str(&k.line());
            text.push('\n');
        }
        write_private(&self.path, text.as_bytes())
    }

    /// Records these keys for the host, replacing what was there (the person has confirmed them).
    pub fn trust(&self, host: &str, port: u16, keys: &[HostKey]) -> Result<(), String> {
        if keys.is_empty() {
            return Err("there is no host key to trust".into());
        }
        let want = pattern(host, port);
        let mut all: Vec<HostKey> = self.read().into_iter().filter(|k| k.pattern != want).collect();
        for k in keys {
            let k = HostKey { pattern: want.clone(), ..k.clone() };
            if !all.contains(&k) {
                all.push(k);
            }
        }
        self.write(&all)
    }

    pub fn forget(&self, host: &str, port: u16) -> Result<usize, String> {
        let want = pattern(host, port);
        let all = self.read();
        let keep: Vec<HostKey> = all.iter().filter(|k| k.pattern != want).cloned().collect();
        let gone = all.len() - keep.len();
        if gone > 0 {
            self.write(&keep)?;
        }
        Ok(gone)
    }
}

pub fn parse_known_line(l: &str) -> Option<HostKey> {
    let l = l.trim();
    if l.is_empty() || l.starts_with('#') || l.starts_with('|') || l.starts_with('@') {
        return None;
    }
    let mut it = l.split_whitespace();
    let (hosts, kind, b64) = (it.next()?, it.next()?, it.next()?);
    if !kind.starts_with("ssh-") && !kind.starts_with("ecdsa-") && !kind.starts_with("sk-") {
        return None;
    }
    crate::models::b64_decode(b64)?;
    Some(HostKey { pattern: hosts.split(',').next()?.to_ascii_lowercase(), kind: kind.into(), b64: b64.into() })
}


pub fn private_dir(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| format!("could not create {}: {e}", p.display()))?;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        private_dir(d)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

