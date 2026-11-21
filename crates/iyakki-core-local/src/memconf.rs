//! The memory choices of `system.conf` as a typed handle (F-ARCH-07): the chat tools of Settings -> Memory (zram, compressor,
//! swappiness, swap file, zswap, the experimental compressed-memory switch, the two boot-speed developer switches) read and write the
//! file through [`MemConf`] and never name the zram crate. The file is `<dir>/system.conf`; the kernel command line is given by the
//! caller (it is read in one place, `sysinfo::cmdline`). Writes keep every key this module does not own.

use std::path::{Path, PathBuf};

pub use iyakki_zram::bootspeed::BootSpeed;
pub use iyakki_zram::compressed::Experiment;
pub use iyakki_zram::config::{Config, Resolved, Size, ZramMode};

/// The file name inside the data folder.
pub const FILE: &str = "system.conf";
const KEY_EXPERIMENT: &str = "compressed-memory";

/// "4G", "512M", "2G": a size in bytes (a bare number is bytes).
pub fn parse_bytes(s: &str) -> Option<u64> {
    iyakki_zram::config::parse_bytes(s)
}

/// `off`, `auto`, a size or a percentage: the mode and the size.
pub fn parse_zram_value(v: &str) -> Option<(ZramMode, Size)> {
    iyakki_zram::config::parse_zram_value(v).map(|(m, s, _)| (m, s))
}

/// The handle on one `system.conf`.
#[derive(Clone, Debug)]
pub struct MemConf {
    path: PathBuf,
}

impl MemConf {
    /// The `system.conf` inside `dir`.
    pub fn in_dir(dir: &Path) -> MemConf {
        MemConf { path: dir.join(FILE) }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The file's text; empty when it is missing.
    pub fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
    /// The choices in force: command line over file, with the parts the command line fixed.
    pub fn resolve(&self, cmdline: &str) -> Resolved {
        iyakki_zram::config::resolve(cmdline, &self.text())
    }
    /// The file's own values (the command line does not count): what a change starts from.
    pub fn file_config(&self) -> Config {
        iyakki_zram::config::resolve("", &self.text()).config
    }
    /// Write all choices, keeping the other keys.
    pub fn save(&self, cfg: &Config) -> Result<(), String> {
        iyakki_zram::save_conf(&self.path, cfg)
    }
    /// The experimental compressed-memory switch in force and whether a boot option fixes it.
    pub fn experiment(&self, cmdline: &str) -> (Experiment, bool) {
        iyakki_zram::compressed::resolve_experiment(cmdline, &self.text())
    }
    pub fn set_experiment(&self, x: Experiment) -> Result<(), String> {
        iyakki_zram::set_conf_key(&self.path, KEY_EXPERIMENT, Some(iyakki_zram::compressed::conf_value(x)))
    }
    /// The two boot-speed developer switches in force.
    pub fn boot_speed(&self, cmdline: &str) -> BootSpeed {
        iyakki_zram::bootspeed::resolve(cmdline, &self.text())
    }
    /// Set the recording and/or the read-ahead of the boot files; `None` leaves one alone.
    pub fn set_boot_speed(&self, record: Option<bool>, prefetch: Option<bool>) -> Result<(), String> {
        use iyakki_zram::bootspeed::{conf_value, KEY_PREFETCH, KEY_RECORD};
        if let Some(r) = record {
            iyakki_zram::set_conf_key(&self.path, KEY_RECORD, Some(conf_value(r)))?;
        }
        if let Some(p) = prefetch {
            iyakki_zram::set_conf_key(&self.path, KEY_PREFETCH, Some(conf_value(p)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-memconf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_missing_file_reads_as_the_defaults_and_nothing_is_created_by_reading() {
        let d = dir("missing");
        let c = MemConf::in_dir(&d);
        assert_eq!(c.text(), "");
        assert_eq!(c.experiment("quiet"), (Experiment::Off, false));
        assert_eq!(c.boot_speed("quiet"), BootSpeed::default());
        assert_eq!(c.resolve("quiet").config.zram, ZramMode::On);
        assert!(!c.path().exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_saved_choice_is_read_back_and_the_other_keys_of_the_file_survive() {
        let d = dir("save");
        let c = MemConf::in_dir(&d);
        c.set_experiment(Experiment::On).unwrap();
        c.set_boot_speed(Some(true), None).unwrap();
        let mut cfg = c.file_config();
        cfg.swappiness = 60;
        cfg.zswap = true;
        c.save(&cfg).unwrap();
        c.set_boot_speed(None, Some(true)).unwrap();
        let text = c.text();
        assert!(text.contains("compressed-memory=on") && text.contains("boot-record=on") && text.contains("boot-prefetch=on"), "{text}");
        assert!(text.contains("swappiness=60") && text.contains("zswap=on"), "{text}");
        assert_eq!(c.experiment(""), (Experiment::On, false));
        let b = c.boot_speed("");
        assert!(b.record && b.prefetch);
        assert_eq!(c.resolve("").config.swappiness, 60);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_boot_option_wins_and_locks_but_the_file_values_stay_what_a_change_starts_from() {
        let d = dir("locked");
        let c = MemConf::in_dir(&d);
        let mut cfg = c.file_config();
        cfg.swappiness = 40;
        c.save(&cfg).unwrap();
        let r = c.resolve("iyakki.swappiness=10");
        assert_eq!((r.config.swappiness, r.swappiness_locked), (10, true));
        assert_eq!(c.file_config().swappiness, 40);
        assert_eq!(c.boot_speed("iyakki.prefetch=off").prefetch_locked, true);
        assert_eq!(c.experiment("iyakki.zram=exp").1, true);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_value_parsers_answer_sizes_and_modes_and_refuse_nonsense() {
        assert_eq!(parse_bytes("2G"), Some(2 << 30));
        assert_eq!(parse_bytes("lots"), None);
        assert_eq!(parse_zram_value("off").map(|x| x.0), Some(ZramMode::Off));
        assert!(parse_zram_value("auto").is_some());
        assert!(parse_zram_value("lots").is_none());
    }
}
