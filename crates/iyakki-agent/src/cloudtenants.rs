//! The tenants this machine remembers for the desktop connector (docs/development/CLOUD-MULTITENANT-PLAN.md, sections 3.3 and 3.4):
//! a name the person chose, the address (`host:port`), the certificate it was pinned to when the person trusted it (trust on first use,
//! like a paired device) and the account name used last. NEVER a password: the password is typed at every sign-in.
//!
//! The list is one small JSON file, written atomically (a temporary file, then a rename) with mode 0600.

use crate::cloudclient::Pin;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE: &str = "cloud-tenants.json";
/// Most tenants one machine remembers.
pub const MAX: usize = 32;
/// The port used when the person gives none.
pub const DEFAULT_PORT: u16 = 443;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tenant {
    pub name: String,
    /// `host:port`, normalised.
    pub addr: String,
    pub pin: Pin,
    /// The account used last at this tenant (empty when none yet).
    pub last_account: String,
}

#[derive(Serialize, Deserialize)]
struct Row {
    name: String,
    addr: String,
    pin: String,
    #[serde(default)]
    last_account: String,
}

#[derive(Default)]
pub struct Tenants {
    list: Vec<Tenant>,
}

/// Where the list lives for a data directory.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// A display name: 1 to 40 characters, no control characters, not only spaces.
pub fn valid_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() {
        return Err("Give the tenant a name".into());
    }
    if n.chars().count() > 40 {
        return Err("The name is longer than 40 characters".into());
    }
    if n.chars().any(char::is_control) {
        return Err("The name has a character that cannot be shown".into());
    }
    Ok(n.to_string())
}

/// An account name as typed at sign-in: no control characters, at most 64 characters (empty is allowed: nothing remembered).
pub fn valid_account(a: &str) -> String {
    a.trim().chars().filter(|c| !c.is_control()).take(64).collect()
}

/// `host`, `host:port`, `[v6]:port` or an IPv4/IPv6 address to the normalised `host:port` (port 443 when none).
pub fn normalise_addr(input: &str) -> Result<String, String> {
    let t = input.trim();
    let t = t.strip_prefix("https://").unwrap_or(t).trim_end_matches('/');
    if t.is_empty() {
        return Err("Give the tenant's address".into());
    }
    if t.contains('/') || t.chars().any(|c| c.is_whitespace() || c.is_control()) || t.contains('@') {
        return Err("The address is a host name or number with an optional port, like cloud.example.org:8443".into());
    }
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or("The address has an opening [ without a closing ]")?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return Err("Nothing may follow the closing ] except :port".into()),
        };
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("That is not an IPv6 address".into());
        }
        (format!("[{h}]"), port)
    } else if t.parse::<std::net::Ipv6Addr>().is_ok() {
        (format!("[{t}]"), None)
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (t.to_string(), None),
        }
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if !host.starts_with('[') && !valid_host(bare) {
        return Err("The host name has a character it cannot have (letters, digits, dots and dashes only)".into());
    }
    let port = match port {
        None => DEFAULT_PORT,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 => n,
            _ => return Err("The port is a number from 1 to 65535".into()),
        },
    };
    Ok(format!("{host}:{port}"))
}

fn valid_host(h: &str) -> bool {
    if h.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

impl Tenants {
    /// The list from `path`; a missing or damaged file is an empty list (a damaged row is dropped, not the whole list).
    pub fn load(path: &Path) -> Tenants {
        let rows: Vec<Row> = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut list = Vec::new();
        for r in rows {
            let (Some(pin), Ok(addr), Ok(name)) = (Pin::parse(&r.pin), normalise_addr(&r.addr), valid_name(&r.name)) else { continue };
            if list.len() < MAX && !list.iter().any(|t: &Tenant| t.name == name) {
                list.push(Tenant { name, addr, pin, last_account: valid_account(&r.last_account) });
            }
        }
        Tenants { list }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let rows: Vec<Row> = self.list.iter().map(|t| Row { name: t.name.clone(), addr: t.addr.clone(), pin: t.pin.hex(), last_account: t.last_account.clone() }).collect();
        let text = serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn list(&self) -> &[Tenant] {
        &self.list
    }

    pub fn get(&self, name: &str) -> Option<&Tenant> {
        self.list.iter().find(|t| t.name == name)
    }

    /// Remember a tenant (name and address are checked; the address is normalised). A name already used is refused: remove it first.
    pub fn add(&mut self, name: &str, addr: &str, pin: Pin) -> Result<&Tenant, String> {
        let name = valid_name(name)?;
        let addr = normalise_addr(addr)?;
        if self.list.iter().any(|t| t.name == name) {
            return Err(format!("There is already a tenant called {name}"));
        }
        if self.list.iter().any(|t| t.addr == addr) {
            return Err("That address is already in the list".into());
        }
        if self.list.len() >= MAX {
            return Err(format!("At most {MAX} tenants are remembered"));
        }
        self.list.push(Tenant { name, addr, pin, last_account: String::new() });
        Ok(self.list.last().expect("just pushed"))
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let n = self.list.len();
        self.list.retain(|t| t.name != name);
        self.list.len() != n
    }

    pub fn set_last_account(&mut self, name: &str, account: &str) {
        if let Some(t) = self.list.iter_mut().find(|t| t.name == name) {
            t.last_account = valid_account(account);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloudtenants-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn addresses_are_checked_and_get_a_port() {
        assert_eq!(normalise_addr("cloud.example.org").unwrap(), "cloud.example.org:443");
        assert_eq!(normalise_addr("https://acme.example.org:8443/").unwrap(), "acme.example.org:8443");
        assert_eq!(normalise_addr("10.0.0.7:9443").unwrap(), "10.0.0.7:9443");
        assert_eq!(normalise_addr("[::1]:8443").unwrap(), "[::1]:8443");
        assert_eq!(normalise_addr("::1").unwrap(), "[::1]:443");
        for bad in ["", "  ", "a b", "host:0", "host:70000", "host:x", "-bad.example", "a..b", "user@host", "host/path", "[::1", "[nope]:1", "ex_ample.org"] {
            assert!(normalise_addr(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(valid_name("  Acme  ").unwrap(), "Acme");
        assert!(valid_name("").is_err());
        assert!(valid_name("   ").is_err());
        assert!(valid_name(&"x".repeat(41)).is_err());
        assert!(valid_name("a\nb").is_err());
    }

    #[test]
    fn add_remove_and_duplicates() {
        let mut t = Tenants::default();
        t.add("Acme", "acme.example.org", Pin([1; 32])).unwrap();
        assert_eq!(t.get("Acme").unwrap().addr, "acme.example.org:443");
        assert!(t.add("Acme", "other.example.org", Pin([2; 32])).is_err(), "same name");
        assert!(t.add("Other", "acme.example.org:443", Pin([2; 32])).is_err(), "same address");
        assert!(t.add("Bad", "a b", Pin([2; 32])).is_err());
        t.set_last_account("Acme", "alice");
        assert_eq!(t.get("Acme").unwrap().last_account, "alice");
        assert!(t.remove("Acme"));
        assert!(!t.remove("Acme"));
        assert!(t.list().is_empty());
    }

    #[test]
    fn the_list_survives_a_restart_is_private_and_holds_no_password() {
        let d = dir("save");
        let p = path_in(&d);
        let mut t = Tenants::default();
        t.add("Acme", "acme.example.org:8443", Pin([0xab; 32])).unwrap();
        t.set_last_account("Acme", "alice");
        t.save(&p).unwrap();
        let back = Tenants::load(&p);
        assert_eq!(back.list(), t.list());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!p.with_extension("json.tmp").exists(), "the temporary file is gone");
        let text = std::fs::read_to_string(&p).unwrap().to_lowercase();
        assert!(!text.contains("password") && !text.contains("secret"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_or_damaged_file_is_an_empty_list_and_a_damaged_row_is_dropped() {
        let d = dir("damaged");
        let p = path_in(&d);
        assert!(Tenants::load(&p).list().is_empty());
        std::fs::write(&p, "not json").unwrap();
        assert!(Tenants::load(&p).list().is_empty());
        let good = Pin([3; 32]).hex();
        std::fs::write(&p, format!(r#"[{{"name":"A","addr":"a.example.org","pin":"{good}"}},{{"name":"B","addr":"b b","pin":"{good}"}},{{"name":"C","addr":"c.example.org","pin":"zz"}}]"#)).unwrap();
        let t = Tenants::load(&p);
        assert_eq!(t.list().len(), 1);
        assert_eq!(t.list()[0].name, "A");
        std::fs::remove_dir_all(&d).ok();
    }
}
