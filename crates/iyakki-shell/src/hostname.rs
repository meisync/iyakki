//! The machine's host name: how it is shown, checked, saved and applied (Settings -> System -> Host name, `iyakki hostname`).
//!
//! The name is chosen in the installer or here and kept as `hostname=` in `/var/lib/iyakki/system.conf` (other keys are
//! left alone); boot applies it. The kernel option `iyakki.hostname=NAME` overrides the file and locks the page.

use std::path::Path;

/// Shown next to the field.
pub const HINT: &str = "Lower-case letters a-z, digits 0-9 and '-', up to 63 characters; it cannot start or end with '-'.";

/// The name now (the kernel's), else `fallback`.
pub fn current(fallback: &str) -> String {
    iyakki_devctl::current_hostname(fallback)
}

/// `Ok(name)` for a usable name (spaces around it dropped, capitals not accepted), else a plain sentence about what is wrong.
pub fn check(input: &str) -> Result<String, String> {
    let n = input.trim();
    if n.is_empty() {
        return Err("Type a name.".into());
    }
    if n.len() > 63 {
        return Err("A host name has at most 63 characters.".into());
    }
    if n.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(format!("Use lower-case letters only (try \"{}\").", n.to_ascii_lowercase()));
    }
    if n.starts_with('-') || n.ends_with('-') {
        return Err("A host name cannot start or end with '-'.".into());
    }
    if !iyakki_usb::install::valid_hostname(n) {
        return Err(format!("Only letters a-z, digits and '-' are allowed. {HINT}"));
    }
    Ok(n.to_string())
}

/// The boot option that fixes the name, when there is one (`iyakki.hostname=` on the kernel command line).
pub fn locked_by(cmdline: &str) -> Option<String> {
    cmdline.split_whitespace().find_map(|t| t.strip_prefix("iyakki.hostname=")).filter(|n| iyakki_usb::install::valid_hostname(n)).map(str::to_string)
}

/// The name saved for the next boots (`hostname=` in `system.conf`), if there is a valid one.
pub fn saved(conf: &Path) -> Option<String> {
    iyakki_zram::config::parse_conf(&std::fs::read_to_string(conf).ok()?).get("hostname").filter(|h| iyakki_usb::install::valid_hostname(h)).cloned()
}

/// Keep `name` for the next boots, leaving the other keys of the file as they are.
pub fn save(conf: &Path, name: &str) -> Result<(), String> {
    let name = check(name)?;
    iyakki_zram::set_conf_key(conf, "hostname", Some(&name))
}

/// Change the name of the running system and write `/etc/hostname`.
pub fn apply(name: &str, etc_hostname: &Path) -> Result<(), String> {
    let name = check(name)?;
    iyakki_devctl::apply_hostname(&name, etc_hostname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_with_a_reason() {
        assert_eq!(check("  kitchen-2 "), Ok("kitchen-2".into()));
        assert!(check("").unwrap_err().contains("Type"));
        assert!(check("Mac").unwrap_err().contains("\"mac\""));
        assert!(check("-a").unwrap_err().contains("start or end"));
        assert!(check("a b").unwrap_err().contains("Only letters"));
        assert!(check("a_b").is_err() && check("a.b").is_err());
        assert!(check(&"a".repeat(64)).unwrap_err().contains("63"));
        assert!(check(&"a".repeat(63)).is_ok());
    }

    #[test]
    fn saving_keeps_the_other_keys_and_the_boot_option_is_read() {
        let dir = std::env::temp_dir().join(format!("iyakki-hostname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("system.conf");
        std::fs::write(&conf, "zram=2G\nswapfile=off\nmystery=1\n").unwrap();
        assert_eq!(saved(&conf), None);
        save(&conf, "den").unwrap();
        assert_eq!(saved(&conf), Some("den".into()));
        let t = std::fs::read_to_string(&conf).unwrap();
        assert!(t.contains("zram=2G") && t.contains("swapfile=off") && t.contains("mystery=1"), "{t}");
        assert!(save(&conf, "Bad Name").is_err());
        assert_eq!(saved(&conf), Some("den".into()), "a refused name changes nothing");
        assert_eq!(locked_by("quiet iyakki.hostname=lab-7 console=tty0"), Some("lab-7".into()));
        assert_eq!(locked_by("iyakki.hostname=Bad_Name"), None);
        assert_eq!(locked_by("quiet"), None);
        assert!(apply("Bad Name", &dir.join("hostname")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
