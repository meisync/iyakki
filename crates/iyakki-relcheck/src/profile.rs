//! The RELEASE image profile, checked without building anything (`iyakki-relcheck release-image-check`; docs/development/RELEASE-IMAGE-PROFILE.md).
//!
//! The profile says: the base image keeps no vendor firmware and no H.264/AAC codecs (owner decision 2026-10-27). What can be checked from files:
//!   * the firmware keep list (`board/iyakki/release-firmware.keep`) names nothing but patterns that `legal/release-profile.txt` allows (`allow-fw`),
//!   * a Buildroot `.config` (or the composed defconfig) sets none of the `forbid-config` symbols and enables only reviewed gst-plugins-bad plugins,
//!   * the two fragments (`iyakki_no_firmware.fragment`, `iyakki_release.fragment`) really switch every literal forbidden symbol off,
//!   * an existing image listing, with the profile applied to it ([`simulate`]), has nothing left that the licence gate or the profile refuses,
//!   * an image folder named `*-release` has nothing under `lib/firmware` but the allowed files ([`image_findings`]).
//! Everything here reads; nothing is unpacked or written.

use crate::cpio::{Entry, Kind};
use crate::table::{self, glob};

pub const PROFILE_RULES: &str = "legal/release-profile.txt";
pub const KEEP_FILE: &str = "dist/buildroot/board/iyakki/release-firmware.keep";
pub const FRAGMENTS: [&str; 2] = ["dist/buildroot/configs/iyakki_no_firmware.fragment", "dist/buildroot/configs/iyakki_release.fragment"];

#[derive(Debug, Default, Clone)]
pub struct Profile {
    /// symbol glob, reason, date
    pub forbid_config: Vec<(String, String, String)>,
    /// plugin name (upper case, without the prefix), reason, date
    pub allow_gst_bad: Vec<(String, String, String)>,
    /// glob under /lib/firmware, reason, date
    pub allow_fw: Vec<(String, String, String)>,
}

pub fn parse_profile(text: &str) -> (Profile, Vec<String>) {
    let (mut p, mut problems) = (Profile::default(), Vec::new());
    for row in table::parse(text) {
        let f = &row.f;
        if f.len() != 4 || !table::valid_reason(&f[2]) || !table::valid_date(&f[3]) {
            problems.push(format!("release profile line {}: a row is `kind | value | reason | date`", row.line));
            continue;
        }
        let t = (f[1].clone(), f[2].clone(), f[3].clone());
        match f[0].as_str() {
            "forbid-config" => p.forbid_config.push(t),
            "allow-gst-bad" => p.allow_gst_bad.push((t.0.to_uppercase(), t.1, t.2)),
            "allow-fw" => p.allow_fw.push(t),
            other => problems.push(format!("release profile line {}: unknown row kind `{other}`", row.line)),
        }
    }
    (p, problems)
}

/// Patterns of a keep list (`prune-firmware.sh` format): `(keep, exclude)`. Comments and blank lines are dropped; `!` lines are exclusions.
pub fn parse_keep(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut keep, mut excl) = (Vec::new(), Vec::new());
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match t.strip_prefix('!') {
            Some(x) => excl.push(x.to_string()),
            None => keep.push(t.to_string()),
        }
    }
    (keep, excl)
}

/// Would `prune-firmware.sh` with this list keep the file `rel` (path relative to /lib/firmware)?
pub fn kept(keep: &[String], excl: &[String], rel: &str) -> bool {
    !excl.iter().any(|x| glob(x, rel)) && keep.iter().any(|p| glob(p, rel))
}

/// Problems with the keep list itself: a pattern with no `allow-fw` row, or no pattern at all (prune-firmware.sh refuses an empty list).
pub fn keep_problems(keep_text: &str, p: &Profile) -> Vec<String> {
    let (keep, _) = parse_keep(keep_text);
    let mut v = Vec::new();
    if keep.is_empty() {
        v.push("release-firmware.keep has no pattern (prune-firmware.sh refuses an empty list): keep at least regulatory.db".to_string());
    }
    for k in &keep {
        if !p.allow_fw.iter().any(|(g, _, _)| g == k) {
            v.push(format!("release-firmware.keep keeps `{k}` but legal/release-profile.txt has no `allow-fw | {k} | reason | date` row: firmware in the base image is the owner's decision"));
        }
    }
    v
}

/// The symbols a Buildroot `.config` (or defconfig) turns on: `SYMBOL=y` and `SYMBOL=m`.
pub fn enabled_symbols(config: &str) -> Vec<String> {
    config.lines().filter_map(|l| {
        let l = l.trim();
        let (k, v) = l.split_once('=')?;
        (k.starts_with("BR2_") && (v == "y" || v == "m")).then(|| k.to_string())
    }).collect()
}

/// The symbols a text switches off: `# SYMBOL is not set`.
pub fn unset_symbols(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("# ")?;
        let sym = rest.strip_suffix(" is not set")?;
        (sym.starts_with("BR2_") && !sym.contains(' ')).then(|| sym.to_string())
    }).collect()
}

/// Problems with a finished `.config`: a forbidden symbol on, or a gst-plugins-bad plugin that is on without an `allow-gst-bad` row.
pub fn config_problems(config: &str, p: &Profile) -> Vec<String> {
    let mut v = Vec::new();
    for s in enabled_symbols(config) {
        if let Some((g, why, _)) = p.forbid_config.iter().find(|(g, _, _)| glob(g, &s)) {
            v.push(format!("{s} is on, forbidden by `{g}`: {why}"));
            continue;
        }
        if let Some(name) = s.strip_prefix("BR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_") {
            if !p.allow_gst_bad.iter().any(|(n, _, _)| n == name) {
                v.push(format!("{s} is on but no `allow-gst-bad | {name}` row says its source licences were read (legal/release-profile.txt)"));
            }
        }
    }
    v
}

/// The fragments must switch every literal (glob-free) forbidden symbol off, and must not turn any forbidden symbol on.
pub fn fragment_problems(fragments: &[(&str, String)], p: &Profile) -> Vec<String> {
    let mut v = Vec::new();
    let all: String = fragments.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
    let unset = unset_symbols(&all);
    for (g, _, _) in &p.forbid_config {
        if !g.contains('*') && !unset.iter().any(|u| u == g) {
            v.push(format!("the release fragments do not say `# {g} is not set`"));
        }
    }
    for (name, text) in fragments {
        for s in enabled_symbols(text) {
            if p.forbid_config.iter().any(|(g, _, _)| glob(g, &s)) {
                v.push(format!("{name} turns on {s}, which the release profile forbids"));
            }
        }
    }
    v
}

/// The profile applied to an image listing: what `post-build.sh` + `release-profile.sh` leave of an image that was built the other way.
/// Drops every file under `lib/firmware` that the keep list does not keep, and the linux-firmware licence texts.
pub fn simulate(entries: &[Entry], keep_text: &str) -> Vec<Entry> {
    let (keep, excl) = parse_keep(keep_text);
    entries.iter().filter(|e| {
        if let Some(rel) = e.path.strip_prefix("lib/firmware/") {
            return e.kind == Kind::Dir || kept(&keep, &excl, rel);
        }
        !(e.path == "usr/share/doc/linux-firmware" || e.path.starts_with("usr/share/doc/linux-firmware/") || e.path.starts_with("usr/share/doc/linux-firmware-restricted"))
    }).cloned().collect()
}

/// What a release image may not contain, beyond the gate's own rows: any file under `lib/firmware` that no `allow-fw` row names.
pub fn image_findings(entries: &[Entry], p: &Profile) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| e.kind != Kind::Dir) {
        if let Some(rel) = e.path.strip_prefix("lib/firmware/") {
            if !p.allow_fw.iter().any(|(g, _, _)| glob(g, rel)) {
                v.push(format!("vendor firmware in a release image: {}", e.path));
            }
        }
    }
    let n = v.len();
    v.truncate(10);
    if n > 10 {
        v.push(format!("... and {} more under lib/firmware", n - 10));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "forbid-config | BR2_PACKAGE_LINUX_FIRMWARE | linux-firmware is vendor firmware | 2026-10-27\n\
        forbid-config | BR2_PACKAGE_LINUX_FIRMWARE_* | all sub-options | 2026-10-27\n\
        forbid-config | BR2_PACKAGE_FFMPEG_GPL | GPL FFmpeg build | 2026-10-27\n\
        allow-gst-bad | MPEGTSDEMUX | selected by WPE; sources read | 2026-10-27\n\
        allow-fw | regulatory.db | wireless-regdb, ISC | 2026-10-27\n\
        allow-fw | regulatory.db.p7s | its signature | 2026-10-27\n";

    fn e(p: &str) -> Entry {
        Entry { path: p.into(), kind: Kind::File, size: 1, mode: 0, target: String::new() }
    }

    #[test]
    fn rows_need_four_fields_a_reason_and_a_date() {
        let (p, probs) = parse_profile(RULES);
        assert!(probs.is_empty(), "{probs:?}");
        assert_eq!((p.forbid_config.len(), p.allow_gst_bad.len(), p.allow_fw.len()), (3, 1, 2));
        let (_, probs) = parse_profile("forbid-config | X | short | 2026-10-27\nallow-fw | a | a real reason here | never\nwhat | a | a real reason here | 2026-10-27\n");
        assert_eq!(probs.len(), 3, "{probs:?}");
    }

    #[test]
    fn the_keep_list_may_only_name_allowed_patterns() {
        let (p, _) = parse_profile(RULES);
        assert!(keep_problems("# nothing but\nregulatory.db\n", &p).is_empty());
        let bad = keep_problems("regulatory.db\ni915/*\n", &p);
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("i915/*") && bad[0].contains("allow-fw"));
        assert_eq!(keep_problems("# only comments\n", &p).len(), 1);
    }

    #[test]
    fn a_config_with_firmware_or_an_unreviewed_plugin_fails() {
        let (p, _) = parse_profile(RULES);
        let ok = "BR2_PACKAGE_WIRELESS_REGDB=y\nBR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_MPEGTSDEMUX=y\n# BR2_PACKAGE_LINUX_FIRMWARE is not set\n";
        assert!(config_problems(ok, &p).is_empty());
        let bad = "BR2_PACKAGE_LINUX_FIRMWARE=y\nBR2_PACKAGE_LINUX_FIRMWARE_I915=y\nBR2_PACKAGE_GST1_PLUGINS_BAD_PLUGIN_WEBRTC=y\nBR2_PACKAGE_FFMPEG_GPL=y\n";
        let v = config_problems(bad, &p);
        assert_eq!(v.len(), 4, "{v:?}");
        assert!(v.iter().any(|x| x.contains("WEBRTC") && x.contains("allow-gst-bad")));
    }

    #[test]
    fn the_fragments_must_switch_every_literal_forbidden_symbol_off() {
        let (p, _) = parse_profile(RULES);
        let good = vec![("a", "# BR2_PACKAGE_LINUX_FIRMWARE is not set\n# BR2_PACKAGE_FFMPEG_GPL is not set\n".to_string())];
        assert!(fragment_problems(&good, &p).is_empty());
        let missing = vec![("a", "# BR2_PACKAGE_LINUX_FIRMWARE is not set\n".to_string())];
        assert_eq!(fragment_problems(&missing, &p).len(), 1);
        let on = vec![("a", "# BR2_PACKAGE_LINUX_FIRMWARE is not set\n# BR2_PACKAGE_FFMPEG_GPL is not set\nBR2_PACKAGE_FFMPEG_GPL=y\n".to_string())];
        assert_eq!(fragment_problems(&on, &p).len(), 1);
    }

    #[test]
    fn simulate_leaves_the_regulatory_database_and_nothing_else_of_the_firmware() {
        let entries = vec![e("lib/firmware/regulatory.db"), e("lib/firmware/regulatory.db.p7s"), e("lib/firmware/amdgpu/psp_13_0_0.bin"), e("lib/firmware/nvidia/ga102/gsp/gsp-535.bin"), e("usr/share/doc/linux-firmware/WHENCE"), e("usr/bin/iyakki")];
        let left = simulate(&entries, "regulatory.db\nregulatory.db.p7s\n");
        let names: Vec<&str> = left.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(names, vec!["lib/firmware/regulatory.db", "lib/firmware/regulatory.db.p7s", "usr/bin/iyakki"]);
        let (p, _) = parse_profile(RULES);
        assert!(image_findings(&left, &p).is_empty());
        let f = image_findings(&entries, &p);
        assert!(f.iter().any(|x| x.contains("amdgpu")) && f.iter().any(|x| x.contains("nvidia")) && f.len() == 2);
    }

    #[test]
    fn exclusions_and_globs_follow_prune_firmware() {
        let (keep, excl) = parse_keep("amdgpu/*\n!amdgpu/isp_*\n# c\n\nregulatory.db\n");
        assert!(kept(&keep, &excl, "amdgpu/psp.bin") && !kept(&keep, &excl, "amdgpu/isp_x.bin") && !kept(&keep, &excl, "i915/x.bin") && kept(&keep, &excl, "regulatory.db"));
    }
}
