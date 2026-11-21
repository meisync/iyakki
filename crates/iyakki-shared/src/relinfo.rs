//! Which IYAKKI this is: the version the program was built as, and, on an installed image, the `/etc/iyakki-release` file the
//! image build writes (version, build date, git describe, variant, kernel). SYSTEM shows it as "About"; `iyakki --version`
//! and the boot log print the first line.

use std::path::Path;

/// The version of this build (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where an image build leaves its description.
pub const RELEASE_FILE: &str = "/etc/iyakki-release";

/// The contents of an `/etc/iyakki-release` file (`KEY=value` lines; unknown keys are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub build_date: String,
    pub git_describe: String,
    pub variant: String,
    pub kernel: String,
    /// `RESTRICTED_FIRMWARE=included`: a build made with IYAKKI_INCLUDE_RESTRICTED_FIRMWARE=1 (docs/RESTRICTED-FIRMWARE.md).
    pub restricted_firmware: bool,
    /// `IMAGE_PROFILE=release` (dist/buildroot/board/iyakki/release-profile.sh); anything else, or no line, is a development/test image.
    pub release_profile: bool,
    /// `FIRMWARE=`: `none` (nothing shipped, the person downloads it), `bundled` (vendor firmware inside), or empty (an older image).
    pub firmware: String,
}

/// What About says about such an image.
/// What About says about a release image / a test image (the releasable flag is machine-readable: `IMAGE_PROFILE=release` + `FIRMWARE=none`).
pub const FIRMWARE_NONE_NOTE: &str = "none (download)";
pub const FIRMWARE_BUNDLED_NOTE: &str = "bundled (not releasable)";

pub const RESTRICTED_NOTE: &str = "includes third-party firmware under its own licences: not for redistribution by the IYAKKI project";

impl ReleaseInfo {
    pub fn parse(text: &str) -> ReleaseInfo {
        let mut r = ReleaseInfo::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "VERSION" => r.version = v,
                "BUILD_DATE" => r.build_date = v,
                "GIT_DESCRIBE" => r.git_describe = v,
                "VARIANT" => r.variant = v,
                "KERNEL" => r.kernel = v,
                "RESTRICTED_FIRMWARE" => r.restricted_firmware = v == "included",
                "IMAGE_PROFILE" => r.release_profile = v == "release",
                "FIRMWARE" => r.firmware = v,
                _ => {}
            }
        }
        r
    }

    /// True only for an image that may be published: the release profile with no vendor firmware inside.
    pub fn releasable(&self) -> bool {
        self.release_profile && self.firmware == "none" && !self.restricted_firmware
    }

    /// The file at `path`, if there is one (a machine that did not boot from an image has none).
    pub fn load(path: &Path) -> Option<ReleaseInfo> {
        std::fs::read_to_string(path).ok().map(|t| ReleaseInfo::parse(&t))
    }

    /// Lines for the About block: label and value.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![("Version", if self.version.is_empty() { VERSION.to_string() } else { self.version.clone() })];
        for (l, x) in [("Built", &self.build_date), ("Source", &self.git_describe), ("Variant", &self.variant), ("Kernel", &self.kernel)] {
            if !x.is_empty() {
                v.push((l, x.clone()));
            }
        }
        if self.restricted_firmware {
            v.push(("Firmware", RESTRICTED_NOTE.to_string()));
        } else if self.releasable() {
            v.push(("Firmware", FIRMWARE_NONE_NOTE.to_string()));
        } else if self.firmware == "bundled" {
            v.push(("Firmware", FIRMWARE_BUNDLED_NOTE.to_string()));
        }
        v
    }
}

/// One line for About: `IYAKKI 0.1.0-alpha.1 · 2026-10-17 · v0.1.0-alpha.1-3-gabc · libre · kernel 6.12`; without an image
/// description just `IYAKKI <version> (not running from an image)`.
pub fn about_line(info: Option<&ReleaseInfo>) -> String {
    match info {
        None => format!("IYAKKI {VERSION} (not running from an image)"),
        Some(i) => {
            let rows = i.rows();
            let mut s = format!("IYAKKI {}", rows[0].1);
            for (l, x) in &rows[1..] {
                s.push_str(&format!(" · {}{x}", if *l == "Kernel" { "kernel " } else { "" }));
            }
            s
        }
    }
}

/// The About line for this machine, read from [`RELEASE_FILE`] once.
pub fn machine_about() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| about_line(ReleaseInfo::load(Path::new(RELEASE_FILE)).as_ref()))
}

/// Where the release image keeps the first-boot notice text (`board/iyakki/release-profile.sh`).
pub const FIRMWARE_NOTICE_FILE: &str = "/usr/share/iyakki/firmware-notice.txt";
/// The inbox note's link: Store, Setup, "Hardware firmware" (Quick install item `FwHardware`).
pub const FIRMWARE_NOTICE_ITEM: &str = "setup:fw-hardware";

/// The first-boot notice of a release image (F-REL-09): `(title, body)` when this image has no vendor firmware (`IMAGE_PROFILE=release`
/// and `FIRMWARE=none`) and ships the notice text, otherwise `None` (a test image or an older image says nothing). The shell leaves it
/// in an administrator's inbox once per unread note, linked to the Hardware firmware item.
pub fn firmware_notice(rel: Option<&ReleaseInfo>, notice_text: Option<&str>) -> Option<(String, String)> {
    let rel = rel.filter(|r| r.release_profile && r.firmware == "none" && !r.restricted_firmware)?;
    let _ = rel;
    let body = notice_text?.trim();
    (!body.is_empty()).then(|| ("Optional hardware firmware is not included".to_string(), body.chars().take(400).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_file_is_read_and_shown() {
        let t = "VERSION=0.1.0-alpha.1\nBUILD_DATE=2026-10-17\nGIT_DESCRIBE=v0.1.0-alpha.1-3-gabc1234\nVARIANT=libre\nKERNEL=6.12.8\nOTHER=x\n";
        let r = ReleaseInfo::parse(t);
        assert_eq!(r.version, "0.1.0-alpha.1");
        assert_eq!(r.variant, "libre");
        assert_eq!(about_line(Some(&r)), "IYAKKI 0.1.0-alpha.1 · 2026-10-17 · v0.1.0-alpha.1-3-gabc1234 · libre · kernel 6.12.8");
        assert_eq!(r.rows().len(), 5);
        assert!(about_line(None).contains(VERSION));
        assert!(ReleaseInfo::load(Path::new("/nonexistent/iyakki-release")).is_none());
        assert_eq!(ReleaseInfo::parse("").rows(), vec![("Version", VERSION.to_string())]);
        // an image built for the builder's own use with the restricted firmware says so
        let r = ReleaseInfo::parse(&format!("{t}RESTRICTED_FIRMWARE=included\n"));
        assert!(r.restricted_firmware && about_line(Some(&r)).ends_with(RESTRICTED_NOTE) && about_line(Some(&r)).contains("not for redistribution by the IYAKKI project"));
        assert!(!ReleaseInfo::parse(t).restricted_firmware && !about_line(Some(&ReleaseInfo::parse(t))).contains("redistribution"));
    }

    #[test]
    fn the_releasable_flag_is_read_from_the_stamp() {
        let rel = ReleaseInfo::parse("VERSION=1\nIMAGE_PROFILE=release\nFIRMWARE=none\n");
        assert!(rel.releasable() && rel.rows().contains(&("Firmware", FIRMWARE_NONE_NOTE.to_string())));
        let dev = ReleaseInfo::parse("VERSION=1\nIMAGE_PROFILE=dev\nFIRMWARE=bundled\n");
        assert!(!dev.releasable() && dev.rows().contains(&("Firmware", FIRMWARE_BUNDLED_NOTE.to_string())));
        assert!(!ReleaseInfo::parse("VERSION=1\n").releasable(), "an old image without the stamp is not releasable");
        assert!(!ReleaseInfo::parse("IMAGE_PROFILE=release\nFIRMWARE=none\nRESTRICTED_FIRMWARE=included\n").releasable());
        assert!(!ReleaseInfo::parse("IMAGE_PROFILE=release\nFIRMWARE=bundled\n").releasable());
    }

    #[test]
    fn only_a_release_image_with_no_firmware_and_a_notice_text_gives_the_first_boot_notice() {
        let rel = ReleaseInfo::parse("IMAGE_PROFILE=release\nFIRMWARE=none\n");
        let n = firmware_notice(Some(&rel), Some("Optional hardware firmware is not included (licence). Install it: Store -> Setup -> Hardware firmware.\n")).unwrap();
        assert_eq!(n.0, "Optional hardware firmware is not included");
        assert!(n.1.contains("Install it") && !n.1.ends_with('\n'));
        assert!(firmware_notice(Some(&ReleaseInfo::parse("IMAGE_PROFILE=dev\nFIRMWARE=bundled\n")), Some("x")).is_none());
        assert!(firmware_notice(Some(&ReleaseInfo::parse("")), Some("x")).is_none() && firmware_notice(None, Some("x")).is_none());
        assert!(firmware_notice(Some(&rel), None).is_none() && firmware_notice(Some(&rel), Some("  \n")).is_none());
    }
}
