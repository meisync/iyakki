use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iyakki_relcheck::{features, links, sbom, secrets, version};

const USAGE: &str = "iyakki-relcheck <links|secrets|version|features|sbom|licenses> [ROOT] [options]
  links               broken relative links and anchors in README, top-level and docs/ markdown
  secrets [--all]     private keys, tokens, .env/ssh key files; exit 1 on a BLOCK finding; --all also lists REVIEW findings
  version             version consistency (workspace, GUI workspace, lock files, CHANGELOG, release notes)
  features            FEATURES.md totals line against its rows
  sbom [--out FILE]   SPDX-lite SBOM from both Cargo.lock files (+ Buildroot legal-info when a build left one)
  licenses            markdown table of the components that are not purely permissive
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
    let root = args.iter().skip(1).find(|a| !a.starts_with("--") && Path::new(a).is_dir()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
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
