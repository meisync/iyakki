//! What the kernel says about the screens (F-ARCH-06): the display connectors and their modes (`/sys/class/drm`), which graphics card to drive, and a
//! one-line signature of the firmware framebuffer (`/sys/class/graphics`, read without opening the device). The shell takes a [`ScreenFacts`] and never
//! opens these files itself: the device has `iyakki_core_local::LocalScreenFacts`, a test has `iyakki_core_fake::FakeScreenFacts`. Reads only.

use serde::{Deserialize, Serialize};

/// One display connector as sysfs describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connector {
    /// "eDP-1", "HDMI-A-1" (the part of the sysfs directory name after `cardN-`).
    pub name: String,
    pub connected: bool,
    /// `enabled` file when readable: "enabled" is true, "disabled" false.
    pub enabled: Option<bool>,
    /// The `modes` file in order, the first being the preferred one. Zero-size and unparsable lines are kept out.
    pub modes: Vec<(u32, u32)>,
}

/// Parse "2560x1600", "2560x1600p-60" or "U:2880x1800p-0" (the first `WxH` found).
pub fn parse_mode(line: &str) -> Option<(u32, u32)> {
    let s = line.trim();
    let s = s.split_once(':').map_or(s, |(_, rest)| rest);
    let (w, rest) = s.split_once('x')?;
    let w: u32 = w.trim().parse().ok()?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let h: u32 = digits.parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// What identifies the displays (connected connectors and their preferred mode), to notice a hot-plug or a mode change.
pub fn signature_of(connectors: &[Connector]) -> String {
    connectors.iter().filter(|c| c.connected).map(|c| format!("{}:{}", c.name, c.modes.first().map_or("?".to_string(), |m| format!("{}x{}", m.0, m.1)))).collect::<Vec<_>>().join(",")
}

pub trait ScreenFacts: Send + Sync {
    /// Every display connector, sorted by directory name; one that cannot be read is left out.
    fn connectors(&self) -> Vec<Connector> {
        Vec::new()
    }
    /// [`signature_of`] the connectors.
    fn signature(&self) -> String {
        signature_of(&self.connectors())
    }
    /// What the firmware framebuffer behind `fb_path` (`/dev/fb0`) is: the driver's name, size, stride and depth in one line, read from sysfs without opening
    /// the device (opening and closing simpledrm's device can spin the kernel when a GPU driver takes over). `None` without a name.
    fn fb_signature(&self, _fb_path: &str) -> Option<String> {
        None
    }
    /// The first graphics card (`/dev/dri/cardN`) that has at least `at_least` connected connectors.
    fn card_with(&self, _at_least: usize) -> Option<String> {
        None
    }
}

/// The facts of a machine without screens.
pub struct NoScreenFacts;

impl ScreenFacts for NoScreenFacts {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_read_from_the_forms_the_kernel_uses_and_the_signature_names_only_connected_ones() {
        assert_eq!(parse_mode("2560x1600"), Some((2560, 1600)));
        assert_eq!(parse_mode("2560x1600p-60"), Some((2560, 1600)));
        assert_eq!(parse_mode("U:2880x1800p-0"), Some((2880, 1800)));
        assert_eq!(parse_mode("0x0"), None);
        assert_eq!(parse_mode("junk"), None);
        let c = |n: &str, up: bool, m: &[(u32, u32)]| Connector { name: n.into(), connected: up, enabled: None, modes: m.to_vec() };
        assert_eq!(signature_of(&[c("eDP-1", true, &[(2560, 1600), (1280, 800)]), c("HDMI-A-1", false, &[(1920, 1080)]), c("DP-1", true, &[])]), "eDP-1:2560x1600,DP-1:?");
        let n = NoScreenFacts;
        assert!(n.connectors().is_empty() && n.signature().is_empty() && n.fb_signature("/dev/fb0").is_none() && n.card_with(1).is_none());
    }
}
