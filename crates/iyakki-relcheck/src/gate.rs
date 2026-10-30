//! `iyakki-relcheck licence-check`: the whole licence and proprietary-software gate in one run (release gate point 7,
//! `docs/project/BACKLOG.md`). Reads `legal/*.txt` (reviewed, dated decisions), the Cargo lock files, the Buildroot build folders under
//! `target/` when they exist, and prints a PASS / FAIL / REVIEW table. Exit code 1 on any FAIL; `--strict` also fails on REVIEW.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{assets, cargolic, cpio, downloads, imagescan, legalinfo, notices, sbom};

pub const CARGO_EXCEPTIONS: &str = "legal/cargo-exceptions.txt";
pub const IMAGE_RULES: &str = "legal/image-rules.txt";
pub const ASSETS: &str = "legal/assets.txt";
pub const DOWNLOADS: &str = "legal/downloads.txt";
pub const NOTICES: &str = "legal/THIRD-PARTY-NOTICES.md";
pub const CYCLONEDX: &str = "legal/sbom.cdx.json";

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_default()
}

/// The Buildroot build folders whose file lists and manifests describe an image: its own, then the Mac and web builds the layered
/// images are repacked from (`dist/buildroot/repack.sh`).
pub fn base_dirs(root: &Path, image_dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = image_dir.parent() {
        v.push(p.to_path_buf());
    }
    for n in ["buildroot-mac", "buildroot-web"] {
        let d = root.join("target").join(n);
        if d.is_dir() && !v.contains(&d) {
            v.push(d);
        }
    }
    v
}

/// path -> package from every base, first wins.
pub fn owners(bases: &[PathBuf]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for b in bases {
        if let Ok(t) = std::fs::read_to_string(b.join("build/packages-file-list.txt")) {
            imagescan::parse_file_list(&t, &mut m);
        }
    }
    m
}

pub fn manifest(bases: &[PathBuf]) -> Vec<sbom::Pkg> {
    let mut seen = std::collections::BTreeSet::new();
    let mut v = Vec::new();
    for b in bases {
        if let Ok(t) = std::fs::read_to_string(b.join("legal-info/manifest.csv")) {
            for p in sbom::parse_buildroot_manifest(&t) {
                if seen.insert(p.name.clone()) {
                    v.push(sbom::Pkg { origin: format!("buildroot:{}", b.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), ..p });
                }
            }
        }
    }
    v
}

/// Licences declared by the package recipes of Buildroot and of `dist/buildroot/package`.
pub fn recipes(root: &Path) -> BTreeMap<String, String> {
    let mut rdirs = vec![root.join("dist/buildroot/package")];
    if let Ok(rd) = std::fs::read_dir(root.join("target/buildroot-src")) {
        rdirs.extend(rd.flatten().map(|e| e.path().join("package")));
    }
    imagescan::recipe_licences(&rdirs)
}

pub struct ImageResult {
    pub name: String,
    pub text: String,
    pub failed: bool,
    /// Packages seen in the image, for the SBOM.
    pub pkgs: Vec<sbom::Pkg>,
}

/// Scan `<dir>/rootfs.cpio` and `<dir>/install-root` of one `images` folder.
pub fn scan_image_dir(root: &Path, dir: &Path, verbose: bool) -> Vec<ImageResult> {
    let (rules, rp) = imagescan::parse_rules(&read(root, IMAGE_RULES));
    let (arows, _) = assets::parse(&read(root, ASSETS));
    let bases = base_dirs(root, dir);
    let (own, man) = (owners(&bases), manifest(&bases));
    let recipes = recipes(root);
    let label = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());
    let mut out = Vec::new();
    let mut run = |name: String, entries: Result<Vec<cpio::Entry>, String>, rootfs: bool| match entries {
        Err(e) => out.push(ImageResult { name: name.clone(), text: format!("== image scan: {name}: cannot read: {e}\n"), failed: true, pkgs: vec![] }),
        Ok(entries) => {
            let r = imagescan::scan(&imagescan::Inputs { entries: &entries, owner: &own, manifest: &man, rules: &rules, assets: &arows, recipes: &recipes, rootfs });
            let mut text = r.render(&name, verbose);
            let mut failed = !r.ok();
            // an image folder called `*-release` is built with IYAKKI_IMAGE_PROFILE=release: it may hold no vendor firmware at all
            if label.ends_with("-release") {
                let (prof, _) = crate::profile::parse_profile(&read(root, crate::profile::PROFILE_RULES));
                for f in crate::profile::image_findings(&entries, &prof) {
                    text.push_str(&format!("-- RELEASE PROFILE: {f}\n"));
                    failed = true;
                }
            }
            for p in &rp {
                text.push_str(&format!("{p}\n"));
                failed = true;
            }
            let pkgs = r.packages.keys().filter_map(|k| man.iter().find(|m| &m.name == k).cloned()).collect();
            out.push(ImageResult { name, text, failed, pkgs });
        }
    };
    let cpio_path = dir.join("rootfs.cpio");
    if cpio_path.is_file() {
        let e = std::fs::File::open(&cpio_path).and_then(|f| cpio::list(std::io::BufReader::with_capacity(1 << 20, f))).map_err(|e| e.to_string());
        run(format!("{label}/rootfs.cpio"), e, true);
    }
    let tree = dir.join("install-root");
    if tree.is_dir() {
        run(format!("{label}/install-root"), cpio::walk(&tree).map_err(|e| e.to_string()), false);
    }
    out
}

pub struct Opts {
    pub root: PathBuf,
    pub strict: bool,
    pub write: bool,
    pub verbose: bool,
    /// `images` folders; empty = every `target/buildroot-*/images` that has a rootfs.cpio
    pub images: Vec<PathBuf>,
    pub registry: Vec<PathBuf>,
    pub skip_images: bool,
}

pub struct Outcome {
    pub text: String,
    pub failed: bool,
}

struct Table(Vec<(String, &'static str, String)>);
impl Table {
    fn add(&mut self, name: &str, status: &'static str, note: String) {
        self.0.push((name.to_string(), status, note));
    }
}

fn plural(n: usize, w: &str) -> String {
    format!("{n} {w}{}", if n == 1 { "" } else { "s" })
}

pub fn run(o: &Opts) -> Outcome {
    let mut t = Table(vec![]);
    let mut detail = String::new();
    let root = &o.root;

    // 1. Cargo
    let pkgs = cargolic::load(root, &o.registry);
    let (ex, ep) = cargolic::parse_exceptions(&read(root, CARGO_EXCEPTIONS));
    let active = cargolic::active_set(root);
    let c = cargolic::check_active(&pkgs, &ex, ep, active.as_ref());
    for f in &c.failures {
        detail.push_str(&format!("FAIL cargo: {f}\n"));
    }
    for f in &c.excepted {
        detail.push_str(&format!("excepted cargo: {f}\n"));
    }
    for f in &c.inactive {
        detail.push_str(&format!("REVIEW cargo: {f}\n"));
    }
    for f in &c.stale {
        detail.push_str(&format!("REVIEW cargo: stale {f}\n"));
    }
    t.add("1 cargo dependency licences", if !c.failures.is_empty() { "FAIL" } else if !c.stale.is_empty() || !c.inactive.is_empty() { "REVIEW" } else { "PASS" }, format!("{} checked ({}), {} reviewed exceptions in use, {}, {} lock-only to review", c.checked, if active.is_some() { "build graph from cargo tree" } else { "cargo tree unavailable: every locked package counted" }, c.excepted.len(), plural(c.failures.len(), "failure"), c.inactive.len()));
    if pkgs.is_empty() {
        t.add("1b cargo lock files found", "FAIL", "no Cargo.lock packages read (wrong ROOT, or the cargo registry is not downloaded: run `cargo fetch`)".into());
    }

    // 2. assets
    let (arows, ap) = assets::parse(&read(root, ASSETS));
    let files = assets::media_files(root);
    let a = assets::check(&arows, &files, ap);
    a.failures.iter().for_each(|f| detail.push_str(&format!("FAIL assets: {f}\n")));
    a.review.iter().for_each(|f| detail.push_str(&format!("REVIEW assets: {f}\n")));
    t.add("4 assets inventory (fonts, icons, sounds, models)", if !a.failures.is_empty() { "FAIL" } else if !a.review.is_empty() { "REVIEW" } else { "PASS" }, format!("{} media files, {} rows, {}, {} to review", a.files, arows.len(), plural(a.failures.len(), "failure"), a.review.len()));

    // 3. downloads
    let (drows, dp) = downloads::parse(&read(root, DOWNLOADS));
    let d = downloads::check(&drows, dp);
    d.failures.iter().for_each(|f| detail.push_str(&format!("FAIL downloads: {f}\n")));
    d.review.iter().for_each(|f| detail.push_str(&format!("REVIEW downloads: {f}\n")));
    t.add("3 Quick install / catalogue inventory (legal/downloads.txt)", if !d.failures.is_empty() { "FAIL" } else if !d.review.is_empty() { "REVIEW" } else { "PASS" }, format!("{} rows, {}, {} to review; the cross-check with the code is `cargo test -p iyakki-agent --test licence_gate`", d.rows, plural(d.failures.len(), "failure"), d.review.len()));

    // 3b. known gaps in what the screens show (tied to the code by the agent test)
    let gaps = crate::table::parse(&read(root, "legal/catalogue-gaps.txt"));
    t.add("3b catalogue / Quick install screens that miss a licence or a 'not shipped' statement (legal/catalogue-gaps.txt)", if gaps.is_empty() { "PASS" } else { "REVIEW" }, format!("{} known gap(s), each dated; a new one fails `cargo test -p iyakki-agent --test licence_gate`", gaps.len()));

    // 5. generated documents
    let want_notices = notices::notices(&pkgs, &arows, &drows);
    let ws = crate::version::manifest_version(&read(root, "Cargo.toml")).unwrap_or_default();
    let want_cdx = notices::cyclonedx("IYAKKI", &ws, &pkgs, &arows);
    if o.write {
        let _ = std::fs::create_dir_all(root.join("legal"));
        for (rel, body) in [(NOTICES, &want_notices), (CYCLONEDX, &want_cdx)] {
            if let Err(e) = std::fs::write(root.join(rel), body) {
                detail.push_str(&format!("FAIL could not write {rel}: {e}\n"));
            }
        }
    }
    let stale: Vec<&str> = [(NOTICES, &want_notices), (CYCLONEDX, &want_cdx)].iter().filter(|(rel, body)| read(root, rel) != **body).map(|(rel, _)| *rel).collect();
    let src_offer = ["LICENSING.md", "docs/LICENSES.md"].iter().any(|f| read(root, f).to_lowercase().contains("corresponding source"));
    let notice_ok = !read(root, "NOTICE").is_empty() && !read(root, "THIRD_PARTY_LICENSES.txt").is_empty() && root.join("LICENSE").is_file();
    t.add("5 notices and SBOM up to date", if stale.is_empty() { "PASS" } else { "FAIL" }, if stale.is_empty() { format!("{NOTICES} and {CYCLONEDX} match a fresh run{}", if o.write { " (just written)" } else { "" }) } else { format!("out of date: {} (run `./run.sh licence-check --write`)", stale.join(", ")) });
    t.add("5b LICENSE / NOTICE / THIRD_PARTY_LICENSES.txt and a source offer in the docs", if src_offer && notice_ok { "PASS" } else { "FAIL" }, format!("source offer text in LICENSING.md or docs/LICENSES.md: {src_offer}; LICENSE, NOTICE, THIRD_PARTY_LICENSES.txt present: {notice_ok}"));

    // 6. buildroot legal-info per build, and images
    let mut build_dirs: Vec<PathBuf> = std::fs::read_dir(root.join("target")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("legal-info/manifest.csv").is_file()).collect();
    build_dirs.sort();
    if build_dirs.is_empty() {
        t.add("5c legal-info completeness (GPL source offer)", "SKIP", "no target/buildroot-*/legal-info found; build an image first. Nothing was checked.".into());
    }
    for b in &build_dirs {
        let r = legalinfo::check(&b.join("legal-info"), &BTreeMap::new());
        r.failures.iter().for_each(|f| detail.push_str(&format!("FAIL legal-info {}: {f}\n", b.file_name().unwrap_or_default().to_string_lossy())));
        t.add(&format!("5c legal-info {}", b.file_name().unwrap_or_default().to_string_lossy()), if r.failures.is_empty() { "PASS" } else { "FAIL" }, format!("{} listed, {} copyleft, {}", plural(r.packages, "package"), r.copyleft, plural(r.failures.len(), "failure")));
    }
    let mut imgs = o.images.clone();
    if imgs.is_empty() && !o.skip_images {
        let mut v: Vec<PathBuf> = std::fs::read_dir(root.join("target")).into_iter().flatten().flatten().map(|e| e.path().join("images")).filter(|p| p.join("rootfs.cpio").is_file()).collect();
        v.sort();
        imgs = v;
    }
    if imgs.is_empty() {
        t.add("2 image content scan", "SKIP", if o.skip_images { "--no-images".into() } else { "no target/buildroot-*/images/rootfs.cpio found. Nothing was checked.".into() });
    }
    let mut image_pkgs: Vec<sbom::Pkg> = Vec::new();
    for dir in &imgs {
        for r in scan_image_dir(root, dir, o.verbose) {
            detail.push_str(&r.text);
            t.add(&format!("2 image scan {}", r.name), if r.failed { "FAIL" } else { "PASS" }, if r.failed { "see the findings below".into() } else { "all files explained".into() });
            image_pkgs.extend(r.pkgs);
        }
    }
    if !image_pkgs.is_empty() && o.write {
        let _ = std::fs::write(root.join("legal/sbom-images.cdx.json"), notices::cyclonedx("IYAKKI images", &ws, &image_pkgs, &[]));
    }

    let mut text = String::from("IYAKKI licence and proprietary-software gate (release gate point 7)\n\n");
    let mut failed = false;
    for (name, st, note) in &t.0 {
        text.push_str(&format!("{st:<7} {name}: {note}\n"));
        failed |= *st == "FAIL" || (*st == "REVIEW" && o.strict);
    }
    text.push_str("\nNot covered by this gate: see LICENSING.md, section 'What the gate does not cover'.\n\n");
    text.push_str(&detail);
    text.push_str(&format!("\nlicence-check: {}\n", if failed { "FAIL" } else { "PASS" }));
    Outcome { text, failed }
}
