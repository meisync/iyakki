//! The two developer switches of the boot-order prefetch (F-BOOT-30), as the person and the kernel command line set them: record which
//! files the first 90 seconds of a boot open (`boot-record=on` in `system.conf`, or `iyakki.bootrecord=1` on the command line), and
//! read those files into the page cache in the background after the critical path (`boot-prefetch=on`, or `iyakki.prefetch=on`).
//! Both are off unless asked for. The command line wins and then fixes the choice (Settings shows it locked), like the other memory
//! choices. Pure: text in, answer out.

use crate::config::{cmdline_param, flag, parse_conf};

pub const KEY_RECORD: &str = "boot-record";
pub const KEY_PREFETCH: &str = "boot-prefetch";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootSpeed {
    pub record: bool,
    pub prefetch: bool,
    /// Fixed by the kernel command line (it cannot be changed from Settings).
    pub record_locked: bool,
    pub prefetch_locked: bool,
}

/// The choices in force: command line over `system.conf` over off.
pub fn resolve(cmdline: &str, conf: &str) -> BootSpeed {
    let m = parse_conf(conf);
    let mut b = BootSpeed { record: m.get(KEY_RECORD).and_then(|v| flag(v)).unwrap_or(false), prefetch: m.get(KEY_PREFETCH).and_then(|v| flag(v)).unwrap_or(false), ..Default::default() };
    if let Some(v) = cmdline_param(cmdline, "iyakki.bootrecord").and_then(flag) {
        (b.record, b.record_locked) = (v, true);
    }
    if let Some(v) = cmdline_param(cmdline, "iyakki.prefetch").and_then(flag) {
        (b.prefetch, b.prefetch_locked) = (v, true);
    }
    b
}

/// The value written to `system.conf`.
pub fn conf_value(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_are_off_unless_asked_for_and_the_command_line_wins_and_locks() {
        assert_eq!(resolve("quiet", ""), BootSpeed::default());
        let f = resolve("quiet", "boot-record=on\nboot-prefetch=on\n");
        assert_eq!((f.record, f.prefetch, f.record_locked, f.prefetch_locked), (true, true, false, false));
        let c = resolve("iyakki.bootrecord=1 iyakki.prefetch=on", "boot-record=off\nboot-prefetch=off\n");
        assert_eq!((c.record, c.prefetch, c.record_locked, c.prefetch_locked), (true, true, true, true));
        let off = resolve("iyakki.prefetch=off", "boot-prefetch=on\n");
        assert_eq!((off.prefetch, off.prefetch_locked), (false, true));
        // not understood: as if absent
        assert_eq!(resolve("iyakki.prefetch=maybe", "boot-record=perhaps\n"), BootSpeed::default());
        assert_eq!((conf_value(true), conf_value(false)), ("on", "off"));
    }
}
