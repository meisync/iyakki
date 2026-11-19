//! The checks that run before something is shipped: the same ones the install review runs (manifest, size, permissions,
//! licence) plus a scan for secrets. Used by the assistant's `store_validate`, by `iyakki pkg validate` and by the private
//! store server (`server.rs`), so what passes here is what a store will accept. Nothing here touches the network.

use std::path::Path;

use serde_json::Value;

use super::manifest::{Meta, Permission};
use super::trust::look_alikes;
use crate::pkg;

/// Licences a store package may carry (the same permissive family the project itself allows).
pub const PERMISSIVE: [&str; 10] = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "CC0-1.0", "0BSD", "Unlicense", "BSL-1.0"];

pub fn permissive(spdx: &str) -> bool {
    PERMISSIVE.contains(&spdx.trim())
}

/// The outcome of a check: `ok` is false when there is at least one problem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
    /// What was found, one fact per line (kind, name, version, permissions in words ...).
    pub facts: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn text(&self) -> String {
        let mut o = self.facts.join("\n");
        for w in &self.warnings {
            o.push_str(&format!("\nwarning: {w}"));
        }
        for p in &self.problems {
            o.push_str(&format!("\nPROBLEM: {p}"));
        }
        o.push_str(if self.ok() { "\nresult: ok to package and publish" } else { "\nresult: NOT ok, fix the problems above" });
        o.trim_start().to_string()
    }
}

// ------------------------------------------------------------------------------------------------ secrets

fn run_len(s: &str, ok: impl Fn(char) -> bool) -> usize {
    s.chars().take_while(|c| ok(*c)).count()
}

/// Lines that look like they hold a credential. Returns `(line number, what)`; never the secret itself.
pub fn secrets_scan(text: &str) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        let lower = l.to_ascii_lowercase();
        let mut hit = None;
        if lower.contains("-----begin") && lower.contains("private key") {
            hit = Some("a private key");
        }
        let after = |p: &str| l.find(p).map(|i| &l[i + p.len()..]);
        if hit.is_none() {
            if after("AKIA").is_some_and(|r| run_len(r, |c| c.is_ascii_uppercase() || c.is_ascii_digit()) >= 16) {
                hit = Some("an AWS access key");
            } else if ["ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "xoxa-", "sk-ant-", "iyk_"].iter().any(|p| after(p).is_some_and(|r| run_len(r, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-') >= 16)) {
                hit = Some("an access token");
            } else if after("sk-").is_some_and(|r| run_len(r, |c| c.is_ascii_alphanumeric()) >= 32) {
                hit = Some("an API key");
            } else if let Some(r) = lower.find("bearer ").map(|i| &l[i + 7..]) {
                if run_len(r, |c| c.is_ascii_alphanumeric() || "._-~+/=".contains(c)) >= 24 {
                    hit = Some("a bearer token");
                }
            }
        }
        if hit.is_none() && ["password", "passwd", "secret", "api_key", "apikey", "token", "private_key"].iter().any(|w| lower.contains(w)) {
            // `name = "value"` or `name: value` with a long literal that is not a variable, placeholder or lookup
            if let Some((_, v)) = l.split_once('=').or_else(|| l.split_once(": ")) {
                let v = v.trim().trim_end_matches([',', ';']).trim_matches(['"', '\'']);
                let placeholder = v.is_empty() || v.starts_with(['$', '{', '<']) || v.contains("environ") || v.contains("getenv") || v.contains("env.") || v.contains("process.") || v.contains("..") || v.contains("(") || v.contains("example") || v.contains("changeme") || v.contains("your");
                let literal = v.len() >= 20 && v.chars().all(|c| c.is_ascii_alphanumeric() || "_-+/=.".contains(c)) && v.chars().any(|c| c.is_ascii_digit()) && v.chars().any(|c| c.is_ascii_alphabetic());
                if !placeholder && literal {
                    hit = Some("a hard-coded secret");
                }
            }
        }
        if let Some(h) = hit {
            out.push((n + 1, h));
        }
    }
    out
}

fn secret_problems(what: &str, text: &str) -> Vec<String> {
    secrets_scan(text).into_iter().map(|(n, w)| format!("{what} line {n} looks like it holds {w}: remove it and read it from the vault or the environment at run time")).collect()
}

// ------------------------------------------------------------------------------------------------ packages

fn permission_words(p: &pkg::Package) -> Vec<String> {
    match p.manifest.kind.as_str() {
        "tool" => crate::wtool::parse_payload(&p.payload).map(|t| t.caps.iter().map(|c| Permission::Cap(c.clone()).words("you")).collect()).unwrap_or_default(),
        "mcp" => serde_json::from_str::<Value>(&p.payload).ok().and_then(|v| v["command"].as_str().map(|c| vec![format!("run the program {c}")])).unwrap_or_default(),
        _ => Meta::parse(p.manifest.meta.as_deref().unwrap_or("{}")).map(|x| x.permissions.iter().filter_map(|p| Permission::parse(p).ok()).map(|p| p.words("you")).collect()).unwrap_or_default(),
    }
}

/// Check package text the way the install review does. `known` is `(name, publisher id)` of other packages, for the look-alike check.
pub fn vet_package(text: &str, known: &[(String, String)]) -> Report {
    let mut r = Report::default();
    let p = match pkg::parse(text) {
        Ok(p) => p,
        Err(e) => {
            r.problems.push(format!("not a valid package: {e}"));
            return r;
        }
    };
    let m = &p.manifest;
    r.facts.push(format!("{} {} {} (licence {}, {} bytes)", m.kind, m.name, m.version, m.license, text.len()));
    if m.signer.is_none() {
        r.warnings.push("the package is not signed: stores accept only signed packages (`iyakki pkg sign`)".into());
    } else {
        r.facts.push("hash ok, signature ok".into());
    }
    if !permissive(&m.license) {
        r.problems.push(format!("the licence {:?} is not in the permissive list ({})", m.license, PERMISSIVE.join(", ")));
    }
    if m.kind == "cli" || m.kind == "app" {
        r.problems.push(format!("a {} package cannot be published to a store server", m.kind));
    }
    let perms = permission_words(&p);
    r.facts.push(format!("permissions: {}", if perms.is_empty() { "none".to_string() } else { perms.join("; ") }));
    let mut publisher = String::new();
    match m.meta.as_deref().map(Meta::parse) {
        Some(Err(e)) => r.problems.push(e),
        Some(Ok(meta)) => {
            publisher = meta.publisher.id.clone();
            if meta.title.is_empty() || meta.summary.is_empty() {
                r.warnings.push("add a title and a summary to the store information so people know what it is".into());
            }
            for want in &meta.permissions {
                if let Err(e) = Permission::parse(want) {
                    r.problems.push(e);
                }
            }
            if matches!(m.kind.as_str(), "skill" | "template" | "theme" | "agent" | "extension") && !meta.permissions.is_empty() && !p.payload.is_empty() {
                r.warnings.push("the permissions in the store information are only a preview; installing shows what the package really asks for".into());
            }
        }
        None => r.warnings.push("no store information (title, summary, permissions): add --meta when you sign".into()),
    }
    r.problems.extend(secret_problems("the payload", &p.payload));
    let like = look_alikes(&m.name, &publisher, known);
    if !like.is_empty() {
        r.warnings.push(format!("the name {:?} looks like {}: pick a name that cannot be mistaken for it", m.name, like.join(", ")));
    }
    r
}

// ------------------------------------------------------------------------------------------------ project folders

/// The manifest every scaffolded project carries.
pub const PROJECT_FILE: &str = "iyakki-app.json";
const SKIP_DIRS: [&str; 7] = [".git", "node_modules", "target", "__pycache__", ".venv", "venv", "dist"];
const MAX_FILES: usize = 400;
const MAX_FILE: u64 = 512 * 1024;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let (p, name) = (e.path(), e.file_name().to_string_lossy().to_string());
        let Ok(t) = e.file_type() else { continue };
        if t.is_symlink() || out.len() > MAX_FILES {
            continue;
        }
        if t.is_dir() {
            if depth < 6 && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, out, depth + 1);
            }
        } else {
            out.push(p);
        }
    }
}

/// The project manifest of a folder, parsed (None when the folder has none).
pub fn read_project(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(PROJECT_FILE)).ok()?).ok()
}

/// Check a project folder (made by `app_scaffold`): its `iyakki-app.json`, the files its kind needs, permissions, licence, secrets.
pub fn vet_dir(dir: &Path) -> Report {
    let mut r = Report::default();
    let Ok(text) = std::fs::read_to_string(dir.join(PROJECT_FILE)) else {
        r.problems.push(format!("there is no {PROJECT_FILE} in this folder (app_scaffold makes one)"));
        return r;
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            r.problems.push(format!("{PROJECT_FILE} is not valid JSON: {e}"));
            return r;
        }
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (kind, name, version, license) = (s("kind"), s("name"), s("version"), s("license"));
    r.facts.push(format!("project {kind} {name} {version} (licence {license})"));
    if !pkg::KINDS.contains(&kind.as_str()) || matches!(kind.as_str(), "cli" | "app") {
        r.problems.push(format!("kind {kind:?}: one of skill, tool, mcp, agent, template, theme, extension, tile"));
    }
    if !crate::skills::valid_skill_name(&name) {
        r.problems.push("name: lowercase letters, digits and dashes, at most 48 characters".into());
    }
    if !pkg::valid_version(&version) {
        r.problems.push("version: digits separated by dots, like 0.1.0".into());
    }
    if !permissive(&license) {
        r.problems.push(format!("licence {license:?} is not in the permissive list ({})", PERMISSIVE.join(", ")));
    }
    if s("title").is_empty() || s("summary").is_empty() {
        r.warnings.push("fill in title and summary".into());
    }
    if s("summary").chars().count() > 140 || s("title").chars().count() > 60 {
        r.problems.push("title is at most 60 characters and summary at most 140".into());
    }
    let perms: Vec<String> = v.get("permissions").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    match perms.iter().map(|p| Permission::parse(p).map(|x| x.words("you"))).collect::<Result<Vec<_>, _>>() {
        Ok(w) => r.facts.push(format!("permissions: {}", if w.is_empty() { "none".to_string() } else { w.join("; ") })),
        Err(e) => r.problems.push(e),
    }
    if perms.iter().any(|p| p == "net:*" || p == "run:*" || p == "write:/" || p == "read:/") {
        r.problems.push("a permission this wide is never accepted: name the host, the program or the folder".into());
    }
    let entry = s("entry");
    if entry.is_empty() || entry.contains("..") || entry.starts_with('/') || !dir.join(&entry).is_file() {
        r.problems.push(format!("entry {entry:?} must be a file inside the project"));
    } else if kind == "skill" {
        match crate::skills::parse(&std::fs::read_to_string(dir.join(&entry)).unwrap_or_default(), crate::skills::Scope::Shared) {
            Ok(sk) => {
                if sk.name != name {
                    r.problems.push(format!("the skill's name {:?} differs from the project name {name:?}", sk.name));
                }
                if sk.body.chars().count() > 3950 {
                    r.problems.push("a skill body is at most 3950 characters".into());
                }
            }
            Err(e) => r.problems.push(format!("the skill file: {e}")),
        }
    }
    if kind == "tile" {
        tile_checks(dir, &name, &version, &mut r);
    }
    let mut files = Vec::new();
    walk(dir, &mut files, 0);
    if !files.iter().any(|f| f.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("readme"))) {
        r.warnings.push("add a README".into());
    }
    if !files.iter().any(|f| f.to_string_lossy().contains("test")) {
        r.warnings.push("there is no test file: add one before you publish".into());
    }
    for f in &files {
        let rel = f.strip_prefix(dir).unwrap_or(f).display().to_string();
        if std::fs::metadata(f).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            continue;
        }
        if let Ok(t) = std::fs::read_to_string(f) {
            r.problems.extend(secret_problems(&rel, &t));
        }
    }
    r.facts.push(format!("{} files scanned for secrets", files.len()));
    r
}

/// The checks of a tile app project: the bundle (manifest, files, sizes), the lint, the tests folder, and that `app.json` agrees with the project file.
fn tile_checks(dir: &Path, name: &str, version: &str, r: &mut Report) {
    use super::tile;
    match tile::Bundle::from_dir(dir) {
        Err(e) => r.problems.push(format!("the tile app: {e}")),
        Ok(b) => {
            let m = &b.manifest;
            r.facts.push(format!("tile app {} \"{}\" in group {}, {} files, {} KB; icon {}", m.id, m.name, m.group, b.files.len(), b.total().div_ceil(1024), if m.icon.is_empty() { "none" } else { &m.icon }));
            let caps = m.caps.permissions();
            r.facts.push(format!("it asks for: {}", if caps.is_empty() { "nothing (no saved data, no network)".to_string() } else { caps.join(", ") }));
            if m.id != name {
                r.problems.push(format!("app.json id {:?} differs from the project name {name:?}", m.id));
            }
            if m.version != version && dir.join(tile::MANIFEST).is_file() {
                r.warnings.push(format!("app.json version {} differs from {version} in the project file; the project file wins when packing", m.version));
            }
            let (p, w) = tile::lint(&b);
            r.problems.extend(p);
            r.warnings.extend(w);
        }
    }
    if !dir.join("tests").is_dir() {
        r.warnings.push("there is no tests folder: add tests/*.js (T.test) before you publish".into());
    }
}
