//! The kernel facts the app-event sources read (F-ARCH-07): the free space of mounted volumes, the health of disks, the Bluetooth links,
//! printers and scanners, the first screen's resolution, the count of good suspends and the link state. The agent decides what an event
//! says; the reading of the machine (`<sys>` is `/sys` on a machine, a made-up folder in a test) is here, in one place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

fn text(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// Free percent of each mounted block-device volume.
pub fn free_percents(mounts_text: &str) -> BTreeMap<String, u8> {
    use std::os::unix::ffi::OsStrExt;
    let mut out = BTreeMap::new();
    for m in iyakki_core_api::parse_mounts(mounts_text).into_iter().filter(|m| m.source.starts_with("/dev/")) {
        let Ok(c) = std::ffi::CString::new(Path::new(&m.target).as_os_str().as_bytes()) else { continue };
        // SAFETY: `c` is a valid NUL-terminated path and `st` is plain data that statvfs fills in.
        let st = unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut st) != 0 {
                continue;
            }
            st
        };
        if st.f_blocks > 0 {
            out.insert(m.target.clone(), ((st.f_bavail as u128 * 100) / st.f_blocks as u128).min(100) as u8);
        }
    }
    out
}

/// Disks that report themselves badly: an NVMe controller whose state is not `live`, or a block device whose state file is not `running`.
pub fn disk_states(sys: &Path) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for name in names_in(&sys.join("class/nvme")) {
        let ok = text(&sys.join("class/nvme").join(&name).join("state")).map(|s| s.trim() == "live").unwrap_or(true);
        out.insert(name, ok);
    }
    for name in names_in(&sys.join("block")) {
        if let Some(s) = text(&sys.join("block").join(&name).join("device/state")) {
            out.insert(name, s.trim() == "running");
        }
    }
    out
}

/// The kernel's Bluetooth links: `<sys>/class/bluetooth/<hciN>:<handle>` entries, told by `hciN:H`.
pub fn bluetooth_links(sys: &Path) -> BTreeSet<String> {
    names_in(&sys.join("class/bluetooth")).into_iter().filter(|n| n.contains(':')).collect()
}

/// Printers (USB printer class nodes `lp*`) and scanners (the scanner class).
pub fn printers_and_scanners(sys: &Path) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for n in names_in(&sys.join("class/usbmisc")) {
        if n.starts_with("lp") {
            out.insert(("printer".to_string(), n));
        }
    }
    for n in names_in(&sys.join("class/scanner")) {
        out.insert(("scanner".to_string(), n));
    }
    out
}

/// `WIDTHxHEIGHT` of the first connected screen's preferred mode (the first line of its `modes` file); empty when none.
pub fn screen_resolution(sys: &Path) -> String {
    let dir = sys.join("class/drm");
    for n in names_in(&dir) {
        let p = dir.join(&n);
        if text(&p.join("status")).is_some_and(|s| s.trim() == "connected") {
            if let Some(m) = text(&p.join("modes")).and_then(|t| t.lines().next().map(str::to_string)) {
                return m;
            }
        }
    }
    String::new()
}

/// The count of good suspends (`<sys>/power/suspend_stats/success`), 0 when unreadable.
pub fn suspend_count(sys: &Path) -> u64 {
    text(&sys.join("power/suspend_stats/success")).and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

/// `online` when a network interface other than loopback has operstate `up`, else `offline`.
pub fn link_state(sys: &Path) -> &'static str {
    for n in names_in(&sys.join("class/net")) {
        if n != "lo" && text(&sys.join("class/net").join(&n).join("operstate")).is_some_and(|s| s.trim() == "up") {
            return "online";
        }
    }
    "offline"
}

/// `online` only when an interface is up AND a default route leaves through an interface that is up (a cable with no lease, an
/// access-point or tether link with no way out, reads `offline`). `route` is the text of `/proc/net/route`.
pub fn link_state_routed(sys: &Path, route: &str) -> &'static str {
    let up = |n: &str| n != "lo" && text(&sys.join("class/net").join(n).join("operstate")).is_some_and(|s| s.trim() == "up");
    let routed = route.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() >= 8 && f[1] == "00000000" && f[7] == "00000000" && u32::from_str_radix(f[3], 16).unwrap_or(0) & 1 != 0 && up(f[0])
    });
    if routed { "online" } else { "offline" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-appevent-facts-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn disks_report_live_and_running_only_as_good_and_a_missing_state_is_left_out() {
        let d = dir("disks");
        put(&d, "class/nvme/nvme0/state", "live\n");
        put(&d, "class/nvme/nvme1/state", "dead\n");
        put(&d, "block/sda/device/state", "running\n");
        put(&d, "block/sdb/device/state", "offline\n");
        std::fs::create_dir_all(d.join("block/loop0")).unwrap();
        let m = disk_states(&d);
        assert_eq!(m.get("nvme0"), Some(&true));
        assert_eq!(m.get("nvme1"), Some(&false));
        assert_eq!(m.get("sda"), Some(&true));
        assert_eq!(m.get("sdb"), Some(&false));
        assert!(!m.contains_key("loop0"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn links_printers_scanners_screen_suspends_and_the_link_state_are_read_from_the_folder() {
        let d = dir("misc");
        assert_eq!(link_state(&d), "offline");
        assert_eq!(screen_resolution(&d), "");
        assert_eq!(suspend_count(&d), 0);
        put(&d, "class/bluetooth/hci0:12/x", "");
        std::fs::create_dir_all(d.join("class/bluetooth/hci0")).unwrap();
        put(&d, "class/usbmisc/lp0/x", "");
        put(&d, "class/usbmisc/hiddev0/x", "");
        put(&d, "class/scanner/s1/x", "");
        put(&d, "class/drm/card0-HDMI-A-1/status", "disconnected\n");
        put(&d, "class/drm/card0-eDP-1/status", "connected\n");
        put(&d, "class/drm/card0-eDP-1/modes", "1920x1080\n1280x720\n");
        put(&d, "power/suspend_stats/success", "7\n");
        put(&d, "class/net/lo/operstate", "up\n");
        assert_eq!(link_state(&d), "offline", "loopback does not count");
        put(&d, "class/net/wlan0/operstate", "up\n");
        assert_eq!(link_state(&d), "online");
        assert_eq!(bluetooth_links(&d), BTreeSet::from(["hci0:12".to_string()]));
        assert_eq!(printers_and_scanners(&d), BTreeSet::from([("printer".to_string(), "lp0".to_string()), ("scanner".to_string(), "s1".to_string())]));
        assert_eq!(screen_resolution(&d), "1920x1080");
        assert_eq!(suspend_count(&d), 7);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_link_that_is_up_without_a_default_route_is_not_online() {
        let d = std::env::temp_dir().join(format!("iyakki-link-routed-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        put(&d, "class/net/eth0/operstate", "up\n");
        let head = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n";
        let lan = format!("{head}eth0\t0001A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n");
        let dflt = format!("{lan}eth0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\n");
        assert_eq!(link_state(&d), "online");
        assert_eq!(link_state_routed(&d, &lan), "offline", "up, but no way out");
        assert_eq!(link_state_routed(&d, ""), "offline");
        assert_eq!(link_state_routed(&d, &dflt), "online");
        put(&d, "class/net/eth0/operstate", "down\n");
        assert_eq!(link_state_routed(&d, &dflt), "offline", "a route through a dead interface does not count");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn free_percent_names_only_mounted_block_devices_that_answer() {
        let m = free_percents("tmpfs /tmp tmpfs rw 0 0\n/dev/nowhere /definitely/not/here ext4 rw 0 0\n");
        assert!(m.is_empty());
    }
}
