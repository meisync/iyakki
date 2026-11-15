//! Experimental compressed memory behind one Off/On setting (F-BOOT-31, owner decision in `docs/BOOT-SPEED.md`).
//!
//! Pure functions, no file or kernel access. Off (the default) changes nothing: zram 50 percent, swappiness 100, and the memory gate
//! counts no saved memory. On picks the mechanism from the machine: zswap in front of a small swap file when there is a data disk with room,
//! zram otherwise. The memory gate may count the memory the compression saved only while the experiment is On, and only half of it,
//! because compressed pages that are touched again come back at full size.
//!
//! Switches: kernel command line `iyakki.zram=exp` (or `iyakki.compressed=on|off`), then `system.conf` key `compressed-memory=on|off`.

use crate::config::{cmdline_param, flag, parse_conf, Config, Size, ZramMode, GIB, MIB};

/// The single setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Experiment {
    #[default]
    Off,
    On,
}

impl Experiment {
    pub fn id(self) -> &'static str {
        match self {
            Experiment::Off => "off",
            Experiment::On => "on",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Experiment::Off => "Compressed memory (experimental): off",
            Experiment::On => "Compressed memory (experimental): on",
        }
    }
}

/// Which mechanism the plan uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    /// The experiment is off: today's defaults, untouched.
    Defaults,
    /// zswap in front of a small swap file on the data disk.
    ZswapWithFile,
    /// zram only (no data disk, or too little room on it).
    ZramOnly,
}

/// What the setting resolves to, with the reason in plain words for Settings and the boot log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub mechanism: Mechanism,
    pub config: Config,
    pub reason: String,
}

/// The setting from the command line (wins, and is then locked) or `system.conf`. Second value: fixed by the command line.
pub fn resolve_experiment(cmdline: &str, conf: &str) -> (Experiment, bool) {
    let on = |b: bool| if b { Experiment::On } else { Experiment::Off };
    if let Some(v) = cmdline_param(cmdline, "iyakki.compressed").and_then(flag) {
        return (on(v), true);
    }
    if cmdline_param(cmdline, "iyakki.zram").is_some_and(|v| v.trim().eq_ignore_ascii_case("exp")) {
        return (Experiment::On, true);
    }
    let from_conf = parse_conf(conf).get("compressed-memory").and_then(|v| flag(v)).unwrap_or(false);
    (on(from_conf), false)
}

/// The swap file size used behind zswap: a quarter of RAM, 512 MiB to 4 GiB.
pub fn zswap_file_size(ram: u64) -> u64 {
    (ram / 4).clamp(512 * MIB, 4 * GIB) / MIB * MIB
}

/// The least free space on the data disk for the swap file: the file itself and as much again left for the person's files.
pub fn data_disk_room_needed(file: u64) -> u64 {
    file.saturating_mul(2).max(GIB)
}

/// Turn the setting into a configuration. `base` is what the person already chose; Off returns it as it is.
/// `data_free` is the free space on the data volume, `None` when there is no data disk (a live stick without storage).
pub fn plan(base: &Config, experiment: Experiment, data_free: Option<u64>, ram: u64) -> Plan {
    if experiment == Experiment::Off {
        return Plan { mechanism: Mechanism::Defaults, config: base.clone(), reason: "off: defaults unchanged".into() };
    }
    let file = zswap_file_size(ram);
    match data_free {
        Some(free) if free >= data_disk_room_needed(file) => {
            let config = Config { zram: ZramMode::Off, zswap: true, swapfile: Some(file), ..base.clone() };
            Plan { mechanism: Mechanism::ZswapWithFile, config, reason: format!("on: zswap with a {} MiB swap file on the data disk", file / MIB) }
        }
        Some(_) => {
            let config = Config { zram: ZramMode::On, size: Size::Auto, zswap: false, swapfile: None, ..base.clone() };
            Plan { mechanism: Mechanism::ZramOnly, config, reason: "on: the data disk has too little free room for a swap file, zram only".into() }
        }
        None => {
            let config = Config { zram: ZramMode::On, size: Size::Auto, zswap: false, swapfile: None, ..base.clone() };
            Plan { mechanism: Mechanism::ZramOnly, config, reason: "on: no data disk, zram only".into() }
        }
    }
}

/// Free bytes (for a normal writer) on the filesystem holding `path`; `None` if it cannot be asked.
#[cfg(unix)]
pub fn data_free(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: a valid NUL-terminated path and a zeroed statvfs of the right type, read only after the call reports success.
    let s = unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        s
    };
    Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

/// The value written to `system.conf` for the setting.
pub fn conf_value(e: Experiment) -> &'static str {
    e.id()
}

/// The memory the compression saved that the memory gate may count: half of (stored uncompressed minus RAM used for it),
/// and nothing unless the experiment is On.
pub fn gate_credit(experiment: Experiment, stored_orig: u64, ram_used: u64) -> u64 {
    if experiment == Experiment::Off {
        return 0;
    }
    stored_orig.saturating_sub(ram_used) / 2
}

/// `MemAvailable` plus the credit, never more than `MemTotal` (the gate must not believe in memory the machine does not have).
pub fn available_with_credit(available: u64, total: u64, credit: u64) -> u64 {
    available.saturating_add(credit).min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_the_default_and_changes_nothing() {
        assert_eq!(resolve_experiment("quiet", ""), (Experiment::Off, false));
        let base = Config::default();
        let p = plan(&base, Experiment::Off, Some(100 * GIB), 8 * GIB);
        assert_eq!((p.mechanism, p.config), (Mechanism::Defaults, base));
        assert_eq!(gate_credit(Experiment::Off, 4 * GIB, GIB), 0);
    }

    #[test]
    fn the_command_line_beats_the_file_and_locks() {
        assert_eq!(resolve_experiment("iyakki.zram=exp", "compressed-memory=off"), (Experiment::On, true));
        assert_eq!(resolve_experiment("iyakki.compressed=off", "compressed-memory=on"), (Experiment::Off, true));
        assert_eq!(resolve_experiment("quiet", "compressed-memory=on\n"), (Experiment::On, false));
        assert_eq!(resolve_experiment("iyakki.zram=4G", "compressed-memory=maybe"), (Experiment::Off, false));
    }

    #[test]
    fn a_data_disk_with_room_gets_zswap_and_a_small_file() {
        let p = plan(&Config::default(), Experiment::On, Some(50 * GIB), 8 * GIB);
        assert_eq!(p.mechanism, Mechanism::ZswapWithFile);
        assert!(p.config.zswap && p.config.zram == ZramMode::Off);
        assert_eq!(p.config.swapfile, Some(2 * GIB));
        // the person's swappiness survives
        let mut b = Config::default();
        b.swappiness = 60;
        assert_eq!(plan(&b, Experiment::On, Some(50 * GIB), 8 * GIB).config.swappiness, 60);
    }

    #[test]
    fn no_disk_or_a_full_disk_falls_back_to_zram() {
        let none = plan(&Config::default(), Experiment::On, None, 4 * GIB);
        assert_eq!(none.mechanism, Mechanism::ZramOnly);
        assert!(none.config.zram == ZramMode::On && !none.config.zswap && none.config.swapfile.is_none());
        let tight = plan(&Config::default(), Experiment::On, Some(GIB), 4 * GIB);
        assert_eq!(tight.mechanism, Mechanism::ZramOnly);
        assert!(tight.reason.contains("too little"));
    }

    #[test]
    fn the_free_space_of_a_folder_is_asked_from_the_system_and_the_setting_round_trips_through_system_conf() {
        let d = std::env::temp_dir();
        assert!(data_free(&d).is_some_and(|f| f > 0));
        assert_eq!(data_free(std::path::Path::new("/no/such/folder/at/all")), None);
        let dir = std::env::temp_dir().join(format!("iyakki-compressed-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("system.conf");
        std::fs::write(&p, "zram=4G\nhostname=box\n").unwrap();
        crate::set_conf_key(&p, "compressed-memory", Some(conf_value(Experiment::On))).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(resolve_experiment("", &text), (Experiment::On, false));
        assert!(text.contains("hostname=box") && text.contains("zram=4G"), "other keys stay: {text}");
        crate::set_conf_key(&p, "compressed-memory", Some(conf_value(Experiment::Off))).unwrap();
        assert_eq!(resolve_experiment("", &std::fs::read_to_string(&p).unwrap()).0, Experiment::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_size_is_a_clamped_quarter_of_ram() {
        assert_eq!(zswap_file_size(GIB), 512 * MIB);
        assert_eq!(zswap_file_size(8 * GIB), 2 * GIB);
        assert_eq!(zswap_file_size(64 * GIB), 4 * GIB);
        assert_eq!(data_disk_room_needed(4 * GIB), 8 * GIB);
        assert_eq!(data_disk_room_needed(512 * MIB), GIB);
    }

    #[test]
    fn the_gate_counts_half_the_saving_only_when_on_and_never_past_total() {
        // 3 GiB stored in 1 GiB of RAM: saved 2 GiB, credit 1 GiB
        assert_eq!(gate_credit(Experiment::On, 3 * GIB, GIB), GIB);
        // a pool that grew past what it stores saves nothing
        assert_eq!(gate_credit(Experiment::On, GIB, 2 * GIB), 0);
        assert_eq!(available_with_credit(GIB, 8 * GIB, GIB), 2 * GIB);
        assert_eq!(available_with_credit(7 * GIB + GIB / 2, 8 * GIB, 4 * GIB), 8 * GIB);
        assert_eq!(available_with_credit(u64::MAX, 8 * GIB, 5), 8 * GIB);
    }
}
