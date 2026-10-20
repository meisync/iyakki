//! The About page lists the programs of the system images with their licences. They come from the repository's `SBOM.json`
//! (written by `iyakki-relcheck` from Buildroot's legal-info); only the Buildroot packages are kept, one tab-separated line each
//! (name, version, licence, image), so the program does not carry the whole SBOM. No dependencies: the SBOM has one package per line.

use std::path::PathBuf;

fn field(line: &str, key: &str) -> String {
    let pat = format!("\"{key}\": \"");
    let Some(i) = line.find(&pat) else { return String::new() };
    let rest = &line[i + pat.len()..];
    let mut out = String::new();
    let mut esc = false;
    for c in rest.chars() {
        match (esc, c) {
            (true, c) => {
                out.push(c);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, '"') => break,
            (false, c) => out.push(c),
        }
    }
    out.replace(['\t', '\n'], " ")
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let sbom = root.join("SBOM.json");
    println!("cargo:rerun-if-changed={}", sbom.display());
    println!("cargo:rerun-if-changed=build.rs");
    let text = std::fs::read_to_string(&sbom).unwrap_or_default();
    let mut rows: Vec<String> = Vec::new();
    for line in text.lines() {
        let supplier = field(line, "supplier");
        let Some(image) = supplier.strip_prefix("buildroot:") else { continue };
        let row = format!("{}\t{}\t{}\t{}", field(line, "name"), field(line, "versionInfo"), field(line, "licenseConcluded"), image);
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("image_components.tsv");
    std::fs::write(out, rows.join("\n")).unwrap();
}
