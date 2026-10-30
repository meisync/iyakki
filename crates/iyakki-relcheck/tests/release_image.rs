//! The RELEASE image profile, proved without a build (docs/development/RELEASE-IMAGE-PROFILE.md, "release-image-check"):
//! the package set of the release image (tests/data/release-image-packages.txt) is scanned with the real `legal/image-rules.txt`, the
//! keep list and the fragments are checked against `legal/release-profile.txt`, and the shell side runs its own test script.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iyakki_relcheck::assets;
use iyakki_relcheck::cpio::{Entry, Kind};
use iyakki_relcheck::imagescan::{self, Inputs, Report, Rules, Why};
use iyakki_relcheck::profile;
use iyakki_relcheck::sbom::Pkg;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn file(p: &str) -> Entry {
    Entry { path: p.into(), kind: Kind::File, size: 1, mode: 0o644, target: String::new() }
}

/// The release image as the scan sees it: the fixture packages with their files and licences, plus IYAKKI's own files.
struct Fixture {
    entries: Vec<Entry>,
    owner: BTreeMap<String, String>,
    manifest: Vec<Pkg>,
}

fn fixture() -> Fixture {
    let mut f = Fixture { entries: vec![], owner: BTreeMap::new(), manifest: vec![] };
    for l in read("crates/iyakki-relcheck/tests/data/release-image-packages.txt").lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let p: Vec<&str> = l.splitn(3, '|').map(str::trim).collect();
        assert_eq!(p.len(), 3, "bad fixture row: {l}");
        if p[1] != "-" {
            f.manifest.push(Pkg { name: p[0].into(), version: "1".into(), license: p[1].into(), source: String::new(), sha256: String::new(), origin: "buildroot".into() });
        }
        for path in p[2].split_whitespace() {
            f.owner.insert(path.into(), p[0].into());
            f.entries.push(file(path));
        }
    }
    // what no Buildroot package owns: IYAKKI's own files, the licence texts that must travel, the release profile's files
    for own in [
        "usr/bin/iyakki", "usr/libexec/iyakki/gpu-fw-guard", "usr/share/iyakki/firmware-notice.txt", "usr/share/iyakki/firmware-notice.long.txt", "etc/modprobe.d/iyakki-release.conf",
        "etc/iyakki-release", "etc/hostname", "etc/os-release", "init", "usr/share/doc/iyakki/LICENSE", "usr/share/doc/iyakki/NOTICE", "usr/share/doc/iyakki/THIRD_PARTY_LICENSES.txt",
        "usr/share/doc/iyakki/LICENSING.md", "etc/ssl/certs/ca-certificates.crt", "lib/modules/6.12.112/modules.devname", "usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so",
    ] {
        f.entries.push(file(own));
    }
    f
}

fn scan_with(f: &Fixture, rules: &Rules) -> Report {
    let (arows, _) = assets::parse(&read("legal/assets.txt"));
    imagescan::scan(&Inputs { entries: &f.entries, owner: &f.owner, manifest: &f.manifest, rules, assets: &arows, recipes: &BTreeMap::new(), rootfs: true })
}

fn rules() -> Rules {
    let (r, problems) = imagescan::parse_rules(&read("legal/image-rules.txt"));
    assert!(problems.is_empty(), "{problems:?}");
    r
}

#[test]
fn the_release_package_set_passes_the_licence_scan() {
    let r = scan_with(&fixture(), &rules());
    assert!(r.ok(), "{}", r.render("release fixture", true));
    // the waivers that carry the image are the reviewed rows, nothing else
    let waived: Vec<&str> = r.waived.iter().map(|w| w.0.as_str()).collect();
    for want in ["package dejavu", "package gst1-plugins-bad", "package skeleton-init-common", "package ifupdown-scripts"] {
        assert!(waived.contains(&want), "{want} not waived: {waived:?}");
    }
    assert!(r.waived.iter().any(|w| w.0.contains("libgstlibav")), "the parked bridge is a reviewed except row: {:?}", r.waived);
}

#[test]
fn the_fixture_is_a_release_set_without_firmware_packages_or_codecs() {
    let f = fixture();
    for bad in ["linux-firmware", "ffmpeg", "x264", "x265", "libopenh264", "faad2", "gst1-plugins-ugly"] {
        assert!(!f.owner.values().any(|p| p == bad), "{bad} is in the release package set");
    }
    assert!(!f.entries.iter().any(|e| e.path.starts_with("lib/firmware/") && !e.path.starts_with("lib/firmware/regulatory.db")), "firmware in the fixture");
    assert!(f.owner.values().any(|p| p == "wireless-regdb") && f.owner.values().any(|p| p == "apple-bce") && f.owner.values().any(|p| p == "gst1-plugins-bad"));
}

#[test]
fn the_gate_catches_every_item_the_profile_removes() {
    let base = fixture();
    let r = rules();
    let add = |paths: &[(&str, Option<(&str, &str)>)]| -> Fixture {
        let mut f = Fixture { entries: base.entries.clone(), owner: base.owner.clone(), manifest: base.manifest.clone() };
        for (p, pkg) in paths {
            f.entries.push(file(p));
            if let Some((name, lic)) = pkg {
                f.owner.insert((*p).into(), (*name).into());
                if !f.manifest.iter().any(|m| m.name == *name) {
                    f.manifest.push(Pkg { name: (*name).into(), version: "1".into(), license: (*lic).into(), source: String::new(), sha256: String::new(), origin: "buildroot".into() });
                }
            }
        }
        f
    };
    let has = |rep: &Report, w: Why, key: &str| rep.findings.iter().any(|f| f.why == w && f.key.contains(key));
    // linux-firmware (the 355 to 1375 `Proprietary` files)
    let rep = scan_with(&add(&[("lib/firmware/amdgpu/psp_13_0_0_sos.bin", Some(("linux-firmware", "Proprietary")))]), &r);
    assert!(has(&rep, Why::ProprietaryPackage, "linux-firmware"), "{}", rep.render("t", true));
    // NVIDIA GSP (438 files in the PC image)
    let rep = scan_with(&add(&[("lib/firmware/nvidia/ga102/gsp/gsp-535.113.01.bin", None)]), &r);
    assert!(has(&rep, Why::Denied, "nvidia"), "{}", rep.render("t", true));
    // firmware that no package owns (the PC repack's 318 files)
    let rep = scan_with(&add(&[("lib/firmware/iwlwifi-9000-pu-b0-jf-b0-46.ucode", None)]), &r);
    assert!(has(&rep, Why::Unattributed, "lib/firmware"), "{}", rep.render("t", true));
    // Apple blobs
    let rep = scan_with(&add(&[("lib/firmware/brcm/brcmfmac4364b2-pcie.apple,j140k.bin", None), ("lib/firmware/facetimehd/firmware.bin", None)]), &r);
    assert!(has(&rep, Why::Denied, "apple") && has(&rep, Why::Denied, "facetimehd"), "{}", rep.render("t", true));
    // H.264/AAC: FFmpeg libraries, encoder plugins, an ACTIVE bridge
    let rep = scan_with(&add(&[("usr/lib/libavcodec.so.60", Some(("ffmpeg", "LGPL-2.1+"))), ("usr/lib/gstreamer-1.0/libgstx264.so", None), ("usr/lib/gstreamer-1.0/libgstlibav.so", None), ("usr/bin/ffmpeg", None)]), &r);
    assert!(has(&rep, Why::Denied, "libavcodec") && has(&rep, Why::Denied, "libgstx264") && has(&rep, Why::Denied, "libgstlibav") && has(&rep, Why::Denied, "usr/bin/ffmpeg"), "{}", rep.render("t", true));
    // a restricted font
    let rep = scan_with(&add(&[("usr/share/fonts/arial.ttf", None)]), &r);
    assert!(has(&rep, Why::Denied, "arial"), "{}", rep.render("t", true));
}

#[test]
fn the_gst_plugins_bad_row_is_what_lets_the_unknown_licence_through() {
    let f = fixture();
    let mut r = rules();
    let before = r.accept.len();
    r.accept.retain(|a| a.0 != "gst1-plugins-bad");
    assert_eq!(r.accept.len(), before - 1, "there is exactly one accept row for gst1-plugins-bad");
    let rep = scan_with(&f, &r);
    assert!(rep.findings.iter().any(|x| x.why == Why::UnknownLicence && x.key == "gst1-plugins-bad"), "{}", rep.render("t", true));
}

#[test]
fn the_keep_list_the_fragments_and_the_rules_agree() {
    let (prof, problems) = profile::parse_profile(&read(profile::PROFILE_RULES));
    assert!(problems.is_empty(), "{problems:?}");
    let keep_text = read(profile::KEEP_FILE);
    assert!(profile::keep_problems(&keep_text, &prof).is_empty(), "{:?}", profile::keep_problems(&keep_text, &prof));
    // the default keeps exactly the two regulatory-database files and nothing from linux-firmware
    let (keep, excl) = profile::parse_keep(&keep_text);
    assert_eq!(keep, vec!["regulatory.db".to_string(), "regulatory.db.p7s".to_string()], "release-firmware.keep keeps more than the regulatory database");
    assert!(excl.is_empty());
    let frags: Vec<(&str, String)> = profile::FRAGMENTS.iter().map(|f| (*f, read(f))).collect();
    let p = profile::fragment_problems(&frags, &prof);
    assert!(p.is_empty(), "{p:?}");
    // the gst-plugins-bad plugins the web fragment turns on are all reviewed
    let web = read("dist/buildroot/configs/iyakki_web.fragment");
    let p = profile::config_problems(&web, &prof);
    assert!(p.is_empty(), "iyakki_web.fragment turns on something the release profile does not allow: {p:?}");
    // the firmware fragment (the `hw` and Mac recipes) is exactly what the release must not use
    let fw = profile::config_problems(&read("dist/buildroot/configs/iyakki_firmware.fragment"), &prof);
    assert!(fw.iter().any(|x| x.contains("BR2_PACKAGE_LINUX_FIRMWARE is on")), "the firmware fragment should be refused by the profile");
}

#[test]
fn a_release_image_listing_with_firmware_is_refused_and_one_without_passes() {
    let (prof, _) = profile::parse_profile(&read(profile::PROFILE_RULES));
    let keep = read(profile::KEEP_FILE);
    let pc = vec![file("lib/firmware/regulatory.db"), file("lib/firmware/regulatory.db.p7s"), file("lib/firmware/amdgpu/psp.bin"), file("lib/firmware/nvidia/ga102/gsp/gsp.bin"), file("usr/share/doc/linux-firmware/WHENCE"), file("usr/bin/iyakki")];
    assert_eq!(profile::image_findings(&pc, &prof).len(), 2);
    let left = profile::simulate(&pc, &keep);
    assert!(profile::image_findings(&left, &prof).is_empty());
    assert_eq!(left.len(), 3);
}

#[test]
fn the_shell_side_of_the_profile_passes_its_own_test() {
    if std::process::Command::new("sh").arg("-c").arg("true").status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("no sh: skipped");
        return;
    }
    let out = std::process::Command::new("sh").arg(root().join("dist/buildroot/test-release-profile.sh")).output().expect("run the script");
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("all checks passed"));
}
