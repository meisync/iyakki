//! Ratchet and inventory: how often the UI (`iyakki-apps` and `iyakki-shell`, non-test code) reaches into `iyakki_agent::` directly
//! (docs/development/UI-BOUNDARY.md, alpha.5 item 0a). Every use is classified by domain; the count per domain may only shrink
//! (`docs/development/ui-agent-uses.allow`: one line per domain and per agent module, so a move between modules cannot hide). The measured table with the per-file counts is `docs/development/UI-AGENT-USES.txt`.
//!
//! Regenerate the table: `IYAKKI_UI_USES_WRITE=1 cargo test -p iyakki-core --test ui_agent_uses`.
//!
//! One use is one path `iyakki_agent::module...`, or one item of a group `iyakki_agent::{module::Item, other}`. Counted: code outside
//! test files (`*_tests.rs`, `tests.rs`, `tests/`), outside `#[cfg(test)] mod` blocks and outside `//` comment lines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ALLOW: &str = include_str!("../../../docs/development/ui-agent-uses.allow");
const TABLE: &str = "docs/development/UI-AGENT-USES.txt";
const NEEDLE: &str = "iyakki_agent::";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "testdata" && name != "tests" {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
            out.push(p);
        }
    }
}

/// The file without its test module (everything from the first `#[cfg(test)]` followed by a `mod` item).
fn non_test_code(text: &str) -> &str {
    let mut at = 0;
    while let Some(i) = text[at..].find("#[cfg(test)]") {
        let start = at + i;
        let after = &text[start + "#[cfg(test)]".len()..];
        let next = after.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("#[") && !l.starts_with("//"));
        if next.is_some_and(|l| l.starts_with("mod ") || l.starts_with("pub mod ") || l.starts_with("pub(crate) mod ")) {
            return &text[..start];
        }
        at = start + 1;
    }
    text
}

/// The domains, in the order of the table. A module that is not listed is `other`.
pub const DOMAINS: [&str; 12] = ["settings", "vault", "accounts", "chat", "files", "network", "display", "sound", "input", "cluster", "store", "other"];

/// The domain of an `iyakki_agent` module (the first path segment after `iyakki_agent::`).
fn domain_of(module: &str) -> &'static str {
    match module {
        "settings" | "profilepolicy" | "profilestore" | "config" | "layout" | "power" | "platform" | "sysinfo" | "clock" | "ntp" | "livemode" | "cloudmode" | "installstartup" | "hwprofile" | "devname" | "scope" | "localos" => "settings",
        "vault" | "pendingplus" | "sharedvault" | "commonvault" | "cloudvault" | "cloudvaultd" | "cloudvaultguard" | "cloudvaultlink" | "authcodes" | "credguard" | "gitkeys" | "grants" | "pending" | "lockscreen" | "cloudkeys" | "cloudrotate" => "vault",
        "cloudtenant" | "cloudtenants" | "cloudtenantdir" | "cloudmembers" | "cloudroles" | "cloudaccess" | "assign" | "sharing" | "inbox" => "accounts",
        "chat" | "agent" | "providers" | "aiprofile" | "voice" | "voicedesk" | "voiceassist" | "skills" | "story" | "assistrun" | "gemini" | "geminilive" | "lemonade" | "models" | "modelhost" | "modelopts" | "mcp" | "extagent" | "tasks" | "jobs" | "flows" | "context" | "kb" => "chat",
        "hostfiles" | "sharedfolder" | "workspace" | "disks" | "backup" | "pkg" | "gitconn" | "gitsite" | "gitbrowse" | "wsexec" | "quickinstall" | "mediadl" | "media" | "qr" => "files",
        "iyakki_proxy" | "ssh" | "netscan" | "netprobe" | "hotspot_radio" | "webengine" | "browsermedia" | "browserext" | "browserdev" | "devmode" | "meteredlink" | "domains" | "exposure" | "macfw" => "network",
        "camdetect" | "camrec" | "cameras" | "ddcci" | "gpu" | "dash" | "accessibility" => "display",
        "sounddoctor" | "webmix" | "player" | "audioout" | "mixer" | "syssounds" | "audiosink" | "audiofmt" | "micdefaults" | "micpcm" | "echocancel" | "btaudiotools" | "bluetoothpair" => "sound",
        "keyboard" | "inputprofile" | "battery" => "input",
        "clusters" | "cluster" | "devcluster" | "k8s" | "k8scr" | "k8sexp" | "k3sdev" | "kubeprofiles" | "swarm" | "clusterprobe" => "cluster",
        "store" | "components" | "dlmanifest" | "imageapps" | "linuxapps" => "store",
        _ => "other",
    }
}

/// Every module name (first path segment) used in the non-test code of `text`; one entry per use.
fn uses(text: &str) -> Vec<String> {
    let code: String = non_test_code(text).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = code[at..].find(NEEDLE) {
        let s = at + i;
        // `iyakki_agent` must be a whole word (not `my_iyakki_agent::`)
        let word_start = s == 0 || !(b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_');
        let rest = s + NEEDLE.len();
        at = rest;
        if !word_start {
            continue;
        }
        if code[rest..].starts_with('{') {
            // a group: one use per top-level item
            let mut depth = 0usize;
            let mut item = String::new();
            for c in code[rest..].chars() {
                match c {
                    '{' => {
                        depth += 1;
                        if depth > 1 {
                            item.push(c);
                        }
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        item.push(c);
                    }
                    ',' if depth == 1 => {
                        push_item(&mut out, &item);
                        item.clear();
                    }
                    _ => item.push(c),
                }
            }
            push_item(&mut out, &item);
        } else {
            let m: String = code[rest..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            out.push(m);
        }
    }
    // a re-export seam (`agent_handles_bN`) does not remove the dependency: every use of it counts as a use of the agent module behind it
    let mut at = 0;
    while let Some(i) = code[at..].find("agent_handles_b") {
        let s = at + i;
        let mut j = s + "agent_handles_b".len();
        while j < code.len() && code.as_bytes()[j].is_ascii_digit() {
            j += 1;
        }
        at = j;
        if code[j..].starts_with("::") {
            let m: String = code[j + 2..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !m.is_empty() && m != "self" {
                out.push(m);
            }
        }
    }
    out
}

fn push_item(out: &mut Vec<String>, item: &str) {
    let m: String = item.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if !m.is_empty() {
        out.push(m);
    }
}

/// (per-domain totals, per-domain-per-module totals, per-file totals)
type Measured = (BTreeMap<&'static str, usize>, BTreeMap<(&'static str, String), usize>, BTreeMap<String, usize>);

fn measure() -> Measured {
    let r = root();
    let mut files = Vec::new();
    rust_files(&r.join("crates/iyakki-apps/src"), &mut files);
    rust_files(&r.join("crates/iyakki-shell/src"), &mut files);
    let (mut dom, mut modules, mut per_file) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    for d in DOMAINS {
        dom.insert(d, 0);
    }
    for f in files {
        let u = uses(&std::fs::read_to_string(&f).unwrap_or_default());
        if u.is_empty() {
            continue;
        }
        per_file.insert(f.strip_prefix(&r).unwrap().to_string_lossy().into_owned(), u.len());
        for m in u {
            let d = domain_of(&m);
            *dom.get_mut(d).unwrap() += 1;
            *modules.entry((d, m)).or_insert(0) += 1;
        }
    }
    (dom, modules, per_file)
}

fn render(m: &Measured) -> String {
    let (dom, modules, files) = m;
    let total: usize = dom.values().sum();
    let mut s = String::new();
    s.push_str("# Direct `iyakki_agent::` uses in the UI (iyakki-apps + iyakki-shell, non-test code), by domain.\n");
    s.push_str("# Generated by crates/iyakki-core/tests/ui_agent_uses.rs (IYAKKI_UI_USES_WRITE=1); the ratchet is docs/development/ui-agent-uses.allow.\n\n");
    s.push_str(&format!("total {total}\n\n== by domain\n"));
    for d in DOMAINS {
        s.push_str(&format!("{d} {}\n", dom[d]));
    }
    s.push_str("\n== by domain and agent module\n");
    for d in DOMAINS {
        let mut v: Vec<_> = modules.iter().filter(|((dd, _), _)| *dd == d).map(|((_, m), n)| (m.as_str(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (name, n) in v {
            s.push_str(&format!("{d} {name} {n}\n"));
        }
    }
    s.push_str("\n== by file\n");
    let mut f: Vec<_> = files.iter().collect();
    f.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (p, n) in f {
        s.push_str(&format!("{p} {n}\n"));
    }
    s
}

fn allowed() -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (i, l) in ALLOW.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (d, n) = l.rsplit_once(' ').unwrap_or_else(|| panic!("allow list line {}: expected `domain NAME count` or `module NAME count`", i + 1));
        assert!(m.insert(d.to_string(), n.trim().parse().unwrap_or_else(|_| panic!("allow list line {}: bad count", i + 1))).is_none(), "line {}: {d} twice", i + 1);
    }
    m
}

#[test]
fn the_ui_does_not_gain_direct_agent_uses_and_the_counts_only_shrink() {
    let m = measure();
    let text = render(&m);
    if std::env::var_os("IYAKKI_UI_USES_WRITE").is_some() {
        std::fs::write(root().join(TABLE), &text).unwrap();
    }
    let allow = allowed();
    let mut grew = Vec::new();
    let mut lower = Vec::new();
    // one line per domain and one per agent module; a module that is not listed is allowed no use at all
    let mut now_all: BTreeMap<String, usize> = BTreeMap::new();
    for d in DOMAINS {
        now_all.insert(format!("domain {d}"), m.0[d]);
    }
    for ((_, module), n) in &m.1 {
        now_all.insert(format!("module {module}"), *n);
    }
    for (key, now) in &now_all {
        match allow.get(key).copied() {
            None if *now > 0 => grew.push(format!("{key}: {now} now, not in the allow list")),
            None => {}
            Some(a) if *now > a => grew.push(format!("{key}: {now} now, {a} allowed")),
            Some(a) if *now < a => lower.push(format!("{key}: {now} now, {a} listed -> lower the number to {now}")),
            _ => {}
        }
    }
    for (key, a) in &allow {
        if !now_all.contains_key(key) {
            lower.push(format!("{key}: no use left (listed {a}) -> remove this line"));
        }
    }
    assert!(grew.is_empty(), "WHAT: the UI (iyakki-apps, iyakki-shell) now names `iyakki_agent::` more often than the allow list permits.\nWHERE: the domains below grew. To see the file and line, run `IYAKKI_UI_USES_WRITE=1 cargo test -p iyakki-core --test ui_agent_uses` and read `git diff {TABLE}` (per-file counts).\nFIX: do not call the agent from an app. Ask the core through the contract: take the facet from the core handle (`cx.core.files()`, `cx.core.media()`, ...) or, for an SDK app, a port the shell adapter hands in. Before: `iyakki_agent::files::list(dir)`. After: `cx.core.files().list(user, dir)`. If no facet exists yet, stop and ask: a new facet is the owner's decision (docs/development/UI-BOUNDARY.md).\nOffending domains:\n  {}", grew.join("\n  "));
    assert!(lower.is_empty(), "Good: fewer direct uses. Lower docs/development/ui-agent-uses.allow so they cannot grow back:\n  {}", lower.join("\n  "));
    let on_disk = std::fs::read_to_string(root().join(TABLE)).unwrap_or_default();
    assert_eq!(on_disk, text, "{TABLE} is out of date: regenerate it with IYAKKI_UI_USES_WRITE=1 cargo test -p iyakki-core --test ui_agent_uses");
}

#[test]
fn the_scanner_counts_paths_and_group_items_and_skips_tests_and_comments() {
    assert_eq!(uses("let a = iyakki_agent::settings::load();"), vec!["settings"]);
    assert_eq!(uses("use iyakki_agent::{mixer::Stats, audioout::{A, B}, store};"), vec!["mixer", "audioout", "store"]);
    assert!(uses("// iyakki_agent::chat::x\nlet y = my_iyakki_agent::chat::z;").is_empty());
    assert_eq!(uses("fn a() { iyakki_agent::chat::a(); }\n#[cfg(test)]\nmod t { fn b() { iyakki_agent::chat::b(); } }").len(), 1);
    assert_eq!(domain_of("mixer"), "sound");
    assert_eq!(domain_of("never_heard_of_it"), "other");
}

/// The dependency itself (alpha.6 split F): the UI crates must not depend on `iyakki-agent` in `[dependencies]` (a dev-dependency, for their tests, and the
/// composition root `iyakki-compose` are fine). While a crate is listed here it still does; the list only shrinks, and the goal is that it is empty.
/// A crate that no longer depends on the agent must be taken off the list, so the dependency cannot come back.
const STILL_DEPENDING: [&str; 0] = [];

/// The names under `[dependencies]` of a manifest (not dev- or build-dependencies, not target tables).
fn normal_dependencies(manifest: &str) -> Vec<String> {
    let mut on = false;
    let mut out = Vec::new();
    for l in manifest.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            on = t == "[dependencies]";
            continue;
        }
        if on && !t.is_empty() && !t.starts_with('#') {
            if let Some(name) = t.split(['=', '.', ' ']).next() {
                out.push(name.to_string());
            }
        }
    }
    out
}

#[test]
fn the_ui_crates_do_not_depend_on_the_agent_and_the_list_of_exceptions_only_shrinks() {
    for krate in ["iyakki-apps", "iyakki-shell", "iyakki-uikit", "iyakki-ui-sdk"] {
        let m = std::fs::read_to_string(root().join("crates").join(krate).join("Cargo.toml")).unwrap();
        let depends = normal_dependencies(&m).iter().any(|d| d == "iyakki-agent");
        let listed = STILL_DEPENDING.contains(&krate);
        assert!(!depends || listed, "{krate} depends on iyakki-agent in [dependencies]: the UI crates reach the agent only through iyakki-agent-api and the handles iyakki-compose installs");
        assert!(depends || !listed, "{krate} no longer depends on iyakki-agent: take it off STILL_DEPENDING in crates/iyakki-core/tests/ui_agent_uses.rs so the dependency cannot come back");
    }
}

/// The ratchet bites: a manifest that puts `iyakki-agent` back under `[dependencies]` is seen (and so fails the test above), while a dev-dependency, a
/// build-dependency, a target table or the API crate `iyakki-agent-api` is not a dependency on the agent.
#[test]
fn the_dependency_scanner_sees_the_agent_when_it_is_added_back_and_only_then() {
    let back = "[package]\nname = \"iyakki-apps\"\n[dependencies]\niyakki-agent = { path = \"../iyakki-agent\" }\niyakki-agent-api.workspace = true\n";
    assert!(normal_dependencies(back).iter().any(|d| d == "iyakki-agent"), "a re-added dependency must be seen");
    let dotted = "[dependencies]\niyakki-agent.workspace = true\n";
    assert!(normal_dependencies(dotted).iter().any(|d| d == "iyakki-agent"));
    let fine = "[dependencies]\niyakki-agent-api.workspace = true\n[dev-dependencies]\niyakki-agent = { path = \"../iyakki-agent\" }\n[target.'cfg(unix)'.dependencies]\niyakki-agent = { path = \"../iyakki-agent\" }\n";
    assert!(!normal_dependencies(fine).iter().any(|d| d == "iyakki-agent"), "dev-dependencies, target tables and the API crate are fine");
    // and the real manifests: nothing is left on the exceptions list, so every UI crate is checked
    assert!(STILL_DEPENDING.is_empty(), "the list of exceptions is empty and must stay empty");
}

#[test]
fn only_the_composition_root_names_both_the_agent_and_the_ui() {
    // every workspace crate that depends on the agent AND on the shell or the apps is the composition root, a host program, or on this list (a ratchet: it shrinks)
    const BOTH: [&str; 4] = ["iyakki-compose", "iyakki-core", "iyakki-remote", "iyakki-gui"];
    let mut found = Vec::new();
    for e in std::fs::read_dir(root().join("crates")).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(m) = std::fs::read_to_string(e.path().join("Cargo.toml")) else { continue };
        let deps = normal_dependencies(&m);
        let agent = deps.iter().any(|d| d == "iyakki-agent");
        let ui = deps.iter().any(|d| d == "iyakki-shell" || d == "iyakki-apps");
        if agent && ui {
            found.push(name);
        }
    }
    found.sort();
    for f in &found {
        assert!(BOTH.contains(&f.as_str()), "{f} depends on both iyakki-agent and the UI: only the composition root (iyakki-compose) may; the others are listed in BOTH and may only shrink");
    }
}
