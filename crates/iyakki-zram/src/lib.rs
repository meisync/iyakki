//! Memory tuning for IYAKKI: zram swap (compressed RAM), an optional swap file, and the figures the SYSTEM tile shows.
//!
//! Everything works on a [`Env`] (where `/sys`, `/proc` and `/dev` are), so it is tested against a fake tree and never
//! touches the real machine in tests. Nothing here is fatal: every step reports a line for the boot log and carries on.
//! Details: `docs/ZRAM-AND-INSTALL.md`.

pub mod apply;
pub mod config;
pub mod mkswap;
pub mod stats;

pub use apply::{apply, preview, reconfigure, Env, KernelSwap, Swapper};
pub use config::{Config, Resolved, Size, ZramMode};
pub use stats::{MemStats, ZramStats};

/// Where the administrator's choices live (written by Settings and by the installer's first boot).
pub const CONF_PATH: &str = "/var/lib/iyakki/system.conf";
/// The swap file, on the data volume.
pub const SWAPFILE_PATH: &str = "/var/lib/iyakki/swapfile";
/// Swap label of the zram device.
pub const ZRAM_LABEL: &str = "IYAKKI-ZRAM";

/// Write the settings to `system.conf` atomically (temporary file, flush, rename), keeping keys this crate does not own.
pub fn save_conf(path: &std::path::Path, cfg: &Config) -> Result<(), String> {
    use std::io::Write;
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let text = config::render_conf(&cfg.into_conf(config::parse_conf(&old)));
    let tmp = path.with_extension("conf.tmp");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Set (`Some`) or remove (`None`) one key of `system.conf`, atomically (temporary file, flush, rename), keeping every other key.
pub fn set_conf_key(path: &std::path::Path, key: &str, value: Option<&str>) -> Result<(), String> {
    use std::io::Write;
    if key.is_empty() || key.contains(['=', '\n', '#']) || value.is_some_and(|v| v.contains(['\n', '#'])) {
        return Err(format!("not a valid setting: {key}"));
    }
    let mut conf = config::parse_conf(&std::fs::read_to_string(path).unwrap_or_default());
    match value {
        Some(v) => conf.insert(key.to_string(), v.to_string()),
        None => conf.remove(key),
    };
    let tmp = path.with_extension("conf.tmp");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(config::render_conf(&conf).as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_is_atomic_and_keeps_foreign_keys() {
        let dir = std::env::temp_dir().join(format!("iyakki-zsave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("sub/system.conf");
        let cfg = Config { size: Size::Percent(25), ..Config::default() };
        save_conf(&p, &cfg).unwrap();
        std::fs::write(&p, format!("{}hostname=box\n", std::fs::read_to_string(&p).unwrap())).unwrap();
        save_conf(&p, &Config { zram: ZramMode::Off, ..cfg.clone() }).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("zram=off") && t.contains("hostname=box"), "{t}");
        assert!(!p.with_extension("conf.tmp").exists(), "no temporary file is left");
        assert_eq!(config::resolve("", &t).config.zram, ZramMode::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_key_is_set_and_removed_without_touching_the_others() {
        let dir = std::env::temp_dir().join(format!("iyakki-zkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("system.conf");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, "zram=4G\nswappiness=80\nfuture-key=keep me\n").unwrap();
        set_conf_key(&p, "hostname", Some("kitchen")).unwrap();
        let c = config::parse_conf(&std::fs::read_to_string(&p).unwrap());
        assert_eq!((c["hostname"].as_str(), c["zram"].as_str(), c["swappiness"].as_str(), c["future-key"].as_str()), ("kitchen", "4G", "80", "keep me"));
        set_conf_key(&p, "hostname", Some("den")).unwrap();
        set_conf_key(&p, "zram", None).unwrap();
        let c = config::parse_conf(&std::fs::read_to_string(&p).unwrap());
        assert_eq!(c.get("hostname").map(String::as_str), Some("den"));
        assert!(!c.contains_key("zram") && c.contains_key("future-key"));
        assert!(set_conf_key(&p, "a=b", Some("x")).is_err() && set_conf_key(&p, "k", Some("x\ny")).is_err());
        assert!(!dir.join("system.conf.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
