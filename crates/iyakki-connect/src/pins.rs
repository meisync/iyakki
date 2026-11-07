//! The per-user file of pinned certificates: one line `host:port SHA256HEX` per tenant, in the user's config directory, mode 600.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use iyakki_link::cloudclient::Pin;

/// `$IYAKKI_CONNECT_CONFIG`, else `$XDG_CONFIG_HOME/iyakki-connect`, else `$HOME/.config/iyakki-connect`.
pub fn config_dir() -> Result<PathBuf, String> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = var("IYAKKI_CONNECT_CONFIG") {
        return Ok(d);
    }
    if let Some(d) = var("XDG_CONFIG_HOME") {
        return Ok(d.join("iyakki-connect"));
    }
    var("HOME").map(|h| h.join(".config").join("iyakki-connect")).ok_or_else(|| "cannot find a configuration folder (set HOME or IYAKKI_CONNECT_CONFIG)".into())
}

/// The key a tenant is stored under: lower-case host, explicit port (443 when none).
pub fn key(addr: &str) -> Result<String, String> {
    let a = addr.trim();
    if a.is_empty() || a.len() > 300 || a.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
        return Err(format!("{a:?} is not an address (use HOST or HOST:PORT)"));
    }
    let (host, port) = if let Some(rest) = a.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or("an address in [ ] needs a closing ]")?;
        (format!("[{h}]"), tail.strip_prefix(':').unwrap_or("443").to_string())
    } else {
        match a.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), p.to_string()),
            _ => (a.to_string(), "443".to_string()),
        }
    };
    let port: u16 = port.parse().map_err(|_| format!("{port:?} is not a port"))?;
    Ok(format!("{}:{port}", host.to_ascii_lowercase()))
}

pub struct Store {
    file: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        Store { file: dir.join("pins") }
    }

    /// The folder the pins (and the device key) live in.
    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }

    pub fn list(&self) -> Result<Vec<(String, Pin)>, String> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(format!("{}: {e}", self.file.display())),
        };
        let mut out = Vec::new();
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#')) {
            let (k, p) = line.split_once(' ').and_then(|(k, p)| Pin::parse(p).map(|p| (k.to_string(), p))).ok_or_else(|| format!("{}: line {} is not understood", self.file.display(), n + 1))?;
            out.push((k, p));
        }
        Ok(out)
    }

    pub fn get(&self, addr: &str) -> Result<Option<Pin>, String> {
        let k = key(addr)?;
        Ok(self.list()?.into_iter().find(|(a, _)| *a == k).map(|(_, p)| p))
    }

    /// Store (or, with `replace`, change) the pin of `addr`.
    pub fn set(&self, addr: &str, pin: Pin) -> Result<(), String> {
        let k = key(addr)?;
        let mut all: Vec<_> = self.list()?.into_iter().filter(|(a, _)| *a != k).collect();
        all.push((k, pin));
        self.write(&all)
    }

    pub fn remove(&self, addr: &str) -> Result<bool, String> {
        let k = key(addr)?;
        let all = self.list()?;
        let kept: Vec<_> = all.iter().filter(|(a, _)| *a != k).cloned().collect();
        if kept.len() == all.len() {
            return Ok(false);
        }
        self.write(&kept)?;
        Ok(true)
    }

    fn write(&self, all: &[(String, Pin)]) -> Result<(), String> {
        let dir = self.file.parent().ok_or("no folder")?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut text = String::from("# IYAKKI connector: pinned tenant certificates (SHA-256). Edit with iyakki-connect pin / unpin.\n");
        for (a, p) in all {
            text.push_str(&format!("{a} {}\n", p.hex()));
        }
        let tmp = self.file.with_extension("new");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }
}
