//! What the IYAKKI menu's About page shows: which IYAKKI this is (version, build, variant, kernel), its own licence, and the
//! licences of everything inside: the Rust crates linked into the program (`THIRD_PARTY_LICENSES.txt`, embedded at build time;
//! the images also carry it in `/usr/share/doc/iyakki/`) and the programs of the system images (the Buildroot packages of
//! `SBOM.json`, extracted by `build.rs`). Plain functions over text, so the tests feed them their own.

use iyakki_agent::relinfo::ReleaseInfo;

/// The licences of the Rust crates in this program, as `cargo about` wrote them.
pub const THIRD_PARTY: &str = include_str!("../../../THIRD_PARTY_LICENSES.txt");
/// The image components: `name \t version \t licence \t image` lines.
pub const IMAGE_COMPONENTS: &str = include_str!(concat!(env!("OUT_DIR"), "/image_components.tsv"));
/// IYAKKI's own licence.
pub const OWN_LICENCE: &str = "Apache-2.0";

/// The licence boundary in plain words (from BOUNDARY.md and LICENSING.md), one point per entry.
pub const LICENCE_BOUNDARY: [&str; 6] = [
    "IYAKKI's own code is Apache-2.0.",
    "The base OS (the Linux kernel GPL-2.0, BusyBox GPL-2.0, GRUB GPL-3.0+, alsa-utils, e2fsprogs, iproute2 and the rest) runs as separate programs or through the kernel's system-call interface; none of it is linked into IYAKKI.",
    "The Mac image's kernel patch and its apple-bce module (t2linux, with two IYAKKI patches) are GPL-2.0 kernel code.",
    "Firmware files in the hw image are redistributable but not open source; each keeps its own licence.",
    "Apple firmware (Wi-Fi, Bluetooth, the FaceTime camera) is never shipped: you copy it from your own Mac.",
    "The source of every GPL and LGPL part comes with the image's legal-info. The full list of dependency licences follows below.",
];

/// What is known about the running kernel's patches: on the Mac image, the i915 patch and apple-bce's two.
pub fn kernel_patches(variant: &str, apple_bce_loaded: bool) -> Option<&'static str> {
    (variant.to_lowercase().contains("mac") || apple_bce_loaded).then_some("with 1 IYAKKI kernel patch (i915 display clock) and the t2linux apple-bce module (2 IYAKKI patches)")
}

/// One licence and the crates under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenceGroup {
    pub name: String,
    pub crates: Vec<String>,
}

/// A program of a system image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub version: String,
    pub licence: String,
    pub image: String,
}

/// The licence groups of a `THIRD_PARTY_LICENSES.txt` (blocks between `====` lines: the name, `Used by:`, `  - crate version`),
/// with blocks of the same licence merged, largest first; and the notes before the first block (the kernel-side paragraph).
pub fn parse_third_party(text: &str) -> (Vec<String>, Vec<LicenceGroup>) {
    let mut blocks = text.split("\n================================================================================\n");
    let head = blocks.next().unwrap_or("");
    // the notes: the paragraph after the title lines, up to the per-licence counts
    let notes: Vec<String> = head.split("\n\n").nth(1).map(|p| vec![p.split_whitespace().collect::<Vec<_>>().join(" ")]).unwrap_or_default();
    let mut groups: Vec<LicenceGroup> = Vec::new();
    for b in blocks {
        let mut lines = b.lines();
        let Some(name) = lines.by_ref().map(str::trim).find(|l| !l.is_empty()) else { continue };
        let mut crates = Vec::new();
        for l in lines {
            if l.starts_with("----") {
                break;
            }
            if let Some(c) = l.trim_start().strip_prefix("- ") {
                crates.push(c.trim().to_string());
            }
        }
        match groups.iter_mut().find(|g| g.name == name) {
            Some(g) => g.crates.extend(crates),
            None => groups.push(LicenceGroup { name: name.to_string(), crates }),
        }
    }
    for g in &mut groups {
        g.crates.sort();
        g.crates.dedup();
    }
    groups.sort_by(|a, b| b.crates.len().cmp(&a.crates.len()).then(a.name.cmp(&b.name)));
    (notes, groups)
}

/// The image components of a TSV made by `build.rs`.
pub fn parse_components(tsv: &str) -> Vec<Component> {
    let mut v: Vec<Component> = tsv
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 4 && !f[0].is_empty()).then(|| Component { name: f[0].into(), version: f[1].into(), licence: f[2].into(), image: f[3].into() })
        })
        .collect();
    v.sort_by(|a, b| a.image.cmp(&b.image).then(a.name.cmp(&b.name)));
    v
}

/// "libre image", "web image", "libre image (aarch64)" from the SBOM's supplier word.
pub fn image_name(word: &str) -> String {
    let w = word.trim_start_matches("buildroot-");
    match w.split_once('-') {
        Some((v, arch)) => format!("{v} image ({arch})"),
        None => format!("{w} image"),
    }
}

/// The facts at the top: version, build, source, variant, kernel, licence.
pub fn facts(release: Option<&ReleaseInfo>, running_kernel: Option<&str>) -> Vec<(String, String)> {
    facts_with(release, running_kernel, false)
}

/// [`facts`], knowing whether the apple-bce module is loaded (`/sys/module/apple_bce`).
pub fn facts_with(release: Option<&ReleaseInfo>, running_kernel: Option<&str>, apple_bce: bool) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = match release {
        Some(r) => r.rows().into_iter().map(|(l, x)| (l.to_string(), x)).collect(),
        None => vec![("Version".into(), iyakki_agent::relinfo::VERSION.to_string()), ("Build".into(), "not running from an image (a development build)".into())],
    };
    if let Some(k) = running_kernel.map(str::trim).filter(|k| !k.is_empty()) {
        match v.iter_mut().find(|(l, _)| l == "Kernel") {
            Some((_, x)) if x != k => *x = format!("{k} (running; the image was built with {x})"),
            Some(_) => {}
            None => v.push(("Kernel".into(), format!("{k} (running)"))),
        }
    }
    if let Some(p) = kernel_patches(release.map_or("", |r| r.variant.as_str()), apple_bce) {
        if let Some((_, x)) = v.iter_mut().find(|(l, _)| l == "Kernel") {
            x.push_str(&format!(" {p}"));
        }
    }
    v.push(("Licence".into(), format!("{OWN_LICENCE} (IYAKKI itself: see LICENSE and NOTICE)")));
    v
}

/// How a line of the About list is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A section title.
    Heading,
    /// `label \t value`.
    Fact,
    /// A licence name with its count.
    Licence,
    /// Crate or component names under it.
    Item,
    /// Explanations.
    Note,
}

/// Everything the About page lists, top to bottom, as lines to scroll through. Long crate lists are wrapped by the caller.
pub fn lines(facts: &[(String, String)], notes: &[String], groups: &[LicenceGroup], comps: &[Component]) -> Vec<(String, Style)> {
    let mut v: Vec<(String, Style)> = vec![("IYAKKI, an OS for agents".into(), Style::Heading)];
    v.extend(facts.iter().map(|(l, x)| (format!("{l}\t{x}"), Style::Fact)));
    v.push((String::new(), Style::Note));
    v.push(("Licence boundary".into(), Style::Heading));
    v.extend(LICENCE_BOUNDARY.iter().map(|t| (format!("\u{2022} {t}"), Style::Note)));
    let total: usize = groups.iter().map(|g| g.crates.len()).sum();
    v.push((String::new(), Style::Note));
    v.push((format!("Rust crates in this program: {total} under {} licences", groups.len()), Style::Heading));
    v.push(("Linked into the iyakki program. The full licence texts are in THIRD_PARTY_LICENSES.txt (on the images: /usr/share/doc/iyakki/).".into(), Style::Note));
    for g in groups {
        v.push((format!("{} \u{00b7} {} crate{}", g.name, g.crates.len(), if g.crates.len() == 1 { "" } else { "s" }), Style::Licence));
        v.push((g.crates.join(", "), Style::Item));
    }
    if !comps.is_empty() {
        let mut images: Vec<&str> = comps.iter().map(|c| c.image.as_str()).collect();
        images.dedup();
        v.push((String::new(), Style::Note));
        v.push((format!("Programs in the system images: {} packages", comps.len()), Style::Heading));
        v.push(("Separate programs built by Buildroot (run, never linked). Their source comes with the image's legal-info.".into(), Style::Note));
        for img in images {
            v.push((image_name(img), Style::Licence));
            for c in comps.iter().filter(|c| c.image == img) {
                v.push((format!("{} {} \u{00b7} {}", c.name, c.version, c.licence), Style::Item));
            }
        }
    }
    if !notes.is_empty() {
        v.push((String::new(), Style::Note));
        v.push(("Kernel side".into(), Style::Heading));
        v.extend(notes.iter().map(|n| (n.clone(), Style::Note)));
    }
    v
}

/// The About row "Secure Boot": the firmware's state (the 5th byte of the `SecureBoot` EFI variable) and the kernel's lockdown mode.
pub fn secure_boot_fact(secure_boot_var: &[u8], lockdown_file: Option<&str>) -> String {
    let on = secure_boot_var.get(4) == Some(&1);
    let (mode, _) = iyakki_hwinfo::secure_boot_summary(on, lockdown_file);
    format!("{}, kernel lockdown: {mode}", if on { "ON" } else { "off" })
}

/// The About lines of this program and machine.
pub fn this_machine() -> Vec<(String, Style)> {
    let release = ReleaseInfo::load(std::path::Path::new(iyakki_agent::relinfo::RELEASE_FILE));
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    let (notes, groups) = parse_third_party(THIRD_PARTY);
    let bce = std::path::Path::new("/sys/module/apple_bce").exists();
    let mut facts = facts_with(release.as_ref(), kernel.as_deref(), bce);
    if let Ok(var) = std::fs::read("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c") {
        let lock = std::fs::read_to_string("/sys/kernel/security/lockdown").ok();
        let at = facts.iter().position(|(l, _)| l == "Licence").unwrap_or(facts.len());
        facts.insert(at, ("Secure Boot".into(), secure_boot_fact(&var, lock.as_deref())));
    }
    lines(&facts, &notes, &groups, &parse_components(IMAGE_COMPONENTS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "IYAKKI third-party Rust dependency licenses\n(generated by cargo about)\n\nKernel-side code is not listed here: it is\nGPL-2.0 like the Linux kernel.\n\nApache License 2.0 — 2 crates\nMIT License — 1 crates\n\n================================================================================\nApache License 2.0\nUsed by:\n  - zeta 1.0.0\n--------------------------------------------------------------------------------\nApache text\n\n================================================================================\nMIT License\nUsed by:\n  - beta 0.2.0\n--------------------------------------------------------------------------------\nMIT text - not a crate\n  - nor this\n\n================================================================================\nApache License 2.0\nUsed by:\n  - alpha 2.1.0\n  - zeta 1.0.0\n--------------------------------------------------------------------------------\nApache text again\n";

    #[test]
    fn the_secure_boot_row_names_the_firmware_state_and_the_lockdown_mode() {
        assert_eq!(secure_boot_fact(&[6, 0, 0, 0, 1], Some("none [integrity] confidentiality")), "ON, kernel lockdown: integrity");
        assert_eq!(secure_boot_fact(&[6, 0, 0, 0, 0], Some("[none] integrity confidentiality")), "off, kernel lockdown: none");
    }

    #[test]
    fn the_licence_boundary_says_what_is_linked_what_is_separate_and_where_the_sources_are() {
        let all = LICENCE_BOUNDARY.join(" ");
        for w in ["Apache-2.0", "GPL-2.0", "separate programs", "legal-info", "never shipped", "not open source"] {
            assert!(all.contains(w), "{w}");
        }
        let text: Vec<String> = lines(&facts(None, None), &[], &[], &[]).into_iter().map(|l| l.0).collect();
        let at = text.iter().position(|t| t == "Licence boundary").expect("a Licence boundary section");
        assert!(text[at + 1].contains("Apache-2.0"));
        // the Mac image's kernel says it carries patches; others just give the release
        let mac = ReleaseInfo::parse("VERSION=0.1.0-alpha.1\nVARIANT=mac\nKERNEL=6.12.112\n");
        let f = facts(Some(&mac), Some("6.12.112"));
        assert!(f.iter().any(|(k, v)| k == "Kernel" && v.starts_with("6.12.112 with 1 IYAKKI kernel patch")), "{f:?}");
        let libre = ReleaseInfo::parse("VERSION=0.1.0-alpha.1\nVARIANT=libre\nKERNEL=6.12.112\n");
        assert!(facts(Some(&libre), Some("6.12.112")).iter().any(|(k, v)| k == "Kernel" && v == "6.12.112"));
    }

    #[test]
    fn licence_blocks_are_grouped_merged_and_counted_without_reading_the_texts() {
        let (notes, groups) = parse_third_party(SAMPLE);
        assert_eq!(notes, ["Kernel-side code is not listed here: it is GPL-2.0 like the Linux kernel."]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], LicenceGroup { name: "Apache License 2.0".into(), crates: vec!["alpha 2.1.0".into(), "zeta 1.0.0".into()] });
        assert_eq!(groups[1].crates, ["beta 0.2.0"], "lines of the licence text are not crates");
    }

    #[test]
    fn the_embedded_file_lists_hundreds_of_crates_under_permissive_licences() {
        let (_, groups) = parse_third_party(THIRD_PARTY);
        let total: usize = groups.iter().map(|g| g.crates.len()).sum();
        assert!(total > 100, "{total}");
        assert!(groups.iter().any(|g| g.name == "Apache License 2.0"));
        assert!(groups.iter().any(|g| g.name == "MIT License"));
        assert!(groups.iter().all(|g| !g.name.contains("GPL") || g.name.contains("Lesser")), "no copyleft licence group: {:?}", groups.iter().map(|g| &g.name).collect::<Vec<_>>());
    }

    #[test]
    fn image_components_come_from_the_sbom_with_their_licences() {
        let c = parse_components("busybox\t1.37.0\tGPL-2.0\tbuildroot-libre\nlinux\t6.12.112\tGPL-2.0\tbuildroot-libre\n\tbad\nwpewebkit\t2.48\tLGPL-2.1\tbuildroot-web\n");
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].name.as_str(), c[2].image.as_str()), ("busybox", "buildroot-web"));
        assert_eq!(image_name("buildroot-libre"), "libre image");
        assert_eq!(image_name("buildroot-libre-aarch64"), "libre image (aarch64)");
        // the repository's SBOM has the kernel of the libre image
        let real = parse_components(IMAGE_COMPONENTS);
        assert!(real.iter().any(|c| c.name == "linux" && c.licence.contains("GPL")), "{} components", real.len());
    }

    #[test]
    fn the_facts_show_version_variant_the_running_kernel_and_apache_2() {
        let r = ReleaseInfo::parse("VERSION=0.1.0-alpha.1\nBUILD_DATE=2026-10-17\nVARIANT=libre\nKERNEL=6.12.112\n");
        let f = facts(Some(&r), Some("6.12.112\n"));
        let get = |l: &str| f.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("Version"), "0.1.0-alpha.1");
        assert_eq!(get("Variant"), "libre");
        assert_eq!(get("Kernel"), "6.12.112");
        assert!(get("Licence").starts_with("Apache-2.0"));
        let f = facts(Some(&r), Some("6.13.0"));
        assert!(f.iter().any(|(k, v)| k == "Kernel" && v.contains("6.13.0 (running") && v.contains("6.12.112")));
        let dev = facts(None, Some("6.8.0-generic"));
        assert!(dev.iter().any(|(k, v)| k == "Build" && v.contains("development")));
        assert!(dev.iter().any(|(k, v)| k == "Kernel" && v == "6.8.0-generic (running)"));
        let all = lines(&dev, &["note".into()], &parse_third_party(SAMPLE).1, &[Component { name: "busybox".into(), version: "1.37.0".into(), licence: "GPL-2.0".into(), image: "buildroot-libre".into() }]);
        let text: Vec<&str> = all.iter().map(|l| l.0.as_str()).collect();
        assert!(text.contains(&"Apache License 2.0 \u{00b7} 2 crates") && text.contains(&"alpha 2.1.0, zeta 1.0.0"), "{text:?}");
        assert!(text.contains(&"busybox 1.37.0 \u{00b7} GPL-2.0"));
        assert!(text.contains(&"Kernel side"));
    }
}
