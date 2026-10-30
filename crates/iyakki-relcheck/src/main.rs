use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iyakki_relcheck::{assets, cpio, features, gate, imagescan, links, profile, sbom, secrets, version};

const USAGE: &str = "iyakki-relcheck <links|secrets|version|features|sbom|licenses> [ROOT] [options]
  links               broken relative links and anchors in README, top-level and docs/ markdown
  secrets [--all]     private keys, tokens, .env/ssh key files; exit 1 on a BLOCK finding; --all also lists REVIEW findings
  version             version consistency (workspace, GUI workspace, lock files, CHANGELOG, release notes)
  features            FEATURES.md totals line against its rows
  sbom [--out FILE]   SPDX-lite SBOM from both Cargo.lock files (+ Buildroot legal-info when a build left one)
  licenses            markdown table of the components that are not purely permissive
  licence-check [ROOT] [--strict] [--write] [--verbose] [--no-images] [--image DIR]...
                      the whole licence and proprietary-software gate (release gate point 7): Cargo licences against the allow list and
                      legal/cargo-exceptions.txt, the asset inventory, the Quick install / catalogue inventory, notices and SBOM freshness,
                      Buildroot legal-info completeness, and the content scan of target/buildroot-*/images. Exit 1 on FAIL (--strict: also REVIEW).
                      --write refreshes legal/THIRD-PARTY-NOTICES.md and legal/sbom.cdx.json
  image-scan [ROOT] (--image DIR | --cpio FILE|- | --tree DIR | --names FILE) [--base BUILD_DIR]... [--verbose]
                      scan one image: every file mapped to its Buildroot package and licence, vendor blobs and unexplained files reported
  release-image-check [ROOT] [--config FILE]... [--simulate-cpio FILE|-] [--base BUILD_DIR]... [--verbose]
                      the RELEASE image profile without a build (docs/development/RELEASE-IMAGE-PROFILE.md): legal/release-profile.txt against
                      board/iyakki/release-firmware.keep and the two release fragments; each --config (a Buildroot .config) must switch nothing
                      forbidden on; --simulate-cpio applies the profile to an existing rootfs.cpio and runs the licence scan on what is left
";

fn registry_dirs() -> Vec<PathBuf> {
    let home = std::env::var("CARGO_HOME").map(PathBuf::from).or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cargo"))).unwrap_or_default();
    std::fs::read_dir(home.join("registry/src")).into_iter().flatten().flatten().map(|e| e.path()).collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let root = args.iter().enumerate().skip(1).find(|(i, a)| !a.starts_with("--") && Path::new(a).is_dir() && !matches!(args[i - 1].as_str(), "--image" | "--base" | "--cpio" | "--tree" | "--names" | "--out" | "--date" | "--config" | "--simulate-cpio")).map(|(_, a)| PathBuf::from(a)).unwrap_or_else(|| PathBuf::from("."));
    let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
    match cmd {
        "links" => {
            let p = links::check_repo(&root);
            for x in &p {
                println!("{}:{}: ({}) {}", x.file.display(), x.line, x.target, x.why);
            }
            println!("links: {} problem(s)", p.len());
            if p.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "secrets" => {
            let f = secrets::scan_tree(&root);
            let blocks: Vec<_> = f.iter().filter(|x| x.level == secrets::Level::Block).collect();
            let reviews: Vec<_> = f.iter().filter(|x| x.level == secrets::Level::Review).collect();
            for x in &blocks {
                println!("BLOCK   {}:{}: {}", x.file.display(), x.line, x.what);
            }
            if args.iter().any(|a| a == "--all") {
                for x in &reviews {
                    println!("REVIEW  {}:{}: {}", x.file.display(), x.line, x.what);
                }
            }
            println!("secrets: {} blocking, {} to review (run with --all to list them)", blocks.len(), reviews.len());
            if blocks.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "version" => {
            let bad = version::check(&root);
            bad.iter().for_each(|b| println!("{b}"));
            println!("version: {} problem(s)", bad.len());
            if bad.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "features" => {
            let text = std::fs::read_to_string(root.join("FEATURES.md")).unwrap_or_default();
            let bad = features::check(&text);
            bad.iter().for_each(|b| println!("{b}"));
            println!("features: {} problem(s); {}", bad.len(), features::summary(&text));
            if bad.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "licence-check" | "license-check" => {
            let images = args.windows(2).filter(|w| w[0] == "--image").map(|w| PathBuf::from(&w[1])).collect();
            let o = gate::Opts { root, strict: args.iter().any(|a| a == "--strict"), write: args.iter().any(|a| a == "--write"), verbose: args.iter().any(|a| a == "--verbose"), images, registry: registry_dirs(), skip_images: args.iter().any(|a| a == "--no-images") };
            let r = gate::run(&o);
            print!("{}", r.text);
            if r.failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
        }
        "image-scan" => {
            let verbose = args.iter().any(|a| a == "--verbose");
            let bases: Vec<PathBuf> = args.windows(2).filter(|w| w[0] == "--base").map(|w| PathBuf::from(&w[1])).collect();
            if let Some(dir) = flag("--image") {
                let rs = gate::scan_image_dir(&root, Path::new(&dir), verbose);
                rs.iter().for_each(|r| print!("{}", r.text));
                return if rs.is_empty() || rs.iter().any(|r| r.failed) { ExitCode::FAILURE } else { ExitCode::SUCCESS };
            }
            let (entries, rootfs) = if let Some(c) = flag("--cpio") {
                let r = if c == "-" { cpio::list(std::io::BufReader::new(std::io::stdin().lock())) } else { std::fs::File::open(&c).and_then(|f| cpio::list(std::io::BufReader::new(f))) };
                (r, true)
            } else if let Some(t) = flag("--tree") {
                (cpio::walk(Path::new(&t)), false)
            } else if let Some(n) = flag("--names") {
                (std::fs::read_to_string(&n).map(|t| cpio::from_names(&t)), true)
            } else {
                eprint!("{USAGE}");
                return ExitCode::from(2);
            };
            let entries = match entries {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("image-scan: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let bases = if bases.is_empty() { gate::base_dirs(&root, &root.join("target/none/images")) } else { bases };
            let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap_or_default();
            let (rules, rp) = imagescan::parse_rules(&read(gate::IMAGE_RULES));
            let (arows, _) = assets::parse(&read(gate::ASSETS));
            let (own, man) = (gate::owners(&bases), gate::manifest(&bases));
            let rep = imagescan::scan(&imagescan::Inputs { entries: &entries, owner: &own, manifest: &man, rules: &rules, assets: &arows, recipes: &gate::recipes(&root), rootfs });
            print!("{}", rep.render("input", verbose));
            rp.iter().for_each(|p| println!("{p}"));
            if rep.ok() && rp.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "release-image-check" => {
            let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap_or_default();
            let (prof, mut problems) = profile::parse_profile(&read(profile::PROFILE_RULES));
            if prof.forbid_config.is_empty() {
                problems.push(format!("{} is missing or has no forbid-config rows", profile::PROFILE_RULES));
            }
            let keep_text = read(profile::KEEP_FILE);
            if keep_text.is_empty() {
                problems.push(format!("{} is missing", profile::KEEP_FILE));
            }
            problems.extend(profile::keep_problems(&keep_text, &prof));
            let frags: Vec<(&str, String)> = profile::FRAGMENTS.iter().map(|f| (*f, read(f))).collect();
            for (f, t) in &frags {
                if t.is_empty() {
                    problems.push(format!("{f} is missing"));
                }
            }
            problems.extend(profile::fragment_problems(&frags, &prof));
            let mut checked = vec![format!("{} rows in {}", prof.forbid_config.len() + prof.allow_gst_bad.len() + prof.allow_fw.len(), profile::PROFILE_RULES), "keep list".to_string(), "fragments".to_string()];
            for c in args.windows(2).filter(|w| w[0] == "--config").map(|w| w[1].clone()) {
                match std::fs::read_to_string(&c) {
                    Ok(t) => {
                        problems.extend(profile::config_problems(&t, &prof).into_iter().map(|p| format!("{c}: {p}")));
                        checked.push(format!(".config {c}"));
                    }
                    Err(e) => problems.push(format!("{c}: {e}")),
                }
            }
            if let Some(c) = flag("--simulate-cpio") {
                let r = if c == "-" { cpio::list(std::io::BufReader::new(std::io::stdin().lock())) } else { std::fs::File::open(&c).and_then(|f| cpio::list(std::io::BufReader::new(f))) };
                match r {
                    Err(e) => problems.push(format!("{c}: {e}")),
                    Ok(entries) => {
                        let left = profile::simulate(&entries, &keep_text);
                        println!("simulation: {} entries in the image, {} left after the release profile", entries.len(), left.len());
                        let bases: Vec<PathBuf> = args.windows(2).filter(|w| w[0] == "--base").map(|w| PathBuf::from(&w[1])).collect();
                        let bases = if bases.is_empty() { gate::base_dirs(&root, &root.join("target/none/images")) } else { bases };
                        let (rules, rp) = imagescan::parse_rules(&read(gate::IMAGE_RULES));
                        let (arows, _) = assets::parse(&read(gate::ASSETS));
                        let (own, man) = (gate::owners(&bases), gate::manifest(&bases));
                        let rep = imagescan::scan(&imagescan::Inputs { entries: &left, owner: &own, manifest: &man, rules: &rules, assets: &arows, recipes: &gate::recipes(&root), rootfs: false });
                        print!("{}", rep.render("simulated release image", args.iter().any(|a| a == "--verbose")));
                        problems.extend(rp);
                        problems.extend(profile::image_findings(&left, &prof));
                        if !rep.ok() {
                            problems.push(format!("the licence scan of the simulated release image has {} finding group(s)", rep.findings.len()));
                        }
                        checked.push("simulation".into());
                    }
                }
            }
            problems.iter().for_each(|p| println!("FAIL {p}"));
            println!("release-image-check: {} ({} problem(s))", checked.join(", "), problems.len());
            if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "sbom" | "licenses" => {
            let mut pkgs = sbom::collect(&root, &registry_dirs());
            pkgs.sort_by(|a, b| (a.origin.as_str(), a.name.as_str(), a.version.as_str()).cmp(&(b.origin.as_str(), b.name.as_str(), b.version.as_str())));
            if cmd == "sbom" {
                let ws = version::manifest_version(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default()).unwrap_or_default();
                let date = flag("--date").unwrap_or_else(|| "unknown".into());
                let json = sbom::to_json("IYAKKI", &ws, &date, &pkgs);
                match flag("--out") {
                    Some(o) => {
                        if let Err(e) = std::fs::write(&o, json) {
                            eprintln!("{o}: {e}");
                            return ExitCode::FAILURE;
                        }
                        let unknown = pkgs.iter().filter(|p| p.license.is_empty()).count();
                        println!("sbom: {} packages written to {o} ({unknown} without a stated licence)", pkgs.len());
                    }
                    None => print!("{json}"),
                }
            } else {
                let mut seen = std::collections::BTreeSet::new();
                println!("| Component | Version | Licence | Origin | Use |\n|---|---|---|---|---|");
                for p in pkgs.iter().filter(|p| sbom::non_permissive(&p.license)) {
                    if seen.insert((p.name.clone(), p.version.clone(), p.origin.starts_with("buildroot"))) {
                        let how = if p.license.to_uppercase().contains(" AND ") { "includes a non-permissive part (AND)" } else if sbom::purely_copyleft(&p.license) { "copyleft only" } else { "permissive alternative exists" };
                        println!("| {} | {} | {} | {} | {how} |", p.name, p.version, p.license.replace('|', "/"), p.origin);
                    }
                }
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
