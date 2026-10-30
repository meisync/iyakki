//! Content scan of an image: every file of a Buildroot `rootfs.cpio` (or an unpacked tree, or a names list) is mapped to the
//! Buildroot package that installed it (`build/packages-file-list.txt`), that package to its licence (`legal-info/manifest.csv`),
//! and the result is checked against the review file `legal/image-rules.txt`. The scan fails on
//!   * a file that matches a `deny` rule (vendor blobs, patent-encumbered codecs, restricted fonts) unless an `except` row names it,
//!   * a file of a package whose licence is proprietary, unknown or missing from the manifest (unless an `accept` row names the package),
//!   * a file that no package owns and no `own` / `generated` rule explains,
//!   * a font, icon, sound, video or model file that is neither owned by a licensed package nor listed in `legal/assets.txt`.
//! The scan reads; it never unpacks anything.

use std::collections::{BTreeMap, BTreeSet};

use crate::assets::AssetRow;
use crate::cpio::{Entry, Kind};
use crate::sbom::Pkg;
use crate::spdx::{self, Class};
use crate::table::{self, glob};

#[derive(Debug, Default, Clone)]
pub struct Rules {
    pub own: Vec<(String, String)>,
    pub generated: Vec<(String, String)>,
    pub deny: Vec<(String, String)>,
    /// glob, reason, date
    pub except: Vec<(String, String, String)>,
    /// package, reason, date
    pub accept: Vec<(String, String, String)>,
    /// path, reason: must exist in a root file system (licence and notice files)
    pub require: Vec<(String, String)>,
}

/// Rows: `own|GLOB|reason`, `generated|GLOB|reason`, `deny|GLOB|reason`, `except|GLOB|reason|date`, `accept|PACKAGE|reason|date`.
/// Globs match the lower-case path without a leading slash.
pub fn parse_rules(text: &str) -> (Rules, Vec<String>) {
    let (mut r, mut problems) = (Rules::default(), Vec::new());
    for row in table::parse(text) {
        let f = &row.f;
        let bad = |m: &str, problems: &mut Vec<String>| problems.push(format!("image rules line {}: {m}", row.line));
        match (f[0].as_str(), f.len()) {
            ("own", 3) | ("generated", 3) | ("deny", 3) | ("require", 3) if table::valid_reason(&f[2]) => {
                let t = (f[1].to_lowercase(), f[2].clone());
                match f[0].as_str() {
                    "own" => r.own.push(t),
                    "generated" => r.generated.push(t),
                    "require" => r.require.push(t),
                    _ => r.deny.push(t),
                }
            }
            ("except", 4) | ("accept", 4) if table::valid_reason(&f[2]) && table::valid_date(&f[3]) => {
                if f[0] == "except" {
                    r.except.push((f[1].to_lowercase(), f[2].clone(), f[3].clone()))
                } else {
                    r.accept.push((f[1].clone(), f[2].clone(), f[3].clone()))
                }
            }
            ("own" | "generated" | "deny" | "except" | "accept" | "require", _) => bad("wrong number of fields, or a missing reason / date", &mut problems),
            _ => bad("unknown row type", &mut problems),
        }
    }
    (r, problems)
}

/// `package,./path` lines of Buildroot's `packages-file-list.txt`.
pub fn parse_file_list(text: &str, into: &mut BTreeMap<String, String>) {
    for l in text.lines() {
        if let Some((pkg, path)) = l.split_once(',') {
            into.entry(crate::cpio::norm(path)).or_insert_with(|| pkg.to_string());
        }
    }
}

const FONT: &[&str] = &["pf2", "ttf", "otf", "ttc", "woff", "woff2", "pfb", "pfa", "pcf", "bdf", "eot"];
const AUDIO: &[&str] = &["wav", "mp3", "ogg", "oga", "flac", "opus", "m4a", "aac", "mid", "midi", "aif", "aiff", "wma"];
const VIDEO: &[&str] = &["mp4", "m4v", "webm", "mkv", "mov", "avi", "mpg", "mpeg"];
const MODEL: &[&str] = &["gguf", "onnx", "safetensors", "pt", "pth", "tflite", "ggml", "npz", "mlmodel"];
const ICON: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "svgz", "ico", "icns", "webp", "xpm", "bmp"];

/// The class of artwork or data a path is, by extension.
pub fn media_class(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_lowercase();
    if path.rsplit('/').next()? == ext {
        return None;
    }
    let e = ext.as_str();
    [(FONT, "font"), (AUDIO, "audio"), (VIDEO, "video"), (MODEL, "model weights"), (ICON, "image or icon")].iter().find(|(l, _)| l.contains(&e)).map(|(_, n)| *n)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    Denied,
    ProprietaryPackage,
    UnknownLicence,
    NoLicenceInfo,
    Unattributed,
    UnlistedMedia,
    MissingNotice,
}

impl Why {
    pub fn label(self) -> &'static str {
        match self {
            Why::Denied => "DENIED FILE",
            Why::ProprietaryPackage => "PROPRIETARY PACKAGE",
            Why::UnknownLicence => "UNRECOGNISED LICENCE",
            Why::NoLicenceInfo => "NO LICENCE INFO",
            Why::Unattributed => "UNATTRIBUTED",
            Why::UnlistedMedia => "UNLISTED MEDIA",
            Why::MissingNotice => "MISSING NOTICE FILE",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub why: Why,
    /// The package, or the deny rule, the finding is about.
    pub key: String,
    pub detail: String,
    pub count: usize,
    pub samples: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub files: usize,
    pub findings: Vec<Finding>,
    /// Files waived by an `except` row or a package waived by an `accept` row: (what, reason, date, files).
    pub waived: Vec<(String, String, String, usize)>,
    /// package -> (files, licence, class)
    pub packages: BTreeMap<String, (usize, String, String)>,
    pub own: usize,
    pub generated: usize,
    /// package -> (files, licence declared by its recipe) for packages with no legal-info row
    pub legal_missing: BTreeMap<String, (usize, String)>,
}

pub struct Inputs<'a> {
    pub entries: &'a [Entry],
    pub owner: &'a BTreeMap<String, String>,
    pub manifest: &'a [Pkg],
    pub rules: &'a Rules,
    pub assets: &'a [AssetRow],
    /// Licences declared by the package recipes (fallback for packages missing from the manifest); may be empty.
    pub recipes: &'a BTreeMap<String, String>,
    /// A root file system (not an installer tree): `require` rows apply.
    pub rootfs: bool,
}

pub fn scan(i: &Inputs) -> Report {
    let mut rep = Report::default();
    let lic: BTreeMap<&str, &Pkg> = i.manifest.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut groups: BTreeMap<(Why, String), Finding> = BTreeMap::new();
    let mut waived: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut add = |why: Why, key: &str, detail: &str, path: &str| {
        let f = groups.entry((why, key.to_string())).or_insert_with(|| Finding { why, key: key.to_string(), detail: detail.to_string(), count: 0, samples: vec![] });
        f.count += 1;
        if f.samples.len() < 5 {
            f.samples.push(path.to_string());
        }
    };
    let links: BTreeMap<&str, &str> = i.entries.iter().filter(|e| e.kind == Kind::Symlink && !e.target.is_empty()).map(|e| (e.path.as_str(), e.target.as_str())).collect();
    let mut pkg_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut legal_missing: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for e in i.entries.iter().filter(|e| e.kind != Kind::Dir) {
        rep.files += 1;
        let low = e.path.to_lowercase();
        let pkg = i.owner.get(&e.path).cloned().or_else(|| resolve_link(&e.path, &links, i.owner));
        // 1. deny rules
        if let Some((pat, why)) = i.rules.deny.iter().find(|(p, _)| match p.strip_prefix("font:") {
            Some(q) => media_class(&e.path) == Some("font") && glob(q, &low),
            None => glob(p, &low),
        }) {
            if let Some((_, reason, date)) = i.rules.except.iter().find(|(p, _, _)| glob(p, &low)) {
                *waived.entry((format!("file {}", e.path), reason.clone(), date.clone())).or_default() += 1;
            } else {
                add(Why::Denied, pat, why, &e.path);
                continue;
            }
        }
        // 2. package and its licence
        if let Some(p) = &pkg {
            *pkg_seen.entry(p.clone()).or_default() += 1;
            let accepted = i.rules.accept.iter().find(|(n, _, _)| n == p);
            match lic.get(p.as_str()) {
                None if accepted.is_none() => {
                    let rec = i.recipes.get(&p.replace('_', "-")).cloned();
                    match rec {
                        None => add(Why::NoLicenceInfo, p, "no row in any legal-info/manifest.csv and no licence in a package recipe: nothing says what this is", &e.path),
                        Some(r) => {
                            let (class, _) = spdx::classify_list(&r);
                            match class {
                                Class::Proprietary => add(Why::ProprietaryPackage, p, &format!("recipe says `{r}`"), &e.path),
                                Class::Unknown => add(Why::UnknownLicence, p, &format!("recipe says `{r}`, which this gate does not recognise"), &e.path),
                                _ => {
                                    let m = legal_missing.entry(p.clone()).or_insert((0, r.clone()));
                                    m.0 += 1;
                                }
                            }
                        }
                    }
                }
                Some(m) => {
                    let (class, _) = spdx::classify_list(&m.license);
                    match (class, accepted) {
                        (Class::Proprietary, None) => add(Why::ProprietaryPackage, p, &format!("licence `{}`", m.license), &e.path),
                        (Class::Unknown, None) => add(Why::UnknownLicence, p, &format!("licence `{}` is not one this gate recognises", m.license), &e.path),
                        _ => {}
                    }
                }
                _ => {}
            }
            if let Some((n, reason, date)) = accepted {
                *waived.entry((format!("package {n}"), reason.clone(), date.clone())).or_default() += 1;
            }
            continue;
        }
        // 3. no package: explained by a rule, or an asset in the inventory, or a failure
        if i.rules.own.iter().any(|(p, _)| glob(p, &low)) {
            rep.own += 1;
            if let Some(c) = media_class(&e.path) {
                if !i.assets.iter().any(|a| a.image_paths.iter().any(|p| glob(&p.to_lowercase(), &low))) {
                    add(Why::UnlistedMedia, c, &format!("{c} file shipped in the image that legal/assets.txt does not list (column image-paths)"), &e.path);
                }
            }
            continue;
        }
        if i.rules.generated.iter().any(|(p, _)| glob(p, &low)) {
            rep.generated += 1;
            continue;
        }
        if let Some(c) = media_class(&e.path) {
            if i.assets.iter().any(|a| a.image_paths.iter().any(|p| glob(&p.to_lowercase(), &low))) {
                rep.own += 1;
                continue;
            }
            add(Why::UnlistedMedia, c, &format!("{c} file that no package owns and legal/assets.txt does not list"), &e.path);
            continue;
        }
        let top: String = e.path.split('/').take(3).collect::<Vec<_>>().join("/");
        add(Why::Unattributed, &top, "no package owns this file and no `own` / `generated` rule explains it", &e.path);
    }
    if i.rootfs {
        let present: BTreeSet<String> = i.entries.iter().map(|e| e.path.to_lowercase()).collect();
        for (path, why) in &i.rules.require {
            if !present.contains(path) {
                add(Why::MissingNotice, path, why, path);
            }
        }
    }
    for (p, n) in &pkg_seen {
        let (l, c) = match lic.get(p.as_str()) {
            Some(m) => (m.license.clone(), format!("{:?}", spdx::classify_list(&m.license).0)),
            None => ("(no manifest row)".into(), "Unknown".into()),
        };
        rep.packages.insert(p.clone(), (*n, l, c));
    }
    rep.findings = groups.into_values().collect();
    if !legal_missing.is_empty() {
        let files = legal_missing.values().map(|v| v.0).sum();
        let names: Vec<String> = legal_missing.iter().map(|(k, v)| format!("{k} [{}]", v.1)).collect();
        rep.findings.push(Finding { why: Why::NoLicenceInfo, key: format!("{} packages with a recipe licence but no legal-info row", legal_missing.len()), detail: "licence texts and the source archive were not collected for these (`make legal-info` in that Buildroot build); the source offer for the copyleft ones cannot be met from this build".into(), count: files, samples: names });
    }
    rep.legal_missing = legal_missing;
    rep.findings.sort_by(|a, b| (a.why, &a.key).cmp(&(b.why, &b.key)));
    rep.waived = waived.into_iter().map(|((w, r, d), n)| (w, r, d, n)).collect();
    rep
}

impl Report {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn render(&self, name: &str, verbose: bool) -> String {
        let mut s = format!("== image scan: {name}: {} files, {} packages, {} own, {} generated\n", self.files, self.packages.len(), self.own, self.generated);
        let mut by: BTreeMap<Why, Vec<&Finding>> = BTreeMap::new();
        self.findings.iter().for_each(|f| by.entry(f.why).or_default().push(f));
        for (why, fs) in &by {
            let n: usize = fs.iter().map(|f| f.count).sum();
            s.push_str(&format!("-- {} ({} groups, {n} files)\n", why.label(), fs.len()));
            for f in fs {
                s.push_str(&format!("   {}: {} file(s) -- {}\n", f.key, f.count, f.detail));
                for x in f.samples.iter().take(if f.key.contains("packages with a recipe licence") { usize::MAX } else if verbose { 5 } else { 2 }) {
                    s.push_str(&format!("       {x}\n"));
                }
            }
        }
        for (w, r, d, n) in &self.waived {
            s.push_str(&format!("-- waived: {w} ({n} file(s)): {r} [{d}]\n"));
        }
        s.push_str(&format!("image scan {name}: {} finding group(s)\n", self.findings.len()));
        s
    }
}

/// A symbolic link belongs to the package that owns what it points to (Buildroot's file list names the real files; the `.so` links and
/// the CA certificate links are made later). Follows up to eight hops.
fn resolve_link(path: &str, links: &BTreeMap<&str, &str>, owner: &BTreeMap<String, String>) -> Option<String> {
    let mut cur = path.to_string();
    for _ in 0..8 {
        let t = links.get(cur.as_str())?;
        let joined = if t.starts_with('/') { t.trim_start_matches('/').to_string() } else { format!("{}/{}", cur.rsplit_once('/').map(|x| x.0).unwrap_or(""), t) };
        let mut parts: Vec<&str> = Vec::new();
        for c in joined.split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                x => parts.push(x),
            }
        }
        cur = parts.join("/");
        if let Some(p) = owner.get(&cur) {
            return Some(p.clone());
        }
    }
    None
}

/// What a Buildroot package recipe declares (`<PKG>_LICENSE = ...` in its `.mk` file), name -> licence list, from the recipe folders given
/// (Buildroot's `package/` and this repository's `dist/buildroot/package/`). This is the fallback when a build never ran
/// `make legal-info`: it tells what the licence is, but the licence texts and the source archive were not collected.
pub fn recipe_licences(dirs: &[std::path::PathBuf]) -> BTreeMap<String, String> {
    fn rec(dir: &std::path::Path, out: &mut BTreeMap<String, Vec<String>>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(&p, out);
            } else if p.extension().is_some_and(|x| x == "mk") {
                let Ok(t) = std::fs::read_to_string(&p) else { continue };
                for l in t.lines() {
                    let Some((var, val)) = l.split_once('=') else { continue };
                    let var = var.trim_end_matches([':', '+', ' ', '\t']);
                    let Some(pkg) = var.strip_suffix("_LICENSE") else { continue };
                    if pkg.is_empty() || !pkg.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') || val.contains("$(") {
                        continue;
                    }
                    let v = val.trim();
                    if !v.is_empty() {
                        out.entry(pkg.to_lowercase().replace('_', "-")).or_default().push(v.to_string());
                    }
                }
            }
        }
    }
    let mut m = BTreeMap::new();
    dirs.iter().for_each(|d| rec(d, &mut m));
    m.into_iter().map(|(k, v)| (k, v.join(", "))).collect()
}

/// The names of packages that own files but have no licence row, for the legal-info completeness report.
pub fn packages_without_licence(owner: &BTreeMap<String, String>, manifest: &[Pkg]) -> BTreeSet<String> {
    let known: BTreeSet<&str> = manifest.iter().map(|p| p.name.as_str()).collect();
    owner.values().filter(|p| !known.contains(p.as_str())).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(p: &str) -> Entry {
        Entry { path: p.into(), kind: Kind::File, size: 1, mode: 0, target: String::new() }
    }
    fn pkg(n: &str, l: &str) -> Pkg {
        Pkg { name: n.into(), version: "1".into(), license: l.into(), source: String::new(), sha256: String::new(), origin: "buildroot".into() }
    }

    #[test]
    fn a_fake_image_with_a_blob_fails_and_a_clean_one_passes() {
        let (rules, probs) = parse_rules(
            "own | usr/bin/iyakki* | IYAKKI's own programs, Apache-2.0\n\
             generated | lib/modules/*/modules.* | depmod indexes written at build time\n\
             deny | lib/firmware/brcm/*.apple,* | Apple Wi-Fi firmware is never shipped\n\
             deny | usr/lib/libavcodec* | patent-encumbered codec libraries are not shipped\n\
             deny | font:*arial* | restricted font\n\
             deny | *libgstlibav* | the codec bridge is not active in a default image\n\
             generated | usr/lib/iyakki-av/* | gst-libav moved here by the image build, LGPL, inactive\n\
             except | usr/lib/iyakki-av/* | the inactive bridge by design, owner decision 2026-10-23 | 2026-10-26\n\
             accept | skeleton-init-common | Buildroot's own skeleton, GPL-2.0, reviewed | 2026-10-26\n",
        );
        assert!(probs.is_empty(), "{probs:?}");
        let mut owner = BTreeMap::new();
        parse_file_list("busybox,./bin/busybox\nlinux-firmware,./lib/firmware/brcm/brcmfmac43430.bin\nlinux-firmware,./lib/firmware/brcm/brcmfmac4364b2-pcie.apple,j140k.txt\nmystery,./usr/bin/mystery\nskeleton-init-common,./etc/passwd\nffmpeg,./usr/lib/libavcodec.so.60\n", &mut owner);
        let manifest = vec![pkg("busybox", "GPL-2.0, bzip2-1.0.4"), pkg("linux-firmware", "Proprietary"), pkg("ffmpeg", "LGPL-2.1+")];
        let entries: Vec<Entry> = ["bin/busybox", "usr/bin/iyakki", "lib/firmware/brcm/brcmfmac43430.bin", "lib/firmware/brcm/brcmfmac4364b2-pcie.apple,j140k.txt", "usr/bin/mystery", "etc/passwd", "usr/lib/libavcodec.so.60", "usr/share/fonts/Arial.ttf", "opt/blob/driver.bin", "usr/share/sounds/chime.wav", "lib/modules/6.12/modules.dep"].iter().map(|p| e(p)).collect();
        let r = scan(&Inputs { entries: &entries, owner: &owner, manifest: &manifest, rules: &rules, assets: &[], recipes: &BTreeMap::new(), rootfs: false });
        let has = |w: Why, k: &str| r.findings.iter().any(|f| f.why == w && f.key.contains(k));
        assert!(has(Why::Denied, "brcm") && has(Why::Denied, "libavcodec") && has(Why::Denied, "arial"), "{}", r.render("t", true));
        assert!(has(Why::ProprietaryPackage, "linux-firmware") && has(Why::NoLicenceInfo, "mystery") && has(Why::Unattributed, "opt/blob") && has(Why::UnlistedMedia, "audio"));
        assert!(!r.findings.iter().any(|f| f.key.contains("busybox") || f.key.contains("skeleton")) && r.waived.iter().any(|w| w.0.contains("skeleton")));
        assert!(!r.ok());
        // a clean image
        let clean = vec![e("bin/busybox"), e("usr/bin/iyakki"), e("lib/modules/6.12/modules.dep"), e("usr/lib/iyakki-av/gstreamer-1.0/libgstlibav.so")];
        let r = scan(&Inputs { entries: &clean, owner: &owner, manifest: &manifest, rules: &rules, assets: &[], recipes: &BTreeMap::new(), rootfs: false });
        assert!(r.ok(), "{}", r.render("t", true));
        assert_eq!((r.own, r.generated), (1, 2));
    }

    #[test]
    fn a_link_belongs_to_the_package_that_owns_its_target() {
        let mut owner = BTreeMap::new();
        owner.insert("usr/lib/libz.so.1.3".to_string(), "libzlib".to_string());
        owner.insert("usr/share/ca-certificates/mozilla/X.crt".to_string(), "ca-certificates".to_string());
        let link = |p: &str, t: &str| Entry { path: p.into(), kind: Kind::Symlink, size: t.len() as u64, mode: 0, target: t.into() };
        let entries = vec![link("usr/lib/libz.so.1", "libz.so.1.3"), link("usr/lib/libz.so", "libz.so.1"), link("etc/ssl/certs/abc.0", "/usr/share/ca-certificates/mozilla/X.crt"), link("usr/lib/odd.so", "../elsewhere/none.so")];
        let manifest = vec![pkg("libzlib", "Zlib"), pkg("ca-certificates", "GPL-2.0+ (script), MPL-2.0 (data)")];
        let (rules, _) = parse_rules("");
        let r = scan(&Inputs { entries: &entries, owner: &owner, manifest: &manifest, rules: &rules, assets: &[], recipes: &BTreeMap::new(), rootfs: false });
        assert_eq!(r.findings.len(), 1, "{}", r.render("t", true));
        assert_eq!(r.findings[0].samples, vec!["usr/lib/odd.so".to_string()]);
    }

    #[test]
    fn required_notice_files_are_checked_in_a_root_file_system() {
        let (rules, p) = parse_rules("require | usr/share/doc/iyakki/notice | the NOTICE file must travel with the image\n");
        assert!(p.is_empty());
        let none: BTreeMap<String, String> = BTreeMap::new();
        let mk = |entries: &[Entry], rootfs| scan(&Inputs { entries, owner: &none, manifest: &[], rules: &rules, assets: &[], recipes: &BTreeMap::new(), rootfs });
        assert!(mk(&[], false).ok());
        assert!(mk(&[], true).findings.iter().any(|f| f.why == Why::MissingNotice));
    }

    #[test]
    fn listed_assets_pass_and_unlisted_ones_do_not() {
        let (rules, _) = parse_rules("own | usr/share/iyakki/* | IYAKKI's own data folder\n");
        let assets = vec![AssetRow { path: "crates/x/logo.png".into(), license: "Apache-2.0".into(), origin: "made for the project".into(), status: "verified".into(), date: "2026-10-26".into(), image_paths: vec!["usr/share/iyakki/logo.png".into()] }];
        let entries = vec![e("usr/share/iyakki/logo.png"), e("usr/share/iyakki/stock-photo.jpg")];
        let r = scan(&Inputs { entries: &entries, owner: &BTreeMap::new(), manifest: &[], rules: &rules, assets: &assets, recipes: &BTreeMap::new(), rootfs: false });
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].samples, vec!["usr/share/iyakki/stock-photo.jpg".to_string()]);
    }

    #[test]
    fn rules_without_reason_or_date_are_refused() {
        let (_, p) = parse_rules("deny | x | no\nexcept | y | a long enough reason here | someday\nwhat | z | whatever reason text\n");
        assert_eq!(p.len(), 3);
    }
}
