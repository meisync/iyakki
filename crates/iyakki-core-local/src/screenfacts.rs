//! The screens on this machine (moved from the shell and `iyakki-apps::display`, F-ARCH-06): [`LocalScreenFacts`] reads `/sys/class/drm` and
//! `/sys/class/graphics` (the roots are fields, so a test points it at a made-up tree) and is the core's
//! [`iyakki_core_api::screenfacts::ScreenFacts`]. Nothing here opens a device node or writes.

use std::path::{Path, PathBuf};

use iyakki_core_api::screenfacts::{parse_mode, Connector, ScreenFacts};

pub struct LocalScreenFacts {
    drm: PathBuf,
    graphics: PathBuf,
}

/// Read every `<drm>/card*-*` connector directory. Unreadable things are simply left out; never fails.
pub fn read_connectors(drm: &Path) -> Vec<Connector> {
    let Ok(rd) = std::fs::read_dir(drm) else { return Vec::new() };
    let mut dirs: Vec<_> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("card") && n.contains('-')).collect();
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let p = drm.join(&d);
        let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|v| v.trim().to_string());
        let Some(status) = read("status") else { continue };
        let name = d.split_once('-').map(|(_, n)| n.to_string()).unwrap_or_else(|| d.clone());
        let modes = read("modes").map(|m| m.lines().filter_map(parse_mode).collect()).unwrap_or_default();
        let enabled = read("enabled").map(|e| e == "enabled");
        out.push(Connector { name, connected: status == "connected", enabled, modes });
    }
    out
}

/// What sysfs says about the screen behind `path` (`/dev/fbN` -> `<sys_graphics>/fbN`): the driver's name, the size, the stride and the depth, in one
/// line. Reading it never opens the device. That matters: when a GPU driver takes over, the firmware's screen (simpledrm) is unplugged, and the last
/// close of its device frees its buffer, which on this kernel (6.12, simpledrm's shmem fbdev) can spin forever in the kernel (`shmem_undo_range`),
/// taking a CPU core and the thread with it. Polling with open/close every 100 ms could be that last close.
pub fn sysfs_signature(sys_graphics: &Path, path: &str) -> Option<String> {
    let node = Path::new(path).file_name()?.to_str()?;
    let dir = sys_graphics.join(node);
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|v| v.trim().to_string());
    let name = read("name")?;
    Some(format!("{name} {} stride {} {} bpp", read("virtual_size").unwrap_or_default(), read("stride").unwrap_or_default(), read("bits_per_pixel").unwrap_or_default()))
}

/// The first `/dev/dri/cardN` whose sysfs directory has at least `at_least` connected connectors.
pub fn card_with_connected(drm: &Path, at_least: usize) -> Option<String> {
    let conns = read_connectors(drm);
    let mut by_card: std::collections::BTreeMap<String, usize> = Default::default();
    if let Ok(rd) = std::fs::read_dir(drm) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some((card, _)) = n.split_once('-') {
                if card.starts_with("card") && card[4..].chars().all(|c| c.is_ascii_digit()) {
                    let name = n.split_once('-').map(|x| x.1.to_string()).unwrap_or_default();
                    if conns.iter().any(|c| c.name == name && c.connected) {
                        *by_card.entry(card.to_string()).or_default() += 1;
                    }
                }
            }
        }
    }
    by_card.into_iter().find(|(_, n)| *n >= at_least).map(|(c, _)| format!("/dev/dri/{c}"))
}

impl LocalScreenFacts {
    /// This machine.
    pub fn system() -> LocalScreenFacts {
        LocalScreenFacts { drm: PathBuf::from("/sys/class/drm"), graphics: PathBuf::from("/sys/class/graphics") }
    }

    /// Other places (a test tree).
    pub fn at(drm: &Path, graphics: &Path) -> LocalScreenFacts {
        LocalScreenFacts { drm: drm.to_path_buf(), graphics: graphics.to_path_buf() }
    }

    /// Where the connectors are read.
    pub fn drm_dir(&self) -> &Path {
        &self.drm
    }
}

impl ScreenFacts for LocalScreenFacts {
    fn connectors(&self) -> Vec<Connector> {
        read_connectors(&self.drm)
    }
    fn fb_signature(&self, fb_path: &str) -> Option<String> {
        sysfs_signature(&self.graphics, fb_path)
    }
    fn card_with(&self, at_least: usize) -> Option<String> {
        card_with_connected(&self.drm, at_least)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-screenfacts-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn connectors_modes_and_the_enabled_flag_are_read_and_an_unreadable_one_is_left_out() {
        let d = tree("conn");
        put(&d, "drm/card0-eDP-1/status", "connected\n");
        put(&d, "drm/card0-eDP-1/modes", "2560x1600\n1280x800\n");
        put(&d, "drm/card0-eDP-1/enabled", "enabled\n");
        put(&d, "drm/card0-HDMI-A-1/status", "disconnected\n");
        put(&d, "drm/card1-DP-1/status", "connected\n");
        put(&d, "drm/card1-DP-1/modes", "bad\n0x0\n1920x1080p-60\n");
        std::fs::create_dir_all(d.join("drm/card0-Writeback-1")).unwrap(); // no status file
        std::fs::create_dir_all(d.join("drm/version")).unwrap();
        let f = LocalScreenFacts::at(&d.join("drm"), &d.join("graphics"));
        let c = f.connectors();
        assert_eq!(c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["HDMI-A-1", "eDP-1", "DP-1"], "sorted by directory name (card0-HDMI, card0-eDP, card1-DP)");
        assert_eq!(c[1].modes, vec![(2560, 1600), (1280, 800)]);
        assert_eq!(c[1].enabled, Some(true));
        assert_eq!(c[2].modes, vec![(1920, 1080)], "unparsable and zero lines are kept out");
        assert!(!c[0].connected);
        assert_eq!(f.signature(), "eDP-1:2560x1600,DP-1:1920x1080");
        assert!(LocalScreenFacts::at(&d.join("nowhere"), &d).connectors().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_card_with_enough_connected_screens_is_the_first_one_and_one_per_card_is_not_two() {
        let d = tree("card");
        put(&d, "drm/card0-eDP-1/status", "connected\n");
        put(&d, "drm/card0-DP-1/status", "disconnected\n");
        put(&d, "drm/card1-DP-2/status", "connected\n");
        put(&d, "drm/card1-DP-3/status", "connected\n");
        let f = LocalScreenFacts::at(&d.join("drm"), &d.join("graphics"));
        assert_eq!(f.card_with(1), Some("/dev/dri/card0".into()), "the first card");
        assert_eq!(f.card_with(2), Some("/dev/dri/card1".into()));
        assert_eq!(f.card_with(3), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_firmware_screen_is_told_by_its_name_size_stride_and_depth_and_a_driver_takeover_changes_it() {
        let d = tree("fb");
        let f = LocalScreenFacts::at(&d.join("drm"), &d.join("graphics"));
        assert_eq!(f.fb_signature("/dev/fb0"), None, "no name: no signature (the caller asks the device)");
        put(&d, "graphics/fb0/name", "simpledrmdrmfb\n");
        put(&d, "graphics/fb0/virtual_size", "2880,1800\n");
        put(&d, "graphics/fb0/stride", "11520\n");
        put(&d, "graphics/fb0/bits_per_pixel", "32\n");
        let firmware = f.fb_signature("/dev/fb0").unwrap();
        assert_eq!(firmware, "simpledrmdrmfb 2880,1800 stride 11520 32 bpp");
        put(&d, "graphics/fb0/name", "amdgpudrmfb\n");
        assert_ne!(f.fb_signature("/dev/fb0").unwrap(), firmware, "same size, another driver");
        assert_eq!(f.fb_signature("/dev/fb1"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
