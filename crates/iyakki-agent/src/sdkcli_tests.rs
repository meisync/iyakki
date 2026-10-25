//! `iyakki sdk`: init for each kind, the refusals, the typings file, doctor with no device / an unreachable one / a working one, docs.

use std::path::PathBuf;

use serde_json::json;

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-sdkcli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn dev(name: &str) -> Device {
    Device { name: name.into(), host: "192.0.2.7".into(), port: 7979, fingerprint: "ab:cd".into(), account: "admin".into(), paired_at: 0 }
}

#[test]
fn each_kind_makes_its_files_and_the_agent_files() {
    let cases: [(&str, &[&str]); 4] = [
        ("tile", &["index.html", "app.js", "app.json", "iyakki-app.json", "tests/app.test.js"]),
        ("skill", &["skill.md", "iyakki-app.json", "tests/test_skill.py"]),
        ("mcp", &["server.py", "iyakki-app.json", "tests/test_server.py"]),
        ("service", &["server.py", "Dockerfile", ".dockerignore", "deploy/app.yaml", "tests/test_server.py", "README.md"]),
    ];
    for (kind, files) in cases {
        let cwd = tmp(kind);
        let msg = init("meeting-notes", kind, None, &cwd).unwrap();
        let p = cwd.join("meeting-notes");
        for f in files.iter().chain(["CLAUDE.md", "AGENTS.md", "iyakki.d.ts", ".gitignore"].iter()) {
            assert!(p.join(f).is_file(), "{kind}: missing {f}");
        }
        assert!(msg.contains("Next:"), "{msg}");
        for md in ["CLAUDE.md", "AGENTS.md"] {
            let t = std::fs::read_to_string(p.join(md)).unwrap();
            assert!(t.contains("meeting-notes") && !t.contains("__") && !t.contains("<!--"), "{kind} {md}: {t}");
            for must in ["iyakki dev logs", "iyakki dev screenshot", "iyakki pkg validate", "ask the person", "Permissions"] {
                assert!(t.contains(must), "{kind} {md} lacks {must}");
            }
            assert!(t.contains("iyakki dev push . --watch"), "{kind} {md}");
        }
        // only the section of this kind is kept
        let t = std::fs::read_to_string(p.join("CLAUDE.md")).unwrap();
        assert_eq!(t.contains("This is a tile app"), kind == "tile");
        assert_eq!(t.contains("This is a service for k3s"), kind == "service");
        assert_eq!(t.contains("tile_test"), kind == "tile", "tile_test is named for tile apps");
        if kind == "service" {
            assert!(!std::fs::read_to_string(p.join("deploy/app.yaml")).unwrap().contains("__NAME__"));
        }
    }
}

#[test]
fn tile_instructions_name_tile_test() {
    let cwd = tmp("tilet");
    init("clock", "tile", None, &cwd).unwrap();
    assert!(std::fs::read_to_string(cwd.join("clock/AGENTS.md")).unwrap().contains("tile_test"));
}

#[test]
fn it_refuses_a_folder_with_files_a_bad_name_and_an_unknown_kind_and_writes_nothing() {
    let cwd = tmp("refuse");
    std::fs::create_dir_all(cwd.join("busy")).unwrap();
    std::fs::write(cwd.join("busy/mine.txt"), "keep").unwrap();
    for kind in KINDS {
        let e = init("busy", kind, None, &cwd).unwrap_err();
        assert!(e.contains("already has files"), "{kind}: {e}");
    }
    assert_eq!(std::fs::read_dir(cwd.join("busy")).unwrap().count(), 1);
    assert_eq!(std::fs::read_to_string(cwd.join("busy/mine.txt")).unwrap(), "keep");
    assert!(init("Bad Name", "tile", None, &cwd).unwrap_err().contains("lowercase"));
    assert!(init("ok", "game", None, &cwd).unwrap_err().contains("unknown kind"));
    assert!(!cwd.join("ok").exists() && !cwd.join("Bad Name").exists());
    // an existing but empty folder is fine, and --dir chooses the place
    std::fs::create_dir_all(cwd.join("empty")).unwrap();
    init("ok", "skill", Some("empty"), &cwd).unwrap();
    assert!(cwd.join("empty/skill.md").is_file());
    init("deep", "tile", Some("a/b/c"), &cwd).unwrap();
    assert!(cwd.join("a/b/c/index.html").is_file());
}

#[test]
fn gitignore_covers_keys_and_build_output() {
    let cwd = tmp("gi");
    init("x1", "tile", None, &cwd).unwrap();
    let g = std::fs::read_to_string(cwd.join("x1/.gitignore")).unwrap();
    for w in ["*.key", "*.iyp", "build/"] {
        assert!(g.lines().any(|l| l == w), "{w}");
    }
}

#[test]
fn the_typings_are_there_and_cover_every_namespace_of_the_bridge() {
    let cwd = tmp("dts");
    init("x2", "tile", None, &cwd).unwrap();
    let t = std::fs::read_to_string(cwd.join("x2/iyakki.d.ts")).unwrap();
    assert_eq!(t, D_TS);
    for w in ["declare const iyakki", "interface Window", "info(): AppInfo", "openFile()", "saveFile(", "ask(question: string)", "show(title: string, body: string)", "toast(text: string)"] {
        assert!(t.contains(w), "{w}");
    }
    for ns in ["app", "storage", "ui", "host", "clipboard", "notify", "agent"] {
        assert!(t.contains(&format!("readonly {ns}: ")) && t.contains(&format!("// ---- {ns} ----")), "{ns}");
    }
    for cap in ["storage", "files", "clipboard", "notifications", "agent"] {
        assert!(t.contains(&format!("\"{cap}\"")), "{cap}");
    }
    // braces are balanced (a cheap parse check)
    assert_eq!(t.matches('{').count(), t.matches('}').count());
    assert_eq!(t.matches('(').count(), t.matches(')').count());
}

#[test]
fn the_templates_and_the_mcp_note_are_marked() {
    let m = include_str!("../../../dist/sdk/templates/.mcp.json");
    assert!(m.contains("WHEN AVAILABLE") && m.contains("NOT BUILT"));
    assert!(serde_json::from_str::<Value>(m).is_ok());
    assert!(include_str!("../../../dist/sdk/EMULATOR.md").contains("NOT BUILT"));
}

#[test]
fn doctor_with_no_device_is_plain_and_does_not_probe() {
    let dir = tmp("doc0");
    let out = doctor(&Home::at(&dir), &|_| panic!("no device to ask"));
    assert!(out.contains("iyakki ") && out.contains("Paired device: none"), "{out}");
    assert!(out.contains("Next step") && out.contains("iyakki dev pair"), "{out}");
    assert!(out.contains("iyakki pkg validate"), "still useful offline: {out}");
}

#[test]
fn doctor_with_a_paired_but_unreachable_device_says_what_to_check() {
    let dir = tmp("doc1");
    let h = Home::at(&dir);
    h.add(dev("mini")).unwrap();
    let out = doctor(&h, &|_| Err("cannot reach 192.0.2.7:7979: timed out".into()));
    assert!(out.contains("Paired device: mini (default)") && out.contains("192.0.2.7:7979"), "{out}");
    assert!(out.contains("mini does not answer: cannot reach") && out.contains("Next step") && out.contains("Developer mode"), "{out}");
}

#[test]
fn doctor_with_an_answering_device_reports_scopes_and_the_next_step() {
    let dir = tmp("doc2");
    let h = Home::at(&dir);
    h.add(dev("mini")).unwrap();
    let ok = |scopes: serde_json::Value| move |_: &Device| Ok(json!({"computer": "laptop", "role": "admin", "status": "on", "scopes": scopes}));
    let out = doctor(&h, &ok(json!(["install", "debug"])));
    assert!(out.contains("mini answers") && out.contains("Scopes held: install, debug") && out.contains("iyakki dev push . --watch"), "{out}");
    let out = doctor(&h, &ok(json!([])));
    assert!(out.contains("Scopes held: none") && out.contains("scope `install`"), "{out}");
}

#[test]
fn docs_lists_the_guides_and_every_path_exists() {
    let all = docs(None).unwrap();
    for t in DOCS {
        assert!(all.contains(t.1), "{}", t.1);
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for t in DOCS {
        assert!(root.join(t.1).is_file(), "{} is listed but missing", t.1);
    }
    assert!(docs(Some("tile")).unwrap().contains("docs/APP-AUTHORING.md"));
    let e = docs(Some("nope")).unwrap_err();
    assert!(e.contains("topics:") && e.contains("emulator"), "{e}");
}

#[test]
fn run_dispatches_and_the_emulator_says_it_is_not_built() {
    let mut lines = Vec::new();
    run(&[], &mut |s| lines.push(s.to_string())).unwrap();
    assert!(lines[0].contains("init <name>") && lines[0].contains("doctor") && lines[0].contains("docs"));
    run(&["docs".into()], &mut |s| lines.push(s.to_string())).unwrap();
    assert!(lines[1].contains("Guides"));
    assert!(run(&["emulator".into(), "start".into()], &mut |_| {}).unwrap_err().contains("not built"));
    assert!(run(&["init".into()], &mut |_| {}).unwrap_err().contains("usage"));
    assert!(run(&["nope".into()], &mut |_| {}).is_err());
    assert_eq!(positional(&["init".into(), "a".into(), "--kind".into(), "mcp".into(), "--dir".into(), "d".into()]), ["init", "a"]);
}
