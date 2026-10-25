//! The authoring side of the store: scaffold a project (a skill, an MCP server, a WASM tool), pack a project folder into a
//! package, and push a signed package to a private store server. The assistant's tools (`app_scaffold`, `store_validate`,
//! `store_publish`) and `iyakki pkg pack|validate|publish` are thin wrappers over this. Nothing here reads the vault or asks
//! the person: callers do that. See docs/APP-AUTHORING.md.

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use super::manifest::{Meta, Publisher};
use super::vet::{read_project, PROJECT_FILE};
use crate::pkg;

type Files = &'static [(&'static str, &'static str)];

macro_rules! t {
    ($dir:literal, $f:literal) => {
        ($f, include_str!(concat!("../../../../dist/templates/", $dir, "/", $f)))
    };
}

const SKILL: Files = &[t!("skill", "iyakki-app.json"), t!("skill", "skill.md"), t!("skill", "README.md"), t!("skill", "tests/test_skill.py")];
const MCP_PY: Files = &[t!("mcp-python", "iyakki-app.json"), t!("mcp-python", "server.py"), t!("mcp-python", "README.md"), t!("mcp-python", "tests/test_server.py")];
const MCP_NODE: Files = &[t!("mcp-node", "iyakki-app.json"), t!("mcp-node", "server.js"), t!("mcp-node", "README.md"), t!("mcp-node", "tests/test_server.js")];
const WASM: Files = &[t!("wasm-tool", "iyakki-app.json"), t!("wasm-tool", "tool.json"), t!("wasm-tool", "module.wat"), t!("wasm-tool", "README.md"), t!("wasm-tool", "tests/check_tool.py")];

const TILE: Files = &[t!("tile", "iyakki-app.json"), t!("tile", "app.json"), t!("tile", "index.html"), t!("tile", "app.js"), t!("tile", "style.css"), t!("tile", "README.md"), t!("tile", "tests/app.test.js")];

/// What `app_scaffold` can make: the kind names and what each is.
pub const SCAFFOLDS: [(&str, &str); 5] = [
    ("tile", "an IYAKKI tile app: a web page (HTML, CSS, JS) that runs in its own tile of the launcher, with headless tests"),
    ("skill", "a skill: text the assistant reads (no code, no access)"),
    ("mcp-python", "an MCP server in Python (standard library, stdio) with a fake-client test"),
    ("mcp-node", "an MCP server in Node (built-ins only, stdio) with a fake-client test"),
    ("wasm-tool", "a sandboxed WASM tool (WAT; compile Rust or C to the same two exports)"),
];

fn files_of(kind: &str) -> Option<Files> {
    Some(match kind {
        "skill" => SKILL,
        "tile" => TILE,
        "mcp" | "mcp-python" => MCP_PY,
        "mcp-node" => MCP_NODE,
        "tool" | "wasm-tool" => WASM,
        _ => return None,
    })
}

pub fn title_of(name: &str) -> String {
    let t = name.replace('-', " ");
    let mut c = t.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Write a starting project into `dir` (inside `root`, which must exist). The folder must be new or empty.
/// Returns what was written and the next steps.
pub fn scaffold(kind: &str, name: &str, root: &Path, dir: &str) -> Result<String, String> {
    let files = files_of(kind).ok_or_else(|| format!("unknown kind {kind:?}: {}", SCAFFOLDS.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")))?;
    if !crate::skills::valid_skill_name(name) {
        return Err("name: lowercase letters, digits and dashes, at most 48 characters (for example meeting-notes)".into());
    }
    let target = crate::workspace::resolve_in(root, if dir.trim().is_empty() { name } else { dir })?;
    if std::fs::read_dir(&target).map(|mut r| r.next().is_some()).unwrap_or(false) {
        return Err(format!("{} already has files in it: pick a new folder", target.display()));
    }
    let title = title_of(name);
    let mut written = Vec::new();
    for (path, content) in files {
        let dest = target.join(path);
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::write(&dest, content.replace("__NAME__", name).replace("__TITLE__", &title)).map_err(|e| format!("{}: {e}", dest.display()))?;
        written.push(path.to_string());
    }
    let test = match kind {
        "skill" => "python3 tests/test_skill.py",
        "mcp" | "mcp-python" => "python3 tests/test_server.py",
        "mcp-node" => "node tests/test_server.js",
        "tile" => "tile_test (the headless tests in tests/)",
        _ => "python3 tests/check_tool.py",
    };
    Ok(format!(
        "Created {} in {} with: {}.\nNext: edit it, run `{test}` there, then store_validate on the folder, then `iyakki pkg keygen` once and `iyakki pkg pack . --key KEY -o {name}.iyp`, then store_publish (the person approves). Raise `version` in {PROJECT_FILE} for every release.",
        kind,
        target.display(),
        written.join(", ")
    ))
}

// ------------------------------------------------------------------------------------------------ pack

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn list(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn read_in(dir: &Path, rel: &str) -> Result<String, String> {
    if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|p| p == "..") {
        return Err(format!("{rel:?}: a file inside the project, please"));
    }
    let p = dir.join(rel);
    std::fs::read_to_string(&p).map_err(|e| crate::scope::explain_io(&e, &p, "read"))
}

/// The store information of a project (`iyakki-app.json`, `README.md`, `CHANGELOG.md`).
pub fn meta_from_project(dir: &Path, v: &Value) -> Result<Meta, String> {
    let clip = |t: String, n: usize| -> String { t.chars().take(n).collect() };
    let mut m = Meta {
        title: s(v, "title"),
        summary: s(v, "summary"),
        categories: list(v, "categories"),
        tags: list(v, "tags"),
        permissions: list(v, "permissions"),
        min_version: s(v, "min_version"),
        support_url: s(v, "support_url"),
        source_url: s(v, "source_url"),
        target: s(v, "target"),
        ext_kind: s(v, "ext_kind"),
        description: clip(std::fs::read_to_string(dir.join("README.md")).unwrap_or_default(), 8000),
        changelog: clip(std::fs::read_to_string(dir.join("CHANGELOG.md")).unwrap_or_default(), 4000),
        ..Default::default()
    };
    if let Some(p) = v.get("publisher") {
        m.publisher = Publisher { id: s(p, "id"), name: s(p, "name"), url: s(p, "url") };
    }
    m.validate()?;
    Ok(m)
}

/// Build the package of a project folder. With a key it is signed and carries the store information; without one it is
/// plain (a store refuses it until it is signed). Returns the package text and notes for the person.
pub fn pack_dir(dir: &Path, key: Option<&SigningKey>) -> Result<(String, Vec<String>), String> {
    let report = super::vet::vet_dir(dir);
    if !report.ok() {
        return Err(format!("the project does not pass its checks:\n{}", report.text()));
    }
    let v = read_project(dir).ok_or("no iyakki-app.json")?;
    let (kind, name, version, license) = (s(&v, "kind"), s(&v, "name"), s(&v, "version"), s(&v, "license"));
    let entry = s(&v, "entry");
    let mut meta = meta_from_project(dir, &v)?;
    let mut tile_payload = None;
    if kind == "tile" {
        let mut b = super::tile::Bundle::from_dir(dir)?;
        if b.manifest.id != name {
            return Err(format!("app.json says id {:?} but the project is called {name:?}: make them the same", b.manifest.id));
        }
        b.manifest.version = version.clone();
        meta.permissions = b.manifest.caps.permissions();
        tile_payload = Some(b.to_payload());
    }
    let mut notes = Vec::new();
    let meta_json = meta.to_json();
    let text = match kind.as_str() {
        "skill" => pkg::build_skill(&read_in(dir, &entry)?, &version, &license, key),
        "tool" => {
            let module = {
                let mp = dir.join(s(&v, "module"));
                std::fs::read(&mp).map_err(|e| crate::scope::explain_io(&e, &mp, "read the module"))?
            };
            pkg::build_tool(&read_in(dir, &entry)?, &module, key)
        }
        "mcp" => {
            let runtime = s(&v, "runtime");
            let flag = match runtime.as_str() {
                "python3" | "python" => "-c",
                "node" => "-e",
                _ => return Err("runtime: python3 or node (the program that runs the server)".into()),
            };
            let payload = json!({"name": name, "command": runtime, "args": [flag, read_in(dir, &entry)?]}).to_string();
            pkg::build_generic("mcp", &name, &version, &license, &payload, None, None, key)
        }
        "tile" => pkg::build_generic("tile", &name, &version, &license, tile_payload.as_deref().unwrap_or(""), None, None, key),
        other => pkg::build_generic(other, &name, &version, &license, &read_in(dir, &entry)?, None, None, key),
    }
    .map_err(|e| e.to_string())?;
    let text = match key {
        Some(k) => pkg::resign(&text, Some(&meta_json), None, k).map_err(|e| e.to_string())?,
        None => {
            notes.push("not signed: sign it with --key before a store will take it".to_string());
            text
        }
    };
    if v.get("version").is_none() {
        notes.push("no version in the project".into());
    }
    Ok((text, notes))
}

// ------------------------------------------------------------------------------------------------ publish

/// What a store server answered to a publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub trust: String,
    pub serial: u64,
}

fn server_base(server: &str) -> Result<String, String> {
    let t = server.trim().trim_end_matches('/').trim_end_matches("/catalog.json");
    if !(t.starts_with("https://") || t.starts_with("http://")) || t.len() > 300 || t.chars().any(|c| c.is_control() || c == ' ') {
        return Err("the store address starts with https:// or http://".into());
    }
    Ok(t.to_string())
}

/// Push signed package text to a private store. Only plain `http` to the loopback or a private address, or `https`, is
/// accepted: a token never travels in the clear over the internet.
pub fn publish_to_server(up: &dyn iyakki_proxy::Upstream, server: &str, token: &str, package: &str) -> Result<Published, String> {
    let base = server_base(server)?;
    if base.starts_with("http://") && !private_host(&base) {
        return Err("the token would travel unencrypted: use an https address (TLS at the ingress), or a loopback or private-network address".into());
    }
    if token.trim().is_empty() {
        return Err("no publish token".into());
    }
    let headers = vec![("Authorization".to_string(), format!("Bearer {}", token.trim())), ("Content-Type".to_string(), "text/plain".to_string())];
    let r = up.request("POST", &format!("{base}/v1/publish"), &headers, package.as_bytes()).map_err(|e| unreachable_text(&base, &e))?;
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    if r.status != 200 {
        return Err(refusal_text(r.status, v["error"].as_str()));
    }
    Ok(Published { name: s(&v, "name"), version: s(&v, "version"), sha256: s(&v, "sha256"), trust: s(&v, "trust"), serial: v["serial"].as_u64().unwrap_or(0) })
}

/// A publish that could not even be sent: what happened and the one next step (never the bare transport text alone).
fn unreachable_text(base: &str, err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    let what = if lower.contains("refused") {
        "the connection was refused: the store's pod is not running or not ready, or its port is not open"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "it did not answer in time: the cluster may be stopped or the pod still starting"
    } else if lower.contains("dns") || lower.contains("resolve") || lower.contains("lookup") {
        "its name could not be found: check the address in STORE, Settings"
    } else {
        "no answer"
    };
    format!("Nothing was sent: could not reach the store at {base} ({what}; {err}). If it was just deployed, wait a minute; otherwise open the K8S tile and check the pod private-store, then publish again.")
}

/// What the store's refusal means and what to do (`why` is the store's own sentence, when it sent one).
pub fn refusal_text(status: u16, why: Option<&str>) -> String {
    let said = why.map(|w| format!(": {w}")).unwrap_or_default();
    match status {
        403 if why.is_some_and(|w| w.contains("read-only")) => format!("the store is read-only ({status}{said}): it accepts no publishing. Its administrator has to switch read-only off (redeploy or Repair without the Read-only option)."),
        401 | 403 => format!("the store did not accept the publish token ({status}{said}). The token in your vault is wrong or was replaced: ask the store's administrator for a publisher token and enter it again (STORE, Settings, Private store), or, for a store deployed from this device, use Repair in Private store, Manage."),
        404 => format!("the store has no publish address ({status}{said}): it is not an IYAKKI private store, or it is an older one. Check the address in STORE, Settings."),
        409 => format!("the store already has this version ({status}{said}): raise the version and publish again."),
        413 => format!("the package is too big for this store ({status}{said}): remove large files from the project."),
        429 => format!("the store is limiting publishes ({status}{said}): wait a minute and try again."),
        500..=599 => format!("the store had a problem ({status}{said}): check its pod in the K8S tile, then try again. Nothing was listed."),
        _ => format!("the store refused it ({status}{said})."),
    }
}

fn private_host(base: &str) -> bool {
    let host = base.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h).trim_matches(['[', ']']);
    if host == "localhost" || host == "::1" || host.ends_with(".svc") || host.ends_with(".cluster.local") || host.ends_with(".local") || !host.contains('.') {
        return true;
    }
    let p: Vec<u8> = host.split('.').filter_map(|x| x.parse().ok()).collect();
    p.len() == 4 && (p[0] == 127 || p[0] == 10 || (p[0] == 172 && (16..32).contains(&p[1])) || (p[0] == 192 && p[1] == 168))
}

/// The folder of a package path or project folder, made absolute inside the workspace.
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    crate::workspace::resolve_in(root, rel)
}
