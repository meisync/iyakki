//! The inventory of shipped artwork and data: every font, icon, image, sound, video and model file in the repository must be covered
//! by a row of `legal/assets.txt`. Row: `path glob | SPDX licence | origin | status | date | image paths (optional, comma separated)`.
//! `status` is `verified` (the licence was checked against the source and is stated in the origin) or `unverified` (the file is there,
//! the owner still has to confirm where it came from): unverified rows are listed as REVIEW and fail the gate under `--strict`.

use std::path::Path;

use crate::imagescan::media_class;
use crate::spdx;
use crate::table::{self, glob};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    pub path: String,
    pub license: String,
    pub origin: String,
    pub status: String,
    pub date: String,
    pub image_paths: Vec<String>,
}

/// Licences an asset may carry once verified (permissive code licences plus the font and content licences in use).
pub const ASSET_ALLOW: &[&str] = &["OFL-1.1", "Ubuntu-font-1.0", "Bitstream-Vera", "LicenseRef-DejaVu", "CC-BY-4.0", "CC-BY-SA-4.0", "LicenseRef-Project-Original"];

pub fn parse(text: &str) -> (Vec<AssetRow>, Vec<String>) {
    let (mut v, mut problems) = (Vec::new(), Vec::new());
    for r in table::parse(text) {
        if r.f.len() < 5 || r.f.len() > 6 {
            problems.push(format!("assets line {}: expected `path | licence | origin | status | date [| image paths]`", r.line));
            continue;
        }
        if !table::valid_reason(&r.f[2]) || !table::valid_date(&r.f[4]) || !matches!(r.f[3].as_str(), "verified" | "unverified") {
            problems.push(format!("assets line {} ({}): origin too short, bad date, or status is not verified/unverified", r.line, r.f[0]));
            continue;
        }
        let image_paths = r.f.get(5).map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
        v.push(AssetRow { path: r.f[0].clone(), license: r.f[1].clone(), origin: r.f[2].clone(), status: r.f[3].clone(), date: r.f[4].clone(), image_paths });
    }
    (v, problems)
}

#[derive(Debug, Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub review: Vec<String>,
    pub files: usize,
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !matches!(name.as_str(), "target" | ".git" | "node_modules") && !name.starts_with('.') {
                walk(base, &e.path(), out);
            }
        } else if ft.is_file() {
            if let Ok(rel) = e.path().strip_prefix(base) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
}

/// All media files of a repository tree (relative paths).
pub fn media_files(root: &Path) -> Vec<String> {
    let mut all = Vec::new();
    walk(root, root, &mut all);
    all.retain(|p| media_class(p).is_some());
    all.sort();
    all
}

pub fn check(rows: &[AssetRow], files: &[String], parse_problems: Vec<String>) -> Report {
    let mut r = Report { failures: parse_problems, files: files.len(), ..Default::default() };
    for f in files {
        if !rows.iter().any(|a| glob(&a.path, f)) {
            r.failures.push(format!("{f}: {} file with no entry in legal/assets.txt", media_class(f).unwrap_or("media")));
        }
    }
    for a in rows {
        if !files.iter().any(|f| glob(&a.path, f)) {
            r.review.push(format!("assets row `{}` matches no file any more", a.path));
        }
        let allow: Vec<&str> = spdx::CARGO_ALLOW.iter().chain(ASSET_ALLOW.iter()).copied().collect();
        let lic_ok = spdx::parse(&a.license).ok().is_some_and(|e| spdx::satisfied(&e, &allow).is_ok());
        match (a.status.as_str(), lic_ok) {
            ("verified", false) => r.failures.push(format!("`{}`: verified, but licence `{}` is not an allowed one", a.path, a.license)),
            ("unverified", _) => r.review.push(format!("`{}`: origin/licence NOT verified ({}; claimed `{}`)", a.path, a.origin, a.license)),
            _ => {}
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_media_file_needs_a_row_and_unverified_rows_are_listed() {
        let (rows, probs) = parse("assets/fonts/*.ttf | OFL-1.1 | Noto Sans, SIL Open Font License text shipped next to it | verified | 2026-10-26 | usr/share/iyakki/fonts/*\nassets/logo.png | Apache-2.0 | supplied by the owner, provenance still to be confirmed | unverified | 2026-10-26\nbroken | only | three\n");
        assert_eq!((rows.len(), probs.len()), (2, 1));
        assert_eq!(rows[0].image_paths, vec!["usr/share/iyakki/fonts/*".to_string()]);
        let files = vec!["assets/fonts/a.ttf".to_string(), "assets/logo.png".into(), "assets/stolen.mp3".into()];
        let r = check(&rows, &files, vec![]);
        assert_eq!(r.failures.len(), 1);
        assert!(r.failures[0].contains("stolen.mp3") && r.failures[0].contains("audio"));
        assert_eq!(r.review.len(), 1);
        assert!(r.review[0].contains("NOT verified"));
        let (rows, _) = parse("x/* | Proprietary | bought from a stock site, licence says no redistribution | verified | 2026-10-26\n");
        assert_eq!(check(&rows, &["x/a.png".to_string()], vec![]).failures.len(), 1);
    }

    #[test]
    fn the_walker_finds_media_and_skips_build_output() {
        let d = std::env::temp_dir().join(format!("iyakki-relcheck-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["a/x.png", "target/y.png", "a/b/z.TTF", "a/readme.md", "node_modules/q.png"] {
            std::fs::create_dir_all(d.join(p).parent().unwrap()).unwrap();
            std::fs::write(d.join(p), b"x").unwrap();
        }
        assert_eq!(media_files(&d), vec!["a/b/z.TTF".to_string(), "a/x.png".into()]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
