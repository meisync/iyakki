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
}

/// What About says about such an image.
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
                _ => {}
            }
        }
        r
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
}
