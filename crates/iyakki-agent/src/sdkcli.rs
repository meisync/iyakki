//! `iyakki sdk`: the computer's side of the SDK (dist/sdk/README.md). `init` starts a project with the store's scaffolds
//! (`store::author::scaffold`, the code the IDE and the assistant use) plus the agent instructions, the bridge's typings and a
//! `.gitignore`; `doctor` says what is ready and what to do next; `docs` lists the guides. All three work offline. The emulator
//! (`dist/sdk/EMULATOR.md`) is specified and NOT BUILT.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::devmode::client::{DevClient, Device, Home};
use crate::store::author;

pub const USAGE: &str = "usage: iyakki sdk <command>
  init <name> [--kind tile|skill|mcp|service] [--dir DIR]   start a project (default kind: tile; default folder: ./NAME) with tests,
                                      CLAUDE.md and AGENTS.md for coding agents, iyakki.d.ts and .gitignore; refuses a folder that has files
  doctor                              what is ready on this computer (program, paired device, does it answer) and the next step
  docs [topic]                        the guides, offline: with a topic, its path and what it covers
  check|test|shot|emulate TILE ...    the tile emulator on this computer, no device (docs/SDK-EMULATOR.md): check = install-card text, test =
                                      scripted runs with golden pictures, shot = one PNG, emulate = the window
  (emulator start|stop|status is specified in dist/sdk/EMULATOR.md and not built yet)
options: --home DIR (developer mode folder for doctor; default ~/.config/iyakki/dev or $IYAKKI_DEV_HOME)";

pub const KINDS: [&str; 4] = ["tile", "skill", "mcp", "service"];

const D_TS: &str = include_str!("../../../dist/sdk/iyakki.d.ts");
const CLAUDE_MD: &str = include_str!("../../../dist/sdk/templates/CLAUDE.md");
const AGENTS_MD: &str = include_str!("../../../dist/sdk/templates/AGENTS.md");

macro_rules! svc {
    ($f:literal) => {
        ($f, include_str!(concat!("../../../dist/sdk/templates/service/", $f)))
    };
}
/// A service project (the store scaffolds have none): published name, content.
const SERVICE: &[(&str, &str)] = &[svc!("README.md"), svc!("server.py"), svc!("Dockerfile"), svc!("deploy/app.yaml"), svc!("tests/test_server.py")];
const SERVICE_DOCKERIGNORE: &str = include_str!("../../../dist/sdk/templates/service/dockerignore");

const GITIGNORE: &str = "# keys and secrets: never commit them\n*.key\n*.pem\n.env\n# build output\n*.iyp\nbuild/\ndist/\nnode_modules/\n__pycache__/\n*.pyc\n# pictures from `iyakki dev screenshot`\nshot*.png\n";

/// The guides: (topic, path in the IYAKKI sources, what it covers). Paths are relative to the sources' root.
pub const DOCS: &[(&str, &str, &str)] = &[
    ("sdk", "dist/sdk/README.md", "this SDK: what it is, install, quickstart"),
    ("bridge", "dist/sdk/iyakki.d.ts", "TypeScript declarations of window.iyakki (tile apps)"),
    ("tile", "docs/APP-AUTHORING.md", "tile apps: files, the bridge, security, the headless tests, packages, publishing to a store"),
    ("loop", "docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md", "the build loop for each kind, rules, k3s services, for coding agents"),
    ("os", "docs/development/CLAUDE-CODE-OS-DEVELOPMENT.md", "changing IYAKKI itself with a coding agent"),
    ("device", "docs/DEVELOPER-MODE.md", "pair a computer, scopes, every `iyakki dev` command, system update"),
    ("api", "docs/development/SDK-API.md", "the design of the whole SDK (namespaces, device loop, versions); designs are marked not built"),
    ("emulator", "dist/sdk/EMULATOR.md", "a QEMU device for the loop: target spec, NOT BUILT"),
    ("test", "docs/LOCAL-TEST.md", "testing IYAKKI on a computer without hardware (QEMU scripts)"),
];

fn flag<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).map(String::as_str)
}

/// Words that are not flags and not the value of one.
fn positional(a: &[String]) -> Vec<&str> {
    let mut v = Vec::new();
    let mut skip = false;
    for x in a {
        if skip {
            skip = false;
        } else if x.starts_with("--") {
            skip = true;
        } else {
            v.push(x.as_str());
        }
    }
    v
}

/// Run `iyakki sdk <args>`; lines go to `out`.
pub fn run(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let pos = positional(a);
    match pos.first().copied() {
        None | Some("help") => out(USAGE),
        Some("init") => {
            let name = pos.get(1).ok_or("usage: iyakki sdk init <name> [--kind tile|skill|mcp|service] [--dir DIR]")?;
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            out(&init(name, flag(a, "--kind").unwrap_or("tile"), flag(a, "--dir"), &cwd)?);
        }
        Some("doctor") => {
            let home = Home::at(&flag(a, "--home").map(PathBuf::from).unwrap_or_else(crate::devmode::client::default_home));
            out(&doctor(&home, &|d: &Device| DevClient::new(&home, d.clone())?.whoami()));
        }
        Some("docs") => out(&docs(pos.get(1).copied())?),
        Some("check" | "test" | "shot" | "emulate") => return emulator_cmd(a, out),
        Some("emulator") => return Err("iyakki sdk emulator is not built yet: the target is specified in dist/sdk/EMULATOR.md".into()),
        Some(other) => return Err(format!("unknown command {other:?}\n{USAGE}")),
    }
    Ok(())
}

/// `iyakki sdk check|test|shot|emulate`: the tile emulator is its own small program (`iyakki-sdk-emulator`, no device and no OS build needed);
/// it is looked for next to this program, then on PATH.
fn emulator_cmd(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("iyakki-sdk-emulator"))).filter(|p| p.is_file());
    let prog = beside.unwrap_or_else(|| PathBuf::from("iyakki-sdk-emulator"));
    let o = std::process::Command::new(&prog).args(a).output().map_err(|_| "the program iyakki-sdk-emulator was not found next to iyakki or on PATH; build it with: cargo build --release -p iyakki-sdk-emulator".to_string())?;
    let text = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
    if !text.is_empty() {
        out(&text);
    }
    if o.status.success() { Ok(()) } else { Err(format!("iyakki-sdk-emulator {} failed", a.first().map(String::as_str).unwrap_or(""))) }
}

/// Keep the sections of a template for `kind` (`<!-- kind:K -->` ... `<!-- /kind -->`), drop the others, fill the names.
pub fn render(template: &str, kind: &str, name: &str) -> String {
    let mut out = String::new();
    let mut keep = true;
    for line in template.lines() {
        if let Some(k) = line.strip_prefix("<!-- kind:").and_then(|r| r.strip_suffix(" -->")) {
            keep = k == kind;
        } else if line == "<!-- /kind -->" {
            keep = true;
        } else if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.replace("__NAME__", name).replace("__TITLE__", &author::title_of(name)).replace("__KIND__", kind)
}

fn write(dest: &Path, content: &str) -> Result<(), String> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    std::fs::write(dest, content).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Create the project `name` of `kind` in `dir` (default: `NAME` under `cwd`). The folder must be new or empty.
pub fn init(name: &str, kind: &str, dir: Option<&str>, cwd: &Path) -> Result<String, String> {
    let scaffold_kind = match kind {
        "tile" | "skill" | "service" | "mcp-node" => kind,
        "mcp" => "mcp-python",
        _ => return Err(format!("unknown kind {kind:?}: {}", KINDS.join(", "))),
    };
    if !crate::skills::valid_skill_name(name) {
        return Err("name: lowercase letters, digits and dashes, at most 48 characters (for example meeting-notes)".into());
    }
    let target = match dir {
        Some(d) if !d.trim().is_empty() => cwd.join(d.trim()),
        _ => cwd.join(name),
    };
    if std::fs::read_dir(&target).map(|mut r| r.next().is_some()).unwrap_or(false) {
        return Err(format!("{} already has files in it: pick a new folder (--dir)", target.display()));
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let note = if scaffold_kind == "service" {
        let title = author::title_of(name);
        for (path, content) in SERVICE {
            write(&target.join(path), &content.replace("__NAME__", name).replace("__TITLE__", &title))?;
        }
        write(&target.join(".dockerignore"), SERVICE_DOCKERIGNORE)?;
        format!("Created a service project {name} in {}.", target.display())
    } else {
        let (root, leaf) = (target.parent().unwrap_or(cwd), target.file_name().and_then(|n| n.to_str()).unwrap_or(name));
        author::scaffold(scaffold_kind, name, root, leaf)?.lines().next().unwrap_or_default().to_string()
    };
    let section = if scaffold_kind == "mcp-python" || scaffold_kind == "mcp-node" { "mcp" } else { scaffold_kind };
    write(&target.join("CLAUDE.md"), &render(CLAUDE_MD, section, name))?;
    write(&target.join("AGENTS.md"), &render(AGENTS_MD, section, name))?;
    write(&target.join("iyakki.d.ts"), D_TS)?;
    write(&target.join(".gitignore"), GITIGNORE)?;
    let first = match section {
        "tile" => format!("cd {} && iyakki dev push . --watch --open   (after `iyakki dev pair <address>` once; `iyakki sdk doctor` checks)", target.display()),
        "service" => "run python3 tests/test_server.py, then build and deploy with the IDE's deploy tools".to_string(),
        _ => "run the test in tests/, then `iyakki pkg validate .`".to_string(),
    };
    Ok(format!("{note}\nAdded CLAUDE.md and AGENTS.md (instructions for coding agents), iyakki.d.ts and .gitignore.\nNext: {first}"))
}

/// What is ready on this computer and what to do next. `probe` asks a device who this computer is. Never an error: a missing device is
/// an ordinary state (everything but `iyakki dev` works without one).
pub fn doctor(home: &Home, probe: &dyn Fn(&Device) -> Result<Value, String>) -> String {
    let mut l = vec![format!("iyakki {} (this program): ok", env!("CARGO_PKG_VERSION"))];
    let devices = home.devices();
    if devices.is_empty() {
        l.push(format!("Paired device: none (looked in {}).", home.dir().display()));
        l.push("Without a device you can still run `iyakki sdk init`, the project's tests, `iyakki pkg validate` and `iyakki pkg pack`.".into());
        l.push("Next step: on the device open Settings, Developer mode, switch it on and choose Pair a computer; then run `iyakki dev pair <address>` here.".into());
        return l.join("\n");
    }
    let def = home.default_name();
    for d in &devices {
        l.push(format!("Paired device: {}{} at {}:{}, acting as {}", d.name, if d.name == def { " (default)" } else { "" }, d.host, d.port, if d.account.is_empty() { "?" } else { &d.account }));
    }
    let Ok(d) = home.device(None) else { return l.join("\n") };
    match probe(&d) {
        Ok(v) => {
            let scopes: Vec<&str> = v["scopes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            l.push(format!("{} answers: this computer is “{}”, role {}, developer mode {}.", d.name, v["computer"].as_str().unwrap_or("?"), v["role"].as_str().unwrap_or("?"), v["status"].as_str().unwrap_or("?")));
            l.push(format!("Scopes held: {}.", if scopes.is_empty() { "none".to_string() } else { scopes.join(", ") }));
            if scopes.contains(&"install") {
                l.push("Next step: in your project folder run `iyakki dev push . --watch --open`, then `iyakki dev logs <name> --follow`.".into());
            } else {
                l.push("Next step: ask an administrator to add the scope `install` for this computer (Settings, Developer mode on the device); without it nothing can be pushed.".into());
            }
        }
        Err(e) => {
            l.push(format!("{} does not answer: {e}", d.name));
            l.push("Next step: check that the device is on and on the same network, that Settings, Developer mode is switched on (it switches itself off after a while), and the address in `iyakki dev devices`. If the device changed address, pair again.".into());
        }
    }
    l.join("\n")
}

/// The guides, offline: all of them, or one topic.
pub fn docs(topic: Option<&str>) -> Result<String, String> {
    let line = |t: &(&str, &str, &str)| format!("{:<9} {}\n          {}", t.0, t.1, t.2);
    match topic {
        None => Ok(format!("Guides (paths are in the IYAKKI sources; `iyakki sdk docs TOPIC` shows one):\n{}", DOCS.iter().map(line).collect::<Vec<_>>().join("\n"))),
        Some(t) => DOCS.iter().find(|d| d.0 == t).map(line).ok_or_else(|| format!("no guide called {t:?}; topics: {}", DOCS.iter().map(|d| d.0).collect::<Vec<_>>().join(", "))),
    }
}

#[cfg(test)]
#[path = "sdkcli_tests.rs"]
mod tests;
