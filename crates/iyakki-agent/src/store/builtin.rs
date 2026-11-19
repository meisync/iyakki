//! The catalog that is part of the system image: what ships with IYAKKI, so the tile has content on the first boot
//! with no network and no stick. Items with a file (skills, the MCP demo) are packages built here from text compiled
//! into the binary; they are trusted because the image is. CLI entries are links to the CLI tile's own installer. The
//! "example paid apps" are made up, marked DEMO, and cannot be installed.

use std::sync::OnceLock;

use super::catalog::{sha256_hex, Action, Catalog, Entry};
use super::manifest::{Meta, PriceModel, Pricing, Publisher};
use crate::skills::{self, Scope};

pub const SKILLS: [(&str, &str); 21] = [
    ("k3s-dev-best-practices", include_str!("../../../../dist/skills/k3s-dev-best-practices.md")),
    ("lang-python", include_str!("../../../../dist/skills/lang-python.md")),
    ("lang-javascript", include_str!("../../../../dist/skills/lang-javascript.md")),
    ("lang-typescript", include_str!("../../../../dist/skills/lang-typescript.md")),
    ("lang-java", include_str!("../../../../dist/skills/lang-java.md")),
    ("lang-c", include_str!("../../../../dist/skills/lang-c.md")),
    ("lang-cpp", include_str!("../../../../dist/skills/lang-cpp.md")),
    ("lang-csharp", include_str!("../../../../dist/skills/lang-csharp.md")),
    ("lang-go", include_str!("../../../../dist/skills/lang-go.md")),
    ("lang-rust", include_str!("../../../../dist/skills/lang-rust.md")),
    ("lang-php", include_str!("../../../../dist/skills/lang-php.md")),
    ("iyakki-app-author", include_str!("../../../../dist/skills/iyakki-app-author.md")),
    ("iyakki-mcp-author", include_str!("../../../../dist/skills/iyakki-mcp-author.md")),
    ("iyakki-skill-author", include_str!("../../../../dist/skills/iyakki-skill-author.md")),
    ("iyakki-tile-author", include_str!("../../../../dist/skills/iyakki-tile-author.md")),
    ("k3s-build-and-deploy", include_str!("../../../../dist/skills/k3s-build-and-deploy.md")),
    ("k3s-dockerfile-patterns", include_str!("../../../../dist/skills/k3s-dockerfile-patterns.md")),
    ("k3s-node-webapp", include_str!("../../../../dist/skills/k3s-node-webapp.md")),
    ("k3s-web-service-patterns", include_str!("../../../../dist/skills/k3s-web-service-patterns.md")),
    ("iyakki-dev-mode", include_str!("../../../../dist/skills/iyakki-dev-mode.md")),
    ("iyakki-app-dev-loop", include_str!("../../../../dist/skills/iyakki-app-dev-loop.md")),
];

/// The three sample skills (the shell puts the same text in place on the first sign-in; a test keeps them equal).
pub const SAMPLE_SKILLS: [(&str, &str); 3] = [
    (
        "sample-explain-simply",
        "---\nname: sample-explain-simply\ndescription: Sample: explain a topic step by step in plain language with an example\ntriggers: explain, simply, eli5, what is\n---\nWhen asked to explain something, answer in short numbered steps, avoid jargon, define any term you must use, and finish with one small concrete example. Ask whether to go deeper.\n",
    ),
    (
        "sample-python-practice",
        "---\nname: sample-python-practice\ndescription: Sample: help someone practise Python by running their code in a container\ntriggers: python, practice, practise, run code, exercise\n---\nWhen someone wants to practise or test Python, write or take their program and run it with sandbox_run (language python), starting the container cluster first if it is not running. Show the output, explain any error in one or two sentences, fix it and run it again. Suggest the next small exercise.\n",
    ),
    (
        "sample-meeting-notes",
        "---\nname: sample-meeting-notes\ndescription: Sample: turn meeting notes into decisions and action items, and offer to schedule them\ntriggers: meeting, notes, minutes, action items\n---\nSummarise the notes as: Decisions, Action items (who and by when) and Open questions. Then offer to put each dated action item in the calendar with calendar_add.\n",
    ),
];

/// The sample tile apps: (name, summary, the files of the app). Packages built from text compiled into the binary, like the sample skills.
pub const SAMPLE_TILES: [(&str, &str, &[(&str, &str)]); 3] = [
    ("calculator", "A calculator that runs in its own tile. It asks for nothing.", &[
        ("app.json", include_str!("../../../../dist/samples/tiles/calculator/app.json")),
        ("index.html", include_str!("../../../../dist/samples/tiles/calculator/index.html")),
        ("style.css", include_str!("../../../../dist/samples/tiles/calculator/style.css")),
        ("app.js", include_str!("../../../../dist/samples/tiles/calculator/app.js")),
    ]),
    ("notes", "Notes and to-dos that stay on this device. It asks only to keep its own list.", &[
        ("app.json", include_str!("../../../../dist/samples/tiles/notes/app.json")),
        ("index.html", include_str!("../../../../dist/samples/tiles/notes/index.html")),
        ("style.css", include_str!("../../../../dist/samples/tiles/notes/style.css")),
        ("app.js", include_str!("../../../../dist/samples/tiles/notes/app.js")),
    ]),
    ("converter", "Length, weight and temperature converter. It asks for nothing.", &[
        ("app.json", include_str!("../../../../dist/samples/tiles/converter/app.json")),
        ("index.html", include_str!("../../../../dist/samples/tiles/converter/index.html")),
        ("style.css", include_str!("../../../../dist/samples/tiles/converter/style.css")),
        ("app.js", include_str!("../../../../dist/samples/tiles/converter/app.js")),
    ]),
];

/// The tests of the sample tile apps (not part of the packages; the test suite runs them on the real engine).
#[cfg(test)]
pub const SAMPLE_TILE_TESTS: [(&str, &str); 3] = [
    ("calculator", include_str!("../../../../dist/samples/tiles/calculator/tests/calculator.test.js")),
    ("notes", include_str!("../../../../dist/samples/tiles/notes/tests/notes.test.js")),
    ("converter", include_str!("../../../../dist/samples/tiles/converter/tests/converter.test.js")),
];

pub const MCP_DEMO_PAYLOAD: &str = r#"{"name":"iyakki-demo","command":"{self}","args":["--mcp-demo"]}"#;
pub const VERSION: &str = "1.0.0";
/// A made-up time for "added": the shipped items are all equally new.
const ADDED: i64 = 1_790_000_000;

pub struct Built {
    pub entry: Entry,
    /// The package text, for items that have a file.
    pub package: Option<String>,
}

pub fn iyakki() -> Publisher {
    Publisher { id: "iyakki".into(), name: "IYAKKI".into(), url: "https://github.com/iyakki".into() }
}

fn clip(s: &str, n: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(n).collect();
    t.trim().to_string()
}

fn base_entry(name: &str, kind: &str, meta: Meta) -> Entry {
    Entry { id: name.into(), kind: kind.into(), name: name.into(), version: VERSION.into(), license: "Apache-2.0".into(), added: ADDED, builtin: name.into(), meta, ..Default::default() }
}

fn skill_item(name: &str, md: &str, categories: &[&str], featured: bool) -> Built {
    let sk = skills::parse(md, Scope::Shared).expect("a shipped skill parses");
    let pretty = if let Some(l) = name.strip_prefix("lang-") {
        format!("Coding in {}", match l { "cpp" => "C++".to_string(), "csharp" => "C#".to_string(), "php" => "PHP".to_string(), "c" => "C".to_string(), other => format!("{}{}", other[..1].to_uppercase(), &other[1..]) })
    } else if name == "k3s-dev-best-practices" {
        "k3s development practices".to_string()
    } else if name == "k3s-build-and-deploy" {
        "Build and deploy to k3s".to_string()
    } else if name == "k3s-dockerfile-patterns" {
        "Dockerfile patterns for k3s".to_string()
    } else if name == "k3s-node-webapp" {
        "Node.js web apps on k3s".to_string()
    } else if name == "k3s-web-service-patterns" {
        "Web services on k3s (any language)".to_string()
    } else if name == "iyakki-dev-mode" {
        "Developer mode: work on a device".to_string()
    } else if name == "iyakki-app-dev-loop" {
        "Build an IYAKKI app: the dev loop".to_string()
    } else {
        name.trim_start_matches("sample-").replace('-', " ")
    };
    let meta = Meta {
        title: clip(&pretty, 60),
        summary: clip(&sk.description, 140),
        description: format!("{}\n\nThis skill is text the assistant reads when a request matches ({}). It runs nothing and asks for no access. Remove it if you do not need it; you can install it again from here.", sk.description, if sk.triggers.is_empty() { "no trigger words".into() } else { format!("trigger words: {}", sk.triggers.join(", ")) }),
        publisher: iyakki(),
        categories: categories.iter().map(|s| s.to_string()).collect(),
        tags: sk.triggers.iter().take(6).map(|t| clip(t, 24)).collect(),
        ..Default::default()
    };
    let payload = skills::render(&sk);
    let package = crate::pkg::build_generic("skill", name, VERSION, "Apache-2.0", &payload, Some(&meta.to_json()), None, None).expect("a shipped skill packs");
    let mut e = base_entry(name, "skill", meta);
    e.size = package.len() as u64;
    e.sha256 = sha256_hex(package.as_bytes());
    e.featured = featured;
    Built { entry: e, package: Some(package) }
}

fn build_all() -> Vec<Built> {
    let mut v = Vec::new();
    for (name, md) in SKILLS {
        let cats: &[&str] = if name.starts_with("lang-") { &["languages", "skills"] } else if name.starts_with("iyakki-") { &["authoring", "skills"] } else { &["devops", "skills"] };
        v.push(skill_item(name, md, cats, matches!(name, "k3s-dev-best-practices" | "lang-python" | "lang-rust" | "iyakki-app-author")));
    }
    for (name, md) in SAMPLE_SKILLS {
        v.push(skill_item(name, md, &["samples", "skills"], false));
    }
    // the sample tile apps
    for (name, summary, files) in SAMPLE_TILES {
        let bundle = super::tile::Bundle::from_files(files, name).expect("a shipped tile app is valid");
        let title = bundle.manifest.name.clone();
        let meta = Meta {
            title: clip(&title, 60),
            summary: clip(summary, 140),
            description: format!("{summary}\n\nA sample custom app for the launcher: plain HTML, CSS and JavaScript on IYAKKI's own browser engine, in its own private page. Installing it adds a tile under “My apps”; removing it takes the tile away. Its source is a good start for your own (see docs/APP-AUTHORING.md)."),
            publisher: iyakki(),
            categories: vec!["samples".into(), "apps".into()],
            permissions: bundle.manifest.caps.permissions(),
            ..Default::default()
        };
        let package = crate::pkg::build_generic("tile", name, VERSION, "Apache-2.0", &bundle.to_payload(), Some(&meta.to_json()), None, None).expect("a shipped tile app packs");
        let mut e = base_entry(name, "tile", meta);
        e.size = package.len() as u64;
        e.sha256 = sha256_hex(package.as_bytes());
        v.push(Built { entry: e, package: Some(package) });
    }
    // the sample MCP server
    let meta = Meta {
        title: "MCP demo server".into(),
        summary: "A tiny tool server with echo, add, now and dice, to try the MCP tile.".into(),
        description: "A sample Model Context Protocol server that is part of IYAKKI. It offers four harmless tools (echo, add, now, dice) so you can see how tools from a server appear in the chat. It starts IYAKKI itself with `--mcp-demo` and reaches nothing else.".into(),
        publisher: iyakki(),
        categories: vec!["samples".into(), "tools".into()],
        permissions: vec!["run:{self}".into()],
        ..Default::default()
    };
    let package = crate::pkg::build_generic("mcp", "iyakki-demo", VERSION, "Apache-2.0", MCP_DEMO_PAYLOAD, Some(&meta.to_json()), None, None).expect("the demo server packs");
    let mut e = base_entry("iyakki-demo", "mcp", meta);
    e.size = package.len() as u64;
    e.sha256 = sha256_hex(package.as_bytes());
    v.push(Built { entry: e, package: Some(package) });
    // command-line tools: links to the CLI tile's installer
    for d in crate::clis::CATALOG {
        let id = format!("cli-{}", d.id.replace('_', "-"));
        if !skills::valid_skill_name(&id) {
            continue;
        }
        let meta = Meta {
            title: clip(d.name, 60),
            summary: clip(d.about, 140),
            description: if matches!(d.source, crate::clis::Source::System) { format!("{}\n\nNothing is downloaded: this comes with the system. Opening it shows what the CLI tile found on this machine and how to get it if it is missing. Home page: {}", d.about, d.home) } else { format!("{}\n\nInstalling opens the CLI tile, which downloads the program from its maker ({}), checks it, and keeps it in IYAKKI's own folder. Home page: {}", d.about, d.hosts.join(", "), d.home) }
                + &cli_licence_text(d),
            publisher: iyakki(),
            categories: vec!["tools".into()],
            permissions: if matches!(d.source, crate::clis::Source::System) { vec![] } else { d.hosts.iter().take(4).map(|h| format!("net:{h}")).collect() },
            source_url: if d.home.starts_with("http") { d.home.to_string() } else { String::new() },
            ..Default::default()
        };
        let mut e = base_entry(&id, "cli", meta);
        e.license = d.license.to_string();
        e.builtin = String::new();
        e.action = Some(Action { open: "cli".into(), id: d.id.into() });
        e.added = ADDED - 86_400 * 30;
        e.featured = matches!(d.id, "kubectl" | "claude-code");
        v.push(Built { entry: e, package: None });
    }
    // driver packs for bare-metal Macs (metadata and instructions; the keyboard-options pack is a real package)
    v.extend(super::drivers::catalog_items());
    // made-up paid apps, to try the price and the disabled Buy button
    for (name, title, kind, model, price, summary) in [
        ("demo-neon-pro-themes", "Neon Pro theme pack", "theme", PriceModel::Paid, 499, "Twelve extra colour themes for every tile."),
        ("demo-meeting-suite", "Meeting suite", "tool", PriceModel::Subscription, 300, "Record, transcribe and summarise meetings."),
        ("demo-voice-packs", "Studio voice packs", "agent", PriceModel::Paid, 1299, "Extra voices for the assistant."),
    ] {
        let meta = Meta {
            title: title.into(),
            summary: format!("DEMO: not a real product. {summary}"),
            description: "This entry is made up to show how a priced app looks. It has no file, cannot be installed, and nothing is sold here. Payments are not part of this version of IYAKKI.".into(),
            publisher: Publisher { id: "example-studio".into(), name: "Example Studio (DEMO)".into(), url: String::new() },
            pricing: Pricing { model, price, currency: "USD".into(), ..Default::default() },
            categories: vec!["demo".into()],
            ..Default::default()
        };
        let mut e = base_entry(name, kind, meta);
        e.builtin = String::new();
        e.demo = true;
        e.license = "Proprietary".into();
        v.push(Built { entry: e, package: None });
    }
    v
}

pub fn items() -> &'static [Built] {
    static ITEMS: OnceLock<Vec<Built>> = OnceLock::new();
    ITEMS.get_or_init(build_all)
}

/// The package text of a built-in item.
pub fn package(key: &str) -> Option<&'static str> {
    items().iter().find(|b| b.entry.builtin == key).and_then(|b| b.package.as_deref())
}

pub fn catalog() -> Catalog {
    Catalog { v: 1, id: "builtin".into(), name: "IYAKKI built-in".into(), serial: 1, issued: ADDED, expires: 0, entries: items().iter().map(|b| b.entry.clone()).collect(), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_k3s_build_and_deploy_skills_fit_reference_each_other_and_carry_the_cluster_rules() {
        let get = |n: &str| SKILLS.iter().find(|(k, _)| *k == n).unwrap_or_else(|| panic!("{n} ships")).1;
        let (bd, df) = (get("k3s-build-and-deploy"), get("k3s-dockerfile-patterns"));
        for md in [bd, df] {
            let sk = skills::parse(md, Scope::Shared).expect("parses");
            assert!(sk.body.chars().count() <= 3950 && md.is_ascii() && sk.triggers.len() <= 12);
            assert!(!md.contains("/home/") && !md.contains("192.168.") && !md.contains(":latest\n"));
        }
        assert!(bd.contains("k3s-dockerfile-patterns") && df.contains("k3s-build-and-deploy") && bd.contains("k3s-dev-best-practices"));
        for w in ["k8s_status", "deploy_build target=test", "deploy_load", "k8s_apply", "deploy_status", "Never switch the active cluster", "read-only", "ImagePullBackOff", "OOMKilled", "store_publish", "nvidia.com/gpu", "HTTP 200"] {
            assert!(bd.contains(w), "{w}");
        }
        for l in ["Node", "Python", "Go", "Rust", "Java", "C#", "PHP", "Static"] {
            assert!(df.contains(l), "{l}");
        }
    }

    #[test]
    fn the_node_and_web_service_skills_fit_reference_each_other_and_carry_the_no_builder_rule() {
        let get = |n: &str| SKILLS.iter().find(|(k, _)| *k == n).unwrap_or_else(|| panic!("{n} ships")).1;
        let (node, web) = (get("k3s-node-webapp"), get("k3s-web-service-patterns"));
        for md in [node, web] {
            let sk = skills::parse(md, Scope::Shared).expect("parses");
            assert!(sk.body.chars().count() <= 3950 && md.is_ascii() && sk.triggers.len() <= 12, "{}", sk.name);
            assert!(!md.contains("/home/") && !md.contains("192.168.") && !md.contains(":latest\n"));
        }
        assert!(node.contains("k3s-web-service-patterns") && node.contains("k3s-build-and-deploy") && node.contains("k3s-dockerfile-patterns"));
        assert!(web.contains("k3s-node-webapp") && web.contains("k3s-build-and-deploy"));
        for w in ["/healthz", "SIGTERM", "node --test", "vault_request", "deploy_status", "process.env", "ConfigMap", "podman"] {
            assert!(node.contains(w) || web.contains(w), "{w}");
        }
        // when deploy_build says there is no image builder the assistant must not apply an image that was never built
        assert!(web.contains("neither podman nor docker is installed") && web.contains("ImagePullBackOff") && web.contains("ConfigMap"));
        assert!(node.contains("No podman or docker"));
    }

    #[test]
    fn the_developer_mode_skills_are_complete_short_and_name_the_commands_they_teach() {
        let get = |n: &str| SKILLS.iter().find(|(name, _)| *name == n).map(|(_, t)| *t).unwrap_or_else(|| panic!("{n} is shipped"));
        let (dev, lp) = (get("iyakki-dev-mode"), get("iyakki-app-dev-loop"));
        for (n, t) in [("iyakki-dev-mode", dev), ("iyakki-app-dev-loop", lp)] {
            let body = t.splitn(3, "---\n").nth(2).unwrap();
            assert!(body.len() <= 3950, "{n}: {} chars", body.len());
            assert!(t.is_ascii(), "{n} is ASCII");
            assert!(t.contains("B- id") && t.contains("bug_report") && t.contains("never F-"), "{n} carries the board rule");
            assert!(crate::skills::parse(t, crate::skills::Scope::Shared).is_ok(), "{n} parses");
        }
        for word in ["iyakki dev pair", "screenshot -o", "patch apply", "update-pack", "NEVER reboot", "input tap X Y", "shell --minutes"] {
            assert!(dev.contains(word), "dev mode skill names `{word}`");
        }
        for word in ["iyakki sdk init", "dev push . --watch --open", "pkg validate", "pkg pack", "iyakki-dev-mode", "iyakki-tile-author"] {
            assert!(lp.contains(word), "app loop skill names `{word}`");
        }
    }

    #[test]
    fn the_shipped_skills_the_samples_the_demo_server_and_the_clis_are_all_there() {
        let c = catalog();
        for (n, _) in SKILLS.iter().chain(SAMPLE_SKILLS.iter()) {
            let e = c.find(n).unwrap_or_else(|| panic!("{n} is listed"));
            assert_eq!(e.kind, "skill");
            assert!(e.meta.publisher.id == "iyakki" && e.size > 0 && e.sha256.len() == 64);
        }
        assert_eq!(c.find("iyakki-demo").unwrap().kind, "mcp");
        assert!(c.entries.iter().filter(|e| e.kind == "cli").count() >= 8);
        let k = c.find("cli-kubectl").expect("kubectl is a Tools item");
        assert_eq!(k.action.as_ref().unwrap(), &Action { open: "cli".into(), id: "kubectl".into() });
        assert!(k.url.is_empty() && k.builtin.is_empty());
        let demos: Vec<&Entry> = c.entries.iter().filter(|e| e.demo).collect();
        assert_eq!(demos.len(), 3);
        assert!(demos.iter().all(|e| e.meta.title.len() > 3 && e.meta.summary.starts_with("DEMO") && e.meta.pricing.model.needs_licence() && e.meta.pricing.price > 0 && e.url.is_empty() && e.action.is_none()));
        assert!(c.entries.iter().any(|e| e.featured));
    }

    #[test]
    fn every_builtin_package_is_a_real_package_that_matches_its_entry_and_every_entry_validates() {
        for b in items() {
            if let Some(text) = &b.package {
                assert_eq!(sha256_hex(text.as_bytes()), b.entry.sha256, "{}", b.entry.id);
                let p = crate::pkg::parse(text).unwrap_or_else(|e| panic!("{}: {e}", b.entry.id));
                assert_eq!((p.manifest.kind.as_str(), p.manifest.name.as_str()), (b.entry.kind.as_str(), b.entry.name.as_str()));
                assert!(p.manifest.signer.is_none(), "trusted because the image is, not by a signature");
                let m = Meta::parse(p.manifest.meta.as_deref().unwrap()).unwrap();
                assert_eq!(m, b.entry.meta, "{}: the package and the catalog say the same", b.entry.id);
            }
            assert!(super::super::catalog::Catalog::parse(&{
                let mut c = Catalog { v: 1, id: "builtin".into(), name: "x".into(), entries: vec![b.entry.clone()], ..Default::default() };
                c.sig.clear();
                c.to_json()
            }, &super::super::trust::Roots::default(), &[], 0, false).unwrap().0.entries.len() == 1, "{} validates", b.entry.id);
        }
    }
}


/// What the store screen says about a command-line tool's licence and delivery: the licence, that IYAKKI never ships the program (it is
/// downloaded from its maker at the person's click), a plain warning for source-available and proprietary ones, and what is (not) verified.
fn cli_licence_text(d: &crate::clis::CliDef) -> String {
    let mut t = format!("\n\nLicence: {}.", d.license);
    if !matches!(d.source, crate::clis::Source::System) {
        t.push_str(" IYAKKI does not ship this program: it is downloaded from its maker at your request and kept in IYAKKI's own folder.");
    }
    if d.license.starts_with("Proprietary") {
        t.push_str(" It is proprietary software under the vendor's terms: downloaded, not shipped, and not open source.");
    } else if d.license.contains("not open source") {
        t.push_str(" It is source-available, not open source: you download it yourself, under the vendor's terms, and IYAKKI does not ship it.");
    }
    if let Some(w) = d.integrity_warning() {
        t.push_str("\n\n");
        t.push_str(&w);
    }
    t
}
